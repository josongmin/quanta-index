//! Bounded disposable sort for native token preparation. Only immutable map
//! roots published through the catalog can become search authority.

use quanta_index_scratch_sqlite::{MapEditPage as EditPage, read_map_edit_budgeted as read_edit};

use super::text_memory::PreparedKey;
use quanta_index_core::index::{
    AttemptRef, IndexError, MapEdit, MapValue, ObjectRef, OrdinalMapEdit, ScratchAllocation,
    ScratchRequest, ScratchSpacePort, WorkBudget, WorkUsage,
    ports::{MAX_OBJECT_INSTALL_BATCH, MapEditCursor},
};
use quanta_index_scratch_sqlite::ScratchConnection;
use rusqlite::{Connection, OptionalExtension as _, params};
use std::cell::Cell;

mod confirmed_batch;
mod token_groups;

pub(super) const MAX_TERM_KEY_BYTES: u64 = 1_024;
pub(super) const SCRATCH_MAX_BYTES: u64 = 4_294_967_296;
const SCRATCH_INITIAL_BYTES: u64 = 256 * 1024;
const PAGE: usize = 64;
const PLANNED_ROOT_OFFSET: u32 = 64;
const PLANNED_ROOT_END: u32 = PLANNED_ROOT_OFFSET * 2;
const INVENTORY_CONFIRM_SQL: &str = "UPDATE inventory SET root=root-64 WHERE root>=64 AND root<128 AND n IN (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,?35,?36,?37,?38,?39,?40,?41,?42,?43,?44,?45,?46,?47,?48,?49,?50,?51,?52,?53,?54,?55,?56,?57,?58,?59,?60,?61,?62,?63,?64,?65,?66,?67,?68,?69,?70,?71,?72,?73,?74,?75,?76,?77,?78,?79,?80,?81,?82,?83,?84,?85,?86,?87,?88,?89,?90,?91,?92,?93,?94,?95,?96,?97,?98,?99,?100,?101,?102,?103,?104,?105,?106,?107,?108,?109,?110,?111,?112,?113,?114,?115,?116,?117,?118,?119,?120,?121,?122,?123,?124,?125,?126,?127,?128,?129,?130,?131,?132,?133,?134,?135,?136,?137,?138,?139,?140,?141,?142,?143,?144,?145,?146,?147,?148,?149,?150,?151,?152,?153,?154,?155,?156,?157,?158,?159,?160,?161,?162,?163,?164,?165,?166,?167,?168,?169,?170,?171,?172,?173,?174,?175,?176,?177,?178,?179,?180,?181,?182,?183,?184,?185,?186,?187,?188,?189,?190,?191,?192,?193,?194,?195,?196,?197,?198,?199,?200,?201,?202,?203,?204,?205,?206,?207,?208,?209,?210,?211,?212,?213,?214,?215,?216,?217,?218,?219,?220,?221,?222,?223,?224,?225,?226,?227,?228,?229,?230,?231,?232,?233,?234,?235,?236,?237,?238,?239,?240,?241,?242,?243,?244,?245,?246,?247,?248,?249,?250,?251,?252,?253,?254,?255,?256)";
const INVENTORY_CONFIRM_CHECK_SQL: &str =
    "SELECT COUNT(*),COUNT(CASE WHEN root>=128 OR n IS NULL OR n NOT IN (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,?35,?36,?37,?38,?39,?40,?41,?42,?43,?44,?45,?46,?47,?48,?49,?50,?51,?52,?53,?54,?55,?56,?57,?58,?59,?60,?61,?62,?63,?64,?65,?66,?67,?68,?69,?70,?71,?72,?73,?74,?75,?76,?77,?78,?79,?80,?81,?82,?83,?84,?85,?86,?87,?88,?89,?90,?91,?92,?93,?94,?95,?96,?97,?98,?99,?100,?101,?102,?103,?104,?105,?106,?107,?108,?109,?110,?111,?112,?113,?114,?115,?116,?117,?118,?119,?120,?121,?122,?123,?124,?125,?126,?127,?128,?129,?130,?131,?132,?133,?134,?135,?136,?137,?138,?139,?140,?141,?142,?143,?144,?145,?146,?147,?148,?149,?150,?151,?152,?153,?154,?155,?156,?157,?158,?159,?160,?161,?162,?163,?164,?165,?166,?167,?168,?169,?170,?171,?172,?173,?174,?175,?176,?177,?178,?179,?180,?181,?182,?183,?184,?185,?186,?187,?188,?189,?190,?191,?192,?193,?194,?195,?196,?197,?198,?199,?200,?201,?202,?203,?204,?205,?206,?207,?208,?209,?210,?211,?212,?213,?214,?215,?216,?217,?218,?219,?220,?221,?222,?223,?224,?225,?226,?227,?228,?229,?230,?231,?232,?233,?234,?235,?236,?237,?238,?239,?240,?241,?242,?243,?244,?245,?246,?247,?248,?249,?250,?251,?252,?253,?254,?255,?256) THEN 1 END)
     FROM inventory WHERE root>=64";
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
const POSTING_RANGE_FIRST_SQL: &str =
    "SELECT k,p,n,term FROM posting WHERE term>=?1 AND term<=?2 ORDER BY term,k LIMIT 64";
const POSTING_RANGE_NEXT_SQL: &str =
    "SELECT k,p,n,term FROM posting WHERE (term,k)>(?1,?2) AND term<=?3 ORDER BY term,k LIMIT 64";
const TOP_NEXT_SQL: &str = "SELECT k,p,n FROM top WHERE surface=?1 AND k>?2 ORDER BY k LIMIT 64";
#[cfg(test)]
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
const POSTING_CARDINALITY_PAGE_SQL: &str =
    "SELECT term, SUM(p IS NULL AND n IS NOT NULL), SUM(p IS NOT NULL AND n IS NULL)
     FROM posting WHERE term>=?1 AND term<=?2 GROUP BY term ORDER BY term";

pub(super) struct TextSpool {
    db: ScratchConnection,
    allocation: Box<dyn ScratchAllocation>,
    confirmed_attempt: Cell<Option<AttemptRef>>,
    token_read_active: Cell<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ConfirmedOccurrenceCount {
    Missing,
    Unvalidated,
    Validated(u64),
}

/// A disposable inventory row is not map authority until its occurrence was
/// installed and every physical output identity was checked by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InventoryPartition {
    Ready(u32),
    Planned(u32),
}

impl InventoryPartition {
    fn sql_root(self) -> Result<i64, IndexError> {
        let root = match self {
            Self::Ready(root) if root < PLANNED_ROOT_OFFSET => root,
            Self::Planned(root) if root < PLANNED_ROOT_OFFSET => root
                .checked_add(PLANNED_ROOT_OFFSET)
                .ok_or(IndexError::ResourceLimited)?,
            Self::Ready(_) | Self::Planned(_) => {
                return Err(IndexError::InvalidInput(
                    "inventory partition ordinal exceeds window".into(),
                ));
            }
        };
        Ok(i64::from(root))
    }

    fn from_sql(raw: i64) -> Result<Self, IndexError> {
        let root = u32::try_from(raw)
            .map_err(|_error| IndexError::CorruptObject("negative inventory partition".into()))?;
        match root {
            0..PLANNED_ROOT_OFFSET => Ok(Self::Ready(root)),
            PLANNED_ROOT_OFFSET..PLANNED_ROOT_END => Ok(Self::Planned(
                root.checked_sub(PLANNED_ROOT_OFFSET)
                    .ok_or(IndexError::ResourceLimited)?,
            )),
            _ => Err(IndexError::CorruptObject(
                "inventory partition exceeds window".into(),
            )),
        }
    }
}

/// Pre-admitted PK migration for exactly one physical occurrence packet.
/// The caller invokes `finish` only after checking every installed identity.
#[must_use]
pub(super) struct InventoryConfirmation<'a> {
    spool: &'a TextSpool,
    attempt: AttemptRef,
    expected_rows: u64,
    references: [ObjectRef; MAX_OBJECT_INSTALL_BATCH],
    record_confirmed: Option<confirmed_batch::ConfirmedOccurrenceRecord<'a>>,
}

impl InventoryConfirmation<'_> {
    pub(super) fn finish(self, budget: &WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()?;
        self.spool.check_confirmed_attempt(self.attempt)?;
        let first =
            reference_fixed_bytes(self.references.first().ok_or(IndexError::ResourceLimited)?)?;
        let mut values = [first; MAX_OBJECT_INSTALL_BATCH];
        for (value, reference) in values.iter_mut().zip(&self.references) {
            *value = reference_fixed_bytes(reference)?;
        }
        let db = self.spool.db.scoped(budget)?;
        // Validate the complete still-planned set again before moving any PK.
        // The private receipt cannot promote a later, unrelated physical leaf.
        let (count, invalid): (i64, i64) = db
            .prepare_cached(INVENTORY_CONFIRM_CHECK_SQL)
            .map_err(|error| sql(&error))?
            .query_row(
                rusqlite::params_from_iter(values.iter().map(<[u8; 33]>::as_slice)),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| sql(&error))?;
        if u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)? != self.expected_rows
            || invalid != 0
        {
            return Err(IndexError::BackendViolation(
                "inventory confirmation planned rows changed after admission".into(),
            ));
        }
        let changed = db
            .prepare_cached(INVENTORY_CONFIRM_SQL)
            .map_err(|error| sql(&error))?
            .execute(rusqlite::params_from_iter(
                values.iter().map(<[u8; 33]>::as_slice),
            ))
            .map_err(|error| sql(&error))?;
        if u64::try_from(changed).map_err(|_error| IndexError::ResourceLimited)?
            != self.expected_rows
        {
            return Err(IndexError::BackendViolation(
                "inventory confirmation changed unexpected row count".into(),
            ));
        }
        let remaining: Option<i64> = db
            .prepare_cached("SELECT 1 FROM inventory WHERE root>=64 LIMIT 1")
            .map_err(|error| sql(&error))?
            .query_row([], |row| row.get(0))
            .optional()
            .map_err(|error| sql(&error))?;
        if remaining.is_some() {
            return Err(IndexError::BackendViolation(
                "inventory confirmation left unconfirmed rows".into(),
            ));
        }
        if let Some(record) = self.record_confirmed {
            record.finish(budget)?;
        }
        budget.checkpoint()
    }
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

// Reuse the canonical codec while retaining only this field's exact width.
// A generic inline map value has a wider maximum and is not stored here.
fn reference_fixed_bytes(reference: &ObjectRef) -> Result<[u8; 33], IndexError> {
    quanta_index_core::index::MapValue::Object(*reference)
        .encoded()?
        .as_slice()?
        .try_into()
        .map_err(|_error| IndexError::BackendViolation("object map value width changed".into()))
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

#[derive(Clone, Copy, Default)]
struct PostingDelta {
    added: u64,
    removed: u64,
}

/// Counts are data from the disposable spool, never posting-map authority.
pub(super) struct PostingCardinalities {
    deltas: [PostingDelta; PAGE],
    len: usize,
    _memory: Option<quanta_index_core::index::MemoryLease>,
}

impl PostingCardinalities {
    pub(super) fn df(&self, slot: usize, prior: u64) -> Result<u64, IndexError> {
        let delta = self
            .deltas
            .get(slot)
            .filter(|_| slot < self.len)
            .ok_or(IndexError::ResourceLimited)?;
        prior
            .checked_sub(delta.removed)
            .and_then(|value| value.checked_add(delta.added))
            .ok_or_else(|| IndexError::CorruptObject("term df overflow or underflow".into()))
    }
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
        let owned_db = ScratchConnection::open(
            &allocation.path().join("text.sqlite"),
            allocation.as_ref(),
            budget,
        )?;
        let db = owned_db.scoped(budget)?;
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
             CREATE TABLE confirmed_occurrence(
                 ref BLOB PRIMARY KEY, validated_count INTEGER) WITHOUT ROWID;
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
            confirmed_attempt: Cell::new(None),
            token_read_active: Cell::new(false),
        })
    }

    fn check_confirmed_attempt(&self, attempt: AttemptRef) -> Result<(), IndexError> {
        match self.confirmed_attempt.get() {
            Some(bound) if bound != attempt => Err(IndexError::FenceLost),
            Some(_) => Ok(()),
            None => {
                self.confirmed_attempt.set(Some(attempt));
                Ok(())
            }
        }
    }

    /// A cursor must refuse an unconfirmed row before yielding even an earlier
    /// ready row from the same SQL page. Both probes use the inventory PK.
    fn require_ready_inventories(&self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            reads: 2,
            bytes: 8192,
            ..WorkUsage::default()
        })?;
        let db = self.db.scoped(budget)?;
        for query in [
            "SELECT 1 FROM inventory WHERE root<0 LIMIT 1",
            "SELECT 1 FROM inventory WHERE root>=64 LIMIT 1",
        ] {
            let found: Option<i64> = db
                .prepare_cached(query)
                .map_err(|error| sql(&error))?
                .query_row([], |row| row.get(0))
                .optional()
                .map_err(|error| sql(&error))?;
            if found.is_some() {
                return Err(IndexError::CorruptObject(
                    "inventory has an unconfirmed or invalid partition".into(),
                ));
            }
        }
        budget.checkpoint()
    }

    /// Admit the complete PK move before a physical occurrence install. A
    /// planned row is a disposable edit, never an installed child witness.
    pub(super) fn admit_inventory_confirmation<'a>(
        &'a self,
        refs: &[ObjectRef],
        expected_rows: u64,
        attempt: AttemptRef,
        record_confirmed: bool,
        budget: &mut WorkBudget,
    ) -> Result<InventoryConfirmation<'a>, IndexError> {
        budget.checkpoint()?;
        self.check_confirmed_attempt(attempt)?;
        if refs.is_empty() || refs.len() > MAX_OBJECT_INSTALL_BATCH || expected_rows == 0 {
            return Err(IndexError::InvalidInput(
                "invalid inventory confirmation envelope".into(),
            ));
        }
        for (slot, reference) in refs.iter().enumerate() {
            if refs
                .get(..slot)
                .ok_or(IndexError::ResourceLimited)?
                .contains(reference)
            {
                return Err(IndexError::BackendViolation(
                    "inventory confirmation repeats a physical reference".into(),
                ));
            }
            let _encoded = reference_fixed_bytes(reference)?;
        }
        budget.charge(WorkUsage {
            reads: 2,
            bytes: 8192,
            ..WorkUsage::default()
        })?;
        let db = self.db.scoped(budget)?;
        let invalid: Option<i64> = db
            .prepare_cached("SELECT 1 FROM inventory WHERE root<0 LIMIT 1")
            .map_err(|error| sql(&error))?
            .query_row([], |row| row.get(0))
            .optional()
            .map_err(|error| sql(&error))?;
        if invalid.is_some() {
            return Err(IndexError::CorruptObject(
                "inventory has a negative partition".into(),
            ));
        }
        let collision: Option<i64> = db
            .prepare_cached(
                "SELECT 1 FROM inventory p JOIN inventory r
                 ON r.root=p.root-64 AND r.k=p.k
                 WHERE p.root>=64 AND p.root<128 LIMIT 1",
            )
            .map_err(|error| sql(&error))?
            .query_row([], |row| row.get(0))
            .optional()
            .map_err(|error| sql(&error))?;
        if collision.is_some() {
            return Err(IndexError::Conflict);
        }
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let mut statement = db
            .prepare_cached("SELECT root,k,p,n FROM inventory WHERE root>=64 ORDER BY root,k")
            .map_err(|error| sql(&error))?;
        let mut rows = statement.query([]).map_err(|error| sql(&error))?;
        let mut count = 0_u64;
        let mut move_bytes = 0_u64;
        while let Some(row) = rows.next().map_err(|error| sql(&error))? {
            budget.checkpoint()?;
            let raw: i64 = row.get(0).map_err(|error| sql(&error))?;
            if !matches!(
                InventoryPartition::from_sql(raw)?,
                InventoryPartition::Planned(_)
            ) {
                return Err(IndexError::CorruptObject(
                    "inventory confirmation includes a ready row".into(),
                ));
            }
            let key = quanta_index_scratch_sqlite::bounded_blob(
                row.get_ref(1).map_err(|error| sql(&error))?,
                usize::try_from(MAX_TERM_KEY_BYTES)
                    .map_err(|_error| IndexError::ResourceLimited)?,
            )?;
            check_term_key(key)?;
            let prior = quanta_index_scratch_sqlite::optional_map_value(
                row.get_ref(2).map_err(|error| sql(&error))?,
            )?;
            let next = quanta_index_scratch_sqlite::optional_map_value(
                row.get_ref(3).map_err(|error| sql(&error))?,
            )?;
            let Some(MapValue::Object(reference)) = next else {
                return Err(IndexError::CorruptObject(
                    "planned inventory lacks an occurrence reference".into(),
                ));
            };
            if prior == next || !refs.contains(&reference) {
                return Err(IndexError::CorruptObject(
                    "planned inventory has unchanged or foreign occurrence".into(),
                ));
            }
            // The inventory table stores only object references. Charge the
            // exact field envelope after decoding it, rather than the larger
            // generic inline-map maximum used for untyped scratch edits.
            let prior_bytes = match prior {
                None => 1_u64,
                Some(MapValue::Object(_)) => 33_u64,
                Some(MapValue::Inline(_)) => {
                    return Err(IndexError::CorruptObject(
                        "planned inventory prior is not an occurrence reference".into(),
                    ));
                }
            };
            count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            let row_bytes = u64::try_from(key.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(prior_bytes)
                .and_then(|bytes| bytes.checked_add(33))
                .and_then(|bytes| bytes.checked_add(72))
                .ok_or(IndexError::ResourceLimited)?;
            move_bytes = move_bytes
                .checked_add(
                    row_bytes
                        .checked_mul(2)
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                reads: 1,
                bytes: row_bytes,
                ..WorkUsage::default()
            })?;
        }
        if count != expected_rows {
            return Err(IndexError::BackendViolation(
                "inventory confirmation row count differs".into(),
            ));
        }
        drop(rows);
        drop(statement);
        // The post-install aggregate rechecks every planned row; its reads and
        // bounded SQL programs are funded now, before the physical call.
        budget.charge(WorkUsage {
            reads: count.checked_add(2).ok_or(IndexError::ResourceLimited)?,
            bytes: count
                .checked_mul(80)
                .and_then(|bytes| bytes.checked_add(8192))
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        drop(
            db.prepare_cached(INVENTORY_CONFIRM_CHECK_SQL)
                .map_err(|error| sql(&error))?,
        );
        drop(
            db.prepare_cached(INVENTORY_CONFIRM_SQL)
                .map_err(|error| sql(&error))?,
        );
        let record = if record_confirmed {
            Some(confirmed_batch::ConfirmedOccurrenceRecord::prepare(
                self, refs, attempt, budget,
            )?)
        } else {
            None
        };
        // UPDATE changes the primary key. Admit both the old and new row
        // envelopes and confirmed-record scratch with one capacity decision
        // before put_batch.
        budget.charge(WorkUsage {
            writes: count,
            bytes: move_bytes,
            ..WorkUsage::default()
        })?;
        let record_bytes = record
            .as_ref()
            .map(confirmed_batch::ConfirmedOccurrenceRecord::scratch_bytes)
            .transpose()?
            .unwrap_or(0);
        let reserve_bytes = move_bytes
            .checked_add(record_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &db,
            self.allocation.as_ref(),
            reserve_bytes,
            budget,
        )?;
        budget.checkpoint()?;
        Ok(InventoryConfirmation {
            spool: self,
            attempt,
            expected_rows,
            references: {
                let mut padded =
                    [*refs.first().ok_or(IndexError::ResourceLimited)?; MAX_OBJECT_INSTALL_BATCH];
                padded
                    .get_mut(..refs.len())
                    .ok_or(IndexError::ResourceLimited)?
                    .copy_from_slice(refs);
                padded
            },
            record_confirmed: record,
        })
    }

    /// A disposable positive memo of objects already confirmed by this attempt.
    /// It cannot grant publication or bypass a first physical installation.
    pub(super) fn confirmed_occurrence_contains(
        &self,
        reference: ObjectRef,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        Ok(!matches!(
            self.confirmed_occurrence_count(reference, attempt, budget)?,
            ConfirmedOccurrenceCount::Missing
        ))
    }

    /// A count is only a hint after an exact, attempt-local confirmed row has
    /// been written and the immutable leaf has passed its first full read.
    pub(super) fn confirmed_occurrence_count(
        &self,
        reference: ObjectRef,
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<ConfirmedOccurrenceCount, IndexError> {
        budget.checkpoint()?;
        self.check_confirmed_attempt(attempt)?;
        // The fixed 33-byte reference and optional 8-byte integer fit this
        // existing per-row logical envelope; no count payload is retained.
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 64,
            ..WorkUsage::default()
        })?;
        let key = reference_bytes(&reference)?;
        let io_scope = self.db.scoped(budget)?;
        let mut statement = io_scope
            .prepare_cached("SELECT ref,validated_count FROM confirmed_occurrence WHERE ref=?1")
            .map_err(|error| sql(&error))?;
        let mut rows = statement.query(params![key]).map_err(|error| sql(&error))?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            budget.checkpoint()?;
            return Ok(ConfirmedOccurrenceCount::Missing);
        };
        let stored = quanta_index_scratch_sqlite::fixed_blob::<33>(
            row.get_ref(0).map_err(|error| sql(&error))?,
        )?;
        if MapValue::decode(&stored)? != MapValue::Object(reference) {
            return Err(IndexError::CorruptObject(
                "confirmed occurrence scratch identity changed".into(),
            ));
        }
        let count = match row.get_ref(1).map_err(|error| sql(&error))? {
            rusqlite::types::ValueRef::Null => ConfirmedOccurrenceCount::Unvalidated,
            rusqlite::types::ValueRef::Integer(value)
                if value > 0
                    && u64::try_from(value)
                        .is_ok_and(|count| count <= super::text::MAX_POSITIONS_PER_TERM) =>
            {
                ConfirmedOccurrenceCount::Validated(
                    u64::try_from(value).map_err(|_error| IndexError::ResourceLimited)?,
                )
            }
            rusqlite::types::ValueRef::Integer(_)
            | rusqlite::types::ValueRef::Real(_)
            | rusqlite::types::ValueRef::Text(_)
            | rusqlite::types::ValueRef::Blob(_) => {
                return Err(IndexError::CorruptObject(
                    "confirmed occurrence scratch count is invalid".into(),
                ));
            }
        };
        if rows.next().map_err(|error| sql(&error))?.is_some() {
            return Err(IndexError::CorruptObject(
                "confirmed occurrence scratch repeats an identity".into(),
            ));
        }
        budget.checkpoint()?;
        Ok(count)
    }

    /// Store a count only after the caller has read and fully validated the
    /// original immutable leaf. An absent or previously validated row is not
    /// permission to create or overwrite a count.
    pub(super) fn record_validated_occurrence_count(
        &self,
        reference: ObjectRef,
        attempt: AttemptRef,
        count: u64,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        self.check_confirmed_attempt(attempt)?;
        if count == 0 || count > super::text::MAX_POSITIONS_PER_TERM {
            return Err(IndexError::CorruptObject(
                "confirmed occurrence count is outside the leaf envelope".into(),
            ));
        }
        budget.charge(WorkUsage {
            writes: 1,
            bytes: 128,
            ..WorkUsage::default()
        })?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.db.scoped(budget)?,
            self.allocation.as_ref(),
            128,
            budget,
        )?;
        let key = reference_bytes(&reference)?;
        let count = i64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?;
        let changed = self
            .db
            .scoped(budget)?
            .prepare_cached(
                "UPDATE confirmed_occurrence SET validated_count=?2
                 WHERE ref=?1 AND validated_count IS NULL",
            )
            .map_err(|error| sql(&error))?
            .execute(params![key, count])
            .map_err(|error| sql(&error))?;
        if changed != 1 {
            return Err(IndexError::BackendViolation(
                "confirmed occurrence count requires one unvalidated row".into(),
            ));
        }
        budget.checkpoint()
    }

    /// Admit every exact row and its scratch allocation before durable `put_batch`.
    #[cfg(test)]
    pub(super) fn admit_confirmed_occurrences(
        &self,
        references: &[ObjectRef],
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        self.check_confirmed_attempt(attempt)?;
        check_batch_width(references.len())?;
        for (slot, reference) in references.iter().enumerate() {
            budget.checkpoint()?;
            if references
                .get(..slot)
                .is_some_and(|prior| prior.contains(reference))
            {
                return Err(IndexError::BackendViolation(
                    "confirmed occurrence scratch batch repeats an identity".into(),
                ));
            }
            let _key = reference_bytes(reference)?;
        }
        let rows = u64::try_from(references.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            writes: rows,
            bytes: rows.checked_mul(128).ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let demands = [128_u64; quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];
        quanta_index_scratch_sqlite::reserve_for_write_batch(
            &*self.db.scoped(budget)?,
            self.allocation.as_ref(),
            demands
                .get(..references.len())
                .ok_or(IndexError::ResourceLimited)?,
            budget,
        )
    }

    /// Call only after the complete `put_batch` output matches every identified leaf.
    #[cfg(test)]
    pub(super) fn record_confirmed_occurrences(
        &self,
        references: &[ObjectRef],
        attempt: AttemptRef,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        self.check_confirmed_attempt(attempt)?;
        check_batch_width(references.len())?;
        let io_scope = self.db.scoped(budget)?;
        let mut statement = io_scope
            .prepare_cached("INSERT OR IGNORE INTO confirmed_occurrence(ref) VALUES(?1)")
            .map_err(|error| sql(&error))?;
        for reference in references {
            budget.checkpoint()?;
            let key = reference_bytes(reference)?;
            let changed = statement
                .execute(params![key])
                .map_err(|error| sql(&error))?;
            if changed > 1 {
                return Err(IndexError::BackendViolation(
                    "confirmed occurrence scratch inserted multiple rows".into(),
                ));
            }
        }
        Ok(())
    }

    /// Only one changed owner's token rows are retained at a time.
    pub(super) fn reset_tokens(&self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        self.require_token_mutation_idle()?;
        budget.charge(WorkUsage {
            writes: 1,
            ..WorkUsage::default()
        })?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.db.scoped(budget)?,
            self.allocation.as_ref(),
            4096,
            budget,
        )?;
        self.db
            .scoped(budget)?
            .execute_batch("DELETE FROM tokens;")
            .map_err(|error| sql(&error))
    }

    pub(super) fn reset_inventories(&self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            writes: 1,
            ..WorkUsage::default()
        })?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &*self.db.scoped(budget)?,
            self.allocation.as_ref(),
            4096,
            budget,
        )?;
        self.db
            .scoped(budget)?
            .execute_batch("DELETE FROM inventory;")
            .map_err(|error| sql(&error))
    }

    pub(super) fn token_batch(
        &self,
        batch: &super::text::TokenBatch,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        const PREFIX: &str = "INSERT INTO tokens(term,pos) VALUES ";
        const ROW_SQL: &str = "(?,?)";
        self.require_token_mutation_idle()?;
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
            &*self.db.scoped(budget)?,
            self.allocation.as_ref(),
            demands,
            budget,
        )?;
        let sql_bytes = rows
            .len()
            .checked_mul(ROW_SQL.len())
            .and_then(|bytes| bytes.checked_add(rows.len().checked_sub(1)?))
            .and_then(|bytes| bytes.checked_add(PREFIX.len()))
            .ok_or(IndexError::ResourceLimited)?;
        let _sql_memory = budget.reserve_memory(
            u64::try_from(sql_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        let mut query = String::new();
        query
            .try_reserve_exact(sql_bytes)
            .map_err(|_error| IndexError::ResourceLimited)?;
        query.push_str(PREFIX);
        for slot in 0..rows.len() {
            if slot != 0 {
                query.push(',');
            }
            query.push_str(ROW_SQL);
        }
        let io_scope = self.db.scoped(budget)?;
        let mut insert = io_scope
            .prepare_cached(&query)
            .map_err(|error| sql(&error))?;
        insert.clear_bindings();
        let result = (|| {
            for (slot, row) in rows.iter().enumerate() {
                budget.checkpoint()?;
                let first = slot
                    .checked_mul(2)
                    .and_then(|value| value.checked_add(1))
                    .ok_or(IndexError::ResourceLimited)?;
                insert
                    .raw_bind_parameter(first, batch.term(row)?)
                    .map_err(|error| sql(&error))?;
                insert
                    .raw_bind_parameter(
                        first.checked_add(1).ok_or(IndexError::ResourceLimited)?,
                        row.position(),
                    )
                    .map_err(|error| sql(&error))?;
            }
            budget.checkpoint()?;
            if insert.raw_execute().map_err(|error| sql(&error))? != rows.len() {
                return Err(IndexError::BackendViolation(
                    "token insertion affected an unexpected row count".into(),
                ));
            }
            budget.checkpoint()?;
            Ok(())
        })();
        // Cached statements must not retain bound token copies on refusal.
        insert.clear_bindings();
        result
    }

    fn require_token_mutation_idle(&self) -> Result<(), IndexError> {
        if self.token_read_active.get() {
            return Err(IndexError::BackendViolation(
                "token table mutation during ordered token traversal".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn visit_token_groups(
        &self,
        limit: usize,
        budget: &mut WorkBudget,
        consume: impl FnMut(
            PreparedKey,
            super::text::PreparedPositions,
            &mut WorkBudget,
        ) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        token_groups::visit(self, limit, budget, consume)
    }

    /// Index keyset traversal needs no corpus-sized DISTINCT materialization.
    #[cfg(test)]
    pub(super) fn token_term_page(
        &self,
        after: Option<&[u8]>,
        budget: &mut WorkBudget,
    ) -> Result<TermPage, IndexError> {
        term_page(
            &*self.db.scoped(budget)?,
            TOKEN_TERM_PAGE_SQL,
            after,
            budget,
        )
    }

    #[cfg(test)]
    pub(super) fn positions(
        &self,
        term: &[u8],
        limit: usize,
        budget: &mut WorkBudget,
    ) -> Result<super::text::PreparedPositions, IndexError> {
        let mut result = super::text::PreparedPositions::new(limit)?;
        let io_scope = self.db.scoped(budget)?;
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
        edits: &[(InventoryPartition, MapEdit)],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        for (partition, _) in edits {
            let _root = partition.sql_root()?;
        }
        let demands = BatchDemands::new(edits, |(_, edit)| {
            u64::try_from(edit.key.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(map_field_reservation(2, 64)?)
                .ok_or(IndexError::ResourceLimited)
        })?;
        let db = self.db.scoped(budget)?;
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
                    rusqlite::types::ValueRef::Integer(root.sql_root()?),
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
            ready_checked: false,
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
            ready_checked: false,
        })
    }

    pub(super) fn posting_root_cursor<'a>(&'a self, term: &'a [u8]) -> RootCursor<'a> {
        RootCursor {
            spool: self,
            target: RootTarget::Posting(term),
            after: None,
            page: EditPage::default(),
            ready_checked: false,
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
        let db = self.db.scoped(budget)?;
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
        term_page(
            &*self.db.scoped(budget)?,
            POSTING_TERM_PAGE_SQL,
            after,
            budget,
        )
    }

    pub(super) fn posting_cardinalities(
        &self,
        terms: &TermPage,
        budget: &mut WorkBudget,
    ) -> Result<PostingCardinalities, IndexError> {
        if terms.is_empty()
            || terms.len() > PAGE
            || terms
                .windows(2)
                .any(|pair| matches!(pair, [left, right] if left.as_slice() >= right.as_slice()))
        {
            return Err(IndexError::CorruptObject(
                "posting term page is not ordered".into(),
            ));
        }
        let memory = budget.reserve_memory(
            u64::try_from(std::mem::size_of::<[PostingDelta; PAGE]>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            quanta_index_core::index::MemoryLifetime::Temporary,
        )?;
        // The original cardinality cost is one logical read per term. Charge
        // every read before the grouped SQL statement can touch the spool.
        for _ in terms.iter() {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })?;
        }
        let io_scope = self.db.scoped(budget)?;
        let mut statement = io_scope
            .prepare_cached(POSTING_CARDINALITY_PAGE_SQL)
            .map_err(|error| sql(&error))?;
        let first = terms.first().ok_or(IndexError::ResourceLimited)?;
        let last = terms.last().ok_or(IndexError::ResourceLimited)?;
        let mut rows = statement
            .query(params![first.as_slice(), last.as_slice()])
            .map_err(|error| sql(&error))?;
        let mut deltas = [PostingDelta::default(); PAGE];
        let mut count = 0usize;
        loop {
            budget.checkpoint()?;
            let Some(row) = rows.next().map_err(|error| sql(&error))? else {
                break;
            };
            let expected = terms.get(count).ok_or_else(|| {
                IndexError::CorruptObject("posting cardinality page has extra term".into())
            })?;
            let rusqlite::types::ValueRef::Blob(term) =
                row.get_ref(0).map_err(|error| sql(&error))?
            else {
                return Err(IndexError::CorruptObject(
                    "posting cardinality key is not a blob".into(),
                ));
            };
            if term != expected.as_slice() {
                return Err(IndexError::CorruptObject(
                    "posting cardinality page key mismatch".into(),
                ));
            }
            let added: i64 = row.get(1).map_err(|error| sql(&error))?;
            let removed: i64 = row.get(2).map_err(|error| sql(&error))?;
            let delta = deltas.get_mut(count).ok_or(IndexError::ResourceLimited)?;
            *delta = PostingDelta {
                added: u64::try_from(added).map_err(|_error| {
                    IndexError::CorruptObject("negative posting addition".into())
                })?,
                removed: u64::try_from(removed).map_err(|_error| {
                    IndexError::CorruptObject("negative posting removal".into())
                })?,
            };
            count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        if count != terms.len() {
            return Err(IndexError::CorruptObject(
                "posting cardinality page is incomplete".into(),
            ));
        }
        Ok(PostingCardinalities {
            deltas,
            len: count,
            _memory: memory,
        })
    }

    pub(super) fn posting_cursor(&self, terms: TermPage) -> PostingCursor<'_> {
        PostingCursor {
            spool: self,
            terms,
            validated: false,
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
        let db = self.db.scoped(budget)?;
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
        let db = self.db.scoped(budget)?;
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
    ready_checked: bool,
}

impl MapEditCursor for InventoryCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        if !self.ready_checked {
            self.spool.require_ready_inventories(budget)?;
            self.ready_checked = true;
        }
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
            let db = self.spool.db.scoped(budget)?;
            let mut statement = db.prepare_cached(query).map_err(|error| sql(&error))?;
            let roots = self.roots;
            self.page = EditPage::read(&mut statement, parameters, 4096, budget, |row, budget| {
                let raw: i64 = row.get(3).map_err(|error| sql(&error))?;
                let InventoryPartition::Ready(root) = InventoryPartition::from_sql(raw)? else {
                    return Err(IndexError::CorruptObject(
                        "inventory cursor encountered an unconfirmed row".into(),
                    ));
                };
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
    ready_checked: bool,
}
impl MapEditCursor for RootCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        if !self.ready_checked {
            if matches!(self.target, RootTarget::Inventory(_)) {
                self.spool.require_ready_inventories(budget)?;
            }
            self.ready_checked = true;
        }
        loop {
            budget.checkpoint()?;
            if let Some(edit) = self.page.pop()? {
                self.after = Some(edit.clone());
                return Ok(Some(edit));
            }
            if self.page.complete() {
                return Ok(None);
            }
            let db = self.spool.db.scoped(budget)?;
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
    validated: bool,
    after: Option<OrdinalMapEdit>,
    page: EditPage,
}

fn decode_posting_range_row(
    row: &rusqlite::Row<'_>,
    terms: &TermPage,
    previous: &mut Option<OrdinalMapEdit>,
    budget: &mut WorkBudget,
) -> Result<OrdinalMapEdit, IndexError> {
    let bound =
        usize::try_from(MAX_TERM_KEY_BYTES).map_err(|_error| IndexError::ResourceLimited)?;
    let term = quanta_index_scratch_sqlite::bounded_blob(
        row.get_ref(3).map_err(|error| sql(&error))?,
        bound,
    )?;
    check_term_key(term)?;
    let slot = terms
        .binary_search_by(|key| key.as_slice().cmp(term))
        .map_err(|_error| IndexError::CorruptObject("posting term outside selected page".into()))?;
    let root = u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?;
    let edit = read_edit(row, 32, root, budget)?;
    if edit.edit.key.len() != 32 {
        return Err(IndexError::CorruptObject(
            "posting owner key width differs".into(),
        ));
    }
    if let Some(prior) = previous.as_ref() {
        let prior_slot =
            usize::try_from(prior.root).map_err(|_error| IndexError::ResourceLimited)?;
        let next_slot = prior_slot
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if slot < prior_slot
            || slot > next_slot
            || (slot == prior_slot && edit.edit.key.as_slice() <= prior.edit.key.as_slice())
        {
            return Err(IndexError::CorruptObject(
                "posting range order or selected term is invalid".into(),
            ));
        }
    } else if slot != 0 {
        return Err(IndexError::CorruptObject(
            "posting range starts after selected term".into(),
        ));
    }
    *previous = Some(edit.clone());
    Ok(edit)
}

impl MapEditCursor for PostingCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        if !self.validated {
            if self.terms.is_empty() || self.terms.len() > PAGE {
                return Err(IndexError::CorruptObject(
                    "posting term page size is invalid".into(),
                ));
            }
            let mut prior: Option<&[u8]> = None;
            for term in self.terms.iter() {
                let key = term.as_slice();
                check_term_key(key)?;
                if prior.is_some_and(|previous| previous >= key) {
                    return Err(IndexError::CorruptObject(
                        "posting term page is not strictly ordered".into(),
                    ));
                }
                prior = Some(key);
            }
            self.validated = true;
        }
        loop {
            if let Some(edit) = self.page.pop()? {
                self.after = Some(edit.clone());
                return Ok(Some(edit));
            }
            if self.page.complete() {
                let last_slot = self
                    .terms
                    .len()
                    .checked_sub(1)
                    .ok_or(IndexError::ResourceLimited)?;
                let observed_slot = self
                    .after
                    .as_ref()
                    .map(|edit| usize::try_from(edit.root))
                    .transpose()
                    .map_err(|_error| IndexError::ResourceLimited)?;
                if observed_slot != Some(last_slot) {
                    return Err(IndexError::CorruptObject(
                        "posting range is missing a selected term".into(),
                    ));
                }
                return Ok(None);
            }
            let first = self.terms.first().ok_or(IndexError::ResourceLimited)?;
            let last = self.terms.last().ok_or(IndexError::ResourceLimited)?;
            let after = self
                .after
                .as_ref()
                .map(|edit| {
                    let slot =
                        usize::try_from(edit.root).map_err(|_error| IndexError::ResourceLimited)?;
                    let term = self.terms.get(slot).ok_or(IndexError::ResourceLimited)?;
                    Ok::<(&[u8], &[u8]), IndexError>((term.as_slice(), edit.edit.key.as_slice()))
                })
                .transpose()?;
            let db = self.spool.db.scoped(budget)?;
            let mut statement = db
                .prepare_cached(if after.is_some() {
                    POSTING_RANGE_NEXT_SQL
                } else {
                    POSTING_RANGE_FIRST_SQL
                })
                .map_err(|error| sql(&error))?;
            let mut previous = self.after.clone();
            let page = if let Some((resume_term, resume_key)) = after {
                EditPage::read(
                    &mut statement,
                    &[&resume_term, &resume_key, &last.as_slice()],
                    4096,
                    budget,
                    |row, budget| decode_posting_range_row(row, &self.terms, &mut previous, budget),
                )?
            } else {
                EditPage::read(
                    &mut statement,
                    &[&first.as_slice(), &last.as_slice()],
                    4096,
                    budget,
                    |row, budget| decode_posting_range_row(row, &self.terms, &mut previous, budget),
                )?
            };
            self.page = page;
        }
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
            let db = self.spool.db.scoped(budget)?;
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

    mod cardinality_pages;
    mod confirmed_batch;
    mod confirmed_occurrences;
    mod edit_pages;
    mod planned_inventory;
    mod planned_wide;
    mod posting_cursor_ranges;
    mod token_groups;

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
        let plan: String = spool.db.scoped(&work)?.query_row(
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
            .scoped(&work)?
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
                    InventoryPartition::Ready(root),
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
                    InventoryPartition::Ready(64),
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
                    .scoped(&work)?
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
                .scoped(&work)?
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
