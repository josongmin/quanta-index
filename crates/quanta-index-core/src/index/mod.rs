//! Native incremental application contracts. No backend or runtime dependency.
mod attempt;
pub mod budget;
pub mod effort;
mod error;
mod lifetime;
mod map_roots;
mod map_value;
mod memory;
mod metadata;
mod mutations;
mod owned_work;
mod owner_coverage;
pub mod ports;
mod query;
mod read_settlement;
mod reclaim;
mod records;
mod registration;
mod scratch;
mod searchable;
mod staging_payload;
mod work;
pub use budget::{WorkBudget, WorkBudgetDenial, WorkLimits, WorkUsage};
pub use error::IndexError;
pub use lifetime::{
    ReadSessionLease, ReadSessionRecoveryPage, ReadSessionRef, ReadTicketLease,
    validate_read_ticket_lease,
};
pub use map_roots::{MapRootBuilder, StagedMapRootIter, StagedMapRoots};
pub use map_value::{
    EncodedMapValue, InlineMapValue, MAX_MAP_INLINE_BYTES, MAX_MAP_INLINE_CHILDREN,
    MAX_MAP_VALUE_WIRE_BYTES, MapValue,
};
pub use memory::{MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort};
pub use metadata::{
    MAX_BINDING_METADATA_BYTES, decode_coverage_binding, decode_member_binding,
    decode_owner_coverage, decode_owner_member_binding, decode_projection_binding,
    decode_projection_certificate, decode_projection_owner,
};
pub use mutations::{
    OwnerMutationCursor, OwnerMutationDigest, SourceChangeCursor, SourceInputCursor,
    SourceSnapshotCursor, decode_owner_mutation_value, owner_mutation_value,
};
pub use owned_work::{
    OWNED_WORK_PRODUCER, OwnedReadyOutcome, OwnedWorkAssignment, OwnedWorkBegin, OwnedWorkDefer,
    OwnedWorkGrant, OwnedWorkPolicy, OwnedWorkReport, OwnedWorkTerminal,
};
pub use owner_coverage::{pending_owner_coverage_value, read_owner_coverage};
pub use ports::{
    AttemptContext, ObjectReadFile, ObjectReadIoPort, ObjectWriteIoPort, ObjectWriteTicket,
};
pub use quanta_index_contract::index::*;
pub use query::{
    FiniteScore, FiniteScoreError, IndexQueryPort, NativeTextHitSource, QueryCoverage,
    QueryOutcome, QueryReadContext,
};
pub use read_settlement::{
    CatalogReadSettlementPort, NativeReadReleaseObservation, NativeReadSettlement,
    NativeReadSettlementScope, NativeReadSettlementStatus,
};
pub use reclaim::{
    MAX_RECLAIM_CANDIDATES, MAX_RECLAIM_CHILDREN, MAX_RECLAIM_DRAIN_PAGE, ObjectNamespaceBinding,
    ObjectUnlinkPort, PhysicalIncarnation, ReclaimCandidate, ReclaimDrainPage, ReclaimKind,
    ReclaimPermit, ReclaimTarget, ReclaimWork, ReclaimWorkPage, RetiringRef,
};
pub use records::{
    AdmissionOutcome, BaseMember, BootstrapRef, DeltaInput, MapBatchRequest, MapBatchWrite,
    MapEdit, MapPage, MapPageCursor, MapPageData, MapPageEntry, MapWrite, ObjectBatchInput,
    ObjectInstallScope, OrdinalMapEdit, OrdinalMapEditData, PlanContext, PlanDecision,
    PreparedPublication, SourceChange, SourceDelta, SourcePathKey, SourceRecordOwner,
    StagedObjectReferenceIter, StagedObjectReferences, StoredObject, StoredObjectData,
    TicketRecord, member_map_key, owner_from_map_key, owner_from_path_map_value, owner_map_key,
    path_owner_map_value, producer_owner_prefix,
};
pub use registration::{
    RegisteredObject, RegisteredObjectBatch, RegisteredObjectData, RegisteredObjectIter,
};
pub use scratch::{
    ScratchAllocation, ScratchIoBinding, ScratchIoScope, ScratchNamespaceLease, ScratchRequest,
    ScratchSpacePort,
};
pub use work::{
    MAX_WORK_ACTION_BYTES, ObjectChildren, WorkAction, WorkActionDigest, WorkActionSet,
};

pub use searchable::{SearchableParentPreparation, SearchablePreparation};
pub use staging_payload::{StagedObjectBatch, StagedPayload, encode_staged_json};
