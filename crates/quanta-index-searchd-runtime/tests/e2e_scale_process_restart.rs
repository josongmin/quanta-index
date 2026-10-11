//! Declared scale-tier source fixtures across real daemon OS-process restart.
//!
//! This is a functional process/custody regression, not a scale timing or
//! Linux release/physical-I/O qualification. The two daemons publish/open the
//! same production state root through SDK and disk adapters.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, LexicalCandidate, ManifestGeneration, RepoId,
    RepoRelativePath, RevisionId, SearchPlaneErrorCodeV2, SourceFileKey, SourcePublicationEvent,
    TextQueryResponse, lex::LanguageCode,
};
use quanta_index_sdk::{ConnectOptions, QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd_harness::{
    E2eRuntime, fixture_source_scope_v1, private_tempdir,
    scale::{
        SCALE_HISTORY_MAX_BYTES, SCALE_HISTORY_MAX_TOTAL_BYTES, ScaleTier, ScopedFile,
        ScopedOracle, generate_scoped_corpus, params_for, repo_query_token,
    },
};
use sha2::{Digest, Sha256};

use crate::fail_closed_wait::{RealTicker, wait_for};
use crate::searchd_binary_process::{SearchdBinaryProcess, terminate_child, wait_for_sockets};

type TestResult = Result<(), Box<dyn Error>>;

const SEED: u64 = 0x5161_5343_414c_4531;
const REPO: &str = "repo-scale-process-restart";
const REVISION: &str = "revision-scale-process-restart";
const EVENT_1: &str = "fixture:scale-process-restart:g1";
const EVENT_2: &str = "fixture:scale-process-restart:g2";
const RETAINED_TOKEN: &str = "scalefileneedle001file00000";
const DELETED_TOKEN: &str = "scalefileneedle000file00000";
const SUPPORTED_PROFILE_TIMEOUT: Duration = Duration::from_secs(600);

fn repo() -> Result<RepoId, Box<dyn Error>> {
    Ok(RepoId::new(REPO)?)
}

fn revision() -> Result<RevisionId, Box<dyn Error>> {
    Ok(RevisionId::new(REVISION)?)
}

fn event(id: &str, prior: Option<&str>) -> SourcePublicationEvent {
    SourcePublicationEvent {
        stream_id: "fixture:scale-process-restart".to_string(),
        event_id: id.to_string(),
        expected_base_event_id: prior.map(str::to_string),
        payload_sha256: [0; 32],
    }
}

fn initial_batch(
    files: Vec<ScopedFile>,
    tier: ScaleTier,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let params = params_for(tier);
    let fixed_count = match tier {
        ScaleTier::Medium => 256,
        ScaleTier::Large => 4_096,
        ScaleTier::Xlarge => 32_768,
        ScaleTier::Small => {
            return Err("process restart fixture requires medium, large or xlarge".into());
        }
    };
    if files.len() != usize::try_from(params.total_files())? || files.len() != fixed_count {
        return Err(format!(
            "source-derived {tier:?} tier must contain exactly {fixed_count} files"
        )
        .into());
    }
    let owner = repo()?;
    let revision = revision()?;
    let mut batch = SearchCorpusBatch::replace_generation(
        owner,
        revision.clone(),
        ManifestGeneration::new(1),
        "manifest:scale-process-restart:g1",
    )
    .source_event(event(EVENT_1, None));
    for (index, file) in files.into_iter().enumerate() {
        let source_repo = RepoId::new(file.source_repo_id.as_str())?;
        let path = RepoRelativePath::new(file.repo_relative_path.as_str());
        let end_byte = u32::try_from(file.content.len())?;
        let end_line = u32::try_from(file.content.lines().count())?;
        let record = ChunkRecord {
            chunk_id: ChunkId::new(format!("scale-process-{index}")),
            repo_relative_path: path.clone(),
            language: LanguageCode::new("rust")?,
            start_byte: 0,
            end_byte,
            start_line: 1,
            end_line,
            text: file.content.into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: Some(source_repo.clone()),
        };
        let scope = fixture_source_scope_v1(
            SourceFileKey {
                source_repo_id: source_repo,
                repo_relative_path: path,
            },
            revision.clone(),
            vec![record],
            Vec::new(),
        )?;
        batch = batch.replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );
    }
    Ok(batch)
}

fn tombstone_batch() -> Result<SearchCorpusBatch, Box<dyn Error>> {
    Ok(SearchCorpusBatch::delta(
        repo()?,
        revision()?,
        ManifestGeneration::new(2),
        ManifestGeneration::new(1),
        "manifest:scale-process-restart:g2",
    )
    .source_event(event(EVENT_2, Some(EVENT_1)))
    .tombstone_scope(SourceFileKey {
        source_repo_id: RepoId::new("repo0")?,
        repo_relative_path: RepoRelativePath::new("src/file_0.rs"),
    }))
}

type SourceKey = (String, String);

struct SourceTruth {
    // fixture_source_scope_v1 appends one newline after each chunk's bytes.
    source: BTreeMap<SourceKey, ([u8; 32], u32)>,
    repo2_paths: BTreeSet<SourceKey>,
}

fn source_truth(files: &[ScopedFile], tier: ScaleTier) -> Result<SourceTruth, Box<dyn Error>> {
    let _oracle = ScopedOracle::from_source(files, tier)?;
    let mut source = BTreeMap::new();
    let mut retained = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    let mut repo2_paths = BTreeSet::new();
    let mut repo2_token_paths = BTreeSet::new();
    for file in files {
        let key = (file.source_repo_id.clone(), file.repo_relative_path.clone());
        let digest: [u8; 32] = Sha256::digest(format!("{}\n", file.content).as_bytes()).into();
        let lines = u32::try_from(file.content.lines().count())?;
        if source.insert(key.clone(), (digest, lines)).is_some() {
            return Err("fixture contains duplicate source identity".into());
        }
        if file.content.contains(RETAINED_TOKEN) {
            let _inserted = retained.insert(key.clone());
        }
        if file.content.contains(DELETED_TOKEN) {
            let _inserted = deleted.insert(key.clone());
        }
        if file.source_repo_id == "repo2" {
            let _inserted = repo2_paths.insert(key.clone());
        }
        if file.content.contains(&repo_query_token(2)) {
            let _inserted = repo2_token_paths.insert(key);
        }
    }
    if retained != BTreeSet::from([("repo1".into(), "src/file_0.rs".into())])
        || deleted != BTreeSet::from([("repo0".into(), "src/file_0.rs".into())])
        || repo2_paths.len() != usize::try_from(params_for(tier).files_per_repo)?
        || repo2_token_paths != repo2_paths
    {
        return Err("planted query tokens differ from independent fixture source".into());
    }
    Ok(SourceTruth {
        source,
        repo2_paths,
    })
}

fn wait_ready(client: &QuantaIndex, tier: ScaleTier) -> TestResult {
    let timeout = if tier == ScaleTier::Medium {
        Duration::from_secs(30)
    } else {
        SUPPORTED_PROFILE_TIMEOUT
    };
    let _ready = wait_for(
        &RealTicker::new(),
        timeout,
        Duration::from_millis(50),
        "real daemon ready with one active repository",
        || client.observability().process_readiness(),
        |report| report.ready && report.active_repositories == 1,
        |error| {
            matches!(
                error,
                SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::NotReady,
                    ..
                }
            )
        },
    )?;
    Ok(())
}

fn query(
    client: &QuantaIndex,
    token: &str,
    top_k: u32,
) -> Result<TextQueryResponse, Box<dyn Error>> {
    Ok(client
        .lexical()
        .query()
        .native(token)
        .active(repo()?, revision()?)
        .top_k(top_k)
        .execute()?)
}

fn check_row(row: &LexicalCandidate, truth: &SourceTruth, generation: u64) -> TestResult {
    let key = (
        row.source_repo_id.as_str().to_owned(),
        row.repo_relative_path.as_str().to_owned(),
    );
    let (expected_hash, expected_lines) = truth
        .source
        .get(&key)
        .ok_or_else(|| format!("foreign source result: {key:?}"))?;
    let source = row
        .source
        .as_ref()
        .ok_or("result has no source commitment")?;
    if source.file.source_repo_id != row.source_repo_id
        || source.file.repo_relative_path != row.repo_relative_path
        || source.revision_id != revision()?
        || source.source_sha256 != *expected_hash
        || row.repo_id != repo()?
        || row.revision_id != revision()?
        || row.manifest_generation != ManifestGeneration::new(generation)
        || row.start_line != 1
        || row.end_line != *expected_lines
    {
        return Err(format!("source identity or bytes changed for {key:?}").into());
    }
    Ok(())
}

fn unique_hit(
    client: &QuantaIndex,
    token: &str,
    generation: u64,
    expected: (&str, &str),
    truth: &SourceTruth,
) -> Result<LexicalCandidate, Box<dyn Error>> {
    let response = query(client, token, 10)?;
    if response.generation
        != GenerationPin::new(repo()?, revision()?, ManifestGeneration::new(generation))
        || response.results.len() != 1
        || response.next_cursor.is_some()
    {
        return Err(format!("unique source query returned wrong page: {token}").into());
    }
    let row = response
        .results
        .into_iter()
        .next()
        .ok_or("missing unique source row")?;
    check_row(&row, truth, generation)?;
    if row.source_repo_id.as_str() != expected.0 || row.repo_relative_path.as_str() != expected.1 {
        return Err(format!("wrong source for unique token {token}").into());
    }
    Ok(row)
}

#[derive(PartialEq)]
struct Observed {
    retained: LexicalCandidate,
    retained_score_bits: u32,
    scoped: Vec<LexicalCandidate>,
    scoped_score_bits: Vec<u32>,
}

fn observe_generation_two(
    client: &QuantaIndex,
    truth: &SourceTruth,
    tier: ScaleTier,
) -> Result<Observed, Box<dyn Error>> {
    wait_ready(client, tier)?;
    let retained = unique_hit(client, RETAINED_TOKEN, 2, ("repo1", "src/file_0.rs"), truth)?;
    let deleted = query(client, DELETED_TOKEN, 10)?;
    let expected_pin = GenerationPin::new(repo()?, revision()?, ManifestGeneration::new(2));
    if deleted.generation != expected_pin
        || !deleted.results.is_empty()
        || deleted.next_cursor.is_some()
    {
        return Err("tombstoned source reappeared".into());
    }
    let scoped = query(
        client,
        &repo_query_token(2),
        params_for(tier).files_per_repo,
    )?;
    let mut observed = BTreeSet::new();
    for row in &scoped.results {
        check_row(row, truth, 2)?;
        if !observed.insert((
            row.source_repo_id.as_str().to_owned(),
            row.repo_relative_path.as_str().to_owned(),
        )) {
            return Err("duplicate source identity in ranked page".into());
        }
    }
    if scoped.generation != expected_pin
        || scoped.results.len() != truth.repo2_paths.len()
        || observed != truth.repo2_paths
        || scoped.next_cursor.is_some()
    {
        return Err("repo2 ranked page differs from source-derived complete set".into());
    }
    Ok(Observed {
        retained_score_bits: retained.score.to_bits(),
        retained,
        scoped_score_bits: scoped
            .results
            .iter()
            .map(|row| row.score.to_bits())
            .collect(),
        scoped: scoped.results,
    })
}

#[test]
fn medium_scale_source_survives_real_daemon_process_restart_and_delete() -> TestResult {
    run_scale_process_restart(ScaleTier::Medium)
}

#[test]
#[ignore = "manual costly Large4096 supported-profile real daemon process restart"]
fn large_scale_source_survives_real_daemon_process_restart_and_delete() -> TestResult {
    run_scale_process_restart(ScaleTier::Large)
}

#[test]
#[ignore = "manual costly XL32768 supported-profile real daemon process restart"]
fn xlarge_scale_source_survives_real_daemon_process_restart_and_delete() -> TestResult {
    run_scale_process_restart(ScaleTier::Xlarge)
}

#[test]
fn restored_shared_object_corpus_serves_in_two_fresh_daemon_processes() -> TestResult {
    let source = private_tempdir()?;
    let destination = private_tempdir()?;
    let path = "src/restored.rs";
    let body = "pub fn restored_process_needle_73() {}";
    let mut runtime = E2eRuntime::boot_in(source.path())?;
    let expected_id = runtime.ingest_text_with_candidate_id("repo-e2e", path, body)?;
    runtime.ingest_text("repo-e2e", "src/decoy.rs", "pub fn unrelated() {}\n")?;
    let generation = runtime.seal()?;
    runtime.activate_last_sealed_generation()?;
    let pin = GenerationPin::new(runtime.repo(), runtime.revision(), generation);
    runtime.stop()?;
    let incarnation = "activations/.activation-root-incarnation-v1";
    let source_incarnation = std::fs::read(source.path().join(incarnation))?;
    let command = scale_process_command(source.path())?;
    let backup = destination.path().join("backup");
    let restored = destination.path().join("restored");
    for (operation, from, to) in [
        ("backup-state", source.path(), backup.as_path()),
        ("restore-state", backup.as_path(), restored.as_path()),
    ] {
        let output = Command::new(command.get_program())
            .arg(operation)
            .arg("--source")
            .arg(from)
            .arg("--destination")
            .arg(to)
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "{operation} refused: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
    }
    if std::fs::read(restored.join(incarnation))? == source_incarnation {
        return Err("restore did not rotate the allocation incarnation".into());
    }
    // The fixture source contains the chunk followed by one framing newline.
    let expected_hash: [u8; 32] =
        Sha256::digest(b"pub fn restored_process_needle_73() {}\n").into();
    // Fixed SHA-256 golden for the public CodeSearch file identity framing.
    let file_id = "file:8998eaad62423ab013d01fd4a54ffa4068781322a295c2aafae820b56c0f2e31";
    let mut first_answer = None;
    for _ in 0..2 {
        let daemon = start_process(&restored, ScaleTier::Medium)?;
        let client = daemon.connect()?;
        let mut answers = Vec::new();
        for (query, id) in [
            (
                client
                    .lexical()
                    .query()
                    .native("restored_process_needle_73"),
                expected_id.as_str(),
            ),
            (
                client
                    .lexical()
                    .query()
                    .code_search("restored_process_needle_73"),
                file_id,
            ),
        ] {
            let answer = query.pinned(pin.clone()).top_k(10).execute()?;
            if answer.generation != pin || answer.results.len() != 1 {
                return Err("restored process returned a different generation or hit count".into());
            }
            let row = answer.results.first().ok_or("restored result absent")?;
            if row.candidate_id.as_str() != id
                || row.repo_relative_path.as_str() != path
                || row.source.as_ref().map(|source| source.source_sha256) != Some(expected_hash)
            {
                return Err("restored process changed the source identity or bytes".into());
            }
            answers.push(answer.results);
        }
        if let Some(first) = &first_answer {
            if first != &answers {
                return Err("fresh daemon restart changed the restored ranked result".into());
            }
        } else {
            first_answer = Some(answers);
        }
        drop(client);
        daemon.stop()?;
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "trace assertions are fixed goldens; process and I/O failures propagate"
)]
fn trace_timeline_process_close_keeps_clock_domains_separate() -> TestResult {
    let first_state = private_tempdir()?;
    let second_state = private_tempdir()?;
    prepare_trace_state(first_state.path())?;
    prepare_trace_state(second_state.path())?;
    let first_trace = first_state.path().join("first-process.jsonl");
    let second_trace = second_state.path().join("second-process.jsonl");

    let mut first = start_traced_process(first_state.path(), &first_trace)?;
    std::thread::sleep(Duration::from_millis(150));
    stop_with_sigterm(&mut first)?;
    let mut second = start_traced_process(second_state.path(), &second_trace)?;
    std::thread::sleep(Duration::from_millis(150));
    stop_with_sigterm(&mut second)?;

    let first_rows = read_trace_rows(&first_trace)?;
    let second_rows = read_trace_rows(&second_trace)?;
    let first_capture = assert_closed_trace(&first_rows)?;
    let second_capture = assert_closed_trace(&second_rows)?;
    assert_ne!(
        first_capture, second_capture,
        "process captures share an identity"
    );
    let first_export = export_trace_status(&first_trace, &first_state.path().join("first-export"))?;
    let second_export =
        export_trace_status(&second_trace, &second_state.path().join("second-export"))?;
    assert_eq!(json_str(&first_export, "capture_status")?, "complete");
    assert_eq!(json_str(&second_export, "capture_status")?, "complete");
    assert_eq!(
        json_str(&first_export, "capture_id")?,
        first_capture.as_str()
    );
    assert_eq!(
        json_str(&second_export, "capture_id")?,
        second_capture.as_str()
    );
    assert_ne!(
        json_str(&first_export, "clock_domain")?,
        json_str(&second_export, "clock_domain")?
    );
    assert_eq!(
        json_str(&first_export, "clock_mapping")?,
        "single_process_capture_origin_only"
    );
    assert_eq!(
        json_str(&second_export, "clock_mapping")?,
        "single_process_capture_origin_only"
    );
    let mixed = first_state.path().join("mixed-processes.jsonl");
    let mut mixed_bytes = std::fs::read(&first_trace)?;
    mixed_bytes.extend(std::fs::read(&second_trace)?);
    std::fs::write(&mixed, mixed_bytes)?;
    let mixed_export = export_trace_status(&mixed, &first_state.path().join("mixed-export"))?;
    assert_eq!(json_str(&mixed_export, "capture_status")?, "invalid");
    assert_eq!(json_field(&mixed_export, "viewer_available")?, false);
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "trace assertions are fixed goldens; process and I/O failures propagate"
)]
fn trace_timeline_process_kill_keeps_partial_without_cross_clock_join() -> TestResult {
    let state = private_tempdir()?;
    prepare_trace_state(state.path())?;
    let trace = state.path().join("killed-process.jsonl");
    let mut child = start_traced_process(state.path(), &trace)?;
    std::thread::sleep(Duration::from_millis(250));
    child.kill()?;
    let _status = child.wait()?;

    let rows = read_trace_prefix_rows(&trace)?;
    let header = rows
        .first()
        .ok_or("trace should contain a flushed header")?;
    assert_eq!(json_str(header, "record_type")?, "header");
    assert!(
        rows.iter().all(
            |row| row.get("record_type").and_then(serde_json::Value::as_str) != Some("footer")
        )
    );
    let capture = json_str(header, "capture_id")?;
    assert!(
        rows.iter()
            .all(|row| row.get("capture_id").and_then(serde_json::Value::as_str) == Some(capture))
    );
    let killed_export = export_trace_status(&trace, &state.path().join("killed-export"))?;
    assert_eq!(json_str(&killed_export, "capture_status")?, "partial");
    assert_eq!(json_str(&killed_export, "capture_id")?, capture);
    assert_eq!(json_str(&killed_export, "clock_domain")?, capture);
    assert_eq!(
        json_str(&killed_export, "clock_mapping")?,
        "single_process_capture_origin_only"
    );

    let other_state = private_tempdir()?;
    prepare_trace_state(other_state.path())?;
    let other_trace = other_state.path().join("other-process.jsonl");
    let mut other = start_traced_process(other_state.path(), &other_trace)?;
    std::thread::sleep(Duration::from_millis(150));
    stop_with_sigterm(&mut other)?;
    let other_export = export_trace_status(&other_trace, &other_state.path().join("other-export"))?;
    assert_eq!(json_str(&other_export, "capture_status")?, "complete");
    assert_ne!(
        json_str(&killed_export, "capture_id")?,
        json_str(&other_export, "capture_id")?
    );
    assert_ne!(
        json_str(&killed_export, "clock_domain")?,
        json_str(&other_export, "clock_domain")?
    );

    let other_rows = read_trace_rows(&other_trace)?;
    let foreign_event = other_rows
        .get(1)
        .ok_or("other process trace has no event")?;
    assert_eq!(json_str(foreign_event, "record_type")?, "event");
    assert_eq!(json_field(foreign_event, "output_seq")?, 1);
    assert_eq!(
        json_str(foreign_event, "capture_id")?,
        json_str(&other_export, "capture_id")?
    );
    let mixed = state.path().join("mixed-after-kill.jsonl");
    std::fs::write(&mixed, format!("{header}\n{foreign_event}\n"))?;
    let mixed_export = export_trace_status(&mixed, &state.path().join("mixed-export"))?;
    assert_eq!(json_str(&mixed_export, "capture_status")?, "invalid");
    assert_eq!(
        json_str(&mixed_export, "reason")?,
        "causal event capture ID changed"
    );
    assert_eq!(json_field(&mixed_export, "viewer_available")?, false);
    Ok(())
}

fn start_traced_process(state: &Path, trace: &Path) -> Result<Child, Box<dyn Error>> {
    let mut command = scale_process_command(state)?;
    let _configured = command.arg("--diagnostic-trace").arg(trace);
    let mut child = command.spawn()?;
    if let Err(error) = wait_for_sockets(state, &mut child) {
        let _cleanup = terminate_child(&mut child);
        return Err(error);
    }
    Ok(child)
}

fn prepare_trace_state(state: &Path) -> TestResult {
    let mut runtime = E2eRuntime::boot_in(state)?;
    runtime.start()?;
    runtime.stop()?;
    Ok(())
}

fn stop_with_sigterm(child: &mut Child) -> Result<(), Box<dyn Error>> {
    let pid = child.id().to_string();
    let status = Command::new("kill").args(["-TERM", &pid]).status()?;
    if !status.success() {
        return Err(format!("failed to send SIGTERM to searchd pid {pid}").into());
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("searchd pid {pid} did not stop cleanly: {status}").into());
    }
    Ok(())
}

fn read_trace_rows(path: &Path) -> Result<Vec<serde_json::Value>, Box<dyn Error>> {
    std::fs::read_to_string(path)?
        .lines()
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect()
}

fn read_trace_prefix_rows(path: &Path) -> Result<Vec<serde_json::Value>, Box<dyn Error>> {
    let raw = std::fs::read(path)?;
    let end = match raw.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => index.checked_add(1).ok_or("trace prefix length overflow")?,
        None => 0,
    };
    raw.get(..end)
        .ok_or("trace prefix exceeds input length")?
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).map_err(Into::into))
        .collect()
}

fn export_trace_status(
    trace: &Path,
    output_root: &Path,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("uv")
        .current_dir(repo_root)
        .args([
            "run",
            "--frozen",
            "--extra",
            "dev",
            "python",
            "-m",
            "tools.benchmark.retrieval.causal_cost_capture",
            "--offline-input",
        ])
        .arg(trace)
        .arg("--out-root")
        .arg(output_root)
        .output()?;
    let status: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output_root.join("timeline-status.json"))?)?;
    let viewer_available = json_field(&status, "viewer_available")?
        .as_bool()
        .ok_or("offline exporter status has non-boolean viewer_available")?;
    if output.status.success() != viewer_available {
        return Err(format!(
            "offline exporter exit/status disagree: exit={} status={} stderr={}",
            output.status,
            status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(status)
}

fn json_field<'a>(
    value: &'a serde_json::Value,
    key: &str,
) -> Result<&'a serde_json::Value, Box<dyn Error>> {
    value
        .get(key)
        .ok_or_else(|| format!("missing trace field {key}").into())
}

fn json_str<'a>(value: &'a serde_json::Value, key: &str) -> Result<&'a str, Box<dyn Error>> {
    json_field(value, key)?
        .as_str()
        .ok_or_else(|| format!("trace field {key} is not a string").into())
}

fn assert_closed_trace(rows: &[serde_json::Value]) -> Result<String, Box<dyn Error>> {
    let header = rows.first().ok_or("closed trace is empty")?;
    if header["record_type"] != "header" {
        return Err("closed trace does not begin with a header".into());
    }
    let capture = header["capture_id"]
        .as_str()
        .ok_or("trace header has no capture identity")?;
    if header["clock_domain"] != capture {
        return Err("trace header clock domain differs from its capture identity".into());
    }
    if rows
        .last()
        .is_none_or(|row| row["record_type"] != "footer" || row["status"] != "complete")
    {
        return Err("gracefully stopped process did not write a complete footer".into());
    }
    if !rows.iter().all(|row| row["capture_id"] == capture) {
        return Err("trace rows crossed process capture identities".into());
    }
    Ok(capture.to_string())
}

fn start_process(
    state: &std::path::Path,
    tier: ScaleTier,
) -> Result<SearchdBinaryProcess, Box<dyn Error>> {
    let mut command = scale_process_command(state)?;
    if tier == ScaleTier::Medium {
        return SearchdBinaryProcess::start_with_command(state, command);
    }
    let _configured = command
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
            SCALE_HISTORY_MAX_BYTES.to_string(),
        )
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
            SCALE_HISTORY_MAX_TOTAL_BYTES.to_string(),
        )
        .env("QUANTA_INDEX_INGEST_MAX_RECORDS", "100000")
        .env(
            "QUANTA_INDEX_INGEST_MAX_TEXT_BYTES",
            (128_u64 * 1024 * 1024).to_string(),
        )
        .env(
            "QUANTA_INDEX_INGEST_MAX_VECTOR_BYTES",
            (256_u64 * 1024 * 1024).to_string(),
        )
        .env(
            "QUANTA_INDEX_SOURCE_PUBLICATION_MAX_BYTES",
            quanta_index_contract::SOURCE_PUBLICATION_UPLOAD_MAX_BYTES.to_string(),
        )
        .env(
            "QUANTA_INDEX_QUERY_DISPATCH_BUDGET_MS",
            SUPPORTED_PROFILE_TIMEOUT.as_millis().to_string(),
        )
        .env(
            "QUANTA_INDEX_PROCESS_MEMORY_CEILING_BYTES",
            (4_u64 * 1024 * 1024 * 1024).to_string(),
        );
    SearchdBinaryProcess::start_with_command_and_timeout(state, command, SUPPORTED_PROFILE_TIMEOUT)
}

// A debug test driver can exercise a freshly built release daemon without
// rebuilding every test dependency under release's unwind profile. The caller
// owns fresh-build/source custody; pin the selected bytes before each launch.
fn scale_process_command(state: &Path) -> Result<Command, Box<dyn Error>> {
    let binary = std::env::var_os("QUANTA_INDEX_SCALE_RESTART_SEARCHD_BINARY");
    let digest = std::env::var_os("QUANTA_INDEX_SCALE_RESTART_SEARCHD_SHA256");
    let Some(binary) = checked_process_binary(binary.as_deref(), digest.as_deref())? else {
        return Ok(crate::searchd_binary_process::searchd_command(state, 2));
    };
    let mut command = Command::new(binary);
    let _configured = command.arg("serve").arg("--state-root").arg(state);
    crate::searchd_binary_process::apply_searchd_env(&mut command, 2);
    Ok(command)
}

fn checked_process_binary(
    binary: Option<&OsStr>,
    digest: Option<&OsStr>,
) -> Result<Option<PathBuf>, Box<dyn Error>> {
    let (binary, digest) = match (binary, digest) {
        (None, None) => return Ok(None),
        (Some(binary), Some(digest)) => (Path::new(binary), digest.to_str()),
        _ => return Err("scale restart binary requires both path and SHA256".into()),
    };
    let digest = digest.ok_or("scale restart binary SHA256 is not UTF-8")?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("scale restart binary requires lowercase SHA256".into());
    }
    let metadata = std::fs::symlink_metadata(binary)?;
    if !metadata.file_type().is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err("scale restart binary is not a regular executable".into());
    }
    if format!("{:x}", Sha256::digest(std::fs::read(binary)?)) != digest {
        return Err("scale restart binary differs from pinned SHA256".into());
    }
    Ok(Some(binary.canonicalize()?))
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "custody predicates are fixed test assertions; I/O setup errors propagate"
)]
fn scale_process_binary_selection_refuses_partial_custody_and_changed_bytes() -> TestResult {
    let root = private_tempdir()?;
    let binary = root.path().join("daemon");
    std::fs::write(&binary, b"abc")?;
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))?;
    let digest = OsStr::new("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(checked_process_binary(None, None)?, None);
    assert!(checked_process_binary(Some(binary.as_os_str()), None).is_err());
    assert!(checked_process_binary(None, Some(digest)).is_err());
    assert!(checked_process_binary(Some(binary.as_os_str()), Some(OsStr::new("bad"))).is_err());
    assert_eq!(
        checked_process_binary(Some(binary.as_os_str()), Some(digest))?,
        Some(binary.canonicalize()?)
    );
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&binary, &link)?;
    assert!(checked_process_binary(Some(link.as_os_str()), Some(digest)).is_err());
    std::fs::write(&binary, b"abd")?;
    assert!(checked_process_binary(Some(binary.as_os_str()), Some(digest)).is_err());
    Ok(())
}

fn connect_process(
    process: &SearchdBinaryProcess,
    state: &std::path::Path,
    tier: ScaleTier,
) -> Result<QuantaIndex, Box<dyn Error>> {
    if tier == ScaleTier::Medium {
        return process.connect();
    }
    Ok(QuantaIndex::connect(
        ConnectOptions::from_state_root(state).with_request_io_timeout(SUPPORTED_PROFILE_TIMEOUT),
    )?)
}

fn run_scale_process_restart(tier: ScaleTier) -> TestResult {
    let files = generate_scoped_corpus(tier, SEED)?;
    let truth = source_truth(&files, tier)?;
    let state = private_tempdir()?;
    let first = start_process(state.path(), tier)?;
    let client = connect_process(&first, state.path(), tier)?;
    let (_full_receipt, first_head) = client
        .search_corpus()
        .publish_and_activate(&initial_batch(files, tier)?, None)?;
    if first_head.active.generation.lexical.manifest_generation != ManifestGeneration::new(1) {
        return Err("full source generation did not activate".into());
    }
    wait_ready(&client, tier)?;
    let _retained_before_delete = unique_hit(
        &client,
        RETAINED_TOKEN,
        1,
        ("repo1", "src/file_0.rs"),
        &truth,
    )?;
    let _deleted_before_delete = unique_hit(
        &client,
        DELETED_TOKEN,
        1,
        ("repo0", "src/file_0.rs"),
        &truth,
    )?;
    let (_delta_receipt, second_head) = client
        .search_corpus()
        .publish_and_activate(&tombstone_batch()?, Some(first_head.active))?;
    if second_head.active.generation.lexical.manifest_generation != ManifestGeneration::new(2) {
        return Err("deleted source generation did not activate".into());
    }
    let before_restart = observe_generation_two(&client, &truth, tier)?;
    drop(client);
    first.stop()?;
    let reopened = start_process(state.path(), tier)?;
    let reopened_client = connect_process(&reopened, state.path(), tier)?;
    let after_restart = observe_generation_two(&reopened_client, &truth, tier)?;
    if before_restart != after_restart {
        return Err(
            "ranked identities, source commitments, or scores changed across process restart"
                .into(),
        );
    }
    drop(reopened_client);
    reopened.stop()?;
    Ok(())
}
