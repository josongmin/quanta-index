//! Bounded projection contribution stamps from one selected immutable view.

use std::{ops::Deref, sync::Arc};

use quanta_index_contract::{
    index::{ProjectionId, ViewSelection},
    native_protocol::{
        NativeProjectionOwnerBinding, NativeProjectionOwnerLookup, NativeProjectionOwnerRows,
        NativeSourceOwnerKeys,
    },
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, WorkBudget,
    effort::{ReservationGuard, ResourceNeed},
    owner_map_key,
    ports::PersistentMapPort,
};

use crate::native::NativeDispatcher;

pub(crate) struct ScopedNativeProjectionOwnerLookup {
    result: NativeProjectionOwnerLookup,
    _memory: MemoryLease,
}

impl Deref for ScopedNativeProjectionOwnerLookup {
    type Target = NativeProjectionOwnerLookup;

    fn deref(&self) -> &Self::Target {
        &self.result
    }
}

impl NativeDispatcher {
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
