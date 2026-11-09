//! An authenticated occurrence block can also be one owner's complete inventory.
//! The block remains childless; its owning contribution supplies the physical edge.

use quanta_index_core::index::{
    IndexError, MapPage, MemoryLease, MemoryLifetime, ObjectRef, WorkBudget, WorkUsage,
    ports::ImmutableObjectPort,
};

use crate::native::text_occurrence::block::CheckedBlock;

fn corrupt(reason: &str) -> IndexError {
    IndexError::CorruptObject(reason.into())
}

fn read(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    budget: &mut WorkBudget,
) -> Result<CheckedBlock, IndexError> {
    budget.checkpoint()?;
    let object = objects.get(reference, budget)?;
    if object.registration.reference != reference || !object.registration.children.is_empty() {
        return Err(corrupt("occurrence inventory identity or edges differ"));
    }
    CheckedBlock::parse(object, budget)
}

fn page(
    checked: &CheckedBlock,
    reference: ObjectRef,
    ordinal: usize,
    limit: usize,
    budget: &mut WorkBudget,
) -> Result<MapPage, IndexError> {
    budget.checkpoint()?;
    let count = checked.term_count();
    let remaining = count.checked_sub(ordinal).ok_or_else(|| {
        IndexError::BackendViolation("occurrence inventory cursor passed final row".into())
    })?;
    let capacity = remaining.min(limit);
    let allocation = budget.reserve_memory(
        MapPage::allocation_bytes(capacity)?,
        MemoryLifetime::Retained,
    )?;
    let mut leases: Vec<MemoryLease> = Vec::new();
    leases
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(capacity)
        .map_err(|_error| IndexError::ResourceLimited)?;
    for slot in ordinal
        ..ordinal
            .checked_add(capacity)
            .ok_or(IndexError::ResourceLimited)?
    {
        budget.checkpoint()?;
        let key = checked.key(slot)?;
        let bytes = u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        let memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(bytes, MemoryLifetime::Retained)?
        };
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(key.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        owned.extend_from_slice(key);
        entries.push((owned, reference.into()));
        leases.extend(memory);
    }
    let end = ordinal
        .checked_add(capacity)
        .ok_or(IndexError::ResourceLimited)?;
    Ok(MapPage::new(entries, end < count, leases, allocation))
}

pub(super) fn lookup(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    key: &[u8],
    budget: &mut WorkBudget,
) -> Result<Option<ObjectRef>, IndexError> {
    budget.checkpoint()?;
    budget.charge(WorkUsage {
        bytes: u64::try_from(key.len()).map_err(|_error| IndexError::ResourceLimited)?,
        ..WorkUsage::default()
    })?;
    let checked = read(objects, reference, budget)?;
    let ordinal = checked.lower_bound(key, budget)?;
    Ok((ordinal < checked.term_count() && checked.key(ordinal)? == key).then_some(reference))
}

pub(super) fn scan(
    objects: &dyn ImmutableObjectPort,
    reference: ObjectRef,
    after: Option<&[u8]>,
    limit: u32,
    budget: &mut WorkBudget,
) -> Result<MapPage, IndexError> {
    budget.checkpoint()?;
    if limit == 0 || limit > 4096 {
        return Err(IndexError::InvalidInput(
            "entry scan limit exceeds bound".into(),
        ));
    }
    let checked = read(objects, reference, budget)?;
    let ordinal = if let Some(after) = after {
        let lower = checked.lower_bound(after, budget)?;
        if lower < checked.term_count() && checked.key(lower)? == after {
            lower.checked_add(1).ok_or(IndexError::ResourceLimited)?
        } else {
            lower
        }
    } else {
        0
    };
    page(
        &checked,
        reference,
        ordinal,
        usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?,
        budget,
    )
}

/// One validated block is retained across bounded inventory pages. A failed
/// retained read, charge or allocation never advances the cursor.
pub(in crate::native::text) struct PackedInventoryCursor<'a> {
    objects: &'a dyn ImmutableObjectPort,
    reference: ObjectRef,
    checked: CheckedBlock,
    ordinal: usize,
    next_count_ordinal: usize,
    count_verified: bool,
}

impl<'a> PackedInventoryCursor<'a> {
    pub(in crate::native::text) fn open(
        objects: &'a dyn ImmutableObjectPort,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<Self, IndexError> {
        let checked = read(objects, reference, budget)?;
        Ok(Self {
            objects,
            reference,
            checked,
            ordinal: 0,
            next_count_ordinal: 0,
            count_verified: false,
        })
    }

    pub(in crate::native::text) fn reference(&self) -> ObjectRef {
        self.reference
    }

    pub(in crate::native::text) fn count_verified(&self) -> bool {
        self.count_verified
    }

    pub(in crate::native::text) fn into_validated_block(self) -> Option<CheckedBlock> {
        self.count_verified.then_some(self.checked)
    }

    /// Reuse this Diff cursor's authenticated DATA for an exact block member.
    /// The first count still crosses the adapter's logical read and refusal
    /// boundary; later counts share only this cursor's bounded lifetime.
    pub(in crate::native::text) fn validated_count(
        &mut self,
        key: &[u8],
        budget: &mut WorkBudget,
    ) -> Result<u64, IndexError> {
        budget.checkpoint()?;
        if !self.count_verified {
            let original = self.checked.object();
            let retained = self.objects.read_retained(original, budget)?;
            budget.checkpoint()?;
            if retained != *original {
                return Err(corrupt("retained occurrence count data changed"));
            }
        }
        let (count, ordinal) =
            match self
                .checked
                .position_count_if_at(self.next_count_ordinal, key, budget)?
            {
                Some(count) => (count, self.next_count_ordinal),
                None => self.checked.position_count_with_ordinal(key, budget)?,
            };
        self.next_count_ordinal = ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        self.count_verified = true;
        Ok(count)
    }

    pub(in crate::native::text) fn next_page(
        &mut self,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<MapPage, IndexError> {
        budget.checkpoint()?;
        if limit == 0 || limit > 64 {
            return Err(IndexError::InvalidInput(
                "occurrence inventory cursor page exceeds bound".into(),
            ));
        }
        if self.ordinal != 0 {
            let original = self.checked.object();
            let retained = self.objects.read_retained(original, budget)?;
            if !std::ptr::eq(&*retained, &**original) && retained != *original {
                return Err(corrupt("retained occurrence inventory data changed"));
            }
        }
        let result = page(
            &self.checked,
            self.reference,
            self.ordinal,
            usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?,
            budget,
        )?;
        self.ordinal = self
            .ordinal
            .checked_add(result.entries.len())
            .ok_or(IndexError::ResourceLimited)?;
        Ok(result)
    }
}
