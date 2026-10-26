//! Diff only roots authenticated and retained by one selected Native view.

use std::{ops::Deref, sync::Arc};

use quanta_index_contract::{
    index::{MapRoot, OwnerMutation, ProducerId, ProjectionId, StableOwnerId, ViewSelection},
    native_protocol::{NativeProjectionSourceDiff, NativeSourceDiffRows},
};
use quanta_index_core::index::{
    IndexError, MapValue, MemoryLease, MemoryLifetime, WorkBudget, WorkUsage,
    effort::{ReservationGuard, ResourceNeed},
    owner_from_map_key, owner_map_key,
    ports::{MapDiffRange, MapDiffVisitor, PersistentMapPort},
    producer_owner_prefix,
};

use crate::native::NativeDispatcher;

pub(crate) struct ScopedNativeProjectionSourceDiff {
    result: NativeProjectionSourceDiff,
    _memory: MemoryLease,
}
impl Deref for ScopedNativeProjectionSourceDiff {
    type Target = NativeProjectionSourceDiff;
    fn deref(&self) -> &Self::Target {
        &self.result
    }
}

struct Rows<'a> {
    values: Vec<OwnerMutation>,
    producer: ProducerId,
    previous: Option<StableOwnerId>,
    limit: usize,
    ticket: &'a quanta_index_kernel::ReadTicket,
}
impl MapDiffVisitor for Rows<'_> {
    fn changed(
        &mut self,
        key: &[u8],
        expected: Option<MapValue>,
        next: Option<MapValue>,
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        self.ticket.validate()?;
        let owner = owner_from_map_key(key)?;
        if owner.producer != self.producer
            || self
                .previous
                .is_some_and(|previous| owner.stable_id <= previous)
            || self.values.len() == self.limit
            || expected == next
        {
            return Err(IndexError::BackendViolation(
                "source diff visitor escaped its range".into(),
            ));
        }
        let expected = expected.map(MapValue::object_ref).transpose()?;
        let next = next.map(MapValue::object_ref).transpose()?;
        budget.charge(WorkUsage {
            bytes: u64::try_from(std::mem::size_of::<OwnerMutation>())
                .map_err(|_error| IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        self.values.push(OwnerMutation {
            owner,
            expected,
            next,
        });
        self.previous = Some(owner.stable_id);
        Ok(())
    }
}

impl NativeDispatcher {
    pub(super) fn diff_projection_source_owners(
        &self,
        selection: ViewSelection,
        projection: ProjectionId,
        producer: ProducerId,
        after: Option<StableOwnerId>,
        limit: u32,
        budget: &mut WorkBudget,
    ) -> Result<ScopedNativeProjectionSourceDiff, IndexError> {
        if !(1..=128).contains(&limit)
            || (after.is_some() && !matches!(selection, ViewSelection::Exact(_)))
        {
            return Err(IndexError::InvalidInput(
                "source diff requires limit 1..=128 and Exact continuation".into(),
            ));
        }
        let reservation = ReservationGuard::try_acquire(
            Arc::clone(&self.interactive_scheduler),
            ResourceNeed {
                provider_units: 0,
                ..self.interactive_need
            },
        )
        .ok_or_else(|| self.interactive_scheduler.denial())?;
        budget.bind_memory_scope(reservation.memory_scope());
        let capacity = usize::try_from(limit).map_err(|_error| IndexError::ResourceLimited)?;
        let bytes = capacity
            .checked_mul(std::mem::size_of::<OwnerMutation>())
            .and_then(|bytes| {
                bytes.checked_add(std::mem::size_of::<ScopedNativeProjectionSourceDiff>())
            })
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget
            .reserve_memory(
                u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
                MemoryLifetime::Retained,
            )?
            .ok_or_else(|| {
                IndexError::BackendViolation("source diff requires allocation custody".into())
            })?;
        let ticket = self.kernel.pin_view(selection, budget)?;
        let result = (|| {
            ticket.validate()?;
            let (view, binding) = self.kernel.read_projection_binding(
                ViewSelection::Exact(ticket.view().identity),
                projection,
                budget,
            )?;
            if view != ticket.view().identity {
                return Err(IndexError::BackendViolation(
                    "source diff selected view changed".into(),
                ));
            }
            // The selected binding's authenticated physical children retain
            // its old SourceCut, even when current Source has advanced.
            let before = binding.map_or(MapRoot(None), |binding| binding.source.owners);
            let source = ticket.view().source;
            let prefix = producer_owner_prefix(producer);
            let cursor = after.map(|stable_id| {
                owner_map_key(quanta_index_contract::index::OwnerKey {
                    producer,
                    stable_id,
                })
            });
            let mut rows = Rows {
                values: Vec::new(),
                producer,
                previous: after,
                limit: capacity,
                ticket: &ticket,
            };
            rows.values
                .try_reserve_exact(capacity)
                .map_err(|_error| IndexError::ResourceLimited)?;
            let has_more = self.objects.visit_diff(
                before,
                source.owners,
                MapDiffRange {
                    prefix: &prefix,
                    after: cursor.as_ref().map(AsRef::as_ref),
                    limit,
                },
                &mut rows,
                budget,
            )?;
            ticket.validate()?;
            budget.checkpoint()?;
            let page = NativeProjectionSourceDiff {
                view,
                projection,
                binding,
                source,
                producer,
                rows: NativeSourceDiffRows::new(rows.values)
                    .map_err(|message| IndexError::BackendViolation(message.into()))?,
                has_more,
            };
            page.validate()
                .map_err(|message| IndexError::BackendViolation(message.into()))?;
            Ok(page)
        })();
        let result = ticket.finish(result, budget)?;
        Ok(ScopedNativeProjectionSourceDiff {
            result,
            _memory: memory,
        })
    }
}
