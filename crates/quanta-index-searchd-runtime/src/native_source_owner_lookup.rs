//! Exact-view owner reads with bounded output and retained reply custody.

use std::{ops::Deref, sync::Arc};

use quanta_index_contract::{
    index::ViewSelection,
    native_protocol::{
        NativeSourceOwnerBinding, NativeSourceOwnerKeys, NativeSourceOwnerLookup,
        NativeSourceOwnerRows,
    },
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, WorkBudget,
    effort::{ReservationGuard, ResourceNeed},
    owner_map_key,
    ports::PersistentMapPort,
};

use crate::native::NativeDispatcher;

pub(crate) struct ScopedNativeSourceOwnerLookup {
    // Drop materialized rows before refunding their backing allocation.
    result: NativeSourceOwnerLookup,
    _memory: MemoryLease,
}

impl Deref for ScopedNativeSourceOwnerLookup {
    type Target = NativeSourceOwnerLookup;

    fn deref(&self) -> &Self::Target {
        &self.result
    }
}

impl NativeDispatcher {
    pub(super) fn lookup_source_owners(
        &self,
        selection: ViewSelection,
        owners: &NativeSourceOwnerKeys,
        budget: &mut WorkBudget,
    ) -> Result<ScopedNativeSourceOwnerLookup, IndexError> {
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
            .checked_mul(std::mem::size_of::<NativeSourceOwnerBinding>())
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<ScopedNativeSourceOwnerLookup>())
            })
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget
            .reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?
            .ok_or_else(|| {
                IndexError::BackendViolation(
                    "native owner lookup requires allocation custody".into(),
                )
            })?;
        let ticket = self.kernel.pin_view(selection, budget)?;
        let result = (|| {
            ticket.validate()?;
            let mut rows = Vec::new();
            rows.try_reserve_exact(owners.as_slice().len())
                .map_err(|_error| IndexError::ResourceLimited)?;
            for owner in owners.as_slice() {
                budget.checkpoint()?;
                ticket.validate()?;
                let source = self.objects.lookup_object(
                    ticket.view().source.owners,
                    &owner_map_key(*owner),
                    budget,
                )?;
                rows.push(NativeSourceOwnerBinding {
                    owner: *owner,
                    source,
                });
            }
            ticket.validate()?;
            budget.checkpoint()?;
            Ok(NativeSourceOwnerLookup {
                view: ticket.view().identity,
                source: ticket.view().source,
                rows: NativeSourceOwnerRows::new(rows)
                    .map_err(|message| IndexError::BackendViolation(message.into()))?,
            })
        })();
        let result = ticket.finish(result, budget)?;
        Ok(ScopedNativeSourceOwnerLookup {
            result,
            _memory: memory,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use quanta_index_contract::index::{
        CollectionId, Digest, MapRoot, OwnerKey, ProducerId, SlotId, SourceCut, StableOwnerId,
        TenantId, ViewRef, WorkspaceKey, WorktreeId,
    };

    use super::*;
    use crate::{
        native_reply::NativeReply,
        native_resources::{
            NativeResourceClass, NativeResourceLedger, NativeResourceObservation,
            NativeResourcePlan,
        },
    };

    struct ChargedWriter(NativeResourceObservation);

    impl Write for ChargedWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let snapshot = self.0.observe().map_err(io::Error::other)?;
            if snapshot.retained_memory_bytes != 8192
                || snapshot.reserved != ResourceNeed::default()
            {
                return Err(io::Error::other(
                    "reply charge escaped its serialization lifetime",
                ));
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn source_owner_reply_retains_charge_without_execution_through_serialization()
    -> Result<(), Box<dyn std::error::Error>> {
        let need = ResourceNeed {
            cpu_units: 1,
            io_bytes: 1024,
            memory_bytes: 8192,
            provider_units: 0,
        };
        let ledger = NativeResourceLedger::new(NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 4,
                io_bytes: 4096,
                memory_bytes: 65536,
                provider_units: 0,
            },
            control: need,
            interactive: need,
            core: need,
            enrichment: need,
        })?;
        let scheduler = ledger.scheduler(NativeResourceClass::Interactive);
        let reservation =
            ReservationGuard::try_acquire(scheduler, need).ok_or("reservation refused")?;
        let memory = reservation
            .memory_scope()
            .ok_or("memory scope absent")?
            .reserve(8192, MemoryLifetime::Retained)?;
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let reply = NativeReply::SourceOwners(ScopedNativeSourceOwnerLookup {
            result: NativeSourceOwnerLookup {
                view: ViewRef {
                    workspace,
                    epoch: 1,
                    sequence: 1,
                    digest: Digest([5; 32]),
                },
                source: SourceCut {
                    owners: MapRoot(None),
                    paths: MapRoot(None),
                    frontiers: MapRoot(None),
                },
                rows: NativeSourceOwnerRows::new(vec![NativeSourceOwnerBinding {
                    owner: OwnerKey {
                        producer: ProducerId([6; 16]),
                        stable_id: StableOwnerId([7; 16]),
                    },
                    source: None,
                }])?,
            },
            _memory: memory,
        });
        drop(reservation);
        serde_json::to_writer(ChargedWriter(ledger.observation()), &reply)?;
        drop(reply);
        let final_state = ledger.observation().observe()?;
        if final_state.retained_memory_bytes != 0 || final_state.reserved != ResourceNeed::default()
        {
            return Err("reply did not refund its charge".into());
        }
        Ok(())
    }
}
