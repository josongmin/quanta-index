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
const SOURCE_RECORD_RESERVE: u64 = 2 * 1024 * 1024;
const MAX_RETAINED_BYTES: u64 = 2 * SOURCE_RECORD_RESERVE;

struct Entry {
    object: StoredObject,
    bytes: u64,
    source_record: bool,
}

struct Window {
    slots: Vec<Option<Entry>>,
    retained: u64,
    general_retained: u64,
    // Backing and shared object DATA drop before their table reservation.
    memory: Option<MemoryLease>,
}

pub(crate) struct AttemptObjectReads<'a> {
    objects: &'a dyn ImmutableObjectPort,
    selected: Option<&'a StoredObject>,
    slot_limit: usize,
    byte_limit: u64,
    source_record_reserve: u64,
    window: Mutex<Window>,
}

impl<'a> AttemptObjectReads<'a> {
    pub(crate) fn for_source_attempt(
        objects: &'a dyn ImmutableObjectPort,
        slot_hint: u64,
    ) -> Result<Self, IndexError> {
        let mut reads = Self::new(objects, slot_hint)?;
        reads.source_record_reserve = SOURCE_RECORD_RESERVE;
        Ok(reads)
    }

    pub(crate) fn new(
        objects: &'a dyn ImmutableObjectPort,
        slot_hint: u64,
    ) -> Result<Self, IndexError> {
        // Small windows do not pay for a corpus-sized table. Two candidate
        // slots bound lookup work; collisions remain eviction, not trust.
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
            source_record_reserve: 0,
            window: Mutex::new(Window {
                slots: Vec::new(),
                retained: 0,
                general_retained: 0,
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
            source_record_reserve: 0,
            window: Mutex::new(Window {
                slots: Vec::new(),
                retained: 0,
                general_retained: 0,
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

    fn slots(&self, reference: ObjectRef) -> Result<[usize; 2], IndexError> {
        let digest = &reference.0.0;
        let prefix: [u8; 4] = digest
            .get(..4)
            .ok_or(IndexError::ResourceLimited)?
            .try_into()
            .map_err(|_width| IndexError::ResourceLimited)?;
        let alternate: [u8; 4] = digest
            .get(4..8)
            .ok_or(IndexError::ResourceLimited)?
            .try_into()
            .map_err(|_width| IndexError::ResourceLimited)?;
        let prefix = usize::try_from(u32::from_le_bytes(prefix))
            .map_err(|_width| IndexError::ResourceLimited)?;
        let alternate = usize::try_from(u32::from_le_bytes(alternate))
            .map_err(|_width| IndexError::ResourceLimited)?;
        let mask = self
            .slot_limit
            .checked_sub(1)
            .ok_or(IndexError::ResourceLimited)?;
        let primary = prefix & mask;
        // With a power-of-two table, the alternate is distinct unless there
        // is only one slot. This preserves the one-slot eviction contract.
        Ok([primary, primary ^ ((alternate & mask) | (mask & 1))])
    }

    fn retain(
        &self,
        object: &StoredObject,
        source_record: bool,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
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
        let reference = object.registration.reference;
        let candidates = self.slots(reference)?;
        let mut window = self.lock()?;
        let same = candidates.into_iter().find(|&slot| {
            window
                .slots
                .get(slot)
                .and_then(Option::as_ref)
                .is_some_and(|entry| entry.object.registration.reference == reference)
        });
        let vacant = candidates
            .into_iter()
            .find(|&slot| window.slots.get(slot).is_none_or(Option::is_none));
        let general_limit = self.byte_limit.saturating_sub(self.source_record_reserve);
        let mut admission = None;
        for slot in vacant.into_iter().chain(candidates) {
            if same.is_some_and(|existing| existing != slot) {
                continue;
            }
            let prior = window.slots.get(slot).and_then(Option::as_ref);
            if prior.is_some_and(|entry| entry.source_record) {
                // Keep the authenticated source records until their consumers
                // finish. A collision may cause a later physical read only.
                continue;
            }
            let prior_bytes = prior.map_or(0, |entry| entry.bytes);
            let retained = window
                .retained
                .checked_sub(prior_bytes)
                .and_then(|used| used.checked_add(bytes))
                .ok_or(IndexError::ResourceLimited)?;
            let general = window
                .general_retained
                .checked_sub(prior_bytes)
                .and_then(|used| used.checked_add(if source_record { 0 } else { bytes }))
                .ok_or(IndexError::ResourceLimited)?;
            let records = retained
                .checked_sub(general)
                .ok_or(IndexError::ResourceLimited)?;
            // A single large body can still use unused record capacity. The
            // overall DATA cap remains authoritative for both classes.
            if retained <= self.byte_limit
                && records <= self.source_record_reserve
                && (source_record || general <= general_limit || bytes > general_limit)
            {
                admission = Some((slot, retained, general));
                break;
            }
        }
        let Some((slot, retained, general)) = admission else {
            return Ok(());
        };
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
            source_record,
        });
        window.retained = retained;
        window.general_retained = general;
        drop(window);
        Ok(())
    }

    pub(crate) fn get_source_record(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        self.get_with_retention(reference, true, budget)
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
        self.get_with_retention(reference, false, budget)
    }

    fn read_retained(
        &self,
        object: &StoredObject,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        // The caller already owns this DATA within the protected attempt. Do
        // not make its reuse depend on whether the bounded lookup window kept
        // the slot. The selected port retains its original charge and refusal.
        budget.checkpoint()?;
        let retained = self.objects.read_retained(object, budget)?;
        if retained.registration.reference != object.registration.reference {
            return Err(IndexError::CorruptObject(
                "retained object reference mismatch".into(),
            ));
        }
        Ok(retained)
    }
}

impl AttemptObjectReads<'_> {
    fn get_with_retention(
        &self,
        reference: ObjectRef,
        source_record: bool,
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
        let candidates = self.slots(reference)?;
        let window = self.lock()?;
        let hit = candidates.into_iter().find_map(|slot| {
            window
                .slots
                .get(slot)
                .and_then(Option::as_ref)
                .filter(|entry| entry.object.registration.reference == reference)
                .map(|entry| (entry.object.clone(), entry.source_record))
        });
        drop(window);
        if let Some((object, already_source_record)) = hit {
            let object = self.objects.read_retained(&object, budget)?;
            if object.registration.reference != reference {
                return Err(IndexError::CorruptObject(
                    "retained object reference mismatch".into(),
                ));
            }
            if source_record && !already_source_record {
                self.retain(&object, true, budget)?;
            }
            return Ok(object);
        }
        let object = self.objects.get(reference, budget)?;
        if object.registration.reference != reference {
            return Err(IndexError::BackendViolation(
                "object read returned another identity".into(),
            ));
        }
        self.retain(&object, source_record, budget)?;
        Ok(object)
    }
}

#[cfg(test)]
mod tests;
