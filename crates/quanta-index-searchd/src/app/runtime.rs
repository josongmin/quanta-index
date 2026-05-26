//! Composed runtime artefacts. Built once per daemon process.

use std::sync::{Arc, RwLock};
use std::{fs, path::Path};

use anyhow::Result;
use memchr::memchr_iter;
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
    LexicalBatchBuildPort, LexicalIndexOpenPort, LexicalIngestPort, RepoMapBundleIngestPort,
    RepoMapGenerationActivatePort, RepoMapQueryPort, SemanticBatchBuildPort, SemanticIndexOpenPort,
    SemanticIngestPort, StructuralError, StructuralMatchBinding, StructuralMatchCandidate,
    StructuralReadiness,
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
    ActivationCatalog, AuxiliaryAuthorityStore, BoundedQueryObsStore, DecimalQueryTextEmbedder,
    DirectHistoryMaterializer, DirectLexicalMaterializer, DirectRuntimeMetadataMaterializer,
    DirectSemanticMaterializer, DirectStructuralMaterializer, HistoryIngestPort, Ledger,
    QueryObsSink, RuntimeMetadataIngestPort, SearchPlaneControlDispatcher, SearchPlaneDispatcher,
    SearchPlaneIngestDispatcher, SemanticAuthorityStore, StructuralIngestPort,
};
use regex::Regex;

use crate::app::config::SearchdConfig;
use crate::app::ipc_dispatcher::{
    SearchPlaneControlIpcAdapter, SearchPlaneIngestIpcAdapter, SearchPlaneQueryIpcAdapter,
};
use crate::app::server::{
    SearchPlaneControlServer, SearchPlaneIngestServer, SearchPlaneQueryServer,
};

pub struct SearchdRuntimeParts {
    pub lex_build_port: Arc<dyn LexicalBatchBuildPort + Send + Sync>,
    pub lex_open_port: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    pub sem_build_port: Arc<dyn SemanticBatchBuildPort + Send + Sync>,
    pub sem_open_port: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    pub repo_map_query_port: Arc<dyn RepoMapQueryPort + Send + Sync>,
    pub repo_map_bundle_ingest_port: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    pub repo_map_generation_activate_port: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
    pub activation_catalog: Arc<ActivationCatalog>,
    pub aux_authority_store: Arc<AuxiliaryAuthorityStore>,
    pub semantic_authority_store: Arc<SemanticAuthorityStore>,
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
        let Ok(pin) = resolve_structural_pin(request) else {
            return StructuralReadiness::Ready;
        };
        let Ok(guard) = self.ledger.read() else {
            return StructuralReadiness::Ready;
        };
        let Some(state) =
            guard.structural_state(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
        else {
            return StructuralReadiness::GenerationNotReady;
        };
        if state.parse_trees().is_empty() {
            return StructuralReadiness::GenerationNotReady;
        }
        if state
            .parse_trees()
            .keys()
            .any(|chunk_id| !state.chunks().contains_key(chunk_id))
        {
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
        for (chunk_id, tree) in state.parse_trees() {
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
                    StructuralAuthorityView::new(chunk.indexed_text.as_ref(), tree),
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
    let text = chunk.indexed_text.as_ref();
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
}

impl SearchdRuntime {
    /// Assemble the runtime from externally-supplied ports.
    pub fn assemble(config: SearchdConfig, parts: SearchdRuntimeParts) -> Result<Self> {
        let SearchdRuntimeParts {
            lex_build_port,
            lex_open_port,
            sem_build_port,
            sem_open_port,
            repo_map_query_port,
            repo_map_bundle_ingest_port,
            repo_map_generation_activate_port,
            activation_catalog,
            aux_authority_store,
            semantic_authority_store,
        } = parts;
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        bootstrap_persisted_lexical_state(&ledger, config.state_root())?;
        bootstrap_persisted_semantic_state(
            &ledger,
            semantic_authority_store.as_ref(),
            sem_build_port.as_ref(),
        )?;
        {
            let mut guard = ledger.write().map_err(|err| {
                anyhow::anyhow!("ledger poisoned during auxiliary authority bootstrap: {err}")
            })?;
            aux_authority_store
                .restore_into(&mut guard)
                .map_err(anyhow::Error::from)?;
        }
        let direct_lex_ingest_port: Arc<dyn LexicalIngestPort + Send + Sync> = Arc::new(
            DirectLexicalMaterializer::new(Arc::clone(&lex_build_port), Arc::clone(&ledger)),
        );
        let direct_sem_ingest_port: Arc<dyn SemanticIngestPort + Send + Sync> =
            Arc::new(DirectSemanticMaterializer::new(
                semantic_authority_store,
                Arc::clone(&sem_build_port),
                Arc::clone(&ledger),
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
            Arc::new(DecimalQueryTextEmbedder),
            query_obs_sink,
        ));
        let control_dispatcher = Arc::new(SearchPlaneControlDispatcher::new(
            repo_map_generation_activate_port,
            activation_catalog,
            Arc::clone(&ledger),
        ));
        let ingest_dispatcher = Arc::new(SearchPlaneIngestDispatcher::new(
            direct_lex_ingest_port,
            direct_sem_ingest_port,
            direct_history_ingest_port,
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

fn bootstrap_persisted_semantic_state(
    ledger: &Arc<RwLock<Ledger>>,
    authority_store: &SemanticAuthorityStore,
    builder: &(dyn SemanticBatchBuildPort + Send + Sync),
) -> Result<()> {
    authority_store
        .replay_into(ledger, builder)
        .map_err(anyhow::Error::from)
}
