//! Work and memory custody shared by repository-key decoding and lookup.

use quanta_index_contract::{
    NativeNormalizationAdmissionV1, NativeNormalizationScratchDemandV1,
    NativeNormalizationScratchOwnerV1,
};

use super::{IndexError, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage};

pub(super) struct RepositoryKeyAdmission<'a> {
    budget: &'a mut WorkBudget,
    leases: [Option<MemoryLease>; 3],
    sizes: [usize; 3],
}

impl<'a> RepositoryKeyAdmission<'a> {
    pub(super) fn new(budget: &'a mut WorkBudget) -> Self {
        Self {
            budget,
            leases: [None, None, None],
            sizes: [0; 3],
        }
    }

    const fn slot(owner: NativeNormalizationScratchOwnerV1) -> usize {
        match owner {
            NativeNormalizationScratchOwnerV1::Decomposition => 0,
            NativeNormalizationScratchOwnerV1::Recomposition => 1,
            NativeNormalizationScratchOwnerV1::Sort => 2,
        }
    }
}

impl NativeNormalizationAdmissionV1 for RepositoryKeyAdmission<'_> {
    type Error = IndexError;

    fn checkpoint_work_v1(&mut self, units: u64) -> Result<(), IndexError> {
        self.budget.charge(WorkUsage {
            bytes: units,
            ..WorkUsage::default()
        })
    }

    fn native_birth_v1(
        &mut self,
        demand: NativeNormalizationScratchDemandV1,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, IndexError> {
        let slot = Self::slot(demand.owner_v1);
        if self.sizes[slot] != demand.current_bytes_v1
            || self.leases[slot].as_ref().is_some_and(|lease| {
                usize::try_from(lease.bytes()).ok() != Some(demand.current_bytes_v1)
            })
            || demand.new_bytes_v1 <= demand.current_bytes_v1
        {
            return Err(IndexError::BackendViolation(
                "repository key NFC scratch growth differs from its live grant".into(),
            ));
        }
        self.budget.charge(WorkUsage {
            bytes: u64::try_from(demand.new_bytes_v1)
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let replacement = self.budget.reserve_memory(
            u64::try_from(demand.new_bytes_v1).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let born = birth();
        if born {
            // The old grant remains live until the replacement birth completes.
            self.leases[slot] = replacement;
            self.sizes[slot] = demand.new_bytes_v1;
        }
        Ok(born)
    }

    fn release_scratch_v1(&mut self, owner: NativeNormalizationScratchOwnerV1) {
        let slot = Self::slot(owner);
        self.leases[slot] = None;
        self.sizes[slot] = 0;
    }
}
