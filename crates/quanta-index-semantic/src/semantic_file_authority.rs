//! File-authority contracts shared by the Legacy adapter and its leaf modules.

use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::{
    PublicationObjectLifetimePort, PublicationObjectReader,
};
use quanta_index_core::publication_capacity::PublicationAllowance;

/// Bind the held work allowance to the target generation and declared base.
///
/// The composition root supplies one factory backed by
/// its canonical file binder; this crate never creates a capacity ledger.
pub trait SemanticFileAdmissionFactory: Send + Sync {
    /// Cold sealed-control reads use the canonical physical file authority.
    fn control_reader_lifetime(&self) -> Arc<dyn PublicationObjectLifetimePort>;

    fn for_work(
        &self,
        held_work: Arc<PublicationAllowance>,
        generation_dir: &Path,
        base_generation_dir: Option<&Path>,
    ) -> Result<Arc<dyn SemanticGenerationFileIo>, CoreError>;
}

/// File operations for one target generation and its declared base.
/// Recovery reconciles the same physical tickets as promotion.
pub trait SemanticGenerationFileIo: Send + Sync {
    fn reconcile_namespace(&self) -> Result<(), CoreError>;
    fn prepare_entry_parent(&self, target: &Path) -> Result<(), CoreError>;
    fn prepare_staging_directory(&self, directory: &Path) -> Result<(), CoreError>;
    fn link_inherited_file(&self, source: &Path, target: &Path) -> Result<(), CoreError>;
    fn copy_inherited_file(&self, source: &Path, target: &Path) -> Result<(), CoreError>;
    fn begin_move(
        &self,
        source: &Path,
        target: &Path,
    ) -> Result<Box<dyn SemanticFileMove>, CoreError>;
    fn remove_file_and_sync(&self, path: &Path) -> Result<(), CoreError>;
    /// Open a file in this build's staging dataset or its declared base.
    fn open_work_dataset_reader(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationObjectReader>, CoreError>;
    /// Inspect a directory within this build's staging dataset or declared base.
    fn list_work_dataset(
        &self,
        prefix: &Path,
        recursive: bool,
    ) -> Result<Vec<SemanticWorkListEntry>, CoreError>;
    /// Publish a complete staging file, with `None` for an existing target in create mode.
    fn put_staging_file(
        &self,
        path: &Path,
        mode: SemanticStagingTargetMode,
        chunks: &[&[u8]],
    ) -> Result<Option<Metadata>, CoreError>;
    fn begin_staging_upload(
        &self,
        path: &Path,
    ) -> Result<Box<dyn SemanticStagingUpload>, CoreError>;
    fn copy_staging_file(
        &self,
        source: &Path,
        target: &Path,
        mode: SemanticStagingTargetMode,
    ) -> Result<(), CoreError>;
    fn rename_staging_file(
        &self,
        source: &Path,
        target: &Path,
        mode: SemanticStagingTargetMode,
    ) -> Result<(), CoreError>;
    fn remove_staging_file_and_sync(&self, path: &Path) -> Result<(), CoreError>;
    fn remove_directory_and_sync(&self, path: &Path) -> Result<(), CoreError>;
}

/// A pathname and its witnessed metadata inside the held work dataset.
pub enum SemanticWorkListEntry {
    File {
        path: PathBuf,
        inode: u64,
        bytes: u64,
        modified: SystemTime,
    },
    Directory(PathBuf),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticStagingTargetMode {
    Create,
    Overwrite,
}

/// One private replacement file. Part reservations are in submission order.
pub trait SemanticStagingUpload: Send {
    fn reserve_part(&mut self, bytes: u64) -> Result<Box<dyn SemanticStagingPart>, CoreError>;
    fn complete(self: Box<Self>) -> Result<Metadata, CoreError>;
    fn abort(self: Box<Self>) -> Result<(), CoreError>;
}

/// A single-use grant for the reserved byte range of one upload part.
pub trait SemanticStagingPart: Send {
    fn write(self: Box<Self>, chunks: &[&[u8]]) -> Result<(), CoreError>;
}

/// One reserved move under the product's physical file authority.
/// A started failure remains owned by that authority for reconciliation.
pub trait SemanticFileMove: Send {
    fn perform_and_sync(&mut self) -> Result<(), CoreError>;
}

/// Reserve maintenance writes under the global capacity authority.
///
/// A receipt written outside a publication obtains one independent parent.
/// The returned child must be
/// backed by the same canonical physical file binder as Lance writes.
pub trait SemanticMaintenanceFileFactory: Send + Sync {
    fn reserve_sidecar_output(
        &self,
        max_encoded_bytes: u64,
    ) -> Result<Box<dyn quanta_index_core::publication_admission::PublicationOutputPermit>, CoreError>;

    /// Reconcile the original physical namespace ticket before another reclaim.
    fn reconcile_retirement_namespace(&self) -> Result<(), CoreError>;

    /// Reserve one directory move into the private reclaim area. The returned
    /// ticket holds the original physical namespace operation through sync.
    fn begin_retirement_directory_move(
        &self,
        source: &Path,
        target: &Path,
    ) -> Result<Box<dyn SemanticFileMove>, CoreError>;

    /// Delete one directory tree under the same physical binder and sync it.
    fn delete_retirement_directory_and_sync(&self, path: &Path) -> Result<(), CoreError>;

    /// Prepare only the private reclaim-area parent for a named entry.
    fn prepare_retirement_area(&self, entry: &Path) -> Result<(), CoreError>;
}
