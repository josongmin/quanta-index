//! The sealed lexical generation: what it commits to and how it is proved.
//!
//! See [`manifest`] for the commitment and its format, [`overlay`] for the
//! repo-metadata sidecars it lists, [`seal`] for the measurement that
//! writes it, [`verify`] for the walk both doors share, and [`scrub`] for
//! the deep re-measurement between seals.

use std::fs::File;
use std::io;
use std::path::{Component, Path};

pub(crate) mod coverage;
mod index_files;
mod manifest;
mod overlay;
mod scrub;
mod seal;
mod verify;

pub use seal::LexicalSealCommitmentStats;

pub(crate) use manifest::{LEXICAL_SEALED_MANIFEST_FILE_NAME, manifest_path, read_manifest};
pub(crate) use overlay::{persist_overlay, remove_overlay};
pub(crate) use scrub::{
    LEXICAL_QUARANTINE_RECEIPT_FILE_NAME, LEXICAL_SCRUB_RECEIPT_FILE_NAME, last_completed_scrub,
    quarantine_content_corrupt, quarantined_by_scrub, refuse_if_quarantined, scrub_step,
};
pub(crate) use seal::seal_generation;
pub(crate) use verify::{
    DiscardingVisitor, SealedGenerationVisitor, walk_sealed_generation,
    walk_sealed_generation_reusing_coverage,
};

/// Resolve a sealed artifact without following symlinks below its generation.
///
/// A prior path metadata check cannot authorize a later resolution, including
/// one through a nested directory.
pub(crate) fn open_regular_nofollow(root: &Path, relative: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags, open, openat};

    let io_error = |error: rustix::io::Errno| io::Error::from_raw_os_error(error.raw_os_error());
    let directory_flags =
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let file_flags = OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK;
    let mut directory = open(root, directory_flags, Mode::empty()).map_err(io_error)?;
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
        directory = openat(&directory, Path::new(name), directory_flags, Mode::empty())
            .map_err(io_error)?;
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
}
