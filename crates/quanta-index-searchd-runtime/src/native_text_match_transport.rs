//! Native text match pages retain row custody through serialization.
//!
//! A stateless page selects its view per request. The read-session form in
//! `native_read_session_transport` reads the session's retained view instead.

use quanta_index_contract::native_protocol::{NativeTextMatchCompletion, NativeTextMatchesRequest};
use quanta_index_core::index::{IndexError, QueryCoverage, QueryOutcome, WorkBudget};
use quanta_index_lexical::native::NativeTextMatchResult;

use crate::native::NativeDispatcher;

impl NativeDispatcher {
    pub(super) fn search_text_matches(
        &self,
        request: NativeTextMatchesRequest,
        budget: &mut WorkBudget,
    ) -> Result<NativeTextMatchResult, IndexError> {
        request.validate().map_err(IndexError::InvalidInput)?;
        // Admission waits within the request budget, as ranked text reads do.
        let prepared = {
            let control = self
                .control_scheduler
                .acquire_control_for_request(self.control_need, budget)?;
            budget.bind_memory_scope(control.memory_scope());
            self.kernel.prepare_query(
                &self.text_match_queries,
                request.selection,
                request.instance,
                request,
                budget,
            )?
        };
        let need = prepared.required_resources();
        let reservation = match self
            .interactive_scheduler
            .acquire_interactive_for_request(need, prepared.admission_budget())
        {
            Ok(reservation) => reservation,
            Err(error) => {
                prepared.abort()?;
                return Err(error);
            }
        };
        checked_match_page(prepared.execute(&reservation)?)
    }
}

/// The reader's outcome must be a fully scanned Core page whose completion
/// agrees with the outcome's exhaustiveness.
pub(super) fn checked_match_page(
    outcome: QueryOutcome<NativeTextMatchResult>,
) -> Result<NativeTextMatchResult, IndexError> {
    let exhausted = outcome.result().completion == NativeTextMatchCompletion::Exhausted;
    if !matches!(outcome.coverage(), QueryCoverage::Core) || outcome.exhaustive() != exhausted {
        return Err(IndexError::BackendViolation(
            "text match reader returned an invalid page".into(),
        ));
    }
    Ok(outcome.into_result())
}
