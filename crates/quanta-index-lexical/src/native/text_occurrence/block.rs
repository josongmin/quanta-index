//! Bounded, owner-local term to position-vector dictionary.
//!
//! A checked layout owns its authenticated immutable payload and DATA lease.

use std::cmp::Ordering;
use std::ops::Range;

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, StagedPayload, StoredObject, WorkBudget,
};

use super::super::{
    text::{MAX_POSITIONS_PER_TERM, PreparedPositions},
    text_memory::{PreparedKey, PreparedRows},
    text_spool::check_term_key,
};

pub(in crate::native) const MAX_BLOCK_BYTES: usize = 8_192;
pub(in crate::native) const MAX_BLOCK_TERMS: usize = 512;
pub(in crate::native) const LAYOUT_POLICY: &[u8] =
    b"QITXOC01;be16-terms-vectors;be32-reserved0;sorted-unique-keys;first-use-vectors;be32-strict-positions;max8192-512-8192";

const MAGIC: &[u8; 8] = b"QITXOC01";
pub(in crate::native) const HEADER_BYTES: usize = 16;
/// Ephemeral sorted ordinals; the exact vector remains in the original
/// first-use slot, so lookup order cannot change the canonical wire.
pub(in crate::native) struct VectorIndex {
    slots: [u16; MAX_BLOCK_TERMS],
    len: usize,
    _memory: Option<MemoryLease>,
}

impl VectorIndex {
    pub(in crate::native) fn within_owner_grant() -> Self {
        Self {
            slots: [0; MAX_BLOCK_TERMS],
            len: 0,
            _memory: None,
        }
    }

    fn new(budget: &WorkBudget) -> Result<Self, IndexError> {
        let memory = budget.reserve_memory(
            u64::try_from(MAX_BLOCK_TERMS * std::mem::size_of::<u16>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut index = Self::within_owner_grant();
        index._memory = memory;
        Ok(index)
    }

    pub(in crate::native) fn locate(
        &self,
        budget: &WorkBudget,
        mut compare: impl FnMut(usize) -> Result<Ordering, IndexError>,
    ) -> Result<(Option<usize>, usize), IndexError> {
        let mut lower = 0;
        let mut upper = self.len;
        while lower < upper {
            budget.checkpoint()?;
            let middle = lower + (upper - lower) / 2;
            let ordinal = usize::from(*self.slots.get(middle).ok_or(IndexError::ResourceLimited)?);
            match compare(ordinal)? {
                Ordering::Less => lower = middle + 1,
                Ordering::Greater => upper = middle,
                Ordering::Equal => return Ok((Some(ordinal), middle)),
            }
        }
        Ok((None, lower))
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "the bounded vector count and search insertion position establish the slot"
    )]
    pub(in crate::native) fn insert(&mut self, slot: usize, ordinal: u16) {
        // Caller proved the insertion slot and the 512-vector cap. Shifting
        // compact ordinals does not move or rewrite the original vectors.
        self.slots.copy_within(slot..self.len, slot + 1);
        self.slots[slot] = ordinal;
        self.len += 1;
    }
}

pub(in crate::native) fn is_packed(payload: &[u8]) -> bool {
    payload.starts_with(MAGIC)
}

pub(in crate::native) struct BlockTerm {
    pub(in crate::native) key: PreparedKey,
    pub(in crate::native) vector: u16,
}

pub(in crate::native) enum BlockPush {
    Added,
    Full {
        term: PreparedKey,
        positions: PreparedPositions,
    },
}

pub(in crate::native) enum SealedBlock {
    Leaf {
        term: PreparedKey,
        positions: PreparedPositions,
    },
    Packed {
        payload: StagedPayload,
        terms: PreparedRows<BlockTerm>,
    },
}

/// The one canonical packed-wire writer accepts either owned draft rows or a
/// checked owner-local borrowed view. Vector ordinals remain first-use IDs.
pub(in crate::native) trait PackedBlockView {
    fn term_count(&self) -> usize;
    fn vector_count(&self) -> usize;
    fn term(&self, ordinal: usize) -> Result<(&[u8], u16), IndexError>;
    fn vector_len(&self, ordinal: usize) -> Result<usize, IndexError>;
    fn position(&self, vector: usize, ordinal: usize) -> Result<u32, IndexError>;
}

pub(in crate::native) fn encode_packed(
    view: &impl PackedBlockView,
    encoded_len: usize,
    budget: &WorkBudget,
) -> Result<StagedPayload, IndexError> {
    budget.checkpoint()?;
    if !(2..=MAX_BLOCK_TERMS).contains(&view.term_count()) || encoded_len > MAX_BLOCK_BYTES {
        return Err(IndexError::ResourceLimited);
    }
    let memory = budget.reserve_memory(
        u64::try_from(encoded_len).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(encoded_len)
        .map_err(|_error| IndexError::ResourceLimited)?;
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(
        &u16::try_from(view.term_count())
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    bytes.extend_from_slice(
        &u16::try_from(view.vector_count())
            .map_err(|_error| IndexError::ResourceLimited)?
            .to_be_bytes(),
    );
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    for ordinal in 0..view.term_count() {
        budget.checkpoint()?;
        let (key, vector) = view.term(ordinal)?;
        bytes.extend_from_slice(
            &u16::try_from(key.len())
                .map_err(|_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(&vector.to_be_bytes());
    }
    for vector in 0..view.vector_count() {
        budget.checkpoint()?;
        let positions = view.vector_len(vector)?;
        bytes.extend_from_slice(
            &u16::try_from(positions)
                .map_err(|_error| IndexError::ResourceLimited)?
                .to_be_bytes(),
        );
        for ordinal in 0..positions {
            bytes.extend_from_slice(&view.position(vector, ordinal)?.to_be_bytes());
        }
    }
    if bytes.len() != encoded_len || bytes.len() > MAX_BLOCK_BYTES {
        return Err(IndexError::BackendViolation(
            "text block wire census changed".into(),
        ));
    }
    budget.checkpoint()?;
    Ok(StagedPayload { bytes, memory })
}

impl PackedBlockView for BlockDraft {
    fn term_count(&self) -> usize {
        self.terms.len()
    }

    fn vector_count(&self) -> usize {
        self.vectors.len()
    }

    fn term(&self, ordinal: usize) -> Result<(&[u8], u16), IndexError> {
        let term = self
            .terms
            .as_slice()
            .get(ordinal)
            .ok_or(IndexError::ResourceLimited)?;
        Ok((term.key.as_slice(), term.vector))
    }

    fn vector_len(&self, ordinal: usize) -> Result<usize, IndexError> {
        Ok(self
            .vectors
            .as_slice()
            .get(ordinal)
            .ok_or(IndexError::ResourceLimited)?
            .as_slice()
            .len())
    }

    fn position(&self, vector: usize, ordinal: usize) -> Result<u32, IndexError> {
        self.vectors
            .as_slice()
            .get(vector)
            .ok_or(IndexError::ResourceLimited)?
            .as_slice()
            .get(ordinal)
            .copied()
            .ok_or(IndexError::ResourceLimited)
    }
}

pub(in crate::native) struct BlockDraft {
    // Both containers acquire their own backing before the first push.
    // The first occurrence of each distinct vector owns its position grant.
    terms: PreparedRows<BlockTerm>,
    vectors: PreparedRows<PreparedPositions>,
    vector_index: Option<VectorIndex>,
    encoded_len: usize,
}

impl BlockDraft {
    pub(in crate::native) fn new() -> Result<Self, IndexError> {
        Ok(Self {
            terms: PreparedRows::new(MAX_BLOCK_TERMS)?,
            vectors: PreparedRows::new(MAX_BLOCK_TERMS)?,
            vector_index: None,
            encoded_len: HEADER_BYTES,
        })
    }

    pub(in crate::native) fn len(&self) -> usize {
        self.terms.len()
    }

    pub(in crate::native) fn try_push(
        &mut self,
        term: PreparedKey,
        positions: PreparedPositions,
        budget: &WorkBudget,
    ) -> Result<BlockPush, IndexError> {
        budget.checkpoint()?;
        let key = term.as_slice();
        check_term_key(key)?;
        if self
            .terms
            .as_slice()
            .last()
            .is_some_and(|last| last.key.as_slice() >= key)
        {
            return Err(IndexError::InvalidInput(
                "text block terms must be strictly sorted".into(),
            ));
        }
        let values = positions.as_slice();
        if values.is_empty()
            || u64::try_from(values.len()).map_err(|_error| IndexError::ResourceLimited)?
                > MAX_POSITIONS_PER_TERM
            || values.windows(2).any(|pair| {
                pair.first()
                    .zip(pair.get(1))
                    .is_some_and(|(previous, current)| previous >= current)
            })
        {
            return Err(IndexError::InvalidInput(
                "text block positions must be nonempty and strictly increasing".into(),
            ));
        }
        let mut new_index = None;
        if self.vector_index.is_none() && self.vectors.len() >= 2 {
            let mut index = VectorIndex::new(budget)?;
            for ordinal in 0..self.vectors.len() {
                let candidate = self
                    .vectors
                    .as_slice()
                    .get(ordinal)
                    .ok_or(IndexError::ResourceLimited)?
                    .as_slice();
                let (same, slot) = index.locate(budget, |prior| {
                    Ok(self
                        .vectors
                        .as_slice()
                        .get(prior)
                        .ok_or(IndexError::ResourceLimited)?
                        .as_slice()
                        .cmp(candidate))
                })?;
                if same.is_some() {
                    return Err(IndexError::BackendViolation(
                        "text block draft repeats a canonical vector".into(),
                    ));
                }
                index.insert(
                    slot,
                    u16::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?,
                );
            }
            new_index = Some(index);
        }
        let (same, insertion_slot) =
            if let Some(index) = self.vector_index.as_ref().or(new_index.as_ref()) {
                index.locate(budget, |ordinal| {
                    Ok(self
                        .vectors
                        .as_slice()
                        .get(ordinal)
                        .ok_or(IndexError::ResourceLimited)?
                        .as_slice()
                        .cmp(values))
                })?
            } else {
                (
                    self.vectors
                        .as_slice()
                        .iter()
                        .position(|prior| prior.as_slice() == values),
                    0,
                )
            };
        let row_bytes = 4_usize
            .checked_add(key.len())
            .ok_or(IndexError::ResourceLimited)?;
        let vector_bytes = if same.is_some() {
            0
        } else {
            2_usize
                .checked_add(
                    values
                        .len()
                        .checked_mul(4)
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .ok_or(IndexError::ResourceLimited)?
        };
        let next_len = self
            .encoded_len
            .checked_add(row_bytes)
            .and_then(|len| len.checked_add(vector_bytes))
            .ok_or(IndexError::ResourceLimited)?;
        if self.terms.len() == MAX_BLOCK_TERMS || next_len > MAX_BLOCK_BYTES {
            return Ok(BlockPush::Full { term, positions });
        }
        let vector = if let Some(index) = same {
            drop(positions);
            u16::try_from(index).map_err(|_error| IndexError::ResourceLimited)?
        } else {
            let index =
                u16::try_from(self.vectors.len()).map_err(|_error| IndexError::ResourceLimited)?;
            self.vectors.push(positions, budget)?;
            index
        };
        if let Err(error) = self.terms.push(BlockTerm { key: term, vector }, budget) {
            if same.is_none() {
                let _discarded = self.vectors.pop();
            }
            return Err(error);
        }
        if same.is_none() {
            if let Some(index) = &mut self.vector_index {
                index.insert(insertion_slot, vector);
            } else if let Some(index) = &mut new_index {
                index.insert(insertion_slot, vector);
            }
        }
        if let Some(index) = new_index {
            self.vector_index = Some(index);
        }
        self.encoded_len = next_len;
        Ok(BlockPush::Added)
    }

    pub(in crate::native) fn finish(
        mut self,
        budget: &WorkBudget,
    ) -> Result<SealedBlock, IndexError> {
        budget.checkpoint()?;
        match self.terms.len() {
            0 => Err(IndexError::InvalidInput(
                "empty text occurrence block".into(),
            )),
            1 => {
                let term = self.terms.pop().ok_or_else(|| {
                    IndexError::BackendViolation("text block singleton row missing".into())
                })?;
                let positions = self.vectors.pop().ok_or_else(|| {
                    IndexError::BackendViolation("text block singleton vector missing".into())
                })?;
                Ok(SealedBlock::Leaf {
                    term: term.key,
                    positions,
                })
            }
            _ => {
                let payload = encode_packed(&self, self.encoded_len, budget)?;
                Ok(SealedBlock::Packed {
                    payload,
                    terms: self.terms,
                })
            }
        }
    }
}

#[derive(Clone, Copy)]
struct RowSpan {
    start: usize,
    end: usize,
    vector: u16,
}

impl RowSpan {
    const EMPTY: Self = Self {
        start: 0,
        end: 0,
        vector: 0,
    };
}

#[derive(Clone, Copy)]
struct VectorSpan {
    start: usize,
    end: usize,
}

impl VectorSpan {
    const EMPTY: Self = Self { start: 0, end: 0 };
}

pub(in crate::native) struct CheckedBlock {
    object: StoredObject,
    rows: [RowSpan; MAX_BLOCK_TERMS],
    vectors: [VectorSpan; MAX_BLOCK_TERMS],
    term_count: usize,
    vector_count: usize,
    // The layout dies before the grant when the execution-local cache drops.
    _memory: Option<MemoryLease>,
}

impl CheckedBlock {
    pub(in crate::native) fn parse(
        object: StoredObject,
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        budget.checkpoint()?;
        if !object.registration.children.is_empty() {
            return Err(corrupt("text block must have no child edges"));
        }
        let payload = object.payload.as_slice();
        if payload.len() < HEADER_BYTES
            || payload.len() > MAX_BLOCK_BYTES
            || payload.get(..MAGIC.len()) != Some(MAGIC.as_slice())
        {
            return Err(corrupt("invalid text block envelope"));
        }
        let mut header_offset = MAGIC.len();
        let term_count = usize::from(read_u16(payload, &mut header_offset)?);
        let vector_count = usize::from(read_u16(payload, &mut header_offset)?);
        if !(2..=MAX_BLOCK_TERMS).contains(&term_count)
            || !(1..=term_count).contains(&vector_count)
            || take(payload, &mut header_offset, 4)? != [0; 4]
        {
            return Err(corrupt("invalid text block header"));
        }
        let memory = budget.reserve_memory(
            u64::try_from(std::mem::size_of::<Self>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let mut checked = Self {
            object,
            rows: [RowSpan::EMPTY; MAX_BLOCK_TERMS],
            vectors: [VectorSpan::EMPTY; MAX_BLOCK_TERMS],
            term_count,
            vector_count,
            _memory: memory,
        };
        let payload = checked.object.payload.as_slice();
        let mut offset = HEADER_BYTES;
        let mut next_vector = 0_usize;
        let mut vector_index = None;
        for index in 0..term_count {
            budget.checkpoint()?;
            let length = usize::from(read_u16(payload, &mut offset)?);
            let start = offset;
            let key = take(payload, &mut offset, length)?;
            let end = offset;
            check_term_key(key)?;
            if index > 0 {
                let prior_index = index
                    .checked_sub(1)
                    .ok_or_else(|| corrupt("text block row index underflow"))?;
                let prior = checked
                    .rows
                    .get(prior_index)
                    .ok_or_else(|| corrupt("text block row exceeds layout"))?;
                let prior_key = payload
                    .get(prior.start..prior.end)
                    .ok_or_else(|| corrupt("text block key exceeds payload"))?;
                if prior_key >= key {
                    return Err(corrupt("text block keys are not strictly sorted"));
                }
            }
            let vector = read_u16(payload, &mut offset)?;
            let vector_index = usize::from(vector);
            if vector_index >= vector_count || vector_index > next_vector {
                return Err(corrupt("text block vector selector is invalid"));
            }
            if vector_index == next_vector {
                next_vector = next_vector
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            *checked
                .rows
                .get_mut(index)
                .ok_or_else(|| corrupt("text block row exceeds layout"))? =
                RowSpan { start, end, vector };
        }
        if next_vector != vector_count {
            return Err(corrupt("text block has an unused vector"));
        }
        for index in 0..vector_count {
            budget.checkpoint()?;
            let count = usize::from(read_u16(payload, &mut offset)?);
            if count == 0
                || u64::try_from(count).map_err(|_error| IndexError::ResourceLimited)?
                    > MAX_POSITIONS_PER_TERM
            {
                return Err(corrupt("text block position count is invalid"));
            }
            let start = offset;
            let byte_count = count.checked_mul(4).ok_or(IndexError::ResourceLimited)?;
            let values = take(payload, &mut offset, byte_count)?;
            let mut previous = None;
            for value in values.chunks_exact(4) {
                budget.checkpoint()?;
                let encoded: [u8; 4] = value
                    .try_into()
                    .map_err(|_error| corrupt("text block position has invalid width"))?;
                let position = u32::from_be_bytes(encoded);
                if previous.is_some_and(|old| old >= position) {
                    return Err(corrupt("text block positions are not strictly increasing"));
                }
                previous = Some(position);
            }
            if index == 1 {
                let mut ready = VectorIndex::new(budget)?;
                ready.insert(0, 0);
                vector_index = Some(ready);
            }
            let (same, insertion_slot) = if let Some(index) = &vector_index {
                index.locate(budget, |ordinal| {
                    let prior = checked
                        .vectors
                        .get(ordinal)
                        .ok_or_else(|| corrupt("text block vector exceeds layout"))?;
                    Ok(payload
                        .get(prior.start..prior.end)
                        .ok_or_else(|| corrupt("text block vector exceeds payload"))?
                        .cmp(values))
                })?
            } else {
                (None, 0)
            };
            if same.is_some() {
                return Err(corrupt("text block repeats a position vector"));
            }
            *checked
                .vectors
                .get_mut(index)
                .ok_or_else(|| corrupt("text block vector exceeds layout"))? =
                VectorSpan { start, end: offset };
            if let Some(lookup) = &mut vector_index {
                lookup.insert(
                    insertion_slot,
                    u16::try_from(index).map_err(|_error| IndexError::ResourceLimited)?,
                );
            }
        }
        if offset != payload.len() {
            return Err(corrupt("text block has trailing bytes"));
        }
        budget.checkpoint()?;
        Ok(checked)
    }

    pub(in crate::native) fn object(&self) -> &StoredObject {
        &self.object
    }

    pub(in crate::native) fn term_count(&self) -> usize {
        self.term_count
    }

    pub(in crate::native) fn key(&self, slot: usize) -> Result<&[u8], IndexError> {
        let row = self
            .rows
            .get(..self.term_count)
            .ok_or_else(layout_invariant)?
            .get(slot)
            .ok_or_else(|| IndexError::InvalidInput("text block term slot out of range".into()))?;
        self.object
            .payload
            .get(row.start..row.end)
            .ok_or_else(layout_invariant)
    }

    /// Return the insertion slot for any raw key, including one beyond the
    /// final term. Parsing has already proved the rows strictly sorted.
    pub(in crate::native) fn lower_bound(
        &self,
        key: &[u8],
        budget: &WorkBudget,
    ) -> Result<usize, IndexError> {
        budget.checkpoint()?;
        let mut lower = 0;
        let mut upper = self.term_count;
        while lower < upper {
            budget.checkpoint()?;
            let middle = upper
                .checked_sub(lower)
                .and_then(|remaining| remaining.checked_div(2))
                .and_then(|half| lower.checked_add(half))
                .ok_or_else(layout_invariant)?;
            if self.key(middle)? < key {
                lower = middle.checked_add(1).ok_or_else(layout_invariant)?;
            } else {
                upper = middle;
            }
        }
        budget.checkpoint()?;
        Ok(lower)
    }

    pub(in crate::native) fn selection(
        &self,
        term: &[u8],
        budget: &WorkBudget,
    ) -> Result<Range<usize>, IndexError> {
        self.selection_with_ordinal(term, budget)
            .map(|(range, _)| range)
    }

    fn selection_with_ordinal(
        &self,
        term: &[u8],
        budget: &WorkBudget,
    ) -> Result<(Range<usize>, usize), IndexError> {
        budget.checkpoint()?;
        check_term_key(term)?;
        let payload = self.object.payload.as_slice();
        let rows = self
            .rows
            .get(..self.term_count)
            .ok_or_else(layout_invariant)?;
        let mut lower = 0;
        let mut upper = rows.len();
        while lower < upper {
            let middle = upper
                .checked_sub(lower)
                .and_then(|remaining| remaining.checked_div(2))
                .and_then(|half| lower.checked_add(half))
                .ok_or_else(layout_invariant)?;
            let row = rows.get(middle).ok_or_else(layout_invariant)?;
            let key = payload
                .get(row.start..row.end)
                .ok_or_else(layout_invariant)?;
            match key.cmp(term) {
                std::cmp::Ordering::Less => {
                    lower = middle.checked_add(1).ok_or_else(layout_invariant)?;
                }
                std::cmp::Ordering::Greater => upper = middle,
                std::cmp::Ordering::Equal => {
                    return Ok((self.selected_vector(row, budget)?, middle));
                }
            }
        }
        budget.checkpoint()?;
        Err(corrupt("term is absent from text block"))
    }

    fn selected_vector(
        &self,
        row: &RowSpan,
        budget: &WorkBudget,
    ) -> Result<Range<usize>, IndexError> {
        let vector = usize::from(row.vector);
        if vector >= self.vector_count {
            return Err(layout_invariant());
        }
        let selected = self.vectors.get(vector).ok_or_else(layout_invariant)?;
        if self
            .object
            .payload
            .get(selected.start..selected.end)
            .is_none()
        {
            return Err(layout_invariant());
        }
        budget.checkpoint()?;
        Ok(selected.start..selected.end)
    }

    /// An exact ordinal from this already checked layout avoids a second
    /// binary search when a sorted inventory cursor visits consecutive keys.
    pub(in crate::native) fn position_count_if_at(
        &self,
        ordinal: usize,
        term: &[u8],
        budget: &WorkBudget,
    ) -> Result<Option<u64>, IndexError> {
        budget.checkpoint()?;
        if ordinal >= self.term_count || self.key(ordinal)? != term {
            return Ok(None);
        }
        check_term_key(term)?;
        let row = self.rows.get(ordinal).ok_or_else(layout_invariant)?;
        let selected = self.selected_vector(row, budget)?;
        u64::try_from(selected.len().div_euclid(4))
            .map(Some)
            .map_err(|_error| IndexError::ResourceLimited)
    }

    pub(in crate::native) fn position_count_with_ordinal(
        &self,
        term: &[u8],
        budget: &WorkBudget,
    ) -> Result<(u64, usize), IndexError> {
        let (selected, ordinal) = self.selection_with_ordinal(term, budget)?;
        Ok((
            u64::try_from(selected.len().div_euclid(4))
                .map_err(|_error| IndexError::ResourceLimited)?,
            ordinal,
        ))
    }

    pub(in crate::native) fn position_count(
        &self,
        term: &[u8],
        budget: &WorkBudget,
    ) -> Result<u64, IndexError> {
        self.position_count_with_ordinal(term, budget)
            .map(|(count, _)| count)
    }
}

fn take<'a>(payload: &'a [u8], offset: &mut usize, count: usize) -> Result<&'a [u8], IndexError> {
    let end = offset
        .checked_add(count)
        .ok_or(IndexError::ResourceLimited)?;
    let selected = payload
        .get(*offset..end)
        .ok_or_else(|| corrupt("truncated text block"))?;
    *offset = end;
    Ok(selected)
}

fn read_u16(payload: &[u8], offset: &mut usize) -> Result<u16, IndexError> {
    let bytes = take(payload, offset, 2)?;
    let encoded: [u8; 2] = bytes
        .try_into()
        .map_err(|_error| corrupt("text block integer has invalid width"))?;
    Ok(u16::from_be_bytes(encoded))
}

fn corrupt(message: &str) -> IndexError {
    IndexError::CorruptObject(message.into())
}

fn layout_invariant() -> IndexError {
    IndexError::BackendViolation("checked text block layout exceeds its owned payload".into())
}

#[cfg(test)]
#[path = "block/tests.rs"]
mod tests;
