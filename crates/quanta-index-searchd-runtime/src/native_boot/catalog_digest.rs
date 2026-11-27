//! Install-time identity for the exact Catalog main and optional WAL bytes.
//!
//! This is neither a `SQLite` backup nor a promise that a running Catalog will
//! continue to have the same bytes after startup.

use std::{
    fs::{self, File},
    os::unix::fs::{FileExt as _, MetadataExt as _},
    path::Path,
};

use quanta_index_catalog::{CATALOG_FILE_NAME, catalog_dir};
use quanta_index_core::index::{Digest, IndexError, MemoryLifetime, WorkBudget, WorkUsage};
use sha2::{Digest as _, Sha256};

use super::NativeBootAuthority;

#[path = "catalog_digest/stock.rs"]
mod stock;

const DOMAIN: &[u8] = b"quanta-index-native-install-catalog-v1\0";
const BUFFER_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    bytes: u64,
    modified_seconds: i64,
    modified_nanos: i64,
    changed_seconds: i64,
    changed_nanos: i64,
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
        changed_seconds: metadata.ctime(),
        changed_nanos: metadata.ctime_nsec(),
    })
}

trait DigestInput {
    fn linked(&self, name: &Path, required: bool) -> Result<Option<FileIdentity>, IndexError>;

    fn with_file(
        &self,
        name: &Path,
        budget: &mut WorkBudget,
        read: &mut dyn FnMut(&File, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError>;
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

fn read_input(
    input: &dyn DigestInput,
    name: &Path,
    expected: FileIdentity,
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
    input.with_file(name, budget, &mut |file, budget| {
        hash_file(file, expected, budget, buffer, hash)?;
        if input.linked(name, true)? != Some(expected) {
            return Err(IndexError::Storage(
                "Catalog digest input link changed during read".into(),
            ));
        }
        Ok(())
    })
}

fn hash_file(
    file: &File,
    expected: FileIdentity,
    budget: &mut WorkBudget,
    buffer: &mut [u8],
    hash: &mut Sha256,
) -> Result<(), IndexError> {
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
    {
        return Err(IndexError::Storage(
            "Catalog digest input changed during read".into(),
        ));
    }
    Ok(())
}

fn digest(input: &dyn DigestInput, budget: &mut WorkBudget) -> Result<Digest, IndexError> {
    budget.checkpoint()?;
    let main_path = Path::new(CATALOG_FILE_NAME);
    let wal_name = format!("{CATALOG_FILE_NAME}-wal");
    let wal_path = Path::new(&wal_name);
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    let main = input
        .linked(main_path, true)?
        .ok_or(IndexError::SnapshotUnavailable)?;
    let wal = input.linked(wal_path, false)?;
    let buffer_bytes = u64::try_from(BUFFER_BYTES).map_err(|_error| IndexError::ResourceLimited)?;
    let _buffer_memory = budget.reserve_memory(buffer_bytes, MemoryLifetime::Temporary)?;
    let mut buffer = vec![0u8; BUFFER_BYTES];
    let mut hash = Sha256::new();
    hash.update(DOMAIN);
    frame(&mut hash, CATALOG_FILE_NAME.as_bytes(), Some(main))?;
    read_input(input, main_path, main, budget, &mut buffer, &mut hash)?;
    frame(&mut hash, wal_name.as_bytes(), wal)?;
    if let Some(wal) = wal {
        read_input(input, wal_path, wal, budget, &mut buffer, &mut hash)?;
    }
    budget.checkpoint()?;
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    if input.linked(main_path, true)? != Some(main) || input.linked(wal_path, false)? != wal {
        return Err(IndexError::Storage(
            "Catalog digest inputs changed after read".into(),
        ));
    }
    Ok(Digest(hash.finalize().into()))
}

/// Hash the Catalog bytes under the same held Root that admitted boot.
pub(super) fn catalog_content_digest(
    authority: &NativeBootAuthority,
    budget: &mut WorkBudget,
) -> Result<Digest, IndexError> {
    stock::catalog_content_digest(&authority.physical, budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    use {
        crate::publication_file_capacity::PublicationFileCapacity,
        crate::publication_inventory::{
            PublicationBootInventoryLimits, seed_publication_capacity_stock_catalog,
        },
        anyhow::{Result, ensure},
        quanta_index_core::{RequestBudgetV1, index::WorkLimits},
        quanta_index_searchd::app::runtime::StateRootLease,
        std::{
            io::Write as _,
            os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, symlink},
            time::Duration,
        },
    };

    const MAIN: &[u8] = b"CATALOG\0\xff";
    const GOLDENS: [(Option<&[u8]>, &str); 3] = [
        (
            None,
            "4f4e1eca6bd90fcc4036000207923e9a43efd954f27983128d5a5d752483d5dd",
        ),
        (
            Some(b""),
            "27cc46593322f89b7f1bc32ecd6bc8f8681ea94116e0582e2ce541fc6d2ac338",
        ),
        (
            Some(b"WAL\0\x01"),
            "b9f67bd5445ecaeb11037ce47625bda8cea6a3753dd4c617958cc87dbd3b90f4",
        ),
    ];

    #[test]
    fn catalog_frame_matches_independent_sha256_golds() -> std::result::Result<(), IndexError> {
        // Generated independently with hashlib/struct using the published
        // domain, BE lengths, fixed basenames and literal sample bytes.
        for (wal, expected) in GOLDENS {
            let mut hash = Sha256::new();
            hash.update(DOMAIN);
            frame(
                &mut hash,
                CATALOG_FILE_NAME.as_bytes(),
                Some(FileIdentity {
                    device: 0,
                    inode: 0,
                    bytes: u64::try_from(MAIN.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    modified_seconds: 0,
                    modified_nanos: 0,
                    changed_seconds: 0,
                    changed_nanos: 0,
                }),
            )?;
            hash.update(MAIN);
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
                        changed_seconds: 0,
                        changed_nanos: 0,
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

    fn with_stock_fixture(
        wal: Option<&[u8]>,
        handles: usize,
        check: impl FnOnce(&PublicationFileCapacity, &Path, &mut WorkBudget) -> Result<()>,
    ) -> Result<()> {
        let temp = tempfile::tempdir()?;
        let lease = StateRootLease::acquire(&temp.path().join("state"))?;
        let root = lease.state_root_identity_v1();
        let directory = catalog_dir(root);
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let write = |name, bytes: &[u8]| -> Result<()> {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(directory.join(name))?
                .write_all(bytes)?;
            Ok(())
        };
        write(CATALOG_FILE_NAME, MAIN)?;
        if let Some(wal) = wal {
            write("catalog-v1.sqlite-wal", wal)?;
        }
        let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let authority = seed_publication_capacity_stock_catalog(
            &lease,
            root,
            PublicationBootInventoryLimits {
                max_bytes: 1024 * 1024,
                max_files: 16,
                max_pending: 16,
                max_entries: 32,
                max_depth: 8,
                max_pending_frames: 16,
            },
            &[],
            &request,
        )?;
        let common = PublicationFileCapacity::new(lease, authority, handles, 64, 256)?;
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 1_000,
                writes: 0,
                bytes: 1024 * 1024,
                mutations: 0,
            },
            request,
        );
        check(&common, &directory, &mut budget)?;
        ensure!(
            common.is_quiescent()?,
            "Catalog digest left physical custody"
        );
        Ok(())
    }

    #[test]
    fn stock_catalog_digest_matches_signed_format_outside_object_ledger() -> Result<()> {
        for (wal, expected) in GOLDENS {
            let handles = if wal.is_some() { 3 } else { 2 };
            with_stock_fixture(wal, handles, |common, directory, budget| {
                ensure!(
                    common
                        .begin_native_reader_open(&directory.join(CATALOG_FILE_NAME))
                        .is_err(),
                    "stock Catalog unexpectedly entered the object ledger"
                );
                let actual = stock::catalog_content_digest(common, budget)?;
                let observed = actual
                    .0
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                ensure!(observed == expected, "Catalog digest differs");
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn stock_catalog_digest_rejects_unfunded_wal_and_replaced_main() -> Result<()> {
        with_stock_fixture(Some(b"committed WAL"), 2, |common, _directory, budget| {
            ensure!(
                matches!(
                    stock::catalog_content_digest(common, budget),
                    Err(IndexError::ResourceLimited)
                ),
                "unfunded WAL reached Catalog digest"
            );
            Ok(())
        })?;
        with_stock_fixture(None, 2, |common, directory, budget| {
            let main = directory.join(CATALOG_FILE_NAME);
            let renamed = directory.join("original");
            fs::rename(&main, &renamed)?;
            symlink(&renamed, &main)?;
            ensure!(
                stock::catalog_content_digest(common, budget).is_err(),
                "Catalog digest accepted a symbolic link to the previous inode"
            );
            Ok(())
        })
    }
}
