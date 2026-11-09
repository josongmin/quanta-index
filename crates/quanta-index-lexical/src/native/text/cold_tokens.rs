//! Owner-local ordered token groups for an admitted ASCII cold source.
//!
//! The canonical normalizer still creates every case key and position. This
//! buffer only replaces the disposable SQL sort when optional memory is available.

use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage};
use std::cmp::Ordering;

use super::{MAX_POSITIONS_PER_TERM, PreparedKey, PreparedPositions, TokenBatch, check_term_key};
use crate::native::text_occurrence::block::{
    HEADER_BYTES, MAX_BLOCK_BYTES, MAX_BLOCK_TERMS, PackedBlockView, VectorIndex,
};

use super::entry_patch::{COLD_ALIAS_MAX_KEY_BYTES, COLD_ALIAS_MIN_TERMS};

// A run occupies at least one ASCII input byte and emits exactly two case
// rows. Four key bytes per input byte is the conservative two-surface bound.
// The parent admits at most 8 MiB, so u32 covers both arenas and row ordinals.
#[derive(Clone, Copy)]
struct Row {
    start: u32,
    end: u32,
    position: u32,
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
            .position)
    }
}

pub(super) struct ColdTokens {
    keys: Vec<u8>,
    rows: Vec<Row>,
    max_rows: usize,
    max_key_bytes: usize,
    ready: bool,
    // Both Vec backings die before their one owner-local optional grant.
    _memory: MemoryLease,
}

impl ColdTokens {
    /// Optional one-block alias preparation. A shape miss has no SQL or object
    /// effect and must use the original grouped BlockDraft path.
    pub(super) fn try_packed_alias(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<Option<ColdPackedAlias<'_>>, IndexError> {
        budget.checkpoint()?;
        if !self.ready {
            return Err(IndexError::BackendViolation(
                "small ASCII token groups were not ordered".into(),
            ));
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
            if key.len() > COLD_ALIAS_MAX_KEY_BYTES {
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
                    "small ASCII token group exceeded its validated envelope".into(),
                ));
            }
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
                    .map(|row| row.position)
                    .cmp(positions.iter().map(|row| row.position)))
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
                key_start: first.start,
                key_end: first.end,
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
                candidate.index.insert(insertion, vector);
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
        body_bytes: usize,
        budget: &WorkBudget,
    ) -> Result<Option<Self>, IndexError> {
        if !u64::try_from(body_bytes).is_ok_and(|bytes| bytes <= super::MAX_SOURCE_BYTES) {
            return Ok(None);
        }
        // Empty sources still need a nonzero optional reservation; no token
        // can use the spare row/key capacity. Keep all backing under one lease.
        let admitted_body = body_bytes.max(1);
        let max_rows = admitted_body
            .checked_mul(2)
            .ok_or(IndexError::ResourceLimited)?;
        let max_key_bytes = admitted_body
            .checked_mul(4)
            .ok_or(IndexError::ResourceLimited)?;
        if u32::try_from(max_key_bytes).is_err() || u32::try_from(max_rows).is_err() {
            return Err(IndexError::BackendViolation(
                "ASCII token bounds exceed u32 offsets".into(),
            ));
        }
        let bytes = max_key_bytes
            .checked_add(
                max_rows
                    .checked_mul(std::mem::size_of::<Row>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .ok_or(IndexError::ResourceLimited)?;
        let Some(memory) = budget.try_reserve_memory_for_optimization(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(None);
        };
        let mut keys = Vec::new();
        if keys.try_reserve_exact(max_key_bytes).is_err() {
            return Ok(None);
        }
        let mut rows = Vec::new();
        if rows.try_reserve_exact(max_rows).is_err() {
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
            max_rows,
            max_key_bytes,
            ready: false,
            _memory: memory,
        }))
    }

    pub(super) fn push_batch(
        &mut self,
        batch: &TokenBatch,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.ready {
            return Err(IndexError::BackendViolation(
                "small ASCII token buffer changed after ordering".into(),
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
                    "small ASCII token census exceeded its proved input bound".into(),
                ));
            }
            budget.charge(WorkUsage {
                bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let start =
                u32::try_from(self.keys.len()).map_err(|_error| IndexError::ResourceLimited)?;
            let end = u32::try_from(end).map_err(|_error| IndexError::ResourceLimited)?;
            self.keys.extend_from_slice(key);
            self.rows.push(Row {
                start,
                end,
                position: token.position(),
            });
        }
        Ok(())
    }

    fn key<'a>(keys: &'a [u8], row: &Row) -> Option<&'a [u8]> {
        let start = usize::try_from(row.start).ok()?;
        let end = usize::try_from(row.end).ok()?;
        keys.get(start..end)
    }

    fn compare(keys: &[u8], left: &Row, right: &Row) -> Ordering {
        Self::key(keys, left)
            .cmp(&Self::key(keys, right))
            .then_with(|| left.position.cmp(&right.position))
    }

    fn sort_checkpoint(
        comparisons: &mut usize,
        checkpoint: &mut impl FnMut() -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        *comparisons = comparisons
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if *comparisons % 1_024 == 0 {
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
        for root in (0..len / 2).rev() {
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
        budget.checkpoint()?;
        let keys = self.keys.as_slice();
        Self::sort_rows(&mut self.rows, keys, || budget.checkpoint())?;
        let mut prior: Option<&Row> = None;
        let mut positions = 0usize;
        for row in &self.rows {
            budget.checkpoint()?;
            let key = Self::key(keys, row).ok_or(IndexError::ResourceLimited)?;
            check_term_key(key)?;
            if let Some(previous) = prior {
                if Self::key(keys, previous).ok_or(IndexError::ResourceLimited)? == key {
                    if previous.position >= row.position {
                        return Err(IndexError::CorruptObject(
                            "small ASCII token position repeats".into(),
                        ));
                    }
                    positions = positions
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                } else {
                    positions = 1;
                }
            } else {
                positions = 1;
            }
            if u64::try_from(positions).map_err(|_error| IndexError::ResourceLimited)?
                > MAX_POSITIONS_PER_TERM
            {
                return Err(IndexError::ResourceLimited);
            }
            prior = Some(row);
        }
        budget.checkpoint()?;
        self.ready = true;
        Ok(())
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
                "small ASCII token groups were not ordered".into(),
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
                .push(row.position, budget)?;
        }
        if let Some((key, positions)) = current {
            consume(key, positions, budget)?;
        }
        budget.checkpoint()
    }
}

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

    #[test]
    fn widened_ascii_boundary_preserves_canonical_groups() -> Result<(), IndexError> {
        use std::collections::BTreeMap;

        let (oversized_work, oversized_meter) = budget()?;
        let oversized = usize::try_from(super::super::MAX_SOURCE_BYTES)
            .map_err(|_error| IndexError::ResourceLimited)?
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        if ColdTokens::try_new(oversized, &oversized_work)?.is_some()
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
            let mut tokens =
                ColdTokens::try_new(source.len(), &work)?.ok_or(IndexError::ResourceLimited)?;
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
        let mut tokens =
            ColdTokens::try_new(source.len(), &work)?.ok_or(IndexError::ResourceLimited)?;
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
        let mut tokens =
            ColdTokens::try_new(source.len(), &work)?.ok_or(IndexError::ResourceLimited)?;
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
        let mut tokens =
            ColdTokens::try_new("a b c d".len(), &work)?.ok_or(IndexError::ResourceLimited)?;
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
}
