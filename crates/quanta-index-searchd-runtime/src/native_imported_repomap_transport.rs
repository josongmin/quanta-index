//! Ticket-bound query of the imported exact Legacy RepoMap graph.

use std::sync::Arc;

use quanta_index_contract::{
    index::{IndexInstanceId, ViewSelection},
    native_protocol::{NativeImportedRepoMapQueryV1, NativeImportedRepoMapResultV1},
};
use quanta_index_core::index::{
    IndexError, MemoryLifetime, QueryCoverage, QueryOutcome, WorkBudget, effort::ReservationGuard,
};
use quanta_index_repomap::native::NativeLegacyRepoMapGraphPageV1;

use crate::{
    native::NativeDispatcher,
    native_graph_transport::GraphAnchor,
    native_result::{ResultValue, ScopedNativeResult, shared_value_bytes},
};

pub(crate) type ScopedImportedRepoMapResultV1 = ScopedNativeResult<NativeImportedRepoMapResultV1>;

pub(crate) fn query(
    dispatcher: &NativeDispatcher,
    selection: ViewSelection,
    instance: IndexInstanceId,
    query: NativeImportedRepoMapQueryV1,
    budget: &mut WorkBudget,
) -> Result<ScopedImportedRepoMapResultV1, IndexError> {
    let registry = dispatcher
        .imported_repomap_graph
        .get()
        .ok_or(IndexError::SnapshotUnavailable)?;
    let direction = query.direction;
    let max_edges = query.max_edges;
    let (prepared, anchor) = {
        let control = ReservationGuard::try_acquire(
            Arc::clone(&dispatcher.control_scheduler),
            dispatcher.control_need,
        )
        .ok_or_else(|| dispatcher.control_scheduler.denial())?;
        budget.bind_memory_scope(control.memory_scope());
        let anchor = GraphAnchor::capture(&query.source, budget)?;
        let prepared = dispatcher
            .kernel
            .prepare_query(registry, selection, instance, query, budget)?;
        (prepared, anchor)
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
    let result = convert(outcome, instance, direction, max_edges, anchor, budget)?;
    budget.checkpoint()?;
    Ok(result)
}

fn convert(
    outcome: QueryOutcome<NativeLegacyRepoMapGraphPageV1>,
    instance: IndexInstanceId,
    direction: quanta_index_contract::native_protocol::NativeGraphDirection,
    max_edges: u32,
    anchor: GraphAnchor,
    budget: &WorkBudget,
) -> Result<ScopedImportedRepoMapResultV1, IndexError> {
    if !matches!(outcome.coverage(), QueryCoverage::Core) || !outcome.exhaustive() {
        return Err(IndexError::BackendViolation(
            "imported RepoMap reader did not return Core coverage".into(),
        ));
    }
    let view = outcome.view();
    let page = outcome.into_result();
    let (node, edges, exhaustive, next, mut payload_memory) = page.into_parts();
    if usize::try_from(max_edges).map_or(true, |bound| edges.len() > bound)
        || node
            .as_ref()
            .is_some_and(|node| node.identity != anchor.value)
        || edges.iter().any(|edge| match direction {
            quanta_index_contract::native_protocol::NativeGraphDirection::Outgoing => {
                &edge.source != &anchor.value
            }
            quanta_index_contract::native_protocol::NativeGraphDirection::Incoming => {
                &edge.target != &anchor.value
            }
        })
        || next.is_some() == exhaustive
        || next
            .is_some_and(|cursor| edges.last().map(|edge| edge.cursor_start) != Some(cursor.start))
    {
        return Err(IndexError::BackendViolation(
            "imported RepoMap adjacency differs".into(),
        ));
    }
    let capacity_memory = budget.reserve_memory(
        u64::try_from(shared_value_bytes::<NativeImportedRepoMapResultV1>()?)
            .map_err(|_| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let (source, source_memory) = anchor.into_parts();
    payload_memory.push(source_memory);
    Ok(ScopedNativeResult::new(ResultValue {
        result: NativeImportedRepoMapResultV1 {
            view,
            instance,
            source,
            direction,
            node,
            edges,
            exhaustive,
            next,
        },
        payload_memory,
        _capacity_memory: capacity_memory,
    }))
}
