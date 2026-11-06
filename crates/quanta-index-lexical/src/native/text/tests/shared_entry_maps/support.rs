//! Test-only readable immutable objects with the documented physical identity.
//! The reference model for map rows lives separately in `ModelMaps`.

use super::super::*;
use quanta_index_core::index::RegisteredObjectBatch;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum BatchFault {
    Short,
    WrongLast,
}

#[derive(Default)]
pub(super) struct CanonicalObjects {
    stored: Mutex<BTreeMap<ObjectRef, StoredObject>>,
    fault: Mutex<Option<BatchFault>>,
    pub(super) batches: AtomicUsize,
}

impl CanonicalObjects {
    pub(super) fn fault_next_batch(&self, fault: BatchFault) -> Result<(), IndexError> {
        *self
            .fault
            .lock()
            .map_err(|_error| IndexError::Storage("object fault lock".into()))? = Some(fault);
        Ok(())
    }

    /// Put malformed bytes under their own genuine immutable identity. This
    /// models an adversarial but content-addressed object, not a forged hash.
    pub(super) fn insert_wire(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<ObjectRef, IndexError> {
        let registration = self.identify(payload, children, budget)?;
        let reference = registration.reference;
        let _previous = self
            .stored
            .lock()
            .map_err(|_error| IndexError::Storage("object store lock".into()))?
            .insert(
                reference,
                StoredObject::new(registration, payload.to_vec(), None),
            );
        Ok(reference)
    }
}

impl ImmutableObjectPort for CanonicalObjects {
    fn identify(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        // Fixed physical-object format, independently specified by the storage
        // contract: domain || QIOBJ001 || BE32 payload || BE32 children ||
        // ordered child digests || payload. Block goldens below are handwritten.
        let payload_len =
            u32::try_from(payload.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let child_count =
            u32::try_from(children.len()).map_err(|_error| IndexError::ResourceLimited)?;
        let wire_len = 16_u64
            .checked_add(u64::from(payload_len))
            .and_then(|bytes| {
                u64::from(child_count)
                    .checked_mul(32)
                    .and_then(|children| bytes.checked_add(children))
            })
            .ok_or(IndexError::ResourceLimited)?;
        budget.charge(WorkUsage {
            bytes: wire_len,
            ..WorkUsage::default()
        })?;
        let mut hash = Sha256::new();
        hash.update(b"quanta-index-immutable-object-v1\0");
        hash.update(b"QIOBJ001");
        hash.update(payload_len.to_be_bytes());
        hash.update(child_count.to_be_bytes());
        for reference in children {
            hash.update(reference.0.0);
        }
        hash.update(payload);
        budget.checkpoint()?;
        RegisteredObject::copy_children(
            ObjectRef(Digest(hash.finalize().into())),
            wire_len,
            children,
            budget,
        )
    }

    fn put(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        let reference = self.insert_wire(payload, children, budget)?;
        self.get(reference, budget)
            .map(|stored| stored.registration.clone())
    }

    fn install(
        &self,
        payload: &[u8],
        children: &[ObjectRef],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObject, IndexError> {
        let registration = self.identify(payload, children, budget)?;
        sink.reserve(&registration)?;
        let _previous = self
            .stored
            .lock()
            .map_err(|_error| IndexError::Storage("object store lock".into()))?
            .insert(
                registration.reference,
                StoredObject::new(registration.clone(), payload.to_vec(), None),
            );
        sink.installed(&registration)?;
        Ok(registration)
    }

    fn install_batch(
        &self,
        inputs: &[ObjectBatchInput],
        sink: &dyn ObjectWriteSink,
        budget: &mut WorkBudget,
    ) -> Result<RegisteredObjectBatch, IndexError> {
        let _previous = self.batches.fetch_add(1, Ordering::Relaxed);
        let mut registrations = Vec::with_capacity(inputs.len());
        for input in inputs {
            registrations.push(self.identify(&input.payload, &input.children, budget)?);
        }
        sink.reserve_batch(&registrations)?;
        {
            let mut stored = self
                .stored
                .lock()
                .map_err(|_error| IndexError::Storage("object store lock".into()))?;
            for (input, registration) in inputs.iter().zip(&registrations) {
                let _previous = stored.insert(
                    registration.reference,
                    StoredObject::new(registration.clone(), input.payload.clone(), None),
                );
            }
        }
        sink.installed_batch(&registrations)?;
        let fault = self
            .fault
            .lock()
            .map_err(|_error| IndexError::Storage("object fault lock".into()))?
            .take();
        match fault {
            Some(BatchFault::Short) => {
                let _missing = registrations.pop();
            }
            Some(BatchFault::WrongLast) => {
                if let Some(last) = registrations.last_mut() {
                    *last = RegisteredObject::unscoped(
                        reference(255),
                        last.bytes,
                        last.children.clone(),
                    );
                }
            }
            None => {}
        }
        Ok(RegisteredObjectBatch::unscoped(registrations))
    }

    fn get(
        &self,
        reference: ObjectRef,
        budget: &mut WorkBudget,
    ) -> Result<StoredObject, IndexError> {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        self.stored
            .lock()
            .map_err(|_error| IndexError::Storage("object store lock".into()))?
            .get(&reference)
            .cloned()
            .ok_or_else(|| IndexError::CorruptObject("missing canonical test object".into()))
    }
}
