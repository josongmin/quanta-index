//! Byte limits shared by semantic control-file producers and consumers.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::Path;

use quanta_index_core::CoreError;
use rustix::fs::{AtFlags, Mode, OFlags, open, openat, statat};
use sha2::{Digest as _, Sha256};

pub(crate) const MAX_SEALED_MARKER_BYTES: usize = 4 * 1024;
pub(crate) const MAX_GENERATION_CONTRACT_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_SCOPE_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_SEALED_MANIFEST_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_RECEIPT_BYTES: usize = 64 * 1024;

/// Pin the family and generation before resolving a control-file name.
///
/// The configured track root may have trusted ancestors, but neither of its
/// generation-owned children may redirect a later read through a symlink.
fn generation_parent(path: &Path) -> io::Result<(File, &std::ffi::OsStr)> {
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

fn open_bounded(path: &Path, max_bytes: usize) -> io::Result<File> {
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
    let file = File::from(descriptor);
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control file is not a regular file",
        ));
    }
    if file.metadata()?.len() > u64::try_from(max_bytes).map_err(io::Error::other)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("control file exceeds {max_bytes} bytes"),
        ));
    }
    Ok(file)
}

pub(crate) fn read_bounded(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
    let file = open_bounded(path, max_bytes)?;
    // A file may grow after metadata. The extra byte detects that race without
    // reading unbounded attacker-controlled content into memory.
    let sentinel_limit = u64::try_from(max_bytes)
        .map_err(io::Error::other)?
        .checked_add(1)
        .ok_or_else(|| io::Error::other("control-file byte limit overflow"))?;
    let mut reader = file.take(sentinel_limit);
    let mut bytes = Vec::new();
    let _read_bytes = reader.read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("control file exceeds {max_bytes} bytes"),
        ));
    }
    Ok(bytes)
}

/// Stream a control-file commitment through the same nofollow and size
/// admission used by the decoders, without retaining its bytes in memory.
pub(crate) fn measure_bounded(path: &Path, max_bytes: usize) -> io::Result<(u64, [u8; 32])> {
    let mut file = open_bounded(path, max_bytes)?;
    let limit = u64::try_from(max_bytes).map_err(io::Error::other)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16].into_boxed_slice();
    let mut length = 0_u64;
    loop {
        // Once opened, an I/O failure does not prove missing or corrupt
        // content. Callers may classify only open-time NotFound that way.
        let read = file.read(&mut buffer).map_err(io::Error::other)?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(u64::try_from(read).map_err(io::Error::other)?)
            .ok_or_else(|| io::Error::other("control-file length overflows u64"))?;
        if length > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("control file exceeds {max_bytes} bytes"),
            ));
        }
        hasher.update(buffer.get(..read).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "control-file read exceeded buffer",
            )
        })?);
    }
    Ok((length, hasher.finalize().into()))
}

pub(crate) fn read_string_bounded(path: &Path, max_bytes: usize) -> io::Result<String> {
    String::from_utf8(read_bounded(path, max_bytes)?)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub(crate) fn ensure_bounded(bytes: &[u8], max_bytes: usize, what: &str) -> Result<(), CoreError> {
    if bytes.len() > max_bytes {
        return Err(CoreError::Storage(format!(
            "semantic: {what} exceeds {max_bytes} encoded bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_sparse_oversized_file_before_allocation() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let generation = temp.path().join("family/g1");
        std::fs::create_dir_all(&generation)?;
        let path = generation.join("oversized");
        let file = File::create(&path)?;
        file.set_len(
            u64::try_from(MAX_RECEIPT_BYTES)?
                .checked_add(1)
                .ok_or("overflow")?,
        )?;
        let error = read_bounded(&path, MAX_RECEIPT_BYTES).expect_err("oversized file");
        if error.kind() != io::ErrorKind::InvalidData {
            return Err(format!("oversized file answered {error}").into());
        }
        let error = measure_bounded(&path, MAX_RECEIPT_BYTES)
            .expect_err("oversized file must not be hashed");
        if error.kind() != io::ErrorKind::InvalidData {
            return Err(format!("oversized hash answered {error}").into());
        }
        Ok(())
    }

    #[test]
    fn refuses_a_control_file_symlink() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let outside = temp.path().join("outside");
        std::fs::write(&outside, b"outside")?;
        let generation = temp.path().join("family/g1");
        std::fs::create_dir_all(&generation)?;
        let link = generation.join("sealed-marker");
        std::os::unix::fs::symlink(&outside, &link)?;
        let error = read_bounded(&link, MAX_SEALED_MARKER_BYTES)
            .expect_err("control-file symlink must be refused");
        if error.kind() != io::ErrorKind::InvalidData || regular_file_present(&link).is_ok() {
            return Err(
                format!("control-file symlink was not classified as invalid: {error}").into(),
            );
        }
        let error = measure_bounded(&link, MAX_SEALED_MARKER_BYTES)
            .expect_err("symlink must not be hashed");
        if error.kind() != io::ErrorKind::InvalidData {
            return Err(format!("symlink hash answered {error}").into());
        }
        Ok(())
    }

    #[test]
    fn dangling_symlink_is_present_but_invalid() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let generation = temp.path().join("family/g1");
        std::fs::create_dir_all(&generation)?;
        let marker = generation.join("SEALED");
        std::os::unix::fs::symlink(temp.path().join("missing"), &marker)?;
        let error = regular_file_present(&marker).expect_err("dangling marker is not absent");
        if error.kind() != io::ErrorKind::InvalidData {
            return Err(format!("dangling marker answered {error}").into());
        }
        Ok(())
    }

    #[test]
    fn refuses_a_family_symlink_before_opening_a_control_file()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let track = temp.path().join("track");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&track)?;
        std::fs::create_dir_all(outside.join("g1"))?;
        std::fs::write(outside.join("g1/SEALED"), b"outside")?;
        std::os::unix::fs::symlink(&outside, track.join("family"))?;
        let marker = track.join("family/g1/SEALED");
        if read_bounded(&marker, MAX_SEALED_MARKER_BYTES).is_ok()
            || regular_file_present(&marker).is_ok()
        {
            return Err("semantic control read followed a family symlink".into());
        }
        Ok(())
    }
}
