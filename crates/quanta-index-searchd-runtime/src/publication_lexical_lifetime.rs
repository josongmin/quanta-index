//! Legacy lexical object reads and GC over the composition's file binder.

use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::{
    PublicationControlOpenTicket, PublicationObjectDeleteTicket, PublicationObjectLifetimePort,
    PublicationObjectOpenTicket, PublicationObjectReader,
};
use quanta_index_core::publication_capacity::PublicationAllowance;
use quanta_index_searchd::app::state_format::STATE_ROOT_LEASE_FILE_NAME;

use crate::publication_file_capacity::{
    DeleteTicket, OpenGuard, PublicationFileCapacity, ReaderLease,
};

#[derive(Clone)]
pub struct PublicationLexicalLifetime {
    binder: Arc<PublicationFileCapacity>,
}

impl PublicationLexicalLifetime {
    #[must_use]
    pub fn new(binder: Arc<PublicationFileCapacity>) -> Self {
        Self { binder }
    }

    fn lexical_root(&self) -> PathBuf {
        self.binder.state_root().join("indexes").join("lexical")
    }

    fn require_object_path(&self, path: &Path) -> Result<(), CoreError> {
        let object_root = self
            .lexical_root()
            .join("lexical-object-pool")
            .join("objects");
        let relative = path.strip_prefix(&object_root).map_err(|_error| {
            CoreError::InvalidContract("lexical object path escapes the held pool".into())
        })?;
        let mut canonical = object_root;
        let mut has_name = false;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(CoreError::InvalidContract(
                    "lexical object path has a non-normal component".into(),
                ));
            };
            canonical.push(name);
            has_name = true;
        }
        if !has_name || canonical.as_os_str() != path.as_os_str() {
            return Err(CoreError::InvalidContract(
                "lexical object path is not canonical beneath the held pool".into(),
            ));
        }
        Ok(())
    }
}

fn io_error(error: CoreError) -> io::Error {
    match error {
        absent @ CoreError::NotFound(_) => io::Error::new(io::ErrorKind::NotFound, absent),
        storage @ CoreError::StorageIo { kind, .. } => io::Error::new(kind, storage),
        other @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::Storage(_)) => io::Error::other(other),
    }
}

fn storage_io(context: &str, error: impl Into<io::Error>) -> CoreError {
    let error = error.into();
    CoreError::storage_io(context, &error)
}

struct ObjectOpen(OpenGuard);

impl PublicationObjectOpenTicket for ObjectOpen {
    fn opened_owned(
        self: Box<Self>,
        file: File,
    ) -> Result<Box<dyn PublicationObjectReader>, CoreError> {
        let Self(guard) = *self;
        Ok(Box::new(ObjectReader(guard.opened_owned(file)?)))
    }
}

struct ObjectReader(ReaderLease);

impl Read for ObjectReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut file = self.0.file().map_err(io_error)?;
        file.read(buffer)
    }
}

impl Seek for ObjectReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let mut file = self.0.file().map_err(io_error)?;
        file.seek(position)
    }
}

impl PublicationObjectReader for ObjectReader {
    fn metadata(&self) -> Result<Metadata, CoreError> {
        self.0
            .file()?
            .metadata()
            .map_err(|error| storage_io("lexical object reader metadata", error))
    }

    fn sync_posix(&self) -> Result<(), CoreError> {
        rustix::fs::fsync(self.0.file()?)
            .map_err(|error| storage_io("lexical object reader POSIX fsync", error))
    }

    fn retire(self: Box<Self>) -> Result<(), CoreError> {
        let Self(reader) = *self;
        reader.retire()
    }
}

struct ObjectDelete(DeleteTicket);

impl PublicationObjectDeleteTicket for ObjectDelete {
    fn source_metadata(&self) -> Result<(Metadata, Metadata), CoreError> {
        self.0.source_metadata()
    }

    fn deletion_started(&mut self) -> Result<(), CoreError> {
        self.0.deletion_started()
    }

    fn unlinked_and_synced(&mut self) -> Result<(), CoreError> {
        self.0.unlinked_and_synced()
    }

    fn confirmed_unchanged_after_failure(&mut self) -> Result<(), CoreError> {
        self.0.confirmed_unchanged_after_failure()
    }

    fn has_started(&self) -> bool {
        self.0.has_started()
    }

    fn is_settled(&self) -> bool {
        self.0.is_settled()
    }
}

impl PublicationObjectLifetimePort for PublicationLexicalLifetime {
    fn validate_state_root(&self, root: &Path) -> Result<(), CoreError> {
        if root.as_os_str() != self.lexical_root().as_os_str() {
            return Err(CoreError::InvalidContract(
                "lexical object root differs from the held file authority".into(),
            ));
        }
        // Anchoring the lease entry revalidates the pinned root and its
        // current lock identity without creating a file or claiming a path.
        if self
            .binder
            .path_observation(&self.binder.state_root().join(STATE_ROOT_LEASE_FILE_NAME))?
            .is_none()
        {
            return Err(CoreError::InvalidContract(
                "state-root lease entry absent".into(),
            ));
        }
        Ok(())
    }

    fn validate_parent(&self, parent: &Arc<PublicationAllowance>) -> Result<(), CoreError> {
        self.binder.validate_held_allowance(parent)
    }

    fn begin_control_reader_open(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationControlOpenTicket>, CoreError> {
        self.validate_state_root(&self.lexical_root())?;
        crate::publication_control_files::require_control_reader_path(&self.binder, path)?;
        Ok(crate::publication_control_files::cold_control_open(
            self.binder.begin_reader_open(path)?,
        ))
    }

    fn begin_reader_open(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationObjectOpenTicket>, CoreError> {
        self.require_object_path(path)?;
        Ok(Box::new(ObjectOpen(self.binder.begin_reader_open(path)?)))
    }

    fn begin_delete(
        &self,
        path: &Path,
    ) -> Result<Box<dyn PublicationObjectDeleteTicket>, CoreError> {
        self.require_object_path(path)?;
        Ok(Box::new(ObjectDelete(self.binder.begin_delete(path)?)))
    }
}

#[cfg(test)]
mod tests;
