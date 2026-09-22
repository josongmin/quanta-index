//! Composed runtime artefacts. Built once per daemon process.

use std::collections::BTreeSet;
use std::fs::File;
#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use std::{
    fs,
    path::{Path, PathBuf},
};

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
    AUX_EPOCH_EXPIRED_CODE, AUX_EPOCH_UNKNOWN_CODE, AuxiliaryAuthorityCatalogPort, CoreError,
    DoorFindingQuarantinePort, FileContributorIngestPort, FileOwnershipIngestPort,
    GenerationIdentityValidatePort, HistoryTextIndexPort, IdempotencyCatalogPort,
    IncompleteGenerationDiscardPort, IntegrityScrubPort, L2UnitEmbeddingProvider,
    LexicalIndexOpenPort, MetricSourcePort, MutationCoordinatorPort, ProcessMemoryProbePort,
    ProviderBudgetLedger, QuarantinedGenerationDiscardPort, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMapBundleIngestPort, RepoMapGenerationActivatePort,
    RepoMapOpenReportV1, RepoMapQuarantinePort, RepoMapSnapshotAcquirePort, RepoMetaIngestPort,
    RepoTopicIngestPort, RequestBudgetV1, SealedGenerationReclaimPort, SealedGenerationScanPort,
    SearchCorpusBatchBuildPort, SearchCorpusIngestPort, SemanticContentRootsPort,
    SemanticEgressPolicyV1, SemanticIndexOpenPort, SemanticIngestPort,
    SemanticScopeStreamBuildPort, StructuralError,
    StructuralMatchBinding, StructuralMatchCandidate, StructuralReadiness, TextEmbeddingProvider,
    TrackDiskUsagePort, WriterIdleSweepPort,
};
use quanta_index_embed::{
    CachingEmbeddingProvider, EmbeddingCacheIdentityV1, FileEmbeddingCache,
    OpenAiEmbedTelemetrySource, OpenAiEmbeddingProvider,
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
    ActivationPromotionParts, AuxiliaryMaterializerParts, AuxiliaryMutationCoordinator,
    BoundedQueryObsStore, CursorKeyStore, DirectHistoryMaterializer,
    DirectRuntimeMetadataMaterializer, DirectSearchCorpusMaterializer, DirectSemanticMaterializer,
    DirectStructuralMaterializer, HashingQueryTextEmbedder, HistoryIngestPort,
    HistoryTextIndexParts, Ledger, ObservabilityScrape, ProviderBoundaryQueryEmbedder,
    QuarantineService, QuarantineServiceParts, QueryObsSink, QueryTextEmbedderPort,
    RuntimeMetadataIngestPort,
    SEARCH_OWNED_SEMANTIC_DIMENSION, SearchCorpusAuthorityInspectPort,
    SearchCorpusAuthorityWritePort, SearchCorpusLifecycleOwner, SearchCorpusLifecycleParts,
    SearchCorpusMaterializerParts, SearchPlaneControlDispatcher, SearchPlaneControlDispatcherParts,
    SearchPlaneDispatcher, SearchPlaneIngestDispatcher, SnapshotRegistries, StructuralIngestPort,
};
use regex::Regex;

use crate::app::boot_inventory::{self, BootInventoryReportV1, HalfSealedPair};
use crate::app::config::{
    ProviderEgressGrantConfig, ProviderWorkBudgetConfig, SearchdConfig, SemanticEmbedderProfile,
};
use crate::app::integrity_scrub::{PacedIntegrityScrubV1, ScrubSchedulerV1, ScrubTalliesV1};
use crate::app::ipc_dispatcher::{
    SearchPlaneControlIpcAdapter, SearchPlaneIngestIpcAdapter, SearchPlaneQueryIpcAdapter,
};
use crate::app::maintenance::{MaintenanceMetricSource, MaintenanceParts, MaintenanceTimer};
use crate::app::semantic_boot;
use crate::app::server::{
    SearchPlaneControlServer, SearchPlaneIngestServer, SearchPlaneQueryServer,
};
use crate::app::socket_access::SocketRole;

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
    pub sem_build_port: Arc<dyn SemanticScopeStreamBuildPort + Send + Sync>,
    pub semantic_generation_scanner: Arc<dyn SealedGenerationScanPort + Send + Sync>,
    pub semantic_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    /// The content roots a sealed semantic generation carries (QI-BB-028):
    /// attested on sealed receipts, proven at activation and rehydrate.
    pub semantic_content_roots: Arc<dyn SemanticContentRootsPort + Send + Sync>,
    pub semantic_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>,
    pub semantic_sealed_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    pub sem_open_port: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    pub repo_map_snapshot_port: Arc<dyn RepoMapSnapshotAcquirePort + Send + Sync>,
    pub repo_map_bundle_ingest_port: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    pub repo_map_generation_activate_port: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
    /// The quarantine's destructive side (QI-BB-026): one discard port per
    /// generation track, and the `RepoMap` store's own list-and-discard.
    pub lexical_quarantine_discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync>,
    pub semantic_quarantine_discard: Arc<dyn QuarantinedGenerationDiscardPort + Send + Sync>,
    /// Where activation and rollback record a content defect a door
    /// proved on the generation they picked (QI-BB-026): each track's
    /// adapter re-proves it and writes the quarantine receipt.
    pub lexical_door_findings: Arc<dyn DoorFindingQuarantinePort + Send + Sync>,
    pub semantic_door_findings: Arc<dyn DoorFindingQuarantinePort + Send + Sync>,
    pub repo_map_quarantine: Arc<dyn RepoMapQuarantinePort + Send + Sync>,
    /// What the `RepoMap` store found on disk when the composition root
    /// opened it (QI-BB-008); surfaced through the boot inventory.
    pub repo_map_open_report: RepoMapOpenReportV1,
    pub search_corpus_lifecycle: Arc<SearchCorpusLifecycleOwner>,
    /// Durable ingest operation journal (QI-BB-032, SEP-21 P02B).
    pub idempotency: Arc<dyn IdempotencyCatalogPort + Send + Sync>,
    /// The state-root-global durable mutation coordinator
    /// (`MutationCoordinatorV1`, SEP-21 P02B).
    pub mutation_coordinator: Arc<dyn MutationCoordinatorPort + Send + Sync>,
    /// Durable auxiliary authority rows (QI-BB-020).
    pub auxiliary_catalog: Arc<dyn AuxiliaryAuthorityCatalogPort + Send + Sync>,
    /// The per-epoch history text index the relevance order scores with
    /// (QI-BB-023 follow-up #1); published with every history epoch,
    /// opened by the history route, reclaimed with epoch retention.
    pub history_text_index: Arc<dyn HistoryTextIndexPort + Send + Sync>,
    /// Adapters that keep their own accounting, for the metrics scrape
    /// (QI-BB-015); the composition root adds its own sources to these.
    pub adapter_metric_sources: Vec<Arc<dyn MetricSourcePort>>,
    /// Every adapter whose sealed generations the integrity scrub proves
    /// as maintenance (QI-BB-017).
    pub integrity_scrub_ports: Vec<Arc<dyn IntegrityScrubPort + Send + Sync>>,
    /// The lexical writer cache's idle sweep, run by the maintenance timer
    /// (QI-BB-016).
    pub writer_idle_sweep: Arc<dyn WriterIdleSweepPort>,
    /// Each track's own byte walker, for the generation disk gauges the
    /// maintenance timer refreshes (QI-BB-015).
    pub lexical_disk_usage: Arc<dyn TrackDiskUsagePort>,
    pub semantic_disk_usage: Arc<dyn TrackDiskUsagePort>,
    /// Where the process reads its resident memory (QI-BB-016): the
    /// kernel in production, a script in a test.
    pub memory_probe: Arc<dyn ProcessMemoryProbePort>,
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
    /// Take the root under [`StateRootAccessV1::Private`].
    pub fn acquire(state_root: &Path) -> Result<Self, CoreError> {
        Self::acquire_with_access(state_root, StateRootAccessV1::Private)
    }

    /// Take the root, refusing typed (`STATE_ROOT_INSECURE`) one wider than
    /// `access` allows; a created root is `0700` whatever the access.
    pub fn acquire_with_access(
        state_root: &Path,
        access: StateRootAccessV1,
    ) -> Result<Self, CoreError> {
        ensure_durable_state_root_v1(state_root)?;
        let state_root_identity_v1 = canonical_state_root_identity_v1(state_root)?;
        ensure_private_state_root_v1(&state_root_identity_v1, access)?;
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
                code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInUse,
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
    use std::os::unix::fs::MetadataExt;

    let file = open(
        path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::CREATE,
        Mode::from_raw_mode(0o600),
    )
    .map(File::from)
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::other(format!(
            "state-root lock is not a regular file: {}",
            path.display()
        )));
    }
    // A lock that already existed must still be exactly this process's
    // (S21-09): expected owner, the exact private mode, and a single
    // link — a foreign, permissive, or hard-linked lock is refused, not
    // adopted.
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(std::io::Error::other(format!(
            "state-root lock is owned by another uid: {}",
            path.display()
        )));
    }
    if metadata.mode() & 0o7777 != 0o600 {
        return Err(std::io::Error::other(format!(
            "state-root lock mode is not exactly 0600: {}",
            path.display()
        )));
    }
    if metadata.nlink() != 1 {
        return Err(std::io::Error::other(format!(
            "state-root lock has more than one link: {}",
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

/// How wide the state root may be (QI-BB-014).
///
/// Private: exactly `0700` — the daemon's index files, catalogs and, by
/// default, its sockets live under the root, and every wider mode is
/// refused typed, not just a writable one. Shared: the operator opened at
/// least one socket to other users, and the default socket directory sits
/// under the root, so the peers need to traverse it; the root may then
/// carry group/other traverse bits (`0710`, `0711`) but still no read or
/// write bit for them. The root is judged as the path resolves (a symlink
/// to a private directory is that directory); the socket directory's own
/// policy handles the socket parent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StateRootAccessV1 {
    Private,
    SharedTraversal,
}

impl StateRootAccessV1 {
    /// Traversal is shared when any socket admits other users.
    #[must_use]
    pub fn for_sockets(policies: &crate::app::socket_access::SocketAccessPolicies) -> Self {
        if crate::app::socket_access::SocketRole::ALL
            .iter()
            .any(|role| policies.for_role(*role).admits_others())
        {
            Self::SharedTraversal
        } else {
            Self::Private
        }
    }

    #[cfg(unix)]
    const fn admits_mode(self, mode: u32) -> bool {
        match self {
            Self::Private => mode == STATE_ROOT_DIRECTORY_MODE,
            // Owner rwx, group/other at most x.
            Self::SharedTraversal => mode & 0o7700 == 0o700 && mode & 0o066 == 0,
        }
    }

    #[cfg(unix)]
    const fn expectation(self) -> &'static str {
        match self {
            Self::Private => "exactly 0700 (chmod 700)",
            Self::SharedTraversal => {
                "0700 with at most the group/other traverse bits, since a socket is shared (chmod 711 or 710)"
            }
        }
    }
}

/// Refuse a state root wider than its policy (QI-BB-014).
///
/// A directory this daemon created is `0700`. A pre-existing one must be
/// owned by the user the daemon runs as and be no wider than
/// [`StateRootAccessV1`] allows: index files, catalogs and sockets live
/// under it, and a root others can read or write lets another local user
/// read or replace any of them under the daemon.
#[cfg(unix)]
fn ensure_private_state_root_v1(
    state_root: &Path,
    access: StateRootAccessV1,
) -> Result<(), CoreError> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = fs::metadata(state_root).map_err(|error| {
        CoreError::Storage(format!(
            "searchd state-root lease: inspect {}: {error}",
            state_root.display()
        ))
    })?;
    let owner = rustix::process::geteuid().as_raw();
    let mode = metadata.mode() & 0o7777;
    if metadata.uid() != owner || !access.admits_mode(mode) {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInsecure,
            message: format!(
                "searchd state root {} is uid {} mode {mode:04o}; it must belong to uid {owner} and be {}",
                state_root.display(),
                metadata.uid(),
                access.expectation()
            ),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_state_root_v1(
    _state_root: &Path,
    _access: StateRootAccessV1,
) -> Result<(), CoreError> {
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
    fn embed_query(
        &self,
        _query_text: &str,
        _budget: &RequestBudgetV1,
    ) -> Result<Vec<f32>, quanta_index_core::CoreError> {
        Err(quanta_index_core::CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                LexicalErrorCode::SemProviderUnavailable,
            ),
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
/// [`QueryTextEmbedderPort`] embeds one text at a time under the request's
/// budget; routing it through the same provider instance the corpus path
/// uses means the two cannot diverge on model identity.
struct QueryEmbedderAdapter(Arc<dyn TextEmbeddingProvider + Send + Sync>);

impl QueryTextEmbedderPort for QueryEmbedderAdapter {
    fn embed_query(
        &self,
        query_text: &str,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<f32>, CoreError> {
        self.0
            .embed_batch_within(&[query_text], budget)?
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

/// The supervisor id provider reservations enroll to (S21-08). The name
/// is fixed at composition so reserved work always has an owner; the
/// supervisor that actually spawns/registers/cancels/joins it is P08
/// (S21-09).
const PROVIDER_SUPERVISOR_ID: &str = "searchd-provider-supervisor";

/// The query-side and corpus-side embedders resolved for one profile.
///
/// Both halves are backed by the same provider for `Hash`/`OpenAi` so model
/// identity matches; for `Unavailable` the query embedder fails closed while
/// the corpus still hash-derives (the deliberate degraded-config contract).
/// `metric_sources` carries whatever the profile built that keeps its own
/// accounting (the embedding cache), for the metrics scrape (QI-BB-015).
/// `provider_ledger` is the process-global provider work ledger the query
/// boundary reserves against (S21-08); `source_egress_policy` gates the
/// corpus derivation batches (`None` is a local-only composition).
struct SemanticEmbedders {
    query: Arc<dyn QueryTextEmbedderPort + Send + Sync>,
    corpus: Arc<dyn TextEmbeddingProvider + Send + Sync>,
    metric_sources: Vec<Arc<dyn MetricSourcePort>>,
    provider_ledger: Arc<ProviderBudgetLedger>,
    source_egress_policy: Option<SemanticEgressPolicyV1>,
}

/// Resolve the (query embedder, corpus embedder) pair for a profile.
///
/// See [`SemanticEmbedders`] for the identity contract the pair upholds.
/// The query half sits behind the provider boundary (S21-08): admission,
/// the declared-model gate and the global reservation run before any
/// provider I/O, and every settlement lands in the ledger's audit ring.
/// An `OpenAi` profile whose egress grant is incomplete refuses boot —
/// external egress without an explicit grant never serves.
fn build_semantic_embedders(
    profile: &SemanticEmbedderProfile,
    state_root: &Path,
    budget: &ProviderWorkBudgetConfig,
    grant: &ProviderEgressGrantConfig,
) -> Result<SemanticEmbedders, CoreError> {
    let provider_ledger = Arc::new(ProviderBudgetLedger::new(budget.to_budget())?);
    match profile {
        SemanticEmbedderProfile::Hash { dimension } => {
            let provider: Arc<dyn TextEmbeddingProvider + Send + Sync> =
                Arc::new(HashingQueryTextEmbedder::new(*dimension));
            let inner: Arc<dyn QueryTextEmbedderPort + Send + Sync> =
                Arc::new(QueryEmbedderAdapter(Arc::clone(&provider)));
            Ok(SemanticEmbedders {
                query: Arc::new(ProviderBoundaryQueryEmbedder::new(
                    inner,
                    Arc::clone(&provider_ledger),
                    SemanticEgressPolicyV1::Loopback,
                    PROVIDER_SUPERVISOR_ID,
                )),
                corpus: provider,
                metric_sources: Vec::new(),
                provider_ledger,
                source_egress_policy: None,
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
            // The provider's retry/transport/HTTP failure counters reach the
            // scrape as `embed_provider_…` (QI-BB-009 #5, QI-BB-015), and
            // how far its raw vectors were from unit as
            // `semantic_embedding_raw_…` (QI-BB-031 #4).
            let raw_norms: Arc<dyn MetricSourcePort> = normalized.raw_norm_tallies();
            let mut metric_sources: Vec<Arc<dyn MetricSourcePort>> =
                vec![Arc::new(OpenAiEmbedTelemetrySource), raw_norms];
            let provider: Arc<dyn TextEmbeddingProvider + Send + Sync> = if tuning.cache_enabled {
                // Persistent content-hash cache under the identity's own
                // namespace (model, revision, dimension, policy — QI-BB-028)
                // so rebuilds / incrementals avoid paid re-embedding of
                // unchanged chunks and a revision rotation never reuses the
                // previous revision's vectors.
                let identity = EmbeddingCacheIdentityV1::of(&normalized);
                let cache = Arc::new(FileEmbeddingCache::new(
                    &state_root.join("embed-cache"),
                    &identity,
                    tuning.cache_retention,
                )?);
                let open_source: Arc<dyn MetricSourcePort> = cache.clone();
                metric_sources.push(open_source);
                let caching = Arc::new(CachingEmbeddingProvider::new(
                    Box::new(normalized),
                    Box::new(SharedFileEmbeddingCache(cache)),
                ));
                let cache_source: Arc<dyn MetricSourcePort> = caching.clone();
                metric_sources.push(cache_source);
                caching
            } else {
                Arc::new(normalized)
            };
            let egress_grant = grant.to_grant("openai", model, model_revision);
            egress_grant.validate()?;
            let policy = SemanticEgressPolicyV1::External(egress_grant);
            let inner: Arc<dyn QueryTextEmbedderPort + Send + Sync> =
                Arc::new(QueryEmbedderAdapter(Arc::clone(&provider)));
            Ok(SemanticEmbedders {
                query: Arc::new(ProviderBoundaryQueryEmbedder::new(
                    inner,
                    Arc::clone(&provider_ledger),
                    policy.clone(),
                    PROVIDER_SUPERVISOR_ID,
                )),
                corpus: provider,
                metric_sources,
                provider_ledger,
                source_egress_policy: Some(policy),
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
                provider_ledger,
                source_egress_policy: None,
            })
        }
    }
}

/// The file store behind an `Arc`, so the scrape can read its open report
/// while the caching provider owns the store's traffic.
struct SharedFileEmbeddingCache(Arc<FileEmbeddingCache>);

impl quanta_index_embed::EmbeddingCache for SharedFileEmbeddingCache {
    fn get(&self, key: &quanta_index_embed::EmbeddingCacheKey) -> Option<Vec<f32>> {
        self.0.get(key)
    }

    fn put(&self, key: &quanta_index_embed::EmbeddingCacheKey, vector: &[f32]) {
        self.0.put(key, vector);
    }

    fn evict(&self, key: &quanta_index_embed::EmbeddingCacheKey) {
        self.0.evict(key);
    }

    fn stats(&self) -> quanta_index_embed::EmbeddingCacheStats {
        self.0.stats()
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
    /// Readiness is a gate on the generation's current structural
    /// materialization.
    ///
    /// The query itself executes against the snapshot its `aux_epoch`
    /// pins (see [`Self::execute`]), which is the one the route read when
    /// it resolved the request.
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
        // The request pins the structural authority epoch the whole query
        // reads (QI-BB-020 W2): every leaf executes against that snapshot,
        // and an epoch the ledger no longer retains — or never had — is
        // refused typed rather than served from the current one.
        let state = self
            .ledger
            .read()
            .map_err(|err| {
                StructuralError::ProducerExecution(format!("structural ledger poisoned: {err}"))
            })?
            .structural_read_at(
                &pin.repo_id,
                &pin.revision_id,
                pin.manifest_generation,
                Some(request.aux_epoch),
                Instant::now(),
            )
            .map_err(map_structural_read_error)?
            .ok_or(StructuralError::GenerationNotReady)?
            .state;
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

/// The structural producer's typed refusal for a ledger read at a pinned
/// epoch: the two epoch refusals keep their wire codes, anything else is
/// a producer execution failure.
fn map_structural_read_error(err: CoreError) -> StructuralError {
    match err {
        CoreError::Typed { code, message } if code == AUX_EPOCH_EXPIRED_CODE => {
            StructuralError::AuxEpochExpired(message)
        }
        CoreError::Typed { code, message } if code == AUX_EPOCH_UNKNOWN_CODE => {
            StructuralError::AuxEpochUnknown(message)
        }
        other @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => StructuralError::ProducerExecution(format!(
            "structural ledger read at the pinned epoch failed: {other}"
        )),
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
    pub repo_map_snapshot_port: Arc<dyn RepoMapSnapshotAcquirePort + Send + Sync>,
    pub query_obs_store: Arc<BoundedQueryObsStore>,
    /// The process-global provider work ledger (S21-08): reservations,
    /// spend accounting and the bounded audit ring the query boundary
    /// records every settlement into. Read by readiness probes and the
    /// metrics scrape.
    pub provider_ledger: Arc<ProviderBudgetLedger>,
    pub semantic_boot: semantic_boot::SemanticBootReport,
    /// What boot inventoried, quarantined and proved (QI-BB-026).
    pub boot_inventory: BootInventoryReportV1,
    /// The one process memory envelope this runtime was validated under
    /// (QI-BB-016).
    pub process_memory_envelope: quanta_index_core::ProcessMemoryEnvelopeV1,
    /// What boot wants the operator to read, one line each: the
    /// development-embedder warning (QI-BB-007) and the writer gate's
    /// ceiling or absence (QI-BB-016). The daemon entry prints them; an
    /// in-process harness keeps them as data.
    pub boot_notices: Vec<String>,
    /// The maintenance timer (QI-BB-016, QI-BB-015). It runs until the
    /// supervisor drains it; since SEP-21 P08 the serving path hands it
    /// to the supervisor (`into_servers_guards_and_boot_notices`) so it
    /// is never a detached thread and never torn down after the lease.
    _maintenance: MaintenanceTimer,
    _search_corpus_lifecycle: Arc<SearchCorpusLifecycleOwner>,
    // Rust drops fields in declaration order. Keep the state-root lease last
    // so every adapter, server, and authority handle is gone before ownership
    // of the shared root is released. The serving path moves these three
    // into the supervisor's `RuntimeGuards` bundle, which drops only after
    // every supervised child has joined or been explicitly escalated
    // (S21-09); `SearchdRuntime::drop` alone is the non-serving fallback.
    _state_root_lease: StateRootLease,
}

/// The serving surfaces of one assembled runtime, split from its
/// lifetime guards (SEP-21 P08 / S21-09).
pub struct RuntimeServers {
    /// The query accept loop's bound server.
    pub query: SearchPlaneQueryServer<
        dyn IpcDispatcher<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse>,
    >,
    /// The control accept loop's bound server.
    pub control: SearchPlaneControlServer<
        dyn IpcDispatcher<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse>,
    >,
    /// The ingest accept loop's bound server.
    pub ingest: SearchPlaneIngestServer<
        dyn IpcDispatcher<SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse>,
    >,
}

/// The lifetime guards of one assembled runtime: the search-corpus
/// lifecycle owner and the state-root lease (SEP-21 P08 / S21-09).
///
/// The maintenance timer is split out at the same time and becomes a
/// supervised child of its own. The supervisor owns this bundle for the
/// whole serving interval; the guards drop only after every supervised
/// child — accept loops, the connections they joined, the maintenance
/// timer, provider tasks — has joined, or after an explicit hard-deadline
/// escalation. In particular the state-root lease is held from
/// construction until every child exits, so a second daemon cannot
/// acquire the same state root while the first is still serving.
pub struct RuntimeGuards {
    /// The search-corpus lifecycle owner (readiness/GC authority).
    pub search_corpus_lifecycle: Arc<SearchCorpusLifecycleOwner>,
    /// The state-root lease; released last, after everything else.
    pub state_root_lease: StateRootLease,
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
            semantic_content_roots,
            semantic_incomplete_discard,
            semantic_sealed_reclaim,
            sem_open_port,
            repo_map_snapshot_port,
            repo_map_bundle_ingest_port,
            repo_map_generation_activate_port,
            lexical_quarantine_discard,
            semantic_quarantine_discard,
            lexical_door_findings,
            semantic_door_findings,
            repo_map_quarantine,
            repo_map_open_report,
            search_corpus_lifecycle,
            idempotency,
            mutation_coordinator,
            auxiliary_catalog,
            history_text_index,
            adapter_metric_sources,
            integrity_scrub_ports,
            writer_idle_sweep,
            lexical_disk_usage,
            semantic_disk_usage,
            memory_probe,
        } = parts;
        state_root_lease
            .require_state_root_v1(config.state_root())
            .map_err(anyhow::Error::from)?;
        // The one envelope (QI-BB-016): every resident byte policy of this
        // config, summed and refused typed over the ceiling, before any
        // adapter holds a byte of it.
        let process_memory_envelope = config.process_memory_envelope()?;
        let profile = config.semantic_embedder_profile();
        let mut boot_notices: Vec<String> = Vec::new();
        if profile.is_dev() {
            // The development label (QI-BB-007): the boot log names the
            // profile and the scrape reports it, so no deployment serves
            // token overlap as semantics unknowingly.
            boot_notices.push(format!(
                "WARNING: semantic embedder profile `{}` is a development/test embedder (hash slots, no learned semantics); name `openai` for a learned provider",
                profile.selector()
            ));
        }
        boot_notices.push(process_memory_envelope.rss_ceiling.map_or_else(
            || {
                "lexical writer gate disabled: no QUANTA_INDEX_PROCESS_RSS_CEILING_BYTES configured"
                    .to_string()
            },
            |ceiling| {
                format!(
                    "lexical writer gate: resident-memory ceiling {ceiling} bytes ({})",
                    crate::app::process_memory::KernelResidentMemoryProbe::semantics()
                )
            },
        ));
        search_corpus_lifecycle
            .require_state_root_v1(config.state_root())
            .map_err(anyhow::Error::from)?;
        let leased_state_root = state_root_lease.state_root_identity_v1().to_path_buf();
        // Offline-only migration (SEP-21 P10 / S21-11): a legacy layout is
        // refused typed here, before any adapter opens it. The boot path
        // carries no legacy decoder and no migrator; the operator runs the
        // offline `migrate-state` command and boots the produced root.
        crate::app::state_format::refuse_legacy_state_root_v1(&leased_state_root)
            .map_err(anyhow::Error::from)?;
        let activation_catalog = search_corpus_lifecycle.activation_catalog();
        let aux_authority_store = search_corpus_lifecycle.authority_store();
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        // The durable search-corpus history first (QI-BB-003): the sealed
        // identities it retains are the serving boundary, and the inventory
        // below is measured against them — a sealed directory the history
        // does not retain is an orphan, not a seed.
        {
            let mut guard = ledger.write().map_err(|err| {
                anyhow::anyhow!("ledger poisoned during search-corpus history bootstrap: {err}")
            })?;
            aux_authority_store
                .restore_into(&mut guard)
                .map_err(anyhow::Error::from)?;
        }
        // Reclaims a crash cut short are finished before any inventory
        // (QI-BB-003): each is out of the generation namespace already.
        let lexical_interrupted =
            boot_inventory::finish_interrupted_reclaims(lexical_sealed_reclaim.as_ref())?;
        let semantic_interrupted =
            boot_inventory::finish_interrupted_reclaims(semantic_sealed_reclaim.as_ref())?;
        boot_notices.extend(
            [
                lexical_interrupted.boot_notice(SearchPlaneTrackKind::Lexical),
                semantic_interrupted.boot_notice(SearchPlaneTrackKind::Semantic),
            ]
            .into_iter()
            .flatten(),
        );
        // Boot inventory (QI-BB-026): identities only, per track; nothing is
        // opened or hashed until the active pairs are proven below.
        let lexical_inventory = boot_inventory::seed_track_readiness(
            &ledger,
            SearchPlaneTrackKind::Lexical,
            lexical_generation_scanner.as_ref(),
        )?;
        let seed_start = std::time::Instant::now();
        let semantic_inventory = semantic_boot::seed_persisted_semantic_readiness(
            &ledger,
            semantic_generation_scanner.as_ref(),
        )
        .map_err(anyhow::Error::from)?;
        let boot_report = semantic_boot::SemanticBootReport {
            // Boot never migrates (SEP-21 P10): a legacy journal would have
            // been refused above, so by construction there is none here.
            // The offline `migrate-state` command is the only migrator.
            migration: semantic_boot::SemanticMigrationOutcome::NoLegacyJournal,
            migration_micros: 0,
            seed: semantic_boot::SemanticSeedReport::from_track_report(&semantic_inventory),
            seed_micros: seed_start.elapsed().as_micros(),
        };
        // The only deep validation boot performs: each active pair, once. A
        // defective active generation fails here, typed, before any bind,
        // and the proven handles go straight into the snapshot registries
        // so the first query after restart is a hit (QI-BB-017 #4).
        let snapshots = SnapshotRegistries::new(config.snapshot_registry_policy());
        let promotion = ActivationPromotionParts {
            lexical_open: Arc::clone(&lex_open_port),
            semantic_open: Arc::clone(&sem_open_port),
            semantic_content_roots: Arc::clone(&semantic_content_roots),
            lexical_door_findings,
            semantic_door_findings,
            snapshots: snapshots.clone(),
        };
        let active_pairs_validated = search_corpus_lifecycle
            .validate_rehydrated_active_generations_v1(&promotion)
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
            quanta_index_search_plane::readiness::restore_auxiliary_rows_into(
                &mut guard,
                auxiliary_catalog.as_ref(),
            )
            .map_err(anyhow::Error::from)?
        };
        // The scrub receipts the adapters keep beside their generations
        // (QI-BB-017): what boot found already proven and what never was.
        let lexical_scrub = boot_inventory::inventory_scrub_receipts(
            SearchPlaneTrackKind::Lexical,
            &integrity_scrub_ports,
        )?;
        let semantic_scrub = boot_inventory::inventory_scrub_receipts(
            SearchPlaneTrackKind::Semantic,
            &integrity_scrub_ports,
        )?;
        let half_sealed_pairs =
            boot_inventory::half_sealed_pairs(&lexical_inventory, &semantic_inventory);
        boot_notices.extend(half_sealed_pairs.iter().map(HalfSealedPair::boot_notice));
        let boot_inventory = BootInventoryReportV1 {
            lexical: lexical_inventory
                .with_scrub(lexical_scrub)
                .with_interrupted_reclaims(lexical_interrupted),
            semantic: semantic_inventory
                .with_scrub(semantic_scrub)
                .with_interrupted_reclaims(semantic_interrupted),
            active_pairs_validated,
            half_sealed_pairs,
            auxiliary_migration,
            auxiliary_rows_restored,
            repo_map: repo_map_open_report,
            socket_access: config.socket_access_policies().clone(),
            semantic_profile_is_dev: profile.is_dev(),
        };
        let auxiliary_parts = AuxiliaryMaterializerParts {
            catalog: Arc::clone(&auxiliary_catalog),
            // The durable MutationCoordinatorV1 (SEP-21 P02B): the
            // state-root-global mutation lease the catalog machine-enforces.
            coordinator: AuxiliaryMutationCoordinator::durable(Arc::clone(&mutation_coordinator)),
            ledger: Arc::clone(&ledger),
        };
        // One registry of opened history text epochs, shared by the route
        // that acquires them and the two mutation paths that retire them.
        let history_text = HistoryTextIndexParts::new(history_text_index);
        let SemanticEmbedders {
            query: query_text_embedder,
            corpus: corpus_embedder,
            metric_sources: embedder_metric_sources,
            provider_ledger,
            source_egress_policy,
        } = build_semantic_embedders(
            config.semantic_embedder_profile(),
            &leased_state_root,
            config.provider_work_budget(),
            config.provider_egress_grant(),
        )
        .map_err(anyhow::Error::from)?;
        let semantic_materializer =
            Arc::new(DirectSemanticMaterializer::new(Arc::clone(&sem_build_port)));
        let direct_sem_ingest_port: Arc<dyn SemanticIngestPort + Send + Sync> =
            semantic_materializer.clone();
        let authority_write_port: Arc<dyn SearchCorpusAuthorityWritePort + Send + Sync> =
            aux_authority_store.clone();
        let authority_inspect_port: Arc<dyn SearchCorpusAuthorityInspectPort + Send + Sync> =
            aux_authority_store;
        let search_corpus_materializer: Arc<DirectSearchCorpusMaterializer> = Arc::new(
            DirectSearchCorpusMaterializer::new_with_search_owned_semantics_from_env(
                SearchCorpusMaterializerParts {
                    builder: Arc::clone(&search_corpus_build_port),
                    ledger: Arc::clone(&ledger),
                    semantic_ingest: Arc::clone(&direct_sem_ingest_port),
                    semantic_embedder: corpus_embedder,
                    authority: authority_write_port,
                    lexical_generation_validator,
                    semantic_generation_validator,
                    semantic_content_roots,
                    lexical_incomplete_discard: Arc::clone(&lexical_incomplete_discard),
                    semantic_incomplete_discard: Arc::clone(&semantic_incomplete_discard),
                    lexical_reclaim: Arc::clone(&lexical_sealed_reclaim),
                    semantic_reclaim: Arc::clone(&semantic_sealed_reclaim),
                    snapshots: snapshots.clone(),
                    idempotency: Arc::clone(&idempotency),
                    resource_policy: config.ingest_resource_policy(),
                    semantic_stream_policy: config.semantic_stream_window_policy(),
                    source_egress_policy,
                    auxiliary_catalog: Arc::clone(&auxiliary_parts.catalog),
                    auxiliary_coordinator: Arc::clone(&auxiliary_parts.coordinator),
                },
            )
            .map_err(anyhow::Error::from)?
            .with_history_text(history_text.clone()),
        );
        let direct_search_corpus_ingest_port: Arc<dyn SearchCorpusIngestPort + Send + Sync> =
            search_corpus_materializer.clone();
        let direct_history_ingest_port: Arc<dyn HistoryIngestPort + Send + Sync> = Arc::new(
            DirectHistoryMaterializer::new(auxiliary_parts.clone())
                .with_history_text(history_text.clone()),
        );
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
        // History text discards that failed after their mutation was
        // durable (QI-BB-020), each left for the next pass.
        let history_text_source: Arc<dyn MetricSourcePort> = Arc::new(history_text.clone());
        metric_sources.push(history_text_source);
        let ingest_source: Arc<dyn MetricSourcePort> = search_corpus_materializer;
        metric_sources.push(ingest_source);
        let semantic_ingest_source: Arc<dyn MetricSourcePort> = semantic_materializer;
        metric_sources.push(semantic_ingest_source);
        let boot_source: Arc<dyn MetricSourcePort> = Arc::new(boot_inventory.clone());
        metric_sources.push(boot_source);
        // The integrity scrub (QI-BB-017) is paced on the maintenance timer:
        // one bounded step at most every policy interval, retiring a
        // quarantined generation's resident handle from the registries.
        let scrub_tallies = Arc::new(ScrubTalliesV1::default());
        let integrity_scrub = (!integrity_scrub_ports.is_empty()).then(|| {
            Mutex::new(PacedIntegrityScrubV1::new(
                ScrubSchedulerV1::new(
                    integrity_scrub_ports,
                    config.integrity_scrub_policy(),
                    snapshots.clone(),
                    Arc::clone(&scrub_tallies),
                ),
                config.integrity_scrub_policy(),
                Instant::now(),
            ))
        });
        let scrub_source: Arc<dyn MetricSourcePort> = scrub_tallies;
        metric_sources.push(scrub_source);
        // The maintenance timer (QI-BB-016, QI-BB-015): the idle writer
        // sweep, the per-track disk gauges and the scrub run on it, and its
        // tallies plus the process gauge join the scrape.
        let maintenance = MaintenanceTimer::start(
            MaintenanceParts {
                writer_sweep: writer_idle_sweep,
                lexical_disk_usage,
                semantic_disk_usage,
                integrity_scrub,
            },
            config.maintenance_policy().tick(),
        )
        .map_err(anyhow::Error::from)?;
        let maintenance_source: Arc<dyn MetricSourcePort> = Arc::new(MaintenanceMetricSource::new(
            maintenance.tallies(),
            memory_probe,
        ));
        metric_sources.push(maintenance_source);
        let observability = Arc::new(ObservabilityScrape::new(
            Arc::clone(&query_obs_store),
            metric_sources,
        ));
        let cursor_keys = CursorKeyStore::open(&leased_state_root).map_err(anyhow::Error::from)?;
        let query_dispatcher = Arc::new(
            SearchPlaneDispatcher::new_with_obs(
                Arc::clone(&lex_open_port),
                Arc::clone(&sem_open_port),
                snapshots.clone(),
                Arc::clone(&repo_map_snapshot_port),
                Arc::new(LedgerStructuralProducer::new(Arc::clone(&ledger))),
                Arc::clone(&ledger),
                activation_catalog.clone(),
                query_text_embedder,
                query_obs_sink,
            )
            .with_cursor_key_store(cursor_keys)
            .with_history_text(history_text)
            .with_response_budget(config.query_response_budget()),
        );
        let quarantine = QuarantineService::new(QuarantineServiceParts {
            lexical_scanner: Arc::clone(&lexical_generation_scanner),
            semantic_scanner: Arc::clone(&semantic_generation_scanner),
            lexical_discard: lexical_quarantine_discard,
            semantic_discard: semantic_quarantine_discard,
            lexical_reclaim: Arc::clone(&lexical_sealed_reclaim),
            semantic_reclaim: Arc::clone(&semantic_sealed_reclaim),
            repo_map: repo_map_quarantine,
            ledger: Arc::clone(&ledger),
            snapshots,
        });
        let control_dispatcher = Arc::new(SearchPlaneControlDispatcher::new(
            SearchPlaneControlDispatcherParts {
                repo_map_activate: repo_map_generation_activate_port,
                lifecycle: SearchCorpusLifecycleParts {
                    activation_catalog,
                    ledger: Arc::clone(&ledger),
                    authority: authority_inspect_port,
                    promotion,
                },
                observability,
                quarantine,
                // No readiness authority is wired in this composition yet:
                // the readiness opcode refuses typed until one is injected.
                readiness: None,
            },
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
            idempotency,
        ));
        let query_adapter: Arc<
            dyn IpcDispatcher<SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse>,
        > = Arc::new(SearchPlaneQueryIpcAdapter::new(query_dispatcher));
        // Each socket carries its own access policy (QI-BB-014); a shared
        // query socket leaves control and ingest private unless they were
        // opened by name.
        let socket_access = config.socket_access_policies();
        // The directory the three sockets share is held to the widest of
        // their policies (QI-BB-014).
        let directory_access = quanta_index_ipc::SocketAccessPolicy::widest(
            SocketRole::ALL
                .iter()
                .map(|role| socket_access.for_role(*role)),
        );
        let query_server = SearchPlaneQueryServer::bind(
            quanta_index_ipc::IpcPlane::Query,
            "quanta-index-query-uds",
            config.query_socket_path(),
            query_adapter,
            config.query_admission_policy(),
            socket_access.for_role(SocketRole::Query).clone(),
            &directory_access,
            query_counters,
        )
        .map_err(anyhow::Error::from)?;
        let control_adapter: Arc<
            dyn IpcDispatcher<SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse>,
        > = Arc::new(SearchPlaneControlIpcAdapter::new(control_dispatcher));
        // Control and ingest mutate; their dispatches serialize by policy
        // while their connections still read independently (QI-BB-002).
        let control_server = SearchPlaneControlServer::bind(
            quanta_index_ipc::IpcPlane::Control,
            "quanta-index-control-uds",
            config.control_socket_path(),
            control_adapter,
            ServerAdmissionPolicy::SERIAL_DISPATCH,
            socket_access.for_role(SocketRole::Control).clone(),
            &directory_access,
            control_counters,
        )
        .map_err(anyhow::Error::from)?;
        let ingest_adapter: Arc<
            dyn IpcDispatcher<SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse>,
        > = Arc::new(SearchPlaneIngestIpcAdapter::new(ingest_dispatcher));
        let ingest_server = SearchPlaneIngestServer::bind(
            quanta_index_ipc::IpcPlane::Ingest,
            "quanta-index-ingest-uds",
            config.ingest_socket_path(),
            ingest_adapter,
            ServerAdmissionPolicy::SERIAL_DISPATCH,
            socket_access.for_role(SocketRole::Ingest).clone(),
            &directory_access,
            ingest_counters,
        )
        .map_err(anyhow::Error::from)?;

        Ok(Self {
            config,
            query_server,
            control_server,
            ingest_server,
            repo_map_snapshot_port,
            query_obs_store,
            provider_ledger,
            semantic_boot: boot_report,
            boot_inventory,
            process_memory_envelope,
            boot_notices,
            _maintenance: maintenance,
            _search_corpus_lifecycle: search_corpus_lifecycle,
            _state_root_lease: state_root_lease,
        })
    }

    /// Split the runtime into its serving surfaces, its maintenance
    /// timer, and its lifetime guards (SEP-21 P08 / S21-09). The
    /// supervisor takes all of them: the servers and the timer are
    /// spawned as supervised children, the guards are held until every
    /// child has joined or been explicitly escalated — never released
    /// before serving ends.
    #[must_use]
    pub fn into_servers_maintenance_guards_and_boot_notices(
        self,
    ) -> (RuntimeServers, MaintenanceTimer, RuntimeGuards, Vec<String>) {
        let Self {
            query_server,
            control_server,
            ingest_server,
            boot_notices,
            _maintenance: maintenance,
            _search_corpus_lifecycle: search_corpus_lifecycle,
            _state_root_lease: state_root_lease,
            ..
        } = self;
        (
            RuntimeServers {
                query: query_server,
                control: control_server,
                ingest: ingest_server,
            },
            maintenance,
            RuntimeGuards {
                search_corpus_lifecycle,
                state_root_lease,
            },
            boot_notices,
        )
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
        LedgerStructuralProducer, LqStructuralBlock, RequestBudgetV1, StructuralProducerPort,
        StructuralReadiness, ensure_durable_state_root_with_v1,
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
            aux_epoch: quanta_index_contract::AuxEpochV1::GENESIS,
        }
    }

    fn pinned_request() -> DomainStructuralQueryRequest {
        request_with_generation(GenerationSelector::Pinned(GenerationPin::new(
            RepoId::new("repo".to_string()).expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("rev".to_string())
                .expect("static fixture ID satisfies canonical policy"),
            quanta_index_contract::ManifestGeneration::new(7),
        )))
    }

    #[test]
    fn structural_readiness_rejects_active_generation_selector() {
        let producer = LedgerStructuralProducer::new(Arc::new(RwLock::new(Ledger::new())));
        let readiness = producer.readiness(&request_with_generation(GenerationSelector::Active {
            repo_id: RepoId::new("repo".to_string())
                .expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev".to_string())
                .expect("static fixture ID satisfies canonical policy"),
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

    fn test_budget() -> super::ProviderWorkBudgetConfig {
        super::ProviderWorkBudgetConfig::default()
    }

    /// A complete egress grant: every field the engine requires, without
    /// source-content consent unless the case opts in.
    fn test_grant() -> super::ProviderEgressGrantConfig {
        super::ProviderEgressGrantConfig {
            tenant_id: "unit-test-tenant".to_string(),
            endpoint: "https://unit.test/v1".to_string(),
            region: "unit-test-region".to_string(),
            retention: "unit-test-30d".to_string(),
            profile: "unit-test-release".to_string(),
            source_content_consent: false,
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
        } = super::build_semantic_embedders(
            &openai_profile(false),
            dir.path(),
            &test_budget(),
            &test_grant(),
        )
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
            &test_budget(),
            &super::ProviderEgressGrantConfig::default(),
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
        match query_embedder.embed_query("anything", &RequestBudgetV1::unbounded()) {
            Err(quanta_index_core::CoreError::Typed { code, .. }) => {
                let expected = quanta_index_contract::lex::LexicalErrorCode::SemProviderUnavailable
                    .as_code_str();
                if code.as_wire_str() != expected {
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

    // S21-08 production wiring: the Hash query half sits behind the
    // provider boundary — a tokenless query is refused before any
    // provider I/O (and before any reservation, so the audit ring stays
    // empty), while a valid query settles and records its audit identity.
    #[test]
    fn hash_profile_query_sits_behind_the_provider_boundary() -> TestRes {
        use quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_DIMENSION;

        let profile = super::SemanticEmbedderProfile::Hash {
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
        };
        let dir = tempfile::tempdir()?;
        let super::SemanticEmbedders {
            query,
            provider_ledger,
            source_egress_policy,
            ..
        } = super::build_semantic_embedders(
            &profile,
            dir.path(),
            &test_budget(),
            &super::ProviderEgressGrantConfig::default(),
        )
        .map_err(|err| format!("hash composition must succeed: {err:?}"))?;
        assert!(
            source_egress_policy.is_none(),
            "a local composition carries no source egress policy"
        );

        let err = query
            .embed_query("   ", &RequestBudgetV1::unbounded())
            .expect_err("a tokenless query must be refused pre-I/O");
        let quanta_index_core::CoreError::Typed { code, .. } = err else {
            return Err("tokenless refusal must be typed".into());
        };
        assert_eq!(code.as_wire_str(), "EMPTY_QUERY");
        assert!(
            provider_ledger
                .audit_tail(8)
                .map_err(|err| format!("audit tail must read: {err:?}"))?
                .is_empty(),
            "a refusal before reservation records no audit event"
        );

        let vector = query
            .embed_query("needle", &RequestBudgetV1::unbounded())
            .map_err(|err| format!("a valid query must embed: {err:?}"))?;
        assert_eq!(vector.len(), SEARCH_OWNED_SEMANTIC_DIMENSION);
        let tail = provider_ledger
            .audit_tail(8)
            .map_err(|err| format!("audit tail must read: {err:?}"))?;
        assert_eq!(tail.len(), 1, "one settled call records one audit event");
        let event = &tail[0];
        assert_eq!(
            event.kind,
            quanta_index_core::ProviderSettlementKindV1::Success
        );
        assert_eq!(
            event.declared_model_id,
            quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_MODEL_ID
        );
        assert_eq!(event.observed_dimension, SEARCH_OWNED_SEMANTIC_DIMENSION);
        Ok(())
    }

    // S21-08 production wiring: an OpenAi profile without a complete
    // egress grant refuses boot — external egress without an explicit
    // grant never serves.
    #[test]
    fn openai_profile_without_a_complete_grant_refuses_boot() {
        let dir = tempfile::tempdir().expect("tempdir");
        match super::build_semantic_embedders(
            &openai_profile(false),
            dir.path(),
            &test_budget(),
            &super::ProviderEgressGrantConfig::default(),
        ) {
            Err(quanta_index_core::CoreError::Typed { code, .. }) => {
                assert_eq!(code.as_wire_str(), "PROVIDER_EGRESS_DENIED");
            }
            Err(other) => panic!("grant refusal must be typed, got {other:?}"),
            Ok(_) => panic!("an incomplete grant must refuse boot"),
        }
    }

    // S21-08 production wiring: an OpenAi profile with a complete grant
    // composes the external policy for both the query boundary and the
    // corpus derivation gate.
    #[test]
    fn openai_profile_with_a_complete_grant_composes_external_policy() -> TestRes {
        let dir = tempfile::tempdir()?;
        let super::SemanticEmbedders {
            source_egress_policy,
            ..
        } = super::build_semantic_embedders(
            &openai_profile(false),
            dir.path(),
            &test_budget(),
            &test_grant(),
        )
        .map_err(|err| format!("a granted composition must succeed: {err:?}"))?;
        let Some(quanta_index_core::SemanticEgressPolicyV1::External(grant)) =
            source_egress_policy
        else {
            return Err("a granted OpenAi composition must carry the external policy".into());
        };
        assert_eq!(grant.model_id, "text-embedding-3-small");
        assert!(!grant.source_content_consent);
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
        } = super::build_semantic_embedders(
            &profile,
            dir.path(),
            &test_budget(),
            &super::ProviderEgressGrantConfig::default(),
        )
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
            .embed_query(query_text, &RequestBudgetV1::unbounded())
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
            .embed_query(query_text, &RequestBudgetV1::unbounded())
            .map_err(|err| format!("hash query re-embed must succeed: {err:?}"))?;
        if query_vector != query_vector_again {
            return Err(
                "hash query derivation must be deterministic across identical input".into(),
            );
        }

        // (4) Discrimination: a DIFFERENT text derives a DIFFERENT vector (the
        // embedder is not a constant — the negative half of the smoke).
        let other_vector = query_embedder
            .embed_query(
                "completely unrelated lexical payload zzz",
                &RequestBudgetV1::unbounded(),
            )
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
        let _embedders = super::build_semantic_embedders(
            &openai_profile(true),
            dir.path(),
            &test_budget(),
            &test_grant(),
        )
        .expect("offline construction must succeed");
        assert!(
            dir.path().join("embed-cache").is_dir(),
            "cache_enabled=true must materialize the embed-cache dir"
        );
    }

    #[test]
    fn build_semantic_embedders_skips_cache_dir_when_cache_disabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _embedders = super::build_semantic_embedders(
            &openai_profile(false),
            dir.path(),
            &test_budget(),
            &test_grant(),
        )
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

    // QI-BB-014: the state root is private by construction and by check —
    // exactly 0700 in private mode; wider modes, readable ones included,
    // are refused typed. With a shared socket the traverse bits alone are
    // admitted, so the peers can reach the socket directory.
    #[cfg(unix)]
    #[test]
    fn a_created_state_root_is_0700_and_any_wider_existing_root_is_refused() -> TestRes {
        use super::StateRootAccessV1;
        use std::os::unix::fs::PermissionsExt as _;
        let parent = tempfile::tempdir()?;
        let created = parent.path().join("fresh").join("state");
        let lease = super::StateRootLease::acquire(&created)?;
        let mode = std::fs::metadata(&created)?.permissions().mode() & 0o7777;
        assert_eq!(mode, 0o700, "a created state root is its owner's alone");
        drop(lease);

        let existing = parent.path().join("existing");
        std::fs::create_dir(&existing)?;
        for (mode, access, admitted) in [
            (0o777, StateRootAccessV1::Private, false),
            (0o755, StateRootAccessV1::Private, false),
            (0o750, StateRootAccessV1::Private, false),
            (0o711, StateRootAccessV1::Private, false),
            (0o700, StateRootAccessV1::Private, true),
            (0o777, StateRootAccessV1::SharedTraversal, false),
            (0o755, StateRootAccessV1::SharedTraversal, false),
            (0o722, StateRootAccessV1::SharedTraversal, false),
            (0o711, StateRootAccessV1::SharedTraversal, true),
            (0o710, StateRootAccessV1::SharedTraversal, true),
            (0o700, StateRootAccessV1::SharedTraversal, true),
        ] {
            std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(mode))?;
            let outcome = super::StateRootLease::acquire_with_access(&existing, access);
            match (outcome, admitted) {
                (Ok(lease), true) => drop(lease),
                (Err(quanta_index_core::CoreError::Typed { code, message }), false) => {
                    assert_eq!(
                        code,
                        quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInsecure,
                        "mode {mode:04o} {access:?}"
                    );
                    assert!(message.contains(&format!("mode {mode:04o}")), "{message}");
                    assert!(message.contains("chmod"), "{message}");
                }
                (other, _) => {
                    return Err(format!(
                        "mode {mode:04o} under {access:?}: expected admitted={admitted}, got {other:?}"
                    )
                    .into());
                }
            }
        }
        Ok(())
    }

    fn private_tempdir() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir()?;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        Ok(dir)
    }

    #[test]
    fn state_root_lease_rejects_a_foreign_runtime_root_v1() -> TestRes {
        let owned_root = private_tempdir()?;
        let foreign_root = private_tempdir()?;
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

        let owned_root = private_tempdir()?;
        let attacker_root = private_tempdir()?;
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
