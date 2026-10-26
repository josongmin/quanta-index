//! Disposable, private map-build scratch. Its rows never authorize a view.

use quanta_index_core::index::ports::MapEditCursor;
use quanta_index_core::index::{
    IndexError, ObjectRef, OrdinalMapEdit, ScratchAllocation, ScratchRequest, WorkBudget, WorkUsage,
};
use quanta_index_scratch_sqlite::{MapEditPage, ScratchConnection};
use rusqlite::{OptionalExtension as _, params, params_from_iter};

use super::{Node, decode_node};
use crate::{
    FileObjectStore,
    object::{AdmittedWire, PreparedObject, decode_stored_wire},
};

const SCRATCH_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const NODE_READ_BOUND: usize = 16 + super::MAX_NODE_CHILDREN * 32 + super::MAX_NODE_PAYLOAD;
const EDIT_READ_BOUND: usize = 4096 + 2 * quanta_index_core::index::MAX_MAP_VALUE_WIRE_BYTES + 128;
// Do not materialize an oversized or non-BLOB scratch value. The borrowed,
// bounded wire supplies its own length; owned backing is admitted before copy.
const NODE_WIRE_SQL: &str = "SELECT CASE WHEN typeof(wire)='blob'
    AND length(wire) BETWEEN 1 AND ?2 THEN wire END FROM node WHERE reference=?1";
// One SQL execution per admitted 64-row packet. Unused bindings are NULL, so
// they cannot match a non-NULL primary key. Keep the parameter count coupled
// to the scratch writer's fixed page size.
const _: [(); 64] = [(); quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];
const MARK_COMPLETE_SQL: &str = concat!(
    "UPDATE mark SET state=2 WHERE state=1 AND reference IN (",
    "?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,",
    "?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,",
    "?33,?34,?35,?36,?37,?38,?39,?40,?41,?42,?43,?44,?45,?46,?47,?48,",
    "?49,?50,?51,?52,?53,?54,?55,?56,?57,?58,?59,?60,?61,?62,?63,?64)"
);

pub(crate) struct MapScratch {
    db: ScratchConnection,
    allocation: Box<dyn ScratchAllocation>,
}

/// One fixed root with bounded keyset pages. Cached rows, EOF and failure all
/// remain inside the original request; a failed cursor cannot skip consumed rows.
pub(super) struct EditCursor<'a> {
    scratch: &'a MapScratch,
    root: u32,
    after: Option<OrdinalMapEdit>,
    page: MapEditPage,
    failed: Option<IndexError>,
}

impl EditCursor<'_> {
    fn next_inner(
        &mut self,
        budget: &mut WorkBudget,
    ) -> Result<Option<OrdinalMapEdit>, IndexError> {
        loop {
            budget.checkpoint()?;
            if let Some(edit) = self.page.pop()? {
                self.after = Some(edit.clone());
                return Ok(Some(edit));
            }
            if self.page.complete() {
                return Ok(None);
            }
            let db = self.scratch.db.scoped()?;
            let query = if self.after.is_some() {
                "SELECT key,expected,next FROM edit WHERE root=?1 AND key>?2 ORDER BY key LIMIT 64"
            } else {
                // A BLOB lower bound would hide corrupt INTEGER/TEXT keys.
                "SELECT key,expected,next FROM edit WHERE root=?1 ORDER BY key LIMIT 64"
            };
            let root = i64::from(self.root);
            let after = self
                .after
                .as_ref()
                .map(|position| position.edit.key.as_slice());
            let first: [&dyn rusqlite::ToSql; 1] = [&root];
            let next: [&dyn rusqlite::ToSql; 2] = [&root, &after];
            let parameters: &[&dyn rusqlite::ToSql] = if after.is_some() { &next } else { &first };
            let mut statement = db
                .prepare_cached(query)
                .map_err(|error| sql_error(&error))?;
            self.page = MapEditPage::read(
                &mut statement,
                parameters,
                u64::try_from(EDIT_READ_BOUND).map_err(|_error| IndexError::ResourceLimited)?,
                budget,
                |row, budget| {
                    quanta_index_scratch_sqlite::read_map_edit_budgeted(
                        row, 4096, self.root, budget,
                    )
                },
            )?;
        }
    }
}

impl MapEditCursor for EditCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        if let Some(error) = &self.failed {
            return Err(error.clone());
        }
        let result = self.next_inner(budget);
        if let Err(error) = &result {
            self.failed = Some(error.clone());
        }
        result
    }
}

impl MapScratch {
    pub(crate) fn open(
        store: &FileObjectStore,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            bytes: 8192,
            ..WorkUsage::default()
        })?;
        let allocation = store.scratch_space().allocate(
            ScratchRequest {
                initial_bytes: 256 * 1024,
                max_bytes: SCRATCH_MAX_BYTES,
            },
            budget,
        )?;
        if allocation.max_bytes() != SCRATCH_MAX_BYTES {
            return Err(IndexError::BackendViolation(
                "map scratch allocation changed its envelope".into(),
            ));
        }
        let owned_db = ScratchConnection::open(
            &allocation.path().join("scratch.sqlite"),
            allocation.as_ref(),
        )?;
        let db = owned_db.scoped()?;
        quanta_index_scratch_sqlite::configure(&db, allocation.as_ref())?;
        quanta_index_scratch_sqlite::reserve_for_write(&db, allocation.as_ref(), 8192, budget)?;
        db.execute_batch(
            "PRAGMA page_size=4096;
             PRAGMA cache_size=-2048;
             PRAGMA mmap_size=0;
             PRAGMA journal_mode=OFF;
             PRAGMA synchronous=OFF;
             PRAGMA temp_store=FILE;
             CREATE TABLE node(reference BLOB PRIMARY KEY NOT NULL, wire BLOB NOT NULL) WITHOUT ROWID;
             CREATE TABLE edit(root INTEGER NOT NULL, key BLOB NOT NULL,
               expected BLOB, next BLOB, PRIMARY KEY(root,key)) WITHOUT ROWID;
             CREATE TABLE mark(reference BLOB PRIMARY KEY NOT NULL, state INTEGER NOT NULL) WITHOUT ROWID;"
        )
        .map_err(|error| sql_error(&error))?;
        drop(db);
        Ok(Self {
            db: owned_db,
            allocation,
        })
    }

    /// Coalesce disposable writes without retaining an operation's VFS scope.
    pub(crate) fn begin_batch(&self, budget: &WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let db = self.db.scoped()?;
        if db.is_autocommit() {
            db.execute_batch("BEGIN IMMEDIATE")
                .map_err(|error| sql_error(&error))?;
        }
        Ok(())
    }

    /// A failed OFF-journal commit poisons this attempt: callers must discard it.
    pub(crate) fn commit_batch(&self, budget: &WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let db = self.db.scoped()?;
        // The preceding full group can also commit the next object's mark.
        // Its final flush then has no new mark transaction to finish.
        if !db.is_autocommit() {
            db.execute_batch("COMMIT")
                .map_err(|error| sql_error(&error))?;
        }
        Ok(())
    }

    pub(crate) fn insert_node(
        &self,
        object: &PreparedObject,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            bytes: object.registration.bytes,
            ..WorkUsage::default()
        })?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.db.scoped()?,
            self.allocation.as_ref(),
            object.registration.bytes,
            budget,
        )?;
        let key = (object.registration.reference.0).0;
        let inserted = {
            let db = self.db.scoped()?;
            let mut statement = db
                .prepare_cached("INSERT OR IGNORE INTO node(reference,wire) VALUES (?1,?2)")
                .map_err(|error| sql_error(&error))?;
            statement
                .execute(params![key.as_slice(), object.wire.as_slice()])
                .map_err(|error| sql_error(&error))?
        };
        if inserted == 0 {
            let equal = {
                let db = self.db.scoped()?;
                let mut statement = db
                    .prepare_cached("SELECT wire FROM node WHERE reference=?1")
                    .map_err(|error| sql_error(&error))?;
                statement
                    .query_row(params![key.as_slice()], |row| {
                        Ok(quanta_index_scratch_sqlite::bounded_blob(
                            row.get_ref(0)?,
                            object.wire.len(),
                        )
                        .map(|existing| existing == object.wire.as_slice()))
                    })
                    .map_err(|error| sql_error(&error))??
            };
            if !equal {
                return Err(IndexError::CorruptObject(
                    "staged object digest collision".into(),
                ));
            }
        }
        Ok(())
    }

    pub(crate) fn prepared(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<PreparedObject>, IndexError> {
        self.wire(reference, budget)?
            .map(|wire| {
                let stored = decode_stored_wire(reference, &wire.bytes, budget)?;
                Ok(PreparedObject::from_wire(stored.registration.clone(), wire))
            })
            .transpose()
    }

    fn wire(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<AdmittedWire>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let key = (reference.0).0;
        let io_scope = self.db.scoped()?;
        let mut statement = io_scope
            .prepare_cached(NODE_WIRE_SQL)
            .map_err(|error| sql_error(&error))?;
        let bound = i64::try_from(NODE_READ_BOUND).map_err(|_error| IndexError::ResourceLimited)?;
        let mut rows = statement
            .query(params![key.as_slice(), bound])
            .map_err(|error| sql_error(&error))?;
        let Some(row) = rows.next().map_err(|error| sql_error(&error))? else {
            return Ok(None);
        };
        let rusqlite::types::ValueRef::Blob(borrowed) =
            row.get_ref(0).map_err(|error| sql_error(&error))?
        else {
            return Err(IndexError::CorruptObject(
                "scratch map node wire has invalid type or physical length".into(),
            ));
        };
        let length = u64::try_from(borrowed.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: length,
            ..WorkUsage::default()
        })?;
        let mut wire = AdmittedWire::new(
            usize::try_from(length).map_err(|_error| IndexError::ResourceLimited)?,
            budget,
        )?;
        wire.bytes.extend_from_slice(borrowed);
        drop(rows);
        drop(statement);
        drop(io_scope);
        Ok(Some(wire))
    }

    pub(crate) fn read_node(
        &self,
        store: &FileObjectStore,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Node, IndexError> {
        if let Some(wire) = self.wire(reference, budget)? {
            decode_node(&decode_stored_wire(reference, &wire.bytes, budget)?, budget)
        } else {
            store.read_node(reference, budget)
        }
    }

    #[cfg(test)]
    pub(crate) fn insert_edit(
        &self,
        ordinal: &OrdinalMapEdit,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.insert_edits(std::slice::from_ref(ordinal), budget)
    }

    pub(super) fn insert_edits(
        &self,
        edits: &[OrdinalMapEdit],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let reservations = [u64::try_from(EDIT_READ_BOUND)
            .map_err(|_error| IndexError::ResourceLimited)?;
            quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];
        let reservations = reservations
            .get(..edits.len())
            .ok_or(IndexError::ResourceLimited)?;
        let db = self.db.scoped()?;
        quanta_index_scratch_sqlite::write_map_edit_batch(
            &db,
            self.allocation.as_ref(),
            "INSERT INTO edit(root,key,expected,next) VALUES ",
            u64::try_from(EDIT_READ_BOUND).map_err(|_error| IndexError::ResourceLimited)?,
            reservations,
            |slot| {
                let edit = edits.get(slot).ok_or(IndexError::ResourceLimited)?;
                Ok((
                    rusqlite::types::ValueRef::Integer(i64::from(edit.root)),
                    &edit.edit,
                    0,
                ))
            },
            budget,
        )
    }

    pub(super) fn edits(&self, root: u32) -> EditCursor<'_> {
        EditCursor {
            scratch: self,
            root,
            after: None,
            page: MapEditPage::default(),
            failed: None,
        }
    }

    pub(crate) fn mark(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Option<u8>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let key = (reference.0).0;
        let db = self.db.scoped()?;
        let mut statement = db
            .prepare_cached("SELECT state FROM mark WHERE reference=?1")
            .map_err(|error| sql_error(&error))?;
        statement
            .query_row(params![key.as_slice()], |row| row.get(0))
            .optional()
            .map_err(|error| sql_error(&error))
    }

    pub(crate) fn set_mark(
        &self,
        reference: ObjectRef,
        state: u8,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            ..WorkUsage::default()
        })?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.db.scoped()?,
            self.allocation.as_ref(),
            64,
            budget,
        )?;
        let key = (reference.0).0;
        let db = self.db.scoped()?;
        let mut statement = db
            .prepare_cached(
                "INSERT INTO mark(reference,state) VALUES (?1,?2)
                 ON CONFLICT(reference) DO UPDATE SET state=excluded.state",
            )
            .map_err(|error| sql_error(&error))?;
        let _updated = statement
            .execute(params![key.as_slice(), state])
            .map_err(|error| sql_error(&error))?;
        Ok(())
    }

    /// Complete one physical install group, admitting each bounded SQL batch
    /// before mutation. The caller owns every authenticated wire through commit.
    pub(crate) fn complete_batch(
        &self,
        objects: &[&PreparedObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if objects.is_empty() || objects.len() > crate::object::MAX_INSTALL_BATCH_INPUTS {
            return Err(IndexError::ResourceLimited);
        }
        for group in objects.chunks(quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS) {
            budget.charge(WorkUsage {
                writes: u64::try_from(group.len()).map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let reservations = [64_u64; quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];
            let db = self.db.scoped()?;
            quanta_index_scratch_sqlite::reserve_for_write_batch(
                &db,
                self.allocation.as_ref(),
                reservations
                    .get(..group.len())
                    .ok_or(IndexError::ResourceLimited)?,
                budget,
            )?;
            let keys: [Option<&[u8]>; quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS] =
                std::array::from_fn(|slot| {
                    group
                        .get(slot)
                        .map(|object| (object.registration.reference.0).0.as_slice())
                });
            let changed = db
                .prepare_cached(MARK_COMPLETE_SQL)
                .map_err(|error| sql_error(&error))?
                .execute(params_from_iter(keys))
                .map_err(|error| sql_error(&error))?;
            // The authenticated install group contains unique references.
            // A duplicate, absent or already-complete mark must fail closed
            // before the caller commits and physically installs this group.
            if changed != group.len() {
                return Err(IndexError::CorruptObject(
                    "completed map node was not active".into(),
                ));
            }
        }
        Ok(())
    }
}

fn sql_error(error: &rusqlite::Error) -> IndexError {
    quanta_index_scratch_sqlite::storage_error(error, "map scratch")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::Arc,
        time::{Duration, Instant},
    };

    use quanta_index_core::index::{
        Digest, MapEdit, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope,
        MemoryScopePort, ScratchSpacePort, WorkLimits,
    };
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Ledger {
        cap: u64,
        live: AtomicU64,
        peak: AtomicU64,
    }
    impl MemoryLeaseOwner for Ledger {
        fn release(&self, bytes: u64) {
            let _prior = self.live.fetch_sub(bytes, Ordering::SeqCst);
        }
    }
    struct Scope(Arc<Ledger>);
    impl MemoryScopePort for Scope {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let prior = self.0.live.load(Ordering::SeqCst);
            let next = prior
                .checked_add(bytes)
                .ok_or(IndexError::ResourceLimited)?;
            if next > self.0.cap {
                return Err(IndexError::ResourceLimited);
            }
            self.0.live.store(next, Ordering::SeqCst);
            let _prior_peak = self.0.peak.fetch_max(next, Ordering::SeqCst);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    use super::*;

    fn assertion_error(detail: String) -> Box<dyn std::error::Error> {
        std::io::Error::other(detail).into()
    }

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(assertion_error(format!(
                    "assertion failed: {}",
                    stringify!($condition)
                )));
            }
        }};
        ($condition:expr, $($message:tt)+) => {{
            if !$condition {
                return Err(assertion_error(format!($($message)+)));
            }
        }};
    }

    #[test]
    fn staged_completion_reopens_one_transaction_after_an_earlier_flush()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::super::{MapStage as _, StagedMap};

        let directory = tempfile::tempdir()?;
        let store = FileObjectStore::open(directory.path().join("store"))?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 1_000,
                bytes: 16 * 1024 * 1024,
                mutations: 1_000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let objects = (0_u16..65)
            .map(|id| store.prepare_object(&id.to_be_bytes(), &[], &mut budget))
            .collect::<Result<Vec<_>, _>>()?;
        let stage = StagedMap::open(&store, &mut budget)?;
        stage.begin_batch(&budget)?;
        for object in &objects {
            stage.set_mark(object.registration.reference, 1, &mut budget)?;
        }
        stage.commit_batch(&budget)?;
        check!(stage.scratch.db.scoped()?.is_autocommit());
        let references = objects.iter().collect::<Vec<_>>();
        stage.complete_batch(&references, &mut budget)?;
        check!(!stage.scratch.db.scoped()?.is_autocommit());
        for object in &objects {
            check!(stage.mark(object.registration.reference, &mut budget)? == Some(2));
        }
        stage.commit_batch(&budget)?;
        check!(stage.scratch.db.scoped()?.is_autocommit());
        Ok(())
    }

    #[test]
    fn completion_batches_preserve_each_mark_charge_and_refuse_unvisited_nodes()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store = FileObjectStore::open(directory.path().join("store"))?;
        let work = |writes| -> Result<WorkBudget, IndexError> {
            Ok(WorkBudget::new(
                WorkLimits {
                    reads: 10_000,
                    writes,
                    bytes: 16 * 1024 * 1024,
                    mutations: 10_000,
                },
                Instant::now()
                    .checked_add(Duration::from_secs(30))
                    .ok_or(IndexError::ResourceLimited)?,
            ))
        };
        let mut setup = work(10_000)?;
        let objects = (0_u16..65)
            .map(|id| store.prepare_object(&id.to_be_bytes(), &[], &mut setup))
            .collect::<Result<Vec<_>, _>>()?;
        let references = objects.iter().collect::<Vec<_>>();
        let scratch = MapScratch::open(&store, &mut setup)?;
        scratch.begin_batch(&setup)?;
        for object in &objects {
            scratch.set_mark(object.registration.reference, 1, &mut setup)?;
        }
        let mut exact = work(65)?;
        scratch.complete_batch(&references, &mut exact)?;
        check!(exact.usage().writes == 65);
        for object in &objects {
            check!(scratch.mark(object.registration.reference, &mut setup)? == Some(2));
        }
        scratch.commit_batch(&setup)?;
        check!(matches!(
            scratch.complete_batch(&references, &mut setup),
            Err(IndexError::CorruptObject(_))
        ));
        drop(scratch);

        let scratch = MapScratch::open(&store, &mut setup)?;
        scratch.begin_batch(&setup)?;
        for object in &objects {
            scratch.set_mark(object.registration.reference, 1, &mut setup)?;
        }
        let mut refused = work(63)?;
        check!(matches!(
            scratch.complete_batch(&references, &mut refused),
            Err(IndexError::ResourceLimited)
        ));
        check!(refused.usage().writes == 0);
        for object in &objects {
            check!(scratch.mark(object.registration.reference, &mut setup)? == Some(1));
        }
        // A failed attempt is discarded. Its marks never authorize installation.
        drop(scratch);

        let scratch = MapScratch::open(&store, &mut setup)?;
        scratch.begin_batch(&setup)?;
        let mut reading = work(100)?;
        check!(matches!(
            scratch.complete_batch(
                references.get(..1).ok_or("missing reference")?,
                &mut reading
            ),
            Err(IndexError::CorruptObject(_))
        ));
        check!(
            scratch
                .mark(
                    objects
                        .first()
                        .ok_or("missing object")?
                        .registration
                        .reference,
                    &mut setup
                )?
                .is_none()
        );
        drop(scratch);

        // A bad last binding inside one SQL packet cannot be hidden by the
        // preceding 63 valid marks. This failed scratch is discarded.
        let scratch = MapScratch::open(&store, &mut setup)?;
        scratch.begin_batch(&setup)?;
        for object in objects.iter().take(63) {
            scratch.set_mark(object.registration.reference, 1, &mut setup)?;
        }
        let first_packet = references.get(..64).ok_or("missing packet")?;
        check!(matches!(
            scratch.complete_batch(first_packet, &mut setup),
            Err(IndexError::CorruptObject(_))
        ));
        drop(scratch);

        // Duplicate references cannot satisfy the group's exact transition
        // count even though their single underlying row was initially active.
        let scratch = MapScratch::open(&store, &mut setup)?;
        scratch.begin_batch(&setup)?;
        let first = objects.first().ok_or("missing object")?;
        scratch.set_mark(first.registration.reference, 1, &mut setup)?;
        check!(matches!(
            scratch.complete_batch(&[first, first], &mut setup),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    #[test]
    fn node_wire_uses_one_vm_run_and_original_read_byte_cancel_limits()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store = FileObjectStore::open(directory.path().join("store"))?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let work = |reads, bytes| {
            WorkBudget::new(
                WorkLimits {
                    reads,
                    writes: 100,
                    bytes,
                    mutations: 100,
                },
                deadline,
            )
        };
        let mut setup = work(100, 1_000_000);
        let object = store.prepare_object(b"node", &[], &mut setup)?;
        let scratch = MapScratch::open(&store, &mut setup)?;
        scratch.insert_node(&object, &mut setup)?;
        let wire_bytes = u64::try_from(object.wire.len())?;
        let mut reading = work(1, wire_bytes);
        let wire = scratch
            .wire(object.registration.reference, &mut reading)?
            .ok_or("missing scratch wire")?;
        check!(wire.bytes.as_slice() == object.wire.as_slice());
        check!(reading.usage().reads == 1);
        check!(reading.usage().bytes == wire_bytes);
        drop(wire);
        let runs = || -> Result<i32, Box<dyn std::error::Error>> {
            let db = scratch.db.scoped()?;
            let statement = db.prepare_cached(NODE_WIRE_SQL)?;
            Ok(statement.get_status(rusqlite::StatementStatus::Run))
        };
        check!(runs()? == 1);
        let mut denied = work(0, wire_bytes);
        check!(matches!(
            scratch.wire(object.registration.reference, &mut denied),
            Err(IndexError::ResourceLimited)
        ));
        let mut cancelled = work(1, wire_bytes);
        cancelled.request().cancel_handle().cancel();
        check!(matches!(
            scratch.wire(object.registration.reference, &mut cancelled),
            Err(IndexError::Cancelled)
        ));
        check!(runs()? == 1);
        let ledger = Arc::new(Ledger {
            cap: 1000,
            live: AtomicU64::new(0),
            peak: AtomicU64::new(0),
        });
        let mut short = work(
            1,
            wire_bytes
                .checked_sub(1)
                .ok_or(IndexError::ResourceLimited)?,
        );
        short.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
        check!(matches!(
            scratch.wire(object.registration.reference, &mut short),
            Err(IndexError::ResourceLimited)
        ));
        check!(ledger.peak.load(Ordering::SeqCst) == 0);
        check!(runs()? == 2);
        let mut absent = work(1, 0);
        check!(
            scratch
                .wire(ObjectRef(Digest([99; 32])), &mut absent)?
                .is_none()
        );
        check!(runs()? == 3);
        Ok(())
    }

    #[test]
    fn node_wire_guards_bad_sql_shapes_before_owned_birth_and_authenticates_bytes()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store = FileObjectStore::open(directory.path().join("store"))?;
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let object = store.prepare_object(b"node", &[], &mut work)?;
        let scratch = MapScratch::open(&store, &mut work)?;
        scratch.insert_node(&object, &mut work)?;
        let key = object.registration.reference.0.0;
        let ledger = Arc::new(Ledger {
            cap: 0,
            live: AtomicU64::new(0),
            peak: AtomicU64::new(0),
        });
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
        for invalid in [
            rusqlite::types::Value::Text("node".into()),
            rusqlite::types::Value::Integer(42),
            rusqlite::types::Value::Blob(Vec::new()),
            rusqlite::types::Value::Blob(vec![
                0;
                NODE_READ_BOUND
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?
            ]),
        ] {
            let changed = scratch.db.scoped()?.execute(
                "UPDATE node SET wire=?1 WHERE reference=?2",
                params![invalid, key.as_slice()],
            )?;
            check!(changed == 1);
            check!(matches!(
                scratch.prepared(object.registration.reference, &mut work),
                Err(IndexError::CorruptObject(_))
            ));
            check!(ledger.peak.load(Ordering::SeqCst) == 0);
        }
        // A bounded BLOB still needs the original wire/hash authentication.
        work.bind_memory_scope(None);
        let changed = scratch.db.scoped()?.execute(
            "UPDATE node SET wire=?1 WHERE reference=?2",
            params![b"not an authenticated object".as_slice(), key.as_slice()],
        )?;
        check!(changed == 1);
        check!(matches!(
            scratch.prepared(object.registration.reference, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    #[test]
    fn prepared_wire_admits_before_decode_and_retains_charge_after_decode()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store = FileObjectStore::open(directory.path().join("store"))?;
        let mut preparing = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let object = store.prepare_object(b"node", &[], &mut preparing)?;
        let scratch = MapScratch::open(&store, &mut preparing)?;
        scratch.insert_node(&object, &mut preparing)?;
        for cap in [0, 20, 1000] {
            let ledger = Arc::new(Ledger {
                cap,
                live: AtomicU64::new(0),
                peak: AtomicU64::new(0),
            });
            preparing.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
            if cap == 0 {
                // An identical staged row is compared through SQLite's borrowed BLOB.
                scratch.insert_node(&object, &mut preparing)?;
                check!(ledger.peak.load(Ordering::SeqCst) == 0);
            }
            let result = scratch.prepared(object.registration.reference, &mut preparing);
            if cap == 1000 {
                let prepared = result?.ok_or("missing scratch object")?;
                check!(prepared.registration == object.registration);
                check!(prepared.wire == object.wire);
                check!(
                    ledger.live.load(Ordering::SeqCst)
                        == 20 + quanta_index_core::index::RegisteredObject::allocation_bytes(0)?
                );
                drop(prepared);
            } else {
                check!(matches!(result, Err(IndexError::ResourceLimited)));
                check!(ledger.peak.load(Ordering::SeqCst) == cap);
            }
            check!(ledger.live.load(Ordering::SeqCst) == 0);
        }
        Ok(())
    }

    #[test]
    fn edit_key_admission_refusal_shared_continuation_and_decode_cleanup()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store = FileObjectStore::open(directory.path().join("store"))?;
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let scratch = MapScratch::open(&store, &mut work)?;
        let expected = ObjectRef(Digest([2; 32]));
        let next = ObjectRef(Digest([4; 32]));
        let key = b"prefix\0a";
        scratch.insert_edit(
            &OrdinalMapEdit::unscoped(
                3,
                MapEdit {
                    key: key.to_vec(),
                    expected: Some((expected).into()),
                    next: Some((next).into()),
                },
            ),
            &mut work,
        )?;
        let bytes = OrdinalMapEdit::allocation_bytes(key.len())?;
        let page_bytes = u64::try_from(
            64_usize
                .checked_mul(std::mem::size_of::<OrdinalMapEdit>())
                .ok_or(IndexError::ResourceLimited)?,
        )?;
        let combined = bytes
            .checked_add(page_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        for cap in [0, combined] {
            let ledger = Arc::new(Ledger {
                cap,
                live: AtomicU64::new(0),
                peak: AtomicU64::new(0),
            });
            work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
            let mut cursor = scratch.edits(3);
            let result = cursor.next_edit(&mut work);
            if cap == 0 {
                check!(matches!(result, Err(IndexError::ResourceLimited)));
                check!(ledger.peak.load(Ordering::SeqCst) == 0);
            } else {
                let position = result?.ok_or("missing edit")?;
                check!(position.root == 3);
                check!(position.edit.key == key);
                check!(position.edit.expected == Some(expected.into()));
                check!(position.edit.next == Some(next.into()));
                let continuation = position.clone();
                check!(std::ptr::eq(
                    position.edit.key.as_ptr(),
                    continuation.edit.key.as_ptr()
                ));
                check!(ledger.live.load(Ordering::SeqCst) == combined);
                drop(position);
                check!(cursor.next_edit(&mut work)?.is_none());
                check!(ledger.peak.load(Ordering::SeqCst) == combined);
                drop(cursor);
                check!(ledger.live.load(Ordering::SeqCst) == bytes);
                drop(continuation);
            }
            check!(ledger.live.load(Ordering::SeqCst) == 0);
        }
        let _changed = scratch.db.scoped()?.execute(
            "UPDATE edit SET expected=?1 WHERE root=3",
            params![&[1_u8; 31]],
        )?;
        let refused = Arc::new(Ledger {
            cap: 0,
            live: AtomicU64::new(0),
            peak: AtomicU64::new(0),
        });
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(refused.clone())))));
        check!(matches!(
            scratch.edits(3).next_edit(&mut work),
            Err(IndexError::CorruptObject(_))
        ));
        check!(refused.peak.load(Ordering::SeqCst) == 0);
        Ok(())
    }

    #[test]
    fn paged_edits_preserve_root_empty_key_inline_values_and_bounded_read_cost()
    -> Result<(), Box<dyn std::error::Error>> {
        use quanta_index_core::index::MapValue;
        let directory = tempfile::tempdir()?;
        let store = FileObjectStore::open(directory.path().join("store"))?;
        let limits = WorkLimits {
            reads: 1000,
            writes: 1000,
            bytes: 10_000_000,
            mutations: 1000,
        };
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let mut setup = WorkBudget::new(limits, deadline);
        let scratch = MapScratch::open(&store, &mut setup)?;
        let value = MapValue::inline(&[11; 32], [ObjectRef(Digest([7; 32])); 5])?;
        for index in 0_u16..129 {
            let key = if index == 0 {
                Vec::new()
            } else {
                index.to_be_bytes().to_vec()
            };
            scratch.insert_edit(
                &OrdinalMapEdit::unscoped(
                    3,
                    MapEdit {
                        key,
                        expected: None,
                        next: Some(value),
                    },
                ),
                &mut setup,
            )?;
        }
        scratch.insert_edit(
            &OrdinalMapEdit::unscoped(
                7,
                MapEdit {
                    key: Vec::new(),
                    expected: None,
                    next: Some(ObjectRef(Digest([99; 32])).into()),
                },
            ),
            &mut setup,
        )?;
        // Independent small-reader contract: all129 original rows must fit
        // 128KiB. One maximal-key query for every row cannot satisfy it.
        let read_limits = WorkLimits {
            reads: 1000,
            writes: 0,
            bytes: 128 * 1024,
            mutations: 0,
        };
        let mut work = WorkBudget::new(read_limits, deadline);
        let mut cursor = scratch.edits(3);
        for index in 0_u16..129 {
            let edit = cursor.next_edit(&mut work)?.ok_or("paged edit missing")?;
            let key = if index == 0 {
                Vec::new()
            } else {
                index.to_be_bytes().to_vec()
            };
            check!(edit.root == 3 && edit.edit.key == key);
            check!(edit.edit.expected.is_none() && edit.edit.next == Some(value));
        }
        check!(cursor.next_edit(&mut work)?.is_none());
        check!(work.usage().reads == 129);
        let mut expired = WorkBudget::new(read_limits, Instant::now());
        check!(matches!(
            cursor.next_edit(&mut expired),
            Err(IndexError::ResourceLimited)
        ));
        check!(matches!(
            cursor.next_edit(&mut work),
            Err(IndexError::ResourceLimited)
        ));
        drop(cursor);
        let mut cached = scratch.edits(3);
        check!(cached.next_edit(&mut setup)?.is_some());
        check!(matches!(
            cached.next_edit(&mut expired),
            Err(IndexError::ResourceLimited)
        ));
        check!(matches!(
            cached.next_edit(&mut setup),
            Err(IndexError::ResourceLimited)
        ));
        drop(cached);

        let _updated = scratch.db.scoped()?.execute(
            "UPDATE edit SET next=?1 WHERE root=3 AND key=?2",
            params![&[1_u8; 31], 70_u16.to_be_bytes().as_slice()],
        )?;
        let mut malformed = scratch.edits(3);
        for _index in 0..70 {
            check!(malformed.next_edit(&mut setup)?.is_some());
        }
        for _repeat in 0..2 {
            check!(matches!(
                malformed.next_edit(&mut setup),
                Err(IndexError::CorruptObject(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn first_edit_page_does_not_hide_non_blob_keys() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store = FileObjectStore::open(directory.path().join("store"))?;
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let scratch = MapScratch::open(&store, &mut work)?;
        for key in [
            rusqlite::types::Value::Integer(1),
            rusqlite::types::Value::Text("invalid-key".into()),
        ] {
            let _deleted = scratch.db.scoped()?.execute("DELETE FROM edit", [])?;
            let _inserted = scratch.db.scoped()?.execute(
                "INSERT INTO edit(root,key,expected,next) VALUES (3,?1,NULL,NULL)",
                params![key],
            )?;
            let ledger = Arc::new(Ledger {
                cap: 0,
                live: AtomicU64::new(0),
                peak: AtomicU64::new(0),
            });
            work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(ledger.clone())))));
            let mut cursor = scratch.edits(3);
            for _repeat in 0..2 {
                check!(matches!(
                    cursor.next_edit(&mut work),
                    Err(IndexError::CorruptObject(_))
                ));
            }
            check!(
                ledger.live.load(Ordering::SeqCst) == 0 && ledger.peak.load(Ordering::SeqCst) == 0
            );
        }
        Ok(())
    }

    #[test]
    fn duplicate_root_fingerprint_rejects_malformed_secondary_scratch_row()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store = FileObjectStore::open(directory.path().join("store"))?;
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        let scratch = MapScratch::open(&store, &mut work)?;
        let edit = MapEdit::objects(b"same".to_vec(), None, Some(ObjectRef(Digest([7; 32]))));
        scratch.insert_edit(&OrdinalMapEdit::unscoped(0, edit.clone()), &mut work)?;
        scratch.insert_edit(&OrdinalMapEdit::unscoped(1, edit), &mut work)?;
        let original = super::super::scratch_edit_fingerprint(&scratch, 0, &mut work)?;
        check!(original == super::super::scratch_edit_fingerprint(&scratch, 1, &mut work)?);
        let _changed = scratch
            .db
            .scoped()?
            .execute("UPDATE edit SET next=zeroblob(31) WHERE root=1", [])?;
        check!(matches!(
            super::super::scratch_edit_fingerprint(&scratch, 1, &mut work),
            Err(IndexError::CorruptObject(_))
        ));
        Ok(())
    }

    struct UndersizedAllocation {
        path: PathBuf,
    }

    impl ScratchAllocation for UndersizedAllocation {
        fn path(&self) -> &Path {
            &self.path
        }

        fn max_bytes(&self) -> u64 {
            SCRATCH_MAX_BYTES - 1
        }

        fn reserved_bytes(&self) -> Result<u64, IndexError> {
            Ok(self.max_bytes())
        }
        fn reserve_at_least(&self, bytes: u64, budget: &mut WorkBudget) -> Result<(), IndexError> {
            budget.checkpoint()?;
            if bytes > self.max_bytes() {
                return Err(IndexError::ResourceLimited);
            }
            Ok(())
        }
    }

    struct UndersizedSpace {
        path: PathBuf,
    }

    impl ScratchSpacePort for UndersizedSpace {
        fn allocate(
            &self,
            _request: ScratchRequest,
            _budget: &mut WorkBudget,
        ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
            fs::create_dir(&self.path)
                .map_err(|error| IndexError::Storage(format!("create test scratch: {error}")))?;
            Ok(Box::new(UndersizedAllocation {
                path: self.path.clone(),
            }))
        }
    }

    #[test]
    fn undersized_scratch_envelope_is_rejected_before_database_creation()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let scratch_path = root.path().join("undersized-scratch");
        let store = FileObjectStore::open_with_scratch(
            root.path().join("objects"),
            Arc::new(UndersizedSpace {
                path: scratch_path.clone(),
            }),
        )?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 10,
                writes: 10,
                bytes: 100_000,
                mutations: 10,
            },
            Instant::now()
                .checked_add(Duration::from_secs(30))
                .ok_or(IndexError::ResourceLimited)?,
        );
        check!(matches!(
            MapScratch::open(&store, &mut budget),
            Err(IndexError::BackendViolation(message)) if message.contains("changed its envelope")
        ));
        check!(scratch_path.is_dir());
        check!(!scratch_path.join("scratch.sqlite").exists());
        Ok(())
    }
}
