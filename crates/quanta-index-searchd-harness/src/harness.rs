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
    AuxEpochV1, BatchIngestMode, BatchPublishReceipt, CapabilityStatusV1, ChunkId, ChunkRecord,
    ContinuationTokenV2, CurrentGenerationRequest, EngineTouched, ExplainCandidateV1,
    FileOwnerProjectionRow, GenerationPin, GenerationSnapshot, GenerationStatusReport,
    GenerationStatusRequest, HistoryOrderV1, HistoryQueryRequest, HistoryScoreV1,
    HybridCandidateV1, HybridQueryRequest, LexicalCandidate, ManifestGeneration,
    MetricsSnapshotRequest, MetricsSnapshotV1, OwnerDocKind, QuarantineDiscardAck,
    QuarantineDiscardRequest, QuarantineInventoryRequest, QuarantineInventoryV1,
    QuarantineTargetV1, QueryResultWindowV2, RawFallbackReasonV1, RepoId, RepoRelativePath,
    RevisionId, RuntimeMetadataQueryRequest, SearchCorpusGenerationIdentityV1,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchCorpusTombstoneScope,
    SearchExplanation, SearchPlaneActivateSearchCorpusGenerationCasRequest,
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneControlIpcResponse, SearchPlaneControlIpcResponseEnvelope, SearchPlaneErrorCodeV2,
    SearchPlaneExplainQueryRequest, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneIpcError, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    SearchPlaneRuntimeMetadataQueryResponse, SearchPlaneStructuralQueryResponse,
    SearchPlaneTrackKind, SemanticContentRootsV1, SemanticCorpusKindV1, SemanticQueryRequest,
    SemanticSourceRecordV1, SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1, SourceRoleV1,
    StructuralCandidate, StructuralIngestBatch, StructuralQueryRequest, StructuralReplaceScope,
    StructuralTreeRecord, SymbolId, TextQueryRequest, TextQueryResponse, TextQuerySyntax,
};
use quanta_index_core::{
    IngestResourcePolicy, IntegrityScrubPolicyV1, LexicalWriterPolicy, ProcessMemoryProbePort,
    SemanticStreamWindowPolicy,
};
use quanta_index_ipc::{
    ClientIoPolicy, IpcError, ServerAdmissionPolicy, send_request, stamp_batch_digest_v1,
};
use quanta_index_search_plane::{
    BoundedQueryObsStore, MetricSample, ObsError, ResponsePayloadBudget,
};
use quanta_index_searchd::app::searchd::drive;
use quanta_index_searchd::app::semantic_boot::SemanticBootReport;
use quanta_index_searchd::app::{
    BootInventoryReportV1, KernelResidentMemoryProbe, MaintenancePolicy, ProcessMemoryCeilings,
    SearchdConfig, SemanticEmbedderProfile, SocketAccessPolicies, SocketRole,
};
use quanta_index_searchd_runtime::build_runtime_with_memory_probe;
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
    SemanticBootReport,
);

/// Everything one daemon start is configured with.
struct DriverSpec<'a> {
    state_root: &'a Path,
    embedder_profile: &'a SemanticEmbedderProfile,
    history_max_generations: usize,
    history_max_bytes: u64,
    ingest_resource_policy: IngestResourcePolicy,
    /// The semantic track's stream window (QI-BB-021).
    semantic_stream_window_policy: SemanticStreamWindowPolicy,
    /// The query socket's admission limits, including the deadline every
    /// dispatch runs under (QI-BB-002).
    query_admission_policy: ServerAdmissionPolicy,
    /// The lexical writer envelope (QI-BB-016).
    lexical_writer_policy: LexicalWriterPolicy,
    /// The process memory ceilings and where the daemon reads its resident
    /// memory (QI-BB-016).
    process_memory_ceilings: ProcessMemoryCeilings,
    memory_probe: &'a Arc<dyn ProcessMemoryProbePort>,
    /// The maintenance timer's cadence (QI-BB-016, QI-BB-015).
    maintenance_policy: MaintenancePolicy,
    /// How the integrity scrub is paced (QI-BB-017).
    integrity_scrub_policy: IntegrityScrubPolicyV1,
    /// How many encoded bytes one ranked page may take (QI-BB-005).
    query_response_budget: ResponsePayloadBudget,
    socket_access: &'a SocketAccessPolicies,
    /// Where the three sockets go: a fresh, unique directory the daemon
    /// creates under `/tmp` when any socket is shared (so the peers the
    /// policy admits can traverse the path), the process temp dir
    /// otherwise.
    socket_directory: Option<&'a Path>,
}

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
    /// Index bytes the retained generations of a pair may hold together:
    /// [`HARNESS_HISTORY_MAX_BYTES`] unless a fixture larger than it widens
    /// it through [`Self::with_history_max_bytes`].
    history_max_bytes: u64,
    /// The resource envelope one search-corpus batch may ask the daemon to
    /// hold (QI-BB-021). The production default is far wider than any
    /// fixture; envelope tests tighten it through
    /// [`Self::boot_with_ingest_resource_policy`].
    ingest_resource_policy: IngestResourcePolicy,
    /// Who may connect to each socket (QI-BB-014). Private everywhere by
    /// default; shared-mode tests open one through
    /// [`Self::boot_with_socket_access`].
    socket_access: SocketAccessPolicies,
    /// The directory the sockets live in when any of them is shared: a
    /// unique path under `/tmp` that the daemon creates with the policy's
    /// mode and group, and that the harness removes on stop. `None` keeps
    /// the sockets loose in the process temp dir.
    socket_directory: Option<PathBuf>,
    /// The window the daemon's semantic track embeds and appends a batch in
    /// (QI-BB-021). The production default holds any fixture in one window;
    /// streaming tests narrow it through
    /// [`Self::boot_with_semantic_stream_window_policy`].
    semantic_stream_window_policy: SemanticStreamWindowPolicy,
    /// The integrity scrub pacing the daemon boots with: dormant (one step a
    /// day) unless set through [`Self::boot_with_integrity_scrub_policy`],
    /// so a test that injects faults into generation directories never
    /// races a scrub writing its receipt into them.
    integrity_scrub_policy: IntegrityScrubPolicyV1,
    /// The ranked page byte budget the daemon boots with: a frame's worth
    /// unless set through [`Self::boot_with_query_response_budget`].
    query_response_budget: ResponsePayloadBudget,
    /// How long the harness waits for each answer: the client default
    /// unless set through [`Self::boot_with_client_request_timeout`], for a
    /// fixture whose seal outlasts it in a debug build.
    client_io: ClientIoPolicy,
    /// The query socket's admission limits (QI-BB-002). The production
    /// default's twenty-second dispatch budget never expires on a fixture;
    /// budget tests shorten it through
    /// [`Self::boot_with_query_admission_policy`].
    query_admission_policy: ServerAdmissionPolicy,
    /// The lexical writer envelope (QI-BB-016); the production default,
    /// narrowed by envelope tests through
    /// [`Self::boot_with_lexical_writer_policy`].
    lexical_writer_policy: LexicalWriterPolicy,
    /// The process memory ceilings (QI-BB-016): the production default,
    /// with a resident-memory gate only where a test installs one through
    /// [`Self::boot_with_memory_probe`].
    process_memory_ceilings: ProcessMemoryCeilings,
    /// Where the daemon reads its resident memory: the kernel, or a
    /// scripted probe a gate test drives.
    memory_probe: Arc<dyn ProcessMemoryProbePort>,
    /// The maintenance timer's cadence (QI-BB-016): fast in the harness,
    /// so an idle sweep or a disk refresh is observable within a test's
    /// patience without a sleep the length of the production tick.
    maintenance_policy: MaintenancePolicy,
    driver: Option<DriverState>,
    query_obs_store: Option<Arc<BoundedQueryObsStore>>,
    chunk_ids_by_path: BTreeMap<String, ChunkId>,
    chunk_records_by_path: BTreeMap<String, ChunkRecord>,
    request_id_counter: AtomicU64,
    /// Each harness-built mutation is a distinct producer declaration: the
    /// sequence is folded into producer-declared content digests so two
    /// helper calls with the same arguments are two batches, not one
    /// replayed body (QI-BB-032 binds `batch_digest` to the body).
    batch_sequence: AtomicU64,
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
    /// The semantic track's boot report: migration outcome plus the
    /// sealed-generation seed counts (LDB-E2E-01).
    semantic_boot: SemanticBootReport,
}

/// Test-only response shape.
///
/// Carries either the candidate list or a typed error code.
/// `engines_touched` is best-effort; the `Text` response variant has no
/// explanation today so this stays empty for plain text queries. Semantic and
/// hybrid rows populate it in later E2E tickets. A hybrid query fills
/// `hybrid_candidates` with the fused rows as the wire carried them, and
/// `candidates` with each row's lane candidate, in the same order.
pub struct E2eQueryResult {
    pub candidates: Vec<LexicalCandidate>,
    pub candidate_ids: Vec<String>,
    pub file_owner_rows: Vec<FileOwnerProjectionRow>,
    pub structural_results: Vec<StructuralCandidate>,
    pub hybrid_candidates: Vec<HybridCandidateV1>,
    pub engines_touched: Vec<EngineTouched>,
    pub explanation: Option<SearchExplanation>,
    pub typed_error: Option<E2eTypedError>,
}

#[derive(Clone, Debug)]
pub struct E2eHistoryResult {
    pub commit_ids: Vec<String>,
    pub diff_paths: Vec<String>,
    /// The order the page was served in, when the daemon answered
    /// (QI-BB-023 follow-up #1).
    pub order: Option<HistoryOrderV1>,
    /// One entry per row of the page (commits or diffs, whichever the page
    /// holds): the row's relevance score under that order, none under
    /// recency.
    pub scores: Vec<Option<HistoryScoreV1>>,
    /// The page's window and continuation, when the daemon answered.
    pub window: Option<QueryResultWindowV2>,
    /// The history authority epoch the page was cut from, when the daemon
    /// answered (QI-BB-020 W2).
    pub read_epoch: Option<AuxEpochV1>,
    pub examined: u64,
    pub next_cursor: Option<ContinuationTokenV2>,
    pub typed_error: Option<E2eTypedError>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct E2eTypedError {
    pub code: E2eErrorCode,
    pub message: String,
}

/// Harness failures are distinct from refusals decoded from the daemon wire.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum E2eErrorCode {
    Remote(SearchPlaneErrorCodeV2),
    HarnessStart,
    HarnessRouteMismatch,
    IpcTransport,
    UnexpectedResponse,
}

impl E2eErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Remote(code) => code.as_wire_str(),
            Self::HarnessStart => "HARNESS_START",
            Self::HarnessRouteMismatch => "HARNESS_ROUTE_MISMATCH",
            Self::IpcTransport => "IPC_TRANSPORT",
            Self::UnexpectedResponse => "UNEXPECTED_RESPONSE",
        }
    }

    /// Exact decoder for the child-process benchmark artifact boundary.
    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "HARNESS_START" => Some(Self::HarnessStart),
            "HARNESS_ROUTE_MISMATCH" => Some(Self::HarnessRouteMismatch),
            "IPC_TRANSPORT" => Some(Self::IpcTransport),
            "UNEXPECTED_RESPONSE" => Some(Self::UnexpectedResponse),
            _ => SearchPlaneErrorCodeV2::from_wire_str(value).map(Self::Remote),
        }
    }
}

impl fmt::Display for E2eErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl fmt::Display for E2eTypedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for E2eTypedError {}

/// One keyset page of a route, exactly as the wire carried it (QI-BB-025
/// W4).
///
/// The daemon's typed answer when it served, its typed refusal when it
/// did not. Harness failures (driver start, transport) are not pages and
/// surface as `Err` from the query instead.
#[derive(Clone, Debug)]
pub enum E2eRoutePage<R> {
    Served(R),
    Refused(E2eTypedError),
}

impl<R> E2eRoutePage<R> {
    /// The served page, or the refusal as an error naming `what`.
    pub fn served(self, what: &str) -> AnyResult<R> {
        match self {
            Self::Served(page) => Ok(page),
            Self::Refused(error) => Err(anyhow::anyhow!("{what}: page refused: {error}")),
        }
    }

    /// The typed refusal, if the daemon refused.
    pub const fn refusal(&self) -> Option<&E2eTypedError> {
        match self {
            Self::Served(_) => None,
            Self::Refused(error) => Some(error),
        }
    }
}

/// One query route's answer, reduced to what a bounded-result contract asserts on.
///
/// Carries the typed refusal if the daemon refused, else the row count and the
/// wire window. Routes that answer without a window (history, runtime
/// metadata, structural) report `None` for it.
#[derive(Clone, Debug)]
pub struct E2eRouteWindowProbe {
    pub typed_error: Option<E2eTypedError>,
    pub returned_rows: usize,
    pub window: Option<QueryResultWindowV2>,
}

pub struct E2eExplainResult {
    pub presence: Option<quanta_index_contract::CandidatePresenceV1>,
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
        Self::boot_with_embedder_profile(SemanticEmbedderProfile::hash_dev())
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
        Self::boot_with_profile_and_history(SemanticEmbedderProfile::hash_dev(), max_generations)
    }

    /// Change the retention window the next daemon start runs under. With
    /// [`Self::reopen`] this is how a test lowers the cap across a
    /// restart, so boot's retention reaps records whose directories are
    /// still on disk: the orphan shape (QI-BB-003).
    #[must_use]
    pub fn with_history_max_generations(mut self, max_generations: usize) -> Self {
        self.history_max_generations = max_generations;
        self
    }

    /// Change the index bytes the retained generations of a pair may hold
    /// together, for the next daemon start: a fixture whose one generation
    /// is larger than the harness default (16 MiB) would otherwise have its
    /// seal refused by retention.
    #[must_use]
    pub const fn with_history_max_bytes(mut self, max_bytes: u64) -> Self {
        self.history_max_bytes = max_bytes;
        self
    }

    /// Like [`Self::boot`] but runs the daemon under `policy` as its ingest
    /// resource envelope, so an envelope refusal can be provoked with a
    /// small batch instead of a hundred-thousand-record one.
    pub fn boot_with_ingest_resource_policy(policy: IngestResourcePolicy) -> AnyResult<Self> {
        let mut runtime = Self::boot()?;
        runtime.ingest_resource_policy = policy;
        Ok(runtime)
    }

    /// Like [`Self::boot`] but binds each socket under its policy in
    /// `access` (QI-BB-014).
    ///
    /// When any socket is shared, the three sockets are placed in a fresh
    /// directory under `/tmp` — the one directory that is sticky and
    /// world-traversable on every supported host — which the daemon creates
    /// with the policy's mode and group; the process temp dir is not
    /// traversable by other users on macOS, so a shared socket there would
    /// be refused at bind. The daemon itself resolves nothing here: the
    /// policies are typed, so a test that wants a name refused at boot goes
    /// through the daemon's config parser instead.
    pub fn boot_with_socket_access(access: SocketAccessPolicies) -> AnyResult<Self> {
        let mut runtime = Self::boot()?;
        let any_shared = SocketRole::ALL
            .into_iter()
            .any(|role| access.for_role(role).admits_others());
        if any_shared {
            runtime.socket_directory = Some(unique_shared_socket_directory());
        }
        runtime.socket_access = access;
        Ok(runtime)
    }

    /// The directory the sockets live in when a shared policy placed them
    /// under `/tmp`; `None` while the sockets are loose in the process temp
    /// dir. Exists only once the daemon has bound (it creates the
    /// directory), and a refused boot leaves nothing behind.
    #[must_use]
    pub fn socket_directory(&self) -> Option<&Path> {
        self.socket_directory.as_deref()
    }

    /// The paths of the three sockets the running daemon bound, in
    /// (query, control, ingest) order; `None` while the driver is stopped.
    #[must_use]
    pub fn socket_paths(&self) -> Option<(&Path, &Path, &Path)> {
        self.driver.as_ref().map(|driver| {
            (
                driver.query_socket.as_path(),
                driver.control_socket.as_path(),
                driver.ingest_socket.as_path(),
            )
        })
    }

    /// Like [`Self::boot`] but runs the daemon's semantic track under
    /// `policy` as its stream window, so a small batch streams through
    /// several windows instead of one.
    pub fn boot_with_semantic_stream_window_policy(
        policy: SemanticStreamWindowPolicy,
    ) -> AnyResult<Self> {
        let mut runtime = Self::boot()?;
        runtime.semantic_stream_window_policy = policy;
        Ok(runtime)
    }

    /// Like [`Self::boot`] but cuts ranked pages at `budget` encoded bytes,
    /// so a byte-cut page can be driven with a small fixture (QI-BB-005).
    pub fn boot_with_query_response_budget(budget: ResponsePayloadBudget) -> AnyResult<Self> {
        let mut runtime = Self::boot()?;
        runtime.query_response_budget = budget;
        Ok(runtime)
    }

    /// Like [`Self::boot`] but waits up to `timeout` for every answer: for a
    /// fixture large enough that its seal outlasts the client default in a
    /// debug build.
    pub fn boot_with_client_request_timeout(timeout: Duration) -> AnyResult<Self> {
        let mut runtime = Self::boot()?;
        runtime.client_io = ClientIoPolicy::try_new(timeout)
            .map_err(|error| anyhow::anyhow!("e2e-harness: client timeout: {error}"))?;
        Ok(runtime)
    }

    /// Like [`Self::boot`] but paces the integrity scrub under `policy`, so
    /// a scrub can complete inside a test without touching process-global
    /// env (QI-BB-017).
    pub fn boot_with_integrity_scrub_policy(policy: IntegrityScrubPolicyV1) -> AnyResult<Self> {
        let mut runtime = Self::boot()?;
        runtime.integrity_scrub_policy = policy;
        Ok(runtime)
    }

    /// Like [`Self::boot`] but admits query dispatches under `policy`, so a
    /// dispatch budget short enough to expire inside a test can be set
    /// without touching process-global env.
    pub fn boot_with_query_admission_policy(policy: ServerAdmissionPolicy) -> AnyResult<Self> {
        let mut runtime = Self::boot()?;
        runtime.query_admission_policy = policy;
        Ok(runtime)
    }

    /// A runtime whose lexical writer envelope is `policy` (QI-BB-016).
    pub fn boot_with_lexical_writer_policy(policy: LexicalWriterPolicy) -> AnyResult<Self> {
        let mut runtime = Self::boot()?;
        runtime.lexical_writer_policy = policy;
        Ok(runtime)
    }

    /// A runtime that reads its resident memory from `probe` under
    /// `ceilings` (QI-BB-016), so a test can drive the lexical writer gate
    /// without a real memory spike.
    pub fn boot_with_memory_probe(
        probe: Arc<dyn ProcessMemoryProbePort>,
        ceilings: ProcessMemoryCeilings,
    ) -> AnyResult<Self> {
        let mut runtime = Self::boot()?;
        runtime.memory_probe = probe;
        runtime.process_memory_ceilings = ceilings;
        Ok(runtime)
    }

    /// The maintenance timer's cadence this runtime boots with.
    #[must_use]
    pub const fn maintenance_policy(&self) -> MaintenancePolicy {
        self.maintenance_policy
    }

    fn boot_with_profile_and_history(
        profile: SemanticEmbedderProfile,
        history_max_generations: usize,
    ) -> AnyResult<Self> {
        let tempdir = private_tempdir()?;
        let state_root = tempdir.path().to_path_buf();
        Ok(Self {
            tempdir: Some(tempdir),
            state_root,
            embedder_profile: profile,
            history_max_generations,
            history_max_bytes: HARNESS_HISTORY_MAX_BYTES,
            ingest_resource_policy: IngestResourcePolicy::DEFAULT,
            socket_access: SocketAccessPolicies::PRIVATE,
            socket_directory: None,
            semantic_stream_window_policy: SemanticStreamWindowPolicy::DEFAULT,
            integrity_scrub_policy: IntegrityScrubPolicyV1::new(
                HARNESS_DORMANT_SCRUB_INTERVAL_MILLIS,
                IntegrityScrubPolicyV1::DEFAULT.max_bytes_per_step,
            )?,
            query_response_budget: ResponsePayloadBudget::DEFAULT,
            client_io: ClientIoPolicy::default(),
            query_admission_policy: ServerAdmissionPolicy::DEFAULT,
            lexical_writer_policy: LexicalWriterPolicy::DEFAULT,
            process_memory_ceilings: ProcessMemoryCeilings::DEFAULT,
            memory_probe: Arc::new(KernelResidentMemoryProbe),
            maintenance_policy: MaintenancePolicy::new(HARNESS_MAINTENANCE_TICK)?,
            driver: None,
            query_obs_store: None,
            chunk_ids_by_path: BTreeMap::new(),
            chunk_records_by_path: BTreeMap::new(),
            request_id_counter: AtomicU64::new(1),
            batch_sequence: AtomicU64::new(1),
            generation_counter: 1,
            last_sealed_search_corpus_identity: None,
        })
    }

    /// The next producer-side declaration sequence for a harness-built
    /// mutation; see the field.
    fn next_batch_sequence(&self) -> u64 {
        self.batch_sequence.fetch_add(1, Ordering::Relaxed)
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

    /// Boot over a caller-owned state root instead of a harness tempdir
    /// (TOPT-03): the daemon creates and serves `state_root`, but the
    /// caller owns the directory lifecycle. Used by the long-path proof,
    /// where the root must be deliberately deep.
    pub fn boot_in(state_root: &Path) -> AnyResult<Self> {
        let mut runtime = Self::boot()?;
        drop(runtime.tempdir.take());
        runtime.state_root = state_root.to_path_buf();
        Ok(runtime)
    }

    /// Stop the driver and release the state root, returning the driver's
    /// terminal result (TOPT-03): explicit tests call `stop` and surface
    /// a driver failure as their error; `Drop` remains the unwind path
    /// that never masks a scenario failure already in flight.
    pub fn stop(mut self) -> AnyResult<()> {
        self.stop_driver()?;
        drop(self.tempdir.take());
        Ok(())
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
        outcome?;
        self.remove_socket_directory()
    }

    /// Remove the shared-mode socket directory, if the daemon created it.
    /// The servers unlink their sockets on shutdown; the directory is the
    /// harness's to remove. A directory that was never created (a boot
    /// refused before any bind) is not an error.
    fn remove_socket_directory(&self) -> AnyResult<()> {
        let Some(directory) = self.socket_directory.as_deref() else {
            return Ok(());
        };
        match std::fs::remove_dir_all(directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(anyhow::anyhow!(
                "e2e-harness: removing socket directory {} failed: {error}",
                directory.display()
            )),
        }
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

    /// The running daemon's semantic boot report (migration outcome and
    /// seed counts), or `None` while the driver is stopped.
    #[must_use]
    pub fn semantic_boot_report(&self) -> Option<&SemanticBootReport> {
        self.driver.as_ref().map(|driver| &driver.semantic_boot)
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
                semantic_boot,
            ) = start_driver(&DriverSpec {
                state_root: &self.state_root,
                embedder_profile: &self.embedder_profile,
                history_max_generations: self.history_max_generations,
                history_max_bytes: self.history_max_bytes,
                ingest_resource_policy: self.ingest_resource_policy,
                semantic_stream_window_policy: self.semantic_stream_window_policy,
                query_admission_policy: self.query_admission_policy,
                lexical_writer_policy: self.lexical_writer_policy,
                process_memory_ceilings: self.process_memory_ceilings,
                memory_probe: &self.memory_probe,
                maintenance_policy: self.maintenance_policy,
                integrity_scrub_policy: self.integrity_scrub_policy,
                query_response_budget: self.query_response_budget,
                socket_access: &self.socket_access,
                socket_directory: self.socket_directory.as_deref(),
            })?;
            self.query_obs_store = Some(Arc::clone(&query_obs_store));
            self.driver = Some(DriverState {
                query_socket,
                control_socket,
                ingest_socket,
                shutdown,
                join: Some(join),
                boot_inventory,
                semantic_boot,
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

    #[expect(
        clippy::expect_used,
        reason = "fixed e2e harness identity is an in-source validated fixture"
    )]
    pub fn repo(&self) -> RepoId {
        RepoId::new("repo-e2e").expect("static fixture ID satisfies canonical policy")
    }

    #[expect(
        clippy::expect_used,
        reason = "fixed e2e harness identity is an in-source validated fixture"
    )]
    pub fn revision(&self) -> RevisionId {
        RevisionId::new("rev-e2e").expect("static fixture ID satisfies canonical policy")
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

    /// What the daemon quarantines right now, over its public control UDS
    /// (QI-BB-026).
    pub fn quarantine_inventory(&mut self) -> AnyResult<QuarantineInventoryV1> {
        let response = self.dispatch_control(SearchPlaneControlIpcRequest::QuarantineInventory(
            QuarantineInventoryRequest,
        ))?;
        let SearchPlaneControlIpcResponse::QuarantineInventory(inventory) = response else {
            return Err(anyhow::anyhow!(
                "e2e-harness: quarantine inventory returned an unexpected control response: {response:?}"
            ));
        };
        Ok(inventory)
    }

    /// Discard one quarantined entry as listed (QI-BB-026). The outer error
    /// is the harness failing to ask; the inner one is the daemon's typed
    /// refusal, which a test asserts on.
    pub fn discard_quarantined(
        &mut self,
        target: QuarantineTargetV1,
    ) -> AnyResult<Result<QuarantineDiscardAck, SearchPlaneIpcError>> {
        let response = self.dispatch_control_response_v1(
            SearchPlaneControlIpcRequest::QuarantineDiscard(QuarantineDiscardRequest { target }),
        )?;
        match response {
            SearchPlaneControlIpcResponse::QuarantineDiscardAck(ack) => Ok(Ok(ack)),
            SearchPlaneControlIpcResponse::Error(error) => Ok(Err(error)),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => Err(anyhow::anyhow!(
                "e2e-harness: quarantine discard returned an unexpected control response: {other:?}"
            )),
        }
    }

    /// The daemon's metrics snapshot, scraped over its public control UDS
    /// exactly as `searchctl metrics` does (QI-BB-015).
    pub fn metrics_snapshot(&mut self) -> AnyResult<MetricsSnapshotV1> {
        let response = self.dispatch_control(SearchPlaneControlIpcRequest::MetricsSnapshot(
            MetricsSnapshotRequest,
        ))?;
        let SearchPlaneControlIpcResponse::MetricsSnapshot(snapshot) = response else {
            return Err(anyhow::anyhow!(
                "e2e-harness: metrics scrape returned an unexpected control response: {response:?}"
            ));
        };
        Ok(snapshot)
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

    /// The composite identity of the last sealed batch as its receipt
    /// attested it — tracks and semantic content roots (QI-BB-028) — for
    /// tests that activate through the control socket themselves.
    #[must_use]
    pub fn last_sealed_search_corpus_identity(&self) -> Option<SearchCorpusGenerationIdentityV1> {
        self.last_sealed_search_corpus_identity.clone()
    }

    /// The daemon's readiness report for the harness's pair: every active
    /// track and the semantic content roots the pair was activated under.
    pub fn generation_status(&mut self) -> AnyResult<GenerationStatusReport> {
        let response = self.dispatch_control(SearchPlaneControlIpcRequest::GenerationStatus(
            GenerationStatusRequest {
                repo_id: self.repo(),
                revision_id: self.revision(),
            },
        ))?;
        if let SearchPlaneControlIpcResponse::GenerationStatusReport(report) = response {
            return Ok(report);
        }
        Err(anyhow::anyhow!(
            "e2e-harness: generation status returned an unexpected control response: {response:?}"
        ))
    }

    /// Send one activation CAS as given and return the daemon's answer
    /// untouched, typed refusals included, for tests that probe the
    /// activation door with a candidate the harness would never build.
    pub fn activate_search_corpus_cas_raw(
        &mut self,
        request: SearchPlaneActivateSearchCorpusGenerationCasRequest,
    ) -> AnyResult<SearchPlaneControlIpcResponse> {
        self.dispatch_control_response_v1(
            SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(request),
        )
    }

    /// Send one rollback CAS as given and return the daemon's answer
    /// untouched, typed refusals included, for tests that probe the
    /// rollback gate (QI-BB-026).
    pub fn rollback_search_corpus_cas_raw(
        &mut self,
        request: SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    ) -> AnyResult<SearchPlaneControlIpcResponse> {
        self.dispatch_control_response_v1(
            SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(request),
        )
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
                    SearchPlaneControlIpcResponse::Error(error)
                        if error.code == SearchPlaneErrorCodeV2::NotReady =>
                    {
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
                    | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
                    | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
                    | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
                    | SearchPlaneControlIpcResponse::QuarantineInventory(_)
                    | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
                    | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => {
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
                // The semantic content roots the active pair was activated
                // under (QI-BB-028), read from the status report: the CAS
                // expectation must name them exactly.
                let semantic_content = self
                    .active_semantic_content_from_control_v1(repo_id, revision_id)?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "e2e-harness: the daemon reports an active pair without semantic content roots"
                        )
                    })?;
                let identity = SearchCorpusGenerationIdentityV1 {
                    lexical,
                    semantic,
                    semantic_content,
                };
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

    /// The active composite root's semantic content roots, as the status
    /// report names them; `None` when the pair has no active root.
    fn active_semantic_content_from_control_v1(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> AnyResult<Option<SemanticContentRootsV1>> {
        let response = self.dispatch_control_response_v1(
            SearchPlaneControlIpcRequest::GenerationStatus(GenerationStatusRequest {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
            }),
        )?;
        match response {
            SearchPlaneControlIpcResponse::GenerationStatusReport(report) => {
                if report.repo_id != *repo_id || report.revision_id != *revision_id {
                    return Err(anyhow::anyhow!(
                        "e2e-harness: generation status response does not match requested authority"
                    ));
                }
                Ok(report.semantic_content)
            }
            SearchPlaneControlIpcResponse::Error(error) => Err(anyhow::anyhow!(
                "e2e-harness generation status failed code={} message={}",
                error.code,
                error.message
            )),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => Err(anyhow::anyhow!(
                "e2e-harness: generation status returned an unexpected control response: {other:?}"
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
                let source_repo_id =
                    chunk
                        .source_repo_id
                        .map(RepoId::new)
                        .transpose()
                        .map_err(|error| {
                            anyhow::anyhow!("e2e harness source repo ID is invalid: {error}")
                        })?;
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
                batch_digest: String::new(),
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
                    let source_repo_id = chunk
                        .source_repo_id
                        .map(RepoId::new)
                        .transpose()
                        .map_err(|error| {
                            anyhow::anyhow!("e2e harness source repo ID is invalid: {error}")
                        })?;
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
                batch_digest: String::new(),
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
                batch_digest: String::new(),
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
        let tree = Self::structural_function_tree_record(path, content, identifier)?;
        self.ingest_structural_tree(path, tree)
    }

    /// The one-function parse tree `ingest_structural_function_tree`
    /// publishes for `content` at `path`: a `function_item` root whose
    /// `identifier` child spans `identifier` and whose `block` child
    /// closes the body.
    pub fn structural_function_tree_record(
        path: &str,
        content: &str,
        identifier: &str,
    ) -> AnyResult<ParseTreeRecord> {
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
        Ok(ParseTreeRecord {
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
        })
    }

    pub fn ingest_structural_tree(&mut self, path: &str, tree: ParseTreeRecord) -> AnyResult<()> {
        let batch = self.structural_tree_batch(path, tree, self.current_generation())?;
        self.publish_structural_batch(batch)
    }

    /// The unsealed one-scope structural batch that publishes `tree` for
    /// the lexical chunk the harness recorded at `path`, into
    /// `generation` — the current one for an ingest before the seal, a
    /// sealed one for a producer that keeps publishing trees for an
    /// active generation.
    pub fn structural_tree_batch(
        &self,
        path: &str,
        tree: ParseTreeRecord,
        generation: ManifestGeneration,
    ) -> AnyResult<StructuralIngestBatch> {
        let chunk_id = self.chunk_ids_by_path.get(path).cloned().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: no lexical chunk recorded for structural path `{path}`")
        })?;
        Ok(StructuralIngestBatch {
            repo_id: self.repo(),
            revision_id: self.revision(),
            generation,
            base_generation: None,
            manifest_digest: format!("struct:{path}:{}", generation.get()),
            batch_digest: String::new(),
            mode: BatchIngestMode::Delta,
            replace_scopes: vec![StructuralReplaceScope {
                scope: scope_key(path),
                scope_digest: format!("struct-scope:{path}:{}", self.next_batch_sequence()),
                trees: vec![StructuralTreeRecord {
                    chunk_id,
                    record: tree,
                }],
            }],
            tombstone_scopes: Vec::new(),
            seal: false,
        })
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
                batch_digest: String::new(),
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
        let batch = self.dirty_batch(path, applied_at_ms, self.current_generation())?;
        self.publish_dirty_batch(batch)
    }

    /// The one-entry dirty batch that marks the lexical chunk the harness
    /// recorded at `path` dirty at `applied_at_ms`, into `generation` —
    /// the current one for an ingest before the seal, a sealed one for a
    /// producer that keeps publishing the overlay of an active
    /// generation.
    pub fn dirty_batch(
        &self,
        path: &str,
        applied_at_ms: u64,
        generation: ManifestGeneration,
    ) -> AnyResult<quanta_index_contract::DirtyIngestBatch> {
        use quanta_index_contract::lex::DirtyRecord;
        use quanta_index_contract::{DirtyIngestBatch, DirtyMutation};

        let chunk_id = self.chunk_ids_by_path.get(path).cloned().ok_or_else(|| {
            anyhow::anyhow!("e2e-harness: no lexical chunk recorded for dirty path `{path}`")
        })?;
        Ok(DirtyIngestBatch {
            repo_id: self.repo(),
            revision_id: self.revision(),
            generation,
            overlay_epoch_ms: applied_at_ms,
            batch_digest: String::new(),
            entries: vec![DirtyMutation::Upsert(DirtyRecord {
                wire_version: 1,
                doc_id: chunk_id,
                applied_at_ms,
                payload_hash: [0x5a; 32],
            })],
        })
    }

    pub fn publish_dirty_batch(
        &mut self,
        batch: quanta_index_contract::DirtyIngestBatch,
    ) -> AnyResult<()> {
        self.dispatch_ingest(SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch))
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
                batch_digest: String::new(),
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
                batch_digest: String::new(),
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
                batch_digest: String::new(),
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
                batch_digest: String::new(),
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
                batch_digest: String::new(),
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
                    batch_digest: String::new(),
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

    /// The first history page in recency order.
    ///
    /// Recency is the order every history fixture of this harness was
    /// written against.
    pub fn query_history(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
    ) -> E2eHistoryResult {
        self.query_history_page(syntax, query_text, top_k, HistoryOrderV1::Recency, None)
    }

    /// One history page in `order` after `cursor` (QI-BB-023).
    pub fn query_history_page(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
        order: HistoryOrderV1,
        cursor: Option<ContinuationTokenV2>,
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
                    cursor: None,
                },
                order,
                cursor,
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eHistoryResult {
                    commit_ids: Vec::new(),
                    diff_paths: Vec::new(),
                    order: None,
                    scores: Vec::new(),
                    window: None,
                    read_epoch: None,
                    examined: 0,
                    next_cursor: None,
                    typed_error: Some(E2eTypedError {
                        code: E2eErrorCode::HarnessStart,
                        message: err.to_string(),
                    }),
                };
            }
        };
        let response =
            wait_for_query_response(&socket, &envelope, self.client_io, query_response_ready);
        let response: SearchPlaneQueryIpcResponseEnvelope = match response {
            Ok(r) => r,
            Err(err) => {
                let query_result = self.semantic_transport_error(err);
                return E2eHistoryResult {
                    commit_ids: Vec::new(),
                    diff_paths: Vec::new(),
                    order: None,
                    scores: Vec::new(),
                    window: None,
                    read_epoch: None,
                    examined: 0,
                    next_cursor: None,
                    typed_error: query_result.typed_error,
                };
            }
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::History(history) => E2eHistoryResult {
                scores: history
                    .commits
                    .iter()
                    .map(|candidate| candidate.score)
                    .chain(history.diffs.iter().map(|candidate| candidate.score))
                    .collect(),
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
                order: Some(history.order),
                window: Some(history.window),
                read_epoch: Some(history.read_epoch),
                examined: history.examined,
                next_cursor: history.next_cursor,
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eHistoryResult {
                commit_ids: Vec::new(),
                diff_paths: Vec::new(),
                order: None,
                scores: Vec::new(),
                window: None,
                read_epoch: None,
                examined: 0,
                next_cursor: None,
                typed_error: Some(E2eTypedError {
                    code: E2eErrorCode::Remote(err.code),
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
        match self.query_runtime_metadata_page(syntax, query_text, top_k, None) {
            Ok(E2eRoutePage::Served(runtime)) => E2eQueryResult {
                candidate_ids: runtime
                    .results
                    .iter()
                    .map(|candidate| candidate.candidate_id.clone())
                    .collect(),
                candidates: runtime.results,
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                hybrid_candidates: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: None,
            },
            Ok(E2eRoutePage::Refused(error)) => refused_query_result(error),
            Err(err) => refused_query_result(E2eTypedError {
                code: E2eErrorCode::HarnessStart,
                message: err.to_string(),
            }),
        }
    }

    /// One runtime-metadata page after `cursor` (QI-BB-025 W4), pinned
    /// to the last sealed generation.
    pub fn query_runtime_metadata_page(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
        cursor: Option<ContinuationTokenV2>,
    ) -> AnyResult<E2eRoutePage<SearchPlaneRuntimeMetadataQueryResponse>> {
        let pin = self.last_sealed_pin();
        self.keyset_page_query(
            SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
                text_query: TextQueryRequest {
                    syntax,
                    query_text: query_text.to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: pin,
                    generation_selector: None,
                    top_k,
                    cursor: None,
                },
                cursor,
            }),
            query_response_ready,
            |payload| match payload {
                SearchPlaneQueryIpcResponse::RuntimeMetadata(page) => Ok(page),
                other @ (SearchPlaneQueryIpcResponse::Text(_)
                | SearchPlaneQueryIpcResponse::Symbol(_)
                | SearchPlaneQueryIpcResponse::Semantic(_)
                | SearchPlaneQueryIpcResponse::Hybrid(_)
                | SearchPlaneQueryIpcResponse::HybridSeed(_)
                | SearchPlaneQueryIpcResponse::History(_)
                | SearchPlaneQueryIpcResponse::Structural(_)
                | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                | SearchPlaneQueryIpcResponse::Explain(_)
                | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
                | SearchPlaneQueryIpcResponse::Error(_)) => Err(query_response_kind(&other)),
            },
        )
    }

    /// One ranked text page after `cursor` (QI-BB-005), pinned to `pin`.
    pub fn query_text_page(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
        pin: Option<GenerationPin>,
        cursor: Option<ContinuationTokenV2>,
    ) -> AnyResult<E2eRoutePage<TextQueryResponse>> {
        self.keyset_page_query(
            SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax,
                query_text: query_text.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: pin,
                generation_selector: None,
                top_k,
                cursor,
            }),
            query_response_ready,
            |payload| match payload {
                SearchPlaneQueryIpcResponse::Text(page) => Ok(page),
                other @ (SearchPlaneQueryIpcResponse::Symbol(_)
                | SearchPlaneQueryIpcResponse::Semantic(_)
                | SearchPlaneQueryIpcResponse::Hybrid(_)
                | SearchPlaneQueryIpcResponse::HybridSeed(_)
                | SearchPlaneQueryIpcResponse::History(_)
                | SearchPlaneQueryIpcResponse::Structural(_)
                | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                | SearchPlaneQueryIpcResponse::Explain(_)
                | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
                | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
                | SearchPlaneQueryIpcResponse::Error(_)) => Err(query_response_kind(&other)),
            },
        )
    }

    /// One structural page after `cursor` (QI-BB-025 W4), pinned to the
    /// last sealed generation.
    pub fn query_structural_page(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
        cursor: Option<ContinuationTokenV2>,
    ) -> AnyResult<E2eRoutePage<SearchPlaneStructuralQueryResponse>> {
        let pin = self.last_sealed_pin();
        self.structural_page_with_pin(syntax, query_text, top_k, pin, cursor)
    }

    fn structural_page_with_pin(
        &mut self,
        syntax: TextQuerySyntax,
        query_text: &str,
        top_k: u32,
        pin: Option<GenerationPin>,
        cursor: Option<ContinuationTokenV2>,
    ) -> AnyResult<E2eRoutePage<SearchPlaneStructuralQueryResponse>> {
        self.keyset_page_query(
            SearchPlaneQueryIpcRequest::Structural(StructuralQueryRequest {
                text_query: TextQueryRequest {
                    syntax,
                    query_text: query_text.to_string(),
                    constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                    generation: pin,
                    generation_selector: None,
                    top_k,
                    cursor: None,
                },
                cursor,
            }),
            query_response_ready_allow_structural_not_ready,
            |payload| match payload {
                SearchPlaneQueryIpcResponse::Structural(page) => Ok(page),
                other @ (SearchPlaneQueryIpcResponse::Text(_)
                | SearchPlaneQueryIpcResponse::Symbol(_)
                | SearchPlaneQueryIpcResponse::Semantic(_)
                | SearchPlaneQueryIpcResponse::Hybrid(_)
                | SearchPlaneQueryIpcResponse::HybridSeed(_)
                | SearchPlaneQueryIpcResponse::History(_)
                | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
                | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
                | SearchPlaneQueryIpcResponse::Explain(_)
                | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
                | SearchPlaneQueryIpcResponse::Error(_)) => Err(query_response_kind(&other)),
            },
        )
    }

    /// Issue one keyset page query and return the daemon's typed page or
    /// refusal; `extract` names the response variant the route answers
    /// with, any other variant being a harness error.
    fn keyset_page_query<R>(
        &mut self,
        payload: SearchPlaneQueryIpcRequest,
        ready: impl Fn(&SearchPlaneQueryIpcResponseEnvelope) -> bool,
        extract: impl FnOnce(SearchPlaneQueryIpcResponse) -> Result<R, &'static str>,
    ) -> AnyResult<E2eRoutePage<R>> {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload,
        };
        let socket = self.ensure_driver()?;
        let response = wait_for_query_response(&socket, &envelope, self.client_io, ready);
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                let failure = self.semantic_transport_error(err);
                return Err(anyhow::anyhow!(
                    "e2e-harness: keyset page transport failure: {}",
                    failure
                        .typed_error
                        .map_or_else(|| "no typed error".to_string(), |error| error.to_string())
                ));
            }
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Error(err) => Ok(E2eRoutePage::Refused(E2eTypedError {
                code: E2eErrorCode::Remote(err.code),
                message: err.message,
            })),
            payload @ (SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)) => {
                extract(payload).map(E2eRoutePage::Served).map_err(|kind| {
                    anyhow::anyhow!("e2e-harness: keyset page route answered with {kind}")
                })
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
                cursor: None,
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
                    hybrid_candidates: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: E2eErrorCode::HarnessStart,
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
        let response = wait_for_query_response(
            &socket,
            &envelope,
            self.client_io,
            query_response_ready_allow_structural_not_ready,
        );
        let response: SearchPlaneQueryIpcResponseEnvelope = match response {
            Ok(r) => r,
            Err(err) => return self.semantic_transport_error(err),
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
                hybrid_candidates: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                hybrid_candidates: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: E2eErrorCode::Remote(err.code),
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
        match self.structural_page_with_pin(syntax, query_text, top_k, pin, None) {
            Ok(E2eRoutePage::Served(structural)) => {
                let results = structural.results;
                E2eQueryResult {
                    candidate_ids: results
                        .iter()
                        .map(|candidate| candidate.candidate_id.clone())
                        .collect(),
                    candidates: Vec::new(),
                    file_owner_rows: Vec::new(),
                    structural_results: results,
                    hybrid_candidates: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: None,
                }
            }
            Ok(E2eRoutePage::Refused(error)) => refused_query_result(error),
            Err(err) => refused_query_result(E2eTypedError {
                code: E2eErrorCode::HarnessStart,
                message: err.to_string(),
            }),
        }
    }

    pub fn query_semantic(
        &mut self,
        query_text: &str,
        top_k: u32,
        lexical_scope: Option<(TextQuerySyntax, &str, u32)>,
    ) -> E2eQueryResult {
        let pin = self.last_sealed_pin();
        self.query_semantic_pinned(query_text, top_k, lexical_scope, pin)
    }

    /// [`Self::query_semantic`] at an explicit pin instead of the last
    /// sealed generation; the lexical scope, when given, pins the same.
    pub fn query_semantic_with_pin(
        &mut self,
        query_text: &str,
        top_k: u32,
        pin: Option<GenerationPin>,
    ) -> E2eQueryResult {
        self.query_semantic_pinned(query_text, top_k, None, pin)
    }

    fn query_semantic_pinned(
        &mut self,
        query_text: &str,
        top_k: u32,
        lexical_scope: Option<(TextQuerySyntax, &str, u32)>,
        pin: Option<GenerationPin>,
    ) -> E2eQueryResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let lexical_scope =
            lexical_scope.map(|(syntax, scope_text, scope_top_k)| TextQueryRequest {
                syntax,
                query_text: scope_text.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: pin.clone(),
                generation_selector: None,
                top_k: scope_top_k,
                cursor: None,
            });
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: query_text.to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: pin,
                generation_selector: None,
                lexical_scope,
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
                    hybrid_candidates: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: E2eErrorCode::HarnessStart,
                        message: err.to_string(),
                    }),
                };
            }
        };
        let response =
            wait_for_query_response(&socket, &envelope, self.client_io, query_response_ready);
        let response: SearchPlaneQueryIpcResponseEnvelope = match response {
            Ok(r) => r,
            Err(err) => return self.semantic_transport_error(err),
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
                hybrid_candidates: Vec::new(),
                engines_touched: semantic.explanation.engines_touched.clone(),
                explanation: Some(semantic.explanation),
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                hybrid_candidates: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: E2eErrorCode::Remote(err.code),
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
                    cursor: None,
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
                    hybrid_candidates: Vec::new(),
                    engines_touched: Vec::new(),
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: E2eErrorCode::HarnessStart,
                        message: err.to_string(),
                    }),
                };
            }
        };
        let response =
            wait_for_query_response(&socket, &envelope, self.client_io, query_response_ready);
        let response: SearchPlaneQueryIpcResponseEnvelope = match response {
            Ok(r) => r,
            Err(err) => return self.semantic_transport_error(err),
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Hybrid(hybrid) => E2eQueryResult {
                candidate_ids: hybrid
                    .results
                    .iter()
                    .map(|row| row.candidate.candidate_id.clone())
                    .collect(),
                candidates: hybrid
                    .results
                    .iter()
                    .map(|row| row.candidate.clone())
                    .collect(),
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                hybrid_candidates: hybrid.results,
                engines_touched: hybrid.explanation.engines_touched.clone(),
                explanation: Some(hybrid.explanation),
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eQueryResult {
                candidates: Vec::new(),
                candidate_ids: Vec::new(),
                file_owner_rows: Vec::new(),
                structural_results: Vec::new(),
                hybrid_candidates: Vec::new(),
                engines_touched: Vec::new(),
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: E2eErrorCode::Remote(err.code),
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
        let response =
            wait_for_query_response(&socket, &envelope, self.client_io, query_response_ready);
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                let failure = self.semantic_transport_error(err);
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
            self.client_io,
        )?;
        Ok(response.payload)
    }

    pub fn candidate_id_for_path(&self, path: &str) -> AnyResult<String> {
        self.chunk_ids_by_path
            .get(path)
            .map(|chunk_id| chunk_id.as_str().to_string())
            .ok_or_else(|| anyhow::anyhow!("e2e-harness: no chunk id recorded for path `{path}`"))
    }

    fn semantic_transport_error(&mut self, err: IpcError) -> E2eQueryResult {
        let mut message = err.to_string();
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
            hybrid_candidates: Vec::new(),
            engines_touched: Vec::new(),
            explanation: None,
            typed_error: Some(E2eTypedError {
                code: E2eErrorCode::IpcTransport,
                message,
            }),
        }
    }

    /// Presence-only explain: is the candidate in its generation's index?
    pub fn explain_candidate(&mut self, candidate: LexicalCandidate) -> E2eExplainResult {
        self.explain_candidate_request(ExplainCandidateV1::Lexical(candidate), None, None)
    }

    /// Scored explain (QI-BB-022): a lexical row's score under the named
    /// query, traced through the plan that ranked it.
    pub fn explain_candidate_under_query(
        &mut self,
        candidate: LexicalCandidate,
        syntax: TextQuerySyntax,
        query_text: &str,
    ) -> E2eExplainResult {
        let text_query = TextQueryRequest {
            syntax,
            query_text: query_text.to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        };
        self.explain_candidate_request(
            ExplainCandidateV1::Lexical(candidate),
            Some(text_query),
            None,
        )
    }

    /// Hybrid explain (QI-BB-022): a hybrid row re-derived lane by lane
    /// under both queries it was fused for. `top_k` is the fused `top_k`
    /// the hybrid ran with, so the explain re-runs the lanes at the same
    /// bound.
    pub fn explain_hybrid_candidate_under_queries(
        &mut self,
        row: HybridCandidateV1,
        syntax: TextQuerySyntax,
        query_text: &str,
        semantic_query_text: &str,
        top_k: u32,
    ) -> E2eExplainResult {
        let text_query = TextQueryRequest {
            syntax,
            query_text: query_text.to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k,
            cursor: None,
        };
        self.explain_candidate_request(
            ExplainCandidateV1::Hybrid(row),
            Some(text_query),
            Some(semantic_query_text.to_string()),
        )
    }

    fn explain_candidate_request(
        &mut self,
        candidate: ExplainCandidateV1,
        text_query: Option<TextQueryRequest>,
        semantic_query_text: Option<String>,
    ) -> E2eExplainResult {
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let row = candidate.lexical_row();
        let pin = GenerationPin::new(
            row.repo_id.clone(),
            row.revision_id.clone(),
            row.manifest_generation,
        );
        let envelope = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Explain(SearchPlaneExplainQueryRequest {
                generation: pin,
                candidate,
                text_query,
                semantic_query_text,
            }),
        };
        let socket = match self.ensure_driver() {
            Ok(socket) => socket,
            Err(err) => {
                return E2eExplainResult {
                    presence: None,
                    explanation: None,
                    typed_error: Some(E2eTypedError {
                        code: E2eErrorCode::HarnessStart,
                        message: err.to_string(),
                    }),
                };
            }
        };
        let response =
            wait_for_query_response(&socket, &envelope, self.client_io, query_response_ready);
        let response: SearchPlaneQueryIpcResponseEnvelope = match response {
            Ok(r) => r,
            Err(err) => return explain_transport_error(err),
        };
        match response.payload {
            SearchPlaneQueryIpcResponse::Explain(explain) => E2eExplainResult {
                presence: Some(explain.presence),
                explanation: Some(explain.explanation),
                typed_error: None,
            },
            SearchPlaneQueryIpcResponse::Error(err) => E2eExplainResult {
                presence: None,
                explanation: None,
                typed_error: Some(E2eTypedError {
                    code: E2eErrorCode::Remote(err.code),
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
        // The roots the plane sealed, attested on the receipt (QI-BB-028);
        // a sealed receipt without them is a daemon defect, never guessed.
        let semantic_content = receipt.semantic_content.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "e2e-harness: sealed search-corpus receipt attests no semantic content roots"
            )
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
            semantic_content,
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
            send_request(&socket, &envelope, self.client_io)?;
        Ok(response.payload)
    }

    /// Build, without publishing, the unsealed single-chunk search-corpus
    /// batch `ingest_text` would publish for `path`, stamped with its
    /// canonical digest, so a test can publish the same body more than once
    /// through [`Self::ingest_once`] and observe the replay.
    pub fn text_search_corpus_batch(
        &self,
        path: &str,
        content: &str,
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
        let mut batch = SearchCorpusIngestBatch {
            repo_id: self.repo(),
            revision_id: self.revision(),
            generation: self.current_generation(),
            base_generation,
            manifest_digest: format!("lex:{path}:{}", self.current_generation().get()),
            batch_digest: String::new(),
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
        };
        stamp_batch_digest_v1(&mut batch)?;
        Ok(batch)
    }

    /// Publish `payload` as a producer would: like the SDK, the harness
    /// stamps the canonical batch digest on every receipt-bearing batch it
    /// sends (QI-BB-032), so harness-built batches and test-supplied
    /// batches alike name their body. A test that must send a batch
    /// verbatim (a forged digest, for instance) uses
    /// [`Self::ingest_once`], which interprets nothing.
    fn dispatch_ingest_response(
        &mut self,
        payload: SearchPlaneIngestIpcRequest,
    ) -> AnyResult<SearchPlaneIngestIpcResponse> {
        let payload = stamped_ingest_request(payload)?;
        let socket = self.ensure_ingest_socket()?;
        let request_id = self.request_id_counter.fetch_add(1, Ordering::Relaxed);
        let envelope = SearchPlaneIngestIpcRequestEnvelope {
            request_id,
            payload,
        };
        let response: SearchPlaneIngestIpcResponseEnvelope =
            send_request(&socket, &envelope, self.client_io)?;
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
            send_request(&socket, &envelope, self.client_io)?;
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
        presence: None,
        explanation: None,
        typed_error: Some(E2eTypedError {
            code: E2eErrorCode::UnexpectedResponse,
            message: format!("expected Explain, got {kind}"),
        }),
    }
}

fn unexpected_history_response(kind: &str) -> E2eHistoryResult {
    E2eHistoryResult {
        commit_ids: Vec::new(),
        diff_paths: Vec::new(),
        order: None,
        scores: Vec::new(),
        window: None,
        read_epoch: None,
        examined: 0,
        next_cursor: None,
        typed_error: Some(E2eTypedError {
            code: E2eErrorCode::UnexpectedResponse,
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
        SearchPlaneQueryIpcResponse::Error(err) => err.code != SearchPlaneErrorCodeV2::NotReady,
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
            err.code != SearchPlaneErrorCodeV2::NotReady
                && err.code != SearchPlaneErrorCodeV2::StrGenerationNotReady
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

/// One-line payload identity for timeout evidence: the variant name,
/// plus the typed code for refusals. Never the message body.
fn describe_query_response_payload(payload: &SearchPlaneQueryIpcResponse) -> String {
    match payload {
        SearchPlaneQueryIpcResponse::Error(err) => format!("Error({:?})", err.code),
        SearchPlaneQueryIpcResponse::Text(_) => "Text".to_string(),
        SearchPlaneQueryIpcResponse::Symbol(_) => "Symbol".to_string(),
        SearchPlaneQueryIpcResponse::Semantic(_) => "Semantic".to_string(),
        SearchPlaneQueryIpcResponse::Hybrid(_) => "Hybrid".to_string(),
        SearchPlaneQueryIpcResponse::HybridSeed(_) => "HybridSeed".to_string(),
        SearchPlaneQueryIpcResponse::History(_) => "History".to_string(),
        SearchPlaneQueryIpcResponse::Structural(_) => "Structural".to_string(),
        SearchPlaneQueryIpcResponse::RepoMapQuery(_) => "RepoMapQuery".to_string(),
        SearchPlaneQueryIpcResponse::Explain(_) => "Explain".to_string(),
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => {
            "ClusterMembershipRead".to_string()
        }
        SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => "RuntimeMetadata".to_string(),
    }
}

/// Cap a last-observation string so timeout evidence stays one line,
/// never a dumped response body.
fn cap_observation(observed: String) -> String {
    const CAP_CHARS: usize = 1000;
    if observed.chars().count() <= CAP_CHARS {
        return observed;
    }
    let prefix: String = observed.chars().take(CAP_CHARS).collect();
    format!("{prefix}…[truncated]")
}

/// Preserve both the caller's per-request limit and the readiness window.
/// Every IPC attempt receives the earliest deadline, never a fresh full
/// request timeout after the readiness window has already been spent.
fn readiness_attempt_deadline(
    client_io: ClientIoPolicy,
    now: Instant,
    readiness_deadline: Instant,
) -> Instant {
    let request_deadline = now
        .checked_add(client_io.request_timeout())
        .unwrap_or(readiness_deadline);
    readiness_deadline
        .min(request_deadline)
        .min(client_io.absolute_deadline().unwrap_or(readiness_deadline))
}

fn wait_for_query_response(
    socket: &Path,
    envelope: &SearchPlaneQueryIpcRequestEnvelope,
    client_io: ClientIoPolicy,
    ready: impl Fn(&SearchPlaneQueryIpcResponseEnvelope) -> bool,
) -> Result<SearchPlaneQueryIpcResponseEnvelope, IpcError> {
    let readiness_deadline = Instant::now()
        .checked_add(READINESS_TIMEOUT)
        .expect("the bounded readiness deadline is representable");
    let mut attempts = 0_u64;
    let mut last = String::from("no attempt completed");
    loop {
        let now = Instant::now();
        if now >= readiness_deadline {
            break;
        }
        let attempt_deadline = readiness_attempt_deadline(client_io, now, readiness_deadline);
        let attempt_io = match ClientIoPolicy::try_with_deadline(attempt_deadline) {
            Ok(policy) => policy,
            Err(_) if Instant::now() >= readiness_deadline => break,
            Err(error) if client_io.absolute_deadline().is_some() => return Err(error),
            Err(error) => {
                // A stricter per-attempt budget can expire while this
                // thread is descheduled. It does not spend the shared
                // readiness window or authorize a terminal response.
                last = format!("transport error: {error}");
                continue;
            }
        };
        attempts = attempts.saturating_add(1);
        match send_request::<_, SearchPlaneQueryIpcResponseEnvelope>(socket, envelope, attempt_io) {
            Ok(response) => {
                let observed = describe_query_response_payload(&response.payload);
                if ready(&response) {
                    if Instant::now() < readiness_deadline {
                        return Ok(response);
                    }
                    last = format!("ready after deadline {observed}");
                    break;
                }
                last = format!("not-ready {observed}");
            }
            Err(transport_error) => {
                last = format!("transport error: {transport_error}");
            }
        }
        let remaining = readiness_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        thread::sleep(READINESS_POLL_INTERVAL.min(remaining));
    }
    Err(IpcError::ReadinessTimeout {
        timeout: READINESS_TIMEOUT,
        attempts,
        last: cap_observation(last),
    })
}

#[cfg(test)]
mod readiness_deadline_tests {
    use std::time::{Duration, Instant};

    use quanta_index_ipc::ClientIoPolicy;

    use super::readiness_attempt_deadline;

    #[test]
    fn every_attempt_uses_the_earliest_owner_deadline() {
        let now = Instant::now();
        let readiness_deadline = now + Duration::from_secs(15);
        let long_io = ClientIoPolicy::try_new(Duration::from_secs(30))
            .expect("a positive request timeout is valid");
        assert_eq!(
            readiness_attempt_deadline(long_io, now, readiness_deadline),
            readiness_deadline,
            "a 30-second IPC limit cannot extend a 15-second readiness window"
        );

        let short_io = ClientIoPolicy::try_new(Duration::from_secs(2))
            .expect("a positive request timeout is valid");
        assert_eq!(
            readiness_attempt_deadline(short_io, now, readiness_deadline),
            now + Duration::from_secs(2),
            "a stricter per-request limit remains effective"
        );

        let caller_deadline = now + Duration::from_secs(3_600);
        let absolute_io = ClientIoPolicy::try_with_deadline(caller_deadline)
            .expect("a future absolute deadline is valid");
        assert_eq!(
            readiness_attempt_deadline(
                absolute_io,
                Instant::now(),
                now + Duration::from_secs(7_200)
            ),
            caller_deadline,
            "a caller's absolute deadline remains effective"
        );
    }
}

fn explain_transport_error(err: IpcError) -> E2eExplainResult {
    let message = err.to_string();
    E2eExplainResult {
        presence: None,
        explanation: None,
        typed_error: Some(E2eTypedError {
            code: E2eErrorCode::IpcTransport,
            message,
        }),
    }
}

fn start_driver(spec: &DriverSpec<'_>) -> AnyResult<DriverHandles> {
    let config = build_config(spec)?;
    let runtime = build_runtime_with_memory_probe(config, Arc::clone(spec.memory_probe))?;
    let query_socket = runtime.query_server.socket_path().to_path_buf();
    let control_socket = runtime.control_server.socket_path().to_path_buf();
    let ingest_socket = runtime.ingest_server.socket_path().to_path_buf();
    let query_obs_store = Arc::clone(&runtime.query_obs_store);
    let boot_inventory = runtime.boot_inventory.clone();
    let semantic_boot = runtime.semantic_boot;
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_drive = Arc::clone(&shutdown);
    let join = thread::Builder::new()
        .name("e2e-harness-driver".into())
        .spawn(move || drive(runtime, &shutdown_for_drive))?;
    let mut last_seen = (false, false, false);
    if !wait_until(SOCKET_APPEAR_TIMEOUT, SOCKET_APPEAR_POLL_INTERVAL, || {
        last_seen = (
            query_socket.exists(),
            control_socket.exists(),
            ingest_socket.exists(),
        );
        last_seen.0 && last_seen.1 && last_seen.2
    }) {
        shutdown.store(true, Ordering::Release);
        let socket_failure = format!(
            "e2e-harness: sockets never appeared query={} (present={}) control={} (present={}) ingest={} (present={})",
            query_socket.display(),
            last_seen.0,
            control_socket.display(),
            last_seen.1,
            ingest_socket.display(),
            last_seen.2
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
        semantic_boot,
    ))
}

const DEFAULT_HISTORY_MAX_GENERATIONS: usize = 8;
/// Index bytes the retained generations of a pair may hold together in a
/// harness daemon unless a test widens it.
const HARNESS_HISTORY_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// The maintenance tick every harness daemon runs on: short enough that
/// an idle sweep or disk refresh lands within a test's bounded wait.
const HARNESS_MAINTENANCE_TICK: Duration = Duration::from_millis(50);
/// The integrity scrub interval a harness daemon boots with unless a test
/// paces the scrub itself: a day, longer than any test runs.
const HARNESS_DORMANT_SCRUB_INTERVAL_MILLIS: u64 = 24 * 60 * 60 * 1_000;

fn build_config(spec: &DriverSpec<'_>) -> AnyResult<SearchdConfig> {
    let mut cfg = SearchdConfig::from_test_state_root(spec.state_root.to_path_buf())
        .try_with_search_corpus_history_retention_limits(
            spec.history_max_generations,
            spec.history_max_bytes,
            128,
            256 * 1024 * 1024,
        )?;
    let (query_socket, control_socket, ingest_socket) =
        spec.socket_directory
            .map_or_else(unique_socket_paths, |directory| {
                (
                    directory.join("query.sock"),
                    directory.join("control.sock"),
                    directory.join("ingest.sock"),
                )
            });
    cfg = SearchdConfig::with_socket_overrides(cfg, query_socket, control_socket);
    cfg = SearchdConfig::with_ingest_socket_override(cfg, ingest_socket);
    Ok(cfg
        .with_semantic_embedder_profile(spec.embedder_profile.clone())
        .with_ingest_resource_policy(spec.ingest_resource_policy)
        .with_semantic_stream_window_policy(spec.semantic_stream_window_policy)
        .with_integrity_scrub_policy(spec.integrity_scrub_policy)
        .with_query_response_budget(spec.query_response_budget)
        .with_query_admission_policy(spec.query_admission_policy)
        .with_lexical_writer_policy(spec.lexical_writer_policy)
        .with_process_memory_ceilings(spec.process_memory_ceilings)
        .with_maintenance_policy(spec.maintenance_policy)
        .with_socket_access_policies(spec.socket_access.clone()))
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
                    code: E2eErrorCode::Remote(err.code),
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
        SearchPlaneQueryIpcResponse::HybridSeed(seed) => {
            (seed.seed_candidates.len(), Some(seed.window))
        }
        SearchPlaneQueryIpcResponse::History(history) => (
            history.commits.len().saturating_add(history.diffs.len()),
            Some(history.window),
        ),
        SearchPlaneQueryIpcResponse::RuntimeMetadata(runtime) => {
            (runtime.results.len(), Some(runtime.window))
        }
        SearchPlaneQueryIpcResponse::Structural(structural) => {
            (structural.results.len(), Some(structural.window))
        }
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

/// The variant name of one query response, for a harness error that
/// names what a route answered with instead of its own page.
fn query_response_kind(payload: &SearchPlaneQueryIpcResponse) -> &'static str {
    match payload {
        SearchPlaneQueryIpcResponse::Text(_) => "Text",
        SearchPlaneQueryIpcResponse::Symbol(_) => "Symbol",
        SearchPlaneQueryIpcResponse::Semantic(_) => "Semantic",
        SearchPlaneQueryIpcResponse::Hybrid(_) => "Hybrid",
        SearchPlaneQueryIpcResponse::HybridSeed(_) => "HybridSeed",
        SearchPlaneQueryIpcResponse::History(_) => "History",
        SearchPlaneQueryIpcResponse::RuntimeMetadata(_) => "RuntimeMetadata",
        SearchPlaneQueryIpcResponse::Structural(_) => "Structural",
        SearchPlaneQueryIpcResponse::RepoMapQuery(_) => "RepoMapQuery",
        SearchPlaneQueryIpcResponse::Explain(_) => "Explain",
        SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => "ClusterMembershipRead",
        SearchPlaneQueryIpcResponse::Error(_) => "Error",
    }
}

/// An empty query result carrying one typed refusal.
fn refused_query_result(error: E2eTypedError) -> E2eQueryResult {
    E2eQueryResult {
        candidates: Vec::new(),
        candidate_ids: Vec::new(),
        file_owner_rows: Vec::new(),
        structural_results: Vec::new(),
        hybrid_candidates: Vec::new(),
        engines_touched: Vec::new(),
        explanation: None,
        typed_error: Some(error),
    }
}

fn unexpected_response(kind: &str) -> E2eQueryResult {
    E2eQueryResult {
        candidates: Vec::new(),
        candidate_ids: Vec::new(),
        file_owner_rows: Vec::new(),
        structural_results: Vec::new(),
        hybrid_candidates: Vec::new(),
        engines_touched: Vec::new(),
        explanation: None,
        typed_error: Some(E2eTypedError {
            code: E2eErrorCode::UnexpectedResponse,
            message: format!("expected Text, got {kind}"),
        }),
    }
}

/// A fresh, unique directory path under `/tmp` for shared-mode sockets.
///
/// Nothing is created here: the daemon creates it at bind with the
/// policy's mode and group, so its creation path is what the test proves.
fn unique_shared_socket_directory() -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_SOCKET_ID.fetch_add(1, Ordering::Relaxed);
    PathBuf::from("/tmp").join(format!("qi-e2e-sockets-{pid}-{nanos}-{sequence}"))
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

/// A temporary directory a daemon will accept as its state root or socket
/// directory: mode `0700`.
///
/// The daemon refuses a state root or socket directory wider than `0700`
/// (QI-BB-014), and a tempdir is created under the process umask, so every
/// test that points a daemon at a tempdir narrows it through here.
pub fn private_tempdir() -> std::io::Result<tempfile::TempDir> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir()?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

/// Stamp the canonical batch digest on whichever receipt-bearing batch
/// `payload` carries (QI-BB-032); a repo-map bundle names none.
pub fn stamped_ingest_request(
    payload: SearchPlaneIngestIpcRequest,
) -> AnyResult<SearchPlaneIngestIpcRequest> {
    fn stamped<B: quanta_index_core::IngestBatchBodyV1 + serde::Serialize>(
        mut batch: B,
    ) -> AnyResult<B> {
        stamp_batch_digest_v1(&mut batch)?;
        Ok(batch)
    }
    Ok(match payload {
        SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishFileContributorBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(stamped(batch)?)
        }
        SearchPlaneIngestIpcRequest::PublishStructuralBatch(batch) => {
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(stamped(batch)?)
        }
        bundle @ SearchPlaneIngestIpcRequest::PublishRepoMapBundle(_) => bundle,
        request @ SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(_) => request,
    })
}
