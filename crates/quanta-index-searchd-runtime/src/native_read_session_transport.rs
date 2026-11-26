//! Credential-bound product read leases over the existing Native control plane.

use std::sync::Arc;

mod dsl;
pub(crate) use dsl::ScopedNativeDslPage;
pub(super) use dsl::{NativeDslCommitValidator, NativeDslCorpusCache};
mod history;
pub(crate) use history::ScopedNativeHistoryPage;
mod hybrid;
mod page;
pub(crate) use page::ScopedNativeTextPage;

use quanta_index_contract::{
    TextQueryRequest,
    index::IndexInstanceId,
    native_protocol::{
        NativeIndexResponse, NativeReadSessionReply, NativeReadSessionRequest,
        NativeReadSessionSettlement, NativeReadSettlementState,
    },
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{
        IndexError, MemoryLease, MemoryLifetime, MemoryScope, MemoryScopePort,
        NativeReadSettlementScope, NativeReadSettlementStatus, WorkBudget,
        effort::ReservationGuard,
    },
};
use quanta_index_ipc::DispatchContextV1;

use crate::{
    native::{NativeDispatcher, plan_native_projected_text, plan_native_text},
    native_reply::NativeReply,
    native_resources::{NativeClassScheduler, NativeResourceLedger},
    native_work_diagnostic::{
        self, ReadFailureRoute, ReadFailureStage, RetainedSearchTimer, SessionCustodyPart,
    },
};

// Catalog observations may finish after Control and Interactive are returned.
// Fund their short retained custody in the same ledger and protected envelope
// used by transport responses, without retaining an execution reservation.
struct ReadAttemptMemory {
    resources: Arc<NativeResourceLedger>,
    _backing: MemoryLease,
}

impl MemoryScopePort for ReadAttemptMemory {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        // Even attempt-local Temporary data is conservatively detached here:
        // its request spans both execution reservations and finalization.
        self.resources.reserve_response_memory(bytes)
    }
}

fn attempt_memory(resources: &Arc<NativeResourceLedger>) -> Result<MemoryScope, IndexError> {
    let bytes = std::mem::size_of::<ReadAttemptMemory>()
        .checked_add(std::mem::size_of::<[usize; 2]>())
        .and_then(|bytes| bytes.checked_add(16))
        .ok_or(IndexError::ResourceLimited)?;
    let bytes = u64::try_from(bytes).map_err(|_error| IndexError::ResourceLimited)?;
    let backing = resources.reserve_response_memory(bytes)?;
    Ok(MemoryScope::new(Arc::new(ReadAttemptMemory {
        resources: Arc::clone(resources),
        _backing: backing,
    })))
}

fn scope(ticket: &quanta_index_kernel::ReadTicket) -> NativeReadSettlementScope {
    let target = ticket.view().identity;
    NativeReadSettlementScope {
        workspace: target.workspace,
        epoch: target.epoch,
        target,
    }
}

pub(super) fn dispatch(
    dispatcher: &NativeDispatcher,
    context: &DispatchContextV1,
    request: NativeReadSessionRequest,
    budget: &RequestBudgetV1,
) -> Result<NativeReply, IndexError> {
    let uid = context
        .principal
        .ok_or_else(|| {
            IndexError::InvalidInput("read session has no authenticated principal".into())
        })?
        .uid;
    let sessions = dispatcher.read_sessions.get().ok_or_else(|| {
        IndexError::InvalidInput("product read sessions are not configured".into())
    })?;
    let mut gate = WorkBudget::with_request(dispatcher.limits, budget.clone());
    if matches!(
        &request,
        NativeReadSessionRequest::SearchText { .. }
            | NativeReadSessionRequest::SearchTextPage { .. }
            | NativeReadSessionRequest::SearchTextPathPageV1 { .. }
            | NativeReadSessionRequest::SearchHybridFile { .. }
            | NativeReadSessionRequest::SearchDslV1 { .. }
            | NativeReadSessionRequest::SearchHistory { .. }
            | NativeReadSessionRequest::ReadHistoryCut { .. }
            | NativeReadSessionRequest::ReadHistoryGeneration { .. }
            | NativeReadSessionRequest::ReadProfile { .. }
    ) {
        let session = match &request {
            NativeReadSessionRequest::SearchText { session, .. }
            | NativeReadSessionRequest::SearchTextPage { session, .. }
            | NativeReadSessionRequest::SearchTextPathPageV1 { session, .. }
            | NativeReadSessionRequest::SearchHybridFile { session, .. }
            | NativeReadSessionRequest::SearchDslV1 { session, .. }
            | NativeReadSessionRequest::SearchHistory { session, .. }
            | NativeReadSessionRequest::ReadHistoryCut { session, .. }
            | NativeReadSessionRequest::ReadHistoryGeneration { session, .. }
            | NativeReadSessionRequest::ReadProfile { session } => *session,
            _ => {
                return Err(IndexError::BackendViolation(
                    "retained request kind differs".into(),
                ));
            }
        };
        gate.bind_memory_scope(Some(attempt_memory(&dispatcher.resources)?));
        let mut work = WorkBudget::with_request(dispatcher.limits, budget.clone());
        let timer = RetainedSearchTimer::new(context);
        let ticket_observation = timer
            .is_enabled()
            .then(|| {
                quanta_index_catalog::TicketValidationObservation::begin(
                    &dispatcher.catalog,
                    Some(quanta_index_searchd::app::thread_cpu_now_ns),
                )
            })
            .flatten();
        let result = sessions.with_ticket(
            uid,
            session,
            &gate,
            || {
                timer.mark(ReadFailureStage::ControlAdmission);
                let control = dispatcher
                    .control_scheduler
                    .acquire_control_for_request(dispatcher.control_need, &gate)?;
                timer.mark_session_custody(SessionCustodyPart::EntryPre);
                Ok(control)
            },
            |ticket, attempt, control| {
                let result = match request {
                    NativeReadSessionRequest::ReadProfile { .. } => {
                        timer.mark(ReadFailureStage::QueryPreparation);
                        // Profile DATA survives execution admission and finalization,
                        // so its canonical loader uses detached response custody.
                        let result = (|| {
                            work.bind_memory_scope(Some(attempt_memory(&dispatcher.resources)?));
                            dispatcher
                                .kernel
                                .read_session_profile_for_attempt(ticket, attempt, &mut work)
                        })();
                        drop(control);
                        result.map(|profile| NativeReply::ReadSessionProfile { session, profile })
                    }
                    NativeReadSessionRequest::ReadHistoryCut { instance, .. } => {
                        timer.mark(ReadFailureStage::QueryPreparation);
                        let result = (|| {
                            work.bind_memory_scope(Some(attempt_memory(&dispatcher.resources)?));
                            dispatcher.kernel.with_history_read_context_for_attempt(
                                ticket,
                                attempt,
                                instance,
                                &mut work,
                                |context| quanta_index_search_plane::native_history::read_native_history_cut_v1(context),
                            )
                        })();
                        drop(control);
                        result.map(|(member, cut, memory)| NativeReply::ReadSessionHistoryCut {
                            value: quanta_index_contract::native_protocol::NativeReadSessionHistoryCutV1 {
                                session,
                                view: ticket.view().identity,
                                instance,
                                member,
                                cut,
                            },
                            _memory: memory,
                        })
                    }

                    NativeReadSessionRequest::ReadHistoryGeneration { instance, .. } => {
                        timer.mark(ReadFailureStage::QueryPreparation);
                        let result = (|| {
                            let slot = NativeReply::reserve_canonical(|bytes| {
                                dispatcher.resources.reserve_response_memory(bytes).map(Some)
                            })?;
                            work.bind_memory_scope(Some(attempt_memory(&dispatcher.resources)?));
                            let (member, source_input, inputs, predecessor_context) = dispatcher.kernel.with_history_read_context_for_attempt(
                                ticket, attempt, instance, &mut work,
                                quanta_index_search_plane::native_history::read_native_history_generation_v1,
                            )?;
                            Ok(slot.finish(NativeIndexResponse::HistoryGeneration(
                                quanta_index_contract::native_protocol::NativeReadSessionHistoryGenerationV1 {
                                    session, view: ticket.view().identity, instance,
                                    member, source_input, inputs, predecessor_context,
                                },
                            )))
                        })();
                        drop(control);
                        result
                    }
                    NativeReadSessionRequest::SearchText {
                        instance, query, ..
                    } => search(
                        dispatcher, ticket, attempt, instance, &query, &mut work, &timer, control,
                    ),
                    NativeReadSessionRequest::SearchTextPage {
                        instance,
                        query,
                        after,
                        ..
                    } => page::search(
                        dispatcher, ticket, attempt, session, instance, &query, after, &mut work,
                        &timer, control,
                    )
                    .map(NativeReply::TextPage),
                    NativeReadSessionRequest::SearchTextPathPageV1 { instance, query, after, .. } => {
                        page::search_paths(
                            dispatcher, ticket, attempt, session, instance, &query, after, &mut work,
                            &timer, control,
                        ).map(NativeReply::TextPathPage)
                    }
                    NativeReadSessionRequest::SearchDslV1 { query, .. } => {
                        drop(control);
                        let need = quanta_index_core::index::effort::ResourceNeed {
                            provider_units: 0,
                            ..dispatcher.interactive_need
                        };
                        timer.mark(ReadFailureStage::InteractiveAdmission);
                        let reservation = dispatcher.interactive_scheduler
                            .acquire_interactive_for_request(need, &work)?;
                        work.bind_memory_scope(reservation.memory_scope());
                        dsl::search(dispatcher, ticket, attempt, session, &query, &mut work, &timer)
                    }
                    NativeReadSessionRequest::SearchHistory {
                        instance,
                        query,
                        after,
                        ..
                    } => history::search(
                        dispatcher, ticket, attempt, session, instance, &query, after, &mut work,
                        &timer, control,
                    )
                    .map(NativeReply::HistoryPage),
                    NativeReadSessionRequest::SearchHybridFile {
                        text_instance,
                        semantic_instance,
                        text_query,
                        semantic_query,
                        file_top_k,
                        ..
                    } => hybrid::search(
                        dispatcher,
                        ticket,
                        attempt,
                        session,
                        text_instance,
                        semantic_instance,
                        &text_query,
                        semantic_query,
                        file_top_k,
                        &mut work,
                        &timer,
                        control,
                    )
                    .map(NativeReply::HybridFile),
                    _ => Err(IndexError::BackendViolation(
                        "retained request kind differs".into(),
                    )),
                };
                if result.is_ok() {
                    timer.mark_session_custody(SessionCustodyPart::ExitPost);
                }
                result
            },
        );
        if let Some(timings) = ticket_observation.and_then(|observation| observation.snapshot()) {
            timer.record_ticket_validation_timings(&timings);
        }
        let observed_budget = if work.first_denial().is_some() {
            &work
        } else {
            &gate
        };
        timer.record_result(context, observed_budget, result.as_ref().err());
        if let Err(error) = &result {
            native_work_diagnostic::record_read_error_if_enabled(
                context,
                ReadFailureRoute::RetainedSearch,
                timer.stage(),
                observed_budget,
                error,
            );
        }
        return result;
    }
    let mut work = WorkBudget::with_request(dispatcher.limits, budget.clone());
    let observe = |stage, work: &WorkBudget, error: &IndexError| {
        native_work_diagnostic::record_read_error_if_enabled(
            context,
            ReadFailureRoute::RetainedControl,
            stage,
            work,
            error,
        );
    };
    let control = dispatcher
        .control_scheduler
        .acquire_control_for_request(dispatcher.control_need, &work)
        .inspect_err(|error| observe(ReadFailureStage::ControlAdmission, &work, error))?;
    let slot = NativeReply::reserve_canonical(|bytes| {
        dispatcher
            .resources
            .reserve_response_memory(bytes)
            .map(Some)
    })
    .inspect_err(|error| observe(ReadFailureStage::ReplyReservation, &work, error))?;
    work.bind_memory_scope(control.memory_scope());
    work.checkpoint()
        .inspect_err(|error| observe(ReadFailureStage::Checkpoint, &work, error))?;
    let reply = (|| -> Result<NativeReadSessionReply, IndexError> {
        Ok(match request {
            NativeReadSessionRequest::Open {
                selection,
                open_key,
            } => NativeReadSessionReply::Status(sessions.open(
                uid,
                open_key,
                selection,
                &gate,
                |selection| dispatcher.kernel.pin_view(selection, &work),
            )?),
            NativeReadSessionRequest::Renew { session, revision } => {
                NativeReadSessionReply::Status(sessions.renew(uid, session, revision, &gate)?)
            }
            NativeReadSessionRequest::Status(session) => {
                NativeReadSessionReply::Status(sessions.status(uid, session, &gate)?)
            }
            NativeReadSessionRequest::Release(session) => {
                let (status, has_remaining_target_pins) =
                    sessions.release(uid, session, &gate, |ticket| {
                        let target = scope(ticket);
                        dispatcher
                            .kernel
                            .release_read_ticket(ticket, target, &mut work)
                            .map(|observation| observation.has_remaining_target_pins)
                    })?;
                NativeReadSessionReply::Released {
                    status,
                    has_remaining_target_pins,
                }
            }
            NativeReadSessionRequest::Settlement(session) => {
                let observation = sessions.with_released_ticket(uid, session, &gate, |ticket| {
                    dispatcher
                        .kernel
                        .observe_read_settlement(ticket, scope(ticket), &mut work)
                })?;
                native_work_diagnostic::record_settlement_if_enabled(context, &observation);
                NativeReadSessionReply::Settlement(NativeReadSessionSettlement {
                    session,
                    view: observation.scope.target,
                    head: observation.head,
                    exact_floor: observation.exact_floor,
                    state: match observation.status {
                        NativeReadSettlementStatus::Pending(_) => {
                            NativeReadSettlementState::Pending
                        }
                        NativeReadSettlementStatus::Incomplete => {
                            NativeReadSettlementState::Incomplete
                        }
                        NativeReadSettlementStatus::SettledAtSnapshot => {
                            NativeReadSettlementState::SettledAtSnapshot
                        }
                    },
                    has_remaining_target_pins: observation.has_remaining_target_pins,
                })
            }
            NativeReadSessionRequest::ReadProfile { .. }
            | NativeReadSessionRequest::ReadHistoryCut { .. }
            | NativeReadSessionRequest::ReadHistoryGeneration { .. }
            | NativeReadSessionRequest::SearchText { .. }
            | NativeReadSessionRequest::SearchTextPage { .. }
            | NativeReadSessionRequest::SearchTextPathPageV1 { .. }
            | NativeReadSessionRequest::SearchHybridFile { .. }
            | NativeReadSessionRequest::SearchDslV1 { .. }
            | NativeReadSessionRequest::SearchHistory { .. } => {
                return Err(IndexError::BackendViolation(
                    "retained text request reached control reply".into(),
                ));
            }
        })
    })()
    .inspect_err(|error| {
        let observed_budget = if work.first_denial().is_some() {
            &work
        } else {
            &gate
        };
        observe(ReadFailureStage::SessionCustody, observed_budget, error);
    })?;
    work.checkpoint()
        .inspect_err(|error| observe(ReadFailureStage::Checkpoint, &work, error))?;
    Ok(slot.finish(NativeIndexResponse::ReadSession(reply)))
}

fn search(
    dispatcher: &NativeDispatcher,
    ticket: &quanta_index_kernel::ReadTicket,
    attempt: &mut quanta_index_kernel::TicketReadAttempt<'_>,
    instance: IndexInstanceId,
    query: &TextQueryRequest,
    work: &mut WorkBudget,
    timer: &RetainedSearchTimer,
    control: ReservationGuard<NativeClassScheduler>,
) -> Result<NativeReply, IndexError> {
    work.bind_memory_scope(control.memory_scope());
    timer.mark(ReadFailureStage::QueryPreparation);
    let planned = plan_native_projected_text(query, |bytes, lifetime| {
        work.reserve_memory(bytes, lifetime)
    })?;
    let limit = planned.result_limit()?;
    let path_projection =
        planned.plan.filters.surface == quanta_index_lexical::symbol::ResultSurface::Path;
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
            let reserve = |bytes, lifetime| work.reserve_memory(bytes, lifetime);
            if path_projection {
                crate::native_text_result::convert_paths(outcome, instance, limit, false, reserve)
                    .map(NativeReply::TextPaths)
            } else {
                crate::native_text_result::convert(outcome, instance, limit, reserve)
                    .map(NativeReply::Text)
            }
        },
    )
}

fn execute_text<T>(
    dispatcher: &NativeDispatcher,
    ticket: &quanta_index_kernel::ReadTicket,
    attempt: &mut quanta_index_kernel::TicketReadAttempt<'_>,
    instance: IndexInstanceId,
    planned: quanta_index_lexical::native::PlannedTextRead,
    work: &mut WorkBudget,
    timer: &RetainedSearchTimer,
    control: ReservationGuard<NativeClassScheduler>,
    project: impl FnOnce(
        quanta_index_core::index::QueryOutcome<quanta_index_lexical::native::TextReadResult>,
        &WorkBudget,
    ) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let result = dispatcher.kernel.with_prepared_stage_for_read_attempt(
        &dispatcher.text_queries,
        ticket,
        attempt,
        instance,
        planned,
        work,
        |prepared| {
            let need = prepared.required_resources();
            drop(control);
            timer.mark(ReadFailureStage::InteractiveAdmission);
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
            timer.mark(ReadFailureStage::QueryExecution);
            let selected = timer.is_enabled();
            let mut timings = quanta_index_kernel::QueryExecutionTimings::default();
            let result = prepared.execute_projected(
                reservation,
                selected.then_some(&mut timings),
                |outcome, work| {
                    timer.mark(ReadFailureStage::ResultConversion);
                    let result = project(outcome, work);
                    timer.mark(ReadFailureStage::QueryExecution);
                    result
                },
            );
            if selected {
                timer.record_execution_timings(timings);
            }
            result?.inspect_err(|_error| timer.mark(ReadFailureStage::ResultConversion))
        },
    )?;
    timer.mark(ReadFailureStage::Checkpoint);
    work.checkpoint()?;
    Ok(result)
}

/// The listener has joined its requests before this explicit lease drain.
pub(super) fn drain(dispatcher: &NativeDispatcher) -> Result<(), IndexError> {
    let Some(sessions) = dispatcher.read_sessions.get() else {
        return Ok(());
    };
    let request = RequestBudgetV1::for_duration(std::time::Duration::from_secs(5));
    let gate = WorkBudget::with_request(dispatcher.limits, request.clone());
    let reservation = ReservationGuard::try_acquire(
        std::sync::Arc::clone(&dispatcher.control_scheduler),
        dispatcher.control_need,
    )
    .ok_or_else(|| dispatcher.control_scheduler.denial())?;
    let mut work = WorkBudget::with_request(dispatcher.limits, request);
    work.bind_memory_scope(reservation.memory_scope());
    let _physical = dispatcher.io.enter_maintenance_scope()?;
    let report = sessions.drain(&gate, |ticket| {
        let target = scope(ticket);
        dispatcher
            .kernel
            .release_read_ticket(ticket, target, &mut work)
            .map(|observation| observation.has_remaining_target_pins)
    })?;
    report.first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_resources::NativeResourcePlan;
    use quanta_index_core::index::effort::ResourceNeed;

    #[test]
    fn observation_memory_outlives_scope_without_execution_or_escaping_the_cap()
    -> Result<(), IndexError> {
        let unit = ResourceNeed {
            cpu_units: 1,
            io_bytes: 1024,
            memory_bytes: 2048,
            provider_units: 0,
        };
        let resources = NativeResourceLedger::new(NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 4,
                io_bytes: 4096,
                memory_bytes: 16 * 1024,
                provider_units: 0,
            },
            control: unit,
            interactive: unit,
            core: unit,
            enrichment: unit,
        })?;
        let baseline = resources.snapshot()?;
        let scope = attempt_memory(&resources)?;
        let attempt = scope.reserve(128, MemoryLifetime::Temporary)?;
        let observation = scope.reserve(1024, MemoryLifetime::Retained)?;
        if resources.snapshot()?.reserved != baseline.reserved {
            return Err(IndexError::BackendViolation(
                "read observation reserved execution capacity".into(),
            ));
        }
        drop(scope);
        if resources.snapshot()?.retained_memory_bytes == 0 {
            return Err(IndexError::BackendViolation(
                "observation lost its charge with the request scope".into(),
            ));
        }
        drop(observation);
        drop(attempt);
        if resources.snapshot()? != baseline {
            return Err(IndexError::BackendViolation(
                "observation memory did not return to baseline".into(),
            ));
        }
        let scope = attempt_memory(&resources)?;
        let mut live = Vec::new();
        let mut refused = false;
        for _ in 0..32 {
            match scope.reserve(1024, MemoryLifetime::Retained) {
                Ok(memory) => live.push(memory),
                Err(IndexError::ResourceLimited) => {
                    refused = true;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        if !refused {
            return Err(IndexError::BackendViolation(
                "read observations escaped the shared memory hard cap".into(),
            ));
        }
        drop(live);
        drop(scope);
        if resources.snapshot()? != baseline {
            return Err(IndexError::BackendViolation(
                "refused observation leaked its memory custody".into(),
            ));
        }
        Ok(())
    }
}
