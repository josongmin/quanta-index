use super::*;
use quanta_index_contract::{
    ChunkId, ChunkRecord, ExactRepoRelativePathV1, GenerationPin, ManifestGeneration, RepoId,
    RepoRelativePath, RevisionId, StructuralBinding, StructuralTreeRecord,
    index::{ProjectionReplayStamp, Refusal, ViewRef},
    lex::{DirtyRecord, LanguageCode},
    native_protocol::{
        NATIVE_DSL_PRODUCER_V1, NativeDslCorpusV1, NativeDslFileV1, NativeDslKindV1,
        NativeDslPageV1, NativeDslQueryV1, NativeDslReplayBasisV1, NativeDslReplayV1,
        NativeDslRowsV1, NativeDslRuntimeCatalogV1, NativeDslScopeV1, NativeReadSessionRequest,
        NativeReadSessionState, NativeReadSessionStatus,
    },
};
use quanta_index_sdk::{
    ConnectOptions, QuantaIndex,
    native_uds::{NativeDslChangesV1, NativeDslFileChangeV1, NativeDslPublicationV1},
};
use quanta_index_searchd_harness::{
    RUNTIME_CATALOG_FILES, RUNTIME_DIRTY_EPOCH_MS, STRUCTURAL_TREE_CONTENT,
    STRUCTURAL_TREE_IDENTIFIER, STRUCTURAL_TREE_PATH, runtime_catalog_batch,
    runtime_catalog_fixture_spec,
    scenarios::{FixtureKind, SCENARIOS},
    structural_function_tree_record,
};
use std::path::{Path, PathBuf};

const TREE_FILE_PREFIX: &str = "// fixture\n";
const RUNTIME_CATALOG_INPUT: &str = "runtime-catalog.json";

enum DslCli<'a> {
    Library,
    Executable(&'a Path),
}

impl DslCli<'_> {
    fn run(&self, args: &[&str]) -> TestResult<Vec<u8>> {
        match self {
            Self::Library => {
                let mut out = Vec::new();
                let mut err = Vec::new();
                let status = quanta_index_searchctl::run(args.iter().copied(), &mut out, &mut err);
                assert_eq!(
                    status,
                    std::process::ExitCode::SUCCESS,
                    "{}",
                    String::from_utf8_lossy(&err)
                );
                Ok(out)
            }
            Self::Executable(path) => {
                let output = std::process::Command::new(path).args(args).output()?;
                assert!(
                    output.status.success(),
                    "{path:?} {args:?}: {}: {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                );
                Ok(output.stdout)
            }
        }
    }
}

fn verify_cli_digest(path: &Path, expected: &str) -> TestResult {
    let mut binary = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let _hashed_bytes = std::io::copy(&mut binary, &mut hasher)?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(format!(
            "searchctl SHA-256 mismatch for {path:?}: expected {expected}, got {actual}"
        )
        .into());
    }
    Ok(())
}

fn pin() -> TestResult<GenerationPin> {
    Ok(GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    ))
}

fn change(id: u8, path: &str, text: &str) -> TestResult<NativeDslFileChangeV1> {
    let chunk_id = ChunkId::new(format!("e2e-{id}-{path}"));
    let (offset, start_line, chunk, trees) = if path == STRUCTURAL_TREE_PATH {
        let chunk = text
            .strip_prefix(TREE_FILE_PREFIX)
            .ok_or("tree fixture is missing its source prefix")?;
        (
            u32::try_from(TREE_FILE_PREFIX.len())?,
            2,
            chunk,
            vec![StructuralTreeRecord {
                chunk_id: chunk_id.clone(),
                record: structural_function_tree_record(path, chunk, STRUCTURAL_TREE_IDENTIFIER)?,
            }],
        )
    } else {
        (0, 1, text, Vec::new())
    };
    Ok(NativeDslFileChangeV1 {
        source_owner: owner(id),
        next: Some(NativeDslFileV1 {
            chunks: vec![ChunkRecord {
                chunk_id,
                repo_relative_path: RepoRelativePath::new(path),
                language: LanguageCode::new(if path.ends_with(".rs") {
                    "rust"
                } else {
                    "markdown"
                })?,
                start_byte: offset,
                end_byte: u32::try_from(text.len())?,
                start_line,
                end_line: start_line
                    + u32::try_from(chunk.bytes().filter(|&byte| byte == b'\n').count())?,
                text: chunk.into(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            }],
            trees,
        }),
    })
}

fn publish(
    client: &NativeUdsClient<&std::path::Path>,
    expected_view: ViewRef,
    scope: &NativeDslScopeV1,
    kind: NativeDslKindV1,
    event_predecessor: u64,
    changes: NativeDslChangesV1,
) -> TestResult<ViewRef> {
    let operation = OperationKey {
        producer: NATIVE_DSL_PRODUCER_V1,
        stream: scope.stream(kind),
        ..operation(event_predecessor + 1)
    };
    let input = NativeDslPublicationV1 {
        operation,
        expected_view,
        event_predecessor,
        authority: Digest([22; 32]),
        changes,
    };
    let memory = MemoryScope::new(Arc::new(CountedNativeTestMemory(Arc::new(AtomicU64::new(0)))));
    let sealed = client.prepare_dsl_publication_sealed_admitted_v1(&input, &budget(), &memory)?;
    let exact = sealed.submission;
    assert_eq!(exact.ingress.operation, operation);
    assert_eq!(
        client.read_sealed_dsl_admitted_v1(exact.replay, &budget(), &memory)?,
        Some(exact)
    );
    assert_eq!(client.begin_input(operation, &budget())?, exact.ingress);
    let mut foreign_replay = exact.replay;
    foreign_replay.authority = Digest([96; 32]);
    assert!(client.read_sealed_dsl_admitted_v1(foreign_replay, &budget(), &memory).is_err());
    assert_eq!(client.status(operation, &budget())?, OperationStatus::Unknown);
    let mut foreign = sealed.request.clone();
    foreign.expected_view.digest = Digest([99; 32]);
    assert!(matches!(
        client.submit_borrowed(&foreign, &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::DigestConflict
    ));
    assert_eq!(client.status(operation, &budget())?, OperationStatus::Unknown);
    let mut foreign = exact;
    foreign.ingress.fence = foreign.ingress.fence.checked_add(1).expect("foreign fence");
    assert!(client.submit_sealed_dsl_admitted_v1(foreign, &budget(), &memory).is_err());
    let mut foreign_request = exact;
    foreign_request.request_ref = ObjectRef(Digest([97; 32]));
    assert!(client.submit_sealed_dsl_admitted_v1(foreign_request, &budget(), &memory).is_err());
    assert_eq!(client.status(operation, &budget())?, OperationStatus::Unknown);
    let first = published(&client.submit_sealed_dsl_admitted_v1(exact, &budget(), &memory)?)?;
    assert_eq!(client.read_sealed_dsl_admitted_v1(exact.replay, &budget(), &memory)?, None);
    let identity = client.operation_request_identity_admitted_v1(operation, &budget(), &memory)?;
    assert!(matches!(
        identity.response(),
        NativeIndexResponse::OperationRequestIdentityV1(reply)
            if reply.payload.request == exact.request_ref
    ));
    assert_eq!(
        published(&client.submit_sealed_dsl_admitted_v1(exact, &budget(), &memory)?)?,
        first
    );
    let mut foreign_stamp = exact;
    foreign_stamp.replay.authority = Digest([98; 32]);
    assert!(client.submit_sealed_dsl_admitted_v1(foreign_stamp, &budget(), &memory).is_err());
    Ok(first)
}

type CatalogDocSlot = fn(&mut NativeDslRuntimeCatalogV1) -> Option<&mut ChunkId>;

#[derive(Clone, Copy)]
enum InvalidCatalog {
    ForeignGeneration,
    ForeignChunk {
        sequence: u64,
        doc_id: CatalogDocSlot,
    },
    StaleEpoch,
    ConflictingEpoch,
    DuplicateKey,
}

fn setup(
    client: &NativeUdsClient<&std::path::Path>,
    socket: &std::path::Path,
    input_dir: &Path,
    fixture: FixtureKind,
    cli: &DslCli<'_>,
    rejects: &[InvalidCatalog],
) -> TestResult<(NativeDslScopeV1, ViewRef, ViewRef)> {
    let pin = pin()?;
    let scope = NativeDslScopeV1::for_pin(&pin);
    let configured = create_text_workspace(client)
        .map_err(|error| format!("configure DSL fixture workspace: {error}"))?;
    let mut source = configured;
    let mut by_path = std::collections::BTreeMap::new();
    let mut changes = Vec::new();
    let tree_file = format!("{TREE_FILE_PREFIX}{STRUCTURAL_TREE_CONTENT}");
    let files: Vec<(&str, &str)> = match fixture {
        FixtureKind::RuntimeCatalog => RUNTIME_CATALOG_FILES.to_vec(),
        FixtureKind::StructuralTree => quanta_index_searchd_harness::scenarios::LEXICAL_CORPUS
            .iter()
            .copied()
            .chain([(STRUCTURAL_TREE_PATH, tree_file.as_str())])
            .collect(),
        _ => return Err("unexpected retained DSL fixture".into()),
    };
    for (index, (path, text)) in files.iter().copied().enumerate() {
        let id = u8::try_from(index + 1)?;
        (source, _) = publish_text_owner(client, source, u64::from(id) + 1, id, path, text)
            .map_err(|error| format!("publish DSL fixture Source owner {path}: {error}"))?;
        let change = change(id, path, text)?;
        let id = change
            .next
            .as_ref()
            .and_then(|file| file.chunks.first())
            .ok_or("fixture chunk missing")?
            .chunk_id
            .clone();
        assert!(by_path.insert(path, id).is_none());
        changes.push(change);
    }
    let corpus_input = NativeDslPublicationV1 {
        operation: OperationKey {
            producer: NATIVE_DSL_PRODUCER_V1,
            stream: scope.stream(NativeDslKindV1::Structural),
            ..operation(1)
        },
        expected_view: source,
        event_predecessor: 0,
        authority: Digest([22; 32]),
        changes: NativeDslChangesV1::Corpus {
            header: NativeDslCorpusV1 {
                pin: pin.clone(),
                source_producer: operation(1).producer,
                structural_ready: fixture == FixtureKind::StructuralTree,
            },
            files: changes,
        },
    };
    if matches!(cli, DslCli::Library) {
        assert_invalid_structural_keeps_current(client, &corpus_input)
            .map_err(|error| format!("refuse invalid Structural fixture: {error}"))?;
    }
    let corpus = cli_publish(
        cli,
        socket,
        &input_dir.join("native-corpus.json"),
        &corpus_input,
    )?;
    if matches!(cli, DslCli::Library) {
        let refused = OperationKey {
            sequence: 101,
            ..corpus_input.operation
        };
        assert_eq!(
            client.resume(refused, &budget())?,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::InvalidInput)),
        );
    }
    let ready = if fixture == FixtureKind::RuntimeCatalog {
        let mut catalog = runtime_catalog_batch(&runtime_catalog_fixture_spec(), &pin, |path| {
            by_path.get(path).cloned()
        })?;
        quanta_index_ipc::stamp_batch_digest_v1(&mut catalog)?;
        let dirty = vec![DirtyRecord {
            wire_version: 1,
            doc_id: by_path
                .get("src/dirty.rs")
                .ok_or("missing dirty fixture")?
                .clone(),
            applied_at_ms: RUNTIME_DIRTY_EPOCH_MS,
            payload_hash: [0x5a; 32],
        }];
        let corpus_certificate = client
            .read_projection_binding(
                ViewSelection::Exact(corpus),
                scope.projection(NativeDslKindV1::Structural),
                &budget(),
            )?
            .binding
            .ok_or("Native DSL corpus not published")?
            .certificate;
        let publication = NativeDslPublicationV1 {
            operation: OperationKey {
                producer: NATIVE_DSL_PRODUCER_V1,
                stream: scope.stream(NativeDslKindV1::RuntimeMetadata),
                ..operation(1)
            },
            expected_view: corpus,
            event_predecessor: 0,
            authority: Digest([22; 32]),
            changes: NativeDslChangesV1::RuntimeMetadata(NativeDslRuntimeCatalogV1 {
                corpus_certificate,
                dirty,
                catalog,
            }),
        };
        if matches!(cli, DslCli::Library) {
            assert_runtime_metadata_publication_refusals(&publication, client)?;
        }
        let ready = cli_publish(
            cli,
            socket,
            &input_dir.join(RUNTIME_CATALOG_INPUT),
            &publication,
        )?;
        assert_eq!(
            client.status(publication.operation, &budget())?,
            OperationStatus::Published(ready)
        );
        if matches!(cli, DslCli::Library) {
            for defect in [
                InvalidCatalog::StaleEpoch,
                InvalidCatalog::ConflictingEpoch,
                InvalidCatalog::DuplicateKey,
            ] {
                assert_invalid_catalog_does_not_publish(client, &publication, ready, defect)?;
            }
        }
        for &defect in rejects {
            assert_invalid_catalog_does_not_publish(client, &publication, ready, defect)?;
        }
        ready
    } else {
        corpus
    };
    Ok((scope, source, ready))
}

fn cli_publish(
    cli: &DslCli<'_>,
    socket: &Path,
    path: &Path,
    input: &NativeDslPublicationV1,
) -> TestResult<ViewRef> {
    std::fs::write(path, serde_json::to_vec(input)?)?;
    let args = [
        "native",
        "publish-dsl",
        "--socket",
        socket.to_str().ok_or("UTF-8 socket required")?,
        "--input",
        path.to_str().ok_or("UTF-8 fixture required")?,
    ];
    let out = cli.run(&args)?;
    assert_eq!(
        cli.run(&args)?,
        out,
        "public CLI must replay the exact JSON input"
    );
    let view = serde_json::from_slice(&out)?;
    let client = NativeUdsClient::new(socket);
    let replay = client.publish_dsl_v1(input, &budget()).map_err(|error| {
        format!(
            "replay CLI publication {} through SDK: {error}",
            path.display()
        )
    })?;
    assert_eq!(published(&replay)?, view);
    let mut conflicting = input.clone();
    conflicting.authority = Digest([97; 32]);
    assert!(matches!(
        client.publish_dsl_v1(&conflicting, &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::DigestConflict
    ));
    Ok(view)
}

fn assert_current(client: &NativeUdsClient<&Path>, view: ViewRef) -> TestResult {
    assert_eq!(
        client
            .read_view(
                ViewSelection::Latest {
                    workspace: view.workspace,
                    epoch: view.epoch,
                },
                &budget()
            )?
            .identity,
        view
    );
    Ok(())
}

fn prove_pending_catalog_recovery(
    running: NativeServiceGuard,
    root: &Path,
    socket: &Path,
    input_path: &Path,
    ready: ViewRef,
    lease: Duration,
) -> TestResult<NativeServiceGuard> {
    let client = NativeUdsClient::new(socket);
    let mut input: NativeDslPublicationV1 =
        serde_json::from_reader(std::fs::File::open(input_path)?)?;
    input.operation.sequence = 10;
    input.expected_view = ready;
    input.event_predecessor = 1;
    let NativeDslChangesV1::RuntimeMetadata(runtime) = &mut input.changes else {
        return Err("expected original catalog".into());
    };
    runtime.catalog.generation = ManifestGeneration::new(2);
    quanta_index_ipc::stamp_batch_digest_v1(&mut runtime.catalog)?;
    let request = client.prepare_dsl_publication_v1(&input, &budget())?;
    let ingress = client.begin_input(input.operation, &budget())?;
    let UpdateKind::ProjectionAdvance(transition) = &request.kind else {
        return Err("expected original catalog operation".into());
    };
    let ProducerCompleteness::CompleteAffectedDomain { certificate } = transition.completeness
    else {
        return Err("expected original catalog certificate".into());
    };
    running.stop()?;
    {
        // Freeze a genuinely admitted operation before any worker can drive it.
        let catalog = Arc::new(SqliteCatalog::open(root, Duration::from_secs(2))?);
        let scratch_root = private_root()?;
        let scratch = Arc::new(FileScratchSpace::open_standalone(
            scratch_root.path().join("scratch"),
            32 * 1024 * 1024,
        )?);
        let store = FileObjectStore::open_managed_with_scratch(
            root.join("native-objects"),
            scratch,
            catalog.clone(),
        )?;
        let sink = FixtureIngressSink {
            catalog: catalog.as_ref(),
            ingress,
        };
        let mut work = WorkBudget::with_request(work_limits(), budget());
        let mut children: Vec<_> = [transition.owners.root.0, Some(certificate)]
            .into_iter()
            .flatten()
            .collect();
        children.sort_unstable();
        children.dedup();
        let stored = store.install(&serde_json::to_vec(&request)?, &children, &sink, &mut work)?;
        catalog.seal_inputs(ingress, stored.reference)?;
        let admitted = catalog.admit_projection(
            ImmutableOperation {
                key: request.operation,
                request: stored.reference,
                source_mode: request.kind.source_mode(),
            },
            ingress,
            &transition.events,
            ProjectionReplayStamp {
                expected_view: ready,
                projection: transition.projection,
                source: transition.target_source,
                context: transition.dependency_context,
            },
            &mut work,
        )?;
        assert!(matches!(admitted, AdmissionOutcome::New(_)));
    }
    let replay = NativeDslReplayV1 {
        operation: input.operation,
        projection: transition.projection,
        basis: NativeDslReplayBasisV1::Source(transition.target_source),
        authority: input.authority,
    };
    let limited = NativeServiceGuard::start(read_session_service_with_retention_and_work_limits(
        root,
        socket,
        lease,
        None,
        WorkLimits {
            bytes: 1024,
            ..work_limits()
        },
    )?);
    assert!(matches!(
        client.request(NativeIndexRequest::ReplayDslV1(replay), &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::ResourceLimited
    ));
    limited.stop()?;
    let running = NativeServiceGuard::start(read_session_service_with_retention(
        root, socket, lease, None,
    )?);
    assert!(matches!(
        client.status(input.operation, &budget())?,
        OperationStatus::Pending(_)
    ));
    assert_current(&client, ready)?;
    assert!(matches!(
        client.request(NativeIndexRequest::ReplayDslV1(NativeDslReplayV1 {
            authority: Digest([99; 32]), ..replay
        }), &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::DigestConflict
    ));
    assert!(matches!(
        client.request(NativeIndexRequest::ReplayDslV1(NativeDslReplayV1 {
            basis: NativeDslReplayBasisV1::SourceWithServingSequence {
                source: transition.target_source,
                serving_event_sequence: input.event_predecessor,
            },
            ..replay
        }), &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::DigestConflict
    ));
    assert!(matches!(
        client.status(input.operation, &budget())?,
        OperationStatus::Pending(_)
    ));
    assert_current(&client, ready)?;
    let response = client.request(NativeIndexRequest::ReplayDslV1(replay), &budget())?;
    let NativeIndexResponse::Submitted(reply) = response else {
        return Err("expected recovered catalog outcome".into());
    };
    assert_eq!(reply.operation, input.operation);
    assert_eq!(
        reply.payload,
        SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::InvalidInput))
    );
    assert_eq!(
        client.request(
            NativeIndexRequest::ReplayDslV1(NativeDslReplayV1 {
                basis: NativeDslReplayBasisV1::SourceWithServingSequence {
                    source: transition.target_source,
                    serving_event_sequence: input.event_predecessor + 1,
                },
                ..replay
            }),
            &budget()
        )?,
        NativeIndexResponse::Submitted(reply)
    );
    assert_current(&client, ready)?;
    Ok(running)
}

fn assert_invalid_structural_keeps_current(
    client: &NativeUdsClient<&Path>,
    valid: &NativeDslPublicationV1,
) -> TestResult {
    use quanta_index_core::{BudgetInterruptionV1, RequestExecutionObserverV1};
    use quanta_index_search_plane::{native_dsl, readiness::StructuralAuthorityState};

    #[derive(Debug, Default)]
    struct DeadlineDuringAssembly(AtomicU64);

    impl RequestExecutionObserverV1 for DeadlineDuringAssembly {
        fn interruption(&self) -> Option<BudgetInterruptionV1> {
            // Allow the observer admission, file entry, and first chunk checkpoint.
            (self.0.fetch_add(1, Ordering::Relaxed) >= 3).then_some(
                BudgetInterruptionV1::DeadlineExceeded {
                    over_by: Duration::from_millis(1),
                },
            )
        }
    }

    let NativeDslChangesV1::Corpus { files, .. } = &valid.changes else {
        return Err("structural corpus required".into());
    };
    let file = files
        .iter()
        .find_map(|change| change.next.as_ref())
        .ok_or("fixture file is missing")?;
    let memory = MemoryScope::new(Arc::new(CountedNativeTestMemory(Arc::new(AtomicU64::new(
        0,
    )))));
    let expired = RequestBudgetV1::observing_execution_admitted(
        Arc::new(DeadlineDuringAssembly::default()),
        &memory,
    )?;
    let mut state = StructuralAuthorityState::default();
    assert!(matches!(
        native_dsl::add_native_dsl_file_v1(&mut state, file.clone(), &expired),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::RequestDeadlineExceeded,
            ..
        })
    ));
    assert!(state.chunks().is_empty());
    native_dsl::add_native_dsl_file_v1(&mut state, file.clone(), &budget())?;
    let mut constraints = QueryConstraintSetV1::unconstrained();
    constraints.repo_relative_path_exact =
        Some(ExactRepoRelativePathV1::new("outside-fixture.rs")?);
    let cancelled = budget();
    cancelled.cancel_handle().cancel();
    assert!(matches!(
        native_dsl::restrict_native_structural_v1(&mut state, &constraints, &cancelled),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::RequestCancelled,
            ..
        })
    ));
    assert_eq!(state.chunks().len(), file.chunks.len());

    let expected = SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::InvalidInput));
    for (sequence, omit_file) in [(101, true), (102, false)] {
        let mut invalid = valid.clone();
        invalid.operation.sequence = sequence;
        let NativeDslChangesV1::Corpus { files, .. } = &mut invalid.changes else {
            return Err("structural corpus required".into());
        };
        if omit_file {
            let _omitted = files.pop().ok_or("omitted fixture file")?;
        } else {
            let chunk = files
                .first_mut()
                .and_then(|file| file.next.as_mut())
                .and_then(|file| file.chunks.first_mut())
                .ok_or("missing chunk fixture")?;
            chunk.start_line += 1;
            chunk.end_line += 1;
        }
        let request = client
            .prepare_dsl_publication_v1(&invalid, &budget())
            .map_err(|error| format!("prepare invalid Structural operation {sequence}: {error}"))?;
        let outcome = client
            .submit_borrowed(&request, &budget())
            .map_err(|error| format!("submit invalid Structural operation {sequence}: {error}"))?;
        assert_eq!(outcome, expected);
        assert_current(client, valid.expected_view)?;
    }
    for (sequence, omit_header) in [(103, true), (104, false)] {
        let mut input = valid.clone();
        input.operation.sequence = sequence;
        let mut request = client
            .prepare_dsl_publication_v1(&input, &budget())
            .map_err(|error| {
                format!("prepare malformed Structural operation {sequence}: {error}")
            })?;
        let ingress = client
            .begin_input(input.operation, &budget())
            .map_err(|error| format!("reopen Structural ingress {sequence}: {error}"))?;
        let UpdateKind::ProjectionAdvance(transition) = &mut request.kind else {
            return Err("expected Structural projection".into());
        };
        if omit_header {
            transition.owners = client
                .stage_mutations(ingress, std::iter::empty::<OwnerMutation>(), &budget())
                .map_err(|error| {
                    format!("stage Structural operation {sequence} without header: {error}")
                })?;
        } else {
            transition.dependency_context =
                quanta_index_contract::native_protocol::native_dsl_context_v1(2, input.authority);
        }
        let certificate = client
            .stage_projection_certificate(
                ingress,
                ProjectionCertificate {
                    projection: transition.projection,
                    producer: transition.producer,
                    prior: transition.expected,
                    target_source: transition.target_source,
                    schema: transition.schema,
                    producer_policy: transition.producer_policy,
                    dependency_context: transition.dependency_context,
                    affected: transition.owners,
                },
                &budget(),
            )
            .map_err(|error| {
                format!("stage malformed Structural certificate {sequence}: {error}")
            })?;
        transition.completeness = ProducerCompleteness::CompleteAffectedDomain { certificate };
        let payload = transition.payload_digest()?;
        transition
            .events
            .first_mut()
            .ok_or("expected Structural event")?
            .payload = payload;
        let outcome = client
            .submit_borrowed(&request, &budget())
            .map_err(|error| {
                format!("submit malformed Structural operation {sequence}: {error}")
            })?;
        assert_eq!(outcome, expected);
        assert_current(client, valid.expected_view)?;
    }
    Ok(())
}

fn assert_invalid_catalog_does_not_publish(
    client: &NativeUdsClient<&Path>,
    valid: &NativeDslPublicationV1,
    ready: ViewRef,
    defect: InvalidCatalog,
) -> TestResult {
    let NativeDslChangesV1::RuntimeMetadata(runtime) = &valid.changes else {
        return Err("runtime publication fixture must carry catalog facts".into());
    };
    let mut invalid = runtime.clone();
    let sequence = match defect {
        InvalidCatalog::ForeignGeneration => {
            invalid.catalog.generation = ManifestGeneration::new(2);
            7
        }
        InvalidCatalog::ForeignChunk { sequence, doc_id } => {
            // Advance the epoch so this fixture reaches the chunk membership check.
            invalid.catalog.overlay_epoch_ms += 1;
            *doc_id(&mut invalid).ok_or("missing fixture chunk")? = ChunkId::new("foreign-chunk");
            sequence
        }
        InvalidCatalog::StaleEpoch => {
            invalid.catalog.overlay_epoch_ms = invalid
                .catalog
                .overlay_epoch_ms
                .checked_sub(1)
                .ok_or("nonzero overlay")?;
            4
        }
        InvalidCatalog::ConflictingEpoch => {
            invalid
                .catalog
                .changed_entries
                .first_mut()
                .ok_or("fixture changed record is missing")?
                .payload_hash = [67; 32];
            5
        }
        InvalidCatalog::DuplicateKey => {
            let mut duplicate = invalid
                .catalog
                .snapshot_entries
                .first()
                .ok_or("fixture snapshot is missing")?
                .clone();
            duplicate.doc_ids.clear();
            invalid.catalog.snapshot_entries.push(duplicate);
            invalid.catalog.overlay_epoch_ms += 1;
            6
        }
    };
    quanta_index_ipc::stamp_batch_digest_v1(&mut invalid.catalog)?;
    let operation = OperationKey {
        sequence,
        ..valid.operation
    };
    let update = client.prepare_dsl_publication_v1(
        &NativeDslPublicationV1 {
            operation,
            expected_view: ready,
            event_predecessor: 1,
            authority: Digest([23; 32]),
            changes: NativeDslChangesV1::RuntimeMetadata(invalid),
        },
        &budget(),
    )?;
    let expected = OperationStatus::Refused(Refusal::InvalidInput);
    assert_eq!(
        client.submit_borrowed(&update, &budget())?,
        SubmitOutcome::Terminal(expected.clone()),
        "expected catalog operation {sequence} to be refused"
    );
    assert_eq!(client.status(operation, &budget())?, expected);
    assert_eq!(
        client.resume(operation, &budget())?,
        SubmitOutcome::Terminal(expected)
    );
    assert_eq!(
        client
            .read_view(
                ViewSelection::Latest {
                    workspace: ready.workspace,
                    epoch: ready.epoch,
                },
                &budget()
            )?
            .identity,
        ready,
    );
    Ok(())
}

fn assert_runtime_metadata_publication_refusals(
    publication: &NativeDslPublicationV1,
    client: &NativeUdsClient<&Path>,
) -> TestResult {
    let NativeDslChangesV1::RuntimeMetadata(runtime) = &publication.changes else {
        return Err("runtime publication fixture must carry catalog facts".into());
    };
    let cancelled = budget();
    cancelled.cancel_handle().cancel();
    assert!(matches!(
        quanta_index_search_plane::native_dsl::validate_native_runtime_catalog_keys_v1(
            &runtime.catalog,
            &cancelled,
        ),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::RequestCancelled,
            ..
        })
    ));
    let mut tampered = runtime.clone();
    tampered.catalog.producer_head_applied_at_ms += 1;
    let mut repeated = runtime.clone();
    repeated.dirty.push(
        runtime
            .dirty
            .first()
            .ok_or("missing fixture dirty doc")?
            .clone(),
    );
    for (runtime, expected) in [
        (tampered, "Native RuntimeMetadata catalog digest differs"),
        (repeated, "invalid Native RuntimeMetadata dirty membership"),
    ] {
        let invalid = NativeDslPublicationV1 {
            operation: publication.operation,
            expected_view: publication.expected_view,
            event_predecessor: publication.event_predecessor,
            authority: publication.authority,
            changes: NativeDslChangesV1::RuntimeMetadata(runtime),
        };
        assert!(matches!(
            client.prepare_dsl_publication_v1(&invalid, &budget()),
            Err(NativeUdsError::Protocol(message)) if message == expected
        ));
    }
    assert_eq!(
        client.status(publication.operation, &budget())?,
        OperationStatus::Unknown
    );
    assert_eq!(
        client
            .read_view(
                ViewSelection::Latest {
                    workspace: publication.expected_view.workspace,
                    epoch: publication.expected_view.epoch,
                },
                &budget(),
            )?
            .identity,
        publication.expected_view,
    );
    Ok(())
}

fn query(
    scope: &NativeDslScopeV1,
    kind: NativeDslKindV1,
    syntax: TextQuerySyntax,
    text: &str,
) -> NativeDslQueryV1 {
    NativeDslQueryV1 {
        scope: scope.clone(),
        kind,
        text: TextQueryRequest {
            query_text: text.to_owned(),
            syntax,
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 10,
            cursor: None,
        },
        after: None,
    }
}

fn cli_query(
    cli: &DslCli<'_>,
    socket: &std::path::Path,
    session: &NativeReadSessionStatus,
    query: &NativeDslQueryV1,
) -> TestResult<NativeDslPageV1> {
    let command = match query.kind {
        NativeDslKindV1::Structural => "structural",
        NativeDslKindV1::RuntimeMetadata => "runtime-metadata",
    };
    let syntax = match query.text.syntax {
        TextQuerySyntax::Native => "native",
        TextQuerySyntax::Sourcegraph => "sourcegraph",
        TextQuerySyntax::CodeSearch => return Err("unexpected native DSL syntax".into()),
    };
    let out = cli.run(&[
        "native",
        command,
        "--socket",
        socket.to_str().ok_or("socket is not UTF-8")?,
        "--session-json",
        &serde_json::to_string(session)?,
        "--repo",
        query.scope.repository.as_str(),
        "--revision",
        query.scope.revision.as_str(),
        "--query-text",
        &query.text.query_text,
        "--syntax",
        syntax,
        "--top-k",
        "10",
        "--output",
        "json",
    ])?;
    Ok(serde_json::from_slice(&out)?)
}

#[test]
fn native_dsl_twelve_retained_rows_match_independent_gold_v1() -> TestResult {
    check_retained_rows(&DslCli::Library, Duration::from_secs(30))
}

#[test]
#[ignore = "requires a release searchctl executable and its SHA-256"]
fn native_dsl_twelve_retained_rows_match_pinned_cli_v1() -> TestResult {
    let source = PathBuf::from(
        std::env::var_os("QUANTA_INDEX_NATIVE_DSL_CLI_BIN")
            .ok_or("missing QUANTA_INDEX_NATIVE_DSL_CLI_BIN")?,
    );
    let sha256 = std::env::var("QUANTA_INDEX_NATIVE_DSL_CLI_SHA256")?;
    if !source.is_absolute() || !std::fs::metadata(&source)?.is_file() {
        return Err(format!(
            "expected an absolute path to a regular searchctl executable: {source:?}"
        )
        .into());
    }
    let binary_dir = tempfile::tempdir()?;
    let path = binary_dir.path().join("quanta-index-searchctl");
    let _copied_bytes = std::fs::copy(&source, &path)?;
    verify_cli_digest(&path, &sha256)?;
    let result = check_retained_rows(&DslCli::Executable(&path), Duration::from_secs(120));
    verify_cli_digest(&path, &sha256)?;
    result
}

fn check_retained_rows(cli: &DslCli<'_>, lease: Duration) -> TestResult {
    let metadata_gold = [
        ("runtime.dirty_no.sourcegraph", &["e2e-1-src/clean.rs"][..]),
        (
            "runtime.changed.sourcegraph",
            &["e2e-3-src/changed.rs", "e2e-4-src/stale.rs"],
        ),
        (
            "runtime.stale.sourcegraph",
            &[
                "e2e-1-src/clean.rs",
                "e2e-2-src/dirty.rs",
                "e2e-3-src/changed.rs",
                "e2e-4-src/stale.rs",
                "e2e-5-src/owner.rs",
                "e2e-6-src/service.rs",
                "e2e-7-src/layer.rs",
                "e2e-8-src/surface.rs",
                "e2e-9-src/snap.rs",
            ],
        ),
        (
            "runtime.snapshot.sourcegraph",
            &["e2e-3-src/changed.rs", "e2e-9-src/snap.rs"],
        ),
        (
            "runtime.meta_owner.sourcegraph",
            &[
                "e2e-5-src/owner.rs",
                "e2e-6-src/service.rs",
                "e2e-7-src/layer.rs",
                "e2e-8-src/surface.rs",
            ],
        ),
        (
            "runtime.meta_service.sourcegraph",
            &[
                "e2e-5-src/owner.rs",
                "e2e-6-src/service.rs",
                "e2e-7-src/layer.rs",
                "e2e-8-src/surface.rs",
            ],
        ),
        (
            "runtime.meta_layer.sourcegraph",
            &[
                "e2e-5-src/owner.rs",
                "e2e-6-src/service.rs",
                "e2e-7-src/layer.rs",
                "e2e-8-src/surface.rs",
            ],
        ),
        (
            "runtime.meta_surface.sourcegraph",
            &[
                "e2e-5-src/owner.rs",
                "e2e-6-src/service.rs",
                "e2e-7-src/layer.rs",
                "e2e-8-src/surface.rs",
            ],
        ),
        ("runtime.affected.sourcegraph", &["e2e-3-src/changed.rs"]),
        (
            "runtime.invalidated_by.sourcegraph",
            &["e2e-3-src/changed.rs"],
        ),
    ];
    for fixture in [FixtureKind::RuntimeCatalog, FixtureKind::StructuralTree] {
        let root = private_root()?;
        let inputs = private_root()?;
        let socket = root.path().join("native.sock");
        let running = NativeServiceGuard::start(read_session_service_with_retention(
            root.path(),
            &socket,
            lease,
            None,
        )?);
        let client = NativeUdsClient::new(socket.as_path());
        let rejects: &[InvalidCatalog] = if matches!(cli, DslCli::Library) {
            &[InvalidCatalog::ForeignGeneration]
        } else {
            &[]
        };
        let (scope, _, ready) = setup(&client, &socket, inputs.path(), fixture, cli, rejects)?;
        let running = if fixture == FixtureKind::RuntimeCatalog && matches!(cli, DslCli::Library) {
            prove_pending_catalog_recovery(
                running,
                root.path(),
                &socket,
                &inputs.path().join(RUNTIME_CATALOG_INPUT),
                ready,
                lease,
            )?
        } else {
            running
        };
        let sdk = QuantaIndex::connect(ConnectOptions::from_native_socket(
            socket.clone(),
            ViewSelection::Exact(ready),
        ))?;
        let reader = sdk.reader().native()?;
        let mut session = reader.open([11; 32], &budget())?;
        let active = Arc::new(AtomicU64::new(0));
        let memory = MemoryScope::new(Arc::new(CountedNativeTestMemory(Arc::clone(&active))));
        let ids: Vec<&str> = if fixture == FixtureKind::RuntimeCatalog {
            metadata_gold.iter().map(|(id, _)| *id).collect()
        } else {
            vec![
                "structural.tree_match.native",
                "structural.tree_match.sourcegraph",
            ]
        };
        for id in ids {
            let scenario = SCENARIOS
                .iter()
                .find(|row| row.id == id)
                .ok_or("missing official DSL scenario")?;
            let (kind, syntax) = if fixture == FixtureKind::RuntimeCatalog {
                (
                    NativeDslKindV1::RuntimeMetadata,
                    TextQuerySyntax::Sourcegraph,
                )
            } else {
                (
                    NativeDslKindV1::Structural,
                    if id.ends_with(".native") {
                        TextQuerySyntax::Native
                    } else {
                        TextQuerySyntax::Sourcegraph
                    },
                )
            };
            let query = query(&scope, kind, syntax, &scenario.query.resolve());
            let page = session.dsl_v1(query.clone(), &budget())?;
            if matches!(cli, DslCli::Library) {
                let admitted =
                    reader.dsl_admitted_v1(session.status(), query.clone(), &budget(), &memory)?;
                let NativeIndexResponse::DslPageV1(admitted_page) = admitted.response() else {
                    return Err("expected admitted Native DSL page".into());
                };
                assert_eq!(&page, admitted_page, "{id}");
                assert!(active.load(Ordering::SeqCst) > 0, "{id}");
                drop(admitted);
                assert_eq!(active.load(Ordering::SeqCst), 0, "{id}");
            }
            assert_eq!(page.view, ready, "{id}");
            assert!(!page.window.has_more(), "{id}");
            match &page.rows {
                NativeDslRowsV1::RuntimeMetadata(rows) => {
                    assert!(
                        rows.iter()
                            .all(|row| (row.start_line, row.end_line) == (1, 1)),
                        "{id}"
                    );
                    let (_, expected) = metadata_gold
                        .iter()
                        .find(|(row, _)| *row == id)
                        .ok_or("missing gold")?;
                    assert_eq!(
                        page.rows.candidate_ids().collect::<Vec<_>>().as_slice(),
                        *expected,
                        "{id}"
                    );
                }
                NativeDslRowsV1::Structural(rows) => {
                    assert_eq!(rows.len(), 1, "{id}");
                    assert_eq!(rows[0].candidate.candidate_id, "e2e-5-src/tree.rs", "{id}");
                    let binding = rows[0]
                        .candidate
                        .bindings
                        .first()
                        .ok_or("missing capture")?;
                    assert_eq!(
                        (
                            binding.start_byte,
                            binding.end_byte,
                            binding.start_line,
                            binding.end_line
                        ),
                        (14, 33, 2, 2),
                        "{id}"
                    );
                    assert_eq!(binding.metavariable.trim_start_matches(':'), "name", "{id}");
                    assert_eq!(rows[0].path.as_str(), "src/tree.rs", "{id}");
                    let _: &StructuralBinding = binding;
                }
            }
            let cli_page = cli_query(cli, &socket, session.status(), &query)?;
            assert_eq!(cli_page, page, "{id}");
        }
        let _remaining_pins = session.release(&budget())?;
        assert_eq!(session.status().state, NativeReadSessionState::Released);
        running.stop()?;
    }
    Ok(())
}

fn retained_bytes_after_response_drain(
    running: &NativeServiceGuard,
    transport: &quanta_index_ipc::IpcServerCounters,
) -> TestResult<u64> {
    // The SDK can receive the frame while the daemon still holds its detached response memory.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if transport.snapshot().connections_live == 0 {
            let state = running.resources.observe()?;
            if state.reserved == ResourceNeed::default() && state.pending_escape_bytes == 0 {
                return Ok(state.retained_memory_bytes);
            }
        }
        if Instant::now() >= deadline {
            return Err("Native DSL fixture did not drain response and execution memory".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn native_dsl_retained_cursor_refuses_another_view_and_keeps_original_rows_v1() -> TestResult {
    let root = private_root()?;
    let inputs = private_root()?;
    let socket = root.path().join("native.sock");
    let service =
        read_session_service_with_retention(root.path(), &socket, Duration::from_secs(30), None)?;
    let transport = service.transport_observation();
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket.as_path());
    let invalid_chunks: [(u64, CatalogDocSlot); 6] = [
        (7, |runtime| {
            Some(&mut runtime.catalog.changed_entries.first_mut()?.doc_id)
        }),
        (8, |runtime| Some(&mut runtime.dirty.first_mut()?.doc_id)),
        (9, |runtime| {
            Some(&mut runtime.catalog.facet_entries.first_mut()?.doc_id)
        }),
        (10, |runtime| {
            runtime
                .catalog
                .snapshot_entries
                .first_mut()?
                .doc_ids
                .first_mut()
        }),
        (11, |runtime| {
            runtime
                .catalog
                .affected_entries
                .first_mut()?
                .doc_ids
                .first_mut()
        }),
        (12, |runtime| {
            runtime
                .catalog
                .invalidated_by_entries
                .first_mut()?
                .doc_ids
                .first_mut()
        }),
    ];
    let rejects =
        invalid_chunks.map(|(sequence, doc_id)| InvalidCatalog::ForeignChunk { sequence, doc_id });
    let (scope, source, ready) = setup(
        &client,
        &socket,
        inputs.path(),
        FixtureKind::RuntimeCatalog,
        &DslCli::Library,
        &rejects,
    )?;
    let missing = client.open_read_session(ViewSelection::Exact(source), [1; 32], &budget())?;
    let retained = client.open_read_session(ViewSelection::Exact(ready), [2; 32], &budget())?;
    let mut request = query(
        &scope,
        NativeDslKindV1::RuntimeMetadata,
        TextQuerySyntax::Sourcegraph,
        "changed:since=1970-01-01T00:00:00.010Z",
    );
    request.text.top_k = 1;
    assert!(
        matches!(client.search_dsl_in_read_session_v1(&missing, request.clone(), &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::SnapshotUnavailable)
    );
    let first = client.search_dsl_in_read_session_v1(&retained, request.clone(), &budget())?;
    let active = Arc::new(AtomicU64::new(0));
    let memory = MemoryScope::new(Arc::new(CountedNativeTestMemory(Arc::clone(&active))));
    let admitted = client.search_dsl_in_read_session_admitted_v1(
        &retained,
        request.clone(),
        &budget(),
        &memory,
    )?;
    let NativeIndexResponse::DslPageV1(admitted_page) = admitted.response() else {
        return Err("expected admitted Native DSL page with continuation".into());
    };
    assert_eq!(admitted_page, &first);
    drop(admitted);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    let publication: NativeDslPublicationV1 =
        serde_json::from_slice(&std::fs::read(inputs.path().join(RUNTIME_CATALOG_INPUT))?)?;
    let NativeDslChangesV1::RuntimeMetadata(expected_catalog) = publication.changes else {
        return Err("expected the published RuntimeMetadata fixture".into());
    };
    let seed = client.read_session_runtime_catalog_page_admitted_v1(
        &retained,
        scope.clone(),
        0,
        &budget(),
        &memory,
    )?;
    let NativeIndexResponse::DslRuntimeCatalogPageV1(page) = seed.response() else {
        return Err(format!("retained RuntimeMetadata header refused: {:?}", seed.response()).into());
    };
    assert_eq!(page.view, ready);
    assert_eq!(page.structural_certificate, expected_catalog.corpus_certificate);
    let mut expected_payload = Vec::new();
    ciborium::ser::into_writer(&expected_catalog, &mut expected_payload)?;
    assert_eq!(page.bytes, expected_payload);
    assert_eq!(
        Some(page.output),
        quanta_index_contract::index::native_immutable_object_ref_v1(&expected_payload, &[]),
    );
    assert!(active.load(Ordering::SeqCst) > 0);
    drop(seed);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    let corpus: NativeDslPublicationV1 =
        serde_json::from_slice(&std::fs::read(inputs.path().join("native-corpus.json"))?)?;
    let NativeDslChangesV1::Corpus { files, .. } = corpus.changes else {
        return Err("expected the Structural fixture".into());
    };
    let roster = client.read_session_structural_chunks_admitted_v1(
        &retained, scope.clone(), None, &budget(), &memory,
    )?;
    let NativeIndexResponse::DslStructuralChunksPageV1(page) = roster.response() else {
        return Err(format!("Structural chunk roster refused: {:?}", roster.response()).into());
    };
    let mut owners = files.iter().filter_map(|file| {
        Some((
            quanta_index_contract::native_protocol::native_dsl_file_owner_v1(file.source_owner),
            file.next.as_ref()?,
        ))
    }).collect::<Vec<_>>();
    owners.sort_unstable_by_key(|(owner, _)| *owner);
    let expected = owners
        .iter()
        .flat_map(|(owner, file)| {
            file.chunks.iter().map(move |chunk| (*owner, chunk.chunk_id.as_str()))
        })
        .collect::<Vec<_>>();
    assert!(expected.windows(2).any(|pair| pair[0].0 != pair[1].0));
    assert_eq!(
        page.chunks
            .iter()
            .map(|chunk| (chunk.owner, chunk.chunk_id.as_str()))
            .collect::<Vec<_>>(),
        expected,
    );
    assert_eq!(page.view, ready);
    assert_eq!(page.structural_certificate, expected_catalog.corpus_certificate);
    assert!(page.after.is_none());
    let (owner, last_file) = owners.last().ok_or("missing Structural fixture owner")?;
    let resume = quanta_index_contract::native_protocol::NativeDslStructuralChunksCursorV1 {
        view: ready, projection: page.projection,
        structural_certificate: page.structural_certificate,
        owner: *owner, chunk_offset: Some(0),
    };
    drop(roster);
    let last = client.read_session_structural_chunks_admitted_v1(
        &retained, scope.clone(), Some(resume), &budget(), &memory,
    )?;
    let NativeIndexResponse::DslStructuralChunksPageV1(page) = last.response() else {
        return Err(format!("Structural chunk continuation refused: {:?}", last.response()).into());
    };
    assert_eq!(
        page.chunks
            .iter()
            .map(|chunk| (chunk.owner, chunk.chunk_id.as_str()))
            .collect::<Vec<_>>(),
        last_file.chunks
            .iter()
            .map(|chunk| (*owner, chunk.chunk_id.as_str()))
            .collect::<Vec<_>>(),
    );
    assert!(page.after.is_none());
    drop(last);
    let foreign = quanta_index_contract::native_protocol::NativeDslStructuralChunksCursorV1 {
        structural_certificate: ObjectRef(Digest([77; 32])), ..resume
    };
    let refused = client.read_session_structural_chunks_admitted_v1(
        &retained, scope.clone(), Some(foreign), &budget(), &memory,
    )?;
    assert!(matches!(refused.response(), NativeIndexResponse::Error(error)
        if error.code == NativeIndexErrorCode::InvalidInput));
    drop(refused);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    let page_size = quanta_index_contract::native_protocol::NATIVE_DSL_RUNTIME_CATALOG_PAGE_BYTES_V1;
    for (session, offset, code) in [
        (&missing, 0, NativeIndexErrorCode::SnapshotUnavailable),
        (&retained, page_size, NativeIndexErrorCode::InvalidInput),
    ] {
        let refusal = client.read_session_runtime_catalog_page_admitted_v1(
            session, scope.clone(), offset, &budget(), &memory,
        )?;
        assert!(matches!(refusal.response(), NativeIndexResponse::Error(error) if error.code == code));
    }
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(
        first.rows.candidate_ids().collect::<Vec<_>>(),
        ["e2e-3-src/changed.rs"]
    );
    request.after = first.after;
    let unrelated = OperationKey {
        producer: NATIVE_DSL_PRODUCER_V1,
        stream: scope.stream(NativeDslKindV1::RuntimeMetadata),
        ..operation(13)
    };
    let ingress = client.begin_input(unrelated, &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![native_text_spec()],
        },
        &budget(),
    )?;
    let same_projection = published(&client.submit(
        UpdateRequest {
            operation: unrelated,
            expected_view: ready,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    assert_ne!(same_projection, ready);
    let metadata = client.published_view_metadata_admitted(unrelated, &budget(), &memory)?;
    assert!(matches!(
        metadata.response(),
        NativeIndexResponse::Error(error) if error.code == NativeIndexErrorCode::DigestConflict
    ));
    drop(metadata);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    let second =
        client.open_read_session(ViewSelection::Exact(same_projection), [5; 32], &budget())?;
    let mut second_query = request.clone();
    second_query.after = None;
    let second_page = client.search_dsl_in_read_session_v1(&second, second_query, &budget())?;
    assert_eq!(second_page.projection, first.projection);
    assert_eq!(second_page.rows, first.rows);
    assert!(matches!(
        client.request(NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchDslV1 {
            session: second.session, query: request.clone(),
        }), &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
    ));
    let recertified = publish(
        &client,
        same_projection,
        &scope,
        NativeDslKindV1::Structural,
        1,
        NativeDslChangesV1::Corpus {
            header: NativeDslCorpusV1 {
                pin: pin()?,
                source_producer: operation(1).producer,
                structural_ready: false,
            },
            files: Vec::new(),
        },
    )?;
    let recertified_session =
        client.open_read_session(ViewSelection::Exact(recertified), [4; 32], &budget())?;
    let mut without_cursor = request.clone();
    without_cursor.after = None;
    let before_second = retained_bytes_after_response_drain(&running, &transport)?;
    assert!(matches!(
        client.search_dsl_in_read_session_v1(&recertified_session, without_cursor.clone(), &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::SnapshotUnavailable
    ));
    let stale_seed = client.read_session_runtime_catalog_page_admitted_v1(
        &recertified_session, scope.clone(), 0, &budget(), &memory,
    )?;
    assert!(matches!(
        stale_seed.response(), NativeIndexResponse::Error(error)
            if error.code == NativeIndexErrorCode::SnapshotUnavailable
    ));
    drop(stale_seed);
    let mut catalog: NativeDslPublicationV1 = serde_json::from_reader(std::fs::File::open(
        inputs.path().join(RUNTIME_CATALOG_INPUT),
    )?)?;
    catalog.operation.sequence = 14;
    catalog.expected_view = recertified;
    catalog.event_predecessor = 1;
    let NativeDslChangesV1::RuntimeMetadata(runtime) = &mut catalog.changes else {
        return Err("expected original catalog".into());
    };
    runtime.corpus_certificate = client
        .read_projection_binding(
            ViewSelection::Exact(recertified),
            scope.projection(NativeDslKindV1::Structural),
            &budget(),
        )?
        .binding
        .ok_or("expected re-sealed corpus")?
        .certificate;
    runtime.catalog.overlay_epoch_ms += 1;
    quanta_index_ipc::stamp_batch_digest_v1(&mut runtime.catalog)?;
    let sealed = client.prepare_dsl_publication_sealed_admitted_v1(&catalog, &budget(), &memory)?;
    assert_eq!(client.status(catalog.operation, &budget())?, OperationStatus::Unknown);
    assert_eq!(
        client.read_sealed_dsl_admitted_v1(sealed.submission.replay, &budget(), &memory)?,
        Some(sealed.submission),
    );
    assert_eq!(client.begin_input(catalog.operation, &budget())?, sealed.submission.ingress);
    assert_current(&client, recertified)?;
    let second_catalog = published(&client.submit_sealed_dsl_admitted_v1(
        sealed.submission, &budget(), &memory,
    )?)?;
    assert_eq!(
        published(&client.submit_sealed_dsl_admitted_v1(sealed.submission, &budget(), &memory)?)?,
        second_catalog,
    );
    assert_eq!(published(&client.publish_dsl_v1(&catalog, &budget())?)?, second_catalog);
    assert_eq!(
        client
            .published_view_metadata(catalog.operation, &budget())?
            .identity,
        second_catalog
    );
    let second_catalog_session =
        client.open_read_session(ViewSelection::Exact(second_catalog), [6; 32], &budget())?;
    let new_page = client.search_dsl_in_read_session_v1(
        &second_catalog_session,
        without_cursor.clone(),
        &budget(),
    )?;
    assert_eq!(new_page.rows, first.rows);
    assert_ne!(new_page.projection, first.projection);
    let both_cached = retained_bytes_after_response_drain(&running, &transport)?;
    assert!(
        both_cached > before_second,
        "both retained corpus and catalog versions are funded"
    );
    let previous_page =
        client.search_dsl_in_read_session_v1(&retained, without_cursor.clone(), &budget())?;
    assert_eq!(previous_page.rows, first.rows);
    assert_eq!(
        retained_bytes_after_response_drain(&running, &transport)?,
        both_cached
    );

    let (new_source, _) =
        publish_text_owner(&client, second_catalog, 11, 10, "src/new.rs", "new quartz")?;
    let latest = client.open_read_session(ViewSelection::Exact(new_source), [3; 32], &budget())?;
    assert!(matches!(
        client.search_dsl_in_read_session_v1(&latest, without_cursor, &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::SnapshotUnavailable
    ));
    let mut foreign = request.clone();
    foreign
        .after
        .as_mut()
        .ok_or("missing gold cursor")?
        .projection = ObjectRef(Digest([77; 32]));
    assert!(matches!(
        client.search_dsl_in_read_session_v1(&retained, foreign, &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
    ));

    assert!(matches!(
        client.search_dsl_in_read_session_v1(&latest, request.clone(), &budget()),
        Err(NativeUdsError::Protocol(_))
    ));
    let next = client.search_dsl_in_read_session_v1(&retained, request, &budget())?;
    assert_eq!(
        next.rows.candidate_ids().collect::<Vec<_>>(),
        ["e2e-4-src/stale.rs"]
    );
    assert!(next.after.is_none());
    for session in [
        missing,
        retained,
        second,
        recertified_session,
        second_catalog_session,
        latest,
    ] {
        let (released, _) = client.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
    }
    running.stop()
}
