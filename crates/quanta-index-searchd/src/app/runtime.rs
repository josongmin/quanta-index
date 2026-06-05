//! Composed runtime artefacts. Built once per daemon process.

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};
use std::{fs, path::Path};

use anyhow::Result;
use memchr::memchr_iter;
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, GenerationSelector, LqFileScope, LqStructuralBlock,
    ManifestGeneration, RepoId, RevisionId, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcResponse, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, SearchPlaneTrackKind,
};
use quanta_index_core::domains::structural::{
    StructuralExecutableFilter, StructuralProducerPort,
    StructuralQueryRequest as DomainStructuralQueryRequest,
};
use quanta_index_core::{
    FileContributorIngestPort, FileOwnershipIngestPort, LexicalBatchBuildPort,
    LexicalIndexOpenPort, LexicalIngestPort, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMapBundleIngestPort, RepoMapGenerationActivatePort,
    RepoMapQueryPort, RepoMetaIngestPort, RepoTopicIngestPort, SemanticBatchBuildPort,
    SemanticIndexOpenPort, SemanticIngestPort, StructuralError, StructuralMatchBinding,
    StructuralMatchCandidate, StructuralReadiness,
};
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
    DirectLexicalMaterializer, DirectRuntimeMetadataMaterializer, DirectSemanticMaterializer,
    DirectStructuralMaterializer, HashingQueryTextEmbedder, HistoryIngestPort, Ledger,
    LegacySemanticJournalStore, QueryObsSink, QueryTextEmbedderPort, RuntimeMetadataIngestPort,
    SEARCH_OWNED_SEMANTIC_DIMENSION, SearchPlaneControlDispatcher, SearchPlaneDispatcher,
    SearchPlaneIngestDispatcher, StructuralIngestPort,
};
use regex::Regex;

use crate::app::config::{QueryTextEmbedderMode, SearchdConfig};
use crate::app::ipc_dispatcher::{
    SearchPlaneControlIpcAdapter, SearchPlaneIngestIpcAdapter, SearchPlaneQueryIpcAdapter,
};
use crate::app::semantic_boot;
use crate::app::server::{
    SearchPlaneControlServer, SearchPlaneIngestServer, SearchPlaneQueryServer,
};

pub struct SearchdRuntimeParts {
    pub lex_build_port: Arc<dyn LexicalBatchBuildPort + Send + Sync>,
    pub lex_open_port: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    pub repo_commit_recency_ingest_port: Arc<dyn RepoCommitRecencyIngestPort + Send + Sync>,
    pub repo_topic_ingest_port: Arc<dyn RepoTopicIngestPort + Send + Sync>,
    pub repo_description_ingest_port: Arc<dyn RepoDescriptionIngestPort + Send + Sync>,
    pub file_ownership_ingest_port: Arc<dyn FileOwnershipIngestPort + Send + Sync>,
    pub file_contributor_ingest_port: Arc<dyn FileContributorIngestPort + Send + Sync>,
    pub repo_meta_ingest_port: Arc<dyn RepoMetaIngestPort + Send + Sync>,
    pub sem_build_port: Arc<dyn SemanticBatchBuildPort + Send + Sync>,
    pub sem_open_port: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    pub repo_map_query_port: Arc<dyn RepoMapQueryPort + Send + Sync>,
    pub repo_map_bundle_ingest_port: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    pub repo_map_generation_activate_port: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
    pub activation_catalog: Arc<ActivationCatalog>,
    pub aux_authority_store: Arc<AuxiliaryAuthorityStore>,
    pub legacy_semantic_journal_store: Arc<LegacySemanticJournalStore>,
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
}

fn build_query_text_embedder(
    mode: QueryTextEmbedderMode,
) -> Arc<dyn QueryTextEmbedderPort + Send + Sync> {
    match mode {
        QueryTextEmbedderMode::DeterministicText => Arc::new(HashingQueryTextEmbedder::new(
            SEARCH_OWNED_SEMANTIC_DIMENSION,
        )),
        QueryTextEmbedderMode::ProviderUnavailable => {
            Arc::new(ProviderUnavailableQueryTextEmbedder)
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
}

impl SearchdRuntime {
    /// Assemble the runtime from externally-supplied ports.
    pub fn assemble(config: SearchdConfig, parts: SearchdRuntimeParts) -> Result<Self> {
        let SearchdRuntimeParts {
            lex_build_port,
            lex_open_port,
            repo_commit_recency_ingest_port,
            repo_topic_ingest_port,
            repo_description_ingest_port,
            file_ownership_ingest_port,
            file_contributor_ingest_port,
            repo_meta_ingest_port,
            sem_build_port,
            sem_open_port,
            repo_map_query_port,
            repo_map_bundle_ingest_port,
            repo_map_generation_activate_port,
            activation_catalog,
            aux_authority_store,
            legacy_semantic_journal_store,
        } = parts;
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        bootstrap_persisted_lexical_state(&ledger, config.state_root())?;
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
        let direct_sem_ingest_port: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
            DirectSemanticMaterializer::new(Arc::clone(&sem_build_port), Arc::clone(&ledger)),
        );
        let direct_lex_ingest_port: Arc<dyn LexicalIngestPort + Send + Sync> =
            Arc::new(DirectLexicalMaterializer::new_with_search_owned_semantics(
                Arc::clone(&lex_build_port),
                Arc::clone(&ledger),
                Arc::clone(&direct_sem_ingest_port),
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            ));
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
        let query_obs_sink: Arc<dyn QueryObsSink + Send + Sync> = query_obs_store.clone();
        let query_dispatcher = Arc::new(SearchPlaneDispatcher::new_with_obs(
            lex_open_port,
            sem_open_port,
            Arc::clone(&repo_map_query_port),
            Arc::new(LedgerStructuralProducer::new(Arc::clone(&ledger))),
            Arc::clone(&ledger),
            activation_catalog.clone(),
            build_query_text_embedder(config.query_text_embedder_mode()),
            query_obs_sink,
        ));
        let control_dispatcher = Arc::new(SearchPlaneControlDispatcher::new(
            repo_map_generation_activate_port,
            activation_catalog,
            Arc::clone(&ledger),
        ));
        let ingest_dispatcher = Arc::new(SearchPlaneIngestDispatcher::new(
            direct_lex_ingest_port,
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
        })
    }
}

fn bootstrap_persisted_lexical_state(
    ledger: &Arc<RwLock<Ledger>>,
    state_root: &Path,
) -> Result<()> {
    seed_persisted_lexical_readiness(ledger, state_root)?;
    Ok(())
}

fn seed_persisted_lexical_readiness(ledger: &Arc<RwLock<Ledger>>, state_root: &Path) -> Result<()> {
    let lexical_root = state_root.join("indexes").join("lexical");
    if !lexical_root.exists() {
        return Ok(());
    }
    let mut guard = ledger
        .write()
        .map_err(|err| anyhow::anyhow!("ledger poisoned during lexical bootstrap: {err}"))?;
    let mut max_generation: Option<ManifestGeneration> = None;
    for repo_entry in fs::read_dir(&lexical_root)? {
        let repo_entry = repo_entry?;
        if !repo_entry.file_type()?.is_dir() {
            continue;
        }
        let repo_id = RepoId::new(repo_entry.file_name().to_string_lossy().into_owned());
        for revision_entry in fs::read_dir(repo_entry.path())? {
            let revision_entry = revision_entry?;
            if !revision_entry.file_type()?.is_dir() {
                continue;
            }
            let revision_id =
                RevisionId::new(revision_entry.file_name().to_string_lossy().into_owned());
            for generation_entry in fs::read_dir(revision_entry.path())? {
                let generation_entry = generation_entry?;
                if !generation_entry.file_type()?.is_dir() {
                    continue;
                }
                let name = generation_entry.file_name().to_string_lossy().into_owned();
                let Some(suffix) = name.strip_prefix('g') else {
                    continue;
                };
                let Ok(raw_generation) = suffix.parse::<u64>() else {
                    continue;
                };
                let generation = ManifestGeneration::new(raw_generation);
                guard.record_track_materialized(
                    &repo_id,
                    &revision_id,
                    SearchPlaneTrackKind::Lexical,
                    generation,
                    None,
                );
                guard.record_track_seal(
                    &repo_id,
                    &revision_id,
                    SearchPlaneTrackKind::Lexical,
                    generation,
                );
                max_generation = match max_generation {
                    Some(current) if current.get() >= generation.get() => Some(current),
                    _ => Some(generation),
                };
            }
        }
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
        LedgerStructuralProducer, LqStructuralBlock, RepoId, RevisionId, StructuralProducerPort,
        StructuralReadiness,
    };
    use quanta_index_contract::LqOptions;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Arc, RwLock};
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
}
