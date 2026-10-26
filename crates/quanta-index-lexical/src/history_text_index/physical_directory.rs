//! Tantivy directory whose files remain under the shared physical authority.

use std::fmt;
use std::io::{self, BufWriter, Read as _, Seek as _, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use quanta_index_core::publication_admission::{
    PublicationControlFileMutation, PublicationObjectReader,
};
use quanta_index_core::{CoreError, HistoryTextFileIoPort, HistoryTextPhysicalLock};
use tantivy::HasLen;
use tantivy::TantivyError;
use tantivy::directory::error::{
    DeleteError, LockError, OpenDirectoryError, OpenReadError, OpenWriteError,
};
use tantivy::directory::{
    AntiCallToken, Directory, DirectoryLock, FileHandle, Lock, OwnedBytes, TerminatingWrite,
    WatchCallback, WatchHandle, WritePtr,
};

fn io_error(error: CoreError) -> io::Error {
    match error {
        absent @ CoreError::NotFound(_) => io::Error::new(io::ErrorKind::NotFound, absent),
        storage @ CoreError::StorageIo { kind, .. } => io::Error::new(kind, storage),
        other @ (CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::Storage(_)
        | CoreError::Typed { .. }) => io::Error::other(other),
    }
}

fn admitted_io_error(context: &str, error: &io::Error) -> CoreError {
    engine_error(context, error)
}

pub(crate) fn engine_error(context: &str, error: &(dyn std::error::Error + 'static)) -> CoreError {
    let mut cause = error;
    let mut observed_io = None;
    loop {
        if let Some(core) = cause.downcast_ref::<CoreError>() {
            return core.clone();
        }
        if let Some(io_error) = cause.downcast_ref::<io::Error>() {
            observed_io = Some(io_error);
        }
        let tantivy_payload = cause.downcast_ref::<TantivyError>().and_then(|outer| {
            let inner: &(dyn std::error::Error + 'static) = match outer {
                TantivyError::IoError(inner) => inner.as_ref(),
                TantivyError::OpenReadError(inner) => inner,
                TantivyError::OpenWriteError(inner) => inner,
                TantivyError::OpenDirectoryError(inner) => inner,
                TantivyError::LockFailure(inner, _) => inner,
                TantivyError::AggregationError(_)
                | TantivyError::IndexAlreadyExists
                | TantivyError::DataCorruption(_)
                | TantivyError::Poisoned
                | TantivyError::FieldNotFound(_)
                | TantivyError::InvalidArgument(_)
                | TantivyError::ErrorInThread(_)
                | TantivyError::IndexBuilderMissingArgument(_)
                | TantivyError::SchemaError(_)
                | TantivyError::SystemError(_)
                | TantivyError::IncompatibleIndex(_)
                | TantivyError::InternalError(_)
                | TantivyError::DeserializeError(_) => return None,
            };
            Some(inner)
        });
        let directory_io = cause
            .downcast_ref::<OpenReadError>()
            .and_then(|inner| match inner {
                OpenReadError::IoError { io_error, .. } => Some(io_error.as_ref()),
                OpenReadError::FileDoesNotExist(_) | OpenReadError::IncompatibleIndex(_) => None,
            })
            .or_else(|| {
                cause
                    .downcast_ref::<OpenWriteError>()
                    .and_then(|inner| match inner {
                        OpenWriteError::IoError { io_error, .. } => Some(io_error.as_ref()),
                        OpenWriteError::FileAlreadyExists(_) => None,
                    })
            })
            .or_else(|| {
                cause
                    .downcast_ref::<OpenDirectoryError>()
                    .and_then(|inner| match inner {
                        OpenDirectoryError::IoError { io_error, .. }
                        | OpenDirectoryError::FailedToCreateTempDir(io_error) => {
                            Some(io_error.as_ref())
                        }
                        OpenDirectoryError::DoesNotExist(_)
                        | OpenDirectoryError::NotADirectory(_) => None,
                    })
            })
            .or_else(|| {
                cause
                    .downcast_ref::<LockError>()
                    .and_then(|inner| match inner {
                        LockError::IoError(io_error) => Some(io_error.as_ref()),
                        LockError::LockBusy => None,
                    })
            })
            .or_else(|| {
                cause
                    .downcast_ref::<DeleteError>()
                    .and_then(|inner| match inner {
                        DeleteError::IoError { io_error, .. } => Some(io_error.as_ref()),
                        DeleteError::FileDoesNotExist(_) => None,
                    })
            });
        let next = tantivy_payload
            .or_else(|| directory_io.map(|inner| -> &(dyn std::error::Error + 'static) { inner }))
            .or_else(|| {
                cause
                    .downcast_ref::<io::Error>()
                    .and_then(io::Error::get_ref)
                    .map(|inner| -> &(dyn std::error::Error + 'static) { inner })
            })
            .or_else(|| cause.source());
        match next {
            Some(inner) => cause = inner,
            None => {
                return observed_io.map_or_else(
                    || CoreError::Storage(format!("{context}: {error}")),
                    |inner| CoreError::storage_io(context, inner),
                );
            }
        }
    }
}

fn add_retirement(mut primary: CoreError, retirement: &CoreError) -> CoreError {
    let message = match &mut primary {
        CoreError::InvalidContract(message)
        | CoreError::NotReady(message)
        | CoreError::NotImplemented(message)
        | CoreError::NotFound(message)
        | CoreError::Storage(message)
        | CoreError::StorageIo { message, .. }
        | CoreError::Typed { message, .. } => message,
    };
    message.push_str("; original history reader retirement failed: ");
    message.push_str(&retirement.to_string());
    primary
}

pub(super) fn read_all(
    io: &dyn HistoryTextFileIoPort,
    path: &Path,
) -> Result<Option<Vec<u8>>, CoreError> {
    let Some(mut reader) = io.open_reader(path)? else {
        return Ok(None);
    };
    let result = (|| {
        let length = reader.metadata()?.len();
        let bound = length
            .checked_add(1)
            .ok_or_else(|| CoreError::Storage("history reader length overflow".into()))?;
        let mut bytes = Vec::new();
        let _read = (&mut *reader)
            .take(bound)
            .read_to_end(&mut bytes)
            .map_err(|error| admitted_io_error("read admitted history file", &error))?;
        if u64::try_from(bytes.len()).map_err(|error| CoreError::Storage(error.to_string()))?
            != length
        {
            return Err(CoreError::Storage(
                "admitted history file changed length during read".into(),
            ));
        }
        Ok(bytes)
    })();
    match (result, reader.retire()) {
        (Ok(bytes), Ok(())) => Ok(Some(bytes)),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(retirement)) => Err(retirement),
        (Err(primary), Err(retirement)) => Err(add_retirement(primary, &retirement)),
    }
}

/// One engine directory. Clones retain exactly the original operation scope.
#[derive(Clone)]
pub(super) struct PhysicalDirectory {
    root: PathBuf,
    io: Arc<dyn HistoryTextFileIoPort>,
}

impl fmt::Debug for PhysicalDirectory {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhysicalDirectory")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl PhysicalDirectory {
    pub(super) fn new(root: &Path, io: Arc<dyn HistoryTextFileIoPort>) -> Self {
        Self {
            root: root.to_path_buf(),
            io,
        }
    }

    fn absolute(&self, relative: &Path) -> io::Result<PathBuf> {
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "non-canonical history engine path",
            ));
        }
        Ok(self.root.join(relative))
    }
}

struct PinnedFileHandle {
    reader: Mutex<Option<Box<dyn PublicationObjectReader>>>,
    len: usize,
}

impl fmt::Debug for PinnedFileHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PinnedFileHandle")
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

impl HasLen for PinnedFileHandle {
    fn len(&self) -> usize {
        self.len
    }
}

impl FileHandle for PinnedFileHandle {
    fn read_bytes(&self, range: Range<usize>) -> io::Result<OwnedBytes> {
        if range.start > range.end {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reversed history file range",
            ));
        }
        if range.end > self.len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "history file range exceeds length",
            ));
        }
        let length = range.end.checked_sub(range.start).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "reversed history file range")
        })?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
        bytes.resize(length, 0);
        let mut guard = self
            .reader
            .lock()
            .map_err(|error| io::Error::other(error.to_string()))?;
        let reader = guard
            .as_mut()
            .ok_or_else(|| io::Error::other("history reader retired"))?;
        let _position = reader.seek(SeekFrom::Start(
            u64::try_from(range.start).map_err(io::Error::other)?,
        ))?;
        reader.read_exact(&mut bytes)?;
        drop(guard);
        Ok(OwnedBytes::new(bytes))
    }
}

impl Drop for PinnedFileHandle {
    fn drop(&mut self) {
        let guard = match self.reader.get_mut() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(reader) = guard.take() {
            let _retirement = reader.retire();
        }
    }
}

struct PhysicalWriter {
    writer: Mutex<Option<Box<dyn PublicationControlFileMutation>>>,
}

/// Tantivy exposes lock release only through Drop. The backend owns the
/// checked FD close and quarantines its authority if retirement fails.
struct HeldPhysicalLock {
    lock: Option<Box<dyn HistoryTextPhysicalLock>>,
}

impl Drop for HeldPhysicalLock {
    fn drop(&mut self) {
        if let Some(lock) = self.lock.take() {
            let _retirement = lock.retire();
        }
    }
}

impl io::Write for PhysicalWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writer
            .get_mut()
            .map_err(|error| io::Error::other(error.to_string()))?
            .as_mut()
            .ok_or_else(|| io::Error::other("history writer settled"))?
            .write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer
            .get_mut()
            .map_err(|error| io::Error::other(error.to_string()))?
            .as_mut()
            .ok_or_else(|| io::Error::other("history writer settled"))?
            .flush()
    }
}

impl TerminatingWrite for PhysicalWriter {
    fn terminate_ref(&mut self, _token: AntiCallToken) -> io::Result<()> {
        let writer = self
            .writer
            .get_mut()
            .map_err(|error| io::Error::other(error.to_string()))?
            .take()
            .ok_or_else(|| io::Error::other("history writer already settled"))?;
        writer.sync_and_settle().map_err(io_error)
    }
}

impl Drop for PhysicalWriter {
    fn drop(&mut self) {
        let guard = match self.writer.get_mut() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(writer) = guard.take() {
            let _retirement = writer.retire_preserving_pending();
        }
    }
}

impl Directory for PhysicalDirectory {
    fn get_file_handle(&self, path: &Path) -> Result<Arc<dyn FileHandle>, OpenReadError> {
        let absolute = self
            .absolute(path)
            .map_err(|error| OpenReadError::wrap_io_error(error, path.to_path_buf()))?;
        let reader = self
            .io
            .open_reader(&absolute)
            .map_err(|error| OpenReadError::wrap_io_error(io_error(error), path.to_path_buf()))?
            .ok_or_else(|| OpenReadError::FileDoesNotExist(path.to_path_buf()))?;
        let len = match reader.metadata().and_then(|metadata| {
            usize::try_from(metadata.len()).map_err(|error| {
                CoreError::Storage(format!("history engine file length overflow: {error}"))
            })
        }) {
            Ok(len) => len,
            Err(primary) => {
                let primary = match reader.retire() {
                    Ok(()) => primary,
                    Err(retirement) => add_retirement(primary, &retirement),
                };
                return Err(OpenReadError::wrap_io_error(
                    io_error(primary),
                    path.to_path_buf(),
                ));
            }
        };
        Ok(Arc::new(PinnedFileHandle {
            reader: Mutex::new(Some(reader)),
            len,
        }))
    }

    fn delete(&self, path: &Path) -> Result<(), DeleteError> {
        let absolute = self.absolute(path).map_err(|error| DeleteError::IoError {
            io_error: Arc::new(error),
            filepath: path.to_path_buf(),
        })?;
        if self
            .io
            .metadata(&absolute)
            .map_err(|error| DeleteError::IoError {
                io_error: Arc::new(io_error(error)),
                filepath: path.to_path_buf(),
            })?
            .is_none()
        {
            return Err(DeleteError::FileDoesNotExist(path.to_path_buf()));
        }
        self.io
            .delete_file(&absolute)
            .map_err(|error| DeleteError::IoError {
                io_error: Arc::new(io_error(error)),
                filepath: path.to_path_buf(),
            })
    }

    fn exists(&self, path: &Path) -> Result<bool, OpenReadError> {
        let absolute = self
            .absolute(path)
            .map_err(|error| OpenReadError::wrap_io_error(error, path.to_path_buf()))?;
        self.io
            .metadata(&absolute)
            .map(|metadata| metadata.is_some())
            .map_err(|error| OpenReadError::wrap_io_error(io_error(error), path.to_path_buf()))
    }

    fn open_write(&self, path: &Path) -> Result<WritePtr, OpenWriteError> {
        let absolute = self
            .absolute(path)
            .map_err(|error| OpenWriteError::wrap_io_error(error, path.to_path_buf()))?;
        let writer = self.io.create_writer(&absolute).map_err(|error| {
            if matches!(
                &error,
                CoreError::StorageIo {
                    kind: io::ErrorKind::AlreadyExists,
                    ..
                }
            ) {
                OpenWriteError::FileAlreadyExists(path.to_path_buf())
            } else {
                OpenWriteError::wrap_io_error(io_error(error), path.to_path_buf())
            }
        })?;
        let writer: Box<dyn TerminatingWrite> = Box::new(PhysicalWriter {
            writer: Mutex::new(Some(writer)),
        });
        Ok(BufWriter::new(writer))
    }

    fn atomic_read(&self, path: &Path) -> Result<Vec<u8>, OpenReadError> {
        let absolute = self
            .absolute(path)
            .map_err(|error| OpenReadError::wrap_io_error(error, path.to_path_buf()))?;
        read_all(self.io.as_ref(), &absolute)
            .map_err(|error| OpenReadError::wrap_io_error(io_error(error), path.to_path_buf()))?
            .ok_or_else(|| OpenReadError::FileDoesNotExist(path.to_path_buf()))
    }

    fn atomic_write(&self, path: &Path, data: &[u8]) -> io::Result<()> {
        let absolute = self.absolute(path)?;
        self.io.atomic_write(&absolute, data).map_err(io_error)
    }

    fn sync_directory(&self) -> io::Result<()> {
        self.io.sync_directory(&self.root).map_err(io_error)
    }

    fn acquire_lock(&self, lock: &Lock) -> Result<DirectoryLock, LockError> {
        let absolute = self
            .absolute(&lock.filepath)
            .map_err(LockError::wrap_io_error)?;
        let owned: Box<dyn HistoryTextPhysicalLock> = self
            .io
            .acquire_lock(&absolute, lock.is_blocking)
            .map_err(|error| LockError::wrap_io_error(io_error(error)))?;
        Ok(DirectoryLock::from(Box::new(HeldPhysicalLock {
            lock: Some(owned),
        })))
    }

    fn watch(&self, _watch_callback: WatchCallback) -> tantivy::Result<WatchHandle> {
        // History epochs are immutable after publication; readers use manual reload.
        Ok(WatchHandle::empty())
    }
}
