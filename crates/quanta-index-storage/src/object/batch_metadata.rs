//! Bounded batch reference tables and admitted internal registration copies.

use quanta_index_core::index::{
    Digest, IndexError, MemoryLease, MemoryLifetime, ObjectRef, RegisteredObject, WorkBudget,
};

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
    pub(super) fn allocation_bytes(objects: &[&PreparedObject]) -> Result<u64, IndexError> {
        if objects.len() > MAX_INSTALL_BATCH_INPUTS {
            return Err(IndexError::ResourceLimited);
        }
        let backing = objects
            .len()
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
