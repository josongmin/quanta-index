//! One retained attempt, one cumulative work budget and two independent readers.

use super::*;
use crate::{
    native_provider_custody::NativeProviderExecutionCustody,
    native_semantic_result::{ScopedNativeHybridFileResult, convert_hybrid},
};
use quanta_index_contract::{
    SemanticQueryRequest,
    native_protocol::{NativeReadSessionId, validate_native_hybrid_file_query},
};
use quanta_index_semantic::native::SemanticReadRequest;

pub(super) fn search(
    dispatcher: &NativeDispatcher,
    ticket: &quanta_index_kernel::ReadTicket,
    attempt: &mut quanta_index_kernel::TicketReadAttempt<'_>,
    session: NativeReadSessionId,
    text_instance: IndexInstanceId,
    semantic_instance: IndexInstanceId,
    text_query: &TextQueryRequest,
    semantic_query: SemanticQueryRequest,
    file_top_k: u32,
    work: &mut WorkBudget,
    timer: &RetainedSearchTimer,
    control: ReservationGuard<NativeClassScheduler>,
) -> Result<ScopedNativeHybridFileResult, IndexError> {
    validate_native_hybrid_file_query(text_query, &semantic_query, file_top_k)
        .map_err(IndexError::InvalidInput)?;
    work.bind_memory_scope(control.memory_scope());
    timer.mark(ReadFailureStage::QueryPreparation);
    let planned = plan_native_text(text_query, |bytes, lifetime| {
        work.reserve_memory(bytes, lifetime)
    })?;
    // Eligibility must be the same for both lanes. Inline lexical filters
    // cannot restrict the independent dense reader's source universe.
    if !planned.plan.raw_filters.is_empty() {
        return Err(IndexError::InvalidInput(
            "Native Hybrid requires shared constraints instead of inline lexical filters".into(),
        ));
    }
    let lexical_limit = planned.result_limit()?;
    let text = execute_text(
        dispatcher,
        ticket,
        attempt,
        text_instance,
        planned,
        work,
        timer,
        control,
        |outcome, work| {
            crate::native_text_result::validate_read(
                &outcome,
                lexical_limit,
                |bytes, lifetime| work.reserve_memory(bytes, lifetime),
            )?;
            Ok(outcome)
        },
    )?;
    // Text outcome retains DATA grants while its Interactive grant is returned.
    // Control and Semantic reuse the original attempt and cumulative work/time.
    timer.mark(ReadFailureStage::ControlAdmission);
    let control = dispatcher
        .control_scheduler
        .acquire_control_for_request(dispatcher.control_need, work)?;
    work.bind_memory_scope(control.memory_scope());
    let dense_limit = semantic_query.top_k;
    let request = SemanticReadRequest::for_source_files(semantic_query)?;
    let (outcome, execution, provider) = dispatcher.kernel.with_prepared_stage_for_read_attempt(
        &dispatcher.semantic_queries,
        ticket,
        attempt,
        semantic_instance,
        request,
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
            if need.provider_units > 0 {
                let custody = NativeProviderExecutionCustody::new_with_owner(
                    reservation,
                    dispatcher.root_lease.clone(),
                )?;
                let outcome = prepared
                    .execute_with_provider_custody(custody.reservation(), &custody.port())?;
                return Ok((outcome, None, Some(custody)));
            }
            let outcome = prepared.execute(&reservation)?;
            Ok((outcome, Some(reservation), None))
        },
    )?;
    timer.mark(ReadFailureStage::ResultConversion);
    let result = convert_hybrid(
        text,
        outcome,
        session,
        text_instance,
        semantic_instance,
        file_top_k,
        dense_limit,
        &text_query.constraints,
        work,
    );
    drop(execution);
    drop(provider);
    result
}
