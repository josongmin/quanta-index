//! Credential-bound product read leases over the existing Native control plane.

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
        IndexError, NativeReadSettlementScope, NativeReadSettlementStatus, WorkBudget,
        effort::ReservationGuard,
    },
};
use quanta_index_ipc::DispatchContextV1;

use crate::{
    native::{NativeDispatcher, ScopedNativeTextResult, plan_native_text},
    native_reply::NativeReply,
    native_resources::NativeClassScheduler,
    native_work_diagnostic::{
        self, ReadFailureRoute, ReadFailureStage, RetainedSearchTimer, SessionCustodyPart,
    },
};

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
    let gate = WorkBudget::with_request(dispatcher.limits, budget.clone());
    if let NativeReadSessionRequest::SearchText {
        session,
        instance,
        query,
    } = request
    {
        let mut work = WorkBudget::with_request(dispatcher.limits, budget.clone());
        let timer = RetainedSearchTimer::new(context);
        let ticket_observation = timer
            .is_enabled()
            .then(|| quanta_index_catalog::TicketValidationObservation::begin(&dispatcher.catalog))
            .flatten();
        let result = sessions
            .with_ticket(
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
                |ticket, control| {
                    let result = search(
                        dispatcher, ticket, instance, &query, &mut work, &timer, control,
                    );
                    if result.is_ok() {
                        timer.mark_session_custody(SessionCustodyPart::ExitPost);
                    }
                    result
                },
            )
            .map(NativeReply::Text);
        if let Some(timings) = ticket_observation.and_then(|observation| observation.snapshot()) {
            timer.record_ticket_validation_timings(timings);
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
            NativeReadSessionRequest::SearchText { .. } => {
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
    instance: IndexInstanceId,
    query: &TextQueryRequest,
    work: &mut WorkBudget,
    timer: &RetainedSearchTimer,
    control: ReservationGuard<NativeClassScheduler>,
) -> Result<ScopedNativeTextResult, IndexError> {
    work.bind_memory_scope(control.memory_scope());
    timer.mark(ReadFailureStage::QueryPreparation);
    let planned = plan_native_text(query, |bytes, lifetime| {
        work.reserve_memory(bytes, lifetime)
    })?;
    let limit = planned.result_limit()?;
    let result = dispatcher.kernel.with_prepared_stage_for_ticket(
        &dispatcher.text_queries,
        ticket,
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
                    let result = crate::native_text_result::convert(
                        outcome,
                        instance,
                        limit,
                        |bytes, lifetime| work.reserve_memory(bytes, lifetime),
                    );
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
