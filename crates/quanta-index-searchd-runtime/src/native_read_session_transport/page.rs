//! Exhaustive lexical rank pages under the session's original ticket.

use super::*;
use crate::native_result::{ResultValue, ScopedNativeResult, shared_value_bytes};
use quanta_index_contract::native_protocol::{
    NativeReadSessionId, NativeTextCursor, NativeTextHit, NativeTextPage, native_text_query_digest,
};
use quanta_index_core::index::{MemoryLease, QueryCoverage, QueryOutcome};
use quanta_index_lexical::native::{NativeTextHitSource, TextReadResult};

pub(crate) type ScopedNativeTextPage = ScopedNativeResult<NativeTextPage>;

pub(super) fn search(
    dispatcher: &NativeDispatcher,
    ticket: &quanta_index_kernel::ReadTicket,
    attempt: &mut quanta_index_kernel::TicketReadAttempt<'_>,
    session: NativeReadSessionId,
    instance: IndexInstanceId,
    query: &TextQueryRequest,
    after: Option<NativeTextCursor>,
    work: &mut WorkBudget,
    timer: &RetainedSearchTimer,
    control: ReservationGuard<NativeClassScheduler>,
) -> Result<ScopedNativeTextPage, IndexError> {
    work.bind_memory_scope(control.memory_scope());
    timer.mark(ReadFailureStage::QueryPreparation);
    let digest = native_text_query_digest(instance, query).map_err(IndexError::InvalidInput)?;
    if after.is_some_and(|cursor| {
        cursor.view != ticket.view().identity
            || cursor.instance != instance
            || cursor.query != digest
            || cursor.score.get() < 0.0
    }) {
        return Err(IndexError::InvalidInput(
            "text page cursor binding differs".into(),
        ));
    }
    let planned = plan_native_text(query, |bytes, lifetime| {
        work.reserve_memory(bytes, lifetime)
    })?;
    if planned.plan.options.count.is_some() {
        return Err(IndexError::InvalidInput(
            "text pages do not support count".into(),
        ));
    }
    let planned = planned.with_rank_after(after.map(|cursor| (cursor.score, cursor.owner)));
    execute_text(
        dispatcher,
        ticket,
        attempt,
        instance,
        planned,
        work,
        timer,
        control,
        |outcome, work| {
            convert(
                outcome,
                session,
                instance,
                query.top_k,
                digest,
                after,
                &query.constraints,
                work,
            )
        },
    )
}

fn convert(
    outcome: QueryOutcome<TextReadResult>,
    session: NativeReadSessionId,
    instance: IndexInstanceId,
    limit: u32,
    digest: quanta_index_contract::index::Digest,
    after: Option<NativeTextCursor>,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    work: &WorkBudget,
) -> Result<ScopedNativeTextPage, IndexError> {
    let view = outcome.view();
    let raw = outcome.result();
    let count = u64::try_from(raw.hits.len()).map_err(|_error| IndexError::ResourceLimited)?;
    if !matches!(outcome.coverage(), QueryCoverage::Core)
        || !outcome.exhaustive()
        || raw.remaining > raw.matched
        || count != raw.remaining.min(u64::from(limit))
        || (after.is_none() && raw.remaining != raw.matched)
    {
        return Err(IndexError::BackendViolation(
            "text page scan or artifact differs".into(),
        ));
    }
    if after.is_some_and(|cursor| cursor.artifact != raw.artifact) {
        return Err(IndexError::InvalidInput(
            "text page cursor artifact differs".into(),
        ));
    }
    let owner_bytes = raw
        .hits
        .len()
        .checked_mul(std::mem::size_of::<quanta_index_contract::index::OwnerKey>())
        .ok_or(IndexError::ResourceLimited)?;
    let _owner_memory = if owner_bytes == 0 {
        None
    } else {
        work.reserve_memory(
            u64::try_from(owner_bytes).map_err(|_error| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?
    };
    // Scratch drops before its temporary allocation grant on every return.
    let mut owners = Vec::new();
    owners
        .try_reserve_exact(raw.hits.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut prior = after.map(|cursor| (cursor.score, cursor.owner));
    for hit in &raw.hits {
        if !crate::native::valid_native_source_path(&hit.path)
            || constraints
                .repo_relative_path_exact
                .as_ref()
                .is_some_and(|expected| expected.as_str() != hit.path)
            || hit.score.get() < 0.0
            || prior.is_some_and(|(score, owner)| {
                score.get().total_cmp(&hit.score.get()).is_lt()
                    || (score == hit.score && owner >= hit.owner)
            })
        {
            return Err(IndexError::BackendViolation(
                "text page rank, path or predicate differs".into(),
            ));
        }
        owners.push(hit.owner);
        prior = Some((hit.score, hit.owner));
    }
    owners.sort_unstable();
    if owners.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(IndexError::BackendViolation(
            "text page repeated source owner".into(),
        ));
    }
    let capacity = raw
        .hits
        .len()
        .checked_mul(
            std::mem::size_of::<NativeTextHit>() + std::mem::size_of::<Option<MemoryLease>>(),
        )
        .and_then(|bytes| bytes.checked_add(shared_value_bytes::<NativeTextPage>().ok()?))
        .ok_or(IndexError::ResourceLimited)?;
    let memory = work.reserve_memory(
        u64::try_from(capacity).map_err(|_error| IndexError::ResourceLimited)?,
        MemoryLifetime::Retained,
    )?;
    let mut hits = Vec::new();
    let mut leases = Vec::new();
    hits.try_reserve_exact(raw.hits.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    leases
        .try_reserve_exact(raw.hits.len())
        .map_err(|_error| IndexError::ResourceLimited)?;
    let mut page = ResultValue {
        result: NativeTextPage {
            session,
            view,
            instance,
            artifact: raw.artifact,
            matched: raw.matched,
            remaining: raw.remaining,
            hits,
            after: None,
        },
        payload_memory: leases,
        _capacity_memory: memory,
    };
    let (_, _, rows, input_memory) = outcome.into_result().into_parts();
    for hit in rows {
        let parts = hit.into_parts();
        // Locals unwind in reverse order: destroy the path before its grant.
        let lease = parts.4;
        let path = parts.2;
        let (owner, origin, score) = (parts.0, parts.1, parts.3);
        let NativeTextHitSource::Durable(source) = origin else {
            return Err(IndexError::BackendViolation(
                "text page returned an overlay".into(),
            ));
        };
        page.result.hits.push(NativeTextHit {
            owner,
            source,
            path,
            score,
        });
        page.payload_memory.push(lease);
    }
    if page.result.remaining > count {
        let last = page.result.hits.last().ok_or_else(|| {
            IndexError::BackendViolation("text page lost continuation anchor".into())
        })?;
        page.result.after = Some(NativeTextCursor {
            view,
            instance,
            query: digest,
            artifact: page.result.artifact,
            score: last.score,
            owner: last.owner,
        });
    }
    drop(input_memory);
    Ok(ScopedNativeResult::new(page))
}
