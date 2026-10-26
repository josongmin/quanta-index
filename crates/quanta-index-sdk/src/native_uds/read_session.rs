//! Explicit product read leases; callers retain the last acknowledged identity.

use super::{
    IndexInstanceId, MAX_NATIVE_TEXT_TOP_K, NativeIndexRequest, NativeIndexResponse,
    NativeTextResult, NativeUdsClient, NativeUdsError, Path, RequestBudgetV1, TextQueryRequest,
    ViewSelection, native_text_result_limit, selection_matches, validate_text_result,
};
use quanta_index_contract::native_protocol::{
    NativeReadSessionId, NativeReadSessionReply, NativeReadSessionRequest,
    NativeReadSessionSettlement, NativeReadSessionState, NativeReadSessionStatus,
    NativeReadSettlementState,
};

fn binding(
    expected: &NativeReadSessionStatus,
    actual: &NativeReadSessionStatus,
) -> Result<(), NativeUdsError> {
    if actual.session != expected.session
        || actual.view != expected.view
        || actual.revision < expected.revision
    {
        return Err(NativeUdsError::Protocol(
            "read session identity or revision differs".into(),
        ));
    }
    Ok(())
}

impl<P: AsRef<Path>> NativeUdsClient<P> {
    /// Retry an uncertain Open with the same key and selection. This is an
    /// explicit lease, not a socket lifetime or an automatically renewed pin.
    pub fn open_read_session(
        &self,
        selection: ViewSelection,
        open_key: [u8; 32],
        budget: &RequestBudgetV1,
    ) -> Result<NativeReadSessionStatus, NativeUdsError> {
        let NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(status)) = self
            .request(
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::Open {
                    selection,
                    open_key,
                }),
                budget,
            )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected read session open reply".into(),
            ));
        };
        if !selection_matches(selection, status.view)
            || status.session == NativeReadSessionId([0; 32])
            || status.revision == 0
            || status.expires_at_ms == 0
            || status.state != NativeReadSessionState::Active
        {
            return Err(NativeUdsError::Protocol(
                "read session open binding differs".into(),
            ));
        }
        Ok(status)
    }

    /// The acknowledged revision is the retry key; a new request budget never
    /// changes the selected view or revives an expired Catalog ticket.
    pub fn renew_read_session(
        &self,
        expected: &NativeReadSessionStatus,
        budget: &RequestBudgetV1,
    ) -> Result<NativeReadSessionStatus, NativeUdsError> {
        let NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(status)) = self
            .request(
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::Renew {
                    session: expected.session,
                    revision: expected.revision,
                }),
                budget,
            )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected read session renew reply".into(),
            ));
        };
        binding(expected, &status)?;
        if expected.revision.checked_add(1) != Some(status.revision)
            || status.expires_at_ms < expected.expires_at_ms
            || status.state != NativeReadSessionState::Active
        {
            return Err(NativeUdsError::Protocol(
                "read session renewal differs".into(),
            ));
        }
        Ok(status)
    }

    pub fn read_session_status(
        &self,
        expected: &NativeReadSessionStatus,
        budget: &RequestBudgetV1,
    ) -> Result<NativeReadSessionStatus, NativeUdsError> {
        let NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(status)) = self
            .request(
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::Status(expected.session)),
                budget,
            )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected read session status reply".into(),
            ));
        };
        binding(expected, &status)?;
        Ok(status)
    }

    /// A failed release is not a successful ACK. Keep this identity to retry
    /// or inspect status; do not reopen Latest to hide an uncertain outcome.
    pub fn release_read_session(
        &self,
        expected: &NativeReadSessionStatus,
        budget: &RequestBudgetV1,
    ) -> Result<(NativeReadSessionStatus, bool), NativeUdsError> {
        let NativeIndexResponse::ReadSession(NativeReadSessionReply::Released {
            status,
            has_remaining_target_pins,
        }) = self.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::Release(expected.session)),
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected read session release reply".into(),
            ));
        };
        binding(expected, &status)?;
        if status.state != NativeReadSessionState::Released {
            return Err(NativeUdsError::Protocol(
                "read session release was not confirmed".into(),
            ));
        }
        Ok((status, has_remaining_target_pins))
    }

    pub fn read_session_settlement(
        &self,
        expected: &NativeReadSessionStatus,
        budget: &RequestBudgetV1,
    ) -> Result<NativeReadSessionSettlement, NativeUdsError> {
        if expected.state != NativeReadSessionState::Released {
            return Err(NativeUdsError::Protocol(
                "settlement needs a confirmed release".into(),
            ));
        }
        let NativeIndexResponse::ReadSession(NativeReadSessionReply::Settlement(result)) = self
            .request(
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::Settlement(
                    expected.session,
                )),
                budget,
            )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected read session settlement reply".into(),
            ));
        };
        let head_scope_matches = (result.head.workspace, result.head.epoch)
            == (expected.view.workspace, expected.view.epoch);
        if result.session != expected.session
            || result.view != expected.view
            || !head_scope_matches
            || result.head.sequence < expected.view.sequence
            || result.exact_floor > result.head.sequence
            || (result.state == NativeReadSettlementState::SettledAtSnapshot
                && (result.has_remaining_target_pins
                    || result.exact_floor <= expected.view.sequence))
        {
            return Err(NativeUdsError::Protocol(
                "read session settlement binding differs".into(),
            ));
        }
        Ok(result)
    }

    pub fn search_text_in_read_session(
        &self,
        expected: &NativeReadSessionStatus,
        instance: IndexInstanceId,
        query: TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeTextResult, NativeUdsError> {
        if expected.state != NativeReadSessionState::Active
            || !(1..=MAX_NATIVE_TEXT_TOP_K).contains(&query.top_k)
        {
            return Err(NativeUdsError::Protocol(
                "read session query is not active or exceeds bounds".into(),
            ));
        }
        let limit = native_text_result_limit(&query)?;
        let NativeIndexResponse::TextRanked(result) = self.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText {
                session: expected.session,
                instance,
                query,
            }),
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected retained text reply".into(),
            ));
        };
        validate_text_result(
            ViewSelection::Exact(expected.view),
            instance,
            limit,
            &result,
        )?;
        Ok(result)
    }
}
