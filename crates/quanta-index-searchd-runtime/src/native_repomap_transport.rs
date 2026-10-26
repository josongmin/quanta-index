//! Pinned `RepoMap` rank route with borrowed canonical reply custody.

use std::sync::Arc;

use quanta_index_contract::{
    index::{IndexInstanceId, ViewRef, ViewSelection},
    native_protocol::{
        NativeIndexResponse, NativeRepoMapQuery, NativeRepoMapResultRef, native_repomap_rank_policy,
    },
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, QueryCoverage, QueryOutcome, WorkBudget,
    effort::ReservationGuard,
};
use quanta_index_repomap::native::NativeRepoMapRankPage;
use serde::{Serialize, Serializer};

use crate::{
    native::NativeDispatcher,
    native_result::{ResultValue, ScopedNativeResult, shared_value_bytes},
};

/// The shared value keeps one page and its retained grants alive through wire
/// emission without copying hits or reason strings.
pub(crate) struct ScopedNativeRepoMapRankResult {
    inner: ScopedNativeResult<RankedPage>,
}

struct RankedPage {
    view: ViewRef,
    instance: IndexInstanceId,
    page: NativeRepoMapRankPage,
}

impl Serialize for ScopedNativeRepoMapRankResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        NativeIndexResponse::serialize_repomap_ranked_ref(&self.inner.borrowed(), serializer)
    }
}

impl RankedPage {
    fn borrowed(&self) -> NativeRepoMapResultRef<'_> {
        NativeRepoMapResultRef {
            view: self.view,
            instance: self.instance,
            policy: native_repomap_rank_policy(),
            hits: self.page.hits(),
            dropped_entries_count: self.page.dropped_entries_count,
            drop_reason_codes: self.page.drop_reason_codes(),
            degraded_reason_codes: self.page.degraded_reason_codes(),
            exhaustive: true,
        }
    }
}

pub(super) fn query(
    dispatcher: &NativeDispatcher,
    selection: ViewSelection,
    instance: IndexInstanceId,
    query: NativeRepoMapQuery,
    budget: &mut WorkBudget,
) -> Result<ScopedNativeRepoMapRankResult, IndexError> {
    let registry = dispatcher
        .repomap
        .get()
        .ok_or(IndexError::SnapshotUnavailable)?;
    let max_hits = query.top_k;
    let prepared = {
        let control = ReservationGuard::try_acquire(
            Arc::clone(&dispatcher.control_scheduler),
            dispatcher.control_need,
        )
        .ok_or_else(|| dispatcher.control_scheduler.denial())?;
        budget.bind_memory_scope(control.memory_scope());
        dispatcher
            .kernel
            .prepare_query(registry, selection, instance, query, budget)?
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
    let result = convert(outcome, instance, max_hits, |bytes, lifetime| {
        budget.reserve_memory(bytes, lifetime)
    })?;
    budget.checkpoint()?;
    Ok(result)
}

fn convert(
    outcome: QueryOutcome<NativeRepoMapRankPage>,
    instance: IndexInstanceId,
    max_hits: u32,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<ScopedNativeRepoMapRankResult, IndexError> {
    if !matches!(outcome.coverage(), QueryCoverage::Core) || !outcome.exhaustive() {
        return Err(IndexError::BackendViolation(
            "native RepoMap rank reader did not return exhaustive core coverage".into(),
        ));
    }
    let view = outcome.view();
    let page = outcome.into_result();
    let result = RankedPage {
        view,
        instance,
        page,
    };
    if usize::try_from(max_hits).map_or(true, |limit| result.page.hits().len() > limit)
        || !result.borrowed().valid_native_rank_result()
    {
        return Err(IndexError::BackendViolation(
            "native RepoMap rank reader returned an invalid ranked page".into(),
        ));
    }
    let capacity_memory = reserve(
        u64::try_from(shared_value_bytes::<RankedPage>()?)
            .map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    Ok(ScopedNativeRepoMapRankResult {
        inner: ScopedNativeResult::new(ResultValue {
            result,
            payload_memory: Vec::new(),
            _capacity_memory: capacity_memory,
        }),
    })
}
