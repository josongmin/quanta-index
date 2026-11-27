//! Exhaustive lexical rank pages under the session's original ticket.
//!
//! The first page of a `(view, instance, query)` also captures the full rank
//! order and keeps it in the session. A later page of the same identity is
//! answered from it: the cursor is binary-searched and only the page's rows
//! are projected, instead of re-ranking the whole union. Without the cached
//! order (disabled, evicted, over its bound or unfunded), a page re-ranks.

use super::*;
use crate::native_read_sessions::{NativeReadSessions, TextOrderKey};
use crate::native_result::{ResultValue, ScopedNativeResult, shared_value_bytes};
use quanta_index_contract::native_protocol::{
    NativeReadSessionId, NativeTextCursor, NativeTextHit, NativeTextPage, NativeTextPathCursorV1,
    native_text_query_digest,
};
use quanta_index_core::index::{MemoryLease, QueryCoverage, QueryOutcome};
use quanta_index_lexical::native::{NativeTextHitSource, TextReadResult};

pub(crate) type ScopedNativeTextPage = ScopedNativeResult<NativeTextPage>;

/// The session's text order cache, as seen by one page request.
pub(super) struct SessionTextOrders<'a> {
    pub(super) sessions: &'a NativeReadSessions,
    pub(super) uid: u32,
    pub(super) session: NativeReadSessionId,
}

impl SessionTextOrders<'_> {
    fn cached(
        &self,
        key: TextOrderKey,
        evict_on_miss: bool,
        budget: &WorkBudget,
    ) -> Result<Option<Arc<quanta_index_lexical::native::TextRankOrder>>, IndexError> {
        self.sessions
            .cached_text_order(self.uid, self.session, key, evict_on_miss, budget)
    }

    /// Keep a captured order on the registry's text order share.
    fn retain(
        &self,
        key: TextOrderKey,
        order: quanta_index_lexical::native::TextRankOrder,
        budget: &WorkBudget,
    ) -> Result<(), IndexError> {
        self.sessions
            .retain_text_order(self.uid, self.session, key, order, budget)
    }
}

/// Apply the session's cached order, or capture one on a first page.
fn order_mode(
    planned: quanta_index_lexical::native::PlannedTextRead,
    orders: &SessionTextOrders<'_>,
    key: TextOrderKey,
    first_page: bool,
    budget: &WorkBudget,
) -> Result<quanta_index_lexical::native::PlannedTextRead, IndexError> {
    if !orders.sessions.text_order_cache_enabled() {
        return Ok(planned);
    }
    if let Some(order) = orders.cached(key, first_page, budget)? {
        return Ok(planned.with_cached_order(order));
    }
    Ok(if first_page {
        planned.with_session_order_capture(orders.sessions.text_order_capture_port())
    } else {
        planned
    })
}
pub(super) fn search(
    dispatcher: &NativeDispatcher,
    ticket: &quanta_index_kernel::ReadTicket,
    attempt: &mut quanta_index_kernel::TicketReadAttempt<'_>,
    session: NativeReadSessionId,
    instance: IndexInstanceId,
    query: &TextQueryRequest,
    after: Option<NativeTextCursor>,
    orders: &SessionTextOrders<'_>,
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
    let key = TextOrderKey {
        view: ticket.view().identity,
        instance,
        query: digest,
        path: false,
    };
    let planned = order_mode(
        planned.with_rank_after(after.map(|cursor| (cursor.score, cursor.owner))),
        orders,
        key,
        after.is_none(),
        work,
    )?;
    execute_text(
        dispatcher,
        ticket,
        attempt,
        instance,
        planned,
        work,
        timer,
        control,
        |mut outcome, work| {
            let order = outcome.result_mut().take_order();
            let page = convert(
                outcome,
                session,
                instance,
                query.top_k,
                digest,
                after,
                &query.constraints,
                work,
            )?;
            if let Some(order) = order {
                orders.retain(key, order, work)?;
            }
            Ok(page)
        },
    )
}

pub(super) fn search_paths(
    dispatcher: &NativeDispatcher,
    ticket: &quanta_index_kernel::ReadTicket,
    attempt: &mut quanta_index_kernel::TicketReadAttempt<'_>,
    session: NativeReadSessionId,
    instance: IndexInstanceId,
    query: &TextQueryRequest,
    after: Option<NativeTextPathCursorV1>,
    orders: &SessionTextOrders<'_>,
    work: &mut WorkBudget,
    timer: &RetainedSearchTimer,
    control: ReservationGuard<NativeClassScheduler>,
) -> Result<crate::native_text_result::ScopedNativeTextPathPage, IndexError> {
    work.bind_memory_scope(control.memory_scope());
    timer.mark(ReadFailureStage::QueryPreparation);
    let digest = native_text_query_digest(instance, query).map_err(IndexError::InvalidInput)?;
    if after.as_ref().is_some_and(|cursor| {
        cursor.view != ticket.view().identity
            || cursor.instance != instance
            || cursor.query != digest
            || cursor.score.get() < 0.0
            || !quanta_index_contract::index::is_canonical_source_path(&cursor.path)
            || query
                .constraints
                .repo_relative_path_exact
                .as_ref()
                .is_some_and(|expected| expected.as_str() != cursor.path)
    }) {
        return Err(IndexError::InvalidInput(
            "path page cursor binding differs".into(),
        ));
    }
    let planned = plan_native_projected_text(query, |bytes, lifetime| {
        work.reserve_memory(bytes, lifetime)
    })?;
    if planned.plan.filters.surface != quanta_index_lexical::symbol::ResultSurface::Path
        || planned.plan.options.count.is_some()
    {
        return Err(IndexError::InvalidInput(
            "path pages require select:path without count".into(),
        ));
    }
    let planned = if let Some(cursor) = &after {
        let bytes = cursor
            .path
            .len()
            .checked_add(cursor.repository.as_str().len())
            .and_then(|bytes| u64::try_from(bytes).ok())
            .ok_or(IndexError::ResourceLimited)?;
        let memory = work.reserve_memory(bytes, MemoryLifetime::Retained)?;
        planned.with_path_rank_after_v1(
            Some((cursor.score, cursor.repository.clone(), cursor.path.clone())),
            memory,
        )
    } else {
        planned
    };
    let key = TextOrderKey {
        view: ticket.view().identity,
        instance,
        query: digest,
        path: true,
    };
    let planned = order_mode(planned, orders, key, after.is_none(), work)?;
    let mut projected = execute_text(
        dispatcher,
        ticket,
        attempt,
        instance,
        planned,
        work,
        timer,
        control,
        |mut outcome, work| {
            let order = outcome.result_mut().take_order();
            let page = crate::native_text_result::convert_path_page(
                outcome,
                session,
                instance,
                query.top_k,
                digest,
                after.as_ref(),
                |bytes, lifetime| work.reserve_memory(bytes, lifetime),
            )?;
            if let Some(order) = order {
                orders.retain(key, order, work)?;
            }
            Ok(page)
        },
    )?;
    // Each row's CodeSearch match comes from the scan; its excerpt is read
    // from the row's Source under this session's retained ticket. Both
    // execution reservations have ended, so this finalization is funded
    // like the response it becomes.
    timer.mark(ReadFailureStage::ResultConversion);
    work.bind_memory_scope(Some(super::attempt_memory(&dispatcher.resources)?));
    crate::native_code_search_excerpt::attach_code_search_matches(
        &mut projected.value,
        &projected.witnesses,
        dispatcher.objects.as_ref(),
        work,
    )?;
    timer.mark(ReadFailureStage::Checkpoint);
    Ok(crate::native_result::ScopedNativeResult::new(
        projected.value,
    ))
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
        || (after.is_none() != (raw.remaining == raw.matched))
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
        if !quanta_index_contract::index::is_canonical_source_path(&hit.path)
            || constraints
                .repo_relative_path_exact
                .as_ref()
                .is_some_and(|expected| expected.as_str() != hit.path)
            || hit.score.get() < 0.0
            || after.is_some_and(|cursor| hit.owner == cursor.owner)
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
