//! Catalog lock files belong to the publication file authority, not SQLite.

use std::{fs::File, os::unix::fs::MetadataExt as _, path::Path, sync::Arc};

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::{
    CoreError, RequestBudgetV1,
    index::{IndexError, ObjectReadFile, ObjectReadIoPort as _, WorkBudget, WorkLimits},
    publication_capacity::PublicationAllowance,
};

use crate::{
    native_object_read_io::NativeObjectReadIo, publication_file_capacity::PublicationFileCapacity,
};

pub(crate) fn catalog_file_error(error: &CoreError) -> IndexError {
    if matches!(
        error,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
            ..
        }
    ) {
        IndexError::ResourceLimited
    } else {
        IndexError::Storage(format!("Catalog regular-file I/O: {error}"))
    }
}

/// Create and open a Catalog lock under the shared publication authority.
/// The returned read owner retains its FD lease through close.
pub(crate) fn open(
    common: Arc<PublicationFileCapacity>,
    path: &Path,
    create_new: bool,
    parent: Option<Arc<PublicationAllowance>>,
) -> Result<Box<dyn ObjectReadFile>, IndexError> {
    if create_new {
        common
            .prepare_empty_lock_with_parent(path, true, parent)
            .map_err(|error| catalog_file_error(&error))?;
    }
    // Session creation has no caller WorkBudget. The process-lived owner is
    // bounded by the common handle and registry ceilings.
    let mut budget = WorkBudget::with_request(
        WorkLimits {
            reads: 0,
            writes: 0,
            bytes: 0,
            mutations: 0,
        },
        RequestBudgetV1::unbounded(),
    );
    NativeObjectReadIo::new(common).open(path, &mut budget)
}

/// Only the held inode may be removed. The authority performs the actual
/// unlink and parent sync, then retires its tracked charge conservatively.
pub(crate) fn unlink(
    common: &PublicationFileCapacity,
    path: &Path,
    held: &File,
) -> Result<(), IndexError> {
    let mut deletion = common
        .begin_delete(path)
        .map_err(|error| catalog_file_error(&error))?;
    let (source, _parent) = deletion
        .source_metadata()
        .map_err(|error| catalog_file_error(&error))?;
    let expected = held
        .metadata()
        .map_err(|error| IndexError::Storage(format!("Catalog held lock metadata: {error}")))?;
    if source.dev() != expected.dev()
        || source.ino() != expected.ino()
        || source.len() != expected.len()
        || source.nlink() != expected.nlink()
    {
        return Err(IndexError::Storage(
            "Catalog delete source differs from held lock".into(),
        ));
    }
    deletion
        .perform_and_sync()
        .map_err(|error| catalog_file_error(&error))
}
