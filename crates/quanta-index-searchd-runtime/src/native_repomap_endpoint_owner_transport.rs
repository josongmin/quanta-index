//! Exact predecessor endpoint supporters from the ticket-pinned Graph member.

use std::sync::Arc;

use quanta_index_contract::{
    RepoMapNodeRef,
    index::{IndexInstanceId, ViewRef, ViewSelection},
    native_protocol::{NativeRepoMapEndpointOwnerV2, NativeRepoMapEndpointOwnersResultV2},
};
use quanta_index_core::index::{
    IndexError, MemoryLifetime, QueryCoverage, WorkBudget, effort::ReservationGuard,
};
use serde::{Serialize, Serializer};

use crate::{
    native::NativeDispatcher,
    native_result::{ResultValue, ScopedNativeResult, shared_value_bytes},
};

pub(crate) struct ScopedEndpointOwnersResponse(
    ScopedNativeResult<NativeRepoMapEndpointOwnersResultV2>,
);

impl Serialize for ScopedEndpointOwnersResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_newtype_variant(
            "NativeIndexResponse",
            73,
            "RepoMapEndpointOwnersV2",
            &*self.0,
        )
    }
}

pub(super) fn read(
    dispatcher: &NativeDispatcher,
    view: ViewRef,
    instance: IndexInstanceId,
    endpoint: RepoMapNodeRef,
    budget: &mut WorkBudget,
) -> Result<ScopedEndpointOwnersResponse, IndexError> {
    let registry = dispatcher
        .repomap_endpoint_owners
        .get()
        .ok_or(IndexError::SnapshotUnavailable)?;
    let prepared = {
        let control = ReservationGuard::try_acquire(
            Arc::clone(&dispatcher.control_scheduler),
            dispatcher.control_need,
        )
        .ok_or_else(|| dispatcher.control_scheduler.denial())?;
        budget.bind_memory_scope(control.memory_scope());
        dispatcher.kernel.prepare_query(
            registry,
            ViewSelection::Exact(view),
            instance,
            endpoint.clone(),
            budget,
        )?
    };
    let need = prepared.required_resources();
    let Some(reservation) =
        ReservationGuard::try_acquire(Arc::clone(&dispatcher.interactive_scheduler), need)
    else {
        let denial = dispatcher.interactive_scheduler.denial();
        prepared.abort()?;
        return Err(denial);
    };
    let outcome = prepared.execute(&reservation)?;
    if !matches!(outcome.coverage(), QueryCoverage::Core)
        || !outcome.exhaustive()
        || outcome.view() != view
    {
        return Err(IndexError::BackendViolation(
            "RepoMap endpoint owners did not bind their exact view".into(),
        ));
    }
    let selected = outcome.into_result();
    let endpoint_bytes = match &endpoint {
        RepoMapNodeRef::File(id) => id.as_str().len(),
        RepoMapNodeRef::Module(id) => id.as_str().len(),
        RepoMapNodeRef::Symbol(id) => id.as_str().len(),
        RepoMapNodeRef::Chunk(id) => id.as_str().len(),
    };
    let base_bytes = shared_value_bytes::<NativeRepoMapEndpointOwnersResultV2>()?
        .checked_add(
            endpoint_bytes
                .checked_mul(2)
                .ok_or(IndexError::ResourceLimited)?,
        )
        .ok_or(IndexError::ResourceLimited)?;
    let retained_bytes = selected.owners.iter().try_fold(base_bytes, |total, row| {
        total
            .checked_add(std::mem::size_of::<NativeRepoMapEndpointOwnerV2>() * 2)
            .and_then(|total| {
                row.prior
                    .wire()
                    .facts
                    .owner_path
                    .as_str()
                    .len()
                    .checked_mul(2)
                    .and_then(|bytes| total.checked_add(bytes))
            })
            .ok_or(IndexError::ResourceLimited)
    })?;
    let capacity_memory = budget.reserve_memory(
        u64::try_from(retained_bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let mut owners = Vec::new();
    owners
        .try_reserve_exact(selected.owners.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    for row in selected.owners {
        let prior = row.prior.wire();
        owners.push(NativeRepoMapEndpointOwnerV2 {
            owner: row.owner,
            owner_path: prior.facts.owner_path.clone(),
            source_owner: prior.source_owner,
            source: prior.source,
            contribution: prior.contribution,
            output: prior.output,
            rank_leaf: row.rank_leaf,
        });
    }
    budget.checkpoint()?;
    Ok(ScopedEndpointOwnersResponse(ScopedNativeResult::new(
        ResultValue {
            result: NativeRepoMapEndpointOwnersResultV2 {
                view,
                instance,
                endpoint,
                owners,
            },
            payload_memory: Vec::new(),
            _capacity_memory: capacity_memory,
        },
    )))
}
