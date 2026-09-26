//! Adversarial mutation tests: every malformed or misleading evidence document
//! must be refused, and missing/unknown evidence must never become a pass.

#![expect(
    clippy::panic_in_result_fn,
    reason = "contract tests use Result-returning setup with assertion-style validation"
)]
use std::error::Error;
use std::fs;

use quanta_index_bench_protocol::sample::{sample_evidence, sample_raw_bytes, sample_sealed};
use quanta_index_bench_protocol::{BenchmarkEvidenceV1, Payload, ProtocolError, RunStore};

const RUN_ID: &str = "run-20260926T120000Z-a1b2c3d4";

#[test]
fn profile_custody_refuses_rust_gc_without_deleting_runs() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("captures"))?;
    fs::create_dir_all(root.path().join("runs/pinned"))?;
    let store = RunStore::new(root.path().canonicalize()?);
    assert!(store.collect(&[]).is_err());
    assert!(root.path().join("runs/pinned").is_dir());
    Ok(())
}

#[cfg(unix)]
#[test]
fn profile_marker_is_checked_after_acquiring_publication_custody() -> Result<(), Box<dyn Error>> {
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    let root = tempfile::tempdir()?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(root.path().join(".custody.lock"))?;
    lock.lock()?;
    fs::create_dir_all(root.path().join("runs/pinned"))?;
    let store = RunStore::new(root.path().canonicalize()?);
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx
            .send(())
            .map_err(|error| format!("start receiver dropped: {error}"))?;
        finished_tx
            .send(store.collect(&[]))
            .map_err(|error| format!("result receiver dropped: {error}"))
    });
    started_rx.recv_timeout(Duration::from_secs(5))?;
    let before_marker = finished_rx.recv_timeout(Duration::from_millis(100));
    fs::create_dir_all(root.path().join("captures"))?;
    drop(lock);
    assert!(
        matches!(before_marker, Err(RecvTimeoutError::Timeout)),
        "{before_marker:?}"
    );
    assert!(finished_rx.recv_timeout(Duration::from_secs(5))?.is_err());
    worker
        .join()
        .map_err(|error| format!("collector thread panicked: {error:?}"))??;
    assert!(root.path().join("runs/pinned").is_dir());
    Ok(())
}

#[cfg(unix)]
#[test]
fn collector_refuses_linked_lock_and_root_before_deleting_runs() -> Result<(), Box<dyn Error>> {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir()?;
    fs::create_dir_all(root.path().join("runs/pinned"))?;
    let store = RunStore::new(root.path().canonicalize()?);
    let outside = tempfile::tempdir()?;
    let outside_file = outside.path().join("lock");
    fs::write(&outside_file, b"foreign")?;
    let lock = root.path().join(".custody.lock");
    symlink(&outside_file, &lock)?;
    assert!(store.collect(&[]).is_err());
    fs::remove_file(&lock)?;
    fs::hard_link(&outside_file, &lock)?;
    assert!(store.collect(&[]).is_err());
    fs::remove_file(&lock)?;
    let root_link = outside.path().join("root-link");
    symlink(root.path(), &root_link)?;
    assert!(RunStore::new(&root_link).collect(&[]).is_err());
    assert_eq!(fs::read(&outside_file)?, b"foreign");
    assert!(root.path().join("runs/pinned").is_dir());
    Ok(())
}

#[cfg(unix)]
#[test]
fn rust_collector_obeys_python_flock_custody() -> Result<(), Box<dyn Error>> {
    use std::io::{BufRead as _, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    // The test owns and reaps this one child even when a handshake fails.
    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _kill_result = self.0.kill();
            let _wait_result = self.0.wait();
        }
    }

    let root = tempfile::tempdir()?;
    let path = root.path().canonicalize()?;
    fs::create_dir_all(path.join("runs/pinned"))?;
    let mut child = OwnedChild(Command::new("python3")
        .args(["-I", "-c", "import fcntl,sys; from pathlib import Path; f=(Path(sys.argv[1])/'.custody.lock').open('a+b'); fcntl.flock(f,fcntl.LOCK_EX); print('locked',flush=True); sys.stdin.buffer.read()"])
        .arg(&path)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit())
        .spawn()?);
    let stdout = child.0.stdout.take().ok_or("Python stdout pipe missing")?;
    let (ready_tx, ready_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout)
            .read_line(&mut line)
            .map(|_count| line);
        let _send_result = ready_tx.send(result);
    });
    assert_eq!(ready_rx.recv_timeout(Duration::from_secs(5))??, "locked\n");
    reader
        .join()
        .map_err(|error| format!("Python handshake panicked: {error:?}"))?;
    let store = RunStore::new(&path);
    let (started_tx, started_rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        started_tx
            .send(())
            .map_err(|error| format!("start receiver dropped: {error}"))?;
        finished_tx
            .send(store.collect(&[]))
            .map_err(|error| format!("result receiver dropped: {error}"))
    });
    started_rx.recv_timeout(Duration::from_secs(5))?;
    let before_marker = finished_rx.recv_timeout(Duration::from_millis(100));
    fs::create_dir_all(path.join("captures"))?;
    drop(child);
    assert!(
        matches!(before_marker, Err(RecvTimeoutError::Timeout)),
        "{before_marker:?}"
    );
    assert!(finished_rx.recv_timeout(Duration::from_secs(5))?.is_err());
    worker
        .join()
        .map_err(|error| format!("collector panicked: {error:?}"))??;
    assert!(path.join("runs/pinned").is_dir());
    Ok(())
}

fn refusal(
    result: Result<BenchmarkEvidenceV1, ProtocolError>,
) -> Result<ProtocolError, Box<dyn Error>> {
    match result {
        Ok(_) => Err("expected a refusal, got a valid document".into()),
        Err(error) => Ok(error),
    }
}

fn sealed_text() -> Result<String, Box<dyn Error>> {
    Ok(sample_sealed()?.to_canonical_json()?)
}

// --------------------------------------------------------------------------
// Document-level mutations
// --------------------------------------------------------------------------

#[test]
fn duplicate_json_key_is_refused() -> Result<(), Box<dyn Error>> {
    let text = sealed_text()?;
    let mutated = text.replacen('{', "{\"family\":\"duplicated\",", 1);
    let error = refusal(BenchmarkEvidenceV1::open(&mutated))?;
    assert!(matches!(error, ProtocolError::Json(_)), "got {error:?}");
    Ok(())
}

#[test]
fn unknown_field_is_refused() -> Result<(), Box<dyn Error>> {
    let text = sealed_text()?;
    let mutated = text.replacen('{', "{\"bogus_field\":1,", 1);
    let error = refusal(BenchmarkEvidenceV1::open(&mutated))?;
    // The manual decoder rejects an unknown field by exact key-set comparison.
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn malformed_digest_is_refused() -> Result<(), Box<dyn Error>> {
    let sealed = sample_sealed()?;
    let declared = sealed.digest.clone().unwrap_or_default();
    let text = sealed.to_canonical_json()?;
    let mutated = text.replace(&declared, "sha256:zz");
    let error = refusal(BenchmarkEvidenceV1::open(&mutated))?;
    assert!(
        matches!(error, ProtocolError::InvalidDigest { .. }),
        "got {error:?}"
    );
    Ok(())
}

#[test]
fn tampered_document_digest_is_refused() -> Result<(), Box<dyn Error>> {
    let text = sealed_text()?;
    let mutated = text.replace("\"wall_ms\":12345", "\"wall_ms\":12346");
    assert_ne!(mutated, text, "tamper target was not present");
    let error = refusal(BenchmarkEvidenceV1::open(&mutated))?;
    assert!(
        matches!(error, ProtocolError::DigestMismatch { .. }),
        "got {error:?}"
    );
    Ok(())
}

#[test]
fn unsupported_protocol_version_is_refused() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.protocol_version = 2;
    let error = refusal(evidence.seal())?;
    assert!(
        matches!(error, ProtocolError::UnsupportedVersion(2)),
        "got {error:?}"
    );
    Ok(())
}

#[test]
fn unknown_protocol_name_is_refused() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.protocol = "SomethingElse".to_owned();
    let error = refusal(evidence.seal())?;
    assert!(
        matches!(error, ProtocolError::UnsupportedProtocol { .. }),
        "got {error:?}"
    );
    Ok(())
}

// --------------------------------------------------------------------------
// Identity mutations
// --------------------------------------------------------------------------

#[test]
fn dirty_source_without_digest_is_refused() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.source.dirty = true;
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn clean_source_with_dirty_digest_is_refused() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.source.dirty_paths_digest = Some("sha256:00".to_owned());
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn canonical_linux_policy_on_a_non_linux_host_is_refused() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.host.policy = "canonical-linux".to_owned();
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn performance_scope_without_an_exclusive_lease_is_refused() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.verdict.scope = "performance".to_owned();
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn unavailable_input_without_reason_is_refused() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    if let Some(input) = evidence.inputs.first_mut() {
        input.availability = "unavailable".to_owned();
        input.digest = None;
        input.reason = None;
    }
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn present_input_without_digest_is_refused() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    if let Some(input) = evidence.inputs.first_mut() {
        input.digest = None;
    }
    let error = refusal(evidence.seal())?;
    assert!(
        matches!(error, ProtocolError::InvalidDigest { .. }),
        "got {error:?}"
    );
    Ok(())
}

#[test]
fn invalid_run_ids_are_refused() -> Result<(), Box<dyn Error>> {
    for run_id in ["latest", "../escape", "nested/id", "", ".hidden"] {
        let mut evidence = sample_evidence()?;
        evidence.run_id = run_id.to_owned();
        let error = refusal(evidence.seal())?;
        assert!(
            matches!(error, ProtocolError::InvalidRunId(_)),
            "run id {run_id:?} produced {error:?}"
        );
    }
    Ok(())
}

// --------------------------------------------------------------------------
// Command / verdict mutations
// --------------------------------------------------------------------------

#[test]
fn timed_out_producer_cannot_carry_a_pass() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.command.status = "timeout".to_owned();
    evidence.command.exit_code = None;
    let error = refusal(evidence.seal())?;
    assert!(
        matches!(error, ProtocolError::InadmissibleCommand(_)),
        "got {error:?}"
    );
    Ok(())
}

#[test]
fn partial_result_cannot_carry_a_pass() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.command.status = "interrupted".to_owned();
    evidence.command.exit_code = None;
    let error = refusal(evidence.seal())?;
    assert!(
        matches!(error, ProtocolError::InadmissibleCommand(_)),
        "got {error:?}"
    );
    Ok(())
}

#[test]
fn non_pass_verdict_requires_a_reason() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.verdict.status = "not_run".to_owned();
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn completed_command_must_exit_zero() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.command.exit_code = Some(1);
    let error = refusal(evidence.seal())?;
    assert!(
        matches!(error, ProtocolError::InadmissibleCommand(_)),
        "got {error:?}"
    );
    Ok(())
}

// --------------------------------------------------------------------------
// Payload confusion
// --------------------------------------------------------------------------

#[test]
fn instruction_count_cannot_be_reported_as_wall_latency() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.payload = Payload::Micro(quanta_index_bench_protocol::MicroPayload {
        bench_id: "lq-norm/pipeline".to_owned(),
        metric: "instructions".to_owned(),
        unit: "ms".to_owned(),
        instrumentation: "instructions".to_owned(),
        statistic: "mean".to_owned(),
        value: 12.0,
        iterations: 1,
        samples: 1,
    });
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn file_only_labels_cannot_become_span_judgments() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.payload = Payload::Retrieval(quanta_index_bench_protocol::RetrievalPayload {
        lane: "native_default".to_owned(),
        metric_space: "span".to_owned(),
        judgments: "mechanically_labeled".to_owned(),
        unjudged: 0,
        rows: vec![quanta_index_bench_protocol::RetrievalRow {
            query_id: "q1".to_owned(),
            metric: "recall@20".to_owned(),
            unit: "ratio".to_owned(),
            value: Some(0.5),
            state: "judged".to_owned(),
        }],
        universe_attested: true,
        corpus_digest: "sha256:".to_owned() + &"ab".repeat(32),
        query_pack_digest: "sha256:".to_owned() + &"cd".repeat(32),
    });
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn unjudged_retrieval_rows_cannot_carry_a_score() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.payload = Payload::Retrieval(quanta_index_bench_protocol::RetrievalPayload {
        lane: "native_default".to_owned(),
        metric_space: "file".to_owned(),
        judgments: "pooled".to_owned(),
        unjudged: 1,
        rows: vec![quanta_index_bench_protocol::RetrievalRow {
            query_id: "q1".to_owned(),
            metric: "recall@20".to_owned(),
            unit: "ratio".to_owned(),
            value: Some(0.0),
            state: "unjudged".to_owned(),
        }],
        universe_attested: false,
        corpus_digest: "sha256:".to_owned() + &"ab".repeat(32),
        query_pack_digest: "sha256:".to_owned() + &"cd".repeat(32),
    });
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn closed_loop_throughput_cannot_claim_an_offered_rate() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.payload = Payload::Load(quanta_index_bench_protocol::LoadPayload {
        arrival: "closed_loop".to_owned(),
        generator_saturated: false,
        points: vec![quanta_index_bench_protocol::LoadPoint {
            label: "clients-8".to_owned(),
            offered_rate: Some(200.0),
            completed_rate: 180.0,
            dropped: 0,
            timeouts: 0,
        }],
        errors: 0,
    });
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn open_loop_points_require_an_offered_rate() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.payload = Payload::Load(quanta_index_bench_protocol::LoadPayload {
        arrival: "open_loop".to_owned(),
        generator_saturated: false,
        points: vec![quanta_index_bench_protocol::LoadPoint {
            label: "rate-200".to_owned(),
            offered_rate: None,
            completed_rate: 180.0,
            dropped: 20,
            timeouts: 0,
        }],
        errors: 0,
    });
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn unmeasured_latency_rows_cannot_carry_percentiles() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    if let Payload::Latency(latency) = &mut evidence.payload
        && let Some(row) = latency.rows.first_mut()
    {
        row.early_stop_reason = Some("fixture-gap".to_owned());
    }
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn measured_latency_rows_require_samples() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    if let Payload::Latency(latency) = &mut evidence.payload
        && let Some(row) = latency.rows.first_mut()
    {
        row.samples = 0;
    }
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn duplicated_case_ids_are_refused() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    if let Payload::Latency(latency) = &mut evidence.payload
        && let Some(row) = latency.rows.first().cloned()
    {
        latency.rows.push(row);
    }
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn agent_outcome_requires_exactly_three_arms() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.payload = Payload::AgentOutcome(quanta_index_bench_protocol::AgentOutcomePayload {
        task_count: 1,
        pair_count: 1,
        arms: vec!["A".to_owned(), "B".to_owned()],
        excluded_pairs: 0,
        unknown_pairs: 0,
        metrics: Vec::new(),
        capture: "recorded_unauthenticated".to_owned(),
        input_digest: "sha256:".to_owned() + &"ef".repeat(32),
    });
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn recorded_experiment_cannot_claim_qualification() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.payload =
        Payload::RecordedExperiment(quanta_index_bench_protocol::RecordedExperimentPayload {
            experiment_id: "scan-vs-index".to_owned(),
            diagnostic_only: false,
            points: vec![quanta_index_bench_protocol::ExperimentPoint {
                label: "2000".to_owned(),
                metric: "scan_ms".to_owned(),
                unit: "ms".to_owned(),
                value: 1.0,
            }],
            source_digest: "sha256:".to_owned() + &"ef".repeat(32),
        });
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

// --------------------------------------------------------------------------
// Raw reference mutations
// --------------------------------------------------------------------------

#[test]
fn raw_path_escape_is_refused() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    if let Some(reference) = evidence.raw.first_mut() {
        reference.path = "../escape.json".to_owned();
    }
    let error = refusal(evidence.seal())?;
    assert!(
        matches!(error, ProtocolError::PathEscape(_)),
        "got {error:?}"
    );
    Ok(())
}

#[test]
fn duplicate_raw_paths_are_refused() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    if let Some(reference) = evidence.raw.first().cloned() {
        evidence.raw.push(reference);
    }
    let error = refusal(evidence.seal())?;
    assert!(
        matches!(error, ProtocolError::DuplicateRaw(_)),
        "got {error:?}"
    );
    Ok(())
}

#[test]
fn evidence_without_raw_artifacts_is_refused() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_evidence()?;
    evidence.raw.clear();
    let error = refusal(evidence.seal())?;
    assert!(matches!(error, ProtocolError::Semantic(_)), "got {error:?}");
    Ok(())
}

#[test]
fn missing_raw_file_blocks_promotion() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let store = RunStore::new(root.path());
    let sealed = sample_sealed()?;
    let staged = store.stage(RUN_ID)?;
    staged.write_evidence(&sealed)?;
    let Err(error) = store.promote(staged) else {
        return Err("promotion accepted a missing raw file".into());
    };
    assert!(
        matches!(error, ProtocolError::MissingRaw(_)),
        "got {error:?}"
    );
    assert!(store.load(RUN_ID).is_err());
    Ok(())
}

#[test]
fn extra_raw_file_blocks_promotion() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let store = RunStore::new(root.path());
    let sealed = sample_sealed()?;
    let staged = store.stage(RUN_ID)?;
    let _declared = staged.write_raw("raw/warm-matrix.json", &sample_raw_bytes())?;
    let _undeclared = staged.write_raw("raw/undeclared.json", b"surprise")?;
    staged.write_evidence(&sealed)?;
    let Err(error) = store.promote(staged) else {
        return Err("promotion accepted an undeclared raw file".into());
    };
    assert!(matches!(error, ProtocolError::ExtraRaw(_)), "got {error:?}");
    Ok(())
}

#[test]
fn tampered_raw_bytes_block_promotion() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let store = RunStore::new(root.path());
    let sealed = sample_sealed()?;
    let staged = store.stage(RUN_ID)?;
    let _reference = staged.write_raw(
        "raw/warm-matrix.json",
        b"{\"sample\":\"warm-matrix\",\"p50_ms\":0.43}\n",
    )?;
    staged.write_evidence(&sealed)?;
    let Err(error) = store.promote(staged) else {
        return Err("promotion accepted tampered raw bytes".into());
    };
    assert!(
        matches!(error, ProtocolError::RawDigestMismatch { .. }),
        "got {error:?}"
    );
    assert!(store.load(RUN_ID).is_err());
    Ok(())
}

#[test]
fn length_mismatch_blocks_promotion() -> Result<(), Box<dyn Error>> {
    let mut evidence = sample_sealed()?;
    if let Some(reference) = evidence.raw.first_mut() {
        reference.bytes = reference.bytes.saturating_add(1);
    }
    let evidence = evidence.seal()?;
    let root = tempfile::tempdir()?;
    let store = RunStore::new(root.path());
    let staged = store.stage(RUN_ID)?;
    let _reference = staged.write_raw("raw/warm-matrix.json", &sample_raw_bytes())?;
    staged.write_evidence(&evidence)?;
    let Err(error) = store.promote(staged) else {
        return Err("promotion accepted a length mismatch".into());
    };
    assert!(
        matches!(error, ProtocolError::RawLengthMismatch { .. }),
        "got {error:?}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn symlinked_raw_reference_is_refused() -> Result<(), Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let store = RunStore::new(root.path());
    let sealed = sample_sealed()?;
    let staged = store.stage(RUN_ID)?;
    let raw_dir = staged.path().join("raw");
    fs::create_dir_all(&raw_dir)?;
    let target = root.path().join("outside.json");
    fs::write(&target, sample_raw_bytes())?;
    std::os::unix::fs::symlink(&target, raw_dir.join("warm-matrix.json"))?;
    staged.write_evidence(&sealed)?;
    let Err(error) = store.promote(staged) else {
        return Err("promotion accepted a symlinked raw file".into());
    };
    assert!(
        matches!(error, ProtocolError::SymlinkRefused(_)),
        "got {error:?}"
    );
    Ok(())
}

#[test]
fn reordered_raw_samples_change_the_digest() -> Result<(), Box<dyn Error>> {
    let mut reordered = sample_raw_bytes();
    reordered.reverse();
    assert_ne!(reordered, sample_raw_bytes());
    let root = tempfile::tempdir()?;
    let store = RunStore::new(root.path());
    let sealed = sample_sealed()?;
    let staged = store.stage(RUN_ID)?;
    let _reference = staged.write_raw("raw/warm-matrix.json", &reordered)?;
    staged.write_evidence(&sealed)?;
    let Err(error) = store.promote(staged) else {
        return Err("promotion accepted reordered raw bytes".into());
    };
    assert!(
        matches!(error, ProtocolError::RawDigestMismatch { .. }),
        "got {error:?}"
    );
    Ok(())
}

// --------------------------------------------------------------------------
// Comparability
// --------------------------------------------------------------------------

#[test]
fn incomparable_baselines_are_refused() -> Result<(), Box<dyn Error>> {
    let candidate = sample_sealed()?;

    let mut other_host = sample_evidence()?;
    other_host.host.identity_digest = "sha256:".to_owned() + &"11".repeat(32);
    let other_host = other_host.seal()?;
    assert!(candidate.require_comparable(&other_host).is_err());

    let mut other_input = sample_evidence()?;
    if let Some(input) = other_input.inputs.first_mut() {
        input.digest = Some("sha256:".to_owned() + &"22".repeat(32));
    }
    let other_input = other_input.seal()?;
    assert!(candidate.require_comparable(&other_input).is_err());

    let mut other_profile = sample_evidence()?;
    other_profile.build.profile = "release".to_owned();
    let other_profile = other_profile.seal()?;
    assert!(candidate.require_comparable(&other_profile).is_err());

    let comparable = sample_sealed()?;
    assert!(candidate.require_comparable(&comparable).is_ok());
    Ok(())
}
