// SPDX-License-Identifier: Apache-2.0
// SPDX-FileCopyrightText: Copyright The Lance Authors

//! Optional admission for the optimized local `file` store.
//!
//! The provider is installed in one dedicated `Session` registry. Its URI and
//! `is_local` behavior remain `file`. Tickets are deliberately non-clone; a
//! dropped ticket after a possible mutation leaves an uncertain charge for the
//! caller's inventory/recovery authority.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use lance_core::Result;
use object_store::local::LocalFileSystem;
use object_store::path::Path;
use url::Url;

use super::admitted_inner::AdmittedInnerStore;
use super::providers::local::FileStoreProvider;
use super::{ObjectStore, ObjectStoreParams, ObjectStoreProvider};

/// A per-session authority backed by the caller's existing publication ledger.
pub use object_store::local::admission::{
    FileAdmission, FileDirectoryDeleteTicket, FileDirectoryPathTicket, FileMoveMode,
    FilePathTicket, FileReaderLease, FileReaderOpenTicket, FileWriteTicket,
};

/// A reader task owns this Arc. In admitted mode the actual FD lives inside
/// the lease until the final task/stream clone drops; the common binder checks
/// its close before retiring the inode pin.
#[derive(Debug)]
pub(crate) enum ReaderFile {
    Plain(File),
    Admitted(Arc<dyn FileReaderLease>),
}

impl ReaderFile {
    pub(crate) fn file(&self) -> io::Result<&File> {
        match self {
            Self::Plain(file) => Ok(file),
            Self::Admitted(lease) => lease.file(),
        }
    }
}

/// Choose and declare a private pathname before `O_EXCL` creation. The caller
/// owns the returned FD and TempPath together through write and persist.
pub(crate) fn create_admitted_temp(
    parent: &StdPath,
    ticket: &mut dyn FileWriteTicket,
) -> io::Result<(File, tempfile::TempPath)> {
    let path: PathBuf = parent.join(format!(
        ".lance-admitted-{:032x}.tmp",
        rand::random::<u128>()
    ));
    ticket.declare_temp_path(&path)?;
    ticket.creation_started()?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    // A collision after declaration is an uncertain started attempt;
    // retrying with another name would rebind the same ticket unsafely.
    let file = options.open(&path)?;
    Ok((file, tempfile::TempPath::try_from_path(path)?))
}

/// One admitted `file` provider for a dedicated Lance Session.
#[derive(Debug, Clone)]
pub struct AdmittedFileStoreProvider {
    admission: Arc<dyn FileAdmission>,
}

impl AdmittedFileStoreProvider {
    pub fn new(admission: Arc<dyn FileAdmission>) -> Self {
        Self { admission }
    }
}

#[async_trait]
impl ObjectStoreProvider for AdmittedFileStoreProvider {
    async fn new_store(&self, base_path: Url, params: &ObjectStoreParams) -> Result<ObjectStore> {
        #[allow(deprecated)]
        if params.object_store.is_some() || params.object_store_wrapper.is_some() {
            return Err(lance_core::Error::invalid_input(
                "qualified file provider refuses custom inner store or wrapper",
            ));
        }
        let mut store = FileStoreProvider.new_store(base_path, params).await?;
        let admitted_local = LocalFileSystem::new().with_admission(Arc::clone(&self.admission));
        store.inner = Arc::new(AdmittedInnerStore::new(
            Arc::new(admitted_local),
            Arc::clone(&self.admission),
        ));
        store.file_admission = Some(Arc::clone(&self.admission));
        Ok(store)
    }

    fn requires_provider_path(&self) -> bool {
        true
    }

    fn extract_path(&self, url: &Url) -> Result<Path> {
        FileStoreProvider.extract_path(url)
    }

    fn calculate_object_store_prefix(
        &self,
        url: &Url,
        options: Option<&std::collections::HashMap<String, String>>,
    ) -> Result<String> {
        FileStoreProvider.calculate_object_store_prefix(url, options)
    }
}

/// Reuse the exact optimized local-copy admission path for generation-local
/// files copied by a dataset owner before Lance opens its connection.
pub fn copy_file_admitted(
    from: &StdPath,
    to: &StdPath,
    admission: &dyn FileAdmission,
) -> Result<()> {
    // The destination must not exist yet. `from_filesystem_path` canonicalizes
    // and rejects an absent destination; it also follows symlinks before the
    // admission authority can validate the caller's original pathname.
    let from = Path::from_absolute_path(from)?;
    let to = Path::from_absolute_path(to)?;
    crate::local::copy_file_admitted(&from, &to, admission)
}

/// Reuse the same per-file retirement path for staging/promotion cleanup.
pub fn remove_dir_all_admitted(directory: &StdPath, admission: &dyn FileAdmission) -> Result<()> {
    let directory = Path::from_absolute_path(directory)?;
    crate::local::remove_dir_all_admitted(&directory, admission)
}
