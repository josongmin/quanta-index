//! A coherent existing-root Catalog read without DDL, recovery or source writes.
//! Physical immutable-object authentication belongs to the object-store owner.

use std::{
    fs,
    os::unix::{ffi::OsStrExt as _, fs::MetadataExt as _},
    path::Path,
    time::Duration,
};

use quanta_index_core::{
    CoreError,
    index::{
        IndexError, MemoryLease, MemoryLifetime, ObjectNamespaceBinding, OperationKey,
        OperationStatus, WorkBudget, WorkUsage,
    },
};
use rusqlite::{Connection, OpenFlags, config::DbConfig};

use super::{NativeBootManifest, boot_manifest, capture_format, read_operation, reclaim, sql};
use crate::{CATALOG_FILE_NAME, CatalogReadOnlyIoAdmission, catalog_dir};

/// Metadata observed in one `SQLite` read transaction. This is not a physical
/// object proof, a producer completion seal or a deployable boot receipt.
pub struct NativeBootCatalogSnapshot {
    pub root_nonce: [u8; 32],
    pub selected: Option<NativeBootManifest>,
    pub namespace: Option<ObjectNamespaceBinding>,
    _allocation: Option<MemoryLease>,
}

/// One admitted read result. The requested candidate cannot be moved away
/// from the retained memory charge owned by the accompanying snapshot.
pub struct NativeBootCatalogCandidate {
    // Data must be dropped before its retained allocation in `snapshot`.
    manifest: NativeBootManifest,
    snapshot: NativeBootCatalogSnapshot,
}

impl NativeBootCatalogCandidate {
    #[must_use]
    pub const fn snapshot(&self) -> &NativeBootCatalogSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub const fn manifest(&self) -> &NativeBootManifest {
        &self.manifest
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    bytes: u64,
    modified_seconds: i64,
    modified_nanos: i64,
}

fn input(path: &Path, optional: bool) -> Result<Option<FileIdentity>, IndexError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(IndexError::Storage(format!(
                "inspect Catalog input: {error}"
            )));
        }
    };
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(IndexError::Storage(
            "Catalog verification input must be a private, owned, single-link regular file".into(),
        ));
    }
    Ok(Some(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        bytes: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanos: metadata.mtime_nsec(),
    }))
}

fn core(error: CoreError) -> IndexError {
    match error {
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            ..
        } => IndexError::RebuildRequired,
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogBusy,
            ..
        } => IndexError::Conflict,
        other @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)
        | CoreError::StorageIo { .. }) => IndexError::Storage(other.to_string()),
    }
}

fn engine(error: &rusqlite::Error) -> IndexError {
    if matches!(error, rusqlite::Error::SqliteFailure(failure, _)
        if matches!(failure.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
    {
        IndexError::Conflict
    } else {
        sql(error)
    }
}

/// Percent-encode raw filesystem bytes; a filename cannot inject URI options.
fn main_only_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
            uri.push(char::from(*byte));
        } else {
            uri.push('%');
            // Nibbles are always in range, without unchecked indexing.
            for nibble in [*byte >> 4, *byte & 15] {
                let digit = if nibble < 10 {
                    b'0'.wrapping_add(nibble)
                } else {
                    b'A'.wrapping_add(nibble.wrapping_sub(10))
                };
                uri.push(char::from(digit));
            }
        }
    }
    uri.push_str("?immutable=1");
    uri
}

/// Verify existing Catalog metadata without mutation.
///
/// Read schema, nonce, selected publication/custody and saved namespace binding.
/// The mandatory I/O owner retains the existing root
/// lease, authenticates native file opens and forbids every source mutation.
/// No fallback/default VFS, CREATE, recovery, allocator seed or marker write.
///
/// `SQLite` needs a write-capable main FD for its exclusive POSIX lock. The VFS
/// denies writes on that handle even outside this scope. Exclusive locking
/// rebuilds the WAL index in memory, including crash-left committed frames;
/// checkpoint-on-close is disabled before the first read. Only when the WAL is
/// absent or zero bytes does the native-locked main-only URI avoid `SQLite`'s
/// attempt to create an empty WAL. Nonempty WAL is never hidden. Input identity
/// is rechecked after close, so a WAL born during acquisition refuses the result.
pub fn verify_native_boot_catalog(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    budget: &mut WorkBudget,
) -> Result<NativeBootCatalogSnapshot, IndexError> {
    Ok(verify(state_root, io, None, budget)?.0)
}

/// Inspect a published candidate alongside the persisted selection in one read transaction.
///
/// The candidate is logical metadata only: callers authenticate its objects before a later mutable
/// selection under the same state-root lease.
pub fn verify_native_boot_candidate(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    operation: OperationKey,
    budget: &mut WorkBudget,
) -> Result<NativeBootCatalogCandidate, IndexError> {
    let (snapshot, candidate) = verify(state_root, io, Some(operation), budget)?;
    let manifest = candidate.ok_or_else(|| {
        IndexError::BackendViolation("requested boot candidate was not read".into())
    })?;
    Ok(NativeBootCatalogCandidate { manifest, snapshot })
}

fn verify(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    candidate: Option<OperationKey>,
    budget: &mut WorkBudget,
) -> Result<(NativeBootCatalogSnapshot, Option<NativeBootManifest>), IndexError> {
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    let path_bytes = u64::try_from(state_root.as_os_str().len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(2048))
        .ok_or(IndexError::ResourceLimited)?;
    let _paths = budget.reserve_memory(path_bytes, MemoryLifetime::Temporary)?;
    let _scope = crate::io_admission::ScopeLease::enter_read_only(io, budget.request());
    let path = catalog_dir(state_root).join(CATALOG_FILE_NAME);
    let wal_path = path.with_file_name(format!("{CATALOG_FILE_NAME}-wal"));
    let main = input(&path, false)?.ok_or(IndexError::SnapshotUnavailable)?;
    let wal = input(&wal_path, true)?;
    if main.bytes < 100 {
        return Err(IndexError::Storage("Catalog header is truncated".into()));
    }
    let input_bytes = main
        .bytes
        .checked_add(wal.map_or(0, |file| file.bytes))
        .ok_or(IndexError::ResourceLimited)?;
    // Admission covers the complete possible WAL recovery scan. The heap WAL
    // index has bounded per-frame storage; the cache and canonical metadata
    // scratch add a fixed envelope. This is logical allocation admission, not
    // a claim about whole-process RSS or SQLite's global allocator.
    budget.charge(WorkUsage {
        reads: 256,
        bytes: input_bytes,
        ..WorkUsage::default()
    })?;
    let envelope = input_bytes
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(2 * 1024 * 1024))
        .ok_or(IndexError::ResourceLimited)?;
    let _scratch = budget.reserve_memory(envelope, MemoryLifetime::Temporary)?;
    // Both result values may contain a full bounded operation and publication.
    // Keep their shared return allowance with the snapshot in the returned pair.
    let retained_bytes = if candidate.is_some() { 32_768 } else { 16_384 };
    let allocation = budget.reserve_memory(retained_bytes, MemoryLifetime::Retained)?;
    let main_only = wal.is_none_or(|file| file.bytes == 0);
    let uri = main_only.then(|| main_only_uri(&path));
    let open_path = uri.as_ref().map_or(path.as_path(), |uri| Path::new(uri));
    let connection = Connection::open_with_flags_and_vfs(
        open_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | if main_only {
                OpenFlags::SQLITE_OPEN_URI
            } else {
                OpenFlags::empty()
            },
        io.read_only_vfs_name(),
    )
    .map_err(|error| engine(&error))?;
    let result = inspect(&connection, &path, candidate, budget, allocation);
    // close() is explicit: no successful verification can hide a close error.
    let closed = connection
        .close()
        .map_err(|(_connection, error)| sql(&error));
    budget.checkpoint()?;
    if input(&path, false)? != Some(main) || input(&wal_path, true)? != wal {
        return Err(IndexError::Storage(
            "Catalog inputs changed during verification".into(),
        ));
    }
    let snapshot = result?;
    closed?;
    Ok(snapshot)
}

fn inspect(
    connection: &Connection,
    path: &Path,
    candidate: Option<OperationKey>,
    budget: &mut WorkBudget,
    allocation: Option<MemoryLease>,
) -> Result<(NativeBootCatalogSnapshot, Option<NativeBootManifest>), IndexError> {
    let configured = connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
        .map_err(|error| sql(&error))?;
    let defensive = connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(|error| sql(&error))?;
    let trusted = connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)
        .map_err(|error| sql(&error))?;
    if !configured || !defensive || trusted {
        return Err(IndexError::BackendViolation(
            "SQLite declined the required verification connection configuration".into(),
        ));
    }
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(|error| sql(&error))?;
    let request = budget.request().clone();
    connection
        .progress_handler(
            1024,
            Some(move || request.checkpoint("verify native Catalog").is_err()),
        )
        .map_err(|error| sql(&error))?;
    // These pragmas change only connection state. Native guards remain the
    // final authority even if a caller accidentally disables query_only.
    connection
        .execute_batch(
            "PRAGMA locking_mode=EXCLUSIVE; PRAGMA query_only=ON;
             PRAGMA temp_store=MEMORY; PRAGMA mmap_size=0; PRAGMA cache_size=-1024;
             BEGIN;",
        )
        .map_err(|error| sql(&error))?;
    if capture_format::preflight_capture_format(connection, path).map_err(core)? {
        return Err(IndexError::RebuildRequired);
    }
    crate::idempotency::verify_installed_schema(connection, path).map_err(core)?;
    crate::sequence::verify_installed_schema(connection, path).map_err(core)?;
    crate::auxiliary::verify_installed_schema(connection, path).map_err(core)?;
    crate::candidate::verify_installed_schema(connection, path).map_err(core)?;
    let root_nonce = capture_format::verify_stamp(connection, path).map_err(core)?;
    let selected = boot_manifest::read_stored(connection)?;
    if let Some(selected) = selected.as_ref() {
        boot_manifest::validate_custody(connection, selected, budget)?;
    }
    let candidate = candidate
        .map(|key| read_candidate(connection, key, selected.as_ref(), budget))
        .transpose()?;
    budget.charge(WorkUsage {
        reads: 4,
        bytes: 1024,
        ..WorkUsage::default()
    })?;
    let namespace = if reclaim::is_bound(connection)? {
        Some(reclaim::bound_namespace(connection)?)
    } else {
        None
    };
    budget.checkpoint()?;
    Ok((
        NativeBootCatalogSnapshot {
            root_nonce,
            selected,
            namespace,
            _allocation: allocation,
        },
        candidate,
    ))
}

fn read_candidate(
    connection: &Connection,
    key: OperationKey,
    selected: Option<&NativeBootManifest>,
    budget: &mut WorkBudget,
) -> Result<NativeBootManifest, IndexError> {
    boot_manifest::charge_operation(connection, key, budget)?;
    let row = read_operation(connection, key)?.ok_or(IndexError::SnapshotUnavailable)?;
    let OperationStatus::Published(identity) = row.status else {
        return Err(IndexError::Conflict);
    };
    if identity.workspace != key.workspace || identity.epoch != key.epoch {
        return Err(IndexError::Storage(
            "boot candidate publication scope mismatch".into(),
        ));
    }
    if let Some(selected) = selected.filter(|value| value.operation.key == key) {
        if selected.operation != row.immutable || selected.publication.identity != identity {
            return Err(IndexError::Storage(
                "selected boot operation changed".into(),
            ));
        }
        return Ok(selected.clone());
    }
    let publication = boot_manifest::load_publication(connection, identity, budget)?;
    Ok(NativeBootManifest {
        operation: row.immutable,
        publication,
    })
}
