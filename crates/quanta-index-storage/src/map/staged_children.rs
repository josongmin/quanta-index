//! Negative membership for immutable nodes authored by one cold staging pass.

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectRef, WorkBudget, WorkUsage,
};

const PREFIX_BITS: usize = 12;
const TABLE_BYTES: usize = (1 << PREFIX_BITS) / 8;

pub(super) struct StagedChildren {
    bits: Vec<u8>,
    // The backing drops before its optional temporary grant.
    _memory: MemoryLease,
}

impl StagedChildren {
    pub(super) fn try_new(budget: &mut WorkBudget) -> Result<Option<Self>, IndexError> {
        let bytes = u64::try_from(TABLE_BYTES).map_err(|_error| IndexError::ResourceLimited)?;
        let Some(memory) =
            budget.try_reserve_memory_for_optimization(bytes, MemoryLifetime::Temporary)?
        else {
            return Ok(None);
        };
        let mut bits = Vec::new();
        if bits.try_reserve_exact(TABLE_BYTES).is_err() || bits.capacity() != TABLE_BYTES {
            return Ok(None);
        }
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        bits.resize(TABLE_BYTES, 0);
        Ok(Some(Self {
            bits,
            _memory: memory,
        }))
    }

    fn position(reference: ObjectRef) -> (usize, u8) {
        let [high, low, ..] = (reference.0).0;
        let prefix = (usize::from(u16::from_be_bytes([high, low]))) >> (16 - PREFIX_BITS);
        (prefix / 8, 1u8 << (prefix % 8))
    }

    pub(super) fn charge_insert(budget: &mut WorkBudget) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            // Two digest bytes, then one byte read and written.
            bytes: 4,
            ..WorkUsage::default()
        })
    }

    pub(super) fn record(&mut self, reference: ObjectRef) {
        let (byte, bit) = Self::position(reference);
        self.bits[byte] |= bit;
    }

    pub(super) fn may_contain(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<bool, IndexError> {
        budget.charge(WorkUsage {
            // Two digest bytes and one byte of the table.
            bytes: 3,
            ..WorkUsage::default()
        })?;
        let (byte, bit) = Self::position(reference);
        Ok(self.bits[byte] & bit != 0)
    }
}
