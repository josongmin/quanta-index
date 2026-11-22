//! Structural SDK custody tests. Authentic Original funding is supplied by the
//! upstream owner; this scope observes both lifetimes without minting authority.

use super::*;
use crate::native_uds::admitted_reply_v1 as decoder;
use quanta_index_contract::{
    AuxEpochV1, FiniteScore, ManifestGeneration, QueryConstraintSetV1, RepoId, RevisionId,
    TextQuerySyntax,
    index::{
        Digest, HistoryGenerationInputV1, HistorySourceCutV1, IndexInstanceId, InstanceSpec,
        ObjectRef, OwnerKey, ViewSelection,
    },
    native_protocol::{
        NativeIndexErrorCode, NativeIndexRequestEnvelope, NativeIndexResponseEnvelope,
        NativeIndexWireError, NativeReadSessionHistoryCutV1, NativeReadSessionHistoryGenerationV1,
        NativeTextCoverage, NativeTextHit, NativeTextResult,
    },
};
use quanta_index_ipc::{
    IndexError, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScopePort,
};
use std::{
    io::Write,
    os::unix::net::UnixListener,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Default)]
struct Counts {
    active_temporary: AtomicU64,
    active_retained: AtomicU64,
    temporary_calls: AtomicU64,
    retained_calls: AtomicU64,
    deny_temporary: AtomicU64,
    deny_retained: AtomicU64,
    denied: AtomicU64,
}
struct Grant {
    counts: Arc<Counts>,
    lifetime: MemoryLifetime,
}
impl MemoryLeaseOwner for Grant {
    fn release(&self, bytes: u64) {
        let active = match self.lifetime {
            MemoryLifetime::Temporary => &self.counts.active_temporary,
            MemoryLifetime::Retained => &self.counts.active_retained,
        };
        assert!(active.fetch_sub(bytes, Ordering::SeqCst) >= bytes);
    }
}
struct Scope(Arc<Counts>);
impl MemoryScopePort for Scope {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let (calls, deny, active) = match lifetime {
            MemoryLifetime::Temporary => (
                &self.0.temporary_calls,
                &self.0.deny_temporary,
                &self.0.active_temporary,
            ),
            MemoryLifetime::Retained => (
                &self.0.retained_calls,
                &self.0.deny_retained,
                &self.0.active_retained,
            ),
        };
        let ordinal = calls.fetch_add(1, Ordering::SeqCst) + 1;
        if ordinal == deny.load(Ordering::SeqCst) {
            let _previous = self.0.denied.fetch_add(1, Ordering::SeqCst);
            return Err(IndexError::ResourceLimited);
        }
        let _previous = active.fetch_add(bytes, Ordering::SeqCst);
        Ok(MemoryLease::new(
            bytes,
            Arc::new(Grant {
                counts: Arc::clone(&self.0),
                lifetime,
            }),
        ))
    }
}
fn scope() -> (MemoryScope, Arc<Counts>) {
    let counts = Arc::new(Counts::default());
    (
        MemoryScope::new(Arc::new(Scope(Arc::clone(&counts)))),
        counts,
    )
}
fn empty(counts: &Counts) {
    assert_eq!(counts.active_temporary.load(Ordering::SeqCst), 0);
    assert_eq!(counts.active_retained.load(Ordering::SeqCst), 0);
}
fn budget() -> RequestBudgetV1 {
    RequestBudgetV1::for_duration(Duration::from_secs(5))
}
fn text_query() -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "alpha".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 1,
        cursor: None,
    }
}
fn semantic_query() -> SemanticQueryRequest {
    SemanticQueryRequest {
        query_text: "query".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        lexical_scope: None,
        top_k: 2,
    }
}

fn exchange(
    expected: NativeIndexRequest,
    response: NativeIndexResponse,
    call: impl FnOnce(
        &NativeUdsClient<std::path::PathBuf>,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError>,
) -> Result<Result<AdmittedNativePublicationReply, NativeUdsError>, Box<dyn std::error::Error>> {
    exchange_with_source(expected, response, None, call)
}

fn exchange_with_source(
    expected: NativeIndexRequest,
    response: NativeIndexResponse,
    source: Option<Arc<dyn crate::DiagnosticAttemptSourceV1>>,
    call: impl FnOnce(
        &NativeUdsClient<std::path::PathBuf>,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError>,
) -> Result<Result<AdmittedNativePublicationReply, NativeUdsError>, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("admitted-read.sock");
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    let server = std::thread::spawn(move || -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
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
        assert_eq!(request.payload, expected);
        let encoded = quanta_index_ipc::encode_response(&NativeIndexResponseEnvelope {
            request_id: request.request_id,
            payload: response,
        })
        .map_err(|error| error.to_string())?;
        stream
            .write_all(&encoded)
            .map_err(|error| error.to_string())
    });
    let mut client = NativeUdsClient::new(path);
    if let Some(source) = source {
        client.set_diagnostic_attempt_source(source);
    }
    let result = call(&client);
    server
        .join()
        .map_err(|_panic| "admitted read peer panicked")??;
    Ok(result)
}

#[derive(Debug, Default)]
struct DiagnosticCounts {
    started: AtomicU64,
    ok: AtomicU64,
    error: AtomicU64,
    cancelled: AtomicU64,
}
#[derive(Debug)]
struct DiagnosticGuard(Arc<DiagnosticCounts>);
impl crate::DiagnosticAttemptSourceV1 for Arc<DiagnosticCounts> {
    fn begin_attempt(&self) -> Option<Box<dyn crate::DiagnosticAttemptGuardV1>> {
        let _previous = self.started.fetch_add(1, Ordering::SeqCst);
        Some(Box::new(DiagnosticGuard(Arc::clone(self))))
    }
}
impl crate::DiagnosticAttemptGuardV1 for DiagnosticGuard {
    fn token(&self) -> quanta_index_contract::DiagnosticAttemptTokenV1 {
        quanta_index_contract::DiagnosticAttemptTokenV1::new([31; 16], std::num::NonZeroU64::MIN)
    }
    fn finish(self: Box<Self>, outcome: quanta_index_contract::DiagnosticAttemptOutcomeV1) {
        use quanta_index_contract::DiagnosticAttemptOutcomeV1 as O;
        let counter = match outcome {
            O::Ok => &self.0.ok,
            O::Error => &self.0.error,
            O::Cancelled => &self.0.cancelled,
        };
        let _previous = counter.fetch_add(1, Ordering::SeqCst);
    }
}

fn charged_borrow(reply: &AdmittedNativePublicationReply, counts: &Counts) -> TestResult {
    assert!(counts.active_retained.load(Ordering::SeqCst) > 0);
    assert_eq!(counts.active_temporary.load(Ordering::SeqCst), 0);
    let text_heap = |rows: &Vec<NativeTextHit>| -> usize {
        rows.capacity() * std::mem::size_of::<NativeTextHit>()
            + rows.iter().map(|row| row.path.capacity()).sum::<usize>()
    };
    let heap = match reply.response() {
        NativeIndexResponse::ReadSessionProfile(profile) => {
            profile.profile.instances.capacity() * std::mem::size_of::<InstanceSpec>()
        }
        NativeIndexResponse::HistoryCut(result) => {
            result.cut.generation.repo_id.as_str().len()
                + result.cut.generation.revision_id.as_str().len()
        }
        NativeIndexResponse::TextRanked(result) => text_heap(&result.hits),
        NativeIndexResponse::TextRankedPage(result) => text_heap(&result.hits),
        NativeIndexResponse::HybridFileRanked(result) => {
            text_heap(&result.lexical_pool)
                + result.dense_pool.capacity()
                    * std::mem::size_of::<
                        quanta_index_contract::native_protocol::NativeSemanticFileHit,
                    >()
                + result
                    .dense_pool
                    .iter()
                    .map(|row| row.path.capacity() + row.vector_id.capacity())
                    .sum::<usize>()
                + result.hits.capacity()
                    * std::mem::size_of::<quanta_index_contract::native_protocol::NativeHybridFileHit>(
                    )
        }
        NativeIndexResponse::Error(error) => error.message.capacity(),
        NativeIndexResponse::ReadSession(_) | NativeIndexResponse::HistoryGeneration(_) => 0,
        _ => return Err("unexpected charged read variant".into()),
    };
    assert!(reply._decoded.bytes() >= heap as u64);
    assert!(
        counts.active_retained.load(Ordering::SeqCst)
            >= reply._decoded.bytes() + reply._body.as_slice().len() as u64
    );
    // Streaming serialization observes the original DATA; no owned extraction.
    ciborium::ser::into_writer(reply.response(), std::io::sink())?;
    assert!(counts.active_retained.load(Ordering::SeqCst) > 0);
    Ok(())
}

#[test]
fn admitted_history_cut_retains_reply_and_rejects_wrong_view() -> TestResult {
    let (status, _) = super::super::profile::tests::fixture()?;
    let instance = IndexInstanceId([8; 16]);
    let cut = NativeReadSessionHistoryCutV1 {
        session: status.session,
        view: status.view,
        instance,
        member: ObjectRef(Digest([9; 32])),
        cut: HistorySourceCutV1 {
            generation: quanta_index_contract::index::AuxiliaryGenerationKeyV1 {
                repo_id: RepoId::new("repo-history-sdk")?,
                revision_id: RevisionId::new("revision-history-sdk")?,
                generation: ManifestGeneration::new(4),
            },
            durable_epoch: AuxEpochV1::new(2),
            snapshot_digest: Digest([10; 32]),
            replay_start: Digest([11; 32]),
        },
    };
    let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadHistoryCut {
        session: status.session,
        instance,
    });
    let (memory, counts) = scope();
    let reply = exchange(
        request.clone(),
        NativeIndexResponse::HistoryCut(cut.clone()),
        |client| {
            client.read_history_cut_in_read_session_admitted(&status, instance, &budget(), &memory)
        },
    )??;
    assert_eq!(
        reply.response(),
        &NativeIndexResponse::HistoryCut(cut.clone())
    );
    charged_borrow(&reply, &counts)?;
    drop(reply);
    empty(&counts);

    let mut wrong_view = cut;
    wrong_view.view.sequence += 1;
    let rejected = exchange(
        request,
        NativeIndexResponse::HistoryCut(wrong_view),
        |client| {
            client.read_history_cut_in_read_session_admitted(&status, instance, &budget(), &memory)
        },
    )?;
    assert!(matches!(rejected, Err(NativeUdsError::Protocol(_))));
    empty(&counts);

    let source_input = ObjectRef(Digest([12; 32]));
    let result = NativeReadSessionHistoryGenerationV1 {
        session: status.session,
        view: status.view,
        instance,
        member: ObjectRef(Digest([9; 32])),
        source_input,
        predecessor_context: Digest([16; 32]),
        inputs: HistoryGenerationInputV1 {
            original: ObjectRef(Digest([13; 32])),
            repo_commit_recency: Some(ObjectRef(Digest([14; 32]))),
            file_contributor: Some(ObjectRef(Digest([15; 32]))),
        },
    };
    let request =
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadHistoryGeneration {
            session: status.session,
            instance,
        });
    let reply = exchange(
        request.clone(),
        NativeIndexResponse::HistoryGeneration(result),
        |client| {
            client.read_history_generation_in_read_session_admitted(
                &status,
                instance,
                source_input,
                &budget(),
                &memory,
            )
        },
    )??;
    assert_eq!(
        reply.response(),
        &NativeIndexResponse::HistoryGeneration(result)
    );
    charged_borrow(&reply, &counts)?;
    drop(reply);
    empty(&counts);
    let rejected = exchange(
        request,
        NativeIndexResponse::HistoryGeneration(result),
        |client| {
            client.read_history_generation_in_read_session_admitted(
                &status,
                instance,
                ObjectRef(Digest([16; 32])),
                &budget(),
                &memory,
            )
        },
    )?;
    assert!(matches!(rejected, Err(NativeUdsError::Protocol(_))));
    empty(&counts);
    Ok(())
}

#[test]
fn admitted_read_raw_existing_route_keeps_nonempty_profile_owner() -> TestResult {
    let (status, profile) = super::super::profile::tests::fixture()?;
    let (memory, counts) = scope();
    let payload = NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile {
        session: status.session,
    });
    let diagnostics = Arc::new(DiagnosticCounts::default());
    let reply = exchange_with_source(
        payload.clone(),
        NativeIndexResponse::ReadSessionProfile(profile.clone()),
        Some(Arc::new(Arc::clone(&diagnostics))),
        |client| client.request_publication_admitted(payload, &budget(), &memory),
    )??;
    assert_eq!(
        reply.response(),
        &NativeIndexResponse::ReadSessionProfile(profile)
    );
    charged_borrow(&reply, &counts)?;
    drop(reply);
    empty(&counts);
    assert_eq!(diagnostics.ok.load(Ordering::SeqCst), 1);
    for code in [
        NativeIndexErrorCode::ResourceLimited,
        NativeIndexErrorCode::Cancelled,
    ] {
        let payload = NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile {
            session: status.session,
        });
        let error = NativeIndexWireError {
            code,
            message: "charged refusal".into(),
        };
        let reply = exchange_with_source(
            payload.clone(),
            NativeIndexResponse::Error(error.clone()),
            Some(Arc::new(Arc::clone(&diagnostics))),
            |client| client.request_publication_admitted(payload, &budget(), &memory),
        )??;
        assert_eq!(reply.response(), &NativeIndexResponse::Error(error));
        charged_borrow(&reply, &counts)?;
        drop(reply);
        empty(&counts);
    }
    assert_eq!(diagnostics.started.load(Ordering::SeqCst), 3);
    assert_eq!(diagnostics.error.load(Ordering::SeqCst), 1);
    assert_eq!(diagnostics.cancelled.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn admitted_read_profile_retains_full_spec_and_ack_survives_profile_refusal() -> TestResult {
    let (status, profile) = super::super::profile::tests::fixture()?;
    let (memory, counts) = scope();
    let selection = ViewSelection::Exact(status.view);
    let key = [21; 32];
    let ack = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::Open {
            selection,
            open_key: key,
        }),
        NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(status)),
        |client| client.open_read_session_admitted(selection, key, &budget(), &memory),
    )??;
    let reply = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile {
            session: status.session,
        }),
        NativeIndexResponse::ReadSessionProfile(profile.clone()),
        |client| client.read_session_profile_admitted(&status, &budget(), &memory),
    )??;
    assert_eq!(
        reply.response(),
        &NativeIndexResponse::ReadSessionProfile(profile)
    );
    charged_borrow(&reply, &counts)?;
    drop(reply);
    let error = NativeIndexWireError {
        code: NativeIndexErrorCode::ResourceLimited,
        message: "profile denied".into(),
    };
    let refusal = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile {
            session: status.session,
        }),
        NativeIndexResponse::Error(error.clone()),
        |client| client.read_session_profile_admitted(&status, &budget(), &memory),
    )??;
    assert_eq!(refusal.response(), &NativeIndexResponse::Error(error));
    charged_borrow(&refusal, &counts)?;
    assert_eq!(
        ack.response(),
        &NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(status))
    );
    let released = NativeReadSessionStatus {
        state: NativeReadSessionState::Released,
        ..status
    };
    let release = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::Release(status.session)),
        NativeIndexResponse::ReadSession(NativeReadSessionReply::Released {
            status: released,
            has_remaining_target_pins: true,
        }),
        |client| client.release_read_session_admitted(&status, &budget(), &memory),
    )??;
    assert_eq!(
        release.response(),
        &NativeIndexResponse::ReadSession(NativeReadSessionReply::Released {
            status: released,
            has_remaining_target_pins: true
        })
    );
    drop((release, refusal, ack));
    empty(&counts);
    Ok(())
}

#[test]
fn admitted_read_text_page_and_hybrid_preserve_fixed_nonempty_gold() -> TestResult {
    let (status, mut page, instance, _) = super::super::page::tests::fixture()?;
    page.after
        .as_mut()
        .ok_or("fixture continuation absent")?
        .query =
        quanta_index_contract::native_protocol::native_text_query_digest(instance, &text_query())?;
    let text = NativeTextResult {
        view: status.view,
        instance,
        coverage: NativeTextCoverage::CoreRequired,
        artifact: page.artifact,
        matched: 1,
        exhaustive: true,
        hits: page.hits.clone(),
    };
    let (memory, counts) = scope();
    let query = text_query();
    let reply = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText {
            session: status.session,
            instance,
            query: query.clone(),
        }),
        NativeIndexResponse::TextRanked(text.clone()),
        |client| {
            client.search_text_in_read_session_admitted(
                &status,
                instance,
                query,
                &budget(),
                &memory,
            )
        },
    )??;
    assert_eq!(reply.response(), &NativeIndexResponse::TextRanked(text));
    charged_borrow(&reply, &counts)?;
    drop(reply);
    empty(&counts);
    let query = text_query();
    let reply = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
            session: status.session,
            instance,
            query: query.clone(),
            after: None,
        }),
        NativeIndexResponse::TextRankedPage(page.clone()),
        |client| {
            client.search_text_page_in_read_session_admitted(
                &status,
                instance,
                query,
                None,
                &budget(),
                &memory,
            )
        },
    )??;
    assert_eq!(reply.response(), &NativeIndexResponse::TextRankedPage(page));
    charged_borrow(&reply, &counts)?;
    drop(reply);
    empty(&counts);
    let (status, mut hybrid) = super::super::hybrid::tests::fixture()?;
    // One file participates in both pools: independent RRF(1,1)=2/61.
    hybrid.lexical_matched = 1;
    hybrid.lexical_pool.push(NativeTextHit {
        owner: hybrid.dense_pool[0].owner,
        source: hybrid.dense_pool[0].source,
        path: "src/a.rs".into(),
        score: FiniteScore::new(0.5)?,
    });
    hybrid.hits[0].lexical_rank = Some(1);
    hybrid.hits[0].score = FiniteScore::new(2.0 / 61.0)?;
    let text_instance = hybrid.text_instance;
    let semantic_instance = hybrid.semantic_instance;
    let text_query = text_query();
    let semantic_query = semantic_query();
    let reply = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHybridFile {
            session: status.session,
            text_instance,
            semantic_instance,
            text_query: text_query.clone(),
            semantic_query: semantic_query.clone(),
            file_top_k: 1,
        }),
        NativeIndexResponse::HybridFileRanked(hybrid.clone()),
        |client| {
            client.search_hybrid_file_in_read_session_admitted(
                &status,
                text_instance,
                semantic_instance,
                text_query,
                semantic_query,
                1,
                &budget(),
                &memory,
            )
        },
    )??;
    assert_eq!(
        reply.response(),
        &NativeIndexResponse::HybridFileRanked(hybrid)
    );
    charged_borrow(&reply, &counts)?;
    drop(reply);
    empty(&counts);
    Ok(())
}

#[test]
fn admitted_read_parser_and_validator_denials_fail_closed_before_scratch_birth() -> TestResult {
    let (status, profile) = super::super::profile::tests::fixture()?;
    let (memory, counts) = scope();
    let client = NativeUdsClient::new("/no/admitted-read-listener.sock");
    counts.deny_temporary.store(1, Ordering::SeqCst);
    assert!(
        client
            .search_text_in_read_session_admitted(
                &status,
                IndexInstanceId([7; 16]),
                text_query(),
                &budget(),
                &memory
            )
            .is_err()
    );
    assert_eq!(counts.denied.load(Ordering::SeqCst), 1);
    assert_eq!(counts.retained_calls.load(Ordering::SeqCst), 0);
    empty(&counts);
    // Each validator has exactly one typed scratch grant before constructing
    // its Vec; use direct independent fixtures to isolate each denial boundary.
    let (memory, counts) = scope();
    counts.deny_temporary.store(1, Ordering::SeqCst);
    assert!(
        super::super::profile::validate_profile_with_memory(&status, &profile, Some(&memory))
            .is_err()
    );
    assert_eq!(counts.denied.load(Ordering::SeqCst), 1);
    empty(&counts);
    let (status, page, instance, digest) = super::super::page::tests::fixture()?;
    let (memory, counts) = scope();
    counts.deny_temporary.store(1, Ordering::SeqCst);
    assert!(
        super::super::page::validate_page_with_memory(
            &status,
            instance,
            1,
            digest,
            None,
            &QueryConstraintSetV1::unconstrained(),
            &page,
            Some(&memory)
        )
        .is_err()
    );
    assert_eq!(counts.denied.load(Ordering::SeqCst), 1);
    empty(&counts);
    let result = NativeTextResult {
        view: status.view,
        instance,
        coverage: NativeTextCoverage::CoreRequired,
        artifact: page.artifact,
        matched: 1,
        exhaustive: true,
        hits: page.hits,
    };
    let (memory, counts) = scope();
    counts.deny_temporary.store(1, Ordering::SeqCst);
    assert!(
        validate_text_result_with_memory(
            ViewSelection::Exact(status.view),
            instance,
            1,
            &QueryConstraintSetV1::unconstrained(),
            &result,
            Some(&memory)
        )
        .is_err()
    );
    assert_eq!(counts.denied.load(Ordering::SeqCst), 1);
    empty(&counts);
    let (status, hybrid) = super::super::hybrid::tests::fixture()?;
    let (memory, counts) = scope();
    counts.deny_temporary.store(1, Ordering::SeqCst);
    assert!(
        super::super::hybrid::validate_hybrid_reply_with_memory(
            &status,
            hybrid.text_instance,
            hybrid.semantic_instance,
            1,
            2,
            1,
            &QueryConstraintSetV1::unconstrained(),
            &hybrid,
            Some(&memory)
        )
        .is_err()
    );
    assert_eq!(counts.denied.load(Ordering::SeqCst), 1);
    empty(&counts);
    Ok(())
}

#[test]
fn admitted_read_decoded_denial_drops_raw_and_cancel_never_sends() -> TestResult {
    let (status, profile) = super::super::profile::tests::fixture()?;
    let (memory, counts) = scope();
    // Existing IPC grants: admitted request frame, raw response, then decoded.
    counts.deny_retained.store(3, Ordering::SeqCst);
    let reply = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile {
            session: status.session,
        }),
        NativeIndexResponse::ReadSessionProfile(profile),
        |client| client.read_session_profile_admitted(&status, &budget(), &memory),
    )?;
    assert!(reply.is_err());
    assert_eq!(counts.denied.load(Ordering::SeqCst), 1);
    empty(&counts);
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("no-send.sock");
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    let diagnostics = Arc::new(DiagnosticCounts::default());
    let mut client = NativeUdsClient::new(path);
    client.set_diagnostic_attempt_source(Arc::new(Arc::clone(&diagnostics)));
    let budget = budget();
    budget.cancel_handle().cancel();
    let (memory, counts) = scope();
    assert!(
        client
            .open_read_session_admitted(
                ViewSelection::Exact(status.view),
                [21; 32],
                &budget,
                &memory
            )
            .is_err()
    );
    assert!(
        client
            .read_session_profile_admitted(&status, &budget, &memory)
            .is_err()
    );
    assert!(
        client
            .search_text_in_read_session_admitted(
                &status,
                IndexInstanceId([7; 16]),
                text_query(),
                &budget,
                &memory
            )
            .is_err()
    );
    assert!(
        client
            .search_text_page_in_read_session_admitted(
                &status,
                IndexInstanceId([7; 16]),
                text_query(),
                None,
                &budget,
                &memory
            )
            .is_err()
    );
    assert!(
        client
            .search_hybrid_file_in_read_session_admitted(
                &status,
                IndexInstanceId([7; 16]),
                IndexInstanceId([8; 16]),
                text_query(),
                semantic_query(),
                1,
                &budget,
                &memory
            )
            .is_err()
    );
    assert!(
        client
            .release_read_session_admitted(&status, &budget, &memory)
            .is_err()
    );
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert_eq!(diagnostics.started.load(Ordering::SeqCst), 0);
    assert_eq!(counts.temporary_calls.load(Ordering::SeqCst), 0);
    assert_eq!(counts.retained_calls.load(Ordering::SeqCst), 0);
    empty(&counts);
    Ok(())
}

#[test]
fn admitted_read_preflight_rejects_oversize_and_truncated_before_typed_decode() -> TestResult {
    let (status, mut profile) = super::super::profile::tests::fixture()?;
    let mut bytes = Vec::new();
    profile.profile.instances = vec![profile.profile.instances[0].clone(); 4097];
    ciborium::ser::into_writer(
        &NativeIndexResponseEnvelope {
            request_id: 1,
            payload: NativeIndexResponse::ReadSessionProfile(profile),
        },
        &mut bytes,
    )?;
    assert!(decoder::prepare(&bytes, decoder::ReplyMode::ReadSession).is_err());
    let (_, mut page, _, _) = super::super::page::tests::fixture()?;
    page.hits[0].path = "x".repeat(quanta_index_contract::index::MAX_SOURCE_PATH_BYTES + 1);
    bytes.clear();
    ciborium::ser::into_writer(
        &NativeIndexResponseEnvelope {
            request_id: 1,
            payload: NativeIndexResponse::TextRankedPage(page),
        },
        &mut bytes,
    )?;
    assert!(decoder::prepare(&bytes, decoder::ReplyMode::ReadSession).is_err());
    bytes.clear();
    ciborium::ser::into_writer(
        &NativeIndexResponseEnvelope {
            request_id: 1,
            payload: NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(status)),
        },
        &mut bytes,
    )?;
    assert!(decoder::prepare(&bytes[..bytes.len() - 1], decoder::ReplyMode::ReadSession).is_err());
    assert!(decoder::prepare(&bytes, decoder::ReplyMode::Publication).is_err());
    // Integer-only fixed ID arrays cannot hide sequence-form records/strings.
    let (_, profile) = super::super::profile::tests::fixture()?;
    bytes.clear();
    ciborium::ser::into_writer(
        &NativeIndexResponseEnvelope {
            request_id: 1,
            payload: NativeIndexResponse::ReadSessionProfile(profile),
        },
        &mut bytes,
    )?;
    let prepared = decoder::prepare(&bytes, decoder::ReplyMode::ReadSession)?;
    assert!(prepared.decoded_retained_limit()? >= std::mem::size_of::<InstanceSpec>() as u64);
    assert!(matches!(
        prepared.decode()?.payload,
        NativeIndexResponse::ReadSessionProfile(_)
    ));
    Ok(())
}

#[test]
fn admitted_read_typed_routes_reject_wrong_binding_filter_and_fused_rank() -> TestResult {
    let (status, mut profile) = super::super::profile::tests::fixture()?;
    profile.session = NativeReadSessionId([99; 32]);
    let (memory, counts) = scope();
    let reply = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile {
            session: status.session,
        }),
        NativeIndexResponse::ReadSessionProfile(profile),
        |client| client.read_session_profile_admitted(&status, &budget(), &memory),
    )?;
    assert!(matches!(reply, Err(NativeUdsError::Protocol(_))));
    empty(&counts);
    let (status, mut page, instance, _) = super::super::page::tests::fixture()?;
    let mut query = text_query();
    query.constraints.repo_relative_path_exact = Some(
        quanta_index_contract::ExactRepoRelativePathV1::new("src/expected.rs")?,
    );
    let text = NativeTextResult {
        view: status.view,
        instance,
        coverage: NativeTextCoverage::CoreRequired,
        artifact: page.artifact,
        matched: 1,
        exhaustive: true,
        hits: page.hits.clone(),
    };
    let reply = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText {
            session: status.session,
            instance,
            query: query.clone(),
        }),
        NativeIndexResponse::TextRanked(text),
        |client| {
            client.search_text_in_read_session_admitted(
                &status,
                instance,
                query,
                &budget(),
                &memory,
            )
        },
    )?;
    assert!(matches!(reply, Err(NativeUdsError::Protocol(_))));
    empty(&counts);
    page.view.sequence += 1;
    let query = text_query();
    let reply = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
            session: status.session,
            instance,
            query: query.clone(),
            after: None,
        }),
        NativeIndexResponse::TextRankedPage(page),
        |client| {
            client.search_text_page_in_read_session_admitted(
                &status,
                instance,
                query,
                None,
                &budget(),
                &memory,
            )
        },
    )?;
    assert!(matches!(reply, Err(NativeUdsError::Protocol(_))));
    empty(&counts);
    let (status, mut hybrid) = super::super::hybrid::tests::fixture()?;
    hybrid.hits[0].score = FiniteScore::new(0.5)?; // Fixed independent gold is 1/61.
    let text_instance = hybrid.text_instance;
    let semantic_instance = hybrid.semantic_instance;
    let text_query = text_query();
    let semantic_query = semantic_query();
    let reply = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHybridFile {
            session: status.session,
            text_instance,
            semantic_instance,
            text_query: text_query.clone(),
            semantic_query: semantic_query.clone(),
            file_top_k: 1,
        }),
        NativeIndexResponse::HybridFileRanked(hybrid),
        |client| {
            client.search_hybrid_file_in_read_session_admitted(
                &status,
                text_instance,
                semantic_instance,
                text_query,
                semantic_query,
                1,
                &budget(),
                &memory,
            )
        },
    )?;
    assert!(matches!(reply, Err(NativeUdsError::Protocol(_))));
    empty(&counts);
    Ok(())
}

#[test]
fn admitted_read_regex_preparation_overflow_and_empty_results_keep_custody() -> TestResult {
    let (status, profile) = super::super::profile::tests::fixture()?;
    let (memory, counts) = scope();
    counts.deny_temporary.store(2, Ordering::SeqCst);
    let mut query = text_query();
    query.query_text = "/alpha/".into();
    assert!(prepare_text(&status, &query, Some(&memory)).is_err());
    assert_eq!(counts.denied.load(Ordering::SeqCst), 1);
    empty(&counts);
    let (memory, counts) = scope();
    assert!(crate::native_uds::temporary_rows::<OwnerKey>(Some(&memory), usize::MAX).is_err());
    assert_eq!(counts.temporary_calls.load(Ordering::SeqCst), 0);
    query.query_text = "x".repeat(quanta_index_contract::MAX_NATIVE_CONTENT_INPUT_BYTES + 1);
    assert!(prepare_text(&status, &query, Some(&memory)).is_err());
    assert_eq!(counts.temporary_calls.load(Ordering::SeqCst), 0);
    empty(&counts);
    // Empty query result has no scratch backing; zero-byte grants are invalid
    // at the canonical authority and must never be requested by the consumer.
    let instance = profile.profile.instances[0].instance;
    let result = NativeTextResult {
        view: status.view,
        instance,
        coverage: NativeTextCoverage::CoreRequired,
        artifact: profile.view.profile,
        matched: 0,
        exhaustive: true,
        hits: Vec::new(),
    };
    let query = text_query();
    let reply = exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText {
            session: status.session,
            instance,
            query: query.clone(),
        }),
        NativeIndexResponse::TextRanked(result),
        |client| {
            client.search_text_in_read_session_admitted(
                &status,
                instance,
                query,
                &budget(),
                &memory,
            )
        },
    )??;
    charged_borrow(&reply, &counts)?;
    drop(reply);
    empty(&counts);
    Ok(())
}

#[test]
fn admitted_read_expired_budget_refuses_before_parser_admission_and_send() -> TestResult {
    let (status, _) = super::super::profile::tests::fixture()?;
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("expired-no-send.sock");
    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(true)?;
    let mut client = NativeUdsClient::new(path);
    let diagnostics = Arc::new(DiagnosticCounts::default());
    client.set_diagnostic_attempt_source(Arc::new(Arc::clone(&diagnostics)));
    let expired = RequestBudgetV1::for_duration(Duration::ZERO);
    let (memory, counts) = scope();
    let instance = IndexInstanceId([7; 16]);
    let replies = [
        client.open_read_session_admitted(
            ViewSelection::Exact(status.view),
            [21; 32],
            &expired,
            &memory,
        ),
        client.read_session_profile_admitted(&status, &expired, &memory),
        client.search_text_in_read_session_admitted(
            &status,
            instance,
            text_query(),
            &expired,
            &memory,
        ),
        client.search_text_page_in_read_session_admitted(
            &status,
            instance,
            text_query(),
            None,
            &expired,
            &memory,
        ),
        client.search_hybrid_file_in_read_session_admitted(
            &status,
            instance,
            IndexInstanceId([8; 16]),
            text_query(),
            semantic_query(),
            1,
            &expired,
            &memory,
        ),
        client.release_read_session_admitted(&status, &expired, &memory),
    ];
    assert!(
        replies
            .iter()
            .all(|reply| matches!(reply, Err(NativeUdsError::Transport(_)))),
        "all six expired reads must report deadline refusal"
    );
    drop(replies);
    empty(&counts);
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let original_deadline_refused = observed_decode_deadline_refused()?;
    deadline_cause_is_classified_once()?;
    assert_eq!(
        counts.temporary_calls.load(Ordering::SeqCst),
        0,
        "an expired request must not construct a token/AST/regex plan"
    );
    assert_eq!(counts.retained_calls.load(Ordering::SeqCst), 0);
    assert_eq!(diagnostics.started.load(Ordering::SeqCst), 0);
    assert!(
        original_deadline_refused,
        "decoded DATA escaped after the Original deadline observer terminated"
    );
    Ok(())
}

struct InterruptAtTemporary {
    memory: MemoryScope,
    ordinal: u64,
    seen: Arc<AtomicU64>,
    interrupt: Box<dyn Fn() + Send + Sync>,
}
impl MemoryScopePort for InterruptAtTemporary {
    fn reserve(&self, bytes: u64, lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let lease = self.memory.reserve(bytes, lifetime)?;
        if lifetime == MemoryLifetime::Temporary {
            let ordinal = self.seen.fetch_add(1, Ordering::SeqCst) + 1;
            if ordinal == self.ordinal {
                (self.interrupt)();
            }
        }
        Ok(lease)
    }
}

fn cancelled_during_validation(
    expected: NativeIndexRequest,
    response: NativeIndexResponse,
    ordinal: u64,
    call: impl FnOnce(
        &NativeUdsClient<std::path::PathBuf>,
        &RequestBudgetV1,
        &MemoryScope,
    ) -> Result<AdmittedNativePublicationReply, NativeUdsError>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let (memory, counts) = scope();
    let budget = budget();
    let cancel = budget.cancel_handle();
    let seen = Arc::new(AtomicU64::new(0));
    let interrupted = MemoryScope::new(Arc::new(InterruptAtTemporary {
        memory,
        ordinal,
        seen: Arc::clone(&seen),
        interrupt: Box::new(move || cancel.cancel()),
    }));
    let reply = exchange(expected, response, |client| {
        call(client, &budget, &interrupted)
    })?;
    assert_eq!(
        seen.load(Ordering::SeqCst),
        ordinal,
        "must reach the actual validator scratch grant"
    );
    assert!(budget.is_cancelled());
    let cancelled = matches!(&reply, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Cancelled);
    drop(reply);
    empty(&counts);
    Ok(cancelled)
}

#[test]
fn admitted_read_validator_cancellation_refuses_success_and_releases_all_grants() -> TestResult {
    let mut completed = Vec::new();
    let (status, profile) = super::super::profile::tests::fixture()?;
    // IPC parser is Temporary#1; profile validation scratch is Temporary#2.
    if !cancelled_during_validation(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile {
            session: status.session,
        }),
        NativeIndexResponse::ReadSessionProfile(profile),
        2,
        |client, budget, memory| client.read_session_profile_admitted(&status, budget, memory),
    )? {
        completed.push("Profile");
    }
    let (status, mut page, instance, _) = super::super::page::tests::fixture()?;
    page.matched = 1;
    page.remaining = 1;
    page.after = None;
    let text = NativeTextResult {
        view: status.view,
        instance,
        coverage: NativeTextCoverage::CoreRequired,
        artifact: page.artifact,
        matched: 1,
        exhaustive: true,
        hits: page.hits.clone(),
    };
    let query = text_query();
    // Query tokens, IPC parser, then the actual result validator scratch.
    if !cancelled_during_validation(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText {
            session: status.session,
            instance,
            query: query.clone(),
        }),
        NativeIndexResponse::TextRanked(text),
        3,
        |client, budget, memory| {
            client.search_text_in_read_session_admitted(&status, instance, query, budget, memory)
        },
    )? {
        completed.push("Text");
    }
    let query = text_query();
    if !cancelled_during_validation(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
            session: status.session,
            instance,
            query: query.clone(),
            after: None,
        }),
        NativeIndexResponse::TextRankedPage(page),
        3,
        |client, budget, memory| {
            client.search_text_page_in_read_session_admitted(
                &status, instance, query, None, budget, memory,
            )
        },
    )? {
        completed.push("Page");
    }
    let (status, hybrid) = super::super::hybrid::tests::fixture()?;
    let text_instance = hybrid.text_instance;
    let semantic_instance = hybrid.semantic_instance;
    let text_query = text_query();
    let semantic_query = semantic_query();
    if !cancelled_during_validation(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHybridFile {
            session: status.session,
            text_instance,
            semantic_instance,
            text_query: text_query.clone(),
            semantic_query: semantic_query.clone(),
            file_top_k: 1,
        }),
        NativeIndexResponse::HybridFileRanked(hybrid),
        3,
        |client, budget, memory| {
            client.search_hybrid_file_in_read_session_admitted(
                &status,
                text_instance,
                semantic_instance,
                text_query,
                semantic_query,
                1,
                budget,
                memory,
            )
        },
    )? {
        completed.push("Hybrid");
    }
    assert!(
        completed.is_empty(),
        "successful DATA escaped after actual validator cancellation: {completed:?}"
    );
    Ok(())
}

#[derive(Debug)]
struct DeadlineObserver(Arc<std::sync::atomic::AtomicBool>);
impl quanta_index_ipc::RequestExecutionObserverV1 for DeadlineObserver {
    fn interruption(&self) -> Option<quanta_index_ipc::BudgetInterruptionV1> {
        self.0.load(Ordering::SeqCst).then_some(
            quanta_index_ipc::BudgetInterruptionV1::DeadlineExceeded {
                over_by: Duration::ZERO,
            },
        )
    }
}

fn observed_decode_deadline_refused() -> Result<bool, Box<dyn std::error::Error>> {
    let (status, profile) = super::super::profile::tests::fixture()?;
    let ended = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observer = Arc::new(DeadlineObserver(Arc::clone(&ended)));
    let (initial_memory, initial_counts) = scope();
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or("deadline overflow")?;
    let budget = RequestBudgetV1::until_with_external_observer_admitted(
        deadline,
        observer,
        &initial_memory,
    )?;
    let (memory, counts) = scope();
    let seen = Arc::new(AtomicU64::new(0));
    let ended_at_decode = Arc::clone(&ended);
    let interrupted = MemoryScope::new(Arc::new(InterruptAtTemporary {
        memory,
        ordinal: 1,
        seen: Arc::clone(&seen),
        interrupt: Box::new(move || ended_at_decode.store(true, Ordering::SeqCst)),
    }));
    let request = NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile {
        session: status.session,
    });
    let reply = exchange(
        request.clone(),
        NativeIndexResponse::ReadSessionProfile(profile),
        |client| client.request_publication_admitted(request, &budget, &interrupted),
    )?;
    // Original observers may terminate a phase before its copied wall deadline.
    assert!(ended.load(Ordering::SeqCst));
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    assert!(
        !budget.is_cancelled(),
        "deadline must not be relabelled cancellation"
    );
    let refused = matches!(&reply, Err(NativeUdsError::Transport(_)));
    drop(reply);
    empty(&counts);
    drop(budget);
    empty(&initial_counts);
    Ok(refused)
}

#[derive(Debug)]
struct DeadlineThenCancelObserver {
    armed: Arc<std::sync::atomic::AtomicBool>,
    observed: Arc<AtomicU64>,
}
impl quanta_index_ipc::RequestExecutionObserverV1 for DeadlineThenCancelObserver {
    fn interruption(&self) -> Option<quanta_index_ipc::BudgetInterruptionV1> {
        if !self.armed.load(Ordering::SeqCst) {
            return None;
        }
        let previous = self.observed.fetch_add(1, Ordering::SeqCst);
        Some(if previous == 0 {
            quanta_index_ipc::BudgetInterruptionV1::DeadlineExceeded {
                over_by: Duration::ZERO,
            }
        } else {
            quanta_index_ipc::BudgetInterruptionV1::Cancelled
        })
    }
}

fn deadline_cause_is_classified_once() -> TestResult {
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = Arc::new(AtomicU64::new(0));
    let observer = Arc::new(DeadlineThenCancelObserver {
        armed: Arc::clone(&armed),
        observed: Arc::clone(&observed),
    });
    let (memory, counts) = scope();
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or("deadline overflow")?;
    let budget =
        RequestBudgetV1::until_with_external_observer_admitted(deadline, observer, &memory)?;
    armed.store(true, Ordering::SeqCst);
    let refusal = super::super::admitted_budget_checkpoint(&budget);
    assert!(
        matches!(&refusal, Err(NativeUdsError::Transport(message)) if message.contains("deadline")),
        "the checkpoint's original deadline cause must survive a later cancellation: {refusal:?}"
    );
    assert_eq!(
        observed.load(Ordering::SeqCst),
        1,
        "classify the returned cause without probing the owner again"
    );
    drop(budget);
    empty(&counts);
    Ok(())
}

fn hybrid_with_chunk(
    span: quanta_index_contract::native_protocol::NativeSemanticChunkSpan,
) -> Result<
    (
        NativeReadSessionStatus,
        quanta_index_contract::native_protocol::NativeHybridFileResult,
    ),
    Box<dyn std::error::Error>,
> {
    use quanta_index_contract::{
        index::{Digest, OwnerInputBinding},
        native_protocol::{NativeSemanticChunk, NativeSemanticChunkSpan},
    };
    let (status, mut result) = super::super::hybrid::tests::fixture()?;
    let row = &mut result.dense_pool[0];
    let input = match span {
        NativeSemanticChunkSpan::SourcePayload { .. } => OwnerInputBinding {
            source: Some(row.source),
            projection: None,
            producer_context: Digest([0; 32]),
            derivation_policy: result.semantic_coverage.desired.policy,
        },
        NativeSemanticChunkSpan::ProjectionPayload { .. }
        | NativeSemanticChunkSpan::RenderedMember { .. } => OwnerInputBinding {
            source: Some(row.source),
            projection: Some(quanta_index_contract::index::ObjectRef(Digest([25; 32]))),
            producer_context: Digest([22; 32]),
            derivation_policy: result.semantic_coverage.desired.policy,
        },
    };
    let chunk = NativeSemanticChunk {
        id: Digest([21; 32]),
        payload: quanta_index_contract::index::ObjectRef(Digest([24; 32])),
        input,
        input_digest: Digest([23; 32]),
        span,
    };
    row.vector_id = chunk.id.0.to_vec();
    // Canonical public contract establishes validity independently of the
    // admitted scanner's field allowlist and SDK validation implementation.
    chunk.validate_binding(&row.vector_id, result.semantic_coverage.desired.policy)?;
    row.chunk = Some(chunk);
    Ok((status, result))
}

fn exchange_admitted_hybrid(
    status: &NativeReadSessionStatus,
    result: quanta_index_contract::native_protocol::NativeHybridFileResult,
    memory: &MemoryScope,
) -> Result<Result<AdmittedNativePublicationReply, NativeUdsError>, Box<dyn std::error::Error>> {
    let text_instance = result.text_instance;
    let semantic_instance = result.semantic_instance;
    let text_query = text_query();
    let semantic_query = semantic_query();
    exchange(
        NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHybridFile {
            session: status.session,
            text_instance,
            semantic_instance,
            text_query: text_query.clone(),
            semantic_query: semantic_query.clone(),
            file_top_k: 1,
        }),
        NativeIndexResponse::HybridFileRanked(result),
        |client| {
            client.search_hybrid_file_in_read_session_admitted(
                status,
                text_instance,
                semantic_instance,
                text_query,
                semantic_query,
                1,
                &budget(),
                memory,
            )
        },
    )
}

#[test]
fn admitted_read_hybrid_preserves_canonical_chunk_provenance_and_rejects_bad_binding() -> TestResult
{
    use quanta_index_contract::{index::Digest, native_protocol::NativeSemanticChunkSpan as Span};
    // Every canonical producer span must survive the one admitted decoder.
    for span in [
        Span::SourcePayload {
            start_byte: 0,
            end_byte: 5,
        },
        Span::ProjectionPayload {
            start_byte: 1,
            end_byte: 4,
        },
        Span::RenderedMember { ordinal: 7 },
    ] {
        let (status, expected) = hybrid_with_chunk(span)?;
        let (memory, counts) = scope();
        let reply = exchange_admitted_hybrid(&status, expected.clone(), &memory)??;
        assert_eq!(
            reply.response(),
            &NativeIndexResponse::HybridFileRanked(expected)
        );
        charged_borrow(&reply, &counts)?;
        drop(reply);
        empty(&counts);
    }
    let (status, valid) = hybrid_with_chunk(Span::SourcePayload {
        start_byte: 0,
        end_byte: 5,
    })?;
    // Refusals must come from the same canonical binding validator after
    // successful field decoding, not from a missing schema identifier.
    for invalid in 0..4 {
        let mut result = valid.clone();
        let row = &mut result.dense_pool[0];
        let chunk = row.chunk.as_mut().ok_or("chunk fixture missing")?;
        match invalid {
            0 => row.vector_id[0] ^= 1,
            1 => chunk.input.derivation_policy = Digest([99; 32]),
            2 => {
                chunk.span = Span::SourcePayload {
                    start_byte: 4,
                    end_byte: 1,
                }
            }
            3 => chunk.input.producer_context = Digest([99; 32]),
            _ => unreachable!(),
        }
        let (memory, counts) = scope();
        let refusal = exchange_admitted_hybrid(&status, result, &memory)?;
        assert!(
            matches!(&refusal, Err(NativeUdsError::Protocol(message))
            if message.starts_with("Hybrid vector witness differs:")),
            "canonical binding case {invalid} bypassed validation: {refusal:?}"
        );
        drop(refusal);
        empty(&counts);
    }
    Ok(())
}

#[test]
fn admitted_read_hybrid_preserves_nonempty_partial_and_pending_coverage_roots() -> TestResult {
    use quanta_index_contract::index::{CoverageStatus, Digest, MapRoot, ObjectRef};
    let missing = MapRoot(Some(ObjectRef(Digest([25; 32]))));
    for status in [
        CoverageStatus::Partial { missing },
        CoverageStatus::Pending {
            previous_missing: missing,
        },
    ] {
        let (session, mut expected) = super::super::hybrid::tests::fixture()?;
        // Same canonical coverage shape as the real Kernel's partial metadata:
        // a prior covered input, a live searchable root and nonempty missing.
        let mut covered = expected.semantic_coverage.desired;
        covered.root = MapRoot(Some(ObjectRef(Digest([26; 32]))));
        expected.semantic_coverage.covered = Some(covered);
        expected.semantic_coverage.status = status;
        assert!(
            !expected
                .semantic_traversal
                .certifies_exhaustive(&expected.semantic_coverage, expected.skipped_stale)
        );
        assert!(!expected.semantic_exhaustive);
        assert!(
            expected.dense_pool[0].chunk.is_none(),
            "isolate coverage-root decoding from the chunk case"
        );
        let (memory, counts) = scope();
        let reply = exchange_admitted_hybrid(&session, expected.clone(), &memory)??;
        assert_eq!(
            reply.response(),
            &NativeIndexResponse::HybridFileRanked(expected)
        );
        charged_borrow(&reply, &counts)?;
        drop(reply);
        empty(&counts);
    }
    Ok(())
}

#[test]
fn admitted_read_hybrid_refuses_chunk_from_another_or_absent_source_file() -> TestResult {
    use quanta_index_contract::{
        index::{Digest, ObjectRef},
        native_protocol::NativeSemanticChunkSpan as Span,
    };
    let mut escaped = Vec::new();
    for (label, span) in [
        (
            "source",
            Span::SourcePayload {
                start_byte: 0,
                end_byte: 5,
            },
        ),
        (
            "projection",
            Span::ProjectionPayload {
                start_byte: 1,
                end_byte: 4,
            },
        ),
        ("rendered", Span::RenderedMember { ordinal: 7 }),
    ] {
        let (status, mut valid) = hybrid_with_chunk(span)?;
        let source = valid.dense_pool[0].source;
        // Unlike generic unsourced projection vectors, every actual file-hit
        // producer binds its chunk input to the authenticated source row.
        valid.dense_pool[0]
            .chunk
            .as_mut()
            .ok_or("chunk missing")?
            .input
            .source = Some(source);
        let (memory, counts) = scope();
        let accepted = exchange_admitted_hybrid(&status, valid.clone(), &memory)??;
        assert_eq!(
            accepted.response(),
            &NativeIndexResponse::HybridFileRanked(valid.clone())
        );
        charged_borrow(&accepted, &counts)?;
        drop(accepted);
        empty(&counts);
        for wrong in [Some(ObjectRef(Digest([99; 32]))), None] {
            let mut forged = valid.clone();
            forged.dense_pool[0]
                .chunk
                .as_mut()
                .ok_or("chunk missing")?
                .input
                .source = wrong;
            let (memory, counts) = scope();
            let reply = exchange_admitted_hybrid(&status, forged, &memory)?;
            let refused = matches!(&reply, Err(NativeUdsError::Protocol(message))
                if message.starts_with("Hybrid vector witness differs:"));
            drop(reply);
            empty(&counts);
            if !refused {
                escaped.push((label, wrong));
            }
        }
    }
    assert!(
        escaped.is_empty(),
        "file-hit chunk source mismatches escaped SDK validation: {escaped:?}"
    );
    Ok(())
}
