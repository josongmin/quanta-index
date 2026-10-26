//! Typed, vendor-independent events for one process-local diagnostic trace.
//!
//! This module owns only identity, timing, and the producer-side sink contract.
//! It performs no I/O and does not decide product outcomes. Adapters provide a
//! nonblocking bounded sink and share one monotonic clock for the capture.

use std::fmt;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Stable identity for one capture. It is diagnostic metadata, never authority.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DiagnosticCaptureIdV1(pub [u8; 16]);

/// A stream-local span identity. Zero is not a valid identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DiagnosticSpanIdV1(NonZeroU64);

impl DiagnosticSpanIdV1 {
    #[must_use]
    pub const fn from_nonzero(value: NonZeroU64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    #[must_use]
    pub const fn as_nonzero(self) -> NonZeroU64 {
        self.0
    }
}

/// Stable, bounded logical lane. It does not imply an OS thread or CPU lane.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DiagnosticLaneV1(pub u16);

/// A clock shared by every producer in one process-local capture.
pub trait DiagnosticClockV1: fmt::Debug + Send + Sync {
    /// Monotonic nanoseconds since the capture origin.
    fn now_ns(&self) -> u64;
}

/// Closed stage vocabulary for the tracing contract.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum DiagnosticStageV1 {
    Request,
    Operation,
    ClientAttempt,
    Ingress,
    QueueWait,
    Dispatch,
    Backend,
    Encode,
    Write,
    BuildPreflight,
    BuildPrepare,
    SemanticPrepare,
    SemanticStream,
    SemanticDelete,
    SemanticAppend,
    SemanticSeal,
    SemanticPromote,
    SemanticBuild,
    LexicalBuild,
    NativeTextPostings,
    NativeTextRoot,
    NativeCoverageScan,
    NativeCoverageMapApply,
    NativeSearchablePrepare,
    NativeCoverageSave,
    NativeMemberAssemble,
    Bm25CensusBuild,
    Bm25LiveBuild,
    Bm25Boundary,
    Finalize,
    Activate,
    CatalogLockWait,
    CatalogLockHold,
    RetentionPairLockWait,
    RetentionPairLockHold,
    RetentionRootLockWait,
    RetentionRootLockHold,
    RetentionRootSnapshot,
    SourceTransactionOne,
    SourceTransactionTwo,
    NativeSourceApply,
    NativeSourcePathInstall,
    NativeSourceMapApply,
    RetentionPlan,
    RetentionMeasure,
    RetentionWrite,
    RetentionReap,
    ClosureRead,
    ClosureHash,
    ClosureDecode,
    SqliteWork,
    PhysicalInodeCensus,
    ManagedObjectReserve,
    ManagedObjectConfirm,
    NativePublish,
    FilePublish,
    LexicalSeal,
    LexicalProof,
    Compaction,
    QueryParse,
    QueryPlan,
    QueryReadView,
    QueryOverlaySnapshot,
    QueryFast,
    QueryDeep,
    QueryLexical,
    QueryEmbedding,
    QueryOverlayEmbedding,
    QueryCandidate,
    QueryVerify,
    QueryRank,
    QueryProject,
    HarnessPhase,
    ObserverSetup,
    ObserverSample,
    ObserverTeardown,
    ArtifactWrite,
    RuntimeBoot,
    BootRestore,
    BootReclaim,
    BootInventory,
    BootActiveProof,
    RuntimeMaintenance,
    RuntimeStop,
    RuntimeJoin,
}

/// Root authority is explicit and independent of the product request ID.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticRootKindV1 {
    Request,
    Operation,
    Capture,
}

/// Admitted IPC plane attached to a server request root.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticPlaneV1 {
    Query,
    Control,
    Ingest,
}

/// Closed scale-harness phase names used to correlate trace spans with the
/// existing strict phase resource record.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticPhaseV1 {
    FullIngestSeal,
    FullIngest,
    FullSeal,
    FullActivate,
    DeltaIngestSeal,
    DeltaActivate,
    NoopSeal,
    NoopActivate,
    DeleteSeal,
    DeleteActivate,
    SameProcessReopen,
    NativeCorpusGeneration,
    NativeConfigSetup,
    NativeStartup,
    NativeWorkspaceCreate,
    NativeProfileConfigure,
    NativeFullBeginInput,
    NativeFullStageSources,
    NativeFullEventAck,
    NativeFirstSearch,
    NativeDeltaStageSource,
    NativeDeltaEventAck,
    NativeDeltaSearch,
    NativeNoopEventAck,
    NativeNoopStableSearch,
    NativeDeleteEventAck,
    NativeDeletedSearch,
    NativeOldPinSearch,
    NativeChurnCycles,
    NativeChurnOldViewSearch,
    NativeDrain,
    NativeReopen,
    NativeReopenReadHead,
    NativeReopenDeletedSearch,
    NativeReopenChurnSearch,
    NativeReopenSearch,
    NativeFinalDrain,
}

/// Closed sync call sites. Labels are diagnostic dimensions, never product
/// labels or arbitrary strings.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(usize)]
pub enum DiagnosticSyncLabelV1 {
    GenerationDirectory,
    PublicationDirectory,
    PublicationParent,
    OverlayDirectory,
    HistoryManifestFile,
    CoverageOrphanDirectory,
    HistoryLayoutDirectory,
    AtomicFile,
    AtomicParent,
    AtomicAtFile,
    AtomicAtParent,
    SealedLeftoverDirectory,
    PoolNewDirectoryFull,
    PoolNewParentFull,
    PoolAdoptionFullBarrier,
    PoolDataFilePosix,
    PoolObjectFileFull,
    PoolControlFileFull,
    PoolDataShardPosix,
    PoolObjectShardFull,
    PoolControlParentFull,
    PoolReusedFilePosix,
    PoolReusedShardPosix,
    PoolAdoptionJournalFull,
    PoolSyncAll,
    StateSyncAll,
}

impl DiagnosticSyncLabelV1 {
    pub const ALL: [Self; 26] = [
        Self::GenerationDirectory,
        Self::PublicationDirectory,
        Self::PublicationParent,
        Self::OverlayDirectory,
        Self::HistoryManifestFile,
        Self::CoverageOrphanDirectory,
        Self::HistoryLayoutDirectory,
        Self::AtomicFile,
        Self::AtomicParent,
        Self::AtomicAtFile,
        Self::AtomicAtParent,
        Self::SealedLeftoverDirectory,
        Self::PoolNewDirectoryFull,
        Self::PoolNewParentFull,
        Self::PoolAdoptionFullBarrier,
        Self::PoolDataFilePosix,
        Self::PoolObjectFileFull,
        Self::PoolControlFileFull,
        Self::PoolDataShardPosix,
        Self::PoolObjectShardFull,
        Self::PoolControlParentFull,
        Self::PoolReusedFilePosix,
        Self::PoolReusedShardPosix,
        Self::PoolAdoptionJournalFull,
        Self::PoolSyncAll,
        Self::StateSyncAll,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::GenerationDirectory => "generation_directory",
            Self::PublicationDirectory => "publication_directory",
            Self::PublicationParent => "publication_parent",
            Self::OverlayDirectory => "overlay_directory",
            Self::HistoryManifestFile => "history_manifest_file",
            Self::CoverageOrphanDirectory => "coverage_orphan_directory",
            Self::HistoryLayoutDirectory => "history_layout_directory",
            Self::AtomicFile => "atomic_file",
            Self::AtomicParent => "atomic_parent",
            Self::AtomicAtFile => "atomic_at_file",
            Self::AtomicAtParent => "atomic_at_parent",
            Self::SealedLeftoverDirectory => "sealed_leftover_directory",
            Self::PoolNewDirectoryFull => "pool_new_directory_full",
            Self::PoolNewParentFull => "pool_new_parent_full",
            Self::PoolAdoptionFullBarrier => "pool_adoption_full_barrier",
            Self::PoolDataFilePosix => "pool_data_file_posix",
            Self::PoolObjectFileFull => "pool_object_file_full",
            Self::PoolControlFileFull => "pool_control_file_full",
            Self::PoolDataShardPosix => "pool_data_shard_posix",
            Self::PoolObjectShardFull => "pool_object_shard_full",
            Self::PoolControlParentFull => "pool_control_parent_full",
            Self::PoolReusedFilePosix => "pool_reused_file_posix",
            Self::PoolReusedShardPosix => "pool_reused_shard_posix",
            Self::PoolAdoptionJournalFull => "pool_adoption_journal_full",
            Self::PoolSyncAll => "pool_sync_all",
            Self::StateSyncAll => "state_sync_all",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticAdoptionFlushReasonV1 {
    RefLimit,
    ByteLimit,
    Explicit,
}

/// Fixed occupancy bands against the existing adoption batch caps: 64 refs
/// and 2,094,425 data bytes (`2 MiB - 2,727-byte manifest allowance`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticOccupancyBucketV1 {
    Zero,
    UpTo25Percent,
    UpTo50Percent,
    UpTo75Percent,
    UpTo100Percent,
    Over100Percent,
}

/// Terminal state emitted by an explicit finish or by guard drop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticTerminalV1 {
    Ok,
    Error { code: u32 },
    Cancelled { code: u32 },
    Aborted,
}

impl DiagnosticTerminalV1 {
    /// Classify a product result for diagnostic finish without consuming it.
    pub fn from_result<T, E>(result: &Result<T, E>) -> Self {
        match result {
            Ok(_) => Self::Ok,
            Err(_) => Self::Error { code: 1 },
        }
    }
}

/// Fixed work counter keys. Values are aggregate work, never synthetic spans.
///
/// The six `SqliteSync*` keys classify a native `xSync` attempt by mutations
/// since its last successful settlement. A reached native sync span emits
/// exactly one of them with value 1, independent of the sync result. This is
/// not evidence of a stable parent, durability credit, or permission to skip.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum DiagnosticCounterV1 {
    FilesVisited,
    FilesRead,
    FilesHashed,
    FilesDecoded,
    InodesVisited,
    UniqueInodes,
    PhysicalBytesMeasured,
    RetainedPairs,
    MeasureCalls,
    RemovedRoots,
    SemanticWindows,
    SemanticOwnerScopes,
    SemanticDeleteCalls,
    SemanticAppendCalls,
    SemanticRowsAppended,
    SemanticRowsDeleted,
    SemanticVectorBytes,
    QueryAdmittedLimit,
    QueryReadLimit,
    QueryByteLimit,
    RegexLiteralCount,
    RegexAlternativeCount,
    RegexNecessaryGramCount,
    BytesRead,
    BytesHashed,
    BytesDecoded,
    /// Accepted logical work during a stage, not physical I/O or failed charges.
    WorkReads,
    WorkWrites,
    WorkBytes,
    WorkMutations,
    SqliteRows,
    SqliteBatches,
    /// Reached lower-VFS xWrite calls in one diagnostic I/O scope. WAL, main
    /// and other calls partition this total; failures are included in it.
    /// These are not cache misses/spills, unique pages or durable commits.
    SqliteWriteCalls,
    /// Sum of requested xWrite byte lengths, including failed/short writes.
    /// The native callback does not prove how many bytes reached storage.
    SqliteWriteRequestedBytes,
    SqliteWriteFailures,
    SqliteWalWriteCalls,
    SqliteMainWriteCalls,
    SqliteOtherWriteCalls,
    /// Native lexical query aggregate; not per-hit spans or physical I/O.
    QueryPostingCalls,
    QueryPostingHits,
    QueryPostingElapsedNs,
    QueryPhraseChecks,
    QueryPhraseElapsedNs,
    Candidates,
    CandidatesVerified,
    Matches,
    RejectedAtGuard,
    SyncCalls,
    SyncFailures,
    SyncElapsedNs,
    SqliteSyncOverwrite,
    SqliteSyncGrowth,
    SqliteSyncTruncate,
    SqliteSyncCreate,
    SqliteSyncNoMutation,
    SqliteSyncUnclassified,
    NewObjects,
    ReusedObjects,
    CompactedDocuments,
    CompactedSegments,
    CompactedBytes,
    PostingMutations,
    MapPageMutations,
    /// Entries passed to one reached managed reserve/confirm call, including
    /// retries and refusals. Not newly created files or confirmed registrations.
    ManagedObjectInputs,
    InvalidAttemptTokens,
    UniqueClosureRefs,
    ClosureDependencyEdges,
    ClosureAuthenticatedBytes,
    ClosureReadElapsedNs,
    ClosureHashElapsedNs,
    ClosureDecodeElapsedNs,
    SqliteElapsedNs,
    QueueDropped,
    QueueAccepted,
    OutputWritten,
    OutputUnwritten,
}

/// Where an authenticated immutable ANN region was observed. This is
/// identity metadata, not additive work or a recall/exhaustion claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticAnnRegionPhaseV1 {
    Installed,
    SelectedQuery,
}

/// Closed index mode copied from the verified region seal, without exposing a
/// backend library type through the diagnostic contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticAnnIndexKindV1 {
    Flat,
    IvfHnswSq,
}

/// One bounded region identity/shape observation.
///
/// `fileset_ref` identifies immutable bytes across installation and query;
/// a selected query also binds
/// the authenticated directory descriptor. Ordinals are directory-local;
/// `chunks` counts file-manifest entries, not unique objects or bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticAnnRegionObservationV1 {
    pub phase: DiagnosticAnnRegionPhaseV1,
    pub fileset_ref: [u8; 32],
    pub descriptor_ref: Option<[u8; 32]>,
    pub ordinal: u64,
    pub rows: u32,
    pub index_kind: DiagnosticAnnIndexKindV1,
    pub files: u32,
    pub chunks: u32,
}

/// One native sync call observation, never a durability or settlement grant.
///
/// Missing syscall results mean that the syscall was not attempted. An
/// unobserved, ambiguous, foreign-FD or overridden call cannot prove that the
/// kernel completed a full barrier, even when the enclosing `SQLite` call succeeds.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the independent native observation flags preserve evidence rather than configuring a state machine"
)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiagnosticSqliteSyncObservationV1 {
    pub observed: bool,
    pub same_native_fd: bool,
    pub ambiguous: bool,
    pub full_requested: bool,
    pub full_default_syscall: bool,
    pub full_result: Option<i32>,
    pub full_errno: Option<i32>,
    pub posix_result: Option<i32>,
    pub posix_errno: Option<i32>,
}

/// Closed lexical object kinds used for bounded publication attribution.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(usize)]
pub enum DiagnosticObjectKindV1 {
    FileBody,
    FilePostings,
    NativeBody,
    NativeTrigrams,
    NativePositions,
    Segment,
    Record,
    MapPage,
    BlobBlock,
    BlobNode,
    AttemptBatch,
}

/// Closed BM25 seal-boundary labels used by canonical trace aggregates.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticBm25BoundaryLabelV1 {
    BaseRead,
    Encode,
}

impl DiagnosticBm25BoundaryLabelV1 {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BaseRead => "base_read",
            Self::Encode => "encode",
        }
    }
}

impl DiagnosticObjectKindV1 {
    pub const ALL: [Self; 11] = [
        Self::FileBody,
        Self::FilePostings,
        Self::NativeBody,
        Self::NativeTrigrams,
        Self::NativePositions,
        Self::Segment,
        Self::Record,
        Self::MapPage,
        Self::BlobBlock,
        Self::BlobNode,
        Self::AttemptBatch,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FileBody => "file_body",
            Self::FilePostings => "file_postings",
            Self::NativeBody => "native_body",
            Self::NativeTrigrams => "native_trigrams",
            Self::NativePositions => "native_positions",
            Self::Segment => "segment",
            Self::Record => "record",
            Self::MapPage => "map_page",
            Self::BlobBlock => "blob_block",
            Self::BlobNode => "blob_node",
            Self::AttemptBatch => "attempt_batch",
        }
    }
}

/// A reference that may point to another stream or process clock.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DiagnosticTraceRefV1 {
    pub capture_id: DiagnosticCaptureIdV1,
    pub span_id: DiagnosticSpanIdV1,
}

/// Producer event. The writer assigns output sequence after dequeue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticEventKindV1 {
    SpanBegin {
        span_id: DiagnosticSpanIdV1,
        parent_span_id: Option<DiagnosticSpanIdV1>,
        root_kind: Option<DiagnosticRootKindV1>,
        stage: DiagnosticStageV1,
        lane: DiagnosticLaneV1,
    },
    SpanEnd {
        span_id: DiagnosticSpanIdV1,
        outcome: DiagnosticTerminalV1,
    },
    RequestIdentity {
        span_id: DiagnosticSpanIdV1,
        request_id: u64,
        plane: DiagnosticPlaneV1,
        connection_id: u64,
    },
    PhaseIdentity {
        span_id: DiagnosticSpanIdV1,
        phase: DiagnosticPhaseV1,
    },
    Counter {
        span_id: DiagnosticSpanIdV1,
        key: DiagnosticCounterV1,
        value: u64,
    },
    AnnRegionObservation {
        span_id: DiagnosticSpanIdV1,
        observation: DiagnosticAnnRegionObservationV1,
    },
    SqliteSyncObservation {
        span_id: DiagnosticSpanIdV1,
        observation: DiagnosticSqliteSyncObservationV1,
    },
    SyncAggregate {
        span_id: DiagnosticSpanIdV1,
        label: DiagnosticSyncLabelV1,
        calls: u64,
        elapsed_sum_ns: u64,
        elapsed_max_ns: u64,
        failures: u64,
    },
    ObjectKindAggregate {
        span_id: DiagnosticSpanIdV1,
        kind: DiagnosticObjectKindV1,
        new_objects: u64,
        reused_objects: u64,
    },
    AdoptionGroupAggregate {
        span_id: DiagnosticSpanIdV1,
        reason: DiagnosticAdoptionFlushReasonV1,
        refs: DiagnosticOccupancyBucketV1,
        bytes: DiagnosticOccupancyBucketV1,
        groups: u64,
        failed_groups: u64,
    },
    Bm25CensusAggregate {
        span_id: DiagnosticSpanIdV1,
        calls: u64,
        elapsed_ns: u64,
        docs: u64,
        encoded_bytes: u64,
    },
    Bm25BoundaryAggregate {
        span_id: DiagnosticSpanIdV1,
        label: DiagnosticBm25BoundaryLabelV1,
        calls: u64,
        elapsed_sum_ns: u64,
        logical_bytes: u64,
        index_files: u64,
    },
    Bm25LiveBuildAggregate {
        span_id: DiagnosticSpanIdV1,
        calls: u64,
        elapsed_ns: u64,
        reused_segments: u64,
        changed_segments: u64,
        new_segments: u64,
        mask_docs: u64,
        new_census_docs: u64,
        newly_dead_docs: u64,
        logical_census_bytes: u64,
        changed_ns: u64,
        new_ns: u64,
        death_ns: u64,
        correction_keys: u64,
        segment_fanout: u64,
        retained_estimate_bytes: u64,
    },
    Link {
        span_id: DiagnosticSpanIdV1,
        target: DiagnosticTraceRefV1,
    },
    Health {
        code: DiagnosticHealthCodeV1,
    },
}

/// One event before serialization. `ts_ns` shares the capture clock domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticEmitV1 {
    pub capture_id: DiagnosticCaptureIdV1,
    pub ts_ns: u64,
    pub kind: DiagnosticEventKindV1,
}

/// Closed reasons that make a capture invalid even when no event was lost.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticHealthCodeV1 {
    ForeignCaptureSpan,
    MissingParentSpan,
    ClockRegressed,
    CounterOverflow,
    InvalidAttemptToken,
    DuplicateAttemptToken,
    UnlinkedAttempt,
}

/// Nonblocking producer-side contract. The writer tracks attempted, accepted,
/// dropped, written, and unwritten events; producers never wait for the sink.
pub trait DiagnosticEventSinkV1: fmt::Debug + Send + Sync {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticTraceErrorV1 {
    SpanIdentityExhausted,
    ClockRegressed,
    ForeignCaptureSpan,
    MissingParentSpan,
}

impl fmt::Display for DiagnosticTraceErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpanIdentityExhausted => f.write_str("diagnostic span identity exhausted"),
            Self::ClockRegressed => f.write_str("diagnostic clock moved backwards"),
            Self::ForeignCaptureSpan => f.write_str("diagnostic span belongs to another capture"),
            Self::MissingParentSpan => f.write_str("diagnostic child span has no parent"),
        }
    }
}

impl std::error::Error for DiagnosticTraceErrorV1 {}

struct TraceSessionV1 {
    capture_id: DiagnosticCaptureIdV1,
    next_span_id: AtomicU64,
    clock: Arc<dyn DiagnosticClockV1>,
    sink: Arc<dyn DiagnosticEventSinkV1>,
}

impl fmt::Debug for TraceSessionV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TraceSessionV1")
            .field("capture_id", &self.capture_id)
            .finish_non_exhaustive()
    }
}

/// Cloneable explicit context. It contains no ambient/thread-local state.
#[derive(Clone, Debug)]
pub struct DiagnosticContextV1 {
    session: Arc<TraceSessionV1>,
    parent_span_id: Option<DiagnosticSpanIdV1>,
    invalid_parent: bool,
}

impl DiagnosticContextV1 {
    /// Create a process-local capture context. Root spans are created explicitly
    /// with [`Self::enter_root`]; construction itself emits no event.
    #[must_use]
    pub fn new(
        capture_id: DiagnosticCaptureIdV1,
        clock: Arc<dyn DiagnosticClockV1>,
        sink: Arc<dyn DiagnosticEventSinkV1>,
    ) -> Self {
        Self {
            session: Arc::new(TraceSessionV1 {
                capture_id,
                next_span_id: AtomicU64::new(1),
                clock,
                sink,
            }),
            parent_span_id: None,
            invalid_parent: false,
        }
    }

    /// Start an explicit request, operation, or capture root.
    pub fn enter_root(
        &self,
        kind: DiagnosticRootKindV1,
        lane: DiagnosticLaneV1,
    ) -> Result<DiagnosticSpanGuardV1, DiagnosticTraceErrorV1> {
        if self.invalid_parent {
            return Err(DiagnosticTraceErrorV1::ForeignCaptureSpan);
        }
        self.enter_span(None, Some(kind), root_stage(kind), lane)
    }

    /// Start a child span under this context's current parent.
    ///
    /// Failures invalidate the capture through health events; callers may
    /// continue product work without treating failed observation as success.
    pub fn enter(
        &self,
        stage: DiagnosticStageV1,
        lane: DiagnosticLaneV1,
    ) -> Result<DiagnosticSpanGuardV1, DiagnosticTraceErrorV1> {
        if self.invalid_parent {
            return Err(DiagnosticTraceErrorV1::ForeignCaptureSpan);
        }
        if self.parent_span_id.is_none() {
            self.health(DiagnosticHealthCodeV1::MissingParentSpan);
            return Err(DiagnosticTraceErrorV1::MissingParentSpan);
        }
        self.enter_span(self.parent_span_id, None, stage, lane)
    }

    /// Derive a context whose children are parented to `span`.
    #[must_use]
    pub fn child_context(&self, span: &DiagnosticSpanGuardV1) -> Self {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return Self {
                session: Arc::clone(&self.session),
                parent_span_id: None,
                invalid_parent: true,
            };
        }
        Self {
            session: Arc::clone(&self.session),
            parent_span_id: Some(span.span_id),
            invalid_parent: false,
        }
    }

    /// Emit an explicit cross-span relationship. It never infers a flow from
    /// request IDs, timestamps, or writer order.
    pub fn link(&self, span: &DiagnosticSpanGuardV1, target: DiagnosticTraceRefV1) {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return;
        }
        self.emit(DiagnosticEventKindV1::Link {
            span_id: span.span_id,
            target,
        });
    }

    /// Add a fixed-key owner aggregate to a live span.
    pub fn counter(&self, span: &DiagnosticSpanGuardV1, key: DiagnosticCounterV1, value: u64) {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return;
        }
        self.emit(DiagnosticEventKindV1::Counter {
            span_id: span.span_id,
            key,
            value,
        });
    }

    /// Attach one immutable region identity to its actual seal or selected
    /// query span. Unlike counters, observations must never be summed.
    pub fn ann_region_observation(
        &self,
        span: &DiagnosticSpanGuardV1,
        observation: DiagnosticAnnRegionObservationV1,
    ) {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return;
        }
        self.emit(DiagnosticEventKindV1::AnnRegionObservation {
            span_id: span.span_id,
            observation,
        });
    }

    /// Attach the native result to its existing live sync span. The observer
    /// cannot authorize publication or replace the enclosing product result.
    pub fn sqlite_sync_observation(
        &self,
        span: &DiagnosticSpanGuardV1,
        observation: DiagnosticSqliteSyncObservationV1,
    ) {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return;
        }
        self.emit(DiagnosticEventKindV1::SqliteSyncObservation {
            span_id: span.span_id,
            observation,
        });
    }

    /// Attach one bounded per-label sync summary to its owning span.
    pub fn sync_aggregate(
        &self,
        span: &DiagnosticSpanGuardV1,
        label: DiagnosticSyncLabelV1,
        calls: u64,
        elapsed_sum_ns: u64,
        elapsed_max_ns: u64,
        failures: u64,
    ) {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return;
        }
        self.emit(DiagnosticEventKindV1::SyncAggregate {
            span_id: span.span_id,
            label,
            calls,
            elapsed_sum_ns,
            elapsed_max_ns,
            failures,
        });
    }

    /// Attach bounded new/reused object counts for one closed object kind.
    pub fn object_kind_aggregate(
        &self,
        span: &DiagnosticSpanGuardV1,
        kind: DiagnosticObjectKindV1,
        new_objects: u64,
        reused_objects: u64,
    ) {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return;
        }
        self.emit(DiagnosticEventKindV1::ObjectKindAggregate {
            span_id: span.span_id,
            kind,
            new_objects,
            reused_objects,
        });
    }

    /// Attach a bounded adoption group histogram cell to its owning span.
    pub fn adoption_group_aggregate(
        &self,
        span: &DiagnosticSpanGuardV1,
        reason: DiagnosticAdoptionFlushReasonV1,
        refs: DiagnosticOccupancyBucketV1,
        bytes: DiagnosticOccupancyBucketV1,
        groups: u64,
        failed_groups: u64,
    ) {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return;
        }
        self.emit(DiagnosticEventKindV1::AdoptionGroupAggregate {
            span_id: span.span_id,
            reason,
            refs,
            bytes,
            groups,
            failed_groups,
        });
    }

    /// Attach exact document-census work for a live-BM25 build.
    pub fn bm25_census_aggregate(
        &self,
        span: &DiagnosticSpanGuardV1,
        calls: u64,
        elapsed_ns: u64,
        docs: u64,
        encoded_bytes: u64,
    ) {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return;
        }
        self.emit(DiagnosticEventKindV1::Bm25CensusAggregate {
            span_id: span.span_id,
            calls,
            elapsed_ns,
            docs,
            encoded_bytes,
        });
    }

    /// Attach one bounded base-read or encoding boundary aggregate.
    pub fn bm25_boundary_aggregate(
        &self,
        span: &DiagnosticSpanGuardV1,
        label: DiagnosticBm25BoundaryLabelV1,
        calls: u64,
        elapsed_sum_ns: u64,
        logical_bytes: u64,
        index_files: u64,
    ) {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return;
        }
        self.emit(DiagnosticEventKindV1::Bm25BoundaryAggregate {
            span_id: span.span_id,
            label,
            calls,
            elapsed_sum_ns,
            logical_bytes,
            index_files,
        });
    }

    /// Attach the fixed live-build dimensions emitted by the lexical owner.
    #[expect(
        clippy::too_many_arguments,
        reason = "fixed schema mirrors bounded producer counters"
    )]
    pub fn bm25_live_build_aggregate(
        &self,
        span: &DiagnosticSpanGuardV1,
        calls: u64,
        elapsed_ns: u64,
        reused_segments: u64,
        changed_segments: u64,
        new_segments: u64,
        mask_docs: u64,
        new_census_docs: u64,
        newly_dead_docs: u64,
        logical_census_bytes: u64,
        changed_ns: u64,
        new_ns: u64,
        death_ns: u64,
        correction_keys: u64,
        segment_fanout: u64,
        retained_estimate_bytes: u64,
    ) {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return;
        }
        self.emit(DiagnosticEventKindV1::Bm25LiveBuildAggregate {
            span_id: span.span_id,
            calls,
            elapsed_ns,
            reused_segments,
            changed_segments,
            new_segments,
            mask_docs,
            new_census_docs,
            newly_dead_docs,
            logical_census_bytes,
            changed_ns,
            new_ns,
            death_ns,
            correction_keys,
            segment_fanout,
            retained_estimate_bytes,
        });
    }

    /// Attach validated transport identity to an admitted request root.
    pub fn request_identity(
        &self,
        span: &DiagnosticSpanGuardV1,
        request_id: u64,
        plane: DiagnosticPlaneV1,
        connection_id: u64,
    ) {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return;
        }
        self.emit(DiagnosticEventKindV1::RequestIdentity {
            span_id: span.span_id,
            request_id,
            plane,
            connection_id,
        });
    }

    /// Attach a strict harness phase label to its measured span.
    pub fn phase_identity(&self, span: &DiagnosticSpanGuardV1, phase: DiagnosticPhaseV1) {
        if !self.owns_span(span) {
            self.health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
            return;
        }
        self.emit(DiagnosticEventKindV1::PhaseIdentity {
            span_id: span.span_id,
            phase,
        });
    }

    #[must_use]
    pub fn capture_id(&self) -> DiagnosticCaptureIdV1 {
        self.session.capture_id
    }

    /// Whether another context shares this exact process-local capture session.
    /// Capture IDs alone are not sufficient: independently created sessions may
    /// intentionally use the same ID while owning different clocks and sinks.
    #[must_use]
    pub fn shares_capture_session(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.session, &other.session)
    }

    /// Whether spans entered from this context inherit an explicit parent.
    #[must_use]
    pub const fn has_parent(&self) -> bool {
        self.parent_span_id.is_some()
    }

    /// Number of live contexts and spans sharing this capture session.
    /// Close refuses a complete footer while a cloned context has escaped.
    #[must_use]
    pub fn strong_count(&self) -> usize {
        Arc::strong_count(&self.session)
    }

    /// Whether this span belongs to this exact capture session. A matching
    /// public capture ID alone does not establish ownership.
    #[must_use]
    pub fn owns_span(&self, span: &DiagnosticSpanGuardV1) -> bool {
        Arc::ptr_eq(&self.session, &span.context.session)
    }

    /// Record a closed trace-health condition without changing product outcome.
    pub fn health(&self, code: DiagnosticHealthCodeV1) {
        self.emit(DiagnosticEventKindV1::Health { code });
    }

    fn enter_span(
        &self,
        parent_span_id: Option<DiagnosticSpanIdV1>,
        root_kind: Option<DiagnosticRootKindV1>,
        stage: DiagnosticStageV1,
        lane: DiagnosticLaneV1,
    ) -> Result<DiagnosticSpanGuardV1, DiagnosticTraceErrorV1> {
        let mut raw_id = self.session.next_span_id.load(Ordering::Relaxed);
        loop {
            let Some(next) = raw_id.checked_add(1) else {
                self.health(DiagnosticHealthCodeV1::CounterOverflow);
                return Err(DiagnosticTraceErrorV1::SpanIdentityExhausted);
            };
            match self.session.next_span_id.compare_exchange_weak(
                raw_id,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(observed) => raw_id = observed,
            }
        }
        let span_id = NonZeroU64::new(raw_id)
            .map(DiagnosticSpanIdV1)
            .ok_or(DiagnosticTraceErrorV1::SpanIdentityExhausted)?;
        let started_ns = self.session.clock.now_ns();
        self.emit_at(
            started_ns,
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id,
                root_kind,
                stage,
                lane,
            },
        );
        Ok(DiagnosticSpanGuardV1 {
            context: self.clone(),
            span_id,
            started_ns,
            finished: false,
        })
    }

    fn emit(&self, kind: DiagnosticEventKindV1) {
        self.emit_at(self.session.clock.now_ns(), kind);
    }

    fn emit_at(&self, ts_ns: u64, kind: DiagnosticEventKindV1) {
        let _accepted = self.session.sink.try_emit(DiagnosticEmitV1 {
            capture_id: self.session.capture_id,
            ts_ns,
            kind,
        });
    }
}

/// Bounded aggregate emitted by physical inode census walkers.
///
/// The walker reports metadata-derived sizes, not bytes read from disk. Keeping
/// this distinction explicit prevents the diagnostic from implying I/O that
/// did not occur.
pub(crate) struct DiagnosticInodeCensusV1 {
    context: DiagnosticContextV1,
    span: Option<DiagnosticSpanGuardV1>,
    inodes_visited: u64,
    files_visited: u64,
    unique_inodes: u64,
    physical_bytes_measured: u64,
    overflowed: bool,
}

impl DiagnosticInodeCensusV1 {
    pub(crate) fn new(context: Option<&DiagnosticContextV1>) -> Option<Self> {
        let context = context?.clone();
        let span = context
            .enter(DiagnosticStageV1::PhysicalInodeCensus, DiagnosticLaneV1(0))
            .map_or_else(
                |trace_error| {
                    context.health(match trace_error {
                        DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                            DiagnosticHealthCodeV1::CounterOverflow
                        }
                        DiagnosticTraceErrorV1::ClockRegressed => {
                            DiagnosticHealthCodeV1::ClockRegressed
                        }
                        DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                            DiagnosticHealthCodeV1::ForeignCaptureSpan
                        }
                        DiagnosticTraceErrorV1::MissingParentSpan => {
                            DiagnosticHealthCodeV1::MissingParentSpan
                        }
                    });
                    None
                },
                Some,
            )?;
        Some(Self {
            context,
            span: Some(span),
            inodes_visited: 0,
            files_visited: 0,
            unique_inodes: 0,
            physical_bytes_measured: 0,
            overflowed: false,
        })
    }

    pub(crate) fn visit_inode(&mut self) {
        Self::checked_accumulate(
            &self.context,
            &mut self.overflowed,
            &mut self.inodes_visited,
            1,
        );
    }

    pub(crate) fn visit_file(&mut self, unique_size: Option<u64>) {
        Self::checked_accumulate(
            &self.context,
            &mut self.overflowed,
            &mut self.files_visited,
            1,
        );
        if let Some(size) = unique_size {
            Self::checked_accumulate(
                &self.context,
                &mut self.overflowed,
                &mut self.unique_inodes,
                1,
            );
            Self::checked_accumulate(
                &self.context,
                &mut self.overflowed,
                &mut self.physical_bytes_measured,
                size,
            );
        }
    }

    fn checked_accumulate(
        context: &DiagnosticContextV1,
        overflowed: &mut bool,
        counter: &mut u64,
        amount: u64,
    ) {
        if let Some(next) = counter.checked_add(amount) {
            *counter = next;
        } else if !*overflowed {
            *overflowed = true;
            context.health(DiagnosticHealthCodeV1::CounterOverflow);
        }
    }

    pub(crate) fn finish(mut self, success: bool) {
        self.emit_counters();
        if let Some(span) = self.span.take() {
            let outcome = if success {
                DiagnosticTerminalV1::Ok
            } else {
                DiagnosticTerminalV1::Error { code: 0 }
            };
            let _trace_completion = span.finish(outcome);
        }
    }

    fn emit_counters(&self) {
        let Some(span) = self.span.as_ref() else {
            return;
        };
        for (key, value) in [
            (DiagnosticCounterV1::InodesVisited, self.inodes_visited),
            (DiagnosticCounterV1::FilesVisited, self.files_visited),
            (DiagnosticCounterV1::UniqueInodes, self.unique_inodes),
            (
                DiagnosticCounterV1::PhysicalBytesMeasured,
                self.physical_bytes_measured,
            ),
        ] {
            if value != 0 {
                self.context.counter(span, key, value);
            }
        }
    }
}

impl Drop for DiagnosticInodeCensusV1 {
    fn drop(&mut self) {
        self.emit_counters();
    }
}

/// RAII completion guard. Unfinished work ends as `Aborted`, never `Ok`.
#[derive(Debug)]
pub struct DiagnosticSpanGuardV1 {
    context: DiagnosticContextV1,
    span_id: DiagnosticSpanIdV1,
    started_ns: u64,
    finished: bool,
}

impl DiagnosticSpanGuardV1 {
    #[must_use]
    pub fn span_id(&self) -> DiagnosticSpanIdV1 {
        self.span_id
    }

    #[must_use]
    pub fn trace_ref(&self) -> DiagnosticTraceRefV1 {
        DiagnosticTraceRefV1 {
            capture_id: self.context.capture_id(),
            span_id: self.span_id,
        }
    }

    /// Convert this span into the fixed-size request attempt token.
    #[must_use]
    pub fn attempt_token(&self) -> quanta_index_contract::ipc::DiagnosticAttemptTokenV1 {
        quanta_index_contract::ipc::DiagnosticAttemptTokenV1::new(
            self.context.capture_id().0,
            self.span_id.as_nonzero(),
        )
    }

    /// Finish exactly once. A clock regression invalidates the capture and
    /// permanently closes the guard without fabricating an end timestamp.
    /// The error is recorded as capture health before it is returned.
    pub fn finish(mut self, outcome: DiagnosticTerminalV1) -> Result<(), DiagnosticTraceErrorV1> {
        let ended_ns = self.context.session.clock.now_ns();
        if ended_ns < self.started_ns {
            self.finished = true;
            self.context.emit_at(
                ended_ns,
                DiagnosticEventKindV1::Health {
                    code: DiagnosticHealthCodeV1::ClockRegressed,
                },
            );
            return Err(DiagnosticTraceErrorV1::ClockRegressed);
        }
        self.context.emit_at(
            ended_ns,
            DiagnosticEventKindV1::SpanEnd {
                span_id: self.span_id,
                outcome,
            },
        );
        self.finished = true;
        Ok(())
    }
}

impl Drop for DiagnosticSpanGuardV1 {
    fn drop(&mut self) {
        if !self.finished {
            let ended_ns = self.context.session.clock.now_ns();
            if ended_ns >= self.started_ns {
                self.context.emit_at(
                    ended_ns,
                    DiagnosticEventKindV1::SpanEnd {
                        span_id: self.span_id,
                        outcome: DiagnosticTerminalV1::Aborted,
                    },
                );
            } else {
                self.context.emit_at(
                    ended_ns,
                    DiagnosticEventKindV1::Health {
                        code: DiagnosticHealthCodeV1::ClockRegressed,
                    },
                );
            }
        }
    }
}

/// Concrete client attempt span; transport adapters own their callback ports.
pub struct DiagnosticAttemptGuardCoreV1 {
    span: DiagnosticSpanGuardV1,
}

impl fmt::Debug for DiagnosticAttemptGuardCoreV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiagnosticAttemptGuardCoreV1")
            .field("span_id", &self.span.span_id())
            .finish()
    }
}

impl DiagnosticAttemptGuardCoreV1 {
    #[must_use]
    pub fn token(&self) -> quanta_index_contract::ipc::DiagnosticAttemptTokenV1 {
        self.span.attempt_token()
    }

    pub fn finish(self, outcome: quanta_index_contract::ipc::DiagnosticAttemptOutcomeV1) {
        let terminal = match outcome {
            quanta_index_contract::ipc::DiagnosticAttemptOutcomeV1::Ok => DiagnosticTerminalV1::Ok,
            quanta_index_contract::ipc::DiagnosticAttemptOutcomeV1::Error => {
                DiagnosticTerminalV1::Error { code: 0 }
            }
            quanta_index_contract::ipc::DiagnosticAttemptOutcomeV1::Cancelled => {
                DiagnosticTerminalV1::Cancelled { code: 0 }
            }
        };
        let DiagnosticAttemptGuardCoreV1 { span } = self;
        match span.finish(terminal) {
            Ok(()) | Err(_) => {}
        }
    }
}

impl DiagnosticContextV1 {
    #[must_use]
    pub fn begin_attempt(&self) -> Option<DiagnosticAttemptGuardCoreV1> {
        if self.invalid_parent {
            return None;
        }
        // A caller may attach an attempt source directly to a capture lease.
        // That span is a standalone operation root until a real parent is
        // supplied; a parentless child would be rejected by the raw parser.
        self.enter_span(
            self.parent_span_id,
            self.parent_span_id
                .is_none()
                .then_some(DiagnosticRootKindV1::Operation),
            DiagnosticStageV1::ClientAttempt,
            DiagnosticLaneV1(0),
        )
        .map_or_else(
            |trace_error| {
                self.health(match trace_error {
                    DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                        DiagnosticHealthCodeV1::CounterOverflow
                    }
                    DiagnosticTraceErrorV1::ClockRegressed => {
                        DiagnosticHealthCodeV1::ClockRegressed
                    }
                    DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                        DiagnosticHealthCodeV1::ForeignCaptureSpan
                    }
                    DiagnosticTraceErrorV1::MissingParentSpan => {
                        DiagnosticHealthCodeV1::MissingParentSpan
                    }
                });
                None
            },
            Some,
        )
        .map(|span| DiagnosticAttemptGuardCoreV1 { span })
    }
}

const fn root_stage(kind: DiagnosticRootKindV1) -> DiagnosticStageV1 {
    match kind {
        DiagnosticRootKindV1::Request => DiagnosticStageV1::Request,
        DiagnosticRootKindV1::Operation => DiagnosticStageV1::Operation,
        DiagnosticRootKindV1::Capture => DiagnosticStageV1::HarnessPhase,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    use super::*;

    #[derive(Debug)]
    struct TestClock(AtomicU64);

    impl TestClock {
        fn advance(&self, amount: u64) {
            let _previous = self.0.fetch_add(amount, Ordering::Relaxed);
        }
    }

    impl DiagnosticClockV1 for TestClock {
        fn now_ns(&self) -> u64 {
            self.0.load(Ordering::Relaxed)
        }
    }

    #[derive(Debug)]
    struct SequencedClock(AtomicUsize);

    impl DiagnosticClockV1 for SequencedClock {
        fn now_ns(&self) -> u64 {
            if self.0.fetch_add(1, Ordering::Relaxed) == 1 {
                9
            } else {
                10
            }
        }
    }

    #[derive(Debug, Default)]
    struct TestSink(Mutex<Vec<DiagnosticEmitV1>>);

    impl DiagnosticEventSinkV1 for TestSink {
        fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
            self.0.lock().expect("recording sink lock").push(event);
            true
        }
    }

    fn recorded_events(sink: &TestSink) -> Vec<DiagnosticEmitV1> {
        sink.0.lock().expect("recording sink lock").clone()
    }

    fn recorded_event(events: &[DiagnosticEmitV1], index: usize) -> DiagnosticEmitV1 {
        *events.get(index).expect("recorded event at index")
    }

    fn context() -> (DiagnosticContextV1, Arc<TestClock>, Arc<TestSink>) {
        let clock = Arc::new(TestClock(AtomicU64::new(10)));
        let sink = Arc::new(TestSink::default());
        let context =
            DiagnosticContextV1::new(DiagnosticCaptureIdV1([7; 16]), clock.clone(), sink.clone());
        (context, clock, sink)
    }

    #[test]
    fn nested_spans_share_clock_and_preserve_explicit_parent() {
        let (context, clock, sink) = context();
        let root = context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))
            .expect("root identity");
        let child_context = context.child_context(&root);
        clock.advance(5);
        let child = child_context
            .enter(DiagnosticStageV1::LexicalBuild, DiagnosticLaneV1(2))
            .expect("child identity");
        clock.advance(7);
        child
            .finish(DiagnosticTerminalV1::Ok)
            .expect("child finish");
        clock.advance(3);
        root.finish(DiagnosticTerminalV1::Ok).expect("root finish");

        let events = recorded_events(&sink);
        assert_eq!(events.len(), 4);
        let DiagnosticEventKindV1::SpanBegin {
            span_id: root_id,
            parent_span_id: None,
            root_kind: Some(DiagnosticRootKindV1::Operation),
            ..
        } = recorded_event(&events, 0).kind
        else {
            panic!("bad root event")
        };
        let DiagnosticEventKindV1::SpanBegin {
            span_id: child_id,
            parent_span_id: Some(parent),
            ..
        } = recorded_event(&events, 1).kind
        else {
            panic!("bad child event")
        };
        assert_eq!(parent, root_id);
        assert_ne!(root_id, child_id);
        assert_eq!(recorded_event(&events, 0).ts_ns, 10);
        assert_eq!(recorded_event(&events, 1).ts_ns, 15);
        assert_eq!(recorded_event(&events, 2).ts_ns, 22);
        assert_eq!(recorded_event(&events, 3).ts_ns, 25);
    }

    #[test]
    fn parentless_child_is_rejected_with_health_and_no_span_begin() {
        let (context, _, sink) = context();
        assert_eq!(
            context
                .enter(DiagnosticStageV1::LexicalBuild, DiagnosticLaneV1(0))
                .err(),
            Some(DiagnosticTraceErrorV1::MissingParentSpan)
        );
        let events = recorded_events(&sink);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            recorded_event(&events, 0).kind,
            DiagnosticEventKindV1::Health {
                code: DiagnosticHealthCodeV1::MissingParentSpan
            }
        ));
    }

    #[test]
    fn ann_region_observation_binds_only_to_owning_span() {
        let (owner_context, _, sink) = context();
        let span = owner_context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))
            .expect("root");
        let observation = DiagnosticAnnRegionObservationV1 {
            phase: DiagnosticAnnRegionPhaseV1::Installed,
            fileset_ref: [3; 32],
            descriptor_ref: None,
            ordinal: 7,
            rows: 256,
            index_kind: DiagnosticAnnIndexKindV1::IvfHnswSq,
            files: 2,
            chunks: 3,
        };
        owner_context.ann_region_observation(&span, observation);
        let (foreign, _, foreign_sink) = context();
        foreign.ann_region_observation(&span, observation);
        span.finish(DiagnosticTerminalV1::Ok).expect("finish");
        let events = recorded_events(&sink);
        assert_eq!(events.len(), 3);
        assert!(matches!(
            recorded_event(&events, 1).kind,
            DiagnosticEventKindV1::AnnRegionObservation { observation: actual, .. }
                if actual == observation
        ));
        assert!(matches!(
            recorded_event(&recorded_events(&foreign_sink), 0).kind,
            DiagnosticEventKindV1::Health {
                code: DiagnosticHealthCodeV1::ForeignCaptureSpan
            }
        ));
    }

    #[test]
    fn sqlite_sync_observation_preserves_unknown_and_rejects_foreign_span() {
        let (owner_context, _, sink) = context();
        let span = owner_context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("root");
        let observation = DiagnosticSqliteSyncObservationV1::default();
        owner_context.sqlite_sync_observation(&span, observation);
        let (foreign, _, foreign_sink) = context();
        foreign.sqlite_sync_observation(&span, observation);
        span.finish(DiagnosticTerminalV1::Ok).expect("finish");
        let events = recorded_events(&sink);
        assert!(matches!(
            recorded_event(&events, 1).kind,
            DiagnosticEventKindV1::SqliteSyncObservation { observation: actual, .. }
                if actual == observation && actual.full_result.is_none()
        ));
        assert!(matches!(
            recorded_event(&recorded_events(&foreign_sink), 0).kind,
            DiagnosticEventKindV1::Health {
                code: DiagnosticHealthCodeV1::ForeignCaptureSpan
            }
        ));
    }

    #[test]
    fn regressed_finish_closes_guard_without_drop_retry() {
        let clock = Arc::new(SequencedClock(AtomicUsize::new(0)));
        let sink = Arc::new(TestSink::default());
        let context =
            DiagnosticContextV1::new(DiagnosticCaptureIdV1([7; 16]), clock.clone(), sink.clone());
        let span = context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("root");
        assert_eq!(
            span.finish(DiagnosticTerminalV1::Ok),
            Err(DiagnosticTraceErrorV1::ClockRegressed)
        );
        assert_eq!(clock.0.load(Ordering::Relaxed), 2);
        let events = recorded_events(&sink);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            recorded_event(&events, 1).kind,
            DiagnosticEventKindV1::Health {
                code: DiagnosticHealthCodeV1::ClockRegressed
            }
        ));
    }

    #[test]
    fn session_identity_and_parent_state_are_explicit() {
        let (capture_context, _, _) = context();
        let clone = capture_context.clone();
        let (same_id_other_session, _, _) = context();
        assert!(capture_context.shares_capture_session(&clone));
        assert!(!capture_context.shares_capture_session(&same_id_other_session));
        assert!(!capture_context.has_parent());

        let root = capture_context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("root span");
        let child = capture_context.child_context(&root);
        assert!(child.has_parent());
    }

    #[test]
    fn exhausted_span_identity_marks_capture_invalid_without_wrapping() {
        let (context, _, sink) = context();
        context
            .session
            .next_span_id
            .store(u64::MAX, Ordering::Relaxed);

        assert!(matches!(
            context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0)),
            Err(DiagnosticTraceErrorV1::SpanIdentityExhausted)
        ));
        assert_eq!(
            context.session.next_span_id.load(Ordering::Relaxed),
            u64::MAX
        );
        let events = recorded_events(&sink);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            recorded_event(&events, 0).kind,
            DiagnosticEventKindV1::Health {
                code: DiagnosticHealthCodeV1::CounterOverflow
            }
        ));
    }

    #[test]
    fn artifact_write_span_preserves_explicit_phase_parent() {
        let (context, _, sink) = context();
        let phase = context
            .enter_root(DiagnosticRootKindV1::Capture, DiagnosticLaneV1(1))
            .expect("phase root");
        let artifact = context
            .child_context(&phase)
            .enter(DiagnosticStageV1::ArtifactWrite, DiagnosticLaneV1(1))
            .expect("artifact span");
        let phase_id = phase.span_id();
        artifact
            .finish(DiagnosticTerminalV1::Ok)
            .expect("artifact finish");
        phase
            .finish(DiagnosticTerminalV1::Ok)
            .expect("phase finish");
        let events = recorded_events(&sink);
        assert!(matches!(
            recorded_event(&events, 1).kind,
            DiagnosticEventKindV1::SpanBegin {
                parent_span_id: Some(parent),
                stage: DiagnosticStageV1::ArtifactWrite,
                ..
            } if parent == phase_id
        ));
    }

    #[test]
    fn fixed_sync_and_adoption_aggregates_are_attached_to_owner_span() {
        let (context, _, sink) = context();
        let owner = context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))
            .expect("owner span");
        context.sync_aggregate(
            &owner,
            DiagnosticSyncLabelV1::PoolAdoptionFullBarrier,
            3,
            30,
            20,
            1,
        );
        context.object_kind_aggregate(&owner, DiagnosticObjectKindV1::NativeTrigrams, 4, 2);
        context.adoption_group_aggregate(
            &owner,
            DiagnosticAdoptionFlushReasonV1::RefLimit,
            DiagnosticOccupancyBucketV1::UpTo100Percent,
            DiagnosticOccupancyBucketV1::UpTo75Percent,
            2,
            1,
        );
        owner
            .finish(DiagnosticTerminalV1::Ok)
            .expect("owner finish");

        let events = recorded_events(&sink);
        assert!(events.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::SyncAggregate {
                label: DiagnosticSyncLabelV1::PoolAdoptionFullBarrier,
                calls: 3,
                elapsed_sum_ns: 30,
                elapsed_max_ns: 20,
                failures: 1,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::ObjectKindAggregate {
                kind: DiagnosticObjectKindV1::NativeTrigrams,
                new_objects: 4,
                reused_objects: 2,
                ..
            }
        )));
        assert!(events.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::AdoptionGroupAggregate {
                reason: DiagnosticAdoptionFlushReasonV1::RefLimit,
                refs: DiagnosticOccupancyBucketV1::UpTo100Percent,
                bytes: DiagnosticOccupancyBucketV1::UpTo75Percent,
                groups: 2,
                failed_groups: 1,
                ..
            }
        )));
    }

    #[test]
    fn dropped_span_is_aborted() {
        let (context, clock, sink) = context();
        let span = context
            .enter_root(DiagnosticRootKindV1::Capture, DiagnosticLaneV1(0))
            .expect("root identity");
        clock.advance(1);
        drop(span);
        let events = recorded_events(&sink);
        assert!(matches!(
            recorded_event(&events, 1).kind,
            DiagnosticEventKindV1::SpanEnd {
                outcome: DiagnosticTerminalV1::Aborted,
                ..
            }
        ));
    }

    #[test]
    fn span_identity_never_wraps() {
        let (context, _, _) = context();
        context
            .session
            .next_span_id
            .store(u64::MAX, Ordering::Relaxed);
        assert_eq!(
            context
                .enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(0))
                .err(),
            Some(DiagnosticTraceErrorV1::SpanIdentityExhausted)
        );
    }

    #[derive(Debug)]
    struct DroppingSink;

    impl DiagnosticEventSinkV1 for DroppingSink {
        fn try_emit(&self, _event: DiagnosticEmitV1) -> bool {
            false
        }
    }

    #[test]
    fn sink_rejection_does_not_change_product_terminal_state() {
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([9; 16]),
            Arc::new(TestClock(AtomicU64::new(0))),
            Arc::new(DroppingSink),
        );
        let span = context
            .enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(0))
            .expect("root identity");
        assert_eq!(span.finish(DiagnosticTerminalV1::Ok), Ok(()));
    }

    #[test]
    fn attempt_source_returns_capture_scoped_token_and_finishes_span() {
        let (context, _, sink) = context();
        let attempt = context.begin_attempt().expect("attempt span");
        let token = attempt.token();
        assert_eq!(token.capture_id(), [7; 16]);
        assert!(token.attempt_span_id().get() > 0);
        attempt.finish(quanta_index_contract::ipc::DiagnosticAttemptOutcomeV1::Ok);
        let events = recorded_events(&sink);
        assert!(matches!(
            events.first().map(|event| event.kind),
            Some(DiagnosticEventKindV1::SpanBegin {
                parent_span_id: None,
                root_kind: Some(DiagnosticRootKindV1::Operation),
                stage: DiagnosticStageV1::ClientAttempt,
                ..
            })
        ));
        assert!(matches!(
            events.last().map(|event| event.kind),
            Some(DiagnosticEventKindV1::SpanEnd {
                outcome: DiagnosticTerminalV1::Ok,
                ..
            })
        ));
    }

    #[test]
    fn attempt_source_preserves_an_explicit_parent() {
        let (context, _, sink) = context();
        let root = context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("operation root");
        let root_id = root.span_id();
        let child_context = context.child_context(&root);
        let attempt = child_context.begin_attempt().expect("attempt span");
        attempt.finish(quanta_index_contract::ipc::DiagnosticAttemptOutcomeV1::Ok);
        root.finish(DiagnosticTerminalV1::Ok).expect("root finish");

        let events = recorded_events(&sink);
        assert!(matches!(
            recorded_event(&events, 1).kind,
            DiagnosticEventKindV1::SpanBegin {
                parent_span_id: Some(parent),
                root_kind: None,
                stage: DiagnosticStageV1::ClientAttempt,
                ..
            } if parent == root_id
        ));
    }

    #[test]
    fn context_strong_count_exposes_escaped_producers_for_close_barrier() {
        let (context, _, _) = context();
        assert_eq!(context.strong_count(), 1);
        let escaped = context.clone();
        assert_eq!(context.strong_count(), 2);
        drop(escaped);
        assert_eq!(context.strong_count(), 1);
    }

    #[test]
    fn foreign_span_cannot_create_parent_or_source_events() {
        let (context, _, sink) = context();
        let foreign = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([8; 16]),
            Arc::new(TestClock(AtomicU64::new(10))),
            Arc::new(TestSink::default()),
        );
        let foreign_span = foreign
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("foreign root");
        let child = context.child_context(&foreign_span);
        assert!(child.begin_attempt().is_none());
        assert_eq!(
            child
                .enter(DiagnosticStageV1::RuntimeBoot, DiagnosticLaneV1(0))
                .err(),
            Some(DiagnosticTraceErrorV1::ForeignCaptureSpan)
        );
        assert_eq!(
            child
                .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
                .err(),
            Some(DiagnosticTraceErrorV1::ForeignCaptureSpan)
        );
        context.counter(&foreign_span, DiagnosticCounterV1::FilesRead, 1);
        context.request_identity(&foreign_span, 1, DiagnosticPlaneV1::Query, 1);
        context.phase_identity(&foreign_span, DiagnosticPhaseV1::FullIngest);
        context.link(&foreign_span, foreign_span.trace_ref());
        let events = recorded_events(&sink);
        assert_eq!(events.len(), 5);
        assert!(events.iter().all(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::Health {
                code: DiagnosticHealthCodeV1::ForeignCaptureSpan
            }
        )));
    }

    #[test]
    fn inode_census_overflow_emits_invalidity_once() {
        let (context, _, sink) = context();
        let root = context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("root");
        let child = context.child_context(&root);
        let mut census = DiagnosticInodeCensusV1::new(Some(&child)).expect("census span");
        census.physical_bytes_measured = u64::MAX;
        census.visit_file(Some(1));
        census.visit_file(Some(2));
        census.finish(true);
        let events = recorded_events(&sink);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.kind,
                    DiagnosticEventKindV1::Health {
                        code: DiagnosticHealthCodeV1::CounterOverflow
                    }
                ))
                .count(),
            1
        );
    }

    #[test]
    fn inode_census_emits_one_bounded_metadata_aggregate() {
        let (context, _, sink) = context();
        let root = context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("root");
        let child = context.child_context(&root);
        let mut census = DiagnosticInodeCensusV1::new(Some(&child)).expect("census span");
        census.visit_inode();
        census.visit_file(Some(41));
        census.visit_inode();
        census.visit_file(None);
        census.finish(true);

        let events = recorded_events(&sink);
        let counters: Vec<_> = events
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::Counter { key, value, .. } = event.kind {
                    Some((key, value))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            counters,
            vec![
                (DiagnosticCounterV1::InodesVisited, 2),
                (DiagnosticCounterV1::FilesVisited, 2),
                (DiagnosticCounterV1::UniqueInodes, 1),
                (DiagnosticCounterV1::PhysicalBytesMeasured, 41),
            ]
        );
        assert!(matches!(
            events.last().map(|event| event.kind),
            Some(DiagnosticEventKindV1::SpanEnd {
                outcome: DiagnosticTerminalV1::Ok,
                ..
            })
        ));
    }

    #[test]
    fn aborted_inode_census_preserves_partial_aggregate() {
        let (context, _, sink) = context();
        let root = context
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("root");
        let child = context.child_context(&root);
        let mut census = DiagnosticInodeCensusV1::new(Some(&child)).expect("census span");
        census.visit_inode();
        census.visit_file(Some(19));
        drop(census);

        let events = recorded_events(&sink);
        assert!(events.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::Counter {
                key: DiagnosticCounterV1::PhysicalBytesMeasured,
                value: 19,
                ..
            }
        )));
        assert!(matches!(
            events.last().map(|event| event.kind),
            Some(DiagnosticEventKindV1::SpanEnd {
                outcome: DiagnosticTerminalV1::Aborted,
                ..
            })
        ));
    }

    #[test]
    fn phase_identity_is_explicitly_attached_to_its_span() {
        let (context, _, sink) = context();
        let phase = context
            .enter_root(DiagnosticRootKindV1::Capture, DiagnosticLaneV1(3))
            .expect("phase span");
        context.phase_identity(&phase, DiagnosticPhaseV1::DeleteActivate);
        phase
            .finish(DiagnosticTerminalV1::Ok)
            .expect("phase finish");

        let events = recorded_events(&sink);
        assert!(events.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::PhaseIdentity {
                phase: DiagnosticPhaseV1::DeleteActivate,
                ..
            }
        )));
    }
}
