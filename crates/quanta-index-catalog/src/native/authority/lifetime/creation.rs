//! Durable admission for read-session lock files. The creation lock serializes
//! creators and orphan recovery across processes; neither age nor PID is proof.

use super::*;

const CREATION_LOCK: &str = ".creation.lease";
type OpenRow = (i64, i64, Vec<u8>);

pub(in crate::native::authority) fn open_intent_digest(
    binding: ReadSessionRef,
) -> Result<Vec<u8>, IndexError> {
    fields_digest(
        b"qidx.native.readsession.open.v1",
        &(int(binding.id)?, int(binding.fence)?),
    )
}

/// `None` means another live creator or sweeper owns the namespace.
pub(super) fn creation_lock(
    catalog: &SqliteCatalog,
    directory: &Path,
) -> Result<Option<LockFile>, IndexError> {
    let path = directory.join(CREATION_LOCK);
    let (file, created) = match open_lock(catalog, &path, true) {
        Ok(file) => (file, true),
        Err(error) => match fs::symlink_metadata(&path) {
            Ok(_) => (open_lock(catalog, &path, false)?, false),
            Err(missing) if missing.kind() == std::io::ErrorKind::NotFound => return Err(error),
            Err(other) => return Err(io_error("stat creation lock", &path, &other)),
        },
    };
    match file.file()?.try_lock() {
        Ok(()) => {
            if created {
                file.file()?
                    .sync_all()
                    .map_err(|error| io_error("sync creation lock", &path, &error))?;
                open_directory(directory)?
                    .sync_all()
                    .map_err(|error| io_error("sync creation directory", directory, &error))?;
            }
            Ok(Some(file))
        }
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(io_error("lock creation file", &path, &error)),
    }
}

fn open_snapshot(
    catalog: &SqliteCatalog,
    budget: &mut WorkBudget,
) -> Result<Vec<OpenRow>, IndexError> {
    let connection = catalog.lock_native_budget(budget)?;
    let mut statement = connection
        .prepare(
            "SELECT id,fence,
             CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
             FROM native_read_session_open_v1 ORDER BY id LIMIT 257",
        )
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    let mut result = Vec::new();
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 32,
            ..WorkUsage::default()
        })?;
        let digest: Option<Vec<u8>> = row.get(2).map_err(|error| sql(&error))?;
        let Some(digest) = digest else {
            return Err(IndexError::Storage(
                "native read session open intent row shape mismatch".into(),
            ));
        };
        if digest.len() != 32 {
            return Err(IndexError::Storage(
                "native read session open intent row shape changed".into(),
            ));
        }
        result.push((
            row.get(0).map_err(|error| sql(&error))?,
            row.get(1).map_err(|error| sql(&error))?,
            digest,
        ));
    }
    drop(rows);
    drop(statement);
    drop(connection);
    Ok(result)
}

/// Reap at most 256 pre-insert intents. Physical files transfer to the existing
/// retirement tombstone and are unlinked only after the SQL commit.
pub(super) fn reap_open_intents(
    catalog: &SqliteCatalog,
    directory: &Path,
    budget: &mut WorkBudget,
) -> Result<(u32, bool), IndexError> {
    budget.checkpoint()?;
    let Some(_namespace_lock) = creation_lock(catalog, directory)? else {
        return Ok((0, true));
    };
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let snapshot = open_snapshot(catalog, budget)?;
    let has_more = snapshot.len() > 256;
    let mut candidates = Vec::with_capacity(snapshot.len().min(256));
    for row in snapshot.into_iter().take(256) {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let binding = ReadSessionRef {
            id: uint(row.0)?,
            fence: uint(row.1)?,
        };
        if row.2 != open_intent_digest(binding)? {
            return Err(IndexError::Storage(
                "native read session open intent digest mismatch".into(),
            ));
        }
        let path = lock_path(directory, binding.id);
        let file = match fs::symlink_metadata(&path) {
            Ok(_) => Some(open_lock(catalog, &path, false)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(io_error("stat open intent lock", &path, &error)),
        };
        let identity = if let Some(ref file) = file {
            match file.file()?.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => {
                    return Err(IndexError::Storage(
                        "open intent lock held without creation lock".into(),
                    ));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(io_error("lock orphan session", &path, &error));
                }
            }
            let pathname = fs::symlink_metadata(&path)
                .map_err(|error| io_error("restat orphan session", &path, &error))?;
            let (device, number) = inode(file)?;
            if pathname.dev().to_le_bytes().as_slice() != device.as_slice()
                || pathname.ino().to_le_bytes().as_slice() != number.as_slice()
                || !pathname.is_file()
            {
                return Err(IndexError::Storage(
                    "orphan read session lock pathname changed inode".into(),
                ));
            }
            Some((device, number))
        } else {
            None
        };
        candidates.push((row, file, identity));
    }
    // Missing files are only confirmed after a durable directory observation.
    if candidates.iter().any(|(_, file, _)| file.is_none()) {
        open_directory(directory)?
            .sync_all()
            .map_err(|error| io_error("sync missing session directory", directory, &error))?;
    }
    let mut connection = catalog.lock_native_budget(budget)?;
    let tx = begin(&mut connection)?;
    let mut reaped = 0u32;
    for (row, file, identity) in &candidates {
        budget.charge(WorkUsage {
            reads: 2,
            writes: if file.is_some() { 2 } else { 1 },
            ..WorkUsage::default()
        })?;
        let current = tx
            .query_row(
                "SELECT id,fence,
                 CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN row_digest END
                 FROM native_read_session_open_v1 WHERE id=?1",
                params![row.0],
                |record| {
                    Ok((
                        record.get(0)?,
                        record.get(1)?,
                        record.get::<_, Option<Vec<u8>>>(2)?,
                    ))
                },
            )
            .map_err(|error| sql(&error))?;
        let Some(digest) = current.2 else {
            return Err(IndexError::Storage(
                "native read session open intent row shape mismatch".into(),
            ));
        };
        if digest.len() != 32 {
            return Err(IndexError::Storage(
                "native read session open intent row shape changed".into(),
            ));
        }
        let current: OpenRow = (current.0, current.1, digest);
        if current != *row {
            return Err(IndexError::Storage(
                "read session open intent changed during recovery".into(),
            ));
        }
        let session_exists: i64 = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM native_read_session_v1 WHERE id=?1)",
                params![row.0],
                |record| record.get(0),
            )
            .map_err(|error| sql(&error))?;
        if session_exists != 0 {
            return Err(IndexError::Storage(
                "read session open intent overlaps committed session".into(),
            ));
        }
        if let Some((device, number)) = identity {
            let retired: RetiredRow = (row.0, device.clone(), number.clone(), Vec::new());
            let inserted = tx
                .execute(
                    "INSERT INTO native_read_session_retire_v1(id,device,inode,row_digest) VALUES (?1,?2,?3,?4)",
                    params![retired.0, &retired.1, &retired.2, retired_digest(&retired)?],
                )
                .map_err(|error| sql(&error))?;
            exactly_one(inserted, "retire orphan read session lock")?;
        }
        let deleted = tx
            .execute(
                "DELETE FROM native_read_session_open_v1 WHERE id=?1 AND fence=?2",
                params![row.0, row.1],
            )
            .map_err(|error| sql(&error))?;
        exactly_one(deleted, "reap read session open intent")?;
        reaped = reaped.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    }
    budget.checkpoint()?;
    tx.commit().map_err(|error| sql(&error))?;
    drop(connection);
    drop(candidates);
    Ok((reaped, has_more))
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::{Duration, Instant};

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

    #[test]
    fn corrupt_open_intent_digest_is_rejected_before_snapshot_allocation()
    -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(1))?;
        let _changed = catalog.lock()?.execute(
            "INSERT INTO native_read_session_open_v1(id,fence,row_digest) VALUES (1,1,zeroblob(32))",
            [],
        )?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 10,
                writes: 0,
                bytes: 4 * 32,
                mutations: 0,
            },
            Instant::now() + Duration::from_secs(30),
        );
        for corrupt_value in [
            "zeroblob(31)",
            "zeroblob(33)",
            "zeroblob(1048577)",
            "'text'",
        ] {
            let _changed = catalog.lock()?.execute(
                &format!(
                    "UPDATE native_read_session_open_v1 SET row_digest={corrupt_value} WHERE id=1"
                ),
                [],
            )?;
            check!(matches!(
                open_snapshot(&catalog, &mut budget),
                Err(IndexError::Storage(_))
            ));
        }
        check_eq!(budget.usage().bytes, 4 * 32);
        let count: i64 = catalog.lock()?.query_row(
            "SELECT count(*) FROM native_read_session_open_v1 WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        check_eq!(count, 1);
        Ok(())
    }
}
