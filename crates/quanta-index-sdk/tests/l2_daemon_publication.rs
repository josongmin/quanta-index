//! L2 process proof against a caller-selected freshly built daemon binary.
//! Run with --ignored and `QUANTA_INDEX_L2_TEST_BINARY`. This test owns and stops
//! only its own child processes; no scripted peer or harness adapter is used.

#![expect(
    clippy::panic_in_result_fn,
    reason = "test assertions are the independent behavioral oracle; Result propagates fixture I/O errors"
)]

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, IngestObservationStatus, ManifestGeneration,
    QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId, SearchCorpusPublishOutcome,
    SearchPlaneErrorCodeV2, SourceFileCoverage, SourceFileKey, SourceFileRevision,
    SourcePublicationEvent, SymbolCoverage, TextQueryRequest, TextQuerySyntax,
    source_file_unit_set_sha256,
};
use quanta_index_sdk::{ConnectOptions, QuantaIndex, SdkError, SearchCorpusBatch};
use std::error::Error;
use std::fs::File;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

struct Daemon {
    child: Child,
    client: QuantaIndex,
}
impl Daemon {
    fn start(binary: &Path, root: &Path, phase: &str) -> TestResult<Self> {
        Self::start_with_policy(binary, root, phase, 4, None)
    }
    fn start_with_policy(
        binary: &Path,
        root: &Path,
        phase: &str,
        retained_generations: usize,
        crash_point: Option<&str>,
    ) -> TestResult<Self> {
        let log = File::create(root.with_file_name(format!("daemon-{phase}.log")))?;
        let mut command = Command::new(binary);
        // Deterministic local embedding and daemon policy, independent of the
        // caller's provider credentials/configuration. Preserve ordinary OS env.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("QUANTA_INDEX_") {
                let _configured = command.env_remove(key);
            }
        }
        let _configured = command
            .args(["serve", "--state-root"])
            .arg(root)
            .env("QUANTA_INDEX_EMBEDDER", "hash-dev")
            .env(
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS",
                retained_generations.to_string(),
            )
            .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES", "67108864")
            .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS", "8")
            .env(
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
                "536870912",
            )
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        if let Some(point) = crash_point {
            let _configured = command.env("QUANTA_INDEX_CRASH_POINT", point);
        }
        let client = QuantaIndex::connect(
            ConnectOptions::from_state_root(root).with_request_io_timeout(Duration::from_secs(15)),
        )?;
        let child = command.spawn()?;
        let mut daemon = Self { child, client };
        let probe = QuantaIndex::connect(
            ConnectOptions::from_state_root(root)
                .with_request_io_timeout(Duration::from_millis(200)),
        )?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(20))
            .ok_or("readiness deadline overflow")?;
        loop {
            if let Some(status) = daemon.child.try_wait()? {
                return Err(format!(
                    "daemon exited during {phase}: {status}; {}",
                    std::fs::read_to_string(root.with_file_name(format!("daemon-{phase}.log")))?
                )
                .into());
            }
            match probe.observability().process_readiness() {
                Ok(report) if report.ready => return Ok(daemon),
                Ok(report) if Instant::now() >= deadline => {
                    return Err(format!("daemon remains unready: {report:?}").into());
                }
                Err(error) if Instant::now() >= deadline => {
                    return Err(format!("daemon readiness: {error}").into());
                }
                Ok(_) | Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
    fn stop(mut self) -> TestResult {
        let status = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(self.child.id().to_string())
            .status()?;
        if !status.success() {
            return Err("failed to signal owned daemon".into());
        }
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(15))
            .ok_or("shutdown deadline overflow")?;
        loop {
            if let Some(status) = self.child.try_wait()? {
                if !status.success() {
                    return Err(format!("daemon shutdown failed: {status}").into());
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err("daemon shutdown timed out".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Daemon {
    fn drop(&mut self) {
        // A failed wait still requires best-effort cleanup. Normal completion
        // goes through stop(), which reports every shutdown failure.
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            let _killed = self.child.kill();
            let _waited = self.child.wait();
        }
    }
}
fn repo() -> TestResult<RepoId> {
    Ok(RepoId::new("l2-process")?)
}
fn revision() -> TestResult<RevisionId> {
    Ok(RevisionId::new("original")?)
}
fn corpus(
    generation: u64,
    base: Option<u64>,
    target_revision: RevisionId,
    event_id: &str,
    parent: Option<&str>,
    files: &[(&str, &str, &str)],
) -> TestResult<SearchCorpusBatch> {
    let mut batch = match base {
        Some(base) => SearchCorpusBatch::delta(
            repo()?,
            target_revision,
            ManifestGeneration::new(generation),
            ManifestGeneration::new(base),
            format!("manifest:{event_id}"),
        ),
        None => SearchCorpusBatch::replace_generation(
            repo()?,
            target_revision,
            ManifestGeneration::new(generation),
            format!("manifest:{event_id}"),
        ),
    }
    .source_event(SourcePublicationEvent {
        stream_id: "l2-process-stream".into(),
        event_id: event_id.into(),
        expected_base_event_id: parent.map(str::to_owned),
        payload_sha256: [0; 32],
    });
    for (path, id, text) in files {
        let language = LanguageCode::new("rust").map_err(str::to_string)?;
        let chunks = vec![ChunkRecord {
            chunk_id: ChunkId::new(*id),
            repo_relative_path: RepoRelativePath::new(*path),
            language: language.clone(),
            start_byte: 0,
            end_byte: u32::try_from(text.len())?,
            start_line: 1,
            end_line: 1,
            text: (*text).into(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }];
        // Fixed SHA-256 values of the literal fixture bytes, independently of
        // query results and the canonical producer unit-set commitment.
        let source_sha256 = match *text {
            "oldneedle" => [
                87, 127, 92, 22, 149, 167, 232, 55, 44, 199, 162, 215, 93, 161, 251, 58, 105, 105,
                178, 124, 113, 156, 195, 45, 169, 118, 92, 83, 235, 215, 126, 176,
            ],
            "newneedle" => [
                230, 132, 109, 76, 167, 62, 157, 39, 74, 107, 228, 163, 229, 153, 82, 252, 47, 225,
                219, 201, 113, 193, 5, 250, 97, 10, 184, 186, 219, 69, 57, 71,
            ],
            "untouchedneedle" => [
                133, 28, 15, 1, 150, 157, 34, 138, 113, 203, 26, 15, 195, 27, 199, 80, 94, 140,
                218, 128, 45, 23, 120, 199, 221, 129, 157, 68, 169, 52, 23, 46,
            ],
            _ => return Err("unregistered fixture text".into()),
        };
        let symbols = Vec::new();
        batch = batch.replace_scope(
            SourceFileCoverage {
                source: SourceFileRevision {
                    file: SourceFileKey {
                        source_repo_id: repo()?,
                        repo_relative_path: RepoRelativePath::new(*path),
                    },
                    revision_id: revision()?,
                    source_sha256,
                },
                language,
                producer_policy_sha256: [0x72; 32],
                symbol_name_source_policy:
                    quanta_index_contract::SymbolNameSourcePolicyV1::Unspecified,
                unit_set_sha256: source_file_unit_set_sha256(&chunks, &symbols)?,
                text_admitted: true,
                symbols: SymbolCoverage::NotRequested,
            },
            text.as_bytes().to_vec(),
            chunks,
            symbols,
        );
    }
    Ok(batch)
}
fn query(client: &QuantaIndex, generation: u64, needle: &str) -> TestResult<Vec<String>> {
    let response = client.lexical().query_request(TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: needle.into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(GenerationPin::new(
            repo()?,
            revision()?,
            ManifestGeneration::new(generation),
        )),
        generation_selector: None,
        top_k: 10,
        cursor: None,
    })?;
    Ok(response
        .results
        .into_iter()
        .map(|row| row.candidate_id)
        .collect())
}

fn assert_incomplete_symbols(client: &QuantaIndex, generation: u64) -> TestResult {
    // No symbol extractor ran for these files. Text remains searchable, but
    // zero matching symbols must not become an exhaustive, successful answer.
    assert!(matches!(
        client
            .symbol()
            .query()
            .native("absent_symbol")
            .pinned(GenerationPin::new(
                repo()?,
                revision()?,
                ManifestGeneration::new(generation),
            ))
            .top_k(10)
            .execute(),
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::SymbolCoverageIncomplete,
            ..
        })
    ));
    Ok(())
}
#[test]
#[ignore = "requires freshly built daemon: set QUANTA_INDEX_L2_TEST_BINARY and run --ignored"]
#[expect(
    clippy::print_stdout,
    reason = "the process proof records its retained artifact root"
)]
fn original_binding_delta_lineage_and_restart_through_real_daemon() -> TestResult {
    let assert_stages = |outcome: &SearchCorpusPublishOutcome, full: bool, text_write: bool| {
        let observation = outcome.observation.as_ref().expect("executed observation");
        assert_eq!(observation.status, IngestObservationStatus::Executed);
        assert!(observation.activation_ns.is_none());
        let lexical_build_ns = observation.lexical_build_ns.expect("lexical build clock");
        let stages = observation
            .lexical_stages
            .as_ref()
            .expect("lexical stage clocks");
        assert_eq!(stages.text_authority_collect_ns.is_some(), full);
        assert_eq!(stages.text_authority_shard_build_ns.is_some(), text_write);
        assert_eq!(stages.text_authority_publish_ns.is_some(), text_write);
        let children = [
            stages.text_authority_collect_ns,
            stages.text_authority_shard_build_ns,
            stages.text_authority_publish_ns,
        ]
        .into_iter()
        .flatten()
        .sum::<u64>();
        assert!(children <= stages.text_authority_ns);
        assert!(stages.text_authority_ns <= lexical_build_ns);
    };
    let requested_binary = std::env::var_os("QUANTA_INDEX_L2_TEST_BINARY")
        .ok_or("QUANTA_INDEX_L2_TEST_BINARY is required")?;
    let root = tempfile::Builder::new()
        .prefix("qi-l2-")
        .tempdir_in("/tmp")?
        .keep();
    println!("L2_PROCESS_ARTIFACT_ROOT={}", root.display());
    let state_root = root.join("state");
    let binary = root.join("daemon-under-test");
    let _copied_bytes = std::fs::copy(requested_binary, &binary)?;
    let daemon = Daemon::start(Path::new(&binary), state_root.as_path(), "first")?;
    let first_files = [
        ("a.rs", "a-old", "oldneedle"),
        ("b.rs", "b-stable", "untouchedneedle"),
    ];
    let original = corpus(1, None, revision()?, "event-one", None, &first_files)?;
    let first = daemon
        .client
        .producer()
        .publish_search_corpus_observed(&original)?;
    assert_stages(&first, true, true);
    assert_eq!(first.receipt.accepted_replace_scopes, 2);
    let other_revision = RevisionId::new("retargeted")?;
    let retargeted = corpus(
        99,
        None,
        other_revision.clone(),
        "event-one",
        None,
        &first_files,
    )?;
    let (replay, activated, _sdk_timings) = daemon
        .client
        .search_corpus()
        .publish_and_activate_observed(&retargeted, None)?;
    assert_eq!(replay.publication, first.publication);
    assert_eq!(replay.receipt, first.receipt.clone().replayed());
    assert_eq!(
        activated.active.generation.lexical,
        first.publication.target
    );
    assert!(
        daemon
            .client
            .generations()
            .active_head(repo()?, other_revision.clone())?
            .is_none()
    );
    assert_eq!(query(&daemon.client, 1, "oldneedle")?, ["a-old"]);
    assert_incomplete_symbols(&daemon.client, 1)?;
    let conflicting_replay = corpus(
        100,
        None,
        other_revision.clone(),
        "event-one",
        None,
        &[("a.rs", "a-new", "newneedle")],
    )?;
    assert!(matches!(
        daemon
            .client
            .producer()
            .publish_search_corpus_observed(&conflicting_replay),
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::BatchDigestConflict,
            ..
        })
    ));
    let second = corpus(
        2,
        Some(1),
        revision()?,
        "event-two",
        Some("event-one"),
        &[("a.rs", "a-new", "newneedle")],
    )?;
    let (second_outcome, second_active, _sdk_timings) = daemon
        .client
        .search_corpus()
        .publish_and_activate_observed(&second, Some(activated.active))?;
    assert_stages(&second_outcome, false, true);
    assert_eq!(second_outcome.receipt.accepted_replace_scopes, 1);
    assert!(query(&daemon.client, 2, "oldneedle")?.is_empty());
    assert_eq!(query(&daemon.client, 2, "newneedle")?, ["a-new"]);
    assert_eq!(query(&daemon.client, 2, "untouchedneedle")?, ["b-stable"]);
    assert_eq!(query(&daemon.client, 1, "oldneedle")?, ["a-old"]);
    assert!(query(&daemon.client, 1, "newneedle")?.is_empty());
    let wrong_base = corpus(
        3,
        Some(1),
        revision()?,
        "event-three",
        Some("event-two"),
        &[],
    )?;
    assert!(matches!(
        daemon
            .client
            .producer()
            .publish_search_corpus_observed(&wrong_base),
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::DeltaBaseConflict,
            ..
        })
    ));
    let correct_base = corpus(
        3,
        Some(2),
        revision()?,
        "event-three",
        Some("event-two"),
        &[],
    )?;
    let (third_outcome, third_active, _sdk_timings) = daemon
        .client
        .search_corpus()
        .publish_and_activate_observed(&correct_base, Some(second_active.active))?;
    assert_stages(&third_outcome, false, false);
    assert_eq!(third_outcome.receipt.accepted_replace_scopes, 0);
    assert_eq!(third_outcome.receipt.accepted_tombstone_scopes, 0);
    assert_eq!(query(&daemon.client, 3, "newneedle")?, ["a-new"]);
    assert_eq!(query(&daemon.client, 3, "untouchedneedle")?, ["b-stable"]);
    let deleted = corpus(
        4,
        Some(3),
        revision()?,
        "event-four",
        Some("event-three"),
        &[],
    )?
    .tombstone_scope(SourceFileKey {
        source_repo_id: repo()?,
        repo_relative_path: RepoRelativePath::new("a.rs"),
    });
    let (deleted_outcome, fourth_active, _sdk_timings) = daemon
        .client
        .search_corpus()
        .publish_and_activate_observed(&deleted, Some(third_active.active))?;
    assert_stages(&deleted_outcome, false, true);
    assert_eq!(deleted_outcome.receipt.accepted_replace_scopes, 0);
    assert_eq!(deleted_outcome.receipt.accepted_tombstone_scopes, 1);
    assert!(query(&daemon.client, 4, "newneedle")?.is_empty());
    assert_eq!(query(&daemon.client, 4, "untouchedneedle")?, ["b-stable"]);
    assert_eq!(query(&daemon.client, 3, "newneedle")?, ["a-new"]);
    daemon.stop()?;

    let restarted = Daemon::start(Path::new(&binary), state_root.as_path(), "restart")?;
    let replay = restarted
        .client
        .producer()
        .publish_search_corpus_observed(&retargeted)?;
    assert_eq!(replay.publication, first.publication);
    assert_eq!(replay.receipt, first.receipt.replayed());
    assert_eq!(
        restarted
            .client
            .generations()
            .active_head(repo()?, revision()?)?,
        Some(fourth_active.active)
    );
    assert!(
        restarted
            .client
            .generations()
            .active_head(repo()?, other_revision)?
            .is_none()
    );
    assert_eq!(query(&restarted.client, 3, "newneedle")?, ["a-new"]);
    assert_incomplete_symbols(&restarted.client, 3)?;
    assert_eq!(query(&restarted.client, 1, "oldneedle")?, ["a-old"]);
    assert!(query(&restarted.client, 4, "newneedle")?.is_empty());
    assert_eq!(
        query(&restarted.client, 4, "untouchedneedle")?,
        ["b-stable"]
    );
    assert_eq!(
        query(&restarted.client, 3, "untouchedneedle")?,
        ["b-stable"]
    );
    restarted.stop()
}

#[test]
#[ignore = "requires freshly built daemon: set QUANTA_INDEX_L2_TEST_BINARY and run --ignored"]
#[expect(
    clippy::print_stdout,
    reason = "the process proof records its retained artifact root"
)]
fn activation_failure_preserves_original_publication_across_restart() -> TestResult {
    let requested_binary = std::env::var_os("QUANTA_INDEX_L2_TEST_BINARY")
        .ok_or("QUANTA_INDEX_L2_TEST_BINARY is required")?;
    let root = tempfile::Builder::new()
        .prefix("qi-l2-activation-")
        .tempdir_in("/tmp")?
        .keep();
    println!("L2_PROCESS_ARTIFACT_ROOT={}", root.display());
    let state_root = root.join("state");
    let binary = root.join("daemon-under-test");
    let _copied_bytes = std::fs::copy(requested_binary, &binary)?;
    let daemon = Daemon::start(&binary, &state_root, "activation-first")?;
    let first = corpus(
        1,
        None,
        revision()?,
        "activation-one",
        None,
        &[("a.rs", "a-old", "oldneedle")],
    )?;
    let (_, first_active, _) = daemon
        .client
        .search_corpus()
        .publish_and_activate_observed(&first, None)?;
    let second = corpus(
        2,
        Some(1),
        revision()?,
        "activation-two",
        Some("activation-one"),
        &[("a.rs", "a-new", "newneedle")],
    )?;
    // Explicitly expecting no active head is valid input but conflicts with G1.
    // Publication must succeed first, without converting the CAS refusal to ACK.
    let failure = daemon
        .client
        .search_corpus()
        .publish_and_activate_observed(&second, None)
        .expect_err("existing G1 must reject activation expecting an empty head");
    let retained = match failure {
        SdkError::ActivationAfterPublish { evidence, source } => {
            assert!(matches!(
                *source,
                SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::CompositeActivationCasConflict,
                    ..
                }
            ));
            evidence
        }
        other @ (SdkError::Usage(_)
        | SdkError::Protocol(_)
        | SdkError::Serialization(_)
        | SdkError::Transport(_)
        | SdkError::Binding { .. }
        | SdkError::PlaneUnavailable { .. }
        | SdkError::Remote { .. }) => {
            return Err(format!("publication evidence was lost: {other}").into());
        }
    };
    assert!(retained.receipt.applied);
    assert_eq!(retained.receipt.accepted_replace_scopes, 1);
    assert_eq!(retained.receipt.accepted_tombstone_scopes, 0);
    assert_eq!(
        retained.publication.target.manifest_generation,
        ManifestGeneration::new(2)
    );
    assert_eq!(retained.publication.target.revision_id, revision()?);
    assert_eq!(query(&daemon.client, 2, "newneedle")?, ["a-new"]);
    assert_eq!(query(&daemon.client, 1, "oldneedle")?, ["a-old"]);
    assert_eq!(
        daemon
            .client
            .generations()
            .active_head(repo()?, revision()?)?,
        Some(first_active.active.clone())
    );
    daemon.stop()?;

    let restarted = Daemon::start(&binary, &state_root, "activation-restart")?;
    let attempted_revision = RevisionId::new("activation-retargeted")?;
    let retargeted = corpus(
        99,
        Some(1),
        attempted_revision.clone(),
        "activation-two",
        Some("activation-one"),
        &[("a.rs", "a-new", "newneedle")],
    )?;
    let replay_failure = restarted
        .client
        .search_corpus()
        .publish_and_activate_observed(&retargeted, None)
        .expect_err("restart must preserve the original active head and CAS refusal");
    assert_eq!(
        replay_failure.published_publication(),
        Some(&retained.publication)
    );
    assert_eq!(
        replay_failure.published_receipt(),
        Some(&retained.receipt.clone().replayed())
    );
    assert!(matches!(
        replay_failure,
        SdkError::ActivationAfterPublish { source, .. }
            if matches!(*source, SdkError::Remote {
                code: SearchPlaneErrorCodeV2::CompositeActivationCasConflict,
                ..
            })
    ));
    let observed_head = restarted
        .client
        .generations()
        .active_head(repo()?, revision()?)?
        .ok_or("original active head disappeared after restart")?;
    assert_eq!(observed_head, first_active.active);
    let (replayed, activated, _) = restarted
        .client
        .search_corpus()
        .publish_and_activate_observed(&retargeted, Some(observed_head))?;
    assert_eq!(replayed.publication, retained.publication);
    assert_eq!(replayed.receipt, retained.receipt.replayed());
    assert_eq!(
        activated.active.generation.lexical,
        replayed.publication.target
    );
    assert_eq!(query(&restarted.client, 2, "newneedle")?, ["a-new"]);
    assert!(query(&restarted.client, 2, "oldneedle")?.is_empty());
    assert_eq!(query(&restarted.client, 1, "oldneedle")?, ["a-old"]);
    assert!(
        restarted
            .client
            .generations()
            .active_head(repo()?, attempted_revision)?
            .is_none()
    );
    restarted.stop()
}

#[test]
#[ignore = "requires freshly built daemon: set QUANTA_INDEX_L2_TEST_BINARY and run --ignored"]
#[expect(
    clippy::print_stdout,
    reason = "the process proof records its retained artifact root"
)]
fn unresolved_cross_stream_publication_orders_activation_after_restart() -> TestResult {
    let requested_binary = std::env::var_os("QUANTA_INDEX_L2_TEST_BINARY")
        .ok_or("QUANTA_INDEX_L2_TEST_BINARY is required")?;
    let root = tempfile::Builder::new()
        .prefix("qi-l2-cross-stream-")
        .tempdir_in("/tmp")?
        .keep();
    println!("L2_PROCESS_ARTIFACT_ROOT={}", root.display());
    let state_root = root.join("state");
    let binary = root.join("daemon-under-test");
    let _copied_bytes = std::fs::copy(requested_binary, &binary)?;
    let first = corpus(
        1,
        None,
        revision()?,
        "cross-a",
        None,
        &[("a.rs", "a-one", "oldneedle")],
    )?
    .source_event(SourcePublicationEvent {
        stream_id: "cross-stream-a".into(),
        event_id: "cross-a".into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    });
    let second = corpus(
        2,
        None,
        revision()?,
        "cross-b",
        None,
        &[("a.rs", "a-two", "newneedle")],
    )?
    .source_event(SourcePublicationEvent {
        stream_id: "cross-stream-b".into(),
        event_id: "cross-b".into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    });
    let daemon = Daemon::start_with_policy(&binary, &state_root, "cross-first", 2, None)?;
    let _first_stage = daemon
        .client
        .producer()
        .publish_search_corpus_observed(&first)?;
    assert!(matches!(
        daemon
            .client
            .producer()
            .publish_search_corpus_observed(&second),
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::NotReady,
            ..
        })
    ));
    assert!(
        daemon
            .client
            .generations()
            .active_head(repo()?, revision()?)?
            .is_none()
    );
    daemon.stop()?;

    let restarted = Daemon::start_with_policy(&binary, &state_root, "cross-restart", 2, None)?;
    assert!(matches!(
        restarted
            .client
            .search_corpus()
            .publish_and_activate(&second, None),
        Err(SdkError::Remote {
            code: SearchPlaneErrorCodeV2::NotReady,
            ..
        })
    ));
    let (_, first_activation) = restarted
        .client
        .search_corpus()
        .publish_and_activate(&first, None)?;
    let (_, second_activation) = restarted
        .client
        .search_corpus()
        .publish_and_activate(&second, Some(first_activation.active))?;
    assert_eq!(
        restarted
            .client
            .generations()
            .active_head(repo()?, revision()?)?,
        Some(second_activation.active)
    );
    assert_eq!(query(&restarted.client, 2, "newneedle")?, ["a-two"]);
    restarted.stop()
}

#[test]
#[ignore = "requires debug daemon crash hooks: set QUANTA_INDEX_L2_TEST_BINARY and run --ignored"]
#[expect(
    clippy::print_stdout,
    reason = "the process proof records its retained artifact root"
)]
fn delta_recovers_across_named_crash_cuts_with_rolled_back_active_head() -> TestResult {
    let requested_binary = std::env::var_os("QUANTA_INDEX_L2_TEST_BINARY")
        .ok_or("QUANTA_INDEX_L2_TEST_BINARY is required")?;
    let root = tempfile::Builder::new()
        .prefix("qi-l2-crash-")
        .tempdir_in("/tmp")?
        .keep();
    println!("L2_PROCESS_ARTIFACT_ROOT={}", root.display());
    let binary = root.join("daemon-under-test");
    let _copied_bytes = std::fs::copy(requested_binary, &binary)?;
    for point in [
        "after_semantic_seal",
        "before_authority_record",
        "after_retention_receipt",
        "after_catalog_transaction",
        "after_ledger_reconcile",
        "after_fence",
        "between_track_reclaims",
        "before_record_forget",
    ] {
        let case = root.join(point);
        std::fs::create_dir(&case)?;
        recover_delta_after_crash(&binary, &case.join("state"), point)?;
        println!("L2_CRASH_CUT_VERIFIED={point} EXIT=86");
    }
    Ok(())
}

fn only_pair_directory(root: &Path) -> TestResult<std::path::PathBuf> {
    let mut directories = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && !matches!(entry.file_name().to_str(), Some(".staging" | ".reclaim"))
        {
            directories.push(entry.path());
        }
    }
    assert_eq!(directories.len(), 1, "fixture must have exactly one pair");
    directories
        .pop()
        .ok_or_else(|| "missing fixture pair".into())
}

fn recover_delta_after_crash(binary: &Path, state: &Path, point: &str) -> TestResult {
    use quanta_index_contract::SearchPlaneRollbackSearchCorpusGenerationCasRequest;
    let initial = Daemon::start_with_policy(binary, state, "initial", 2, None)?;
    let first = corpus(
        1,
        None,
        revision()?,
        "event-one",
        None,
        &[
            ("a.rs", "a-old", "oldneedle"),
            ("b.rs", "b-stable", "untouchedneedle"),
        ],
    )?;
    let (_, first_active) = initial
        .client
        .search_corpus()
        .publish_and_activate(&first, None)
        .map_err(|error| format!("{point}: initial generation one publication: {error}"))?;
    let second = corpus(
        2,
        Some(1),
        revision()?,
        "event-two",
        Some("event-one"),
        &[("a.rs", "a-new", "newneedle")],
    )?;
    let (_, second_active) = initial
        .client
        .search_corpus()
        .publish_and_activate(&second, Some(first_active.active.clone()))
        .map_err(|error| format!("{point}: initial generation two publication: {error}"))?;
    // Source lineage remains event-two, while visible generation one is pinned.
    // Retention of target three may now retire its inactive physical base two.
    let rollback = initial.client.generations().rollback(
        SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: second_active.active,
            target: first_active.active.generation,
        },
    )?;
    assert_eq!(
        rollback.active.generation.lexical.manifest_generation,
        ManifestGeneration::new(1)
    );
    initial.stop()?;

    let mut crashing = Daemon::start_with_policy(binary, state, "crash", 2, Some(point))?;
    let third = corpus(
        3,
        Some(2),
        revision()?,
        "event-three",
        Some("event-two"),
        &[],
    )?;
    assert!(
        crashing
            .client
            .producer()
            .publish_search_corpus_observed(&third)
            .is_err()
    );
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(15))
        .ok_or("crash deadline overflow")?;
    loop {
        if let Some(status) = crashing.child.try_wait()? {
            assert_eq!(status.code(), Some(86));
            break;
        }
        if Instant::now() >= deadline {
            return Err("named crash point did not terminate the daemon".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(crashing);

    // Inspect persisted state before starting recovery: the declared retention
    // cap alone is not evidence that the base authority was actually retired.
    if !matches!(point, "after_semantic_seal" | "before_authority_record") {
        let pair = only_pair_directory(&state.join("authorities/search-corpus"))?;
        assert!(pair.join("g1.cbor").try_exists()?);
        assert!(pair.join("g3.cbor").try_exists()?);
        assert!(
            !pair.join("g2.cbor").try_exists()?,
            "base authority survives {point}"
        );
    }
    for track in ["lexical", "semantic"] {
        if point == "before_record_forget"
            || (point == "between_track_reclaims" && track == "lexical")
        {
            let pair = only_pair_directory(&state.join("indexes").join(track))?;
            assert!(pair.join("g1").try_exists()?);
            assert!(pair.join("g3").try_exists()?);
            assert!(
                !pair.join("g2").try_exists()?,
                "physical {track} base survives {point}"
            );
        }
    }

    let recovered = Daemon::start_with_policy(binary, state, "recovered", 2, None)?;
    let current = recovered
        .client
        .generations()
        .active_head(repo()?, revision()?)?
        .ok_or("rollback head disappeared after crash")?;
    assert_eq!(current, rollback.active);
    let (publication, active, _sdk_timings) = recovered
        .client
        .search_corpus()
        .publish_and_activate_observed(&third, Some(current))
        .map_err(|error| format!("{point}: recovered generation three publication: {error}"))?;
    assert!(publication.receipt.applied);
    assert_eq!(
        publication.publication.target.manifest_generation,
        ManifestGeneration::new(3)
    );
    assert_eq!(query(&recovered.client, 3, "newneedle")?, ["a-new"]);
    assert_eq!(
        query(&recovered.client, 3, "untouchedneedle")?,
        ["b-stable"]
    );
    assert!(query(&recovered.client, 3, "oldneedle")?.is_empty());
    let replay = recovered
        .client
        .producer()
        .publish_search_corpus_observed(&third)?;
    assert_eq!(replay.receipt, publication.receipt.replayed());
    assert_eq!(replay.publication, publication.publication);
    assert_eq!(
        recovered
            .client
            .generations()
            .active_head(repo()?, revision()?)?,
        Some(active.active)
    );
    recovered.stop()
}
