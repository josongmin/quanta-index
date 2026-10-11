//! Opt-in native incremental index control wire. No generic object CRUD.

use crate::index::{
    CatalogView, IndexInstanceId, IndexProfile, IngressRef, MapRoot, ObjectRef, OperationKey,
    OperationReply, OperationStatus, OwnerKey, OwnerMutation, OwnerMutationSet,
    ProjectionCertificate, ProjectionOwnerContribution, SourceInput, SourceRecord, SubmitOutcome,
    UpdateRequest, ViewRef, ViewSelection, WorkClaim, WorkIntent, WorkStatus, WorkspaceKey,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeIndexRequestEnvelope {
    pub request_id: u64,
    pub payload: NativeIndexRequest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeIndexResponseEnvelope {
    pub request_id: u64,
    pub payload: NativeIndexResponse,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeIndexRequest {
    CreateWorkspace {
        workspace: WorkspaceKey,
        epoch: u64,
    },
    /// Opens or fences one durable operation-scoped ingress before any input write.
    BeginInput(OperationKey),
    /// A staged record belongs to the ingress operation and must be retried
    /// with that operation key after an uncertain ACK.
    StageSource {
        ingress: IngressRef,
        path: String,
        bytes: Vec<u8>,
    },
    /// Bounded child-first source upload; reply record refs preserve input order.
    StageSources {
        ingress: IngressRef,
        inputs: Vec<SourceInput>,
    },
    StageProfile {
        ingress: IngressRef,
        profile: IndexProfile,
    },
    /// Opaque producer projection payload, durably staged as a leaf.
    StageProjectionOutput {
        ingress: IngressRef,
        bytes: Vec<u8>,
    },
    StageProjectionOwner {
        ingress: IngressRef,
        contribution: ProjectionOwnerContribution,
    },
    StageProjectionCertificate {
        ingress: IngressRef,
        certificate: ProjectionCertificate,
    },
    /// At most 128 ordered owner changes per request; `base` is the previous
    /// sealed page root, not a substitute for the original source CAS.
    StageMutationPage {
        ingress: IngressRef,
        base: MapRoot,
        mutations: Vec<OwnerMutation>,
    },
    SealMutations {
        ingress: IngressRef,
        root: MapRoot,
        count: u64,
    },
    DiscardInput(IngressRef),
    /// Exact case-sensitive byte literal at one acquired view. `after` is an
    /// exclusive owner cursor from the previous page.
    SearchLiteral {
        selection: ViewSelection,
        instance: IndexInstanceId,
        literal: Vec<u8>,
        after: Option<Vec<u8>>,
        limit: u32,
    },
    /// Exact pinned file inventory prefix page.
    QueryPaths {
        selection: ViewSelection,
        instance: IndexInstanceId,
        prefix: String,
        after: Option<String>,
        limit: u32,
    },
    Submit(UpdateRequest),
    /// Resume the original admitted operation from its immutable journal entry.
    Resume(OperationKey),
    ReadView(ViewSelection),
    Status(OperationKey),
    ReadyWork {
        workspace: WorkspaceKey,
        epoch: u64,
        limit: u32,
    },
    ClaimWork {
        intent: WorkIntent,
        lease_millis: u64,
    },
    RenewWork {
        claim: WorkClaim,
        lease_millis: u64,
    },
    WorkStatus(WorkIntent),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeIndexResponse {
    Created(ViewRef),
    InputOpened(IngressRef),
    Staged(ObjectRef),
    StagedSources(Vec<ObjectRef>),
    MutationPageStaged(MapRoot),
    MutationsStaged(OwnerMutationSet),
    InputDiscarded(IngressRef),
    LiteralPage(NativeLiteralPage),
    PathPage(NativePathPage),
    Submitted(OperationReply<SubmitOutcome>),
    View(CatalogView),
    Status(OperationReply<OperationStatus>),
    ReadyWork(Vec<WorkIntent>),
    Claimed(WorkClaim),
    Renewed(WorkClaim),
    WorkStatus(WorkStatus),
    Error(NativeIndexWireError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeIndexErrorCode {
    Denied,
    InvalidInput,
    Conflict,
    DigestConflict,
    FenceLost,
    ResourceLimited,
    Cancelled,
    SnapshotUnavailable,
    CorruptObject,
    BackendViolation,
    Storage,
    RebuildRequired,
    MaintenanceRequired,
    Overloaded,
    ResponseTooLarge,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeIndexWireError {
    pub code: NativeIndexErrorCode,
    pub message: String,
}

impl std::fmt::Display for NativeIndexWireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for NativeIndexWireError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLiteralHit {
    pub owner: OwnerKey,
    pub record: SourceRecord,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLiteralPage {
    pub view: ViewRef,
    pub hits: Vec<NativeLiteralHit>,
    pub after: Option<Vec<u8>>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeFilePathEntry {
    pub owner: OwnerKey,
    pub path: String,
    pub source: ObjectRef,
    pub bytes: ObjectRef,
    pub byte_len: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativePathPage {
    pub view: ViewRef,
    pub entries: Vec<NativeFilePathEntry>,
    pub has_more: bool,
}

// Closed manual codecs preserve the named JSON/CBOR representation.
quanta_index_contract_base::native_serde_struct!(NativeIndexRequestEnvelope {
    request_id: u64,
    payload: NativeIndexRequest
});
quanta_index_contract_base::native_serde_struct!(NativeIndexResponseEnvelope {
    request_id: u64,
    payload: NativeIndexResponse
});
quanta_index_contract_base::native_serde_enum!(NativeIndexRequest { units {  } newtypes { Submit = 6 => UpdateRequest, Resume = 7 => OperationKey, ReadView = 8 => ViewSelection, Status = 9 => OperationKey, WorkStatus = 13 => WorkIntent, BeginInput = 14 => OperationKey, DiscardInput = 17 => IngressRef } records { CreateWorkspace = 0 { workspace: WorkspaceKey, epoch: u64 }, StageSource = 1 { ingress: IngressRef, path: String, bytes: Vec<u8> }, StageProfile = 2 { ingress: IngressRef, profile: IndexProfile }, StageProjectionOutput = 3 { ingress: IngressRef, bytes: Vec<u8> }, StageProjectionOwner = 4 { ingress: IngressRef, contribution: ProjectionOwnerContribution }, StageProjectionCertificate = 5 { ingress: IngressRef, certificate: ProjectionCertificate }, ReadyWork = 10 { workspace: WorkspaceKey, epoch: u64, limit: u32 }, ClaimWork = 11 { intent: WorkIntent, lease_millis: u64 }, RenewWork = 12 { claim: WorkClaim, lease_millis: u64 }, StageMutationPage = 15 { ingress: IngressRef, base: MapRoot, mutations: Vec<OwnerMutation> }, SealMutations = 16 { ingress: IngressRef, root: MapRoot, count: u64 }, SearchLiteral = 18 { selection: ViewSelection, instance: IndexInstanceId, literal: Vec<u8>, after: Option<Vec<u8>>, limit: u32 }, QueryPaths = 19 { selection: ViewSelection, instance: IndexInstanceId, prefix: String, after: Option<String>, limit: u32 }, StageSources = 20 { ingress: IngressRef, inputs: Vec<SourceInput> } } });
quanta_index_contract_base::native_serde_enum!(NativeIndexResponse { units { } newtypes { Created = 0 => ViewRef, Staged = 1 => ObjectRef, Submitted = 2 => OperationReply<SubmitOutcome>, View = 3 => CatalogView, Status = 4 => OperationReply<OperationStatus>, ReadyWork = 5 => Vec<WorkIntent>, Claimed = 6 => WorkClaim, Renewed = 7 => WorkClaim, WorkStatus = 8 => WorkStatus, Error = 9 => NativeIndexWireError, InputOpened = 10 => IngressRef, MutationPageStaged = 11 => MapRoot, MutationsStaged = 12 => OwnerMutationSet, InputDiscarded = 13 => IngressRef, LiteralPage = 14 => NativeLiteralPage, PathPage = 15 => NativePathPage, StagedSources = 16 => Vec<ObjectRef> } records {  } });
quanta_index_contract_base::native_serde_enum!(NativeIndexErrorCode { units { Denied = 0, InvalidInput = 1, Conflict = 2, DigestConflict = 3, FenceLost = 4, ResourceLimited = 5, Cancelled = 6, SnapshotUnavailable = 7, CorruptObject = 8, BackendViolation = 9, Storage = 10, RebuildRequired = 11, MaintenanceRequired = 12, Overloaded = 13, ResponseTooLarge = 14 } newtypes {  } records {  } });
quanta_index_contract_base::native_serde_struct!(NativeIndexWireError {
    code: NativeIndexErrorCode,
    message: String
});
quanta_index_contract_base::native_serde_struct!(NativeLiteralHit {
    owner: OwnerKey,
    record: SourceRecord
});
quanta_index_contract_base::native_serde_struct!(NativeLiteralPage { view: ViewRef, hits: Vec<NativeLiteralHit>, after: Option<Vec<u8>>, has_more: bool });
quanta_index_contract_base::native_serde_struct!(NativeFilePathEntry {
    owner: OwnerKey,
    path: String,
    source: ObjectRef,
    bytes: ObjectRef,
    byte_len: u64
});
quanta_index_contract_base::native_serde_struct!(NativePathPage { view: ViewRef, entries: Vec<NativeFilePathEntry>, has_more: bool });
