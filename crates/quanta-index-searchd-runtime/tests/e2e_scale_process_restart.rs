//! Backup/restore and process trace regressions for the Legacy daemon.

use std::error::Error;
use std::path::Path;
use std::process::{Child, Command};
use std::time::Duration;

use quanta_index_contract::{GenerationPin, ManifestGeneration, RepoId, RevisionId};
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, publish_legacy_source, terminate_child, wait_for_sockets,
};
use quanta_index_searchd_harness::{private_tempdir, source_fixture::ScaleTier};
use sha2::{Digest, Sha256};

use crate::scale_process_fixture::{scale_process_binary, scale_process_command, start_process};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn restored_shared_object_corpus_serves_in_two_fresh_daemon_processes() -> TestResult {
    let source = private_tempdir()?;
    let destination = private_tempdir()?;
    let path = "src/restored.rs";
    let body = "pub fn restored_process_needle_73() {}";
    let expected_id = format!("e2e-1-{path}");
    let binary = scale_process_binary()?;
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let producer = binary.start(source.path(), 2)?;
    let producer_client = producer.connect()?;
    let (_head, receipt) = publish_legacy_source(
        &producer_client,
        &repo,
        &revision,
        1,
        None,
        &[
            (path, body, 1),
            ("src/decoy.rs", "pub fn unrelated() {}\n", 2),
        ],
        None,
    )?;
    if receipt.accepted_replace_scopes != 2 || receipt.accepted_semantic_replace_scopes != 2 {
        return Err(
            format!("restored source fixture did not publish both files: {receipt:?}").into(),
        );
    }
    let pin = GenerationPin::new(repo, revision, ManifestGeneration::new(1));
    drop(producer_client);
    producer.stop()?;
    binary.verify()?;
    let incarnation = "activations/.activation-root-incarnation-v1";
    let source_incarnation = std::fs::read(source.path().join(incarnation))?;
    let command = scale_process_command(source.path(), &binary)?;
    let backup = destination.path().join("backup");
    let restored = destination.path().join("restored");
    for (operation, from, to) in [
        ("backup-state", source.path(), backup.as_path()),
        ("restore-state", backup.as_path(), restored.as_path()),
    ] {
        binary.verify()?;
        let mut migration = binary.command()?;
        // Offline commands require the same explicit policy as the source daemon.
        for (name, value) in command.get_envs() {
            if let Some(value) = value {
                let _configured = migration.env(name, value);
            } else {
                let _configured = migration.env_remove(name);
            }
        }
        let output = migration
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
        binary.verify()?;
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
        let daemon = start_process(&restored, ScaleTier::Medium, &binary)?;
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
    binary.verify()
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "trace assertions are fixed goldens; process and I/O failures propagate"
)]
fn trace_timeline_process_close_keeps_clock_domains_separate() -> TestResult {
    let binary = scale_process_binary()?;
    let first_state = private_tempdir()?;
    let second_state = private_tempdir()?;
    prepare_trace_state(first_state.path(), &binary)?;
    prepare_trace_state(second_state.path(), &binary)?;
    let first_trace = first_state.path().join("first-process.jsonl");
    let second_trace = second_state.path().join("second-process.jsonl");

    let mut first = start_traced_process(first_state.path(), &first_trace, &binary)?;
    std::thread::sleep(Duration::from_millis(150));
    stop_with_sigterm(&mut first)?;
    let mut second = start_traced_process(second_state.path(), &second_trace, &binary)?;
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
    binary.verify()
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "trace assertions are fixed goldens; process and I/O failures propagate"
)]
fn trace_timeline_process_kill_keeps_partial_without_cross_clock_join() -> TestResult {
    let binary = scale_process_binary()?;
    let state = private_tempdir()?;
    prepare_trace_state(state.path(), &binary)?;
    let trace = state.path().join("killed-process.jsonl");
    let mut child = start_traced_process(state.path(), &trace, &binary)?;
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
    prepare_trace_state(other_state.path(), &binary)?;
    let other_trace = other_state.path().join("other-process.jsonl");
    let mut other = start_traced_process(other_state.path(), &other_trace, &binary)?;
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
    let killed_wire = std::fs::read_to_string(&trace)?;
    let other_wire = std::fs::read_to_string(&other_trace)?;
    let raw_header = killed_wire
        .lines()
        .next()
        .ok_or("trace wire header absent")?;
    let raw_foreign_event = other_wire
        .lines()
        .nth(1)
        .ok_or("foreign wire event absent")?;
    let mixed = state.path().join("mixed-after-kill.jsonl");
    std::fs::write(&mixed, format!("{raw_header}\n{raw_foreign_event}\n"))?;
    let mixed_export_root = state.path().join("mixed-export");
    let mixed_export = export_trace_status(&mixed, &mixed_export_root)?;
    assert_eq!(json_str(&mixed_export, "capture_status")?, "invalid");
    assert_eq!(
        json_str(&mixed_export, "reason")?,
        format!(
            "unsafe benchmark evidence {}: causal event capture ID changed",
            mixed_export_root
                .canonicalize()?
                .join("causal-timeline.jsonl")
                .display()
        )
    );
    assert_eq!(json_field(&mixed_export, "viewer_available")?, false);
    binary.verify()
}

fn start_traced_process(
    state: &Path,
    trace: &Path,
    binary: &LegacySearchdBinary,
) -> Result<Child, Box<dyn Error>> {
    let mut command = scale_process_command(state, binary)?;
    let _configured = command.arg("--diagnostic-trace").arg(trace);
    let mut child = command.spawn()?;
    let ready = wait_for_sockets(state, &mut child).and_then(|()| {
        binary.verify()?;
        Ok(())
    });
    if let Err(error) = ready {
        let _cleanup = terminate_child(&mut child);
        return Err(error);
    }
    Ok(child)
}

fn prepare_trace_state(state: &Path, binary: &LegacySearchdBinary) -> TestResult {
    let daemon = binary.start(state, 2)?;
    daemon.stop()?;
    binary.verify()
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
    let rows = std::fs::read_to_string(path)?
        .lines()
        .map(|line| serde_json::from_str(line).map_err(Into::into))
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    normalize_trace_rows(rows)
}

fn read_trace_prefix_rows(path: &Path) -> Result<Vec<serde_json::Value>, Box<dyn Error>> {
    let raw = std::fs::read(path)?;
    let end = match raw.iter().rposition(|byte| *byte == b'\n') {
        Some(index) => index.checked_add(1).ok_or("trace prefix length overflow")?,
        None => 0,
    };
    let rows = raw
        .get(..end)
        .ok_or("trace prefix exceeds input length")?
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).map_err(Into::into))
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    normalize_trace_rows(rows)
}

fn normalize_trace_rows(
    rows: Vec<serde_json::Value>,
) -> Result<Vec<serde_json::Value>, Box<dyn Error>> {
    let schema_version = rows
        .first()
        .and_then(|row| row.get("schema_version"))
        .and_then(serde_json::Value::as_u64)
        .ok_or("trace schema version absent")?;
    rows.into_iter()
        .map(|row| {
            quanta_index_searchd::trace_wire::normalize_record(schema_version, row)
                .map_err(Into::into)
        })
        .collect()
}

fn export_trace_status(
    trace: &Path,
    output_root: &Path,
) -> Result<serde_json::Value, Box<dyn Error>> {
    // Resolve the macOS /tmp directory alias while preserving each leaf for
    // the exporter's own symlink and exclusive-output checks.
    let trace = trace
        .parent()
        .ok_or("trace input has no parent")?
        .canonicalize()?
        .join(trace.file_name().ok_or("trace input has no filename")?);
    let output_root = output_root
        .parent()
        .ok_or("trace output has no parent")?
        .canonicalize()?
        .join(
            output_root
                .file_name()
                .ok_or("trace output has no filename")?,
        );
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
        .arg(&trace)
        .arg("--out-root")
        .arg(&output_root)
        .output()?;
    let status_path = output_root.join("timeline-status.json");
    let status_bytes = std::fs::read(&status_path).map_err(|error| {
        format!(
            "offline exporter did not provide {}: {error}; exit={} stderr={}",
            status_path.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
    })?;
    let status: serde_json::Value = serde_json::from_slice(&status_bytes)?;
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
