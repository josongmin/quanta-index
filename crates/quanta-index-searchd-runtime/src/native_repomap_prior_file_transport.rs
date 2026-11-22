//! Owner-only predecessor facts from the exact ticket-pinned RepoMap member.

use std::sync::Arc;

use quanta_index_contract::{
    index::{IndexInstanceId, OwnerKey, ViewRef, ViewSelection},
    native_protocol::NativeIndexResponse,
};
use quanta_index_core::index::{
    IndexError, MemoryLifetime, QueryCoverage, WorkBudget, effort::ReservationGuard,
};
use quanta_index_repomap::native::NativeRepoMapPriorFileContextV1;
use serde::{Serialize, Serializer};

use crate::{
    native::NativeDispatcher,
    native_result::{ResultValue, ScopedNativeResult, shared_value_bytes},
};

struct PriorFileResponse {
    view: ViewRef,
    instance: IndexInstanceId,
    owner: OwnerKey,
    file: Option<NativeRepoMapPriorFileContextV1>,
}

pub(crate) struct ScopedPriorFileResponse(ScopedNativeResult<PriorFileResponse>);

impl Serialize for ScopedPriorFileResponse {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        NativeIndexResponse::serialize_repomap_prior_file_ref_v1(
            self.0.view,
            self.0.instance,
            self.0.owner,
            self.0
                .file
                .as_ref()
                .map(NativeRepoMapPriorFileContextV1::wire),
            serializer,
        )
    }
}

pub(super) fn read(
    dispatcher: &NativeDispatcher,
    view: ViewRef,
    instance: IndexInstanceId,
    owner: OwnerKey,
    budget: &mut WorkBudget,
) -> Result<ScopedPriorFileResponse, IndexError> {
    let registry = dispatcher
        .repomap_prior_files
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
            owner,
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
            "RepoMap predecessor file did not bind its exact view".into(),
        ));
    }
    let capacity_memory = budget.reserve_memory(
        u64::try_from(shared_value_bytes::<PriorFileResponse>()?)
            .map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    budget.checkpoint()?;
    Ok(ScopedPriorFileResponse(ScopedNativeResult::new(
        ResultValue {
            result: PriorFileResponse {
                view,
                instance,
                owner,
                file: outcome.into_result(),
            },
            payload_memory: Vec::new(),
            _capacity_memory: capacity_memory,
        },
    )))
}
