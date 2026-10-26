//! Admit Lance local file I/O through the canonical binder.
//!
//! The `PublicationFileCapacity` binder owns the capacity ledger. Compilation
//! and runtime qualification require the collected repository verification.

use std::fmt;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lance_io::object_store::file_admission::{
    FileAdmission, FileDirectoryDeleteTicket, FileDirectoryPathTicket, FileMoveMode,
    FilePathTicket, FileReaderLease, FileReaderOpenTicket, FileWriteTicket,
};
use quanta_index_core::CoreError;
use quanta_index_core::publication_capacity::PublicationAllowance;

use crate::publication_file_capacity::{
    FileMutation, OpenGuard, PublicationFileCapacity, ReaderLease,
};
use crate::publication_file_deletions::{OwnedNamespaceTicket, PublicationFileDeletions};

fn io_error(error: CoreError) -> io::Error {
    // Keep the typed CoreError in the source chain. Semantic's public mapper
    // downcasts the real cause; ENOSPC and lookalike strings stay Storage.
    match error {
        absent @ CoreError::NotFound(_) => io::Error::new(io::ErrorKind::NotFound, absent),
        observed @ CoreError::StorageIo { kind, .. } => io::Error::new(kind, observed),
        other @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::Storage(_)) => io::Error::other(other),
    }
}

#[derive(Clone)]
pub struct PublicationLanceAdmission {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
    scope: AdmissionScope,
}

#[derive(Clone)]
enum AdmissionScope {
    ReadOnly,
    Work(Arc<PublicationAllowance>),
    RetirementOnly,
}

impl PublicationLanceAdmission {
    #[must_use]
    pub fn read_only(deletions: Arc<PublicationFileDeletions>) -> Self {
        Self {
            binder: deletions.binder(),
            deletions,
            scope: AdmissionScope::ReadOnly,
        }
    }

    #[must_use]
    pub fn retirement_only(deletions: Arc<PublicationFileDeletions>) -> Self {
        Self {
            binder: deletions.binder(),
            deletions,
            scope: AdmissionScope::RetirementOnly,
        }
    }

    /// The caller must pass its held work parent, never a newly reserved
    /// allowance. The binder authenticates ledger identity at child creation.
    ///
    /// # Errors
    /// Returns a contract error if the parent belongs to another authority or
    /// has already closed.
    pub fn for_work(
        deletions: Arc<PublicationFileDeletions>,
        held_work: Arc<PublicationAllowance>,
    ) -> Result<Self, CoreError> {
        let binder = deletions.binder();
        binder.validate_held_allowance(&held_work)?;
        Ok(Self {
            binder,
            deletions,
            scope: AdmissionScope::Work(held_work),
        })
    }

    fn work_parent(&self) -> io::Result<Arc<PublicationAllowance>> {
        match &self.scope {
            AdmissionScope::Work(parent) => {
                self.binder
                    .validate_held_allowance(parent)
                    .map_err(io_error)?;
                Ok(Arc::clone(parent))
            }
            AdmissionScope::ReadOnly | AdmissionScope::RetirementOnly => {
                Err(io_error(CoreError::InvalidContract(
                    "Lance admission scope refuses filesystem creation or growth".into(),
                )))
            }
        }
    }

    fn retirement_allowed(&self) -> io::Result<()> {
        match &self.scope {
            AdmissionScope::Work(_) => {
                let _parent = self.work_parent()?;
                Ok(())
            }
            AdmissionScope::RetirementOnly => Ok(()),
            AdmissionScope::ReadOnly => Err(io_error(CoreError::InvalidContract(
                "read-only Lance admission refuses filesystem mutation".into(),
            ))),
        }
    }
}

impl fmt::Debug for PublicationLanceAdmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublicationLanceAdmission")
            .field("state_root", &self.binder.state_root())
            .finish_non_exhaustive()
    }
}

struct WriteCustody {
    mutation: FileMutation,
    // Retire only after the producing vendor code drops its own writer FD.
    // The binder keeps this distinct physical FD until this lease drops.
    settled_lease: Option<ReaderLease>,
    final_path: PathBuf,
}

impl fmt::Debug for WriteCustody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WriteCustody")
            .field("final_path", &self.final_path)
            .field("pending_id", &self.mutation.pending_id())
            .finish_non_exhaustive()
    }
}

impl FileWriteTicket for WriteCustody {
    fn declare_temp_path(&mut self, path: &Path) -> io::Result<()> {
        self.mutation.declare_temp_path(path).map_err(io_error)
    }

    fn creation_started(&mut self) -> io::Result<()> {
        self.mutation.creation_started().map_err(io_error)
    }

    fn created(&mut self, file: &File, temporary_path: &Path) -> io::Result<()> {
        self.mutation
            .created(file, temporary_path)
            .map_err(io_error)
    }

    fn reserve_end(&mut self, candidate_end: u64) -> io::Result<()> {
        self.mutation.reserve_end(candidate_end).map_err(io_error)
    }

    fn before_replace(&mut self, final_path: &Path) -> io::Result<()> {
        self.mutation.before_replace(final_path).map_err(io_error)
    }

    fn replaced_and_synced(&mut self, file: &File, final_path: &Path) -> io::Result<()> {
        let _retained = self
            .mutation
            .replaced_and_synced(file, final_path)
            .map_err(io_error)?;
        // The binder retains this identity; the caller's writer FD is still
        // live until confirmed_writer_fd_retired transfers the owned lease.
        Ok(())
    }

    fn confirmed_writer_fd_retired(&mut self, writer: File) -> io::Result<()> {
        self.mutation
            .confirmed_safe_writer_fd_retired(writer)
            .map_err(io_error)?;
        self.settled_lease = Some(self.mutation.take_owned_lease().map_err(io_error)?);
        Ok(())
    }

    fn confirmed_auxiliary_writer_fd_retired(&mut self, writer: File) -> io::Result<()> {
        self.mutation
            .confirmed_auxiliary_writer_fd_retired(writer)
            .map_err(io_error)
    }

    fn confirmed_uncreated_absent_and_synced(&mut self) -> io::Result<()> {
        self.mutation
            .confirmed_uncreated_absent_and_synced()
            .map_err(io_error)
    }

    fn confirmed_safe_absent_after_writer_close(&mut self, writer: File) -> io::Result<()> {
        self.mutation
            .confirmed_safe_absent_after_writer_close(writer)
            .map_err(io_error)
    }
}

struct PathCustody(OwnedNamespaceTicket);
impl fmt::Debug for PathCustody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PathCustody")
    }
}
impl FilePathTicket for PathCustody {
    fn perform_and_sync(&mut self) -> io::Result<()> {
        self.0.perform_and_sync().map_err(io_error)
    }
}

struct DirectoryMoveCustody(OwnedNamespaceTicket);
impl fmt::Debug for DirectoryMoveCustody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DirectoryMoveCustody")
    }
}
impl FileDirectoryPathTicket for DirectoryMoveCustody {
    fn perform_and_sync(&mut self) -> io::Result<()> {
        self.0.perform_and_sync().map_err(io_error)
    }
}

struct DirectoryDeleteCustody(OwnedNamespaceTicket);
impl fmt::Debug for DirectoryDeleteCustody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DirectoryDeleteCustody")
    }
}
impl FileDirectoryDeleteTicket for DirectoryDeleteCustody {
    fn perform_and_sync(&mut self) -> io::Result<()> {
        self.0.perform_and_sync().map_err(io_error)
    }
}
struct ReadOpenCustody(Option<OpenGuard>);
impl fmt::Debug for ReadOpenCustody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReadOpenCustody")
    }
}
struct ReadLeaseCustody(ReaderLease);
impl fmt::Debug for ReadLeaseCustody {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReadLeaseCustody")
    }
}
impl FileReaderLease for ReadLeaseCustody {
    fn file(&self) -> io::Result<&File> {
        self.0.file().map_err(io_error)
    }
}
impl FileReaderOpenTicket for ReadOpenCustody {
    fn opened_owned(&mut self, file: File) -> io::Result<Arc<dyn FileReaderLease>> {
        let guard = self
            .0
            .take()
            .ok_or_else(|| io::Error::other("reader open reused"))?;
        Ok(Arc::new(ReadLeaseCustody(
            guard.opened_owned(file).map_err(io_error)?,
        )))
    }
}

impl FileAdmission for PublicationLanceAdmission {
    fn validate_list_scope(&self, path: &Path) -> io::Result<()> {
        self.deletions
            .reconcile_pending_namespace()
            .map_err(io_error)?;
        self.binder.validate_list_scope(path).map_err(io_error)
    }
    fn reconcile_pending_namespace(&self) -> io::Result<()> {
        self.deletions
            .reconcile_pending_namespace()
            .map_err(io_error)
    }
    fn prepare_parent_dirs(&self, final_path: &Path) -> io::Result<()> {
        let _work = self.work_parent()?;
        self.binder
            .ensure_private_parent_directories(final_path)
            .map_err(io_error)
    }

    fn begin_create(&self, final_path: &Path) -> io::Result<Box<dyn FileWriteTicket>> {
        let parent = self.work_parent()?;
        self.deletions
            .reconcile_pending_namespace()
            .map_err(io_error)?;
        Ok(Box::new(WriteCustody {
            mutation: self
                .binder
                .begin_create_with_allowance(final_path, 1, parent)
                .map_err(io_error)?,
            settled_lease: None,
            final_path: final_path.to_owned(),
        }))
    }

    fn begin_byte_copy(
        &self,
        source: &File,
        final_path: &Path,
    ) -> io::Result<Box<dyn FileWriteTicket>> {
        let parent = self.work_parent()?;
        Ok(Box::new(WriteCustody {
            mutation: self
                .binder
                .begin_byte_copy_with_allowance(source, final_path, parent)
                .map_err(io_error)?,
            settled_lease: None,
            final_path: final_path.to_owned(),
        }))
    }

    fn begin_link(&self, source: &Path, target: &Path) -> io::Result<Box<dyn FilePathTicket>> {
        let _work = self.work_parent()?;
        Ok(Box::new(PathCustody(
            self.deletions
                .begin_link(source, target)
                .map_err(io_error)?,
        )))
    }

    fn begin_move(
        &self,
        source: &Path,
        target: &Path,
        mode: FileMoveMode,
    ) -> io::Result<Box<dyn FilePathTicket>> {
        self.retirement_allowed()?;
        let ticket = match mode {
            FileMoveMode::Create => self.deletions.begin_move_if_absent(source, target),
            FileMoveMode::Overwrite => self.deletions.begin_move(source, target),
            FileMoveMode::OverwriteFromAlias => {
                let _work = self.work_parent()?;
                self.deletions.begin_staged_move(source, target)
            }
        }
        .map_err(io_error)?;
        Ok(Box::new(PathCustody(ticket)))
    }

    fn delete_file_and_sync(&self, path: &Path) -> io::Result<()> {
        self.retirement_allowed()?;
        self.deletions.delete_file_and_sync(path).map_err(io_error)
    }

    fn begin_directory_move(
        &self,
        source: &Path,
        target: &Path,
    ) -> io::Result<Box<dyn FileDirectoryPathTicket>> {
        self.retirement_allowed()?;
        Ok(Box::new(DirectoryMoveCustody(
            self.deletions
                .begin_directory_move(source, target)
                .map_err(io_error)?,
        )))
    }

    fn begin_directory_delete(
        &self,
        path: &Path,
    ) -> io::Result<Box<dyn FileDirectoryDeleteTicket>> {
        self.retirement_allowed()?;
        Ok(Box::new(DirectoryDeleteCustody(
            self.deletions
                .begin_directory_delete(path)
                .map_err(io_error)?,
        )))
    }

    fn begin_reader_open(&self, path: &Path) -> io::Result<Box<dyn FileReaderOpenTicket>> {
        self.deletions
            .reconcile_pending_namespace()
            .map_err(io_error)?;
        Ok(Box::new(ReadOpenCustody(Some(
            self.binder.begin_reader_open(path).map_err(io_error)?,
        ))))
    }
}

#[cfg(test)]
mod tests;
