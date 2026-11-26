//! Bounded disposable sort for native token preparation. Only immutable map
//! roots published through the catalog can become search authority.

use quanta_index_scratch_sqlite::{MapEditPage as EditPage, read_map_edit_budgeted as read_edit};

pub(super) use super::text_memory::{MAX_TERM_KEY_BYTES, check_term_key};
use super::text_inventory_descriptor::inventory_reference;
use super::text_memory::PreparedKey;
use super::text_occurrence::positions::{MAX_POSITIONS_PER_TERM, PreparedPositions};
use super::text_token_batch::TokenBatch;
use quanta_index_core::index::{
    AttemptRef, Digest, IndexError, MapEdit, MapValue, MemoryCreditLimits, MemoryLease,
    MemoryLifetime, ObjectRef, OrdinalMapEdit, ScratchAllocation, ScratchRequest, ScratchSpacePort,
    WorkBudget, WorkUsage,
    ports::{MAX_OBJECT_INSTALL_BATCH, MapEditCursor},
};
use quanta_index_scratch_sqlite::ScratchConnection;
#[cfg(test)]
use rusqlite::Connection;
use rusqlite::{OptionalExtension as _, params};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

mod confirmed_batch;
pub(super) use confirmed_batch::ConfirmedOccurrencePrograms;
mod packed_runs;
mod posting_case_pages;
mod term_anchors;
pub(super) use packed_runs::OrderedColdRun;
use packed_runs::{PackedRangeCursor, PostingPacketCache, PostingRow, PostingRuns};
mod token_groups;
pub(super) use posting_case_pages::PostingCasePages;

pub(super) const SCRATCH_MAX_BYTES: u64 = 4_294_967_296;
const SCRATCH_INITIAL_BYTES: u64 = 256 * 1024;
const PAGE: usize = 64;
pub(super) const MAX_INVENTORY_ROOTS: usize = MAX_OBJECT_INSTALL_BATCH;
// Keep the SQL partition boundary equal to the physical install envelope.
const INVENTORY_ROOT_LIMIT: u32 = 256;
const _: () = assert!(MAX_INVENTORY_ROOTS == 256);
#[cfg(test)]
const PLANNED_ROOT_OFFSET: u32 = INVENTORY_ROOT_LIMIT;
#[cfg(test)]
const PLANNED_ROOT_END: u32 = INVENTORY_ROOT_LIMIT * 2;
const INVENTORY_FIRST_SQL: &str = "SELECT k,p,n,root FROM inventory ORDER BY root,k LIMIT 64";
const INVENTORY_NEXT_SQL: &str =
    "SELECT k,p,n,root FROM inventory WHERE (root,k)>(?1,?2) ORDER BY root,k LIMIT 64";
const INVENTORY_ROOT_FIRST_SQL: &str =
    "SELECT k,p,n FROM inventory WHERE root=?1 AND k>=?2 ORDER BY k LIMIT 64";
const INVENTORY_ROOT_NEXT_SQL: &str =
    "SELECT k,p,n FROM inventory WHERE root=?1 AND k>?2 ORDER BY k LIMIT 64";
const TOP_NEXT_SQL: &str = "SELECT k,p,n FROM top WHERE surface=?1 AND k>?2 ORDER BY k LIMIT 64";
#[cfg(test)]
const TOKEN_TERM_PAGE_SQL: &str = "WITH RECURSIVE next(term,n) AS (
       SELECT (SELECT term FROM tokens WHERE term>?1 ORDER BY term LIMIT 1), 1
       UNION ALL
       SELECT (SELECT term FROM tokens WHERE term>next.term ORDER BY term LIMIT 1), n+1
       FROM next WHERE term IS NOT NULL AND n<64
     ) SELECT term FROM next WHERE term IS NOT NULL ORDER BY n";
pub(super) struct TextSpool {
    db: ScratchConnection,
    postings: PostingRuns,
    allocation: Box<dyn ScratchAllocation>,
    confirmed_attempt: Cell<Option<AttemptRef>>,
    token_read_active: Cell<bool>,
    inventory_read_active: Cell<bool>,
    inventory_packet: RefCell<PacketEvidence>,
    inventory_packet_memory: RefCell<Option<MemoryLease>>,
    inventory_packet_memory_reserved: Cell<bool>,
    inventory_receipt: Cell<Option<AttemptRef>>,
    inventory_poisoned: Cell<bool>,
    term_anchor_state: Cell<term_anchors::State>,
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
            Self::Ready(root) | Self::Planned(root) => root,
        };
        if root >= INVENTORY_ROOT_LIMIT {
            return Err(IndexError::InvalidInput(
                "inventory partition ordinal exceeds window".into(),
            ));
        }
        Ok(i64::from(root))
    }

    fn from_sql(raw: i64) -> Result<Self, IndexError> {
        let root = u32::try_from(raw)
            .map_err(|_error| IndexError::CorruptObject("negative inventory partition".into()))?;
        if root >= INVENTORY_ROOT_LIMIT {
            return Err(IndexError::CorruptObject(
                "inventory partition exceeds window".into(),
            ));
        }
        Ok(Self::Ready(root))
    }
}

/// Exact in-process witness of the current packet's successful SQL INSERTs.
///
/// The scratch connection is private and never handed to a concurrent writer.
/// Exact retained SQL witness; the sorted fixed backing avoids per-row scans.
struct PacketEvidence {
    rows: u64,
    references: [ObjectRef; MAX_OBJECT_INSTALL_BATCH],
    references_len: usize,
}

/// A single 64-row SQL batch is checked before any write. Its stack backing is
/// bounded and admitted as temporary memory when planned rows are present.
struct PacketDelta {
    rows: u64,
    references: [ObjectRef; PAGE],
    references_len: usize,
}

impl PacketEvidence {
    fn allocation_bytes() -> Result<u64, IndexError> {
        u64::try_from(std::mem::size_of::<Self>()).map_err(|_error| IndexError::ResourceLimited)
    }
}

impl PacketDelta {
    fn allocation_bytes() -> Result<u64, IndexError> {
        u64::try_from(std::mem::size_of::<Self>()).map_err(|_error| IndexError::ResourceLimited)
    }
}

impl PacketEvidence {
    fn empty() -> Self {
        Self {
            rows: 0,
            references: [ObjectRef(Digest([0; 32])); MAX_OBJECT_INSTALL_BATCH],
            references_len: 0,
        }
    }

    #[expect(
        clippy::expect_used,
        reason = "private sorted array length is checked before every post-SQL update"
    )]
    fn contains(&self, reference: ObjectRef) -> bool {
        self.references
            .get(..self.references_len)
            .expect("packet evidence length is within fixed backing")
            .binary_search(&reference)
            .is_ok()
    }

    fn check_delta(&self, delta: &PacketDelta) -> Result<(), IndexError> {
        let _rows = self
            .rows
            .checked_add(delta.rows)
            .ok_or(IndexError::ResourceLimited)?;
        if self
            .references_len
            .checked_add(delta.references_len)
            .ok_or(IndexError::ResourceLimited)?
            > MAX_OBJECT_INSTALL_BATCH
        {
            return Err(IndexError::ResourceLimited);
        }
        Ok(())
    }

    #[expect(
        clippy::expect_used,
        reason = "pre-SQL check_delta and the exclusive writer prove post-SQL fixed-array bounds"
    )]
    fn apply_delta(&mut self, delta: &PacketDelta) {
        // check_delta precedes SQL, and this exclusive writer is the only
        // updater before the successful SQL packet reaches this method.
        self.rows = self
            .rows
            .checked_add(delta.rows)
            .expect("packet row sum was checked before SQL");
        for reference in delta
            .references
            .get(..delta.references_len)
            .expect("delta length is bounded by its fixed backing")
        {
            let active = self
                .references
                .get(..self.references_len)
                .expect("evidence length was checked before SQL");
            let (Ok(slot) | Err(slot)) = active.binary_search(reference);
            let next = slot
                .checked_add(1)
                .expect("insertion slot is within backing");
            self.references.copy_within(slot..self.references_len, next);
            *self
                .references
                .get_mut(slot)
                .expect("insertion slot was checked before SQL") = *reference;
            self.references_len = self
                .references_len
                .checked_add(1)
                .expect("evidence capacity was checked before SQL");
        }
    }

    fn check(&self, refs: &[ObjectRef], expected_rows: u64) -> Result<(), IndexError> {
        if self.rows != expected_rows {
            return Err(IndexError::BackendViolation(
                "inventory confirmation row count differs".into(),
            ));
        }
        if self.references_len > refs.len() {
            return Err(IndexError::CorruptObject(
                "planned inventory has foreign occurrence".into(),
            ));
        }
        if self.references_len < refs.len() {
            return Err(IndexError::BackendViolation(
                "inventory confirmation reference cardinality differs".into(),
            ));
        }
        for reference in refs {
            if !self.contains(*reference) {
                return Err(IndexError::CorruptObject(
                    "planned inventory has foreign occurrence".into(),
                ));
            }
        }
        Ok(())
    }
}

impl PacketDelta {
    fn empty() -> Self {
        Self {
            rows: 0,
            references: [ObjectRef(Digest([0; 32])); PAGE],
            references_len: 0,
        }
    }

    fn include(
        &mut self,
        evidence: &PacketEvidence,
        reference: ObjectRef,
    ) -> Result<(), IndexError> {
        self.rows = self
            .rows
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if evidence.contains(reference) {
            return Ok(());
        }
        let active = self
            .references
            .get(..self.references_len)
            .ok_or(IndexError::ResourceLimited)?;
        let Err(slot) = active.binary_search(&reference) else {
            return Ok(());
        };
        if self.references_len >= PAGE {
            return Err(IndexError::ResourceLimited);
        }
        let next = slot.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        self.references.copy_within(slot..self.references_len, next);
        *self
            .references
            .get_mut(slot)
            .ok_or(IndexError::ResourceLimited)? = reference;
        self.references_len = self
            .references_len
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }
}

/// One exclusive physical-packet receipt. A cancelled or abandoned receipt
/// poisons the private scratch attempt; `journal_mode=OFF` supplies no rollback.
#[must_use]
pub(super) struct InventoryConfirmation<'a> {
    spool: &'a TextSpool,
    attempt: AttemptRef,
    db_changes: u64,
    record_confirmed: Option<confirmed_batch::ConfirmedOccurrenceRecord<'a>>,
    finished: bool,
}

/// One frozen inventory packet's global shape check. The private scratch
/// connection cannot be written through an inventory API while this lives.
#[must_use]
pub(super) struct ReadyInventoryReadLease<'a> {
    spool: &'a TextSpool,
    db_changes: u64,
}

pub(super) struct InventoryEditRef<'a> {
    pub(super) root: u32,
    pub(super) key: &'a [u8],
    pub(super) expected: Option<&'a [u8]>,
    pub(super) next: Option<&'a [u8]>,
}

#[derive(Clone, Copy)]
enum InventoryEditScope {
    All(u32),
    Root(u32),
}

pub(super) struct ReadyInventoryCursor<'lease, 'spool, C> {
    lease: &'lease ReadyInventoryReadLease<'spool>,
    cursor: C,
}

impl<C: MapEditCursor> MapEditCursor for ReadyInventoryCursor<'_, '_, C> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        self.lease.check_epoch(budget)?;
        self.cursor.next_edit(budget)
    }

    fn subtree_packing(
        &self,
    ) -> Result<quanta_index_core::index::ports::RootPackingPlan, IndexError> {
        self.cursor.subtree_packing()
    }
}

impl<'spool> ReadyInventoryReadLease<'spool> {
    pub(super) fn check_epoch(&self, budget: &WorkBudget) -> Result<(), IndexError> {
        // The wrapped cursor separately checks whether the packet is ready.
        budget.checkpoint()?;
        if self.spool.db.change_count()? != self.db_changes {
            self.spool.invalidate_inventory();
            return Err(IndexError::BackendViolation(
                "inventory scratch changed during frozen read".into(),
            ));
        }
        Ok(())
    }

    pub(super) fn cursor(
        &self,
        roots: usize,
    ) -> Result<ReadyInventoryCursor<'_, 'spool, InventoryCursor<'spool>>, IndexError> {
        let mut cursor = self.spool.inventory_cursor(roots)?;
        cursor.ready_checked = true;
        Ok(ReadyInventoryCursor {
            lease: self,
            cursor,
        })
    }

    pub(super) fn root_cursor(
        &self,
        root: usize,
    ) -> Result<ReadyInventoryCursor<'_, 'spool, RootCursor<'spool>>, IndexError> {
        self.root_cursor_for(root, 0)
    }

    pub(super) fn root_cursor_for(
        &self,
        root: usize,
        output_root: u32,
    ) -> Result<ReadyInventoryCursor<'_, 'spool, RootCursor<'spool>>, IndexError> {
        let mut cursor = self.spool.inventory_root_cursor(root)?;
        cursor.ready_checked = true;
        cursor.output_root = output_root;
        Ok(ReadyInventoryCursor {
            lease: self,
            cursor,
        })
    }

    /// Visit the same ordered SQL pages without materializing retained edits.
    /// Only the previous key survives advancing a row or opening another page.
    pub(super) fn visit_edits(
        &self,
        roots: usize,
        previous_key: &mut [u8],
        budget: &mut WorkBudget,
        visit: impl FnMut(InventoryEditRef<'_>, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        let roots = u32::try_from(roots).map_err(|_width| IndexError::ResourceLimited)?;
        self.visit_inventory_edits(InventoryEditScope::All(roots), previous_key, budget, visit)
    }

    pub(super) fn visit_root_edits(
        &self,
        slot: usize,
        previous_key: &mut [u8],
        budget: &mut WorkBudget,
        visit: impl FnMut(InventoryEditRef<'_>, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        let root = u32::try_from(slot).map_err(|_width| IndexError::ResourceLimited)?;
        if root >= INVENTORY_ROOT_LIMIT {
            return Err(IndexError::InvalidInput(
                "inventory root exceeds batch".into(),
            ));
        }
        self.visit_inventory_edits(InventoryEditScope::Root(root), previous_key, budget, visit)
    }

    fn visit_inventory_edits(
        &self,
        scope: InventoryEditScope,
        previous_key: &mut [u8],
        budget: &mut WorkBudget,
        mut visit: impl FnMut(InventoryEditRef<'_>, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        use quanta_index_scratch_sqlite::bounded_blob;
        use rusqlite::types::ValueRef;

        fn value_wire(value: ValueRef<'_>) -> Result<Option<&[u8]>, IndexError> {
            match value {
                ValueRef::Null => Ok(None),
                value @ (ValueRef::Integer(_)
                | ValueRef::Real(_)
                | ValueRef::Text(_)
                | ValueRef::Blob(_)) => {
                    bounded_blob(value, quanta_index_core::index::MAX_MAP_VALUE_WIRE_BYTES)
                        .map(Some)
                }
            }
        }

        let key_bound =
            usize::try_from(MAX_TERM_KEY_BYTES).map_err(|_width| IndexError::ResourceLimited)?;
        let mut previous_root = None;
        let mut previous_len = 0;
        loop {
            self.check_epoch(budget)?;
            self.spool.require_ready_packet()?;
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 4096,
                ..WorkUsage::default()
            })?;
            let db = self.spool.db.scoped(budget)?;
            let prior = previous_key
                .get(..previous_len)
                .ok_or(IndexError::ResourceLimited)?;
            let start = match scope {
                InventoryEditScope::All(_) => previous_root,
                InventoryEditScope::Root(root) => Some(root),
            };
            let bounds: [&dyn rusqlite::ToSql; 2] = [&start, &prior];
            let (query, parameters): (&str, &[&dyn rusqlite::ToSql]) =
                match (scope, previous_root.is_some()) {
                    (InventoryEditScope::All(_), false) => (INVENTORY_FIRST_SQL, &[]),
                    (InventoryEditScope::All(_), true) => (INVENTORY_NEXT_SQL, &bounds),
                    (InventoryEditScope::Root(_), false) => (INVENTORY_ROOT_FIRST_SQL, &bounds),
                    (InventoryEditScope::Root(_), true) => (INVENTORY_ROOT_NEXT_SQL, &bounds),
                };
            let mut statement = db.prepare_cached(query).map_err(|error| sql(&error))?;
            let mut rows = statement.query(parameters).map_err(|error| sql(&error))?;
            let mut count = 0;
            loop {
                self.check_epoch(budget)?;
                self.spool.require_ready_packet()?;
                let Some(row) = rows.next().map_err(|error| sql(&error))? else {
                    break;
                };
                if count != 0 {
                    budget.charge(WorkUsage {
                        reads: 1,
                        ..WorkUsage::default()
                    })?;
                }
                if count == PAGE {
                    return Err(IndexError::ResourceLimited);
                }
                let root = match scope {
                    InventoryEditScope::Root(_) => 0,
                    InventoryEditScope::All(roots) => {
                        let raw: i64 = row.get(3).map_err(|error| sql(&error))?;
                        let InventoryPartition::Ready(root) = InventoryPartition::from_sql(raw)?
                        else {
                            return Err(IndexError::CorruptObject(
                                "inventory cursor encountered an unconfirmed row".into(),
                            ));
                        };
                        if root >= roots {
                            return Err(IndexError::CorruptObject(
                                "inventory root ordinal out of bounds".into(),
                            ));
                        }
                        root
                    }
                };
                let key = bounded_blob(row.get_ref(0).map_err(|error| sql(&error))?, key_bound)?;
                check_term_key(key)?;
                let prior = previous_key
                    .get(..previous_len)
                    .ok_or(IndexError::ResourceLimited)?;
                if previous_root
                    .is_some_and(|previous| previous > root || (previous == root && prior >= key))
                {
                    return Err(IndexError::CorruptObject(
                        "global entry-map edits are foreign or unordered".into(),
                    ));
                }
                let expected = value_wire(row.get_ref(1).map_err(|error| sql(&error))?)?;
                let next = value_wire(row.get_ref(2).map_err(|error| sql(&error))?)?;
                visit(
                    InventoryEditRef {
                        root,
                        key,
                        expected,
                        next,
                    },
                    budget,
                )?;
                budget.charge(WorkUsage {
                    bytes: u64::try_from(key.len())
                        .map_err(|_width| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                previous_key
                    .get_mut(..key.len())
                    .ok_or(IndexError::ResourceLimited)?
                    .copy_from_slice(key);
                previous_len = key.len();
                previous_root = Some(root);
                count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            if count < PAGE {
                return Ok(());
            }
        }
    }
}

impl Drop for ReadyInventoryReadLease<'_> {
    fn drop(&mut self) {
        self.spool.inventory_read_active.set(false);
    }
}

impl InventoryConfirmation<'_> {
    pub(super) fn finish(mut self, budget: &WorkBudget) -> Result<(), IndexError> {
        budget.checkpoint()?;
        self.spool.check_inventory_receipt(self.attempt)?;
        let db = self.spool.db.scoped(budget)?;
        if db.total_changes() != self.db_changes {
            return Err(IndexError::BackendViolation(
                "inventory scratch changed during physical installation".into(),
            ));
        }
        if let Some(record) = self.record_confirmed.take() {
            record.finish(budget)?;
        }
        budget.checkpoint()?;
        *self.spool.inventory_packet.borrow_mut() = PacketEvidence::empty();
        self.spool.inventory_receipt.set(None);
        self.finished = true;
        Ok(())
    }
}

impl Drop for InventoryConfirmation<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.spool.inventory_poisoned.set(true);
        }
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

#[cfg(test)]
fn check_batch_width(rows: usize) -> Result<(), IndexError> {
    if rows == 0 || rows > quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS {
        return Err(IndexError::ResourceLimited);
    }
    Ok(())
}

struct RowDemands<const WIDTH: usize> {
    bytes: [u64; WIDTH],
    len: usize,
}

type BatchDemands = RowDemands<{ quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS }>;

/// The bounded page array and every copied SQL key have independent custody.
#[derive(Debug)]
pub(super) struct TermPage {
    keys: Vec<PreparedKey>,
    counts: Option<TermPageCounts>,
    memory: Option<quanta_index_core::index::MemoryLease>,
    case_ranges: bool,
}

/// Private DATA provenance for one sealed scratch source. It never grants
/// object or map publication authority and stays alive with derived pages.
#[derive(Debug)]
struct PostingPageSource {
    expected_changes: Cell<u64>,
    _memory: Option<MemoryLease>,
}

#[derive(Debug)]
struct TermPageCounts {
    deltas: [PostingDelta; PAGE],
    source: Rc<PostingPageSource>,
    digest: Option<[u8; 32]>,
    _memory: Option<MemoryLease>,
}

impl TermPage {
    pub(super) fn new() -> Self {
        Self {
            keys: Vec::new(),
            counts: None,
            memory: None,
            case_ranges: false,
        }
    }

    #[cfg(test)]
    pub(super) fn push(&mut self, bytes: &[u8], budget: &WorkBudget) -> Result<(), IndexError> {
        // A manually extended page must take the independent SQL path.
        self.counts = None;
        self.push_key(bytes, budget)
    }

    fn counted(source: Rc<PostingPageSource>, budget: &WorkBudget) -> Result<Self, IndexError> {
        let memory = budget.reserve_memory(
            u64::try_from(std::mem::size_of::<TermPageCounts>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        Ok(Self {
            counts: Some(TermPageCounts {
                deltas: [PostingDelta::default(); PAGE],
                source,
                digest: None,
                _memory: memory,
            }),
            ..Self::new()
        })
    }

    fn push_counted(
        &mut self,
        bytes: &[u8],
        delta: PostingDelta,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        if self
            .counts
            .as_ref()
            .is_none_or(|counts| counts.digest.is_some())
        {
            return Err(IndexError::BackendViolation(
                "term count page is not writable".into(),
            ));
        }
        let slot = self.keys.len();
        self.push_key(bytes, budget)?;
        *self
            .counts
            .as_mut()
            .ok_or(IndexError::ResourceLimited)?
            .deltas
            .get_mut(slot)
            .ok_or(IndexError::ResourceLimited)? = delta;
        Ok(())
    }

    fn push_from(
        &mut self,
        page: &Self,
        slot: usize,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        let source = page
            .counts
            .as_ref()
            .ok_or_else(|| IndexError::BackendViolation("case source lacks count data".into()))?;
        let destination = self.counts.as_ref().ok_or(IndexError::ResourceLimited)?;
        if source.digest.is_none() || !Rc::ptr_eq(&source.source, &destination.source) {
            return Err(IndexError::BackendViolation(
                "case count source differs".into(),
            ));
        }
        let key = page.keys.get(slot).ok_or(IndexError::ResourceLimited)?;
        let delta = *source.deltas.get(slot).ok_or(IndexError::ResourceLimited)?;
        self.push_counted(key.as_slice(), delta, budget)
    }

    fn sort_counted(&mut self) -> Result<(), IndexError> {
        let counts = self.counts.as_mut().ok_or(IndexError::ResourceLimited)?;
        // The fixed page has at most64 rows; keys and their counts move together.
        for slot in 1..self.keys.len() {
            let mut at = slot;
            while let Some(prior) = at.checked_sub(1) {
                let previous = self.keys.get(prior).ok_or(IndexError::ResourceLimited)?;
                let current = self.keys.get(at).ok_or(IndexError::ResourceLimited)?;
                if previous.as_slice() <= current.as_slice() {
                    break;
                }
                self.keys.swap(prior, at);
                counts.deltas.swap(prior, at);
                at = prior;
            }
        }
        Ok(())
    }

    fn push_key(&mut self, bytes: &[u8], budget: &WorkBudget) -> Result<(), IndexError> {
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

    fn case_cut(&self) -> Result<Option<usize>, IndexError> {
        if !self.case_ranges {
            return Ok(None);
        }
        let cut = self
            .keys
            .iter()
            .position(|key| key.as_slice().first() == Some(&1))
            .ok_or_else(|| {
                IndexError::CorruptObject("paired posting page lacks folded case".into())
            })?;
        if cut == 0
            || self
                .keys
                .get(..cut)
                .is_none_or(|keys| keys.iter().any(|key| key.as_slice().first() != Some(&0)))
            || self
                .keys
                .get(cut..)
                .is_none_or(|keys| keys.iter().any(|key| key.as_slice().first() != Some(&1)))
        {
            return Err(IndexError::CorruptObject(
                "paired posting page has invalid case ranges".into(),
            ));
        }
        Ok(Some(cut))
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

const INLINE_POSTING_TERM_BYTES: usize = 32;

enum PostingTermStorage {
    Inline {
        len: u8,
        bytes: [u8; INLINE_POSTING_TERM_BYTES],
    },
    Owned(PreparedKey),
}

pub(super) struct PostingTermKey(PostingTermStorage);

impl PostingTermKey {
    pub(super) fn copy(term: &[u8], budget: &WorkBudget) -> Result<Self, IndexError> {
        if term.len() > INLINE_POSTING_TERM_BYTES {
            return PreparedKey::copy(term, budget)
                .map(|owned| Self(PostingTermStorage::Owned(owned)));
        }
        budget.checkpoint()?;
        let len = u8::try_from(term.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let mut bytes = [0; INLINE_POSTING_TERM_BYTES];
        bytes
            .get_mut(..term.len())
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(term);
        Ok(Self(PostingTermStorage::Inline { len, bytes }))
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "inline length is checked by the private constructor"
    )]
    pub(super) fn as_slice(&self) -> &[u8] {
        match &self.0 {
            PostingTermStorage::Inline { len, bytes } => &bytes[..usize::from(*len)],
            PostingTermStorage::Owned(term) => term.as_slice(),
        }
    }
}

/// One bounded posting edit. The fixed owner and small term live in the
/// admitted row backing. Long terms retain their existing separate grant.
pub(super) struct PostingScratchRow {
    pub(super) term: PostingTermKey,
    pub(super) owner: [u8; 32],
    pub(super) expected: Option<MapValue>,
    pub(super) next: Option<MapValue>,
}

#[derive(Clone, Copy, Debug, Default)]
struct PostingDelta {
    added: u64,
    removed: u64,
}

/// Counts are data from the disposable spool, never posting-map authority.
pub(super) struct PostingCardinalities {
    deltas: [PostingDelta; PAGE],
    len: usize,
    page_digest: [u8; 32],
    changes: u64,
    _memory: Option<quanta_index_core::index::MemoryLease>,
}

/// Page and scratch-epoch binding for the original posting stream. Its lower
/// bound can select the existing authoritative dense fallback.
pub(super) struct DensePostingPrepassProof {
    page_digest: [u8; 32],
    changes: u64,
    lower_bound: u64,
}

impl PostingCardinalities {
    pub(super) fn dense_prepass_proof(&self) -> DensePostingPrepassProof {
        DensePostingPrepassProof {
            page_digest: self.page_digest,
            changes: self.changes,
            // Each physical edit contributes to at most one add and one
            // removal. Saturation remains strictly above the small prepass cap.
            lower_bound: self
                .deltas
                .iter()
                .take(self.len)
                .fold(0_u64, |total, delta| {
                    total.saturating_add(delta.added.max(delta.removed))
                }),
        }
    }

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

impl<const WIDTH: usize> RowDemands<WIDTH> {
    fn new<T>(
        rows: &[T],
        mut demand: impl FnMut(&T) -> Result<u64, IndexError>,
    ) -> Result<Self, IndexError> {
        if rows.is_empty() || rows.len() > WIDTH {
            return Err(IndexError::ResourceLimited);
        }
        let mut bytes = [0; WIDTH];
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

#[cfg(test)]
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
    /// Caller-local grants that may be born while preparing one planned
    /// inventory packet. SQL query backing has its separate Scratch bound.
    pub(super) fn inventory_confirmation_memory_bound(
        &self,
        maximum_refs: usize,
    ) -> Result<MemoryCreditLimits, IndexError> {
        let confirmation = confirmed_batch::confirmation_program_memory_bound(maximum_refs)?;
        let evidence = if self.inventory_packet_memory_reserved.get() {
            MemoryCreditLimits {
                temporary_data_bytes: 0,
                retained_data_bytes: 0,
                max_live_temporary_leases: 0,
                max_live_retained_leases: 0,
            }
        } else {
            MemoryCreditLimits {
                temporary_data_bytes: 0,
                retained_data_bytes: PacketEvidence::allocation_bytes()?,
                max_live_temporary_leases: 0,
                max_live_retained_leases: 1,
            }
        };
        confirmation
            .checked_add(evidence)?
            .checked_add(MemoryCreditLimits {
                temporary_data_bytes: PacketDelta::allocation_bytes()?,
                retained_data_bytes: 0,
                max_live_temporary_leases: 1,
                max_live_retained_leases: 0,
            })
    }
    pub(super) fn new(
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        Self::new_with_posting_order(scratch, budget, false)
    }

    pub(super) fn new_cold_postings(
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        Self::new_with_posting_order(scratch, budget, true)
    }

    fn new_with_posting_order(
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
        cold: bool,
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
             CREATE TABLE posting_packet(
                 phase INTEGER NOT NULL,run INTEGER NOT NULL,page INTEGER NOT NULL,
                 first_term BLOB NOT NULL,first_owner BLOB NOT NULL,
                 last_term BLOB NOT NULL,last_owner BLOB NOT NULL,payload BLOB NOT NULL,
                 PRIMARY KEY(phase,run,page)) WITHOUT ROWID;
             CREATE INDEX posting_packet_seek ON posting_packet(phase,run,last_term,last_owner);
             CREATE TABLE posting_term_packet(
                 seq INTEGER PRIMARY KEY,first_term BLOB NOT NULL,last_term BLOB NOT NULL,payload BLOB NOT NULL);
             CREATE INDEX posting_term_packet_seek ON posting_term_packet(last_term);
             CREATE TABLE top(surface INTEGER NOT NULL,k BLOB NOT NULL,p BLOB,n BLOB,
                 PRIMARY KEY(surface,k)) WITHOUT ROWID;
             CREATE TABLE confirmed_occurrence(
                 ref BLOB PRIMARY KEY, validated_count INTEGER) WITHOUT ROWID;
             CREATE TABLE confirmed_cold_root(
                 k BLOB PRIMARY KEY, root BLOB NOT NULL) WITHOUT ROWID;
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
            postings: PostingRuns::new(cold),
            allocation,
            confirmed_attempt: Cell::new(None),
            token_read_active: Cell::new(false),
            inventory_read_active: Cell::new(false),
            inventory_packet: RefCell::new(PacketEvidence::empty()),
            inventory_packet_memory: RefCell::new(None),
            inventory_packet_memory_reserved: Cell::new(false),
            inventory_receipt: Cell::new(None),
            inventory_poisoned: Cell::new(false),
            term_anchor_state: Cell::new(term_anchors::State::Empty),
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

    pub(super) fn invalidate_inventory(&self) {
        self.inventory_poisoned.set(true);
    }

    pub(super) fn ensure_inventory_usable(&self) -> Result<(), IndexError> {
        self.check_inventory_idle()
    }

    fn check_inventory_idle(&self) -> Result<(), IndexError> {
        if self.inventory_poisoned.get() {
            return Err(IndexError::BackendViolation(
                "inventory packet attempt was abandoned or failed".into(),
            ));
        }
        if self.inventory_receipt.get().is_some() {
            return Err(IndexError::BackendViolation(
                "inventory packet confirmation is pending".into(),
            ));
        }
        Ok(())
    }

    fn check_inventory_write_idle(&self) -> Result<(), IndexError> {
        self.check_inventory_idle()?;
        if self.inventory_read_active.get() {
            return Err(IndexError::BackendViolation(
                "inventory read lease blocks scratch mutation".into(),
            ));
        }
        Ok(())
    }

    fn check_posting_usable(&self) -> Result<(), IndexError> {
        self.postings.check()?;
        if self.inventory_poisoned.get() {
            return Err(IndexError::BackendViolation(
                "posting scratch attempt was poisoned".into(),
            ));
        }
        Ok(())
    }

    fn check_inventory_receipt(&self, attempt: AttemptRef) -> Result<(), IndexError> {
        if self.inventory_poisoned.get() {
            return self.check_inventory_idle();
        }
        if self.inventory_receipt.get() != Some(attempt) {
            return Err(IndexError::FenceLost);
        }
        self.check_confirmed_attempt(attempt)
    }

    fn require_ready_packet(&self) -> Result<(), IndexError> {
        self.check_inventory_idle()?;
        if self.inventory_packet.borrow().rows != 0 {
            return Err(IndexError::CorruptObject(
                "inventory has an unconfirmed packet".into(),
            ));
        }
        Ok(())
    }

    /// The only inventory writer is this spool. A live planned packet blocks
    /// every cursor before even an earlier ready row can escape.
    fn require_ready_inventories(&self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        self.require_ready_packet()?;
        budget.charge(WorkUsage {
            reads: 2,
            bytes: 8192,
            ..WorkUsage::default()
        })?;
        let db = self.db.scoped(budget)?;
        for (query, lower_bound) in [
            ("SELECT 1 FROM inventory WHERE root<0 LIMIT 1", None),
            (
                "SELECT 1 FROM inventory WHERE root>=?1 LIMIT 1",
                Some(i64::from(INVENTORY_ROOT_LIMIT)),
            ),
        ] {
            let mut statement = db.prepare_cached(query).map_err(|error| sql(&error))?;
            let found: Option<i64> = match lower_bound {
                Some(root) => statement.query_row([root], |row| row.get(0)),
                None => statement.query_row([], |row| row.get(0)),
            }
            .optional()
            .map_err(|error| sql(&error))?;
            if found.is_some() {
                return Err(IndexError::CorruptObject(
                    "inventory has an invalid root".into(),
                ));
            }
        }
        budget.checkpoint()
    }

    pub(super) fn begin_inventory_read(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<ReadyInventoryReadLease<'_>, IndexError> {
        if self.inventory_read_active.get() {
            return Err(IndexError::BackendViolation(
                "inventory read lease already active".into(),
            ));
        }
        self.require_ready_inventories(budget)?;
        let db_changes = self.db.change_count()?;
        self.inventory_read_active.set(true);
        Ok(ReadyInventoryReadLease {
            spool: self,
            db_changes,
        })
    }

    /// Bind one complete SQL-inserted packet to the expected physical outputs.
    /// The actual `put_batch` return is checked by the caller before finish.
    pub(super) fn admit_inventory_confirmation<'a>(
        &'a self,
        refs: &[ObjectRef],
        expected_rows: u64,
        attempt: AttemptRef,
        record_confirmed: bool,
        budget: &mut WorkBudget,
    ) -> Result<InventoryConfirmation<'a>, IndexError> {
        self.admit_inventory_confirmation_with_programs(
            refs,
            expected_rows,
            attempt,
            record_confirmed,
            None,
            budget,
        )
    }

    pub(super) fn admit_inventory_confirmation_with_programs<'a>(
        &'a self,
        refs: &[ObjectRef],
        expected_rows: u64,
        attempt: AttemptRef,
        record_confirmed: bool,
        programs: Option<ConfirmedOccurrencePrograms>,
        budget: &mut WorkBudget,
    ) -> Result<InventoryConfirmation<'a>, IndexError> {
        budget.checkpoint()?;
        self.check_inventory_write_idle()?;
        if programs.is_some() && !record_confirmed {
            return Err(IndexError::BackendViolation(
                "confirmation programs have no record".into(),
            ));
        }
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
        self.inventory_packet.borrow().check(refs, expected_rows)?;
        let db = self.db.scoped(budget)?;
        let record = if record_confirmed {
            Some(match programs {
                Some(programs) => {
                    confirmed_batch::ConfirmedOccurrenceRecord::prepare_with_programs(
                        self,
                        refs,
                        attempt,
                        Some(programs),
                        budget,
                    )?
                }
                None => confirmed_batch::ConfirmedOccurrenceRecord::prepare(
                    self, refs, attempt, budget,
                )?,
            })
        } else {
            None
        };
        let record_bytes = record
            .as_ref()
            .map(confirmed_batch::ConfirmedOccurrenceRecord::scratch_bytes)
            .transpose()?
            .unwrap_or(0);
        if record_bytes != 0 {
            quanta_index_scratch_sqlite::reserve_for_write(
                &db,
                self.allocation.as_ref(),
                record_bytes,
                budget,
            )?;
        }
        budget.checkpoint()?;
        let db_changes = db.total_changes();
        self.inventory_receipt.set(Some(attempt));
        Ok(InventoryConfirmation {
            spool: self,
            attempt,
            db_changes,
            record_confirmed: record,
            finished: false,
        })
    }

    /// A disposable same-attempt DATA index of completely installed cold
    /// dense roots. The caller still owns the content proof and catalog
    /// publication. Exact 32-byte keys and canonical object references are
    /// checked again on every SQL hit.
    pub(super) fn confirmed_cold_roots_lookup(
        &self,
        keys: &[Option<[u8; 32]>],
        found: &mut [Option<ObjectRef>],
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        self.check_posting_usable()?;
        self.postings.check_page_source(self)?;
        self.check_confirmed_attempt(attempt)?;
        if keys.len() != found.len() || keys.len() > PAGE {
            return Err(IndexError::InvalidInput(
                "confirmed cold root lookup exceeds page".into(),
            ));
        }
        let db = self.db.scoped(budget)?;
        let mut query = db
            .prepare_cached("SELECT k,root FROM confirmed_cold_root WHERE k=?1")
            .map_err(|error| sql(&error))?;
        for (key, output) in keys.iter().zip(found) {
            budget.checkpoint()?;
            if output.is_some() {
                return Err(IndexError::BackendViolation(
                    "confirmed cold root lookup output was not empty".into(),
                ));
            }
            let Some(key) = key else { continue };
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 65,
                ..WorkUsage::default()
            })?;
            let mut rows = query
                .query(params![key.as_slice()])
                .map_err(|error| sql(&error))?;
            if let Some(row) = rows.next().map_err(|error| sql(&error))? {
                let stored_key = quanta_index_scratch_sqlite::fixed_blob::<32>(
                    row.get_ref(0).map_err(|error| sql(&error))?,
                )?;
                if stored_key != *key {
                    return Err(IndexError::CorruptObject(
                        "confirmed cold root scratch key differs".into(),
                    ));
                }
                let wire = quanta_index_scratch_sqlite::fixed_blob::<33>(
                    row.get_ref(1).map_err(|error| sql(&error))?,
                )?;
                let MapValue::Object(root) = MapValue::decode(&wire)? else {
                    return Err(IndexError::CorruptObject(
                        "confirmed cold root scratch value differs".into(),
                    ));
                };
                if rows.next().map_err(|error| sql(&error))?.is_some() {
                    return Err(IndexError::CorruptObject(
                        "confirmed cold root scratch repeats a key".into(),
                    ));
                }
                *output = Some(root);
            }
        }
        budget.checkpoint()
    }

    /// Record only caller-verified install results, in one admitted scratch
    /// write group. A conflicting previously recorded identity is an error.
    pub(super) fn record_confirmed_cold_roots(
        &self,
        keys: &[Option<[u8; 32]>],
        roots: &[Option<ObjectRef>],
        attempt: AttemptRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        self.check_posting_usable()?;
        self.postings.check_page_source(self)?;
        self.check_confirmed_attempt(attempt)?;
        if keys.len() != roots.len() || keys.len() > PAGE {
            return Err(IndexError::InvalidInput(
                "confirmed cold root record exceeds page".into(),
            ));
        }
        let mut count = 0_u64;
        for (key, root) in keys.iter().zip(roots) {
            budget.checkpoint()?;
            match (key, root) {
                (Some(_), Some(root)) => {
                    let _wire = reference_fixed_bytes(root)?;
                    count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                }
                (None, None) => {}
                _ => {
                    return Err(IndexError::BackendViolation(
                        "confirmed cold root record loses an installed ordinal".into(),
                    ));
                }
            }
        }
        if count == 0 {
            return Ok(());
        }
        let db = self.db.scoped(budget)?;
        let scratch_bytes = count.checked_mul(128).ok_or(IndexError::ResourceLimited)?;
        quanta_index_scratch_sqlite::reserve_for_write(
            &db,
            self.allocation.as_ref(),
            scratch_bytes,
            budget,
        )?;
        budget.charge(WorkUsage {
            writes: count,
            bytes: scratch_bytes,
            ..WorkUsage::default()
        })?;
        let mut insert = db
            .prepare_cached("INSERT OR IGNORE INTO confirmed_cold_root(k,root) VALUES(?1,?2)")
            .map_err(|error| sql(&error))?;
        let mut existing = db
            .prepare_cached("SELECT root FROM confirmed_cold_root WHERE k=?1")
            .map_err(|error| sql(&error))?;
        for (key, root) in keys.iter().zip(roots) {
            budget.checkpoint()?;
            let (Some(key), Some(root)) = (key, root) else {
                continue;
            };
            let wire = reference_fixed_bytes(root)?;
            let changed = insert
                .execute(params![key.as_slice(), wire.as_slice()])
                .map_err(|error| sql(&error))?;
            if changed == 1 {
                continue;
            }
            if changed != 0 {
                return Err(IndexError::BackendViolation(
                    "confirmed cold root inserted multiple rows".into(),
                ));
            }
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 33,
                ..WorkUsage::default()
            })?;
            let mut rows = existing
                .query(params![key.as_slice()])
                .map_err(|error| sql(&error))?;
            let row = rows.next().map_err(|error| sql(&error))?.ok_or_else(|| {
                IndexError::CorruptObject("confirmed cold root scratch conflict vanished".into())
            })?;
            let stored = quanta_index_scratch_sqlite::fixed_blob::<33>(
                row.get_ref(0).map_err(|error| sql(&error))?,
            )?;
            if rows.next().map_err(|error| sql(&error))?.is_some() || stored != wire {
                return Err(IndexError::BackendViolation(
                    "confirmed cold posting identity changed".into(),
                ));
            }
        }
        self.postings.accept_top_changes(self)?;
        budget.checkpoint()
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
                        .is_ok_and(|count| count <= MAX_POSITIONS_PER_TERM) =>
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
        if count == 0 || count > MAX_POSITIONS_PER_TERM {
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
        self.check_inventory_write_idle()?;
        if self.inventory_packet.borrow().rows != 0 {
            return Err(IndexError::BackendViolation(
                "cannot reset an unconfirmed inventory packet".into(),
            ));
        }
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
        let result = self
            .db
            .scoped(budget)?
            .execute_batch("DELETE FROM inventory;")
            .map_err(|error| sql(&error));
        if result.is_err() {
            self.inventory_poisoned.set(true);
        }
        result
    }

    pub(super) fn token_batch(
        &self,
        batch: &TokenBatch,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.token_rows(
            batch.rows().len(),
            |slot| {
                let row = batch.rows().get(slot).ok_or(IndexError::ResourceLimited)?;
                Ok((batch.term(row)?, row.position()))
            },
            budget,
        )
    }

    /// Insert a bounded page borrowed from a validated canonical token arena.
    /// SQL row admission and bindings are identical to `token_batch`.
    pub(super) fn token_batch_borrowed(
        &self,
        rows: &[(&[u8], u32)],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.token_rows(
            rows.len(),
            |slot| rows.get(slot).copied().ok_or(IndexError::ResourceLimited),
            budget,
        )
    }

    fn token_rows<'a>(
        &self,
        row_count: usize,
        mut row_at: impl FnMut(usize) -> Result<(&'a [u8], u32), IndexError>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        use std::io::Write as _;

        const PREFIX: &str = "INSERT INTO tokens(term,pos) VALUES ";
        const ROW_SQL: &str = "(?,?)";
        const MAX_SQL_BYTES: usize = PREFIX.len()
            + quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS * ROW_SQL.len()
            + quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS
            - 1;
        self.require_token_mutation_idle()?;
        let slots: [usize; quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS] =
            std::array::from_fn(|slot| slot);
        let demands = BatchDemands::new(
            slots.get(..row_count).ok_or(IndexError::ResourceLimited)?,
            |slot| {
                u64::try_from(row_at(*slot)?.0.len())
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .checked_add(64)
                    .ok_or(IndexError::ResourceLimited)
            },
        )?;
        let demands = demands.as_slice()?;
        budget.charge(WorkUsage {
            writes: u64::try_from(row_count).map_err(|_error| IndexError::ResourceLimited)?,
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
        // This fixed ASCII template is bounded by the 64-row SQL envelope.
        // It needs no second heap grant while a cold arena remains live.
        let mut query = [0_u8; MAX_SQL_BYTES];
        let remaining = (|| -> std::io::Result<usize> {
            let mut tail = query.as_mut_slice();
            tail.write_all(PREFIX.as_bytes())?;
            for slot in 0..row_count {
                if slot != 0 {
                    tail.write_all(b",")?;
                }
                tail.write_all(ROW_SQL.as_bytes())?;
            }
            Ok(tail.len())
        })()
        .map_err(|_error| {
            IndexError::BackendViolation("token SQL template exceeds bound".into())
        })?;
        let query_bytes = MAX_SQL_BYTES
            .checked_sub(remaining)
            .and_then(|length| query.get(..length))
            .ok_or(IndexError::ResourceLimited)?;
        let query = std::str::from_utf8(query_bytes).map_err(|_error| {
            IndexError::BackendViolation("token SQL template is not ASCII".into())
        })?;
        let io_scope = self.db.scoped(budget)?;
        let mut insert = io_scope
            .prepare_cached(query)
            .map_err(|error| sql(&error))?;
        insert.clear_bindings();
        let result = (|| {
            for slot in 0..row_count {
                budget.checkpoint()?;
                let (term, position) = row_at(slot)?;
                let first = slot
                    .checked_mul(2)
                    .and_then(|value| value.checked_add(1))
                    .ok_or(IndexError::ResourceLimited)?;
                insert
                    .raw_bind_parameter(first, term)
                    .map_err(|error| sql(&error))?;
                insert
                    .raw_bind_parameter(
                        first.checked_add(1).ok_or(IndexError::ResourceLimited)?,
                        position,
                    )
                    .map_err(|error| sql(&error))?;
            }
            budget.checkpoint()?;
            if insert.raw_execute().map_err(|error| sql(&error))? != row_count {
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
            PreparedPositions,
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
    ) -> Result<PreparedPositions, IndexError> {
        let mut result = PreparedPositions::new(limit)?;
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
        self.check_inventory_write_idle()?;
        let has_planned = edits
            .iter()
            .any(|(partition, _)| matches!(partition, InventoryPartition::Planned(_)));
        let _delta_memory = if has_planned {
            budget.reserve_memory(PacketDelta::allocation_bytes()?, MemoryLifetime::Temporary)?
        } else {
            None
        };
        let mut delta = PacketDelta::empty();
        for (partition, edit) in edits {
            let _root = partition.sql_root()?;
            if edit.expected == edit.next {
                return Err(IndexError::BackendViolation(
                    "inventory scratch contains an unchanged edit".into(),
                ));
            }
            match edit.expected {
                None | Some(MapValue::Object(_)) => {}
                Some(MapValue::Inline(_)) => {
                    return Err(IndexError::CorruptObject(
                        "inventory prior is not an occurrence reference".into(),
                    ));
                }
            }
            if matches!(partition, InventoryPartition::Planned(_)) {
                check_term_key(&edit.key)?;
                let next = edit.next.ok_or_else(|| {
                    IndexError::CorruptObject(
                        "planned inventory lacks an occurrence reference".into(),
                    )
                })?;
                let reference = inventory_reference(next)?;
                delta.include(&self.inventory_packet.borrow(), reference)?;
            } else if let Some(next) = edit.next {
                let _reference = inventory_reference(next)?;
            }
        }
        self.inventory_packet.borrow().check_delta(&delta)?;
        if has_planned && !self.inventory_packet_memory_reserved.get() {
            let memory = budget.reserve_memory(
                PacketEvidence::allocation_bytes()?,
                MemoryLifetime::Retained,
            )?;
            *self.inventory_packet_memory.borrow_mut() = memory;
            self.inventory_packet_memory_reserved.set(true);
        }
        let demands = BatchDemands::new(edits, |(_, edit)| {
            u64::try_from(edit.key.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_add(map_field_reservation(2, 64)?)
                .ok_or(IndexError::ResourceLimited)
        })?;
        let db = self.db.scoped(budget)?;
        let before = db.total_changes();
        let result = quanta_index_scratch_sqlite::write_map_edit_batch(
            &db,
            self.allocation.as_ref(),
            "INSERT INTO inventory(root,k,p,n) VALUES ",
            4096,
            demands.as_slice()?,
            |slot| {
                let (root, edit) = edits.get(slot).ok_or(IndexError::ResourceLimited)?;
                Ok((
                    rusqlite::types::ValueRef::Integer(root.sql_root()?),
                    edit,
                    0,
                ))
            },
            budget,
        )
        .and_then(|()| {
            if db.total_changes().checked_sub(before)
                != Some(u64::try_from(edits.len()).map_err(|_error| IndexError::ResourceLimited)?)
            {
                return Err(IndexError::BackendViolation(
                    "inventory SQL insert count changed outside the bound packet".into(),
                ));
            }
            Ok(())
        });
        match &result {
            Err(_) => self.inventory_poisoned.set(true),
            Ok(()) => self.inventory_packet.borrow_mut().apply_delta(&delta),
        }
        result
    }

    pub(super) fn inventory_cursor(&self, roots: usize) -> Result<InventoryCursor<'_>, IndexError> {
        if roots == 0 || roots > MAX_INVENTORY_ROOTS {
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
        if root >= MAX_INVENTORY_ROOTS {
            return Err(IndexError::InvalidInput(
                "inventory root exceeds batch".into(),
            ));
        }
        Ok(RootCursor {
            spool: self,
            root: u32::try_from(root).map_err(|_error| IndexError::ResourceLimited)?,
            output_root: 0,
            after: None,
            page: EditPage::default(),
            ready_checked: false,
        })
    }

    /// The original prepass entry point remains available to focused tests.
    #[cfg(test)]
    pub(super) fn posting_root_cursors<'a>(
        &'a self,
        terms: &TermPage,
        eligible: &[bool],
        budget: &mut WorkBudget,
    ) -> Result<PostingRootCursors<'a>, IndexError> {
        self.packed_root_cursors(terms, eligible, None, budget)
    }

    /// Sparse pages use one authenticated prepass; provably dense pages use
    /// the original per-root cursor. The global cursor rechecks edits and EOF.
    pub(super) fn posting_root_cursors_with_proof<'a>(
        &'a self,
        terms: &TermPage,
        eligible: &[bool],
        proof: &DensePostingPrepassProof,
        budget: &mut WorkBudget,
    ) -> Result<PostingRootCursors<'a>, IndexError> {
        self.packed_root_cursors(terms, eligible, Some(proof), budget)
    }

    pub(super) fn posting_write_batch_rows(
        &self,
        budget: &WorkBudget,
    ) -> Result<usize, IndexError> {
        self.check_posting_usable()?;
        let db = self.db.scoped(budget)?;
        quanta_index_scratch_sqlite::max_supported_wide_edit_batch_rows(&db)
    }

    fn posting_batch_fields<T>(
        &self,
        rows: &[T],
        fields: impl for<'a> Fn(&'a T) -> (&'a [u8], quanta_index_scratch_sqlite::MapEditFields<'a>),
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.postings.append(self, rows, fields, budget)
    }

    pub(super) fn posting_batch_rows(
        &self,
        rows: &[PostingScratchRow],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.posting_batch_fields(
            rows,
            |row| {
                (
                    row.term.as_slice(),
                    quanta_index_scratch_sqlite::MapEditFields {
                        key: &row.owner,
                        expected: row.expected.as_ref(),
                        next: row.next.as_ref(),
                    },
                )
            },
            budget,
        )
    }

    pub(super) fn begin_ordered_cold_run(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<Option<OrderedColdRun<'_>>, IndexError> {
        self.postings.begin_ordered_cold_run(self, budget)
    }

    pub(super) fn finish_owner_major_posting_run(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.postings.finish_owner_major_run(self, budget)
    }

    pub(super) fn prepare_ordered_cold_run_parent(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        self.postings.prepare_ordered_cold_run_parent(self, budget)
    }

    pub(super) fn ordered_cold_run_credit_limits() -> Result<MemoryCreditLimits, IndexError> {
        PostingRuns::ordered_cold_run_credit_limits()
    }

    #[cfg(test)]
    pub(in crate::native) fn posting_packet_layout(
        &self,
        budget: &WorkBudget,
    ) -> Result<Vec<(i64, i64, i64)>, IndexError> {
        let db = self.db.scoped(budget)?;
        let mut statement = db
            .prepare("SELECT phase,run,page FROM posting_packet ORDER BY phase,run,page")
            .map_err(|error| sql(&error))?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|error| sql(&error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql(&error))
    }

    #[cfg(test)]
    pub(super) fn posting_batch(
        &self,
        rows: &[(Vec<u8>, MapEdit)],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.posting_batch_fields(rows, |(term, edit)| (term.as_slice(), edit.into()), budget)
    }

    pub(super) fn posting_term_page(
        &self,
        after: Option<&[u8]>,
        budget: &mut WorkBudget,
    ) -> Result<TermPage, IndexError> {
        self.check_posting_usable()?;
        self.packed_term_page(after, None, budget)
    }

    pub(super) fn posting_cardinalities(
        &self,
        terms: &TermPage,
        budget: &mut WorkBudget,
    ) -> Result<PostingCardinalities, IndexError> {
        self.check_posting_usable()?;
        self.packed_cardinalities(terms, budget)
    }

    pub(super) fn posting_cursor(&self, terms: TermPage) -> PostingCursor<'_> {
        self.posting_cursor_with_packets(terms, None)
    }

    fn posting_cursor_with_packets<'a>(
        &'a self,
        terms: TermPage,
        cache: Option<&'a PostingPacketCache>,
    ) -> PostingCursor<'a> {
        PostingCursor {
            spool: self,
            terms,
            cache,
            validated: false,
            case_cut: None,
            range_start: 0,
            range_exhausted: false,
            after: None,
            range: None,
            finished: false,
        }
    }

    pub(super) fn top_batch(
        &self,
        rows: &[(u8, MapEdit)],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.check_posting_usable()?;
        self.postings.check_page_source(self)?;
        let demands = BatchDemands::new(rows, |(_surface, edit)| {
            let key =
                u64::try_from(edit.key.len()).map_err(|_error| IndexError::ResourceLimited)?;
            key.checked_add(map_field_reservation(2, 64)?)
                .ok_or(IndexError::ResourceLimited)
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
                Ok((
                    rusqlite::types::ValueRef::Integer(i64::from(*surface)),
                    edit,
                    0,
                ))
            },
            budget,
        )?;
        self.postings.accept_top_changes(self)?;
        Ok(())
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
        if self.ready_checked {
            self.spool.require_ready_packet()?;
        } else {
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
            let prior_root = self.after.as_ref().map(OrdinalMapEdit::root);
            let prior_key = self
                .after
                .as_ref()
                .map(OrdinalMapEdit::key)
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
                check_term_key(edit.key())?;
                Ok(edit)
            })?;
        }
    }
}

/// One immutable inventory root tagged with its output map ordinal.
pub(super) struct RootCursor<'a> {
    spool: &'a TextSpool,
    root: u32,
    output_root: u32,
    after: Option<OrdinalMapEdit>,
    page: EditPage,
    ready_checked: bool,
}
impl MapEditCursor for RootCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        if self.ready_checked {
            self.spool.require_ready_packet()?;
        } else {
            self.spool.require_ready_inventories(budget)?;
        }
        self.ready_checked = true;
        loop {
            budget.checkpoint()?;
            if let Some(edit) = self.page.pop()? {
                self.after = Some(edit.clone());
                return Ok(Some(edit));
            }
            if self.page.complete() {
                return Ok(None);
            }
            let root = self.root;
            let db = self.spool.db.scoped(budget)?;
            let after = self.after.as_ref().map(OrdinalMapEdit::key);
            let query = if after.is_some() {
                INVENTORY_ROOT_NEXT_SQL
            } else {
                INVENTORY_ROOT_FIRST_SQL
            };
            let mut statement = db.prepare_cached(query).map_err(|error| sql(&error))?;
            self.page = EditPage::read(
                &mut statement,
                &[&root, &after.unwrap_or_default()],
                4096,
                budget,
                |row, budget| {
                    let edit = read_edit(
                        row,
                        usize::try_from(MAX_TERM_KEY_BYTES)
                            .map_err(|_error| IndexError::ResourceLimited)?,
                        self.output_root,
                        budget,
                    )?;
                    check_term_key(edit.key())?;
                    Ok(edit)
                },
            )?;
        }
    }
}

pub(super) struct PostingRootCursors<'a> {
    spool: &'a TextSpool,
    prepared_changes: Option<u64>,
    keys: [Option<PreparedKey>; PAGE],
    lengths: [u8; PAGE],
    offsets: [usize; PAGE],
    rows: Vec<Option<OrdinalMapEdit>>,
    row_memory: Option<quanta_index_core::index::MemoryLease>,
    shared: Rc<RefCell<SharedRange<'a>>>,
}

impl<'a> PostingRootCursors<'a> {
    fn fallback(spool: &'a TextSpool, budget: &WorkBudget) -> Result<Self, IndexError> {
        let memory = budget.reserve_memory(
            u64::try_from(
                std::mem::size_of::<SharedRange<'a>>()
                    .checked_add(
                        std::mem::size_of::<usize>()
                            .checked_mul(2)
                            .ok_or(IndexError::ResourceLimited)?,
                    )
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        Ok(Self {
            spool,
            prepared_changes: None,
            keys: std::array::from_fn(|_| None),
            lengths: [0; PAGE],
            offsets: [0; PAGE],
            rows: Vec::new(),
            row_memory: None,
            shared: Rc::new(RefCell::new(SharedRange::new(spool, memory))),
        })
    }

    pub(super) fn cursor(
        &mut self,
        slot: usize,
        term: &'a [u8],
    ) -> Result<PostingRootCursor<'a>, IndexError> {
        self.spool.check_posting_usable()?;
        if let Some(changes) = self.prepared_changes {
            self.spool.check_posting_epoch(changes)?;
        }
        if slot >= PAGE {
            return Err(IndexError::ResourceLimited);
        }
        let Some(key) = self.keys.get(slot).and_then(Option::as_ref) else {
            check_term_key(term)?;
            let mut shared = self.shared.borrow_mut();
            if shared.poisoned || shared.active.is_some() {
                return Err(IndexError::BackendViolation(
                    "shared posting root cursor is incomplete".into(),
                ));
            }
            let floor = shared.retry_slot.or(shared.last_completed);
            if let Some(floor) = floor {
                if slot < floor {
                    return Err(IndexError::CorruptObject(
                        "posting root cursor moved backward".into(),
                    ));
                }
                if let Some(witness_slot) = shared.witness_slot {
                    let same_replay = slot == floor && witness_slot == floor;
                    if same_replay {
                        if shared.witness_term()? != term {
                            return Err(IndexError::CorruptObject(
                                "posting root replay term differs".into(),
                            ));
                        }
                    } else if slot > witness_slot && term <= shared.witness_term()? {
                        return Err(IndexError::CorruptObject(
                            "posting root order differs".into(),
                        ));
                    }
                }
            }
            let crossed_case = shared.witness_slot.is_some_and(|previous| slot > previous)
                && shared.witness_term()?.first() != term.first();
            if shared.last_completed == Some(slot) || shared.retry_slot == Some(slot) {
                shared.cursor.clear();
                shared.lookahead = None;
            }
            shared.retry_slot = None;
            shared.active = Some(slot);
            drop(shared);
            return Ok(PostingRootCursor::Shared(SharedPostingRootCursor {
                spool: self.spool,
                shared: Rc::clone(&self.shared),
                slot,
                term,
                seek_surface: crossed_case,
                seen: false,
                finished: false,
            }));
        };
        if key.as_slice() != term {
            return Err(IndexError::CorruptObject(
                "sparse posting cursor term differs from selected slot".into(),
            ));
        }
        if self.shared.borrow().active.is_some() {
            return Err(IndexError::BackendViolation(
                "posting root cursor is incomplete".into(),
            ));
        }
        let len = usize::from(std::mem::take(
            self.lengths
                .get_mut(slot)
                .ok_or(IndexError::ResourceLimited)?,
        ));
        if len == 0 {
            return Err(IndexError::BackendViolation(
                "sparse posting cursor was consumed twice".into(),
            ));
        }
        let mut rows: [Option<OrdinalMapEdit>; 8] = std::array::from_fn(|_| None);
        for (index, output) in rows.iter_mut().enumerate().take(len) {
            let offset = self
                .offsets
                .get(slot)
                .ok_or(IndexError::ResourceLimited)?
                .checked_add(index)
                .ok_or(IndexError::ResourceLimited)?;
            *output = self
                .rows
                .get_mut(offset)
                .ok_or(IndexError::ResourceLimited)?
                .take();
            if output.is_none() {
                return Err(IndexError::BackendViolation(
                    "sparse posting prepass lost a row".into(),
                ));
            }
        }
        Ok(PostingRootCursor::Prefetched {
            spool: self.spool,
            changes: self.prepared_changes.ok_or(IndexError::ResourceLimited)?,
            rows,
            next: 0,
            len,
        })
    }
}

struct SharedRange<'a> {
    spool: &'a TextSpool,
    cursor: Vec<PackedRangeCursor<'a>>,
    lookahead: Option<PostingRow>,
    active: Option<usize>,
    retry_slot: Option<usize>,
    witness: [u8; 1024],
    witness_len: usize,
    witness_slot: Option<usize>,
    last_completed: Option<usize>,
    db_changes: Option<u64>,
    poisoned: bool,
    _memory: Option<MemoryLease>,
    cursor_memory: Option<MemoryLease>,
}

impl<'a> SharedRange<'a> {
    fn new(spool: &'a TextSpool, memory: Option<MemoryLease>) -> Self {
        Self {
            spool,
            cursor: Vec::new(),
            lookahead: None,
            active: None,
            retry_slot: None,
            witness: [0; 1024],
            witness_len: 0,
            witness_slot: None,
            last_completed: None,
            db_changes: None,
            poisoned: false,
            _memory: memory,
            cursor_memory: None,
        }
    }
    fn witness_term(&self) -> Result<&[u8], IndexError> {
        self.witness
            .get(..self.witness_len)
            .ok_or(IndexError::ResourceLimited)
    }
    fn bind_active_term(&mut self, term: &[u8], budget: &mut WorkBudget) -> Result<(), IndexError> {
        let slot = self.active.ok_or(IndexError::ResourceLimited)?;
        if self.witness_slot == Some(slot) {
            if self.witness_term()? != term {
                return Err(IndexError::CorruptObject(
                    "posting active root term differs".into(),
                ));
            }
            return Ok(());
        }
        if term.len() > self.witness.len() {
            return Err(IndexError::ResourceLimited);
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(term.len()).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.witness
            .get_mut(..term.len())
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(term);
        self.witness_len = term.len();
        self.witness_slot = Some(slot);
        Ok(())
    }
    fn check_epoch(&mut self, budget: &WorkBudget) -> Result<(), IndexError> {
        self.spool.check_posting_usable()?;
        budget.checkpoint()?;
        let changes = self.spool.db.change_count()?;
        match self.db_changes {
            Some(previous) if previous != changes => {
                self.poisoned = true;
                Err(IndexError::BackendViolation(
                    "posting scratch changed during shared read".into(),
                ))
            }
            Some(_) => Ok(()),
            None => {
                self.db_changes = Some(changes);
                Ok(())
            }
        }
    }
    fn complete(&mut self, slot: usize, term: &[u8]) -> Result<(), IndexError> {
        if self.witness_slot != Some(slot) || self.witness_term()? != term {
            return Err(IndexError::CorruptObject(
                "posting completed root term differs".into(),
            ));
        }
        self.last_completed = Some(slot);
        self.active = None;
        Ok(())
    }
}

pub(super) struct SharedPostingRootCursor<'a> {
    spool: &'a TextSpool,
    shared: Rc<RefCell<SharedRange<'a>>>,
    slot: usize,
    term: &'a [u8],
    seek_surface: bool,
    seen: bool,
    finished: bool,
}

impl MapEditCursor for SharedPostingRootCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        let result = (|| {
            self.spool.check_posting_usable()?;
            budget.checkpoint()?;
            if self.finished {
                return Ok(None);
            }
            let mut shared = self.shared.borrow_mut();
            if shared.poisoned || shared.active != Some(self.slot) {
                return Err(IndexError::BackendViolation(
                    "posting shared root lease differs".into(),
                ));
            }
            shared.check_epoch(budget)?;
            if self.seek_surface {
                if let Some(cursor) = shared.cursor.first_mut() {
                    cursor.reposition(self.term, budget)?;
                }
                shared.lookahead = None;
                self.seek_surface = false;
            }
            shared.bind_active_term(self.term, budget)?;
            if shared.cursor.is_empty() {
                if shared.cursor.capacity() == 0 {
                    let backing = std::mem::size_of::<PackedRangeCursor<'_>>();
                    let memory = budget.reserve_memory(
                        u64::try_from(backing).map_err(|_error| IndexError::ResourceLimited)?,
                        MemoryLifetime::Temporary,
                    )?;
                    shared
                        .cursor
                        .try_reserve_exact(1)
                        .map_err(|_error| IndexError::ResourceLimited)?;
                    if shared.cursor.capacity() != 1 {
                        shared.cursor = Vec::new();
                        return Err(IndexError::ResourceLimited);
                    }
                    shared.cursor_memory = memory;
                }
                shared
                    .cursor
                    .push(PackedRangeCursor::new(self.spool, self.term, budget)?);
            }
            loop {
                if shared.lookahead.is_none() {
                    shared.lookahead = shared
                        .cursor
                        .get_mut(0)
                        .ok_or(IndexError::ResourceLimited)?
                        .next(budget)?;
                }
                match shared.lookahead.as_ref() {
                    Some(row) if row.term.as_slice() < self.term => {
                        // A forward-only root factory may skip Tree slots; MapInput
                        // separately authenticates the complete original stream.
                        shared.lookahead = None;
                    }
                    Some(row) if row.term.as_slice() == self.term => {
                        let row = shared.lookahead.take().ok_or(IndexError::ResourceLimited)?;
                        self.seen = true;
                        return row.ordinal(0, budget).map(Some);
                    }
                    Some(_) | None => {
                        if !self.seen {
                            return Err(IndexError::CorruptObject(
                                "posting root is missing its edit".into(),
                            ));
                        }
                        shared.complete(self.slot, self.term)?;
                        self.finished = true;
                        return Ok(None);
                    }
                }
            }
        })();
        if result.is_err() {
            self.shared.borrow_mut().poisoned = true;
        }
        result
    }
}

impl Drop for SharedPostingRootCursor<'_> {
    fn drop(&mut self) {
        if !self.finished
            && let Ok(mut shared) = self.shared.try_borrow_mut()
            && !shared.poisoned
        {
            // A warm-page probe may legitimately stop after one row. A
            // subsequent same-slot request restarts with full Work charge.
            shared.cursor.clear();
            shared.lookahead = None;
            shared.active = None;
            shared.retry_slot = Some(self.slot);
        }
    }
}

pub(super) enum PostingRootCursor<'a> {
    Shared(SharedPostingRootCursor<'a>),
    Prefetched {
        spool: &'a TextSpool,
        changes: u64,
        rows: [Option<OrdinalMapEdit>; 8],
        next: usize,
        len: usize,
    },
}

impl MapEditCursor for PostingRootCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        match self {
            Self::Shared(cursor) => cursor.next_edit(budget),
            Self::Prefetched {
                spool,
                changes,
                rows,
                next,
                len,
            } => {
                spool.check_posting_usable()?;
                spool.check_posting_epoch(*changes)?;
                if *next == *len {
                    return Ok(None);
                }
                let slot = *next;
                *next = next.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                rows.get_mut(slot)
                    .ok_or(IndexError::ResourceLimited)?
                    .take()
                    .ok_or_else(|| IndexError::BackendViolation("sparse posting row lost".into()))
                    .map(Some)
            }
        }
    }
}

pub(super) struct PostingCursor<'a> {
    spool: &'a TextSpool,
    terms: TermPage,
    cache: Option<&'a PostingPacketCache>,
    validated: bool,
    case_cut: Option<usize>,
    range_start: usize,
    range_exhausted: bool,
    after: Option<OrdinalMapEdit>,
    range: Option<PackedRangeCursor<'a>>,
    finished: bool,
}

impl PostingCursor<'_> {
    pub(super) fn check_cardinalities(
        &self,
        proof: &DensePostingPrepassProof,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        proof.check_page(self.spool, &self.terms, budget)
    }
}

impl MapEditCursor for PostingCursor<'_> {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        self.spool.check_posting_usable()?;
        budget.checkpoint()?;
        if self.finished {
            return Ok(None);
        }
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
            self.case_cut = self.terms.case_cut()?;
            self.validated = true;
        }
        loop {
            budget.checkpoint()?;
            let range_end = self
                .case_cut
                .filter(|_| self.range_start == 0)
                .unwrap_or(self.terms.len());
            let first = self
                .terms
                .get(self.range_start)
                .ok_or(IndexError::ResourceLimited)?;
            let last = self
                .terms
                .get(
                    range_end
                        .checked_sub(1)
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?;
            if self.range.is_none() {
                self.range = Some(PackedRangeCursor::new_with_packets(
                    self.spool,
                    first.as_slice(),
                    self.cache,
                    budget,
                )?);
            }
            let edit = if self.range_exhausted {
                None
            } else {
                let range_start = self.range_start;
                let previous = self.after.as_ref();
                let terms = &self.terms;
                self.range
                    .as_mut()
                    .ok_or(IndexError::ResourceLimited)?
                    .next_row(budget, |row, budget| {
                        if row.term > last.as_slice() {
                            return Ok(None);
                        }
                        let slot = terms
                            .binary_search_by(|key| key.as_slice().cmp(row.term))
                            .map_err(|_error| {
                                IndexError::CorruptObject(
                                    "posting term outside selected page".into(),
                                )
                            })?;
                        if slot < range_start || slot >= range_end {
                            return Err(IndexError::CorruptObject(
                                "posting term outside selected surface".into(),
                            ));
                        }
                        if let Some(before) = previous {
                            let prior_slot = usize::try_from(before.root())
                                .map_err(|_error| IndexError::ResourceLimited)?;
                            if slot < prior_slot
                                || slot
                                    > prior_slot
                                        .checked_add(1)
                                        .ok_or(IndexError::ResourceLimited)?
                                || (slot == prior_slot && row.owner.as_slice() <= before.key())
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
                        let root =
                            u32::try_from(slot).map_err(|_error| IndexError::ResourceLimited)?;
                        row.ordinal(root, budget).map(Some)
                    })?
                    .flatten()
            };
            if let Some(edit) = edit {
                self.after = Some(edit.clone());
                return Ok(Some(edit));
            }
            let observed = self
                .after
                .as_ref()
                .map(|edit| {
                    usize::try_from(edit.root()).map_err(|_error| IndexError::ResourceLimited)
                })
                .transpose()?;
            if observed
                != Some(
                    range_end
                        .checked_sub(1)
                        .ok_or(IndexError::ResourceLimited)?,
                )
            {
                return Err(IndexError::CorruptObject(
                    "posting range is missing a selected term".into(),
                ));
            }
            if range_end == self.terms.len() {
                if let Some(range) = self.range.as_mut() {
                    range.remember_packet();
                }
                self.finished = true;
                return Ok(None);
            }
            self.range_exhausted = true;
            let next = self
                .terms
                .get(range_end)
                .ok_or(IndexError::ResourceLimited)?;
            self.range
                .as_mut()
                .ok_or(IndexError::ResourceLimited)?
                .reposition(next.as_slice(), budget)?;
            self.range_start = range_end;
            self.range_exhausted = false;
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
    fn subtree_packing(
        &self,
    ) -> Result<quanta_index_core::index::ports::RootPackingPlan, IndexError> {
        self.spool.check_posting_usable()?;
        let mut packing = quanta_index_core::index::ports::RootPackingPlan::default();
        packing.select(0, quanta_index_core::index::ports::RootPackingMode::Nested)?;
        packing.select(1, quanta_index_core::index::ports::RootPackingMode::Nested)?;
        Ok(packing)
    }

    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        budget.checkpoint()?;
        self.spool.check_posting_usable()?;
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
                .map(OrdinalMapEdit::key)
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
    mod cold_term_cap;
    mod confirmed_batch;
    mod confirmed_occurrences;
    mod directory_descriptors;
    mod edit_pages;
    mod inventory_read_lease;
    mod paired_eof_epoch;
    mod planned_inventory;
    mod planned_wide;
    mod posting_cursor_ranges;
    mod posting_packed;
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
    fn dense_posting_page_uses_authenticated_counts_without_sparse_packet_scan()
    -> Result<(), IndexError> {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        let mut rows = Vec::new();
        for (term, owners) in [
            (b"\0a".as_slice(), 1_u16..3),
            (b"\0b".as_slice(), 3_u16..23),
            (b"\0c".as_slice(), 23_u16..24),
        ] {
            for owner in owners {
                let mut key = vec![0; 30];
                key.extend_from_slice(&owner.to_be_bytes());
                rows.push((
                    term.to_vec(),
                    MapEdit {
                        key,
                        expected: None,
                        next: Some(MapValue::Object(ObjectRef(Digest([1; 32])))),
                    },
                ));
            }
        }
        spool.posting_batch(&rows, &mut work)?;
        let terms = spool.posting_term_page(None, &mut work)?;
        check_eq!(terms.len(), 3);
        let cardinalities = spool.posting_cardinalities(&terms, &mut work)?;
        check_eq!(cardinalities.df(1, 0)?, 20);
        let proof = cardinalities.dense_prepass_proof();
        drop(cardinalities);
        let before = work.usage();
        let mut roots = spool.posting_root_cursors_with_proof(
            &terms,
            &[true, false, true],
            &proof,
            &mut work,
        )?;
        // The authoritative fallback does no eager packet reads.
        check_eq!(work.usage().reads - before.reads, 0);
        for (slot, expected) in [(0, &[1_u16, 2][..]), (2, &[23_u16][..])] {
            let mut cursor = roots.cursor(
                slot,
                terms
                    .get(slot)
                    .ok_or(IndexError::ResourceLimited)?
                    .as_slice(),
            )?;
            for owner in expected {
                let edit = cursor
                    .next_edit(&mut work)?
                    .ok_or(IndexError::ResourceLimited)?;
                let expected_owner = owner.to_be_bytes();
                check_eq!(edit.key().get(30..), Some(expected_owner.as_slice()));
            }
            check!(cursor.next_edit(&mut work)?.is_none());
        }
        drop(roots);

        let mut other_page = TermPage::new();
        for slot in 0..2 {
            other_page.push(
                terms
                    .get(slot)
                    .ok_or(IndexError::ResourceLimited)?
                    .as_slice(),
                &work,
            )?;
        }
        let before = work.usage();
        drop(spool.posting_root_cursors_with_proof(
            &other_page,
            &[true, true],
            &proof,
            &mut work,
        )?);
        check!(work.usage().reads > before.reads);

        let stale = DensePostingPrepassProof {
            page_digest: proof.page_digest,
            changes: proof
                .changes
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?,
            lower_bound: proof.lower_bound,
        };
        let before = work.usage();
        drop(spool.posting_root_cursors_with_proof(
            &terms,
            &[true, false, true],
            &stale,
            &mut work,
        )?);
        check!(work.usage().reads > before.reads);
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
            check_eq!(row.root(), root);
            check_eq!(row.key(), key);
            check_eq!(row.expected()?, None);
            check_eq!(row.next()?, Some(reference.into()));
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
                    InventoryPartition::Ready(PLANNED_ROOT_OFFSET),
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
        for root in [-1, i64::from(PLANNED_ROOT_OFFSET)] {
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
             INSERT INTO tokens VALUES(zeroblob(1025),0);",
        ).map_err(|error| sql(&error))?;
        for statement in [TOKEN_TERM_PAGE_SQL] {
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
",
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
            }
        }
        for sql_text in [TOKEN_TERM_PAGE_SQL] {
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
