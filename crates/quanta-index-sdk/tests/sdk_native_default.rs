//! Ordinary SDK default must require authoritative Native selection.

use quanta_index_sdk::{ConnectOptions, QuantaIndex, SdkError};

#[test]
fn ordinary_sdk_requires_native_selection_before_transport_birth()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("native.sock");
    let options = ConnectOptions::default().with_query_socket(&socket);
    assert!(
        matches!(
            QuantaIndex::connect_query_only(options),
            Err(SdkError::Usage(message)) if message.contains("Native") && message.contains("selection")
        ),
        "ordinary query-only constructor accepted absent Native selection"
    );
    let options = ConnectOptions::default()
        .with_query_socket(&socket)
        .with_control_socket(directory.path().join("control.sock"))
        .with_ingest_socket(directory.path().join("ingest.sock"));
    assert!(
        matches!(
            QuantaIndex::connect(options),
            Err(SdkError::Usage(message)) if message.contains("Native") && message.contains("selection")
        ),
        "ordinary full constructor accepted absent Native selection"
    );
    Ok(())
}

#[cfg(unix)]
mod wire {
    use super::*;
    use quanta_index_contract::{
        FiniteScore, QueryConstraintSetV1, SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
        index::{
            CatalogView, CollectionId, CoverageBinding, CoverageStatus, Digest, GroupActivation,
            IndexInstanceId, IndexKind, IndexProfile, InputBinding, InputDomain, InstanceSpec,
            MapRoot, ObjectRef, OwnerKey, ProducerId, PublicationGroupId, SchemaId, SlotId,
            SourceCut, StableOwnerId, StorageFormatId, TenantId, ViewRef, ViewSelection,
            WorkspaceKey, WorktreeId,
        },
        native_protocol::{
            NativeHybridFileHit, NativeHybridFileResult, NativeIndexRequest,
            NativeIndexRequestEnvelope, NativeIndexResponse, NativeIndexResponseEnvelope,
            NativeReadSessionId, NativeReadSessionProfile, NativeReadSessionReply,
            NativeReadSessionRequest, NativeReadSessionState, NativeReadSessionStatus,
            NativeSemanticHit, NativeSemanticResult, NativeSemanticTraversal, NativeTextCoverage,
            NativeTextHit, NativeTextPage, NativeTextResult,
        },
    };
    use quanta_index_ipc::RequestBudgetV1;
    use quanta_index_sdk::NativeQueryError;
    use std::{
        io::Write,
        os::unix::net::UnixListener,
        time::{Duration, Instant},
    };

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
    const OPEN_KEY: [u8; 32] = [77; 32];

    struct Fixture {
        view: CatalogView,
        status: NativeReadSessionStatus,
        text: InstanceSpec,
        semantic: InstanceSpec,
        hit: NativeTextHit,
    }

    impl Fixture {
        fn new() -> TestResult<Self> {
            let mut view = CatalogView {
                identity: ViewRef {
                    workspace: WorkspaceKey {
                        tenant: TenantId([1; 16]),
                        collection: CollectionId([2; 16]),
                        worktree: WorktreeId([3; 16]),
                        slot: SlotId([4; 16]),
                    },
                    epoch: 9,
                    sequence: 11,
                    digest: Digest([0; 32]),
                },
                profile: ObjectRef(Digest([5; 32])),
                source: SourceCut {
                    owners: MapRoot(None),
                    paths: MapRoot(None),
                    frontiers: MapRoot(None),
                },
                projections: MapRoot(None),
                members: MapRoot(None),
                coverage: MapRoot(None),
            };
            view.identity.digest = view.identity_digest()?;
            let text = InstanceSpec {
                instance: IndexInstanceId([6; 16]),
                kind: IndexKind([7; 16]),
                group: PublicationGroupId([8; 16]),
                activation: GroupActivation::CoreRequired,
                input_domain: InputDomain::SourceLocal,
                schema: SchemaId([9; 16]),
                policy: Digest([10; 32]),
                format: StorageFormatId([11; 16]),
            };
            let mut semantic = text.clone();
            semantic.instance = IndexInstanceId([12; 16]);
            semantic.kind = IndexKind([13; 16]);
            semantic.group = PublicationGroupId([14; 16]);
            semantic.activation = GroupActivation::AsyncCoverage;
            let status = NativeReadSessionStatus {
                session: NativeReadSessionId([15; 32]),
                view: view.identity,
                expires_at_ms: 50_000,
                revision: 1,
                state: NativeReadSessionState::Active,
            };
            let hit = NativeTextHit {
                owner: OwnerKey {
                    producer: ProducerId([16; 16]),
                    stable_id: StableOwnerId([17; 16]),
                },
                source: ObjectRef(Digest([18; 32])),
                path: "src/gold.rs".into(),
                score: FiniteScore::new(2.5)?,
            };
            Ok(Self {
                view,
                status,
                text,
                semantic,
                hit,
            })
        }
        fn selection(&self) -> ViewSelection {
            ViewSelection::Exact(self.view.identity)
        }
        fn profile(&self) -> NativeIndexResponse {
            NativeIndexResponse::ReadSessionProfile(NativeReadSessionProfile {
                session: self.status.session,
                view: self.view.clone(),
                profile: IndexProfile {
                    instances: {
                        // Same-kind decoy deliberately precedes the configured instance.
                        let mut decoy = self.text.clone();
                        decoy.instance = IndexInstanceId([91; 16]);
                        decoy.policy = Digest([92; 32]);
                        vec![decoy, self.text.clone(), self.semantic.clone()]
                    },
                },
            })
        }
        fn open(&self) -> NativeIndexRequest {
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::Open {
                selection: self.selection(),
                open_key: OPEN_KEY,
            })
        }
        fn read_profile(&self) -> NativeIndexRequest {
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile {
                session: self.status.session,
            })
        }
        fn release(&self) -> (NativeIndexRequest, NativeIndexResponse) {
            let mut status = self.status;
            status.state = NativeReadSessionState::Released;
            (
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::Release(status.session)),
                NativeIndexResponse::ReadSession(NativeReadSessionReply::Released {
                    status,
                    has_remaining_target_pins: true,
                }),
            )
        }
        fn text_result(&self) -> NativeTextResult {
            NativeTextResult {
                view: self.view.identity,
                instance: self.text.instance,
                coverage: NativeTextCoverage::CoreRequired,
                artifact: ObjectRef(Digest([19; 32])),
                matched: 1,
                exhaustive: true,
                hits: vec![self.hit.clone()],
            }
        }
        fn semantic_result(&self) -> TestResult<NativeSemanticResult> {
            let desired = InputBinding {
                root: MapRoot(Some(ObjectRef(Digest([21; 32])))),
                schema: self.semantic.schema,
                policy: self.semantic.policy,
                context: Digest([20; 32]),
            };
            Ok(NativeSemanticResult {
                view: self.view.identity,
                instance: self.semantic.instance,
                coverage: CoverageBinding {
                    instance: self.semantic.instance,
                    desired,
                    covered: Some(desired),
                    searchable: desired.root.0,
                    status: CoverageStatus::Complete,
                    owners: desired.root,
                },
                traversal: NativeSemanticTraversal::ExactCapped,
                exhaustive: false,
                scanned_owners: 1,
                skipped_stale: 0,
                hits: vec![NativeSemanticHit {
                    owner: self.hit.owner,
                    vector_id: b"gold-vector".to_vec(),
                    score: FiniteScore::new(0.75)?,
                    chunk: None,
                }],
            })
        }
    }

    fn text_query() -> TextQueryRequest {
        TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "gold".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        }
    }
    fn semantic_query() -> SemanticQueryRequest {
        SemanticQueryRequest {
            query_text: "gold".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            lexical_scope: None,
            top_k: 1,
        }
    }

    fn peer(
        listener: UnixListener,
        steps: Vec<(NativeIndexRequest, NativeIndexResponse)>,
    ) -> std::thread::JoinHandle<Result<(), String>> {
        std::thread::spawn(move || {
            listener.set_nonblocking(true).map_err(|e| e.to_string())?;
            let end = Instant::now() + Duration::from_secs(8);
            for (index, (expected, response)) in steps.into_iter().enumerate() {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < end =>
                        {
                            std::thread::sleep(Duration::from_millis(2))
                        }
                        Err(e) => return Err(format!("step {index} accept: {e}")),
                    }
                };
                stream.set_nonblocking(false).map_err(|e| e.to_string())?;
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .map_err(|e| e.to_string())?;
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .map_err(|e| e.to_string())?;
                let request: NativeIndexRequestEnvelope =
                    quanta_index_ipc::decode_request(&mut stream).map_err(|e| e.to_string())?;
                if request.payload != expected {
                    return Err(format!(
                        "step {index}: wrong actual wire {:?}, expected {:?}",
                        request.payload, expected
                    ));
                }
                let reply = NativeIndexResponseEnvelope {
                    request_id: request.request_id,
                    payload: response,
                };
                let bytes = quanta_index_ipc::encode_response(&reply).map_err(|e| e.to_string())?;
                stream.write_all(&bytes).map_err(|e| e.to_string())?;
            }
            Ok(())
        })
    }

    #[test]
    fn ordinary_sdk_native_reader_preserves_file_page_hybrid_and_vector_dtos() -> TestResult {
        let f = Fixture::new()?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("native.sock");
        let listener = UnixListener::bind(&path)?;
        let text = f.text_result();
        let page = NativeTextPage {
            session: f.status.session,
            view: f.view.identity,
            instance: f.text.instance,
            artifact: text.artifact,
            matched: 1,
            remaining: 1,
            hits: vec![f.hit.clone()],
            after: None,
        };
        let dense = f.semantic_result()?;
        let hybrid = NativeHybridFileResult {
            session: f.status.session,
            view: f.view.identity,
            text_instance: f.text.instance,
            semantic_instance: f.semantic.instance,
            lexical_artifact: text.artifact,
            lexical_matched: 1,
            semantic_coverage: dense.coverage,
            semantic_traversal: NativeSemanticTraversal::ExactCapped,
            semantic_exhaustive: false,
            scanned_owners: 1,
            skipped_stale: 0,
            file_top_k: 1,
            lexical_pool: vec![f.hit.clone()],
            dense_pool: vec![],
            hits: vec![NativeHybridFileHit {
                owner: f.hit.owner,
                lexical_rank: Some(1),
                dense_rank: None,
                score: FiniteScore::new(1.0 / 61.0)?,
            }],
        };
        let script = vec![
            (
                f.open(),
                NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(f.status)),
            ),
            (f.read_profile(), f.profile()),
            (
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText {
                    session: f.status.session,
                    instance: f.text.instance,
                    query: text_query(),
                }),
                NativeIndexResponse::TextRanked(text.clone()),
            ),
            (
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
                    session: f.status.session,
                    instance: f.text.instance,
                    query: text_query(),
                    after: None,
                }),
                NativeIndexResponse::TextRankedPage(page.clone()),
            ),
            (
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchHybridFile {
                    session: f.status.session,
                    text_instance: f.text.instance,
                    semantic_instance: f.semantic.instance,
                    text_query: text_query(),
                    semantic_query: semantic_query(),
                    file_top_k: 1,
                }),
                NativeIndexResponse::HybridFileRanked(hybrid.clone()),
            ),
            (
                NativeIndexRequest::SearchSemantic {
                    selection: f.selection(),
                    instance: f.semantic.instance,
                    query: semantic_query(),
                },
                NativeIndexResponse::SemanticRanked(dense.clone()),
            ),
            f.release(),
        ];
        let handle = peer(listener, script);
        let client = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &path,
            f.selection(),
        ))?;
        let reader = client.reader().native()?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let mut session = reader.open(OPEN_KEY, &budget)?;
        let _captured_profile = session.load_profile(&budget)?;
        assert_eq!(session.text(&f.text, text_query(), &budget)?, text);
        assert_eq!(
            session.text_page(&f.text, text_query(), None, &budget)?,
            page
        );
        assert_eq!(
            session.hybrid_file(
                &f.text,
                &f.semantic,
                text_query(),
                semantic_query(),
                1,
                &budget
            )?,
            hybrid
        );
        assert_eq!(
            reader.semantic(f.semantic.instance, semantic_query(), &budget)?,
            dense
        );
        assert!(
            session.release(&budget)?,
            "remaining target-pin witness was discarded"
        );
        assert!(matches!(
            session.text(&f.text, text_query(), &budget),
            Err(NativeQueryError::InactiveSession)
        ));
        handle
            .join()
            .map_err(|_| "scripted Native peer panicked")??;
        Ok(())
    }

    #[test]
    fn ordinary_sdk_query_only_has_no_native_write_capability() -> TestResult {
        let f = Fixture::new()?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("native.sock");
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let query = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &path,
            f.selection(),
        ))?;
        assert!(matches!(
            query.producer().native(),
            Err(SdkError::PlaneUnavailable {
                plane: "native-write"
            })
        ));
        assert!(query.reader().native().is_ok());
        let full = QuantaIndex::connect(ConnectOptions::from_native_socket(&path, f.selection()))?;
        assert!(full.producer().native().is_ok());
        assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
        Ok(())
    }

    #[test]
    fn ordinary_sdk_native_cancel_deadline_and_zero_key_do_not_send() -> TestResult {
        let f = Fixture::new()?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("native.sock");
        let listener = UnixListener::bind(&path)?;
        listener.set_nonblocking(true)?;
        let client = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &path,
            f.selection(),
        ))?;
        let reader = client.reader().native()?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(3));
        assert!(matches!(
            reader.open([0; 32], &budget),
            Err(NativeQueryError::MissingOpenKey)
        ));
        budget.cancel_handle().cancel();
        assert!(reader.open(OPEN_KEY, &budget).is_err());
        let expired = RequestBudgetV1::until(Instant::now() - Duration::from_millis(1));
        assert!(reader.open(OPEN_KEY, &expired).is_err());
        assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
        Ok(())
    }

    #[test]
    fn ordinary_sdk_native_rejects_wrong_full_instance_spec_before_query() -> TestResult {
        let f = Fixture::new()?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("native.sock");
        let handle = peer(
            UnixListener::bind(&path)?,
            vec![
                (
                    f.open(),
                    NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(f.status)),
                ),
                (f.read_profile(), f.profile()),
                f.release(),
            ],
        );
        let client = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &path,
            f.selection(),
        ))?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(3));
        let mut session = client.reader().native()?.open(OPEN_KEY, &budget)?;
        assert!(matches!(
            session.text(&f.text, text_query(), &budget),
            Err(NativeQueryError::ProfileRequired)
        ));
        let _captured_profile = session.load_profile(&budget)?;
        let mut wrong = f.text.clone();
        wrong.policy = Digest([99; 32]);
        assert!(matches!(
            session.text(&wrong, text_query(), &budget),
            Err(NativeQueryError::InstanceMismatch)
        ));
        wrong = f.text.clone();
        wrong.input_domain = InputDomain::Projection {
            projection: quanta_index_contract::index::ProjectionId([88; 16]),
            producer: ProducerId([89; 16]),
        };
        assert!(matches!(
            session.text(&wrong, text_query(), &budget),
            Err(NativeQueryError::InstanceMismatch)
        ));
        assert!(
            session.release(&budget)?,
            "scripted peer retained target pins"
        );
        handle
            .join()
            .map_err(|_| "scripted Native peer panicked")??;
        Ok(())
    }

    #[test]
    fn ordinary_sdk_profile_failure_keeps_acknowledged_session_for_release() -> TestResult {
        let f = Fixture::new()?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("native.sock");
        let mut bad = match f.profile() {
            NativeIndexResponse::ReadSessionProfile(profile) => profile,
            _ => return Err("fixture profile variant".into()),
        };
        bad.view.identity.workspace.slot = SlotId([199; 16]);
        let handle = peer(
            UnixListener::bind(&path)?,
            vec![
                (
                    f.open(),
                    NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(f.status)),
                ),
                (
                    f.read_profile(),
                    NativeIndexResponse::ReadSessionProfile(bad),
                ),
                f.release(),
            ],
        );
        let client = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &path,
            f.selection(),
        ))?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(3));
        let mut session = client.reader().native()?.open(OPEN_KEY, &budget)?;
        assert!(session.load_profile(&budget).is_err());
        assert_eq!(session.status(), &f.status);
        assert!(session.profile().is_none());
        assert!(session.release(&budget)?);
        handle
            .join()
            .map_err(|_| "scripted Native peer panicked")??;
        Ok(())
    }

    #[test]
    fn ordinary_sdk_native_unknown_session_is_not_reopened_or_queried() -> TestResult {
        let f = Fixture::new()?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("native.sock");
        let mut gone = f.status;
        gone.state = NativeReadSessionState::Unknown;
        let handle = peer(
            UnixListener::bind(&path)?,
            vec![
                (
                    f.open(),
                    NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(f.status)),
                ),
                (f.read_profile(), f.profile()),
                (
                    NativeIndexRequest::ReadSession(NativeReadSessionRequest::Status(
                        f.status.session,
                    )),
                    NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(gone)),
                ),
            ],
        );
        let client = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &path,
            f.selection(),
        ))?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(3));
        let mut session = client.reader().native()?.open(OPEN_KEY, &budget)?;
        let _captured_profile = session.load_profile(&budget)?;
        session.refresh_status(&budget)?;
        assert_eq!(session.status(), &gone);
        assert!(matches!(
            session.text(&f.text, text_query(), &budget),
            Err(NativeQueryError::InactiveSession)
        ));
        handle
            .join()
            .map_err(|_| "scripted Native peer panicked")??;
        Ok(())
    }

    #[derive(Debug, Default)]
    struct Attempts {
        next: std::sync::atomic::AtomicU64,
        outcomes: std::sync::Arc<
            std::sync::Mutex<Vec<quanta_index_contract::DiagnosticAttemptOutcomeV1>>,
        >,
    }

    #[derive(Debug)]
    struct Attempt {
        token: quanta_index_contract::DiagnosticAttemptTokenV1,
        outcomes: std::sync::Arc<
            std::sync::Mutex<Vec<quanta_index_contract::DiagnosticAttemptOutcomeV1>>,
        >,
    }

    impl quanta_index_sdk::DiagnosticAttemptGuardV1 for Attempt {
        fn token(&self) -> quanta_index_contract::DiagnosticAttemptTokenV1 {
            self.token
        }
        fn finish(self: Box<Self>, outcome: quanta_index_contract::DiagnosticAttemptOutcomeV1) {
            match self.outcomes.lock() {
                Ok(mut outcomes) => outcomes.push(outcome),
                Err(_) => panic!("attempt outcome lock poisoned"),
            }
        }
    }

    impl quanta_index_sdk::DiagnosticAttemptSourceV1 for Attempts {
        fn begin_attempt(&self) -> Option<Box<dyn quanta_index_sdk::DiagnosticAttemptGuardV1>> {
            let number = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            let token = quanta_index_contract::DiagnosticAttemptTokenV1::new(
                [42; 16],
                std::num::NonZeroU64::new(number)?,
            );
            Some(Box::new(Attempt {
                token,
                outcomes: self.outcomes.clone(),
            }))
        }
    }

    #[test]
    fn ordinary_sdk_native_diagnostics_follow_actual_native_send_outcomes() -> TestResult {
        use quanta_index_contract::{
            DiagnosticAttemptOutcomeV1 as Outcome,
            native_protocol::{NativeIndexErrorCode, NativeIndexWireError},
        };
        let f = Fixture::new()?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("native.sock");
        let handle = peer(
            UnixListener::bind(&path)?,
            vec![
                (
                    f.open(),
                    NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(f.status)),
                ),
                (f.read_profile(), f.profile()),
                (
                    NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText {
                        session: f.status.session,
                        instance: f.text.instance,
                        query: text_query(),
                    }),
                    NativeIndexResponse::Error(NativeIndexWireError {
                        code: NativeIndexErrorCode::Cancelled,
                        message: "original query cancelled".into(),
                    }),
                ),
                f.release(),
            ],
        );
        let source = std::sync::Arc::new(Attempts::default());
        let client = QuantaIndex::connect_query_only_with_diagnostic_attempts(
            ConnectOptions::from_native_socket(&path, f.selection()),
            source.clone(),
        )?;
        let reader = client.reader().native()?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(3));
        assert!(matches!(
            reader.open([0; 32], &budget),
            Err(NativeQueryError::MissingOpenKey)
        ));
        let mut session = reader.open(OPEN_KEY, &budget)?;
        let _captured_profile = session.load_profile(&budget)?;
        assert!(session.text(&f.text, text_query(), &budget).is_err());
        assert!(
            session.release(&budget)?,
            "scripted peer retained target pins"
        );
        handle
            .join()
            .map_err(|_| "scripted Native peer panicked")??;
        assert_eq!(source.next.load(std::sync::atomic::Ordering::Relaxed), 4);
        let outcomes = source
            .outcomes
            .lock()
            .map_err(|_| "attempt lock poisoned")?;
        assert_eq!(
            *outcomes,
            vec![Outcome::Ok, Outcome::Ok, Outcome::Cancelled, Outcome::Ok]
        );
        Ok(())
    }

    // This observer measures the real SDK's MemoryScope leases. It is not a
    // Semantica Original issuer or qualification of that owner's wrapper costs.
    struct ReplyMemory {
        live: std::sync::Arc<std::sync::atomic::AtomicU64>,
    }
    struct ReplyLease {
        live: std::sync::Arc<std::sync::atomic::AtomicU64>,
    }
    impl quanta_index_ipc::MemoryLeaseOwner for ReplyLease {
        fn release(&self, bytes: u64) {
            let before = self
                .live
                .fetch_sub(bytes, std::sync::atomic::Ordering::AcqRel);
            assert!(before >= bytes, "SDK reply lease underflow");
        }
    }
    impl quanta_index_ipc::MemoryScopePort for ReplyMemory {
        fn reserve(
            &self,
            bytes: u64,
            _lifetime: quanta_index_ipc::MemoryLifetime,
        ) -> Result<quanta_index_ipc::MemoryLease, quanta_index_ipc::IndexError> {
            let _previous = self
                .live
                .fetch_add(bytes, std::sync::atomic::Ordering::AcqRel);
            Ok(quanta_index_ipc::MemoryLease::new(
                bytes,
                std::sync::Arc::new(ReplyLease {
                    live: self.live.clone(),
                }),
            ))
        }
    }
    fn reply_memory() -> (
        quanta_index_ipc::MemoryScope,
        std::sync::Arc<std::sync::atomic::AtomicU64>,
    ) {
        let live = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        (
            quanta_index_ipc::MemoryScope::new(std::sync::Arc::new(ReplyMemory {
                live: live.clone(),
            })),
            live,
        )
    }

    #[test]
    fn ordinary_sdk_admitted_reader_keeps_owner_and_checks_captured_role_before_send() -> TestResult
    {
        let f = Fixture::new()?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("native.sock");
        let gold = f.text_result();
        let handle = peer(
            UnixListener::bind(&path)?,
            vec![
                (
                    f.open(),
                    NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(f.status)),
                ),
                (f.read_profile(), f.profile()),
                (
                    NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchText {
                        session: f.status.session,
                        instance: f.text.instance,
                        query: text_query(),
                    }),
                    NativeIndexResponse::TextRanked(gold.clone()),
                ),
                f.release(),
            ],
        );
        let client = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &path,
            f.selection(),
        ))?;
        let reader = client.reader().native()?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(3));
        let (memory, live) = reply_memory();
        let opened = reader.open_admitted(OPEN_KEY, &budget, &memory)?;
        let ack = match opened.response() {
            NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(status)) => *status,
            _ => return Err("missing admitted Open ACK".into()),
        };
        let profile = reader.profile_admitted(&ack, &budget, &memory)?;
        assert_eq!(profile.response(), &f.profile());
        let mut foreign = ack;
        foreign.session = NativeReadSessionId([199; 32]);
        assert!(matches!(
            reader.text_admitted(&foreign, &profile, &f.text, text_query(), &budget, &memory),
            Err(NativeQueryError::ProfileBindingMismatch)
        ));
        let mut wrong = f.text.clone();
        wrong.kind = IndexKind([198; 16]);
        assert!(matches!(
            reader.text_admitted(&ack, &profile, &wrong, text_query(), &budget, &memory),
            Err(NativeQueryError::InstanceMismatch)
        ));
        let text = reader.text_admitted(&ack, &profile, &f.text, text_query(), &budget, &memory)?;
        assert!(
            live.load(std::sync::atomic::Ordering::Acquire) > 0,
            "full DATA escaped without raw/decoded grants"
        );
        assert!(matches!(text.response(),NativeIndexResponse::TextRanked(answer) if answer==&gold));
        let released = reader.release_admitted(&ack, &budget, &memory)?;
        assert!(matches!(
            released.response(),
            NativeIndexResponse::ReadSession(NativeReadSessionReply::Released {
                has_remaining_target_pins: true,
                ..
            })
        ));
        handle
            .join()
            .map_err(|_| "scripted Native peer panicked")??;
        drop(text);
        drop(profile);
        drop(opened);
        drop(released);
        assert_eq!(
            live.load(std::sync::atomic::Ordering::Acquire),
            0,
            "last DATA owner did not refund reply leases"
        );
        Ok(())
    }

    #[test]
    fn ordinary_sdk_admitted_profile_denial_preserves_ack_and_charged_error_until_release()
    -> TestResult {
        use quanta_index_contract::native_protocol::{NativeIndexErrorCode, NativeIndexWireError};
        let f = Fixture::new()?;
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("native.sock");
        let handle = peer(
            UnixListener::bind(&path)?,
            vec![
                (
                    f.open(),
                    NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(f.status)),
                ),
                (
                    f.read_profile(),
                    NativeIndexResponse::Error(NativeIndexWireError {
                        code: NativeIndexErrorCode::ResourceLimited,
                        message: "profile original memory denied".into(),
                    }),
                ),
                f.release(),
            ],
        );
        let client = QuantaIndex::connect_query_only(ConnectOptions::from_native_socket(
            &path,
            f.selection(),
        ))?;
        let reader = client.reader().native()?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(3));
        let (memory, live) = reply_memory();
        let opened = reader.open_admitted(OPEN_KEY, &budget, &memory)?;
        let ack = match opened.response() {
            NativeIndexResponse::ReadSession(NativeReadSessionReply::Status(status)) => *status,
            _ => return Err("missing admitted Open ACK".into()),
        };
        let refusal = reader.profile_admitted(&ack, &budget, &memory)?;
        assert!(
            matches!(refusal.response(),NativeIndexResponse::Error(error) if error.code==NativeIndexErrorCode::ResourceLimited && error.message=="profile original memory denied")
        );
        assert_eq!(ack, f.status);
        assert!(matches!(
            reader.text_admitted(&ack, &refusal, &f.text, text_query(), &budget, &memory),
            Err(NativeQueryError::ProfileRequired)
        ));
        assert!(live.load(std::sync::atomic::Ordering::Acquire) > 0);
        let released = reader.release_admitted(&ack, &budget, &memory)?;
        assert!(
            matches!(released.response(),NativeIndexResponse::ReadSession(NativeReadSessionReply::Released {status,..}) if status.session==ack.session && status.state==NativeReadSessionState::Released)
        );
        handle
            .join()
            .map_err(|_| "scripted Native peer panicked")??;
        drop(refusal);
        drop(opened);
        drop(released);
        assert_eq!(live.load(std::sync::atomic::Ordering::Acquire), 0);
        Ok(())
    }
}
