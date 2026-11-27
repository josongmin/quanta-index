//! Attempt-private ordered token spill for the segmented Text writer.
//!
//! The producer calls `push` for each already prepared case-key position.
//! It owns the logical token-run charge. This spool charges only its actual
//! staging copies, scratch writes, and ordered scratch reads.
#![expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "SQL bind and reservation slots are bounded by the fixed scratch batch"
)]

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ScratchAllocation, ScratchRequest, ScratchSpacePort,
    WorkBudget, WorkUsage,
};
use quanta_index_scratch_sqlite::ScratchConnection;
use std::cmp::Ordering;
use std::io::Write as _;
use std::mem::size_of;

const INITIAL_BYTES: u64 = 256 * 1024;
const MAX_BYTES: u64 = 4_294_967_296;
const BATCH: usize = quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS;
const KEY_BYTES: usize = super::wire::MAX_KEY;
const ARENA_BYTES: usize = BATCH * KEY_BYTES;
const INSERT_PREFIX: &str = "INSERT INTO tokens(term,doc,pos) VALUES ";
const INSERT_ROW: &str = "(?,?,?)";
const MAX_INSERT_SQL: usize = INSERT_PREFIX.len() + BATCH * INSERT_ROW.len() + BATCH - 1;

fn sql(error: &rusqlite::Error) -> IndexError {
    quanta_index_scratch_sqlite::storage_error(error, "native text segment spill")
}

fn width(value: usize) -> Result<u64, IndexError> {
    u64::try_from(value).map_err(|_error| IndexError::ResourceLimited)
}

#[derive(Clone, Copy)]
struct Pending {
    start: u32,
    len: u16,
    doc: u16,
    position: u32,
}

pub(super) enum SpillEvent<'a> {
    Position(u32),
    EndDocument { key: &'a [u8], doc: u16 },
    EndTerm,
}

/// One disposable, bounded write packet over a physically admitted SQLite map.
pub(super) struct SegmentSpill {
    // The connection closes before its namespace allocation retires.
    db: ScratchConnection,
    allocation: Box<dyn ScratchAllocation>,
    arena: Vec<u8>,
    rows: Vec<Pending>,
    _arena_memory: Option<MemoryLease>,
    _rows_memory: Option<MemoryLease>,
    inserted: u64,
    poisoned: bool,
}

impl SegmentSpill {
    pub(super) fn new(
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let allocation = scratch.allocate(
            ScratchRequest {
                initial_bytes: INITIAL_BYTES,
                max_bytes: MAX_BYTES,
            },
            budget,
        )?;
        if allocation.max_bytes() != MAX_BYTES {
            return Err(IndexError::BackendViolation(
                "native text segment spill envelope changed".into(),
            ));
        }
        let db = ScratchConnection::open(
            &allocation.path().join("text_segment.sqlite"),
            allocation.as_ref(),
            budget,
        )?;
        {
            let scoped = db.scoped(budget)?;
            scoped
                .execute_batch(
                    "PRAGMA page_size=4096; PRAGMA cache_size=-2048; PRAGMA mmap_size=0;
                     PRAGMA temp_store=MEMORY; PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;",
                )
                .map_err(|error| sql(&error))?;
            quanta_index_scratch_sqlite::configure(&scoped, allocation.as_ref())?;
            quanta_index_scratch_sqlite::reserve_for_write(
                &scoped,
                allocation.as_ref(),
                8192,
                budget,
            )?;
            scoped
                .execute_batch(
                    "CREATE TABLE tokens(
                       term BLOB NOT NULL, doc INTEGER NOT NULL, pos INTEGER NOT NULL,
                       PRIMARY KEY(term,doc,pos)) WITHOUT ROWID;
                     BEGIN IMMEDIATE;",
                )
                .map_err(|error| sql(&error))?;
        }
        let arena_memory = budget.reserve_memory(width(ARENA_BYTES)?, MemoryLifetime::Temporary)?;
        let rows_memory = budget.reserve_memory(
            width(
                BATCH
                    .checked_mul(size_of::<Pending>())
                    .ok_or(IndexError::ResourceLimited)?,
            )?,
            MemoryLifetime::Temporary,
        )?;
        let mut arena = Vec::new();
        arena
            .try_reserve_exact(ARENA_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if arena.capacity() > ARENA_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        let mut rows = Vec::new();
        rows.try_reserve_exact(BATCH)
            .map_err(|_error| IndexError::ResourceLimited)?;
        if rows.capacity() > BATCH {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            db,
            allocation,
            arena,
            rows,
            _arena_memory: arena_memory,
            _rows_memory: rows_memory,
            inserted: 0,
            poisoned: false,
        })
    }

    pub(super) fn push(
        &mut self,
        key: &[u8],
        doc: u16,
        position: u32,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.poisoned {
            return Err(IndexError::BackendViolation(
                "native text segment spill attempt was poisoned".into(),
            ));
        }
        let result = (|| {
            budget.checkpoint()?;
            let _canonical = crate::native::text_memory::decode_case_term_key(key)?;
            if key.len() > KEY_BYTES || usize::from(doc) >= super::wire::MAX_DOCS {
                return Err(IndexError::ResourceLimited);
            }
            if self.rows.len() == BATCH {
                self.flush(budget)?;
            }
            let start = self.arena.len();
            let end = start
                .checked_add(key.len())
                .ok_or(IndexError::ResourceLimited)?;
            if end > ARENA_BYTES {
                return Err(IndexError::ResourceLimited);
            }
            budget.charge(WorkUsage {
                bytes: width(
                    key.len()
                        .checked_add(size_of::<Pending>())
                        .ok_or(IndexError::ResourceLimited)?,
                )?,
                ..WorkUsage::default()
            })?;
            self.arena.extend_from_slice(key);
            self.rows.push(Pending {
                start: u32::try_from(start).map_err(|_error| IndexError::ResourceLimited)?,
                len: u16::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
                doc,
                position,
            });
            if self.rows.len() == BATCH {
                self.flush(budget)?;
            }
            Ok(())
        })();
        if result.is_err() {
            self.poisoned = true;
        }
        result
    }

    fn flush(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if self.poisoned {
            return Err(IndexError::BackendViolation(
                "native text segment spill attempt was poisoned".into(),
            ));
        }
        if self.rows.is_empty() {
            return Ok(());
        }
        let result = (|| {
            let next_inserted = self
                .inserted
                .checked_add(width(self.rows.len())?)
                .ok_or(IndexError::ResourceLimited)?;
            let mut demands = [0u64; BATCH];
            for (slot, row) in self.rows.iter().enumerate() {
                demands[slot] = u64::from(row.len)
                    .checked_add(64)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            let demands = demands
                .get(..self.rows.len())
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                writes: width(self.rows.len())?,
                bytes: demands.iter().try_fold(0u64, |sum, item| {
                    sum.checked_add(*item).ok_or(IndexError::ResourceLimited)
                })?,
                ..WorkUsage::default()
            })?;
            let scoped = self.db.scoped(budget)?;
            quanta_index_scratch_sqlite::reserve_for_write_batch(
                &scoped,
                self.allocation.as_ref(),
                demands,
                budget,
            )?;
            let mut sql_bytes = [0u8; MAX_INSERT_SQL];
            let remaining = (|| -> std::io::Result<usize> {
                let mut tail = sql_bytes.as_mut_slice();
                tail.write_all(INSERT_PREFIX.as_bytes())?;
                for slot in 0..self.rows.len() {
                    if slot != 0 {
                        tail.write_all(b",")?;
                    }
                    tail.write_all(INSERT_ROW.as_bytes())?;
                }
                Ok(tail.len())
            })()
            .map_err(|_error| {
                IndexError::BackendViolation("spill SQL packet exceeded bound".into())
            })?;
            let used = MAX_INSERT_SQL
                .checked_sub(remaining)
                .ok_or(IndexError::ResourceLimited)?;
            let query =
                std::str::from_utf8(sql_bytes.get(..used).ok_or(IndexError::ResourceLimited)?)
                    .map_err(|_error| {
                        IndexError::BackendViolation("spill SQL packet is not ASCII".into())
                    })?;
            let mut insert = scoped.prepare_cached(query).map_err(|error| sql(&error))?;
            insert.clear_bindings();
            let result = (|| {
                for (slot, row) in self.rows.iter().enumerate() {
                    budget.checkpoint()?;
                    let start =
                        usize::try_from(row.start).map_err(|_error| IndexError::ResourceLimited)?;
                    let end = start
                        .checked_add(usize::from(row.len))
                        .ok_or(IndexError::ResourceLimited)?;
                    let key = self
                        .arena
                        .get(start..end)
                        .ok_or(IndexError::ResourceLimited)?;
                    let first = slot
                        .checked_mul(3)
                        .and_then(|value| value.checked_add(1))
                        .ok_or(IndexError::ResourceLimited)?;
                    insert
                        .raw_bind_parameter(first, key)
                        .map_err(|error| sql(&error))?;
                    insert
                        .raw_bind_parameter(first + 1, i64::from(row.doc))
                        .map_err(|error| sql(&error))?;
                    insert
                        .raw_bind_parameter(first + 2, i64::from(row.position))
                        .map_err(|error| sql(&error))?;
                }
                budget.checkpoint()?;
                let changed = insert.raw_execute().map_err(|error| sql(&error))?;
                if changed != self.rows.len() {
                    return Err(IndexError::BackendViolation(
                        "native text segment spill insert count changed".into(),
                    ));
                }
                budget.checkpoint()
            })();
            insert.clear_bindings();
            result?;
            Ok(next_inserted)
        })();
        match result {
            Ok(inserted) => {
                self.inserted = inserted;
                self.arena.clear();
                self.rows.clear();
                Ok(())
            }
            Err(error) => {
                self.poisoned = true;
                Err(error)
            }
        }
    }

    /// Emit positions followed by their document fence, then finish each term.
    /// Copy a borrowed SQLite key once per term to survive the next cursor step.
    pub(super) fn visit_sorted(
        mut self,
        budget: &mut WorkBudget,
        mut visit: impl FnMut(SpillEvent<'_>, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        self.flush(budget)?;
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let scoped = self.db.scoped(budget)?;
        let mut statement = scoped
            .prepare_cached("SELECT term,doc,pos FROM tokens ORDER BY term,doc,pos")
            .map_err(|error| sql(&error))?;
        let mut rows = statement.query([]).map_err(|error| sql(&error))?;
        let mut prior = [0u8; KEY_BYTES];
        let mut prior_len = 0usize;
        let mut prior_doc = 0u16;
        let mut prior_position = 0u32;
        let mut have_prior = false;
        let mut visited = 0u64;
        while let Some(row) = rows.next().map_err(|error| sql(&error))? {
            budget.checkpoint()?;
            let key = quanta_index_scratch_sqlite::bounded_blob(
                row.get_ref(0).map_err(|error| sql(&error))?,
                KEY_BYTES,
            )?;
            let _canonical = crate::native::text_memory::decode_case_term_key(key)?;
            let raw_doc: i64 = row.get(1).map_err(|error| sql(&error))?;
            let raw_position: i64 = row.get(2).map_err(|error| sql(&error))?;
            let doc = u16::try_from(raw_doc).map_err(|_error| {
                IndexError::CorruptObject("native text segment spill docID width".into())
            })?;
            let position = u32::try_from(raw_position).map_err(|_error| {
                IndexError::CorruptObject("native text segment spill position width".into())
            })?;
            if usize::from(doc) >= super::wire::MAX_DOCS {
                return Err(IndexError::CorruptObject(
                    "native text segment spill docID exceeds segment".into(),
                ));
            }
            budget.charge(WorkUsage {
                reads: 1,
                bytes: width(
                    key.len()
                        .checked_add(16)
                        .ok_or(IndexError::ResourceLimited)?,
                )?,
                ..WorkUsage::default()
            })?;
            let prior_key = prior.get(..prior_len).ok_or(IndexError::ResourceLimited)?;
            let order = if have_prior {
                key.cmp(prior_key)
            } else {
                Ordering::Greater
            };
            if order == Ordering::Less
                || order == Ordering::Equal
                    && (doc < prior_doc || doc == prior_doc && position <= prior_position)
            {
                return Err(IndexError::CorruptObject(
                    "native text segment spill order or duplicate".into(),
                ));
            }
            if have_prior && (doc != prior_doc || order == Ordering::Greater) {
                visit(
                    SpillEvent::EndDocument {
                        key: prior_key,
                        doc: prior_doc,
                    },
                    budget,
                )?;
                if order == Ordering::Greater {
                    visit(SpillEvent::EndTerm, budget)?;
                }
            }
            if order == Ordering::Greater {
                budget.charge(WorkUsage {
                    bytes: width(key.len())?,
                    ..WorkUsage::default()
                })?;
                prior
                    .get_mut(..key.len())
                    .ok_or(IndexError::ResourceLimited)?
                    .copy_from_slice(key);
                prior_len = key.len();
            }
            prior_doc = doc;
            prior_position = position;
            have_prior = true;
            visit(SpillEvent::Position(position), budget)?;
            visited = visited.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        if visited != self.inserted {
            return Err(IndexError::CorruptObject(
                "native text segment spill row count changed".into(),
            ));
        }
        if have_prior {
            visit(
                SpillEvent::EndDocument {
                    key: prior.get(..prior_len).ok_or(IndexError::ResourceLimited)?,
                    doc: prior_doc,
                },
                budget,
            )?;
            visit(SpillEvent::EndTerm, budget)?;
        }
        budget.checkpoint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::WorkLimits;
    use std::time::{Duration, Instant};

    struct TestScratch;
    struct TestAllocation(tempfile::TempDir);

    impl ScratchAllocation for TestAllocation {
        fn path(&self) -> &std::path::Path {
            self.0.path()
        }

        fn max_bytes(&self) -> u64 {
            MAX_BYTES
        }

        fn reserved_bytes(&self) -> Result<u64, IndexError> {
            Ok(MAX_BYTES)
        }

        fn reserve_at_least(&self, bytes: u64, budget: &mut WorkBudget) -> Result<(), IndexError> {
            budget.checkpoint()?;
            if bytes > MAX_BYTES {
                return Err(IndexError::ResourceLimited);
            }
            Ok(())
        }
    }

    impl ScratchSpacePort for TestScratch {
        fn allocate(
            &self,
            _request: ScratchRequest,
            budget: &mut WorkBudget,
        ) -> Result<Box<dyn ScratchAllocation>, IndexError> {
            budget.checkpoint()?;
            let directory = tempfile::tempdir().map_err(|error| {
                IndexError::Storage(format!("text segment spill test scratch: {error}"))
            })?;
            Ok(Box::new(TestAllocation(directory)))
        }
    }

    #[test]
    fn counter_overflow_poisoned_before_sql_write() -> Result<(), IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: u64::MAX,
                writes: u64::MAX,
                bytes: u64::MAX,
                mutations: u64::MAX,
            },
            deadline,
        );
        let mut spill = SegmentSpill::new(&TestScratch, &mut budget)?;
        spill.inserted = u64::MAX;
        spill.arena.push(0);
        spill.rows.push(Pending {
            start: 0,
            len: 1,
            doc: 0,
            position: 0,
        });

        assert!(matches!(
            spill.flush(&mut budget),
            Err(IndexError::ResourceLimited)
        ));
        assert!(spill.poisoned);
        assert_eq!(spill.inserted, u64::MAX);
        assert_eq!(spill.rows.len(), 1);
        let rows: i64 = spill
            .db
            .scoped(&budget)?
            .query_row("SELECT COUNT(*) FROM tokens", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        assert_eq!(rows, 0);
        assert!(matches!(
            spill.flush(&mut budget),
            Err(IndexError::BackendViolation(_))
        ));
        Ok(())
    }
}
