//! Private bounded disk sort for one native literal preparation. The catalog
//! owns installed objects; this disposable database never publishes a root.
use quanta_index_core::index::{
    IndexError, ObjectRef, OrdinalMapEdit, ScratchAllocation, ScratchRequest, ScratchSpacePort,
    WorkBudget, WorkUsage, ports::MapEditCursor,
};
use quanta_index_scratch_sqlite::ScratchConnection;
#[cfg(test)]
use rusqlite::Connection;
use rusqlite::{OptionalExtension, params};

pub(super) struct LiteralEditSpool {
    db: ScratchConnection,
    allocation: Box<dyn ScratchAllocation>,
}

const SCRATCH_MAX_BYTES: u64 = 4_294_967_296;
const SCRATCH_INITIAL_BYTES: u64 = 256 * 1024;
const GRAM_AFTER_SQL: &str = "SELECT g FROM posting WHERE g>?1 GROUP BY g ORDER BY g LIMIT 64";
const POSTING_AFTER_SQL: &str = "SELECT k,p,n FROM posting WHERE g=?1 AND k>?2 ORDER BY k LIMIT 1";
const TOP_AFTER_SQL: &str = "SELECT k,p,n FROM edits WHERE surface=?1 AND k>?2 ORDER BY k LIMIT 1";

fn sql(error: &rusqlite::Error) -> IndexError {
    quanta_index_scratch_sqlite::storage_error(error, "literal scratch")
}
impl LiteralEditSpool {
    pub(super) fn new(
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        let allocation = scratch.allocate(
            ScratchRequest {
                initial_bytes: SCRATCH_INITIAL_BYTES,
                max_bytes: SCRATCH_MAX_BYTES,
            },
            budget,
        )?;
        if allocation.max_bytes() != SCRATCH_MAX_BYTES {
            return Err(IndexError::BackendViolation(
                "literal scratch allocation changed its envelope".into(),
            ));
        }
        let owned_db = ScratchConnection::open(
            &allocation.path().join("literal.sqlite"),
            allocation.as_ref(),
        )?;
        let db = owned_db.scoped()?;
        db.execute_batch(
            "PRAGMA page_size=4096; PRAGMA cache_size=-2048;
            PRAGMA mmap_size=0; PRAGMA temp_store=FILE; PRAGMA journal_mode=OFF;
            PRAGMA synchronous=OFF;",
        )
        .map_err(|error| sql(&error))?;
        quanta_index_scratch_sqlite::configure(&db, allocation.as_ref())?;
        quanta_index_scratch_sqlite::reserve_for_write(&db, allocation.as_ref(), 8192, budget)?;
        db.execute_batch(
            "
            CREATE TABLE edits(surface INTEGER NOT NULL,k BLOB NOT NULL,p BLOB,n BLOB,
                PRIMARY KEY(surface,k)) WITHOUT ROWID;
            CREATE TABLE posting(g BLOB NOT NULL,k BLOB NOT NULL,p BLOB,n BLOB,
                PRIMARY KEY(g,k)) WITHOUT ROWID; BEGIN IMMEDIATE;",
        )
        .map_err(|error| sql(&error))?;
        let page_size: u32 = db
            .query_row("PRAGMA page_size", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        let max_pages: u32 = db
            .query_row("PRAGMA max_page_count", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        if page_size != 4096
            || u64::from(max_pages)
                .checked_mul(u64::from(page_size))
                .ok_or(IndexError::ResourceLimited)?
                > allocation.reserved_bytes()?
        {
            return Err(IndexError::Storage(
                "literal scratch page limit was not installed".into(),
            ));
        }
        drop(db);
        Ok(Self {
            db: owned_db,
            allocation,
        })
    }

    pub(super) fn edit(
        &self,
        surface: u8,
        key: &[u8],
        expected: Option<ObjectRef>,
        next: Option<ObjectRef>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let incoming = u64::try_from(key.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(128)
            .ok_or(IndexError::ResourceLimited)?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.db.scoped()?,
            self.allocation.as_ref(),
            incoming,
            budget,
        )?;
        let inserted = self
            .db
            .scoped()?
            .execute(
                "INSERT INTO edits(surface,k,p,n) VALUES (?1,?2,?3,?4)",
                params![
                    surface,
                    key,
                    expected
                        .map(Into::into)
                        .as_ref()
                        .map(quanta_index_scratch_sqlite::map_value)
                        .transpose()?,
                    next.map(Into::into)
                        .as_ref()
                        .map(quanta_index_scratch_sqlite::map_value)
                        .transpose()?
                ],
            )
            .map_err(|error| sql(&error))?;
        if inserted != 1 {
            return Err(IndexError::BackendViolation(
                "edit spool insert affected an unexpected row count".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn posting(
        &self,
        gram: [u8; 3],
        key: &[u8],
        expected: Option<ObjectRef>,
        next: Option<ObjectRef>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if expected.is_some() == next.is_some() {
            return Err(IndexError::InvalidInput(
                "posting delta must add or remove membership".into(),
            ));
        }
        budget.charge(WorkUsage {
            writes: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let incoming = u64::try_from(key.len())
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(3 + 128)
            .ok_or(IndexError::ResourceLimited)?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.db.scoped()?,
            self.allocation.as_ref(),
            incoming,
            budget,
        )?;
        let inserted = self
            .db
            .scoped()?
            .execute(
                "INSERT INTO posting(g,k,p,n) VALUES (?1,?2,?3,?4)",
                params![
                    gram.as_slice(),
                    key,
                    expected
                        .map(Into::into)
                        .as_ref()
                        .map(quanta_index_scratch_sqlite::map_value)
                        .transpose()?,
                    next.map(Into::into)
                        .as_ref()
                        .map(quanta_index_scratch_sqlite::map_value)
                        .transpose()?
                ],
            )
            .map_err(|error| sql(&error))?;
        if inserted != 1 {
            return Err(IndexError::BackendViolation(
                "posting spool insert affected an unexpected row count".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn gram_page(
        &self,
        after: Option<[u8; 3]>,
        budget: &mut WorkBudget,
    ) -> Result<super::text_memory::PreparedRows<[u8; 3]>, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let query = if after.is_some() {
            GRAM_AFTER_SQL
        } else {
            "SELECT g FROM posting GROUP BY g ORDER BY g LIMIT 64"
        };
        let io_scope = self.db.scoped()?;
        let mut statement = io_scope.prepare(query).map_err(|error| sql(&error))?;
        let mut rows = if let Some(after) = after {
            statement
                .query(params![after.as_slice()])
                .map_err(|error| sql(&error))?
        } else {
            statement.query([]).map_err(|error| sql(&error))?
        };
        let mut grams = super::text_memory::PreparedRows::new(64)?;
        while let Some(row) = rows.next().map_err(|error| sql(&error))? {
            grams.push(
                quanta_index_scratch_sqlite::fixed_blob(
                    row.get_ref(0).map_err(|error| sql(&error))?,
                )?,
                budget,
            )?;
        }
        budget.checkpoint()?;
        Ok(grams)
    }

    pub(super) fn cardinality(
        &self,
        gram: [u8; 3],
        prior: u64,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let (add, remove): (i64, i64) = self.db.scoped()?.query_row(
            "SELECT COALESCE(SUM(n IS NOT NULL),0),COALESCE(SUM(p IS NOT NULL),0) FROM posting WHERE g=?1",
            params![gram.as_slice()], |row| Ok((row.get(0)?,row.get(1)?))).map_err(|error| sql(&error))?;
        let add = u64::try_from(add)
            .map_err(|_error| IndexError::CorruptObject("negative posting additions".into()))?;
        let remove = u64::try_from(remove)
            .map_err(|_error| IndexError::CorruptObject("negative posting removals".into()))?;
        prior
            .checked_sub(remove)
            .and_then(|count| count.checked_add(add))
            .ok_or_else(|| {
                IndexError::CorruptObject("posting cardinality overflow or underflow".into())
            })
    }

    pub(super) fn posting_cursor<'a>(&'a self, grams: &'a [[u8; 3]]) -> PostingCursor<'a> {
        PostingCursor {
            spool: self,
            grams,
            slot: 0,
            after: None,
        }
    }

    pub(super) fn top_cursor(&self) -> TopCursor<'_> {
        TopCursor {
            spool: self,
            surface: 0,
            after: None,
        }
    }
}

pub(super) struct PostingCursor<'a> {
    spool: &'a LiteralEditSpool,
    grams: &'a [[u8; 3]],
    slot: usize,
    after: Option<OrdinalMapEdit>,
}

impl MapEditCursor for PostingCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        while let Some(gram) = self.grams.get(self.slot) {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 4096,
                ..WorkUsage::default()
            })?;
            let root = u32::try_from(self.slot).map_err(|_error| IndexError::ResourceLimited)?;
            let row = if let Some(after) = self
                .after
                .as_ref()
                .map(|position| position.edit.key.as_slice())
            {
                self.spool.db.scoped()?.query_row(
                    POSTING_AFTER_SQL,
                    params![gram.as_slice(), after],
                    |row| {
                        Ok(quanta_index_scratch_sqlite::read_map_edit(
                            row, 32, root, budget,
                        ))
                    },
                )
            } else {
                self.spool.db.scoped()?.query_row(
                    "SELECT k,p,n FROM posting WHERE g=?1 ORDER BY k LIMIT 1",
                    params![gram.as_slice()],
                    |row| {
                        Ok(quanta_index_scratch_sqlite::read_map_edit(
                            row, 32, root, budget,
                        ))
                    },
                )
            }
            .optional()
            .map_err(|error| sql(&error))?;
            if let Some(edit) = row.transpose()? {
                self.after = Some(edit.clone());
                return Ok(Some(edit));
            }
            self.slot = self
                .slot
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            self.after = None;
        }
        Ok(None)
    }
}

pub(super) struct TopCursor<'a> {
    spool: &'a LiteralEditSpool,
    surface: u8,
    after: Option<OrdinalMapEdit>,
}

impl MapEditCursor for TopCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        while self.surface < 2 {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 4096,
                ..WorkUsage::default()
            })?;
            let max_key_bytes = if self.surface == 0 { 3 } else { 32 };
            let row = if let Some(after) = self
                .after
                .as_ref()
                .map(|position| position.edit.key.as_slice())
            {
                self.spool.db.scoped()?.query_row(
                    TOP_AFTER_SQL,
                    params![self.surface, after],
                    |row| {
                        Ok(quanta_index_scratch_sqlite::read_map_edit(
                            row,
                            max_key_bytes,
                            u32::from(self.surface),
                            budget,
                        ))
                    },
                )
            } else {
                self.spool.db.scoped()?.query_row(
                    "SELECT k,p,n FROM edits WHERE surface=?1 ORDER BY k LIMIT 1",
                    params![self.surface],
                    |row| {
                        Ok(quanta_index_scratch_sqlite::read_map_edit(
                            row,
                            max_key_bytes,
                            u32::from(self.surface),
                            budget,
                        ))
                    },
                )
            }
            .optional()
            .map_err(|error| sql(&error))?;
            if let Some(edit) = row.transpose()? {
                self.after = Some(edit.clone());
                return Ok(Some(edit));
            }
            self.surface = self
                .surface
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            self.after = None;
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::Path,
        time::{Duration, Instant},
    };

    use rusqlite::{StatementStatus, ToSql};

    use super::*;
    use quanta_index_core::index::{Digest, MapEdit, ObjectRef, WorkLimits};

    struct TestScratch;
    struct TestAllocation(tempfile::TempDir, u64);

    impl ScratchAllocation for TestAllocation {
        fn path(&self) -> &Path {
            self.0.path()
        }

        fn max_bytes(&self) -> u64 {
            self.1
        }

        fn reserved_bytes(&self) -> Result<u64, IndexError> {
            // This fixture admits its whole envelope; storage tests cover growth.
            Ok(self.1)
        }

        fn reserve_at_least(&self, bytes: u64, budget: &mut WorkBudget) -> Result<(), IndexError> {
            budget.checkpoint()?;
            if bytes > self.1 {
                return Err(IndexError::ResourceLimited);
            }
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
            Ok(Box::new(TestAllocation(directory, request.max_bytes)))
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

    fn probe_keyset(
        db: &Connection,
        query: &str,
        args: &[&dyn ToSql],
    ) -> Result<(Vec<Vec<u8>>, i32), IndexError> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition)))); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!($($message)+))); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!()))); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+)))); }
            }};
        }

        let mut plan = db
            .prepare(&format!("EXPLAIN QUERY PLAN {query}"))
            .map_err(|error| sql(&error))?;
        let mut plan_rows = plan.query(args).map_err(|error| sql(&error))?;
        let mut indexed = false;
        while let Some(row) = plan_rows.next().map_err(|error| sql(&error))? {
            let detail: String = row.get(3).map_err(|error| sql(&error))?;
            check!(!detail.contains("USE TEMP B-TREE"), "{detail}");
            indexed |= detail.starts_with("SEARCH ");
        }
        check!(indexed, "keyset query did not use an index seek: {query}");

        let mut statement = db.prepare(query).map_err(|error| sql(&error))?;
        let mut keys = Vec::new();
        {
            let mut rows = statement.query(args).map_err(|error| sql(&error))?;
            while let Some(row) = rows.next().map_err(|error| sql(&error))? {
                keys.push(row.get(0).map_err(|error| sql(&error))?);
            }
        }
        check_eq!(statement.get_status(StatementStatus::Sort), 0);
        Ok((keys, statement.get_status(StatementStatus::VmStep)))
    }

    #[test]
    fn late_keyset_seeks_skip_existing_prefix_without_sqlite_sort() -> Result<(), IndexError> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition)))); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!($($message)+))); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!()))); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+)))); }
            }};
        }

        let mut work = budget()?;
        let spool = LiteralEditSpool::new(&TestScratch, &mut work)?;
        let target = [255_u8, 255, 0];
        {
            let io_scope = spool.db.scoped()?;
            let mut posting = io_scope
                .prepare("INSERT INTO posting(g,k) VALUES(?1,?2)")
                .map_err(|error| sql(&error))?;
            let io_scope = spool.db.scoped()?;
            let mut top = io_scope
                .prepare("INSERT INTO edits(surface,k) VALUES(0,?1)")
                .map_err(|error| sql(&error))?;
            for number in 0_u16..8192 {
                let key = number.to_be_bytes();
                let gram = [key[0], key[1], 0];
                let _inserted = posting
                    .execute(params![gram, [0_u8]])
                    .map_err(|error| sql(&error))?;
                let _inserted = posting
                    .execute(params![target, key])
                    .map_err(|error| sql(&error))?;
                let _inserted = top.execute(params![key]).map_err(|error| sql(&error))?;
            }
        }

        let early = 9_u16.to_be_bytes();
        let late = 4096_u16.to_be_bytes();
        let early_gram = [early[0], early[1], 0];
        let late_gram = [late[0], late[1], 0];
        let early_args: [&dyn ToSql; 1] = [&early_gram];
        let late_args: [&dyn ToSql; 1] = [&late_gram];
        let (early_keys, early_steps) =
            probe_keyset(&*spool.db.scoped()?, GRAM_AFTER_SQL, &early_args)?;
        let (late_keys, late_steps) =
            probe_keyset(&*spool.db.scoped()?, GRAM_AFTER_SQL, &late_args)?;
        let expected_early: Vec<Vec<u8>> = (10_u16..74)
            .map(|number| {
                let key = number.to_be_bytes();
                vec![key[0], key[1], 0]
            })
            .collect();
        let expected_late_grams: Vec<[u8; 3]> = (4097_u16..4161)
            .map(|number| {
                let key = number.to_be_bytes();
                [key[0], key[1], 0]
            })
            .collect();
        let expected_late: Vec<Vec<u8>> = expected_late_grams
            .iter()
            .map(|gram| gram.to_vec())
            .collect();
        check_eq!(early_keys, expected_early);
        check_eq!(late_keys, expected_late);
        check!(late_steps <= early_steps.saturating_add(128));
        let late_page = spool.gram_page(Some(late_gram), &mut work)?;
        check_eq!(late_page.as_slice(), expected_late_grams.as_slice());

        let surface = 0_u8;
        for query in [POSTING_AFTER_SQL, TOP_AFTER_SQL] {
            let (early_args, late_args): ([&dyn ToSql; 2], [&dyn ToSql; 2]) =
                if query == POSTING_AFTER_SQL {
                    ([&target, &early], [&target, &late])
                } else {
                    ([&surface, &early], [&surface, &late])
                };
            let (early_keys, early_steps) = probe_keyset(&*spool.db.scoped()?, query, &early_args)?;
            let (late_keys, late_steps) = probe_keyset(&*spool.db.scoped()?, query, &late_args)?;
            check_eq!(early_keys, vec![10_u16.to_be_bytes().to_vec()]);
            check_eq!(late_keys, vec![4097_u16.to_be_bytes().to_vec()]);
            check!(late_steps <= early_steps.saturating_add(128));
        }
        let target_grams = [target];
        let mut posting_cursor = spool.posting_cursor(&target_grams);
        posting_cursor.after = Some(OrdinalMapEdit::copy_object_key(
            0, &late, None, None, &work,
        )?);
        check_eq!(
            posting_cursor
                .next_edit(&mut work)?
                .ok_or_else(|| IndexError::BackendViolation("missing late posting".into()))?
                .edit
                .key,
            4097_u16.to_be_bytes().to_vec()
        );
        let mut top_cursor = spool.top_cursor();
        top_cursor.after = Some(OrdinalMapEdit::copy_object_key(
            0, &late, None, None, &work,
        )?);
        check_eq!(
            top_cursor
                .next_edit(&mut work)?
                .ok_or_else(|| IndexError::BackendViolation("missing late top edit".into()))?
                .edit
                .key,
            4097_u16.to_be_bytes().to_vec()
        );
        Ok(())
    }

    #[test]
    fn posting_keyset_pages_match_independent_sorted_delta() -> Result<(), IndexError> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition)))); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!($($message)+))); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!()))); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+)))); }
            }};
        }

        let mut budget = budget()?;
        let spool = LiteralEditSpool::new(&TestScratch, &mut budget)?;
        let present = ObjectRef(Digest([7; 32]));
        let mut expected = BTreeMap::new();
        // Insert in reverse order; the public cursor must produce canonical
        // gram and owner order across both 64-gram page boundaries.
        for value in (0_u16..130).rev() {
            let [high, low] = value.to_be_bytes();
            let gram = [high, low, 9];
            for owner in (0_u8..3).rev() {
                let edit = MapEdit {
                    key: vec![owner],
                    expected: None,
                    next: Some((present).into()),
                };
                spool.posting(
                    gram,
                    &edit.key,
                    edit.expected
                        .map(quanta_index_core::index::MapValue::object_ref)
                        .transpose()?,
                    edit.next
                        .map(quanta_index_core::index::MapValue::object_ref)
                        .transpose()?,
                    &mut budget,
                )?;
                check!(expected.insert((gram, edit.key.clone()), edit).is_none());
            }
        }
        let mut after = None;
        let mut observed = BTreeMap::new();
        let mut page_lengths = Vec::new();
        loop {
            let grams = spool.gram_page(after, &mut budget)?;
            if grams.len() == 0 {
                break;
            }
            page_lengths.push(grams.len());
            after = grams.as_slice().last().copied();
            for gram in grams.as_slice() {
                check_eq!(spool.cardinality(*gram, 0, &mut budget)?, 3);
            }
            let mut cursor = spool.posting_cursor(grams.as_slice());
            while let Some(item) = cursor.next_edit(&mut budget)? {
                let ordinal =
                    usize::try_from(item.root).map_err(|_error| IndexError::ResourceLimited)?;
                let gram = *grams
                    .as_slice()
                    .get(ordinal)
                    .ok_or_else(|| IndexError::BackendViolation("scratch test ordinal".into()))?;
                let key = (gram, item.edit.key.clone());
                check!(observed.insert(key, item.edit.clone()).is_none());
            }
        }
        check_eq!(page_lengths, vec![64, 64, 2]);
        check_eq!(observed, expected);
        Ok(())
    }

    #[test]
    fn duplicate_and_cardinality_underflow_are_errors() -> Result<(), IndexError> {
        macro_rules! check {
            ($condition:expr $(,)?) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: {}", file!(), line!(), stringify!($condition)))); }
            }};
            ($condition:expr, $($message:tt)+) => {{
                if !$condition { return Err(IndexError::BackendViolation(format!($($message)+))); }
            }};
        }
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!()))); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+)))); }
            }};
        }

        let mut budget = budget()?;
        let spool = LiteralEditSpool::new(&TestScratch, &mut budget)?;
        let edit = MapEdit {
            key: vec![1],
            expected: Some((ObjectRef(Digest([8; 32]))).into()),
            next: None,
        };
        spool.posting(
            *b"abc",
            &edit.key,
            edit.expected
                .map(quanta_index_core::index::MapValue::object_ref)
                .transpose()?,
            edit.next
                .map(quanta_index_core::index::MapValue::object_ref)
                .transpose()?,
            &mut budget,
        )?;
        check!(
            spool
                .posting(
                    *b"abc",
                    &edit.key,
                    edit.expected
                        .map(quanta_index_core::index::MapValue::object_ref)
                        .transpose()?,
                    edit.next
                        .map(quanta_index_core::index::MapValue::object_ref)
                        .transpose()?,
                    &mut budget
                )
                .is_err()
        );
        check!(matches!(
            spool.cardinality(*b"abc", 0, &mut budget),
            Err(IndexError::CorruptObject(_))
        ));
        check_eq!(spool.cardinality(*b"abc", 1, &mut budget)?, 0);
        Ok(())
    }

    #[test]
    fn malformed_literal_rows_refuse_without_advancing_each_cursor() -> Result<(), IndexError> {
        for (posting, statement) in [
            (
                true,
                "INSERT INTO posting VALUES(x'000102',x'01',NULL,zeroblob(31))",
            ),
            (
                true,
                "INSERT INTO posting VALUES(x'000102',zeroblob(33),NULL,zeroblob(32))",
            ),
            (
                false,
                "INSERT INTO edits VALUES(0,x'000102',zeroblob(1048576),NULL)",
            ),
            (
                false,
                "INSERT INTO edits VALUES(0,zeroblob(4),NULL,zeroblob(32))",
            ),
            (false, "INSERT INTO edits VALUES(1,x'01',NULL,'not-a-blob')"),
            (
                false,
                "INSERT INTO edits VALUES(1,zeroblob(33),NULL,zeroblob(32))",
            ),
        ] {
            let mut work = budget()?;
            let spool = LiteralEditSpool::new(&TestScratch, &mut work)?;
            spool
                .db
                .scoped()?
                .execute_batch(statement)
                .map_err(|error| sql(&error))?;
            let mut cursor: Box<dyn MapEditCursor + '_> = if posting {
                Box::new(spool.posting_cursor(&[[0, 1, 2]]))
            } else {
                Box::new(spool.top_cursor())
            };
            for _retry in 0..2 {
                if !matches!(
                    cursor.next_edit(&mut work),
                    Err(IndexError::CorruptObject(_))
                ) {
                    return Err(IndexError::BackendViolation(format!(
                        "malformed literal row escaped refusal: {statement}"
                    )));
                }
            }
        }
        Ok(())
    }

    #[test]
    fn gram_page_refuses_unfunded_array_and_preserves_original_control() -> Result<(), IndexError> {
        use quanta_index_core::index::{MemoryLease, MemoryLifetime, MemoryScope, MemoryScopePort};
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        struct Refuse(Arc<AtomicUsize>);
        impl MemoryScopePort for Refuse {
            fn reserve(
                &self,
                bytes: u64,
                lifetime: MemoryLifetime,
            ) -> Result<MemoryLease, IndexError> {
                if bytes != 24 || lifetime != MemoryLifetime::Temporary {
                    return Err(IndexError::BackendViolation(
                        "gram page requested unexpected backing".into(),
                    ));
                }
                let _previous = self.0.fetch_add(1, Ordering::Relaxed);
                Err(IndexError::ResourceLimited)
            }
        }
        let mut work = budget()?;
        let spool = LiteralEditSpool::new(&TestScratch, &mut work)?;
        let calls = Arc::new(AtomicUsize::new(0));
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Refuse(Arc::clone(&calls))))));
        if spool.gram_page(None, &mut work)?.len() != 0 || calls.load(Ordering::Relaxed) != 0 {
            return Err(IndexError::BackendViolation(
                "empty gram page allocated backing".into(),
            ));
        }
        spool
            .db
            .scoped()?
            .execute_batch("INSERT INTO posting VALUES(x'000102',x'01',NULL,zeroblob(32))")
            .map_err(|error| sql(&error))?;
        if !matches!(
            spool.gram_page(None, &mut work),
            Err(IndexError::ResourceLimited)
        ) || calls.load(Ordering::Relaxed) != 1
        {
            return Err(IndexError::BackendViolation(
                "gram page bypassed backing admission".into(),
            ));
        }
        work.request().cancel_handle().cancel();
        if !matches!(spool.gram_page(None, &mut work), Err(IndexError::Cancelled))
            || calls.load(Ordering::Relaxed) != 1
        {
            return Err(IndexError::BackendViolation(
                "gram page replaced original control".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn gram_page_refuses_wrong_width_without_an_owned_blob_copy() -> Result<(), IndexError> {
        let mut work = budget()?;
        let spool = LiteralEditSpool::new(&TestScratch, &mut work)?;
        spool
            .db
            .scoped()?
            .execute_batch("INSERT INTO posting VALUES(zeroblob(4),x'01',NULL,zeroblob(32))")
            .map_err(|error| sql(&error))?;
        if !matches!(
            spool.gram_page(None, &mut work),
            Err(IndexError::CorruptObject(_))
        ) {
            return Err(IndexError::BackendViolation(
                "wrong-width gram escaped refusal".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn scratch_admission_refusal_and_engine_full_have_distinct_outcomes() -> Result<(), IndexError>
    {
        macro_rules! check_eq {
            ($actual:expr, $expected:expr $(,)?) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("test oracle failed at {}:{}: actual={left:?}, expected={right:?}", file!(), line!()))); }
            }};
            ($actual:expr, $expected:expr, $($message:tt)+) => {{
                let (left, right) = (&$actual, &$expected);
                if *left != *right { return Err(IndexError::BackendViolation(format!("{}; actual={left:?}, expected={right:?}", format!($($message)+)))); }
            }};
        }

        let mut budget = budget()?;
        let spool = LiteralEditSpool::new(&TestScratch, &mut budget)?;
        let refused = quanta_index_scratch_sqlite::reserve_for_write(
            &*spool.db.scoped()?,
            spool.allocation.as_ref(),
            SCRATCH_MAX_BYTES,
            &mut budget,
        );
        check_eq!(refused, Err(IndexError::ResourceLimited));
        let untouched: i64 = spool
            .db
            .scoped()?
            .query_row("SELECT count(*) FROM edits", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        check_eq!(untouched, 0);
        let _limit: u32 = spool
            .db
            .scoped()?
            .query_row("PRAGMA max_page_count=8", [], |row| row.get(0))
            .map_err(|error| sql(&error))?;
        let failed = spool.db.scoped()?.execute(
            "INSERT INTO edits(surface,k,n) VALUES(0,x'01',zeroblob(1048576))",
            [],
        );
        let Err(error) = failed else {
            return Err(IndexError::BackendViolation(
                "literal scratch main-page cap accepted an oversized write".into(),
            ));
        };
        if !matches!(
            &error,
            rusqlite::Error::SqliteFailure(failure, _)
                if failure.code == rusqlite::ErrorCode::DiskFull
        ) || !matches!(sql(&error), IndexError::Storage(_))
        {
            return Err(IndexError::BackendViolation(format!(
                "literal scratch main-page cap did not preserve its FULL storage failure: {error:?}"
            )));
        }
        Ok(())
    }
}
