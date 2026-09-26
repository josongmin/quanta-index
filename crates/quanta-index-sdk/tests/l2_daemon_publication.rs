//! L2 process proof against a caller-selected freshly built daemon binary.
//! Run with --ignored and QUANTA_INDEX_L2_TEST_BINARY. This test owns and stops
//! only its own child processes; no scripted peer or harness adapter is used.

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, QueryConstraintSetV1, RepoId,
    RepoRelativePath, RevisionId, SearchPlaneErrorCodeV2, SourceFileCoverage, SourceFileKey,
    SourceFileRevision, SourcePublicationEvent, SymbolCoverage, TextQueryRequest, TextQuerySyntax,
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
            .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS", "4")
            .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES", "67108864")
            .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS", "8")
            .env(
                "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
                "536870912",
            )
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        let child = command.spawn()?;
        let client = QuantaIndex::connect(
            ConnectOptions::from_state_root(root).with_request_io_timeout(Duration::from_secs(15)),
        )?;
        let mut daemon = Self { child, client };
        let probe = QuantaIndex::connect(
            ConnectOptions::from_state_root(root)
                .with_request_io_timeout(Duration::from_millis(200)),
        )?;
        let deadline = Instant::now() + Duration::from_secs(20);
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
                Ok(_) => std::thread::sleep(Duration::from_millis(20)),
                Err(error) if Instant::now() >= deadline => {
                    return Err(format!("daemon readiness: {error}").into());
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
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
        let deadline = Instant::now() + Duration::from_secs(15);
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
        if self.child.try_wait().ok().flatten().is_none() {
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
                unit_set_sha256: source_file_unit_set_sha256(&chunks, &symbols)?,
                text_admitted: true,
                symbols: SymbolCoverage::NotRequested,
            },
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
#[test]
#[ignore = "requires freshly built daemon: set QUANTA_INDEX_L2_TEST_BINARY and run --ignored"]
#[expect(
    clippy::print_stdout,
    reason = "the process proof records its retained artifact root"
)]
fn original_binding_delta_lineage_and_restart_through_real_daemon() -> TestResult {
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
    let other_revision = RevisionId::new("retargeted")?;
    let retargeted = corpus(
        99,
        None,
        other_revision.clone(),
        "event-one",
        None,
        &first_files,
    )?;
    let (replay, activated) = daemon
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
    let second = corpus(
        2,
        Some(1),
        revision()?,
        "event-two",
        Some("event-one"),
        &[("a.rs", "a-new", "newneedle")],
    )?;
    let (_, second_active) = daemon
        .client
        .search_corpus()
        .publish_and_activate(&second, Some(activated.active))?;
    assert!(query(&daemon.client, 2, "oldneedle")?.is_empty());
    assert_eq!(query(&daemon.client, 2, "newneedle")?, ["a-new"]);
    assert_eq!(query(&daemon.client, 2, "untouchedneedle")?, ["b-stable"]);
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
    let (_, third_active) = daemon
        .client
        .search_corpus()
        .publish_and_activate(&correct_base, Some(second_active.active))?;
    assert_eq!(query(&daemon.client, 3, "newneedle")?, ["a-new"]);
    assert_eq!(query(&daemon.client, 3, "untouchedneedle")?, ["b-stable"]);
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
        Some(third_active.active)
    );
    assert!(
        restarted
            .client
            .generations()
            .active_head(repo()?, other_revision)?
            .is_none()
    );
    assert_eq!(query(&restarted.client, 3, "newneedle")?, ["a-new"]);
    assert_eq!(
        query(&restarted.client, 3, "untouchedneedle")?,
        ["b-stable"]
    );
    restarted.stop()
}
