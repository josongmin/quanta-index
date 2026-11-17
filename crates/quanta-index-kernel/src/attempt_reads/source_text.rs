//! Attempt-local validation state for next source bodies.

use quanta_index_core::index::{IndexError, MemoryLease, MemoryLifetime, ObjectRef, WorkBudget};

const MAX_CLAIMS: u64 = 32_768;

pub(super) struct SourceTextChecks {
    claims: Vec<Claim>,
    limit: usize,
    // Drop the table before releasing its allocation credit.
    _memory: MemoryLease,
}

struct Claim {
    key: (ObjectRef, u64),
    verified: bool,
}

impl SourceTextChecks {
    pub(super) fn try_new(
        owner_count: u64,
        budget: &WorkBudget,
    ) -> Result<Option<Self>, IndexError> {
        let limit = usize::try_from(owner_count.min(MAX_CLAIMS))
            .map_err(|_width| IndexError::ResourceLimited)?;
        if limit == 0 {
            return Ok(None);
        }
        let bytes = limit
            .checked_mul(std::mem::size_of::<Claim>())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?;
        let Some(memory) =
            budget.try_reserve_memory_for_optimization(bytes, MemoryLifetime::Temporary)?
        else {
            return Ok(None);
        };
        let mut claims = Vec::new();
        if claims.try_reserve_exact(limit).is_err() {
            return Ok(None);
        }
        Ok(Some(Self {
            claims,
            limit,
            _memory: memory,
        }))
    }

    pub(super) fn defer(&mut self, reference: ObjectRef, byte_len: u64) -> bool {
        if self.claims.len() == self.limit {
            return false;
        }
        self.claims.push(Claim {
            key: (reference, byte_len),
            verified: false,
        });
        true
    }

    pub(super) fn finish_collection(&mut self) {
        self.claims.sort_unstable_by_key(|claim| claim.key);
        self.claims.dedup_by_key(|claim| claim.key);
    }

    pub(super) fn observe(&mut self, reference: ObjectRef, byte_len: u64) {
        if let Ok(slot) = self
            .claims
            .binary_search_by_key(&(reference, byte_len), |claim| claim.key)
            && let Some(claim) = self.claims.get_mut(slot)
        {
            claim.verified = true;
        }
    }

    pub(super) fn remaining(&self) -> impl Iterator<Item = (ObjectRef, u64)> + '_ {
        self.claims
            .iter()
            .filter(|claim| !claim.verified)
            .map(|claim| claim.key)
    }
}
