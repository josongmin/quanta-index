//! Composed runtime artefacts. Built once per daemon process.

use std::collections::BTreeSet;
use std::fs::File;
#[cfg(not(unix))]
use std::fs::OpenOptions;
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
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
    SearchPlaneTrackKind,
};
use quanta_index_core::domains::structural::{
    StructuralExecutableFilter, StructuralProducerPort,
    StructuralQueryRequest as DomainStructuralQueryRequest,
};
use quanta_index_core::{
    AuxiliaryAuthorityCatalogPort, CoreError, FileContributorIngestPort, FileOwnershipIngestPort,
    GenerationIdentityValidatePort, IdempotencyCatalogPort, IncompleteGenerationDiscardPort,
    L2UnitEmbeddingProvider, LexicalIndexOpenPort, MetricSourcePort,
    QuarantinedGenerationDiscardPort, RepoCommitRecencyIngestPort, RepoDescriptionIngestPort,
    RepoMapBundleIngestPort, RepoMapGenerationActivatePort, RepoMapOpenReportV1,
    RepoMapQuarantinePort, RepoMapQueryPort, RepoMetaIngestPort, RepoTopicIngestPort,
    SealedGenerationReclaimPort, SealedGenerationScanPort, SearchCorpusBatchBuildPort,
    SearchCorpusIngestPort, SemanticBatchBuildPort, SemanticIndexOpenPort, SemanticIngestPort,
    StructuralError, StructuralMatchBinding, StructuralMatchCandidate, StructuralReadiness,
    TextEmbeddingProvider,
};
use quanta_index_embed::{
    CachingEmbeddingProvider, EmbeddingCacheIdentityV1, FileEmbeddingCache, OpenAiEmbeddingProvider,
};
use quanta_index_ipc::{IpcDispatcher, IpcServerCounters, ServerAdmissionPolicy};
use quanta_index_lq_structural::{
    StructuralAuthorityCandidate as LqStructuralAuthorityCandidate, StructuralAuthorityMatcher,
    StructuralAuthorityPatternError, StructuralAuthorityPatternRef, StructuralAuthorityView,
    StructuralError as LqStructuralError, StructuralErrorCode as LqStructuralErrorCode,
    StructuralPattern as LqStructuralPattern, TruthfulSubsetAuthorityMatcher,
    compile_authoritative_pattern,
};
use quanta_index_search_plane::{
    AuxiliaryMaterializerParts, AuxiliaryMutationCoordinator, BoundedQueryObsStore,
    DirectHistoryMaterializer, DirectRuntimeMetadataMaterializer, DirectSearchCorpusMaterializer,
    DirectSemanticMaterializer, DirectStructuralMaterializer, HashingQueryTextEmbedder,
    HistoryIngestPort, Ledger, ObservabilityScrape, QuarantineService, QuarantineServiceParts,
    QueryObsSink, QueryTextEmbedderPort, RuntimeMetadataIngestPort,
    SEARCH_OWNED_SEMANTIC_DIMENSION, SearchCorpusLifecycleOwner, SearchCorpusMaterializerParts,
    SearchPlaneControlDispatcher, SearchPlaneDispatcher, SearchPlaneIngestDispatcher,
    SnapshotRegistries, StructuralIngestPort,
};
use regex::Regex;

use crate::app::boot_inventory::{self, BootInventoryReportV1};
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
    pub lexical_sealed_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    pub lex_open_port: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    pub repo_commit_recency_ingest_port: Arc<dyn RepoCommitRecencyIngestPort + Send + Sync>,
    pub repo_topic_ingest_port: Arc<dyn RepoTopicIngestPort + Send + Sync>,
    pub repo_description_ingest_port: Arc<dyn RepoDescriptionIngestPort + Send + Sync>,
    pub file_ownership_ingest_port: Arc<dyn FileOwnershipIngestPort + Send + Sync>,
    pub file_contributor_ingest_port: Arc<dyn FileContributorIngestPort + Send + Sync>,
    pub repo_meta_ingest_port: Arc<dyn RepoMetaIngestPort + Send + Sync>,
    pub sem_build_port: Arc<dyn SemanticBatchBuildPort + Send + Sync>,
    pub semantic_generation_scanner: Arc<dyn SealedGenerationScanPort + Send + Sync>,
    pub semantic_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    pub semantic_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>,
    pub semantic_sealed_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    pub sem_open_port: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    pub repo_map_query_port: Arc<dyn RepoMapQueryPort + Send + Sync>,
    pub repo_map_bundle_ingest_port: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    pub repo_map_generation_activate_port: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
    /// The quarantine's destructive side (QI-BB-026): one discard port per
    /// generation track, and the `RepoMap` store's own list-and-discard.
    pub lexical_quarantine_discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync>,
    pub semantic_quarantine_discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync>,
    pub repo_map_quarantine: Arc<dyn RepoMapQuarantinePort + Send + Sync>,
    /// What the `RepoMap` store found on disk when the composition root
    /// opened it (QI-BB-008); surfaced through the boot inventory.
    pub repo_map_open_report: RepoMapOpenReportV1,
    pub search_corpus_lifecycle: Arc<SearchCorpusLifecycleOwner>,
    pub legacy_semantic_journal_store: Arc<LegacySemanticJournalStore>,
    /// Durable ingest idempotency records (QI-BB-032).
    pub idempotency: Arc<dyn IdempotencyCatalogPort + Send + Sync>,
    /// Durable auxiliary authority rows (QI-BB-020).
    pub auxiliary_catalog: Arc<dyn AuxiliaryAuthorityCatalogPort + Send + Sync>,
    /// Adapters that keep their own accounting, for the metrics scrape
    /// (QI-BB-015); the composition root adds its own sources to these.
    pub adapter_metric_sources: Vec<Arc<dyn MetricSourcePort>>,
}

/// Exclusive process-lifetime ownership of one daemon state root.
///
/// The lock file is intentionally persistent; the OS lock, not file presence,
/// owns liveness. Dropping the file handle releases the lease after crashes and
/// normal shutdown without stale-file recovery heuristics.
#[derive(Debug)]
pub struct StateRootLease {
    _file: File,
    state_root_identity_v1: PathBuf,
}

impl StateRootLease {
    pub fn acquire(state_root: &Path) -> Result<Self, CoreError> {
        ensure_durable_state_root_v1(state_root)?;
        let state_root_identity_v1 = canonical_state_root_identity_v1(state_root)?;
        ensure_private_state_root_v1(&state_root_identity_v1)?;
        let path = state_root_identity_v1.join(".searchd-state-root.lock");
        let file = open_state_root_lock_nofollow_v1(&path).map_err(|error| {
            CoreError::Storage(format!(
                "searchd state-root lease: open {}: {error}",
                path.display()
            ))
        })?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self {
                _file: file,
                state_root_identity_v1,
            }),
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

    pub fn require_state_root_v1(&self, state_root: &Path) -> Result<(), CoreError> {
        let observed = canonical_state_root_identity_v1(state_root)?;
        if observed == self.state_root_identity_v1 {
            return Ok(());
        }
        Err(CoreError::InvalidContract(format!(
            "searchd state-root lease: identity mismatch: lease={} requested={}",
            self.state_root_identity_v1.display(),
            observed.display(),
        )))
    }

    #[must_use]
    pub fn state_root_identity_v1(&self) -> &Path {
        &self.state_root_identity_v1
    }
}

#[cfg(unix)]
fn open_state_root_lock_nofollow_v1(path: &Path) -> std::io::Result<File> {
    use rustix::fs::{Mode, OFlags, open};

    let file = open(
        path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::CREATE,
        Mode::from_raw_mode(0o600),
    )
    .map(File::from)
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other(format!(
            "state-root lock is not a regular file: {}",
            path.display()
        )));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_state_root_lock_nofollow_v1(path: &Path) -> std::io::Result<File> {
    if fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(std::io::Error::other(format!(
            "state-root lock is a symlink: {}",
            path.display()
        )));
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn canonical_state_root_identity_v1(state_root: &Path) -> Result<PathBuf, CoreError> {
    fs::canonicalize(state_root).map_err(|error| {
        CoreError::Storage(format!(
            "searchd state-root lease: resolve state-root identity {}: {error}",
            state_root.display(),
        ))
    })
}

/// Mode of a state-root directory this daemon creates: its owner only.
#[cfg(unix)]
const STATE_ROOT_DIRECTORY_MODE: u32 = 0o700;

/// Refuse a state root another local user could write into (QI-BB-014).
///
/// A directory this daemon created is `0700`. A pre-existing one must be
/// owned by the user the daemon runs as and carry no group or other write
/// bit: index files, catalogs and sockets live under it, and a writable
/// root lets another local user replace any of them under the daemon. A
/// root others can read is the operator's choice and is not refused.
#[cfg(unix)]
fn ensure_private_state_root_v1(state_root: &Path) -> Result<(), CoreError> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = fs::metadata(state_root).map_err(|error| {
        CoreError::Storage(format!(
            "searchd state-root lease: inspect {}: {error}",
            state_root.display()
        ))
    })?;
    let owner = rustix::process::geteuid().as_raw();
    let mode = metadata.mode() & 0o7777;
    if metadata.uid() != owner || mode & 0o022 != 0 {
        return Err(CoreError::Typed {
            code: "STATE_ROOT_INSECURE".to_string(),
            message: format!(
                "searchd state root {} is uid {} mode {mode:04o}; it must belong to uid {owner} and carry no group/other write bit (chmod go-w)",
                state_root.display(),
                metadata.uid()
            ),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_state_root_v1(_state_root: &Path) -> Result<(), CoreError> {
    Ok(())
}

#[cfg(unix)]
fn create_private_directory_v1(directory: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    fs::DirBuilder::new()
        .mode(STATE_ROOT_DIRECTORY_MODE)
        .create(directory)
}

#[cfg(not(unix))]
fn create_private_directory_v1(directory: &Path) -> std::io::Result<()> {
    fs::create_dir(directory)
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
        match create_private_directory_v1(directory) {
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

    fn model_id(&self) -> &'static str {
        // Sentinel only: this embedder always fails `embed_query` before any
        // model-identity comparison runs, so this value is never used to decide
        // a query. It must not collide with a real model id.
        "provider-unavailable"
    }

    fn model_revision(&self) -> &'static str {
        "unavailable"
    }
}

struct NoopQueryObsSink;

impl QueryObsSink for NoopQueryObsSink {
    fn emit(&self, _sample: quanta_index_search_plane::MetricSample) {}
}

/// Adapts the batch-capable [`TextEmbeddingProvider`] to the query side.
///
/// [`QueryTextEmbedderPort`] embeds one text at a time; routing it through the
/// same provider instance the corpus path uses means the two cannot diverge on
/// model identity.
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

    fn model_revision(&self) -> &str {
        self.0.model_revision()
    }
}

/// The query-side and corpus-side embedders resolved for one profile.
///
/// Both halves are backed by the same provider for `Hash`/`OpenAi` so model
/// identity matches; for `Unavailable` the query embedder fails closed while
/// the corpus still hash-derives (the deliberate degraded-config contract).
/// `metric_sources` carries whatever the profile built that keeps its own
/// accounting (the embedding cache), for the metrics scrape (QI-BB-015).
struct SemanticEmbedders {
    query: Arc<dyn QueryTextEmbedderPort + Send + Sync>,
    corpus: Arc<dyn TextEmbeddingProvider + Send + Sync>,
    metric_sources: Vec<Arc<dyn MetricSourcePort>>,
}

/// Resolve the (query embedder, corpus embedder) pair for a profile.
///
/// See [`SemanticEmbedders`] for the identity contract the pair upholds.
fn build_semantic_embedders(
    profile: &SemanticEmbedderProfile,
    state_root: &Path,
) -> Result<SemanticEmbedders, CoreError> {
    match profile {
        SemanticEmbedderProfile::Hash { dimension } => {
            let provider: Arc<dyn TextEmbeddingProvider + Send + Sync> =
                Arc::new(HashingQueryTextEmbedder::new(*dimension));
            Ok(SemanticEmbedders {
                query: Arc::new(QueryEmbedderAdapter(Arc::clone(&provider))),
                corpus: provider,
                metric_sources: Vec::new(),
            })
        }
        SemanticEmbedderProfile::OpenAi {
            model,
            model_revision,
            dimension,
            api_key,
            tuning,
        } => {
            // Thread the env-resolved operational knobs into the provider config
            // (mapping owned + unit-tested on OpenAiEmbedderTuning::provider_config).
            let openai = OpenAiEmbeddingProvider::with_reqwest(tuning.provider_config(
                model.clone(),
                model_revision.clone(),
                *dimension,
                api_key.clone(),
            ))?;
            // Raw provider output is unit-normalized by the shared wrapper
            // before either the cache or a path sees it (QI-BB-031).
            let normalized = L2UnitEmbeddingProvider::new(openai)?;
            let mut metric_sources: Vec<Arc<dyn MetricSourcePort>> = Vec::new();
            let provider: Arc<dyn TextEmbeddingProvider + Send + Sync> = if tuning.cache_enabled {
                // Persistent content-hash cache under the identity's own
                // namespace (model, revision, dimension, policy — QI-BB-028)
                // so rebuilds / incrementals avoid paid re-embedding of
                // unchanged chunks and a revision rotation never reuses the
                // previous revision's vectors.
                let identity = EmbeddingCacheIdentityV1::of(&normalized);
                let cache = FileEmbeddingCache::new(
                    &state_root.join("embed-cache"),
                    &identity,
                    tuning.cache_retention,
                )?;
                let caching = Arc::new(CachingEmbeddingProvider::new(
                    Box::new(normalized),
                    Box::new(cache),
                ));
                let cache_source: Arc<dyn MetricSourcePort> = caching.clone();
                metric_sources.push(cache_source);
                caching
            } else {
                Arc::new(normalized)
            };
            Ok(SemanticEmbedders {
                query: Arc::new(QueryEmbedderAdapter(Arc::clone(&provider))),
                corpus: provider,
                metric_sources,
            })
        }
        SemanticEmbedderProfile::Unavailable => {
            let corpus: Arc<dyn TextEmbeddingProvider + Send + Sync> = Arc::new(
                HashingQueryTextEmbedder::new(SEARCH_OWNED_SEMANTIC_DIMENSION),
            );
            Ok(SemanticEmbedders {
                query: Arc::new(ProviderUnavailableQueryTextEmbedder),
                corpus,
                metric_sources: Vec::new(),
            })
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
            .structural_snapshot(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
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
    /// What boot inventoried, quarantined and proved (QI-BB-026).
    pub boot_inventory: BootInventoryReportV1,
    _search_corpus_lifecycle: Arc<SearchCorpusLifecycleOwner>,
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
            lexical_sealed_reclaim,
            lex_open_port,
            repo_commit_recency_ingest_port,
            repo_topic_ingest_port,
            repo_description_ingest_port,
            file_ownership_ingest_port,
            file_contributor_ingest_port,
            repo_meta_ingest_port,
            sem_build_port,
            semantic_generation_scanner,
            semantic_generation_validator,
            semantic_incomplete_discard,
            semantic_sealed_reclaim,
            sem_open_port,
            repo_map_query_port,
            repo_map_bundle_ingest_port,
            repo_map_generation_activate_port,
            lexical_quarantine_discard,
            semantic_quarantine_discard,
            repo_map_quarantine,
            repo_map_open_report,
            search_corpus_lifecycle,
            legacy_semantic_journal_store,
            idempotency,
            auxiliary_catalog,
            adapter_metric_sources,
        } = parts;
        state_root_lease
            .require_state_root_v1(config.state_root())
            .map_err(anyhow::Error::from)?;
        search_corpus_lifecycle
            .require_state_root_v1(config.state_root())
            .map_err(anyhow::Error::from)?;
        let leased_state_root = state_root_lease.state_root_identity_v1().to_path_buf();
        let activation_catalog = search_corpus_lifecycle.activation_catalog();
        let aux_authority_store = search_corpus_lifecycle.authority_store();
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        // Boot inventory (QI-BB-026): identities only, per track; nothing is
        // opened or hashed until the active pairs are proven below.
        let lexical_inventory = boot_inventory::seed_track_readiness(
            &ledger,
            SearchPlaneTrackKind::Lexical,
            lexical_generation_scanner.as_ref(),
        )?;
        let semantic_root = quanta_index_semantic::semantic_state_root(&leased_state_root);
        let migration_start = std::time::Instant::now();
        let migration = semantic_boot::migrate_legacy_semantic_journal(
            legacy_semantic_journal_store.as_ref(),
            sem_build_port.as_ref(),
            &semantic_root,
        )
        .map_err(anyhow::Error::from)?;
        let migration_micros = migration_start.elapsed().as_micros();
        let seed_start = std::time::Instant::now();
        let semantic_inventory = semantic_boot::seed_persisted_semantic_readiness(
            &ledger,
            semantic_generation_scanner.as_ref(),
        )
        .map_err(anyhow::Error::from)?;
        let boot_report = semantic_boot::SemanticBootReport {
            migration,
            migration_micros,
            seed: semantic_boot::SemanticSeedReport::from_track_report(&semantic_inventory),
            seed_micros: seed_start.elapsed().as_micros(),
        };
        // The only deep validation boot performs: each active pair, once. A
        // defective active generation fails here, typed, before any bind.
        let active_pairs_validated = search_corpus_lifecycle
            .validate_rehydrated_active_generations_v1(
                lexical_generation_validator.as_ref(),
                semantic_generation_validator.as_ref(),
            )
            .map_err(anyhow::Error::from)?;
        // The pre-catalog snapshot files, if this state root still has
        // them, move into the catalog once; then the auxiliary authorities
        // are rebuilt from the catalog's rows (QI-BB-020).
        let auxiliary_migration = aux_authority_store
            .migrate_legacy_auxiliary_snapshots(auxiliary_catalog.as_ref())
            .map_err(anyhow::Error::from)?;
        let auxiliary_rows_restored = {
            let mut guard = ledger.write().map_err(|err| {
                anyhow::anyhow!("ledger poisoned during auxiliary authority bootstrap: {err}")
            })?;
            aux_authority_store
                .restore_into(&mut guard)
                .map_err(anyhow::Error::from)?;
            quanta_index_search_plane::readiness::restore_auxiliary_rows_into(
                &mut guard,
                auxiliary_catalog.as_ref(),
            )
            .map_err(anyhow::Error::from)?
        };
        let boot_inventory = BootInventoryReportV1 {
            lexical: lexical_inventory,
            semantic: semantic_inventory,
            active_pairs_validated,
            auxiliary_migration,
            auxiliary_rows_restored,
            repo_map: repo_map_open_report,
        };
        let auxiliary_parts = AuxiliaryMaterializerParts {
            catalog: Arc::clone(&auxiliary_catalog),
            coordinator: AuxiliaryMutationCoordinator::shared(),
            ledger: Arc::clone(&ledger),
        };
        let SemanticEmbedders {
            query: query_text_embedder,
            corpus: corpus_embedder,
            metric_sources: embedder_metric_sources,
        } = build_semantic_embedders(config.semantic_embedder_profile(), &leased_state_root)
            .map_err(anyhow::Error::from)?;
        let direct_sem_ingest_port: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
            DirectSemanticMaterializer::new(Arc::clone(&sem_build_port), Arc::clone(&ledger)),
        );
        let snapshots = SnapshotRegistries::new(config.snapshot_registry_policy());
        let search_corpus_materializer: Arc<DirectSearchCorpusMaterializer> = Arc::new(
            DirectSearchCorpusMaterializer::new_with_search_owned_semantics_from_env(
                SearchCorpusMaterializerParts {
                    builder: Arc::clone(&search_corpus_build_port),
                    ledger: Arc::clone(&ledger),
                    semantic_ingest: Arc::clone(&direct_sem_ingest_port),
                    semantic_embedder: corpus_embedder,
                    authority: aux_authority_store,
                    lexical_generation_validator: Arc::clone(&lexical_generation_validator),
                    semantic_generation_validator: Arc::clone(&semantic_generation_validator),
                    lexical_incomplete_discard: Arc::clone(&lexical_incomplete_discard),
                    semantic_incomplete_discard: Arc::clone(&semantic_incomplete_discard),
                    lexical_reclaim: lexical_sealed_reclaim,
                    semantic_reclaim: semantic_sealed_reclaim,
                    snapshots: snapshots.clone(),
                    idempotency: Arc::clone(&idempotency),
                    resource_policy: config.ingest_resource_policy(),
                    auxiliary_catalog: Arc::clone(&auxiliary_parts.catalog),
                    auxiliary_coordinator: Arc::clone(&auxiliary_parts.coordinator),
                },
            )
            .map_err(anyhow::Error::from)?,
        );
        let direct_search_corpus_ingest_port: Arc<dyn SearchCorpusIngestPort + Send + Sync> =
            search_corpus_materializer.clone();
        let direct_history_ingest_port: Arc<dyn HistoryIngestPort + Send + Sync> =
            Arc::new(DirectHistoryMaterializer::new(auxiliary_parts.clone()));
        let direct_runtime_ingest_port: Arc<dyn RuntimeMetadataIngestPort + Send + Sync> = Arc::new(
            DirectRuntimeMetadataMaterializer::new(auxiliary_parts.clone()),
        );
        let direct_structural_ingest_port: Arc<dyn StructuralIngestPort + Send + Sync> =
            Arc::new(DirectStructuralMaterializer::new(auxiliary_parts));

        let query_obs_store = Arc::new(BoundedQueryObsStore::default());
        let query_obs_sink: Arc<dyn QueryObsSink + Send + Sync> =
            if std::env::var_os(BENCH_DISABLE_QUERY_OBS_ENV).is_some() {
                Arc::new(NoopQueryObsSink)
            } else {
                query_obs_store.clone()
            };
        // The metrics scrape (QI-BB-015): every source is registered here,
        // before any socket exists, so a scrape never sees a partial set.
        // The socket counters are created ahead of their servers for the
        // same reason.
        let query_counters = Arc::new(IpcServerCounters::for_plane("query"));
        let control_counters = Arc::new(IpcServerCounters::for_plane("control"));
        let ingest_counters = Arc::new(IpcServerCounters::for_plane("ingest"));
        let mut metric_sources: Vec<Arc<dyn MetricSourcePort>> = adapter_metric_sources;
        metric_sources.extend(embedder_metric_sources);
        for counters in [&query_counters, &control_counters, &ingest_counters] {
            let source: Arc<dyn MetricSourcePort> = counters.clone();
            metric_sources.push(source);
        }
        let snapshot_source: Arc<dyn MetricSourcePort> = Arc::new(snapshots.clone());
        metric_sources.push(snapshot_source);
        let ingest_source: Arc<dyn MetricSourcePort> = search_corpus_materializer;
        metric_sources.push(ingest_source);
        let boot_source: Arc<dyn MetricSourcePort> = Arc::new(boot_inventory.clone());
        metric_sources.push(boot_source);
        let observability = Arc::new(ObservabilityScrape::new(
            Arc::clone(&query_obs_store),
            metric_sources,
        ));
        let query_dispatcher = Arc::new(SearchPlaneDispatcher::new_with_obs(
            Arc::clone(&lex_open_port),
            Arc::clone(&sem_open_port),
            snapshots.clone(),
            Arc::clone(&repo_map_query_port),
            Arc::new(LedgerStructuralProducer::new(Arc::clone(&ledger))),
            Arc::clone(&ledger),
            activation_catalog.clone(),
            query_text_embedder,
            query_obs_sink,
        ));
        let quarantine = QuarantineService::new(QuarantineServiceParts {
            lexical_scanner: Arc::clone(&lexical_generation_scanner),
            semantic_scanner: Arc::clone(&semantic_generation_scanner),
            lexical_discard: lexical_quarantine_discard,
            semantic_discard: semantic_quarantine_discard,
            repo_map: repo_map_quarantine,
        });
        let control_dispatcher = Arc::new(SearchPlaneControlDispatcher::new(
            repo_map_generation_activate_port,
            activation_catalog,
            Arc::clone(&ledger),
            lexical_generation_validator,
            semantic_generation_validator,
            observability,
            quarantine,
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
            snapshots,
            idempotency,
        ));
        let query_adapter: Arc<
            dyn IpcDispatcher<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse>,
        > = Arc::new(SearchPlaneQueryIpcAdapter::new(query_dispatcher));
        let query_server = SearchPlaneQueryServer::bind(
            "quanta-index-query-uds",
            config.query_socket_path(),
            query_adapter,
            config.query_admission_policy(),
            query_counters,
        )
        .map_err(anyhow::Error::from)?;
        let control_adapter: Arc<
            dyn IpcDispatcher<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse>,
        > = Arc::new(SearchPlaneControlIpcAdapter::new(control_dispatcher));
        // Control and ingest mutate; their dispatches serialize by policy
        // while their connections still read independently (QI-BB-002).
        let control_server = SearchPlaneControlServer::bind(
            "quanta-index-control-uds",
            config.control_socket_path(),
            control_adapter,
            ServerAdmissionPolicy::SERIAL_DISPATCH,
            control_counters,
        )
        .map_err(anyhow::Error::from)?;
        let ingest_adapter: Arc<
            dyn IpcDispatcher<SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse>,
        > = Arc::new(SearchPlaneIngestIpcAdapter::new(ingest_dispatcher));
        let ingest_server = SearchPlaneIngestServer::bind(
            "quanta-index-ingest-uds",
            config.ingest_socket_path(),
            ingest_adapter,
            ServerAdmissionPolicy::SERIAL_DISPATCH,
            ingest_counters,
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
            boot_inventory,
            _search_corpus_lifecycle: search_corpus_lifecycle,
            _state_root_lease: state_root_lease,
        })
    }
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "Result-returning runtime tests assert with `assert!` on fixture invariants; a violated fixture invariant is not a propagatable error"
)]
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
            model_revision: "unit-test".to_string(),
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
        let super::SemanticEmbedders {
            query: query_embedder,
            corpus: corpus_embedder,
            ..
        } = super::build_semantic_embedders(&openai_profile(false), dir.path())
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
        if query_embedder.model_revision() != corpus_embedder.model_revision() {
            return Err(format!(
                "query/corpus model_revision drift: query={:?} corpus={:?}",
                query_embedder.model_revision(),
                corpus_embedder.model_revision()
            )
            .into());
        }
        if query_embedder.model_revision() != "unit-test" {
            return Err(format!(
                "the pinned revision must reach both halves, got {:?}",
                query_embedder.model_revision()
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
        let super::SemanticEmbedders {
            query: query_embedder,
            corpus: corpus_embedder,
            ..
        } = super::build_semantic_embedders(
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
        let super::SemanticEmbedders {
            query: query_embedder,
            corpus: corpus_embedder,
            ..
        } = super::build_semantic_embedders(&profile, dir.path())
            .map_err(|err| format!("hermetic hash embedder construction must succeed: {err:?}"))?;

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
                .map_err(|err| std::io::Error::other(format!("sync log poisoned: {err}")))?
                .push(path.to_path_buf());
            Ok(())
        })?;

        assert!(state_root.is_dir());
        assert_eq!(
            synced
                .into_inner()
                .map_err(|err| format!("sync log poisoned: {err}"))?,
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

    // QI-BB-014: the state root is private by construction and by check.
    #[cfg(unix)]
    #[test]
    fn a_created_state_root_is_private_and_a_writable_one_is_refused() -> TestRes {
        use std::os::unix::fs::PermissionsExt as _;
        let parent = tempfile::tempdir()?;
        let created = parent.path().join("fresh").join("state");
        let lease = super::StateRootLease::acquire(&created)?;
        let mode = std::fs::metadata(&created)?.permissions().mode() & 0o7777;
        assert_eq!(mode, 0o700, "a created state root is its owner's alone");
        drop(lease);

        let permissive = parent.path().join("permissive");
        std::fs::create_dir(&permissive)?;
        std::fs::set_permissions(&permissive, std::fs::Permissions::from_mode(0o777))?;
        let error = super::StateRootLease::acquire(&permissive)
            .expect_err("a state root others can write into must be refused");
        let quanta_index_core::CoreError::Typed { code, message } = error else {
            return Err(format!("expected a typed refusal, got {error:?}").into());
        };
        assert_eq!(code, "STATE_ROOT_INSECURE");
        assert!(message.contains("chmod go-w"), "{message}");

        // Readable by others is the operator's call; writable is not.
        let readable = parent.path().join("readable");
        std::fs::create_dir(&readable)?;
        std::fs::set_permissions(&readable, std::fs::Permissions::from_mode(0o755))?;
        let lease = super::StateRootLease::acquire(&readable)?;
        drop(lease);
        Ok(())
    }

    #[test]
    fn state_root_lease_rejects_a_foreign_runtime_root_v1() -> TestRes {
        let owned_root = tempfile::tempdir()?;
        let foreign_root = tempfile::tempdir()?;
        let lease = super::StateRootLease::acquire(owned_root.path())?;

        let rejected = lease.require_state_root_v1(foreign_root.path());
        let Err(quanta_index_core::CoreError::InvalidContract(message)) = rejected else {
            return Err("state-root lease accepted a foreign runtime root".into());
        };
        assert!(message.contains("identity mismatch"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn state_root_lease_refuses_symlink_lock_file_v1() -> TestRes {
        use std::os::unix::fs::symlink;

        let owned_root = tempfile::tempdir()?;
        let attacker_root = tempfile::tempdir()?;
        let attacker_target = attacker_root.path().join("attacker-lock");
        std::fs::write(&attacker_target, b"attacker-controlled")?;
        let lock_path = owned_root.path().join(".searchd-state-root.lock");
        symlink(&attacker_target, &lock_path)?;

        let result = super::StateRootLease::acquire(owned_root.path());
        let Err(quanta_index_core::CoreError::Storage(message)) = result else {
            return Err("state-root lease followed a symlink lock file".into());
        };
        assert!(message.contains(".searchd-state-root.lock"));
        Ok(())
    }
}
