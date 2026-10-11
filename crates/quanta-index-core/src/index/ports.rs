//! Small capability ports: physical data never owns the product head.
use super::{
    AdmissionOutcome, AttemptRef, BaseMember, CatalogView, DeltaInput, ImmutableOperation,
    IndexError, IngressRef, InstanceSpec, MapBatchRequest, MapBatchWrite, MapEdit, MapPage,
    MapRoot, MapWrite, MemberBinding, ObjectBatchInput, ObjectRef, OperationKey, OperationStatus,
    OrdinalMapEdit, OwnerInputBinding, OwnerKey, OwnerMemberBinding, PlanContext, PlanDecision,
    PreparedPublication, Refusal, RegisteredObject, SourceEvent, StoredObject, TicketId,
    TicketRecord, ViewRef, ViewSelection, WorkAction, WorkBudget, WorkClaim, WorkIntent,
    WorkStatus, WorkspaceKey,
};
use std::{future::Future, pin::Pin};

pub type IndexFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, IndexError>> + Send + 'a>>;

pub trait ImmutableObjectPort: Send + Sync {
    /// Computes the canonical immutable identity without storage access or
    /// requiring child objects to exist. Charges encoding work before allocation.
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError>;
    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError>;
    /// Durable allocation intent precedes installation; confirmation follows fsync.
    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError>;
    /// Bounded child-first durable group. Results retain input order; identical
    /// immutable wire objects may share one physical installation.
    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<Vec<RegisteredObject>, IndexError>;
    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError>;
}

/// A bounded producer of sorted, unique edits. Reaching EOF seals the complete
/// logical edit stream; a producer error must never become a successful prefix.
pub trait MapEditCursor {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError>;
}

pub trait PersistentMapPort: Send + Sync {
    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError>;
    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError>;
    /// Edits are unique and sorted. Preconditions are checked before any root is returned.
    fn apply(
        &self,
        base: MapRoot,
        edits: &[MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapWrite, IndexError>;
    /// Independent roots share one bounded COW staging and durable custody
    /// group. Preconditions are validated against each input base before IO.
    fn apply_batch(
        &self,
        requests: &[MapBatchRequest],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<MapBatchWrite, IndexError>;
    /// Checks every precondition against the original bases before durable
    /// installation. Only final roots are returned; scratch is not published.
    fn apply_stream(
        &self,
        bases: &[MapRoot],
        edits: &mut dyn MapEditCursor,
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<Vec<MapRoot>, IndexError>;
}

pub trait ObjectWriteSink: Send + Sync {
    fn reserve(&self, object: &RegisteredObject) -> Result<(), IndexError>;
    fn installed(&self, object: &RegisteredObject) -> Result<(), IndexError>;
    fn reserve_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError>;
    fn installed_batch(&self, objects: &[RegisteredObject]) -> Result<(), IndexError>;
}

pub trait ObjectCustodyPort: Send + Sync {
    fn reserve_objects(
        &self,
        attempt: AttemptRef,
        objects: &[RegisteredObject],
    ) -> Result<(), IndexError>;
    fn register_objects(
        &self,
        attempt: AttemptRef,
        objects: &[RegisteredObject],
    ) -> Result<(), IndexError>;
}

/// Catalog-owned custody before operation admission. Physical installation
/// follows reservation; only confirmed inputs may seal an immutable request.
pub trait InputStagingPort: Send + Sync {
    fn open_ingress(&self, operation: OperationKey) -> Result<IngressRef, IndexError>;
    fn reserve_inputs(
        &self,
        ingress: IngressRef,
        objects: &[RegisteredObject],
    ) -> Result<(), IndexError>;
    fn confirm_inputs(
        &self,
        ingress: IngressRef,
        objects: &[RegisteredObject],
    ) -> Result<(), IndexError>;
    fn seal_inputs(&self, ingress: IngressRef, request: ObjectRef) -> Result<(), IndexError>;
    /// Deletes stage metadata in bounded, budgeted groups. Interruption rolls
    /// back the whole discard, preserving its fence and custody for retry.
    fn discard_inputs(
        &self,
        ingress: IngressRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>;
}

pub struct AttemptContext<'a> {
    pub attempt: AttemptRef,
    pub objects: &'a dyn ImmutableObjectPort,
    pub maps: &'a dyn PersistentMapPort,
    pub custody: &'a dyn ObjectCustodyPort,
    pub budget: &'a mut WorkBudget,
}

pub trait OperationJournalPort: Send + Sync {
    fn admit(
        &self,
        operation: ImmutableOperation,
        ingress: IngressRef,
        events: &[SourceEvent],
        budget: &mut WorkBudget,
    ) -> Result<AdmissionOutcome, IndexError>;
    fn inspect(&self, key: OperationKey) -> Result<OperationStatus, IndexError>;
    fn operation(&self, key: OperationKey) -> Result<ImmutableOperation, IndexError>;
    fn claim_recovery(&self, key: OperationKey) -> Result<AttemptRef, IndexError>;
    fn refuse(&self, attempt: AttemptRef, reason: Refusal) -> Result<OperationStatus, IndexError>;
}

pub trait CatalogPublicationPort: Send + Sync {
    fn create_workspace(
        &self,
        genesis: CatalogView,
        objects: &[RegisteredObject],
    ) -> Result<ViewRef, IndexError>;
    /// Sole publication linearization point: CAS, fence, roots, work and original receipt.
    fn commit_transition(
        &self,
        publication: PreparedPublication,
        actions: &mut dyn WorkActionCursor,
        budget: &mut WorkBudget,
    ) -> Result<OperationStatus, IndexError>;
}

/// Sorted retire/create actions from one sealed immutable manifest. Producer
/// failure or invalid EOF rolls the complete catalog transaction back.
pub trait WorkActionCursor {
    fn next_action(&mut self, budget: &mut WorkBudget) -> Result<Option<WorkAction>, IndexError>;
}

pub trait CatalogReadPort: Send + Sync {
    fn acquire_ticket(&self, selection: ViewSelection) -> Result<TicketRecord, IndexError>;
    fn release_ticket(&self, id: TicketId) -> Result<(), IndexError>;
}

/// Durable owner jobs share catalog publication and custody authority.
///
/// Claims use catalog time, and retained prepared artifacts avoid repeating expensive
/// derivation after an unrelated view CAS conflict or lost ACK.
pub trait WorkClaimPort: Send + Sync {
    fn ready_work(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<Vec<WorkIntent>, IndexError>;
    fn claim_work(
        &self,
        intent: &WorkIntent,
        lease: std::time::Duration,
    ) -> Result<WorkClaim, IndexError>;
    /// Renewal keeps the fence stable. Validation uses the catalog's current
    /// lease, so a previously admitted immutable request remains executable.
    fn renew_work(
        &self,
        claim: &WorkClaim,
        lease: std::time::Duration,
    ) -> Result<WorkClaim, IndexError>;
    fn work_status(&self, intent: &WorkIntent) -> Result<WorkStatus, IndexError>;
    fn prepared_work(&self, claim: &WorkClaim) -> Result<Option<OwnerMemberBinding>, IndexError>;
    fn stage_work(&self, claim: &WorkClaim, binding: OwnerMemberBinding) -> Result<(), IndexError>;
}

pub trait IndexOwnerBuildPort: Send + Sync {
    type Candidate: Send;
    fn prepare_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate>;
    fn verify_owner<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, OwnerMemberBinding>;
}

pub trait IndexBuildPort: Send + Sync {
    type Delta: Send;
    type Plan: Send;
    type Candidate: Send;
    fn plan(
        &self,
        base: BaseMember,
        input: DeltaInput<Self::Delta>,
        context: &PlanContext,
        budget: &mut WorkBudget,
    ) -> Result<PlanDecision<Self::Plan>, IndexError>;
    fn prepare<'a>(
        &'a self,
        plan: Self::Plan,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate>;
    fn verify_and_seal<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding>;
}

pub trait IndexReadPort: Send + Sync {
    type Reader;
    fn open_exact(
        &self,
        member: &MemberBinding,
        ticket: &TicketRecord,
        budget: &mut WorkBudget,
    ) -> Result<Self::Reader, IndexError>;
}
