//! Process-death-fenced read custody. A deadline may reject a read, but never
//! authorizes reclaim while the independently opened OS session lock is held.

use std::any::Any;
use std::fs::{self, File, TryLockError};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use quanta_index_core::index::ports::CatalogReadPort;
use quanta_index_core::index::{
    CollectionId, IndexError, ReadSessionLease, ReadSessionRecoveryPage, ReadSessionRef,
    ReadTicketLease, SlotId, TenantId, TicketRecord, ViewSelection, WorkBudget, WorkUsage,
    WorktreeId, validate_read_ticket_lease,
};
use rusqlite::{Connection, OptionalExtension as _, params};
use rustix::fs::{Mode, OFlags, open};

use super::*;

pub(super) mod creation;
use creation::{creation_lock, reap_open_intents};

const MAX_RECOVERY_SESSIONS: u32 = 256;
const MAX_RECOVERY_PINS: u32 = 256;
const SESSION_DIRECTORY: &str = "native-read-sessions";

// Each ID is an array of 16 u8 values; all-255 has the widest canonical JSON encoding.
pub(super) fn max_workspace_key_bytes() -> Result<i64, IndexError> {
    let widest = WorkspaceKey {
        tenant: TenantId([u8::MAX; 16]),
        collection: CollectionId([u8::MAX; 16]),
        worktree: WorktreeId([u8::MAX; 16]),
        slot: SlotId([u8::MAX; 16]),
    };
    i64::try_from(workspace_key(widest)?.len()).map_err(|_error| IndexError::ResourceLimited)
}

pub(super) type SessionRow = (i64, i64, Vec<u8>, Vec<u8>, Vec<u8>);
pub(super) type PinRow = (i64, i64, i64, i64, i64, Vec<u8>, i64, i64, Option<Vec<u8>>);
type RetiredRow = (i64, Vec<u8>, Vec<u8>, Vec<u8>);
type OptionalSessionIdentity = (Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>);
type OptionalRetiredRow = (i64, Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>);

pub(super) struct CatalogSessionLease {
    pub(super) binding: ReadSessionRef,
    catalog_path: PathBuf,
    pub(super) file: LockFile,
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

fn retired_snapshot(
    catalog: &SqliteCatalog,
    budget: &mut WorkBudget,
) -> Result<Vec<RetiredRow>, IndexError> {
    let connection = catalog.lock_native_budget(budget)?;
    let mut statement = connection
        .prepare(
            "SELECT id,
         CASE WHEN typeof(device)='blob' AND length(device)=8 THEN device END,
         CASE WHEN typeof(inode)='blob' AND length(inode)=8 THEN inode END,
         CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
         FROM native_read_session_retire_v1 ORDER BY id LIMIT 257",
        )
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 48,
            ..WorkUsage::default()
        })?;
        let (device, inode, digest): OptionalSessionIdentity = (
            row.get(1).map_err(|error| sql(&error))?,
            row.get(2).map_err(|error| sql(&error))?,
            row.get(3).map_err(|error| sql(&error))?,
        );
        let (Some(device), Some(inode), Some(digest)) = (device, inode, digest) else {
            return Err(IndexError::Storage(
                "retired read session row shape mismatch".into(),
            ));
        };
        if device.len() != 8 || inode.len() != 8 || digest.len() != 32 {
            return Err(IndexError::Storage(
                "retired read session row shape changed".into(),
            ));
        }
        result.push((
            row.get(0).map_err(|error| sql(&error))?,
            device,
            inode,
            digest,
        ));
    }
    drop(rows);
    drop(statement);
    drop(connection);
    Ok(result)
}

fn cleanup_retired(
    catalog: &SqliteCatalog,
    directory: &Path,
    budget: &mut WorkBudget,
) -> Result<bool, IndexError> {
    cleanup_retired_rows(
        catalog,
        directory,
        retired_snapshot(catalog, budget)?,
        budget,
    )
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
    let mut has_more = retired.len() > 256;
    for row in retired.into_iter().take(256) {
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
    let row = connection
        .query_row(
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

pub(super) fn guard<'a>(
    catalog: &SqliteCatalog,
    lease: &'a dyn ReadSessionLease,
) -> Result<&'a CatalogSessionLease, IndexError> {
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
    if inode(&fresh)? != inode(&concrete.file)? {
        return Err(IndexError::Storage(
            "read session lock inode changed".into(),
        ));
    }
    // A second description cannot acquire an exclusive lock while this guard
    // owns it. File descriptors copied from another process cannot synthesize
    // this private concrete type.
    match fresh.file()?.try_lock() {
        Err(TryLockError::WouldBlock) => Ok(concrete),
        Ok(()) => Err(IndexError::Storage("read session OS lock was lost".into())),
        Err(TryLockError::Error(error)) => Err(io_error("probe guard lock", &path, &error)),
    }
}

fn pin_digest(row: &PinRow) -> Result<Vec<u8>, IndexError> {
    fields_digest(
        b"qidx.native.pin.v2",
        &(
            &row.0, &row.1, &row.2, &row.3, &row.4, &row.5, &row.6, &row.7,
        ),
    )
}

pub(super) fn checked_pin(
    connection: &Connection,
    ticket: ReadTicketLease,
) -> Result<PinRow, IndexError> {
    let max_workspace_bytes = max_workspace_key_bytes()?;
    let row: PinRow = connection
        .query_row(
            "SELECT ticket,session_id,session_fence,fence,expires_at_ms,
         CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND ?2 THEN workspace END,
         epoch,sequence,
         CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
         FROM native_pin_v1 WHERE ticket=?1",
            params![ticket_to_integer(ticket.id)?, max_workspace_bytes],
            |row| {
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
            },
        )
        .optional()
        .map_err(|error| sql(&error))?
        .ok_or(IndexError::SnapshotUnavailable)?;
    if row.5.is_empty()
        || row.5.len()
            > usize::try_from(max_workspace_bytes).map_err(|_error| IndexError::ResourceLimited)?
        || row.8.as_ref().is_some_and(|digest| digest.len() != 32)
    {
        return Err(IndexError::Storage("native pin row shape changed".into()));
    }
    if row.1 != int(ticket.session.id)?
        || row.2 != int(ticket.session.fence)?
        || row.3 != int(ticket.fence)?
        || row.4 != int(ticket.expires_at_ms)?
    {
        return Err(IndexError::SnapshotUnavailable);
    }
    if row.8.as_ref() != Some(&pin_digest(&row)?) {
        return Err(IndexError::Storage("native pin digest mismatch".into()));
    }
    Ok(row)
}

// Both public reads and execution preparation use this original pin issuer.
// The caller already authenticated the same live session and sampled its clock
// inside the held transaction. No intermediate pin is committed by this helper.
fn issue_ticket(
    tx: &rusqlite::Transaction<'_>,
    binding: ReadSessionRef,
    selection: ViewSelection,
    milliseconds: u64,
    now: u64,
) -> Result<TicketRecord, IndexError> {
    let expiry = now
        .checked_add(milliseconds)
        .ok_or(IndexError::ResourceLimited)?;
    let view = match selection {
        ViewSelection::Latest { workspace, epoch } => {
            let current = current_view(tx, workspace)?.ok_or(IndexError::SnapshotUnavailable)?;
            if current.identity.epoch != epoch {
                return Err(IndexError::SnapshotUnavailable);
            }
            current
        }
        ViewSelection::Exact(identity) => {
            let floor = retention::exact_floor_for_epoch(tx, identity.workspace, identity.epoch)?;
            if identity.sequence < floor {
                return Err(IndexError::SnapshotUnavailable);
            }
            read_view_row(tx, identity)?
        }
    };
    ensure_roots(tx, &view)?;
    let fence = next_fence(tx)?;
    let inserted = tx.execute(
            "INSERT INTO native_pin_v1(session_id,session_fence,fence,expires_at_ms,workspace,epoch,sequence) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![int(binding.id)?,int(binding.fence)?,int(fence)?,int(expiry)?,workspace_key(view.identity.workspace)?,int(view.identity.epoch)?,int(view.identity.sequence)?],
        ).map_err(|error| sql(&error))?;
    exactly_one(inserted, "insert read pin")?;
    let id = ticket_from_integer(tx.last_insert_rowid())?;
    let pin = (
        ticket_to_integer(id)?,
        int(binding.id)?,
        int(binding.fence)?,
        int(fence)?,
        int(expiry)?,
        workspace_key(view.identity.workspace)?,
        int(view.identity.epoch)?,
        int(view.identity.sequence)?,
        None,
    );
    let changed = tx
        .execute(
            "UPDATE native_pin_v1 SET row_digest=?2 WHERE ticket=?1",
            params![pin.0, pin_digest(&pin)?],
        )
        .map_err(|error| sql(&error))?;
    exactly_one(changed, "seal read pin")?;
    Ok(TicketRecord {
        lease: ReadTicketLease {
            id,
            session: binding,
            fence,
            expires_at_ms: expiry,
        },
        view,
    })
}

impl SqliteCatalog {
    pub(super) fn acquire_materialization_ticket_impl(
        &self,
        session: &dyn ReadSessionLease,
        request: &UpdateRequest,
        lease: Duration,
    ) -> Result<TicketRecord, IndexError> {
        let UpdateKind::Materialize { claim } = &request.kind else {
            return Err(IndexError::InvalidInput(
                "materialization preparation requires an owner claim".into(),
            ));
        };
        let concrete = guard(self, session)?;
        let guard_inode = inode(&concrete.file)?;
        let milliseconds = validate_read_ticket_lease(lease)?;
        let mut connection = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut connection)?;
        let session_row = checked_session(&tx, concrete.binding)?;
        if (session_row.2, session_row.3) != guard_inode {
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
            validate_live_claim_row(&work, claim, now)?;
            validate_owned_live_claim(&assignment, claim, now)?;
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
        let ticket = issue_ticket(
            &tx,
            concrete.binding,
            ViewSelection::Latest {
                workspace: request.expected_view.workspace,
                epoch: request.expected_view.epoch,
            },
            milliseconds,
            now,
        )?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(connection);
        Ok(ticket)
    }
}

impl CatalogReadPort for SqliteCatalog {
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
        let guard_inode = inode(&concrete.file)?;
        let milliseconds = validate_read_ticket_lease(lease)?;
        let mut connection = self.lock_native_budget(budget)?;
        let tx = begin(&mut connection)?;
        let session_row = checked_session(&tx, concrete.binding)?;
        if (session_row.2, session_row.3) != guard_inode {
            return Err(IndexError::Storage("read session inode mismatch".into()));
        }
        let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
        let ticket = issue_ticket(&tx, concrete.binding, selection, milliseconds, now)?;
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
        let concrete = guard(self, session)?;
        let guard_inode = inode(&concrete.file)?;
        if ticket.session != concrete.binding {
            return Err(IndexError::SnapshotUnavailable);
        }
        let milliseconds = validate_read_ticket_lease(lease)?;
        let mut connection = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut connection)?;
        let session_row = checked_session(&tx, concrete.binding)?;
        if (session_row.2, session_row.3) != guard_inode {
            return Err(IndexError::Storage("read session inode mismatch".into()));
        }
        let pin = checked_pin(&tx, ticket)?;
        let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
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
        tx.commit().map_err(|error| sql(&error))?;
        drop(connection);
        Ok(next)
    }

    fn validate_ticket(
        &self,
        session: &dyn ReadSessionLease,
        ticket: ReadTicketLease,
    ) -> Result<(), IndexError> {
        let concrete = guard(self, session)?;
        let guard_inode = inode(&concrete.file)?;
        if ticket.session != concrete.binding {
            return Err(IndexError::SnapshotUnavailable);
        }
        let mut connection = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut connection)?;
        let session_row = checked_session(&tx, concrete.binding)?;
        if (session_row.2, session_row.3) != guard_inode {
            return Err(IndexError::Storage("read session inode mismatch".into()));
        }
        let pin = checked_pin(&tx, ticket)?;
        let now = catalog_now_ms(&tx, self.clock.now_unix_ms())?;
        if now >= uint(pin.4)? {
            return Err(IndexError::SnapshotUnavailable);
        }
        tx.commit().map_err(|error| sql(&error))?;
        drop(connection);
        Ok(())
    }

    fn release_ticket(
        &self,
        session: &dyn ReadSessionLease,
        ticket: ReadTicketLease,
    ) -> Result<(), IndexError> {
        let concrete = guard(self, session)?;
        let guard_inode = inode(&concrete.file)?;
        if ticket.session != concrete.binding {
            return Err(IndexError::SnapshotUnavailable);
        }
        let mut connection = self
            .lock()
            .map_err(|error| IndexError::Storage(error.to_string()))?;
        let tx = begin(&mut connection)?;
        let session_row = checked_session(&tx, concrete.binding)?;
        if (session_row.2, session_row.3) != guard_inode {
            return Err(IndexError::Storage("read session inode mismatch".into()));
        }
        let pin = checked_pin(&tx, ticket)?;
        let changed = tx
            .execute(
                "DELETE FROM native_pin_v1 WHERE ticket=?1 AND fence=?2",
                params![pin.0, pin.3],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(changed, "release read pin")?;
        tx.commit().map_err(|error| sql(&error))?;
        drop(connection);
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
        let directory = session_directory(self)?;
        let (open_intents_reaped, more_intents) = reap_open_intents(self, &directory, budget)?;
        let mut cursor = after;
        let mut cursor_id = after.map(|value| int(value.id)).transpose()?.unwrap_or(0);
        let max_workspace_bytes = max_workspace_key_bytes()?;
        budget.checkpoint()?;
        let candidates: Vec<SessionRow> = {
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
            let mut result = Vec::new();
            while let Some(row) = rows.next().map_err(|error| sql(&error))? {
                budget.charge(WorkUsage {
                    reads: 1,
                    bytes: 48,
                    ..WorkUsage::default()
                })?;
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
                result.push((
                    row.get(0).map_err(|error| sql(&error))?,
                    row.get(1).map_err(|error| sql(&error))?,
                    device,
                    inode,
                    digest,
                ));
            }
            drop(rows);
            drop(statement);
            drop(connection);
            result
        };
        // No filesystem operation runs under the Catalog mutex or SQL transaction.
        // A successfully probed dead lock remains owned until after retirement commit.
        let mut probes = Vec::with_capacity(candidates.len());
        for row in candidates {
            if row.4 != session_digest(&row)? {
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
            let dead_lock = match file.file()?.try_lock() {
                Ok(()) => Some(file),
                Err(TryLockError::WouldBlock) => None,
                Err(TryLockError::Error(error)) => {
                    return Err(io_error("probe recovery lock", &path, &error));
                }
            };
            probes.push((row, dead_lock));
        }
        let mut connection = self.lock_native_budget(budget)?;
        let tx = begin(&mut connection)?;
        let mut recovered = 0u32;
        let mut pins_released = 0u32;
        let mut partial = false;
        for (row, dead_lock) in &probes {
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
                let pin: Option<(PinRow, usize)> = {
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
                        let length = usize::try_from(length)
                            .map_err(|_error| IndexError::ResourceLimited)?;
                        budget.charge(WorkUsage {
                            bytes: u64::try_from(length)
                                .map_err(|_error| IndexError::ResourceLimited)?
                                .checked_add(32)
                                .ok_or(IndexError::ResourceLimited)?,
                            ..WorkUsage::default()
                        })?;
                        Some((
                            (
                                record.get(0).map_err(|error| sql(&error))?,
                                record.get(1).map_err(|error| sql(&error))?,
                                record.get(2).map_err(|error| sql(&error))?,
                                record.get(3).map_err(|error| sql(&error))?,
                                record.get(4).map_err(|error| sql(&error))?,
                                record.get(5).map_err(|error| sql(&error))?,
                                record.get(6).map_err(|error| sql(&error))?,
                                record.get(7).map_err(|error| sql(&error))?,
                                record.get(8).map_err(|error| sql(&error))?,
                            ),
                            length,
                        ))
                    } else {
                        None
                    }
                };
                let Some((pin, workspace_bytes)) = pin else {
                    break;
                };
                if pin.5.len() != workspace_bytes
                    || pin.8.as_ref().is_some_and(|digest| digest.len() != 32)
                {
                    return Err(IndexError::Storage(
                        "native recovery pin row shape changed".into(),
                    ));
                }
                if pin.1 != row.0 || pin.2 != row.1 || pin.8.as_ref() != Some(&pin_digest(&pin)?) {
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
        drop(probes);
        let more_locks = cleanup_retired(self, &directory, budget)?;
        Ok(ReadSessionRecoveryPage {
            after: cursor,
            has_more: more_intents || more_sessions || more_locks,
            recovered,
            pins_released,
            open_intents_reaped,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::Instant;

    use quanta_index_core::index::WorkLimits;

    use super::*;

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
        check_eq!(checked_pin(&connection, ticket)?.5, pin.5);
        for corrupted in [
            "UPDATE native_pin_v1 SET workspace=zeroblob(0)",
            "UPDATE native_pin_v1 SET workspace=zeroblob(1048577)",
            "UPDATE native_pin_v1 SET workspace='text'",
        ] {
            let _changed = connection.execute(corrupted, [])?;
            check!(matches!(
                checked_pin(&connection, ticket),
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
        let stale = retired_snapshot(&catalog, &mut budget()?)?;
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
        if !retired_snapshot(&catalog, &mut budget()?)?.is_empty() {
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
                check!(matches!(
                    retired_snapshot(&catalog, &mut budget()?),
                    Err(IndexError::Storage(_))
                ));
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
