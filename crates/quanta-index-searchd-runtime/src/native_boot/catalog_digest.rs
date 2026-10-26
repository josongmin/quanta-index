//! Install-time identity for the exact Catalog main and optional WAL bytes.
//!
//! This is neither a `SQLite` backup nor a promise that a running Catalog will
//! continue to have the same bytes after startup.

use std::{
    fs,
    os::unix::fs::{FileExt as _, MetadataExt as _},
    path::Path,
};

use quanta_index_catalog::{CATALOG_FILE_NAME, catalog_dir};
use quanta_index_core::index::{
    Digest, IndexError, MemoryLifetime, ObjectReadIoPort, WorkBudget, WorkUsage,
};
use sha2::{Digest as _, Sha256};

const DOMAIN: &[u8] = b"quanta-index-native-install-catalog-v1\0";
const BUFFER_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    bytes: u64,
    modified_seconds: i64,
    modified_nanos: i64,
}

fn identity(metadata: &fs::Metadata) -> Result<FileIdentity, IndexError> {
    if !metadata.file_type().is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(IndexError::Storage(
            "Catalog digest input must be a private, owned, single-link regular file".into(),
        ));
    }
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        bytes: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanos: metadata.mtime_nsec(),
    })
}

fn linked(path: &Path, required: bool) -> Result<Option<FileIdentity>, IndexError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => identity(&metadata).map(Some),
        Err(error) if !required && error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) if required && error.kind() == std::io::ErrorKind::NotFound => {
            Err(IndexError::SnapshotUnavailable)
        }
        Err(error) => Err(IndexError::Storage(format!(
            "inspect Catalog digest input: {error}"
        ))),
    }
}

fn frame(
    hash: &mut Sha256,
    basename: &[u8],
    present: Option<FileIdentity>,
) -> Result<(), IndexError> {
    let name_len = u32::try_from(basename.len()).map_err(|_error| IndexError::ResourceLimited)?;
    hash.update(name_len.to_be_bytes());
    hash.update(basename);
    match present {
        Some(file) => {
            hash.update([1]);
            hash.update(file.bytes.to_be_bytes());
        }
        None => hash.update([0]),
    }
    Ok(())
}

fn read_file(
    path: &Path,
    expected: FileIdentity,
    io: &dyn ObjectReadIoPort,
    budget: &mut WorkBudget,
    buffer: &mut [u8],
    hash: &mut Sha256,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: expected
            .bytes
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let owner = io.open(path, budget)?;
    let file = owner.file()?;
    let opened =
        identity(&file.metadata().map_err(|error| {
            IndexError::Storage(format!("inspect admitted Catalog FD: {error}"))
        })?)?;
    if opened != expected {
        return Err(IndexError::Storage(
            "Catalog digest input changed before admitted read".into(),
        ));
    }
    let mut remaining = expected.bytes;
    let mut offset = 0u64;
    let buffer_bytes = u64::try_from(buffer.len()).map_err(|_error| IndexError::ResourceLimited)?;
    while remaining != 0 {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let wanted = usize::try_from(remaining.min(buffer_bytes))
            .map_err(|_error| IndexError::ResourceLimited)?;
        let chunk = buffer
            .get_mut(..wanted)
            .ok_or(IndexError::ResourceLimited)?;
        let count = file
            .read_at(chunk, offset)
            .map_err(|error| IndexError::Storage(format!("read Catalog digest input: {error}")))?;
        if count == 0 {
            return Err(IndexError::Storage(
                "Catalog digest input truncated during read".into(),
            ));
        }
        hash.update(buffer.get(..count).ok_or(IndexError::ResourceLimited)?);
        remaining = remaining
            .checked_sub(u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        offset = offset
            .checked_add(u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
    }
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let mut probe = [0u8; 1];
    if file
        .read_at(&mut probe, expected.bytes)
        .map_err(|error| IndexError::Storage(format!("probe Catalog digest EOF: {error}")))?
        != 0
    {
        return Err(IndexError::Storage(
            "Catalog digest input grew during read".into(),
        ));
    }
    if identity(
        &file.metadata().map_err(|error| {
            IndexError::Storage(format!("reinspect admitted Catalog FD: {error}"))
        })?,
    )? != expected
        || linked(path, true)? != Some(expected)
    {
        return Err(IndexError::Storage(
            "Catalog digest input changed during read".into(),
        ));
    }
    drop(owner);
    Ok(())
}

/// Hash a stable install-time Catalog file pair under the caller's root lease.
/// The common reader is the only file-open authority; there is no raw fallback.
pub(super) fn catalog_content_digest(
    state_root: &Path,
    io: &dyn ObjectReadIoPort,
    budget: &mut WorkBudget,
) -> Result<Digest, IndexError> {
    budget.checkpoint()?;
    let directory = catalog_dir(state_root);
    let main_path = directory.join(CATALOG_FILE_NAME);
    let wal_name = format!("{CATALOG_FILE_NAME}-wal");
    let wal_path = directory.join(&wal_name);
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    let main = linked(&main_path, true)?.ok_or(IndexError::SnapshotUnavailable)?;
    let wal = linked(&wal_path, false)?;
    let buffer_bytes = u64::try_from(BUFFER_BYTES).map_err(|_error| IndexError::ResourceLimited)?;
    let _buffer_memory = budget.reserve_memory(buffer_bytes, MemoryLifetime::Temporary)?;
    let mut buffer = vec![0u8; BUFFER_BYTES];
    let mut hash = Sha256::new();
    hash.update(DOMAIN);
    frame(&mut hash, CATALOG_FILE_NAME.as_bytes(), Some(main))?;
    read_file(&main_path, main, io, budget, &mut buffer, &mut hash)?;
    frame(&mut hash, wal_name.as_bytes(), wal)?;
    if let Some(wal) = wal {
        read_file(&wal_path, wal, io, budget, &mut buffer, &mut hash)?;
    }
    budget.checkpoint()?;
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    if linked(&main_path, true)? != Some(main) || linked(&wal_path, false)? != wal {
        return Err(IndexError::Storage(
            "Catalog digest inputs changed after read".into(),
        ));
    }
    Ok(Digest(hash.finalize().into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_frame_matches_independent_sha256_golds() -> Result<(), IndexError> {
        // Generated independently with hashlib/struct using the published
        // domain, BE lengths, fixed basenames and literal sample bytes.
        let main = b"CATALOG\0\xff";
        for (wal, expected) in [
            (
                None,
                "4f4e1eca6bd90fcc4036000207923e9a43efd954f27983128d5a5d752483d5dd",
            ),
            (
                Some(&b""[..]),
                "27cc46593322f89b7f1bc32ecd6bc8f8681ea94116e0582e2ce541fc6d2ac338",
            ),
            (
                Some(&b"WAL\0\x01"[..]),
                "b9f67bd5445ecaeb11037ce47625bda8cea6a3753dd4c617958cc87dbd3b90f4",
            ),
        ] {
            let mut hash = Sha256::new();
            hash.update(DOMAIN);
            frame(
                &mut hash,
                CATALOG_FILE_NAME.as_bytes(),
                Some(FileIdentity {
                    device: 0,
                    inode: 0,
                    bytes: u64::try_from(main.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    modified_seconds: 0,
                    modified_nanos: 0,
                }),
            )?;
            hash.update(main);
            let wal_name = b"catalog-v1.sqlite-wal";
            let wal_identity = wal
                .map(|bytes| {
                    Ok(FileIdentity {
                        device: 0,
                        inode: 0,
                        bytes: u64::try_from(bytes.len())
                            .map_err(|_error| IndexError::ResourceLimited)?,
                        modified_seconds: 0,
                        modified_nanos: 0,
                    })
                })
                .transpose()?;
            frame(&mut hash, wal_name, wal_identity)?;
            if let Some(bytes) = wal {
                hash.update(bytes);
            }
            let observed = format!("{:x}", hash.finalize());
            if observed != expected {
                return Err(IndexError::Storage(format!(
                    "Catalog digest golden differs: expected {expected}, observed {observed}"
                )));
            }
        }
        Ok(())
    }
}
