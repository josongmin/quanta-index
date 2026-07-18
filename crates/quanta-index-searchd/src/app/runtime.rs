//! Composed runtime artefacts. Built once per daemon process.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::sync::{Arc, RwLock};
use std::{
    fs,
    path::{Path, PathBuf},
};

use super::LegacySemanticJournalStore;
use anyhow::Result;
use fs2::FileExt;
use memchr::memchr_iter;
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, GenerationSelector, LqFileScope, LqStructuralBlock,
    ManifestGeneration, SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SearchPlaneTrackKind,
};
use quanta_index_core::domains::structural::{
    StructuralExecutableFilter, StructuralProducerPort,
    StructuralQueryRequest as DomainStructuralQueryRequest,
};
use quanta_index_core::{
    CoreError, FileContributorIngestPort, FileOwnershipIngestPort, GenerationIdentityValidatePort,
    IncompleteGenerationDiscardPort, LexicalIndexOpenPort, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMapBundleIngestPort, RepoMapGenerationActivatePort,
    RepoMapQueryPort, RepoMetaIngestPort, RepoTopicIngestPort, SealedGenerationScanPort,
    SearchCorpusBatchBuildPort, SearchCorpusIngestPort, SemanticBatchBuildPort,
    SemanticIndexOpenPort, SemanticIngestPort, StructuralError, StructuralMatchBinding,
    StructuralMatchCandidate, StructuralReadiness, TextEmbeddingProvider,
};
use quanta_index_embed::{CachingEmbeddingProvider, FileEmbeddingCache, OpenAiEmbeddingProvider};
use quanta_index_ipc::IpcDispatcher;
use quanta_index_lq_structural::{
    StructuralAuthorityCandidate as LqStructuralAuthorityCandidate, StructuralAuthorityMatcher,
    StructuralAuthorityPatternError, StructuralAuthorityPatternRef, StructuralAuthorityView,
    StructuralError as LqStructuralError, StructuralErrorCode as LqStructuralErrorCode,
    StructuralPattern as LqStructuralPattern, TruthfulSubsetAuthorityMatcher,
    compile_authoritative_pattern,
};
use quanta_index_search_plane::{
    ActivationCatalog, AuxiliaryAuthorityStore, BoundedQueryObsStore, DirectHistoryMaterializer,
    DirectRuntimeMetadataMaterializer, DirectSearchCorpusMaterializer, DirectSemanticMaterializer,
    DirectStructuralMaterializer, HashingQueryTextEmbedder, HistoryIngestPort, Ledger,
    QueryObsSink, QueryTextEmbedderPort, RuntimeMetadataIngestPort,
    SEARCH_OWNED_SEMANTIC_DIMENSION, SearchCorpusMaterializerParts, SearchPlaneControlDispatcher,
    SearchPlaneDispatcher, SearchPlaneIngestDispatcher, StructuralIngestPort,
};
use regex::Regex;

use crate::app::config::{SearchdConfig, SemanticEmbedderProfile};
use crate::app::ipc_dispatcher::{
    SearchPlaneControlIpcAdapter, SearchPlaneIngestIpcAdapter, SearchPlaneQueryIpcAdapter,
};
use crate::app::semantic_boot;
use crate::app::server::{
    SearchPlaneControlServer, SearchPlaneIngestServer, SearchPlaneQueryServer,
};

const BENCH_DISABLE_QUERY_OBS_ENV: &str = "QUANTA_INDEX_BENCH_DISABLE_QUERY_OBS";

pub struct SearchdRuntimeParts {
    pub state_root_lease: StateRootLease,
    pub search_corpus_build_port: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    pub lexical_generation_scanner: Arc<dyn SealedGenerationScanPort + Send + Sync>,
    pub lexical_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    pub lexical_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>,
    pub lex_open_port: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    pub repo_commit_recency_ingest_port: Arc<dyn RepoCommitRecencyIngestPort + Send + Sync>,
    pub repo_topic_ingest_port: Arc<dyn RepoTopicIngestPort + Send + Sync>,
    pub repo_description_ingest_port: Arc<dyn RepoDescriptionIngestPort + Send + Sync>,
    pub file_ownership_ingest_port: Arc<dyn FileOwnershipIngestPort + Send + Sync>,
    pub file_contributor_ingest_port: Arc<dyn FileContributorIngestPort + Send + Sync>,
    pub repo_meta_ingest_port: Arc<dyn RepoMetaIngestPort + Send + Sync>,
    pub sem_build_port: Arc<dyn SemanticBatchBuildPort + Send + Sync>,
    pub semantic_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    pub semantic_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>,
    pub sem_open_port: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    pub repo_map_query_port: Arc<dyn RepoMapQueryPort + Send + Sync>,
    pub repo_map_bundle_ingest_port: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    pub repo_map_generation_activate_port: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
    pub activation_catalog: Arc<ActivationCatalog>,
    pub aux_authority_store: Arc<AuxiliaryAuthorityStore>,
    pub legacy_semantic_journal_store: Arc<LegacySemanticJournalStore>,
}

/// Exclusive process-lifetime ownership of one daemon state root.
///
/// The lock file is intentionally persistent; the OS lock, not file presence,
/// owns liveness. Dropping the file handle releases the lease after crashes and
/// normal shutdown without stale-file recovery heuristics.
#[derive(Debug)]
pub struct StateRootLease {
    _file: File,
}

impl StateRootLease {
    pub fn acquire(state_root: &Path) -> Result<Self, CoreError> {
        ensure_durable_state_root_v1(state_root)?;
        let path = state_root.join(".searchd-state-root.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                CoreError::Storage(format!(
                    "searchd state-root lease: open {}: {error}",
                    path.display()
                ))
            })?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self { _file: file }),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Err(CoreError::Typed {
                code: "STATE_ROOT_IN_USE".to_string(),
                message: format!(
                    "searchd state root already has a live owner: {}",
                    state_root.display()
                ),
            }),
            Err(error) => Err(CoreError::Storage(format!(
                "searchd state-root lease: lock {}: {error}",
                path.display()
            ))),
        }
    }
}

fn ensure_durable_state_root_v1(state_root: &Path) -> Result<(), CoreError> {
    ensure_durable_state_root_with_v1(state_root, &|parent| {
        File::open(parent).and_then(|directory| directory.sync_all())
    })
}

fn ensure_durable_state_root_with_v1(
    state_root: &Path,
    sync_parent: &dyn Fn(&Path) -> std::io::Result<()>,
) -> Result<(), CoreError> {
    if state_root.is_dir() {
        return Ok(());
    }

    let mut missing: Vec<PathBuf> = Vec::new();
    let mut cursor = state_root;
    while !cursor.exists() {
        missing.push(cursor.to_path_buf());
        cursor = cursor.parent().ok_or_else(|| {
            CoreError::Storage(format!(
                "searchd state-root lease: no existing ancestor for {}",
                state_root.display()
            ))
        })?;
    }
    if !cursor.is_dir() {
        return Err(CoreError::Storage(format!(
            "searchd state-root lease: ancestor is not a directory: {}",
            cursor.display()
        )));
    }

    for directory in missing.iter().rev() {
        match fs::create_dir(directory) {
            Ok(()) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists && directory.is_dir() => {}
            Err(error) => {
                return Err(CoreError::Storage(format!(
                    "searchd state-root lease: create {}: {error}",
                    directory.display()
                )));
            }
        }
        let parent = directory.parent().ok_or_else(|| {
            CoreError::Storage(format!(
                "searchd state-root lease: created directory has no parent: {}",
                directory.display()
            ))
        })?;
        sync_parent(parent).map_err(|error| {
            CoreError::Storage(format!(
                "searchd state-root lease: fsync parent {} after creating {}: {error}",
                parent.display(),
                directory.display()
            ))
        })?;
    }
    Ok(())
}

struct ProviderUnavailableQueryTextEmbedder;

impl QueryTextEmbedderPort for ProviderUnavailableQueryTextEmbedder {
    fn embed_query(&self, _query_text: &str) -> Result<Vec<f32>, quanta_index_core::CoreError> {
        Err(quanta_index_core::CoreError::Typed {
            code: LexicalErrorCode::SemProviderUnavailable
                .as_code_str()
                .to_string(),
            message: "query-time embedder is not configured for this runtime".to_string(),
        })
    }

    fn model_id(&self) -> &str {
        // Sentinel only: this embedder always fails `embed_query` before any
        // model-identity comparison runs, so this value is never used to decide
        // a query. It must not collide with a real model id.
        "provider-unavailable"
    }

    fn model_version(&self) -> Option<&str> {
        None
    }
}

struct NoopQueryObsSink;

impl QueryObsSink for NoopQueryObsSink {
    fn emit(&self, _sample: quanta_index_search_plane::MetricSample) {}
}

/// Adapts the batch-capable [`TextEmbeddingProvider`] to the query-side
/// [`QueryTextEmbedderPort`] (single-text embed), so one provider instance serves
/// both the query and corpus paths and they cannot diverge on model identity.
struct QueryEmbedderAdapter(Arc<dyn TextEmbeddingProvider + Send + Sync>);

impl QueryTextEmbedderPort for QueryEmbedderAdapter {
    fn embed_query(&self, query_text: &str) -> Result<Vec<f32>, CoreError> {
        self.0
            .embed_batch(&[query_text])?
            .into_iter()
            .next()
            .ok_or_else(|| {
                CoreError::Storage("embedder returned no vector for query text".to_string())
            })
    }

    fn model_id(&self) -> &str {
        self.0.model_id()
    }

    fn model_version(&self) -> Option<&str> {
        self.0.model_version()
    }
}

/// Resolve the (query embedder, corpus embedder) pair for a profile. Both are
/// backed by the SAME provider for `Hash`/`OpenAi` so model identity matches; for
/// `Unavailable` the query embedder fails closed while the corpus still
/// hash-derives (the deliberate degraded-config contract).
fn build_semantic_embedders(
    profile: &SemanticEmbedderProfile,
    state_root: &Path,
) -> Result<
    (
        Arc<dyn QueryTextEmbedderPort + Send + Sync>,
        Arc<dyn TextEmbeddingProvider + Send + Sync>,
    ),
    CoreError,
> {
    match profile {
        SemanticEmbedderProfile::Hash { dimension } => {
            let provider: Arc<dyn TextEmbeddingProvider + Send + Sync> =
                Arc::new(HashingQueryTextEmbedder::new(*dimension));
            Ok((
                Arc::new(QueryEmbedderAdapter(Arc::clone(&provider))),
                provider,
            ))
        }
        SemanticEmbedderProfile::OpenAi {
            model,
            dimension,
            api_key,
            tuning,
        } => {
            // Thread the env-resolved operational knobs into the provider config
            // (mapping owned + unit-tested on OpenAiEmbedderTuning::provider_config).
            let openai = OpenAiEmbeddingProvider::with_reqwest(tuning.provider_config(
                model.clone(),
                *dimension,
                api_key.clone(),
            ))?;
            let provider: Arc<dyn TextEmbeddingProvider + Send + Sync> = if tuning.cache_enabled {
                // Persistent content-hash cache (model+dim scoped) so rebuilds /
                // incrementals avoid paid re-embedding of unchanged chunks.
                let cache = FileEmbeddingCache::new(state_root.join("embed-cache"))?;
                Arc::new(CachingEmbeddingProvider::new(
                    Box::new(openai),
                    Box::new(cache),
                ))
            } else {
                Arc::new(openai)
            };
            Ok((
                Arc::new(QueryEmbedderAdapter(Arc::clone(&provider))),
                provider,
            ))
        }
        SemanticEmbedderProfile::Unavailable => {
            let corpus: Arc<dyn TextEmbeddingProvider + Send + Sync> = Arc::new(
                HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION),
            );
            Ok((Arc::new(ProviderUnavailableQueryTextEmbedder), corpus))
        }
    }
}

struct LedgerStructuralProducer {
    ledger: Arc<RwLock<Ledger>>,
    matcher: TruthfulSubsetAuthorityMatcher,
}

impl LedgerStructuralProducer {
    fn new(ledger: Arc<RwLock<Ledger>>) -> Self {
        Self {
            ledger,
            matcher: TruthfulSubsetAuthorityMatcher::new(),
        }
    }
}

impl StructuralProducerPort for LedgerStructuralProducer {
    fn readiness(&self, request: &DomainStructuralQueryRequest) -> StructuralReadiness {
        let pin = match resolve_structural_pin(request) {
            Ok(pin) => pin,
            Err(err) => return StructuralReadiness::InvalidRequest(err.to_string().into()),
        };
        let guard = match self.ledger.read() {
            Ok(guard) => guard,
            Err(err) => {
                return StructuralReadiness::ProducerExecution(
                    format!("structural ledger poisoned during readiness: {err}").into(),
                );
            }
        };
        let Some(state) =
            guard.structural_state(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
        else {
            drop(guard);
            return StructuralReadiness::GenerationNotReady;
        };
        let readiness_snapshot = (
            state.parse_trees().is_empty(),
            state
                .parse_trees()
                .keys()
                .any(|chunk_id| !state.chunks().contains_key(chunk_id)),
        );
        drop(guard);
        if readiness_snapshot.0 {
            return StructuralReadiness::GenerationNotReady;
        }
        if readiness_snapshot.1 {
            return StructuralReadiness::ShardUnavailable;
        }
        StructuralReadiness::Ready
    }

    fn execute(
        &self,
        request: &DomainStructuralQueryRequest,
    ) -> Result<Vec<StructuralMatchCandidate>, StructuralError> {
        let pin = resolve_structural_pin(request)?;
        if !repo_matches_structural_filters(&pin, &request.filters)? {
            return Ok(Vec::new());
        }
        let requested_lang = request
            .requested_lang
            .as_deref()
            .or(request.pattern.lang.as_deref())
            .map(str::trim)
            .filter(|lang| !lang.is_empty());
        let state = self
            .ledger
            .read()
            .map_err(|err| {
                StructuralError::ProducerExecution(format!("structural ledger poisoned: {err}"))
            })?
            .structural_state(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
            .cloned()
            .ok_or(StructuralError::GenerationNotReady)?;
        let requested_pattern = requested_lang
            .map(|lang| compile_live_structural_pattern(&request.pattern, lang))
            .transpose()?;
        let mut results = Vec::new();
        let mut observed_unsupported_lang: Option<String> = None;
        let mut saw_supported_lang = false;
        let candidate_scope = request
            .candidate_scope
            .as_ref()
            .map(|ids| ids.iter().map(String::as_str).collect::<BTreeSet<_>>());
        for (chunk_id, tree) in state.parse_trees() {
            if candidate_scope
                .as_ref()
                .is_some_and(|scope| !scope.contains(chunk_id.as_str()))
            {
                continue;
            }
            let tree_lang = tree.lang.as_str();
            if let Some(lang) = requested_lang
                && tree_lang != lang
            {
                continue;
            }
            let compiled_pattern_storage;
            let compiled_pattern = if let Some(pattern) = requested_pattern.as_ref() {
                saw_supported_lang = true;
                pattern
            } else {
                compiled_pattern_storage =
                    match compile_live_structural_pattern(&request.pattern, tree_lang) {
                        Ok(pattern) => pattern,
                        Err(StructuralError::LangNotSupported(lang)) => {
                            if observed_unsupported_lang.is_none() {
                                observed_unsupported_lang = Some(lang);
                            }
                            continue;
                        }
                        Err(err) => return Err(err),
                    };
                saw_supported_lang = true;
                &compiled_pattern_storage
            };
            let authority_pattern = lower_live_authority_pattern(compiled_pattern)?;
            let chunk = state
                .chunks()
                .get(chunk_id)
                .ok_or(StructuralError::ShardUnavailable)?;
            if !chunk_matches_structural_filters(chunk, &request.filters)? {
                continue;
            }
            let authority_candidates = self
                .matcher
                .match_authority(
                    authority_pattern,
                    StructuralAuthorityView::new(chunk.text.as_ref(), tree),
                )
                .map_err(map_live_authority_error)?;
            results.extend(project_structural_candidates(
                chunk_id,
                chunk,
                &authority_candidates,
            )?);
        }
        if requested_lang.is_none()
            && !saw_supported_lang
            && let Some(lang) = observed_unsupported_lang
        {
            return Err(StructuralError::LangNotSupported(lang));
        }
        Ok(results)
    }
}

fn repo_matches_structural_filters(
    pin: &GenerationPin,
    filters: &[StructuralExecutableFilter],
) -> Result<bool, StructuralError> {
    for filter in filters {
        match filter {
            StructuralExecutableFilter::RepoRegexNoRev { pattern } => {
                if !compile_structural_filter_regex("repo", pattern)?.is_match(pin.repo_id.as_str())
                {
                    return Ok(false);
                }
            }
            StructuralExecutableFilter::FileRegex { .. } => {}
        }
    }
    Ok(true)
}

fn chunk_matches_structural_filters(
    chunk: &ChunkRecord,
    filters: &[StructuralExecutableFilter],
) -> Result<bool, StructuralError> {
    for filter in filters {
        match filter {
            StructuralExecutableFilter::RepoRegexNoRev { .. } => {}
            StructuralExecutableFilter::FileRegex { pattern, scope } => {
                let regex = compile_structural_filter_regex("file", pattern)?;
                let path = chunk.repo_relative_path.as_str();
                let path_match = regex.is_match(path);
                let matched = match scope {
                    LqFileScope::PathOnly => path_match,
                    LqFileScope::NameOnly => path
                        .rsplit('/')
                        .next()
                        .is_some_and(|name| regex.is_match(name)),
                    LqFileScope::NameAndPath => {
                        path_match
                            || path
                                .rsplit('/')
                                .next()
                                .is_some_and(|name| regex.is_match(name))
                    }
                };
                if !matched {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn compile_structural_filter_regex(
    filter_name: &str,
    pattern: &str,
) -> Result<Regex, StructuralError> {
    Regex::new(pattern).map_err(|err| {
        StructuralError::InvalidRequest(format!(
            "{filter_name} filter pattern failed to compile as regex: {err}"
        ))
    })
}

fn resolve_structural_pin(
    request: &DomainStructuralQueryRequest,
) -> Result<GenerationPin, StructuralError> {
    match &request.generation {
        GenerationSelector::Pinned(pin) => Ok(pin.clone()),
        GenerationSelector::Active { .. } => Err(StructuralError::InvalidRequest(
            "search-plane structural producer requires a pinned generation".to_string(),
        )),
    }
}

fn compile_live_structural_pattern(
    block: &LqStructuralBlock,
    lang: &str,
) -> Result<LqStructuralPattern, StructuralError> {
    compile_authoritative_pattern(block, lang).map_err(|err| match err.code {
        LqStructuralErrorCode::StrLangNotSupported => {
            StructuralError::LangNotSupported(lang.to_string())
        }
        LqStructuralErrorCode::StrHoleKindUnsupported => {
            StructuralError::HoleKindUnsupported(err.to_string())
        }
        LqStructuralErrorCode::StrParseFail
        | LqStructuralErrorCode::StrInvalidMetavar
        | LqStructuralErrorCode::PlanLimitExceeded => {
            StructuralError::InvalidRequest(err.to_string())
        }
    })
}

fn lower_live_authority_pattern(
    pattern: &LqStructuralPattern,
) -> Result<StructuralAuthorityPatternRef<'_>, StructuralError> {
    StructuralAuthorityPatternRef::try_from(pattern).map_err(|err| match err {
        StructuralAuthorityPatternError::UnsupportedShape => StructuralError::InvalidRequest(
            "the current structural adapter set executes only parse-tree-authoritative structural shapes: descendant anchors, contiguous sibling sequences, variadic holes, where constraints, and inside/outside context".to_string(),
        ),
    })
}

#[expect(
    clippy::needless_pass_by_value,
    reason = "callers use map_err with an owned structural error"
)]
fn map_live_authority_error(err: LqStructuralError) -> StructuralError {
    match err.code {
        LqStructuralErrorCode::StrLangNotSupported => {
            StructuralError::LangNotSupported(err.detail.to_string())
        }
        LqStructuralErrorCode::StrHoleKindUnsupported => {
            StructuralError::HoleKindUnsupported(err.to_string())
        }
        LqStructuralErrorCode::StrParseFail => StructuralError::ShardUnavailable,
        LqStructuralErrorCode::StrInvalidMetavar | LqStructuralErrorCode::PlanLimitExceeded => {
            StructuralError::ProducerExecution(err.to_string())
        }
    }
}

fn project_structural_candidates(
    chunk_id: &ChunkId,
    chunk: &ChunkRecord,
    candidates: &[LqStructuralAuthorityCandidate],
) -> Result<Vec<StructuralMatchCandidate>, StructuralError> {
    candidates
        .iter()
        .map(|candidate| project_structural_candidate(chunk_id, chunk, candidate))
        .collect()
}

fn project_structural_candidate(
    chunk_id: &ChunkId,
    chunk: &ChunkRecord,
    candidate: &LqStructuralAuthorityCandidate,
) -> Result<StructuralMatchCandidate, StructuralError> {
    let bindings = candidate
        .binding
        .bindings
        .iter()
        .map(|(metavar, span)| {
            let (start_line, end_line) = chunk_line_span(chunk, span.start(), span.end())?;
            Ok(StructuralMatchBinding {
                metavariable: metavar.as_str().to_string(),
                start_byte: span.start(),
                end_byte: span.end(),
                start_line,
                end_line,
            })
        })
        .collect::<Result<Vec<_>, StructuralError>>()?;
    Ok(StructuralMatchCandidate {
        candidate_id: chunk_id.as_str().to_string(),
        pattern_start_byte: candidate.pattern_span.start(),
        pattern_end_byte: candidate.pattern_span.end(),
        bindings,
    })
}

fn chunk_line_span(
    chunk: &ChunkRecord,
    start_byte: u32,
    end_byte: u32,
) -> Result<(u32, u32), StructuralError> {
    let text = chunk.text.as_ref();
    let text_bytes = text.as_bytes();
    let start = usize::try_from(start_byte).map_err(|err| {
        StructuralError::ProducerExecution(format!("invalid structural start byte: {err}"))
    })?;
    let end = usize::try_from(end_byte).map_err(|err| {
        StructuralError::ProducerExecution(format!("invalid structural end byte: {err}"))
    })?;
    if start > end || end > text.len() {
        return Err(StructuralError::ShardUnavailable);
    }
    let start_line_offset = count_line_breaks(
        text_bytes
            .get(..start)
            .ok_or(StructuralError::ShardUnavailable)?,
    );
    let end_line_offset = if end == 0 {
        0
    } else {
        count_line_breaks(
            text_bytes
                .get(..end.saturating_sub(1))
                .ok_or(StructuralError::ShardUnavailable)?,
        )
    };
    Ok((
        chunk.start_line.saturating_add(start_line_offset),
        chunk.start_line.saturating_add(end_line_offset),
    ))
}

fn count_line_breaks(text: &[u8]) -> u32 {
    u32::try_from(memchr_iter(b'\n', text).count()).map_or(u32::MAX, core::convert::identity)
}

/// Composed runtime: direct-ingest adapters, ledger, and UDS servers.
pub struct SearchdRuntime {
    pub config: SearchdConfig,
    pub query_server: SearchPlaneQueryServer<
        dyn IpcDispatcher<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse>,
    >,
    pub control_server: SearchPlaneControlServer<
        dyn IpcDispatcher<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse>,
    >,
    pub ingest_server: SearchPlaneIngestServer<
        dyn IpcDispatcher<SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse>,
    >,
    pub repo_map_query_port: Arc<dyn RepoMapQueryPort + Send + Sync>,
    pub query_obs_store: Arc<BoundedQueryObsStore>,
    pub semantic_boot: semantic_boot::SemanticBootReport,
    // Rust drops fields in declaration order. Keep the state-root lease last
    // so every adapter, server, and authority handle is gone before ownership
    // of the shared root is released.
    _state_root_lease: StateRootLease,
}

impl SearchdRuntime {
    /// Assemble the runtime from externally-supplied ports.
    pub fn assemble(config: SearchdConfig, parts: SearchdRuntimeParts) -> Result<Self> {
        let SearchdRuntimeParts {
            state_root_lease,
            search_corpus_build_port,
            lexical_generation_scanner,
            lexical_generation_validator,
            lexical_incomplete_discard,
            lex_open_port,
            repo_commit_recency_ingest_port,
            repo_topic_ingest_port,
            repo_description_ingest_port,
            file_ownership_ingest_port,
            file_contributor_ingest_port,
            repo_meta_ingest_port,
            sem_build_port,
            semantic_generation_validator,
            semantic_incomplete_discard,
            sem_open_port,
            repo_map_query_port,
            repo_map_bundle_ingest_port,
            repo_map_generation_activate_port,
            activation_catalog,
            aux_authority_store,
            legacy_semantic_journal_store,
        } = parts;
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        bootstrap_persisted_lexical_state(
            &ledger,
            lexical_generation_scanner.as_ref(),
            lexical_generation_validator.as_ref(),
        )?;
        let semantic_root = quanta_index_semantic::semantic_state_root(config.state_root());
        let migration_start = std::time::Instant::now();
        let migration = semantic_boot::migrate_legacy_semantic_journal(
            legacy_semantic_journal_store.as_ref(),
            sem_build_port.as_ref(),
            &semantic_root,
        )
        .map_err(anyhow::Error::from)?;
        let migration_micros = migration_start.elapsed().as_micros();
        let seed_start = std::time::Instant::now();
        let seed = semantic_boot::seed_persisted_semantic_readiness(&ledger, &semantic_root)
            .map_err(anyhow::Error::from)?;
        let boot_report = semantic_boot::SemanticBootReport {
            migration,
            migration_micros,
            seed,
            seed_micros: seed_start.elapsed().as_micros(),
        };
        {
            let mut guard = ledger.write().map_err(|err| {
                anyhow::anyhow!("ledger poisoned during auxiliary authority bootstrap: {err}")
            })?;
            aux_authority_store
                .restore_into(&mut guard)
                .map_err(anyhow::Error::from)?;
        }
        let (query_text_embedder, corpus_embedder) =
            build_semantic_embedders(config.semantic_embedder_profile(), config.state_root())
                .map_err(anyhow::Error::from)?;
        let direct_sem_ingest_port: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
            DirectSemanticMaterializer::new(Arc::clone(&sem_build_port), Arc::clone(&ledger)),
        );
        let direct_search_corpus_ingest_port: Arc<dyn SearchCorpusIngestPort + Send + Sync> =
            Arc::new(
                DirectSearchCorpusMaterializer::new_with_search_owned_semantics_from_env(
                    SearchCorpusMaterializerParts {
                        builder: Arc::clone(&search_corpus_build_port),
                        ledger: Arc::clone(&ledger),
                        semantic_ingest: Arc::clone(&direct_sem_ingest_port),
                        semantic_embedder: corpus_embedder,
                        authority: aux_authority_store.clone(),
                        lexical_generation_validator: Arc::clone(&lexical_generation_validator),
                        semantic_generation_validator: Arc::clone(&semantic_generation_validator),
                        lexical_incomplete_discard: Arc::clone(&lexical_incomplete_discard),
                        semantic_incomplete_discard: Arc::clone(&semantic_incomplete_discard),
                    },
                )
                .map_err(anyhow::Error::from)?,
            );
        let direct_history_ingest_port: Arc<dyn HistoryIngestPort + Send + Sync> = Arc::new(
            DirectHistoryMaterializer::new(aux_authority_store.clone(), Arc::clone(&ledger)),
        );
        let direct_runtime_ingest_port: Arc<dyn RuntimeMetadataIngestPort + Send + Sync> =
            Arc::new(DirectRuntimeMetadataMaterializer::new(
                aux_authority_store.clone(),
                Arc::clone(&ledger),
            ));
        let direct_structural_ingest_port: Arc<dyn StructuralIngestPort + Send + Sync> = Arc::new(
            DirectStructuralMaterializer::new(aux_authority_store, Arc::clone(&ledger)),
        );

        let query_obs_store = Arc::new(BoundedQueryObsStore::default());
        let query_obs_sink: Arc<dyn QueryObsSink + Send + Sync> =
            if std::env::var_os(BENCH_DISABLE_QUERY_OBS_ENV).is_some() {
                Arc::new(NoopQueryObsSink)
            } else {
                query_obs_store.clone()
            };
        let query_dispatcher = Arc::new(SearchPlaneDispatcher::new_with_obs(
            Arc::clone(&lex_open_port),
            Arc::clone(&sem_open_port),
            Arc::clone(&repo_map_query_port),
            Arc::new(LedgerStructuralProducer::new(Arc::clone(&ledger))),
            Arc::clone(&ledger),
            activation_catalog.clone(),
            query_text_embedder,
            query_obs_sink,
        ));
        let control_dispatcher = Arc::new(SearchPlaneControlDispatcher::new(
            repo_map_generation_activate_port,
            activation_catalog,
            Arc::clone(&ledger),
            lexical_generation_validator,
            semantic_generation_validator,
        ));
        let ingest_dispatcher = Arc::new(SearchPlaneIngestDispatcher::new(
            direct_search_corpus_ingest_port,
            direct_history_ingest_port,
            repo_commit_recency_ingest_port,
            repo_topic_ingest_port,
            repo_description_ingest_port,
            file_ownership_ingest_port,
            file_contributor_ingest_port,
            repo_meta_ingest_port,
            direct_runtime_ingest_port,
            direct_structural_ingest_port,
            repo_map_bundle_ingest_port,
        ));
        let query_adapter: Arc<
            dyn IpcDispatcher<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse>,
        > = Arc::new(SearchPlaneQueryIpcAdapter::new(query_dispatcher));
        let query_server = SearchPlaneQueryServer::bind(
            "quanta-index-query-uds",
            config.query_socket_path(),
            query_adapter,
        )
        .map_err(anyhow::Error::from)?;
        let control_adapter: Arc<
            dyn IpcDispatcher<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse>,
        > = Arc::new(SearchPlaneControlIpcAdapter::new(control_dispatcher));
        let control_server = SearchPlaneControlServer::bind(
            "quanta-index-control-uds",
            config.control_socket_path(),
            control_adapter,
        )
        .map_err(anyhow::Error::from)?;
        let ingest_adapter: Arc<
            dyn IpcDispatcher<SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse>,
        > = Arc::new(SearchPlaneIngestIpcAdapter::new(ingest_dispatcher));
        let ingest_server = SearchPlaneIngestServer::bind(
            "quanta-index-ingest-uds",
            config.ingest_socket_path(),
            ingest_adapter,
        )
        .map_err(anyhow::Error::from)?;

        Ok(Self {
            config,
            query_server,
            control_server,
            ingest_server,
            repo_map_query_port,
            query_obs_store,
            semantic_boot: boot_report,
            _state_root_lease: state_root_lease,
        })
    }
}

fn bootstrap_persisted_lexical_state(
    ledger: &Arc<RwLock<Ledger>>,
    scanner: &dyn SealedGenerationScanPort,
    validator: &dyn GenerationIdentityValidatePort,
) -> Result<()> {
    seed_persisted_lexical_readiness(ledger, scanner, validator)?;
    Ok(())
}

fn seed_persisted_lexical_readiness(
    ledger: &Arc<RwLock<Ledger>>,
    scanner: &dyn SealedGenerationScanPort,
    validator: &dyn GenerationIdentityValidatePort,
) -> Result<()> {
    let persisted = scanner
        .scan_sealed_generations()
        .map_err(anyhow::Error::from)?;
    let mut guard = ledger
        .write()
        .map_err(|err| anyhow::anyhow!("ledger poisoned during lexical bootstrap: {err}"))?;
    let mut max_generation: Option<ManifestGeneration> = None;
    for candidate in persisted {
        if candidate.track != SearchPlaneTrackKind::Lexical {
            return Err(anyhow::anyhow!(
                "lexical bootstrap scanner returned non-lexical track {:?}",
                candidate.track
            ));
        }
        validator
            .validate_generation_identity(&candidate)
            .map_err(anyhow::Error::from)?;
        guard.record_track_materialized(
            &candidate.repo_id,
            &candidate.revision_id,
            SearchPlaneTrackKind::Lexical,
            candidate.manifest_generation,
            Some(candidate.manifest_digest.as_str()),
        );
        guard.record_track_seal_with_digest(
            &candidate.repo_id,
            &candidate.revision_id,
            SearchPlaneTrackKind::Lexical,
            candidate.manifest_generation,
            candidate.manifest_digest.as_str(),
        );
        max_generation = match max_generation {
            Some(current) if current.get() >= candidate.manifest_generation.get() => Some(current),
            _ => Some(candidate.manifest_generation),
        };
    }
    if let Some(max_generation) = max_generation {
        guard.lexical_materialize(max_generation, None);
        guard.lexical_seal(max_generation);
    }
    drop(guard);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DomainStructuralQueryRequest, GenerationPin, GenerationSelector, Ledger,
        LedgerStructuralProducer, LqStructuralBlock, StructuralProducerPort, StructuralReadiness,
        ensure_durable_state_root_with_v1,
    };
    use quanta_index_contract::{LqOptions, RepoId, RevisionId};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Arc, Mutex, RwLock};
    type TestRes = Result<(), Box<dyn std::error::Error>>;

    fn request_with_generation(generation: GenerationSelector) -> DomainStructuralQueryRequest {
        DomainStructuralQueryRequest {
            pattern: LqStructuralBlock {
                lang: None,
                nodes: Vec::new(),
                exprs: Vec::new(),
            },
            requested_lang: None,
            filters: Vec::new(),
            candidate_scope: None,
            options: LqOptions::defaults(),
            generation,
        }
    }

    fn pinned_request() -> DomainStructuralQueryRequest {
        request_with_generation(GenerationSelector::Pinned(GenerationPin::new(
            RepoId::new("repo".to_string()),
            RevisionId::new("rev".to_string()),
            quanta_index_contract::ManifestGeneration::new(7),
        )))
    }

    #[test]
    fn structural_readiness_rejects_active_generation_selector() {
        let producer = LedgerStructuralProducer::new(Arc::new(RwLock::new(Ledger::new())));
        let readiness = producer.readiness(&request_with_generation(GenerationSelector::Active {
            repo_id: RepoId::new("repo".to_string()),
            revision_id: RevisionId::new("rev".to_string()),
        }));
        assert!(matches!(
            readiness,
            StructuralReadiness::InvalidRequest(message)
                if message.as_ref()
                    == "structural: invalid request: search-plane structural producer requires a pinned generation"
        ));
    }

    #[test]
    fn structural_readiness_fails_closed_on_poisoned_ledger() {
        let result = (|| -> TestRes {
            struct PanicOnDrop;

            impl Drop for PanicOnDrop {
                fn drop(&mut self) {
                    std::panic::resume_unwind(Box::new("poison structural ledger"));
                }
            }

            let ledger = Arc::new(RwLock::new(Ledger::new()));
            let poisoned = Arc::clone(&ledger);
            let unwind = catch_unwind(AssertUnwindSafe(move || -> TestRes {
                let _guard = poisoned.write().map_err(|err| {
                    format!("test must acquire write lock before poisoning: {err}")
                })?;
                let _tripwire = PanicOnDrop;
                Ok(())
            }));
            match unwind {
                Err(_) => {}
                Ok(Ok(())) => return Err("poison tripwire did not unwind".into()),
                Ok(Err(err)) => return Err(err),
            }
            let producer = LedgerStructuralProducer::new(ledger);
            let readiness = producer.readiness(&pinned_request());
            assert!(matches!(
                readiness,
                StructuralReadiness::ProducerExecution(message)
                    if message.starts_with("structural ledger poisoned during readiness:")
            ));
            Ok(())
        })();
        assert!(result.is_ok(), "{result:?}");
    }

    // The cache_enabled knob gates whether the OpenAi provider is wrapped in a
    // FileEmbeddingCache (which materializes an `embed-cache` dir under the state
    // root). These two tests pin BOTH branches of the composition root — provider
    // construction is offline (a non-empty key builds the client without any
    // network call), so the only observable is the cache dir.
    fn openai_profile(cache_enabled: bool) -> super::SemanticEmbedderProfile {
        super::SemanticEmbedderProfile::OpenAi {
            model: "text-embedding-3-small".to_string(),
            dimension: 1536,
            api_key: "sk-unit-test".to_string(),
            tuning: crate::app::config::OpenAiEmbedderTuning {
                cache_enabled,
                ..Default::default()
            },
        }
    }

    // CASE-COVERS (positive half of the identity-coupling invariant): the query
    // embedder and corpus embedder that `build_semantic_embedders` returns for the
    // SAME provider profile MUST advertise the SAME model identity. This is the
    // property that makes the query-time model-identity gate automatically
    // consistent with the vectors the corpus derivation indexed — instead of the
    // two sides being hardcoded/asserted to agree, they share one provider identity
    // by construction. A real (OpenAi) provider is used so the identity is a genuine
    // network-model id (`openai:text-embedding-3-small`), not the hash fixture's
    // constant.
    //
    // If a future refactor built the query and corpus embedders from DIFFERENT
    // providers (the exact drift this seam exists to prevent), their model_id()s
    // would diverge and this assertion would fail.
    #[test]
    fn build_semantic_embedders_couples_query_and_corpus_model_identity_for_real_provider()
    -> TestRes {
        let dir = tempfile::tempdir()?;
        let (query_embedder, corpus_embedder) =
            super::build_semantic_embedders(&openai_profile(false), dir.path())
                .map_err(|err| format!("offline construction must succeed: {err:?}"))?;

        // Positive: matched identity across the two sides (id AND version).
        if query_embedder.model_id() != corpus_embedder.model_id() {
            return Err(format!(
                "query/corpus model_id drift: query={:?} corpus={:?} (the shared-provider seam is broken)",
                query_embedder.model_id(),
                corpus_embedder.model_id()
            )
            .into());
        }
        if query_embedder.model_version() != corpus_embedder.model_version() {
            return Err(format!(
                "query/corpus model_version drift: query={:?} corpus={:?}",
                query_embedder.model_version(),
                corpus_embedder.model_version()
            )
            .into());
        }
        // The coupled identity is the genuine provider model id (the OpenAI provider
        // namespaces it as `openai:<model>`), NOT the hash fixture constant — proving
        // the real provider (not a silent hash fallback) is wired on BOTH sides in the
        // production `OpenAi` profile.
        if query_embedder.model_id() != "openai:text-embedding-3-small" {
            return Err(format!(
                "expected real provider model id on the query side, got {:?} (hash fallback leaked?)",
                query_embedder.model_id()
            )
            .into());
        }
        if query_embedder.model_id() == quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_MODEL_ID {
            return Err(
                "OpenAi profile must NOT advertise the search-owned hash fixture identity".into(),
            );
        }
        Ok(())
    }

    // CASE-COVERS (negative half of the identity-coupling invariant): the
    // deliberately-degraded `Unavailable` profile DECOUPLES the query identity from
    // the corpus identity — the query embedder advertises the `provider-unavailable`
    // sentinel while the corpus still hash-derives under the search-owned hash id.
    // Because those identities differ, a semantic query in this config fails closed
    // at the query-time model-identity gate (it never silently reuses the hash
    // corpus vectors as if a real query embedder had produced the query vector).
    // This is the structural counter-case to the positive coupling test above:
    // matched profile -> identities agree; degraded profile -> identities disagree
    // by design, which the gate then rejects.
    #[test]
    fn unavailable_profile_decouples_query_identity_from_corpus_identity() -> TestRes {
        let dir = tempfile::tempdir()?;
        let (query_embedder, corpus_embedder) = super::build_semantic_embedders(
            &super::SemanticEmbedderProfile::Unavailable,
            dir.path(),
        )
        .map_err(|err| format!("unavailable-profile construction must succeed: {err:?}"))?;

        // Corpus side still derives under the search-owned hash identity (so the
        // generation materializes) — the deliberate degraded-config contract.
        if corpus_embedder.model_id() != quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_MODEL_ID {
            return Err(format!(
                "unavailable profile must keep the hash corpus identity, got {:?}",
                corpus_embedder.model_id()
            )
            .into());
        }

        // Query side is the fail-closed sentinel, NOT the corpus identity: the two
        // sides are decoupled, so a query cannot masquerade as the hash-indexed model.
        if query_embedder.model_id() == corpus_embedder.model_id() {
            return Err(
                "unavailable profile must NOT let the query embedder advertise the corpus identity"
                    .into(),
            );
        }

        // And the sentinel query embedder itself fails closed on embed — proving no
        // real query vector is ever produced in this config (no silent hash fallback).
        match query_embedder.embed_query("anything") {
            Err(quanta_index_core::CoreError::Typed { code, .. }) => {
                let expected = quanta_index_contract::lex::LexicalErrorCode::SemProviderUnavailable
                    .as_code_str();
                if code != expected {
                    return Err(
                        format!("expected SEM_PROVIDER_UNAVAILABLE, got code {code}").into(),
                    );
                }
            }
            other => {
                return Err(
                    format!("unavailable query embedder must fail closed, got {other:?}").into(),
                );
            }
        }
        Ok(())
    }

    // A1 HERMETIC SEMANTIC SMOKE (searchd real vector path, NO network): the
    // `Hash` profile is the deterministic hermetic embedder searchd defaults to
    // when `QUANTA_INDEX_EMBEDDER` is unset / `hash` (see
    // `config::semantic_embedder_profile_from_env`). This smoke proves the REAL
    // searchd semantic vector derivation — the exact query + corpus embedders
    // `SearchdRuntime::assemble` wires for the Hash profile — actually produces
    // non-degenerate, dimension-correct, deterministic vectors and that the query
    // and corpus sides share ONE model identity (so a query vector is comparable
    // against corpus-indexed vectors). This is the offline vector-path proof; the
    // full daemon ingest->query round-trip over UDS is proven on the semantica
    // product-path integration test (`search_owner_semantic_product_path_contract_test`,
    // which spawns the real searchd process with `QUANTA_INDEX_EMBEDDER=hash`).
    //
    // The RELEASE truth proof — `QUANTA_INDEX_EMBEDDER=openai` + `OPENAI_API_KEY`
    // + a live `/v1/embeddings` ingest->query run — is a documented MANUAL rail and
    // is deliberately NOT run here (needs network + key). See A1-closeout (qidx).
    #[test]
    fn hermetic_hash_profile_derives_real_coupled_semantic_vectors_v1() -> TestRes {
        use quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_DIMENSION;

        // The hermetic default: the exact profile `semantic_embedder_profile_from_env`
        // returns for `QUANTA_INDEX_EMBEDDER=hash` / unset.
        let profile = super::SemanticEmbedderProfile::Hash {
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
        };
        let dir = tempfile::tempdir()?;
        let (query_embedder, corpus_embedder) =
            super::build_semantic_embedders(&profile, dir.path()).map_err(|err| {
                format!("hermetic hash embedder construction must succeed: {err:?}")
            })?;

        // (1) Query + corpus share ONE model identity by construction (no drift
        // between the vector the query path embeds and the vectors the corpus
        // derivation indexed).
        if query_embedder.model_id() != corpus_embedder.model_id() {
            return Err(format!(
                "hash profile query/corpus model_id drift: query={:?} corpus={:?}",
                query_embedder.model_id(),
                corpus_embedder.model_id()
            )
            .into());
        }
        if query_embedder.model_id() != quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_MODEL_ID {
            return Err(format!(
                "hash profile must advertise the search-owned hash identity, got {:?}",
                query_embedder.model_id()
            )
            .into());
        }

        // (2) The REAL query vector path: a non-empty text derives a
        // dimension-correct, non-degenerate (not all-zero) vector.
        let corpus_text = "semantica hermetic semantic ingest anchor token";
        let query_text = "semantica hermetic semantic ingest anchor token";
        let corpus_vectors = corpus_embedder
            .embed_batch(&[corpus_text])
            .map_err(|err| format!("hash corpus embed_batch must derive a real vector: {err:?}"))?;
        let corpus_vector = corpus_vectors
            .first()
            .ok_or("hash corpus derivation returned no vector")?;
        let query_vector = query_embedder
            .embed_query(query_text)
            .map_err(|err| format!("hash query embed must derive a real vector: {err:?}"))?;

        if corpus_vector.len() != SEARCH_OWNED_SEMANTIC_DIMENSION
            || query_vector.len() != SEARCH_OWNED_SEMANTIC_DIMENSION
        {
            return Err(format!(
                "hash vectors must have the search-owned dimension {SEARCH_OWNED_SEMANTIC_DIMENSION}, got corpus={} query={}",
                corpus_vector.len(),
                query_vector.len()
            )
            .into());
        }
        if corpus_vector.iter().all(|value| *value == 0.0)
            || query_vector.iter().all(|value| *value == 0.0)
        {
            return Err(
                "hash vectors must be non-degenerate (an all-zero vector means the real derivation did not run)"
                    .into(),
            );
        }

        // (3) Determinism: re-deriving the SAME text yields the identical vector
        // (searchd's real path must be reproducible for cache/incremental sanity).
        let query_vector_again = query_embedder
            .embed_query(query_text)
            .map_err(|err| format!("hash query re-embed must succeed: {err:?}"))?;
        if query_vector != query_vector_again {
            return Err(
                "hash query derivation must be deterministic across identical input".into(),
            );
        }

        // (4) Discrimination: a DIFFERENT text derives a DIFFERENT vector (the
        // embedder is not a constant — the negative half of the smoke).
        let other_vector = query_embedder
            .embed_query("completely unrelated lexical payload zzz")
            .map_err(|err| format!("hash query embed (other text) must succeed: {err:?}"))?;
        if query_vector == other_vector {
            return Err(
                "hash query derivation must distinguish distinct inputs (constant embedder detected)"
                    .into(),
            );
        }

        Ok(())
    }

    #[test]
    fn build_semantic_embedders_creates_cache_dir_when_cache_enabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        // `expect` only needs the CoreError (Err) to be Debug; the Ok tuple of
        // trait objects is not, so we bind it rather than format it.
        let _embedders = super::build_semantic_embedders(&openai_profile(true), dir.path())
            .expect("offline construction must succeed");
        assert!(
            dir.path().join("embed-cache").is_dir(),
            "cache_enabled=true must materialize the embed-cache dir"
        );
    }

    #[test]
    fn build_semantic_embedders_skips_cache_dir_when_cache_disabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _embedders = super::build_semantic_embedders(&openai_profile(false), dir.path())
            .expect("offline construction must succeed");
        assert!(
            !dir.path().join("embed-cache").exists(),
            "cache_enabled=false must NOT create the embed-cache dir (bare provider)"
        );
    }

    #[test]
    fn fresh_nested_state_root_syncs_every_created_parent_in_order() -> TestRes {
        let parent = tempfile::tempdir()?;
        let state_root = parent.path().join("one").join("two").join("state");
        let synced = Mutex::new(Vec::new());

        ensure_durable_state_root_with_v1(&state_root, &|path| {
            synced
                .lock()
                .map_err(|_| std::io::Error::other("sync log poisoned"))?
                .push(path.to_path_buf());
            Ok(())
        })?;

        assert!(state_root.is_dir());
        assert_eq!(
            synced.into_inner().map_err(|_| "sync log poisoned")?,
            vec![
                parent.path().to_path_buf(),
                parent.path().join("one"),
                parent.path().join("one").join("two"),
            ]
        );
        Ok(())
    }

    #[test]
    fn fresh_state_root_parent_sync_failure_is_not_acknowledged() -> TestRes {
        let parent = tempfile::tempdir()?;
        let state_root = parent.path().join("state");
        let error = ensure_durable_state_root_with_v1(&state_root, &|_path| {
            Err(std::io::Error::other("injected parent sync failure"))
        })
        .expect_err("parent sync failure must fail state-root bootstrap");

        assert!(format!("{error:?}").contains("injected parent sync failure"));
        assert!(!state_root.join(".searchd-state-root.lock").exists());
        Ok(())
    }
}
