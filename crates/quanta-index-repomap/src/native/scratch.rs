//! Private, disposable inventory spool. No row is a published source of truth.

use quanta_index_core::index::{
    IndexError, MapEdit, MapRoot, ObjectRef, OrdinalMapEdit, OwnerKey, ProducerId,
    ScratchAllocation, ScratchRequest, ScratchSpacePort, StableOwnerId, WorkBudget, WorkUsage,
    owner_map_key,
    ports::{MapEditCursor, PersistentMapPort},
};
use quanta_index_scratch_sqlite::ScratchConnection;
use rusqlite::{params, types::ValueRef};

const SCRATCH_PAGE_SIZE: u32 = 4096;
const SCRATCH_MAX_BYTES: u64 = 4_294_967_296;
const SCRATCH_INITIAL_BYTES: u64 = 256 * 1024;
const SCRATCH_CACHE_KIB: i32 = -2048;
const MAX_ROW_BYTES: usize = 4096 + 38 + 3 * 32 + 64;

pub(super) struct InventoryScratch {
    db: ScratchConnection,
    allocation: Box<dyn ScratchAllocation>,
}

pub(super) struct OwnerRow {
    pub(super) position: OrdinalMapEdit,
    pub(super) next_source: Option<ObjectRef>,
}
impl std::ops::Deref for OwnerRow {
    type Target = MapEdit;
    fn deref(&self) -> &Self::Target {
        &self.position.edit
    }
}
pub(super) struct PathRow {
    pub(super) position: OrdinalMapEdit,
}
impl std::ops::Deref for PathRow {
    type Target = MapEdit;
    fn deref(&self) -> &Self::Target {
        &self.position.edit
    }
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
        )?;
        let db = owned_db.scoped()?;
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
            "CREATE TABLE owner(key BLOB PRIMARY KEY NOT NULL, prior BLOB,
               next_source BLOB, next_ref BLOB) WITHOUT ROWID;
             CREATE TABLE path(key BLOB PRIMARY KEY NOT NULL, prior BLOB,
               has_prior INTEGER NOT NULL DEFAULT 0, next_ref BLOB,
               has_next INTEGER NOT NULL DEFAULT 0) WITHOUT ROWID;",
        )
        .map_err(|error| sql_error(&error))?;
        drop(db);
        Ok(Self {
            db: owned_db,
            allocation,
        })
    }

    pub(super) fn record_owner(
        &self,
        owner: OwnerKey,
        prior: Option<ObjectRef>,
        next_source: Option<ObjectRef>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            bytes: 128,
            ..WorkUsage::default()
        })?;
        let key = owner_map_key(owner);
        let prior_bytes = prior.map(|reference| (reference.0).0);
        let next_bytes = next_source.map(|reference| (reference.0).0);
        self.reserve_write(128, budget)?;
        let inserted = self
            .db
            .scoped()?
            .execute(
                "INSERT INTO owner(key,prior,next_source) VALUES (?1,?2,?3)",
                params![
                    key.as_slice(),
                    prior_bytes.as_ref().map(<[u8; 32]>::as_slice),
                    next_bytes.as_ref().map(<[u8; 32]>::as_slice)
                ],
            )
            .map_err(|error| sql_error(&error))?;
        if inserted != 1 {
            return Err(IndexError::Storage(
                "inventory owner spool lost a row".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn record_prior_path(
        &self,
        path: &[u8],
        prior: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.ensure_path(path, budget)?;
        budget.charge(WorkUsage {
            writes: 1,
            ..WorkUsage::default()
        })?;
        let incoming = u64::try_from(path.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(96)
            .ok_or(IndexError::ResourceLimited)?;
        self.reserve_write(incoming, budget)?;
        let changed = self
            .db
            .scoped()?
            .execute(
                "UPDATE path SET prior=?2,has_prior=1 WHERE key=?1 AND has_prior=0",
                params![path, (prior.0).0.as_slice()],
            )
            .map_err(|error| sql_error(&error))?;
        if changed != 1 {
            return Err(IndexError::CorruptObject(
                "duplicate prior file path".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn record_next_path(
        &self,
        path: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.ensure_path(path, budget)?;
        budget.charge(WorkUsage {
            writes: 1,
            ..WorkUsage::default()
        })?;
        let incoming = u64::try_from(path.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(96)
            .ok_or(IndexError::ResourceLimited)?;
        self.reserve_write(incoming, budget)?;
        let changed = self
            .db
            .scoped()?
            .execute(
                "UPDATE path SET has_next=1 WHERE key=?1 AND has_next=0",
                params![path],
            )
            .map_err(|error| sql_error(&error))?;
        if changed != 1 {
            return Err(IndexError::InvalidInput(
                "duplicate target file path".into(),
            ));
        }
        Ok(())
    }

    fn ensure_path(&self, path: &[u8], budget: &mut WorkBudget) -> Result<(), IndexError> {
        let incoming = u64::try_from(path.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(128)
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            writes: 1,
            bytes: incoming,
            ..WorkUsage::default()
        })?;
        self.reserve_write(incoming, budget)?;
        let _inserted = self
            .db
            .scoped()?
            .execute("INSERT OR IGNORE INTO path(key) VALUES (?1)", params![path])
            .map_err(|error| sql_error(&error))?;
        Ok(())
    }

    pub(super) fn validate_paths(
        &self,
        root: MapRoot,
        maps: &dyn PersistentMapPort,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let mut after: Option<OrdinalMapEdit> = None;
        while let Some(path) = self.next_path(
            after.as_ref().map(|position| position.edit.key.as_slice()),
            budget,
        )? {
            budget.checkpoint()?;
            if maps.lookup(root, &path.key, budget)? != path.expected {
                return Err(IndexError::Conflict);
            }
            after = Some(path.position.clone());
        }
        Ok(())
    }

    pub(super) fn set_next(
        &self,
        owner: &[u8],
        path: &[u8],
        next: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 2,
            ..WorkUsage::default()
        })?;
        self.reserve_write(
            u64::try_from(owner.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(96)
                .ok_or(IndexError::ResourceLimited)?,
            budget,
        )?;
        let updated_owner = self.db.scoped()?.execute(
            "UPDATE owner SET next_ref=?2 WHERE key=?1 AND next_source IS NOT NULL AND next_ref IS NULL",
            params![owner, (next.0).0.as_slice()],
        ).map_err(|error| sql_error(&error))?;
        self.reserve_write(
            u64::try_from(path.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(96)
                .ok_or(IndexError::ResourceLimited)?,
            budget,
        )?;
        let updated_path = self
            .db
            .scoped()?
            .execute(
                "UPDATE path SET next_ref=?2 WHERE key=?1 AND has_next=1 AND next_ref IS NULL",
                params![path, (next.0).0.as_slice()],
            )
            .map_err(|error| sql_error(&error))?;
        if updated_owner != 1 || updated_path != 1 {
            return Err(IndexError::CorruptObject(
                "inventory contribution assignment is not unique".into(),
            ));
        }
        Ok(())
    }

    fn reserve_write(
        &self,
        incoming_bytes: u64,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.db.scoped()?,
            self.allocation.as_ref(),
            incoming_bytes,
            budget,
        )
    }

    pub(super) fn next_owner(
        &self,
        after: Option<&[u8]>,
        budget: &mut WorkBudget,
    ) -> Result<Option<OwnerRow>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let sql = if after.is_some() {
            "SELECT key,prior,next_source,next_ref FROM owner WHERE key>?1 ORDER BY key LIMIT 1"
        } else {
            "SELECT key,prior,next_source,next_ref FROM owner ORDER BY key LIMIT 1"
        };
        let io_scope = self.db.scoped()?;
        let mut statement = io_scope.prepare(sql).map_err(|error| sql_error(&error))?;
        let mut rows = if let Some(after) = after {
            statement
                .query(params![after])
                .map_err(|error| sql_error(&error))?
        } else {
            statement.query([]).map_err(|error| sql_error(&error))?
        };
        let Some(row) = rows.next().map_err(|error| sql_error(&error))? else {
            return Ok(None);
        };
        let key = borrow_blob(row.get_ref(0).map_err(|error| sql_error(&error))?, budget)?;
        let prior = optional_ref(row.get_ref(1).map_err(|error| sql_error(&error))?)?;
        let next_source = optional_ref(row.get_ref(2).map_err(|error| sql_error(&error))?)?;
        let next = optional_ref(row.get_ref(3).map_err(|error| sql_error(&error))?)?;
        Ok(Some(OwnerRow {
            position: OrdinalMapEdit::copy_object_key(0, key, prior, next, budget)?,
            next_source,
        }))
    }

    pub(super) fn next_path(
        &self,
        after: Option<&[u8]>,
        budget: &mut WorkBudget,
    ) -> Result<Option<PathRow>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let sql = if after.is_some() {
            "SELECT key,prior,next_ref FROM path WHERE key>?1 ORDER BY key LIMIT 1"
        } else {
            "SELECT key,prior,next_ref FROM path ORDER BY key LIMIT 1"
        };
        let io_scope = self.db.scoped()?;
        let mut statement = io_scope.prepare(sql).map_err(|error| sql_error(&error))?;
        let mut rows = if let Some(after) = after {
            statement
                .query(params![after])
                .map_err(|error| sql_error(&error))?
        } else {
            statement.query([]).map_err(|error| sql_error(&error))?
        };
        let Some(row) = rows.next().map_err(|error| sql_error(&error))? else {
            return Ok(None);
        };
        let key = borrow_blob(row.get_ref(0).map_err(|error| sql_error(&error))?, budget)?;
        let prior = optional_ref(row.get_ref(1).map_err(|error| sql_error(&error))?)?;
        let next = optional_ref(row.get_ref(2).map_err(|error| sql_error(&error))?)?;
        Ok(Some(PathRow {
            position: OrdinalMapEdit::copy_object_key(1, key, prior, next, budget)?,
        }))
    }

    pub(super) fn owner_from_key(key: &[u8]) -> Result<OwnerKey, IndexError> {
        if key.len() != 38 || key.get(..6) != Some(b"owner\0".as_slice()) {
            return Err(IndexError::CorruptObject(
                "invalid inventory owner key".into(),
            ));
        }
        let producer = key
            .get(6..22)
            .ok_or_else(|| IndexError::CorruptObject("missing inventory producer".into()))?;
        let stable = key
            .get(22..38)
            .ok_or_else(|| IndexError::CorruptObject("missing inventory stable owner".into()))?;
        Ok(OwnerKey {
            producer: ProducerId(producer.try_into().map_err(|_bytes| {
                IndexError::CorruptObject("invalid inventory producer".into())
            })?),
            stable_id: StableOwnerId(stable.try_into().map_err(|_bytes| {
                IndexError::CorruptObject("invalid inventory stable owner".into())
            })?),
        })
    }
}

pub(super) struct InventoryEditCursor<'a> {
    scratch: &'a InventoryScratch,
    root: u32,
    after: Option<OrdinalMapEdit>,
}

impl<'a> InventoryEditCursor<'a> {
    pub(super) fn new(scratch: &'a InventoryScratch) -> Self {
        Self {
            scratch,
            root: 0,
            after: None,
        }
    }
}

impl MapEditCursor for InventoryEditCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        loop {
            budget.checkpoint()?;
            if self.root == 0 {
                if let Some(row) = self.scratch.next_owner(
                    self.after
                        .as_ref()
                        .map(|position| position.edit.key.as_slice()),
                    budget,
                )? {
                    budget.charge(WorkUsage {
                        bytes: u64::try_from(row.key.len())
                            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                        ..WorkUsage::default()
                    })?;
                    self.after = Some(row.position.clone());
                    if row.expected != row.next {
                        return Ok(Some(row.position));
                    }
                    continue;
                }
                self.root = 1;
                self.after = None;
            }
            if let Some(row) = self.scratch.next_path(
                self.after
                    .as_ref()
                    .map(|position| position.edit.key.as_slice()),
                budget,
            )? {
                budget.charge(WorkUsage {
                    bytes: u64::try_from(row.key.len())
                        .map_err(|_conversion_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                self.after = Some(row.position.clone());
                if row.expected != row.next {
                    return Ok(Some(row.position));
                }
                continue;
            }
            return Ok(None);
        }
    }
}

fn optional_ref(value: ValueRef<'_>) -> Result<Option<ObjectRef>, IndexError> {
    match value {
        ValueRef::Null => Ok(None),
        ValueRef::Blob(bytes) => {
            let digest = bytes.try_into().map_err(|_bytes| {
                IndexError::CorruptObject("invalid inventory reference".into())
            })?;
            Ok(Some(ObjectRef(quanta_index_core::index::Digest(digest))))
        }
        ValueRef::Integer(_) | ValueRef::Real(_) | ValueRef::Text(_) => Err(
            IndexError::CorruptObject("invalid inventory reference type".into()),
        ),
    }
}

fn borrow_blob<'a>(value: ValueRef<'a>, budget: &mut WorkBudget) -> Result<&'a [u8], IndexError> {
    let ValueRef::Blob(bytes) = value else {
        return Err(IndexError::CorruptObject(
            "invalid inventory key type".into(),
        ));
    };
    if bytes.len() > MAX_ROW_BYTES {
        return Err(IndexError::CorruptObject(
            "inventory key exceeds row bound".into(),
        ));
    }
    budget.charge(WorkUsage {
        reads: 1,
        bytes: u64::try_from(bytes.len())
            .map_err(|_conversion_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    Ok(bytes)
}

fn sql_error(error: &rusqlite::Error) -> IndexError {
    quanta_index_scratch_sqlite::storage_error(error, "inventory scratch")
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::Path,
        sync::atomic::{AtomicU64, Ordering},
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{Digest, WorkLimits};

    use super::*;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(format!("assertion failed: {}", stringify!($condition)).into());
            }
        }};
        ($condition:expr, $($message:tt)+) => {{
            if !$condition {
                return Err(format!($($message)+).into());
            }
        }};
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if !(*left == *right) => {
                    return Err(format!("assertion `left == right` failed\n left: {left:?}\n right: {right:?}").into());
                }
                _ => {}
            }
        }};
        ($left:expr, $right:expr, $($message:tt)+) => {{
            match (&$left, &$right) {
                (left, right) if !(*left == *right) => {
                    return Err(format!("assertion `left == right` failed\n left: {left:?}\n right: {right:?}: {}", format_args!($($message)+)).into());
                }
                _ => {}
            }
        }};
    }

    struct TestScratch;
    struct TestAllocation(tempfile::TempDir, u64, AtomicU64);

    impl ScratchAllocation for TestAllocation {
        fn path(&self) -> &Path {
            self.0.path()
        }

        fn max_bytes(&self) -> u64 {
            self.1
        }

        fn reserved_bytes(&self) -> Result<u64, IndexError> {
            Ok(self.2.load(Ordering::Acquire))
        }

        fn reserve_at_least(
            &self,
            target_bytes: u64,
            budget: &mut WorkBudget,
        ) -> Result<(), IndexError> {
            budget.checkpoint()?;
            if target_bytes > self.1 {
                return Err(IndexError::ResourceLimited);
            }
            let _previous = self.2.fetch_max(target_bytes, Ordering::AcqRel);
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
            let directory = tempfile::tempdir()
                .map_err(|error| IndexError::Storage(format!("test scratch: {error}")))?;
            Ok(Box::new(TestAllocation(
                directory,
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
                bytes: 64 * 1024 * 1024,
                mutations: 100_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    fn reference(number: u32) -> ObjectRef {
        let mut bytes = [0u8; 32];
        for (target, value) in bytes.iter_mut().zip(number.to_be_bytes()) {
            *target = value;
        }
        ObjectRef(Digest(bytes))
    }

    #[test]
    fn cross_page_path_cycle_emits_sorted_exact_owner_and_path_edits()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut work = budget()?;
        let scratch = InventoryScratch::open(&TestScratch, &mut work)?;
        let path = |number: u16| format!("path\0p{number:04}").into_bytes();
        let mut expected = BTreeMap::new();
        for number in 0u16..256 {
            let mut stable = [0u8; 16];
            stable[..2].copy_from_slice(&number.to_be_bytes());
            let owner = OwnerKey {
                producer: ProducerId([7; 16]),
                stable_id: StableOwnerId(stable),
            };
            let old = reference(u32::from(number).checked_add(1).ok_or("old ref overflow")?);
            let source = reference(
                u32::from(number)
                    .checked_add(513)
                    .ok_or("source ref overflow")?,
            );
            scratch.record_owner(owner, Some(old), Some(source), &mut work)?;
            scratch.record_prior_path(&path(number), old, &mut work)?;
            let target = if number == 255 {
                0
            } else {
                number.checked_add(1).ok_or("target overflow")?
            };
            scratch.record_next_path(&path(target), &mut work)?;
            let new = reference(
                u32::from(number)
                    .checked_add(1025)
                    .ok_or("new ref overflow")?,
            );
            scratch.set_next(&owner_map_key(owner), &path(target), new, &mut work)?;
            let _previous = expected.insert(owner_map_key(owner).to_vec(), (old, new));
        }
        let mut cursor = InventoryEditCursor::new(&scratch);
        let mut owner_count = 0usize;
        let mut path_count = 0usize;
        let mut previous = None;
        while let Some(edit) = cursor.next_edit(&mut work)? {
            if let Some((root, key)) = &previous {
                check!(*root < edit.root || (*root == edit.root && key < &edit.edit.key));
            }
            previous = Some((edit.root, edit.edit.key.clone()));
            match edit.root {
                0 => {
                    let pair = expected.get(&edit.edit.key).ok_or("unexpected owner")?;
                    check_eq!(edit.edit.expected, Some(pair.0.into()));
                    check_eq!(edit.edit.next, Some(pair.1.into()));
                    owner_count = owner_count.checked_add(1).ok_or("owner count overflow")?;
                }
                1 => {
                    check_eq!(edit.edit.key.get(..5), Some(b"path\0".as_slice()));
                    check!(edit.edit.expected.is_some());
                    check!(edit.edit.next.is_some());
                    path_count = path_count.checked_add(1).ok_or("path count overflow")?;
                }
                _ => return Err("unexpected edit root".into()),
            }
        }
        check_eq!(owner_count, 256);
        check_eq!(path_count, 256);
        check!(matches!(
            scratch.record_next_path(&path(0), &mut work),
            Err(IndexError::InvalidInput(_))
        ));
        let directory = scratch.allocation.path().to_path_buf();
        drop(scratch);
        check!(!directory.exists());
        Ok(())
    }

    #[test]
    fn inventory_scratch_main_page_limit_preserves_ambiguous_storage_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut work = budget()?;
        let scratch = InventoryScratch::open(&TestScratch, &mut work)?;
        let page_size: u32 = scratch
            .db
            .scoped()?
            .query_row("PRAGMA page_size", [], |row| row.get(0))?;
        let max_pages: u32 =
            scratch
                .db
                .scoped()?
                .query_row("PRAGMA max_page_count", [], |row| row.get(0))?;
        let temp_store: u32 = scratch
            .db
            .scoped()?
            .query_row("PRAGMA temp_store", [], |row| row.get(0))?;
        check_eq!(page_size, SCRATCH_PAGE_SIZE);
        check!(
            u64::from(max_pages)
                .checked_mul(u64::from(page_size))
                .ok_or("page byte count overflow")?
                <= scratch.allocation.reserved_bytes()?
        );
        check!(scratch.allocation.reserved_bytes()? >= SCRATCH_INITIAL_BYTES);
        check_eq!(temp_store, 1);
        let _smaller_limit: u32 =
            scratch
                .db
                .scoped()?
                .query_row("PRAGMA max_page_count=8", [], |row| row.get(0))?;
        let inserted = scratch
            .db
            .scoped()?
            .execute("INSERT INTO path(key) VALUES (zeroblob(1048576))", []);
        let Err(error) = inserted else {
            return Err("configured main-page cap accepted an oversized write".into());
        };
        check!(matches!(
            &error,
            rusqlite::Error::SqliteFailure(failure, _)
                if failure.code == rusqlite::ErrorCode::DiskFull
        ));
        check!(matches!(sql_error(&error), IndexError::Storage(_)));
        Ok(())
    }
}
