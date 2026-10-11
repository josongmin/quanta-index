use quanta_index_contract::index::{
    AttemptRef, CatalogView, InputBinding, InstanceSpec, MapRoot, MemberBinding, ObjectRef,
    OperationStatus, OwnerKey, OwnerMutationSet, SourceCut, SourceEvent, SourceRecord, TicketId,
    ViewRef, WorkClaim,
};

/// Canonical key for owner-scoped source, coverage and missing maps. Instance
/// selection belongs to the enclosing root, not an ambiguous key prefix.
#[must_use]
pub fn owner_map_key(owner: OwnerKey) -> Vec<u8> {
    let mut key = b"owner\0".to_vec();
    key.extend_from_slice(&owner.producer.0);
    key.extend_from_slice(&owner.stable_id.0);
    key
}

/// A source owner's complete old/new contribution, decoded under the attempt budget.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceChange {
    pub owner: OwnerKey,
    pub prior: Option<(ObjectRef, SourceRecord)>,
    pub next: Option<(ObjectRef, SourceRecord)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDelta {
    pub prior: SourceCut,
    pub next: SourceCut,
    pub changed: OwnerMutationSet,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisteredObject {
    pub reference: ObjectRef,
    pub bytes: u64,
    pub children: Vec<ObjectRef>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredObject {
    pub registration: RegisteredObject,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapEdit {
    pub key: Vec<u8>,
    pub expected: Option<ObjectRef>,
    pub next: Option<ObjectRef>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapWrite {
    pub root: MapRoot,
}

/// An edit in one logical multi-root transaction. Ordinal and key ordering
/// spans the complete cursor, including producer page boundaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrdinalMapEdit {
    pub root: u32,
    pub edit: MapEdit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapBatchRequest {
    pub base: MapRoot,
    pub edits: Vec<MapEdit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapBatchWrite {
    /// One output root per input request, in the same order.
    pub roots: Vec<MapRoot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectBatchInput {
    pub payload: Vec<u8>,
    pub children: Vec<ObjectRef>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapPage {
    pub entries: Vec<(Vec<u8>, ObjectRef)>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdmissionOutcome {
    New(AttemptRef),
    Existing(OperationStatus),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedPublication {
    pub attempt: AttemptRef,
    pub expected: ViewRef,
    pub next: CatalogView,
    pub work_actions: super::WorkActionSet,
    pub complete_work: Option<WorkClaim>,
    pub source_events: Vec<SourceEvent>,
    /// Projection outbox events share stream authority but never advance source.
    pub projection_events: Vec<SourceEvent>,
    pub expected_source: Option<SourceCut>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TicketRecord {
    pub id: TicketId,
    pub view: CatalogView,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanContext {
    pub instance: InstanceSpec,
    pub source: SourceCut,
    pub input: InputBinding,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BaseMember {
    Absent,
    Present(Box<MemberBinding>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeltaInput<D> {
    Bootstrap,
    Patch(D),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanDecision<P> {
    Reuse(Box<MemberBinding>),
    Incremental(P),
    RebuildRequired,
    MaintenanceRequired,
}

quanta_index_contract::native_serde_struct!(RegisteredObject { reference: ObjectRef, bytes: u64, children: Vec<ObjectRef> });
