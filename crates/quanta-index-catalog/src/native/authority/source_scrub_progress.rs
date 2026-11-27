//! Restart-safe progress for an explicitly selected exact Source-body scrub.

use quanta_index_contract::index::{MapRoot, OwnerKey};
use quanta_index_core::index::{MemoryLifetime, WorkUsage, owner_from_map_key, owner_map_key};
use rusqlite::{OptionalExtension as _, params};

use super::*;

const DOMAIN: &[u8] = b"qidx.native.source-scrub-progress.v1";
const ROW_MEMORY_BYTES: u64 = 4096;

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS native_source_scrub_progress_v1 (
    workspace BLOB NOT NULL,
    epoch INTEGER NOT NULL CHECK(typeof(epoch)='integer' AND epoch>=0),
    sequence INTEGER NOT NULL CHECK(typeof(sequence)='integer' AND sequence>=0),
    view_digest BLOB NOT NULL CHECK(typeof(view_digest)='blob' AND length(view_digest)=32),
    owners_root BLOB CHECK(owners_root IS NULL OR (typeof(owners_root)='blob' AND length(owners_root)=32)),
    after_key BLOB CHECK(after_key IS NULL OR (typeof(after_key)='blob' AND length(after_key)=38)),
    complete INTEGER NOT NULL CHECK(typeof(complete)='integer' AND complete IN (0,1)),
    row_digest BLOB NOT NULL CHECK(typeof(row_digest)='blob' AND length(row_digest)=32),
    PRIMARY KEY(workspace,epoch,sequence),
    FOREIGN KEY(workspace,epoch,sequence) REFERENCES native_view_v1(workspace,epoch,sequence) ON DELETE CASCADE
) WITHOUT ROWID;
";

pub(super) const OBJECTS: &[(&str, &str, &str)] = &[(
    "table",
    "native_source_scrub_progress_v1",
    "native Source scrub progress",
)];

pub(super) fn is_schema_object(kind: &str, name: &str) -> bool {
    OBJECTS
        .iter()
        .any(|(expected_kind, expected_name, _)| *expected_kind == kind && *expected_name == name)
}

pub(super) fn verify_schema_with_refusal(
    connection: &Connection,
    path: &Path,
    refusal: impl Fn(&str) -> CoreError,
) -> Result<(), CoreError> {
    crate::connection::verify_installed_schema_objects_with_refusal(
        connection, path, SCHEMA, OBJECTS, refusal,
    )
}

fn ensure_schema(
    connection: &Connection,
    path: &Path,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        writes: 1,
        mutations: 1,
        bytes: u64::try_from(SCHEMA.len()).map_err(|_| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    connection
        .execute_batch(SCHEMA)
        .map_err(|error| sql(&error))?;
    require_schema(connection, path, budget)
}

fn require_schema(
    connection: &Connection,
    path: &Path,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let sql_bytes: Option<i64> = connection
        .query_row(
            "SELECT CASE WHEN typeof(sql)='text' THEN length(CAST(sql AS BLOB)) END
             FROM sqlite_master WHERE type='table' AND name='native_source_scrub_progress_v1'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?
        .ok_or(IndexError::SnapshotUnavailable)?;
    let max_sql_bytes = i64::try_from(SCHEMA.len()).map_err(|_| IndexError::ResourceLimited)?;
    if !matches!(sql_bytes, Some(bytes) if bytes >= 0 && bytes <= max_sql_bytes) {
        return Err(IndexError::Storage(
            "native Source scrub progress schema differs".into(),
        ));
    }
    verify_schema_with_refusal(connection, path, |label| {
        CoreError::Storage(format!("native Source scrub {label} schema differs"))
    })
    .map_err(|error| IndexError::Storage(error.to_string()))
}

#[derive(Clone, Copy)]
struct Progress {
    view: ViewRef,
    owners: MapRoot,
    after: Option<OwnerKey>,
    complete: bool,
    row_digest: [u8; 32],
}

fn digest_for(
    view: ViewRef,
    owners: MapRoot,
    after: Option<OwnerKey>,
    complete: bool,
) -> Result<Vec<u8>, IndexError> {
    fields_digest(
        DOMAIN,
        &(
            workspace_key(view.workspace)?,
            int(view.epoch)?,
            int(view.sequence)?,
            view.digest.0,
            owners.0.map(|object| object.0.0),
            after.map(owner_map_key).map(|key| key.to_vec()),
            i64::from(complete),
        ),
    )
}

fn authenticated_view(
    connection: &Connection,
    view: ViewRef,
    budget: &mut WorkBudget,
) -> Result<MapRoot, IndexError> {
    budget.charge(WorkUsage {
        reads: 3,
        ..WorkUsage::default()
    })?;
    let workspace = workspace_key(view.workspace)?;
    let floor = retention::exact_floor_for_epoch_by_key(connection, &workspace, view.epoch)?;
    if view.sequence < floor {
        return Err(IndexError::SnapshotUnavailable);
    }
    let selected = read_view_row_by_key(connection, view, &workspace)?;
    ensure_roots_with_budget(connection, &selected, Some(budget))?;
    Ok(selected.source.owners)
}

fn read_progress(
    connection: &Connection,
    view: ViewRef,
    owners: MapRoot,
    budget: &mut WorkBudget,
) -> Result<Option<Progress>, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: ROW_MEMORY_BYTES,
        ..WorkUsage::default()
    })?;
    let _memory = budget.reserve_memory(ROW_MEMORY_BYTES, MemoryLifetime::Temporary)?;
    let row: Option<(Option<Vec<u8>>, Option<Vec<u8>>, Option<Vec<u8>>, Option<i64>, Option<Vec<u8>>)> = connection
        .query_row(
            "SELECT
             CASE WHEN typeof(view_digest)='blob' AND length(view_digest)=32 THEN view_digest END,
             CASE WHEN owners_root IS NULL THEN NULL WHEN typeof(owners_root)='blob' AND length(owners_root)=32 THEN owners_root ELSE x'00' END,
             CASE WHEN after_key IS NULL THEN NULL WHEN typeof(after_key)='blob' AND length(after_key)=38 THEN after_key ELSE x'00' END,
             CASE WHEN typeof(complete)='integer' AND complete IN (0,1) THEN complete END,
             CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
             FROM native_source_scrub_progress_v1 WHERE workspace=?1 AND epoch=?2 AND sequence=?3",
            params![workspace_key(view.workspace)?, int(view.epoch)?, int(view.sequence)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let Some((identity, root, after, complete, digest)) = row else {
        return Ok(None);
    };
    let malformed = || IndexError::Storage("native Source scrub progress row mismatch".into());
    let identity: [u8; 32] = identity
        .ok_or_else(malformed)?
        .try_into()
        .map_err(|_| malformed())?;
    let root = match root {
        Some(bytes) => Some(ObjectRef(Digest(
            bytes.try_into().map_err(|_| malformed())?,
        ))),
        None => None,
    };
    let after = after
        .map(|key| owner_from_map_key(&key).map_err(|_| malformed()))
        .transpose()?;
    let complete = match complete {
        Some(0) => false,
        Some(1) => true,
        _ => return Err(malformed()),
    };
    let digest: [u8; 32] = digest
        .ok_or_else(malformed)?
        .try_into()
        .map_err(|_| malformed())?;
    if identity != view.digest.0
        || MapRoot(root) != owners
        || digest_for(view, owners, after, complete)?.as_slice() != digest.as_slice()
    {
        return Err(malformed());
    }
    Ok(Some(Progress {
        view,
        owners,
        after,
        complete,
        row_digest: digest,
    }))
}

impl SqliteCatalog {
    /// Start one explicit exact-view scrub. An existing or completed request
    /// cannot be reset by this method; resume uses the page method instead.
    pub fn start_source_body_scrub_progress(
        &self,
        view: ViewRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let mut connection = self.lock_native_budget(budget)?;
        let tx = begin(&mut connection)?;
        let owners = authenticated_view(&tx, view, budget)?;
        ensure_schema(&tx, self.path(), budget)?;
        if read_progress(&tx, view, owners, budget)?.is_some() {
            return Err(IndexError::Conflict);
        }
        budget.charge(WorkUsage {
            writes: 1,
            mutations: 1,
            bytes: ROW_MEMORY_BYTES,
            ..WorkUsage::default()
        })?;
        let _memory = budget.reserve_memory(ROW_MEMORY_BYTES, MemoryLifetime::Temporary)?;
        let digest = digest_for(view, owners, None, false)?;
        let changed = tx
            .execute(
                "INSERT INTO native_source_scrub_progress_v1
                 (workspace,epoch,sequence,view_digest,owners_root,after_key,complete,row_digest)
                 VALUES (?1,?2,?3,?4,?5,NULL,0,?6)",
                params![
                    workspace_key(view.workspace)?,
                    int(view.epoch)?,
                    int(view.sequence)?,
                    view.digest.0.as_slice(),
                    owners.0.map(|object| object.0.0.to_vec()),
                    digest,
                ],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(changed, "start Source scrub progress")?;
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))
    }

    /// Execute one page without holding the Catalog transaction across object
    /// reads. The callback is trusted maintenance code: it must return `next`
    /// only from the authenticated scan, and `None` only after observed EOF.
    /// A crash before the final CAS repeats that page; a committed CAS proves
    /// the callback completed before progress advanced.
    pub fn with_source_body_scrub_progress_page<T>(
        &self,
        view: ViewRef,
        budget: &mut WorkBudget,
        run_page: impl FnOnce(
            MapRoot,
            Option<OwnerKey>,
            &mut WorkBudget,
        ) -> Result<(T, Option<OwnerKey>), IndexError>,
    ) -> Result<Option<T>, IndexError> {
        budget.checkpoint()?;
        let progress = {
            let connection = self.lock_native_budget(budget)?;
            require_schema(&connection, self.path(), budget)?;
            let owners = authenticated_view(&connection, view, budget)?;
            read_progress(&connection, view, owners, budget)?
                .ok_or(IndexError::SnapshotUnavailable)?
        };
        if progress.complete {
            return Ok(None);
        }
        let (page, next) = run_page(progress.owners, progress.after, budget)?;
        self.advance_source_body_scrub_progress(progress, next, budget)?;
        Ok(Some(page))
    }

    fn advance_source_body_scrub_progress(
        &self,
        expected: Progress,
        next: Option<OwnerKey>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if next.is_some_and(|next| {
            expected
                .after
                .is_some_and(|prior| owner_map_key(next) <= owner_map_key(prior))
        }) {
            return Err(IndexError::BackendViolation(
                "Source scrub cursor did not advance".into(),
            ));
        }
        budget.checkpoint()?;
        let mut connection = self.lock_native_budget(budget)?;
        let tx = begin(&mut connection)?;
        let owners = authenticated_view(&tx, expected.view, budget)?;
        if owners != expected.owners {
            return Err(IndexError::SnapshotUnavailable);
        }
        let current = read_progress(&tx, expected.view, owners, budget)?
            .ok_or(IndexError::SnapshotUnavailable)?;
        if current.row_digest != expected.row_digest || current.complete {
            return Err(IndexError::Conflict);
        }
        let complete = next.is_none();
        let after = next.or(expected.after);
        budget.charge(WorkUsage {
            writes: 1,
            mutations: 1,
            bytes: ROW_MEMORY_BYTES,
            ..WorkUsage::default()
        })?;
        let _memory = budget.reserve_memory(ROW_MEMORY_BYTES, MemoryLifetime::Temporary)?;
        let digest = digest_for(expected.view, owners, after, complete)?;
        let changed = tx
            .execute(
                "UPDATE native_source_scrub_progress_v1 SET after_key=?4,complete=?5,row_digest=?6
                 WHERE workspace=?1 AND epoch=?2 AND sequence=?3 AND row_digest=?7",
                params![
                    workspace_key(expected.view.workspace)?,
                    int(expected.view.epoch)?,
                    int(expected.view.sequence)?,
                    after.map(owner_map_key).map(|key| key.to_vec()),
                    i64::from(complete),
                    digest,
                    expected.row_digest.as_slice(),
                ],
            )
            .map_err(|error| sql(&error))?;
        if changed != 1 {
            return Err(IndexError::Conflict);
        }
        budget.checkpoint()?;
        tx.commit().map_err(|error| sql(&error))
    }
}
