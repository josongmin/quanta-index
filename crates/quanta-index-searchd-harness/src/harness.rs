//! E2E-00 — reusable tempdir-backed runtime harness.
//!
//! Parent harness for E2E-01..07. Owns a `TempDir` plus a lazily-started
//! searchd driver thread so a single test can: publish typed ingest batches
//! through the real ingest UDS frontdoor, seal a generation, drop+reopen
//! the runtime, then issue public query IPC requests and read typed responses
//! back. No in-memory shortcut: every byte goes through the same public daemon
//! surfaces the production runtime exposes.
//!
//! The harness intentionally does not add any new public API to
//! `quanta-index-searchd-runtime`. `reopen` is implemented by dropping
//! the current driver thread and reconstructing a fresh runtime over
//! the same `state_root`.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result as AnyResult;
use quanta_index_contract::lex::{
    LanguageCode, ParseNode, ParseRoleTag, ParseTreeRecord, SymbolKindCode, SymbolKindFamily,
    SymbolRecord, SymbolRelationship, SymbolSpan, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    BatchIngestMode, BatchPublishReceipt, CapabilityStatusV1, ChunkId, ChunkRecord,
    CurrentGenerationRequest, EngineTouched, FileOwnerProjectionRow, GenerationPin,
    GenerationSnapshot, HistoryCursor, HistoryQueryRequest, HybridQueryRequest, LexicalCandidate,
    ManifestGeneration, OwnerDocKind, QueryResultWindowV1, RawFallbackReasonV1, RepoId,
    RepoRelativePath, RevisionId, RuntimeMetadataQueryRequest, SearchCorpusGenerationIdentityV1,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchCorpusTombstoneScope,
    SearchExplanation, SearchPlaneActivateSearchCorpusGenerationCasRequest,
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneControlIpcResponse, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneExplainQueryRequest, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchPlaneTrackKind, SemanticCorpusKindV1,
    SemanticQueryRequest, SemanticSourceRecordV1, SemanticSourceReplaceScopeV1,
    SemanticSourceScopeKeyV1, SourceRoleV1, StructuralCandidate, StructuralIngestBatch,
    StructuralQueryRequest, StructuralReplaceScope, StructuralTreeRecord, SymbolId,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::IngestResourcePolicy;
use quanta_index_ipc::{ClientIoPolicy, IpcError, send_request};
use quanta_index_search_plane::{BoundedQueryObsStore, MetricSample, ObsError};
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd::app::{BootInventoryReportV1, SearchdConfig, SemanticEmbedderProfile};
use quanta_index_searchd_runtime::build_runtime;
use tempfile::TempDir;

#[derive(Clone, Debug)]
pub struct E2eRuntimeCatalogSpec {
    pub producer_head_applied_at_ms: u64,
    pub generation_materialized_at_ms: u64,
    pub changed: Vec<E2eRuntimeChangedSpec>,
    pub facets: Vec<E2eRuntimeFacetSpec>,
    pub snapshots: Vec<E2eRuntimeSnapshotSpec>,
    pub affected: Vec<E2eRuntimeEdgeSpec>,
    pub invalidated_by: Vec<E2eRuntimeEdgeSpec>,
}

#[derive(Clone, Debug)]
pub struct E2eRuntimeChangedSpec {
    pub path: String,
    pub applied_at_ms: u64,
}

#[derive(Clone, Debug)]
pub struct E2eRuntimeFacetSpec {
    pub path: String,
    pub owner: Option<String>,
    pub service: Option<String>,
    pub layer: Option<String>,
    pub surface: Option<String>,
}

#[derive(Clone, Debug)]
pub struct E2eRuntimeSnapshotSpec {
    pub name: String,
    pub paths: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct E2eRuntimeEdgeSpec {
    pub key: String,
    pub paths: Vec<String>,
}

static NEXT_SOCKET_ID: AtomicU64 = AtomicU64::new(0);
const READINESS_TIMEOUT: Duration = Duration::from_secs(15);
const SOCKET_APPEAR_TIMEOUT: Duration = Duration::from_secs(5);
const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(1);
const SOCKET_APPEAR_POLL_INTERVAL: Duration = Duration::from_millis(5);

type DriverJoin = thread::JoinHandle<AnyResult<()>>;
type DriverHandles = (
    PathBuf,
    PathBuf,
    PathBuf,
    Arc<AtomicBool>,
    DriverJoin,
    Arc<BoundedQueryObsStore>,
    BootInventoryReportV1,
);

fn structural_role_tags(
    root_end: u32,
    identifier_start: u32,
    identifier_end: u32,
    block_start: u32,
    block_end: u32,
) -> Vec<ParseRoleTag> {
    vec![
        ParseRoleTag {
            role: "item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: root_end,
        },
        ParseRoleTag {
            role: "expr".to_string().into_boxed_str(),
            byte_start: identifier_start,
            byte_end: identifier_end,
        },
        ParseRoleTag {
            role: "stmt".to_string().into_boxed_str(),
            byte_start: block_start,
            byte_end: block_end,
        },
    ]
}

/// Tempdir-backed runtime handle.
pub struct E2eRuntime {
    tempdir: Option<TempDir>,
    state_root: PathBuf,
    /// Embedder profile the lazily-started daemon is configured with.
    ///
    /// Default is `Hash` (deterministic, key-free, network-free) so existing
    /// callers and the CI relevance rail stay deterministic. A caller that wants
    /// a real-neural local A/B selects `OpenAi` via `boot_with_embedder_profile`.
    /// Held on the runtime (not mutated through process-global env) so two
    /// harness instances in one process can disagree on embedder.
    embedder_profile: SemanticEmbedderProfile,
    /// Sealed generations the daemon keeps per repo/revision pair. The
    /// harness default (8) is wide enough that ordinary tests never reap;
    /// GC tests narrow it through [`Self::boot_with_history_max_generations`].
    history_max_generations: usize,
    /// The resource envelope one search-corpus batch may ask the daemon to
    /// hold (QI-BB-021). The production default is far wider than any
    /// fixture; envelope tests tighten it through
    /// [`Self::boot_with_ingest_resource_policy`].
    ingest_resource_policy: IngestResourcePolicy,
    driver: Option<DriverState>,
    query_obs_store: Option<Arc<BoundedQueryObsStore>>,
    chunk_ids_by_path: BTreeMap<String, ChunkId>,
    chunk_records_by_path: BTreeMap<String, ChunkRecord>,
    request_id_counter: AtomicU64,
    generation_counter: u64,
    last_sealed_search_corpus_identity: Option<SearchCorpusGenerationIdentityV1>,
}

struct DriverState {
    query_socket: PathBuf,
    control_socket: PathBuf,
    ingest_socket: PathBuf,
    shutdown: Arc<AtomicBool>,
    join: Option<DriverJoin>,
    /// What this daemon start inventoried, quarantined and proved
    /// (QI-BB-026).
    boot_inventory: BootInventoryReportV1,
}

/// Test-only response shape.
///
/// Carries either the candidate list or a typed error code.
/// `engines_touched` is best-effort; the `Text` response variant has no
/// explanation today so this stays empty for plain text queries. Semantic and
/// hybrid rows populate it in later E2E tickets.
pub struct E2eQueryResult {
    pub candidates: Vec<LexicalCandidate>,
    pub candidate_ids: Vec<String>,
    pub file_owner_rows: Vec<FileOwnerProjectionRow>,
    pub structural_results: Vec<StructuralCandidate>,
    pub engines_touched: Vec<EngineTouched>,
    pub explanation: Option<SearchExplanation>,
    pub typed_error: Option<E2eTypedError>,
}

pub struct E2eHistoryResult {
    pub commit_ids: Vec<String>,
    pub diff_paths: Vec<String>,
    /// The page's window and continuation, when the daemon answered.
    pub window: Option<QueryResultWindowV1>,
    pub examined: u64,
    pub next_cursor: Option<HistoryCursor>,
    pub typed_error: Option<E2eTypedError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct E2eTypedError {
    pub code: String,
    pub message: String,
}

impl fmt::Display for E2eTypedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for E2eTypedError {}

/// One query route's answer, reduced to what a bounded-result contract asserts on.
///
/// Carries the typed refusal if the daemon refused, else the row count and the
/// wire window. Routes that answer without a window (history, runtime
/// metadata, structural) report `None` for it.
#[derive(Clone, Debug)]
pub struct E2eRouteWindowProbe {
    pub typed_error: Option<E2eTypedError>,
    pub returned_rows: usize,
    pub window: Option<QueryResultWindowV1>,
}

pub struct E2eExplainResult {
    pub explanation: Option<SearchExplanation>,
    pub typed_error: Option<E2eTypedError>,
}

pub struct E2eHistoryFixtureSpec<'a> {
    pub commit_sha: &'a str,
    pub file_path: &'a str,
    pub author: &'a str,
    pub committer: &'a str,
    pub message: &'a str,
    pub author_time_ms: u64,
    pub committer_time_ms: u64,
    pub applied_at_ms: u64,
    pub ref_name: &'a str,
    pub tag_name: &'a str,
    pub added_text: &'a str,
    pub removed_text: &'a str,
    pub touched_text: &'a str,
}

pub struct E2eTextChunkSpec<'a> {
    pub content: &'a str,
    pub start_line: u32,
    pub end_line: u32,
    pub source_repo_id: Option<&'a str>,
}

impl E2eRuntime {
    /// Create a fresh tempdir and an owned publisher. The driver is NOT
    /// started yet — it boots lazily on first `query_text`. The daemon under
    /// test uses the default `Hash` embedder profile.
    pub fn boot() -> AnyResult<Self> {
        Self::boot_with_embedder_profile(SemanticEmbedderProfile::default())
    }

    /// Like [`Self::boot`] but selects the daemon's semantic embedder profile
    /// explicitly (e.g. `OpenAi` for a local A/B run).
    ///
    /// The profile is recorded on the runtime and applied when the driver lazily
    /// starts, so this is the harness-owned override the relevance rail uses to
    /// run a deterministic `Hash` semantic gate in CI and a real-neural profile
    /// locally — without touching process-global env. Selecting `OpenAi`
    /// requires a key/network at query time; CI MUST stay on the `Hash` default.
    pub fn boot_with_embedder_profile(profile: SemanticEmbedderProfile) -> AnyResult<Self> {
        Self::boot_with_profile_and_history(profile, DEFAULT_HISTORY_MAX_GENERATIONS)
    }

    /// Like [`Self::boot`] but keeps only `max_generations` sealed
    /// generations per pair, so retention (and the physical GC behind it)
    /// runs inside a test instead of never.
    pub fn boot_with_history_max_generations(max_generations: usize) -> AnyResult<Self> {
        Self::boot_with_profile_and_history(SemanticEmbedderProfile::default(), max_generations)
    }

    /// Like [`Self::boot`] but runs the daemon under `policy` as its ingest
    /// resource envelope, so an envelope refusal can be provoked with a
    /// small batch instead of a hundred-thousand-record one.
    pub fn boot_with_ingest_resource_policy(policy: IngestResourcePolicy) -> AnyResult<Self> {
        let mut runtime = Self::boot()?;
        runtime.ingest_resource_policy = policy;
        Ok(runtime)
    }

    fn boot_with_profile_and_history(
        profile: SemanticEmbedderProfile,
        history_max_generations: usize,
    ) -> AnyResult<Self> {
        let tempdir = tempfile::tempdir()?;
        let state_root = tempdir.path().to_path_buf();
        Ok(Self {
            tempdir: Some(tempdir),
            state_root,
            embedder_profile: profile,
            history_max_generations,
            ingest_resource_policy: IngestResourcePolicy::DEFAULT,
            driver: None,
            query_obs_store: None,
            chunk_ids_by_path: BTreeMap::new(),
            chunk_records_by_path: BTreeMap::new(),
            request_id_counter: AtomicU64::new(1),
            generation_counter: 1,
            last_sealed_search_corpus_identity: None,
        })
    }

    /// The daemon's state root. Fault-injection tests use it to reach the
    /// adapters' on-disk layout directly; it is never a query-path input.
    #[must_use]
    pub fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// The embedder profile the (lazily-started) daemon is configured with.
    #[must_use]
    pub fn embedder_profile(&self) -> &SemanticEmbedderProfile {
        &self.embedder_profile
    }

    /// Stop the driver (if running) and reconstruct a publisher over the
    /// same `state_root` so further ingest is possible, then leave the
    /// driver stopped so first query lazy-starts a fresh runtime.
    /// Mirrors a process restart against persistent storage.
    #[must_use]
    #[expect(
        clippy::panic,
        reason = "the harness is test infrastructure; a failed daemon restart invalidates every assertion that follows and must abort the test rather than return a runtime the test would keep driving"
    )]
    pub fn reopen(mut self) -> Self {
        if let Err(error) = self.stop_driver() {
            panic!("e2e-harness: daemon process restart failed: {error:#}");
        }
        self
    }

    fn stop_driver(&mut self) -> AnyResult<()> {
        let outcome = if let Some(mut driver) = self.driver.take() {
            driver.shutdown.store(true, Ordering::Release);
            driver.join.take().map_or(Ok(()), |join| match join.join() {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => Err(anyhow::anyhow!(
                    "e2e-harness: daemon driver returned an error: {error:#}"
                )),
                Err(panic) => Err(anyhow::anyhow!(
                    "e2e-harness: daemon driver panicked: {panic:?}"
                )),
            })
        } else {
            Ok(())
        };
        self.query_obs_store = None;
        outcome
    }

    /// Start the daemon now instead of on the first query, surfacing a boot
    /// refusal (a defective active generation, for instance) as the error it
    /// is rather than as a failed query.
    pub fn start(&mut self) -> AnyResult<()> {
        drop(self.ensure_driver()?);
        Ok(())
    }

    /// The running daemon's boot inventory report, or `None` while the
    /// driver is stopped.
    #[must_use]
    pub fn boot_inventory(&self) -> Option<&BootInventoryReportV1> {
        self.driver.as_ref().map(|driver| &driver.boot_inventory)
    }

    fn ensure_driver(&mut self) -> AnyResult<PathBuf> {
        if self.driver.is_none() {
            let (
                query_socket,
                control_socket,
                ingest_socket,
                shutdown,
                join,
                query_obs_store,
                boot_inventory,
            ) = start_driver(
                &self.state_root,
                &self.embedder_profile,
                self.history_max_generations,
                self.ingest_resource_policy,
            )?;
            self.query_obs_store = Some(Arc::clone(&query_obs_store));
            self.driver = Some(DriverState {
                query_socket,
                control_socket,
                ingest_socket,
                shutdown,
                join: Some(join),
                boot_inventory,
            });
        }
        self.driver
            .as_ref()
            .map(|driver| driver.query_socket.clone())
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: driver missing after ensure_driver"))
    }

    fn ensure_control_socket(&mut self) -> AnyResult<PathBuf> {
        drop(self.ensure_driver()?);
        self.driver
            .as_ref()
            .map(|driver| driver.control_socket.clone())
            .ok_or_else(|| {
                anyhow::anyhow!("e2e-harness: control socket missing after ensure_driver")
            })
    }

    fn ensure_ingest_socket(&mut self) -> AnyResult<PathBuf> {
        drop(self.ensure_driver()?);
        self.driver
            .as_ref()
            .map(|driver| driver.ingest_socket.clone())
            .ok_or_else(|| {
                anyhow::anyhow!("e2e-harness: ingest socket missing after ensure_driver")
            })
    }

    pub fn repo(&self) -> RepoId {
        RepoId::new("repo-e2e")
    }

    pub fn revision(&self) -> RevisionId {
        RevisionId::new("rev-e2e")
    }

    /// Current generation pin. Stable until `seal()` is called, then
    /// advances on the next ingest.
    pub fn current_generation(&self) -> ManifestGeneration {
        ManifestGeneration::new(self.generation_counter)
    }

    pub fn generation_pin(&self) -> GenerationPin {
        GenerationPin::new(self.repo(), self.revision(), self.current_generation())
    }

    fn lexical_batch_contract(&self) -> (BatchIngestMode, Option<ManifestGeneration>) {
        if self.generation_counter <= 1 {
            (BatchIngestMode::ReplaceGeneration, None)
        } else {
            (
                BatchIngestMode::Delta,
                Some(ManifestGeneration::new(
                    self.generation_counter.saturating_sub(1),
                )),
            )
        }
    }

    pub fn query_metrics_snapshot(&self) -> AnyResult<Vec<MetricSample>> {
        let store = self.query_obs_store.as_ref().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: query metrics unavailable before driver startup")
        })?;
        Ok(store.snapshot())
    }

    pub fn query_metric_errors(&self) -> AnyResult<Vec<ObsError>> {
        let store = self.query_obs_store.as_ref().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: query metrics unavailable before driver startup")
        })?;
        Ok(store.errors())
    }

    /// Promote a sealed harness generation as one lexical plus semantic corpus.
    ///
    /// The candidate is reconstructed only from the validated sealed ingest
    /// receipt, then sent through the daemon's public control UDS. The
    /// daemon's current lexical plus semantic authority is reloaded before
    /// every CAS. The first activation observes no current authority and sends
    /// `None`; later activations never trust producer-cached active state.
    pub fn activate_last_sealed_generation(&mut self) -> AnyResult<()> {
        let candidate = self
            .last_sealed_search_corpus_identity
            .clone()
            .ok_or_else(|| {
                anyhow::anyhow!("e2e-harness: cannot activate before a sealed receipt is validated")
            })?;
        let expected_active = self.current_search_corpus_identity_from_control_v1(
            &candidate.lexical.repo_id,
            &candidate.lexical.revision_id,
        )?;
        let response = self.dispatch_control(
            SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
                SearchPlaneActivateSearchCorpusGenerationCasRequest {
                    candidate: candidate.clone(),
                    expected_active: expected_active.clone(),
                },
            ),
        )?;
        let SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(ack) = response else {
            return Err(anyhow::anyhow!(
                "e2e-harness: composite activation returned an unexpected control response"
            ));
        };
        if ack.active != candidate {
            return Err(anyhow::anyhow!(
                "e2e-harness: composite activation ack active identity differs from the sealed candidate"
            ));
        }
        if ack.previous_sealed_active != expected_active {
            return Err(anyhow::anyhow!(
                "e2e-harness: composite activation ack previous identity differs from the CAS expectation"
            ));
        }
        Ok(())
    }

    fn current_search_corpus_identity_from_control_v1(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> AnyResult<Option<SearchCorpusGenerationIdentityV1>> {
        let mut read_track =
            |track: SearchPlaneTrackKind| -> AnyResult<Option<GenerationSnapshot>> {
                let response = self.dispatch_control_response_v1(
                    SearchPlaneControlIpcRequest::CurrentGeneration(CurrentGenerationRequest {
                        repo_id: repo_id.clone(),
                        revision_id: revision_id.clone(),
                        track,
                    }),
                )?;
                match response {
                    SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(snapshot) => {
                        if snapshot.repo_id != *repo_id
                            || snapshot.revision_id != *revision_id
                            || snapshot.track != track
                        {
                            return Err(anyhow::anyhow!(
                                "e2e-harness: current generation response does not match requested authority"
                            ));
                        }
                        Ok(Some(snapshot))
                    }
                    SearchPlaneControlIpcResponse::Error(error) if error.code == "NOT_READY" => {
                        Ok(None)
                    }
                    SearchPlaneControlIpcResponse::Error(error) => Err(anyhow::anyhow!(
                        "e2e-harness current generation failed code={} message={}",
                        error.code,
                        error.message
                    )),
                    other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
                    | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
                    | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
                    | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
                        Err(anyhow::anyhow!(
                            "e2e-harness: current generation returned an unexpected control response: {other:?}"
                        ))
                    }
                }
            };

        let lexical = read_track(SearchPlaneTrackKind::Lexical)?;
        let semantic = read_track(SearchPlaneTrackKind::Semantic)?;
        match (lexical, semantic) {
            (None, None) => Ok(None),
            (Some(lexical), Some(semantic)) => {
                let identity = SearchCorpusGenerationIdentityV1 { lexical, semantic };
                identity.validate_v1().map_err(|error| {
                    anyhow::anyhow!(
                        "e2e-harness: daemon current search corpus identity is invalid: {error}"
                    )
                })?;
                Ok(Some(identity))
            }
            (lexical, semantic) => Err(anyhow::anyhow!(
                "e2e-harness: daemon current search corpus authority is split: lexical_present={} semantic_present={}",
                lexical.is_some(),
                semantic.is_some()
            )),
        }
    }

    /// Ingest one chunk through the typed ingest front door.
    ///
    /// `repo` is informational metadata only — the publish itself goes
    /// against the harness's owning `repo()` so the matching query can
    /// pin to a stable triple.
    pub fn ingest_text(&mut self, repo: &str, path: &str, content: &str) -> AnyResult<()> {
        let _candidate_id = self.ingest_text_with_candidate_id(repo, path, content)?;
        Ok(())
    }

    pub fn ingest_text_with_candidate_id(
        &mut self,
        repo: &str,
        path: &str,
        content: &str,
    ) -> AnyResult<String> {
        let mut ids = self.ingest_text_chunks(
            repo,
            path,
            &[E2eTextChunkSpec {
                content,
                start_line: 1,
                end_line: 2,
                source_repo_id: None,
            }],
        )?;
        ids.pop().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: no candidate id returned for path `{path}`")
        })
    }

    pub fn ingest_text_chunks(
        &mut self,
        _repo: &str,
        path: &str,
        chunks: &[E2eTextChunkSpec<'_>],
    ) -> AnyResult<Vec<String>> {
        if chunks.is_empty() {
            return Err(anyhow::anyhow!(
                "e2e-harness: ingest_text_chunks requires at least one chunk"
            ));
        }
        let language = LanguageCode::new(language_from_path(path)).map_err(|err| {
            anyhow::anyhow!("language_from_path must return canonical lowercase codes: {err}")
        })?;
        let records = chunks
            .iter()
            .map(|chunk| {
                let chunk_id = ChunkId::new(format!(
                    "e2e-{}-{path}",
                    self.request_id_counter.fetch_add(1, Ordering::Relaxed)
                ));
                let source_repo_id = chunk.source_repo_id.map(RepoId::new);
                let record = ChunkRecord {
                    chunk_id,
                    repo_relative_path: RepoRelativePath::new(path),
                    language: language.clone(),
                    start_byte: 0,
                    end_byte: u32::try_from(chunk.content.len()).map_err(|err| {
                        anyhow::anyhow!("e2e harness content length overflow: {err}")
                    })?,
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    text: chunk.content.to_string().into_boxed_str(),
                    structural: None,
                    parent_chunk_id: None,
                    source_repo_id,
                };
                Ok::<ChunkRecord, anyhow::Error>(record)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (mode, base_generation) = self.lexical_batch_contract();
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(
            SearchCorpusIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                base_generation,
                manifest_digest: format!("lex:{path}:{}", self.current_generation().get()),
                batch_digest: format!(
                    "lex-batch:{path}:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                mode,
                bundle_payload: None,
                clear_surfaces: Vec::new(),
                replace_scopes: vec![SearchCorpusReplaceScope {
                    scope: scope_key(path),
                    scope_digest: format!("scope:{path}:{}-chunks", records.len()),
                    chunks: records.clone(),
                    symbols: Vec::new(),
                }],
                tombstone_scopes: Vec::new(),
                semantic_replace_scopes: semantic_source_scopes_for_chunk_records(&records),
                semantic_tombstone_scopes: Vec::new(),
                seal: false,
            },
        ))?;
        let Some(last_record) = records.last().cloned() else {
            return Err(anyhow::anyhow!(
                "e2e-harness: ingest_text_chunks built no records for path `{path}`"
            ));
        };
        let _old = self
            .chunk_ids_by_path
            .insert(path.to_string(), last_record.chunk_id.clone());
        let _old = self
            .chunk_records_by_path
            .insert(path.to_string(), last_record);
        Ok(records
            .iter()
            .map(|record| record.chunk_id.as_str().to_string())
            .collect())
    }

    /// Ingest several files as ONE multi-scope corpus batch (one scope per file).
    ///
    /// This is the realistic shape of a production ingest wave: many files in a
    /// single batch. The semantic derivation then embeds the whole wave in one
    /// batched provider call instead of one call per file — the batching that the
    /// per-file `ingest_text*` helpers cannot exercise (each makes a single-scope
    /// batch). Returns the chunk ids across all files in ingest order.
    pub fn ingest_text_files_one_batch(
        &mut self,
        files: &[(&str, &[E2eTextChunkSpec<'_>])],
    ) -> AnyResult<Vec<String>> {
        if files.is_empty() {
            return Err(anyhow::anyhow!(
                "e2e-harness: ingest_text_files_one_batch requires at least one file"
            ));
        }
        let mut scopes = Vec::with_capacity(files.len());
        let mut all_ids = Vec::new();
        let mut all_records = Vec::new();
        for (path, chunks) in files {
            if chunks.is_empty() {
                return Err(anyhow::anyhow!(
                    "e2e-harness: file `{path}` requires at least one chunk"
                ));
            }
            let language = LanguageCode::new(language_from_path(path)).map_err(|err| {
                anyhow::anyhow!("language_from_path must return canonical lowercase codes: {err}")
            })?;
            let records = chunks
                .iter()
                .map(|chunk| {
                    let chunk_id = ChunkId::new(format!(
                        "e2e-{}-{path}",
                        self.request_id_counter.fetch_add(1, Ordering::Relaxed)
                    ));
                    let source_repo_id = chunk.source_repo_id.map(RepoId::new);
                    Ok::<ChunkRecord, anyhow::Error>(ChunkRecord {
                        chunk_id,
                        repo_relative_path: RepoRelativePath::new(*path),
                        language: language.clone(),
                        start_byte: 0,
                        end_byte: u32::try_from(chunk.content.len()).map_err(|err| {
                            anyhow::anyhow!("e2e harness content length overflow: {err}")
                        })?,
                        start_line: chunk.start_line,
                        end_line: chunk.end_line,
                        text: chunk.content.to_string().into_boxed_str(),
                        structural: None,
                        parent_chunk_id: None,
                        source_repo_id,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            for record in &records {
                all_ids.push(record.chunk_id.as_str().to_string());
            }
            all_records.extend(records.iter().cloned());
            if let Some(last_record) = records.last().cloned() {
                let _old = self
                    .chunk_ids_by_path
                    .insert((*path).to_string(), last_record.chunk_id.clone());
                let _old = self
                    .chunk_records_by_path
                    .insert((*path).to_string(), last_record);
            }
            scopes.push(SearchCorpusReplaceScope {
                scope: scope_key(path),
                scope_digest: format!("scope:{path}:{}-chunks", records.len()),
                chunks: records,
                symbols: Vec::new(),
            });
        }
        let (mode, base_generation) = self.lexical_batch_contract();
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(
            SearchCorpusIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                base_generation,
                manifest_digest: format!(
                    "lex-multi:{}:{}",
                    files.len(),
                    self.current_generation().get()
                ),
                batch_digest: format!(
                    "lex-multi-batch:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                mode,
                bundle_payload: None,
                clear_surfaces: Vec::new(),
                replace_scopes: scopes,
                tombstone_scopes: Vec::new(),
                semantic_replace_scopes: semantic_source_scopes_for_chunk_records(&all_records),
                semantic_tombstone_scopes: Vec::new(),
                seal: false,
            },
        ))?;
        Ok(all_ids)
    }

    pub fn publish_repo_metadata_bundle(&mut self, payload: Vec<u8>) -> AnyResult<()> {
        let (mode, base_generation) = self.lexical_batch_contract();
        let semantic_records = self
            .chunk_records_by_path
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let semantic_replace_scopes = semantic_source_scopes_for_chunk_records(&semantic_records);
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(
            SearchCorpusIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                base_generation,
                manifest_digest: format!("lex-meta:{}", self.current_generation().get()),
                batch_digest: format!(
                    "lex-meta-batch:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                mode,
                bundle_payload: Some(payload),
                clear_surfaces: Vec::new(),
                replace_scopes: Vec::new(),
                tombstone_scopes: Vec::new(),
                semantic_replace_scopes,
                semantic_tombstone_scopes: Vec::new(),
                seal: false,
            },
        ))?;
        Ok(())
    }

    pub fn publish_search_corpus_batch(&mut self, batch: SearchCorpusIngestBatch) -> AnyResult<()> {
        let sealed_authority = batch.seal.then(|| {
            (
                batch.repo_id.clone(),
                batch.revision_id.clone(),
                batch.generation,
                batch.manifest_digest.clone(),
            )
        });
        let response = self.dispatch_ingest_response(
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
        )?;
        if let Some((repo_id, revision_id, generation, manifest_digest)) = sealed_authority {
            let SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt) = response else {
                return Err(anyhow::anyhow!(
                    "e2e-harness: sealed search-corpus publish returned an unexpected ingest response"
                ));
            };
            if !receipt.sealed {
                return Err(anyhow::anyhow!(
                    "e2e-harness: sealed search-corpus publish receipt did not confirm a sealed generation"
                ));
            }
            if receipt.generation != generation {
                return Err(anyhow::anyhow!(
                    "e2e-harness: sealed search-corpus publish receipt generation differs from the request"
                ));
            }
            if receipt.manifest_digest.as_deref() != Some(manifest_digest.as_str()) {
                return Err(anyhow::anyhow!(
                    "e2e-harness: sealed search-corpus publish receipt manifest digest differs from the request"
                ));
            }
            self.last_sealed_search_corpus_identity =
                Some(self.search_corpus_identity_from_sealed_receipt(
                    repo_id,
                    revision_id,
                    generation,
                    &receipt,
                )?);
        }
        Ok(())
    }

    pub fn ingest_structural_function_tree(
        &mut self,
        path: &str,
        content: &str,
        identifier: &str,
    ) -> AnyResult<()> {
        let identifier_start = content.find(identifier).ok_or_else(|| {
            anyhow::anyhow!(
                "e2e-harness: identifier `{identifier}` not present in structural content"
            )
        })?;
        let identifier_end = identifier_start.saturating_add(identifier.len());
        let byte_end = u32::try_from(content.len())
            .map_err(|err| anyhow::anyhow!("e2e harness structural content overflow: {err}"))?;
        let identifier_start = u32::try_from(identifier_start)
            .map_err(|err| anyhow::anyhow!("e2e harness identifier start overflow: {err}"))?;
        let identifier_end = u32::try_from(identifier_end)
            .map_err(|err| anyhow::anyhow!("e2e harness identifier end overflow: {err}"))?;
        let block_start = byte_end.saturating_sub(2);
        let tree = ParseTreeRecord {
            wire_version: 1,
            lang: LanguageCode::new(language_from_path(path)).map_err(|err| {
                anyhow::anyhow!("language_from_path must return canonical lowercase codes: {err}")
            })?,
            root: ParseNode {
                kind: "function_item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end,
                children: vec![
                    ParseNode {
                        kind: "identifier".to_string().into_boxed_str(),
                        byte_start: identifier_start,
                        byte_end: identifier_end,
                        children: Vec::new(),
                    },
                    ParseNode {
                        kind: "block".to_string().into_boxed_str(),
                        byte_start: block_start,
                        byte_end,
                        children: Vec::new(),
                    },
                ],
            },
            source_hash: compute_parse_tree_source_hash(content),
            role_tag_schema_version: 1,
            role_tags: structural_role_tags(
                byte_end,
                identifier_start,
                identifier_end,
                block_start,
                byte_end,
            ),
        };
        self.ingest_structural_tree(path, tree)
    }

    pub fn ingest_structural_tree(&mut self, path: &str, tree: ParseTreeRecord) -> AnyResult<()> {
        let chunk_id = self.chunk_ids_by_path.get(path).cloned().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: no lexical chunk recorded for structural path `{path}`")
        })?;
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishStructuralBatch(
            StructuralIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                base_generation: None,
                manifest_digest: format!("struct:{path}:{}", self.current_generation().get()),
                batch_digest: format!(
                    "struct-batch:{path}:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                mode: BatchIngestMode::Delta,
                replace_scopes: vec![StructuralReplaceScope {
                    scope: scope_key(path),
                    scope_digest: format!("struct-scope:{path}"),
                    trees: vec![StructuralTreeRecord {
                        chunk_id,
                        record: tree,
                    }],
                }],
                tombstone_scopes: Vec::new(),
                seal: false,
            },
        ))?;
        Ok(())
    }

    pub fn ingest_history_fixture(&mut self, file_path: &str) -> AnyResult<()> {
        self.ingest_history_fixture_spec(&E2eHistoryFixtureSpec {
            commit_sha: "0123456789abcdef0123456789abcdef01234567",
            file_path,
            author: "alice",
            committer: "alice",
            message: "fix: sample history alpha_content_needle",
            author_time_ms: 11,
            committer_time_ms: 12,
            applied_at_ms: 13,
            ref_name: "refs/heads/main",
            tag_name: "v1.0.0",
            added_text: "history added line",
            removed_text: "",
            touched_text: "history touched line",
        })
    }

    pub fn ingest_history_fixture_spec(
        &mut self,
        spec: &E2eHistoryFixtureSpec<'_>,
    ) -> AnyResult<()> {
        use quanta_index_contract::lex::{CommitRecord, CommitSha, DiffHunkRecord};
        use quanta_index_contract::{
            DiffHunkSide, HistoryDiffHunkUpsert, HistoryIngestBatch, HistoryRefMutation,
            HistoryRefUpsert,
        };

        let commit_sha = CommitSha::from_hex(spec.commit_sha).map_err(|err| {
            anyhow::anyhow!(
                "e2e-harness: invalid history fixture commit_sha `{}`: {err}",
                spec.commit_sha
            )
        })?;
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishHistoryBatch(
            HistoryIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                manifest_digest: Some(format!(
                    "history:{}:{}",
                    spec.file_path,
                    self.current_generation().get()
                )),
                batch_digest: format!(
                    "history-batch:{}:{}",
                    spec.file_path,
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                commits: vec![CommitRecord {
                    wire_version: 1,
                    sha: commit_sha,
                    parents: Vec::new(),
                    author_time_ms: spec.author_time_ms,
                    committer_time_ms: spec.committer_time_ms,
                    applied_at_ms: spec.applied_at_ms,
                    author: spec.author.to_string().into_boxed_str(),
                    author_name: None,
                    author_email: None,
                    committer: spec.committer.to_string().into_boxed_str(),
                    committer_name: None,
                    committer_email: None,
                    message: spec.message.to_string().into_boxed_str(),
                    is_merge: false,
                    tags: vec![spec.tag_name.to_string().into_boxed_str()],
                }],
                refs: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                    name: spec.ref_name.to_string().into_boxed_str(),
                    sha: commit_sha,
                })],
                tags: vec![HistoryRefMutation::Upsert(HistoryRefUpsert {
                    name: spec.tag_name.to_string().into_boxed_str(),
                    sha: commit_sha,
                })],
                diff_hunks: vec![HistoryDiffHunkUpsert {
                    commit_sha,
                    file_path: spec.file_path.to_string().into_boxed_str(),
                    record: DiffHunkRecord {
                        wire_version: 1,
                        hunk_header: "@@ -1 +1 @@".to_string().into_boxed_str(),
                        side: DiffHunkSide::After,
                        added_text: spec.added_text.to_string().into_boxed_str(),
                        removed_text: spec.removed_text.to_string().into_boxed_str(),
                        touched_text: spec.touched_text.to_string().into_boxed_str(),
                        byte_start: 0,
                        byte_end: 20,
                    },
                }],
            },
        ))?;
        Ok(())
    }

    pub fn publish_history_batch(
        &mut self,
        batch: quanta_index_contract::HistoryIngestBatch,
    ) -> AnyResult<()> {
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch))
    }

    pub fn publish_repo_commit_recency_batch(
        &mut self,
        batch: quanta_index_contract::RepoCommitRecencyIngestBatch,
    ) -> AnyResult<()> {
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(
            batch,
        ))
    }

    pub fn publish_repo_meta_batch(
        &mut self,
        batch: quanta_index_contract::RepoMetaIngestBatch,
    ) -> AnyResult<()> {
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(batch))
    }

    pub fn publish_repo_topic_batch(
        &mut self,
        batch: quanta_index_contract::RepoTopicIngestBatch,
    ) -> AnyResult<()> {
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(batch))
    }

    pub fn publish_repo_description_batch(
        &mut self,
        batch: quanta_index_contract::RepoDescriptionIngestBatch,
    ) -> AnyResult<()> {
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(
            batch,
        ))
    }

    pub fn publish_file_ownership_batch(
        &mut self,
        batch: quanta_index_contract::FileOwnershipIngestBatch,
    ) -> AnyResult<()> {
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(
            batch,
        ))
    }

    pub fn publish_file_contributor_batch(
        &mut self,
        batch: quanta_index_contract::FileContributorIngestBatch,
    ) -> AnyResult<()> {
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishFileContributorBatch(
            batch,
        ))
    }

    pub fn publish_structural_batch(
        &mut self,
        batch: quanta_index_contract::StructuralIngestBatch,
    ) -> AnyResult<()> {
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishStructuralBatch(batch))
    }

    pub fn ingest_dirty_for_path(&mut self, path: &str, applied_at_ms: u64) -> AnyResult<()> {
        use quanta_index_contract::lex::DirtyRecord;
        use quanta_index_contract::{DirtyIngestBatch, DirtyMutation};

        let chunk_id = self.chunk_ids_by_path.get(path).cloned().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: no lexical chunk recorded for dirty path `{path}`")
        })?;
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishDirtyBatch(
            DirtyIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                overlay_epoch_ms: applied_at_ms,
                batch_digest: format!(
                    "dirty-batch:{path}:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                entries: vec![DirtyMutation::Upsert(DirtyRecord {
                    wire_version: 1,
                    doc_id: chunk_id,
                    applied_at_ms,
                    payload_hash: [0x5a; 32],
                })],
            },
        ))?;
        Ok(())
    }

    pub fn ingest_runtime_catalog(&mut self, catalog: &E2eRuntimeCatalogSpec) -> AnyResult<()> {
        use quanta_index_contract::{
            RuntimeCatalogIngestBatch, RuntimeChangedRecord, RuntimeDocFacetRecord,
            RuntimeEdgeAuthorityRecord, RuntimeSnapshotRecord,
        };

        let mut changed_entries = Vec::with_capacity(catalog.changed.len());
        for changed in &catalog.changed {
            let chunk_id = self
                .chunk_ids_by_path
                .get(&changed.path)
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "e2e-harness: no lexical chunk recorded for runtime changed path `{}`",
                        changed.path
                    )
                })?;
            changed_entries.push(RuntimeChangedRecord {
                doc_id: chunk_id,
                applied_at_ms: changed.applied_at_ms,
                payload_hash: [0xaa; 32],
            });
        }
        let mut facet_entries = Vec::with_capacity(catalog.facets.len());
        for facet in &catalog.facets {
            let chunk_id = self
                .chunk_ids_by_path
                .get(&facet.path)
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "e2e-harness: no lexical chunk recorded for runtime facet path `{}`",
                        facet.path
                    )
                })?;
            facet_entries.push(RuntimeDocFacetRecord {
                doc_id: chunk_id,
                owner: facet.owner.clone(),
                service: facet.service.clone(),
                layer: facet.layer.clone(),
                surface: facet.surface.clone(),
            });
        }
        let mut snapshot_entries = Vec::with_capacity(catalog.snapshots.len());
        for snapshot in &catalog.snapshots {
            let mut doc_ids = Vec::with_capacity(snapshot.paths.len());
            for path in &snapshot.paths {
                let chunk_id = self.chunk_ids_by_path.get(path).cloned().ok_or_else(|| {
                    anyhow::anyhow!(
                        "e2e-harness: no lexical chunk recorded for runtime snapshot path `{path}`"
                    )
                })?;
                doc_ids.push(chunk_id);
            }
            snapshot_entries.push(RuntimeSnapshotRecord {
                name: snapshot.name.clone(),
                doc_ids,
            });
        }
        let mut affected_entries = Vec::with_capacity(catalog.affected.len());
        for edge in &catalog.affected {
            let mut doc_ids = Vec::with_capacity(edge.paths.len());
            for path in &edge.paths {
                let chunk_id = self.chunk_ids_by_path.get(path).cloned().ok_or_else(|| {
                    anyhow::anyhow!(
                        "e2e-harness: no lexical chunk recorded for runtime affected path `{path}`"
                    )
                })?;
                doc_ids.push(chunk_id);
            }
            affected_entries.push(RuntimeEdgeAuthorityRecord {
                key: edge.key.clone(),
                doc_ids,
            });
        }
        let mut invalidated_by_entries = Vec::with_capacity(catalog.invalidated_by.len());
        for edge in &catalog.invalidated_by {
            let mut doc_ids = Vec::with_capacity(edge.paths.len());
            for path in &edge.paths {
                let chunk_id = self.chunk_ids_by_path.get(path).cloned().ok_or_else(|| {
                    anyhow::anyhow!(
                        "e2e-harness: no lexical chunk recorded for runtime invalidated_by path `{path}`"
                    )
                })?;
                doc_ids.push(chunk_id);
            }
            invalidated_by_entries.push(RuntimeEdgeAuthorityRecord {
                key: edge.key.clone(),
                doc_ids,
            });
        }
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(
            RuntimeCatalogIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                overlay_epoch_ms: catalog.generation_materialized_at_ms,
                batch_digest: format!(
                    "runtime-catalog:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                producer_head_applied_at_ms: catalog.producer_head_applied_at_ms,
                generation_materialized_at_ms: catalog.generation_materialized_at_ms,
                changed_entries,
                facet_entries,
                snapshot_entries,
                affected_entries,
                invalidated_by_entries,
            },
        ))?;
        Ok(())
    }

    pub fn evict_dirty_for_path(&mut self, path: &str) -> AnyResult<()> {
        use quanta_index_contract::{DirtyDelete, DirtyIngestBatch, DirtyMutation};

        let chunk_id = self.chunk_ids_by_path.get(path).cloned().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: no lexical chunk recorded for dirty path `{path}`")
        })?;
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishDirtyBatch(
            DirtyIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                overlay_epoch_ms: 0,
                batch_digest: format!(
                    "dirty-evict:{path}:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                entries: vec![DirtyMutation::Delete(DirtyDelete { doc_id: chunk_id })],
            },
        ))?;
        Ok(())
    }

    pub fn tombstone_structural_for_path(&mut self, path: &str) -> AnyResult<()> {
        use quanta_index_contract::StructuralTombstoneScope;

        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishStructuralBatch(
            StructuralIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                base_generation: None,
                manifest_digest: format!("struct-del:{path}:{}", self.current_generation().get()),
                batch_digest: format!(
                    "struct-del-batch:{path}:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                mode: BatchIngestMode::Delta,
                replace_scopes: Vec::new(),
                tombstone_scopes: vec![StructuralTombstoneScope {
                    scope: scope_key(path),
                }],
                seal: false,
            },
        ))?;
        Ok(())
    }

    pub fn delete_chunk_for_path(&mut self, path: &str) -> AnyResult<()> {
        let (mode, base_generation) = self.lexical_batch_contract();
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(
            SearchCorpusIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                base_generation,
                manifest_digest: format!("lex-del:{path}:{}", self.current_generation().get()),
                batch_digest: format!(
                    "lex-del-batch:{path}:{}",
                    self.request_id_counter.load(Ordering::Relaxed)
                ),
                mode,
                bundle_payload: None,
                clear_surfaces: Vec::new(),
                replace_scopes: Vec::new(),
                tombstone_scopes: vec![SearchCorpusTombstoneScope {
                    scope: scope_key(path),
                }],
                semantic_replace_scopes: Vec::new(),
                semantic_tombstone_scopes: Vec::new(),
                seal: false,
            },
        ))?;
        drop(self.chunk_records_by_path.remove(path));
        Ok(())
    }

    pub fn ingest_symbol(
        &mut self,
        _repo: &str,
        path: &str,
        symbol_id: &str,
        symbol_name: &str,
    ) -> AnyResult<()> {
        let record = SymbolRecord {
            symbol_id: SymbolId::new(symbol_id),
            repo_relative_path: RepoRelativePath::new(path),
            language: LanguageCode::new(language_from_path(path)).map_err(|err| {
                anyhow::anyhow!("language_from_path must return canonical lowercase codes: {err}")
            })?,
            symbol_kind: SymbolKindCode::new("function")
                .map_err(|err| anyhow::anyhow!("invalid test symbol kind: {err}"))?,
            symbol_kind_family: Some(SymbolKindFamily::Callable),
            local_name: symbol_name.to_string().into_boxed_str(),
            qualified_name: format!("crate::{symbol_name}").into_boxed_str(),
            signature: None,
            visibility: None,
            definition_span: SymbolSpan {
                path: path.to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: u32::try_from(symbol_name.len())
                    .map_err(|err| anyhow::anyhow!("e2e harness symbol length overflow: {err}"))?,
                line_start: 1,
                line_end: 1,
            },
            container_qualified_name: Some("crate".to_string().into_boxed_str()),
            relationship: SymbolRelationship::Def,
        };
        let chunks = self
            .chunk_records_by_path
            .get(path)
            .cloned()
            .into_iter()
            .collect::<Vec<_>>();
        let semantic_replace_scopes = semantic_source_scopes_for_chunk_records(&chunks);
        let (mode, base_generation) = self.lexical_batch_contract();
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(
            SearchCorpusIngestBatch {
                repo_id: self.repo(),
                revision_id: self.revision(),
                generation: self.current_generation(),
                base_generation,
                manifest_digest: format!("lex-symbol:{path}:{}", self.current_generation().get()),
                batch_digest: format!("lex-symbol-batch:{path}:{symbol_id}"),
                mode,
                bundle_payload: None,
                clear_surfaces: Vec::new(),
                replace_scopes: vec![SearchCorpusReplaceScope {
                    scope: scope_key(path),
                    scope_digest: format!("scope-symbol:{path}:{symbol_id}"),
                    chunks,
                    symbols: vec![record],
                }],
                tombstone_scopes: Vec::new(),
                semantic_replace_scopes,
                semantic_tombstone_scopes: Vec::new(),
                seal: false,
            },
        ))?;
        Ok(())
    }

    /// Seal the current generation. Returns the sealed `ManifestGeneration`
    /// then advances the harness's pin so subsequent ingests target the
    /// next generation.
    pub fn seal(&mut self) -> AnyResult<ManifestGeneration> {
        self.seal_lexical_generation_for_tracks(&[SearchPlaneTrackKind::Lexical])
    }

    /// Seal the current generation through the search-corpus ingest surface.
    ///
    /// There is no separate structural or semantic seal IPC. Those tracks
    /// become ready only after their authority has been ingested and the
    /// lexical track for the same generation has been sealed/activated.
    pub fn seal_lexical_generation_for_tracks(
        &mut self,
        tracks: &[SearchPlaneTrackKind],
    ) -> AnyResult<ManifestGeneration> {
        let sealed = self.current_generation();
        if tracks.is_empty() {
            return Err(anyhow::anyhow!(
                "e2e-harness: seal helper requires at least the lexical track"
            ));
        }
        if !tracks.contains(&SearchPlaneTrackKind::Lexical) {
            return Err(anyhow::anyhow!(
                "e2e-harness: structural/semantic readiness piggybacks on lexical seal; include SearchPlaneTrackKind::Lexical"
            ));
        }
        if tracks.contains(&SearchPlaneTrackKind::Lexical) {
            let (mode, base_generation) = self.lexical_batch_contract();
            let manifest_digest = format!("lex-seal:{}", sealed.get());
            let response = self.dispatch_ingest_response(
                SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(SearchCorpusIngestBatch {
                    repo_id: self.repo(),
                    revision_id: self.revision(),
                    generation: sealed,
                    base_generation,
                    manifest_digest: manifest_digest.clone(),
                    batch_digest: format!("lex-seal-batch:{}", sealed.get()),
                    mode,
                    bundle_payload: None,
                    clear_surfaces: Vec::new(),
                    replace_scopes: Vec::new(),
                    tombstone_scopes: Vec::new(),
                    semantic_replace_scopes: Vec::new(),
                    semantic_tombstone_scopes: Vec::new(),
                    seal: true,
                }),
            )?;
            let SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt) = response else {
                return Err(anyhow::anyhow!(
                    "e2e-harness: seal returned an unexpected ingest response"
                ));
            };
            if !receipt.sealed {
                return Err(anyhow::anyhow!(
                    "e2e-harness: seal receipt did not confirm a sealed generation"
                ));
            }
            if receipt.generation != sealed {
                return Err(anyhow::anyhow!(
                    "e2e-harness: seal receipt generation differs from the request"
                ));
            }
            if receipt.manifest_digest.as_deref() != Some(manifest_digest.as_str()) {
                return Err(anyhow::anyhow!(
                    "e2e-harness: seal receipt manifest digest differs from the request"
                ));
            }
            if receipt.accepted_clear_surfaces != 0
                || receipt.accepted_replace_scopes != 0
                || receipt.accepted_tombstone_scopes != 0
            {
                return Err(anyhow::anyhow!(
                    "e2e-harness: empty seal receipt reported accepted mutations"
                ));
            }
            let identity = self.search_corpus_identity_from_sealed_receipt(
                self.repo(),
                self.revision(),
                sealed,
                &receipt,
            )?;
            self.last_sealed_search_corpus_identity = Some(identity);
        }
        self.generation_counter = self.generation_counter.saturating_add(1);
        Ok(sealed)
    }

    /// Issue a `TextQueryRequest` against the live socket, lazy-starting
    /// the driver on first call. The query is pinned to whatever
    /// generation was most recently sealed (i.e. `current_generation() -
    /// 1`), matching how production callers pin queries to a sealed
    /// manifest.
    pub fn query_text(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
    ) -> E2eQueryResult {
        let pin = self.last_sealed_pin();
        self.query_text_with_pin(syntax, query_text, top_k, pin)
    }

    pub fn query_structural(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
    ) -> E2eQueryResult {
        let pin = self.last_sealed_pin();
        self.query_structural_with_pin(syntax, query_text, top_k, pin)
    }

    pub fn query_history(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
    ) -> E2eHistoryResult {
        self.query_history_page(syntax, query_text, top_k, None)
    }

    /// One history page after `cursor` (QI-BB-023).
    pub fn query_history_page(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
        cursor: Option<HistoryCursor>,
    ) -> E2eHistoryResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::History(HistoryQueryRequest {
                text_query: TextQueryRequest {
                    syntax,
                    query_text: query_text.to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: self.last_sealed_pin(),
                    generation_selector: None,
                    top_k,
                },
                cursor,
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eHistoryResult {
                    commit_ids: Vec::new(),
                    diff_paths: Vec::new(),
                    window: None,
                    examined: 0,
                    next_cursor: None,
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        let (readiness_reached, response) =
            wait_for_query_response(&socket, &envelope, query_response_ready);
        let response: SearchPlaneQueryIpcResponseEnvelope = match response {
            Ok(r) => r,
            Err(err) => {
                let query_result = self.semantic_transport_error(readiness_reached, err);
                return E2eHistoryResult {
                    commit_ids: Vec::new(),
                    diff_paths: Vec::new(),
                    window: None,
                    examined: 0,
                    next_cursor: None,
                    typed_error: query_result.typed_error,
                };
            }
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::History(history) => E2eHistoryResult {
                commit_ids: history
                    .commits
                    .into_iter()
                    .map(|candidate| candidate.sha.to_string())
                    .collect(),
                diff_paths: history
                    .diffs
                    .into_iter()
                    .map(|candidate| candidate.repo_relative_path.as_str().to_string())
                    .collect(),
                window: Some(history.window),
                examined: history.examined,
                next_cursor: history.next_cursor,
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eHistoryResult {
                commit_ids: Vec::new(),
                diff_paths: Vec::new(),
                window: None,
                examined: 0,
                next_cursor: None,
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Text(_) => unexpected_history_response("Text"),
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_history_response("Symbol"),
            SearchPlaneQueryIpcResponse::Semantic(_) => unexpected_history_response("Semantic"),
            SearchPlaneQueryIpcResponse::Hybrid(_) => unexpected_history_response("Hybrid"),
            SearchPlaneQueryIpcResponse::HybridSeed(_) => unexpected_history_response("HybridSeed"),
            SearchPlaneQueryIpcResponse::Structural(_) => unexpected_history_response("Structural"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => {
                unexpected_history_response("RepoMapQuery")
            }
            SearchPlaneQueryIpcResponse::Explain(_) => unexpected_history_response("Explain"),
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => {
                unexpected_history_response("ClusterMembershipRead")
            }
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                unexpected_history_response("RuntimeMetadata")
            }
        }
    }

    pub fn query_runtime_metadata(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
    ) -> E2eQueryResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
                text_query: TextQueryRequest {
                    syntax,
                    query_text: query_text.to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: self.last_sealed_pin(),
                    generation_selector: None,
                    top_k,
                },
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eQueryResult {
                    candidates: Vec::new(),
                    candidate_ids: Vec::new(),
                    file_owner_rows: Vec::new(),
                    structural_results: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        let (readiness_reached, response) =
            wait_for_query_response(&socket, &envelope, query_response_ready);
        let response: SearchPlaneQueryIpcResponseEnvelope = match response {
            Ok(r) => r,
            Err(err) => return self.semantic_transport_error(readiness_reached, err),
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::RuntimeMetadata(runtime) => E2eQueryResult {
                candidate_ids: runtime
                    .results
                    .iter()
                    .map(|candidate| candidate.candidate_id.clone())
                    .collect(),
                candidates: runtime.results,
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Text(_) => unexpected_response("Text"),
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_response("Symbol"),
            SearchPlaneQueryIpcResponse::Semantic(_) => unexpected_response("Semantic"),
            SearchPlaneQueryIpcResponse::Hybrid(_) => unexpected_response("Hybrid"),
            SearchPlaneQueryIpcResponse::HybridSeed(_) => unexpected_response("HybridSeed"),
            SearchPlaneQueryIpcResponse::History(_) => unexpected_response("History"),
            SearchPlaneQueryIpcResponse::Structural(_) => unexpected_response("Structural"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => unexpected_response("RepoMapQuery"),
            SearchPlaneQueryIpcResponse::Explain(_) => unexpected_response("Explain"),
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => {
                unexpected_response("ClusterMembershipRead")
            }
        }
    }

    /// Variant that lets a self-test exercise the "no generation pin"
    /// invalid-contract path explicitly.
    pub fn query_text_with_pin(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
        pin: Option<GenerationPin>,
    ) -> E2eQueryResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax,
                query_text: query_text.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: pin,
                generation_selector: None,
                top_k,
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eQueryResult {
                    candidates: Vec::new(),
                    candidate_ids: Vec::new(),
                    file_owner_rows: Vec::new(),
                    structural_results: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        // Wait for the runtime to leave NOT_READY before reading the real
        // payload — mirrors the readiness wait every existing dsl_scenarios
        // test does. For an invalid-contract request (no pin), the runtime
        // returns INVALID_REQUEST immediately, which already satisfies the
        // "non-NOT_READY" predicate.
        let (readiness_reached, response) = wait_for_query_response(
            &socket,
            &envelope,
            query_response_ready_allow_structural_not_ready,
        );
        let response: SearchPlaneQueryIpcResponseEnvelope = match response {
            Ok(r) => r,
            Err(err) => return self.semantic_transport_error(readiness_reached, err),
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Text(text) => E2eQueryResult {
                candidate_ids: text
                    .results
                    .iter()
                    .map(|c| c.candidate_id.clone())
                    .collect(),
                file_owner_rows: text.file_owner_rows.unwrap_or_default(),
                candidates: text.results,
                structural_results: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_response("Symbol"),
            SearchPlaneQueryIpcResponse::Semantic(_) => unexpected_response("Semantic"),
            SearchPlaneQueryIpcResponse::Hybrid(_) => unexpected_response("Hybrid"),
            SearchPlaneQueryIpcResponse::HybridSeed(_) => unexpected_response("HybridSeed"),
            SearchPlaneQueryIpcResponse::History(_) => unexpected_response("History"),
            SearchPlaneQueryIpcResponse::Structural(_) => unexpected_response("Structural"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => unexpected_response("RepoMapQuery"),
            SearchPlaneQueryIpcResponse::Explain(_) => unexpected_response("Explain"),
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => {
                unexpected_response("ClusterMembershipRead")
            }
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                unexpected_response("RuntimeMetadata")
            }
        }
    }

    pub fn query_structural_with_pin(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
        pin: Option<GenerationPin>,
    ) -> E2eQueryResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax,
                    query_text: query_text.to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: pin,
                    generation_selector: None,
                    top_k,
                },
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eQueryResult {
                    candidates: Vec::new(),
                    candidate_ids: Vec::new(),
                    file_owner_rows: Vec::new(),
                    structural_results: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        let (readiness_reached, response) = wait_for_query_response(
            &socket,
            &envelope,
            query_response_ready_allow_structural_not_ready,
        );
        let response: SearchPlaneQueryIpcResponseEnvelope = match response {
            Ok(r) => r,
            Err(err) => return self.semantic_transport_error(readiness_reached, err),
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Structural(structural) => {
                let results = structural.results;
                E2eQueryResult {
                    candidate_ids: results
                        .iter()
                        .map(|candidate| candidate.candidate_id.clone())
                        .collect(),
                    candidates: Vec::new(),
                    file_owner_rows: Vec::new(),
                    structural_results: results,
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: None,
                }
            }
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Text(_) => unexpected_response("Text"),
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_response("Symbol"),
            SearchPlaneQueryIpcResponse::Semantic(_) => unexpected_response("Semantic"),
            SearchPlaneQueryIpcResponse::Hybrid(_) => unexpected_response("Hybrid"),
            SearchPlaneQueryIpcResponse::HybridSeed(_) => unexpected_response("HybridSeed"),
            SearchPlaneQueryIpcResponse::History(_) => unexpected_response("History"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => unexpected_response("RepoMapQuery"),
            SearchPlaneQueryIpcResponse::Explain(_) => unexpected_response("Explain"),
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => {
                unexpected_response("ClusterMembershipRead")
            }
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                unexpected_response("RuntimeMetadata")
            }
        }
    }

    pub fn query_semantic(
        &mut self,
        query_text: &str,
        top_k: u32,
        lexical_scope: Option<(TextQuerySyntax, &str, u32)>,
    ) -> E2eQueryResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: query_text.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: self.last_sealed_pin(),
                generation_selector: None,
                lexical_scope: lexical_scope.map(|(syntax, query_text, scope_top_k)| {
                    TextQueryRequest {
                        syntax,
                        query_text: query_text.to_string(),
                        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                        generation: self.last_sealed_pin(),
                        generation_selector: None,
                        top_k: scope_top_k,
                    }
                }),
                top_k,
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eQueryResult {
                    candidates: Vec::new(),
                    candidate_ids: Vec::new(),
                    file_owner_rows: Vec::new(),
                    structural_results: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        let (readiness_reached, response) =
            wait_for_query_response(&socket, &envelope, query_response_ready);
        let response: SearchPlaneQueryIpcResponseEnvelope = match response {
            Ok(r) => r,
            Err(err) => return self.semantic_transport_error(readiness_reached, err),
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Semantic(semantic) => E2eQueryResult {
                candidate_ids: semantic
                    .results
                    .iter()
                    .map(|c| c.candidate_id.clone())
                    .collect(),
                candidates: semantic.results,
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                engines_touched: semantic.explanation.engines_touched.clone(),
                explanation: Some(semantic.explanation),
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Text(_) => unexpected_response("Text"),
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_response("Symbol"),
            SearchPlaneQueryIpcResponse::Hybrid(_) => unexpected_response("Hybrid"),
            SearchPlaneQueryIpcResponse::HybridSeed(_) => unexpected_response("HybridSeed"),
            SearchPlaneQueryIpcResponse::History(_) => unexpected_response("History"),
            SearchPlaneQueryIpcResponse::Structural(_) => unexpected_response("Structural"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => unexpected_response("RepoMapQuery"),
            SearchPlaneQueryIpcResponse::Explain(_) => unexpected_response("Explain"),
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => {
                unexpected_response("ClusterMembershipRead")
            }
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                unexpected_response("RuntimeMetadata")
            }
        }
    }

    pub fn query_hybrid(
        &mut self,
        syntax: TextQuerySyntax,
        text_query: &str,
        semantic_query_text: &str,
        top_k: u32,
    ) -> E2eQueryResult {
        let pin = self.last_sealed_pin();
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax,
                    query_text: text_query.to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: pin.clone(),
                    generation_selector: None,
                    top_k: 50,
                },
                semantic_query_text: semantic_query_text.to_string(),
                generation: pin,
                generation_selector: None,
                top_k,
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eQueryResult {
                    candidates: Vec::new(),
                    candidate_ids: Vec::new(),
                    file_owner_rows: Vec::new(),
                    structural_results: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        let (readiness_reached, response) =
            wait_for_query_response(&socket, &envelope, query_response_ready);
        let response: SearchPlaneQueryIpcResponseEnvelope = match response {
            Ok(r) => r,
            Err(err) => return self.semantic_transport_error(readiness_reached, err),
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Hybrid(hybrid) => E2eQueryResult {
                candidate_ids: hybrid
                    .results
                    .iter()
                    .map(|c| c.candidate_id.clone())
                    .collect(),
                candidates: hybrid.results,
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                engines_touched: hybrid.explanation.engines_touched.clone(),
                explanation: Some(hybrid.explanation),
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Text(_) => unexpected_response("Text"),
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_response("Symbol"),
            SearchPlaneQueryIpcResponse::Semantic(_) => unexpected_response("Semantic"),
            SearchPlaneQueryIpcResponse::History(_) => unexpected_response("History"),
            SearchPlaneQueryIpcResponse::HybridSeed(_) => unexpected_response("HybridSeed"),
            SearchPlaneQueryIpcResponse::Structural(_) => unexpected_response("Structural"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => unexpected_response("RepoMapQuery"),
            SearchPlaneQueryIpcResponse::Explain(_) => unexpected_response("Explain"),
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => {
                unexpected_response("ClusterMembershipRead")
            }
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                unexpected_response("RuntimeMetadata")
            }
        }
    }

    /// Issue any query route, built by the caller from the last sealed pin,
    /// and reduce the daemon's answer to typed error / row count / window.
    ///
    /// This is the route-agnostic front door for contract truth tables that
    /// must drive every variant through the same code path: the caller owns
    /// the payload shape, the harness owns transport and readiness. Harness
    /// failures (driver start, transport, a route that has no result list)
    /// are `Err`; only a daemon-typed refusal becomes `typed_error`.
    pub fn probe_query_route(
        &mut self,
        build: impl FnOnce(Option<GenerationPin>) -> SearchPlaneQueryIpcRequest,
    ) -> AnyResult<E2eRouteWindowProbe> {
        let payload = build(self.last_sealed_pin());
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload,
        };
        let socket = self.ensure_driver()?;
        let (readiness_reached, response) =
            wait_for_query_response(&socket, &envelope, query_response_ready);
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                let failure = self.semantic_transport_error(readiness_reached, err);
                return Err(anyhow::anyhow!(
                    "e2e-harness: route probe transport failure: {}",
                    failure
                        .typed_error
                        .map_or_else(|| "no typed error".to_string(), |error| error.to_string())
                ));
            }
        };
        route_window_probe_from_response(response.payload)
    }

    /// Issue one query exactly once and return the daemon's raw answer, with
    /// no readiness retry: for asserting a `NOT_READY` that is the expected
    /// steady state (a quarantined generation, for instance) rather than a
    /// transient the harness should wait out.
    pub fn query_once(
        &mut self,
        build: impl FnOnce(Option<GenerationPin>) -> SearchPlaneQueryIpcRequest,
    ) -> AnyResult<SearchPlaneQueryIpcResponse> {
        let payload = build(self.last_sealed_pin());
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload,
        };
        let socket = self.ensure_driver()?;
        let response = send_request::<_, SearchPlaneQueryIpcResponseEnvelope>(
            &socket,
            &envelope,
            ClientIoPolicy::default(),
        )?;
        Ok(response.payload)
    }

    pub fn candidate_id_for_path(&self, path: &str) -> AnyResult<String> {
        self.chunk_ids_by_path
            .get(path)
            .map(|chunk_id| chunk_id.as_str().to_string())
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: no chunk id recorded for path `{path}`"))
    }

    fn semantic_transport_error(
        &mut self,
        readiness_reached: bool,
        err: impl std::fmt::Display,
    ) -> E2eQueryResult {
        let mut message = if readiness_reached {
            err.to_string()
        } else {
            format!("readiness timeout before IPC response: {err}")
        };
        if let Some(mut driver) = self.driver.take() {
            driver.shutdown.store(true, Ordering::Release);
            if let Some(join) = driver.join.take() {
                match join.join() {
                    Ok(Ok(())) => message.push_str("; driver exited cleanly before response"),
                    Ok(Err(driver_err)) => {
                        message.push_str("; driver exited with error: ");
                        message.push_str(&driver_err.to_string());
                    }
                    Err(panic) => {
                        let panic_message = format!("{panic:?}");
                        message.push_str("; driver panicked: ");
                        message.push_str(&panic_message);
                    }
                }
            }
        }
        E2eQueryResult {
            candidates: Vec::new(),
            candidate_ids: Vec::new(),
            file_owner_rows: Vec::new(),
            structural_results: Vec::new(),
            engines_touched: Vec::new(),
            explanation: None,
            typed_error: Some(E2eTypedError {
                code: "IPC_TRANSPORT".to_string(),
                message,
            }),
        }
    }

    pub fn explain_candidate(&mut self, candidate: LexicalCandidate) -> E2eExplainResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let pin = GenerationPin::new(
            candidate.repo_id.clone(),
            candidate.revision_id.clone(),
            candidate.manifest_generation,
        );
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Explain(SearchPlaneExplainQueryRequest {
                generation: pin,
                candidate,
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eExplainResult {
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: "HARNESS_START".to_string(),
                        message: err.to_string(),
                    }),
                };
            }
        };
        let (readiness_reached, response) =
            wait_for_query_response(&socket, &envelope, query_response_ready);
        let response: SearchPlaneQueryIpcResponseEnvelope = match response {
            Ok(r) => r,
            Err(err) => return explain_transport_error(readiness_reached, err),
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Explain(explain) => E2eExplainResult {
                explanation: Some(explain.explanation),
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eExplainResult {
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
            },
            SearchPlaneQueryIpcResponse::Text(_) => unexpected_explain_response("Text"),
            SearchPlaneQueryIpcResponse::Symbol(_) => unexpected_explain_response("Symbol"),
            SearchPlaneQueryIpcResponse::Semantic(_) => unexpected_explain_response("Semantic"),
            SearchPlaneQueryIpcResponse::Hybrid(_) => unexpected_explain_response("Hybrid"),
            SearchPlaneQueryIpcResponse::HybridSeed(_) => unexpected_explain_response("HybridSeed"),
            SearchPlaneQueryIpcResponse::History(_) => unexpected_explain_response("History"),
            SearchPlaneQueryIpcResponse::Structural(_) => unexpected_explain_response("Structural"),
            SearchPlaneQueryIpcResponse::RepoMapQuery(_) => {
                unexpected_explain_response("RepoMapQuery")
            }
            SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => {
                unexpected_explain_response("ClusterMembershipRead")
            }
            SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => {
                unexpected_explain_response("RuntimeMetadata")
            }
        }
    }

    fn last_sealed_pin(&self) -> Option<GenerationPin> {
        if self.generation_counter <= 1 {
            None
        } else {
            Some(GenerationPin::new(
                self.repo(),
                self.revision(),
                ManifestGeneration::new(self.generation_counter.saturating_sub(1)),
            ))
        }
    }

    fn search_corpus_identity_from_sealed_receipt(
        &self,
        repo_id: RepoId,
        revision_id: RevisionId,
        sealed: ManifestGeneration,
        receipt: &BatchPublishReceipt,
    ) -> AnyResult<SearchCorpusGenerationIdentityV1> {
        let manifest_digest = receipt.manifest_digest.clone().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: sealed search-corpus receipt carries no manifest digest")
        })?;
        let identity = SearchCorpusGenerationIdentityV1 {
            lexical: GenerationSnapshot {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: sealed,
                manifest_digest: manifest_digest.clone(),
            },
            semantic: GenerationSnapshot {
                repo_id,
                revision_id,
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation: sealed,
                manifest_digest,
            },
        };
        identity.validate_v1().map_err(|err| {
            anyhow::anyhow!("e2e-harness: sealed composite identity is invalid: {err}")
        })?;
        Ok(identity)
    }

    fn dispatch_ingest(&mut self, payload: SearchPlaneIngestIpcRequest) -> AnyResult<()> {
        drop(self.dispatch_ingest_response(payload)?);
        Ok(())
    }

    /// The running daemon's ingest socket, for tests that drive the ingest
    /// transport themselves (concurrent publishers, for instance).
    pub fn ingest_socket_path(&mut self) -> AnyResult<PathBuf> {
        self.ensure_ingest_socket()
    }

    /// Issue one ingest request and return the daemon's raw answer, typed
    /// errors included, with no harness interpretation.
    pub fn ingest_once(
        &mut self,
        payload: SearchPlaneIngestIpcRequest,
    ) -> AnyResult<SearchPlaneIngestIpcResponse> {
        let socket = self.ensure_ingest_socket()?;
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id,
            payload,
        };
        let response: SearchPlaneIngestIpcResponseEnvelope =
            send_request(&socket, &envelope, ClientIoPolicy::default())?;
        Ok(response.payload)
    }

    /// Build, without publishing, the unsealed single-chunk search-corpus
    /// batch `ingest_text` would publish for `path`, under an explicit batch
    /// digest, so a test can publish the same body more than once.
    pub fn text_search_corpus_batch(
        &self,
        path: &str,
        content: &str,
        batch_digest: &str,
    ) -> AnyResult<SearchCorpusIngestBatch> {
        let language = LanguageCode::new(language_from_path(path)).map_err(|err| {
            anyhow::anyhow!("language_from_path must return canonical lowercase codes: {err}")
        })?;
        let record = ChunkRecord {
            chunk_id: ChunkId::new(format!("e2e-idem-{path}")),
            repo_relative_path: RepoRelativePath::new(path),
            language,
            start_byte: 0,
            end_byte: u32::try_from(content.len())
                .map_err(|err| anyhow::anyhow!("e2e harness content length overflow: {err}"))?,
            start_line: 1,
            end_line: 2,
            text: content.to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        };
        let records = vec![record];
        let (mode, base_generation) = self.lexical_batch_contract();
        Ok(SearchCorpusIngestBatch {
            repo_id: self.repo(),
            revision_id: self.revision(),
            generation: self.current_generation(),
            base_generation,
            manifest_digest: format!("lex:{path}:{}", self.current_generation().get()),
            batch_digest: batch_digest.to_string(),
            mode,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SearchCorpusReplaceScope {
                scope: scope_key(path),
                scope_digest: format!("scope:{path}:1-chunks"),
                chunks: records.clone(),
                symbols: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: semantic_source_scopes_for_chunk_records(&records),
            semantic_tombstone_scopes: Vec::new(),
            seal: false,
        })
    }

    fn dispatch_ingest_response(
        &mut self,
        payload: SearchPlaneIngestIpcRequest,
    ) -> AnyResult<SearchPlaneIngestIpcResponse> {
        let socket = self.ensure_ingest_socket()?;
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id,
            payload,
        };
        let response: SearchPlaneIngestIpcResponseEnvelope =
            send_request(&socket, &envelope, ClientIoPolicy::default())?;
        if response.request_id != request_id {
            return Err(anyhow::anyhow!(
                "e2e-harness ingest response request_id {} differs from request {request_id}",
                response.request_id
            ));
        }
        if let SearchPlaneIngestIpcResponse::Error(err) = response.payload {
            return Err(anyhow::anyhow!(
                "e2e-harness ingest failed code={} message={}",
                err.code,
                err.message
            ));
        }
        Ok(response.payload)
    }

    fn dispatch_control(
        &mut self,
        payload: SearchPlaneControlIpcRequest,
    ) -> AnyResult<SearchPlaneControlIpcResponse> {
        let response = self.dispatch_control_response_v1(payload)?;
        if let SearchPlaneControlIpcResponse::Error(err) = response {
            return Err(anyhow::anyhow!(
                "e2e-harness control failed code={} message={}",
                err.code,
                err.message
            ));
        }
        Ok(response)
    }

    fn dispatch_control_response_v1(
        &mut self,
        payload: SearchPlaneControlIpcRequest,
    ) -> AnyResult<SearchPlaneControlIpcResponse> {
        let socket = self.ensure_control_socket()?;
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneControlIpcRequestEnvelope {
            request_id,
            payload,
        };
        let response: SearchPlaneControlIpcResponseEnvelope =
            send_request(&socket, &envelope, ClientIoPolicy::default())?;
        if response.request_id != request_id {
            return Err(anyhow::anyhow!(
                "e2e-harness control response request_id {} differs from request {request_id}",
                response.request_id
            ));
        }
        Ok(response.payload)
    }
}

fn unexpected_explain_response(kind: &str) -> E2eExplainResult {
    E2eExplainResult {
        explanation: None,
        typed_error: Some(E2eTypedError {
            code: "UNEXPECTED_RESPONSE".to_string(),
            message: format!("expected Explain, got {kind}"),
        }),
    }
}

fn unexpected_history_response(kind: &str) -> E2eHistoryResult {
    E2eHistoryResult {
        commit_ids: Vec::new(),
        diff_paths: Vec::new(),
        window: None,
        examined: 0,
        next_cursor: None,
        typed_error: Some(E2eTypedError {
            code: "UNEXPECTED_RESPONSE".to_string(),
            message: format!("expected History, got {kind}"),
        }),
    }
}

impl Drop for E2eRuntime {
    #[expect(
        clippy::panic,
        clippy::print_stderr,
        reason = "the harness is test infrastructure: a driver that fails on teardown must fail the test, and while another panic is already unwinding the only channel left to report it on is stderr"
    )]
    fn drop(&mut self) {
        if let Err(error) = self.stop_driver() {
            if thread::panicking() {
                eprintln!(
                    "e2e-harness: daemon driver failed while another panic was unwinding: {error:#}"
                );
            } else {
                panic!("e2e-harness: daemon driver failed during drop: {error:#}");
            }
        }
        drop(self.tempdir.take());
    }
}

fn query_response_ready(response: &SearchPlaneQueryIpcResponseEnvelope) -> bool {
    match &response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => err.code != "NOT_READY",
        SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => true,
    }
}

fn query_response_ready_allow_structural_not_ready(
    response: &SearchPlaneQueryIpcResponseEnvelope,
) -> bool {
    match &response.payload {
        SearchPlaneQueryIpcResponse::Error(err) => {
            err.code != "NOT_READY" && err.code != "STR_GENERATION_NOT_READY"
        }
        SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => true,
    }
}

fn wait_for_query_response(
    socket: &Path,
    envelope: &SearchPlaneQueryIpcRequestEnvelope,
    ready: impl Fn(&SearchPlaneQueryIpcResponseEnvelope) -> bool,
) -> (bool, Result<SearchPlaneQueryIpcResponseEnvelope, IpcError>) {
    let mut cached_response: Option<SearchPlaneQueryIpcResponseEnvelope> = None;
    let readiness_reached =
        wait_until(
            READINESS_TIMEOUT,
            READINESS_POLL_INTERVAL,
            || match send_request::<_, SearchPlaneQueryIpcResponseEnvelope>(
                socket,
                envelope,
                ClientIoPolicy::default(),
            ) {
                Ok(response) => {
                    if ready(&response) {
                        cached_response = Some(response);
                        return true;
                    }
                    false
                }
                Err(_transport_error) => false,
            },
        );
    if let Some(response) = cached_response {
        return (readiness_reached, Ok(response));
    }
    (
        readiness_reached,
        send_request(socket, envelope, ClientIoPolicy::default()),
    )
}

fn explain_transport_error(
    readiness_reached: bool,
    err: impl std::fmt::Display,
) -> E2eExplainResult {
    let message = if readiness_reached {
        err.to_string()
    } else {
        format!("readiness timeout before IPC response: {err}")
    };
    E2eExplainResult {
        explanation: None,
        typed_error: Some(E2eTypedError {
            code: "IPC_TRANSPORT".to_string(),
            message,
        }),
    }
}

fn start_driver(
    state_root: &Path,
    embedder_profile: &SemanticEmbedderProfile,
    history_max_generations: usize,
    ingest_resource_policy: IngestResourcePolicy,
) -> AnyResult<DriverHandles> {
    let config = build_config(
        state_root,
        embedder_profile,
        history_max_generations,
        ingest_resource_policy,
    )?;
    let runtime = build_runtime(config)?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let query_obs_store = Arc::clone(&runtime.query_obs_store);
    let boot_inventory = runtime.boot_inventory.clone();
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("e2e-harness-driver".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;
    if !wait_until(SOCKET_APPEAR_TIMEOUT, SOCKET_APPEAR_POLL_INTERVAL, || {
        query_socket.exists() && control_socket.exists() && ingest_socket.exists()
    }) {
        shutdown.store(true, Ordering::Release);
        let socket_failure = format!(
            "e2e-harness: sockets never appeared query={} control={} ingest={}",
            query_socket.display(),
            control_socket.display(),
            ingest_socket.display()
        );
        return match join.join() {
            Ok(Ok(())) => Err(anyhow::anyhow!("{socket_failure}")),
            Ok(Err(error)) => Err(anyhow::anyhow!(
                "{socket_failure}; daemon driver returned an error: {error:#}"
            )),
            Err(panic) => Err(anyhow::anyhow!(
                "{socket_failure}; daemon driver panicked: {panic:?}"
            )),
        };
    }
    Ok((
        query_socket,
        control_socket,
        ingest_socket,
        shutdown,
        join,
        query_obs_store,
        boot_inventory,
    ))
}

const DEFAULT_HISTORY_MAX_GENERATIONS: usize = 8;

fn build_config(
    state_root: &Path,
    embedder_profile: &SemanticEmbedderProfile,
    history_max_generations: usize,
    ingest_resource_policy: IngestResourcePolicy,
) -> AnyResult<SearchdConfig> {
    let mut cfg = SearchdConfig::from_state_root(state_root.to_path_buf())
        .try_with_search_corpus_history_retention_limits(
            history_max_generations,
            16 * 1024 * 1024,
            128,
            256 * 1024 * 1024,
        )?;
    let (query_socket, control_socket, ingest_socket) = unique_socket_paths();
    cfg = SearchdConfig::with_socket_overrides(cfg, query_socket, control_socket);
    cfg = SearchdConfig::with_ingest_socket_override(cfg, ingest_socket);
    Ok(cfg
        .with_semantic_embedder_profile(embedder_profile.clone())
        .with_ingest_resource_policy(ingest_resource_policy))
}

/// Reduce one query response to its bounded-result shape. Routes that do
/// not answer with a result list are not probe-able and are a harness error,
/// not a daemon refusal.
fn route_window_probe_from_response(
    payload: SearchPlaneQueryIpcResponse,
) -> AnyResult<E2eRouteWindowProbe> {
    let (returned_rows, window) = match payload {
        SearchPlaneQueryIpcResponse::Error(err) => {
            return Ok(E2eRouteWindowProbe {
                typed_error: Some(E2eTypedError {
                    code: err.code,
                    message: err.message,
                }),
                returned_rows: 0,
                window: None,
            });
        }
        SearchPlaneQueryIpcResponse::Text(text) => (text.results.len(), Some(text.window)),
        SearchPlaneQueryIpcResponse::Symbol(symbol) => (symbol.results.len(), Some(symbol.window)),
        SearchPlaneQueryIpcResponse::Semantic(semantic) => {
            (semantic.results.len(), Some(semantic.window))
        }
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => (hybrid.results.len(), Some(hybrid.window)),
        // The hybrid-seed window describes the active candidate list, which
        // is `seed_candidates_v2` whenever it is present and the legacy list
        // only otherwise (QI-BB-019 tracks collapsing the two).
        SearchPlaneQueryIpcResponse::HybridSeed(seed) => (
            seed.seed_candidates_v2
                .as_ref()
                .map_or(seed.seed_candidates.len(), Vec::len),
            Some(seed.window),
        ),
        SearchPlaneQueryIpcResponse::History(history) => (
            history.commits.len().saturating_add(history.diffs.len()),
            None,
        ),
        SearchPlaneQueryIpcResponse::RuntimeMetadata(runtime) => (runtime.results.len(), None),
        SearchPlaneQueryIpcResponse::Structural(structural) => (structural.results.len(), None),
        SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => {
            return Err(anyhow::anyhow!(
                "e2e-harness: route answered without a bounded result list; it cannot be window-probed"
            ));
        }
    };
    Ok(E2eRouteWindowProbe {
        typed_error: None,
        returned_rows,
        window,
    })
}

fn unexpected_response(kind: &str) -> E2eQueryResult {
    E2eQueryResult {
        candidates: Vec::new(),
        candidate_ids: Vec::new(),
        file_owner_rows: Vec::new(),
        structural_results: Vec::new(),
        engines_touched: Vec::new(),
        explanation: None,
        typed_error: Some(E2eTypedError {
            code: "UNEXPECTED_RESPONSE".to_string(),
            message: format!("expected Text, got {kind}"),
        }),
    }
}

fn unique_socket_paths() -> (PathBuf, PathBuf, PathBuf) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    let query = std::env::temp_dir().join(format!("qi-e2e-query-{pid}-{nanos}-{sequence}.sock"));
    let control =
        std::env::temp_dir().join(format!("qi-e2e-control-{pid}-{nanos}-{sequence}.sock"));
    let ingest = std::env::temp_dir().join(format!("qi-e2e-ingest-{pid}-{nanos}-{sequence}.sock"));
    (query, control, ingest)
}

fn wait_until<F>(timeout: Duration, poll_interval: Duration, mut cond: F) -> bool
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if cond() {
            return true;
        }
        thread::sleep(poll_interval);
    }
    false
}

fn language_from_path(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("ts") => "typescript",
        Some("js") => "javascript",
        Some("md") => "markdown",
        Some(_) | None => "text",
    }
}

fn semantic_source_scopes_for_chunk_records(
    records: &[ChunkRecord],
) -> Vec<SemanticSourceReplaceScopeV1> {
    records
        .iter()
        .map(|record| {
            let owner_id = record.chunk_id.as_str().to_string();
            SemanticSourceReplaceScopeV1 {
                scope: SemanticSourceScopeKeyV1 {
                    corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                    owner_kind: OwnerDocKind::Chunk,
                    owner_id: owner_id.clone(),
                },
                scope_digest: format!("e2e-harness:semantic-source:scope:{owner_id}"),
                sources: vec![SemanticSourceRecordV1 {
                    record_id: owner_id.clone(),
                    corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                    owner_kind: OwnerDocKind::Chunk,
                    owner_id: owner_id.clone(),
                    source_doc_id: owner_id.clone(),
                    parent_owner_id: Some(owner_id),
                    repo_relative_path: record.repo_relative_path.clone(),
                    language: Some(record.language.as_str().to_string()),
                    package: None,
                    symbol_kind: None,
                    visibility: None,
                    source_role: SourceRoleV1::RawFallbackText,
                    generated: false,
                    capability_status: CapabilityStatusV1::Degraded,
                    raw_fallback_reason: Some(
                        RawFallbackReasonV1::IntentNotRecoverableFromStructure,
                    ),
                    authority_digest: "e2e-harness:direct-text-source:v1".to_string(),
                    render_policy_digest: "e2e-harness:direct-text-source:v1".to_string(),
                    card_schema_version: 0,
                    text: record.text.to_string(),
                }],
                cluster_memberships: Vec::new(),
            }
        })
        .collect()
}

fn scope_key(path: &str) -> quanta_index_contract::SearchScopeKey {
    quanta_index_contract::SearchScopeKey {
        doc_surface: quanta_index_contract::SearchScopeSurface::Chunk,
        repo_relative_path: RepoRelativePath::new(path),
    }
}
