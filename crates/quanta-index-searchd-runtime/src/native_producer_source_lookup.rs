//! Bounded producer-owner enumeration against one current Native source view.

use std::{ops::Deref, sync::Arc};

use quanta_index_contract::{
    index::{OwnerKey, ProducerId, StableOwnerId, ViewSelection},
    native_protocol::{
        NativeProducerSourceOwner, NativeProducerSourcePage, NativeProducerSourceRows,
    },
};
use quanta_index_core::index::{
    IndexError, MapPage, MemoryLease, MemoryLifetime, WorkBudget,
    effort::{ReservationGuard, ResourceNeed},
    owner_from_map_key, owner_map_key,
    ports::{PersistentMapPort, SourceAuthorityPort},
    producer_owner_prefix,
};

use crate::native::NativeDispatcher;

fn append_producer_rows(
    page: &MapPage,
    after_key: &[u8],
    prefix: &[u8; 22],
    producer: ProducerId,
    max_rows: usize,
    rows: &mut Vec<NativeProducerSourceOwner>,
) -> Result<bool, IndexError> {
    let mut previous = after_key;
    for (key, _reference) in &page.entries {
        if key.as_slice() <= previous {
            return Err(IndexError::BackendViolation(
                "producer source scan is not strictly ordered".into(),
            ));
        }
        previous = key.as_slice();
    }
    let mut reached_end = false;
    for (key, reference) in &page.entries {
        if !key.starts_with(prefix) {
            reached_end = true;
            break;
        }
        let owner = owner_from_map_key(key)?;
        if owner.producer != producer {
            return Err(IndexError::CorruptObject(
                "producer source owner key disagrees with prefix".into(),
            ));
        }
        if rows.len() == max_rows {
            return Ok(true);
        }
        rows.push(NativeProducerSourceOwner {
            owner,
            source: reference.object_ref()?,
        });
    }
    if !reached_end && page.has_more {
        return Err(IndexError::BackendViolation(
            "producer source scan omitted its prefix lookahead".into(),
        ));
    }
    Ok(false)
}

pub(crate) struct ScopedNativeProducerSourcePage {
    // Drop materialized rows before refunding their retained allocation.
    result: NativeProducerSourcePage,
    _memory: MemoryLease,
}

impl Deref for ScopedNativeProducerSourcePage {
    type Target = NativeProducerSourcePage;

    fn deref(&self) -> &Self::Target {
        &self.result
    }
}

impl NativeDispatcher {
    pub(super) fn list_producer_source_owners(
        &self,
        selection: ViewSelection,
        producer: ProducerId,
        after: Option<StableOwnerId>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ScopedNativeProducerSourcePage, IndexError> {
        if !(1..=128).contains(&limit) {
            return Err(IndexError::InvalidInput(
                "producer source page limit must be 1..=128".into(),
            ));
        }
        if after.is_some() && matches!(&selection, ViewSelection::Latest { .. }) {
            return Err(IndexError::InvalidInput(
                "producer source continuation requires an exact view".into(),
            ));
        }
        let page_limit = limit.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        let need = ResourceNeed {
            provider_units: 0,
            ..self.interactive_need
        };
        let reservation =
            ReservationGuard::try_acquire(Arc::clone(&self.interactive_scheduler), need)
                .ok_or_else(|| self.interactive_scheduler.denial())?;
        budget.bind_memory_scope(reservation.memory_scope());
        let max_rows = usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?;
        let bytes = max_rows
            .checked_mul(std::mem::size_of::<NativeProducerSourceOwner>())
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<ScopedNativeProducerSourcePage>())
            })
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget
            .reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?
            .ok_or_else(|| {
                IndexError::BackendViolation(
                    "producer source page requires allocation custody".into(),
                )
            })?;

        let ticket = self.kernel.pin_view(selection, budget)?;
        let result = (|| {
            ticket.validate()?;
            let view = ticket.view().identity;
            let source = ticket.view().source;
            // This port rejects a historical Exact view. A producer-wide epoch is
            // current authority, not an attribute of arbitrary retained history.
            let source_epoch = self.catalog.source_epoch(view, producer, budget)?;
            let prefix = producer_owner_prefix(producer);
            let owner_after_key = after.map(|stable_id| {
                owner_map_key(OwnerKey {
                    producer,
                    stable_id,
                })
            });
            let after_key = owner_after_key
                .as_ref()
                .map_or(prefix.as_slice(), |key| key.as_slice());
            let page = self
                .objects
                .scan(source.owners, Some(after_key), page_limit, budget)?;
            if page.entries.len()
                > usize::try_from(page_limit).map_err(|_error| IndexError::ResourceLimited)?
                || (page.entries.is_empty() && page.has_more)
            {
                return Err(IndexError::BackendViolation(
                    "producer source scan exceeded its page or did not advance".into(),
                ));
            }
            let mut rows = Vec::new();
            rows.try_reserve_exact(max_rows)
                .map_err(|_error| IndexError::ResourceLimited)?;
            let has_more =
                append_producer_rows(&page, after_key, &prefix, producer, max_rows, &mut rows)?;
            ticket.validate()?;
            if self.catalog.source_epoch(view, producer, budget)? != source_epoch {
                return Err(IndexError::Conflict);
            }
            if !rows.is_empty() && source_epoch.is_none() {
                return Err(IndexError::CorruptObject(
                    "producer source owners exist without epoch authority".into(),
                ));
            }
            budget.checkpoint()?;
            Ok(NativeProducerSourcePage {
                view,
                source,
                producer,
                source_epoch,
                rows: NativeProducerSourceRows::new(rows)
                    .map_err(|message| IndexError::BackendViolation(message.into()))?,
                has_more,
            })
        })();
        let result = ticket.finish(result, budget)?;
        Ok(ScopedNativeProducerSourcePage {
            result,
            _memory: memory,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use quanta_index_contract::index::{
        CollectionId, Digest, MapRoot, ObjectRef, SlotId, SourceCut, TenantId, ViewRef,
        WorkspaceKey, WorktreeId,
    };
    use quanta_index_core::index::effort::ResourceNeed;

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
                    "producer page charge escaped serialization lifetime",
                ));
            }
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn owner(producer: ProducerId, stable: u8) -> OwnerKey {
        OwnerKey {
            producer,
            stable_id: StableOwnerId([stable; 16]),
        }
    }

    #[test]
    fn producer_page_lookahead_stops_at_the_prefix_boundary() -> Result<(), IndexError> {
        let producer = ProducerId([1; 16]);
        let other = ProducerId([2; 16]);
        let prefix = producer_owner_prefix(producer);
        let first = owner(producer, 1);
        let second = owner(producer, 2);
        let page = MapPage::new(
            vec![
                (
                    owner_map_key(first).to_vec(),
                    (ObjectRef(Digest([11; 32]))).into(),
                ),
                (
                    owner_map_key(second).to_vec(),
                    (ObjectRef(Digest([12; 32]))).into(),
                ),
            ],
            true,
            Vec::new(),
            None,
        );
        let mut rows = Vec::new();
        if !append_producer_rows(&page, &prefix, &prefix, producer, 1, &mut rows)?
            || rows.as_slice()
                != [NativeProducerSourceOwner {
                    owner: first,
                    source: ObjectRef(Digest([11; 32])),
                }]
        {
            return Err(IndexError::BackendViolation(
                "producer page did not preserve bounded lookahead".into(),
            ));
        }
        let after = owner_map_key(first);
        let page = MapPage::new(
            vec![
                (
                    owner_map_key(second).to_vec(),
                    (ObjectRef(Digest([12; 32]))).into(),
                ),
                (
                    owner_map_key(owner(other, 1)).to_vec(),
                    (ObjectRef(Digest([13; 32]))).into(),
                ),
            ],
            true,
            Vec::new(),
            None,
        );
        rows.clear();
        if append_producer_rows(&page, &after, &prefix, producer, 1, &mut rows)?
            || rows.as_slice()
                != [NativeProducerSourceOwner {
                    owner: second,
                    source: ObjectRef(Digest([12; 32])),
                }]
        {
            return Err(IndexError::BackendViolation(
                "producer page crossed the next producer boundary".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn producer_page_rejects_unordered_or_malformed_keys() -> Result<(), IndexError> {
        let producer = ProducerId([1; 16]);
        let prefix = producer_owner_prefix(producer);
        let first = owner_map_key(owner(producer, 1));
        let second = owner_map_key(owner(producer, 2));
        let unordered = MapPage::new(
            vec![
                (second.to_vec(), (ObjectRef(Digest([2; 32]))).into()),
                (first.to_vec(), (ObjectRef(Digest([1; 32]))).into()),
            ],
            false,
            Vec::new(),
            None,
        );
        let mut rows = Vec::new();
        if !matches!(
            append_producer_rows(&unordered, &prefix, &prefix, producer, 2, &mut rows),
            Err(IndexError::BackendViolation(_))
        ) {
            return Err(IndexError::BackendViolation(
                "unordered producer page was accepted".into(),
            ));
        }
        let malformed = MapPage::new(
            vec![(
                prefix.iter().copied().chain([1]).collect(),
                (ObjectRef(Digest([3; 32]))).into(),
            )],
            false,
            Vec::new(),
            None,
        );
        if !matches!(
            append_producer_rows(&malformed, &prefix, &prefix, producer, 2, &mut rows),
            Err(IndexError::CorruptObject(_))
        ) {
            return Err(IndexError::BackendViolation(
                "malformed producer owner key was accepted".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn producer_page_reply_retains_memory_without_execution_through_serialization()
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
        let producer = ProducerId([6; 16]);
        let reply = NativeReply::ProducerSourceOwners(ScopedNativeProducerSourcePage {
            result: NativeProducerSourcePage {
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
                producer,
                source_epoch: Some(7),
                rows: NativeProducerSourceRows::new(vec![NativeProducerSourceOwner {
                    owner: owner(producer, 1),
                    source: ObjectRef(Digest([7; 32])),
                }])?,
                has_more: false,
            },
            _memory: memory,
        });
        drop(reservation);
        serde_json::to_writer(ChargedWriter(ledger.observation()), &reply)?;
        drop(reply);
        let final_state = ledger.observation().observe()?;
        if final_state.retained_memory_bytes != 0 || final_state.reserved != ResourceNeed::default()
        {
            return Err("producer page reply did not refund its charge".into());
        }
        Ok(())
    }
}
