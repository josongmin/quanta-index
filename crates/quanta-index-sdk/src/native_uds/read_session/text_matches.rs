//! Native text match pages, stateless or on a retained read session.
//!
//! Every page is checked against the request it answers before it is
//! returned, so a caller never sees rows of another view, query or order.

use super::{
    NativeIndexRequest, NativeIndexResponse, NativeReadSessionRequest, NativeReadSessionState,
    NativeReadSessionStatus, NativeUdsClient, NativeUdsError, Path, RequestBudgetV1, ViewSelection,
};
use quanta_index_contract::native_protocol::{NativeTextMatchPage, NativeTextMatchesRequest};

impl<P: AsRef<Path>> NativeUdsClient<P> {
    /// One match page over `request.selection`. A continuation reads its
    /// cursor's exact view, which must still be retained by the daemon.
    pub fn search_text_matches(
        &self,
        request: &NativeTextMatchesRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeTextMatchPage, NativeUdsError> {
        request.validate().map_err(NativeUdsError::Protocol)?;
        let NativeIndexResponse::TextMatchPage(page) =
            self.request(NativeIndexRequest::TextMatches(request.clone()), budget)?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected text match page reply".into(),
            ));
        };
        page.validate_for(request)
            .map_err(NativeUdsError::Protocol)?;
        Ok(page)
    }

    /// One match page over the view `expected` retains. The request must
    /// select that view exactly; the session keeps it readable across later
    /// publications for as long as the caller holds the lease.
    pub fn search_text_matches_in_read_session(
        &self,
        expected: &NativeReadSessionStatus,
        request: &NativeTextMatchesRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeTextMatchPage, NativeUdsError> {
        request.validate().map_err(NativeUdsError::Protocol)?;
        if expected.state != NativeReadSessionState::Active
            || request.selection != ViewSelection::Exact(expected.view)
        {
            return Err(NativeUdsError::Protocol(
                "text match request must select its active session's view".into(),
            ));
        }
        let NativeIndexResponse::TextMatchPage(page) = self.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextMatches {
                session: expected.session,
                request: request.clone(),
            }),
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected retained text match page reply".into(),
            ));
        };
        page.validate_for(request)
            .map_err(NativeUdsError::Protocol)?;
        Ok(page)
    }
}
