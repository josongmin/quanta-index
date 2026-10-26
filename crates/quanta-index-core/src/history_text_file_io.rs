//! Physical file custody for the existing history text index.

use std::ffi::OsString;
use std::fs::Metadata;
use std::path::Path;
use std::sync::Arc;

use crate::CoreError;
use crate::publication_admission::{PublicationControlFileMutation, PublicationObjectReader};

/// One anchored directory entry, observed under the current root lease.
pub struct HistoryTextDirectoryEntry {
    pub name: OsString,
    pub metadata: Metadata,
}

/// An original lock FD and its namespace custody, retained until the last owner.
///
/// Failed retirement keeps the existing physical authority unavailable for
/// further publication; dropping a lock never fabricates a successful close.
pub trait HistoryTextPhysicalLock: Send + Sync {
    fn retire(self: Box<Self>) -> Result<(), CoreError>;
}

/// Filesystem operations scoped to the history authority's canonical root.
///
/// Publication scopes subdivide one already-held operation allowance. Reads
/// retain the original FD through the returned reader or engine file handle.
/// No method creates a new capacity ledger or silently tops up a held parent.
pub trait HistoryTextFileIoPort: Send + Sync {
    /// The original request interruption actually observed by this operation's
    /// admitted I/O. Engine worker-dead errors can erase their source chain;
    /// this witness must never be inferred from a later cancellation probe.
    fn original_interruption(&self) -> Result<Option<CoreError>, CoreError>;
    fn validate_root(&self, root: &Path) -> Result<(), CoreError>;
    fn metadata(&self, path: &Path) -> Result<Option<Metadata>, CoreError>;
    fn list_directory(&self, path: &Path) -> Result<Vec<HistoryTextDirectoryEntry>, CoreError>;
    fn ensure_directory(&self, path: &Path) -> Result<(), CoreError>;
    fn open_reader(
        &self,
        path: &Path,
    ) -> Result<Option<Box<dyn PublicationObjectReader>>, CoreError>;

    /// Create a visible new file; each write reserves growth before its syscall.
    ///
    /// The original writer remains owned until `sync_and_settle` or checked
    /// pending retirement. Existing names are refused, never truncated.
    fn create_writer(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationControlFileMutation>, CoreError>;

    /// Stage exact bytes and atomically replace the authenticated target inode.
    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<(), CoreError>;
    fn delete_file(&self, path: &Path) -> Result<(), CoreError>;
    fn link_file(&self, source: &Path, target: &Path) -> Result<(), CoreError>;
    fn copy_file(&self, source: &Path, target: &Path) -> Result<(), CoreError>;
    /// Recover publication paths before probing or reserving a new allowance.
    ///
    /// True proves the original matching move completed. False means no move
    /// was recovered, including after completing an original tree cleanup of
    /// `source`. Unchanged failures and competing intents remain explicit.
    fn recover_directory_move(&self, source: &Path, target: &Path) -> Result<bool, CoreError>;
    fn move_directory_if_absent(&self, source: &Path, target: &Path) -> Result<(), CoreError>;

    /// Recover an original matching tree deletion before an absence probe.
    /// The receipt uses the original plan's path bytes, never a second scan.
    fn recover_tree_delete(&self, path: &Path) -> Result<Option<u64>, CoreError>;

    /// Remove a tree using the canonical pending-deletion owner.
    ///
    /// Returned bytes describe removed path lengths for the existing receipt;
    /// they are not a physical refund. Unique inode pins govern actual refund.
    fn remove_tree(&self, path: &Path) -> Result<Option<u64>, CoreError>;
    fn sync_directory(&self, path: &Path) -> Result<(), CoreError>;
    fn acquire_lock(
        &self,
        path: &Path,
        blocking: bool,
    ) -> Result<Box<dyn HistoryTextPhysicalLock>, CoreError>;
}

/// Composition-owned capability factory on the same publication authority.
///
/// Reserve the explicit history-operation allowance before opening its first
/// mutable engine or manifest file. Read and retirement scopes cannot create
/// or grow files. Scope cloning grants no additional global capacity.
/// Publication retains the original request budget through engine-owned writes;
/// retirement and physical recovery remain callable after request cancellation.
pub trait HistoryTextFileAdmissionFactory: Send + Sync {
    fn for_read(&self) -> Result<Arc<dyn HistoryTextFileIoPort>, CoreError>;
    fn for_publication(
        &self,
        budget: &crate::RequestBudgetV1,
    ) -> Result<Arc<dyn HistoryTextFileIoPort>, CoreError>;
    fn for_retirement(&self) -> Result<Arc<dyn HistoryTextFileIoPort>, CoreError>;
}
