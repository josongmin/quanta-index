//! Process-death-fenced read custody. A deadline may reject a read, but never
//! authorizes reclaim while the independently opened OS session lock is held.

use std::any::Any;
use std::fs::{self, File, TryLockError};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use quanta_index_contract::index::MapRoot;
use quanta_index_core::index::ports::CatalogReadPort;
use quanta_index_core::index::{
    CollectionId, IndexError, MemoryLease, MemoryLifetime, ReadSessionLease,
    ReadSessionRecoveryPage, ReadSessionRef, ReadTicketLease, SlotId, TenantId, TicketRecord,
    ViewSelection, WorkBudget, WorkUsage, WorktreeId, validate_read_ticket_lease,
};
use quanta_index_core::{DiagnosticLaneV1, DiagnosticStageV1, DiagnosticTerminalV1};
use rusqlite::{Connection, OptionalExtension as _, params};
use rustix::fs::{Mode, OFlags, open};

use super::*;
use crate::connection::AbandonedReadPin;

pub(super) mod creation;
mod read_attempt;
use creation::{creation_lock, prepare_open_intents, reap_open_intents};

const MAX_RECOVERY_SESSIONS: u32 = 256;
const MAX_RECOVERY_PINS: u32 = 256;
const RECOVERY_LOOKAHEAD_SLOTS: usize = MAX_RECOVERY_SESSIONS as usize + 1;
const SESSION_DIRECTORY: &str = "native-read-sessions";

// Each ID is an array of 16 u8 values; all-255 has the widest canonical JSON encoding.
pub(super) fn max_workspace_key_bytes() -> Result<i64, IndexError> {
    static MAX_BYTES: OnceLock<i64> = OnceLock::new();
    if let Some(bytes) = MAX_BYTES.get() {
        return Ok(*bytes);
    }
    let widest = WorkspaceKey {
        tenant: TenantId([u8::MAX; 16]),
        collection: CollectionId([u8::MAX; 16]),
        worktree: WorktreeId([u8::MAX; 16]),
        slot: SlotId([u8::MAX; 16]),
    };
    let bytes = i64::try_from(workspace_key(widest)?.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    // Cache only a successful encoding. A transient encoding failure can retry.
    let _installed = MAX_BYTES.set(bytes);
    Ok(bytes)
}

pub(super) type SessionRow = (i64, i64, Vec<u8>, Vec<u8>, Vec<u8>);
pub(super) type PinRow = (i64, i64, i64, i64, i64, Vec<u8>, i64, i64, Option<Vec<u8>>);
type SessionRecoveryCandidate = (SessionRow, Option<LockFile>);
type RetiredRow = (i64, Vec<u8>, Vec<u8>, Vec<u8>);
type OptionalSessionIdentity = (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>);
type OptionalRetiredRow = (i64, Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>);

struct RecoveryRows<Row> {
    rows: Vec<Row>,
    _memory: Option<MemoryLease>,
}

impl<Row> RecoveryRows<Row> {
    fn empty() -> Self {
        Self {
            rows: Vec::new(),
            _memory: None,
        }
    }

    fn reserve(
        &mut self,
        capacity: usize,
        payload_bytes: usize,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        let bytes = std::mem::size_of::<Row>()
            .checked_add(payload_bytes)
            .and_then(|slot| slot.checked_mul(capacity))
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(capacity)
            .map_err(|_| IndexError::ResourceLimited)?;
        if rows.capacity() > capacity {
            return Err(IndexError::ResourceLimited);
        }
        self.rows = rows;
        self._memory = memory;
        Ok(())
    }
}

pub(super) struct CatalogSessionLease {
    pub(super) binding: ReadSessionRef,
    catalog_path: PathBuf,
    pub(super) file: LockFile,
    read_observations: std::sync::Mutex<Option<Arc<read_attempt::ObservationGroup>>>,
}

pub(super) enum LockFile {
    Plain(File),
    Admitted(Box<dyn quanta_index_core::index::ObjectReadFile>),
}

impl LockFile {
    fn file(&self) -> Result<&File, IndexError> {
        match self {
            Self::Plain(file) => Ok(file),
            Self::Admitted(owner) => owner.file(),
        }
    }
}

impl ReadSessionLease for CatalogSessionLease {
    fn binding(&self) -> ReadSessionRef {
        self.binding
    }
}

fn io_error(action: &str, path: &Path, error: &dyn std::fmt::Display) -> IndexError {
    IndexError::Storage(format!(
        "native read session {action} {}: {error}",
        path.display()
    ))
}

fn session_directory(catalog: &SqliteCatalog) -> Result<PathBuf, IndexError> {
    let parent = catalog
        .path
        .parent()
        .ok_or_else(|| IndexError::Storage("catalog path has no parent".into()))?;
    let directory = parent.join(SESSION_DIRECTORY);
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => {
            let parent_file = open_directory(parent)?;
            parent_file
                .sync_all()
                .map_err(|error| io_error("sync parent", parent, &error))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(io_error("create directory", &directory, &error)),
    }
    let opened = open_directory(&directory)?;
    let metadata = opened
        .metadata()
        .map_err(|error| io_error("stat directory", &directory, &error))?;
    if metadata.uid() != rustix::process::geteuid().as_raw() || metadata.mode() & 0o7777 != 0o700 {
        return Err(IndexError::Storage(
            "native read session directory ownership or mode mismatch".into(),
        ));
    }
    Ok(directory)
}

fn open_directory(path: &Path) -> Result<File, IndexError> {
    open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|error| io_error("open directory", path, &error))
}

fn lock_path(directory: &Path, id: u64) -> PathBuf {
    directory.join(format!("{id:016x}.lease"))
}

fn open_lock(catalog: &SqliteCatalog, path: &Path, create: bool) -> Result<LockFile, IndexError> {
    let mut flags = OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW;
    if create {
        flags |= OFlags::CREATE | OFlags::EXCL;
    }
    let file = if let Some(owner) = catalog.connection.admission() {
        LockFile::Admitted(owner.open_lock_file(path, create)?)
    } else {
        LockFile::Plain(
            open(path, flags, Mode::from_raw_mode(0o600))
                .map(File::from)
                .map_err(|error| io_error("open lock", path, &error))?,
        )
    };
    let metadata = file
        .file()?
        .metadata()
        .map_err(|error| io_error("stat lock", path, &error))?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o7777 != 0o600
        || metadata.nlink() != 1
    {
        return Err(IndexError::Storage(format!(
            "native read session lock is foreign or insecure: {}",
            path.display()
        )));
    }
    Ok(file)
}

pub(super) fn inode(file: &LockFile) -> Result<(Vec<u8>, Vec<u8>), IndexError> {
    let metadata = file
        .file()?
        .metadata()
        .map_err(|error| io_error("stat lock inode", Path::new("session"), &error))?;
    Ok((
        metadata.dev().to_le_bytes().to_vec(),
        metadata.ino().to_le_bytes().to_vec(),
    ))
}

fn session_digest(row: &SessionRow) -> Result<Vec<u8>, IndexError> {
    fields_digest(
        b"qidx.native.readsession.v1",
        &(&row.0, &row.1, &row.2, &row.3),
    )
}

fn retired_digest(row: &RetiredRow) -> Result<Vec<u8>, IndexError> {
    fields_digest(
        b"qidx.native.readsession.retire.v1",
        &(&row.0, &row.1, &row.2),
    )
}

#[cfg(test)]
fn read_open_failpoint(phase: &str) {
    if std::env::var("QIDX_NATIVE_READ_OPEN_CRASH").is_ok_and(|selected| selected == phase) {
        let Some(root) = std::env::var_os("QIDX_NATIVE_OPEN_CHILD_ROOT") else {
            panic!("read session crash failpoint has no child root");
        };
        if let Err(error) = std::fs::write(Path::new(&root).join("open-phase-ready"), phase) {
            panic!("read session crash failpoint readiness write failed: {error}");
        }
        loop {
            std::thread::park_timeout(Duration::from_secs(1));
        }
    }
}

/// Retry at most `limit` releases transferred by `abandon_ticket`, each pin
/// independently.
///
/// Every examined pin gets exactly one attempt per round, oldest first. A
/// committed delete, or an exact pin proven absent (`SnapshotUnavailable`:
/// already deleted by an earlier commit whose acknowledgement was lost), ends
/// custody. Any other release error is recorded on that pin, which moves to
/// the back of the queue for a later round; it neither fails the round nor
/// stops the other pins or the dead-session scan that follows. Only round
/// budget, cancellation and custody-lock errors fail the round itself.
/// Returns `(settled, deferred)`.
fn settle_abandoned_pins(
    catalog: &SqliteCatalog,
    limit: u32,
    budget: &mut WorkBudget,
) -> Result<(u32, u32), IndexError> {
    let lock = || {
        catalog
            .abandoned_read_pins
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))
    };
    let examined: Vec<AbandonedReadPin> = {
        let pending = lock()?;
        let count = pending
            .len()
            .min(usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?);
        let mut examined = Vec::new();
        examined
            .try_reserve_exact(count)
            .map_err(|_allocation_error| IndexError::ResourceLimited)?;
        examined.extend(pending.iter().take(count).cloned());
        examined
    };
    let mut settled = 0_u32;
    let mut deferred = 0_u32;
    for pin in examined {
        budget.checkpoint()?;
        budget.charge(WorkUsage {
            reads: 1,
            mutations: 1,
            ..WorkUsage::default()
        })?;
        let outcome = match catalog.release_ticket(pin.session.as_ref(), pin.ticket) {
            Ok(()) | Err(IndexError::SnapshotUnavailable) => None,
            Err(error) => Some(error),
        };
        let settled_now = {
            let mut pending = lock()?;
            let Some(index) = pending.iter().position(|held| held.ticket == pin.ticket) else {
                continue;
            };
            let mut held = pending.remove(index);
            match outcome {
                Some(error) => {
                    held.last_error = Some(error);
                    pending.push(held);
                    false
                }
                None => true,
            }
        };
        if settled_now {
            settled = settled.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        } else {
            deferred = deferred.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
    }
    if deferred > 0 {
        // A deferred pin stays queued: keep the next round's read recovery due.
        catalog.record_read_recovery_demand();
    }
    Ok((settled, deferred))
}

impl SqliteCatalog {
    /// Observe pins this Catalog holds in settlement custody, oldest first,
    /// with the error of each one's latest failed settlement attempt.
    pub fn abandoned_read_pins(
        &self,
    ) -> Result<Vec<(ReadTicketLease, Option<IndexError>)>, IndexError> {
        let pending = self
            .abandoned_read_pins
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let mut observed = Vec::new();
        observed
            .try_reserve_exact(pending.len())
            .map_err(|_allocation_error| IndexError::ResourceLimited)?;
        observed.extend(
            pending
                .iter()
                .map(|pin| (pin.ticket, pin.last_error.clone())),
        );
        drop(pending);
        Ok(observed)
    }
}

// None defers cleanup before any row is copied or lock is unlinked.
fn retired_snapshot(
    catalog: &SqliteCatalog,
    budget: &mut WorkBudget,
) -> Result<Option<RecoveryRows<RetiredRow>>, IndexError> {
    let connection = catalog.lock_native_budget(budget)?;
    let mut statement = connection
        .prepare(
            "SELECT id,
         CASE WHEN typeof(device)='blob' AND length(device)=8 THEN device END,
         CASE WHEN typeof(inode)='blob' AND length(inode)=8 THEN inode END,
         CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
         FROM native_read_session_retire_v1 ORDER BY id LIMIT ?1",
        )
        .map_err(|error| sql(&error))?;
    let mut rows = statement
        .query(params![RECOVERY_LOOKAHEAD_SLOTS as i64])
        .map_err(|error| sql(&error))?;
    let mut page = RecoveryRows::empty();
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 48,
            ..WorkUsage::default()
        })?;
        let device = row.get_ref(1).map_err(|error| sql(&error))?;
        let inode = row.get_ref(2).map_err(|error| sql(&error))?;
        let digest = row.get_ref(3).map_err(|error| sql(&error))?;
        let (Ok(device), Ok(inode), Ok(digest)) =
            (device.as_blob(), inode.as_blob(), digest.as_blob())
        else {
            return Err(IndexError::Storage(
                "retired read session row shape mismatch".into(),
            ));
        };
        if device.len() != 8 || inode.len() != 8 || digest.len() != 32 {
            return Err(IndexError::Storage(
                "retired read session row shape changed".into(),
            ));
        }
        if page.rows.is_empty() {
            match page.reserve(RECOVERY_LOOKAHEAD_SLOTS, 48, budget) {
                Ok(()) => {}
                Err(IndexError::ResourceLimited) => {
                    // Retirement is already durable. Leave the lock and its
                    // tombstone for the next funded cleanup, and keep the
                    // committed recovery cursor and counters observable.
                    budget.checkpoint()?;
                    return Ok(None);
                }
                Err(error) => return Err(error),
            }
        }
        page.rows.push((
            row.get(0).map_err(|error| sql(&error))?,
            device.to_vec(),
            inode.to_vec(),
            digest.to_vec(),
        ));
    }
    drop(rows);
    drop(statement);
    drop(connection);
    Ok(Some(page))
}

fn cleanup_retired(
    catalog: &SqliteCatalog,
    directory: &Path,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    let Some(RecoveryRows { rows, _memory }) = retired_snapshot(catalog, budget)? else {
        return Ok(true);
    };
    cleanup_retired_rows(catalog, directory, rows, budget)
}

fn cleanup_retired_rows(
    catalog: &SqliteCatalog,
    directory: &Path,
    retired: Vec<RetiredRow>,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    cleanup_retired_rows_with_hook(catalog, directory, retired, budget, |_| Ok(()))
}

fn cleanup_retired_rows_with_hook(
    catalog: &SqliteCatalog,
    directory: &Path,
    retired: Vec<RetiredRow>,
    budget: &mut WorkBudget,
    mut after_open: impl FnMut(&RetiredRow) -> Result<(), IndexError>,
) -> Result<bool, IndexError> {
    let limit = RECOVERY_LOOKAHEAD_SLOTS - 1;
    let mut has_more = retired.len() > limit;
    for row in retired.into_iter().take(limit) {
        budget.charge(WorkUsage {
            reads: 1,
            writes: 1,
            ..WorkUsage::default()
        })?;
        if row.3 != retired_digest(&row)? {
            return Err(IndexError::Storage(
                "native retired read session digest mismatch".into(),
            ));
        }
        let path = lock_path(directory, uint(row.0)?);
        let file = match open_lock(catalog, &path, false) {
            Ok(file) => Some(file),
            Err(_error) if matches!(fs::symlink_metadata(&path), Err(ref missing) if missing.kind() == std::io::ErrorKind::NotFound) => {
                None
            }
            Err(error) => return Err(error),
        };
        if let Some(ref file) = file {
            if inode(file)? != (row.1.clone(), row.2.clone()) {
                return Err(IndexError::Storage(
                    "retired read session lock inode mismatch".into(),
                ));
            }
            after_open(&row)?;
            match file.file()?.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => {
                    has_more = true;
                    continue;
                }
                Err(TryLockError::Error(error)) => {
                    return Err(io_error("lock retired file", &path, &error));
                }
            }
            match fs::symlink_metadata(&path) {
                Ok(metadata)
                    if metadata.dev().to_le_bytes().as_slice() == row.1.as_slice()
                        && metadata.ino().to_le_bytes().as_slice() == row.2.as_slice()
                        && metadata.is_file() =>
                {
                    if let Some(owner) = catalog.connection.admission() {
                        owner.unlink_lock_file(&path, file.file()?)?;
                    } else {
                        match fs::remove_file(&path) {
                            Ok(()) => {}
                            Err(error)
                                if error.kind() == std::io::ErrorKind::NotFound
                                    && matches!(fs::symlink_metadata(&path), Err(ref missing) if missing.kind() == std::io::ErrorKind::NotFound) =>
                                {}
                            Err(error) => {
                                return Err(io_error("unlink retired lock", &path, &error));
                            }
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(_) => {
                    return Err(IndexError::Storage(
                        "retired lock pathname changed inode".into(),
                    ));
                }
                Err(error) => return Err(io_error("stat retired lock pathname", &path, &error)),
            }
        }
        open_directory(directory)?
            .sync_all()
            .map_err(|error| io_error("sync retired directory", directory, &error))?;
        let mut connection = catalog.lock_native_budget(budget)?;
        let tx = begin(&mut connection)?;
        let current: Option<OptionalRetiredRow> = tx
            .query_row(
                "SELECT id,
                 CASE WHEN typeof(device)='blob' AND length(device)=8 THEN device END,
                 CASE WHEN typeof(inode)='blob' AND length(inode)=8 THEN inode END,
                 CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
                 FROM native_read_session_retire_v1 WHERE id=?1",
                params![row.0],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get::<_, Option<Vec<u8>>>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                        row.get::<_, Option<Vec<u8>>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| sql(&error))?;
        let (id, device, inode, digest) = match current {
            Some((id, Some(device), Some(inode), Some(digest))) => (id, device, inode, digest),
            Some(_) => {
                return Err(IndexError::Storage(
                    "retired read session row shape mismatch".into(),
                ));
            }
            None => {
                tx.commit().map_err(|error| sql(&error))?;
                drop(connection);
                continue;
            }
        };
        if device.len() != 8 || inode.len() != 8 || digest.len() != 32 {
            return Err(IndexError::Storage(
                "retired read session row shape changed".into(),
            ));
        }
        let current = (id, device, inode, digest);
        if current != row {
            return Err(IndexError::Storage(
                "retired read session changed during unlink".into(),
            ));
        }
        let changed = tx
            .execute(
                "DELETE FROM native_read_session_retire_v1 WHERE id=?1",
                params![row.0],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(changed, "confirm retired lock unlink")?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(connection);
    }
    Ok(has_more)
}

pub(super) fn checked_session(
    connection: &Connection,
    binding: ReadSessionRef,
) -> Result<SessionRow, IndexError> {
    let row = cached_query_row(
        connection,
        "SELECT id,fence,
             CASE WHEN typeof(device)='blob' AND length(device)=8 THEN device END,
             CASE WHEN typeof(inode)='blob' AND length(inode)=8 THEN inode END,
             CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
             FROM native_read_session_v1 WHERE id=?1",
        params![int(binding.id)?],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
            ))
        },
    )
    .optional()
    .map_err(|error| sql(&error))?
    .ok_or(IndexError::SnapshotUnavailable)?;
    let (Some(device), Some(inode), Some(digest)) = (row.2, row.3, row.4) else {
        return Err(IndexError::Storage(
            "native read session row shape mismatch".into(),
        ));
    };
    if device.len() != 8 || inode.len() != 8 || digest.len() != 32 {
        return Err(IndexError::Storage(
            "native read session row shape changed".into(),
        ));
    }
    let row: SessionRow = (row.0, row.1, device, inode, digest);
    if row.1 != int(binding.fence)? || row.4 != session_digest(&row)? {
        return Err(IndexError::Storage(
            "native read session fence or digest mismatch".into(),
        ));
    }
    Ok(row)
}

pub(super) struct AuthenticatedSessionIdentity<'a> {
    pub(super) binding: ReadSessionRef,
    pub(super) inode: (Vec<u8>, Vec<u8>),
    // The checked identity must remain scoped to the original lock owner.
    lease: &'a CatalogSessionLease,
}

pub(super) fn guard<'a>(
    catalog: &SqliteCatalog,
    lease: &'a dyn ReadSessionLease,
) -> Result<AuthenticatedSessionIdentity<'a>, IndexError> {
    let any: &dyn Any = lease;
    let concrete = any.downcast_ref::<CatalogSessionLease>().ok_or_else(|| {
        IndexError::InvalidInput("read session guard is not catalog owned".into())
    })?;
    if concrete.catalog_path != catalog.path {
        return Err(IndexError::InvalidInput(
            "read session belongs to another catalog".into(),
        ));
    }
    let directory = session_directory(catalog)?;
    let path = lock_path(&directory, concrete.binding.id);
    let fresh = open_lock(catalog, &path, false)?;
    let fresh_inode = inode(&fresh)?;
    let held_inode = inode(&concrete.file)?;
    if fresh_inode != held_inode {
        return Err(IndexError::Storage(
            "read session lock inode changed".into(),
        ));
    }
    // A second description cannot acquire an exclusive lock while this guard
    // owns it. File descriptors copied from another process cannot synthesize
    // this private concrete type.
    match fresh.file()?.try_lock() {
        Err(TryLockError::WouldBlock) => Ok(AuthenticatedSessionIdentity {
            binding: concrete.binding,
            inode: held_inode,
            lease: concrete,
        }),
        Ok(()) => Err(IndexError::Storage("read session OS lock was lost".into())),
        Err(TryLockError::Error(error)) => Err(io_error("probe guard lock", &path, &error)),
    }
}

fn pin_digest(row: &PinRow) -> Result<[u8; 32], IndexError> {
    struct DigestWriter(Sha256);

    impl std::io::Write for DigestWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut digest = DigestWriter(Sha256::new());
    digest.0.update(b"qidx.native.pin.v2");
    serde_json::to_writer(
        &mut digest,
        &(
            &row.0, &row.1, &row.2, &row.3, &row.4, &row.5, &row.6, &row.7,
        ),
    )
    .map_err(|error| IndexError::Storage(format!("encode native catalog row: {error}")))?;
    Ok(digest.0.finalize().into())
}

fn pin_row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<PinRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

fn canonical_pin_workspace(workspace: &[u8]) -> bool {
    struct MatchBytes<'a>(Option<&'a [u8]>);

    impl std::io::Write for MatchBytes<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.and_then(|tail| tail.strip_prefix(bytes));
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let Ok(decoded) = serde_json::from_slice::<WorkspaceKey>(workspace) else {
        return false;
    };
    let mut output = MatchBytes(Some(workspace));
    serde_json::to_writer(&mut output, &decoded).is_ok() && output.0 == Some(&[])
}

fn check_pin_row_shape(row: &PinRow, max_workspace_bytes: i64) -> Result<(), IndexError> {
    if row.5.is_empty()
        || row.5.len()
            > usize::try_from(max_workspace_bytes).map_err(|_error| IndexError::ResourceLimited)?
        || row.8.as_ref().is_some_and(|digest| digest.len() != 32)
        || !canonical_pin_workspace(&row.5)
    {
        return Err(IndexError::Storage("native pin row shape changed".into()));
    }
    Ok(())
}

fn check_pin_row_digest(row: &PinRow) -> Result<(), IndexError> {
    if row.8.as_deref() != Some(pin_digest(row)?.as_slice()) {
        return Err(IndexError::Storage("native pin digest mismatch".into()));
    }
    Ok(())
}

fn pin_row_scratch_bytes(workspace_bytes: i64) -> Result<u64, IndexError> {
    // Workspace and digest copies plus bounded parsing and diagnostic scratch.
    uint(workspace_bytes)?
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(32 + 256))
        .ok_or(IndexError::ResourceLimited)
}

pub(super) struct PinReadScratch {
    max_workspace_bytes: i64,
    _memory: Option<MemoryLease>,
}

impl PinReadScratch {
    pub(super) fn reserve(
        budget: Option<&WorkBudget>,
        lifetime: MemoryLifetime,
    ) -> Result<Self, IndexError> {
        let max_workspace_bytes = max_workspace_key_bytes()?;
        let memory = match budget {
            Some(budget) => {
                budget.reserve_memory(pin_row_scratch_bytes(max_workspace_bytes)?, lifetime)?
            }
            None => None,
        };
        Ok(Self {
            max_workspace_bytes,
            _memory: memory,
        })
    }
}

fn pin_data_version(transaction: &rusqlite::Transaction<'_>) -> rusqlite::Result<i64> {
    transaction.query_row("PRAGMA main.data_version", [], |row| row.get(0))
}

enum PinRowsError {
    Sql(rusqlite::Error),
    Check(IndexError),
}

impl From<rusqlite::Error> for PinRowsError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sql(error)
    }
}

impl From<IndexError> for PinRowsError {
    fn from(error: IndexError) -> Self {
        Self::Check(error)
    }
}

impl PinRowsError {
    fn into_index_error(self) -> IndexError {
        match self {
            Self::Sql(error) => sql(&error),
            Self::Check(error) => error,
        }
    }
}

pub(crate) fn certify_open_pin_rows(
    transaction: &rusqlite::Transaction<'_>,
    path: &Path,
) -> Result<i64, CoreError> {
    let version = pin_data_version(transaction)
        .map_err(|error| engine_error("read native pin data version", path, &error))?;
    verify_pin_rows(transaction, None).map_err(|error| match error {
        PinRowsError::Sql(error) => engine_error("validate persisted native pins", path, &error),
        PinRowsError::Check(error) => {
            crate::io_admission::storage("validate persisted native pins", path, &error)
        }
    })?;
    Ok(version)
}

impl SqliteCatalog {
    /// Catalog writers encode workspace keys before inserting pins and only
    /// renew or remove existing pins. Changes by another SQLite connection
    /// invalidate certification, including persisted noncanonical workspace
    /// encodings. Check the connection-local data version inside the same
    /// transaction that will use the workspace-first pin index.
    pub(super) fn require_pin_integrity(
        &self,
        transaction: &rusqlite::Transaction<'_>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        use std::sync::atomic::Ordering;

        budget.checkpoint()?;
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let version = pin_data_version(transaction).map_err(|error| sql(&error))?;
        if self.pin_data_version.load(Ordering::Relaxed) != version {
            verify_pin_rows(transaction, Some(budget)).map_err(PinRowsError::into_index_error)?;
            self.pin_data_version.store(version, Ordering::Relaxed);
        }
        Ok(())
    }
}

fn verify_pin_rows(
    connection: &Connection,
    mut budget: Option<&mut WorkBudget>,
) -> Result<(), PinRowsError> {
    let max_workspace_bytes = max_workspace_key_bytes()?;
    let mut statement = connection.prepare(
            "SELECT ticket,session_id,session_fence,fence,expires_at_ms,
             CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND ?1 THEN workspace END,
             epoch,sequence,
             CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END,
             CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND ?1 THEN length(workspace) END
             FROM native_pin_v1 ORDER BY ticket",
        )?;
    let mut rows = statement.query(params![max_workspace_bytes])?;
    loop {
        if let Some(budget) = budget.as_deref_mut() {
            budget.checkpoint()?;
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
        }
        let Some(record) = rows.next()? else {
            return Ok(());
        };
        let length: Option<i64> = record.get(9)?;
        let length =
            length.ok_or_else(|| IndexError::Storage("native pin row shape changed".into()))?;
        let row_bytes = pin_row_scratch_bytes(length)?;
        let _row_memory = if let Some(budget) = budget.as_deref_mut() {
            budget.charge(WorkUsage {
                bytes: row_bytes,
                ..WorkUsage::default()
            })?;
            budget.reserve_memory(row_bytes, MemoryLifetime::Temporary)?
        } else {
            None
        };
        let pin = pin_row_from_sql(record)?;
        check_pin_row_shape(&pin, max_workspace_bytes)?;
        check_pin_row_digest(&pin)?;
    }
}

pub(super) fn checked_pin(
    connection: &Connection,
    ticket: ReadTicketLease,
    scratch: &PinReadScratch,
) -> Result<PinRow, IndexError> {
    let max_workspace_bytes = scratch.max_workspace_bytes;
    let row: PinRow = cached_query_row(
        connection,
        "SELECT ticket,session_id,session_fence,fence,expires_at_ms,
         CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND ?2 THEN workspace END,
         epoch,sequence,
         CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
         FROM native_pin_v1 WHERE ticket=?1",
        params![ticket_to_integer(ticket.id)?, max_workspace_bytes],
        pin_row_from_sql,
    )
    .optional()
    .map_err(|error| sql(&error))?
    .ok_or(IndexError::SnapshotUnavailable)?;
    check_pin_row_shape(&row, max_workspace_bytes)?;
    if row.1 != int(ticket.session.id)?
        || row.2 != int(ticket.session.fence)?
        || row.3 != int(ticket.fence)?
        || row.4 != int(ticket.expires_at_ms)?
    {
        return Err(IndexError::SnapshotUnavailable);
    }
    check_pin_row_digest(&row)?;
    Ok(row)
}

// Both public reads and execution preparation use this original pin issuer.
// The caller already authenticated the same live session and sampled its clock
// inside the held transaction. No intermediate pin is committed by this helper.
#[derive(Clone, Copy)]
enum TicketAdmission {
    Read,
    SourceBodyScrub,
}

fn issue_ticket(
    catalog: &SqliteCatalog,
    tx: &rusqlite::Transaction<'_>,
    binding: ReadSessionRef,
    selection: ViewSelection,
    milliseconds: u64,
    now: u64,
    budget: &WorkBudget,
    admission: TicketAdmission,
) -> Result<(TicketRecord, bool), IndexError> {
    let expiry = now
        .checked_add(milliseconds)
        .ok_or(IndexError::ResourceLimited)?;
    // Hold one scratch grant through view lookup, root verification and pin
    // sealing. The requested workspace key is encoded once and kept throughout.
    let workspace_bytes =
        u64::try_from(max_workspace_key_bytes()?).map_err(|_| IndexError::ResourceLimited)?;
    // Workspace bytes are encoded as JSON integers (at most four bytes each).
    // The view-root input has two counters and a 32-byte object key, which also
    // bounds the pin input with seven counters and no object key.
    let root_digest_input = workspace_bytes
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_add(2 * 20 + 32 * 4 + 16))
        .and_then(u64::checked_next_power_of_two)
        .ok_or(IndexError::ResourceLimited)?;
    let ticket_backing = workspace_bytes
        .checked_next_power_of_two()
        .and_then(|bytes| bytes.checked_add(root_digest_input))
        .and_then(|bytes| {
            bytes.checked_add(
                u64::try_from(std::mem::size_of::<[ObjectRef; MAX_VIEW_ROOTS + 1]>()).ok()? + 32,
            )
        })
        .ok_or(IndexError::ResourceLimited)?;
    let _ticket_memory = budget.reserve_memory(ticket_backing, MemoryLifetime::Temporary)?;
    let workspace = match selection {
        ViewSelection::Latest { workspace, .. } => workspace,
        ViewSelection::Exact(identity) => identity.workspace,
    };
    let workspace_blob = workspace_key(workspace)?;
    let view = match selection {
        ViewSelection::Latest { epoch, .. } => {
            let current = current_view_by_key(tx, workspace, &workspace_blob)?
                .ok_or(IndexError::SnapshotUnavailable)?;
            if current.identity.epoch != epoch {
                return Err(IndexError::SnapshotUnavailable);
            }
            current
        }
        ViewSelection::Exact(identity) => {
            let registered: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM native_workspace_v1 WHERE workspace=?1)",
                    params![&workspace_blob],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            if !registered {
                return Err(IndexError::SnapshotUnavailable);
            }
            let floor =
                retention::exact_floor_for_epoch_by_key(tx, &workspace_blob, identity.epoch)?;
            if identity.sequence < floor {
                return Err(IndexError::SnapshotUnavailable);
            }
            read_view_row_by_key(tx, identity, &workspace_blob)?
        }
    };
    ensure_roots_with_budget(tx, &view, Some(budget))?;
    let quarantined = match view_quarantine::refuse_if_quarantined(
        catalog,
        tx,
        &workspace_blob,
        int(view.identity.epoch)?,
        int(view.identity.sequence)?,
        Some(budget),
    ) {
        Ok(()) => false,
        Err(IndexError::CorruptObject(_))
            if matches!(admission, TicketAdmission::SourceBodyScrub) =>
        {
            true
        }
        Err(error) => return Err(error),
    };
    budget.checkpoint()?;
    let fence = next_fence(tx)?;
    let session_id = int(binding.id)?;
    let session_fence = int(binding.fence)?;
    let fence_sql = int(fence)?;
    let expiry_sql = int(expiry)?;
    let epoch = int(view.identity.epoch)?;
    let sequence = int(view.identity.sequence)?;
    let inserted = tx.execute(
            "INSERT INTO native_pin_v1(session_id,session_fence,fence,expires_at_ms,workspace,epoch,sequence) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![session_id,session_fence,fence_sql,expiry_sql,&workspace_blob,epoch,sequence],
        ).map_err(|error| sql(&error))?;
    exactly_one(inserted, "insert read pin")?;
    let id = ticket_from_integer(tx.last_insert_rowid())?;
    let pin = (
        ticket_to_integer(id)?,
        session_id,
        session_fence,
        fence_sql,
        expiry_sql,
        workspace_blob,
        epoch,
        sequence,
        None,
    );
    let changed = tx
        .execute(
            "UPDATE native_pin_v1 SET row_digest=?2 WHERE ticket=?1",
            params![pin.0, pin_digest(&pin)?],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(changed, "seal read pin")?;
    Ok((
        TicketRecord {
            lease: ReadTicketLease {
                id,
                session: binding,
                fence,
                expires_at_ms: expiry,
            },
            view,
        },
        quarantined,
    ))
}

// A maintenance pin has retention custody only. Its lease is never exposed as
// a read ticket, so ordinary query validation still refuses quarantined views.
struct SourceScrubPin<'a> {
    catalog: &'a SqliteCatalog,
    session: Arc<dyn ReadSessionLease>,
    lease: Option<ReadTicketLease>,
}

impl SourceScrubPin<'_> {
    fn finish<T>(
        mut self,
        result: Result<T, IndexError>,
        budget: &WorkBudget,
    ) -> Result<T, IndexError> {
        let released = self
            .catalog
            .release_ticket(self.session.as_ref(), self.lease.expect("held scrub pin"));
        if released.is_ok() {
            self.lease = None;
        }
        match (result, released) {
            (Err(primary), _) => Err(primary),
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Ok(value), Ok(())) => {
                budget.checkpoint()?;
                Ok(value)
            }
        }
    }
}

impl Drop for SourceScrubPin<'_> {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        if self
            .catalog
            .release_ticket(self.session.as_ref(), lease)
            .is_err()
        {
            // The Catalog retains the exact session and pin until its existing
            // abandoned-pin recovery settles the failed release.
            drop(
                self.catalog
                    .abandon_ticket(Arc::clone(&self.session), lease),
            );
        }
    }
}

impl SqliteCatalog {
    /// Hold an exact retained view while a bounded Source-body page is read.
    /// The quarantine decision and pin insert share one Catalog transaction;
    /// only this maintenance callback can enter a quarantined view.
    pub fn with_source_body_scrub_pin<T>(
        &self,
        session: &Arc<dyn ReadSessionLease>,
        lease: Duration,
        view: ViewRef,
        budget: &mut WorkBudget,
        read_page: impl FnOnce(MapRoot, bool, &mut WorkBudget) -> Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        budget.checkpoint()?;
        let concrete = guard(self, session.as_ref())?;
        let milliseconds = validate_read_ticket_lease(lease)?;
        let mut connection = self.lock_native_budget(budget)?;
        let tx = begin(&mut connection)?;
        let session_row = checked_session(&tx, concrete.binding)?;
        if (session_row.2, session_row.3) != concrete.inode {
            return Err(IndexError::Storage("read session inode mismatch".into()));
        }
        let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
        let (record, previously_quarantined) = issue_ticket(
            self,
            &tx,
            concrete.binding,
            ViewSelection::Exact(view),
            milliseconds,
            now,
            budget,
            TicketAdmission::SourceBodyScrub,
        )?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(connection);
        let pin = SourceScrubPin {
            catalog: self,
            session: Arc::clone(session),
            lease: Some(record.lease),
        };
        let result = read_page(record.view.source.owners, previously_quarantined, budget);
        pin.finish(result, budget)
    }
}

impl SqliteCatalog {
    pub(super) fn acquire_materialization_ticket_impl(
        &self,
        session: &dyn ReadSessionLease,
        request: &UpdateRequest,
        lease: Duration,
        budget: &WorkBudget,
    ) -> Result<TicketRecord, IndexError> {
        let UpdateKind::Materialize { claim } = &request.kind else {
            return Err(IndexError::InvalidInput(
                "materialization preparation requires an owner claim".into(),
            ));
        };
        let concrete = guard(self, session)?;
        let milliseconds = validate_read_ticket_lease(lease)?;
        let mut connection = self.lock_native_budget(budget)?;
        let tx = begin(&mut connection)?;
        let session_row = checked_session(&tx, concrete.binding)?;
        if (session_row.2, session_row.3) != concrete.inode {
            return Err(IndexError::Storage("read session inode mismatch".into()));
        }
        let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
        let validated = if request.operation.producer == OWNED_WORK_PRODUCER {
            owned_work::validate_request_state(&tx, request, now)?
        } else {
            None
        };
        if let Some((work, assignment)) = validated {
            // The original request validator read and authenticated these rows
            // in this held transaction. Retain the live-claim checks without
            // decoding the same immutable work and assignment again.
            validate_live_claim_row(&work, claim, now, None)?;
            validate_owned_live_claim(&assignment, claim, now, None)?;
            if assignment.request.operation != request.operation {
                return Err(IndexError::WorkFenceLost);
            }
        } else {
            // Terminal owned requests and ordinary materialization preserve
            // the original validation sequence and rejection precedence.
            validate_materialization_claim(&tx, request, now)?;
            let _live = live_claim(&tx, claim, now)?;
        }
        let operation_scope = (request.operation.workspace, request.operation.epoch);
        if (request.expected_view.workspace, request.expected_view.epoch) != operation_scope
            || (claim.intent.workspace, claim.intent.epoch) != operation_scope
        {
            return Err(IndexError::InvalidInput(
                "materialization request belongs to another workspace epoch".into(),
            ));
        }
        let (ticket, _) = issue_ticket(
            self,
            &tx,
            concrete.binding,
            ViewSelection::Latest {
                workspace: request.expected_view.workspace,
                epoch: request.expected_view.epoch,
            },
            milliseconds,
            now,
            budget,
            TicketAdmission::Read,
        )?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(connection);
        Ok(ticket)
    }
}

// SqliteWork lanes 200-205 observe session guard, connection acquisition,
// BEGIN, authenticated rows, durable clock and COMMIT, in that order.
fn observe_ticket_validation<T>(
    catalog: &SqliteCatalog,
    budget: Option<&WorkBudget>,
    lane: u16,
    work: impl FnOnce() -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let span = crate::io_admission::catalog_work_span(
        budget.and_then(|budget| budget.request().diagnostic_context()),
        DiagnosticStageV1::SqliteWork,
        DiagnosticLaneV1(lane),
    );
    let result = crate::ticket_diagnostics::observe(catalog, lane, work);
    if let Some(span) = span {
        let _finished = span.finish(DiagnosticTerminalV1::from_result(&result));
    }
    result
}

fn validate_live_ticket(
    catalog: &SqliteCatalog,
    session: &dyn ReadSessionLease,
    ticket: ReadTicketLease,
    budget: Option<&WorkBudget>,
) -> Result<(), IndexError> {
    let concrete = observe_ticket_validation(catalog, budget, 200, || guard(catalog, session))?;
    if ticket.session != concrete.binding {
        return Err(IndexError::SnapshotUnavailable);
    }
    let mut connection = observe_ticket_validation(catalog, budget, 201, || {
        budget.map_or_else(
            || {
                catalog
                    .lock()
                    .map_err(|error| IndexError::Storage(error.to_string()))
            },
            |budget| catalog.lock_native_budget(budget),
        )
    })?;
    let connection_for_begin = &mut *connection;
    let tx = observe_ticket_validation(catalog, budget, 202, move || begin(connection_for_begin))?;
    let (_pin_scratch, pin) = observe_ticket_validation(catalog, budget, 203, || {
        let session_row = checked_session(&tx, concrete.binding)?;
        if (session_row.2, session_row.3) != concrete.inode {
            return Err(IndexError::Storage("read session inode mismatch".into()));
        }
        let scratch = PinReadScratch::reserve(budget, MemoryLifetime::Temporary)?;
        let pin = checked_pin(&tx, ticket, &scratch)?;
        Ok((scratch, pin))
    })?;
    let now = observe_ticket_validation(catalog, budget, 204, || {
        catalog_now_ms(&tx, catalog.clock.now_unix_ms())
    })?;
    if now >= uint(pin.4)? {
        return Err(IndexError::SnapshotUnavailable);
    }
    view_quarantine::refuse_if_quarantined(catalog, &tx, &pin.5, pin.6, pin.7, budget)?;
    if let Some(budget) = budget {
        budget.checkpoint()?;
    }
    let result = observe_ticket_validation(catalog, budget, 205, || {
        tx.commit().map_err(|error| sql(&error))
    });
    drop(connection);
    result
}

fn renew_live_ticket(
    catalog: &SqliteCatalog,
    session: &dyn ReadSessionLease,
    ticket: ReadTicketLease,
    lease: Duration,
    budget: Option<&WorkBudget>,
) -> Result<ReadTicketLease, IndexError> {
    let concrete = guard(catalog, session)?;
    if ticket.session != concrete.binding {
        return Err(IndexError::SnapshotUnavailable);
    }
    let milliseconds = validate_read_ticket_lease(lease)?;
    let mut connection = match budget {
        Some(budget) => catalog.lock_native_budget(budget)?,
        None => catalog
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?,
    };
    let tx = begin(&mut connection)?;
    let session_row = checked_session(&tx, concrete.binding)?;
    if (session_row.2, session_row.3) != concrete.inode {
        return Err(IndexError::Storage("read session inode mismatch".into()));
    }
    let pin_scratch = PinReadScratch::reserve(budget, MemoryLifetime::Temporary)?;
    let pin = checked_pin(&tx, ticket, &pin_scratch)?;
    view_quarantine::refuse_if_quarantined(catalog, &tx, &pin.5, pin.6, pin.7, budget)?;
    let now = catalog_now_ms(&tx, catalog.clock.now_unix_ms())?;
    if now >= uint(pin.4)? {
        return Err(IndexError::SnapshotUnavailable);
    }
    let next = ReadTicketLease {
        fence: next_fence(&tx)?,
        expires_at_ms: now
            .checked_add(milliseconds)
            .ok_or(IndexError::ResourceLimited)?,
        ..ticket
    };
    let updated: PinRow = (
        pin.0,
        pin.1,
        pin.2,
        int(next.fence)?,
        int(next.expires_at_ms)?,
        pin.5,
        pin.6,
        pin.7,
        None,
    );
    let changed = tx.execute("UPDATE native_pin_v1 SET fence=?2,expires_at_ms=?3,row_digest=?4 WHERE ticket=?1 AND fence=?5",params![updated.0,updated.3,updated.4,pin_digest(&updated)?,pin.3]).map_err(|error| sql(&error))?;
    exactly_one(changed, "renew read pin")?;
    if let Some(budget) = budget {
        budget.checkpoint()?;
    }
    tx.commit().map_err(|error| sql(&error))?;
    drop(connection);
    Ok(next)
}

impl CatalogReadPort for SqliteCatalog {
    fn begin_read_attempt<'a>(
        &'a self,
        session: &'a dyn ReadSessionLease,
        ticket: ReadTicketLease,
        budget: &'a WorkBudget,
    ) -> Result<Box<dyn quanta_index_core::index::CatalogReadAttempt + 'a>, IndexError> {
        read_attempt::start(self, session, ticket, budget)
    }

    fn open_read_session(&self) -> Result<Arc<dyn ReadSessionLease>, IndexError> {
        let directory = session_directory(self)?;
        let _creation_lock = creation_lock(self, &directory)?.ok_or(IndexError::ResourceLimited)?;
        // Commit the allocator first. A failed file creation burns the number,
        // so neither restart nor retry can adopt an earlier private lock.
        let mut connection = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut connection)?;
        let binding = ReadSessionRef {
            id: next_fence(&tx)?,
            fence: next_fence(&tx)?,
        };
        let inserted = tx
            .execute(
                "INSERT INTO native_read_session_open_v1(id,fence,row_digest) VALUES (?1,?2,?3)",
                params![
                    int(binding.id)?,
                    int(binding.fence)?,
                    creation::open_intent_digest(binding)?
                ],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(inserted, "record read session open intent")?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(connection);
        #[cfg(test)]
        read_open_failpoint("intent");
        let path = lock_path(&directory, binding.id);
        let file = open_lock(self, &path, true)?;
        file.file()?
            .try_lock()
            .map_err(|error| io_error("lock", &path, &error))?;
        file.file()?
            .sync_all()
            .map_err(|error| io_error("sync lock", &path, &error))?;
        open_directory(&directory)?
            .sync_all()
            .map_err(|error| io_error("sync directory", &directory, &error))?;
        #[cfg(test)]
        read_open_failpoint("file");
        let (device, inode) = inode(&file)?;
        let row: SessionRow = (
            int(binding.id)?,
            int(binding.fence)?,
            device,
            inode,
            Vec::new(),
        );
        let digest = session_digest(&row)?;
        let mut connection = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut connection)?;
        let inserted = tx.execute(
            "INSERT INTO native_read_session_v1(id,fence,device,inode,row_digest) VALUES (?1,?2,?3,?4,?5)",
            params![row.0,row.1,row.2,row.3,digest],
        ).map_err(|error| sql(&error))?;
        exactly_one(inserted, "insert read session")?;
        let deleted = tx
            .execute(
                "DELETE FROM native_read_session_open_v1 WHERE id=?1 AND fence=?2",
                params![int(binding.id)?, int(binding.fence)?],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(deleted, "transfer read session open intent")?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(connection);
        #[cfg(test)]
        read_open_failpoint("session");
        Ok(Arc::new(CatalogSessionLease {
            binding,
            catalog_path: self.path.clone(),
            file,
            read_observations: std::sync::Mutex::default(),
        }))
    }

    fn acquire_ticket(
        &self,
        session: &dyn ReadSessionLease,
        selection: ViewSelection,
        lease: Duration,
        budget: &WorkBudget,
    ) -> Result<TicketRecord, IndexError> {
        let concrete = guard(self, session)?;
        let milliseconds = validate_read_ticket_lease(lease)?;
        let mut connection = self.lock_native_budget(budget)?;
        let tx = begin(&mut connection)?;
        let session_row = checked_session(&tx, concrete.binding)?;
        if (session_row.2, session_row.3) != concrete.inode {
            return Err(IndexError::Storage("read session inode mismatch".into()));
        }
        let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
        let (ticket, _) = issue_ticket(
            self,
            &tx,
            concrete.binding,
            selection,
            milliseconds,
            now,
            budget,
            TicketAdmission::Read,
        )?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(connection);
        Ok(ticket)
    }

    fn renew_ticket(
        &self,
        session: &dyn ReadSessionLease,
        ticket: ReadTicketLease,
        lease: Duration,
    ) -> Result<ReadTicketLease, IndexError> {
        renew_live_ticket(self, session, ticket, lease, None)
    }

    fn renew_ticket_for_request(
        &self,
        session: &dyn ReadSessionLease,
        ticket: ReadTicketLease,
        lease: Duration,
        budget: &WorkBudget,
    ) -> Result<ReadTicketLease, IndexError> {
        renew_live_ticket(self, session, ticket, lease, Some(budget))
    }

    fn validate_ticket(
        &self,
        session: &dyn ReadSessionLease,
        ticket: ReadTicketLease,
    ) -> Result<(), IndexError> {
        validate_live_ticket(self, session, ticket, None)
    }

    fn validate_ticket_for_request(
        &self,
        session: &dyn ReadSessionLease,
        ticket: ReadTicketLease,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        validate_live_ticket(self, session, ticket, Some(budget))
    }

    fn release_ticket(
        &self,
        session: &dyn ReadSessionLease,
        ticket: ReadTicketLease,
    ) -> Result<(), IndexError> {
        let concrete = guard(self, session)?;
        if ticket.session != concrete.binding {
            return Err(IndexError::SnapshotUnavailable);
        }
        let mut connection = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut connection)?;
        let session_row = checked_session(&tx, concrete.binding)?;
        if (session_row.2, session_row.3) != concrete.inode {
            return Err(IndexError::Storage("read session inode mismatch".into()));
        }
        let pin_scratch = PinReadScratch::reserve(None, MemoryLifetime::Temporary)?;
        let pin = checked_pin(&tx, ticket, &pin_scratch)?;
        let changed = tx
            .execute(
                "DELETE FROM native_pin_v1 WHERE ticket=?1 AND fence=?2",
                params![pin.0, pin.3],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(changed, "release read pin")?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(connection);
        self.record_released_pin(&pin.5, pin.6, pin.7);
        Ok(())
    }

    fn abandon_ticket(
        &self,
        session: Arc<dyn ReadSessionLease>,
        ticket: ReadTicketLease,
    ) -> Result<(), IndexError> {
        // Ownership is checked without I/O: the pin must belong to a session
        // this Catalog issued, and the retry uses that same session lease.
        let any: &dyn Any = session.as_ref();
        let concrete = any.downcast_ref::<CatalogSessionLease>().ok_or_else(|| {
            IndexError::InvalidInput("abandoned read session is not catalog owned".into())
        })?;
        if concrete.catalog_path != self.path || ticket.session != concrete.binding {
            return Err(IndexError::InvalidInput(
                "abandoned read pin belongs to another catalog or session".into(),
            ));
        }
        let mut pending = self
            .abandoned_read_pins
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        pending
            .try_reserve(1)
            .map_err(|_allocation_error| IndexError::ResourceLimited)?;
        pending.push(AbandonedReadPin {
            session,
            ticket,
            last_error: None,
        });
        drop(pending);
        self.record_read_recovery_demand();
        Ok(())
    }

    fn recover_read_sessions(
        &self,
        after: Option<ReadSessionRef>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ReadSessionRecoveryPage, IndexError> {
        if limit == 0 || limit > MAX_RECOVERY_SESSIONS {
            return Err(IndexError::InvalidInput(
                "read session recovery limit must be 1..=256".into(),
            ));
        }
        if after.is_some_and(|value| value.id == 0 || value.fence == 0) {
            return Err(IndexError::InvalidInput(
                "invalid read recovery cursor".into(),
            ));
        }
        let (abandoned_settled, abandoned_deferred) = settle_abandoned_pins(self, limit, budget)?;
        let directory = session_directory(self)?;
        let mut cursor = after;
        let mut cursor_id = after.map(|value| int(value.id)).transpose()?.unwrap_or(0);
        let max_workspace_bytes = max_workspace_key_bytes()?;
        budget.checkpoint()?;
        let mut page = {
            let connection = self.lock_native_budget(budget)?;
            let mut statement = connection
                .prepare(
                    "SELECT id,fence,
                 CASE WHEN typeof(device)='blob' AND length(device)=8 THEN device END,
                 CASE WHEN typeof(inode)='blob' AND length(inode)=8 THEN inode END,
                 CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
                 FROM native_read_session_v1 WHERE id>?1 ORDER BY id LIMIT ?2",
                )
                .map_err(|error| sql(&error))?;
            let mut rows = statement
                .query(params![cursor_id, i64::from(limit)])
                .map_err(|error| sql(&error))?;
            let mut page = RecoveryRows::<SessionRecoveryCandidate>::empty();
            while let Some(row) = rows.next().map_err(|error| sql(&error))? {
                budget.charge(WorkUsage {
                    reads: 1,
                    bytes: 48,
                    ..WorkUsage::default()
                })?;
                if page.rows.is_empty() {
                    // Fund one page before copying blobs. Its rows also hold the
                    // OS probes, so their backing survives until retirement commits.
                    let capacity =
                        usize::try_from(limit).map_err(|_| IndexError::ResourceLimited)?;
                    page.reserve(capacity, 48, budget)?;
                }
                let (device, inode, digest): OptionalSessionIdentity = (
                    row.get(2).map_err(|error| sql(&error))?,
                    row.get(3).map_err(|error| sql(&error))?,
                    row.get(4).map_err(|error| sql(&error))?,
                );
                let (Some(device), Some(inode), Some(digest)) = (device, inode, digest) else {
                    return Err(IndexError::Storage(
                        "native read session row shape mismatch".into(),
                    ));
                };
                if device.len() != 8 || inode.len() != 8 || digest.len() != 32 {
                    return Err(IndexError::Storage(
                        "native read session row shape changed".into(),
                    ));
                }
                page.rows.push((
                    (
                        row.get(0).map_err(|error| sql(&error))?,
                        row.get(1).map_err(|error| sql(&error))?,
                        device,
                        inode,
                        digest,
                    ),
                    None,
                ));
            }
            drop(rows);
            drop(statement);
            drop(connection);
            page
        };
        // No filesystem operation runs under the Catalog mutex or SQL transaction.
        // A successfully probed dead lock remains owned until after retirement commit.
        for (row, dead_lock) in &mut page.rows {
            budget.checkpoint()?;
            if row.4 != session_digest(row)? {
                return Err(IndexError::Storage(
                    "native read session digest mismatch".into(),
                ));
            }
            let path = lock_path(&directory, uint(row.0)?);
            let file = open_lock(self, &path, false)?;
            if inode(&file)? != (row.2.clone(), row.3.clone()) {
                return Err(IndexError::Storage(
                    "read recovery lock inode mismatch".into(),
                ));
            }
            *dead_lock = match file.file()?.try_lock() {
                Ok(()) => Some(file),
                Err(TryLockError::WouldBlock) => None,
                Err(TryLockError::Error(error)) => {
                    return Err(io_error("probe recovery lock", &path, &error));
                }
            };
        }
        let open_intents = prepare_open_intents(self, &directory, budget)?;
        let more_intents = open_intents.has_more;
        let mut connection = self.lock_native_budget(budget)?;
        let tx = begin(&mut connection)?;
        let open_intents_reaped = reap_open_intents(&tx, &open_intents, budget)?;
        let mut recovered = 0u32;
        let mut released_views: Vec<(Vec<u8>, i64, i64)> = Vec::new();
        let mut pins_released = 0u32;
        let mut partial = false;
        for (row, dead_lock) in &page.rows {
            let binding = ReadSessionRef {
                id: uint(row.0)?,
                fence: uint(row.1)?,
            };
            if checked_session(&tx, binding)? != *row {
                return Err(IndexError::Storage(
                    "read session changed between probe and retirement".into(),
                ));
            }
            if dead_lock.is_none() {
                cursor = Some(binding);
                cursor_id = row.0;
                continue;
            }
            while pins_released < MAX_RECOVERY_PINS {
                budget.charge(WorkUsage {
                    reads: 1,
                    ..WorkUsage::default()
                })?;
                let pin = {
                    let mut statement = tx.prepare(
                        "SELECT ticket,session_id,session_fence,fence,expires_at_ms,
                         CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND ?2 THEN workspace END,
                         epoch,sequence,
                         CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END,
                         CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND ?2 THEN length(workspace) END
                         FROM native_pin_v1 WHERE session_id=?1 ORDER BY ticket LIMIT 1",
                    ).map_err(|error| sql(&error))?;
                    let mut rows = statement
                        .query(params![row.0, max_workspace_bytes])
                        .map_err(|error| sql(&error))?;
                    if let Some(record) = rows.next().map_err(|error| sql(&error))? {
                        let length: Option<i64> = record.get(9).map_err(|error| sql(&error))?;
                        let length = length.ok_or_else(|| {
                            IndexError::Storage(
                                "native recovery pin workspace shape mismatch".into(),
                            )
                        })?;
                        budget.charge(WorkUsage {
                            bytes: uint(length)?
                                .checked_add(32)
                                .ok_or(IndexError::ResourceLimited)?,
                            ..WorkUsage::default()
                        })?;
                        let memory = budget.reserve_memory(
                            pin_row_scratch_bytes(length)?,
                            MemoryLifetime::Temporary,
                        )?;
                        Some((
                            memory,
                            pin_row_from_sql(record).map_err(|error| sql(&error))?,
                            usize::try_from(length)
                                .map_err(|_error| IndexError::ResourceLimited)?,
                        ))
                    } else {
                        None
                    }
                };
                let Some((_pin_memory, pin, workspace_bytes)) = pin else {
                    break;
                };
                if pin.5.len() != workspace_bytes
                    || pin.8.as_ref().is_some_and(|digest| digest.len() != 32)
                    || !canonical_pin_workspace(&pin.5)
                {
                    return Err(IndexError::Storage(
                        "native recovery pin row shape changed".into(),
                    ));
                }
                if pin.1 != row.0
                    || pin.2 != row.1
                    || pin.8.as_deref() != Some(pin_digest(&pin)?.as_slice())
                {
                    return Err(IndexError::Storage(
                        "native recovery pin digest mismatch".into(),
                    ));
                }
                budget.charge(WorkUsage {
                    writes: 1,
                    ..WorkUsage::default()
                })?;
                let changed = tx
                    .execute("DELETE FROM native_pin_v1 WHERE ticket=?1", params![pin.0])
                    .map_err(|error| sql(&error))?;
                exactly_one(changed, "retire dead read pin")?;
                released_views
                    .try_reserve(1)
                    .map_err(|_allocation_error| IndexError::ResourceLimited)?;
                released_views.push((pin.5.clone(), pin.6, pin.7));
                pins_released = pins_released
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
            let remaining: i64 = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM native_pin_v1 WHERE session_id=?1)",
                    params![row.0],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            if remaining != 0 {
                partial = true;
                break;
            }
            budget.charge(WorkUsage {
                writes: 2,
                ..WorkUsage::default()
            })?;
            let retired: RetiredRow = (row.0, row.2.clone(), row.3.clone(), Vec::new());
            let inserted = tx.execute(
                "INSERT INTO native_read_session_retire_v1(id,device,inode,row_digest) VALUES (?1,?2,?3,?4)",
                params![retired.0,&retired.1,&retired.2,retired_digest(&retired)?],
            ).map_err(|error| sql(&error))?;
            exactly_one(inserted, "record retired read lock")?;
            let deleted = tx
                .execute(
                    "DELETE FROM native_read_session_v1 WHERE id=?1 AND fence=?2",
                    params![row.0, row.1],
                )
                .map_err(|error| sql(&error))?;
            exactly_one(deleted, "retire dead read session")?;
            recovered = recovered
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            cursor = Some(binding);
            cursor_id = row.0;
        }
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let more_sessions: bool = partial
            || tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM native_read_session_v1 WHERE id>?1)",
                    params![cursor_id],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(connection);
        for (workspace, epoch, sequence) in &released_views {
            self.record_released_pin(workspace, *epoch, *sequence);
        }
        drop(open_intents);
        drop(page);
        let more_locks = cleanup_retired(self, &directory, budget)?;
        Ok(ReadSessionRecoveryPage {
            after: cursor,
            has_more: more_intents || more_sessions || more_locks,
            recovered,
            pins_released,
            open_intents_reaped,
            abandoned_settled,
            abandoned_deferred,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use quanta_index_core::index::{
        MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort, WorkLimits,
    };

    use super::{
        CatalogReadPort, CollectionId, Connection, IndexError, PinReadScratch, PinRow,
        PinRowsError, ReadSessionRef, ReadTicketLease, RetiredRow, SlotId, SqliteCatalog, TenantId,
        WorkBudget, WorkspaceKey, WorktreeId, begin, checked_pin, checked_session,
        cleanup_retired_rows, cleanup_retired_rows_with_hook, decode, fields_digest, int,
        lock_path, max_workspace_key_bytes, params, pin_digest, retired_digest, retired_snapshot,
        session_directory, ticket_from_integer, verify_pin_rows, workspace_key,
    };

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(format!("assertion failed: {}", stringify!($condition)).into());
            }
        }};
    }

    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if left != right {
                return Err(format!("assertion failed: {left:?} != {right:?}").into());
            }
        }};
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 1000,
                writes: 1000,
                bytes: 64 * 1024,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    struct ScratchLimit(u64);

    impl MemoryLeaseOwner for ScratchLimit {
        fn release(&self, _bytes: u64) {}
    }

    impl MemoryScopePort for ScratchLimit {
        fn reserve(&self, bytes: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
            if bytes > self.0 {
                return Err(IndexError::ResourceLimited);
            }
            Ok(MemoryLease::new(bytes, Arc::new(Self(self.0))))
        }
    }

    #[test]
    fn widest_canonical_workspace_pin_is_accepted_and_corrupt_shapes_are_rejected()
    -> Result<(), Box<dyn Error>> {
        let connection = Connection::open_in_memory()?;
        connection.execute_batch(
            "CREATE TABLE native_pin_v1 (
                ticket INTEGER, session_id INTEGER, session_fence INTEGER, fence INTEGER,
                expires_at_ms INTEGER, workspace BLOB, epoch INTEGER, sequence INTEGER,
                row_digest BLOB
            );",
        )?;
        let widest = WorkspaceKey {
            tenant: TenantId([u8::MAX; 16]),
            collection: CollectionId([u8::MAX; 16]),
            worktree: WorktreeId([u8::MAX; 16]),
            slot: SlotId([u8::MAX; 16]),
        };
        let encoded = workspace_key(widest)?;
        check_eq!(i64::try_from(encoded.len())?, max_workspace_key_bytes()?);
        let pin: PinRow = (1, 1, 1, 1, 1_000, encoded, 1, 1, None);
        let persisted = fields_digest(
            b"qidx.native.pin.v2",
            &(1_i64, 1_i64, 1_i64, 1_i64, 1_000_i64, &pin.5, 1_i64, 1_i64),
        )?;
        let digest = pin_digest(&pin)?;
        check_eq!(digest.as_slice(), persisted.as_slice());
        let _changed = connection.execute(
            "INSERT INTO native_pin_v1 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                pin.0,
                pin.1,
                pin.2,
                pin.3,
                pin.4,
                &pin.5,
                pin.6,
                pin.7,
                pin_digest(&pin)?
            ],
        )?;
        let ticket = ReadTicketLease {
            id: ticket_from_integer(1)?,
            session: ReadSessionRef { id: 1, fence: 1 },
            fence: 1,
            expires_at_ms: 1_000,
        };
        let unscoped = PinReadScratch::reserve(None, MemoryLifetime::Temporary)?;
        check_eq!(checked_pin(&connection, ticket, &unscoped)?.5, pin.5);
        let scratch_bytes = 2 * u64::try_from(pin.5.len())? + 32 + 256;
        let mut work = budget()?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(ScratchLimit(
            scratch_bytes - 1,
        )))));
        check_eq!(
            verify_pin_rows(&connection, Some(&mut work)).map_err(PinRowsError::into_index_error),
            Err(IndexError::ResourceLimited)
        );
        check!(matches!(
            PinReadScratch::reserve(Some(&work), MemoryLifetime::Temporary),
            Err(IndexError::ResourceLimited)
        ));
        let mut work = budget()?;
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(ScratchLimit(
            scratch_bytes,
        )))));
        verify_pin_rows(&connection, Some(&mut work)).map_err(PinRowsError::into_index_error)?;
        let funded = PinReadScratch::reserve(Some(&work), MemoryLifetime::Temporary)?;
        check_eq!(checked_pin(&connection, ticket, &funded)?.5, pin.5);
        // A rechecksummed JSON alias would pass the row digest but would not
        // match the canonical workspace key used by Exact pin/reclaim joins.
        let shorter = WorkspaceKey {
            tenant: TenantId([0; 16]),
            collection: CollectionId([0; 16]),
            worktree: WorktreeId([0; 16]),
            slot: SlotId([0; 16]),
        };
        let mut alias = workspace_key(shorter)?;
        alias.push(b' ');
        check_eq!(decode::<WorkspaceKey>(&alias)?, shorter);
        check!(i64::try_from(alias.len())? <= max_workspace_key_bytes()?);
        let aliased: PinRow = (pin.0, pin.1, pin.2, pin.3, pin.4, alias, pin.6, pin.7, None);
        check_eq!(
            connection.execute(
                "UPDATE native_pin_v1 SET workspace=?1,row_digest=?2 WHERE ticket=?3",
                params![&aliased.5, pin_digest(&aliased)?, aliased.0],
            )?,
            1
        );
        check_eq!(
            checked_pin(&connection, ticket, &unscoped),
            Err(IndexError::Storage("native pin row shape changed".into()))
        );
        check_eq!(
            verify_pin_rows(&connection, Some(&mut budget()?))
                .map_err(PinRowsError::into_index_error),
            Err(IndexError::Storage("native pin row shape changed".into()))
        );
        for corrupted in [
            "UPDATE native_pin_v1 SET workspace=zeroblob(0)",
            "UPDATE native_pin_v1 SET workspace=zeroblob(1048577)",
            "UPDATE native_pin_v1 SET workspace='text'",
        ] {
            let _changed = connection.execute(corrupted, [])?;
            check!(matches!(
                checked_pin(&connection, ticket, &unscoped),
                Err(IndexError::Storage(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn stale_retirement_snapshot_and_open_fd_are_idempotent_after_other_unlink()
    -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(1))?;
        let session = catalog.open_read_session()?;
        let binding = session.binding();
        let mut connection = catalog.lock()?;
        let tx = begin(&mut connection)?;
        let original = checked_session(&tx, binding)?;
        let retired: RetiredRow = (original.0, original.2, original.3, Vec::new());
        let inserted = tx.execute(
            "INSERT INTO native_read_session_retire_v1(id,device,inode,row_digest) VALUES (?1,?2,?3,?4)",
            params![retired.0,&retired.1,&retired.2,retired_digest(&retired)?],
        )?;
        if inserted != 1 {
            return Err(format!("expected one retirement row, inserted {inserted}").into());
        }
        let deleted = tx.execute(
            "DELETE FROM native_read_session_v1 WHERE id=?1",
            params![retired.0],
        )?;
        if deleted != 1 {
            return Err(format!("expected one live session deletion, deleted {deleted}").into());
        }
        tx.commit()?;
        drop(connection);
        drop(session);

        let directory = session_directory(&catalog)?;
        let stale = retired_snapshot(&catalog, &mut budget()?)?
            .ok_or("unscoped retirement snapshot deferred")?
            .rows;
        let fresh = stale.clone();
        if stale.len() != 1 {
            return Err(format!("expected one stale retirement row, found {}", stale.len()).into());
        }
        let mut other_budget = budget()?;
        let mut stale_budget = budget()?;
        let mut invoked = false;
        let _has_more =
            cleanup_retired_rows_with_hook(&catalog, &directory, stale, &mut stale_budget, |_| {
                if !invoked {
                    let _other_has_more = cleanup_retired_rows(
                        &catalog,
                        &directory,
                        fresh.clone(),
                        &mut other_budget,
                    )?;
                    invoked = true;
                }
                Ok(())
            })?;
        if !invoked {
            return Err("concurrent retirement cleanup hook was not invoked".into());
        }
        if !retired_snapshot(&catalog, &mut budget()?)?
            .ok_or("unscoped retirement snapshot deferred")?
            .rows
            .is_empty()
        {
            return Err("retirement row survived concurrent cleanup".into());
        }
        if lock_path(&directory, binding.id).exists() {
            return Err("retired session lock survived concurrent cleanup".into());
        }
        Ok(())
    }

    #[test]
    fn oversized_session_identity_is_rejected_without_retiring_lease() -> Result<(), Box<dyn Error>>
    {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(1))?;
        let session = catalog.open_read_session()?;
        let binding = session.binding();
        let directory = session_directory(&catalog)?;
        let path = lock_path(&directory, binding.id);
        let valid = {
            let connection = catalog.lock()?;
            checked_session(&connection, binding)?
        };
        let mut budget = budget()?;
        for column in ["device", "inode", "row_digest"] {
            let (short, long) = if column == "row_digest" {
                ("zeroblob(31)", "zeroblob(33)")
            } else {
                ("zeroblob(7)", "zeroblob(9)")
            };
            for corrupt_value in [short, long, "zeroblob(1048577)", "'text'"] {
                let _changed = catalog.lock()?.execute(
                    "UPDATE native_read_session_v1 SET device=?2,inode=?3,row_digest=?4 WHERE id=?1",
                    params![int(binding.id)?, &valid.2, &valid.3, &valid.4],
                )?;
                let _changed = catalog.lock()?.execute(
                    &format!(
                        "UPDATE native_read_session_v1 SET {column}={corrupt_value} WHERE id=?1"
                    ),
                    params![int(binding.id)?],
                )?;
                check!(matches!(
                    catalog.recover_read_sessions(None, 1, &mut budget),
                    Err(IndexError::Storage(_))
                ));
            }
        }
        check_eq!(budget.usage().bytes, 12 * 48);
        check!(path.exists());
        let count: i64 = catalog.lock()?.query_row(
            "SELECT count(*) FROM native_read_session_v1 WHERE id=?1",
            params![int(binding.id)?],
            |row| row.get(0),
        )?;
        check_eq!(count, 1);
        Ok(())
    }

    #[test]
    fn retired_snapshot_rejects_corrupt_blob_shapes_without_deleting_tombstone()
    -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(1))?;
        let retired: RetiredRow = (1, vec![0; 8], vec![0; 8], Vec::new());
        let _changed = catalog.lock()?.execute(
            "INSERT INTO native_read_session_retire_v1(id,device,inode,row_digest) VALUES (?1,?2,?3,?4)",
            params![retired.0,&retired.1,&retired.2,retired_digest(&retired)?],
        )?;
        let digest = retired_digest(&retired)?;
        for column in ["device", "inode", "row_digest"] {
            let (short, long) = if column == "row_digest" {
                ("zeroblob(31)", "zeroblob(33)")
            } else {
                ("zeroblob(7)", "zeroblob(9)")
            };
            for corrupt_value in [short, long, "zeroblob(1048577)", "'text'"] {
                let _changed = catalog.lock()?.execute(
                    "UPDATE native_read_session_retire_v1 SET device=?1,inode=?2,row_digest=?3 WHERE id=1",
                    params![&retired.1,&retired.2,&digest],
                )?;
                let _changed = catalog.lock()?.execute(
                    &format!("UPDATE native_read_session_retire_v1 SET {column}={corrupt_value} WHERE id=1"),
                    [],
                )?;
                let mut work = budget()?;
                work.bind_memory_scope(Some(MemoryScope::new(Arc::new(ScratchLimit(0)))));
                check!(matches!(
                    retired_snapshot(&catalog, &mut work),
                    Err(IndexError::Storage(_))
                ));
                check!(work.first_denial().is_none());
            }
        }
        let count: i64 = catalog.lock()?.query_row(
            "SELECT count(*) FROM native_read_session_retire_v1 WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        check_eq!(count, 1);
        Ok(())
    }
}
