//! Read-only Tantivy directory over generation-anchored file descriptors.
//!
//! `MmapDirectory` reopens a pathname for each component. A metadata check
//! before `Index::open` cannot constrain that later resolution. This adapter
//! opens each read with `O_NOFOLLOW` and keeps the descriptor in the handle.
//! Segment range reads copy their requested bytes. Tantivy can request an entire
//! term dictionary or fieldnorm component, so this moves that component into
//! heap at open. Mapping a file that an external process can truncate would
//! instead risk a process-fatal fault; descriptor-anchored reads return an I/O
//! error under that race. This directory does not by itself promise a peak
//! RSS ceiling; the cost needs source- and workload-bound measurement.

use std::fs::File;
use std::io;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use rustix::fs::{FlockOperation, Mode, OFlags, flock, openat};
use tantivy::HasLen;
use tantivy::directory::error::{DeleteError, LockError, OpenReadError, OpenWriteError};
use tantivy::directory::{
    Directory, DirectoryLock, FileHandle, Lock, META_LOCK, OwnedBytes, WatchCallback, WatchHandle,
    WritePtr,
};

pub(crate) const MAX_INDEX_CONTROL_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct SealedIndexDirectory {
    root: Arc<File>,
    sealed_meta: Option<Arc<[u8]>>,
}

impl SealedIndexDirectory {
    #[cfg(test)]
    pub(crate) fn new(root: &Path) -> io::Result<Self> {
        Ok(Self::from_opened(super::open_generation_dir_nofollow(
            root,
        )?))
    }

    pub(crate) fn from_opened(root: File) -> Self {
        Self {
            root: Arc::new(root),
            sealed_meta: None,
        }
    }

    pub(crate) fn from_opened_with_meta(root: File, meta: Vec<u8>) -> Self {
        Self {
            root: Arc::new(root),
            sealed_meta: Some(Arc::from(meta)),
        }
    }

    fn is_meta(path: &Path) -> bool {
        path == Path::new(crate::TANTIVY_INDEX_META_FILE_NAME)
    }

    fn open(&self, path: &Path) -> Result<File, OpenReadError> {
        super::open_regular_below(&self.root, path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                OpenReadError::FileDoesNotExist(path.to_path_buf())
            } else {
                OpenReadError::wrap_io_error(error, path.to_path_buf())
            }
        })
    }
}

#[derive(Debug)]
struct SealedFileHandle {
    file: File,
    len: usize,
}

#[derive(Debug)]
struct SealedMetaHandle {
    bytes: Arc<[u8]>,
}

impl HasLen for SealedMetaHandle {
    fn len(&self) -> usize {
        self.bytes.len()
    }
}

impl FileHandle for SealedMetaHandle {
    fn read_bytes(&self, range: Range<usize>) -> io::Result<OwnedBytes> {
        if self.bytes.get(range.clone()).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "range outside sealed meta",
            ));
        }
        Ok(OwnedBytes::new(Arc::clone(&self.bytes)).slice(range))
    }
}

impl HasLen for SealedFileHandle {
    fn len(&self) -> usize {
        self.len
    }
}

impl FileHandle for SealedFileHandle {
    fn read_bytes(&self, range: Range<usize>) -> io::Result<OwnedBytes> {
        if range.start > range.end {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "reversed sealed file range",
            ));
        }
        if range.end > self.len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "range outside sealed file",
            ));
        }
        let length = range.end.checked_sub(range.start).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "reversed sealed file range")
        })?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
        bytes.resize(length, 0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.file.read_exact_at(
                &mut bytes,
                u64::try_from(range.start).map_err(io::Error::other)?,
            )?;
        }
        #[cfg(not(unix))]
        {
            use std::io::{Seek, SeekFrom};
            let mut file = self.file.try_clone()?;
            file.seek(SeekFrom::Start(
                u64::try_from(range.start).map_err(io::Error::other)?,
            ))?;
            file.read_exact(&mut bytes)?;
        }
        Ok(OwnedBytes::new(bytes))
    }
}

impl Directory for SealedIndexDirectory {
    fn get_file_handle(&self, path: &Path) -> Result<Arc<dyn FileHandle>, OpenReadError> {
        if Self::is_meta(path)
            && let Some(bytes) = &self.sealed_meta
        {
            return Ok(Arc::new(SealedMetaHandle {
                bytes: Arc::clone(bytes),
            }));
        }
        let file = self.open(path)?;
        let len = usize::try_from(
            file.metadata()
                .map_err(|error| OpenReadError::wrap_io_error(error, path.to_path_buf()))?
                .len(),
        )
        .map_err(|error| {
            OpenReadError::wrap_io_error(io::Error::other(error), path.to_path_buf())
        })?;
        Ok(Arc::new(SealedFileHandle { file, len }))
    }

    fn exists(&self, path: &Path) -> Result<bool, OpenReadError> {
        if Self::is_meta(path) && self.sealed_meta.is_some() {
            return Ok(true);
        }
        match self.open(path) {
            Ok(_) => Ok(true),
            Err(OpenReadError::FileDoesNotExist(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn atomic_read(&self, path: &Path) -> Result<Vec<u8>, OpenReadError> {
        if Self::is_meta(path)
            && let Some(bytes) = &self.sealed_meta
        {
            return Ok(bytes.as_ref().to_vec());
        }
        let mut file = self.open(path)?;
        super::read_opened_bounded(&mut file, MAX_INDEX_CONTROL_BYTES)
            .map_err(|error| OpenReadError::wrap_io_error(error, path.to_path_buf()))
    }

    fn delete(&self, path: &Path) -> Result<(), DeleteError> {
        Err(DeleteError::IoError {
            io_error: Arc::new(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "sealed index is read-only",
            )),
            filepath: path.to_path_buf(),
        })
    }

    fn open_write(&self, path: &Path) -> Result<WritePtr, OpenWriteError> {
        Err(OpenWriteError::wrap_io_error(
            io::Error::new(io::ErrorKind::PermissionDenied, "sealed index is read-only"),
            path.to_path_buf(),
        ))
    }

    fn atomic_write(&self, _path: &Path, _content: &[u8]) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "sealed index is read-only",
        ))
    }

    fn sync_directory(&self) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "sealed index is read-only",
        ))
    }

    fn acquire_lock(&self, lock: &Lock) -> Result<DirectoryLock, LockError> {
        if lock.filepath != META_LOCK.filepath {
            return Err(LockError::wrap_io_error(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "sealed index cannot acquire a writer lock",
            )));
        }
        let descriptor = openat(
            self.root.as_ref(),
            lock.filepath.as_path(),
            OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(|error| {
            LockError::wrap_io_error(io::Error::from_raw_os_error(error.raw_os_error()))
        })?;
        let file = File::from(descriptor);
        if !file.metadata().map_err(LockError::wrap_io_error)?.is_file() {
            return Err(LockError::wrap_io_error(io::Error::new(
                io::ErrorKind::InvalidData,
                "sealed index lock is not a regular file",
            )));
        }
        let operation = if lock.is_blocking {
            FlockOperation::LockExclusive
        } else {
            FlockOperation::NonBlockingLockExclusive
        };
        flock(&file, operation).map_err(|error| {
            LockError::wrap_io_error(io::Error::from_raw_os_error(error.raw_os_error()))
        })?;
        Ok(DirectoryLock::from(Box::new(file)))
    }

    fn watch(&self, _callback: WatchCallback) -> tantivy::Result<WatchHandle> {
        Ok(WatchHandle::empty())
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::path::Path;

    use tantivy::directory::{Directory, META_LOCK};

    use super::SealedIndexDirectory;

    #[test]
    fn sealed_handle_keeps_its_inode_and_refuses_later_redirects() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let file = root.path().join("segment.store");
        let target = outside.path().join("segment.store");
        std::fs::write(&file, b"inside")?;
        std::fs::write(&target, b"else!!")?;
        let directory = SealedIndexDirectory::new(root.path())?;
        let opened = directory.get_file_handle(Path::new("segment.store"))?;
        std::fs::remove_file(&file)?;
        std::os::unix::fs::symlink(&target, &file)?;
        if opened.read_bytes(0..6)?.as_slice() != b"inside"
            || directory
                .get_file_handle(Path::new("segment.store"))
                .is_ok()
            || directory.exists(Path::new("segment.store")).is_ok()
            || directory.atomic_read(Path::new("segment.store")).is_ok()
        {
            return Err("sealed directory followed a redirected path".into());
        }
        Ok(())
    }

    #[test]
    fn metadata_lock_does_not_follow_a_redirect() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let target = outside.path().join("lock");
        std::fs::write(&target, b"outside")?;
        std::os::unix::fs::symlink(&target, root.path().join(&META_LOCK.filepath))?;
        let directory = SealedIndexDirectory::new(root.path())?;
        if directory.acquire_lock(&META_LOCK).is_ok() || std::fs::read(&target)? != b"outside" {
            return Err("metadata lock followed a redirected path".into());
        }
        Ok(())
    }

    #[test]
    fn directory_handle_keeps_the_opened_generation_after_path_replacement()
    -> Result<(), Box<dyn Error>> {
        let parent = tempfile::tempdir()?;
        let generation = parent.path().join("generation");
        std::fs::create_dir(&generation)?;
        std::fs::write(generation.join("segment.store"), b"original")?;
        let directory = SealedIndexDirectory::new(&generation)?;
        std::fs::rename(&generation, parent.path().join("old-generation"))?;
        std::fs::create_dir(&generation)?;
        std::fs::write(generation.join("segment.store"), b"replaced")?;
        let opened = directory.get_file_handle(Path::new("segment.store"))?;
        if opened.read_bytes(0..8)?.as_slice() != b"original" {
            return Err("directory handle followed a replacement generation".into());
        }
        Ok(())
    }

    #[test]
    fn verified_meta_bytes_are_the_bytes_the_index_will_read() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let meta = Path::new(crate::TANTIVY_INDEX_META_FILE_NAME);
        std::fs::write(root.path().join(meta), b"verified-meta")?;
        let opened = super::super::open_generation_dir_nofollow(root.path())?;
        let directory =
            SealedIndexDirectory::from_opened_with_meta(opened, b"verified-meta".to_vec());
        std::fs::write(root.path().join(meta), b"different-meta")?;
        let handle = directory.get_file_handle(meta)?;
        let first = handle.read_bytes(0..13)?;
        let second = handle.read_bytes(0..13)?;
        if directory.atomic_read(meta)? != b"verified-meta"
            || !directory.exists(meta)?
            || first.as_slice() != b"verified-meta"
            || first.as_ptr() != second.as_ptr()
        {
            return Err("verified meta bytes were reopened or copied".into());
        }
        Ok(())
    }
}
