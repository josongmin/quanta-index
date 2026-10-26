//! One atomic overlay corpus evaluated by the selected native text reader.

use crate::{
    native::{NativeDispatcher, plan_native_text},
    native_overlay::{OverlayCorpusSnapshot, OverlaySessionKey},
    native_result::{ResultValue, ScopedNativeResult, shared_value_bytes},
    native_text_result,
};
use quanta_index_contract::{
    TextQueryRequest,
    index::IndexInstanceId,
    native_protocol::{
        NativeOverlayCorpusStamp, NativeOverlayRevision, NativeOverlayTextHit,
        NativeOverlayTextQuery, NativeOverlayTextResult, NativeOverlayTextSource,
    },
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        IndexError, MemoryLease, MemoryLifetime, QueryOutcome, WorkBudget, effort::ReservationGuard,
    },
};
use quanta_index_ipc::DispatchContextV1;
use quanta_index_lexical::native::{
    NativeTextHitSource, PlannedTextRead, TextOverlayReadDocument, TextReadResult,
};
use std::sync::Arc;

pub type ScopedOverlayTextResult = ScopedNativeResult<NativeOverlayTextResult>;

pub(super) fn plan(
    snapshot: &OverlayCorpusSnapshot,
    query: &TextQueryRequest,
    budget: &WorkBudget,
) -> Result<PlannedTextRead, IndexError> {
    plan_with_reservation(snapshot, query, |bytes, lifetime| {
        budget.reserve_memory(bytes, lifetime)
    })
}

pub(super) fn plan_with_reservation(
    snapshot: &OverlayCorpusSnapshot,
    query: &TextQueryRequest,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<PlannedTextRead, IndexError> {
    let count = snapshot.documents.iter().filter(|row| !row.retired).count();
    let bytes = count
        .checked_mul(std::mem::size_of::<TextOverlayReadDocument>())
        .ok_or(IndexError::ResourceLimited)?;
    let memory = if bytes == 0 {
        None
    } else {
        reserve(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?
    };
    let mut documents = Vec::new();
    documents
        .try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if documents.capacity() > count {
        return Err(IndexError::ResourceLimited);
    }
    for row in snapshot.documents.iter().filter(|row| !row.retired) {
        documents.push(TextOverlayReadDocument {
            owner: row.owner,
            revision: row.revision,
            text: row.text.clone(),
            path: row.path.clone(),
        });
    }
    Ok(plan_native_text(query, &reserve)?.with_overlay(documents, memory))
}

pub(super) fn stamp(
    snapshot: &OverlayCorpusSnapshot,
    reserve: &impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<(NativeOverlayCorpusStamp, Option<MemoryLease>), IndexError> {
    let bytes = snapshot
        .documents
        .len()
        .checked_mul(std::mem::size_of::<NativeOverlayRevision>())
        .ok_or(IndexError::ResourceLimited)?;
    let memory = if bytes == 0 {
        None
    } else {
        reserve(
            u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Retained,
        )?
    };
    let mut revisions = Vec::new();
    revisions
        .try_reserve_exact(snapshot.documents.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    if revisions.capacity() > snapshot.documents.len() {
        return Err(IndexError::ResourceLimited);
    }
    for row in &snapshot.documents {
        revisions.push(NativeOverlayRevision {
            owner: row.owner,
            revision: row.revision,
            deleted: row.text.is_none(),
            retired: row.retired,
        });
    }
    Ok((
        NativeOverlayCorpusStamp {
            session: snapshot.session.wire,
            identity: snapshot.identity,
            revisions,
        },
        memory,
    ))
}

pub(super) fn convert(
    outcome: QueryOutcome<TextReadResult>,
    instance: IndexInstanceId,
    limit: u32,
    snapshot: &OverlayCorpusSnapshot,
    reserve: impl Fn(u64, MemoryLifetime) -> Result<Option<MemoryLease>, IndexError>,
) -> Result<ScopedOverlayTextResult, IndexError> {
    if outcome.view().workspace != snapshot.session.workspace {
        return Err(IndexError::BackendViolation(
            "overlay query selected scope differs".into(),
        ));
    }
    native_text_result::validate_read(&outcome, limit, &reserve)?;
    let view = outcome.view();
    let result = outcome.into_result();
    let count = result.hits.len();
    let leases = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
    let value_bytes = shared_value_bytes::<NativeOverlayTextResult>()?;
    let bytes = count
        .checked_mul(std::mem::size_of::<NativeOverlayTextHit>())
        .and_then(|v| {
            leases
                .checked_mul(std::mem::size_of::<Option<MemoryLease>>())
                .and_then(|m| v.checked_add(m))
        })
        .and_then(|v| v.checked_add(value_bytes))
        .ok_or(IndexError::ResourceLimited)?;
    let memory = reserve(
        u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let stamped = stamp(snapshot, &reserve)?;
    // The stamp's owned Vec must drop before its retained capacity grant on
    // every fallible branch below.
    let corpus_memory = stamped.1;
    let corpus = stamped.0;
    let mut hits = Vec::new();
    hits.try_reserve_exact(count)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if hits.capacity() > count {
        return Err(IndexError::ResourceLimited);
    }
    let mut paths = Vec::new();
    paths
        .try_reserve_exact(leases)
        .map_err(|_error| IndexError::ResourceLimited)?;
    if paths.capacity() > leases {
        return Err(IndexError::ResourceLimited);
    }
    // Preserve the input hit container's grant until its iterator is gone,
    // including panic unwinding while output rows are being converted.
    let parts = result.into_parts();
    let old_memory = parts.3;
    let input_hits = parts.2;
    let artifact = parts.0;
    let matched = parts.1;
    let mut output = ResultValue {
        result: NativeOverlayTextResult {
            view,
            instance,
            artifact,
            matched,
            exhaustive: true,
            corpus,
            hits,
        },
        payload_memory: paths,
        _capacity_memory: memory,
    };
    output.payload_memory.push(corpus_memory);
    for hit in input_hits {
        let (owner, source, path, score, path_memory) = hit.into_parts();
        let source = match source {
            NativeTextHitSource::Durable(source) => NativeOverlayTextSource::Durable(source),
            NativeTextHitSource::Overlay { revision } => {
                NativeOverlayTextSource::Overlay { revision }
            }
        };
        output.result.hits.push(NativeOverlayTextHit {
            owner,
            source,
            path,
            score,
        });
        output.payload_memory.push(path_memory);
    }
    drop(old_memory);
    Ok(ScopedNativeResult::new(output))
}

pub(super) fn query(
    dispatcher: &NativeDispatcher,
    context: &DispatchContextV1,
    request: &NativeOverlayTextQuery,
    request_budget: &RequestBudgetV1,
) -> Result<ScopedOverlayTextResult, IndexError> {
    let overlay = dispatcher
        .overlay
        .get()
        .ok_or_else(|| IndexError::InvalidInput("Native overlay is not configured".into()))?;
    if request.instance != overlay.text_instance {
        return Err(IndexError::InvalidInput(
            "overlay text instance differs".into(),
        ));
    }
    let key = OverlaySessionKey::from_dispatch(request.session, context)?;
    let mut budget = WorkBudget::with_request(dispatcher.limits, request_budget.clone());
    let (snapshot, prepared, limit) = {
        let capture = ReservationGuard::try_acquire(
            Arc::clone(&dispatcher.interactive_scheduler),
            dispatcher.interactive_need,
        )
        .ok_or_else(|| dispatcher.interactive_scheduler.denial())?;
        budget.bind_memory_scope(capture.memory_scope());
        let snapshot = overlay.snapshot_corpus(key, &mut budget)?;
        if request
            .expected_corpus
            .is_some_and(|identity| identity != snapshot.identity)
        {
            return Err(IndexError::Conflict);
        }
        let planned = plan(&snapshot, &request.query, &budget)?;
        let limit = planned.result_limit()?;
        let prepared = dispatcher.kernel.prepare_query(
            &dispatcher.text_queries,
            request.selection,
            request.instance,
            planned,
            &mut budget,
        )?;
        (snapshot, prepared, limit)
    };
    let Some(reservation) = ReservationGuard::try_acquire(
        Arc::clone(&dispatcher.interactive_scheduler),
        prepared.required_resources(),
    ) else {
        let denial = dispatcher.interactive_scheduler.denial();
        prepared.abort()?;
        return Err(denial);
    };
    let outcome = prepared.execute(&reservation)?;
    overlay.validate_corpus(&snapshot, &budget)?;
    let output = convert(
        outcome,
        request.instance,
        limit,
        &snapshot,
        |bytes, lifetime| budget.reserve_memory(bytes, lifetime),
    )?;
    overlay.validate_corpus(&snapshot, &budget)?;
    budget.checkpoint()?;
    Ok(output)
}
