//! Disposable, bounded expected path changes. Scratch has no publication authority.

use quanta_index_core::index::{
    IndexError, MapValue, OwnerKey, ScratchAllocation, ScratchRequest, ScratchSpacePort,
    WorkBudget, WorkUsage,
};
use quanta_index_scratch_sqlite::ScratchConnection;
use rusqlite::{params, types::ValueRef};

const SCRATCH_PAGE_SIZE: u32 = 4096;
const SCRATCH_MAX_BYTES: u64 = 4_294_967_296;
const SCRATCH_INITIAL_BYTES: u64 = 256 * 1024;
const SCRATCH_CACHE_KIB: i32 = -2048;

pub(super) struct InventoryScratch {
    db: ScratchConnection,
    allocation: Box<dyn ScratchAllocation>,
}

impl InventoryScratch {
    pub(super) fn open(
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            bytes: 8192,
            ..WorkUsage::default()
        })?;
        let allocation = scratch.allocate(
            ScratchRequest {
                initial_bytes: SCRATCH_INITIAL_BYTES,
                max_bytes: SCRATCH_MAX_BYTES,
            },
            budget,
        )?;
        if allocation.max_bytes() != SCRATCH_MAX_BYTES {
            return Err(IndexError::BackendViolation(
                "inventory scratch allocation changed its envelope".into(),
            ));
        }
        let owned_db = ScratchConnection::open(
            &allocation.path().join("inventory.sqlite"),
            allocation.as_ref(),
            budget,
        )?;
        let db = owned_db.scoped(budget)?;
        db.execute_batch(&format!(
            "PRAGMA page_size=4096;
             PRAGMA cache_size={SCRATCH_CACHE_KIB};
             PRAGMA mmap_size=0;
             PRAGMA journal_mode=OFF;
             PRAGMA synchronous=OFF;
             PRAGMA temp_store=FILE;"
        ))
        .map_err(|error| sql_error(&error))?;
        quanta_index_scratch_sqlite::configure(&db, allocation.as_ref())?;
        let page_size: u32 = db
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        let max_pages: u32 = db
            .query_row("PRAGMA max_page_count", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        let cache_kib: i32 = db
            .query_row("PRAGMA cache_size", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        let mmap_size: i64 = db
            .query_row("PRAGMA mmap_size", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        let temp_store: u32 = db
            .query_row("PRAGMA temp_store", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        let journal_mode: String = db
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        let synchronous: u32 = db
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        let admitted_main_bytes = u64::from(max_pages)
            .checked_mul(u64::from(page_size))
            .ok_or(IndexError::ResourceLimited)?;
        if page_size != SCRATCH_PAGE_SIZE
            || admitted_main_bytes > allocation.reserved_bytes()?
            || cache_kib != SCRATCH_CACHE_KIB
            || mmap_size != 0
            || temp_store != 1
            || journal_mode != "off"
            || synchronous != 0
        {
            return Err(IndexError::Storage(
                "inventory scratch resource limits were not installed".into(),
            ));
        }
        quanta_index_scratch_sqlite::reserve_for_write(&db, allocation.as_ref(), 8192, budget)?;
        db.execute_batch(
            "CREATE TABLE path(
                key BLOB NOT NULL,
                repo BLOB NOT NULL,
                prior BLOB,
                next BLOB,
                has_prior INTEGER NOT NULL DEFAULT 0,
                has_next INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(key,repo)
            ) WITHOUT ROWID;",
        )
        .map_err(|error| sql_error(&error))?;
        drop(db);
        Ok(Self {
            db: owned_db,
            allocation,
        })
    }

    #[cfg(test)]
    pub(super) fn record_prior_path(
        &self,
        path: &[u8],
        owner: OwnerKey,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.record_repository_path(path, b"repo\0", owner, false, budget)
    }

    #[cfg(test)]
    pub(super) fn record_next_path(
        &self,
        path: &[u8],
        owner: OwnerKey,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.record_repository_path(path, b"repo\0", owner, true, budget)
    }

    pub(super) fn record_prior_repository_path(
        &self,
        path: &[u8],
        repo: &[u8],
        owner: OwnerKey,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.record_repository_path(path, repo, owner, false, budget)
    }

    pub(super) fn record_next_repository_path(
        &self,
        path: &[u8],
        repo: &[u8],
        owner: OwnerKey,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.record_repository_path(path, repo, owner, true, budget)
    }

    fn record_repository_path(
        &self,
        path: &[u8],
        repo: &[u8],
        owner: OwnerKey,
        next: bool,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let path_bytes = u64::try_from(path.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let repo_bytes = u64::try_from(repo.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let logical_bytes = path_bytes
            .checked_add(repo_bytes.saturating_sub(5))
            .ok_or(IndexError::ResourceLimited)?
            .checked_add(128)
            .ok_or(IndexError::ResourceLimited)?;
        let incoming = path_bytes
            .checked_add(repo_bytes)
            .ok_or(IndexError::ResourceLimited)?
            .checked_add(128)
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            writes: 2,
            bytes: logical_bytes,
            ..WorkUsage::default()
        })?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.db.scoped(budget)?,
            self.allocation.as_ref(),
            incoming,
            budget,
        )?;
        let db = self.db.scoped(budget)?;
        let _inserted = db
            .execute(
                "INSERT OR IGNORE INTO path(key,repo) VALUES (?1,?2)",
                params![path, repo],
            )
            .map_err(|error| sql_error(&error))?;
        let owner = owner_bytes(owner);
        let changed = if next {
            db.execute(
                "UPDATE path SET next=?3,has_next=1 WHERE key=?1 AND repo=?2 AND has_next=0",
                params![path, repo, owner.as_slice()],
            )
        } else {
            db.execute(
                "UPDATE path SET prior=?3,has_prior=1 WHERE key=?1 AND repo=?2 AND has_prior=0",
                params![path, repo, owner.as_slice()],
            )
        }
        .map_err(|error| sql_error(&error))?;
        if changed != 1 {
            return Err(IndexError::CorruptObject(
                "duplicate source path owner transition".into(),
            ));
        }
        Ok(())
    }

    /// Lookup is only an expectation check against a separately authenticated map diff.
    #[cfg(test)]
    pub(super) fn expected_at(
        &self,
        path: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<(Option<MapValue>, Option<MapValue>)>, IndexError> {
        self.expected_repository_at(path, b"repo\0", budget)
    }

    pub(super) fn expected_repository_at(
        &self,
        path: &[u8],
        repo: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<(Option<MapValue>, Option<MapValue>)>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: u64::try_from(path.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(
                    u64::try_from(repo.len().saturating_sub(b"repo\0".len()))
                        .map_err(|_error| IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let db = self.db.scoped(budget)?;
        let mut statement = db
            .prepare("SELECT prior,next,has_prior,has_next FROM path WHERE key=?1 AND repo=?2")
            .map_err(|error| sql_error(&error))?;
        let mut rows = statement
            .query(params![path, repo])
            .map_err(|error| sql_error(&error))?;
        let Some(row) = rows.next().map_err(|error| sql_error(&error))? else {
            return Ok(None);
        };
        let prior = owner_value(
            row.get_ref(0).map_err(|error| sql_error(&error))?,
            row.get(2).map_err(|error| sql_error(&error))?,
        )?;
        let next = owner_value(
            row.get_ref(1).map_err(|error| sql_error(&error))?,
            row.get(3).map_err(|error| sql_error(&error))?,
        )?;
        if rows.next().map_err(|error| sql_error(&error))?.is_some() {
            return Err(IndexError::CorruptObject(
                "duplicate expected path transition".into(),
            ));
        }
        Ok(Some((prior, next)))
    }

    pub(super) fn count(&self, budget: &mut WorkBudget) -> Result<u64, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let count: i64 = self
            .db
            .scoped(budget)?
            .query_row("SELECT COUNT(*) FROM path", [], |row| row.get(0))
            .map_err(|error| sql_error(&error))?;
        u64::try_from(count)
            .map_err(|_error| IndexError::CorruptObject("negative path count".into()))
    }
}

fn owner_bytes(owner: OwnerKey) -> [u8; 32] {
    let mut bytes = [0; 32];
    bytes[..16].copy_from_slice(&owner.producer.0);
    bytes[16..].copy_from_slice(&owner.stable_id.0);
    bytes
}

fn owner_value(value: ValueRef<'_>, present: i64) -> Result<Option<MapValue>, IndexError> {
    match (present, value) {
        (0, ValueRef::Null) => Ok(None),
        (1, ValueRef::Blob(bytes)) if bytes.len() == 32 => Ok(Some(MapValue::inline(bytes, None)?)),
        _ => Err(IndexError::CorruptObject(
            "invalid expected path owner".into(),
        )),
    }
}

fn sql_error(error: &rusqlite::Error) -> IndexError {
    quanta_index_scratch_sqlite::storage_error(error, "inventory scratch")
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{ProducerId, StableOwnerId, WorkLimits};
    use std::{
        path::Path,
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    struct TestScratch;
    struct TestAllocation(tempfile::TempDir, u64, AtomicU64);

    impl ScratchAllocation for TestAllocation {
        fn path(&self) -> &Path {
            self.0.path()
        }
        fn max_bytes(&self) -> u64 {
            SCRATCH_MAX_BYTES
        }
        fn reserved_bytes(&self) -> Result<u64, IndexError> {
            Ok(self.2.load(Ordering::Acquire))
        }
        fn reserve_at_least(&self, bytes: u64, budget: &mut WorkBudget) -> Result<(), IndexError> {
            budget.checkpoint()?;
            if bytes > self.1 {
                return Err(IndexError::ResourceLimited);
            }
            let _previous = self.2.fetch_max(bytes, Ordering::AcqRel);
            Ok(())
        }
    }

    impl ScratchSpacePort for TestScratch {
        fn allocate(
            &self,
            request: ScratchRequest,
            budget: &mut WorkBudget,
        ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
            budget.checkpoint()?;
            Ok(Box::new(TestAllocation(
                tempfile::tempdir().map_err(|error| IndexError::Storage(error.to_string()))?,
                request.max_bytes,
                AtomicU64::new(request.initial_bytes),
            )))
        }
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 100_000,
                writes: 100_000,
                bytes: 100_000_000,
                mutations: 100_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    fn owner(id: u8) -> OwnerKey {
        OwnerKey {
            producer: ProducerId([7; 16]),
            stable_id: StableOwnerId([id; 16]),
        }
    }

    #[test]
    fn path_expectations_support_swaps_and_reject_duplicate_next_owner()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut work = budget()?;
        let scratch = InventoryScratch::open(&TestScratch, &mut work)?;
        scratch.record_prior_path(b"path\0a", owner(1), &mut work)?;
        scratch.record_prior_path(b"path\0b", owner(2), &mut work)?;
        scratch.record_next_path(b"path\0a", owner(2), &mut work)?;
        scratch.record_next_path(b"path\0b", owner(1), &mut work)?;
        assert_eq!(scratch.count(&mut work)?, 2);
        assert_eq!(
            scratch.expected_at(b"path\0a", &mut work)?,
            Some((
                Some(quanta_index_core::index::path_owner_map_value(owner(1))?),
                Some(quanta_index_core::index::path_owner_map_value(owner(2))?)
            ))
        );
        assert!(matches!(
            scratch.record_next_path(b"path\0a", owner(1), &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        let path = scratch.allocation.path().to_path_buf();
        drop(scratch);
        assert!(!path.exists());
        Ok(())
    }

    #[test]
    fn repository_expectations_keep_same_path_owners_distinct()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut work = budget()?;
        let scratch = InventoryScratch::open(&TestScratch, &mut work)?;
        scratch.record_next_repository_path(b"path\0a", b"repo\0first", owner(1), &mut work)?;
        scratch.record_next_repository_path(b"path\0a", b"repo\0second", owner(2), &mut work)?;
        assert_eq!(scratch.count(&mut work)?, 2);
        assert_eq!(
            scratch.expected_repository_at(b"path\0a", b"repo\0first", &mut work)?,
            Some((
                None,
                Some(quanta_index_core::index::path_owner_map_value(owner(1))?)
            ))
        );
        assert_eq!(
            scratch.expected_repository_at(b"path\0a", b"repo\0second", &mut work)?,
            Some((
                None,
                Some(quanta_index_core::index::path_owner_map_value(owner(2))?)
            ))
        );
        assert!(matches!(
            scratch.record_next_repository_path(b"path\0a", b"repo\0first", owner(2), &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    #[test]
    fn scratch_page_limit_remains_bounded() -> Result<(), Box<dyn std::error::Error>> {
        let mut work = budget()?;
        let scratch = InventoryScratch::open(&TestScratch, &mut work)?;
        let db = scratch.db.scoped(&work)?;
        let page_size: u32 = db.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        let max_pages: u32 = db.query_row("PRAGMA max_page_count", [], |row| row.get(0))?;
        assert_eq!(page_size, SCRATCH_PAGE_SIZE);
        assert!(
            u64::from(max_pages) * u64::from(page_size) <= scratch.allocation.reserved_bytes()?
        );
        Ok(())
    }
}
