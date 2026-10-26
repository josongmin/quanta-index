//! Bounded in-place Control updates under the held publication allowance.

use std::fs::{File, Metadata};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path};
use std::sync::Arc;

use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_core::CoreError;
use quanta_index_core::publication_admission::{
    PublicationControlFileMutation, PublicationControlFileUpdatePort, PublicationControlOpenTicket,
    PublicationControlOutputAdmission, PublicationControlReader, PublicationObjectLifetimePort,
    validate_control_object_lifetime,
};
use rusqlite::ffi::quanta_capacity_vfs::close_owned_file;

use crate::publication_file_capacity::{
    FileMutation, OpenGuard, PublicationFileCapacity, ReaderLease,
};
use crate::publication_file_deletions::PublicationFileDeletions;

fn contract(message: &str) -> CoreError {
    CoreError::InvalidContract(format!("publication Control file update: {message}"))
}

fn storage(context: &str, error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!(
        "publication Control file update {context}: {error}"
    ))
}

fn storage_io(context: &str, error: impl Into<io::Error>) -> CoreError {
    let error = error.into();
    CoreError::storage_io(
        &format!("publication Control file update {context}"),
        &error,
    )
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

fn bound_error() -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::IngestResourceBudgetExceeded,
        message: "publication Control file update exceeds held target width".into(),
    }
}

fn with_close_context(mut primary: CoreError, close: impl std::fmt::Display) -> CoreError {
    let context = format!("; original writer FD close failed: {close}");
    let message = match &mut primary {
        CoreError::InvalidContract(message)
        | CoreError::NotReady(message)
        | CoreError::NotImplemented(message)
        | CoreError::NotFound(message)
        | CoreError::Storage(message)
        | CoreError::StorageIo { message, .. }
        | CoreError::Typed { message, .. } => message,
    };
    message.push_str(&context);
    primary
}

/// One exact binder and lifetime capability for existing Control files.
pub struct PublicationControlFiles {
    binder: Arc<PublicationFileCapacity>,
    deletions: Arc<PublicationFileDeletions>,
    lifetime: Arc<dyn PublicationObjectLifetimePort>,
}

impl PublicationControlFiles {
    #[must_use]
    pub fn new(
        deletions: Arc<PublicationFileDeletions>,
        lifetime: Arc<dyn PublicationObjectLifetimePort>,
    ) -> Self {
        Self {
            binder: deletions.binder(),
            deletions,
            lifetime,
        }
    }

    fn close_rejected_writer(&self, writer: File, primary: CoreError) -> CoreError {
        match close_owned_file(writer) {
            Ok(()) => primary,
            Err(error) => {
                self.binder.mark_close_unwitnessed();
                with_close_context(primary, error)
            }
        }
    }

    fn require_control_path(&self, path: &Path) -> Result<(), CoreError> {
        require_control_path(&self.binder, path)
    }

    fn require_reader_path(&self, path: &Path) -> Result<(), CoreError> {
        require_control_reader_path(&self.binder, path)
    }
}

fn require_control_path(binder: &PublicationFileCapacity, path: &Path) -> Result<(), CoreError> {
    let pool = binder
        .state_root()
        .join("indexes/lexical/lexical-object-pool");
    if path.parent() == Some(pool.as_path())
        && let Some(name) = path.file_name().and_then(|name| name.to_str())
        && is_pool_control_temporary(name)
        && pool.join(name).as_os_str() == path.as_os_str()
    {
        return Ok(());
    }
    let attempts = binder
        .state_root()
        .join("indexes/lexical/lexical-object-pool/attempts");
    let leaf = path
        .file_name()
        .ok_or_else(|| contract("Control journal leaf is absent"))?;
    if !matches!(path.components().next_back(), Some(Component::Normal(_)))
        || path.parent() != Some(attempts.as_path())
        || attempts.join(leaf).as_os_str() != path.as_os_str()
    {
        return Err(contract(
            "Control file path is not a canonical direct attempt entry",
        ));
    }
    Ok(())
}

/// Apply the same canonical Control reader whitelist during cold and writer opens.
pub(crate) fn require_control_reader_path(
    binder: &PublicationFileCapacity,
    path: &Path,
) -> Result<(), CoreError> {
    let pool = binder
        .state_root()
        .join("indexes/lexical/lexical-object-pool");
    if path == pool.join(".lifecycle-gate") || path == pool.join(".lifecycle-epoch") {
        return Ok(());
    }
    require_control_path(binder, path)
}

fn is_pool_control_temporary(name: &str) -> bool {
    let suffix = name
        .strip_prefix("..lifecycle-gate.tmp-")
        .or_else(|| name.strip_prefix("..lifecycle-epoch.tmp-"));
    let Some((pid, sequence)) = suffix.and_then(|suffix| suffix.split_once('-')) else {
        return false;
    };
    !pid.is_empty()
        && !sequence.is_empty()
        && pid.bytes().all(|byte| byte.is_ascii_digit())
        && sequence.bytes().all(|byte| byte.is_ascii_digit())
}

impl PublicationControlFileUpdatePort for PublicationControlFiles {
    fn begin_reader_open(
        &self,
        path: &Path,
        control: PublicationControlOutputAdmission,
    ) -> Result<Box<dyn PublicationControlOpenTicket>, CoreError> {
        validate_control_object_lifetime(Some(&control), Some(&self.lifetime))?;
        self.lifetime
            .validate_state_root(&self.binder.state_root().join("indexes/lexical"))?;
        self.require_reader_path(path)?;
        Ok(Box::new(ControlOpen {
            guard: self.binder.begin_reader_open(path)?,
            control: Some(control),
        }))
    }

    fn reconcile_deletions(&self) -> Result<(), CoreError> {
        self.deletions.reconcile_retired_control_creates()
    }

    fn remove_file_and_sync(
        &self,
        path: &Path,
        control: PublicationControlOutputAdmission,
    ) -> Result<(), CoreError> {
        validate_control_object_lifetime(Some(&control), Some(&self.lifetime))?;
        self.lifetime
            .validate_state_root(&self.binder.state_root().join("indexes/lexical"))?;
        self.require_control_path(path)?;
        let _parent = control
            .held_file_allowance()?
            .ok_or_else(|| contract("held Control file allowance is absent"))?;
        self.deletions.retry_pending_deletion()?;
        let Some(expected) = self.binder.path_observation(path)? else {
            return Ok(());
        };
        self.deletions
            .delete_owned_file_and_sync(path, expected.inode, expected.bytes)
    }

    fn begin_update(
        &self,
        path: &Path,
        writer: File,
        expected_length: u64,
        target_end: u64,
        control: PublicationControlOutputAdmission,
    ) -> Result<Box<dyn PublicationControlFileMutation>, CoreError> {
        let mut writer = Some(writer);
        let result = (|| {
            validate_control_object_lifetime(Some(&control), Some(&self.lifetime))?;
            self.lifetime
                .validate_state_root(&self.binder.state_root().join("indexes/lexical"))?;
            self.require_control_path(path)?;
            let parent = control
                .held_file_allowance()?
                .ok_or_else(|| contract("held Control file allowance is absent"))?;
            let actual = writer
                .as_ref()
                .ok_or_else(|| contract("original writer FD is absent"))?;
            let metadata = actual
                .metadata()
                .map_err(|error| storage_io("stat original writer", error))?;
            if !metadata.is_file() || metadata.len() != expected_length {
                return Err(contract(
                    "original writer differs from expected journal width",
                ));
            }
            if self.binder.has_retired_safe_resize_pending(path)? {
                // The previous original writer has already been checked closed.
                // This separate FD is consumed by recovery; the new original
                // remains untouched until its own resize ticket is reserved.
                let witness = actual
                    .try_clone()
                    .map_err(|error| storage_io("clone pending recovery witness", error))?;
                let _retained = self.binder.recover_pending_with_fd(path, path, witness)?;
            }
            let mut mutation = self
                .binder
                .begin_resize_with_allowance(path, actual, target_end, parent)?;
            // This precedes every write, set_len, and caller-visible handle.
            mutation.mutation_started()?;
            let update: Box<dyn PublicationControlFileMutation> = Box::new(ControlFileMutation {
                binder: Arc::clone(&self.binder),
                writer: writer.take(),
                mutation: Some(mutation),
                target_end,
                _control: control,
            });
            Ok(update)
        })();
        match result {
            Ok(update) => Ok(update),
            Err(primary) => Err(match writer.take() {
                Some(writer) => self.close_rejected_writer(writer, primary),
                None => primary,
            }),
        }
    }
}

struct ControlOpen {
    guard: OpenGuard,
    control: Option<PublicationControlOutputAdmission>,
}

/// Transfer a cold reader-open claim without creating a Control writer allowance.
pub(crate) fn cold_control_open(guard: OpenGuard) -> Box<dyn PublicationControlOpenTicket> {
    Box::new(ControlOpen {
        guard,
        control: None,
    })
}

impl PublicationControlOpenTicket for ControlOpen {
    fn opened_owned(
        self: Box<Self>,
        file: File,
    ) -> Result<Box<dyn PublicationControlReader>, CoreError> {
        let Self { guard, control } = *self;
        Ok(Box::new(ControlReader {
            reader: guard.opened_owned(file)?,
            _control: control,
        }))
    }
}

struct ControlReader {
    reader: ReaderLease,
    _control: Option<PublicationControlOutputAdmission>,
}

impl Read for ControlReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut file = self.reader.file().map_err(io_error)?;
        file.read(buffer)
    }
}

impl Seek for ControlReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let mut file = self.reader.file().map_err(io_error)?;
        file.seek(position)
    }
}

impl PublicationControlReader for ControlReader {
    fn metadata(&self) -> Result<Metadata, CoreError> {
        self.reader
            .file()?
            .metadata()
            .map_err(|error| storage_io("reader metadata", error))
    }

    fn sync_posix(&self) -> Result<(), CoreError> {
        rustix::fs::fsync(self.reader.file()?).map_err(|error| storage_io("reader fsync", error))
    }

    fn lock_exclusive(&self) -> Result<(), CoreError> {
        rustix::fs::flock(
            self.reader.file()?,
            rustix::fs::FlockOperation::LockExclusive,
        )
        .map_err(|error| storage_io("reader exclusive lock", error))
    }

    fn try_lock_exclusive(&self) -> Result<bool, CoreError> {
        match rustix::fs::flock(
            self.reader.file()?,
            rustix::fs::FlockOperation::NonBlockingLockExclusive,
        ) {
            Ok(()) => Ok(true),
            Err(rustix::io::Errno::WOULDBLOCK) => Ok(false),
            Err(error) => Err(storage_io("reader try exclusive lock", error)),
        }
    }

    fn retire(self: Box<Self>) -> Result<(), CoreError> {
        let Self { reader, _control } = *self;
        reader.retire()
    }
}

struct ControlFileMutation {
    binder: Arc<PublicationFileCapacity>,
    writer: Option<File>,
    mutation: Option<FileMutation>,
    target_end: u64,
    // Retain the exact parent until the resize settles or becomes pending.
    _control: PublicationControlOutputAdmission,
}

impl ControlFileMutation {
    fn file_mut(&mut self) -> io::Result<&mut File> {
        self.writer
            .as_mut()
            .ok_or_else(|| io::Error::other(contract("original writer FD retired")))
    }

    fn mutation_mut(&mut self) -> Result<&mut FileMutation, CoreError> {
        self.mutation
            .as_mut()
            .ok_or_else(|| contract("resize ticket absent"))
    }
}

impl Read for ControlFileMutation {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file_mut()?.read(buf)
    }
}

impl Seek for ControlFileMutation {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file_mut()?.seek(pos)
    }
}

impl Write for ControlFileMutation {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let end = self
            .file_mut()?
            .stream_position()?
            .checked_add(
                u64::try_from(buf.len())
                    .map_err(|error| io::Error::other(storage("write width", error)))?,
            )
            .ok_or_else(|| io::Error::other(bound_error()))?;
        if end > self.target_end {
            return Err(io::Error::other(bound_error()));
        }
        self.file_mut()?.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file_mut()?.flush()
    }
}

impl PublicationControlFileMutation for ControlFileMutation {
    fn metadata(&self) -> Result<Metadata, CoreError> {
        self.writer
            .as_ref()
            .ok_or_else(|| contract("original writer FD retired"))?
            .metadata()
            .map_err(|error| storage_io("writer metadata", error))
    }

    fn set_len(&mut self, len: u64) -> Result<(), CoreError> {
        if len > self.target_end {
            return Err(bound_error());
        }
        self.writer
            .as_ref()
            .ok_or_else(|| contract("original writer FD retired"))?
            .set_len(len)
            .map_err(|error| storage_io("set_len", error))
    }

    fn sync_and_settle(mut self: Box<Self>) -> Result<(), CoreError> {
        let actual = self
            .writer
            .as_ref()
            .ok_or_else(|| contract("original writer FD retired"))?;
        let actual_length = actual
            .metadata()
            .map_err(|error| storage_io("stat original writer", error))?
            .len();
        if actual_length != self.target_end {
            return Err(contract("final file width differs from reserved target"));
        }
        let _retained = self
            .mutation
            .as_mut()
            .ok_or_else(|| contract("resize ticket absent"))?
            .settle_after_file_and_parent_sync(actual)?;
        let writer = self
            .writer
            .take()
            .ok_or_else(|| contract("original writer FD retired"))?;
        self.mutation_mut()?
            .confirmed_safe_writer_fd_retired(writer)?;
        let lease = self.mutation_mut()?.take_owned_lease()?;
        drop(self.mutation.take());
        lease.retire()
    }

    fn retire_preserving_pending(mut self: Box<Self>) -> Result<(), CoreError> {
        let writer = self
            .writer
            .take()
            .ok_or_else(|| contract("original writer FD retired"))?;
        self.mutation_mut()?
            .confirmed_safe_writer_fd_retired(writer)
    }
}

impl Drop for ControlFileMutation {
    fn drop(&mut self) {
        if let Some(writer) = self.writer.take() {
            if let Some(mutation) = self.mutation.as_mut() {
                let _retirement = mutation.confirmed_safe_writer_fd_retired(writer);
            } else if close_owned_file(writer).is_err() {
                self.binder.mark_close_unwitnessed();
            }
        }
        // The started mutation's Drop retains its pending ticket on errors.
    }
}

#[cfg(test)]
#[path = "publication_control_files/tests.rs"]
mod tests;
