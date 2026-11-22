//! Bounded projection contribution stamps from one selected immutable view.

use std::{ops::Deref, sync::Arc};

use quanta_index_contract::{
    index::{ObjectRef, OwnerKey, ProducerId, ProjectionId, StableOwnerId, ViewRef, ViewSelection},
    native_protocol::{
        MAX_NATIVE_SOURCE_OWNER_LOOKUP, NativeProducerProjectionOwnerPageV1,
        NativeProducerProjectionOwnerRowsV1, NativeProducerProjectionOwnerV1,
        NativeProjectionOwnerBinding, NativeProjectionOwnerLookup, NativeProjectionOwnerRows,
        NativeSourceOwnerKeys,
    },
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, WorkBudget, decode_projection_owner,
    effort::{ReservationGuard, ResourceNeed},
    owner_map_key,
    ports::{ImmutableObjectPort, PersistentMapPort},
};

use crate::native::NativeDispatcher;
use crate::native_producer_source_lookup::ProducerOwnerWindow;

pub(crate) struct ScopedNativeProjectionOwnerLookup {
    result: NativeProjectionOwnerLookup,
    _memory: MemoryLease,
}

pub(crate) struct ScopedProducerProjectionOwnersV1 {
    result: NativeProducerProjectionOwnerPageV1,
    _memory: MemoryLease,
}

impl Deref for ScopedProducerProjectionOwnersV1 {
    type Target = NativeProducerProjectionOwnerPageV1;

    fn deref(&self) -> &Self::Target {
        &self.result
    }
}

impl Deref for ScopedNativeProjectionOwnerLookup {
    type Target = NativeProjectionOwnerLookup;

    fn deref(&self) -> &Self::Target {
        &self.result
    }
}

impl NativeDispatcher {
    pub(super) fn list_producer_projection_owners_v1(
        &self,
        view: ViewRef,
        projection: ProjectionId,
        producer: ProducerId,
        after: Option<StableOwnerId>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ScopedProducerProjectionOwnersV1, IndexError> {
        let max_rows = usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?;
        if !(1..=MAX_NATIVE_SOURCE_OWNER_LOOKUP).contains(&max_rows) {
            return Err(IndexError::InvalidInput(
                "producer projection page exceeds its bound".into(),
            ));
        }
        let need = ResourceNeed {
            provider_units: 0,
            ..self.interactive_need
        };
        let reservation =
            ReservationGuard::try_acquire(Arc::clone(&self.interactive_scheduler), need)
                .ok_or_else(|| self.interactive_scheduler.denial())?;
        budget.bind_memory_scope(reservation.memory_scope());
        let bytes = max_rows
            .checked_mul(std::mem::size_of::<NativeProducerProjectionOwnerV1>())
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<ScopedProducerProjectionOwnersV1>())
            })
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget
            .reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?
            .ok_or_else(|| {
                IndexError::BackendViolation(
                    "producer projection page requires allocation custody".into(),
                )
            })?;

        let selection = ViewSelection::Exact(view);
        let ticket = self.kernel.pin_view(selection, budget)?;
        let result = (|| {
            ticket.validate()?;
            let (selected, binding) = self
                .kernel
                .read_projection_binding(selection, projection, budget)?;
            let binding = binding.ok_or(IndexError::SnapshotUnavailable)?;
            if selected != view || binding.projection != projection || binding.producer != producer
            {
                return Err(IndexError::InvalidInput(
                    "producer projection basis differs".into(),
                ));
            }
            let reference_bytes = max_rows
                .checked_mul(std::mem::size_of::<(OwnerKey, ObjectRef)>())
                .ok_or(IndexError::ResourceLimited)?;
            let _reference_memory = budget
                .reserve_memory(
                    u64::try_from(reference_bytes).map_err(|_error| IndexError::ResourceLimited)?,
                    MemoryLifetime::Temporary,
                )?
                .ok_or_else(|| {
                    IndexError::BackendViolation(
                        "producer projection scan requires allocation custody".into(),
                    )
                })?;
            let (references, has_more) = ProducerOwnerWindow {
                producer,
                after,
                limit,
            }
            .scan(
                self.objects.as_ref(),
                binding.owners,
                |owner, contribution| (owner, contribution),
                budget,
            )?;
            let mut rows = Vec::new();
            rows.try_reserve_exact(references.len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            for (owner, reference) in references {
                ticket.validate()?;
                let stored = self.objects.get(reference, budget)?;
                let contribution = decode_projection_owner(&stored.payload, budget)?;
                if stored.registration.reference != reference
                    || stored.registration.children != contribution.object_children()
                    || contribution.owner != owner
                    || contribution.projection != projection
                    || contribution.producer != producer
                    || contribution.source_owner.is_some() != contribution.source.is_some()
                {
                    return Err(IndexError::CorruptObject(
                        "producer projection page contribution differs".into(),
                    ));
                }
                if let (Some(source_owner), Some(source)) =
                    (contribution.source_owner, contribution.source)
                {
                    let bound_source = self.objects.lookup_object(
                        binding.source.owners,
                        &owner_map_key(source_owner),
                        budget,
                    )?;
                    if bound_source != Some(source) {
                        return Err(IndexError::CorruptObject(
                            "producer projection page source differs from binding".into(),
                        ));
                    }
                }
                rows.push(NativeProducerProjectionOwnerV1 {
                    owner,
                    contribution: reference,
                    source_owner: contribution.source_owner,
                    source: contribution.source,
                    dependency_stamp: contribution.dependency_stamp,
                    output: contribution.output,
                });
            }
            let rows = if rows.is_empty() {
                None
            } else {
                Some(
                    NativeProducerProjectionOwnerRowsV1::new(rows)
                        .map_err(|message| IndexError::BackendViolation(message.into()))?,
                )
            };
            let page = NativeProducerProjectionOwnerPageV1 {
                view,
                binding,
                source_records_root: binding.source.owners,
                rows,
                has_more,
            };
            if !page.matches_request(view, projection, producer, after, limit) {
                return Err(IndexError::BackendViolation(
                    "producer projection page differs".into(),
                ));
            }
            ticket.validate()?;
            budget.checkpoint()?;
            Ok(page)
        })();
        let result = ticket.finish(result, budget)?;
        Ok(ScopedProducerProjectionOwnersV1 {
            result,
            _memory: memory,
        })
    }

    pub(super) fn lookup_projection_owners(
        &self,
        selection: ViewSelection,
        projection: ProjectionId,
        owners: &NativeSourceOwnerKeys,
        budget: &mut WorkBudget,
    ) -> Result<ScopedNativeProjectionOwnerLookup, IndexError> {
        let need = ResourceNeed {
            provider_units: 0,
            ..self.interactive_need
        };
        let reservation =
            ReservationGuard::try_acquire(Arc::clone(&self.interactive_scheduler), need)
                .ok_or_else(|| self.interactive_scheduler.denial())?;
        budget.bind_memory_scope(reservation.memory_scope());
        let bytes = owners
            .as_slice()
            .len()
            .checked_mul(std::mem::size_of::<NativeProjectionOwnerBinding>())
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<ScopedNativeProjectionOwnerLookup>())
            })
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget
            .reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?
            .ok_or_else(|| {
                IndexError::BackendViolation(
                    "native projection lookup requires allocation custody".into(),
                )
            })?;
        let ticket = self.kernel.pin_view(selection, budget)?;
        let result = (|| {
            ticket.validate()?;
            let (view, binding) = self.kernel.read_projection_binding(
                ViewSelection::Exact(ticket.view().identity),
                projection,
                budget,
            )?;
            if view != ticket.view().identity
                || binding.is_some_and(|stamp| stamp.projection != projection)
            {
                return Err(IndexError::BackendViolation(
                    "projection lookup binding differs from selected view".into(),
                ));
            }
            let mut rows = Vec::new();
            rows.try_reserve_exact(owners.as_slice().len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            for owner in owners.as_slice() {
                budget.checkpoint()?;
                ticket.validate()?;
                let contribution = match binding {
                    Some(stamp) => {
                        self.objects
                            .lookup_object(stamp.owners, &owner_map_key(*owner), budget)?
                    }
                    None => None,
                };
                rows.push(NativeProjectionOwnerBinding {
                    owner: *owner,
                    contribution,
                });
            }
            ticket.validate()?;
            budget.checkpoint()?;
            Ok(NativeProjectionOwnerLookup {
                view,
                projection,
                binding,
                rows: NativeProjectionOwnerRows::new(rows)
                    .map_err(|message| IndexError::BackendViolation(message.into()))?,
            })
        })();
        let result = ticket.finish(result, budget)?;
        Ok(ScopedNativeProjectionOwnerLookup {
            result,
            _memory: memory,
        })
    }
}
