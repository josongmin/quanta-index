//! Descriptor-anchored access to sealed generation artifacts.

use std::ffi::{OsStr, OsString};
use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Component, Path};

/// Resolve a sealed artifact without following symlinks below its generation.
///
/// A prior path metadata check cannot authorize a later resolution, including
/// one through a nested directory.
pub(crate) fn open_regular_nofollow(root: &Path, relative: &Path) -> io::Result<File> {
    let directory = open_generation_dir_nofollow(root)?;
    open_regular_below(&directory, relative)
}

/// An optional entry is absent only when no directory entry exists. `exists`
/// follows symlinks and treats dangling links and inspection failures as absent.
pub(crate) fn optional_entry_metadata(path: &Path) -> io::Result<Option<Metadata>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Check a top-level entry relative to an already opened generation. The
/// result includes symlinks, including dangling links.
pub(crate) fn optional_entry_at(root: &File, name: &Path) -> io::Result<bool> {
    use rustix::fs::{AtFlags, statat};

    if name.components().count() != 1
        || !matches!(name.components().next(), Some(Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "entry name is not top-level",
        ));
    }
    match statat(root, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(true),
        Err(error) if error == rustix::io::Errno::NOENT => Ok(false),
        Err(error) => Err(io::Error::from_raw_os_error(error.raw_os_error())),
    }
}

/// Enumerate entries from the pinned generation or one nofollow child
/// directory, never by resolving the generation pathname a second time.
pub(crate) fn entry_names_at(root: &File, child: Option<&OsStr>) -> io::Result<Vec<OsString>> {
    use rustix::fs::{Dir, Mode, OFlags, openat};

    let directory = match child {
        Some(name) => File::from(
            openat(
                root,
                Path::new(name),
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))?,
        ),
        None => root.try_clone()?,
    };
    let mut entries = Dir::read_from(&directory)
        .map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))?;
    let mut names = Vec::new();
    while let Some(entry) = entries.read() {
        let entry = entry.map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))?;
        let bytes = entry.file_name().to_bytes();
        if bytes != b"." && bytes != b".." {
            names.push(OsString::from_vec(bytes.to_vec()));
        }
    }
    Ok(names)
}

pub(crate) fn open_generation_dir_nofollow(root: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open};

    let directory_flags =
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let descriptor = open(root, directory_flags, Mode::empty())
        .map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))?;
    Ok(File::from(descriptor))
}

pub(crate) fn open_regular_below(root: &File, relative: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, openat};

    let io_error = |error: rustix::io::Errno| io::Error::from_raw_os_error(error.raw_os_error());
    let directory_flags =
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let file_flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let mut directory = root.try_clone()?;
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "sealed artifact path has a non-normal component",
            ));
        };
        if components.peek().is_none() {
            let descriptor =
                openat(&directory, Path::new(name), file_flags, Mode::empty()).map_err(io_error)?;
            let file = File::from(descriptor);
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "sealed artifact is not a regular file: {}",
                        relative.display()
                    ),
                ));
            }
            return Ok(file);
        }
        directory = File::from(
            openat(&directory, Path::new(name), directory_flags, Mode::empty())
                .map_err(io_error)?,
        );
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        "sealed artifact path is empty",
    ))
}

pub(crate) fn is_unsafe_artifact_path(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::InvalidData
        || error.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error())
        || error.raw_os_error() == Some(rustix::io::Errno::NOTDIR.raw_os_error())
}

/// Read a regular file after admitting its opened descriptor's length. The
/// sentinel detects growth without allowing an unbounded `read_to_end`.
///
/// Wrap OS read errors so callers only classify our admission failures and
/// actual short reads as corrupt content.
pub(crate) fn read_opened_bounded(file: &mut File, ceiling: usize) -> io::Result<Vec<u8>> {
    let length = file.metadata().map_err(io::Error::other)?.len();
    let admitted = usize::try_from(length)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if admitted > ceiling {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file exceeds {ceiling}-byte limit"),
        ));
    }
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(admitted)
        .map_err(|error| io::Error::new(io::ErrorKind::OutOfMemory, error))?;
    bytes.resize(admitted, 0);
    file.read_exact(&mut bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            error
        } else {
            io::Error::other(error)
        }
    })?;
    let mut extra = [0_u8; 1];
    if file.read(&mut extra).map_err(io::Error::other)? != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file grew during read",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod path_tests {
    use std::io::Read as _;
    use std::path::Path;

    #[test]
    fn sealed_artifact_open_is_anchored_below_generation() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("text-authority"))?;
        std::fs::write(root.path().join("text-authority/shard"), b"inside")?;
        std::fs::write(outside.path().join("shard"), b"outside")?;
        let mut file =
            super::open_regular_nofollow(root.path(), Path::new("text-authority/shard"))?;
        let mut bytes = Vec::new();
        let _read = file.read_to_end(&mut bytes)?;
        if bytes != b"inside" {
            return Err("anchored open read unexpected content".into());
        }

        std::fs::remove_file(root.path().join("text-authority/shard"))?;
        std::fs::remove_dir(root.path().join("text-authority"))?;
        std::os::unix::fs::symlink(outside.path(), root.path().join("text-authority"))?;
        let outside_shard = outside.path().join("shard");
        for relative in [
            Path::new("text-authority/shard"),
            Path::new("../shard"),
            outside_shard.as_path(),
        ] {
            if super::open_regular_nofollow(root.path(), relative).is_ok() {
                return Err(
                    format!("untrusted artifact path was opened: {}", relative.display()).into(),
                );
            }
        }
        Ok(())
    }

    #[test]
    fn optional_entry_sees_dangling_links_by_path_and_pinned_root()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let link = root.path().join("repo-meta.cbor");
        let name = Path::new("repo-meta.cbor");
        let opened = super::open_generation_dir_nofollow(root.path())?;
        if super::optional_entry_metadata(&link)?.is_some()
            || super::optional_entry_at(&opened, name)?
        {
            return Err("missing optional entry was present".into());
        }
        std::os::unix::fs::symlink("missing-target", &link)?;
        if super::optional_entry_metadata(&link)?.is_none()
            || !super::optional_entry_at(&opened, name)?
        {
            return Err("dangling optional entry was treated as absent".into());
        }
        Ok(())
    }
}
