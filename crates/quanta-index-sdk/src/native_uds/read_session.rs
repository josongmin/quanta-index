//! Explicit product read leases; callers retain the last acknowledged identity.

mod admitted;
mod history;
mod hybrid;
mod page;
mod path_page_v1;
mod profile;
mod text_matches;

use super::{
    IndexInstanceId, MAX_NATIVE_TEXT_TOP_K, NativeIndexRequest, NativeIndexResponse,
    NativeTextPathResultV1, NativeTextResult, NativeUdsClient, NativeUdsError, Path,
    RequestBudgetV1, TextQueryRequest, ViewSelection, selection_matches, validate_text_result,
};
use quanta_index_contract::native_protocol::{
    NativeReadSessionId, NativeReadSessionReply, NativeReadSessionRequest,
    NativeReadSessionSettlement, NativeReadSessionState, NativeReadSessionStatus,
    NativeReadSettlementState,
};
use crate::ClientLexicalQueryObservationV1;

// The original budget owns cancellation, absolute and phase-observed deadlines.
// Do not approximate it with a copied clock or cancellation flag.
pub(super) fn admitted_budget_checkpoint(budget: &RequestBudgetV1) -> Result<(), NativeUdsError> {
    budget.checkpoint("native_admitted_read").map_err(|cause| {
        let (code, message) = cause.into_search_plane_wire();
        if code == quanta_index_ipc::REQUEST_CANCELLED_CODE {
            NativeUdsError::Refused(super::NativeIndexWireError {
                code: super::NativeIndexErrorCode::Cancelled,
                message,
            })
        } else {
            NativeUdsError::Transport(message)
        }
    })
}

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
    /// A retained replay returns the existing session's current state,
    /// including its state after closing or release.
    pub fn open_read_session(
        &self,
        selection: ViewSelection,
        open_key: [u8; 32],
        budget: &RequestBudgetV1,
    ) -> Result<NativeReadSessionStatus, NativeUdsError> {
        self.open_read_session_with_observation(selection, open_key, budget, None)
    }

    pub(crate) fn open_read_session_with_observation(
        &self,
        selection: ViewSelection,
        open_key: [u8; 32],
        budget: &RequestBudgetV1,
        observation: Option<&mut ClientLexicalQueryObservationV1>,
    ) -> Result<NativeReadSessionStatus, NativeUdsError> {
        let NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(status)) = self
            .request_lexical(
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::Open {
                    selection,
                    open_key,
                }),
                budget,
                observation,
            )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected read session open reply".into(),
            ));
        };
        validate_open(selection, &status)?;
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
        validate_release(expected, &status)?;
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
        let (limit, path_projection) = prepare_text(expected, &query, None)?;
        if path_projection {
            return Err(NativeUdsError::Protocol(
                "select:path requires search_text_paths_in_read_session".into(),
            ));
        }
        let constraints = query.constraints.clone();
        let response = self.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText {
                session: expected.session,
                instance,
                query,
            }),
            budget,
        )?;
        match response {
            NativeIndexResponse::TextRanked(result) => {
                validate_text_result(
                    ViewSelection::Exact(expected.view),
                    instance,
                    limit,
                    &constraints,
                    &result,
                )?;
                Ok(result)
            }
            _ => Err(NativeUdsError::Protocol(
                "unexpected retained text reply".into(),
            )),
        }
    }

    /// Preserve repository identities for an unpaged retained `select:path` query.
    pub fn search_text_paths_in_read_session(
        &self,
        expected: &NativeReadSessionStatus,
        instance: IndexInstanceId,
        query: TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<NativeTextPathResultV1, NativeUdsError> {
        let (limit, path_projection) = prepare_text(expected, &query, None)?;
        if !path_projection {
            return Err(NativeUdsError::Protocol(
                "text path projection required".into(),
            ));
        }
        let constraints = query.constraints.clone();
        let NativeIndexResponse::TextPathRankedV1(result) = self.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText {
                session: expected.session,
                instance,
                query,
            }),
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected retained text path reply".into(),
            ));
        };
        if result.previews.is_some() {
            return Err(NativeUdsError::Protocol(
                "unexpected retained text path previews".into(),
            ));
        }
        let count = result.ranked.ranking.matched.min(u64::from(limit));
        super::path_output_v1::validate_ranking(
            ViewSelection::Exact(expected.view),
            instance,
            limit,
            count,
            &constraints,
            &result.ranked,
            None,
        )?;
        Ok(result)
    }
}

fn validate_open(
    selection: ViewSelection,
    status: &NativeReadSessionStatus,
) -> Result<(), NativeUdsError> {
    if !selection_matches(selection, status.view)
        || status.session == NativeReadSessionId([0; 32])
        || status.revision == 0
        || status.expires_at_ms == 0
    {
        return Err(NativeUdsError::Protocol(
            "read session open binding differs".into(),
        ));
    }
    Ok(())
}

fn validate_release(
    expected: &NativeReadSessionStatus,
    status: &NativeReadSessionStatus,
) -> Result<(), NativeUdsError> {
    binding(expected, status)?;
    if status.state != NativeReadSessionState::Released {
        return Err(NativeUdsError::Protocol(
            "read session release was not confirmed".into(),
        ));
    }
    Ok(())
}

pub(super) fn response_matches_request(
    request: &NativeReadSessionRequest,
    response: &NativeIndexResponse,
) -> bool {
    use NativeIndexResponse as S;
    use NativeReadSessionRequest as R;
    match (request, response) {
        (R::Open { selection, .. }, S::ReadSession(NativeReadSessionReply::Status(status))) => {
            validate_open(*selection, status).is_ok()
        }
        (
            R::Renew { session, revision },
            S::ReadSession(NativeReadSessionReply::Status(status)),
        ) => {
            status.session == *session
                && revision.checked_add(1) == Some(status.revision)
                && status.state == NativeReadSessionState::Active
        }
        (R::Status(session), S::ReadSession(NativeReadSessionReply::Status(status))) => {
            status.session == *session
        }
        (R::Release(session), S::ReadSession(NativeReadSessionReply::Released { status, .. })) => {
            status.session == *session && status.state == NativeReadSessionState::Released
        }
        (R::Settlement(session), S::ReadSession(NativeReadSessionReply::Settlement(result))) => {
            result.session == *session
        }
        (R::ReadProfile { session }, S::ReadSessionProfile(result)) => result.session == *session,
        (R::SearchDslV1 { session, .. }, S::DslPageV1(page)) => page.session == *session,
        (
            R::ReadDslRuntimeCatalogV1 { session, offset, .. },
            S::DslRuntimeCatalogPageV1(page),
        ) => page.session == *session && page.matches_offset_v1(*offset),
        (R::ReadDslStructuralChunksV1 { session, .. }, S::DslStructuralChunksPageV1(page)) =>
            page.session == *session,
        (R::ReadHistoryCut { session, instance }, S::HistoryCut(result)) => {
            result.session == *session && result.instance == *instance
        }
        (R::ReadHistoryGeneration { session, instance }, S::HistoryGeneration(result)) => {
            result.session == *session && result.instance == *instance
        }
        (R::SearchText { instance, .. }, S::TextRanked(result)) => result.instance == *instance,
        (R::SearchText { instance, .. }, S::TextPathRankedV1(result)) => {
            result.ranked.ranking.instance == *instance
        }
        (
            R::SearchTextPage {
                session, instance, ..
            },
            S::TextRankedPage(result),
        ) => result.session == *session && result.instance == *instance,
        (
            R::SearchTextPathPageV1 {
                session, instance, ..
            },
            S::TextPathPageV1(page),
        ) => page.session == *session && page.ranked.ranking.instance == *instance,
        (
            R::SearchHybridFile {
                session,
                text_instance,
                semantic_instance,
                file_top_k,
                ..
            },
            S::HybridFileRanked(result),
        ) => {
            result.session == *session
                && result.text_instance == *text_instance
                && result.semantic_instance == *semantic_instance
                && result.file_top_k == *file_top_k
        }
        _ => false,
    }
}

fn prepare_text(
    expected: &NativeReadSessionStatus,
    query: &TextQueryRequest,
    memory: Option<&super::MemoryScope>,
) -> Result<(u32, bool), NativeUdsError> {
    if expected.state != NativeReadSessionState::Active
        || !(1..=MAX_NATIVE_TEXT_TOP_K).contains(&query.top_k)
    {
        return Err(NativeUdsError::Protocol(
            "read session query is not active or exceeds bounds".into(),
        ));
    }
    let mut path_projection = false;
    let limit = super::native_text_result_limit_checked_with_memory(query, memory, |parsed| {
        path_projection = parsed.filters.iter().any(|filter| {
            matches!(
                filter,
                quanta_index_contract::LqFilter::Select {
                    dim: quanta_index_contract::LqSelect::Path
                }
            )
        });
        Ok(())
    })?;
    Ok((limit, path_projection))
}
