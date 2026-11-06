//! Bounded, owner-local term to position-vector dictionary.
//!
//! A checked layout owns its authenticated immutable payload and DATA lease.

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
const HEADER_BYTES: usize = 16;

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

pub(in crate::native) struct BlockDraft {
    // Both containers acquire their own backing before the first push.
    // The first occurrence of each distinct vector owns its position grant.
    terms: PreparedRows<BlockTerm>,
    vectors: PreparedRows<PreparedPositions>,
    encoded_len: usize,
}

impl BlockDraft {
    pub(in crate::native) fn new() -> Result<Self, IndexError> {
        Ok(Self {
            terms: PreparedRows::new(MAX_BLOCK_TERMS)?,
            vectors: PreparedRows::new(MAX_BLOCK_TERMS)?,
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
        let mut same = None;
        for (index, saved) in self.vectors.as_slice().iter().enumerate() {
            // The 512-row envelope is bounded, but distinct vectors can make
            // this scan quadratic. Keep cancellation live within that bound.
            if index.is_multiple_of(32) {
                budget.checkpoint()?;
            }
            if saved.as_slice() == values {
                same = Some(index);
                break;
            }
        }
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
                let memory = budget.reserve_memory(
                    u64::try_from(self.encoded_len)
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    MemoryLifetime::Temporary,
                )?;
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(self.encoded_len)
                    .map_err(|_error| IndexError::ResourceLimited)?;
                bytes.extend_from_slice(MAGIC);
                bytes.extend_from_slice(
                    &u16::try_from(self.terms.len())
                        .map_err(|_error| IndexError::ResourceLimited)?
                        .to_be_bytes(),
                );
                bytes.extend_from_slice(
                    &u16::try_from(self.vectors.len())
                        .map_err(|_error| IndexError::ResourceLimited)?
                        .to_be_bytes(),
                );
                bytes.extend_from_slice(&0_u32.to_be_bytes());
                for row in self.terms.as_slice() {
                    budget.checkpoint()?;
                    let key = row.key.as_slice();
                    bytes.extend_from_slice(
                        &u16::try_from(key.len())
                            .map_err(|_error| IndexError::ResourceLimited)?
                            .to_be_bytes(),
                    );
                    bytes.extend_from_slice(key);
                    bytes.extend_from_slice(&row.vector.to_be_bytes());
                }
                for vector in self.vectors.as_slice() {
                    budget.checkpoint()?;
                    let positions = vector.as_slice();
                    bytes.extend_from_slice(
                        &u16::try_from(positions.len())
                            .map_err(|_error| IndexError::ResourceLimited)?
                            .to_be_bytes(),
                    );
                    for position in positions {
                        bytes.extend_from_slice(&position.to_be_bytes());
                    }
                }
                if bytes.len() != self.encoded_len || bytes.len() > MAX_BLOCK_BYTES {
                    return Err(IndexError::BackendViolation(
                        "text block wire census changed".into(),
                    ));
                }
                budget.checkpoint()?;
                Ok(SealedBlock::Packed {
                    payload: StagedPayload { bytes, memory },
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
            for (prior_index, prior) in checked
                .vectors
                .get(..index)
                .ok_or_else(|| corrupt("text block vector exceeds layout"))?
                .iter()
                .enumerate()
            {
                if prior_index.is_multiple_of(32) {
                    budget.checkpoint()?;
                }
                if payload
                    .get(prior.start..prior.end)
                    .ok_or_else(|| corrupt("text block vector exceeds payload"))?
                    == values
                {
                    return Err(corrupt("text block repeats a position vector"));
                }
            }
            *checked
                .vectors
                .get_mut(index)
                .ok_or_else(|| corrupt("text block vector exceeds layout"))? =
                VectorSpan { start, end: offset };
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

    pub(in crate::native) fn selection(
        &self,
        term: &[u8],
        budget: &WorkBudget,
    ) -> Result<Range<usize>, IndexError> {
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
                    let vector = usize::from(row.vector);
                    if vector >= self.vector_count {
                        return Err(layout_invariant());
                    }
                    let selected = self.vectors.get(vector).ok_or_else(layout_invariant)?;
                    if payload.get(selected.start..selected.end).is_none() {
                        return Err(layout_invariant());
                    }
                    budget.checkpoint()?;
                    return Ok(selected.start..selected.end);
                }
            }
        }
        budget.checkpoint()?;
        Err(corrupt("term is absent from text block"))
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
