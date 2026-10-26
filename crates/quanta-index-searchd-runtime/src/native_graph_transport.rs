//! Ticket-bound exact graph adjacency route and result allocation custody.

use std::sync::Arc;

use quanta_index_contract::{
    ChunkId, FileId, RepoMapModuleId, RepoMapNodeRef, SymbolId,
    index::{IndexInstanceId, ViewSelection},
    native_protocol::{
        MAX_NATIVE_GRAPH_EDGES, MAX_NATIVE_GRAPH_ITEMS, NativeGraphDirection, NativeGraphQuery,
        NativeGraphResult,
    },
};
use quanta_index_core::index::{
    IndexError, MemoryLease, MemoryLifetime, QueryCoverage, QueryOutcome, WorkBudget,
    effort::ReservationGuard,
};
use quanta_index_repomap::native::GraphPage;

use crate::{
    native::NativeDispatcher,
    native_result::{ResultValue, ScopedNativeResult, shared_value_bytes},
};

pub(crate) type ScopedNativeGraphResult = ScopedNativeResult<NativeGraphResult>;

struct GraphAnchor {
    value: RepoMapNodeRef,
    _memory: Option<MemoryLease>,
}

impl GraphAnchor {
    fn capture(source: &RepoMapNodeRef, budget: &WorkBudget) -> Result<Self, IndexError> {
        let text = match source {
            RepoMapNodeRef::File(id) => id.as_str(),
            RepoMapNodeRef::Module(id) => id.as_str(),
            RepoMapNodeRef::Symbol(id) => id.as_str(),
            RepoMapNodeRef::Chunk(id) => id.as_str(),
        };
        let bytes = text
            .len()
            .checked_add(std::mem::size_of::<RepoMapNodeRef>())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = budget.reserve_memory(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?;
        let mut copied = String::new();
        copied
            .try_reserve_exact(text.len())
            .map_err(|_error| IndexError::ResourceLimited)?;
        copied.push_str(text);
        let value = match source {
            RepoMapNodeRef::File(_) => RepoMapNodeRef::File(FileId::new(copied)),
            RepoMapNodeRef::Module(_) => RepoMapNodeRef::Module(RepoMapModuleId::new(copied)),
            RepoMapNodeRef::Symbol(_) => RepoMapNodeRef::Symbol(SymbolId::new(copied)),
            RepoMapNodeRef::Chunk(_) => RepoMapNodeRef::Chunk(ChunkId::new(copied)),
        };
        Ok(Self {
            value,
            _memory: memory,
        })
    }
}

pub(super) fn query(
    dispatcher: &NativeDispatcher,
    selection: ViewSelection,
    instance: IndexInstanceId,
    query: NativeGraphQuery,
    budget: &mut WorkBudget,
) -> Result<ScopedNativeGraphResult, IndexError> {
    let registry = dispatcher
        .graph
        .get()
        .ok_or(IndexError::SnapshotUnavailable)?;
    let max_supporters = query.max_supporters;
    let max_edges = query.max_edges;
    let direction = query.direction;
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
    let result = convert(
        outcome,
        instance,
        max_supporters,
        max_edges,
        direction,
        &anchor.value,
        |bytes, lifetime| budget.reserve_memory(bytes, lifetime),
    )?;
    budget.checkpoint()?;
    Ok(result)
}

fn convert(
    outcome: QueryOutcome<GraphPage>,
    instance: IndexInstanceId,
    max_supporters: u32,
    max_edges: u32,
    direction: NativeGraphDirection,
    expected_source: &RepoMapNodeRef,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<ScopedNativeGraphResult, IndexError> {
    if !matches!(outcome.coverage(), QueryCoverage::Core) || !outcome.exhaustive() {
        return Err(IndexError::BackendViolation(
            "native graph reader did not return exhaustive core coverage".into(),
        ));
    }
    let view = outcome.view();
    // Keep the page intact on every fallible path: its data fields drop
    // before its retained grants, including a refused canonical reply slot.
    let page = outcome.into_result();
    let edge_supporters = page.edges.iter().try_fold(0_u32, |sum, row| {
        sum.checked_add(row.supporters)
            .ok_or(IndexError::ResourceLimited)
    })?;
    if max_supporters == 0
        || usize::try_from(max_supporters).map_or(true, |count| count > MAX_NATIVE_GRAPH_ITEMS)
        || max_edges == 0
        || max_edges > MAX_NATIVE_GRAPH_EDGES
        || page.source_supporters > max_supporters
        || page.source.is_some() != (page.source_supporters > 0)
        || page
            .source
            .as_ref()
            .is_some_and(|node| &node.identity != expected_source)
        || usize::try_from(max_edges).map_or(true, |bound| page.edges.len() > bound)
        || usize::try_from(edge_supporters).map_or(true, |count| count > MAX_NATIVE_GRAPH_ITEMS)
        || page.edges.iter().any(|row| {
            row.supporters == 0
                || row.supporters > max_supporters
                || page.source.is_none()
                || direction.endpoint(&row.edge) != expected_source
        })
    {
        return Err(IndexError::BackendViolation(
            "native graph reader returned inconsistent adjacency".into(),
        ));
    }
    for (ordinal, row) in page.edges.iter().enumerate() {
        let preceding = page.edges.get(..ordinal).ok_or_else(|| {
            IndexError::BackendViolation("native graph adjacency order exceeds bounds".into())
        })?;
        if preceding.iter().any(|prior| prior.edge == row.edge) {
            return Err(IndexError::BackendViolation(
                "native graph reader returned duplicate adjacency".into(),
            ));
        }
    }
    let capacity_memory = reserve(
        u64::try_from(shared_value_bytes::<NativeGraphResult>()?)
            .map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let (source, source_supporters, edges, payload_memory) = page.into_parts();
    let value = ResultValue {
        result: NativeGraphResult {
            view,
            instance,
            source,
            source_supporters,
            edges,
            exhaustive: true,
        },
        payload_memory,
        _capacity_memory: capacity_memory,
    };
    Ok(ScopedNativeResult::new(value))
}
