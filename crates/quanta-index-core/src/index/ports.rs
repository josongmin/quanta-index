//! Small capability ports: physical data never owns the product head.
use super::error::ExistingFileReadFailureV1;
pub use super::install_ports::{MAX_OBJECT_INSTALL_BATCH, ObjectWriteSink, PreparedObjectMapBatch};
use super::{
    AdmissionOutcome, AttemptRef, BaseMember, BootstrapRef, CatalogView, DeltaInput, Digest,
    HistoryAppendV1, HistoryGenerationAdvanceV1, HistoryTransition, ImmutableOperation, IndexError,
    IngressRef, InstanceSpec, MapBatchRequest, MapBatchWrite, MapEdit, MapPage, MapRoot, MapWrite,
    MemberBinding, ObjectBatchInput, ObjectInstallScope, ObjectRef, OperationKey, OperationStatus,
    OrdinalMapEdit, OwnedReadyOutcome, OwnedWorkAssignment, OwnedWorkBegin, OwnedWorkGrant,
    OwnedWorkPolicy, OwnedWorkReport, OwnerInputBinding, OwnerKey, OwnerMemberBinding,
    OwnerMutationSet, PlanContext, PlanDecision, PreparedPublication, ProjectionBinding,
    ReclaimCandidate, ReclaimDrainPage, ReclaimPermit, ReclaimTarget, ReclaimWork, ReclaimWorkPage,
    Refusal, RegisteredObject, RetiringRef, SourceEvent, StoredObject,
    TicketRecord, ViewRef, ViewSelection, WorkAction, WorkBudget, WorkClaim, WorkIntent, WorkStatus,
    WorkspaceKey,
};
use quanta_index_contract::native_protocol::SourceStageReplayReceiptV1;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

pub type IndexFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, IndexError>> + Send + 'a>>;

/// Selected filesystem adapter's actual read FD and its physical lifetime.
///
/// A borrowed FD must not be extracted or duplicated beyond this owner.
/// Dropping the owner closes the actual FD before retiring capacity custody;
/// an uncertain close must retain custody or quarantine the physical authority.
/// Immutable namespace ownership may share this guard across store callers;
/// sharing it does not grant independent stream positions on the same FD.
pub trait ObjectReadFile: Send + Sync {
    fn file(&self) -> Result<&std::fs::File, IndexError>;
}

/// File-store I/O facet; it grants no Catalog read or reclaim authority.
///
/// Admission precedes open. The returned owner binds the actual opened inode
/// and retains the same capacity authority through close. Failure cannot fall
/// back to an unqualified filesystem open. Other physical stores need not use
/// this filesystem-specific facet to implement `ImmutableObjectPort`.
///
/// Reserve the returned owner's allocation from the supplied budget before
/// allocation and retain that charge through the owner's physical close.
pub trait ObjectReadIoPort: std::fmt::Debug + Send + Sync {
    /// Tracked temporary owner backing for at most one existing-object read.
    fn read_owner_memory_bound(&self) -> Result<Option<super::MemoryCreditLimits>, IndexError> {
        Ok(None)
    }

    fn open(
        &self,
        path: &std::path::Path,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectReadFile>, IndexError>;
}

/// Physical capacity for one file-store output; Catalog remains its logical
/// installation authority. Reserve the complete bound before `O_EXCL` create.
pub trait ObjectWriteIoPort: std::fmt::Debug + Send + Sync {
    /// Tracked ticket backing for at most `maximum_outputs` simultaneous writes.
    fn write_ticket_memory_bound(
        &self,
        _maximum_outputs: usize,
    ) -> Result<Option<super::MemoryCreditLimits>, IndexError> {
        Ok(None)
    }

    /// Reconcile only an orphaned, physically retired safe create at this
    /// canonical output path. A live writer, absent output or different inode
    /// grants no recovery. The original physical authority must reauthenticate
    /// the real FD, sync the file/parent and retire the FD before settlement.
    /// Normal immutable reads must never acquire this mutation capability.
    /// A settled path is a no-op; Catalog confirmation remains the caller's job.
    fn recover_retired_visible_create(
        &self,
        target: &std::path::Path,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>;

    fn begin(
        &self,
        target: &std::path::Path,
        temporary: &std::path::Path,
        max_bytes: u64,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ObjectWriteTicket>, IndexError>;
}

/// The directory identities obtained from the exact descriptors whose sync
/// succeeded. The storage adapter must obtain these before closing those FDs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectWriteSyncDirectories {
    pub objects: crate::publication_capacity::InodeKey,
    pub shard: crate::publication_capacity::InodeKey,
}

/// Retains the original output and its independent FD pin through publication.
///
/// No method grants overwrite, Catalog confirmation or logical reclaim.
/// A started ticket cannot refund merely on `Drop`. Failed paths retain custody
/// until proven cleanup or the physical owner's explicit recovery/quarantine.
pub trait ObjectWriteTicket: Send {
    fn creation_started(&mut self) -> Result<(), IndexError>;
    /// Open the declared temporary through the ticket's held namespace.
    /// `None` selects the store's existing create path. A successful open
    /// transfers the sole original writer without a fallible post-open step.
    fn open_declared_temp(&mut self) -> Result<Option<std::fs::File>, IndexError> {
        Ok(None)
    }
    fn created(&mut self, writer: &std::fs::File) -> Result<(), IndexError>;
    /// Publish through the held namespace using atomic no-replace rename.
    /// `Some(true)` consumed the temporary name. `Some(false)` left it in
    /// place after a destination collision. An error must leave it unrenamed.
    /// Tickets that own temporary creation also own this rename.
    fn publish_declared_temp(
        &mut self,
        _writer: &std::fs::File,
        _final_path: &std::path::Path,
    ) -> Result<Option<bool>, IndexError> {
        Ok(None)
    }
    fn uncreated(&mut self) -> Result<(), IndexError>;
    /// Consume and physically close the sole original writer. The independent
    /// pin remains owned until the shared batch sync and settlement complete.
    fn retire_writer(&mut self, writer: std::fs::File) -> Result<(), IndexError>;
    /// Unlink only this declared temporary, sync its parent and close both
    /// original/pin FDs before refund. Never unlink an installed canonical name.
    fn cancel(&mut self, writer: std::fs::File) -> Result<(), IndexError>;
    /// Called only after the store's file/shard/shared barrier protocol and
    /// original writer close. Authenticate and settle without issuing another
    /// file/parent fsync, then physically retire the independent pin.
    fn synced(self: Box<Self>, directories: ObjectWriteSyncDirectories) -> Result<(), IndexError>;
}

/// Upper bounds for one batch's physical inputs, independent of object identity.
/// The caller must enforce these bounds on the actual packet before effects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_field_names,
    reason = "Callers must distinguish these maxima from actual packet counts."
)]
pub struct ObjectInstallMemoryShape {
    pub maximum_inputs: usize,
    pub maximum_total_payload_bytes: usize,
    pub maximum_payload_bytes: usize,
    pub maximum_total_children: usize,
}

impl ObjectInstallMemoryShape {
    pub fn validate(self) -> Result<(), IndexError> {
        if self.maximum_inputs == 0
            || self.maximum_inputs > MAX_OBJECT_INSTALL_BATCH
            || self.maximum_payload_bytes > self.maximum_total_payload_bytes
        {
            return Err(IndexError::InvalidInput(
                "object install memory shape is invalid".into(),
            ));
        }
        Ok(())
    }
}

pub trait ImmutableObjectPort: Send + Sync {
    /// Effect-free tracked allocation upper bound for `install_batch`.
    /// Unknown backends decline rather than inventing a memory allowance.
    fn install_batch_memory_bound(
        &self,
        _shape: ObjectInstallMemoryShape,
    ) -> Result<Option<super::MemoryCreditLimits>, IndexError> {
        Ok(None)
    }

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
    /// At most `MAX_OBJECT_INSTALL_BATCH` child-first objects. Results retain input order; identical
    /// immutable wire objects may share one physical installation. Returned slots
    /// and registration children retain data-only allocation custody after
    /// execution ends; consuming iteration must preserve the slot reservation.
    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<crate::index::RegisteredObjectBatch, IndexError>;
    /// Effect-free optional admission for one at-most-256-ref union of at
    /// most two childless packets. The returned handle retains original wire,
    /// registration, shard-path and output-slot grants. None declines before
    /// any persistent effect and does not record a terminal memory denial.
    /// Each packet retains `packet_bytes`; `maximum_payload` is a staging bound.
    fn try_prepare_leaf_install_run(
        &self,
        _attempt: AttemptRef,
        _maximum_payload: usize,
        _packet_bytes: usize,
        budget: &mut WorkBudget,
    ) -> Result<Option<Box<dyn super::object_install::PreparedObjectInstallRun + '_>>, IndexError>
    {
        budget.checkpoint()?;
        Ok(None)
    }
    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError>;

    /// Read one authenticated, childless UTF-8 leaf without retaining its body
    /// beyond the visitor. An override must call the visitor exactly once on
    /// success and preserve the original request's work and cancellation checks.
    fn visit_utf8_leaf(
        &self,
        reference: ObjectRef,
        byte_len: u64,
        budget: &mut WorkBudget,
        visit: &mut dyn FnMut(&str, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        visit_utf8_leaf(self, reference, byte_len, budget, visit)
    }

    /// Consume DATA previously returned by this port in the same protected read
    /// attempt. The caller retains that DATA's original allocation custody.
    /// This is not a fresh authentication or a publication/reclaim permission.
    /// Adapters own the logical charge and cancellation policy for reuse; an
    /// adapter without such a policy performs its original read instead.
    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        self.get(object.registration.reference, budget)
    }
}

/// Default UTF-8 leaf verification for independent object ports and attempts.
/// A source attempt may override the port method to record a same-attempt proof.
pub fn visit_utf8_leaf(
    objects: &(impl ImmutableObjectPort + ?Sized),
    reference: ObjectRef,
    byte_len: u64,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(&str, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    budget.checkpoint()?;
    let stored = objects.get(reference, budget)?;
    budget.checkpoint()?;
    if stored.registration.reference != reference
        || !stored.registration.children.is_empty()
        || u64::try_from(stored.payload.len()).map_err(|_error| IndexError::ResourceLimited)?
            != byte_len
    {
        return Err(IndexError::CorruptObject(
            "UTF-8 leaf body identity, edges or length mismatch".into(),
        ));
    }
    let text = std::str::from_utf8(&stored.payload).map_err(|_error| {
        IndexError::InvalidInput("source text admission requires UTF-8".into())
    })?;
    visit(text, budget)
}

/// One fixed physical layout decision for every root in a bounded edit stream.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RootPackingMode {
    #[default]
    None,
    Terminal,
    Nested,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootPackingPlan {
    modes: [RootPackingMode; 256],
}

impl Default for RootPackingPlan {
    fn default() -> Self {
        Self {
            modes: [RootPackingMode::None; 256],
        }
    }
}

impl RootPackingPlan {
    pub fn select(&mut self, ordinal: u32, mode: RootPackingMode) -> Result<(), IndexError> {
        let index = usize::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?;
        let slot = self.modes.get_mut(index).ok_or_else(|| {
            IndexError::InvalidInput("packed map root ordinal is out of bounds".into())
        })?;
        *slot = mode;
        Ok(())
    }

    #[must_use]
    pub fn mode(&self, ordinal: usize) -> RootPackingMode {
        self.modes.get(ordinal).copied().unwrap_or_default()
    }

    pub fn validate_for_roots(self, roots: usize) -> Result<(), IndexError> {
        let unused_modes = self.modes.get(roots..).ok_or_else(|| {
            IndexError::InvalidInput("packed map root ordinal is out of bounds".into())
        })?;
        if unused_modes
            .iter()
            .any(|mode| *mode != RootPackingMode::None)
        {
            return Err(IndexError::InvalidInput(
                "packed map root ordinal is out of bounds".into(),
            ));
        }
        Ok(())
    }
}

/// A bounded producer of sorted, unique edits. Reaching EOF seals the complete
/// logical edit stream; a producer error must never become a successful prefix.
pub trait MapEditCursor {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError>;

    /// The consumer validates all selected ordinals before reading any edits.
    /// Unaware producers retain the ordinary map representation.
    fn subtree_packing(&self) -> Result<RootPackingPlan, IndexError> {
        Ok(RootPackingPlan::default())
    }
}

/// One bounded, exclusive-keyset page of differences between immutable roots.
/// Prefix and continuation keys use the same byte ordering as `scan`.
#[derive(Clone, Copy, Debug)]
pub struct MapDiffRange<'a> {
    pub prefix: &'a [u8],
    pub after: Option<&'a [u8]>,
    pub limit: u32,
}

/// Receives borrowed changed keys in strictly increasing order.
///
/// Values are the exact before/after values; equal values are never visited. The visitor
/// owns admission for any retained copies. A visited prefix is provisional
/// until `visit_diff` succeeds; an error must not become a published prefix.
pub trait MapDiffVisitor {
    fn changed(
        &mut self,
        key: &[u8],
        expected: Option<super::MapValue>,
        next: Option<super::MapValue>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>;
}

pub const MAX_MAP_LOOKUP_BATCH: usize = 64;

pub const MAX_OBJECT_MAP_BATCH_OBJECTS: usize = 2;
pub const MAX_OBJECT_MAP_BATCH_ROOTS: usize = 2;
pub const MAX_OBJECT_MAP_BATCH_KEY_BYTES: usize = 64;
pub const MAX_OWNER_MAP_BATCH_OBJECTS: usize = 4;
pub const MAX_OWNER_MAP_BATCH_ROOTS: usize = 4;

/// One explicitly linked object/map backend's bounded installation facet.
///
/// Each root has one scalar edit, checked against its original base before
/// installation. Object DATA and dependent COW nodes share the original
/// child-first installer and sink; no intermediate root is published. Returned
/// object references preserve input order and roots preserve base order.
///
/// Composition must supply this facet from the same backend as the read ports;
/// neither pointer comparison nor an identity hint grants that relationship.
pub trait ObjectMapBatchPort: Send + Sync {
    /// Prepare the materialized owner's two metadata objects and two COW roots
    /// on this same explicitly linked backend. Unsupported backends keep their
    /// original installation path; an error must never select another backend.
    /// The caller retains its attempt/pin and original funding for the scope.
    fn prepare_owner_maps(
        self: Arc<Self>,
        _objects: &[ObjectBatchInput],
        _bases: &[MapRoot],
        _edits: &[&MapEdit],
        _attempt: AttemptRef,
        _budget: &mut WorkBudget,
    ) -> Result<Option<Arc<dyn PreparedOwnerMapScope>>, IndexError> {
        Ok(None)
    }

    fn install_objects_and_maps(
        &self,
        objects: &[ObjectBatchInput],
        bases: &[MapRoot],
        edits: &[&MapEdit],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<(super::StagedObjectReferences, super::StagedMapRoots), IndexError>;

    /// Prepare one checked original-base map edit without installing objects.
    /// The returned attempt-bound token retains its COW staging and custody.
    fn prepare_one_map(
        self: Arc<Self>,
        base: MapRoot,
        edit: &MapEdit,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn PreparedObjectMapBatch>, IndexError>;
}

/// A single attempt's canonical resident stage, not publication authority.
///
/// Its read facets authenticate prepared wire through the original decoder.
/// A real write first flushes owner metadata through the original sink/barriers;
/// after that, all facets use the same installed backend. Without an early
/// write, a prepared delta and coverage can join at most four DATA/four roots.
pub trait PreparedOwnerMapScope: Send + Sync {
    fn references(&self) -> &[ObjectRef];
    fn roots(&self) -> &[MapRoot];
    fn objects(&self) -> &dyn ImmutableObjectPort;
    fn maps(&self) -> &dyn PersistentMapPort;
    fn writer(self: Arc<Self>) -> Arc<dyn ObjectMapBatchPort>;
    /// Transfer pending metadata only when `prepare_one_map` has not already
    /// transferred this stage into the searchable's prepared token. A flushed
    /// stage reports Installed. Unconsumed DATA is dropped without I/O.
    fn take_prepared(
        &self,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<OwnerMapCompletion, IndexError>;
}

/// Distinguish an installed owner prefix from one transferred into Semantic's
/// token. The consumer must require the full prefix whenever it was not flushed.
pub enum OwnerMapCompletion {
    Prepared(Box<dyn PreparedObjectMapBatch>),
    Transferred,
    Installed,
}

pub trait PersistentMapPort: Send + Sync {
    /// Optional Original credit for scanning a freshly built canonical map
    /// with at most `max_entries` values. The bound covers the backend's
    /// simultaneous scan and read allocations; unknown backends return None.
    fn cold_scan_memory_credit_limits(
        &self,
        _max_entries: usize,
    ) -> Result<Option<super::MemoryCreditLimits>, IndexError> {
        Ok(None)
    }

    /// Optional DATA reuse inside one protected, funded read call. The caller
    /// keeps its original view pin and memory reservation until this reader is
    /// dropped. Reuse must retain the original object allocation custody and
    /// perform the original logical read/decode charges and cancellation checks;
    /// it grants no authentication, publication or reclaim authority. A backend
    /// without this policy continues through its original reader.
    fn scoped_reads<'a>(
        &'a self,
        _budget: &WorkBudget,
    ) -> Result<Option<Box<dyn PersistentMapPort + 'a>>, IndexError> {
        Ok(None)
    }

    fn lookup(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<crate::index::MapValue>, IndexError>;
    /// Read at most64 keys against one retained immutable root, preserving
    /// input order, duplicates and absence. The caller admits output backing
    /// before allocation and discards the entire output on error; a partial
    /// prefix is never success. The scalar implementation is semantically exact.
    fn lookup_batch(
        &self,
        root: MapRoot,
        keys: &[&[u8]],
        values: &mut [Option<crate::index::MapValue>],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if keys.len() > MAX_MAP_LOOKUP_BATCH || keys.len() != values.len() {
            return Err(IndexError::InvalidInput(
                "map lookup batch shape exceeds bound".into(),
            ));
        }
        budget.charge(super::WorkUsage {
            bytes: u64::try_from(
                keys.len()
                    .checked_mul(std::mem::size_of::<Option<super::MapValue>>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            ..super::WorkUsage::default()
        })?;
        for (key, value) in keys.iter().zip(values) {
            *value = self.lookup(root, key, budget)?;
        }
        Ok(())
    }
    /// Object-domain projection of the one value contract; inline data is an error.
    fn lookup_object(
        &self,
        root: MapRoot,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<ObjectRef>, IndexError> {
        self.lookup(root, key, budget)?
            .map(crate::index::MapValue::object_ref)
            .transpose()
    }
    /// Returns a complete keyset page in strictly increasing key order, with
    /// at most `limit` entries and every key strictly greater than `after`.
    /// `has_more` is true exactly when another entry exists in this immutable
    /// root; an empty page cannot be a continuation. Errors never seal a prefix.
    /// Backends charge bounded page allocation and reads to the supplied budget.
    /// Reserve returned entry/key capacity from the actual `WorkBudget` scope
    /// before allocating. Page clones, cursors and continuation positions share
    /// immutable data and retain its charge after execution ends. Copying keys
    /// for a new producer/result owner requires that owner's separate admission.
    fn scan(
        &self,
        root: MapRoot,
        after: Option<&[u8]>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError>;
    /// Return the greatest key in an immutable map, or an empty page. Backends
    /// must follow only the rightmost path; a whole-map scan is not permitted.
    /// The returned one-row page retains the same funded key/value custody as
    /// `scan`. Unsupported backends fail instead of silently scanning.
    fn last_entry(&self, _root: MapRoot, _budget: &mut WorkBudget) -> Result<MapPage, IndexError> {
        Err(IndexError::BackendViolation(
            "map backend has no bounded last-entry seek".into(),
        ))
    }
    /// Visit one-row scan pages through EOF under the same immutable-root
    /// authority and Work budget as `scan`. The visitor may stage disposable
    /// effects. It must not transfer the root's authority during the visit.
    /// `false` selects the scalar path before any reads, charges or callbacks.
    /// Once a page has been delivered, an error aborts the visit without replay.
    fn try_visit_scan_pages(
        &self,
        _root: MapRoot,
        budget: &mut WorkBudget,
        _visitor: &mut dyn FnMut(MapPage, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        Ok(false)
    }
    /// Structural diff, without a whole-map scan or materialized key union.
    /// Equal immutable subtree references are skipped without reads, so this
    /// does not revalidate unchanged objects. Callers retain both roots through
    /// their existing read authority. Returns true exactly when a further
    /// changed key exists in the range, beyond the visited page. Limits and key
    /// bounds match `scan`; failures propagate even after visitor callbacks.
    /// Backends without structural diff must refuse, never silently full-scan.
    fn visit_diff(
        &self,
        _before: MapRoot,
        _after: MapRoot,
        _range: MapDiffRange<'_>,
        _visitor: &mut dyn MapDiffVisitor,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        budget.checkpoint()?;
        Err(IndexError::BackendViolation(
            "map backend does not support structural diff".into(),
        ))
    }
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
    ) -> Result<crate::index::StagedMapRoots, IndexError>;
}

/// Catalog authority paired with one managed object namespace at service
/// construction.
///
/// The managed store calls this port directly with the original
/// budget; an arbitrary sink implementation cannot authorize installation.
/// Every reservation/confirmation validates the frozen scope, exact object
/// registration, child list and active fence, including bootstrap custody.
pub trait ManagedObjectInstallPort: Send + Sync {
    /// Tracked Catalog allocation upper bound for reserve/confirm/advance of
    /// one bounded object packet. It grants no logical custody authority.
    fn managed_install_memory_bound(
        &self,
        _shape: ObjectInstallMemoryShape,
    ) -> Result<Option<super::MemoryCreditLimits>, IndexError> {
        Ok(None)
    }

    fn reserve_managed(
        &self,
        scope: ObjectInstallScope,
        objects: &[RegisteredObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>;
    fn confirm_managed(
        &self,
        scope: ObjectInstallScope,
        objects: &[RegisteredObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>;

    /// Opt in only when advancing both groups is one durable transaction.
    fn supports_atomic_attempt_advance(&self) -> bool {
        false
    }

    /// Atomically confirm a synced group and reserve its child-first successor.
    /// The successor must not be written until this call returns successfully.
    fn advance_attempt_managed(
        &self,
        _attempt: AttemptRef,
        _confirmed: &[RegisteredObject],
        _next: &[RegisteredObject],
        _budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        Err(IndexError::BackendViolation(
            "managed authority does not support atomic attempt advance".into(),
        ))
    }
}

/// One durable authority for bounded candidate, Reclaiming and Draining
/// phases.
///
/// A page lookup ends its SQL transaction before the managed store
/// acquires the object gate. Registered and provisional edges both count.
pub trait CatalogReclaimPort: Send + Sync {
    fn reclaim_work_page(
        &self,
        after: Option<ObjectRef>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ReclaimWorkPage, IndexError>;
    /// Recheck all roots, pins, work, bootstrap, ingress, attempt, provisional
    /// and registered incoming edges. New adoption may cancel only Retiring.
    fn retire_candidate(
        &self,
        candidate: ReclaimCandidate,
        budget: &mut WorkBudget,
    ) -> Result<Option<RetiringRef>, IndexError>;
    /// Called under the store's object gate. Persist this frozen generation,
    /// namespace and physical incarnation before unlink.
    fn enter_reclaiming(
        &self,
        retiring: RetiringRef,
        target: ReclaimTarget,
        budget: &mut WorkBudget,
    ) -> Result<ReclaimPermit, IndexError>;
    fn validate_reclaiming(
        &self,
        permit: ReclaimPermit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>;
    /// Paired store calls this only after it verifies absence and directory
    /// sync. Persists Unlinked/Draining; new reserve still fails.
    fn mark_unlinked(
        &self,
        permit: ReclaimPermit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>;
    /// Atomically remove a sorted prefix of at most 32 remaining direct edges,
    /// enqueue newly unreferenced children and persist that prefix. The rows
    /// themselves are the cursor. An empty remainder advances to Deleted and
    /// permits a later installation under a new generation.
    fn drain_unlinked(
        &self,
        permit: ReclaimPermit,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ReclaimDrainPage, IndexError>;
}

/// One immutable pairing for a managed store. The catalog implements both
/// custody and reclaim phases; callers cannot swap a per-operation verifier.
pub trait ManagedObjectAuthority: ManagedObjectInstallPort + CatalogReclaimPort {
    /// Durable one-time pairing with the held managed namespace. A different
    /// root, objects directory or lease inode refuses before any installation.
    /// This startup transaction is bounded and has no physical IO under SQL.
    fn bind_namespace(&self, binding: super::ObjectNamespaceBinding) -> Result<(), IndexError>;
}

pub trait ManagedObjectReclaimPort: Send + Sync {
    /// A managed store owns the complete physical mutation scope. It derives
    /// the gate from `work.object()` and checks the paired catalog phase; no
    /// caller-supplied gate, verifier or child cursor is accepted.
    /// `None` means Retiring was invalidated by a concurrent legitimate root.
    /// `Some(page)` reports one committed prefix; `has_more` requests another
    /// caller-budgeted step. No fresh internal deadline or budget is allowed.
    fn reclaim_one(
        &self,
        work: ReclaimWork,
        child_limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<Option<ReclaimDrainPage>, IndexError>;
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
    /// Open a writable submission fence or return the existing sealed ingress.
    /// A sealed ingress only permits admission of its exact request, so the
    /// caller must verify it and skip input installation.
    fn open_submission_ingress(
        &self,
        operation: OperationKey,
        budget: &WorkBudget,
    ) -> Result<(IngressRef, Option<ObjectRef>), IndexError>;
    /// Adopts exact confirmed registrations under the original request budget.
    /// An empty batch still validates the open ingress fence.
    fn adopt_registered_inputs(
        &self,
        ingress: IngressRef,
        objects: &[RegisteredObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>;
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
    /// Record the checked mutation set for the still-current open ingress.
    /// The Catalog must recheck its fence and custody before the write commits.
    fn persist_source_stage_replay_receipt_v1(
        &self,
        receipt: SourceStageReplayReceiptV1,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>;
    /// Recover a sealed stage only while its original ingress remains current.
    fn read_source_stage_replay_receipt_v1(
        &self,
        operation: OperationKey,
        correlation: Digest,
        budget: &mut WorkBudget,
    ) -> Result<Option<SourceStageReplayReceiptV1>, IndexError>;
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
    pub scratch: &'a dyn super::ScratchSpacePort,
    pub budget: &'a mut WorkBudget,
}

impl AttemptContext<'_> {
    /// Reborrow the attempt while a verified UTF-8 leaf is live. The object
    /// port owns the body and the callback cannot extend its lifetime.
    pub fn with_utf8_leaf<T>(
        &mut self,
        reference: ObjectRef,
        byte_len: u64,
        visit: impl FnOnce(&str, &mut AttemptContext<'_>) -> Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        let objects = self.objects;
        let maps = self.maps;
        let custody = self.custody;
        let scratch = self.scratch;
        let attempt = self.attempt;
        let mut visitor = Some(visit);
        let mut output = None;
        let mut repeated = false;
        let result =
            objects.visit_utf8_leaf(reference, byte_len, self.budget, &mut |text, budget| {
                let Some(visit) = visitor.take() else {
                    repeated = true;
                    return Err(IndexError::BackendViolation(
                        "UTF-8 leaf visitor was repeated".into(),
                    ));
                };
                let mut scoped = AttemptContext {
                    attempt,
                    objects,
                    maps,
                    custody,
                    scratch,
                    budget,
                };
                output = Some(visit(text, &mut scoped)?);
                Ok(())
            });
        if repeated {
            return Err(IndexError::BackendViolation(
                "UTF-8 leaf visitor was repeated".into(),
            ));
        }
        result?;
        output
            .ok_or_else(|| IndexError::BackendViolation("UTF-8 leaf visitor was not called".into()))
    }
}

/// Selects request-bounded observation or mandatory post-failure reconciliation.
///
/// Reconciliation must remain available after the original request expires so
/// a durable terminal result cannot be replaced with a cancellation claim.
pub enum OperationInspectAccess<'a> {
    Request(&'a WorkBudget),
    PostFailureReconcile,
}

pub trait OperationJournalPort: Send + Sync {
    fn admit(
        &self,
        operation: ImmutableOperation,
        ingress: IngressRef,
        events: &[SourceEvent],
        budget: &mut WorkBudget,
    ) -> Result<AdmissionOutcome, IndexError>;
    /// Reserved owned admission checks the exact frozen request, binds its
    /// confirmed physical root through the canonical pure identity capability,
    /// and seals the ingress in the same admission transaction. A previously
    /// sealed root must match exactly. Failure commits neither seal nor admission.
    /// Generic admission still requires an explicitly sealed ingress and must
    /// reject the reserved producer namespace.
    fn admit_owned(
        &self,
        operation: ImmutableOperation,
        ingress: IngressRef,
        request: &super::UpdateRequest,
        identity: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<AdmissionOutcome, IndexError>;
    fn inspect(
        &self,
        key: OperationKey,
        access: OperationInspectAccess<'_>,
    ) -> Result<OperationStatus, IndexError>;
    fn admit_projection(
        &self,
        operation: ImmutableOperation,
        ingress: IngressRef,
        events: &[SourceEvent],
        _replay: quanta_index_contract::index::ProjectionReplayStamp,
        budget: &mut WorkBudget,
    ) -> Result<AdmissionOutcome, IndexError> {
        self.admit(operation, ingress, events, budget)
    }
    fn projection_replay(
        &self,
        key: OperationKey,
        budget: &mut WorkBudget,
    ) -> Result<(OperationStatus, Option<quanta_index_contract::index::ProjectionReplayStamp>), IndexError> {
        Ok((self.inspect(key, OperationInspectAccess::Request(budget))?, None))
    }
    /// Exact Published operation metadata. This is not a Catalog read pin and
    /// must not substitute the current head or read immutable object bodies.
    /// Backends without an exact durable metadata reader refuse explicitly.
    fn published_view_metadata(
        &self,
        _key: OperationKey,
        budget: &mut WorkBudget,
    ) -> Result<CatalogView, IndexError> {
        budget.checkpoint()?;
        Err(IndexError::SnapshotUnavailable)
    }
    fn operation(&self, key: OperationKey) -> Result<ImmutableOperation, IndexError>;
    /// Exact immutable operation lookup for an active request. The original
    /// request budget bounds Catalog lock acquisition and the returned result;
    /// this read does not add work-usage charges to the legacy lookup.
    fn operation_request(
        &self,
        key: OperationKey,
        budget: &WorkBudget,
    ) -> Result<ImmutableOperation, IndexError>;
    /// Rotate a pending attempt under the original recovery budget. A committed
    /// fence is returned even if the request expires during commit.
    fn claim_recovery(
        &self,
        key: OperationKey,
        budget: &WorkBudget,
    ) -> Result<AttemptRef, IndexError>;
    fn refuse(&self, attempt: AttemptRef, reason: Refusal) -> Result<OperationStatus, IndexError>;
}

/// Producer-wide source epoch bound to the exact current catalog view.
/// Resync admission does not change it; publication switches it atomically.
pub trait SourceAuthorityPort: Send + Sync {
    fn source_epoch(
        &self,
        view: ViewRef,
        producer: super::ProducerId,
        budget: &mut WorkBudget,
    ) -> Result<Option<u64>, IndexError>;
}

pub trait CatalogPublicationPort: Send + Sync {
    /// Sole publication linearization point: CAS, fence, roots, work and original receipt.
    ///
    /// Consumes prepared event buffers synchronously; implementations must not
    /// retain those buffers after returning under the caller's temporary lease.
    /// The attempt-fenced producer seals the frozen work stream before this call.
    /// Its complete count/digest EOF and dependency custody are verified within
    /// the transaction; any cursor failure rolls back view, receipt and work.
    fn commit_transition(
        &self,
        publication: PreparedPublication,
        actions: &mut dyn WorkActionCursor,
        budget: &mut WorkBudget,
    ) -> Result<OperationStatus, IndexError>;
}

/// Durable genesis custody exists before the workspace head or any view root.
///
/// Confirmation follows the immutable store's physical sync; publication
/// atomically replaces bootstrap custody with the first view root.
pub trait CatalogGenesisPort: Send + Sync {
    /// Derives the canonical empty profile through the selected identity port.
    ///
    /// Reopening the same workspace, epoch and profile returns the same stage;
    /// a changed identity conflicts.
    fn begin_bootstrap(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        identity: &dyn ImmutableObjectPort,
        budget: &mut WorkBudget,
    ) -> Result<BootstrapRef, IndexError>;
    fn reserve_bootstrap(
        &self,
        stage: BootstrapRef,
        object: &RegisteredObject,
    ) -> Result<(), IndexError>;
    fn confirm_bootstrap(
        &self,
        stage: BootstrapRef,
        object: &RegisteredObject,
    ) -> Result<(), IndexError>;
    fn publish_bootstrap(
        &self,
        stage: BootstrapRef,
        genesis: CatalogView,
        budget: &mut WorkBudget,
    ) -> Result<ViewRef, IndexError>;
    /// Explicit fenced abandonment. An expired bootstrap is not collected
    /// by TTL; this transition queues custody for bounded physical GC while
    /// preventing later publication by the old stage.
    fn abort_bootstrap(
        &self,
        stage: BootstrapRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError>;
}

/// One bounded page of terminal operation custody rows. The count is rows,
/// not distinct immutable objects; terminal receipts and registered objects
/// remain catalog authorities after this release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalCustodyPage {
    pub released: u32,
    pub has_more: bool,
}

/// Durable terminal-ready ordering is independent of producer operation keys.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalCustodyCandidate {
    pub sequence: u64,
    pub operation: OperationKey,
}

pub trait CatalogCustodyMaintenancePort: Send + Sync {
    /// One indexed terminal-custody candidate in durable insertion order.
    ///
    /// `through` freezes a head sweep's upper frontier. This
    /// reads the durable ready set, never scans settled replay receipts.
    fn next_terminal_custody(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        after: Option<u64>,
        through: Option<u64>,
        budget: &mut WorkBudget,
    ) -> Result<Option<TerminalCustodyCandidate>, IndexError>;
    fn release_terminal_custody(
        &self,
        key: OperationKey,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<TerminalCustodyPage, IndexError>;
}

/// Durable lower bound for new Exact view acquisitions.
///
/// Moving the floor does not revoke tickets already pinned or delete any view.
/// A retained Published receipt proves the original outcome identity; it does
/// not promise a new Exact acquisition below this floor.
pub trait CatalogExactHistoryPort: Send + Sync {
    fn exact_history_floor(&self, workspace: WorkspaceKey, epoch: u64) -> Result<u64, IndexError>;
    /// Retain the current sequence and its preceding sequence window.
    ///
    /// Derive and advance the monotone floor inside one catalog transaction; widening
    /// the configured window cannot restore already retired history.
    fn retain_exact_history_tail(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        sequence_window: u64,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError>;
    fn advance_exact_history_floor(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        retain_from_sequence: u64,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError>;
    /// Examines at most `limit` views below the explicit floor. `after` is the
    /// last examined sequence, including blocked rows. Revisit blockers by
    /// starting a later sweep with `after=None`.
    fn retire_exact_history_page(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        after: Option<u64>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ViewRetirementPage, IndexError>;
}

/// Catalog-row retirement only. It neither unlinks immutable objects nor
/// changes Published receipts; those identities may outlive Exact access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ViewRetirementPage {
    pub after: Option<u64>,
    pub examined: u32,
    pub retired: u32,
    pub blocked: u32,
    pub has_more: bool,
}

/// Cursor for the last examined work row, ordered by its created view and
/// canonical work identity. Blocked rows remain eligible for a later sweep.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct WorkRetirementCursor {
    pub sequence: u64,
    pub work: Digest,
}

/// One bounded page of terminal work eviction.
///
/// Eviction removes the retained work status and prepared ABA candidate. It
/// includes inactive managed work only with exactly one owned request. A
/// terminal receipt and its request remain for replay. A superseded assignment
/// that was never dispatched has no receipt, so its unused request is evicted.
/// A later Create for the same intent starts fresh. Immutable objects remain intact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalWorkRetirementPage {
    pub after: Option<WorkRetirementCursor>,
    pub examined: u32,
    pub retired: u32,
    pub blocked: u32,
    pub has_more: bool,
}

pub trait CatalogTerminalWorkRetentionPort: Send + Sync {
    /// Examines at most `limit` work rows created below the explicit Exact
    /// floor. Pending, claimed, pinned, and managed rows without a safe terminal
    /// disposition stay in place. Restart from `after=None` to revisit blockers.
    fn retire_terminal_work_page(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        after: Option<WorkRetirementCursor>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<TerminalWorkRetirementPage, IndexError>;
}

/// Sorted retire/create actions from one attempt-fenced, frozen producer stream.
/// Producer failure or invalid EOF rolls the complete catalog transaction back.
pub trait WorkActionCursor {
    fn next_action(&mut self, budget: &mut WorkBudget) -> Result<Option<WorkAction>, IndexError>;
}

/// A diagnostic-only lexical scope, confined to the current thread.
///
/// It restores nesting on drop and grants no execution, funding or read authority.
/// Keep it inside a synchronous call; it must never cross an await.
pub trait CatalogDiagnosticScope {}

/// Original request observation for Catalog APIs without a work-budget argument.
pub trait CatalogDiagnosticPort: Send + Sync {
    /// Bind this exact Catalog and thread. None masks an enclosing observation.
    ///
    /// An inactive Off scope needs no allocation, clock or events. Explicit
    /// budget-aware calls retain their own context, including explicit Off.
    fn enter_diagnostic_scope(
        &self,
        context: Option<&crate::DiagnosticContextV1>,
    ) -> Option<Box<dyn CatalogDiagnosticScope + '_>>;
}

pub trait CatalogReadPort: Send + Sync {
    /// Bind one exact ticket and original request. Entry observation is durable
    /// before return; no caller work runs under a held Catalog transaction.
    fn begin_read_attempt<'a>(
        &'a self,
        session: &'a dyn super::ReadSessionLease,
        ticket: super::ReadTicketLease,
        budget: &'a WorkBudget,
    ) -> Result<Box<dyn super::CatalogReadAttempt + 'a>, IndexError>;
    fn open_read_session(&self) -> Result<Arc<dyn super::ReadSessionLease>, IndexError>;
    /// Checks the original request before waiting and before pin commit.
    ///
    /// Once committed, returns the ticket even if the request expires during
    /// commit, so its owner can retain or release the durable pin.
    fn acquire_ticket(
        &self,
        session: &dyn super::ReadSessionLease,
        selection: ViewSelection,
        lease: Duration,
        budget: &WorkBudget,
    ) -> Result<TicketRecord, IndexError>;
    fn renew_ticket(
        &self,
        session: &dyn super::ReadSessionLease,
        ticket: super::ReadTicketLease,
        lease: Duration,
    ) -> Result<super::ReadTicketLease, IndexError>;
    /// Refuse before commit on the original request's deadline or cancellation.
    /// Once committed, return the new lease even if the request expires during
    /// commit, so the caller can retain custody and replay its ACK.
    fn renew_ticket_for_request(
        &self,
        session: &dyn super::ReadSessionLease,
        ticket: super::ReadTicketLease,
        lease: Duration,
        budget: &WorkBudget,
    ) -> Result<super::ReadTicketLease, IndexError>;
    fn validate_ticket(
        &self,
        session: &dyn super::ReadSessionLease,
        ticket: super::ReadTicketLease,
    ) -> Result<(), IndexError>;
    /// Validate the same ticket under the caller's original deadline and
    /// cancellation, including any wait to enter Catalog.
    fn validate_ticket_for_request(
        &self,
        session: &dyn super::ReadSessionLease,
        ticket: super::ReadTicketLease,
        budget: &WorkBudget,
    ) -> Result<(), IndexError>;
    fn release_ticket(
        &self,
        session: &dyn super::ReadSessionLease,
        ticket: super::ReadTicketLease,
    ) -> Result<(), IndexError>;
    fn recover_read_sessions(
        &self,
        after: Option<super::ReadSessionRef>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<super::ReadSessionRecoveryPage, IndexError>;
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
    /// Bounded eligible page. Due managed retries and expired managed claims
    /// precede unstarted pending work: their frozen total lifetime has begun.
    /// Each lane retains timestamp/canonical-work ordering; discovery is read-only
    /// and cannot extend an assignment's deadline, lease or dispatch allowance.
    /// A fresh scan budget's actual deadline refusal yields `RetryDeadline`;
    /// unrelated capacity, cancellation and storage failures stay errors.
    fn ready_owned_work(
        &self,
        workspace: WorkspaceKey,
        epoch: u64,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<OwnedReadyOutcome, IndexError>;
    /// Atomically installs claim + original request + frozen policy.
    ///
    /// Checks the supplied budget before returning success, including after
    /// commit. `SQLite` calls themselves are not preempted. If that check refuses, a committed claim remains
    /// durable and a retry by the same intent recovers its existing assignment.
    /// Existing assignments retain their policy, overall deadline and dispatch count.
    fn acquire_owned_work(
        &self,
        intent: &WorkIntent,
        policy: OwnedWorkPolicy,
        budget: &WorkBudget,
    ) -> Result<Option<OwnedWorkAssignment>, IndexError>;
    /// Called after execution resource reservation. Charges a complete grant
    /// before execution; expired fences and exhausted jobs cannot dispatch.
    ///
    /// Returns a process-local deadline anchored at the Catalog clock sample,
    /// so connection waiting is charged once and commit/delivery cannot extend it.
    fn begin_owned_work(
        &self,
        assignment: &OwnedWorkAssignment,
    ) -> Result<OwnedWorkBegin, IndexError>;
    /// Only the current active grant may renew a managed claim, using its
    /// frozen lease policy and overall deadline. Manual renewal cannot do so.
    fn renew_owned_work(&self, grant: &OwnedWorkGrant) -> Result<WorkClaim, IndexError>;
    fn finish_owned_work(
        &self,
        grant: &OwnedWorkGrant,
        report: OwnedWorkReport,
    ) -> Result<(), IndexError>;
    /// Validate the exact immutable owned request and inspect its status in one
    /// Catalog transaction. Unknown/pending requests also require their live
    /// materialization claim. Terminal status permits replay only; the returned
    /// observation does not replace validation at execution or publication.
    /// Connection acquisition and the returned observation honor the request budget.
    fn validate_owned_work(
        &self,
        request: &super::UpdateRequest,
        budget: &WorkBudget,
    ) -> Result<OperationStatus, IndexError>;
    /// Manual claim uses the caller's original budget for Catalog mutex
    /// waiting and the pre-commit checkpoint. Once committed, the durable
    /// claim is returned even if that budget expires after the checkpoint.
    fn claim_work(
        &self,
        intent: &WorkIntent,
        lease: std::time::Duration,
        budget: &WorkBudget,
    ) -> Result<WorkClaim, IndexError>;
    /// Renewal keeps the fence stable. Validation uses the catalog's current
    /// lease, so a previously admitted immutable request remains executable.
    /// Manual renewal uses the caller's original budget for mutex waiting and
    /// the pre-commit checkpoint; a committed renewal returns its durable claim.
    fn renew_work(
        &self,
        claim: &WorkClaim,
        lease: std::time::Duration,
        budget: &WorkBudget,
    ) -> Result<WorkClaim, IndexError>;
    /// Validate materialization under one Catalog clock/transaction boundary.
    /// Managed work requires both its exact immutable request and live claim;
    /// a terminal replay allowance cannot authorize a new materialization.
    fn validate_materialization(&self, request: &super::UpdateRequest) -> Result<(), IndexError>;
    /// Validate materialization and issue its original session's latest-view
    /// read ticket in one transaction/clock sample. Managed requests require
    /// the exact immutable owned body and live claim. No pin is committed on
    /// failure. The original request funds view decoding and bounds entry and
    /// pre-commit cancellation. Execution still checks prepared work before the provider and
    /// publication still checks the live claim and view CAS.
    fn acquire_materialization_ticket(
        &self,
        session: &dyn super::ReadSessionLease,
        request: &super::UpdateRequest,
        lease: Duration,
        budget: &WorkBudget,
    ) -> Result<TicketRecord, IndexError>;
    /// The caller's original budget bounds mutex waiting. A successful read
    /// rechecks it after releasing the guard; in-flight `SQLite` calls are not preempted.
    fn work_status(
        &self,
        intent: &WorkIntent,
        budget: &WorkBudget,
    ) -> Result<WorkStatus, IndexError>;
    /// Observe the exact live claim and its prepared artifact in one read snapshot.
    /// The original budget bounds connection acquisition and returning the observation.
    fn prepared_work(
        &self,
        claim: &WorkClaim,
        budget: &WorkBudget,
    ) -> Result<Option<OwnerMemberBinding>, IndexError>;
    /// Durably retain the prepared artifact under the same live claim. The
    /// original budget bounds connection acquisition and the pre-commit check.
    /// A committed stage remains recoverable even if the caller then expires.
    fn stage_work(
        &self,
        claim: &WorkClaim,
        binding: OwnerMemberBinding,
        budget: &WorkBudget,
    ) -> Result<(), IndexError>;
}

pub trait IndexOwnerBuildPort: Send + Sync {
    type Candidate: Send;
    /// Reject an incompatible published search layout before owner derivation.
    /// This check is pure: it must not allocate, mutate storage, or call a provider.
    fn validate_coverage(&self, _coverage: &super::CoverageBinding) -> Result<(), IndexError> {
        Ok(())
    }
    /// Prepare a search layout in the same attempt as owner coverage. A new
    /// member is already verified and stored. An admitted layout object may be
    /// installed with its coverage parent through the existing child-first batch;
    /// its provisional identity is not readable before installation. Adoption
    /// remains the caller's original publication CAS. Membership-only
    /// changes preserve immutable regions while readers revalidate candidates
    /// against the selected owners. The default layout is the owner map.
    fn prepare_searchable<'a>(
        &'a self,
        _instance: &'a InstanceSpec,
        _prior: Option<&'a super::CoverageBinding>,
        owners: MapRoot,
        _materialized: Option<(OwnerKey, ObjectRef)>,
        _object_maps: Option<&'a Arc<dyn ObjectMapBatchPort>>,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, super::SearchablePreparation> {
        Box::pin(async move {
            context.budget.checkpoint()?;
            Ok(super::SearchablePreparation::existing(owners.0))
        })
    }
    /// Immutable minimum logical demand of one owner build. CPU, IO and memory
    /// must fit the service's execution envelope; the declared provider units
    /// are reserved during execution. Non-provider adapters explicitly return
    /// `ResourceNeed::default()`. This declaration must remain constant after registration.
    fn resource_demand(&self) -> super::effort::ResourceNeed;
    /// Validates recipe and input-domain compatibility before profile admission.
    /// Must not invoke derivation, allocate artifacts or access external providers.
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError>;
    /// `prior` is a validated published member and only a reuse candidate.
    /// It does not prove the old artifact matches `input`: the adapter must
    /// certify its dependency identity and return a new exact binding or derive.
    fn prepare_owner<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        owner: OwnerKey,
        input: OwnerInputBinding,
        prior: Option<OwnerMemberBinding>,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate>;
    fn verify_owner<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, OwnerMemberBinding>;
}

/// A required group artifact derived from a certified projection, committed
/// in the same view as that projection. Bootstrap is explicit; a failed delta
/// must never fall back to a whole projection rebuild.
pub trait IndexProjectionBuildPort: Send + Sync {
    type Candidate: Send;
    fn resource_demand(&self) -> super::effort::ResourceNeed;
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError>;
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "The future owns its fixed certified projection snapshot independently of the caller's temporary binding"
    )]
    fn prepare_projection<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<MemberBinding>,
        projection: ProjectionBinding,
        affected: Option<OwnerMutationSet>,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate>;
    fn verify_projection<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding>;
}

/// Verify a sealed, complete History input under the existing attempt's
/// original budget and custody. No file SourceDelta or provider is implied.
/// The Kernel owns profile/view CAS; the adapter owns History cut/replay and
/// row semantics, including the exact prior member for DELTA.
pub trait IndexHistoryBuildPort: Send + Sync {
    fn resource_demand(&self) -> super::effort::ResourceNeed;
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError>;
    fn verify_history(
        &self,
        instance: &InstanceSpec,
        prior: Option<&MemberBinding>,
        transition: &HistoryTransition,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError>;
    fn append_history(
        &self,
        instance: &InstanceSpec,
        prior: &MemberBinding,
        append: &HistoryAppendV1,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError>;
    fn advance_history_generation(
        &self,
        instance: &InstanceSpec,
        prior: &MemberBinding,
        advance: &HistoryGenerationAdvanceV1,
        next_view_sequence: u64,
        context: &mut AttemptContext<'_>,
    ) -> Result<MemberBinding, IndexError>;
}

pub trait IndexBuildPort: Send + Sync {
    type Delta: Send;
    type Plan: Send;
    type Candidate: Send;
    /// Immutable minimum logical resources required by one serial build. Adapters that
    /// call a provider must declare its units; other adapters must explicitly
    /// return `ResourceNeed::default()`. This declaration must remain constant after registration.
    fn resource_demand(&self) -> super::effort::ResourceNeed;
    /// Validates adapter-specific configuration before profile admission.
    /// Validation has no build, storage or external-provider side effects.
    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError>;
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
    /// Replace only the physical artifact for one owner range of the exact
    /// selected member, preserving its input and logical commitment.
    fn compact_source_local<'a>(
        &'a self,
        _prior: MemberBinding,
        _owner: OwnerKey,
        _plan: PlanContext,
        _context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, MemberBinding> {
        Box::pin(async {
            Err(IndexError::InvalidInput(
                "index format does not support physical compaction".into(),
            ))
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExistingTreeEntryKindV1 {
    Directory,
    File,
    Symlink,
    Socket,
    Other,
}

/// Metadata only. Implementations must admit directory FD/buffer/entry costs
/// before effects, authenticate the actual existing root and no-follow paths,
/// preserve typed budget failures, and retain close custody. The callback's
/// name is borrowed; it cannot outlive the current admitted directory entry.
/// This port does not grant Source, Catalog, or publication authority.
pub trait ExistingTreeMetadataIoPortV1: std::fmt::Debug + Send + Sync {
    fn visit_directory_v1(
        &self,
        path: &std::path::Path,
        budget: &mut WorkBudget,
        visit: &mut dyn FnMut(
            &std::ffi::OsStr,
            ExistingTreeEntryKindV1,
            u64,
            &mut WorkBudget,
        ) -> Result<(), ExistingFileReadFailureV1>,
    ) -> Result<(), ExistingFileReadFailureV1>;
}
