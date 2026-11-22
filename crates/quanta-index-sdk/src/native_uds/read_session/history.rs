//! Native History query transport reuses one explicit retained session.

use super::super::{AdmittedNativePublicationReply, MemoryScope, NativeUdsClient, NativeUdsError};
use super::admitted_budget_checkpoint;
use quanta_index_contract::{
    HistoryQueryRequest,
    index::{Digest, IndexInstanceId, ObjectRef},
    native_protocol::{
        NativeHistoryCursor, NativeHistoryPage, NativeIndexRequest, NativeIndexResponse,
        NativeReadSessionHistoryCutV1, NativeReadSessionRequest, NativeReadSessionState,
        NativeReadSessionStatus, validate_native_history_query,
    },
};
use quanta_index_ipc::RequestBudgetV1;
use std::path::Path;

impl<P: AsRef<Path>> NativeUdsClient<P> {
    /// Read the exact committed History cut through an already retained view.
    /// The server authenticates the member; this method checks the response
    /// belongs to the requested session and instance.
    pub fn read_history_cut_in_read_session(
        &self,
        expected: &NativeReadSessionStatus,
        instance: IndexInstanceId,
        budget: &RequestBudgetV1,
    ) -> Result<NativeReadSessionHistoryCutV1, NativeUdsError> {
        admitted_budget_checkpoint(budget)?;
        if expected.state != NativeReadSessionState::Active {
            return Err(NativeUdsError::Protocol(
                "History session is not active".into(),
            ));
        }
        let NativeIndexResponse::HistoryCut(result) = self.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadHistoryCut {
                session: expected.session,
                instance,
            }),
            budget,
        )?
        else {
            return Err(NativeUdsError::Protocol(
                "unexpected History cut reply".into(),
            ));
        };
        validate_history_cut_reply(expected, instance, &result)?;
        admitted_budget_checkpoint(budget)?;
        Ok(result)
    }

    /// Read a retained History cut through the installed bound-peer route.
    /// The returned owner keeps the raw and decoded response grants alive.
    pub fn read_history_cut_in_read_session_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        instance: IndexInstanceId,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        admitted_budget_checkpoint(budget)?;
        if expected.state != NativeReadSessionState::Active {
            return Err(NativeUdsError::Protocol(
                "History session is not active".into(),
            ));
        }
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadHistoryCut {
            session: expected.session,
            instance,
        });
        let reply = self.request_publication_admitted_ref(&request, budget, memory)?;
        match reply.response() {
            NativeIndexResponse::HistoryCut(result) => {
                validate_history_cut_reply(expected, instance, result)?;
            }
            NativeIndexResponse::Error(_) => {}
            _ => {
                return Err(NativeUdsError::Protocol(
                    "unexpected History cut reply".into(),
                ));
            }
        }
        admitted_budget_checkpoint(budget)?;
        Ok(reply)
    }

    /// Verify the producer's staged closure against the History member of the
    /// exact retained publication on the admitted, bound peer.
    pub fn read_history_generation_in_read_session_admitted(
        &self,
        expected: &NativeReadSessionStatus,
        instance: IndexInstanceId,
        source_input: ObjectRef,
        budget: &RequestBudgetV1,
        memory: &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError> {
        admitted_budget_checkpoint(budget)?;
        if expected.state != NativeReadSessionState::Active {
            return Err(NativeUdsError::Protocol(
                "History session is not active".into(),
            ));
        }
        let request =
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadHistoryGeneration {
                session: expected.session,
                instance,
            });
        let reply = self.request_publication_admitted_ref(&request, budget, memory)?;
        match reply.response() {
            NativeIndexResponse::HistoryGeneration(value)
                if value.session == expected.session
                    && value.view == expected.view
                    && value.instance == instance
                    && value.member.0 != Digest([0; 32])
                    && value.source_input == source_input
                    && source_input.0 != Digest([0; 32]) => {}
            NativeIndexResponse::Error(_) => {}
            _ => {
                return Err(NativeUdsError::Protocol(
                    "History generation reply differs from the expected publication".into(),
                ));
            }
        }
        admitted_budget_checkpoint(budget)?;
        Ok(reply)
    }

    /// Query the captured History member; never reopen Latest between pages.
    pub fn search_history_in_read_session(
        &self,
        expected: &NativeReadSessionStatus,
        instance: IndexInstanceId,
        query: HistoryQueryRequest,
        after: Option<NativeHistoryCursor>,
        budget: &RequestBudgetV1,
    ) -> Result<NativeHistoryPage, NativeUdsError> {
        admitted_budget_checkpoint(budget)?;
        if expected.state != NativeReadSessionState::Active {
            return Err(NativeUdsError::Protocol(
                "History session is not active".into(),
            ));
        }
        validate_native_history_query(&query).map_err(NativeUdsError::Protocol)?;
        if let Some(cursor) = &after {
            cursor
                .validate_request(expected.view, instance, &query)
                .map_err(NativeUdsError::Protocol)?;
        }
        let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHistory {
            session: expected.session,
            instance,
            query,
            after,
        });
        // Borrow the funded request DTO for both transport and reply binding.
        let request_id = super::super::allocate_request_id(&self.next_request_id)?;
        let envelope = super::super::BorrowedNativeRequestEnvelope {
            request_id,
            payload: &request,
        };
        let response = self.request_wire(request_id, &envelope, budget, Some(&request))?;
        let NativeIndexResponse::HistoryPage(page) = response else {
            return Err(NativeUdsError::Protocol(
                "unexpected retained History reply".into(),
            ));
        };
        let NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHistory {
            query,
            after,
            ..
        }) = &request
        else {
            return Err(NativeUdsError::Protocol(
                "History request binding disappeared".into(),
            ));
        };
        page.validate_reply(
            expected.session,
            expected.view,
            instance,
            query,
            after.as_ref(),
        )
        .map_err(NativeUdsError::Protocol)?;
        admitted_budget_checkpoint(budget)?;
        Ok(page)
    }
}

fn validate_history_cut_reply(
    expected: &NativeReadSessionStatus,
    instance: IndexInstanceId,
    result: &NativeReadSessionHistoryCutV1,
) -> Result<(), NativeUdsError> {
    if result.session != expected.session
        || result.view != expected.view
        || result.instance != instance
        || result.cut.durable_epoch == quanta_index_contract::AuxEpochV1::GENESIS
        || result.cut.snapshot_digest.0 == [0; 32]
        || result.cut.replay_start.0 == [0; 32]
    {
        return Err(NativeUdsError::Protocol(
            "History cut differs from retained member".into(),
        ));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use quanta_index_contract::{
        AuxEpochV1, CandidateCountV1, CommitCandidate, HistoryOrderV1, QueryConstraintSetV1,
        QueryResultWindowV2, TextQueryRequest, TextQuerySyntax,
        index::{Digest, ObjectRef},
        lex::CommitSha,
        native_protocol::{NativeIndexRequestEnvelope, NativeIndexResponseEnvelope},
    };
    use std::{
        io::Write,
        os::unix::net::UnixListener,
        time::{Duration, Instant},
    };
    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    fn fixture() -> TestResult<(
        NativeReadSessionStatus,
        HistoryQueryRequest,
        NativeHistoryPage,
    )> {
        let (status, _) = super::super::profile::tests::fixture()?;
        let query = HistoryQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "type:commit fix author:alice".into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: 2,
                cursor: None,
            },
            order: HistoryOrderV1::Recency,
            cursor: None,
        };
        let page = NativeHistoryPage {
            session: status.session,
            view: status.view,
            instance: IndexInstanceId([8; 16]),
            member: ObjectRef(Digest([9; 32])),
            order: HistoryOrderV1::Recency,
            commits: vec![CommitCandidate {
                sha: CommitSha::from_bytes([1; 20]),
                parent_ids: Vec::new(),
                committed_at_unix_s: 123,
                author: "alice".into(),
                committer: "maintainer".into(),
                message: "fix historic deletion".into(),
                is_merge: false,
                tags: vec!["v1".into()],
                score: None,
            }],
            diffs: Vec::new(),
            matched: 1,
            remaining: 1,
            window: QueryResultWindowV2::exact_probe(1),
            read_epoch: AuxEpochV1::new(3),
            examined: 7,
            after: None,
        };
        Ok((status, query, page))
    }

    fn exchange(
        page: NativeHistoryPage,
        call: impl FnOnce(
            &NativeUdsClient<std::path::PathBuf>,
        ) -> Result<NativeHistoryPage, NativeUdsError>,
    ) -> TestResult<Result<NativeHistoryPage, NativeUdsError>> {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("history.sock");
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let peer = std::thread::spawn(move || -> Result<(), String> {
            let end = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < end =>
                    {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(error) => return Err(error.to_string()),
                }
            };
            stream
                .set_nonblocking(false)
                .map_err(|error| error.to_string())?;
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).map_err(|error| error.to_string())?;
            let NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHistory {
                session,
                instance,
                query,
                after,
            }) = request.payload
            else {
                return Err(
                    "History client issued a different operation or reopened a view".into(),
                );
            };
            if session != page.session
                || instance != IndexInstanceId([8; 16])
                || query.order != HistoryOrderV1::Recency
                || query.text_query.query_text != "type:commit fix author:alice"
                || after.is_some()
            {
                return Err(
                    "History request changed session, instance, predicate, order or cursor".into(),
                );
            }
            let response = NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload: NativeIndexResponse::HistoryPage(page),
            };
            let frame =
                quanta_index_ipc::encode_response(&response).map_err(|error| error.to_string())?;
            stream.write_all(&frame).map_err(|error| error.to_string())
        });
        let result = call(&NativeUdsClient::new(socket));
        peer.join().map_err(|_| "History peer panicked")??;
        Ok(result)
    }

    #[test]
    fn native_history_sdk_sends_one_retained_request_and_preserves_commit_rows() -> TestResult {
        let (status, query, page) = fixture()?;
        let expected = page.clone();
        let actual = exchange(page, |client| {
            client.search_history_in_read_session(
                &status,
                IndexInstanceId([8; 16]),
                query,
                None,
                &RequestBudgetV1::for_duration(Duration::from_secs(2)),
            )
        })??;
        assert_eq!(actual, expected);
        assert_eq!(actual.examined, 7);
        assert_eq!(actual.window.candidate_count(), CandidateCountV1::Exact(1));
        Ok(())
    }
    #[test]
    fn native_history_sdk_refuses_unbound_view_and_false_cardinality() -> TestResult {
        for axis in 0..2 {
            let (status, query, mut page) = fixture()?;
            if axis == 0 {
                page.view.sequence += 1;
            } else {
                page.commits.clear();
            }
            let result = exchange(page, |client| {
                client.search_history_in_read_session(
                    &status,
                    IndexInstanceId([8; 16]),
                    query,
                    None,
                    &RequestBudgetV1::for_duration(Duration::from_secs(2)),
                )
            })?;
            assert!(matches!(result, Err(NativeUdsError::Protocol(_))));
        }
        Ok(())
    }
    #[test]
    fn native_history_sdk_cancelled_and_inactive_requests_do_not_connect() -> TestResult {
        let dir = tempfile::tempdir()?;
        let socket = dir.path().join("no-connect.sock");
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let client = NativeUdsClient::new(socket);
        let (mut status, query, _) = fixture()?;
        status.state = NativeReadSessionState::Released;
        assert!(
            client
                .search_history_in_read_session(
                    &status,
                    IndexInstanceId([8; 16]),
                    query.clone(),
                    None,
                    &RequestBudgetV1::for_duration(Duration::from_secs(2))
                )
                .is_err()
        );
        status.state = NativeReadSessionState::Active;
        let expired = RequestBudgetV1::for_duration(Duration::ZERO);
        assert!(
            client
                .search_history_in_read_session(
                    &status,
                    IndexInstanceId([8; 16]),
                    query,
                    None,
                    &expired
                )
                .is_err()
        );
        assert!(
            matches!(listener.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock)
        );
        Ok(())
    }
}
