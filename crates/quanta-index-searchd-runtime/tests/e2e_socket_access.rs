//! Linux real-UID access checks against a pinned Legacy daemon.

#![cfg(target_os = "linux")]
#![forbid(unsafe_code)]

use std::error::Error;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::process_envelope_scrape::expect_eq;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn mode_of(path: &Path) -> TestResult<u32> {
    Ok(std::fs::symlink_metadata(path)?.mode() & 0o7777)
}

/// The server runs as the fixture owner and the helper as a distinct kernel UID.
#[test]
#[ignore = "requires an explicit Linux root container with setpriv and SETUID/SETGID"]
fn linux_two_real_uids_enforce_operator_events_and_serve_listed_query() -> TestResult {
    use quanta_index_contract::{
        ProcessRequestEventPlaneV1, ProcessRequestEventStageV1, RepoId, RevisionId,
    };
    use quanta_index_searchd_harness::legacy_reference::{
        LegacySearchdBinary, SearchdBinaryProcess, daemon_socket_paths, publish_legacy_source,
    };

    use crate::fail_closed_wait::{RealTicker, WaitError, wait_for};

    if rustix::process::geteuid().as_raw() != 0 {
        return Err("Linux real-UID proof requires a root runner; never skip it".into());
    }
    let other_uid = 65_534_u32;
    let root = tempfile::tempdir_in("/tmp")?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o711))?;
    let binary = LegacySearchdBinary::from_environment()?;
    let mut command = binary.benchmark_command(root.path(), 8)?;
    let uid_policy = format!("shared:uid={other_uid}");
    let _configured = command
        .env("QUANTA_INDEX_QUERY_SOCKET_ACCESS", &uid_policy)
        .env("QUANTA_INDEX_CONTROL_SOCKET_ACCESS", &uid_policy);
    let daemon = SearchdBinaryProcess::start_with_pinned_command(root.path(), command)?;
    let outcome = (|| -> TestResult {
        let client = daemon.connect()?;
        let (_active, _receipt) = publish_legacy_source(
            &client,
            &RepoId::new("repo-e2e")?,
            &RevisionId::new("rev-e2e")?,
            1,
            None,
            &[("src/linux_uid.rs", "needle real uid", 1)],
            None,
        )?;
        let [query, control, ingest] = daemon_socket_paths(root.path());
        expect_eq("listed query socket mode", &mode_of(&query)?, &0o666)?;
        expect_eq("listed control socket mode", &mode_of(&control)?, &0o666)?;
        expect_eq("private ingest socket mode", &mode_of(&ingest)?, &0o600)?;
        let directory = root.path().join("search-plane");
        expect_eq(
            "listed socket directory mode",
            &mode_of(&directory)?,
            &0o711,
        )?;
        let read_events = || {
            client
                .observability()
                .request_events(ProcessRequestEventPlaneV1::Query, 1024)
        };
        // An owner can read the bounded ring. The outsider's Admin request
        // must return only a typed refusal, leaving this query window intact.
        let before = read_events()?;
        run_real_uid_probe(other_uid, "deny-events", &query, &control, root.path())?;
        let after_denial = read_events()?;
        expect_eq(
            "process instance",
            &after_denial.process_instance,
            &before.process_instance,
        )?;
        expect_eq(
            "denied read did not advance query ring",
            &after_denial.next_sequence,
            &before.next_sequence,
        )?;
        expect_eq(
            "denied Admin request did not disclose or change query ring",
            &after_denial,
            &before,
        )?;
        run_real_uid_probe(other_uid, "query", &query, &control, root.path())?;
        // A client may receive its response before the server records the
        // terminal event. Await that observable completion, not a fixed delay.
        let after_query = wait_for(
            &RealTicker::new(),
            Duration::from_secs(5),
            Duration::from_millis(10),
            "listed UID query terminal event",
            read_events,
            |window| {
                window.events.iter().any(|event| {
                    event.request_id.get() == 0xe306_u64
                        && event.stage == ProcessRequestEventStageV1::ResponseWritten
                })
            },
            |_| false,
        )
        .map_err(|error| -> Box<dyn std::error::Error> {
            match error {
                WaitError::Timeout(timeout) => timeout.into(),
                WaitError::Terminal(error) => error.into(),
            }
        })?;
        expect_eq(
            "listed UID query process instance",
            &after_query.process_instance,
            &before.process_instance,
        )?;
        Ok(())
    })();
    let stopped = daemon.stop().and_then(|()| binary.verify());
    match (outcome, stopped) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(probe), Err(cleanup)) => {
            Err(format!("real-UID probe failed: {probe}; daemon cleanup failed: {cleanup}").into())
        }
    }
}

/// The same test binary is copied outside any 0700 Cargo checkout so a
/// deprivileged OS process can execute the real SDK/IPC client code.
fn run_real_uid_probe(
    uid: u32,
    mode: &str,
    query: &Path,
    control: &Path,
    root: &Path,
) -> TestResult {
    let executable = std::env::current_exe()?;
    let directory = tempfile::tempdir_in("/tmp")?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755))?;
    let copy = directory.path().join("uid-probe-test-binary");
    let _copied = std::fs::copy(executable, &copy)?;
    std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o755))?;
    let stdout_path = directory.path().join("stdout");
    let stderr_path = directory.path().join("stderr");
    let stdout = std::fs::File::create(&stdout_path)?;
    let stderr = std::fs::File::create(&stderr_path)?;
    let mut child = Command::new("setpriv")
        .arg(format!("--reuid={uid}"))
        .arg(format!("--regid={uid}"))
        .arg("--clear-groups")
        .arg("--")
        .arg(copy)
        .arg("--ignored")
        .arg("--exact")
        .arg("e2e_socket_access::linux_real_uid_client_helper")
        .arg("--nocapture")
        .env("QI_E3_REAL_UID_MODE", mode)
        .env("QI_E3_REAL_UID_EXPECTED", uid.to_string())
        .env("QI_E3_REAL_UID_QUERY_SOCKET", query)
        .env("QI_E3_REAL_UID_CONTROL_SOCKET", control)
        .env("QI_E3_REAL_UID_STATE_ROOT", root)
        .current_dir("/")
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()?;
    let start = Instant::now();
    let deadline = Duration::from_secs(45);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _killed = child.kill();
                let reaped = child.wait();
                return Err(format!(
                    "real-UID {mode} child wait failed: {error}; reap: {reaped:?}"
                )
                .into());
            }
        }
        if start.elapsed() >= deadline {
            let _killed = child.kill();
            let reaped = child.wait();
            let stderr = std::fs::read_to_string(&stderr_path)?;
            return Err(format!(
                "real-UID {mode} child timed out; reap: {reaped:?}; stderr: {stderr}"
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let stdout = std::fs::read_to_string(&stdout_path)?;
    let stderr = std::fs::read_to_string(&stderr_path)?;
    if !status.success() {
        return Err(format!(
            "real-UID {mode} child failed: {status}; stdout: {stdout}; stderr: {stderr}"
        )
        .into());
    }
    if !stdout.contains("running 1 test") || !stdout.contains("1 passed") {
        return Err(format!("real-UID {mode} child did not run exactly one test: {stdout}").into());
    }
    Ok(())
}

/// Invoked only through the explicit parent fixture under setpriv. Missing
/// custody or an unchanged UID is an error, never a default-CI false pass.
#[test]
#[ignore = "only the explicit Linux real-UID parent may launch this helper"]
fn linux_real_uid_client_helper() -> TestResult {
    use quanta_index_contract::{
        GenerationSelector, ProcessRequestEventPlaneV1, ProcessRequestEventsRequestV1,
        QueryConstraintSetV1, RepoId, RevisionId, SearchPlaneControlIpcRequest,
        SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
        SearchPlaneControlIpcResponseEnvelope, SearchPlaneErrorCodeV2, SearchPlaneQueryIpcRequest,
        SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
        SearchPlaneQueryIpcResponseEnvelope, TextQueryRequest, TextQuerySyntax,
    };
    use quanta_index_ipc::{ClientIoPolicy, send_request};

    let expected: u32 = std::env::var("QI_E3_REAL_UID_EXPECTED")?.parse()?;
    expect_eq(
        "real child effective UID",
        &rustix::process::geteuid().as_raw(),
        &expected,
    )?;
    expect_eq(
        "real child effective GID",
        &rustix::process::getegid().as_raw(),
        &expected,
    )?;
    if expected == 0 {
        return Err("real-UID helper was left as the daemon owner".into());
    }
    let state_root = std::path::PathBuf::from(std::env::var("QI_E3_REAL_UID_STATE_ROOT")?);
    for directory in [state_root.clone(), state_root.join("catalog")] {
        match std::fs::read_dir(&directory) {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
            Err(error) => {
                return Err(format!(
                    "private daemon directory refusal differed for {directory:?}: {error}"
                )
                .into());
            }
            Ok(_entries) => return Err(format!("different UID listed {directory:?}").into()),
        }
    }
    let query = std::env::var("QI_E3_REAL_UID_QUERY_SOCKET")?;
    let control = std::env::var("QI_E3_REAL_UID_CONTROL_SOCKET")?;
    match std::env::var("QI_E3_REAL_UID_MODE")?.as_str() {
        "deny-events" => {
            let response: SearchPlaneControlIpcResponseEnvelope = send_request(
                Path::new(&control),
                &SearchPlaneControlIpcRequestEnvelope {
                    request_id: 0xe305,
                    diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
                    payload: SearchPlaneControlIpcRequest::ProcessRequestEventsV1(
                        ProcessRequestEventsRequestV1 {
                            plane: ProcessRequestEventPlaneV1::Query,
                            limit: 1024,
                        },
                    ),
                },
                ClientIoPolicy::default(),
            )?;
            expect_eq(
                "denied operator response request ID",
                &response.request_id,
                &0xe305,
            )?;
            if !matches!(&response.payload, SearchPlaneControlIpcResponse::Error(error)
                if error.code == SearchPlaneErrorCodeV2::ControlAuthorizationDenied)
            {
                return Err(format!("other UID received operator events: {response:?}").into());
            }
        }
        "query" => {
            let response: SearchPlaneQueryIpcResponseEnvelope = send_request(
                Path::new(&query),
                &SearchPlaneQueryIpcRequestEnvelope {
                    request_id: 0xe306,
                    diagnostic_attempt: quanta_index_contract::DiagnosticAttemptFieldV1::Absent,
                    payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                        syntax: TextQuerySyntax::Native,
                        query_text: "needle".to_owned(),
                        constraints: QueryConstraintSetV1::unconstrained(),
                        generation: None,
                        generation_selector: Some(GenerationSelector::Active {
                            repo_id: RepoId::new("repo-e2e")?,
                            revision_id: RevisionId::new("rev-e2e")?,
                        }),
                        top_k: 5,
                        cursor: None,
                    }),
                },
                ClientIoPolicy::default(),
            )?;
            expect_eq(
                "listed UID query response request ID",
                &response.request_id,
                &0xe306,
            )?;
            let SearchPlaneQueryIpcResponse::Text(page) = &response.payload else {
                return Err(format!("listed UID query was refused: {response:?}").into());
            };
            if page.results.len() != 1 || page.selected_active_head.is_none() {
                return Err(format!("listed UID query lacks the selected result: {page:?}").into());
            }
        }
        other => return Err(format!("unexpected real-UID helper mode: {other}").into()),
    }
    Ok(())
}
