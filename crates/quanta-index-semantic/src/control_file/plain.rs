//! Bounded, nofollow reads for standalone persisted generations.

use std::fs::File;
use std::io::{self, Read as _};
use std::path::Path;

use rustix::fs::{Mode, OFlags, openat};
use sha2::{Digest as _, Sha256};

use super::product::generation_parent;

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
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control file is not a regular file",
        ));
    }
    if metadata.len() > u64::try_from(max_bytes).map_err(io::Error::other)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("control file exceeds {max_bytes} bytes"),
        ));
    }
    Ok(file)
}

pub(super) fn read_bounded(path: &Path, max_bytes: usize) -> io::Result<Vec<u8>> {
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
pub(super) fn measure_bounded(path: &Path, max_bytes: usize) -> io::Result<(u64, [u8; 32])> {
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
