//! Text match pages over a read session's retained view.

use quanta_index_contract::{index::ViewSelection, native_protocol::NativeTextMatchesRequest};
use quanta_index_core::index::{IndexError, WorkBudget, effort::ReservationGuard};
use quanta_index_lexical::native::NativeTextMatchResult;

use crate::native::NativeDispatcher;
use crate::native_resources::NativeClassScheduler;
use crate::native_text_match_transport::checked_match_page;
use crate::native_work_diagnostic::{ReadFailureStage, RetainedSearchTimer};

/// The request must select the session's view exactly; the session, not the
/// request, decides which view is read.
pub(super) fn search(
    dispatcher: &NativeDispatcher,
    ticket: &quanta_index_kernel::ReadTicket,
    attempt: &mut quanta_index_kernel::TicketReadAttempt<'_>,
    request: NativeTextMatchesRequest,
    work: &mut WorkBudget,
    timer: &RetainedSearchTimer,
    control: ReservationGuard<NativeClassScheduler>,
) -> Result<NativeTextMatchResult, IndexError> {
    work.bind_memory_scope(control.memory_scope());
    timer.mark(ReadFailureStage::QueryPreparation);
    request.validate().map_err(IndexError::InvalidInput)?;
    if request.selection != ViewSelection::Exact(ticket.view().identity) {
        return Err(IndexError::InvalidInput(
            "text match request must select its read session's view".into(),
        ));
    }
    let instance = request.instance;
    let outcome = dispatcher.kernel.with_prepared_stage_for_read_attempt(
        &dispatcher.text_match_queries,
        ticket,
        attempt,
        instance,
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
            prepared.execute(&reservation)
        },
    )?;
    timer.mark(ReadFailureStage::ResultConversion);
    let page = checked_match_page(outcome)?;
    timer.mark(ReadFailureStage::Checkpoint);
    work.checkpoint()?;
    Ok(page)
}
