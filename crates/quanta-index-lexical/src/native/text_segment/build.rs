//! Bounded, document-partitioned construction for the segment format.
#![expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::integer_division,
    reason = "index arithmetic is bounded by fixed window/page limits and checked wire counts"
)]

use super::super::cold_tokens::ColdTokenProgramView;
use super::super::token_prefix_cache::TokenPrefixCache;
use super::super::token_prefix_prepare;
use super::super::{
    TextRecipe, TokenBatch, check_admission, owner_key, prepare_indexed_token_cursor,
    visit_prepared_token_batches,
};
use super::spill::{SegmentSpill, SpillEvent};
use super::wire::{self, Doc, SegmentEdge};
use quanta_index_core::index::{
    AttemptContext, DeltaInput, Digest, IndexError, MemoryLease, MemoryLifetime, ObjectBatchInput,
    ObjectRef, ScratchSpacePort, SourceCut, SourceDelta, SourceInputCursor, SourceRecord,
    WorkBudget, WorkUsage,
};
use std::cmp::Ordering;
#[expect(
    clippy::disallowed_types,
    reason = "bounded term lookup with full-key collision checks"
)]
use std::hash::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::mem::size_of;

// A segment is sealed only between documents. The resident rows switch to
// ordered scratch once this working set cannot hold the active document.
const MAX_ROWS: usize = 65_536;
const MAX_KEYS_BYTES: usize = 16 * 1024 * 1024;
const FIRST_KEYS_BYTES: usize = 64 * 1024;
const LOOKUP_SLOTS: usize = 2 * MAX_ROWS;
const LOOKUP_TERM_MASK: u32 = 0xffff;
const LOOKUP_GENERATION_SHIFT: u32 = 16;
const NONE: u32 = u32::MAX;
const PAGE_BATCH: usize = 16;
const PAGE_ROUTE_BYTES: usize = 32 + 2 + wire::MAX_KEY + 2 + wire::MAX_KEY;

fn width(bytes: usize) -> Result<u64, IndexError> {
    u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)
}

pub(super) fn copy_work(budget: &mut WorkBudget, bytes: usize) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        bytes: width(bytes)?,
        ..WorkUsage::default()
    })
}

pub(super) fn funded_vec<T>(
    capacity: usize,
    budget: &WorkBudget,
) -> Result<(Vec<T>, Option<MemoryLease>), IndexError> {
    if capacity == 0 {
        budget.checkpoint()?;
        return Ok((Vec::new(), None));
    }
    let bytes = capacity
        .checked_mul(size_of::<T>())
        .ok_or(IndexError::ResourceLimited)?;
    let memory = budget.reserve_memory(width(bytes)?, MemoryLifetime::Temporary)?;
    let mut values = Vec::new();
    values
        .try_reserve_exact(capacity)
        .map_err(|_| IndexError::ResourceLimited)?;
    if values.capacity() > capacity {
        return Err(IndexError::ResourceLimited);
    }
    Ok((values, memory))
}

#[derive(Clone, Copy)]
struct Row {
    doc: u16,
    position: u32,
    next: u32,
}

#[derive(Clone, Copy)]
struct Key {
    start: u32,
    len: u16,
}

struct Term {
    key: Key,
    hash: u64,
    first: u32,
    last: u32,
}

pub(super) struct Window {
    keys: Vec<u8>,
    rows: Vec<Row>,
    terms: Vec<Term>,
    order: Vec<u32>,
    // Upper 16 bits identify the window, lower 16 bits address its term.
    lookup: Vec<u32>,
    lookup_generation: u16,
    pub(super) docs: Vec<Doc>,
    spill: Option<SegmentSpill>,
    // Backing is dropped before its Original grants.
    _keys_memory: Option<MemoryLease>,
    _rows_memory: Option<MemoryLease>,
    _terms_memory: Option<MemoryLease>,
    _order_memory: Option<MemoryLease>,
    _lookup_memory: Option<MemoryLease>,
    _docs_memory: Option<MemoryLease>,
}

impl Window {
    pub(super) fn new(budget: &mut WorkBudget) -> Result<Self, IndexError> {
        let (keys, keys_memory) = funded_vec(FIRST_KEYS_BYTES, budget)?;
        let (rows, rows_memory) = funded_vec(MAX_ROWS, budget)?;
        let (terms, terms_memory) = funded_vec(MAX_ROWS, budget)?;
        let (order, order_memory) = funded_vec(MAX_ROWS, budget)?;
        let (mut lookup, lookup_memory) = funded_vec(LOOKUP_SLOTS, budget)?;
        copy_work(
            budget,
            LOOKUP_SLOTS
                .checked_mul(size_of::<u32>())
                .ok_or(IndexError::ResourceLimited)?,
        )?;
        lookup.resize(LOOKUP_SLOTS, 0);
        let (docs, docs_memory) = funded_vec(wire::MAX_DOCS, budget)?;
        Ok(Self {
            keys,
            rows,
            terms,
            order,
            lookup,
            lookup_generation: 1,
            docs,
            spill: None,
            _keys_memory: keys_memory,
            _rows_memory: rows_memory,
            _terms_memory: terms_memory,
            _order_memory: order_memory,
            _lookup_memory: lookup_memory,
            _docs_memory: docs_memory,
        })
    }

    fn key(&self, key: Key) -> Result<&[u8], IndexError> {
        let start = usize::try_from(key.start).map_err(|_| IndexError::ResourceLimited)?;
        let end = start
            .checked_add(usize::from(key.len))
            .ok_or(IndexError::ResourceLimited)?;
        self.keys.get(start..end).ok_or(IndexError::ResourceLimited)
    }

    fn term_key(&self, slot: u32) -> Result<&[u8], IndexError> {
        let slot = usize::try_from(slot).map_err(|_| IndexError::ResourceLimited)?;
        self.key(self.terms.get(slot).ok_or(IndexError::ResourceLimited)?.key)
    }

    fn reserve_keys(&mut self, needed: usize, budget: &mut WorkBudget) -> Result<(), IndexError> {
        if needed <= self.keys.capacity() {
            return Ok(());
        }
        if needed > MAX_KEYS_BYTES {
            return Err(IndexError::ResourceLimited);
        }
        let mut capacity = self.keys.capacity().max(FIRST_KEYS_BYTES);
        while capacity < needed {
            capacity = capacity.checked_mul(2).ok_or(IndexError::ResourceLimited)?;
        }
        let (mut next, memory) = funded_vec(capacity, budget)?;
        copy_work(budget, self.keys.len())?;
        next.extend_from_slice(&self.keys);
        self.keys = next;
        self._keys_memory = memory;
        Ok(())
    }

    pub(super) fn should_seal_before(&self, source_bytes: u64) -> Result<bool, IndexError> {
        if self.docs.is_empty() {
            return Ok(false);
        }
        let bytes = usize::try_from(source_bytes).map_err(|_| IndexError::ResourceLimited)?;
        let possible_rows = bytes.checked_mul(2).ok_or(IndexError::ResourceLimited)?;
        let possible_keys = bytes.checked_mul(4).ok_or(IndexError::ResourceLimited)?;
        Ok(self.spill.is_some()
            || self.docs.len() == wire::MAX_DOCS
            || self.rows.len() >= MAX_ROWS / 2
            || self.keys.len() >= MAX_KEYS_BYTES / 2
            || self
                .rows
                .len()
                .checked_add(possible_rows)
                .is_none_or(|sum| sum > MAX_ROWS)
            || self
                .keys
                .len()
                .checked_add(possible_keys)
                .is_none_or(|sum| sum > MAX_KEYS_BYTES))
    }

    fn intern_term(
        &mut self,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<Option<u32>, IndexError> {
        #[expect(
            clippy::disallowed_types,
            reason = "bounded hash table; full-key equality validates hits"
        )]
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        copy_work(budget, key.len())?;
        let hash = hasher.finish();
        let generation = u32::from(self.lookup_generation) << LOOKUP_GENERATION_SHIFT;
        let mut slot = usize::try_from(
            hash % u64::try_from(LOOKUP_SLOTS).map_err(|_| IndexError::ResourceLimited)?,
        )
        .map_err(|_| IndexError::ResourceLimited)?;
        let term_slot = loop {
            budget.checkpoint()?;
            copy_work(budget, size_of::<u32>())?;
            let cell = *self.lookup.get(slot).ok_or(IndexError::ResourceLimited)?;
            if cell & !LOOKUP_TERM_MASK != generation {
                let end = self
                    .keys
                    .len()
                    .checked_add(key.len())
                    .ok_or(IndexError::ResourceLimited)?;
                if end > MAX_KEYS_BYTES || self.terms.len() == MAX_ROWS {
                    return Ok(None);
                }
                let id = u32::from(
                    u16::try_from(self.terms.len())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                );
                let start =
                    u32::try_from(self.keys.len()).map_err(|_| IndexError::ResourceLimited)?;
                let len = u16::try_from(key.len()).map_err(|_| IndexError::ResourceLimited)?;
                copy_work(
                    budget,
                    key.len()
                        .checked_add(size_of::<Term>())
                        .and_then(|n| n.checked_add(size_of::<u32>() * 2))
                        .ok_or(IndexError::ResourceLimited)?,
                )?;
                self.reserve_keys(end, budget)?;
                self.keys.extend_from_slice(key);
                self.terms.push(Term {
                    key: Key { start, len },
                    hash,
                    first: NONE,
                    last: NONE,
                });
                self.order.push(id);
                *self
                    .lookup
                    .get_mut(slot)
                    .ok_or(IndexError::ResourceLimited)? = generation | id;
                break id;
            }
            let found = cell & LOOKUP_TERM_MASK;
            let term = self
                .terms
                .get(usize::try_from(found).map_err(|_| IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?;
            if term.hash == hash {
                let existing = self.key(term.key)?;
                copy_work(budget, existing.len().min(key.len()))?;
                if existing == key {
                    break found;
                }
            }
            slot = slot.checked_add(1).ok_or(IndexError::ResourceLimited)? % LOOKUP_SLOTS;
        };
        Ok(Some(term_slot))
    }

    fn clear_for_next_segment(&mut self) -> Result<(), IndexError> {
        self.lookup_generation = self
            .lookup_generation
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.keys.clear();
        self.rows.clear();
        self.terms.clear();
        self.order.clear();
        self.docs.clear();
        Ok(())
    }

    fn activate_spill(
        &mut self,
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<&mut SegmentSpill, IndexError> {
        let mut spill = SegmentSpill::new(scratch, budget)?;
        for term in &self.terms {
            let key = self.key(term.key)?;
            let mut cursor = term.first;
            while cursor != NONE {
                let row = self.row(cursor)?;
                copy_work(budget, size_of::<Row>())?;
                spill.push(key, row.doc, row.position, budget)?;
                cursor = row.next;
            }
        }
        self.keys.clear();
        self.rows.clear();
        self.terms.clear();
        self.order.clear();
        Ok(self.spill.insert(spill))
    }

    pub(super) fn enable_spill(
        &mut self,
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.spill.is_none() {
            let _spill = self.activate_spill(scratch, budget)?;
        }
        Ok(())
    }

    fn append_position(
        &mut self,
        term_slot: u32,
        doc: u16,
        position: u32,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.rows.len() == MAX_ROWS {
            return Err(IndexError::ResourceLimited);
        }
        let row_slot = u32::try_from(self.rows.len()).map_err(|_| IndexError::ResourceLimited)?;
        let term = self
            .terms
            .get_mut(usize::try_from(term_slot).map_err(|_| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        copy_work(
            budget,
            size_of::<Row>()
                .checked_add(size_of::<u32>())
                .ok_or(IndexError::ResourceLimited)?,
        )?;
        if term.last == NONE {
            term.first = row_slot;
        } else {
            self.rows
                .get_mut(usize::try_from(term.last).map_err(|_| IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?
                .next = row_slot;
        }
        term.last = row_slot;
        self.rows.push(Row {
            doc,
            position,
            next: NONE,
        });
        Ok(())
    }

    pub(super) fn append_group(
        &mut self,
        key: &[u8],
        doc: u16,
        positions: impl IntoIterator<Item = u32>,
        scratch: &dyn ScratchSpacePort,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let mut term = None;
        for position in positions {
            if let Some(spill) = self.spill.as_mut() {
                spill.push(key, doc, position, budget)?;
                continue;
            }
            if self.rows.len() < MAX_ROWS {
                if term.is_none() {
                    term = self.intern_term(key, budget)?;
                }
                if let Some(slot) = term {
                    self.append_position(slot, doc, position, budget)?;
                    continue;
                }
            }
            self.activate_spill(scratch, budget)?
                .push(key, doc, position, budget)?;
        }
        Ok(())
    }

    pub(super) fn append_document(
        &mut self,
        owner: [u8; 32],
        source: ObjectRef,
        record: &SourceRecord,
        cache: &mut Option<Box<TokenPrefixCache>>,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        let doc = u16::try_from(self.docs.len()).map_err(|_| IndexError::ResourceLimited)?;
        let scratch = context.scratch;
        let mut tokens = 0;
        token_prefix_prepare::with_source_text(record, context, cache, |text, context, cache| {
            let cursor = prepare_indexed_token_cursor(text, context.budget)?;
            let mut cursor = if let Some(state) = cache.as_mut() {
                let (cursor, program) =
                    token_prefix_prepare::try_prepare(state, cursor, context.budget)?;
                if let Some((program, count)) = program {
                    let view = ColdTokenProgramView::from_program(&program)?;
                    view.visit_borrowed_groups(context.budget, |key, positions, budget| {
                        self.append_group(key, doc, positions, scratch, budget)
                    })?;
                    tokens = count;
                    return Ok(());
                }
                cursor
            } else {
                cursor
            };
            let mut batch = TokenBatch::new(context.budget)?;
            tokens = visit_prepared_token_batches(
                &mut cursor,
                &mut batch,
                context.budget,
                |batch, budget| {
                    for token in batch.rows() {
                        self.append_group(
                            batch.term(token)?,
                            doc,
                            std::iter::once(token.position()),
                            scratch,
                            budget,
                        )?;
                    }
                    Ok(())
                },
            )?;
            Ok(())
        })?;
        copy_work(context.budget, size_of::<Doc>())?;
        self.docs.push(Doc {
            owner,
            source,
            tokens,
        });
        Ok(())
    }

    fn compare_terms(
        &self,
        left: u32,
        right: u32,
        budget: &mut WorkBudget,
    ) -> Result<Ordering, IndexError> {
        let a = self.term_key(left)?;
        let b = self.term_key(right)?;
        let mut compared = 0usize;
        let mut order = Ordering::Equal;
        for (&x, &y) in a.iter().zip(b) {
            compared = compared.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            if x != y {
                order = x.cmp(&y);
                break;
            }
        }
        copy_work(
            budget,
            compared.checked_add(1).ok_or(IndexError::ResourceLimited)?,
        )?;
        if order == Ordering::Equal {
            order = a.len().cmp(&b.len());
        }
        Ok(order)
    }

    fn sift(
        &mut self,
        mut root: usize,
        end: usize,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        loop {
            let child = root
                .checked_mul(2)
                .and_then(|n| n.checked_add(1))
                .ok_or(IndexError::ResourceLimited)?;
            if child >= end {
                break;
            }
            let right = child.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            let larger = if right < end
                && self.compare_terms(self.order[child], self.order[right], budget)?
                    == Ordering::Less
            {
                right
            } else {
                child
            };
            if self.compare_terms(self.order[root], self.order[larger], budget)? != Ordering::Less {
                break;
            }
            copy_work(budget, 2 * size_of::<u32>())?;
            self.order.swap(root, larger);
            root = larger;
        }
        Ok(())
    }

    fn sort(&mut self, budget: &mut WorkBudget) -> Result<(), IndexError> {
        let len = self.order.len();
        for start in (0..len / 2).rev() {
            self.sift(start, len, budget)?;
        }
        for end in (1..len).rev() {
            copy_work(budget, 2 * size_of::<u32>())?;
            self.order.swap(0, end);
            self.sift(0, end, budget)?;
        }
        Ok(())
    }

    fn row(&self, slot: u32) -> Result<Row, IndexError> {
        self.rows
            .get(usize::try_from(slot).map_err(|_| IndexError::ResourceLimited)?)
            .copied()
            .ok_or(IndexError::ResourceLimited)
    }
}

fn varint_len(value: u32) -> usize {
    if value < 1 << 7 {
        1
    } else if value < 1 << 14 {
        2
    } else if value < 1 << 21 {
        3
    } else if value < 1 << 28 {
        4
    } else {
        5
    }
}

#[derive(Clone, Copy)]
struct PageKey {
    start: usize,
    len: usize,
}

impl PageKey {
    fn bytes(self, payload: &[u8]) -> Result<&[u8], IndexError> {
        payload
            .get(self.start..self.start + self.len)
            .ok_or(IndexError::ResourceLimited)
    }
}

struct Page {
    bytes: Vec<u8>,
    first: Option<PageKey>,
    last: Option<PageKey>,
}

#[derive(Default)]
struct Posting {
    header: Option<usize>,
    df: u16,
    previous_doc: u16,
}

struct Pages {
    page: Page,
    positions: Vec<u8>,
    pending: Vec<ObjectBatchInput>,
    pending_keys: Vec<(PageKey, PageKey)>,
    pending_leases: Vec<Option<MemoryLease>>,
    routes: Vec<u8>,
    children: Vec<ObjectRef>,
    count: usize,
    _page_memory: Option<MemoryLease>,
    _positions_memory: Option<MemoryLease>,
    _pending_memory: Option<MemoryLease>,
    _pending_keys_memory: Option<MemoryLease>,
    _pending_leases_memory: Option<MemoryLease>,
    _route_memory: Option<MemoryLease>,
    _children_memory: Option<MemoryLease>,
}

impl Pages {
    fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let (mut bytes, page_memory) = funded_vec(wire::PAGE_BYTES, budget)?;
        bytes.extend_from_slice(wire::PAGE_MAGIC);
        let positions_capacity = usize::try_from(super::super::MAX_POSITIONS_PER_TERM)
            .map_err(|_| IndexError::ResourceLimited)?
            .checked_mul(5)
            .ok_or(IndexError::ResourceLimited)?;
        let (positions, positions_memory) = funded_vec(positions_capacity, budget)?;
        let (pending, pending_memory) = funded_vec(PAGE_BATCH, budget)?;
        let (pending_keys, pending_keys_memory) = funded_vec(PAGE_BATCH, budget)?;
        let (pending_leases, pending_leases_memory) = funded_vec(PAGE_BATCH, budget)?;
        let (routes, route_memory) = funded_vec(wire::MAX_PAGES * PAGE_ROUTE_BYTES, budget)?;
        let (children, children_memory) = funded_vec(wire::MAX_PAGES, budget)?;
        Ok(Self {
            page: Page {
                bytes,
                first: None,
                last: None,
            },
            positions,
            pending,
            pending_keys,
            pending_leases,
            routes,
            children,
            count: 0,
            _page_memory: page_memory,
            _positions_memory: positions_memory,
            _pending_memory: pending_memory,
            _pending_keys_memory: pending_keys_memory,
            _pending_leases_memory: pending_leases_memory,
            _route_memory: route_memory,
            _children_memory: children_memory,
        })
    }

    fn install_pending(&mut self, context: &mut AttemptContext<'_>) -> Result<(), IndexError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        if self.pending.len() != self.pending_keys.len()
            || self.pending.len() != self.pending_leases.len()
        {
            return Err(IndexError::BackendViolation(
                "segment page batch custody differs".into(),
            ));
        }
        let references = context.put_batch(&self.pending)?;
        if references.len() != self.pending.len() {
            return Err(IndexError::BackendViolation(
                "segment page batch output differs".into(),
            ));
        }
        for ((&reference, &(first, last)), input) in
            references.iter().zip(&self.pending_keys).zip(&self.pending)
        {
            let first = first.bytes(&input.payload)?;
            let last = last.bytes(&input.payload)?;
            copy_work(context.budget, 32 + 4 + first.len() + last.len())?;
            wire::write_route(&mut self.routes, reference, first, last)?;
            self.children.push(reference);
            self.count = self
                .count
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
        }
        self.pending.clear();
        self.pending_keys.clear();
        self.pending_leases.clear();
        Ok(())
    }

    fn flush(&mut self, context: &mut AttemptContext<'_>) -> Result<(), IndexError> {
        let Some(first) = self.page.first else {
            return Ok(());
        };
        let last = self.page.last.ok_or(IndexError::ResourceLimited)?;
        if self
            .count
            .checked_add(self.pending.len())
            .is_none_or(|n| n >= wire::MAX_PAGES)
        {
            return Err(IndexError::ResourceLimited);
        }
        copy_work(context.budget, self.page.bytes.len())?;
        self.pending.push(ObjectBatchInput {
            payload: std::mem::take(&mut self.page.bytes),
            children: Vec::new(),
        });
        self.pending_keys.push((first, last));
        self.pending_leases.push(self._page_memory.take());
        self.page.first = None;
        self.page.last = None;
        if self.pending.len() == PAGE_BATCH {
            self.install_pending(context)?;
        }
        let (mut bytes, memory) = funded_vec(wire::PAGE_BYTES, context.budget)?;
        bytes.extend_from_slice(wire::PAGE_MAGIC);
        self.page.bytes = bytes;
        self._page_memory = memory;
        Ok(())
    }

    fn finish(&mut self, context: &mut AttemptContext<'_>) -> Result<(), IndexError> {
        self.flush(context)?;
        self.install_pending(context)
    }

    fn start_fragment(&mut self, term: &[u8]) -> Result<usize, IndexError> {
        let key = PageKey {
            start: self.page.bytes.len() + size_of::<u16>(),
            len: term.len(),
        };
        if self.page.first.is_none() {
            self.page.first = Some(key);
        }
        self.page.last = Some(key);
        wire::push_key(&mut self.page.bytes, term)?;
        let header = self.page.bytes.len();
        self.page.bytes.extend_from_slice(&[0; 6]);
        Ok(header)
    }

    fn finish_fragment(&mut self, header: usize, df: u16) -> Result<(), IndexError> {
        if df == 0 {
            return Err(IndexError::ResourceLimited);
        }
        let body = self
            .page
            .bytes
            .len()
            .checked_sub(header + 6)
            .ok_or(IndexError::ResourceLimited)?;
        let body = u32::try_from(body).map_err(|_| IndexError::ResourceLimited)?;
        self.page
            .bytes
            .get_mut(header..header + 2)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(&df.to_be_bytes());
        self.page
            .bytes
            .get_mut(header + 2..header + 6)
            .ok_or(IndexError::ResourceLimited)?
            .copy_from_slice(&body.to_be_bytes());
        Ok(())
    }
}

impl Posting {
    fn finish(&mut self, pages: &mut Pages) -> Result<(), IndexError> {
        if let Some(header) = self.header.take() {
            pages.finish_fragment(header, self.df)?;
        }
        self.df = 0;
        Ok(())
    }

    fn append_doc(
        &mut self,
        pages: &mut Pages,
        term: &[u8],
        doc: u16,
        tf: usize,
        context: &mut AttemptContext<'_>,
    ) -> Result<(), IndexError> {
        if tf == 0 {
            return Err(wire::corrupt("empty term document"));
        }
        let tf = u32::try_from(tf).map_err(|_| IndexError::ResourceLimited)?;
        let absolute = u32::from(doc);
        let mut delta = if self.df == 0 {
            absolute
        } else {
            u32::from(
                doc.checked_sub(self.previous_doc)
                    .filter(|value| *value > 0)
                    .ok_or_else(|| wire::corrupt("doc order"))?,
            )
        };
        let mut body_bytes = varint_len(delta) + varint_len(tf) + pages.positions.len();
        if self.header.is_some() && pages.page.bytes.len() + body_bytes > wire::PAGE_BYTES {
            self.finish(pages)?;
            pages.flush(context)?;
            delta = absolute;
            body_bytes = varint_len(delta) + varint_len(tf) + pages.positions.len();
        }
        if self.header.is_none() {
            let fragment_bytes = 8 + term.len() + body_bytes;
            if pages.page.bytes.len() + fragment_bytes > wire::PAGE_BYTES {
                pages.flush(context)?;
            }
            if pages.page.bytes.len() + fragment_bytes > wire::PAGE_BYTES {
                return Err(IndexError::ResourceLimited);
            }
            self.header = Some(pages.start_fragment(term)?);
        }
        wire::push_varint(&mut pages.page.bytes, delta);
        wire::push_varint(&mut pages.page.bytes, tf);
        pages.page.bytes.extend_from_slice(&pages.positions);
        self.df = self.df.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        self.previous_doc = doc;
        Ok(())
    }
}

fn append_position(
    positions: &mut Vec<u8>,
    previous: &mut Option<u32>,
    tf: &mut usize,
    position: u32,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let value = if let Some(previous) = *previous {
        position
            .checked_sub(previous)
            .filter(|delta| *delta > 0)
            .ok_or_else(|| wire::corrupt("position order"))?
    } else {
        position
    };
    if u64::try_from(*tf).map_err(|_| IndexError::ResourceLimited)?
        >= super::super::MAX_POSITIONS_PER_TERM
    {
        return Err(IndexError::ResourceLimited);
    }
    copy_work(budget, varint_len(value))?;
    wire::push_varint(positions, value);
    *previous = Some(position);
    *tf += 1;
    Ok(())
}

fn emit_postings(
    window: &Window,
    pages: &mut Pages,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    for &id in &window.order {
        context.budget.checkpoint()?;
        let term_row = window
            .terms
            .get(usize::try_from(id).map_err(|_| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        let term = window.key(term_row.key)?;
        let mut cursor = term_row.first;
        let mut posting = Posting::default();
        while cursor != NONE {
            context.budget.checkpoint()?;
            let first = window.row(cursor)?;
            let mut scan = cursor;
            let mut tf = 0usize;
            let mut previous_position = None;
            pages.positions.clear();
            while scan != NONE {
                let row = window.row(scan)?;
                if row.doc != first.doc {
                    break;
                }
                append_position(
                    &mut pages.positions,
                    &mut previous_position,
                    &mut tf,
                    row.position,
                    context.budget,
                )?;
                scan = row.next;
            }
            posting.append_doc(pages, term, first.doc, tf, context)?;
            cursor = scan;
        }
        posting.finish(pages)?;
    }
    pages.finish(context)
}

fn emit_spilled_postings(
    spill: SegmentSpill,
    pages: &mut Pages,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let mut posting = Posting::default();
    let mut previous = None;
    let mut tf = 0;
    let AttemptContext {
        attempt,
        objects,
        maps,
        custody,
        scratch,
        ..
    } = *context;
    spill.visit_sorted(context.budget, |event, budget| match event {
        SpillEvent::Position(position) => append_position(
            &mut pages.positions,
            &mut previous,
            &mut tf,
            position,
            budget,
        ),
        SpillEvent::EndDocument { key, doc } => {
            let mut scoped = AttemptContext {
                attempt,
                objects,
                maps,
                custody,
                scratch,
                budget,
            };
            posting.append_doc(pages, key, doc, tf, &mut scoped)?;
            pages.positions.clear();
            previous = None;
            tf = 0;
            Ok(())
        }
        SpillEvent::EndTerm => posting.finish(pages),
    })?;
    pages.finish(context)
}

fn seal_window(
    window: &mut Window,
    recipe: Digest,
    edges: &mut Vec<SegmentEdge>,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    if window.docs.is_empty() {
        return Ok(());
    }
    if edges.len() == wire::MAX_SEGMENTS {
        return Err(IndexError::ResourceLimited);
    }
    let mut pages = Pages::new(context.budget)?;
    if let Some(spill) = window.spill.take() {
        emit_spilled_postings(spill, &mut pages, context)?;
    } else {
        window.sort(context.budget)?;
        emit_postings(window, &mut pages, context)?;
    }
    let bytes = 8usize
        .checked_add(32 + 2 + 2)
        .and_then(|n| n.checked_add(window.docs.len() * wire::DOC_BYTES))
        .and_then(|n| n.checked_add(pages.routes.len()))
        .ok_or(IndexError::ResourceLimited)?;
    let (mut output, _memory) = funded_vec(bytes, context.budget)?;
    copy_work(context.budget, bytes)?;
    wire::write_segment(
        &mut output,
        recipe,
        &window.docs,
        pages.count,
        &pages.routes,
    )?;
    if output.len() != bytes {
        return Err(IndexError::ResourceLimited);
    }
    let reference = context.put(&output, &pages.children)?;
    let first = window
        .docs
        .first()
        .ok_or(IndexError::ResourceLimited)?
        .owner;
    let last = window.docs.last().ok_or(IndexError::ResourceLimited)?.owner;
    let tokens = window.docs.iter().try_fold(0u64, |sum, doc| {
        sum.checked_add(doc.tokens)
            .ok_or(IndexError::ResourceLimited)
    })?;
    copy_work(context.budget, size_of::<SegmentEdge>())?;
    edges.push(SegmentEdge {
        reference,
        first,
        last,
        docs: u16::try_from(window.docs.len()).map_err(|_| IndexError::ResourceLimited)?,
        tokens,
    });
    window.clear_for_next_segment()
}

pub(super) fn seal_observed(
    window: &mut Window,
    recipe: Digest,
    edges: &mut Vec<SegmentEdge>,
    cache: &mut Option<Box<TokenPrefixCache>>,
    context: &mut AttemptContext<'_>,
) -> Result<(), IndexError> {
    let diagnostic = context.budget.request().diagnostic_context().cloned();
    super::super::observe_attempt(
        context,
        diagnostic.as_ref(),
        quanta_index_core::DiagnosticStageV1::NativeTextPostings,
        |_child, context| {
            token_prefix_prepare::with_discardable_cache(cache, context, |context| {
                seal_window(window, recipe, edges, context)
            })
        },
    )
}

/// Produce one attempt-local FULL root. The caller publishes its returned ref.
pub(in crate::native::text) fn build(
    recipe: TextRecipe,
    source: SourceCut,
    input: DeltaInput<SourceDelta>,
    context: &mut AttemptContext<'_>,
) -> Result<(ObjectRef, Digest), IndexError> {
    if let DeltaInput::Patch(delta) = &input {
        if delta.next != source || delta.prior.owners.0.is_some() {
            return Err(IndexError::Conflict);
        }
    }
    let policy = recipe.output_policy();
    let (mut edges, _edge_memory) = funded_vec::<SegmentEdge>(wire::MAX_SEGMENTS, context.budget)?;
    let mut window: Option<Window> = None;
    let mut cache = None;
    let mut cache_initialized = false;
    let mut prior_owner: Option<[u8; 32]> = None;
    let mut docs = 0u64;
    let mut tokens = 0u64;
    let mut changes = SourceInputCursor::new(source, input, context.maps, context.objects)?;
    while let Some(change) = changes.next_change(context.budget)? {
        context.budget.checkpoint()?;
        if change.prior.is_some() {
            return Err(IndexError::Conflict);
        }
        let (reference, record) = change
            .next
            .ok_or_else(|| wire::corrupt("FULL owner deletion"))?;
        let owner = owner_key(change.owner);
        if prior_owner.is_some_and(|prior| prior >= owner) {
            return Err(wire::corrupt("owner order"));
        }
        prior_owner = Some(owner);
        let indexed = check_admission(&record, recipe)?;
        if !indexed {
            continue;
        }
        if !cache_initialized {
            cache = TokenPrefixCache::try_new(recipe, 64, context.budget)?;
            cache_initialized = true;
        }
        if window.as_ref().is_some_and(|w| !w.docs.is_empty())
            && window
                .as_ref()
                .ok_or(IndexError::ResourceLimited)?
                .should_seal_before(record.byte_len)?
        {
            seal_observed(
                window.as_mut().ok_or(IndexError::ResourceLimited)?,
                policy,
                &mut edges,
                &mut cache,
                context,
            )?;
        }
        let current = if let Some(current) = window.as_mut() {
            current
        } else {
            window.insert(Window::new(context.budget)?)
        };
        current.append_document(owner, reference, &record, &mut cache, context)?;
        let added = current
            .docs
            .last()
            .ok_or(IndexError::ResourceLimited)?
            .tokens;
        docs = docs.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        tokens = tokens
            .checked_add(added)
            .ok_or(IndexError::ResourceLimited)?;
    }
    if let Some(current) = window.as_mut() {
        seal_observed(current, policy, &mut edges, &mut cache, context)?;
    }
    drop(cache);
    drop(window);
    let diagnostic = context.budget.request().diagnostic_context().cloned();
    super::super::observe_attempt(
        context,
        diagnostic.as_ref(),
        quanta_index_core::DiagnosticStageV1::NativeTextRoot,
        |_child, context| {
            let cut = wire::TextCut::of(source);
            let root_bytes = 8usize
                .checked_add(32 + 2 * 33 + 2)
                .and_then(|n| n.checked_add(edges.len() * wire::ROOT_ROW_BYTES))
                .ok_or(IndexError::ResourceLimited)?;
            let (mut root, _root_memory) = funded_vec(root_bytes, context.budget)?;
            wire::write_root(&mut root, policy, cut, &edges)?;
            if root.len() != root_bytes - (2 - cut.edges().count()) * 32 {
                return Err(IndexError::ResourceLimited);
            }
            let child_count = cut
                .edges()
                .count()
                .checked_add(edges.len())
                .ok_or(IndexError::ResourceLimited)?;
            let (mut children, _children_memory) = funded_vec(child_count, context.budget)?;
            children.extend(cut.edges());
            children.extend(edges.iter().map(|edge| edge.reference));
            copy_work(context.budget, root.len())?;
            let artifact = context.put(&root, &children)?;
            Ok((
                artifact,
                wire::logical_commitment(policy, cut, docs, tokens),
            ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::index::{MemoryScope, MemoryScopePort, WorkLimits};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn empty_root_children_need_no_memory_lease() -> Result<(), IndexError> {
        struct RejectAllocation;

        impl MemoryScopePort for RejectAllocation {
            fn reserve(&self, _: u64, _: MemoryLifetime) -> Result<MemoryLease, IndexError> {
                Err(IndexError::BackendViolation(
                    "empty root children must not request a memory lease".into(),
                ))
            }
        }

        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            deadline,
        );
        budget.bind_memory_scope(Some(MemoryScope::new(Arc::new(RejectAllocation))));
        let (children, lease) = funded_vec::<ObjectRef>(0, &budget)?;
        assert!(children.is_empty());
        assert!(lease.is_none());
        budget.request().cancel_handle().cancel();
        assert!(matches!(
            funded_vec::<ObjectRef>(0, &budget),
            Err(IndexError::Cancelled)
        ));
        Ok(())
    }

    #[test]
    fn new_segment_ignores_stale_term_slots() -> Result<(), IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(5))
            .ok_or(IndexError::ResourceLimited)?;
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 100,
                writes: 100,
                bytes: 1_000_000,
                mutations: 100,
            },
            deadline,
        );
        let mut window = Window::new(&mut budget)?;
        assert_eq!(window.intern_term(b"\0other", &mut budget)?, Some(0));
        assert_eq!(window.intern_term(b"\0one", &mut budget)?, Some(1));
        let before_reset = budget.usage();
        window.clear_for_next_segment()?;
        assert_eq!(budget.usage(), before_reset);
        assert_eq!(window.intern_term(b"\0one", &mut budget)?, Some(0));
        assert_eq!(window.intern_term(b"\0one", &mut budget)?, Some(0));
        assert_eq!(window.intern_term(b"\0other", &mut budget)?, Some(1));
        assert_eq!(window.term_key(0)?, b"\0one");
        assert_eq!(window.term_key(1)?, b"\0other");
        Ok(())
    }
}
