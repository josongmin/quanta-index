//! Descriptor-anchored access to sealed generation artifacts.

use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path};

#[derive(Clone, Copy, Eq, PartialEq)]
struct OpenedFileIdentity {
    device: u64,
    inode: u64,
    bytes: u64,
    links: u64,
    owner: u32,
    group: u32,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl OpenedFileIdentity {
    fn from(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            links: metadata.nlink(),
            owner: metadata.uid(),
            group: metadata.gid(),
            mode: metadata.mode(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

fn changed_entry(relative: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "sealed artifact changed while it was read: {}",
            relative.display()
        ),
    )
}

/// Resolve a sealed artifact without following symlinks below its generation.
///
/// A prior path metadata check cannot authorize a later resolution, including
/// one through a nested directory.
#[cfg(test)]
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

/// Inspect a top-level entry relative to an already opened generation without
/// following it. The result includes symlinks, including dangling links, and
/// lets control-file readers reject special inodes before attempting to open
/// them.
pub(crate) fn optional_entry_file_type_at(
    root: &File,
    name: &Path,
) -> io::Result<Option<rustix::fs::FileType>> {
    use rustix::fs::{AtFlags, FileType, statat};

    if name.components().count() != 1
        || !matches!(name.components().next(), Some(Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "entry name is not top-level",
        ));
    }
    match statat(root, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(metadata) => Ok(Some(FileType::from_raw_mode(metadata.st_mode))),
        Err(error) if error == rustix::io::Errno::NOENT => Ok(None),
        Err(error) => Err(io::Error::from_raw_os_error(error.raw_os_error())),
    }
}

#[cfg(test)]
pub(crate) fn optional_entry_at(root: &File, name: &Path) -> io::Result<bool> {
    Ok(optional_entry_file_type_at(root, name)?.is_some())
}

#[cfg(test)]
pub(crate) fn open_generation_dir_nofollow(root: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open, openat};

    let directory_flags =
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let family = root.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "generation has no family directory",
        )
    })?;
    let generation = root.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "generation has no directory name",
        )
    })?;
    // Opening the generation path directly protects only its final component.
    // Pin the family first so a swapped family symlink cannot redirect the
    // subsequent generation open outside the track root.
    let family = open(family, directory_flags, Mode::empty())
        .map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))?;
    let descriptor = openat(
        &family,
        Path::new(generation),
        directory_flags,
        Mode::empty(),
    )
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
        || error.raw_os_error() == Some(rustix::io::Errno::OPNOTSUPP.raw_os_error())
}

fn read_regular_bounded_with_post_read(
    root: &File,
    relative: &Path,
    ceiling: usize,
    post_read: impl FnOnce() -> io::Result<()>,
) -> io::Result<Vec<u8>> {
    use rustix::fs::{AtFlags, FileType, fstat, statat};

    // The final named-inode comparison is safe only for a direct child. A
    // multi-component statat could traverse a replaced intermediate link.
    if relative.components().count() != 1
        || !matches!(relative.components().next(), Some(Component::Normal(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "bounded control file is not top-level",
        ));
    }
    let mut file = open_regular_below(root, relative)?;
    let before = OpenedFileIdentity::from(&file.metadata()?);
    let bytes = read_opened_bounded(&mut file, ceiling)?;
    post_read()?;
    let after = OpenedFileIdentity::from(&file.metadata()?);
    if after != before {
        return Err(changed_entry(relative));
    }
    let named = match statat(root, relative, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(named) => named,
        Err(error)
            if matches!(
                error,
                rustix::io::Errno::NOENT | rustix::io::Errno::LOOP | rustix::io::Errno::NOTDIR
            ) =>
        {
            return Err(changed_entry(relative));
        }
        Err(error) => return Err(io::Error::from_raw_os_error(error.raw_os_error())),
    };
    let opened =
        fstat(&file).map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()))?;
    if FileType::from_raw_mode(named.st_mode) != FileType::RegularFile
        || (named.st_dev, named.st_ino) != (opened.st_dev, opened.st_ino)
    {
        return Err(changed_entry(relative));
    }
    Ok(bytes)
}

/// Read one top-level control file from a pinned generation and prove that the
/// same named inode remained stable for the whole read.
pub(crate) fn read_regular_bounded_at(
    root: &File,
    relative: &Path,
    ceiling: usize,
) -> io::Result<Vec<u8>> {
    read_regular_bounded_with_post_read(root, relative, ceiling, || Ok(()))
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
    fn generation_open_refuses_a_family_symlink() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let track = temp.path().join("track");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&track)?;
        std::fs::create_dir_all(outside.join("g1"))?;
        std::os::unix::fs::symlink(&outside, track.join("family"))?;

        if super::open_generation_dir_nofollow(&track.join("family/g1")).is_ok() {
            return Err("generation open followed a family symlink".into());
        }
        Ok(())
    }

    #[test]
    fn sealed_artifact_open_is_anchored_below_generation() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let generation = root.path().join("family/g1");
        std::fs::create_dir_all(generation.join("text-authority"))?;
        std::fs::write(generation.join("text-authority/shard"), b"inside")?;
        std::fs::write(outside.path().join("shard"), b"outside")?;
        let mut file =
            super::open_regular_nofollow(&generation, Path::new("text-authority/shard"))?;
        let mut bytes = Vec::new();
        let _read = file.read_to_end(&mut bytes)?;
        if bytes != b"inside" {
            return Err("anchored open read unexpected content".into());
        }

        std::fs::remove_file(generation.join("text-authority/shard"))?;
        std::fs::remove_dir(generation.join("text-authority"))?;
        std::os::unix::fs::symlink(outside.path(), generation.join("text-authority"))?;
        let outside_shard = outside.path().join("shard");
        for relative in [
            Path::new("text-authority/shard"),
            Path::new("../shard"),
            outside_shard.as_path(),
        ] {
            if super::open_regular_nofollow(&generation, relative).is_ok() {
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
        let generation = root.path().join("family/g1");
        std::fs::create_dir_all(&generation)?;
        let link = generation.join("repo-meta.cbor");
        let name = Path::new("repo-meta.cbor");
        let opened = super::open_generation_dir_nofollow(&generation)?;
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

    #[test]
    fn bounded_control_read_refuses_a_named_inode_replacement()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let generation = root.path().join("family/g1");
        std::fs::create_dir_all(&generation)?;
        let path = generation.join("identity.cbor");
        let displaced = generation.join("displaced.cbor");
        std::fs::write(&path, b"original")?;
        let opened = super::open_generation_dir_nofollow(&generation)?;
        let result = super::read_regular_bounded_with_post_read(
            &opened,
            Path::new("identity.cbor"),
            64,
            || {
                std::fs::rename(&path, &displaced)?;
                std::fs::write(&path, b"replacement")
            },
        );
        if !matches!(result, Err(error) if error.kind() == std::io::ErrorKind::InvalidData) {
            return Err("named control-file replacement was not refused".into());
        }
        Ok(())
    }
}
