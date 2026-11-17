//! Bounded batch reference tables and admitted internal registration copies.

use quanta_index_core::index::{
    Digest, IndexError, MemoryLease, MemoryLifetime, ObjectRef, RegisteredObject, WorkBudget,
    WorkUsage,
};
use std::path::{Path, PathBuf};

use super::{MAX_BATCH_INPUTS, MAX_INSTALL_BATCH_INPUTS, PreparedObject};

/// Sorted digest-to-ordinal lookup without allocator-owned tree nodes.
pub(super) struct BatchIndex {
    entries: [(ObjectRef, usize); MAX_BATCH_INPUTS],
    count: usize,
}

impl BatchIndex {
    pub(super) fn new() -> Self {
        Self {
            entries: [(ObjectRef(Digest([0; 32])), 0); MAX_BATCH_INPUTS],
            count: 0,
        }
    }

    pub(super) fn lookup(&self, reference: ObjectRef) -> Result<Option<usize>, IndexError> {
        let entries = self
            .entries
            .get(..self.count)
            .ok_or_else(|| IndexError::BackendViolation("batch index bounds invalid".into()))?;
        match entries.binary_search_by_key(&reference, |entry| entry.0) {
            Ok(slot) => entries
                .get(slot)
                .map(|entry| Some(entry.1))
                .ok_or_else(|| IndexError::BackendViolation("batch index slot missing".into())),
            Err(_insertion) => Ok(None),
        }
    }

    pub(super) fn insert(
        &mut self,
        reference: ObjectRef,
        ordinal: usize,
    ) -> Result<(), IndexError> {
        if self.count == MAX_BATCH_INPUTS || ordinal >= MAX_BATCH_INPUTS {
            return Err(IndexError::ResourceLimited);
        }
        let entries = self
            .entries
            .get(..self.count)
            .ok_or_else(|| IndexError::BackendViolation("batch index bounds invalid".into()))?;
        let position = entries
            .binary_search_by_key(&reference, |entry| entry.0)
            .err()
            .ok_or_else(|| IndexError::BackendViolation("batch digest inserted twice".into()))?;
        let count = self
            .count
            .checked_add(1)
            .ok_or(IndexError::ResourceLimited)?;
        let entries = self.entries.get_mut(..count).ok_or_else(|| {
            IndexError::BackendViolation("batch index growth bounds invalid".into())
        })?;
        let destination = position.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        entries.copy_within(position..self.count, destination);
        *entries.get_mut(position).ok_or_else(|| {
            IndexError::BackendViolation("batch index insertion slot missing".into())
        })? = (reference, ordinal);
        self.count = count;
        Ok(())
    }
}

pub(super) struct BatchReferences {
    sorted: [ObjectRef; MAX_INSTALL_BATCH_INPUTS],
    count: usize,
}

impl BatchReferences {
    pub(super) fn new(
        objects: &[&PreparedObject],
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        if objects.len() > MAX_INSTALL_BATCH_INPUTS {
            return Err(IndexError::ResourceLimited);
        }
        let mut table = Self {
            sorted: [ObjectRef(Digest([0; 32])); MAX_INSTALL_BATCH_INPUTS],
            count: objects.len(),
        };
        for (slot, object) in table.sorted.iter_mut().zip(objects) {
            *slot = object.registration.reference;
        }
        let sorted = table.sorted.get_mut(..table.count).ok_or_else(|| {
            IndexError::BackendViolation("batch reference table bounds invalid".into())
        })?;
        sorted.sort_unstable();
        if sorted
            .iter()
            .zip(sorted.iter().skip(1))
            .any(|(left, right)| left == right)
        {
            return Err(IndexError::BackendViolation(
                "duplicate object in staged batch".into(),
            ));
        }
        let mut seen = [false; MAX_INSTALL_BATCH_INPUTS];
        for object in objects {
            budget.checkpoint()?;
            for child in &object.registration.children {
                if let Ok(slot) = sorted.binary_search(child) {
                    let visited = seen.get(slot).ok_or_else(|| {
                        IndexError::BackendViolation("batch child slot bounds invalid".into())
                    })?;
                    if !visited {
                        return Err(IndexError::BackendViolation(
                            "staged batch is not child-first".into(),
                        ));
                    }
                }
            }
            let slot = sorted
                .binary_search(&object.registration.reference)
                .map_err(|_error| {
                    IndexError::BackendViolation("batch object missing from reference table".into())
                })?;
            *seen.get_mut(slot).ok_or_else(|| {
                IndexError::BackendViolation("batch object slot bounds invalid".into())
            })? = true;
        }
        Ok(table)
    }

    pub(super) fn contains(&self, reference: ObjectRef) -> Result<bool, IndexError> {
        let sorted = self.sorted.get(..self.count).ok_or_else(|| {
            IndexError::BackendViolation("batch reference table bounds invalid".into())
        })?;
        Ok(sorted.binary_search(&reference).is_ok())
    }
}

pub(super) struct RegistrationBatch {
    registrations: Vec<RegisteredObject>,
    // All metadata and copied child backing drops before its one grant.
    _memory: Option<MemoryLease>,
}

impl RegistrationBatch {
    pub(super) fn try_capacity(
        count: usize,
        budget: &WorkBudget,
    ) -> Result<Option<Self>, IndexError> {
        if count == 0 || count > MAX_INSTALL_BATCH_INPUTS {
            return Err(IndexError::ResourceLimited);
        }
        let bytes = count
            .checked_mul(std::mem::size_of::<RegisteredObject>())
            .ok_or(IndexError::ResourceLimited)?;
        let Some(memory) = budget.try_reserve_memory_for_optimization(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(None);
        };
        let mut registrations = Vec::new();
        if registrations.try_reserve_exact(count).is_err() || registrations.capacity() > count {
            return Ok(None);
        }
        Ok(Some(Self {
            registrations,
            _memory: Some(memory),
        }))
    }

    pub(super) fn push_prepared(&mut self, object: RegisteredObject) -> Result<(), IndexError> {
        if self.registrations.len() == self.registrations.capacity()
            || self.registrations.len() == MAX_INSTALL_BATCH_INPUTS
        {
            return Err(IndexError::BackendViolation(
                "prepared registration capacity exhausted".into(),
            ));
        }
        self.registrations.push(object);
        Ok(())
    }

    pub(super) fn allocation_bytes(objects: &[&PreparedObject]) -> Result<u64, IndexError> {
        Self::allocation_bytes_for_count(objects.len())
    }

    pub(super) fn allocation_bytes_for_count(count: usize) -> Result<u64, IndexError> {
        if count > MAX_INSTALL_BATCH_INPUTS {
            return Err(IndexError::ResourceLimited);
        }
        let backing = count
            .checked_mul(std::mem::size_of::<RegisteredObject>())
            .ok_or(IndexError::ResourceLimited)?;
        u64::try_from(backing).map_err(|_error| IndexError::ResourceLimited)
    }

    pub(super) fn new(
        objects: &[&PreparedObject],
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        let bytes = Self::allocation_bytes(objects)?;
        let memory = if bytes == 0 {
            None
        } else {
            budget.reserve_memory(bytes, MemoryLifetime::Temporary)?
        };
        let mut registrations = Vec::new();
        registrations
            .try_reserve_exact(objects.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        for object in objects {
            budget.checkpoint()?;
            registrations.push(object.registration.clone());
        }
        budget.checkpoint()?;
        Ok(Self {
            registrations,
            _memory: memory,
        })
    }

    pub(super) fn as_slice(&self) -> &[RegisteredObject] {
        &self.registrations
    }
}

pub(super) enum ChildValidationSlot {
    Validated,
    Untracked,
    Vacant(usize),
}

/// Full-reference witnesses from successful path checks in one physical
/// install group. The caller must hold the governing gate for each entry.
pub(super) struct ValidatedExternalChildren<'a> {
    slots: Vec<Option<&'a ObjectRef>>,
    count: usize,
    limit: usize,
    _memory: MemoryLease,
}

impl<'a> ValidatedExternalChildren<'a> {
    const MAX_VALIDATED: usize = 2_048;
    const SLOT_BYTES: u64 = std::mem::size_of::<Option<&ObjectRef>>() as u64;

    pub(super) fn try_new(
        children: usize,
        budget: &mut WorkBudget,
    ) -> Result<Option<Self>, IndexError> {
        if children < 2 {
            return Ok(None);
        }
        let limit = children.min(Self::MAX_VALIDATED);
        let slots = (limit * 2).next_power_of_two();
        let bytes =
            u64::try_from(slots).map_err(|_| IndexError::ResourceLimited)? * Self::SLOT_BYTES;
        let Some(memory) =
            budget.try_reserve_memory_for_optimization(bytes, MemoryLifetime::Temporary)?
        else {
            return Ok(None);
        };
        let mut references = Vec::new();
        if references.try_reserve_exact(slots).is_err() || references.capacity() > slots {
            return Ok(None);
        }
        budget.charge(WorkUsage {
            bytes,
            ..WorkUsage::default()
        })?;
        references.resize(slots, None);
        Ok(Some(Self {
            slots: references,
            count: 0,
            limit,
            _memory: memory,
        }))
    }

    pub(super) fn locate(
        &self,
        reference: &ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<ChildValidationSlot, IndexError> {
        budget.charge(WorkUsage {
            bytes: 2,
            ..WorkUsage::default()
        })?;
        let [_, low, high, ..] = (reference.0).0;
        let mask = self.slots.len() - 1;
        let mut index = usize::from(u16::from_le_bytes([low, high])) & mask;
        for _ in &self.slots {
            budget.charge(WorkUsage {
                bytes: Self::SLOT_BYTES,
                ..WorkUsage::default()
            })?;
            let Some(known) = self.slots[index] else {
                return Ok(if self.count < self.limit {
                    ChildValidationSlot::Vacant(index)
                } else {
                    ChildValidationSlot::Untracked
                });
            };
            budget.charge(WorkUsage {
                bytes: 32,
                ..WorkUsage::default()
            })?;
            if known == reference {
                return Ok(ChildValidationSlot::Validated);
            }
            index = (index + 1) & mask;
        }
        Err(IndexError::BackendViolation(
            "validated child table lost its vacant slot".into(),
        ))
    }

    pub(super) fn remember(
        &mut self,
        index: usize,
        reference: &'a ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        budget.charge(WorkUsage {
            bytes: Self::SLOT_BYTES,
            ..WorkUsage::default()
        })?;
        let slot = self
            .slots
            .get_mut(index)
            .filter(|slot| slot.is_none())
            .ok_or_else(|| {
                IndexError::BackendViolation("validated child slot is not vacant".into())
            })?;
        *slot = Some(reference);
        self.count += 1;
        Ok(())
    }
}

/// Exact shard directories for a bounded install. All path backing is funded
/// before the caller reserves custody or writes any immutable object.
pub(super) struct BatchShards {
    paths: [Option<PathBuf>; MAX_INSTALL_BATCH_INPUTS],
    prefixes: [u8; MAX_INSTALL_BATCH_INPUTS],
    count: usize,
    // Path backing drops before this one temporary allocation grant.
    _memory: Option<MemoryLease>,
}

impl BatchShards {
    pub(super) fn allocation_bytes_for_count(
        objects: &Path,
        count: usize,
    ) -> Result<u64, IndexError> {
        let path_bytes = objects
            .as_os_str()
            .as_encoded_bytes()
            .len()
            .checked_add(3)
            .ok_or(IndexError::ResourceLimited)?;
        let allocation = path_bytes
            .checked_mul(count)
            .ok_or(IndexError::ResourceLimited)?;
        u64::try_from(allocation).map_err(|_error| IndexError::ResourceLimited)
    }

    /// Preallocate every possible shard pathname before staging an optional
    /// leaf run. Restriction later only drops paths; it cannot allocate.
    pub(super) fn try_all(objects: &Path, budget: &WorkBudget) -> Result<Option<Self>, IndexError> {
        let path_bytes = objects
            .as_os_str()
            .as_encoded_bytes()
            .len()
            .checked_add(3)
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = path_bytes
            .checked_mul(256)
            .ok_or(IndexError::ResourceLimited)?;
        let Some(memory) = budget.try_reserve_memory_for_optimization(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
        else {
            return Ok(None);
        };
        let mut paths = std::array::from_fn(|_| None);
        let mut prefixes = [0; MAX_INSTALL_BATCH_INPUTS];
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for (index, slot) in paths.iter_mut().enumerate() {
            budget.checkpoint()?;
            let first = u8::try_from(index).map_err(|_error| IndexError::ResourceLimited)?;
            let hex = [
                *HEX.get(index >> 4).ok_or(IndexError::ResourceLimited)?,
                *HEX.get(index & 15).ok_or(IndexError::ResourceLimited)?,
            ];
            let shard = std::str::from_utf8(&hex).map_err(|_error| IndexError::ResourceLimited)?;
            let mut path = PathBuf::new();
            if path.try_reserve_exact(path_bytes).is_err() || path.capacity() > path_bytes {
                return Ok(None);
            }
            path.push(objects);
            path.push(shard);
            *slot = Some(path);
            *prefixes.get_mut(index).ok_or(IndexError::ResourceLimited)? = first;
        }
        Ok(Some(Self {
            paths,
            prefixes,
            count: 256,
            _memory: Some(memory),
        }))
    }

    pub(super) fn restrict_to(
        &mut self,
        registrations: &[RegisteredObject],
    ) -> Result<(), IndexError> {
        let mut used = [false; 256];
        for registration in registrations {
            let first = (registration.reference.0)
                .0
                .first()
                .copied()
                .ok_or(IndexError::ResourceLimited)?;
            *used
                .get_mut(usize::from(first))
                .ok_or(IndexError::ResourceLimited)? = true;
        }
        for (prefix, path) in self.prefixes.iter().zip(&mut self.paths).take(self.count) {
            if !used
                .get(usize::from(*prefix))
                .copied()
                .ok_or(IndexError::ResourceLimited)?
            {
                *path = None;
            }
        }
        Ok(())
    }

    pub(super) fn new(
        objects: &Path,
        registrations: &[RegisteredObject],
        budget: &WorkBudget,
    ) -> Result<Self, IndexError> {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        if registrations.len() > MAX_INSTALL_BATCH_INPUTS {
            return Err(IndexError::ResourceLimited);
        }
        let mut present = [false; 256];
        for registration in registrations {
            budget.checkpoint()?;
            let first = (registration.reference.0)
                .0
                .first()
                .copied()
                .ok_or_else(|| IndexError::BackendViolation("empty object digest".into()))?;
            *present
                .get_mut(usize::from(first))
                .ok_or(IndexError::ResourceLimited)? = true;
        }
        let count = present.iter().filter(|used| **used).count();
        let path_bytes = objects
            .as_os_str()
            .as_encoded_bytes()
            .len()
            .checked_add(3)
            .ok_or(IndexError::ResourceLimited)?;
        let allocation = Self::allocation_bytes_for_count(objects, count)?;
        let memory = if allocation == 0 {
            None
        } else {
            budget.reserve_memory(allocation, MemoryLifetime::Temporary)?
        };
        let mut paths = std::array::from_fn(|_| None);
        let mut prefixes = [0; MAX_INSTALL_BATCH_INPUTS];
        let mut filled = 0usize;
        for (first, used) in present.into_iter().enumerate() {
            if !used {
                continue;
            }
            budget.checkpoint()?;
            let byte = u8::try_from(first).map_err(|_error| IndexError::ResourceLimited)?;
            let hex = [
                *HEX.get(usize::from(byte >> 4))
                    .ok_or(IndexError::ResourceLimited)?,
                *HEX.get(usize::from(byte & 0x0f))
                    .ok_or(IndexError::ResourceLimited)?,
            ];
            let shard = std::str::from_utf8(&hex).map_err(|_error| {
                IndexError::BackendViolation("invalid object shard encoding".into())
            })?;
            let mut path = PathBuf::new();
            path.try_reserve_exact(path_bytes)
                .map_err(|_error| IndexError::ResourceLimited)?;
            if path.capacity() > path_bytes {
                return Err(IndexError::ResourceLimited);
            }
            path.push(objects);
            path.push(shard);
            *paths.get_mut(filled).ok_or(IndexError::ResourceLimited)? = Some(path);
            *prefixes
                .get_mut(filled)
                .ok_or(IndexError::ResourceLimited)? = byte;
            filled = filled.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        if filled != count {
            return Err(IndexError::BackendViolation(
                "batch shard count changed during preparation".into(),
            ));
        }
        Ok(Self {
            paths,
            prefixes,
            count,
            _memory: memory,
        })
    }

    #[cfg(test)]
    pub(super) fn iter(&self) -> impl Iterator<Item = &Path> {
        self.paths
            .iter()
            .take(self.count)
            .filter_map(Option::as_deref)
    }

    pub(super) fn indexed_iter(&self) -> impl Iterator<Item = (usize, &Path)> {
        self.prefixes
            .iter()
            .zip(self.paths.iter())
            .take(self.count)
            .filter_map(|(prefix, path)| path.as_deref().map(|path| (usize::from(*prefix), path)))
    }
}
