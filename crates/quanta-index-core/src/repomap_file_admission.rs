//! Physical custody of immutable `RepoMap` projections, independent of visibility.

use std::path::Path;

use crate::CoreError;
use crate::publication_admission::PublicationObjectReader;
use crate::publication_capacity::InodeKey;

/// The existing projection's funding class; the runtime also authenticates its path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepoMapPhysicalFileClass {
    RootUuid,
    Candidate,
    Incident,
    Payload,
}

/// Publication never overwrites an existing immutable projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepoMapImmutablePutOutcome {
    Created,
    ExactReplay,
}

/// Installed before the UUID or boot reconciliation can mutate the layout.
///
/// All operations share the composition's leased root, capacity binder and
/// deletion owner. Catalog transactions remain the visibility authority.
pub trait RepoMapPhysicalFilePort: Send + Sync {
    fn validate_root_and_reconcile(&self, root: &Path) -> Result<(), CoreError>;

    /// Create or authenticate private directories through held nofollow FDs.
    fn ensure_private_directory(&self, path: &Path) -> Result<(), CoreError>;

    /// Publish an immutable projection under one exact-byte reservation.
    ///
    /// Reserve exact encoded bytes from the matching held parent before a staged
    /// birth. Promote without replacement, sync both parents and checked-close
    /// the original writer. An existing projection requires exact-byte replay.
    fn put_immutable(
        &self,
        path: &Path,
        bytes: &[u8],
        class: RepoMapPhysicalFileClass,
    ) -> Result<RepoMapImmutablePutOutcome, CoreError>;

    /// Return the original authenticated FD under its physical pin.
    ///
    /// Absence is
    /// returned only after pending namespace recovery. The runtime validates
    /// the canonical path and its configured class byte bound before returning
    /// a reader; consumers must explicitly retire it on success and failure.
    fn open_reader(
        &self,
        path: &Path,
    ) -> Result<Option<Box<dyn PublicationObjectReader>>, CoreError>;

    /// Delete only the reader-witnessed inode and width. A live reader retains
    /// the physical charge until checked retirement, even after namespace sync.
    fn delete_file_and_sync(
        &self,
        path: &Path,
        expected_inode: InodeKey,
        expected_length: u64,
    ) -> Result<(), CoreError>;
}
