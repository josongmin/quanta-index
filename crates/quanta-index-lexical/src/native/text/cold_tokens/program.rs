//! Flat, borrowed canonical token programs for prefix reuse.

use super::{
    Arc, COLD_ALIAS_MIN_TERMS, COLD_INLINE_ANCHOR_KEY_BYTES, ColdBlockShape, ColdPackedAlias,
    ColdTokens, HEADER_BYTES, IndexError, MAX_BLOCK_BYTES, MAX_BLOCK_TERMS, MAX_POSITIONS_PER_TERM,
    MemoryLease, MemoryLifetime, Ordering, PackedBlockView, PreparedKey, PreparedPositions, Row,
    SerializeSeq, SerializeStruct, VectorIndex, WorkBudget, WorkUsage, check_term_key,
    measured_encoded_len, next_block_len, position_capacity,
};

const POSITION_CHUNK_ROWS: usize = 256;

fn position_chunks(rows: usize) -> Result<usize, IndexError> {
    rows.checked_add(POSITION_CHUNK_ROWS - 1)
        .and_then(|rounded| rounded.checked_div(POSITION_CHUNK_ROWS))
        .ok_or(IndexError::ResourceLimited)
}

/// A verified prefix borrows its original canonical rows. The suffix owns only
/// rows actually produced for this owner. Neither side copies token rows into
/// the other owner's arena.
pub(in crate::native::text) enum ColdTokenProgramView<'a> {
    Pair {
        prefix: Option<(&'a ColdTokens, u32)>,
        suffix: &'a ColdTokens,
    },
    Flat(&'a ColdTokenProgram),
}

#[derive(Clone, Copy)]
struct RowLocator(u64);

const _: () = assert!(super::super::MAX_NFC_BYTES <= (1_u64 << RowLocator::POSITION_BITS));
const _: () = assert!(super::super::MAX_NFC_BYTES + 1 < (1_u64 << RowLocator::ROW_BITS));

impl RowLocator {
    const SLAB_BITS: u32 = 6;
    const ROW_BITS: u32 = 25;
    const POSITION_BITS: u32 = 24;
    const SLAB_MASK: u64 = (1_u64 << Self::SLAB_BITS) - 1;
    const ROW_MASK: u64 = (1_u64 << Self::ROW_BITS) - 1;
    const POSITION_MASK: u64 = (1_u64 << Self::POSITION_BITS) - 1;

    fn new(slab: u8, row: usize, position: u32) -> Result<Self, IndexError> {
        let row = u64::try_from(row).map_err(|_error| IndexError::ResourceLimited)?;
        if u64::from(slab) > Self::SLAB_MASK
            || row > Self::ROW_MASK
            || u64::from(position) > Self::POSITION_MASK
        {
            return Err(IndexError::ResourceLimited);
        }
        Ok(Self(
            u64::from(slab)
                | (row << Self::SLAB_BITS)
                | (u64::from(position) << (Self::SLAB_BITS + Self::ROW_BITS)),
        ))
    }

    fn slab(self) -> u8 {
        self.0.to_le_bytes()[0] & ((1_u8 << Self::SLAB_BITS) - 1)
    }

    fn row(self) -> u32 {
        let bytes = self.0.to_le_bytes();
        (u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) >> Self::SLAB_BITS)
            & ((1_u32 << Self::ROW_BITS) - 1)
    }

    fn position(self) -> u32 {
        let bytes = self.0.to_le_bytes();
        (u32::from_le_bytes([bytes[3], bytes[4], bytes[5], bytes[6]])
            >> ((Self::SLAB_BITS + Self::ROW_BITS) % 8))
            & ((1_u32 << Self::POSITION_BITS) - 1)
    }

    fn with_slab(self, slab: u8) -> Result<Self, IndexError> {
        Self::new(
            slab,
            usize::try_from(self.row()).map_err(|_error| IndexError::ResourceLimited)?,
            self.position(),
        )
    }
}

#[cfg(test)]
mod row_locator_tests {
    use super::*;

    #[test]
    fn checked_locator_fits_eight_bytes_and_refuses_each_overflow() -> Result<(), IndexError> {
        assert_eq!(std::mem::size_of::<RowLocator>(), 8);
        let locator = RowLocator::new(63, (1 << 25) - 1, (1 << 24) - 1)?;
        assert_eq!(
            (locator.slab(), locator.row(), locator.position()),
            (63, (1 << 25) - 1, (1 << 24) - 1)
        );
        let remapped = locator.with_slab(0)?;
        assert_eq!(
            (remapped.slab(), remapped.row(), remapped.position()),
            (0, (1 << 25) - 1, (1 << 24) - 1)
        );
        assert!(matches!(
            RowLocator::new(64, 0, 0),
            Err(IndexError::ResourceLimited)
        ));
        assert!(matches!(
            RowLocator::new(0, 1 << 25, 0),
            Err(IndexError::ResourceLimited)
        ));
        assert!(matches!(
            RowLocator::new(0, 0, 1 << 24),
            Err(IndexError::ResourceLimited)
        ));
        assert!(matches!(
            locator.with_slab(64),
            Err(IndexError::ResourceLimited)
        ));
        Ok(())
    }
}

/// Flat ownership: each cached program points directly to canonical row
/// slabs, never to a previous program. An empty locator array denotes one
/// complete slab in its existing order. Only merged programs store locators.
pub(in crate::native::text) struct ColdTokenProgram {
    slabs: Vec<Arc<ColdTokens>>,
    sorted: Vec<RowLocator>,
    chunk_mins: Vec<u32>,
    _memory: MemoryLease,
    shared_memory: Option<MemoryLease>,
}

/// Both vector allocations and their Original grant are acquired before any
/// suffix token mutation. Construction after fill only writes into these
/// already funded capacities.
struct SelectedPrefixRows {
    rows: Vec<RowLocator>,
    _memory: Option<MemoryLease>,
}

struct ProgramBacking {
    slabs: Vec<Arc<ColdTokens>>,
    sorted: Vec<RowLocator>,
    chunk_mins: Vec<u32>,
    memory: MemoryLease,
}

pub(in crate::native::text) struct ColdProgramGrant {
    selected: SelectedPrefixRows,
    used: [bool; 64],
    prefix_count: usize,
    backing: ProgramBacking,
}

impl ColdProgramGrant {
    fn direct_position_observer<'a>(
        &'a mut self,
        count: usize,
        budget: &mut WorkBudget,
    ) -> Result<impl FnMut(usize, u32) -> Result<(), IndexError> + use<'a>, IndexError> {
        let chunks = position_chunks(count)?;
        let minima = &mut self.backing.chunk_mins;
        if !minima.is_empty() || minima.capacity() < chunks {
            return Err(IndexError::BackendViolation(
                "cold direct position index exceeds prefill grant".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(
                chunks
                    .checked_mul(std::mem::size_of::<u32>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let mut minimum = u32::MAX;
        let last = count.checked_sub(1);
        Ok(move |slot, position| {
            minimum = minimum.min(position);
            if slot % POSITION_CHUNK_ROWS == POSITION_CHUNK_ROWS - 1 || Some(slot) == last {
                minima.push(minimum);
                minimum = u32::MAX;
            }
            Ok(())
        })
    }

    pub(in crate::native::text) fn prepare_suffix(
        &mut self,
        suffix: &mut ColdTokens,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if self.backing.sorted.capacity() != 0 {
            return suffix.sort_and_validate(budget);
        }
        // Reuse the canonical validation walk to collect the positional index.
        let observe = self.direct_position_observer(suffix.rows.len(), budget)?;
        suffix.sort_and_validate_positions(budget, observe)
    }
}

impl ColdTokenProgram {
    #[cfg(test)]
    pub(in crate::native::text) fn drop_last_position_chunk_for_test(&mut self) {
        let _dropped = self.chunk_mins.pop();
    }

    pub(in crate::native::text) fn into_shared_with_memory(
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
                "cold program shared backing exceeds grant".into(),
            ));
        }
        self.shared_memory = Some(memory);
        Ok(Arc::new(self))
    }

    fn parts_in_slabs(
        slabs: &[Arc<ColdTokens>],
        locator: RowLocator,
    ) -> Result<(&[u8], u32), IndexError> {
        let slab = slabs
            .get(usize::from(locator.slab()))
            .ok_or(IndexError::ResourceLimited)?;
        let row = slab
            .rows
            .get(usize::try_from(locator.row()).map_err(|_error| IndexError::ResourceLimited)?)
            .ok_or(IndexError::ResourceLimited)?;
        if row.position() != locator.position() {
            return Err(IndexError::BackendViolation(
                "cold program locator position differs from row".into(),
            ));
        }
        Ok((
            ColdTokens::key(&slab.keys, row).ok_or(IndexError::ResourceLimited)?,
            row.position(),
        ))
    }

    fn parts(&self, locator: RowLocator) -> Result<(&[u8], u32), IndexError> {
        Self::parts_in_slabs(&self.slabs, locator)
    }

    fn direct_slab(&self) -> Result<Option<&ColdTokens>, IndexError> {
        if !self.sorted.is_empty() {
            return Ok(None);
        }
        match self.slabs.as_slice() {
            [slab] if slab.ready && !slab.rows.is_empty() => Ok(Some(slab)),
            _ => Err(IndexError::BackendViolation(
                "cold direct program lacks a complete slab".into(),
            )),
        }
    }

    fn row_count(&self) -> Result<usize, IndexError> {
        Ok(self
            .direct_slab()?
            .map_or(self.sorted.len(), |slab| slab.rows.len()))
    }

    /// Each indexed run owns exactly one row on each canonical case surface.
    /// This was checked while constructing the flat program and is reused
    /// only when the cache proves the entire normalized source body equal.
    pub(in crate::native::text) fn indexed_run_count(&self) -> Result<u64, IndexError> {
        let rows = self.row_count()?;
        if rows == 0 || !rows.is_multiple_of(2) {
            return Err(IndexError::BackendViolation(
                "cold program has invalid case row count".into(),
            ));
        }
        u64::try_from(rows.checked_div(2).ok_or(IndexError::ResourceLimited)?)
            .map_err(|_error| IndexError::ResourceLimited)
    }

    fn select_prefix(
        &self,
        cut: u32,
        expected: usize,
        selected: &mut Vec<RowLocator>,
        used: &mut [bool; 64],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        let direct = self.direct_slab()?;
        let rows = direct.map_or(self.sorted.len(), |slab| slab.rows.len());
        if self.chunk_mins.len() != position_chunks(rows)? {
            return Err(IndexError::BackendViolation(
                "cold prefix position chunk count differs".into(),
            ));
        }
        for (chunk, minimum) in self.chunk_mins.iter().enumerate() {
            budget.checkpoint()?;
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<u32>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            if *minimum >= cut {
                continue;
            }
            let start = chunk
                .checked_mul(POSITION_CHUNK_ROWS)
                .ok_or(IndexError::ResourceLimited)?;
            let end = start
                .checked_add(POSITION_CHUNK_ROWS)
                .ok_or(IndexError::ResourceLimited)?
                .min(rows);
            let width = if direct.is_some() {
                std::mem::size_of::<Row>()
            } else {
                std::mem::size_of::<RowLocator>()
            };
            budget.charge(WorkUsage {
                bytes: u64::try_from(
                    end.checked_sub(start)
                        .and_then(|count| count.checked_mul(width))
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            for index in start..end {
                let locator = if let Some(slab) = direct {
                    let position = slab
                        .rows
                        .get(index)
                        .ok_or(IndexError::ResourceLimited)?
                        .position();
                    if position >= cut {
                        continue;
                    }
                    RowLocator::new(0, index, position)?
                } else {
                    let locator = *self.sorted.get(index).ok_or(IndexError::ResourceLimited)?;
                    if locator.position() >= cut {
                        continue;
                    }
                    locator
                };
                if selected.len() == expected {
                    return Err(IndexError::BackendViolation(
                        "cold prefix row count exceeds current cursor".into(),
                    ));
                }
                *used
                    .get_mut(usize::from(locator.slab()))
                    .ok_or(IndexError::ResourceLimited)? = true;
                budget.charge(WorkUsage {
                    bytes: u64::try_from(std::mem::size_of::<RowLocator>())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                selected.push(locator);
            }
        }
        Ok(())
    }

    fn compare_suffix_to_prefix(
        &self,
        prefix: RowLocator,
        suffix: &ColdTokens,
        suffix_at: usize,
        additional_probe: bool,
        budget: &mut WorkBudget,
    ) -> Result<Ordering, IndexError> {
        if additional_probe {
            // The prefix row and the probed suffix row are both read again.
            budget.charge(WorkUsage {
                bytes: u64::try_from(
                    std::mem::size_of::<Row>()
                        .checked_mul(2)
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
        }
        let left_key = self.parts(prefix)?.0;
        let row = suffix
            .rows
            .get(suffix_at)
            .ok_or(IndexError::ResourceLimited)?;
        let right_key = ColdTokens::key(&suffix.keys, row).ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(
                left_key
                    .len()
                    .checked_add(right_key.len())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        Ok(right_key.cmp(left_key))
    }

    /// Optional cache construction before any owner output effect. A miss
    /// leaves the ordinary canonical owner path untouched.
    pub(in crate::native::text) fn try_reserve_for_suffix(
        inherited: Option<(&ColdTokenProgram, u32)>,
        expected_prefix_rows: usize,
        suffix_max_rows: usize,
        budget: &mut WorkBudget,
    ) -> Result<Option<ColdProgramGrant>, IndexError> {
        if inherited.is_none() && expected_prefix_rows != 0 {
            return Err(IndexError::BackendViolation(
                "cold prefix rows lack a source program".into(),
            ));
        }
        let selected_bytes = expected_prefix_rows
            .checked_mul(std::mem::size_of::<RowLocator>())
            .ok_or(IndexError::ResourceLimited)?;
        let selected_memory = if selected_bytes == 0 {
            None
        } else {
            let Some(memory) = budget.try_reserve_memory_for_optimization(
                u64::try_from(selected_bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?
            else {
                return Ok(None);
            };
            Some(memory)
        };
        let mut selected = Vec::new();
        if selected.try_reserve_exact(expected_prefix_rows).is_err()
            || selected.capacity() > expected_prefix_rows
        {
            return Ok(None);
        }
        let mut used = [false; 64];
        if let Some((program, cut)) = inherited {
            program.select_prefix(cut, expected_prefix_rows, &mut selected, &mut used, budget)?;
        }
        if selected.len() != expected_prefix_rows {
            return Err(IndexError::BackendViolation(
                "cold prefix row count differs from current cursor".into(),
            ));
        }
        let inherited_slabs = used.iter().filter(|selected| **selected).count();
        let slab_count = inherited_slabs
            .checked_add(usize::from(suffix_max_rows != 0))
            .ok_or(IndexError::ResourceLimited)?;
        if slab_count == 0 || slab_count > 64 {
            return Ok(None);
        }
        let max_rows = expected_prefix_rows
            .checked_add(suffix_max_rows)
            .ok_or(IndexError::ResourceLimited)?;
        if max_rows == 0 {
            return Ok(None);
        }
        let maximum_chunks = position_chunks(max_rows)?;
        let locator_capacity = if inherited.is_some() { max_rows } else { 0 };
        let bytes = locator_capacity
            .checked_mul(std::mem::size_of::<RowLocator>())
            .and_then(|bytes| {
                bytes.checked_add(slab_count.checked_mul(std::mem::size_of::<Arc<ColdTokens>>())?)
            })
            .and_then(|bytes| {
                bytes.checked_add(maximum_chunks.checked_mul(std::mem::size_of::<u32>())?)
            })
            .ok_or(IndexError::ResourceLimited)?;
        let Some(memory) = budget.try_reserve_memory_for_optimization(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(None);
        };
        let mut slabs = Vec::new();
        if slabs.try_reserve_exact(slab_count).is_err() {
            return Ok(None);
        }
        let mut sorted = Vec::new();
        let mut chunk_mins = Vec::new();
        if sorted.try_reserve_exact(locator_capacity).is_err()
            || chunk_mins.try_reserve_exact(maximum_chunks).is_err()
            || slabs.capacity() > slab_count
            || sorted.capacity() > locator_capacity
            || chunk_mins.capacity() > maximum_chunks
        {
            return Ok(None);
        }
        budget.checkpoint()?;
        Ok(Some(ColdProgramGrant {
            selected: SelectedPrefixRows {
                rows: selected,
                _memory: selected_memory,
            },
            used,
            prefix_count: expected_prefix_rows,
            backing: ProgramBacking {
                slabs,
                sorted,
                chunk_mins,
                memory,
            },
        }))
    }

    #[cfg(test)]
    pub(in crate::native::text) fn from_suffix(
        suffix: Option<Arc<ColdTokens>>,
        inherited: Option<(&ColdTokenProgram, u32)>,
        budget: &mut WorkBudget,
    ) -> Result<Option<Self>, IndexError> {
        let suffix = suffix.filter(|tokens| !tokens.rows.is_empty());
        let suffix_rows = suffix.as_ref().map_or(0, |tokens| tokens.rows.len());
        let mut expected_prefix_rows = 0;
        if let Some((program, cut)) = inherited {
            budget.charge(WorkUsage {
                bytes: u64::try_from(
                    program
                        .row_count()?
                        .checked_mul(std::mem::size_of::<RowLocator>())
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            expected_prefix_rows = if let Some(slab) = program.direct_slab()? {
                slab.rows.iter().filter(|row| row.position() < cut).count()
            } else {
                program
                    .sorted
                    .iter()
                    .filter(|locator| locator.position() < cut)
                    .count()
            };
        }
        let Some(mut memory) =
            Self::try_reserve_for_suffix(inherited, expected_prefix_rows, suffix_rows, budget)?
        else {
            return Ok(None);
        };
        if inherited.is_none()
            && let Some(slab) = &suffix
        {
            // Fixtures supply an already sorted slab. Account for their extra
            // walk, which the production validation pass fuses instead.
            budget.charge(WorkUsage {
                bytes: u64::try_from(
                    slab.rows
                        .len()
                        .checked_mul(std::mem::size_of::<Row>())
                        .ok_or(IndexError::ResourceLimited)?,
                )
                .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let mut observe = memory.direct_position_observer(suffix_rows, budget)?;
            for (slot, row) in slab.rows.iter().enumerate() {
                budget.checkpoint()?;
                observe(slot, row.position())?;
            }
        }
        Self::from_suffix_with_memory(suffix, inherited, memory, budget).map(Some)
    }

    /// Consume a prefill Original grant. A caller must obtain this before the
    /// first suffix row is charged, so admission failure never causes a second
    /// full-body token pass after partial mutation work.
    pub(in crate::native::text) fn from_suffix_with_memory(
        suffix: Option<Arc<ColdTokens>>,
        inherited: Option<(&ColdTokenProgram, u32)>,
        grant: ColdProgramGrant,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let ColdProgramGrant {
            selected,
            used,
            prefix_count,
            mut backing,
        } = grant;
        let suffix = suffix.filter(|tokens| !tokens.rows.is_empty());
        if suffix.is_none() && inherited.is_none() {
            return Err(IndexError::BackendViolation(
                "empty cold prefix program".into(),
            ));
        }
        if suffix.as_ref().is_some_and(|tokens| !tokens.ready)
            || inherited.is_some_and(|(program, _)| program.slabs.is_empty())
        {
            return Err(IndexError::BackendViolation(
                "cold prefix program is not ready".into(),
            ));
        }
        if inherited.is_some_and(|(_, cut)| {
            suffix
                .as_ref()
                .is_some_and(|tokens| tokens.rows.iter().any(|row| row.position() < cut))
        }) {
            return Err(IndexError::BackendViolation(
                "cold suffix precedes prefix boundary".into(),
            ));
        }
        let inherited_slabs = used.iter().filter(|selected| **selected).count();
        let slab_count = inherited_slabs
            .checked_add(usize::from(suffix.is_some()))
            .ok_or(IndexError::ResourceLimited)?;
        if slab_count > 64 {
            return Err(IndexError::ResourceLimited);
        }
        let total = prefix_count
            .checked_add(suffix.as_ref().map_or(0, |tokens| tokens.rows.len()))
            .ok_or(IndexError::ResourceLimited)?;
        let chunk_count = position_chunks(total)?;
        let direct = inherited.is_none();
        let locator_count = if direct { 0 } else { total };
        let metadata_bytes = locator_count
            .checked_mul(std::mem::size_of::<RowLocator>())
            .and_then(|bytes| {
                bytes.checked_add(slab_count.checked_mul(std::mem::size_of::<Arc<ColdTokens>>())?)
            })
            .and_then(|bytes| {
                bytes.checked_add(chunk_count.checked_mul(std::mem::size_of::<u32>())?)
            })
            .ok_or(IndexError::ResourceLimited)?;
        if backing.memory.bytes()
            < u64::try_from(metadata_bytes).map_err(|_error| IndexError::ResourceLimited)?
        {
            return Err(IndexError::BackendViolation(
                "cold program metadata exceeds prefill grant".into(),
            ));
        }
        if backing.slabs.capacity() < slab_count
            || backing.sorted.capacity() < locator_count
            || backing.chunk_mins.capacity() < chunk_count
        {
            return Err(IndexError::BackendViolation(
                "cold program exceeds prefill vector capacity".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: u64::try_from(
                slab_count
                    .checked_mul(std::mem::size_of::<Arc<ColdTokens>>())
                    .ok_or(IndexError::ResourceLimited)?,
            )
            .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        if direct {
            let slab = suffix.ok_or(IndexError::ResourceLimited)?;
            if prefix_count != 0
                || slab_count != 1
                || slab.rows.len() != total
                || backing.chunk_mins.len() != chunk_count
            {
                return Err(IndexError::BackendViolation(
                    "cold direct program differs from its prefill grant".into(),
                ));
            }
            backing.slabs.push(slab);
            return Ok(Self {
                slabs: backing.slabs,
                sorted: backing.sorted,
                chunk_mins: backing.chunk_mins,
                _memory: backing.memory,
                shared_memory: None,
            });
        }
        let mut remap = [0_u8; 64];
        if let Some((program, _)) = inherited {
            for (old, slab) in program.slabs.iter().enumerate() {
                if *used.get(old).ok_or(IndexError::ResourceLimited)? {
                    *remap.get_mut(old).ok_or(IndexError::ResourceLimited)? =
                        u8::try_from(backing.slabs.len())
                            .map_err(|_error| IndexError::ResourceLimited)?;
                    backing.slabs.push(Arc::clone(slab));
                }
            }
        }
        let suffix_slot =
            u8::try_from(backing.slabs.len()).map_err(|_error| IndexError::ResourceLimited)?;
        if let Some(suffix) = suffix {
            backing.slabs.push(suffix);
        }
        let mut prior = inherited.map(|_| selected.rows.iter().copied().peekable());
        let mut suffix_at = 0usize;
        let mut suffix_run_end = 0usize;
        let mut known_not_below_boundary = None::<usize>;
        let mut chunk_minimum = u32::MAX;
        while backing.sorted.len() < total {
            budget.checkpoint()?;
            let old = prior.as_mut().and_then(|rows| rows.peek().copied());
            let next = if let Some(row) = backing
                .slabs
                .get(usize::from(suffix_slot))
                .and_then(|slab| slab.rows.get(suffix_at))
            {
                Some(RowLocator::new(suffix_slot, suffix_at, row.position())?)
            } else {
                None
            };
            let take_old = match (old, next) {
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (Some(left), Some(right)) => {
                    if suffix_at < suffix_run_end {
                        false
                    } else {
                        let prefix = inherited.ok_or(IndexError::ResourceLimited)?.0;
                        let suffix_slab = backing
                            .slabs
                            .get(usize::from(right.slab()))
                            .ok_or(IndexError::ResourceLimited)?;
                        let suffix_not_below = if known_not_below_boundary.take() == Some(suffix_at)
                        {
                            true
                        } else {
                            prefix.compare_suffix_to_prefix(
                                left,
                                suffix_slab,
                                suffix_at,
                                false,
                                budget,
                            )? != Ordering::Less
                        };
                        if suffix_not_below {
                            true
                        } else {
                            // Find the first suffix key that is not strictly below
                            // this immutable prefix key. Equal keys keep all prefix
                            // positions first because every prefix position is < cut.
                            let mut last_below = suffix_at;
                            let mut stride = 1usize;
                            let mut first_not_below = suffix_slab.rows.len();
                            loop {
                                let probe = suffix_at
                                    .checked_add(stride)
                                    .ok_or(IndexError::ResourceLimited)?;
                                if probe >= suffix_slab.rows.len() {
                                    break;
                                }
                                let result = prefix.compare_suffix_to_prefix(
                                    left,
                                    suffix_slab,
                                    probe,
                                    true,
                                    budget,
                                )?;
                                if result == Ordering::Less {
                                    last_below = probe;
                                    stride =
                                        stride.checked_mul(2).ok_or(IndexError::ResourceLimited)?;
                                } else {
                                    first_not_below = probe;
                                    break;
                                }
                            }
                            let mut lower = last_below
                                .checked_add(1)
                                .ok_or(IndexError::ResourceLimited)?;
                            let mut upper = first_not_below;
                            while lower < upper {
                                let middle = lower
                                    .checked_add(
                                        upper
                                            .checked_sub(lower)
                                            .and_then(|distance| distance.checked_div(2))
                                            .ok_or(IndexError::ResourceLimited)?,
                                    )
                                    .ok_or(IndexError::ResourceLimited)?;
                                let result = prefix.compare_suffix_to_prefix(
                                    left,
                                    suffix_slab,
                                    middle,
                                    true,
                                    budget,
                                )?;
                                if result == Ordering::Less {
                                    lower =
                                        middle.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                                } else {
                                    upper = middle;
                                }
                            }
                            suffix_run_end = lower;
                            if lower < suffix_slab.rows.len() {
                                // This is the first suffix key not below the
                                // unchanged prefix head; its exact order is irrelevant.
                                known_not_below_boundary = Some(lower);
                            }
                            false
                        }
                    }
                }
                (None, None) => {
                    return Err(IndexError::BackendViolation(
                        "cold prefix merge lost rows".into(),
                    ));
                }
            };
            let locator = if take_old {
                known_not_below_boundary = None;
                let old = prior
                    .as_mut()
                    .and_then(Iterator::next)
                    .ok_or(IndexError::ResourceLimited)?;
                // The selected scratch locator is read once before writing
                // the final flat program's own locator below.
                budget.charge(WorkUsage {
                    bytes: u64::try_from(std::mem::size_of::<RowLocator>())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                old.with_slab(
                    *remap
                        .get(usize::from(old.slab()))
                        .ok_or(IndexError::ResourceLimited)?,
                )?
            } else {
                suffix_at = suffix_at
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                next.ok_or(IndexError::ResourceLimited)?
            };
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<RowLocator>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            chunk_minimum = chunk_minimum.min(locator.position());
            backing.sorted.push(locator);
            if backing.sorted.len() % POSITION_CHUNK_ROWS == 0 {
                budget.charge(WorkUsage {
                    bytes: u64::try_from(std::mem::size_of::<u32>())
                        .map_err(|_error| IndexError::ResourceLimited)?,
                    ..WorkUsage::default()
                })?;
                backing.chunk_mins.push(chunk_minimum);
                chunk_minimum = u32::MAX;
            }
        }
        if backing.sorted.len() % POSITION_CHUNK_ROWS != 0 {
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<u32>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            backing.chunk_mins.push(chunk_minimum);
        }
        if backing.chunk_mins.len() != chunk_count {
            return Err(IndexError::BackendViolation(
                "cold prefix position chunk count differs after merge".into(),
            ));
        }
        Ok(Self {
            slabs: backing.slabs,
            sorted: backing.sorted,
            chunk_mins: backing.chunk_mins,
            _memory: backing.memory,
            shared_memory: None,
        })
    }
}

#[derive(Clone, Copy)]
struct ProgramGroup<'a> {
    key: &'a [u8],
    rows: ProgramRows<'a>,
}

#[derive(Clone, Copy)]
enum ProgramRows<'a> {
    Pair {
        prefix: &'a [Row],
        suffix: &'a [Row],
    },
    Flat {
        locators: &'a [RowLocator],
    },
}

impl Default for ProgramGroup<'_> {
    fn default() -> Self {
        Self {
            key: &[],
            rows: ProgramRows::Pair {
                prefix: &[],
                suffix: &[],
            },
        }
    }
}

struct BorrowedProgramOccurrence<'a>(ProgramGroup<'a>);

impl serde::Serialize for BorrowedProgramOccurrence<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        struct Positions<'a>(ProgramGroup<'a>);
        impl serde::Serialize for Positions<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let mut sequence = serializer
                    .serialize_seq(Some(self.0.len().map_err(serde::ser::Error::custom)?))?;
                for position in self.0.positions() {
                    sequence.serialize_element(&position)?;
                }
                sequence.end()
            }
        }
        let mut record = serializer.serialize_struct("Occurrence", 1)?;
        record.serialize_field("positions", &Positions(self.0))?;
        record.end()
    }
}

impl<'a> ProgramGroup<'a> {
    fn pair_prefix(self) -> Result<&'a [Row], IndexError> {
        match self.rows {
            ProgramRows::Pair { prefix, .. } => Ok(prefix),
            ProgramRows::Flat { .. } => Err(IndexError::BackendViolation(
                "flat group used as pair".into(),
            )),
        }
    }

    fn pair_suffix(self) -> Result<&'a [Row], IndexError> {
        match self.rows {
            ProgramRows::Pair { suffix, .. } => Ok(suffix),
            ProgramRows::Flat { .. } => Err(IndexError::BackendViolation(
                "flat group used as pair".into(),
            )),
        }
    }

    fn len(self) -> Result<usize, IndexError> {
        match self.rows {
            ProgramRows::Pair { prefix, suffix } => prefix
                .len()
                .checked_add(suffix.len())
                .ok_or(IndexError::ResourceLimited),
            ProgramRows::Flat { locators, .. } => Ok(locators.len()),
        }
    }

    fn positions(self) -> impl Iterator<Item = u32> + 'a {
        let (prefix, suffix, locators) = match self.rows {
            ProgramRows::Pair { prefix, suffix } => (prefix, suffix, &[][..]),
            ProgramRows::Flat { locators, .. } => (&[][..], &[][..], locators),
        };
        prefix
            .iter()
            .chain(suffix.iter())
            .map(|row| row.position())
            .chain(locators.iter().map(|locator| locator.position()))
    }

    fn position(self, ordinal: usize) -> Result<u32, IndexError> {
        match self.rows {
            ProgramRows::Pair { prefix, suffix } => prefix
                .get(ordinal)
                .or_else(|| {
                    ordinal
                        .checked_sub(prefix.len())
                        .and_then(|at| suffix.get(at))
                })
                .map(|row| row.position())
                .ok_or(IndexError::ResourceLimited),
            ProgramRows::Flat { locators, .. } => locators
                .get(ordinal)
                .map(|locator| locator.position())
                .ok_or(IndexError::ResourceLimited),
        }
    }
}

/// The packed encoder's term descriptors borrow either original row slab.
/// Vector ordinals still refer to the canonical writer's deduplicated vectors.
#[expect(
    clippy::large_enum_variant,
    reason = "Both bounded alias variants borrow canonical rows; boxing would add an unfunded heap allocation"
)]
pub(in crate::native::text) enum ProgramPackedAlias<'a> {
    Direct(ColdPackedAlias<'a>),
    Prefix(MergedPackedAlias<'a>),
}

pub(in crate::native::text) struct MergedPackedAlias<'a> {
    terms: [ProgramTerm<'a>; MAX_BLOCK_TERMS],
    vectors: [u16; MAX_BLOCK_TERMS],
    index: VectorIndex,
    term_count: usize,
    vector_count: usize,
    encoded_len: usize,
    _memory: MemoryLease,
}

#[derive(Clone, Copy)]
struct ProgramTerm<'a> {
    group: ProgramGroup<'a>,
    vector: u16,
}

impl Default for ProgramTerm<'_> {
    fn default() -> Self {
        Self {
            group: ProgramGroup {
                key: &[],
                rows: ProgramRows::Pair {
                    prefix: &[],
                    suffix: &[],
                },
            },
            vector: 0,
        }
    }
}

impl ProgramPackedAlias<'_> {
    pub(in crate::native::text) fn encoded_len(&self) -> usize {
        match self {
            Self::Direct(alias) => alias.encoded_len(),
            Self::Prefix(alias) => alias.encoded_len,
        }
    }

    pub(in crate::native::text) fn first_key(&self) -> Result<&[u8], IndexError> {
        self.term(0).map(|(key, _)| key)
    }
}

impl PackedBlockView for ProgramPackedAlias<'_> {
    fn term_count(&self) -> usize {
        match self {
            Self::Direct(alias) => alias.term_count(),
            Self::Prefix(alias) => alias.term_count,
        }
    }

    fn vector_count(&self) -> usize {
        match self {
            Self::Direct(alias) => alias.vector_count(),
            Self::Prefix(alias) => alias.vector_count,
        }
    }

    fn term(&self, ordinal: usize) -> Result<(&[u8], u16), IndexError> {
        match self {
            Self::Direct(alias) => alias.term(ordinal),
            Self::Prefix(alias) => {
                let term = alias
                    .terms
                    .get(ordinal)
                    .ok_or(IndexError::ResourceLimited)?;
                Ok((term.group.key, term.vector))
            }
        }
    }

    fn vector_len(&self, ordinal: usize) -> Result<usize, IndexError> {
        match self {
            Self::Direct(alias) => alias.vector_len(ordinal),
            Self::Prefix(alias) => {
                let term = usize::from(
                    *alias
                        .vectors
                        .get(ordinal)
                        .ok_or(IndexError::ResourceLimited)?,
                );
                alias
                    .terms
                    .get(term)
                    .ok_or(IndexError::ResourceLimited)?
                    .group
                    .len()
            }
        }
    }

    fn position(&self, vector: usize, ordinal: usize) -> Result<u32, IndexError> {
        match self {
            Self::Direct(alias) => alias.position(vector, ordinal),
            Self::Prefix(alias) => {
                let term = usize::from(
                    *alias
                        .vectors
                        .get(vector)
                        .ok_or(IndexError::ResourceLimited)?,
                );
                alias
                    .terms
                    .get(term)
                    .ok_or(IndexError::ResourceLimited)?
                    .group
                    .position(ordinal)
            }
        }
    }
}

impl<'a> ColdTokenProgramView<'a> {
    /// `first_suffix_position` is the zero-based number of *all* preceding
    /// canonical runs, including runs excluded from the index. The caller must
    /// separately prove the exact NFC byte prefix and delimiter boundary.
    pub(in crate::native::text) fn new(
        prefix: Option<(&'a ColdTokens, u32)>,
        suffix: &'a ColdTokens,
    ) -> Result<Self, IndexError> {
        if !suffix.ready || prefix.is_some_and(|(tokens, _)| !tokens.ready) {
            return Err(IndexError::BackendViolation(
                "cold token program is not ordered".into(),
            ));
        }
        Ok(Self::Pair { prefix, suffix })
    }

    pub(in crate::native::text) fn from_program(
        program: &'a ColdTokenProgram,
    ) -> Result<Self, IndexError> {
        if let Some(slab) = program.direct_slab()? {
            return Self::new(None, slab);
        }
        if program.slabs.is_empty() {
            return Err(IndexError::BackendViolation(
                "empty cold token program".into(),
            ));
        }
        Ok(Self::Flat(program))
    }

    fn next_source_group(
        tokens: &'a ColdTokens,
        at: &mut usize,
        before: Option<u32>,
        after: Option<u32>,
        budget: &WorkBudget,
    ) -> Result<Option<ProgramGroup<'a>>, IndexError> {
        while let Some(first) = tokens.rows.get(*at) {
            budget.checkpoint()?;
            let key = ColdTokens::key(&tokens.keys, first).ok_or(IndexError::ResourceLimited)?;
            let start = *at;
            let mut end = start;
            while let Some(row) = tokens.rows.get(end) {
                if ColdTokens::key(&tokens.keys, row).ok_or(IndexError::ResourceLimited)? != key {
                    break;
                }
                end = end.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            *at = end;
            let rows = tokens
                .rows
                .get(start..end)
                .ok_or(IndexError::ResourceLimited)?;
            let selected = if let Some(limit) = before {
                let count = rows.partition_point(|row| row.position() < limit);
                rows.get(..count).ok_or(IndexError::ResourceLimited)?
            } else {
                if after.is_some_and(|limit| rows.first().is_some_and(|row| row.position() < limit))
                {
                    return Err(IndexError::BackendViolation(
                        "cold suffix position invalid".into(),
                    ));
                }
                rows
            };
            if !selected.is_empty() {
                return Ok(Some(if before.is_some() {
                    ProgramGroup {
                        key,
                        rows: ProgramRows::Pair {
                            prefix: selected,
                            suffix: &[],
                        },
                    }
                } else {
                    ProgramGroup {
                        key,
                        rows: ProgramRows::Pair {
                            prefix: &[],
                            suffix: selected,
                        },
                    }
                }));
            }
        }
        Ok(None)
    }

    fn for_each_group(
        &self,
        budget: &mut WorkBudget,
        mut consume: impl FnMut(ProgramGroup<'a>, &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        if let Self::Flat(program) = self {
            let program: &'a ColdTokenProgram = program;
            let mut at = 0usize;
            while let Some(first) = program.sorted.get(at) {
                budget.checkpoint()?;
                let (key, position) = program.parts(*first)?;
                let start = at;
                at = at.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                let mut previous = position;
                while let Some(locator) = program.sorted.get(at) {
                    let (candidate, position) = program.parts(*locator)?;
                    if candidate != key {
                        break;
                    }
                    if position <= previous {
                        return Err(IndexError::CorruptObject(
                            "cold token program position repeats".into(),
                        ));
                    }
                    previous = position;
                    at = at.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                }
                let locators = program
                    .sorted
                    .get(start..at)
                    .ok_or(IndexError::ResourceLimited)?;
                let group = ProgramGroup {
                    key,
                    rows: ProgramRows::Flat { locators },
                };
                check_term_key(group.key)?;
                if u64::try_from(group.len()?).map_err(|_error| IndexError::ResourceLimited)?
                    > MAX_POSITIONS_PER_TERM
                {
                    return Err(IndexError::ResourceLimited);
                }
                consume(group, budget)?;
            }
            return budget.checkpoint();
        }
        let Self::Pair {
            prefix: pair_prefix,
            suffix: pair_suffix,
        } = self
        else {
            return Err(IndexError::BackendViolation(
                "cold program variant changed".into(),
            ));
        };
        let prefix_source = *pair_prefix;
        let suffix_source = *pair_suffix;
        let mut prefix_at = 0;
        let mut suffix_at = 0;
        let mut prefix = prefix_source
            .map(|(tokens, limit)| {
                Self::next_source_group(tokens, &mut prefix_at, Some(limit), None, budget)
            })
            .transpose()?
            .flatten();
        let mut suffix = Self::next_source_group(
            suffix_source,
            &mut suffix_at,
            None,
            prefix_source.map(|(_, limit)| limit),
            budget,
        )?;
        loop {
            budget.checkpoint()?;
            let group = match (prefix, suffix) {
                (None, None) => break,
                (Some(left), None) => {
                    prefix = prefix_source
                        .map(|(tokens, limit)| {
                            Self::next_source_group(
                                tokens,
                                &mut prefix_at,
                                Some(limit),
                                None,
                                budget,
                            )
                        })
                        .transpose()?
                        .flatten();
                    left
                }
                (None, Some(right)) => {
                    suffix = Self::next_source_group(
                        suffix_source,
                        &mut suffix_at,
                        None,
                        prefix_source.map(|(_, limit)| limit),
                        budget,
                    )?;
                    right
                }
                (Some(left), Some(right)) => match left.key.cmp(right.key) {
                    Ordering::Less => {
                        prefix = prefix_source
                            .map(|(tokens, limit)| {
                                Self::next_source_group(
                                    tokens,
                                    &mut prefix_at,
                                    Some(limit),
                                    None,
                                    budget,
                                )
                            })
                            .transpose()?
                            .flatten();
                        left
                    }
                    Ordering::Greater => {
                        suffix = Self::next_source_group(
                            suffix_source,
                            &mut suffix_at,
                            None,
                            prefix_source.map(|(_, limit)| limit),
                            budget,
                        )?;
                        right
                    }
                    Ordering::Equal => {
                        prefix = prefix_source
                            .map(|(tokens, limit)| {
                                Self::next_source_group(
                                    tokens,
                                    &mut prefix_at,
                                    Some(limit),
                                    None,
                                    budget,
                                )
                            })
                            .transpose()?
                            .flatten();
                        suffix = Self::next_source_group(
                            suffix_source,
                            &mut suffix_at,
                            None,
                            prefix_source.map(|(_, limit)| limit),
                            budget,
                        )?;
                        ProgramGroup {
                            key: left.key,
                            rows: ProgramRows::Pair {
                                prefix: left.pair_prefix()?,
                                suffix: right.pair_suffix()?,
                            },
                        }
                    }
                },
            };
            check_term_key(group.key)?;
            if group.len()? == 0
                || u64::try_from(group.len()?).map_err(|_error| IndexError::ResourceLimited)?
                    > MAX_POSITIONS_PER_TERM
            {
                return Err(IndexError::ResourceLimited);
            }
            consume(group, budget)?;
        }
        budget.checkpoint()
    }

    /// Expose the authenticated term and its ordered positions under the
    /// program's existing grants, through the end of the visitor call.
    pub(in crate::native::text) fn visit_borrowed_groups(
        &self,
        budget: &mut WorkBudget,
        mut consume: impl FnMut(
            &[u8],
            &mut dyn Iterator<Item = u32>,
            &mut WorkBudget,
        ) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        self.for_each_group(budget, |group, budget| {
            consume(group.key, &mut group.positions(), budget)
        })
    }

    pub(in crate::native::text) fn visit_groups(
        &self,
        limit: usize,
        budget: &mut WorkBudget,
        mut consume: impl FnMut(
            PreparedKey,
            PreparedPositions,
            &mut WorkBudget,
        ) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        if let Self::Pair {
            prefix: None,
            suffix,
        } = self
        {
            return suffix.visit_groups(limit, budget, consume);
        }
        self.for_each_group(budget, |group, budget| {
            let key = PreparedKey::copy(group.key, budget)?;
            let mut positions = PreparedPositions::new(limit)?;
            for position in group.positions() {
                positions.push(position, budget)?;
            }
            consume(key, positions, budget)
        })
    }

    pub(in crate::native::text) fn replay_borrowed_pages(
        &self,
        budget: &mut WorkBudget,
        mut consume: impl FnMut(&[(&[u8], u32)], &mut WorkBudget) -> Result<(), IndexError>,
    ) -> Result<(), IndexError> {
        if let Self::Pair {
            prefix: None,
            suffix,
        } = self
        {
            return suffix.replay_borrowed_pages(budget, consume);
        }
        let mut page = [(&[][..], 0_u32); quanta_index_scratch_sqlite::MAX_WRITE_BATCH_ROWS];
        let mut used = 0usize;
        self.for_each_group(budget, |group, budget| {
            for position in group.positions() {
                if used == page.len() {
                    consume(page.get(..used).ok_or(IndexError::ResourceLimited)?, budget)?;
                    used = 0;
                }
                *page.get_mut(used).ok_or(IndexError::ResourceLimited)? = (group.key, position);
                used = used.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            }
            Ok(())
        })?;
        if used != 0 {
            consume(page.get(..used).ok_or(IndexError::ResourceLimited)?, budget)?;
        }
        Ok(())
    }

    pub(in crate::native::text) fn block_shape(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<ColdBlockShape, IndexError> {
        if let Self::Pair {
            prefix: None,
            suffix,
        } = self
        {
            return suffix.block_shape(budget);
        }
        let mut shape = ColdBlockShape::default();
        // Bounded, non-escaping scratch; the shape pass adds no heap backing.
        #[expect(
            clippy::large_stack_arrays,
            reason = "Bounded non-escaping shape scratch"
        )]
        let mut vectors = [ProgramGroup::default(); MAX_BLOCK_TERMS];
        let mut index = VectorIndex::within_owner_grant();
        let mut vector_count = 0usize;
        let mut block_terms = 0usize;
        let mut block_len = HEADER_BYTES;
        let mut block_key_bytes = 0usize;
        let mut block_position_bytes = 0usize;
        let mut only_positions = ProgramGroup::default();
        self.for_each_group(budget, |group, budget| {
            let key = group.key;
            let positions = group.len()?;
            shape.term_count = shape
                .term_count
                .checked_add(1)
                .ok_or(IndexError::ResourceLimited)?;
            shape.maximum_key_bytes = shape.maximum_key_bytes.max(key.len());
            shape.maximum_group_positions = shape.maximum_group_positions.max(positions);
            loop {
                let capacity = position_capacity(positions)?;
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
                    Ok(vectors
                        .get(ordinal)
                        .ok_or(IndexError::ResourceLimited)?
                        .positions()
                        .cmp(group.positions()))
                })?;
                if let Some(next) =
                    next_block_len(block_len, block_terms, key.len(), positions, same.is_none())?
                {
                    block_len = next;
                    block_key_bytes = block_key_bytes
                        .checked_add(key.len())
                        .ok_or(IndexError::ResourceLimited)?;
                    block_terms = block_terms
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                    if block_terms == 1 {
                        only_positions = group;
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
                            .ok_or(IndexError::ResourceLimited)? = group;
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
                        &BorrowedProgramOccurrence(group),
                        budget,
                    )?)?;
                    break;
                }
                let payload_bytes = if block_terms == 1 {
                    measured_encoded_len(&BorrowedProgramOccurrence(only_positions), budget)?
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
            Ok(())
        })?;
        if block_terms != 0 {
            let payload_bytes = if block_terms == 1 {
                measured_encoded_len(&BorrowedProgramOccurrence(only_positions), budget)?
            } else {
                block_len
            };
            shape.record(payload_bytes)?;
        }
        budget.checkpoint()?;
        Ok(shape)
    }

    pub(in crate::native::text) fn try_packed_alias(
        &self,
        budget: &mut WorkBudget,
    ) -> Result<Option<ProgramPackedAlias<'a>>, IndexError> {
        if let Self::Pair {
            prefix: None,
            suffix,
        } = self
        {
            let suffix: &'a ColdTokens = suffix;
            return suffix
                .try_packed_alias(budget)
                .map(|alias| alias.map(ProgramPackedAlias::Direct));
        }
        let mut footprint = super::AliasFootprint::default();
        self.for_each_group(budget, |group, _budget| {
            footprint.add_key(group.key)?;
            footprint.largest_vector = footprint.largest_vector.max(group.len()?);
            Ok(())
        })?;
        if !footprint.can_fit() {
            return Ok(None);
        }
        let bytes = u64::try_from(std::mem::size_of::<MergedPackedAlias<'a>>())
            .map_err(|_error| IndexError::ResourceLimited)?;
        let Some(memory) =
            budget.try_reserve_memory_for_optimization(bytes, MemoryLifetime::Temporary)?
        else {
            return Ok(None);
        };
        // The Optional Original grant above covers this bounded alias body.
        #[expect(
            clippy::large_stack_arrays,
            reason = "Bounded alias body has an admitted Original grant"
        )]
        let mut candidate = MergedPackedAlias {
            terms: [ProgramTerm::default(); MAX_BLOCK_TERMS],
            vectors: [0; MAX_BLOCK_TERMS],
            index: VectorIndex::within_owner_grant(),
            term_count: 0,
            vector_count: 0,
            encoded_len: HEADER_BYTES,
            _memory: memory,
        };
        let mut rejected = false;
        self.for_each_group(budget, |group, budget| {
            if rejected {
                return Ok(());
            }
            if candidate.term_count == MAX_BLOCK_TERMS
                || (candidate.term_count == 0 && group.key.len() > COLD_INLINE_ANCHOR_KEY_BYTES)
            {
                rejected = true;
                return Ok(());
            }
            let inspected = group
                .key
                .len()
                .checked_add(
                    group
                        .len()?
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
                    .terms
                    .get(term)
                    .ok_or(IndexError::ResourceLimited)?
                    .group
                    .positions()
                    .cmp(group.positions()))
            })?;
            let vector = match same {
                Some(vector) => {
                    u16::try_from(vector).map_err(|_error| IndexError::ResourceLimited)?
                }
                None => u16::try_from(candidate.vector_count)
                    .map_err(|_error| IndexError::ResourceLimited)?,
            };
            let row_bytes = 4usize
                .checked_add(group.key.len())
                .ok_or(IndexError::ResourceLimited)?;
            let vector_bytes = if same.is_some() {
                0
            } else {
                group
                    .len()?
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
                rejected = true;
                return Ok(());
            }
            // This descriptor is new physical metadata for a split slab; the
            // canonical Row backing itself remains borrowed and unchanged.
            budget.charge(WorkUsage {
                bytes: u64::try_from(std::mem::size_of::<ProgramTerm<'a>>())
                    .map_err(|_error| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })?;
            let ordinal = candidate.term_count;
            *candidate
                .terms
                .get_mut(ordinal)
                .ok_or(IndexError::ResourceLimited)? = ProgramTerm { group, vector };
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
                    u16::try_from(ordinal).map_err(|_error| IndexError::ResourceLimited)?;
                candidate.index.insert(insertion, vector)?;
                candidate.vector_count = candidate
                    .vector_count
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            Ok(())
        })?;
        if rejected
            || u64::try_from(candidate.term_count).map_err(|_error| IndexError::ResourceLimited)?
                < COLD_ALIAS_MIN_TERMS
        {
            return Ok(None);
        }
        Ok(Some(ProgramPackedAlias::Prefix(candidate)))
    }
}
