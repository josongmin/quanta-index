//! Bounded DATA reuse within one Source publication attempt, or one borrowed
//! artifact already read under a pinned materialization.
//!
//! The original object port authenticates every miss. Shared immutable bytes
//! retain their original allocation leases; this window grants no storage,
//! publication, FD or cross-window authority. The borrowed artifact adds no
//! table or lease and cannot outlive the selected pinned view.

use std::sync::{Mutex, MutexGuard, TryLockError};

use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, ObjectBatchInput, ObjectRef, RegisteredObject,
    RegisteredObjectBatch, StoredObject, WorkBudget,
    ports::{ImmutableObjectPort, ObjectWriteSink},
};

const MAX_SLOTS: u64 = 32_768;
const MAX_RETAINED_BYTES: u64 = 4 * 1024 * 1024;

struct Entry {
    object: StoredObject,
    bytes: u64,
}

struct Window {
    slots: Vec<Option<Entry>>,
    retained: u64,
    // Backing and shared object DATA drop before their table reservation.
    memory: Option<MemoryLease>,
}

pub(crate) struct AttemptObjectReads<'a> {
    objects: &'a dyn ImmutableObjectPort,
    selected: Option<&'a StoredObject>,
    slot_limit: usize,
    byte_limit: u64,
    window: Mutex<Window>,
}

impl<'a> AttemptObjectReads<'a> {
    pub(crate) fn new(
        objects: &'a dyn ImmutableObjectPort,
        slot_hint: u64,
    ) -> Result<Self, IndexError> {
        // Small windows do not pay for a corpus-sized table. Direct addressing
        // bounds lookup work and makes collisions ordinary eviction, not trust.
        let slots = slot_hint.saturating_mul(8).min(MAX_SLOTS);
        let slots = usize::try_from(slots).map_err(|_error| IndexError::ResourceLimited)?;
        Ok(Self::with_bounds(
            objects,
            if slots == 0 {
                0
            } else {
                slots
                    .checked_next_power_of_two()
                    .ok_or(IndexError::ResourceLimited)?
            },
            MAX_RETAINED_BYTES,
        ))
    }

    fn with_bounds(objects: &'a dyn ImmutableObjectPort, slots: usize, bytes: u64) -> Self {
        Self {
            objects,
            selected: None,
            slot_limit: slots,
            byte_limit: bytes,
            window: Mutex::new(Window {
                slots: Vec::new(),
                retained: 0,
                memory: None,
            }),
        }
    }

    pub(crate) fn selected(objects: &'a dyn ImmutableObjectPort, object: &'a StoredObject) -> Self {
        Self {
            objects,
            selected: Some(object),
            slot_limit: 0,
            byte_limit: 0,
            window: Mutex::new(Window {
                slots: Vec::new(),
                retained: 0,
                memory: None,
            }),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, Window>, IndexError> {
        self.window.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => IndexError::ResourceLimited,
            TryLockError::Poisoned(_) => {
                IndexError::BackendViolation("attempt object DATA window poisoned".into())
            }
        })
    }

    fn slot(&self, reference: ObjectRef) -> Result<usize, IndexError> {
        let prefix: [u8; 4] = reference
            .0
            .0
            .get(..4)
            .ok_or(IndexError::ResourceLimited)?
            .try_into()
            .map_err(|_width| IndexError::ResourceLimited)?;
        let prefix = usize::try_from(u32::from_le_bytes(prefix))
            .map_err(|_width| IndexError::ResourceLimited)?;
        let mask = self
            .slot_limit
            .checked_sub(1)
            .ok_or(IndexError::ResourceLimited)?;
        Ok(prefix & mask)
    }

    fn retain(&self, object: &StoredObject, budget: &WorkBudget) -> Result<(), IndexError> {
        let bytes = object.memory_bytes().map_or_else(
            || {
                StoredObject::allocation_bytes(
                    object.payload.len(),
                    object.registration.children.len(),
                )
            },
            Ok,
        )?;
        if bytes > self.byte_limit {
            return Ok(());
        }
        let slot = self.slot(object.registration.reference)?;
        let mut window = self.lock()?;
        let old_bytes = window
            .slots
            .get(slot)
            .and_then(Option::as_ref)
            .map_or(0, |entry| entry.bytes);
        let retained = window
            .retained
            .checked_sub(old_bytes)
            .and_then(|used| used.checked_add(bytes))
            .ok_or(IndexError::ResourceLimited)?;
        if retained > self.byte_limit {
            return Ok(());
        }
        if window.slots.is_empty() {
            let bytes = self
                .slot_limit
                .checked_mul(std::mem::size_of::<Option<Entry>>())
                .ok_or(IndexError::ResourceLimited)?;
            let memory = budget.reserve_memory(
                u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?,
                MemoryLifetime::Temporary,
            )?;
            let mut slots = Vec::new();
            slots
                .try_reserve_exact(self.slot_limit)
                .map_err(|_allocation| IndexError::ResourceLimited)?;
            slots.resize_with(self.slot_limit, || None);
            window.slots = slots;
            window.memory = memory;
        }
        *window
            .slots
            .get_mut(slot)
            .ok_or(IndexError::ResourceLimited)? = Some(Entry {
            object: object.clone(),
            bytes,
        });
        window.retained = retained;
        drop(window);
        Ok(())
    }
}

impl ImmutableObjectPort for AttemptObjectReads<'_> {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.objects.identify(payload, children, budget)
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.objects.put(payload, children, budget)
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        self.objects.install(payload, children, sink, budget)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        self.objects.install_batch(inputs, sink, budget)
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        if let Some(selected) = self.selected {
            if selected.registration.reference != reference {
                return self.objects.get(reference, budget);
            }
            budget.checkpoint()?;
            let object = self.objects.read_retained(selected, budget)?;
            if object.registration.reference != reference {
                return Err(IndexError::CorruptObject(
                    "retained object reference mismatch".into(),
                ));
            }
            return Ok(object);
        }
        if self.slot_limit == 0 {
            return self.objects.get(reference, budget);
        }
        budget.checkpoint()?;
        let hit = self
            .lock()?
            .slots
            .get(self.slot(reference)?)
            .and_then(Option::as_ref)
            .filter(|entry| entry.object.registration.reference == reference)
            .map(|entry| entry.object.clone());
        if let Some(object) = hit {
            let object = self.objects.read_retained(&object, budget)?;
            if object.registration.reference != reference {
                return Err(IndexError::CorruptObject(
                    "retained object reference mismatch".into(),
                ));
            }
            return Ok(object);
        }
        let object = self.objects.get(reference, budget)?;
        if object.registration.reference != reference {
            return Err(IndexError::BackendViolation(
                "object read returned another identity".into(),
            ));
        }
        self.retain(&object, budget)?;
        Ok(object)
    }
}

#[cfg(test)]
mod tests;
