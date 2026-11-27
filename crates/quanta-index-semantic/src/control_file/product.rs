//! Product-owned bounded Semantic control and dataset readers.
//!
//! This module accepts the held physical lifetime directly. Its source does
//! not depend on the Lance file-admission extension.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::Path;

use quanta_index_core::CoreError;
use quanta_index_core::domains::generation::TreeReaderSettlement;
use quanta_index_core::publication_admission::{
    PublicationObjectLifetimePort, PublicationObjectReader,
};
use rustix::fs::{AtFlags, Mode, OFlags, open, openat, statat};
use sha2::{Digest as _, Sha256};

/// Pin the family and generation before resolving a control-file name.
///
/// The configured track root may have trusted ancestors, but neither of its
/// generation-owned children may redirect a later read through a symlink.
pub(super) fn generation_parent(path: &Path) -> io::Result<(File, &std::ffi::OsStr)> {
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "control file has no name"))?;
    let generation = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "control file has no generation directory",
        )
    })?;
    let generation_name = generation.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "generation has no directory name",
        )
    })?;
    let family = generation.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "generation has no family directory",
        )
    })?;
    let directory_flags =
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let family = open(family, directory_flags, Mode::empty())
        .map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))?;
    let generation = openat(
        &family,
        Path::new(generation_name),
        directory_flags,
        Mode::empty(),
    )
    .map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))?;
    Ok((File::from(generation), name))
}

/// Distinguish an absent control file from a present but unsafe filesystem
/// object. Callers may treat only the former as an incomplete generation.
pub(crate) fn regular_file_present(path: &Path) -> io::Result<bool> {
    let (generation, name) = match generation_parent(path) {
        Ok(opened) => opened,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    match statat(&generation, Path::new(name), AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) if rustix::fs::FileType::from_raw_mode(metadata.st_mode).is_file() => Ok(true),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("control file {} is not a regular file", path.display()),
        )),
        Err(error) if error == rustix::io::Errno::NOENT => Ok(false),
        Err(error) => Err(io::Error::from_raw_os_error(error.raw_os_error())),
    }
}

/// Open a committed dataset file only after the product physical lifetime
/// admits its path; the returned reader retains the exact descriptor until
/// checked retirement by the seal or scrub caller.
pub(crate) fn open_dataset_reader_with_control_lifetime(
    path: &Path,
    lifetime: &dyn PublicationObjectLifetimePort,
) -> io::Result<Box<dyn PublicationObjectReader>> {
    let opening = lifetime
        .begin_reader_open(path)
        .map_err(control_lifetime_error)?;
    opening
        .opened_owned(open_dataset_file(path)?)
        .map_err(control_lifetime_error)
}

fn open_dataset_file(path: &Path) -> io::Result<File> {
    open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))
}

/// Hold the exact dataset FD until Core finishes hashing or scrubbing it.
pub(crate) enum DatasetReadFile {
    Plain(File),
    Product(Box<dyn PublicationObjectReader>),
}

impl io::Read for DatasetReadFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(file) => file.read(buffer),
            Self::Product(reader) => reader.read(buffer),
        }
    }
}

impl TreeReaderSettlement for DatasetReadFile {
    fn settle(self) -> io::Result<()> {
        match self {
            Self::Plain(file) => {
                drop(file);
                Ok(())
            }
            Self::Product(reader) => reader.retire().map_err(control_lifetime_error),
        }
    }
}

pub(crate) fn open_dataset_reader(
    path: &Path,
    lifetime: Option<&dyn PublicationObjectLifetimePort>,
) -> io::Result<DatasetReadFile> {
    match lifetime {
        Some(lifetime) => {
            open_dataset_reader_with_control_lifetime(path, lifetime).map(DatasetReadFile::Product)
        }
        None => open_dataset_file(path).map(DatasetReadFile::Plain),
    }
}

pub(super) fn control_lifetime_error(error: CoreError) -> io::Error {
    match error {
        absent @ CoreError::NotFound(_) => io::Error::new(io::ErrorKind::NotFound, absent),
        storage @ CoreError::StorageIo { kind, .. } => io::Error::new(kind, storage),
        other => io::Error::other(other),
    }
}

/// Read a sealed scope-level Control file through the product-owned physical
/// lifetime. The open ticket precedes the anchored syscall; checked retirement
/// witnesses the exact FD close even when validation or reading fails.
pub(crate) fn read_bounded_with_control_lifetime(
    path: &Path,
    max_bytes: usize,
    lifetime: &dyn PublicationObjectLifetimePort,
) -> io::Result<Vec<u8>> {
    let opening = lifetime
        .begin_control_reader_open(path)
        .map_err(control_lifetime_error)?;
    let (generation, name) = generation_parent(path)?;
    let descriptor = openat(
        &generation,
        Path::new(name),
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )
    .map_err(|error| {
        if error == rustix::io::Errno::LOOP {
            io::Error::new(io::ErrorKind::InvalidData, "control file is a symlink")
        } else {
            io::Error::from_raw_os_error(error.raw_os_error())
        }
    })?;
    let mut reader = opening
        .opened_owned(File::from(descriptor))
        .map_err(control_lifetime_error)?;
    let result = (|| {
        let metadata = reader.metadata().map_err(control_lifetime_error)?;
        if !metadata.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control file is not a regular file",
            ));
        }
        let limit = u64::try_from(max_bytes).map_err(io::Error::other)?;
        if metadata.len() > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("control file exceeds {max_bytes} bytes"),
            ));
        }
        let sentinel_limit = limit
            .checked_add(1)
            .ok_or_else(|| io::Error::other("control-file byte limit overflow"))?;
        let mut bytes = Vec::new();
        let _read_bytes = (&mut reader).take(sentinel_limit).read_to_end(&mut bytes)?;
        if bytes.len() > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("control file exceeds {max_bytes} bytes"),
            ));
        }
        Ok(bytes)
    })();
    let retired = reader.retire().map_err(control_lifetime_error);
    match (result, retired) {
        (Ok(bytes), Ok(())) => Ok(bytes),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(retirement)) => Err(io::Error::new(
            io::ErrorKind::Other,
            format!("{primary}; reader retirement failed: {retirement}"),
        )),
    }
}

pub(crate) fn measure_bounded_with_control_lifetime(
    path: &Path,
    max_bytes: usize,
    lifetime: &dyn PublicationObjectLifetimePort,
) -> io::Result<(u64, [u8; 32])> {
    let bytes = read_bounded_with_control_lifetime(path, max_bytes, lifetime)?;
    let length = u64::try_from(bytes.len()).map_err(io::Error::other)?;
    Ok((length, Sha256::digest(&bytes).into()))
}
