//! Ranked lexical pages in the one row order (QI-BB-005).
//!
//! A text or symbol page is ordered by score descending, then repo-relative
//! path, start line, end line and candidate id ascending
//! ([`LexicalRowOrderKey::order`]). Every column of that key is a fast
//! column of the index, so the collectors here rank, cut and group rows
//! without reading a stored document: a stored document is fetched only
//! for a row a page returns.
//!
//! [`RankedPageCollector`] keeps the first `limit` rows strictly after an
//! optional cursor, in exact order — ties at the page boundary are broken
//! by the key, never by index order, so consecutive pages neither repeat
//! nor skip a tied row. Strings are read only for a row that ties or beats
//! the page's current boundary; the rest are rejected on their score. It
//! can count every row after the cursor in the same pass, and it lets the
//! engine skip blocks that cannot reach the boundary when the scores are
//! unboosted.
//!
//! [`GroupedPageCollector`] is the projection: one representative per
//! group (the group's first row in page order), kept per segment by
//! segment-local ordinals and resolved to strings once per group, so its
//! memory is the groups, not the matches.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};
use std::sync::Arc;

use quanta_index_contract::{LexicalCandidate, LexicalCursor, LexicalRowOrderKey, SymbolCandidate};
use quanta_index_core::LexicalMemoryReservation;
use tantivy::collector::{Collector, SegmentCollector};
use tantivy::columnar::StrColumn;
use tantivy::fastfield::Column;
use tantivy::query::Weight;
use tantivy::{DocAddress, DocId, Score, SegmentOrdinal, SegmentReader, TantivyError};

use crate::budgeted_search::CollectionBudget;

#[path = "ranked_rows.rs"]
mod rows;
use rows::CollectionMemory;
pub(crate) use rows::RankedRows;

#[cfg(test)]
#[path = "ranked_page_tests.rs"]
mod tests;

/// The fast columns every ranked row is keyed by.
pub(crate) const RANKED_SOURCE_REPO_COLUMN: &str = "repo_id";
pub(crate) const RANKED_PATH_COLUMN: &str = "repo_relative_path";
pub(crate) const RANKED_CANDIDATE_ID_COLUMN: &str = "candidate_id";
pub(crate) const RANKED_START_LINE_COLUMN: &str = "start_line";
pub(crate) const RANKED_END_LINE_COLUMN: &str = "end_line";

/// One row's owned order key.
#[derive(Debug)]
pub(crate) struct RankedRowKey {
    pub(crate) source_repo_id: String,
    pub(crate) score: f32,
    pub(crate) repo_relative_path: String,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    pub(crate) candidate_id: String,
    // Declared after strings: their allocations drop before the lease.
    _string_memory: Option<LexicalMemoryReservation>,
}

impl RankedRowKey {
    pub(crate) fn order_key(&self) -> LexicalRowOrderKey<'_> {
        LexicalRowOrderKey {
            score: self.score,
            source_repo_id: &self.source_repo_id,
            repo_relative_path: &self.repo_relative_path,
            start_line: self.start_line,
            end_line: self.end_line,
            candidate_id: &self.candidate_id,
        }
    }

    fn order(&self, other: &Self) -> Ordering {
        self.order_key().order(&other.order_key())
    }
}

/// A row as a page keeps it: its key and where its document is.
#[derive(Debug)]
pub(crate) struct RankedRow {
    pub(crate) key: RankedRowKey,
    pub(crate) address: DocAddress,
}

/// Heap entry: the greatest entry is the row that comes last in the page.
struct Latest(RankedRow);

impl PartialEq for Latest {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Latest {}

impl PartialOrd for Latest {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Latest {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.key.order(&other.0.key)
    }
}

fn missing_column(name: &str) -> TantivyError {
    TantivyError::SchemaError(format!(
        "the ranked row column `{name}` is not a fast column of this segment"
    ))
}

/// One segment's ranked-row columns.
struct RankedRowColumns {
    source_repo: StrColumn,
    path: StrColumn,
    candidate_id: StrColumn,
    start_line: Column<u64>,
    end_line: Column<u64>,
}

impl RankedRowColumns {
    fn open(reader: &SegmentReader) -> tantivy::Result<Self> {
        let fast = reader.fast_fields();
        Ok(Self {
            source_repo: fast
                .str(RANKED_SOURCE_REPO_COLUMN)?
                .ok_or_else(|| missing_column(RANKED_SOURCE_REPO_COLUMN))?,
            path: fast
                .str(RANKED_PATH_COLUMN)?
                .ok_or_else(|| missing_column(RANKED_PATH_COLUMN))?,
            candidate_id: fast
                .str(RANKED_CANDIDATE_ID_COLUMN)?
                .ok_or_else(|| missing_column(RANKED_CANDIDATE_ID_COLUMN))?,
            start_line: fast.u64(RANKED_START_LINE_COLUMN)?,
            end_line: fast.u64(RANKED_END_LINE_COLUMN)?,
        })
    }

    fn ord(column: &StrColumn, doc: DocId, name: &str) -> tantivy::Result<u64> {
        let mut ordinals = column.term_ords(doc);
        let ordinal = ordinals.next().ok_or_else(|| {
            TantivyError::InternalError(format!("document {doc} has no `{name}` value"))
        })?;
        if ordinals.next().is_some() {
            return Err(TantivyError::InternalError(format!(
                "document {doc} has duplicate `{name}` values"
            )));
        }
        Ok(ordinal)
    }

    fn line(column: &Column<u64>, doc: DocId, name: &str) -> tantivy::Result<u32> {
        let value = column.first(doc).ok_or_else(|| {
            TantivyError::InternalError(format!("document {doc} has no `{name}` value"))
        })?;
        u32::try_from(value).map_err(|error| {
            TantivyError::InternalError(format!("document {doc} `{name}` {value}: {error}"))
        })
    }

    fn string(column: &StrColumn, ord: u64, name: &str) -> tantivy::Result<String> {
        let mut out = String::new();
        if column.ord_to_str(ord, &mut out)? {
            Ok(out)
        } else {
            Err(TantivyError::InternalError(format!(
                "`{name}` ordinal {ord} names no term"
            )))
        }
    }

    fn key(
        &self,
        doc: DocId,
        score: f32,
        collection: Option<&CollectionBudget>,
    ) -> tantivy::Result<RankedRowKey> {
        let source_repo = Self::ord(&self.source_repo, doc, RANKED_SOURCE_REPO_COLUMN)?;
        let path = Self::ord(&self.path, doc, RANKED_PATH_COLUMN)?;
        let candidate_id = Self::ord(&self.candidate_id, doc, RANKED_CANDIDATE_ID_COLUMN)?;
        // Dictionary decode workspace remains outside retained-key admission
        // until sealed admission provides an authoritative pre-decode bound.
        let repo = Self::string(&self.source_repo, source_repo, RANKED_SOURCE_REPO_COLUMN)?;
        let path = Self::string(&self.path, path, RANKED_PATH_COLUMN)?;
        let id = Self::string(&self.candidate_id, candidate_id, RANKED_CANDIDATE_ID_COLUMN)?;
        let bytes = repo
            .len()
            .checked_add(path.len())
            .and_then(|n| n.checked_add(id.len()))
            .ok_or_else(|| TantivyError::InvalidArgument("ranked key bytes overflow".into()))?;
        let memory = collection
            .map(|budget| budget.reserve_bytes(bytes))
            .transpose()?;
        // Copy into admitted exact-capacity output while the decode workspace
        // remains alive; the lease follows this key across every ownership move.
        Ok(RankedRowKey {
            source_repo_id: Self::copy_key_string(&repo)?,
            score,
            repo_relative_path: Self::copy_key_string(&path)?,
            start_line: Self::line(&self.start_line, doc, RANKED_START_LINE_COLUMN)?,
            end_line: Self::line(&self.end_line, doc, RANKED_END_LINE_COLUMN)?,
            candidate_id: Self::copy_key_string(&id)?,
            _string_memory: memory,
        })
    }

    fn copy_key_string(value: &str) -> tantivy::Result<String> {
        let mut output = String::new();
        output.try_reserve_exact(value.len()).map_err(|error| {
            TantivyError::InvalidArgument(format!("ranked key allocation failed: {error}"))
        })?;
        output.push_str(value);
        Ok(output)
    }
}

/// Where a boosted score stands against a cursor, before any string is
/// read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScorePosition {
    /// A higher score: the row came before the cursor.
    Before,
    /// A lower score, or no cursor: the row is after it.
    After,
    /// The cursor's score: the rest of the key decides.
    Tied,
}

fn score_position(after: Option<&LexicalCursor>, score: f32) -> ScorePosition {
    match after.map(|cursor| score.total_cmp(&cursor.score)) {
        None | Some(Ordering::Less) => ScorePosition::After,
        Some(Ordering::Greater) => ScorePosition::Before,
        Some(Ordering::Equal) => ScorePosition::Tied,
    }
}

/// What a ranked page collect produced.
#[derive(Debug)]
pub(crate) struct RankedPageFruit {
    /// The first rows after the cursor, in page order, at most the limit.
    pub(crate) rows: RankedRows,
    /// Every matching row after the cursor, when the collect counted.
    pub(crate) matched: u64,
}

/// The first `limit` rows strictly after `after`, in exact page order.
pub(crate) struct RankedPageCollector {
    limit: usize,
    after: Option<Arc<LexicalCursor>>,
    boost: f32,
    count: bool,
    collection: Option<CollectionBudget>,
    materializes_all: bool,
}

impl RankedPageCollector {
    pub(crate) const fn new(
        limit: usize,
        after: Option<Arc<LexicalCursor>>,
        boost: f32,
        count: bool,
    ) -> Self {
        Self {
            limit,
            after,
            boost,
            count,
            collection: None,
            materializes_all: false,
        }
    }

    pub(crate) fn with_collection_budget(mut self, collection: CollectionBudget) -> Self {
        self.collection = Some(collection);
        self.materializes_all = true;
        self
    }

    pub(crate) fn with_resource_budget(mut self, collection: CollectionBudget) -> Self {
        self.collection = Some(collection);
        self
    }

    /// Block skipping is exact only when a row's page score is its engine
    /// score and nothing needs every row counted.
    fn prunes(&self) -> bool {
        !self.count && self.boost.to_bits() == 1.0_f32.to_bits()
    }
}

/// One segment's share of a ranked page.
pub(crate) struct RankedPageSegment {
    columns: RankedRowColumns,
    segment_ord: SegmentOrdinal,
    limit: usize,
    after: Option<Arc<LexicalCursor>>,
    boost: f32,
    heap: BinaryHeap<Latest>,
    heap_memory: Option<LexicalMemoryReservation>,
    matched: u64,
    error: Option<TantivyError>,
    collection: Option<CollectionBudget>,
    materializes_all: bool,
}

impl RankedPageSegment {
    /// The score a row must reach to still matter to the page: the page's
    /// last score once the page is full.
    fn boundary(&self) -> Option<f32> {
        (self.heap.len() >= self.limit)
            .then(|| self.heap.peek().map(|latest| latest.0.key.score))
            .flatten()
    }

    fn offer(&mut self, doc: DocId, engine_score: Score) -> tantivy::Result<()> {
        if self.materializes_all
            && self
                .collection
                .as_ref()
                .is_some_and(|ledger| !ledger.admit())
        {
            return Ok(());
        }
        let score = engine_score * self.boost;
        let position = score_position(self.after.as_deref(), score);
        if position == ScorePosition::Before {
            return Ok(());
        }
        // A row below a full page's boundary cannot enter it; counted, it
        // needs no string when its score alone places it after the cursor.
        let enters = self.limit > 0 && self.boundary().is_none_or(|boundary| score >= boundary);
        if !enters && position == ScorePosition::After {
            self.matched = self.matched.saturating_add(1);
            return Ok(());
        }
        let key = self.columns.key(doc, score, self.collection.as_ref())?;
        if position == ScorePosition::Tied
            && !self
                .after
                .as_deref()
                .is_some_and(|cursor| cursor.admits(&key.order_key()))
        {
            return Ok(());
        }
        self.matched = self.matched.saturating_add(1);
        if !enters {
            return Ok(());
        }
        let row = RankedRow {
            key,
            address: DocAddress::new(self.segment_ord, doc),
        };
        if self.heap.len() < self.limit {
            self.reserve_heap_slot()?;
            self.heap.push(Latest(row));
        } else if let Some(mut latest) = self.heap.peek_mut()
            && row.key.order(&latest.0.key) == Ordering::Less
        {
            *latest = Latest(row);
        }
        Ok(())
    }

    fn reserve_heap_slot(&mut self) -> tantivy::Result<()> {
        let Some(collection) = &self.collection else {
            return Ok(());
        };
        if self.heap.len() < self.heap.capacity() {
            return Ok(());
        }
        let capacity = self
            .heap
            .capacity()
            .checked_mul(2)
            .map(|size| size.max(4).min(self.limit))
            .ok_or_else(|| {
                TantivyError::InvalidArgument("ranked heap capacity overflow".to_string())
            })?;
        let bytes = capacity
            .checked_mul(std::mem::size_of::<Latest>())
            .ok_or_else(|| {
                TantivyError::InvalidArgument("ranked heap byte size overflow".to_string())
            })?;
        let replacement = collection.reserve_bytes(bytes)?;
        let additional = capacity.checked_sub(self.heap.len()).ok_or_else(|| {
            TantivyError::InternalError("ranked heap capacity shrank".to_string())
        })?;
        self.heap.try_reserve_exact(additional).map_err(|error| {
            TantivyError::InvalidArgument(format!("ranked heap allocation failed: {error}"))
        })?;
        self.heap_memory = Some(replacement);
        Ok(())
    }

    fn record(&mut self, doc: DocId, score: Score) {
        if self.error.is_some() {
            return;
        }
        if let Err(error) = self.offer(doc, score) {
            if let Some(collection) = &self.collection {
                collection.abort();
            }
            self.error = Some(error);
        }
    }

    fn finish(self) -> tantivy::Result<RankedPageFruit> {
        if let Some(error) = self.error {
            return Err(error);
        }
        if self
            .collection
            .as_ref()
            .is_some_and(CollectionBudget::stopped)
        {
            return Err(TantivyError::InvalidArgument(
                "ranked collection exceeded its examined-candidate budget".to_string(),
            ));
        }
        let mut rows = RankedRows::with_capacity(
            self.heap.len(),
            self.collection
                .as_ref()
                .map(|collection| collection.resources.clone()),
        )?;
        for latest in self.heap {
            rows.push(latest.0)?;
        }
        rows.sort_by(|left, right| left.key.order(&right.key))?;
        Ok(RankedPageFruit {
            rows,
            matched: self.matched,
        })
    }
}

impl SegmentCollector for RankedPageSegment {
    type Fruit = tantivy::Result<RankedPageFruit>;

    fn collect(&mut self, doc: DocId, score: Score) {
        self.record(doc, score);
    }

    fn harvest(self) -> Self::Fruit {
        self.finish()
    }
}

impl Collector for RankedPageCollector {
    type Fruit = RankedPageFruit;
    type Child = RankedPageSegment;

    fn for_segment(
        &self,
        segment_ord: SegmentOrdinal,
        reader: &SegmentReader,
    ) -> tantivy::Result<RankedPageSegment> {
        Ok(RankedPageSegment {
            columns: RankedRowColumns::open(reader)?,
            segment_ord,
            limit: self.limit,
            after: self.after.clone(),
            boost: self.boost,
            heap: BinaryHeap::new(),
            heap_memory: None,
            matched: 0,
            error: None,
            collection: self.collection.clone(),
            materializes_all: self.materializes_all,
        })
    }

    fn requires_scoring(&self) -> bool {
        true
    }

    fn merge_fruits(
        &self,
        segment_fruits: Vec<tantivy::Result<RankedPageFruit>>,
    ) -> tantivy::Result<RankedPageFruit> {
        let capacity = segment_fruits.iter().try_fold(0_usize, |total, fruit| {
            let count = fruit
                .as_ref()
                .map_err(|error| TantivyError::InvalidArgument(error.to_string()))?
                .rows
                .len();
            total.checked_add(count).ok_or_else(|| {
                TantivyError::InvalidArgument("ranked merge size overflow".to_string())
            })
        })?;
        let mut rows = RankedRows::with_capacity(
            capacity,
            self.collection
                .as_ref()
                .map(|collection| collection.resources.clone()),
        )?;
        let mut matched = 0_u64;
        for fruit in segment_fruits {
            let fruit = fruit?;
            matched = matched.saturating_add(fruit.matched);
            if let Some(collection) = &self.collection {
                for _row in fruit.rows.iter() {
                    collection.charge_work(1)?;
                }
            }
            for row in fruit.rows {
                rows.push(row)?;
            }
        }
        rows.sort_by(|left, right| left.key.order(&right.key))?;
        rows.truncate(self.limit);
        Ok(RankedPageFruit { rows, matched })
    }

    fn collect_segment(
        &self,
        weight: &dyn Weight,
        segment_ord: SegmentOrdinal,
        reader: &SegmentReader,
    ) -> tantivy::Result<tantivy::Result<RankedPageFruit>> {
        let mut segment = self.for_segment(segment_ord, reader)?;
        let alive = reader.alive_bitset();
        if self.prunes() {
            // The engine skips what cannot beat the threshold; the page's
            // boundary is lowered by one ulp so a row tying it still
            // arrives and is ordered by the rest of its key.
            weight.for_each_pruning(Score::MIN, reader, &mut |doc, score| {
                if alive.is_none_or(|alive| alive.is_alive(doc)) {
                    segment.record(doc, score);
                }
                segment.boundary().map_or(Score::MIN, f32::next_down)
            })?;
        } else {
            weight.for_each(reader, &mut |doc, score| {
                if alive.is_none_or(|alive| alive.is_alive(doc)) {
                    segment.record(doc, score);
                }
            })?;
        }
        Ok(segment.finish())
    }
}

/// How a projection groups rows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProjectionGroup {
    /// One group per source repository in the containing generation.
    Repo,
    /// One group per repo-relative path (`select:path`, `select:file`,
    /// `select:file.owners`).
    Path,
}

/// A segment's best row of one group, by segment-local ordinals.
#[derive(Clone, Copy, Debug)]
struct SegmentBest {
    source_repo: u64,
    score: f32,
    path: u64,
    start_line: u32,
    end_line: u32,
    candidate_id: u64,
    doc: DocId,
}

impl SegmentBest {
    /// Page order within one segment: ordinals order as their strings do.
    fn order(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then(self.source_repo.cmp(&other.source_repo))
            .then(self.path.cmp(&other.path))
            .then(self.start_line.cmp(&other.start_line))
            .then(self.end_line.cmp(&other.end_line))
            .then(self.candidate_id.cmp(&other.candidate_id))
    }
}

/// What a grouped collect produced.
#[derive(Debug)]
pub(crate) struct GroupedPageFruit {
    /// Every group's representative, in page order.
    pub(crate) representatives: RankedRows,
    /// Every matching row, grouped or not.
    pub(crate) matched: u64,
}

/// One representative per group: the group's first row in page order.
pub(crate) struct GroupedPageCollector {
    group: ProjectionGroup,
    boost: f32,
    collection: CollectionBudget,
}

impl GroupedPageCollector {
    pub(crate) const fn new(
        group: ProjectionGroup,
        boost: f32,
        collection: CollectionBudget,
    ) -> Self {
        Self {
            group,
            boost,
            collection,
        }
    }
}

/// One segment's groups.
pub(crate) struct GroupedPageSegment {
    columns: RankedRowColumns,
    segment_ord: SegmentOrdinal,
    group: ProjectionGroup,
    boost: f32,
    bests: BTreeMap<(u64, u64), SegmentBest>,
    map_memory: CollectionMemory,
    matched: u64,
    error: Option<TantivyError>,
    collection: CollectionBudget,
}

impl GroupedPageSegment {
    fn offer(&mut self, doc: DocId, engine_score: Score) -> tantivy::Result<()> {
        if !self.collection.admit() {
            return Ok(());
        }
        self.matched = self.matched.saturating_add(1);
        let candidate = SegmentBest {
            source_repo: RankedRowColumns::ord(
                &self.columns.source_repo,
                doc,
                RANKED_SOURCE_REPO_COLUMN,
            )?,
            score: engine_score * self.boost,
            path: RankedRowColumns::ord(&self.columns.path, doc, RANKED_PATH_COLUMN)?,
            start_line: RankedRowColumns::line(
                &self.columns.start_line,
                doc,
                RANKED_START_LINE_COLUMN,
            )?,
            end_line: RankedRowColumns::line(&self.columns.end_line, doc, RANKED_END_LINE_COLUMN)?,
            candidate_id: RankedRowColumns::ord(
                &self.columns.candidate_id,
                doc,
                RANKED_CANDIDATE_ID_COLUMN,
            )?,
            doc,
        };
        let group = match self.group {
            ProjectionGroup::Repo => (candidate.source_repo, 0),
            ProjectionGroup::Path => (candidate.source_repo, candidate.path),
        };
        match self.bests.entry(group) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                let memory = self
                    .collection
                    .reserve_map_entry::<(u64, u64), SegmentBest>()?;
                self.map_memory.hold(memory)?;
                let _inserted = slot.insert(candidate);
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                if candidate.order(slot.get()) == Ordering::Less {
                    let _replaced = slot.insert(candidate);
                }
            }
        }
        Ok(())
    }

    fn finish(self) -> tantivy::Result<GroupedPageFruit> {
        if let Some(error) = self.error {
            return Err(error);
        }
        // Refuse before decoding any group strings or allocating the output
        // buffer. The search wrapper preserves the canonical typed error.
        if self.collection.stopped() {
            return Err(TantivyError::InvalidArgument(
                "grouped collection exceeded its examined-candidate budget".to_string(),
            ));
        }
        let mut representatives =
            RankedRows::with_capacity(self.bests.len(), Some(self.collection.resources.clone()))?;
        for best in self.bests.into_values() {
            self.collection.charge_work(1)?;
            representatives.push(RankedRow {
                key: self
                    .columns
                    .key(best.doc, best.score, Some(&self.collection))?,
                address: DocAddress::new(self.segment_ord, best.doc),
            })?;
        }
        Ok(GroupedPageFruit {
            representatives,
            matched: self.matched,
        })
    }
}

impl SegmentCollector for GroupedPageSegment {
    type Fruit = tantivy::Result<GroupedPageFruit>;

    fn collect(&mut self, doc: DocId, score: Score) {
        if self.error.is_some() {
            return;
        }
        if let Err(error) = self.offer(doc, score) {
            self.collection.abort();
            self.error = Some(error);
        }
    }

    fn harvest(self) -> Self::Fruit {
        self.finish()
    }
}

impl Collector for GroupedPageCollector {
    type Fruit = GroupedPageFruit;
    type Child = GroupedPageSegment;

    fn for_segment(
        &self,
        segment_ord: SegmentOrdinal,
        reader: &SegmentReader,
    ) -> tantivy::Result<GroupedPageSegment> {
        Ok(GroupedPageSegment {
            columns: RankedRowColumns::open(reader)?,
            segment_ord,
            group: self.group,
            boost: self.boost,
            bests: BTreeMap::new(),
            map_memory: CollectionMemory::new(self.collection.resources.clone()),
            matched: 0,
            error: None,
            collection: self.collection.clone(),
        })
    }

    fn requires_scoring(&self) -> bool {
        true
    }

    fn merge_fruits(
        &self,
        segment_fruits: Vec<tantivy::Result<GroupedPageFruit>>,
    ) -> tantivy::Result<GroupedPageFruit> {
        let mut map_memory = CollectionMemory::new(self.collection.resources.clone());
        let mut groups: BTreeMap<(String, String), RankedRow> = BTreeMap::new();
        let mut matched = 0_u64;
        for fruit in segment_fruits {
            let fruit = fruit?;
            matched = matched.saturating_add(fruit.matched);
            for row in fruit.representatives {
                self.collection.charge_work(1)?;
                let key_memory = self.collection.reserve_bytes(match self.group {
                    ProjectionGroup::Repo => row.key.source_repo_id.len(),
                    ProjectionGroup::Path => row
                        .key
                        .source_repo_id
                        .len()
                        .checked_add(row.key.repo_relative_path.len())
                        .ok_or_else(|| {
                            TantivyError::InvalidArgument("group key byte size overflow".into())
                        })?,
                })?;
                map_memory.hold(key_memory)?;
                let group = match self.group {
                    ProjectionGroup::Repo => (row.key.source_repo_id.clone(), String::new()),
                    ProjectionGroup::Path => (
                        row.key.source_repo_id.clone(),
                        row.key.repo_relative_path.clone(),
                    ),
                };
                match groups.entry(group) {
                    std::collections::btree_map::Entry::Vacant(slot) => {
                        map_memory.hold(
                            self.collection
                                .reserve_map_entry::<(String, String), RankedRow>()?,
                        )?;
                        let _inserted: &mut RankedRow = slot.insert(row);
                    }
                    std::collections::btree_map::Entry::Occupied(mut slot) => {
                        if row.key.order(&slot.get().key) == Ordering::Less {
                            let _replaced: RankedRow = slot.insert(row);
                        }
                    }
                }
            }
        }
        let mut representatives =
            RankedRows::with_capacity(groups.len(), Some(self.collection.resources.clone()))?;
        for row in groups.into_values() {
            representatives.push(row)?;
        }
        representatives.sort_by(|left, right| left.key.order(&right.key))?;
        Ok(GroupedPageFruit {
            representatives,
            matched,
        })
    }
}

/// A row the adapter returns, positioned in the ranked page order.
pub(crate) trait RankedRowView {
    fn ranked_key(&self) -> LexicalRowOrderKey<'_>;
}

impl RankedRowView for LexicalCandidate {
    fn ranked_key(&self) -> LexicalRowOrderKey<'_> {
        self.order_key()
    }
}

impl RankedRowView for SymbolCandidate {
    fn ranked_key(&self) -> LexicalRowOrderKey<'_> {
        self.order_key()
    }
}

/// The rows strictly after `after`, in page order: what the collectors do,
/// for rows the unindexed scan matched in memory.
pub(crate) fn rank_in_memory<T: RankedRowView>(
    rows: Vec<T>,
    after: Option<&LexicalCursor>,
) -> Vec<T> {
    let mut rows: Vec<T> = rows
        .into_iter()
        .filter(|row| after.is_none_or(|cursor| cursor.admits(&row.ranked_key())))
        .collect();
    rows.sort_by(|left, right| left.ranked_key().order(&right.ranked_key()));
    rows
}

/// One row per group, each group's first in page order: the grouped
/// collector's answer for rows matched in memory.
pub(crate) fn group_in_memory<T: RankedRowView>(rows: Vec<T>, group: ProjectionGroup) -> Vec<T> {
    let mut groups: BTreeMap<(String, String), T> = BTreeMap::new();
    for row in rows {
        let key = match group {
            ProjectionGroup::Repo => (row.ranked_key().source_repo_id.to_string(), String::new()),
            ProjectionGroup::Path => (
                row.ranked_key().source_repo_id.to_string(),
                row.ranked_key().repo_relative_path.to_string(),
            ),
        };
        match groups.entry(key) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                let _inserted: &mut T = slot.insert(row);
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                if row.ranked_key().order(&slot.get().ranked_key()) == Ordering::Less {
                    let _replaced: T = slot.insert(row);
                }
            }
        }
    }
    groups.into_values().collect()
}
