//! Owner-local ordered token groups for an admitted cold source.
//!
//! The canonical normalizer still creates every case key and position. This
//! buffer only replaces the disposable SQL sort when optional memory is available.

use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage};
use quanta_index_lq_text_normalizer::{CaseMode, TokenCursor, case_chars};
use serde::ser::{SerializeSeq, SerializeStruct};
use std::cmp::Ordering;
use std::sync::Arc;

#[cfg(test)]
use super::TokenBatch;
use super::{
    MAX_POSITIONS_PER_TERM, MAX_TERM_KEY_BYTES, PreparedKey, PreparedPositions, check_term_key,
};
pub(super) use crate::native::text_memory::append_canonical_case_key;
use crate::native::text_memory::measured_encoded_len;
use crate::native::text_occurrence::block::{
    HEADER_BYTES, MAX_BLOCK_BYTES, MAX_BLOCK_TERMS, PackedBlockView, VectorIndex, next_block_len,
};

use super::entry_patch::{COLD_ALIAS_MIN_TERMS, COLD_INLINE_ANCHOR_KEY_BYTES};

const ROW_START_BITS: u32 = 28;
const ROW_POSITION_BITS: u32 = 24;
const ROW_LENGTH_LOW_BITS: u32 = 8;
const ROW_START_MASK: u32 = (1 << ROW_START_BITS) - 1;
const ROW_POSITION_MASK: u32 = (1 << ROW_POSITION_BITS) - 1;
const ROW_LENGTH_LOW_MASK: u32 = (1 << ROW_LENGTH_LOW_BITS) - 1;
const ROW_LENGTH_MASK: u32 = (1 << 10) - 1;
const _: () = assert!(super::MAX_NFC_BYTES * 14 < (1_u64 << ROW_START_BITS));
const _: () = assert!(super::MAX_NFC_BYTES <= (1_u64 << ROW_POSITION_BITS));
const _: () = assert!(MAX_TERM_KEY_BYTES <= (1_u64 << 10));

// A row retains the start and length instead of a separate end word. The
// 10-bit length minus one occupies the spare bits of the two used words.
#[derive(Clone, Copy)]
struct Row {
    start_and_length_high: u32,
    position_and_length_low: u32,
}

impl Row {
    fn new(start: u32, end: u32, position: u32) -> Result<Self, IndexError> {
        let length_minus_one = end
            .checked_sub(start)
            .and_then(|length| length.checked_sub(1))
            .filter(|length| (1..=ROW_LENGTH_MASK).contains(length))
            .ok_or(IndexError::ResourceLimited)?;
        if start > ROW_START_MASK || position > ROW_POSITION_MASK {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self {
            start_and_length_high: start
                | ((length_minus_one >> ROW_LENGTH_LOW_BITS) << ROW_START_BITS),
            position_and_length_low: position
                | ((length_minus_one & ROW_LENGTH_LOW_MASK) << ROW_POSITION_BITS),
        })
    }

    fn start(self) -> u32 {
        self.start_and_length_high & ROW_START_MASK
    }

    fn end(self) -> Option<u32> {
        let length_minus_one = ((self.start_and_length_high >> ROW_START_BITS)
            << ROW_LENGTH_LOW_BITS)
            | (self.position_and_length_low >> ROW_POSITION_BITS);
        self.start().checked_add(length_minus_one)?.checked_add(1)
    }

    fn position(self) -> u32 {
        self.position_and_length_low & ROW_POSITION_MASK
    }
}

/// Physical payload shape only; the canonical writer still owns every output.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ColdBlockShape {
    pub(super) object_count: usize,
    pub(super) total_payload_bytes: usize,
    pub(super) maximum_payload_bytes: usize,
    pub(super) maximum_key_bytes: usize,
    pub(super) term_count: usize,
    /// Maximum live owned key/vector bytes plus the incoming group's grow copy.
    pub(super) maximum_key_position_peak: usize,
    pub(super) maximum_group_positions: usize,
}

fn position_capacity(len: usize) -> Result<usize, IndexError> {
    if len == 0
        || u64::try_from(len).map_err(|_error| IndexError::ResourceLimited)?
            > MAX_POSITIONS_PER_TERM
    {
        return Err(IndexError::ResourceLimited);
    }
    len.checked_next_power_of_two()
        .ok_or(IndexError::ResourceLimited)
}

impl ColdBlockShape {
    fn record(&mut self, payload_bytes: usize) -> Result<(), IndexError> {
        self.object_count = self
            .object_count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.total_payload_bytes = self
            .total_payload_bytes
            .checked_add(payload_bytes)
            .ok_or(IndexError::ResourceLimited)?;
        self.maximum_payload_bytes = self.maximum_payload_bytes.max(payload_bytes);
        Ok(())
    }
}

#[derive(Default)]
struct AliasFootprint {
    terms: usize,
    term_bytes: usize,
    largest_vector: usize,
}

impl AliasFootprint {
    fn add_key(&mut self, key: &[u8]) -> Result<(), IndexError> {
        self.terms = self
            .terms
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        self.term_bytes = self
            .term_bytes
            .checked_add(key.len())
            .ok_or(IndexError::ResourceLimited)?;
        Ok(())
    }

    fn can_fit(&self) -> bool {
        // Every term needs its own row. Even with perfect vector reuse,
        // the largest vector must be present at least once.
        self.terms <= MAX_BLOCK_TERMS
            && u64::try_from(self.terms).is_ok_and(|terms| terms >= COLD_ALIAS_MIN_TERMS)
            && self
                .terms
                .checked_mul(4)
                .and_then(|bytes| bytes.checked_add(self.term_bytes))
                .and_then(|bytes| bytes.checked_add(HEADER_BYTES + 2))
                .and_then(|bytes| bytes.checked_add(self.largest_vector.checked_mul(4)?))
                .is_some_and(|minimum| minimum <= MAX_BLOCK_BYTES)
    }
}

/// Borrowed sequence with the same Serde structure as `Occurrence` and
/// `Positions`; it is used only by the canonical JSON length counter.
struct BorrowedOccurrence<'a>(&'a [Row]);

impl serde::Serialize for BorrowedOccurrence<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        struct Positions<'a>(&'a [Row]);
        impl serde::Serialize for Positions<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
                for row in self.0 {
                    sequence.serialize_element(&row.position())?;
                }
                sequence.end()
            }
        }
        let mut record = serializer.serialize_struct("Occurrence", 1)?;
        record.serialize_field("positions", &Positions(self.0))?;
        record.end()
    }
}

/// Exact backing for both canonical case surfaces of one source.
pub(super) struct CanonicalTokenCensus {
    rows: usize,
    key_bytes: usize,
    exact: bool,
}

impl CanonicalTokenCensus {
    pub(super) fn row_bound(&self) -> usize {
        self.rows
    }

    /// Bound the physical arena from the already admitted NFC byte length.
    /// Token runs are separated by at least one byte, so at most ceil(N/2)
    /// runs emit two case rows. On the pinned Rust toolchain, one lowercase
    /// scalar emits at most three Unicode scalars of at most four UTF-8 bytes.
    /// Sensitive keys therefore use at most N bytes and folded keys at most
    /// 12N bytes, plus one case prefix for each row.
    pub(super) fn upper_bound(normalized_bytes: usize) -> Result<Option<Self>, IndexError> {
        if normalized_bytes == 0
            || !u64::try_from(normalized_bytes).is_ok_and(|bytes| bytes <= super::MAX_NFC_BYTES)
        {
            return Ok(None);
        }
        let runs = normalized_bytes
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?
            .div_euclid(2);
        let rows = runs.checked_mul(2).ok_or(IndexError::ResourceLimited)?;
        let key_bytes = normalized_bytes
            .checked_mul(13)
            .and_then(|bytes| bytes.checked_add(rows))
            .ok_or(IndexError::ResourceLimited)?;
        if u32::try_from(rows).is_err() || u32::try_from(key_bytes).is_err() {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Some(Self {
            rows,
            key_bytes,
            exact: false,
        }))
    }

    pub(super) fn count<L>(
        cursor: &mut TokenCursor<'_, L>,
        source_bytes: usize,
        budget: &mut WorkBudget,
    ) -> Result<Option<Self>, IndexError> {
        cursor.rewind();
        if !u64::try_from(source_bytes).is_ok_and(|bytes| bytes <= super::MAX_SOURCE_BYTES) {
            return Ok(None);
        }
        // The normalized scan and case conversion are charged even when the
        // optional arena is denied. No second NFC backing is constructed.
        budget.charge(WorkUsage {
            bytes: u64::try_from(cursor.text().len())
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut rows = 0usize;
        let mut key_bytes = 0usize;
        let maximum =
            usize::try_from(MAX_TERM_KEY_BYTES).map_err(|_error| IndexError::ResourceLimited)?;
        while let Some(run) = cursor.next_run() {
            budget.checkpoint()?;
            if !run.is_indexable() {
                continue;
            }
            let _position =
                u32::try_from(run.position).map_err(|_error| IndexError::ResourceLimited)?;
            let mut run_key_bytes = 0usize;
            for case in [CaseMode::Sensitive, CaseMode::Folded] {
                let length = case_chars(run.text, case).try_fold(1usize, |bytes, ch| {
                    bytes
                        .checked_add(ch.len_utf8())
                        .filter(|next| *next <= maximum)
                        .ok_or(IndexError::ResourceLimited)
                })?;
                run_key_bytes = run_key_bytes
                    .checked_add(length)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            budget.charge(WorkUsage {
                bytes: u64::try_from(run_key_bytes)
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            rows = rows.checked_add(2).ok_or(IndexError::ResourceLimited)?;
            key_bytes = key_bytes
                .checked_add(run_key_bytes)
                .ok_or(IndexError::ResourceLimited)?;
        }
        cursor.rewind();
        if rows == 0 {
            return Ok(None);
        }
        if u32::try_from(rows).is_err() || u32::try_from(key_bytes).is_err() {
            return Err(IndexError::BackendViolation(
                "canonical token census exceeds u32 offsets".into(),
            ));
        }
        Ok(Some(Self {
            rows,
            key_bytes,
            exact: true,
        }))
    }

    pub(super) fn arena_bytes(&self) -> Result<usize, IndexError> {
        self.rows
            .checked_mul(std::mem::size_of::<Row>())
            .and_then(|rows| rows.checked_add(self.key_bytes))
            .ok_or(IndexError::ResourceLimited)
    }
}

#[derive(Clone, Copy, Default)]
struct BorrowedTerm {
    key_start: u32,
    key_end: u32,
    row_start: u32,
    row_end: u32,
    vector: u16,
}

/// A single packed candidate borrows the already sorted token arena. Its
/// fixed metadata is owner-local and cannot outlive `ColdTokens`.
pub(super) struct ColdPackedAlias<'a> {
    tokens: &'a ColdTokens,
    terms: [BorrowedTerm; MAX_BLOCK_TERMS],
    vectors: [u16; MAX_BLOCK_TERMS],
    index: VectorIndex,
    term_count: usize,
    vector_count: usize,
    encoded_len: usize,
    _memory: MemoryLease,
}

impl ColdPackedAlias<'_> {
    pub(super) fn encoded_len(&self) -> usize {
        self.encoded_len
    }

    #[cfg(test)]
    pub(super) fn first_key(&self) -> Result<&[u8], IndexError> {
        self.term(0).map(|(key, _vector)| key)
    }

    fn rows_for(&self, ordinal: usize) -> Result<&[Row], IndexError> {
        let term = self.terms.get(ordinal).ok_or(IndexError::ResourceLimited)?;
        self.tokens
            .rows
            .get(
                usize::try_from(term.row_start).map_err(|_error| IndexError::ResourceLimited)?
                    ..usize::try_from(term.row_end)
                        .map_err(|_error| IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)
    }
}

impl PackedBlockView for ColdPackedAlias<'_> {
    fn term_count(&self) -> usize {
        self.term_count
    }

    fn vector_count(&self) -> usize {
        self.vector_count
    }

    fn term(&self, ordinal: usize) -> Result<(&[u8], u16), IndexError> {
        let term = self.terms.get(ordinal).ok_or(IndexError::ResourceLimited)?;
        let key = self
            .tokens
            .keys
            .get(
                usize::try_from(term.key_start).map_err(|_error| IndexError::ResourceLimited)?
                    ..usize::try_from(term.key_end)
                        .map_err(|_error| IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        Ok((key, term.vector))
    }

    fn vector_len(&self, ordinal: usize) -> Result<usize, IndexError> {
        let term = usize::from(
            *self
                .vectors
                .get(ordinal)
                .ok_or(IndexError::ResourceLimited)?,
        );
        Ok(self.rows_for(term)?.len())
    }

    fn position(&self, vector: usize, ordinal: usize) -> Result<u32, IndexError> {
        let term = usize::from(
            *self
                .vectors
                .get(vector)
                .ok_or(IndexError::ResourceLimited)?,
        );
        Ok(self
            .rows_for(term)?
            .get(ordinal)
            .ok_or(IndexError::ResourceLimited)?
            .position())
    }
}

pub(super) struct ColdTokens {
    keys: Vec<u8>,
    rows: Vec<Row>,
    max_rows: usize,
    max_key_bytes: usize,
    alias_can_fit: bool,
    exact: bool,
    ready: bool,
    // Both Vec backings die before their one owner-local optional grant.
    _memory: MemoryLease,
    // The Arc header is funded independently when this arena joins a program.
    shared_memory: Option<MemoryLease>,
}

impl ColdTokens {
    pub(super) fn into_shared_with_memory(
        mut self,
        memory: MemoryLease,
    ) -> Result<Arc<Self>, IndexError> {
        if memory.bytes()
            < u64::try_from(
                MemoryLease::shared_allocation_backing_layout(std::alloc::Layout::new::<Self>())?
                    .size(),
            )
            .map_err(|_error| IndexError::ResourceLimited)?
        {
            return Err(IndexError::BackendViolation(
                "cold token shared backing exceeds grant".into(),
            ));
        }
        self.shared_memory = Some(memory);
        Ok(Arc::new(self))
    }

    /// Encode the remaining normalized runs directly into the admitted arena.
    pub(super) fn fill_from_cursor(
        &mut self,
        cursor: &mut TokenCursor<'_, Option<MemoryLease>>,
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        if self.ready || !self.rows.is_empty() || !self.keys.is_empty() {
            return Err(IndexError::BackendViolation(
                "cold direct source was already used".into(),
            ));
        }
        let mut indexed = 0u64;
        while let Some(run) = cursor.next_run() {
            budget.checkpoint()?;
            if !run.is_indexable() {
                continue;
            }
            let position =
                u32::try_from(run.position).map_err(|_error| IndexError::ResourceLimited)?;
            let next_rows = self
                .rows
                .len()
                .checked_add(2)
                .ok_or(IndexError::ResourceLimited)?;
            if next_rows > self.max_rows || next_rows > self.rows.capacity() {
                return Err(IndexError::BackendViolation(
                    "cold direct rows exceeded exact census".into(),
                ));
            }
            // One run scan, two case encodes, and two Row writes remain. The
            // old 128-byte packet work and a second key copy no longer occur.
            let row_bytes = 2usize
                .checked_mul(std::mem::size_of::<Row>())
                .ok_or(IndexError::ResourceLimited)?;
            let row_bytes =
                u64::try_from(row_bytes).map_err(|_error| IndexError::ResourceLimited)?;
            let bytes = u64::try_from(run.text.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .checked_mul(4)
                .and_then(|bytes| bytes.checked_add(row_bytes))
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                mutations: 1,
                bytes,
                ..WorkUsage::default()
            })?;
            for case in [CaseMode::Sensitive, CaseMode::Folded] {
                let start =
                    u32::try_from(self.keys.len()).map_err(|_error| IndexError::ResourceLimited)?;
                append_canonical_case_key(&mut self.keys, run.text, case, self.max_key_bytes)?;
                let end =
                    u32::try_from(self.keys.len()).map_err(|_error| IndexError::ResourceLimited)?;
                self.rows.push(Row::new(start, end, position)?);
            }
            indexed = indexed.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        if self.rows.len() > self.max_rows || self.keys.len() > self.max_key_bytes {
            return Err(IndexError::BackendViolation(
                "cold direct source exceeded its proved input bound".into(),
            ));
        }
        if self.exact && (self.rows.len() != self.max_rows || self.keys.len() != self.max_key_bytes)
        {
            return Err(IndexError::BackendViolation(
                "cold direct source differs from canonical census".into(),
            ));
        }
        Ok(indexed)
    }

    /// Optional one-block alias preparation. A shape miss has no SQL or object
    /// effect and must use the original grouped `BlockDraft` path.
    pub(super) fn try_packed_alias(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<Option<ColdPackedAlias<'_>>, IndexError> {
        budget.checkpoint()?;
        if !self.ready {
            return Err(IndexError::BackendViolation(
                "cold token groups were not ordered".into(),
            ));
        }
        if !self.alias_can_fit {
            return Ok(None);
        }
        let metadata_bytes = std::mem::size_of::<ColdPackedAlias<'_>>();
        let Some(memory) = budget.try_reserve_memory_for_optimization(
            u64::try_from(metadata_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(None);
        };
        let mut candidate = ColdPackedAlias {
            tokens: self,
            terms: [BorrowedTerm::default(); MAX_BLOCK_TERMS],
            vectors: [0; MAX_BLOCK_TERMS],
            index: VectorIndex::within_owner_grant(),
            term_count: 0,
            vector_count: 0,
            encoded_len: HEADER_BYTES,
            _memory: memory,
        };
        let mut start = 0usize;
        while start < self.rows.len() {
            budget.checkpoint()?;
            if candidate.term_count == MAX_BLOCK_TERMS {
                return Ok(None);
            }
            let first = self.rows.get(start).ok_or(IndexError::ResourceLimited)?;
            let key = Self::key(&self.keys, first).ok_or(IndexError::ResourceLimited)?;
            // The direct fast path emits an inline alias. Only its first
            // persisted anchor key has the fixed SmallMap width.
            if start == 0 && key.len() > COLD_INLINE_ANCHOR_KEY_BYTES {
                return Ok(None);
            }
            check_term_key(key)?;
            let mut end = start;
            while let Some(row) = self.rows.get(end) {
                budget.checkpoint()?;
                if Self::key(&self.keys, row).ok_or(IndexError::ResourceLimited)? != key {
                    break;
                }
                end = end.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            let positions = self
                .rows
                .get(start..end)
                .ok_or(IndexError::ResourceLimited)?;
            if positions.is_empty()
                || u64::try_from(positions.len()).map_err(|_error| IndexError::ResourceLimited)?
                    > MAX_POSITIONS_PER_TERM
            {
                return Err(IndexError::BackendViolation(
                    "cold token group exceeded its validated envelope".into(),
                ));
            }
            let inspected = key
                .len()
                .checked_add(
                    positions
                        .len()
                        .checked_mul(std::mem::size_of::<u32>())
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                bytes: u64::try_from(inspected).map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let (same, insertion) = candidate.index.locate(budget, |prior| {
                let term = usize::from(
                    *candidate
                        .vectors
                        .get(prior)
                        .ok_or(IndexError::ResourceLimited)?,
                );
                Ok(candidate
                    .rows_for(term)?
                    .iter()
                    .map(|row| row.position())
                    .cmp(positions.iter().map(|row| row.position())))
            })?;
            let vector = match same {
                Some(vector) => {
                    u16::try_from(vector).map_err(|_error| IndexError::ResourceLimited)?
                }
                None => u16::try_from(candidate.vector_count)
                    .map_err(|_error| IndexError::ResourceLimited)?,
            };
            let row_bytes = 4usize
                .checked_add(key.len())
                .ok_or(IndexError::ResourceLimited)?;
            let vector_bytes = if same.is_some() {
                0
            } else {
                positions
                    .len()
                    .checked_mul(4)
                    .and_then(|bytes| bytes.checked_add(2))
                    .ok_or(IndexError::ResourceLimited)?
            };
            let encoded_len = candidate
                .encoded_len
                .checked_add(row_bytes)
                .and_then(|bytes| bytes.checked_add(vector_bytes))
                .ok_or(IndexError::ResourceLimited)?;
            if encoded_len > MAX_BLOCK_BYTES {
                return Ok(None);
            }
            let term_ordinal = candidate.term_count;
            *candidate
                .terms
                .get_mut(term_ordinal)
                .ok_or(IndexError::ResourceLimited)? = BorrowedTerm {
                key_start: first.start(),
                key_end: first.end().ok_or(IndexError::ResourceLimited)?,
                row_start: u32::try_from(start).map_err(|_error| IndexError::ResourceLimited)?,
                row_end: u32::try_from(end).map_err(|_error| IndexError::ResourceLimited)?,
                vector,
            };
            candidate.term_count = candidate
                .term_count
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            candidate.encoded_len = encoded_len;
            if same.is_none() {
                *candidate
                    .vectors
                    .get_mut(candidate.vector_count)
                    .ok_or(IndexError::ResourceLimited)? =
                    u16::try_from(term_ordinal).map_err(|_error| IndexError::ResourceLimited)?;
                candidate.index.insert(insertion, vector)?;
                candidate.vector_count = candidate
                    .vector_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            start = end;
        }
        if u64::try_from(candidate.term_count).map_err(|_error| IndexError::ResourceLimited)?
            < COLD_ALIAS_MIN_TERMS
        {
            return Ok(None);
        }
        Ok(Some(candidate))
    }

    pub(super) fn try_new(
        census: &CanonicalTokenCensus,
        budget: &WorkBudget,
    ) -> Result<Option<Self>, IndexError> {
        if !u32::try_from(census.key_bytes).is_ok_and(|bytes| bytes <= ROW_START_MASK) {
            return Ok(None);
        }
        let bytes = census.arena_bytes()?;
        let Some(memory) = budget.try_reserve_memory_for_optimization(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(None);
        };
        let mut keys = Vec::new();
        if keys.try_reserve_exact(census.key_bytes).is_err() {
            return Ok(None);
        }
        let mut rows = Vec::new();
        if rows.try_reserve_exact(census.rows).is_err() {
            return Ok(None);
        }
        let actual = keys
            .capacity()
            .checked_add(
                rows.capacity()
                    .checked_mul(std::mem::size_of::<Row>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        if actual > bytes {
            return Ok(None);
        }
        budget.checkpoint()?;
        Ok(Some(Self {
            keys,
            rows,
            max_rows: census.rows,
            max_key_bytes: census.key_bytes,
            alias_can_fit: false,
            exact: census.exact,
            ready: false,
            _memory: memory,
            shared_memory: None,
        }))
    }

    /// Replay one SQL-sized page at a time from the validated arena. The
    /// page contains only borrowed keys and positions; no second key or row
    /// backing is allocated while the arena's Original grant is live.
    pub(super) fn replay_borrowed_pages(
        &self,
        budget: &mut WorkBudget,
        mut consume: impl FnMut(&[(&[u8], u32)], &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if !self.ready {
            return Err(IndexError::BackendViolation(
                "cold replay requires a validated canonical arena".into(),
            ));
        }
        let mut page = [(&[][..], 0_u32); quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];
        for rows in self.rows.chunks(page.len()) {
            let entries = page
                .get_mut(..rows.len())
                .ok_or(IndexError::ResourceLimited)?;
            for (entry, row) in entries.iter_mut().zip(rows) {
                budget.checkpoint()?;
                let key = Self::key(&self.keys, row).ok_or(IndexError::ResourceLimited)?;
                check_term_key(key)?;
                *entry = (key, row.position());
            }
            consume(entries, budget)?;
        }
        budget.checkpoint()
    }

    #[cfg(test)]
    pub(super) fn push_batch(
        &mut self,
        batch: &TokenBatch,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.ready {
            return Err(IndexError::BackendViolation(
                "cold token buffer changed after ordering".into(),
            ));
        }
        for token in batch.rows() {
            budget.checkpoint()?;
            let key = batch.term(token)?;
            check_term_key(key)?;
            let end = self
                .keys
                .len()
                .checked_add(key.len())
                .ok_or(IndexError::ResourceLimited)?;
            if self.rows.len() >= self.max_rows || end > self.max_key_bytes {
                return Err(IndexError::BackendViolation(
                    "cold token census exceeded its proved input bound".into(),
                ));
            }
            budget.charge(WorkUsage {
                bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let start =
                u32::try_from(self.keys.len()).map_err(|_error| IndexError::ResourceLimited)?;
            let end = u32::try_from(end).map_err(|_error| IndexError::ResourceLimited)?;
            let row = Row::new(start, end, token.position())?;
            self.keys.extend_from_slice(key);
            self.rows.push(row);
        }
        Ok(())
    }

    fn key<'a>(keys: &'a [u8], row: &Row) -> Option<&'a [u8]> {
        let Ok(start) = usize::try_from(row.start()) else {
            return None;
        };
        let Ok(end) = usize::try_from(row.end()?) else {
            return None;
        };
        keys.get(start..end)
    }

    fn compare(keys: &[u8], left: &Row, right: &Row) -> Ordering {
        Self::key(keys, left)
            .cmp(&Self::key(keys, right))
            .then_with(|| left.position().cmp(&right.position()))
    }

    fn sort_checkpoint(
        comparisons: &mut usize,
        checkpoint: &mut impl FnMut() -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        *comparisons = comparisons
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if (*comparisons).is_multiple_of(1_024) {
            checkpoint()?;
        }
        Ok(())
    }

    fn sift_down(
        rows: &mut [Row],
        keys: &[u8],
        mut root: usize,
        end: usize,
        comparisons: &mut usize,
        checkpoint: &mut impl FnMut() -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        while let Some(left) = root
            .checked_mul(2)
            .and_then(|index| index.checked_add(1))
            .filter(|index| *index < end)
        {
            Self::sort_checkpoint(comparisons, checkpoint)?;
            let mut selected = root;
            if Self::compare(
                keys,
                rows.get(selected).ok_or(IndexError::ResourceLimited)?,
                rows.get(left).ok_or(IndexError::ResourceLimited)?,
            ) == Ordering::Less
            {
                selected = left;
            }
            if let Some(right) = left.checked_add(1).filter(|index| *index < end) {
                Self::sort_checkpoint(comparisons, checkpoint)?;
                if Self::compare(
                    keys,
                    rows.get(selected).ok_or(IndexError::ResourceLimited)?,
                    rows.get(right).ok_or(IndexError::ResourceLimited)?,
                ) == Ordering::Less
                {
                    selected = right;
                }
            }
            if selected == root {
                return Ok(());
            }
            rows.swap(root, selected);
            root = selected;
        }
        Ok(())
    }

    fn sort_rows(
        rows: &mut [Row],
        keys: &[u8],
        mut checkpoint: impl FnMut() -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        checkpoint()?;
        // The standard sort remains bounded for ordinary small sources.
        // Larger inputs use an in-place heap: no second row arena, worst-case
        // O(n log n), and cancellation is observed every 1,024 comparisons.
        if rows.len() <= 2_048 {
            rows.sort_unstable_by(|left, right| Self::compare(keys, left, right));
            return checkpoint();
        }
        let mut comparisons = 0usize;
        let len = rows.len();
        for root in (0..len.div_euclid(2)).rev() {
            Self::sift_down(rows, keys, root, len, &mut comparisons, &mut checkpoint)?;
        }
        for end in (1..rows.len()).rev() {
            rows.swap(0, end);
            Self::sift_down(rows, keys, 0, end, &mut comparisons, &mut checkpoint)?;
        }
        checkpoint()
    }

    /// Finish the full ordered stream before any immutable object or map edit
    /// can be installed. Duplicate `(key,position)` has the same refusal as the
    /// SQL token table's primary key.
    pub(super) fn sort_and_validate(&mut self, budget: &WorkBudget) -> Result<(), IndexError> {
        self.sort_and_validate_positions(budget, |_slot, _position| Ok(()))
    }

    fn sort_and_validate_positions(
        &mut self,
        budget: &WorkBudget,
        mut visit: impl FnMut(usize, u32) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        let keys = self.keys.as_slice();
        Self::sort_rows(&mut self.rows, keys, || budget.checkpoint())?;
        let mut prior: Option<&Row> = None;
        let mut positions = 0usize;
        let mut alias = AliasFootprint::default();
        for (slot, row) in self.rows.iter().enumerate() {
            budget.checkpoint()?;
            let key = Self::key(keys, row).ok_or(IndexError::ResourceLimited)?;
            check_term_key(key)?;
            if let Some(previous) = prior
                && Self::key(keys, previous).ok_or(IndexError::ResourceLimited)? == key
            {
                if previous.position() >= row.position() {
                    return Err(IndexError::CorruptObject(
                        "cold token position repeats".into(),
                    ));
                }
                positions = positions
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            } else {
                alias.largest_vector = alias.largest_vector.max(positions);
                positions = 1;
                alias.add_key(key)?;
            }
            if u64::try_from(positions).map_err(|_error| IndexError::ResourceLimited)?
                > MAX_POSITIONS_PER_TERM
            {
                return Err(IndexError::ResourceLimited);
            }
            visit(slot, row.position())?;
            prior = Some(row);
        }
        budget.checkpoint()?;
        alias.largest_vector = alias.largest_vector.max(positions);
        self.alias_can_fit = alias.can_fit();
        self.ready = true;
        Ok(())
    }

    /// Walk the already sorted canonical input without key, position, payload,
    /// object, or SQL allocation. The writer's split arithmetic and JSON
    /// length counter determine the exact physical output shape.
    pub(super) fn block_shape(&self, budget: &WorkBudget) -> Result<ColdBlockShape, IndexError> {
        budget.checkpoint()?;
        if !self.ready {
            return Err(IndexError::BackendViolation(
                "cold token groups were not ordered".into(),
            ));
        }
        let mut shape = ColdBlockShape::default();
        let mut vectors = [BorrowedTerm::default(); MAX_BLOCK_TERMS];
        let mut index = VectorIndex::within_owner_grant();
        let mut vector_count = 0usize;
        let mut block_terms = 0usize;
        let mut block_len = HEADER_BYTES;
        let mut block_key_bytes = 0usize;
        let mut block_position_bytes = 0usize;
        let mut only_positions = (0usize, 0usize);
        let mut start = 0usize;
        while start < self.rows.len() {
            budget.checkpoint()?;
            let first = self.rows.get(start).ok_or(IndexError::ResourceLimited)?;
            let key = Self::key(&self.keys, first).ok_or(IndexError::ResourceLimited)?;
            check_term_key(key)?;
            let mut end = start;
            while let Some(row) = self.rows.get(end) {
                budget.checkpoint()?;
                if Self::key(&self.keys, row).ok_or(IndexError::ResourceLimited)? != key {
                    break;
                }
                end = end.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            let positions = self
                .rows
                .get(start..end)
                .ok_or(IndexError::ResourceLimited)?;
            if positions.is_empty()
                || u64::try_from(positions.len()).map_err(|_error| IndexError::ResourceLimited)?
                    > MAX_POSITIONS_PER_TERM
            {
                return Err(IndexError::BackendViolation(
                    "cold token group exceeded its validated envelope".into(),
                ));
            }
            shape.term_count = shape
                .term_count
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            shape.maximum_key_bytes = shape.maximum_key_bytes.max(key.len());
            shape.maximum_group_positions = shape.maximum_group_positions.max(positions.len());
            loop {
                let capacity = position_capacity(positions.len())?;
                let group_growth_bytes = capacity
                    .checked_add(capacity.div_euclid(2))
                    .and_then(|positions| positions.checked_mul(std::mem::size_of::<u32>()))
                    .ok_or(IndexError::ResourceLimited)?;
                let live = block_key_bytes
                    .checked_add(block_position_bytes)
                    .and_then(|bytes| bytes.checked_add(key.len()))
                    .and_then(|bytes| bytes.checked_add(group_growth_bytes))
                    .ok_or(IndexError::ResourceLimited)?;
                shape.maximum_key_position_peak = shape.maximum_key_position_peak.max(live);
                let (same, insertion) = index.locate(budget, |ordinal| {
                    let prior = vectors.get(ordinal).ok_or(IndexError::ResourceLimited)?;
                    let rows = self
                        .rows
                        .get(
                            usize::try_from(prior.row_start)
                                .map_err(|_error| IndexError::ResourceLimited)?
                                ..usize::try_from(prior.row_end)
                                    .map_err(|_error| IndexError::ResourceLimited)?,
                        )
                        .ok_or(IndexError::ResourceLimited)?;
                    Ok(rows
                        .iter()
                        .map(|row| row.position())
                        .cmp(positions.iter().map(|row| row.position())))
                })?;
                if let Some(next) = next_block_len(
                    block_len,
                    block_terms,
                    key.len(),
                    positions.len(),
                    same.is_none(),
                )? {
                    block_len = next;
                    block_key_bytes = block_key_bytes
                        .checked_add(key.len())
                        .ok_or(IndexError::ResourceLimited)?;
                    block_terms = block_terms
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                    if block_terms == 1 {
                        only_positions = (start, end);
                    }
                    if same.is_none() {
                        block_position_bytes = block_position_bytes
                            .checked_add(
                                capacity
                                    .checked_mul(std::mem::size_of::<u32>())
                                    .ok_or(IndexError::ResourceLimited)?,
                            )
                            .ok_or(IndexError::ResourceLimited)?;
                        *vectors
                            .get_mut(vector_count)
                            .ok_or(IndexError::ResourceLimited)? = BorrowedTerm {
                            row_start: u32::try_from(start)
                                .map_err(|_error| IndexError::ResourceLimited)?,
                            row_end: u32::try_from(end)
                                .map_err(|_error| IndexError::ResourceLimited)?,
                            ..BorrowedTerm::default()
                        };
                        vector_count = vector_count
                            .checked_add(1)
                            .ok_or(IndexError::ResourceLimited)?;
                        index.insert(
                            insertion,
                            u16::try_from(
                                vector_count
                                    .checked_sub(1)
                                    .ok_or(IndexError::ResourceLimited)?,
                            )
                            .map_err(|_error| IndexError::ResourceLimited)?,
                        )?;
                    }
                    break;
                }
                if block_terms == 0 {
                    shape.record(measured_encoded_len(
                        &BorrowedOccurrence(positions),
                        budget,
                    )?)?;
                    break;
                }
                let payload_bytes = if block_terms == 1 {
                    measured_encoded_len(
                        &BorrowedOccurrence(
                            self.rows
                                .get(only_positions.0..only_positions.1)
                                .ok_or(IndexError::ResourceLimited)?,
                        ),
                        budget,
                    )?
                } else {
                    block_len
                };
                shape.record(payload_bytes)?;
                block_terms = 0;
                vector_count = 0;
                block_key_bytes = 0;
                block_position_bytes = 0;
                index = VectorIndex::within_owner_grant();
                block_len = HEADER_BYTES;
            }
            start = end;
        }
        if block_terms != 0 {
            let payload_bytes = if block_terms == 1 {
                measured_encoded_len(
                    &BorrowedOccurrence(
                        self.rows
                            .get(only_positions.0..only_positions.1)
                            .ok_or(IndexError::ResourceLimited)?,
                    ),
                    budget,
                )?
            } else {
                block_len
            };
            shape.record(payload_bytes)?;
        }
        budget.checkpoint()?;
        Ok(shape)
    }

    pub(super) fn visit_groups(
        &self,
        limit: usize,
        budget: &mut WorkBudget,
        mut consume: impl FnMut(
            PreparedKey,
            PreparedPositions,
            &mut WorkBudget,
        ) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        budget.checkpoint()?;
        if !self.ready {
            return Err(IndexError::BackendViolation(
                "cold token groups were not ordered".into(),
            ));
        }
        let mut current: Option<(PreparedKey, PreparedPositions)> = None;
        for row in &self.rows {
            budget.checkpoint()?;
            let key = Self::key(&self.keys, row).ok_or(IndexError::ResourceLimited)?;
            if current
                .as_ref()
                .is_some_and(|(prior, _)| prior.as_slice() != key)
            {
                let (prior, positions) = current.take().ok_or(IndexError::ResourceLimited)?;
                consume(prior, positions, budget)?;
            }
            if current.is_none() {
                current = Some((
                    PreparedKey::copy(key, budget)?,
                    PreparedPositions::new(limit)?,
                ));
            }
            current
                .as_mut()
                .ok_or(IndexError::ResourceLimited)?
                .1
                .push(row.position(), budget)?;
        }
        if let Some((key, positions)) = current {
            consume(key, positions, budget)?;
        }
        budget.checkpoint()
    }
}

mod program;
pub(super) use program::{ColdTokenProgram, ColdTokenProgramView};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::text::SourceRun;
    use crate::native::text_occurrence::block::{
        BlockDraft, BlockPush, SealedBlock, encode_packed,
    };
    use quanta_index_core::index::{MemoryLeaseOwner, MemoryScope, MemoryScopePort, WorkLimits};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    #[derive(Default)]
    struct Meter {
        live: AtomicU64,
        ceiling: AtomicU64,
        reservations: AtomicU64,
    }

    impl MemoryLeaseOwner for Meter {
        fn release(&self, bytes: u64) {
            let previous = self.live.fetch_sub(bytes, Ordering::Relaxed);
            assert!(
                previous >= bytes,
                "cold token memory released without admission"
            );
        }
    }

    struct Scope(Arc<Meter>);

    impl MemoryScopePort for Scope {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, IndexError> {
            let _previous = self
                .0
                .live
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                    live.checked_add(bytes)
                        .filter(|next| *next <= self.0.ceiling.load(Ordering::Relaxed))
                })
                .map_err(|_error| IndexError::ResourceLimited)?;
            let _prior_reservations = self.0.reservations.fetch_add(1, Ordering::Relaxed);
            Ok(MemoryLease::new(bytes, self.0.clone()))
        }
    }

    fn budget() -> Result<(WorkBudget, Arc<Meter>), IndexError> {
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(30))
            .ok_or(IndexError::ResourceLimited)?;
        let mut work = WorkBudget::new(
            WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 100_000_000,
                mutations: 1_000_000,
            },
            deadline,
        );
        let meter = Arc::new(Meter::default());
        meter.ceiling.store(1_000_000, Ordering::Relaxed);
        work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Scope(meter.clone())))));
        Ok((work, meter))
    }

    fn source_runs() -> [SourceRun; 5] {
        [
            SourceRun {
                start: 0,
                end: 1,
                position: 0,
            },
            SourceRun {
                start: 2,
                end: 3,
                position: 1,
            },
            SourceRun {
                start: 4,
                end: 5,
                position: 2,
            },
            SourceRun {
                start: 6,
                end: 7,
                position: 3,
            },
            SourceRun {
                start: 8,
                end: 9,
                position: 4,
            },
        ]
    }

    fn exact_census(
        source: &str,
        budget: &mut WorkBudget,
    ) -> Result<CanonicalTokenCensus, IndexError> {
        let mut cursor = TokenCursor::new(source);
        CanonicalTokenCensus::count(&mut cursor, source.len(), budget)?
            .ok_or(IndexError::ResourceLimited)
    }

    #[test]
    fn compact_row_roundtrips_boundaries_and_rejects_overflow() -> Result<(), IndexError> {
        for (start, end, position) in [
            (0, 2, 0),
            (65_535, 65_806, 7_654_321),
            (268_435_455, 268_436_479, 16_777_215),
        ] {
            let row = Row::new(start, end, position)?;
            if (row.start(), row.end(), row.position()) != (start, Some(end), position) {
                return Err(IndexError::BackendViolation(
                    "compact token row differs".into(),
                ));
            }
        }
        for (start, end, position) in [
            (268_435_456, 268_435_458, 0),
            (0, 1_025, 0),
            (0, 2, 16_777_216),
            (0, 1, 0),
            (2, 1, 0),
        ] {
            if !matches!(
                Row::new(start, end, position),
                Err(IndexError::ResourceLimited)
            ) {
                return Err(IndexError::BackendViolation(
                    "compact token row accepted fields outside its range".into(),
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn unicode_upper_bound_covers_exact_runs_and_checked_limits() -> Result<(), IndexError> {
        if CanonicalTokenCensus::upper_bound(0)?.is_some()
            || CanonicalTokenCensus::upper_bound(usize::MAX)?.is_some()
            || CanonicalTokenCensus::upper_bound(
                usize::try_from(super::super::MAX_NFC_BYTES)
                    .map_err(|_error| IndexError::ResourceLimited)?
                    + 1,
            )?
            .is_some()
        {
            return Err(IndexError::BackendViolation(
                "upper census accepted empty or over-limit NFC length".into(),
            ));
        }
        let one = CanonicalTokenCensus::upper_bound(1)?.ok_or(IndexError::ResourceLimited)?;
        if (one.rows, one.key_bytes, one.arena_bytes()?) != (2, 15, 31) {
            return Err(IndexError::BackendViolation(
                "one-byte Unicode upper census changed".into(),
            ));
        }
        let maximum = usize::try_from(super::super::MAX_NFC_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?;
        let largest =
            CanonicalTokenCensus::upper_bound(maximum)?.ok_or(IndexError::ResourceLimited)?;
        if (largest.rows, largest.key_bytes, largest.arena_bytes()?)
            != (maximum, 14 * maximum, 22 * maximum)
        {
            return Err(IndexError::BackendViolation(
                "maximum Unicode upper census changed".into(),
            ));
        }
        for source in ["a", "a b", "Alpha alpha β", "İ café Σ", "a!b!c", "𐐀 İ"] {
            let (mut work, _meter) = budget()?;
            let mut cursor = TokenCursor::new(source);
            let bound = CanonicalTokenCensus::upper_bound(cursor.text().len())?
                .ok_or(IndexError::ResourceLimited)?;
            let exact = CanonicalTokenCensus::count(&mut cursor, source.len(), &mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            if bound.exact
                || !exact.exact
                || bound.rows < exact.rows
                || bound.key_bytes < exact.key_bytes
                || bound.arena_bytes()? < exact.arena_bytes()?
            {
                return Err(IndexError::BackendViolation(format!(
                    "Unicode upper census did not cover {source:?}"
                )));
            }
        }
        Ok(())
    }

    #[test]
    fn upper_bound_direct_fill_preserves_case_groups_and_refunds() -> Result<(), IndexError> {
        use std::collections::BTreeMap;

        let (mut work, meter) = budget()?;
        let mut cursor = super::super::prepare_indexed_token_cursor("A a İ", &mut work)?;
        let bound = CanonicalTokenCensus::upper_bound(cursor.text().len())?
            .ok_or(IndexError::ResourceLimited)?;
        let mut tokens = ColdTokens::try_new(&bound, &work)?.ok_or(IndexError::ResourceLimited)?;
        let before = work.usage();
        if tokens.fill_from_cursor(&mut cursor, &mut work)? != 3 {
            return Err(IndexError::BackendViolation(
                "upper census changed indexed run count".into(),
            ));
        }
        // The source runs are 1, 1, and 2 bytes and each emits two 8-byte rows.
        let after = work.usage();
        if after.bytes.checked_sub(before.bytes) != Some(64)
            || after.mutations.checked_sub(before.mutations) != Some(3)
        {
            return Err(IndexError::BackendViolation(format!(
                "cold fill expected 64 bytes and three mutations: {before:?} -> {after:?}"
            )));
        }
        tokens.sort_and_validate(&work)?;
        let mut groups = BTreeMap::new();
        tokens.visit_groups(8192, &mut work, |key, positions, _budget| {
            if groups
                .insert(key.as_slice().to_vec(), positions.as_slice().to_vec())
                .is_some()
            {
                return Err(IndexError::BackendViolation(
                    "upper census emitted duplicate group".into(),
                ));
            }
            Ok(())
        })?;
        let gold = BTreeMap::from([
            (b"\0A".to_vec(), vec![0]),
            (b"\0a".to_vec(), vec![1]),
            ("\0İ".as_bytes().to_vec(), vec![2]),
            (b"\x01a".to_vec(), vec![0, 1]),
            ("\u{1}i\u{307}".as_bytes().to_vec(), vec![2]),
        ]);
        if groups != gold {
            return Err(IndexError::BackendViolation(
                "upper census changed canonical case groups".into(),
            ));
        }
        drop((tokens, cursor));
        if meter.live.load(Ordering::Relaxed) != 0 || work.first_denial().is_some() {
            return Err(IndexError::BackendViolation(
                "upper census retained a grant or recorded a denial".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn upper_bound_optional_admission_miss_keeps_exact_census_available() -> Result<(), IndexError>
    {
        let (mut work, meter) = budget()?;
        let bound = CanonicalTokenCensus::upper_bound(5)?.ok_or(IndexError::ResourceLimited)?;
        meter.ceiling.store(
            u64::try_from(bound.arena_bytes()? - 1)
                .map_err(|_error| IndexError::ResourceLimited)?,
            Ordering::Relaxed,
        );
        if ColdTokens::try_new(&bound, &work)?.is_some()
            || meter.live.load(Ordering::Relaxed) != 0
            || work.first_denial().is_some()
        {
            return Err(IndexError::BackendViolation(
                "rejected upper census retained an optional grant".into(),
            ));
        }
        let exact = exact_census("Ab aB", &mut work)?;
        let tokens = ColdTokens::try_new(&exact, &work)?.ok_or(IndexError::ResourceLimited)?;
        drop(tokens);
        if meter.live.load(Ordering::Relaxed) != 0 || work.first_denial().is_some() {
            return Err(IndexError::BackendViolation(
                "exact census fallback leaked its grant".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn direct_cold_emission_matches_public_normalizer_without_packet_key_copy()
    -> Result<(), IndexError> {
        use quanta_index_lq_text_normalizer::tokenize;
        use std::collections::BTreeMap;

        fn collect(
            source: &str,
            direct: bool,
        ) -> Result<(BTreeMap<Vec<u8>, Vec<u32>>, u64, u64), IndexError> {
            let (mut work, meter) = budget()?;
            let mut cursor = super::super::prepare_indexed_token_cursor(source, &mut work)?;
            let census = CanonicalTokenCensus::count(&mut cursor, source.len(), &mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            let mut tokens =
                ColdTokens::try_new(&census, &work)?.ok_or(IndexError::ResourceLimited)?;
            let indexed = if direct {
                tokens.fill_from_cursor(&mut cursor, &mut work)?
            } else {
                let mut packet = TokenBatch::new(&work)?;
                super::super::visit_prepared_token_batches(
                    &mut cursor,
                    &mut packet,
                    &mut work,
                    |batch, budget| tokens.push_batch(batch, budget),
                )?
            };
            let bytes = work.usage().bytes;
            let reservations = meter.reservations.load(Ordering::Relaxed);
            tokens.sort_and_validate(&work)?;
            let mut groups = BTreeMap::new();
            tokens.visit_groups(8192, &mut work, |key, positions, _budget| {
                if groups
                    .insert(key.as_slice().to_vec(), positions.as_slice().to_vec())
                    .is_some()
                {
                    return Err(IndexError::BackendViolation(
                        "canonical cold group repeated".into(),
                    ));
                }
                Ok(())
            })?;
            if indexed != 7 {
                return Err(IndexError::BackendViolation(
                    "fixed indexable source count changed".into(),
                ));
            }
            drop(tokens);
            drop(cursor);
            if meter.live.load(Ordering::Relaxed) != 0 {
                return Err(IndexError::BackendViolation(
                    "cold source grants outlived the fixture".into(),
                ));
            }
            Ok((groups, bytes, reservations))
        }

        let mut source = "Ab aB ab É cafe\u{301} İx ".to_owned();
        source.extend(std::iter::repeat_n('x', 257));
        source.push_str(" tail");
        let mut expected = BTreeMap::<Vec<u8>, Vec<u32>>::new();
        for (case, marker) in [(CaseMode::Sensitive, 0_u8), (CaseMode::Folded, 1_u8)] {
            let tokenized = tokenize(&source, case);
            for token in tokenized.indexable() {
                let mut key = vec![marker];
                key.extend_from_slice(token.text.as_bytes());
                expected
                    .entry(key)
                    .or_default()
                    .push(u32::try_from(token.position).map_err(|_| IndexError::ResourceLimited)?);
            }
        }
        if expected.get(b"\0Ab".as_slice()) != Some(&vec![0])
            || expected.get(b"\x01ab".as_slice()) != Some(&vec![0, 1, 2])
            || expected.get("\x01café".as_bytes()) != Some(&vec![4])
            || expected.get(b"\x01tail".as_slice()) != Some(&vec![7])
        {
            return Err(IndexError::BackendViolation(
                "fixed public normalizer gold changed".into(),
            ));
        }
        let (reference, reference_bytes, reference_reservations) = collect(&source, false)?;
        let (direct, direct_bytes, direct_reservations) = collect(&source, true)?;
        if reference != expected || direct != expected {
            return Err(IndexError::BackendViolation(
                "cold direct emission differs from public normalizer".into(),
            ));
        }
        if direct_bytes >= reference_bytes || direct_reservations >= reference_reservations {
            return Err(IndexError::BackendViolation(
                "cold direct emission still materializes the intermediate packet".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn direct_cold_emission_refuses_census_drift_without_unfunded_growth() -> Result<(), IndexError>
    {
        let (mut work, meter) = budget()?;
        let mut cursor = super::super::prepare_indexed_token_cursor("Ab aB", &mut work)?;
        let mut census = CanonicalTokenCensus::count(&mut cursor, 5, &mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        census.key_bytes = census
            .key_bytes
            .checked_sub(1)
            .ok_or(IndexError::ResourceLimited)?;
        let mut tokens = ColdTokens::try_new(&census, &work)?.ok_or(IndexError::ResourceLimited)?;
        if !matches!(
            tokens.fill_from_cursor(&mut cursor, &mut work),
            Err(IndexError::BackendViolation(_))
        ) {
            return Err(IndexError::BackendViolation(
                "direct cold source accepted a false key census".into(),
            ));
        }
        drop(tokens);
        drop(cursor);
        if meter.live.load(Ordering::Relaxed) != 0 {
            return Err(IndexError::BackendViolation(
                "direct cold drift retained funded backing".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn direct_cold_emission_observes_original_cancellation_and_refunds() -> Result<(), IndexError> {
        let (mut work, meter) = budget()?;
        let mut cursor = super::super::prepare_indexed_token_cursor("cafe\u{301} Ab", &mut work)?;
        let census = CanonicalTokenCensus::count(&mut cursor, 9, &mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        let mut tokens = ColdTokens::try_new(&census, &work)?.ok_or(IndexError::ResourceLimited)?;
        work.request().cancel_handle().cancel();
        if !matches!(
            tokens.fill_from_cursor(&mut cursor, &mut work),
            Err(IndexError::Cancelled)
        ) {
            return Err(IndexError::BackendViolation(
                "direct cold source ignored original cancellation".into(),
            ));
        }
        drop(tokens);
        drop(cursor);
        if meter.live.load(Ordering::Relaxed) != 0 {
            return Err(IndexError::BackendViolation(
                "cancelled direct cold source retained backing".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn exact_ascii_arena_admits_below_old_body_bound_and_refunds() -> Result<(), IndexError> {
        let (mut work, meter) = budget()?;
        let mut source = "Ab ".repeat(1_000);
        source.extend(std::iter::repeat_n('!', 102_401 - source.len()));
        let census = exact_census(&source, &mut work)?;
        // 1,000 runs, two 3-byte case keys and two 8-byte rows each.
        const EXACT_ARENA_BYTES: u64 = 22_000;
        let arena_bytes =
            u64::try_from(census.arena_bytes()?).map_err(|_error| IndexError::ResourceLimited)?;
        if arena_bytes != EXACT_ARENA_BYTES || source.len() * 28 != 2_867_228 {
            return Err(IndexError::BackendViolation(
                "fixed ASCII arena oracle changed".into(),
            ));
        }
        meter.ceiling.store(EXACT_ARENA_BYTES, Ordering::Relaxed);
        let tokens = ColdTokens::try_new(&census, &work)?.ok_or(IndexError::ResourceLimited)?;
        if meter.live.load(Ordering::Relaxed) != EXACT_ARENA_BYTES {
            return Err(IndexError::BackendViolation(
                "exact ASCII arena charged a different layout".into(),
            ));
        }
        drop(tokens);
        if meter.live.load(Ordering::Relaxed) != 0 || work.first_denial().is_some() {
            return Err(IndexError::BackendViolation(
                "exact ASCII arena was not fully refunded".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn cancelled_unicode_census_releases_funded_nfc() -> Result<(), IndexError> {
        let (mut work, meter) = budget()?;
        let source = "cafe\u{301} ".repeat(1_000);
        let mut cursor = super::super::prepare_indexed_token_cursor(&source, &mut work)?;
        if cursor.text() != "café ".repeat(1_000) || meter.live.load(Ordering::Relaxed) == 0 {
            return Err(IndexError::BackendViolation(
                "funded Unicode NFC cursor was not retained".into(),
            ));
        }
        work.request().cancel_handle().cancel();
        if !matches!(
            CanonicalTokenCensus::count(&mut cursor, source.len(), &mut work),
            Err(IndexError::Cancelled)
        ) {
            return Err(IndexError::BackendViolation(
                "Unicode census ignored cancellation".into(),
            ));
        }
        drop(cursor);
        if meter.live.load(Ordering::Relaxed) != 0 {
            return Err(IndexError::BackendViolation(
                "cancelled Unicode census leaked its funded input".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn widened_ascii_boundary_preserves_canonical_groups() -> Result<(), IndexError> {
        use std::collections::BTreeMap;

        let (mut oversized_work, oversized_meter) = budget()?;
        let oversized = usize::try_from(super::super::MAX_SOURCE_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let oversized_source = "a".repeat(oversized);
        let mut oversized_cursor = TokenCursor::new(&oversized_source);
        if CanonicalTokenCensus::count(&mut oversized_cursor, oversized, &mut oversized_work)?
            .is_some()
            || oversized_work.first_denial().is_some()
            || oversized_meter.live.load(Ordering::Relaxed) != 0
        {
            return Err(IndexError::BackendViolation(
                "oversized ASCII body entered memory fast path".into(),
            ));
        }
        for body_bytes in [4096_usize, 4097, 8192, 16_383] {
            let (mut work, meter) = budget()?;
            let mut source = "Ab ".repeat(body_bytes / 3);
            source.push_str(&"x".repeat(body_bytes % 3));
            if source.len() != body_bytes {
                return Err(IndexError::BackendViolation(
                    "ASCII test body length changed".into(),
                ));
            }
            let census = exact_census(&source, &mut work)?;
            let mut tokens =
                ColdTokens::try_new(&census, &work)?.ok_or(IndexError::ResourceLimited)?;
            let mut canonical = BTreeMap::<Vec<u8>, Vec<u32>>::new();
            let indexed =
                super::super::visit_indexed_token_batches(&source, &mut work, |batch, budget| {
                    for row in batch.rows() {
                        canonical
                            .entry(batch.term(row)?.to_vec())
                            .or_default()
                            .push(row.position());
                    }
                    tokens.push_batch(batch, budget)
                })?;
            tokens.sort_and_validate(&work)?;
            let mut observed = BTreeMap::<Vec<u8>, Vec<u32>>::new();
            tokens.visit_groups(8192, &mut work, |key, positions, _budget| {
                if observed
                    .insert(key.as_slice().to_vec(), positions.as_slice().to_vec())
                    .is_some()
                {
                    return Err(IndexError::BackendViolation(
                        "ordered ASCII group repeated".into(),
                    ));
                }
                Ok(())
            })?;
            if indexed == 0
                || observed != canonical
                || !observed.contains_key(b"\0Ab".as_slice())
                || !observed.contains_key(b"\x01ab".as_slice())
            {
                return Err(IndexError::BackendViolation(
                    "ASCII token groups differ from canonical".into(),
                ));
            }
            drop(tokens);
            if meter.live.load(Ordering::Relaxed) != 0 || work.first_denial().is_some() {
                return Err(IndexError::BackendViolation(
                    "ASCII fast path memory leaked".into(),
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn large_sort_observes_cancellation_before_completion() -> Result<(), IndexError> {
        let (mut work, meter) = budget()?;
        let source = "j i h g f e d c b a ".repeat(1_024);
        let census = exact_census(&source, &mut work)?;
        let mut tokens = ColdTokens::try_new(&census, &work)?.ok_or(IndexError::ResourceLimited)?;
        let indexed =
            super::super::visit_indexed_token_batches(&source, &mut work, |batch, budget| {
                tokens.push_batch(batch, budget)
            })?;
        if indexed != 10_240 || tokens.rows.len() != 20_480 {
            return Err(IndexError::BackendViolation(
                "large ASCII cancellation fixture changed its row count".into(),
            ));
        }
        let mut checkpoints = 0usize;
        let result = ColdTokens::sort_rows(&mut tokens.rows, &tokens.keys, || {
            checkpoints = checkpoints
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            if checkpoints == 3 {
                work.request().cancel_handle().cancel();
            }
            work.checkpoint()
        });
        if !matches!(result, Err(IndexError::Cancelled)) || checkpoints != 3 || tokens.ready {
            return Err(IndexError::BackendViolation(
                "large ASCII sort did not stop at its cancellation checkpoint".into(),
            ));
        }
        drop(tokens);
        if meter.live.load(Ordering::Relaxed) != 0 {
            return Err(IndexError::BackendViolation(
                "cancelled ASCII sort leaked its optional arena".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn borrowed_alias_uses_canonical_wire_and_falls_back_without_optional_memory()
    -> Result<(), IndexError> {
        let (mut work, meter) = budget()?;
        let source = "a b c d e";
        let mut batch = TokenBatch::new(&work)?;
        batch.prepare(source, &source_runs(), &work)?;
        let census = exact_census(source, &mut work)?;
        let mut tokens = ColdTokens::try_new(&census, &work)?.ok_or(IndexError::ResourceLimited)?;
        tokens.push_batch(&batch, &mut work)?;
        tokens.sort_and_validate(&work)?;
        drop(batch);

        let borrowed = tokens
            .try_packed_alias(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        if borrowed.term_count() != 10
            || borrowed.vector_count() != 5
            || borrowed.first_key()? != b"\0a"
        {
            return Err(IndexError::BackendViolation(
                "cold alias shape changed".into(),
            ));
        }
        for ordinal in 0_usize..5 {
            let folded = ordinal.checked_add(5).ok_or(IndexError::ResourceLimited)?;
            if borrowed.term(ordinal)?.1 != borrowed.term(folded)?.1 {
                return Err(IndexError::BackendViolation(
                    "cold alias vector first use changed".into(),
                ));
            }
        }
        let direct = encode_packed(&borrowed, borrowed.encoded_len(), &work)?;
        let shape = tokens.block_shape(&work)?;
        if shape.object_count != 1
            || shape.term_count != 10
            || shape.total_payload_bytes != direct.bytes.len()
            || shape.maximum_payload_bytes != direct.bytes.len()
        {
            return Err(IndexError::BackendViolation(
                "borrowed block shape differs from canonical writer".into(),
            ));
        }
        let mut expected = b"QITXOC01\x00\x0a\x00\x05\x00\x00\x00\x00".to_vec();
        for surface in [0_u8, 1_u8] {
            for (vector, letter) in b"abcde".iter().enumerate() {
                expected.extend_from_slice(&[0, 2, surface, *letter]);
                expected.extend_from_slice(
                    &u16::try_from(vector)
                        .map_err(|_error| IndexError::ResourceLimited)?
                        .to_be_bytes(),
                );
            }
        }
        for position in 0_u32..5 {
            expected.extend_from_slice(&[0, 1]);
            expected.extend_from_slice(&position.to_be_bytes());
        }
        if direct.bytes != expected {
            return Err(IndexError::BackendViolation(
                "borrowed alias differs from literal wire".into(),
            ));
        }
        drop(borrowed);
        let mut draft = BlockDraft::new()?;
        tokens.visit_groups(8192, &mut work, |key, positions, budget| {
            if !matches!(draft.try_push(key, positions, budget)?, BlockPush::Added) {
                return Err(IndexError::BackendViolation(
                    "canonical draft unexpectedly full".into(),
                ));
            }
            Ok(())
        })?;
        let SealedBlock::Packed { payload, .. } = draft.finish(&work)? else {
            return Err(IndexError::BackendViolation(
                "canonical draft did not pack".into(),
            ));
        };
        if payload.bytes != expected {
            return Err(IndexError::BackendViolation(
                "canonical draft differs from literal wire".into(),
            ));
        }
        drop(payload);
        drop(direct);
        meter
            .ceiling
            .store(meter.live.load(Ordering::Relaxed), Ordering::Relaxed);
        if tokens.try_packed_alias(&mut work)?.is_some() || work.first_denial().is_some() {
            return Err(IndexError::BackendViolation(
                "optional alias denial changed request".into(),
            ));
        }
        drop(tokens);
        if meter.live.load(Ordering::Relaxed) != 0 {
            return Err(IndexError::BackendViolation(
                "cold alias memory lease leaked".into(),
            ));
        }
        meter.ceiling.store(1_000_000, Ordering::Relaxed);
        let mut batch = TokenBatch::new(&work)?;
        let runs = source_runs();
        batch.prepare(
            "a b c d",
            runs.get(..4).ok_or(IndexError::ResourceLimited)?,
            &work,
        )?;
        let census = exact_census("a b c d", &mut work)?;
        let mut tokens = ColdTokens::try_new(&census, &work)?.ok_or(IndexError::ResourceLimited)?;
        tokens.push_batch(&batch, &mut work)?;
        tokens.sort_and_validate(&work)?;
        if tokens.try_packed_alias(&mut work)?.is_some() {
            return Err(IndexError::BackendViolation(
                "sub-threshold alias bypassed canonical fallback".into(),
            ));
        }
        let mut fallback_terms = 0_u64;
        tokens.visit_groups(8192, &mut work, |_key, _positions, _budget| {
            fallback_terms = fallback_terms
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            Ok(())
        })?;
        if fallback_terms != 8 {
            return Err(IndexError::BackendViolation(
                "cold alias fallback lost grouped terms".into(),
            ));
        }
        drop(tokens);
        drop(batch);
        if meter.live.load(Ordering::Relaxed) != 0 {
            return Err(IndexError::BackendViolation(
                "cold alias fallback memory lease leaked".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn borrowed_block_shape_matches_canonical_multi_block_and_leaf_output() -> Result<(), IndexError>
    {
        let (mut work, meter) = budget()?;
        let mut source = String::new();
        for _repeat in 0..20 {
            for ordinal in 0..257_u16 {
                source.push_str(&format!("t{ordinal:03} "));
            }
        }
        let census = exact_census(&source, &mut work)?;
        let mut tokens = ColdTokens::try_new(&census, &work)?.ok_or(IndexError::ResourceLimited)?;
        let indexed =
            super::super::visit_indexed_token_batches(&source, &mut work, |batch, budget| {
                tokens.push_batch(batch, budget)
            })?;
        if indexed != 5_140 {
            return Err(IndexError::BackendViolation(
                "block shape fixture changed".into(),
            ));
        }
        tokens.sort_and_validate(&work)?;
        let planned = tokens.block_shape(&work)?;
        let mut actual = ColdBlockShape::default();
        let mut draft = BlockDraft::new()?;
        fn finish(
            sealed: SealedBlock,
            actual: &mut ColdBlockShape,
            budget: &WorkBudget,
        ) -> Result<(), IndexError> {
            match sealed {
                SealedBlock::Leaf { positions, .. } => {
                    let payload =
                        crate::native::text_memory::encode(positions.as_occurrence(), budget)?;
                    actual.record(payload.as_slice().len())?;
                }
                SealedBlock::Packed { payload, .. } => actual.record(payload.bytes.len())?,
            }
            Ok(())
        }
        tokens.visit_groups(8192, &mut work, |key, positions, budget| {
            actual.term_count = actual
                .term_count
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            actual.maximum_key_bytes = actual.maximum_key_bytes.max(key.as_slice().len());
            match draft.try_push(key, positions, budget)? {
                BlockPush::Added => Ok(()),
                BlockPush::Full { term, positions } => {
                    if draft.len() != 0 {
                        finish(
                            std::mem::replace(&mut draft, BlockDraft::new()?).finish(budget)?,
                            &mut actual,
                            budget,
                        )?;
                    }
                    match draft.try_push(term, positions, budget)? {
                        BlockPush::Added => Ok(()),
                        BlockPush::Full { term: _, positions } => {
                            let payload = crate::native::text_memory::encode(
                                positions.as_occurrence(), budget,
                            )?;
                            actual.record(payload.as_slice().len())
                        }
                    }
                }
            }
        })?;
        if draft.len() != 0 {
            finish(draft.finish(&work)?, &mut actual, &work)?;
        }
        if planned.object_count != actual.object_count
            || planned.total_payload_bytes != actual.total_payload_bytes
            || planned.maximum_payload_bytes != actual.maximum_payload_bytes
            || planned.maximum_key_bytes != actual.maximum_key_bytes
            || planned.term_count != actual.term_count
            || planned.object_count < 2
            || planned.maximum_key_position_peak == 0
        {
            return Err(IndexError::BackendViolation(format!(
                "cold physical preflight differs from canonical writer: planned={planned:?}, actual={actual:?}"
            )));
        }
        drop(tokens);
        if meter.live.load(Ordering::Relaxed) != 0 || work.first_denial().is_some() {
            return Err(IndexError::BackendViolation(
                "block shape leaked memory or denied work".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn exact_group_count_skips_514_term_alias_probe_but_keeps_512_term_alias()
    -> Result<(), IndexError> {
        for source_terms in [256_u16, 257_u16] {
            let (mut work, meter) = budget()?;
            let mut source = String::new();
            for ordinal in 0..source_terms {
                source.push_str(&format!("t{ordinal:03} "));
            }
            let census = exact_census(&source, &mut work)?;
            let mut tokens =
                ColdTokens::try_new(&census, &work)?.ok_or(IndexError::ResourceLimited)?;
            let indexed =
                super::super::visit_indexed_token_batches(&source, &mut work, |batch, budget| {
                    tokens.push_batch(batch, budget)
                })?;
            if indexed != u64::from(source_terms) {
                return Err(IndexError::BackendViolation(
                    "alias boundary source changed".into(),
                ));
            }
            tokens.sort_and_validate(&work)?;
            let before = work.usage();
            let live_before = meter.live.load(Ordering::Relaxed);
            let reservations_before = meter.reservations.load(Ordering::Relaxed);
            let alias = tokens.try_packed_alias(&mut work)?;
            if source_terms == 257 {
                if alias.is_some()
                    || work.usage() != before
                    || meter.live.load(Ordering::Relaxed) != live_before
                    || meter.reservations.load(Ordering::Relaxed) != reservations_before
                {
                    return Err(IndexError::BackendViolation(
                        "514 distinct case terms performed a rejected alias probe".into(),
                    ));
                }
                let shape = tokens.block_shape(&work)?;
                if shape.term_count != 514 || shape.object_count != 2 {
                    return Err(IndexError::BackendViolation(
                        "514-term direct block shape changed".into(),
                    ));
                }
                let mut seen = 0_usize;
                tokens.visit_groups(8_192, &mut work, |key, positions, _budget| {
                    let case =
                        u8::try_from(seen / 257).map_err(|_error| IndexError::ResourceLimited)?;
                    let ordinal =
                        u32::try_from(seen % 257).map_err(|_error| IndexError::ResourceLimited)?;
                    let term = format!("t{ordinal:03}");
                    if key.as_slice().first() != Some(&case)
                        || key.as_slice().get(1..) != Some(term.as_bytes())
                        || positions.as_slice() != [ordinal]
                    {
                        return Err(IndexError::BackendViolation(
                            "514-term direct fallback lost canonical group".into(),
                        ));
                    }
                    seen += 1;
                    Ok(())
                })?;
                if seen != 514 {
                    return Err(IndexError::BackendViolation(
                        "514-term direct fallback incomplete".into(),
                    ));
                }
            } else {
                let alias = alias.ok_or(IndexError::ResourceLimited)?;
                if alias.term_count() != 512
                    || alias.vector_count() != 256
                    || work.usage().bytes != before.bytes + 4_608
                    || work.usage().reads != before.reads
                    || work.usage().writes != before.writes
                    || work.usage().mutations != before.mutations
                    || meter.reservations.load(Ordering::Relaxed) != reservations_before + 1
                {
                    return Err(IndexError::BackendViolation(
                        "512-term eligible alias changed its exact probe".into(),
                    ));
                }
                drop(alias);
                if meter.live.load(Ordering::Relaxed) != live_before {
                    return Err(IndexError::BackendViolation(
                        "512-term alias grant was not refunded".into(),
                    ));
                }
            }
            if work.first_denial().is_some() {
                return Err(IndexError::BackendViolation(
                    "alias cardinality gate recorded a terminal denial".into(),
                ));
            }
            drop(tokens);
            if meter.live.load(Ordering::Relaxed) != 0 {
                return Err(IndexError::BackendViolation(
                    "alias boundary source leaked memory".into(),
                ));
            }
        }
        Ok(())
    }

    #[test]
    fn impossible_alias_footprint_skips_paid_probe_for_direct_and_flat_programs()
    -> Result<(), IndexError> {
        for source_terms in [4_u16, 80] {
            let (mut work, meter) = budget()?;
            let mut source = String::from("a ");
            let long_prefix = "z".repeat(60);
            for ordinal in 1..source_terms {
                source.push_str(&format!("{long_prefix}t{ordinal:03} "));
            }
            let census = exact_census(&source, &mut work)?;
            let mut tokens =
                ColdTokens::try_new(&census, &work)?.ok_or(IndexError::ResourceLimited)?;
            let indexed =
                super::super::visit_indexed_token_batches(&source, &mut work, |batch, budget| {
                    tokens.push_batch(batch, budget)
                })?;
            assert_eq!(indexed, u64::from(source_terms));
            tokens.sort_and_validate(&work)?;
            let tokens = Arc::new(tokens);
            let direct = ColdTokenProgram::from_suffix(Some(Arc::clone(&tokens)), None, &mut work)?
                .ok_or(IndexError::ResourceLimited)?;
            let flat = ColdTokenProgram::from_suffix(
                None,
                Some((&direct, u32::from(source_terms))),
                &mut work,
            )?
            .ok_or(IndexError::ResourceLimited)?;
            for view in [&direct, &flat] {
                let view = ColdTokenProgramView::from_program(view)?;
                let before = work.usage();
                let reservations = meter.reservations.load(Ordering::Relaxed);
                assert!(view.try_packed_alias(&mut work)?.is_none());
                assert_eq!(work.usage(), before);
                assert_eq!(meter.reservations.load(Ordering::Relaxed), reservations);
                let shape = view.block_shape(&mut work)?;
                let expected_terms = usize::from(source_terms)
                    .checked_mul(2)
                    .ok_or(IndexError::ResourceLimited)?;
                assert_eq!(shape.term_count, expected_terms);
                if source_terms == 4 {
                    assert_eq!(shape.object_count, 1);
                } else {
                    assert!(shape.object_count > 1);
                }
            }
            drop(flat);
            drop(direct);
            drop(tokens);
            assert_eq!(meter.live.load(Ordering::Relaxed), 0);
        }
        Ok(())
    }
}
