//! Recovery-only directory DATA checks. The caller retains the actual root lease.
pub use quanta_index_core::index::existing_file::{
    ExistingFileReadFailureV1 as RecoveryReadFailureV1,
};
pub(super) use quanta_index_core::index::existing_file::read_existing_file_v1 as read_existing;
use quanta_index_core::index::{IndexError, WorkBudget, WorkUsage};
use std::{fs, os::unix::fs::MetadataExt as _, path::Path};
#[derive(Eq, PartialEq)]
pub(super) struct Identity {
    device: u64,
    inode: u64,
    bytes: u64,
    links: u64,
    pub(super) owner: u32,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl Identity {
    fn from(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            links: metadata.nlink(),
            owner: metadata.uid(),
            mode: metadata.mode(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

fn storage(error: std::io::Error) -> RecoveryReadFailureV1 {
    quanta_index_core::CoreError::storage_io("existing activation read", &error).into()
}

fn charge(budget: &mut WorkBudget, reads: u64, bytes: u64) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads,
        bytes,
        ..WorkUsage::default()
    })
}

pub(super) fn directory(
    path: &Path,
    budget: &mut WorkBudget,
) -> Result<Identity, RecoveryReadFailureV1> {
    charge(budget, 1, 0)?;
    let metadata = fs::symlink_metadata(path).map_err(storage)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(IndexError::DigestConflict.into());
    }
    Ok(Identity::from(&metadata))
}
