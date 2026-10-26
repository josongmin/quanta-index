//! Non-persisted resource admission for one source-corpus publication.
//!
//! This is a custody boundary, not an estimator or an implementation of a
//! process-wide ledger. The materializer must supply a measured footprint and
//! authenticated build plan. A runtime port must account for concurrent work,
//! retained files, pins, and durable writes before returning a permit.

use std::fs::{File, Metadata};
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::sync::Arc;

use quanta_index_contract::SearchCorpusIngestBatch;

use crate::publication_capacity::PublicationAllowance;
use crate::{CoreError, IngestBatchFootprint, RequestBudgetV1};

/// Work remaining after the target's recovery state is authenticated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicationBuildWork {
    /// Both target tracks already exist; only finalization remains.
    FinalizeOnly,
    /// Build the missing tracks named by the authenticated recovery plan.
    Materialize { lexical: bool, semantic: bool },
}

/// Exact batch and measured work presented at the publication boundary.
///
/// The footprint is the existing batch resource measurement. It is not an
/// estimate of all memory, I/O, temporary disk, or retained physical bytes.
pub struct PublicationAdmissionRequest<'a> {
    pub batch: &'a SearchCorpusIngestBatch,
    pub footprint: IngestBatchFootprint,
    pub work: PublicationBuildWork,
}

/// The publication allowance that owns one output's provisional charge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicationOutputClass {
    Control,
    Lexical,
    Semantic,
    Source,
    Scratch,
}

/// A positive, checked upper bound charged before one output is created.
///
/// This subdivides the already held control or work allowance; it must not
/// charge an unrelated ledger a second time. An implementation must bind the
/// output to an exact durable identity before it can release a charge. The
/// path-bound child owns exactly one file slot. Multi-file writers reserve a
/// separate child for each file rather than sharing one prospective path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicationOutputReservation {
    class: PublicationOutputClass,
    max_encoded_bytes: u64,
    max_files: u64,
}

impl PublicationOutputReservation {
    pub fn new(
        class: PublicationOutputClass,
        max_encoded_bytes: u64,
        max_files: u64,
    ) -> Result<Self, CoreError> {
        if max_encoded_bytes == 0 || max_files != 1 {
            return Err(CoreError::InvalidContract(
                "publication output reservation needs positive bytes and exactly one file".into(),
            ));
        }
        Ok(Self {
            class,
            max_encoded_bytes,
            max_files,
        })
    }

    #[must_use]
    pub const fn class(self) -> PublicationOutputClass {
        self.class
    }

    #[must_use]
    pub const fn max_encoded_bytes(self) -> u64 {
        self.max_encoded_bytes
    }

    #[must_use]
    pub const fn max_files(self) -> u64 {
        self.max_files
    }
}

/// One output's non-clone provisional-to-durable transition.
///
/// `bind_output` must identify exactly one prospective staging and target path
/// before `creation_started`. The ledger validates both paths against its owned
/// root and any replacement/reuse identity. `creation_started` must be called
/// before the first filesystem create. From that point, `Drop` cannot refund
/// the charge: a failed create, rename, sync,
/// or ambiguous result may have left bytes behind. `committed` follows durable
/// promotion and authenticates the actual inode and widths against the bound
/// paths and reservation. For a subsequent rename of that committed inode,
/// `rename_started` validates and records its current path and exact new target
/// before the filesystem operation. The charge stays held while rename or
/// parent sync is uncertain. Only after rename and both affected parent
/// directories are synced may `renamed_and_synced` authenticate the same inode
/// at the new path and transfer path custody without another charge. An inode
/// overwritten at the target stays charged until its actual last reference is
/// deleted. `delete_file_and_sync` transfers removal to the authority, which
/// retains any uncertain delete ticket after an error; it cannot retire a
/// preexisting replaced file. Repeated or out-of-order transitions fail
/// closed. Crash recovery must rebuild charges from physical inventory before
/// admitting more work.
pub trait PublicationOutputPermit: Send {
    fn bind_output(&mut self, staging: &Path, target: &Path) -> Result<(), CoreError>;
    fn creation_started(&mut self) -> Result<(), CoreError>;
    /// Bind the exact returned writer FD and declared temporary pathname before
    /// the first payload write. The runtime keeps an independent FD witness.
    fn created(&mut self, writer: &File, staging: &Path) -> Result<(), CoreError>;
    /// Recheck a possibly replaced old destination inode before rename.
    fn before_replace(&mut self) -> Result<(), CoreError>;
    /// Authenticate the durable destination, then consume and physically close
    /// the actual writer FD before any settled lease may be transferred.
    fn committed(&mut self, writer: File, actual_encoded_bytes: u64) -> Result<(), CoreError>;
    /// Physically close the actual writer after an uncertain write, promotion,
    /// or sync. This proves FD retirement only; all physical/pending charges
    /// remain held for independent reconciliation or boot inventory.
    fn retire_writer_preserving_pending(&mut self, writer: File) -> Result<(), CoreError>;
    /// Cancel a real staged birth whose initial FD registration was rejected.
    ///
    /// Authenticate and unlink only its declared temporary, sync the parent,
    /// prove zero links and checked-close the original FD before refund.
    /// An uncertain witness preserves the original charge and cause.
    fn cancel_rejected_created(&mut self, writer: File) -> Result<(), CoreError>;
    /// Acquire the canonical path ticket before changing the namespace.
    fn rename_started(&mut self, target: &Path) -> Result<(), CoreError>;
    /// Authenticate the renamed inode after both affected parents sync.
    fn renamed_and_synced(&mut self) -> Result<(), CoreError>;
    /// Execute removal of this committed output through its retained authority.
    /// The authority owns any started ticket beyond this permit's lifetime and
    /// must reconcile the same inode on retry without deleting a replacement.
    fn delete_file_and_sync(&mut self) -> Result<(), CoreError>;
    /// Failed create returned no FD. Authenticates declared temp and target.
    fn confirmed_uncreated_absent_and_synced(&mut self) -> Result<(), CoreError>;
    /// Failed creation/write left a writer FD. Consumes that actual FD only
    /// after the caller has unlinked and synced the temporary pathname.
    fn confirmed_safe_absent_after_writer_close(&mut self, writer: File) -> Result<(), CoreError>;
}

/// Keep the parent allowance alive until the last borrowed output settles.
/// Field order releases the child before the parent on ordinary drop/unwind.
struct OwnedPublicationOutputPermit {
    child: Box<dyn PublicationOutputPermit>,
    _parent: Arc<dyn PublicationResourcePermit>,
}

impl PublicationOutputPermit for OwnedPublicationOutputPermit {
    fn cancel_rejected_created(&mut self, writer: File) -> Result<(), CoreError> {
        self.child.cancel_rejected_created(writer)
    }
    fn bind_output(&mut self, staging: &Path, target: &Path) -> Result<(), CoreError> {
        self.child.bind_output(staging, target)
    }

    fn creation_started(&mut self) -> Result<(), CoreError> {
        self.child.creation_started()
    }

    fn created(&mut self, writer: &File, staging: &Path) -> Result<(), CoreError> {
        self.child.created(writer, staging)
    }

    fn before_replace(&mut self) -> Result<(), CoreError> {
        self.child.before_replace()
    }

    fn committed(&mut self, writer: File, actual_encoded_bytes: u64) -> Result<(), CoreError> {
        self.child.committed(writer, actual_encoded_bytes)
    }

    fn retire_writer_preserving_pending(&mut self, writer: File) -> Result<(), CoreError> {
        self.child.retire_writer_preserving_pending(writer)
    }

    fn rename_started(&mut self, target: &Path) -> Result<(), CoreError> {
        self.child.rename_started(target)
    }

    fn renamed_and_synced(&mut self) -> Result<(), CoreError> {
        self.child.renamed_and_synced()
    }

    fn delete_file_and_sync(&mut self) -> Result<(), CoreError> {
        self.child.delete_file_and_sync()
    }

    fn confirmed_uncreated_absent_and_synced(&mut self) -> Result<(), CoreError> {
        self.child.confirmed_uncreated_absent_and_synced()
    }

    fn confirmed_safe_absent_after_writer_close(&mut self, writer: File) -> Result<(), CoreError> {
        self.child.confirmed_safe_absent_after_writer_close(writer)
    }
}

/// Borrowed authority for transient publication output writers.
///
/// This is not an owned reservation and must not be stored in a publication
/// DTO. The owner retains the parent allowances; each successful `reserve`
/// returns a child permit that retains its parent independently.
#[derive(Clone, Copy)]
pub struct PublicationOutputAdmission<'a> {
    control: Option<&'a Arc<dyn PublicationResourcePermit>>,
    work: Option<&'a Arc<dyn PublicationResourcePermit>>,
}

impl<'a> PublicationOutputAdmission<'a> {
    pub(crate) fn new(
        control: Option<&'a Arc<dyn PublicationResourcePermit>>,
        work: Option<&'a Arc<dyn PublicationResourcePermit>>,
    ) -> Self {
        Self { control, work }
    }

    pub fn reserve(
        &self,
        request: PublicationOutputReservation,
    ) -> Result<Option<Box<dyn PublicationOutputPermit>>, CoreError> {
        if self.control.is_none() && self.work.is_none() {
            return Ok(None);
        }
        let parent = match request.class() {
            PublicationOutputClass::Control => self.control,
            PublicationOutputClass::Lexical
            | PublicationOutputClass::Semantic
            | PublicationOutputClass::Source
            | PublicationOutputClass::Scratch => self.work,
        }
        .ok_or_else(|| {
            CoreError::InvalidContract(
                "publication output has no matching held resource permit".into(),
            )
        })?;
        let parent = Arc::clone(parent);
        let child = parent.reserve_output(request)?;
        Ok(Some(Box::new(OwnedPublicationOutputPermit {
            child,
            _parent: parent,
        })))
    }

    /// The exact already-held work parent used by physical writers. This does
    /// not reserve a second child or top up the global capacity ledger.
    pub fn held_work_file_allowance(&self) -> Result<Option<Arc<PublicationAllowance>>, CoreError> {
        match self.work {
            Some(parent) => parent.held_file_allowance(),
            None if self.control.is_none() => Ok(None),
            None => Err(CoreError::InvalidContract(
                "publication work has no matching held resource permit".into(),
            )),
        }
    }

    /// Retain the already admitted work parent for synchronous owned writers.
    /// This grants no new allowance and must not be serialized in a DTO.
    pub fn owned_work_admission(
        &self,
    ) -> Result<Option<PublicationWorkOutputAdmission>, CoreError> {
        match self.work {
            Some(parent) => Ok(Some(PublicationWorkOutputAdmission {
                parent: Arc::clone(parent),
            })),
            None if self.control.is_none() => Ok(None),
            None => Err(CoreError::InvalidContract(
                "publication work has no matching held resource permit".into(),
            )),
        }
    }

    /// Retain the already admitted Control parent for owned control writers.
    /// Cloning this authority grants no extra capacity or Work funding.
    pub fn owned_control_admission(
        &self,
    ) -> Result<Option<PublicationControlOutputAdmission>, CoreError> {
        match self.control {
            Some(parent) => Ok(Some(PublicationControlOutputAdmission {
                parent: Arc::clone(parent),
            })),
            None if self.work.is_none() => Ok(None),
            None => Err(CoreError::InvalidContract(
                "publication control has no matching held resource permit".into(),
            )),
        }
    }

    /// Return the already held control parent without a second reservation.
    pub fn held_control_file_allowance(
        &self,
    ) -> Result<Option<Arc<PublicationAllowance>>, CoreError> {
        match self.control {
            Some(parent) => parent.held_file_allowance(),
            None if self.work.is_none() => Ok(None),
            None => Err(CoreError::InvalidContract(
                "publication control has no matching held resource permit".into(),
            )),
        }
    }
}

/// Owned access to one publication's existing work allowance.
///
/// Child permits retain this same parent independently; cloning the authority
/// neither reserves more capacity nor authorizes Control outputs.
#[derive(Clone)]
pub struct PublicationWorkOutputAdmission {
    parent: Arc<dyn PublicationResourcePermit>,
}

impl PublicationWorkOutputAdmission {
    /// Borrow the same physical parent without creating another reservation.
    pub fn held_file_allowance(&self) -> Result<Option<Arc<PublicationAllowance>>, CoreError> {
        self.parent.held_file_allowance()
    }

    fn object_lifetime(&self) -> Option<Arc<dyn PublicationObjectLifetimePort>> {
        self.parent.object_lifetime()
    }

    pub fn reserve(
        &self,
        request: PublicationOutputReservation,
    ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
        if request.class() == PublicationOutputClass::Control {
            return Err(CoreError::InvalidContract(
                "publication work authority cannot fund Control outputs".into(),
            ));
        }
        let parent = Arc::clone(&self.parent);
        let child = parent.reserve_output(request)?;
        Ok(Box::new(OwnedPublicationOutputPermit {
            child,
            _parent: parent,
        }))
    }
}

/// Owned access to one publication's existing Control allowance.
/// Child permits retain this exact parent; Work classes always refuse.
#[derive(Clone)]
pub struct PublicationControlOutputAdmission {
    parent: Arc<dyn PublicationResourcePermit>,
}

impl PublicationControlOutputAdmission {
    pub fn held_file_allowance(&self) -> Result<Option<Arc<PublicationAllowance>>, CoreError> {
        self.parent.held_file_allowance()
    }

    #[must_use]
    pub fn object_lifetime(&self) -> Option<Arc<dyn PublicationObjectLifetimePort>> {
        self.parent.object_lifetime()
    }

    /// Obtain the same authority's update port before opening a mutable FD.
    /// Only explicit fixtures without a physical allowance may return None.
    pub fn file_updates(
        &self,
    ) -> Result<Option<Arc<dyn PublicationControlFileUpdatePort>>, CoreError> {
        let port = self.parent.control_file_updates();
        if self.held_file_allowance()?.is_some() && port.is_none() {
            return Err(CoreError::InvalidContract(
                "physical Control parent lacks its file update authority".into(),
            ));
        }
        Ok(port)
    }

    pub fn reserve(
        &self,
        request: PublicationOutputReservation,
    ) -> Result<Box<dyn PublicationOutputPermit>, CoreError> {
        if request.class() != PublicationOutputClass::Control {
            return Err(CoreError::InvalidContract(
                "publication Control authority cannot fund Work outputs".into(),
            ));
        }
        let parent = Arc::clone(&self.parent);
        let child = parent.reserve_output(request)?;
        Ok(Box::new(OwnedPublicationOutputPermit {
            child,
            _parent: parent,
        }))
    }
}

/// Opaque ownership of one actual Control writer through growth and sync.
/// A dropped or explicitly retired uncommitted writer retains pending charge.
pub trait PublicationControlFileMutation: Read + Write + Seek + Send {
    /// Metadata of the actual writer FD, never a fresh pathname probe.
    fn metadata(&self) -> Result<std::fs::Metadata, CoreError>;

    fn set_len(&mut self, length: u64) -> Result<(), CoreError>;
    /// Verify the reserved final width, sync and physically retire the writer.
    fn sync_and_settle(self: Box<Self>) -> Result<(), CoreError>;
    /// Check physical close without claiming durability or refunding mutation.
    fn retire_preserving_pending(self: Box<Self>) -> Result<(), CoreError>;
}

/// Control journal updates use the same physical parent and exact writer FD.
/// Implementations reserve before mutation and never top up the global ledger.
pub trait PublicationControlFileUpdatePort: Send + Sync {
    /// Reserve reader custody before the caller opens an anchored Control FD.
    fn begin_reader_open(
        &self,
        path: &Path,
        control: PublicationControlOutputAdmission,
    ) -> Result<Box<dyn PublicationControlOpenTicket>, CoreError>;
    fn begin_update(
        &self,
        path: &Path,
        writer: File,
        expected_length: u64,
        target_end: u64,
        control: PublicationControlOutputAdmission,
    ) -> Result<Box<dyn PublicationControlFileMutation>, CoreError>;
    /// Reconcile retained physical deletions and retired Control creates before
    /// rediscovering canonical pathnames. Physical recovery is not publication.
    fn reconcile_deletions(&self) -> Result<(), CoreError>;
    /// Remove a Control file through the same retained physical authority.
    /// Repeated absence after a settled deletion adds no capacity refund.
    fn remove_file_and_sync(
        &self,
        path: &Path,
        control: PublicationControlOutputAdmission,
    ) -> Result<(), CoreError>;
}

/// An owned Control reader, including the exact FD used for gate locking.
/// Its physical pin survives unlink until checked retirement of this FD.
pub trait PublicationControlReader: Read + Seek + Send {
    fn metadata(&self) -> Result<Metadata, CoreError>;
    fn sync_posix(&self) -> Result<(), CoreError>;
    fn lock_exclusive(&self) -> Result<(), CoreError>;
    /// Returns false only when another holder currently owns the gate.
    fn try_lock_exclusive(&self) -> Result<bool, CoreError>;
    fn retire(self: Box<Self>) -> Result<(), CoreError>;
}

/// Keep the Control reader-open claim across the caller's anchored open.
pub trait PublicationControlOpenTicket: Send {
    fn opened_owned(
        self: Box<Self>,
        file: File,
    ) -> Result<Box<dyn PublicationControlReader>, CoreError>;
}

/// An actual object FD owned by the physical lifetime authority. No raw FD or
/// clone escapes this boundary. Explicit retirement reports close failures.
pub trait PublicationObjectReader: Read + Seek + Send {
    fn metadata(&self) -> Result<Metadata, CoreError>;
    /// POSIX fsync on this exact reader FD, preserving the existing adoption
    /// barrier. Stronger device flushes belong to the writer's full-sync policy.
    fn sync_posix(&self) -> Result<(), CoreError>;
    fn retire(self: Box<Self>) -> Result<(), CoreError>;
}

/// Hold the reader-open claim across the caller's anchored open operation.
pub trait PublicationObjectOpenTicket: Send {
    fn opened_owned(
        self: Box<Self>,
        file: File,
    ) -> Result<Box<dyn PublicationObjectReader>, CoreError>;
}

/// One canonical deletion. A started ticket must remain in its existing GC
/// progress owner until settlement; dropping it does not prove reclamation.
pub trait PublicationObjectDeleteTicket: Send {
    fn source_metadata(&self) -> Result<(Metadata, Metadata), CoreError>;
    fn deletion_started(&mut self) -> Result<(), CoreError>;
    /// Validate absence, sync the anchored parent and close the deletion FD
    /// before refund. Other held reader pins still prevent reclamation.
    fn unlinked_and_synced(&mut self) -> Result<(), CoreError>;
    fn confirmed_unchanged_after_failure(&mut self) -> Result<(), CoreError>;
    fn has_started(&self) -> bool;
    fn is_settled(&self) -> bool;
}

/// Reads and GC share the physical authority that funds canonical outputs.
///
/// Implementations authenticate the already-held parent before any writer
/// creates a root, temporary file or output; this is not another ledger.
pub trait PublicationObjectLifetimePort: Send + Sync {
    fn validate_state_root(&self, root: &Path) -> Result<(), CoreError>;
    fn validate_parent(&self, parent: &Arc<PublicationAllowance>) -> Result<(), CoreError>;
    /// Claim an existing Control FD for read-only open and lifecycle locking.
    /// This capability cannot create, grow, delete or reserve a writer parent.
    fn begin_control_reader_open(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationControlOpenTicket>, CoreError>;
    fn begin_reader_open(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationObjectOpenTicket>, CoreError>;
    fn begin_delete(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationObjectDeleteTicket>, CoreError>;
}

/// Reject missing or foreign physical lifetime custody before writer setup.
/// Explicit test parents without physical allowances stay unqualified.
pub fn validate_object_lifetime(
    work: Option<&PublicationWorkOutputAdmission>,
    lifetime: Option<&Arc<dyn PublicationObjectLifetimePort>>,
) -> Result<(), CoreError> {
    match work
        .map(PublicationWorkOutputAdmission::held_file_allowance)
        .transpose()?
        .flatten()
    {
        Some(parent) => {
            let supplied = lifetime.ok_or_else(|| {
                CoreError::InvalidContract(
                    "physical publication writer lacks object lifetime authority".into(),
                )
            })?;
            let owned = work
                .and_then(PublicationWorkOutputAdmission::object_lifetime)
                .ok_or_else(|| {
                    CoreError::InvalidContract(
                        "physical work parent lacks its object lifetime capability".into(),
                    )
                })?;
            if !Arc::ptr_eq(&owned, supplied) {
                return Err(CoreError::InvalidContract(
                    "publication writer and reader use different lifetime capabilities".into(),
                ));
            }
            supplied.validate_parent(&parent)
        }
        None if lifetime.is_some() => Err(CoreError::InvalidContract(
            "qualified object writer lacks a physical work parent".into(),
        )),
        None => Ok(()),
    }
}

/// Authenticate the exact Control capability before a physical writer opens.
pub fn validate_control_object_lifetime(
    control: Option<&PublicationControlOutputAdmission>,
    lifetime: Option<&Arc<dyn PublicationObjectLifetimePort>>,
) -> Result<(), CoreError> {
    match control
        .map(PublicationControlOutputAdmission::held_file_allowance)
        .transpose()?
        .flatten()
    {
        Some(parent) => {
            let supplied = lifetime.ok_or_else(|| {
                CoreError::InvalidContract(
                    "physical Control writer lacks object lifetime authority".into(),
                )
            })?;
            let owned = control
                .and_then(PublicationControlOutputAdmission::object_lifetime)
                .ok_or_else(|| {
                    CoreError::InvalidContract(
                        "physical Control parent lacks its lifetime capability".into(),
                    )
                })?;
            if !Arc::ptr_eq(&owned, supplied) {
                return Err(CoreError::InvalidContract(
                    "Control writer and reader use different lifetime capabilities".into(),
                ));
            }
            supplied.validate_parent(&parent)
        }
        None if lifetime.is_some() => Err(CoreError::InvalidContract(
            "qualified Control writer lacks a physical Control parent".into(),
        )),
        None => Ok(()),
    }
}

/// Synchronous, thread-bound Catalog funding custody. It is deliberately not
/// Send: `SQLite` callbacks must execute on the thread that entered the scope.
pub trait CatalogPublicationFundingScope {}

/// Bind `SQLite` create and growth callbacks to an already held parent.
///
/// Implementations authenticate the authority and callback scope, retain the
/// parent for each mutation, and refuse missing or exhausted funding.
pub trait CatalogPublicationFundingPort: Send + Sync {
    fn enter_funded_scope(
        &self,
        parent: Arc<PublicationAllowance>,
    ) -> Result<Box<dyn CatalogPublicationFundingScope>, CoreError>;
}

/// Service-owned admission for lifecycle files outside a publication event.
///
/// Each positive exact encoded bound funds one Control file from the same
/// physical ledger. This does not qualify an entire event's control allowance.
pub trait DurableControlFileAdmissionPort: Send + Sync {
    /// Pin an existing service-owned Control file before its actual FD opens.
    /// This grants no create, growth, deletion or publication allowance.
    fn begin_reader_open(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationControlOpenTicket>, CoreError>;

    /// Settle retained deletion and retired creation custody before
    /// rediscovering staged pathnames. This does not publish a domain event.
    fn reconcile_deletions(&self) -> Result<(), CoreError>;

    /// Retain deletion custody across errors and subsequent constructor retries.
    fn delete_file_and_sync(&self, path: &Path) -> Result<(), CoreError>;

    fn reserve_output(
        &self,
        max_encoded_bytes: u64,
    ) -> Result<Box<dyn PublicationOutputPermit>, CoreError>;
}

/// Custody through journal settlement and source-event reconciliation.
///
/// Implementations release only provisional capacity on `Drop`.
/// Physical writes remain charged until confirmed durable deletion.
pub trait PublicationResourcePermit: Send + Sync {
    /// Missing update custody cannot qualify a physical Control journal writer.
    fn control_file_updates(&self) -> Option<Arc<dyn PublicationControlFileUpdatePort>> {
        None
    }

    /// The exact non-persisted lifetime capability of this physical parent.
    /// An absent capability cannot qualify a canonical object writer.
    fn object_lifetime(&self) -> Option<Arc<dyn PublicationObjectLifetimePort>> {
        None
    }

    /// A qualified resource permit returns its own already-held work/control
    /// allowance. `None` is reserved for explicit unqualified test fixtures;
    /// the qualified Semantic factory must refuse it before any write.
    fn held_file_allowance(&self) -> Result<Option<Arc<PublicationAllowance>>, CoreError>;

    /// Immediately subdivide this publication's held allowance before a leaf
    /// output write. This cannot wait on or inherit cancellation from the peer
    /// request: an accepted durable publication must reach settlement. Capacity
    /// refusal remains retryable under its original journal. No accepting
    /// default is provided.
    fn reserve_output(
        &self,
        request: PublicationOutputReservation,
    ) -> Result<Box<dyn PublicationOutputPermit>, CoreError>;
}

/// The composition-owned admission authority for a complete publication.
///
/// There is intentionally no accepting default. A runtime must provide
/// authenticated inventory, a shared ledger, and checked output bounds before
/// this port can qualify whole-publication resource limits. Bounds may come
/// from an encoder's conservative estimate or an explicit per-publication
/// allowance enforced before every physical create and growth. Reserving an
/// allowance without binding all writers does not qualify that limit. A
/// refusal should use the typed `IngestResourceBudgetExceeded` error code.
pub trait PublicationResourceAdmissionPort: Send + Sync {
    /// Reserve journal and control work before the outer operation is prepared.
    ///
    /// Terminal replay must return before this call. The returned permit and
    /// the subsequent work permit belong to the same authority and publication
    /// owner. Their charges must be disjoint; dropping the work permit must not
    /// release the journal's allowance before terminal settlement.
    fn try_acquire_control(
        &self,
        batch: &SearchCorpusIngestBatch,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn PublicationResourcePermit>, CoreError>;

    /// Acquire before the first durable source-event reservation.
    ///
    /// After control admission, the outer journal may already be durable.
    /// Request cancellation must not abort that accepted durable operation;
    /// a capacity refusal remains retryable under its original journal. For
    /// a direct materializer call without a journal, the budget can still
    /// refuse before its first durable reservation.
    fn try_acquire(
        &self,
        request: &PublicationAdmissionRequest<'_>,
        budget: &RequestBudgetV1,
    ) -> Result<Box<dyn PublicationResourcePermit>, CoreError>;
}
