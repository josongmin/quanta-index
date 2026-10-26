//! At most sixteen real branches, ordered by the authenticated nibble mask.
//! Leaves own no branch allocation. Growth funds old and new backing together.

use super::{Branch, IndexError, MapBuffer, WorkBudget, WorkUsage};

pub(super) struct Branches {
    mask: u16,
    values: MapBuffer<Branch>,
}

impl Branches {
    pub(super) fn new() -> Self {
        Self {
            mask: 0,
            values: MapBuffer::new(16),
        }
    }

    pub(super) fn for_decode(mask: u16, budget: &WorkBudget) -> Result<Self, IndexError> {
        let capacity =
            usize::try_from(mask.count_ones()).map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self {
            mask: 0,
            values: MapBuffer::with_capacity(capacity, 16, budget)?,
        })
    }

    fn bit(edge: usize) -> Result<u16, IndexError> {
        1_u16
            .checked_shl(u32::try_from(edge).map_err(|_error| IndexError::ResourceLimited)?)
            .ok_or_else(|| IndexError::CorruptObject("map branch out of bounds".into()))
    }

    fn rank(&self, bit: u16) -> Result<usize, IndexError> {
        usize::try_from((self.mask & bit.wrapping_sub(1)).count_ones())
            .map_err(|_error| IndexError::ResourceLimited)
    }

    pub(super) fn get(&self, edge: usize) -> Result<Option<&Branch>, IndexError> {
        let bit = Self::bit(edge)?;
        if self.mask & bit == 0 {
            return Ok(None);
        }
        self.values.get(self.rank(bit)?).map(Some).ok_or_else(|| {
            IndexError::BackendViolation("map branch mask disagrees with backing".into())
        })
    }

    pub(super) fn len(&self) -> usize {
        self.values.len()
    }

    /// Lookup consumes its decoded node. Moving a selected child out requires
    /// no new backing and releases every unselected branch with this node.
    pub(super) fn into_child(mut self, edge: usize) -> Result<Option<Branch>, IndexError> {
        let bit = Self::bit(edge)?;
        if self.mask & bit == 0 {
            return Ok(None);
        }
        let rank = self.rank(bit)?;
        self.values.remove(rank).map(Some)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (usize, &Branch)> {
        (0..16)
            .filter(|edge| self.mask & (1_u16 << edge) != 0)
            .zip(self.values.iter())
    }

    pub(super) fn capacity_bytes(&self) -> Result<usize, IndexError> {
        self.values.capacity_bytes()
    }

    /// The decoder has already billed authenticated wire bytes. Its backing is
    /// still admitted before allocation; mutating writers use `replace` below.
    pub(super) fn append_decoded(
        &mut self,
        edge: usize,
        branch: Branch,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        let bit = Self::bit(edge)?;
        if self.mask >= bit {
            return Err(IndexError::CorruptObject(
                "map branches are not ordered".into(),
            ));
        }
        self.values.push(branch, budget)?;
        self.mask |= bit;
        Ok(())
    }

    /// Charge the actual backing growth/copy and bounded shifted records before
    /// mutation. Refused growth leaves the mask, values and prior lease intact.
    pub(super) fn replace(
        &mut self,
        edge: usize,
        branch: Option<Branch>,
        budget: &mut WorkBudget,
    ) -> Result<Option<Branch>, IndexError> {
        budget.checkpoint()?;
        let bit = Self::bit(edge)?;
        let rank = self.rank(bit)?;
        let present = self.mask & bit != 0;
        if !present && branch.is_none() {
            return Ok(None);
        }
        let shifted = if present && branch.is_some() {
            0
        } else {
            self.values
                .len()
                .checked_sub(rank)
                .ok_or(IndexError::ResourceLimited)?
        };
        let growth = if !present && branch.is_some() {
            self.values.growth_work_bytes()?
        } else {
            0
        };
        let bytes = shifted
            .checked_add(1)
            .and_then(|records| records.checked_mul(std::mem::size_of::<Branch>()))
            .and_then(|bytes| bytes.checked_add(growth))
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        match (present, branch) {
            (false, Some(branch)) => {
                self.values.insert(rank, branch, budget)?;
                self.mask |= bit;
                Ok(None)
            }
            (true, Some(branch)) => {
                let old = self.values.get_mut(rank).ok_or_else(|| {
                    IndexError::BackendViolation("map branch mask disagrees with backing".into())
                })?;
                Ok(Some(std::mem::replace(old, branch)))
            }
            (true, None) => {
                let old = self.values.remove(rank)?;
                self.mask &= !bit;
                Ok(Some(old))
            }
            (false, None) => Ok(None),
        }
    }
}
