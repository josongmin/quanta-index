//! Borrow the History producer's retained DATA through response encoding/write.

use quanta_index_contract::{
    CandidateCountV1, HistoryQueryRequest, QueryResultWindowV2,
    index::{Digest, IndexInstanceId, ViewRef},
    native_protocol::{
        NativeHistoryCursor, NativeHistoryPageRef, NativeReadSessionId, native_history_query_digest,
    },
};
use quanta_index_core::index::IndexError;
use quanta_index_search_plane::native_history::NativeHistoryQueryResultV1;
use serde::{Serialize, Serializer};

pub(crate) struct ScopedNativeHistoryPage {
    // Rows and their original decoded grants drop together, never before write.
    result: NativeHistoryQueryResultV1,
    window: QueryResultWindowV2,
    session: NativeReadSessionId,
    query: Digest,
    order: quanta_index_contract::HistoryOrderV1,
}

impl ScopedNativeHistoryPage {
    pub(super) fn from_producer(
        result: NativeHistoryQueryResultV1,
        session: NativeReadSessionId,
        view: ViewRef,
        instance: IndexInstanceId,
        query: &HistoryQueryRequest,
        prior: Option<(
            quanta_index_contract::index::ObjectRef,
            quanta_index_contract::AuxEpochV1,
        )>,
    ) -> Result<Self, IndexError> {
        let raw = result.page();
        let digest =
            native_history_query_digest(instance, query).map_err(IndexError::InvalidInput)?;
        let count = raw
            .commits
            .len()
            .checked_add(raw.diffs.len())
            .ok_or(IndexError::ResourceLimited)?;
        let count = u32::try_from(count).map_err(|_| IndexError::ResourceLimited)?;
        if raw.view != view
            || raw.instance != instance
            || raw.remaining > raw.matched
            || raw.window.candidate_count() != CandidateCountV1::Exact(raw.remaining)
            || raw.window.returned() != count
            || u64::from(count) != raw.remaining.min(u64::from(query.text_query.top_k))
            || (prior.is_none() && raw.remaining != raw.matched)
            || (!raw.commits.is_empty() && !raw.diffs.is_empty())
            || raw.window.has_more() != raw.next_cursor.is_some()
            || raw.examined < raw.matched
        {
            return Err(IndexError::BackendViolation(
                "History producer row/window authority differs".into(),
            ));
        }
        if let Some((member, epoch)) = prior {
            if member != raw.artifact || epoch != raw.epoch {
                return Err(IndexError::InvalidInput(
                    "History cursor names another member or epoch".into(),
                ));
            }
        }
        if let Some(next) = &raw.next_cursor {
            if count == 0 || next.order.order() != query.order || next.aux_epoch != raw.epoch {
                return Err(IndexError::BackendViolation(
                    "History producer continuation authority differs".into(),
                ));
            }
        }
        let window = QueryResultWindowV2::pageable(
            count,
            CandidateCountV1::Exact(raw.remaining),
            raw.next_cursor.is_some(),
            Vec::new(),
        )
        .map_err(|error| IndexError::BackendViolation(error.into()))?;
        Ok(Self {
            result,
            window,
            session,
            query: digest,
            order: query.order,
        })
    }
}

impl Serialize for ScopedNativeHistoryPage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let raw = self.result.page();
        let page = NativeHistoryPageRef {
            session: self.session,
            view: raw.view,
            instance: raw.instance,
            member: raw.artifact,
            query: self.query,
            order: self.order,
            commits: &raw.commits,
            diffs: &raw.diffs,
            matched: raw.matched,
            remaining: raw.remaining,
            window: &self.window,
            read_epoch: raw.epoch,
            examined: raw.examined,
            after: raw.next_cursor.as_ref(),
        };
        serializer.serialize_newtype_variant("NativeIndexResponse", 39, "HistoryPage", &page)
    }
}

// Final factory name and registry mount remain supplier-bound until NC-H/Root ACK.
pub(super) fn search(
    dispatcher: &super::NativeDispatcher,
    ticket: &quanta_index_kernel::ReadTicket,
    attempt: &mut quanta_index_kernel::TicketReadAttempt<'_>,
    session: NativeReadSessionId,
    instance: IndexInstanceId,
    query: &HistoryQueryRequest,
    after: Option<NativeHistoryCursor>,
    work: &mut quanta_index_core::index::WorkBudget,
    timer: &super::RetainedSearchTimer,
    control: quanta_index_core::index::effort::ReservationGuard<super::NativeClassScheduler>,
) -> Result<ScopedNativeHistoryPage, IndexError> {
    work.bind_memory_scope(control.memory_scope());
    timer.mark(super::ReadFailureStage::QueryPreparation);
    quanta_index_contract::native_protocol::validate_native_history_query(query)
        .map_err(IndexError::InvalidInput)?;
    if let Some(cursor) = &after {
        cursor
            .validate_request(ticket.view().identity, instance, query)
            .map_err(IndexError::InvalidInput)?;
    }
    let prior = after
        .as_ref()
        .map(|cursor| (cursor.member, cursor.boundary.aux_epoch));
    let planned = quanta_index_search_plane::native_history::plan_native_history_read_v1(
        query,
        after.map(|cursor| (cursor.member, cursor.boundary)),
        work,
    )
    .map_err(|error| match work.first_denial() {
        Some(quanta_index_core::index::WorkBudgetDenial::Cancelled) => IndexError::Cancelled,
        Some(_) => IndexError::WorkLimitExceeded,
        None => IndexError::InvalidInput(error.to_string()),
    })?;
    let result = dispatcher.kernel.with_prepared_stage_for_read_attempt(
        &dispatcher.history_queries,
        ticket,
        attempt,
        instance,
        planned,
        work,
        |prepared| {
            let need = prepared.required_resources();
            drop(control);
            timer.mark(super::ReadFailureStage::InteractiveAdmission);
            let reservation = match dispatcher
                .interactive_scheduler
                .acquire_interactive_for_request(need, prepared.admission_budget())
            {
                Ok(reservation) => reservation,
                Err(error) => {
                    prepared.abort();
                    return Err(error);
                }
            };
            timer.mark(super::ReadFailureStage::QueryExecution);
            let selected = timer.is_enabled();
            let mut timings = quanta_index_kernel::QueryExecutionTimings::default();
            let result = prepared.execute_projected(
                reservation,
                selected.then_some(&mut timings),
                |outcome, _work| {
                    timer.mark(super::ReadFailureStage::ResultConversion);
                    if outcome.view() != ticket.view().identity
                        || !outcome.exhaustive()
                        || !matches!(
                            outcome.coverage(),
                            quanta_index_core::index::QueryCoverage::Core
                        )
                    {
                        return Err(IndexError::BackendViolation(
                            "History producer is not a complete same-ticket core read".into(),
                        ));
                    }
                    let result = ScopedNativeHistoryPage::from_producer(
                        outcome.into_result(),
                        session,
                        ticket.view().identity,
                        instance,
                        query,
                        prior,
                    );
                    timer.mark(super::ReadFailureStage::QueryExecution);
                    result
                },
            );
            if selected {
                timer.record_execution_timings(timings);
            }
            result?.inspect_err(|_| timer.mark(super::ReadFailureStage::ResultConversion))
        },
    )?;
    timer.mark(super::ReadFailureStage::Checkpoint);
    work.checkpoint()?;
    Ok(result)
}
