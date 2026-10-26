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
        return sessions
            .with_ticket(uid, session, &gate, |ticket| {
                search(dispatcher, ticket, instance, &query, &mut work)
            })
            .map(NativeReply::Text);
    }
    let slot = NativeReply::reserve_canonical(|bytes| {
        dispatcher
            .resources
            .reserve_response_memory(bytes)
            .map(Some)
    })?;
    let control = ReservationGuard::try_acquire(
        std::sync::Arc::clone(&dispatcher.control_scheduler),
        dispatcher.control_need,
    )
    .ok_or_else(|| dispatcher.control_scheduler.denial())?;
    let mut work = WorkBudget::with_request(dispatcher.limits, budget.clone());
    work.bind_memory_scope(control.memory_scope());
    work.checkpoint()?;
    let reply = match request {
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
            let result = sessions.with_released_ticket(uid, session, &gate, |ticket| {
                let observation =
                    dispatcher
                        .kernel
                        .observe_read_settlement(ticket, scope(ticket), &mut work)?;
                Ok(NativeReadSessionSettlement {
                    session,
                    view: ticket.view().identity,
                    head: observation.head,
                    exact_floor: observation.exact_floor,
                    state: match observation.status {
                        NativeReadSettlementStatus::Pending => NativeReadSettlementState::Pending,
                        NativeReadSettlementStatus::Incomplete => {
                            NativeReadSettlementState::Incomplete
                        }
                        NativeReadSettlementStatus::SettledAtSnapshot => {
                            NativeReadSettlementState::SettledAtSnapshot
                        }
                    },
                    has_remaining_target_pins: observation.has_remaining_target_pins,
                })
            })?;
            NativeReadSessionReply::Settlement(result)
        }
        NativeReadSessionRequest::SearchText { .. } => {
            return Err(IndexError::BackendViolation(
                "retained text request reached control reply".into(),
            ));
        }
    };
    work.checkpoint()?;
    Ok(slot.finish(NativeIndexResponse::ReadSession(reply)))
}

fn search(
    dispatcher: &NativeDispatcher,
    ticket: &mut quanta_index_kernel::ReadTicket,
    instance: IndexInstanceId,
    query: &TextQueryRequest,
    work: &mut WorkBudget,
) -> Result<ScopedNativeTextResult, IndexError> {
    let control = ReservationGuard::try_acquire(
        std::sync::Arc::clone(&dispatcher.control_scheduler),
        dispatcher.control_need,
    )
    .ok_or_else(|| dispatcher.control_scheduler.denial())?;
    work.bind_memory_scope(control.memory_scope());
    let planned = plan_native_text(query, |bytes, lifetime| {
        work.reserve_memory(bytes, lifetime)
    })?;
    let limit = planned.result_limit()?;
    let mut session = dispatcher
        .kernel
        .begin_query_session_with_ticket(ticket, work)?;
    let prepared = session.prepare_stage(&dispatcher.text_queries, instance, planned)?;
    let need = prepared.required_resources();
    drop(control);
    let Some(reservation) = ReservationGuard::try_acquire(
        std::sync::Arc::clone(&dispatcher.interactive_scheduler),
        need,
    ) else {
        prepared.abort();
        return Err(dispatcher.interactive_scheduler.denial());
    };
    let outcome = prepared.execute(&reservation)?;
    let result = crate::native_text_result::convert(outcome, instance, limit, |bytes, lifetime| {
        session.reserve_query_memory(bytes, lifetime)
    });
    session.close()?;
    let result = result?;
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
