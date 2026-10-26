//! Owned disposable preparation space. Allocation never grants publication or
//! physical object reclamation authority.

use std::{fmt::Debug, path::Path, sync::Arc};

use super::{IndexError, WorkBudget};

/// Initial admission and a separate growth envelope for preparation.
///
/// Consumers must enforce the returned main-file envelope. Secondary spill
/// requires separate admission and enforcement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScratchRequest {
    pub initial_bytes: u64,
    pub max_bytes: u64,
}

/// Local operation custody for the explicitly installed scratch file driver.
/// Backend codes are interpreted only by their producing driver and operation.
pub trait ScratchIoScope {
    fn is_capacity_refusal(&self, backend_error_code: i32) -> bool;
}

/// The exclusive namespace lock and its actual file-handle custody.
/// Keep this lease through the last allocation; retirement follows actual close.
pub trait ScratchNamespaceLease: Debug + Send + Sync {
    fn namespace_identity(&self) -> crate::publication_capacity::InodeKey;
}

/// The physical owner of a managed scratch namespace and its file driver.
/// No database handle, publication authority or execution grant crosses this port.
pub trait ScratchIoBinding: Debug + Send + Sync {
    fn driver_name(&self) -> &str;
    fn enter_scope(&self) -> Box<dyn ScratchIoScope>;
    /// Prepare and exclusively lock the namespace through the same file authority.
    /// Handle admission must precede open; failure retains uncertain custody.
    fn acquire_namespace(&self, path: &Path) -> Result<Box<dyn ScratchNamespaceLease>, IndexError>;
    /// Delete only after all connection/file handles have retired. A failed
    /// deletion keeps its charge and must not fall back to untracked cleanup.
    /// Request cleanup borrows the original budget through every bounded step.
    /// None is lifecycle cleanup (including Drop), without a new request grant.
    /// Cancellation before a destructive step preserves remaining custody;
    /// an admitted unlink/sync/close settlement finishes before cancellation.
    fn retire_namespace(
        &self,
        path: &Path,
        budget: Option<&mut WorkBudget>,
    ) -> Result<(), IndexError>;
}

/// An owner-private directory and its reservation.
///
/// Keep this allocation alive until all file/database handles inside it have closed.
/// Its implementation
/// retains namespace custody through normal completion, error and unwind.
pub trait ScratchAllocation: Send {
    fn path(&self) -> &Path;
    fn max_bytes(&self) -> u64;
    fn reserved_bytes(&self) -> Result<u64, IndexError>;
    /// Atomically reserve additional shared capacity before increasing a writer's limit.
    /// Denial leaves the existing reservation unchanged. Reservations never shrink
    /// until namespace cleanup succeeds.
    fn reserve_at_least(&self, bytes: u64, budget: &mut WorkBudget) -> Result<(), IndexError>;
    /// Absence explicitly leaves backend I/O unqualified. A managed physical
    /// profile must supply the same owner used for namespace preparation/cleanup.
    fn io_binding(&self) -> Option<Arc<dyn ScratchIoBinding>> {
        None
    }
}

/// Shared admission for disposable preparation space.
///
/// A successful reservation is not proof that external files or `SQLite` secondary spill are
/// bounded; those writers require their own enforcement against this envelope.
pub trait ScratchSpacePort: Send + Sync {
    fn allocate(
        &self,
        request: ScratchRequest,
        budget: &mut WorkBudget,
    ) -> Result<Box<dyn ScratchAllocation>, IndexError>;
}
