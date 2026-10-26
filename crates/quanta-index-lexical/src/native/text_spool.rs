//! Bounded disposable sort for native token preparation. Only immutable map
//! roots published through the catalog can become search authority.

use quanta_index_scratch_sqlite::{MapEditPage as EditPage, read_map_edit_budgeted as read_edit};

use super::text_memory::PreparedKey;
use quanta_index_core::index::{
    IndexError, MapEdit, ObjectRef, OrdinalMapEdit, ScratchAllocation, ScratchRequest,
    ScratchSpacePort, WorkBudget, WorkUsage, ports::MapEditCursor,
};
use quanta_index_scratch_sqlite::ScratchConnection;
use rusqlite::{Connection, OptionalExtension as _, params};

pub(super) const MAX_TERM_KEY_BYTES: u64 = 1_024;
pub(super) const SCRATCH_MAX_BYTES: u64 = 4_294_967_296;
const SCRATCH_INITIAL_BYTES: u64 = 256 * 1024;
const PAGE: usize = 64;
const INVENTORY_FIRST_SQL: &str = "SELECT k,p,n,root FROM inventory ORDER BY root,k LIMIT 64";
const INVENTORY_NEXT_SQL: &str =
    "SELECT k,p,n,root FROM inventory WHERE (root,k)>(?1,?2) ORDER BY root,k LIMIT 64";
const INVENTORY_ROOT_FIRST_SQL: &str =
    "SELECT k,p,n FROM inventory WHERE root=?1 AND k>=?2 ORDER BY k LIMIT 64";
const INVENTORY_ROOT_NEXT_SQL: &str =
    "SELECT k,p,n FROM inventory WHERE root=?1 AND k>?2 ORDER BY k LIMIT 64";
const POSTING_ROOT_FIRST_SQL: &str =
    "SELECT k,p,n FROM posting WHERE term=?1 AND k>=?2 ORDER BY k LIMIT 64";
const POSTING_ROOT_NEXT_SQL: &str =
    "SELECT k,p,n FROM posting WHERE term=?1 AND k>?2 ORDER BY k LIMIT 64";
const TOP_NEXT_SQL: &str = "SELECT k,p,n FROM top WHERE surface=?1 AND k>?2 ORDER BY k LIMIT 64";
const TOKEN_TERM_PAGE_SQL: &str = "WITH RECURSIVE next(term,n) AS (
       SELECT (SELECT term FROM tokens WHERE term>?1 ORDER BY term LIMIT 1), 1
       UNION ALL
       SELECT (SELECT term FROM tokens WHERE term>next.term ORDER BY term LIMIT 1), n+1
       FROM next WHERE term IS NOT NULL AND n<64
     ) SELECT term FROM next WHERE term IS NOT NULL ORDER BY n";
const POSTING_TERM_PAGE_SQL: &str = "WITH RECURSIVE next(term,n) AS (
       SELECT (SELECT term FROM posting WHERE term>?1 ORDER BY term LIMIT 1), 1
       UNION ALL
       SELECT (SELECT term FROM posting WHERE term>next.term ORDER BY term LIMIT 1), n+1
       FROM next WHERE term IS NOT NULL AND n<64
     ) SELECT term FROM next WHERE term IS NOT NULL ORDER BY n";

pub(super) struct TextSpool {
    db: ScratchConnection,
    allocation: Box<dyn ScratchAllocation>,
}

fn sql(error: &rusqlite::Error) -> IndexError {
    quanta_index_scratch_sqlite::storage_error(error, "text scratch")
}

fn map_field_reservation(count: u64, overhead: u64) -> Result<u64, IndexError> {
    u64::try_from(quanta_index_core::index::MAX_MAP_VALUE_WIRE_BYTES)
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(count)
        .and_then(|bytes| bytes.checked_add(overhead))
        .ok_or(IndexError::ResourceLimited)
}

fn reference_bytes(
    reference: &ObjectRef,
) -> Result<quanta_index_scratch_sqlite::SqlMapValue, IndexError> {
    quanta_index_scratch_sqlite::map_value(&quanta_index_core::index::MapValue::Object(*reference))
}

fn check_batch_width(rows: usize) -> Result<(), IndexError> {
    if rows == 0 || rows > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
        return Err(IndexError::ResourceLimited);
    }
    Ok(())
}

struct BatchDemands {
    bytes: [u64; quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS],
    len: usize,
}

/// The bounded page array and every copied SQL key have independent custody.
#[derive(Debug)]
pub(super) struct TermPage {
    keys: Vec<PreparedKey>,
    memory: Option<quanta_index_core::index::MemoryLease>,
}

impl TermPage {
    pub(super) fn new() -> Self {
        Self {
            keys: Vec::new(),
            memory: None,
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8], budget: &WorkBudget) -> Result<(), IndexError> {
        if self.keys.len() == PAGE {
            return Err(IndexError::ResourceLimited);
        }
        if self.keys.capacity() == 0 {
            let bytes = PAGE
                .checked_mul(std::mem::size_of::<PreparedKey>())
                .ok_or(IndexError::ResourceLimited)?;
            let memory = budget.reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                quanta_index_core::index::MemoryLifetime::Temporary,
            )?;
            self.keys
                .try_reserve_exact(PAGE)
                .map_err(|_error| IndexError::ResourceLimited)?;
            self.memory = memory;
        }
        self.keys.push(PreparedKey::copy(bytes, budget)?);
        Ok(())
    }
}

impl std::ops::Deref for TermPage {
    type Target = [PreparedKey];
    fn deref(&self) -> &Self::Target {
        &self.keys
    }
}

pub(super) struct TermPageIter {
    keys: std::vec::IntoIter<PreparedKey>,
    _memory: Option<quanta_index_core::index::MemoryLease>,
}

impl Iterator for TermPageIter {
    type Item = PreparedKey;
    fn next(&mut self) -> Option<Self::Item> {
        self.keys.next()
    }
}

impl IntoIterator for TermPage {
    type Item = PreparedKey;
    type IntoIter = TermPageIter;
    fn into_iter(self) -> Self::IntoIter {
        TermPageIter {
            keys: self.keys.into_iter(),
            _memory: self.memory,
        }
    }
}

impl BatchDemands {
    fn new<T>(
        rows: &[T],
        mut demand: impl FnMut(&T) -> Result<u64, IndexError>,
    ) -> Result<Self, IndexError> {
        check_batch_width(rows.len())?;
        let mut bytes = [0; quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];
        for (slot, row) in bytes.iter_mut().zip(rows) {
            *slot = demand(row)?;
        }
        Ok(Self {
            bytes,
            len: rows.len(),
        })
    }

    fn as_slice(&self) -> Result<&[u64], IndexError> {
        self.bytes
            .get(..self.len)
            .ok_or(IndexError::ResourceLimited)
    }
}

fn term_page(
    db: &Connection,
    statement_sql: &str,
    after: Option<&[u8]>,
    budget: &mut WorkBudget,
) -> Result<TermPage, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let mut keys = TermPage::new();
    let mut statement = db
        .prepare_cached(statement_sql)
        .map_err(|error| sql(&error))?;
    let mut rows = statement
        .query(params![after.unwrap_or_default()])
        .map_err(|error| sql(&error))?;
    while let Some(row) = rows.next().map_err(|error| sql(&error))? {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        keys.push(
            quanta_index_scratch_sqlite::bounded_blob(
                row.get_ref(0).map_err(|error| sql(&error))?,
                usize::try_from(MAX_TERM_KEY_BYTES)
                    .map_err(|_error| IndexError::ResourceLimited)?,
            )?,
            budget,
        )?;
    }
    Ok(keys)
}

impl TextSpool {
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
                "text scratch allocation changed its envelope".into(),
            ));
        }
        let owned_db =
            ScratchConnection::open(&allocation.path().join("text.sqlite"), allocation.as_ref())?;
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
             CREATE TABLE tokens(term BLOB NOT NULL,pos INTEGER NOT NULL,
                 PRIMARY KEY(term,pos)) WITHOUT ROWID;
             CREATE TABLE inventory(root INTEGER NOT NULL,k BLOB NOT NULL,p BLOB,n BLOB,
                 PRIMARY KEY(root,k)) WITHOUT ROWID;
             CREATE TABLE posting(term BLOB NOT NULL,k BLOB NOT NULL,p BLOB,n BLOB,
                 PRIMARY KEY(term,k)) WITHOUT ROWID;
             CREATE TABLE top(surface INTEGER NOT NULL,k BLOB NOT NULL,p BLOB,n BLOB,
                 PRIMARY KEY(surface,k)) WITHOUT ROWID;
             CREATE INDEX top_confirmed_posting ON top(n) WHERE surface=0 AND n IS NOT NULL;
             BEGIN IMMEDIATE;",
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
                "text scratch page limit was not installed".into(),
            ));
        }
        drop(db);
        Ok(Self {
            db: owned_db,
            allocation,
        })
    }

    /// Only one changed owner's token rows are retained at a time.
    pub(super) fn reset_tokens(&self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            ..WorkUsage::default()
        })?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.db.scoped()?,
            self.allocation.as_ref(),
            4096,
            budget,
        )?;
        self.db
            .scoped()?
            .execute_batch("DELETE FROM tokens;")
            .map_err(|error| sql(&error))
    }

    pub(super) fn reset_inventories(&self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            ..WorkUsage::default()
        })?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.db.scoped()?,
            self.allocation.as_ref(),
            4096,
            budget,
        )?;
        self.db
            .scoped()?
            .execute_batch("DELETE FROM inventory;")
            .map_err(|error| sql(&error))
    }

    pub(super) fn token_batch(
        &self,
        batch: &super::text::TokenBatch,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let rows = batch.rows();
        let demands = BatchDemands::new(rows, |row| {
            u64::try_from(batch.term(row)?.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(64)
                .ok_or(IndexError::ResourceLimited)
        })?;
        let demands = demands.as_slice()?;
        budget.charge(WorkUsage {
            writes: u64::try_from(rows.len()).map_err(|_error| IndexError::ResourceLimited)?,
            bytes: demands.iter().try_fold(0u64, |sum, row| {
                sum.checked_add(*row).ok_or(IndexError::ResourceLimited)
            })?,
            ..WorkUsage::default()
        })?;
        quanta_index_scratch_sqlite::reserve_for_write_batch(
            &*self.db.scoped()?,
            self.allocation.as_ref(),
            demands,
            budget,
        )?;
        let io_scope = self.db.scoped()?;
        let mut insert = io_scope
            .prepare_cached("INSERT INTO tokens(term,pos) VALUES(?1,?2)")
            .map_err(|error| sql(&error))?;
        for row in rows {
            budget.checkpoint()?;
            if insert
                .execute(params![batch.term(row)?, row.position()])
                .map_err(|error| sql(&error))?
                != 1
            {
                return Err(IndexError::BackendViolation(
                    "token insertion affected an unexpected row count".into(),
                ));
            }
        }
        Ok(())
    }

    /// Index keyset traversal needs no corpus-sized DISTINCT materialization.
    pub(super) fn token_term_page(
        &self,
        after: Option<&[u8]>,
        budget: &mut WorkBudget,
    ) -> Result<TermPage, IndexError> {
        term_page(&*self.db.scoped()?, TOKEN_TERM_PAGE_SQL, after, budget)
    }

    pub(super) fn positions(
        &self,
        term: &[u8],
        limit: usize,
        budget: &mut WorkBudget,
    ) -> Result<super::text::PreparedPositions, IndexError> {
        let mut result = super::text::PreparedPositions::new(limit)?;
        let io_scope = self.db.scoped()?;
        let mut statement = io_scope
            .prepare_cached("SELECT pos FROM tokens WHERE term=?1 ORDER BY pos")
            .map_err(|error| sql(&error))?;
        let rows = statement
            .query_map(params![term], |row| row.get::<_, i64>(0))
            .map_err(|error| sql(&error))?;
        for row in rows {
            if result.as_slice().len() >= limit {
                return Err(IndexError::ResourceLimited);
            }
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 16,
                ..WorkUsage::default()
            })?;
            let position = u32::try_from(row.map_err(|error| sql(&error))?)
                .map_err(|_error| IndexError::CorruptObject("invalid token position".into()))?;
            result.push(position, budget)?;
        }
        Ok(result)
    }

    pub(super) fn inventory_batch(
        &self,
        edits: &[(u32, MapEdit)],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let root_limit = u32::try_from(PAGE).map_err(|_error| IndexError::ResourceLimited)?;
        if edits.iter().any(|(root, _)| *root >= root_limit) {
            return Err(IndexError::InvalidInput(
                "inventory root ordinal exceeds batch".into(),
            ));
        }
        let demands = BatchDemands::new(edits, |(_, edit)| {
            u64::try_from(edit.key.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(map_field_reservation(2, 64)?)
                .ok_or(IndexError::ResourceLimited)
        })?;
        let db = self.db.scoped()?;
        quanta_index_scratch_sqlite::write_map_edit_batch(
            &db,
            self.allocation.as_ref(),
            "INSERT INTO inventory(root,k,p,n) VALUES ",
            4096,
            demands.as_slice()?,
            |slot| {
                let (root, edit) = edits.get(slot).ok_or(IndexError::ResourceLimited)?;
                if edit.expected == edit.next {
                    return Err(IndexError::BackendViolation(
                        "inventory scratch contains an unchanged edit".into(),
                    ));
                }
                Ok((
                    rusqlite::types::ValueRef::Integer(i64::from(*root)),
                    edit,
                    0,
                ))
            },
            budget,
        )
    }

    pub(super) fn inventory_cursor(&self, roots: usize) -> Result<InventoryCursor<'_>, IndexError> {
        if roots == 0 || roots > PAGE {
            return Err(IndexError::InvalidInput(
                "invalid inventory root count".into(),
            ));
        }
        Ok(InventoryCursor {
            spool: self,
            roots: u32::try_from(roots).map_err(|_error| IndexError::ResourceLimited)?,
            after: None,
            page: EditPage::default(),
        })
    }

    pub(super) fn inventory_root_cursor(&self, root: usize) -> Result<RootCursor<'_>, IndexError> {
        if root >= PAGE {
            return Err(IndexError::InvalidInput(
                "inventory root exceeds batch".into(),
            ));
        }
        Ok(RootCursor {
            spool: self,
            target: RootTarget::Inventory(
                u32::try_from(root).map_err(|_error| IndexError::ResourceLimited)?,
            ),
            after: None,
            page: EditPage::default(),
        })
    }

    pub(super) fn posting_root_cursor<'a>(&'a self, term: &'a [u8]) -> RootCursor<'a> {
        RootCursor {
            spool: self,
            target: RootTarget::Posting(term),
            after: None,
            page: EditPage::default(),
        }
    }

    pub(super) fn posting_batch(
        &self,
        rows: &[(Vec<u8>, MapEdit)],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let fields_bytes = map_field_reservation(2, 64)?;
        let demands = BatchDemands::new(rows, |(term, edit)| {
            u64::try_from(term.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(
                    u64::try_from(edit.key.len()).map_err(|_error| IndexError::ResourceLimited)?,
                )
                .and_then(|bytes| bytes.checked_add(fields_bytes))
                .ok_or(IndexError::ResourceLimited)
        })?;
        let db = self.db.scoped()?;
        quanta_index_scratch_sqlite::write_map_edit_batch(
            &db,
            self.allocation.as_ref(),
            "INSERT INTO posting(term,k,p,n) VALUES ",
            4096,
            demands.as_slice()?,
            |slot| {
                let (term, edit) = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
                Ok((rusqlite::types::ValueRef::Blob(term), edit, 0))
            },
            budget,
        )
    }

    pub(super) fn posting_term_page(
        &self,
        after: Option<&[u8]>,
        budget: &mut WorkBudget,
    ) -> Result<TermPage, IndexError> {
        term_page(&*self.db.scoped()?, POSTING_TERM_PAGE_SQL, after, budget)
    }

    pub(super) fn posting_cardinality(
        &self,
        term: &[u8],
        prior: u64,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let io_scope = self.db.scoped()?;
        let mut statement = io_scope
            .prepare_cached(
                "SELECT COALESCE(SUM(p IS NULL AND n IS NOT NULL),0),
                        COALESCE(SUM(p IS NOT NULL AND n IS NULL),0)
                 FROM posting WHERE term=?1",
            )
            .map_err(|error| sql(&error))?;
        let (added, removed): (i64, i64) = statement
            .query_row(params![term], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| sql(&error))?;
        let added = u64::try_from(added)
            .map_err(|_error| IndexError::CorruptObject("negative posting addition".into()))?;
        let removed = u64::try_from(removed)
            .map_err(|_error| IndexError::CorruptObject("negative posting removal".into()))?;
        prior
            .checked_sub(removed)
            .and_then(|value| value.checked_add(added))
            .ok_or_else(|| IndexError::CorruptObject("term df overflow or underflow".into()))
    }

    pub(super) fn posting_cursor(&self, terms: TermPage) -> PostingCursor<'_> {
        PostingCursor {
            spool: self,
            terms,
            slot: 0,
            after: None,
            page: EditPage::default(),
        }
    }

    /// Data-only reuse within this disposable prepare. top surface0 receives
    /// next references only after the original attempt confirms their install.
    /// This is not registration authority; final parent installation authenticates
    /// the exact child in the Catalog again.
    pub(super) fn has_confirmed_posting(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let db = self.db.scoped()?;
        let mut statement = db.prepare_cached(
            "SELECT 1 FROM top INDEXED BY top_confirmed_posting WHERE surface=0 AND n=?1 LIMIT 1"
        ).map_err(|error| sql(&error))?;
        let found: Option<i64> = statement
            .query_row([reference_bytes(&reference)?], |row| row.get(0))
            .optional()
            .map_err(|error| sql(&error))?;
        budget.checkpoint()?;
        Ok(found.is_some())
    }

    pub(super) fn top_batch(
        &self,
        rows: &[(u8, MapEdit)],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let index_bytes = map_field_reservation(1, 32)?;
        let demands = BatchDemands::new(rows, |(surface, edit)| {
            let key =
                u64::try_from(edit.key.len()).map_err(|_error| IndexError::ResourceLimited)?;
            let base = key
                .checked_add(map_field_reservation(2, 64)?)
                .ok_or(IndexError::ResourceLimited)?;
            if *surface == 0 && edit.next.is_some() {
                base.checked_add(key)
                    .and_then(|bytes| bytes.checked_add(index_bytes))
                    .ok_or(IndexError::ResourceLimited)
            } else {
                Ok(base)
            }
        })?;
        let db = self.db.scoped()?;
        quanta_index_scratch_sqlite::write_map_edit_batch(
            &db,
            self.allocation.as_ref(),
            "INSERT INTO top(surface,k,p,n) VALUES ",
            4096,
            demands.as_slice()?,
            |slot| {
                let (surface, edit) = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
                let secondary = if *surface == 0 && edit.next.is_some() {
                    u64::try_from(edit.key.len())
                        .map_err(|_error| IndexError::ResourceLimited)?
                        .checked_add(index_bytes)
                        .ok_or(IndexError::ResourceLimited)?
                } else {
                    0
                };
                Ok((
                    rusqlite::types::ValueRef::Integer(i64::from(*surface)),
                    edit,
                    secondary,
                ))
            },
            budget,
        )
    }

    pub(super) fn top_cursor(&self) -> TopCursor<'_> {
        TopCursor {
            spool: self,
            surface: 0,
            after: None,
            page: EditPage::default(),
        }
    }
}

pub(super) struct InventoryCursor<'a> {
    spool: &'a TextSpool,
    roots: u32,
    after: Option<OrdinalMapEdit>,
    page: EditPage,
}

impl MapEditCursor for InventoryCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        loop {
            budget.checkpoint()?;
            if let Some(edit) = self.page.pop()? {
                self.after = Some(edit.clone());
                return Ok(Some(edit));
            }
            if self.page.complete() {
                return Ok(None);
            }
            let prior_root = self.after.as_ref().map(|position| position.root);
            let prior_key = self
                .after
                .as_ref()
                .map(|position| position.edit.key.as_slice())
                .unwrap_or_default();
            let bounds: [&dyn rusqlite::ToSql; 2] = [&prior_root, &prior_key];
            let (query, parameters): (&str, &[&dyn rusqlite::ToSql]) = if self.after.is_some() {
                (INVENTORY_NEXT_SQL, &bounds)
            } else {
                (INVENTORY_FIRST_SQL, &[])
            };
            let db = self.spool.db.scoped()?;
            let mut statement = db.prepare_cached(query).map_err(|error| sql(&error))?;
            let roots = self.roots;
            self.page = EditPage::read(&mut statement, parameters, 4096, budget, |row, budget| {
                let raw: i64 = row.get(3).map_err(|error| sql(&error))?;
                let root = u32::try_from(raw).map_err(|_error| {
                    IndexError::CorruptObject("negative inventory root ordinal".into())
                })?;
                if root >= roots {
                    return Err(IndexError::CorruptObject(
                        "inventory root ordinal out of bounds".into(),
                    ));
                }
                let edit = read_edit(
                    row,
                    usize::try_from(MAX_TERM_KEY_BYTES)
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    root,
                    budget,
                )?;
                check_term_key(&edit.edit.key)?;
                Ok(edit)
            })?;
        }
    }
}

enum RootTarget<'a> {
    Inventory(u32),
    Posting(&'a [u8]),
}

/// One immutable logical root, normalized to ordinal0 for compact preparation.
pub(super) struct RootCursor<'a> {
    spool: &'a TextSpool,
    target: RootTarget<'a>,
    after: Option<OrdinalMapEdit>,
    page: EditPage,
}
impl MapEditCursor for RootCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        loop {
            budget.checkpoint()?;
            if let Some(edit) = self.page.pop()? {
                self.after = Some(edit.clone());
                return Ok(Some(edit));
            }
            if self.page.complete() {
                return Ok(None);
            }
            let db = self.spool.db.scoped()?;
            let after = self.after.as_ref().map(|row| row.edit.key.as_slice());
            let inventory = matches!(self.target, RootTarget::Inventory(_));
            let (query, selector, bound): (&str, &dyn rusqlite::ToSql, usize) = match &self.target {
                RootTarget::Inventory(root) => (
                    if after.is_some() {
                        INVENTORY_ROOT_NEXT_SQL
                    } else {
                        INVENTORY_ROOT_FIRST_SQL
                    },
                    root,
                    usize::try_from(MAX_TERM_KEY_BYTES)
                        .map_err(|_error| IndexError::ResourceLimited)?,
                ),
                RootTarget::Posting(term) => (
                    if after.is_some() {
                        POSTING_ROOT_NEXT_SQL
                    } else {
                        POSTING_ROOT_FIRST_SQL
                    },
                    term,
                    32,
                ),
            };
            let mut statement = db.prepare_cached(query).map_err(|error| sql(&error))?;
            self.page = EditPage::read(
                &mut statement,
                &[selector, &after.unwrap_or_default()],
                4096,
                budget,
                |row, budget| {
                    let edit = read_edit(row, bound, 0, budget)?;
                    if inventory {
                        check_term_key(&edit.edit.key)?;
                    } else if edit.edit.key.len() != 32 {
                        return Err(IndexError::CorruptObject(
                            "posting owner key width differs".into(),
                        ));
                    }
                    Ok(edit)
                },
            )?;
        }
    }
}

pub(super) fn check_term_key(key: &[u8]) -> Result<(), IndexError> {
    if key.len() < 2
        || u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?
            > MAX_TERM_KEY_BYTES
        || !matches!(key.first(), Some(0 | 1))
        || key
            .get(1..)
            .is_none_or(|term| std::str::from_utf8(term).is_err())
    {
        return Err(IndexError::CorruptObject("invalid native term key".into()));
    }
    Ok(())
}

pub(super) struct PostingCursor<'a> {
    spool: &'a TextSpool,
    terms: TermPage,
    slot: usize,
    after: Option<OrdinalMapEdit>,
    page: EditPage,
}

impl MapEditCursor for PostingCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        while let Some(term) = self.terms.get(self.slot) {
            if let Some(edit) = self.page.pop()? {
                self.after = Some(edit.clone());
                return Ok(Some(edit));
            }
            if self.page.complete() {
                self.slot = self
                    .slot
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                self.after = None;
                self.page = EditPage::default();
                continue;
            }
            let root = u32::try_from(self.slot).map_err(|_error| IndexError::ResourceLimited)?;
            let db = self.spool.db.scoped()?;
            let mut statement = db
                .prepare_cached(if self.after.is_some() {
                    POSTING_ROOT_NEXT_SQL
                } else {
                    POSTING_ROOT_FIRST_SQL
                })
                .map_err(|error| sql(&error))?;
            let after = self
                .after
                .as_ref()
                .map(|row| row.edit.key.as_slice())
                .unwrap_or_default();
            self.page = EditPage::read(
                &mut statement,
                &[&term.as_slice(), &after],
                4096,
                budget,
                |row, budget| {
                    let edit = read_edit(row, 32, root, budget)?;
                    if edit.edit.key.len() != 32 {
                        return Err(IndexError::CorruptObject(
                            "posting owner key width differs".into(),
                        ));
                    }
                    Ok(edit)
                },
            )?;
        }
        Ok(None)
    }
}

pub(super) struct TopCursor<'a> {
    spool: &'a TextSpool,
    surface: u8,
    after: Option<OrdinalMapEdit>,
    page: EditPage,
}

impl MapEditCursor for TopCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        while self.surface < 2 {
            if let Some(edit) = self.page.pop()? {
                self.after = Some(edit.clone());
                return Ok(Some(edit));
            }
            if self.page.complete() {
                self.surface = self
                    .surface
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                self.after = None;
                self.page = EditPage::default();
                continue;
            }
            let bound = if self.surface == 0 {
                usize::try_from(MAX_TERM_KEY_BYTES).map_err(|_error| IndexError::ResourceLimited)?
            } else {
                32
            };
            let db = self.spool.db.scoped()?;
            let mut statement = db
                .prepare_cached(TOP_NEXT_SQL)
                .map_err(|error| sql(&error))?;
            let after = self
                .after
                .as_ref()
                .map(|row| row.edit.key.as_slice())
                .unwrap_or_default();
            self.page = EditPage::read(
                &mut statement,
                &[&self.surface, &after],
                4096,
                budget,
                |row, budget| read_edit(row, bound, u32::from(self.surface), budget),
            )?;
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! check {
        ($condition:expr $(,)?) => {{
            if !$condition {
                return Err(IndexError::BackendViolation(format!(
                    "test assertion failed at {}:{}: {}", file!(), line!(), stringify!($condition)
                )).into());
            }
        }};
        ($condition:expr, $($message:tt)+) => {{
            if !$condition {
                return Err(IndexError::BackendViolation(format!($($message)+)).into());
            }
        }};
    }
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            let left = &$left;
            let right = &$right;
            if *left != *right {
                return Err(IndexError::BackendViolation(format!(
                    "test equality failed at {}:{}: {left:?} != {right:?}",
                    file!(),
                    line!()
                ))
                .into());
            }
        }};
    }

    use quanta_index_core::index::WorkLimits;
    use std::time::{Duration, Instant};

    mod edit_pages;

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 1000,
                writes: 1000,
                bytes: 10_000_000,
                mutations: 1000,
            },
            Instant::now()
                .checked_add(Duration::from_secs(10))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[test]
    fn confirmed_posting_seek_is_indexed_exact_local_and_obeys_original_control()
    -> Result<(), IndexError> {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        let reference = ObjectRef(quanta_index_core::index::Digest([90; 32]));
        check!(!spool.has_confirmed_posting(reference, &mut work)?);
        let plan: String = spool.db.scoped()?.query_row(
            "EXPLAIN QUERY PLAN SELECT 1 FROM top INDEXED BY top_confirmed_posting WHERE surface=0 AND n=?1 LIMIT 1",
            [reference_bytes(&reference)?], |row| row.get(3)).map_err(|error| sql(&error))?;
        check!(
            plan.contains("SEARCH top USING COVERING INDEX top_confirmed_posting"),
            "{plan}"
        );
        spool.top_batch(
            &[
                (
                    1,
                    MapEdit {
                        key: b"owner".to_vec(),
                        expected: None,
                        next: Some((reference).into()),
                    },
                ),
                (
                    0,
                    MapEdit {
                        key: b"\0deleted".to_vec(),
                        expected: Some((reference).into()),
                        next: None,
                    },
                ),
            ],
            &mut work,
        )?;
        check!(!spool.has_confirmed_posting(reference, &mut work)?);
        spool.top_batch(
            &[(
                0,
                MapEdit {
                    key: b"\0inline".to_vec(),
                    expected: None,
                    next: Some(quanta_index_core::index::MapValue::inline(
                        &reference.0.0,
                        None,
                    )?),
                },
            )],
            &mut work,
        )?;
        check!(!spool.has_confirmed_posting(reference, &mut work)?);
        spool.top_batch(
            &[(
                0,
                MapEdit {
                    key: b"\0term".to_vec(),
                    expected: None,
                    next: Some((reference).into()),
                },
            )],
            &mut work,
        )?;
        check!(spool.has_confirmed_posting(reference, &mut work)?);
        let other = TextSpool::new(&TestScratch, &mut work)?;
        check!(!other.has_confirmed_posting(reference, &mut work)?);
        work.request().cancel_handle().cancel();
        check!(matches!(
            spool.has_confirmed_posting(reference, &mut work),
            Err(IndexError::Cancelled)
        ));
        let mut denied = WorkBudget::new(
            WorkLimits {
                reads: 0,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            Instant::now()
                .checked_add(Duration::from_secs(10))
                .ok_or(IndexError::ResourceLimited)?,
        );
        check!(matches!(
            spool.has_confirmed_posting(reference, &mut denied),
            Err(IndexError::ResourceLimited)
        ));
        Ok(())
    }

    #[test]
    fn inventory_ordinals_preserve_all_drafts_across_token_resets_and_refuse_foreign_rows()
    -> Result<(), IndexError> {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        let plan: String = spool
            .db
            .scoped()?
            .query_row(
                &format!("EXPLAIN QUERY PLAN {INVENTORY_NEXT_SQL}"),
                params![0, b"\0a".as_slice()],
                |row| row.get(3),
            )
            .map_err(|error| sql(&error))?;
        check!(
            plan.contains("SEARCH inventory USING PRIMARY KEY"),
            "{plan}"
        );
        let reference = ObjectRef(quanta_index_core::index::Digest([90; 32]));
        for (root, key) in [(1, b"\0a".as_slice()), (0, b"\0z".as_slice())] {
            spool.inventory_batch(
                &[(
                    root,
                    MapEdit {
                        key: key.to_vec(),
                        expected: None,
                        next: Some((reference).into()),
                    },
                )],
                &mut work,
            )?;
            spool.reset_tokens(&mut work)?;
        }
        let mut cursor = spool.inventory_cursor(2)?;
        for (root, key) in [(0, b"\0z".as_slice()), (1, b"\0a".as_slice())] {
            let row = cursor
                .next_edit(&mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(row.root, root);
            check_eq!(row.edit.key.as_slice(), key);
            check_eq!(row.edit.expected, None);
            check_eq!(row.edit.next, Some(reference.into()));
        }
        check!(cursor.next_edit(&mut work)?.is_none());
        let mut short = spool.inventory_cursor(1)?;
        check!(short.next_edit(&mut work)?.is_some());
        for _ in 0..2 {
            check!(matches!(
                short.next_edit(&mut work),
                Err(IndexError::CorruptObject(_))
            ));
        }
        let before = work.usage();
        check!(matches!(
            spool.inventory_batch(
                &[(
                    64,
                    MapEdit {
                        key: b"\0a".to_vec(),
                        expected: None,
                        next: Some((reference).into())
                    }
                )],
                &mut work
            ),
            Err(IndexError::InvalidInput(_))
        ));
        check_eq!(work.usage(), before);
        spool.reset_inventories(&mut work)?;
        check!(spool.inventory_cursor(2)?.next_edit(&mut work)?.is_none());
        for root in [-1, 64] {
            check_eq!(
                spool
                    .db
                    .scoped()?
                    .execute(
                        "INSERT INTO inventory VALUES(?1,x'0161',NULL,zeroblob(32))",
                        params![root],
                    )
                    .map_err(|error| sql(&error))?,
                1
            );
            let mut malformed = spool.inventory_cursor(2)?;
            check!(matches!(
                malformed.next_edit(&mut work),
                Err(IndexError::CorruptObject(_))
            ));
            check!(malformed.after.is_none());
            spool.reset_inventories(&mut work)?;
        }
        Ok(())
    }

    #[test]
    fn batch_width_refuses_before_demand_materialization() -> Result<(), IndexError> {
        check!(check_batch_width(0).is_err());
        check!(check_batch_width(PAGE).is_ok());
        check!(check_batch_width(PAGE.checked_add(1).ok_or(IndexError::ResourceLimited)?).is_err());
        for rows in [[(); 0].as_slice(), [(); 65].as_slice()] {
            let mut calls = 0u64;
            let result = BatchDemands::new(rows, |()| {
                calls = calls.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                Ok(1)
            });
            check!(matches!(result, Err(IndexError::ResourceLimited)));
            check_eq!(calls, 0);
        }
        Ok(())
    }

    #[test]
    fn stack_batch_demands_preserve_order_length_and_original_refusal() -> Result<(), IndexError> {
        let demands = BatchDemands::new(&[3, 7, 11], |bytes| Ok(*bytes))?;
        check_eq!(demands.as_slice()?, &[3, 7, 11]);
        let full = BatchDemands::new(&[19; 64], |bytes| Ok(*bytes))?;
        check_eq!(full.as_slice()?, &[19; 64]);
        let mut calls = 0u64;
        let result = BatchDemands::new(&[3, 7, 11], |bytes| {
            calls = calls.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            if *bytes == 7 {
                return Err(IndexError::Cancelled);
            }
            Ok(*bytes)
        });
        check!(matches!(result, Err(IndexError::Cancelled)));
        check_eq!(calls, 2);
        Ok(())
    }

    struct TestScratch;
    struct TestAllocation(tempfile::TempDir, u64);

    impl ScratchAllocation for TestAllocation {
        fn path(&self) -> &std::path::Path {
            self.0.path()
        }
        fn max_bytes(&self) -> u64 {
            self.1
        }
        fn reserved_bytes(&self) -> Result<u64, IndexError> {
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
            let directory = tempfile::tempdir().map_err(|error| {
                IndexError::Storage(format!("text spool test scratch: {error}"))
            })?;
            Ok(Box::new(TestAllocation(directory, request.max_bytes)))
        }
    }

    #[test]
    fn malformed_edit_rows_refuse_without_advancing_each_text_cursor() -> Result<(), IndexError> {
        enum CursorKind {
            Inventory,
            Posting,
            TermTop,
            OwnerTop,
        }
        for (kind, statement) in [
            (
                CursorKind::Inventory,
                "INSERT INTO inventory VALUES(0,x'0161',NULL,zeroblob(31))",
            ),
            (
                CursorKind::Inventory,
                "INSERT INTO inventory VALUES(0,zeroblob(1025),NULL,zeroblob(32))",
            ),
            (
                CursorKind::Posting,
                "INSERT INTO posting VALUES(x'0161',x'01',zeroblob(33),NULL)",
            ),
            (
                CursorKind::Posting,
                "INSERT INTO posting VALUES(x'0161',zeroblob(33),NULL,zeroblob(32))",
            ),
            (
                CursorKind::TermTop,
                "INSERT INTO top VALUES(0,x'0161',NULL,zeroblob(1048576))",
            ),
            (
                CursorKind::TermTop,
                "INSERT INTO top VALUES(0,zeroblob(1025),NULL,zeroblob(32))",
            ),
            (
                CursorKind::OwnerTop,
                "INSERT INTO top VALUES(1,x'01','not-a-blob',NULL)",
            ),
            (
                CursorKind::OwnerTop,
                "INSERT INTO top VALUES(1,zeroblob(33),NULL,zeroblob(32))",
            ),
        ] {
            let mut work = budget()?;
            let spool = TextSpool::new(&TestScratch, &mut work)?;
            spool
                .db
                .scoped()?
                .execute_batch(statement)
                .map_err(|error| sql(&error))?;
            let mut cursor: Box<dyn MapEditCursor + '_> = match kind {
                CursorKind::Inventory => Box::new(spool.inventory_cursor(1)?),
                CursorKind::Posting => {
                    Box::new(spool.posting_cursor(spool.posting_term_page(None, &mut work)?))
                }
                CursorKind::TermTop | CursorKind::OwnerTop => Box::new(spool.top_cursor()),
            };
            for _retry in 0..2 {
                check!(
                    matches!(
                        cursor.next_edit(&mut work),
                        Err(IndexError::CorruptObject(_))
                    ),
                    "{statement}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn term_pages_refuse_oversized_sql_blob_before_owned_key_copy() -> Result<(), IndexError> {
        let db = Connection::open_in_memory().map_err(|error| sql(&error))?;
        db.execute_batch(
            "CREATE TABLE tokens(term BLOB NOT NULL,pos INTEGER NOT NULL,PRIMARY KEY(term,pos)) WITHOUT ROWID;
             CREATE TABLE posting(term BLOB NOT NULL,k BLOB NOT NULL,PRIMARY KEY(term,k)) WITHOUT ROWID;
             INSERT INTO tokens VALUES(zeroblob(1025),0);
             INSERT INTO posting VALUES(zeroblob(1025),x'01');",
        ).map_err(|error| sql(&error))?;
        for statement in [TOKEN_TERM_PAGE_SQL, POSTING_TERM_PAGE_SQL] {
            check!(matches!(
                term_page(&db, statement, None, &mut budget()?),
                Err(IndexError::CorruptObject(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn term_pages_match_distinct_key_oracle_with_bounded_page_ordering() -> Result<(), IndexError> {
        let db = Connection::open_in_memory().map_err(|error| sql(&error))?;
        db.execute_batch(
            "CREATE TABLE tokens(term BLOB NOT NULL,pos INTEGER NOT NULL,PRIMARY KEY(term,pos)) WITHOUT ROWID;
             CREATE TABLE posting(term BLOB NOT NULL,k BLOB NOT NULL,PRIMARY KEY(term,k)) WITHOUT ROWID;",
        ).map_err(|error| sql(&error))?;
        let mut expected = Vec::new();
        for index in 0u16..129 {
            let term = format!("term-{index:03}").into_bytes();
            expected.push(term.clone());
            for position in 0..3u32 {
                if db
                    .execute(
                        "INSERT INTO tokens(term,pos) VALUES(?1,?2)",
                        params![term, position],
                    )
                    .map_err(|error| sql(&error))?
                    != 1
                {
                    return Err(IndexError::BackendViolation(
                        "fixture token insertion affected an unexpected row count".into(),
                    ));
                }
                if db
                    .execute(
                        "INSERT INTO posting(term,k) VALUES(?1,?2)",
                        params![term, position.to_be_bytes()],
                    )
                    .map_err(|error| sql(&error))?
                    != 1
                {
                    return Err(IndexError::BackendViolation(
                        "fixture posting insertion affected an unexpected row count".into(),
                    ));
                }
            }
        }
        for sql_text in [TOKEN_TERM_PAGE_SQL, POSTING_TERM_PAGE_SQL] {
            let mut actual = Vec::new();
            let mut after: Option<Vec<u8>> = None;
            let mut work = budget()?;
            loop {
                let page = term_page(&db, sql_text, after.as_deref(), &mut work)?;
                if page.is_empty() {
                    break;
                }
                check!(page.len() <= PAGE);
                after = page.last().map(|key| key.as_slice().to_vec());
                actual.extend(page.iter().map(|key| key.as_slice().to_vec()));
            }
            check_eq!(actual, expected);
            check_eq!(work.usage().reads, 133);
            let mut explain = db
                .prepare(&format!("EXPLAIN QUERY PLAN {sql_text}"))
                .map_err(|error| sql(&error))?;
            let start = Vec::<u8>::new();
            let details = explain
                .query_map(params![start], |row| row.get::<_, String>(3))
                .map_err(|error| sql(&error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| sql(&error))?;
            check!(
                details
                    .iter()
                    .all(|detail| !detail.contains("USE TEMP B-TREE FOR DISTINCT")),
                "{details:?}"
            );
            check!(
                details
                    .iter()
                    .any(|detail| detail.contains("USING PRIMARY KEY")),
                "{details:?}"
            );
        }
        Ok(())
    }
}
