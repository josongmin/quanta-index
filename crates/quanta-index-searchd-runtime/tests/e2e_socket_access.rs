//! QI-BB-014 shared mode through the real daemon: one socket opened to a
//! group, the other two left private, and what the daemon says about it.
//!
//! The oracles are independent of the daemon: file modes and group ids are
//! read back with `stat`, the process's own ids with `geteuid`/`getegid`/
//! `getgroups`, and the daemon's account of its policies is checked twice —
//! as the typed boot inventory the harness holds and as the `boot_socket_*`
//! gauges a metrics scrape returns over the (private) control socket.
//!
//! What a single-uid test cannot prove is a real stranger being refused at
//! `connect` (kernel, `0660`) or at `accept` (peer credentials); the accept
//! path is proven in the IPC crate through a scripted credential source,
//! and the default rail proves the daemon binds, serves and reports as
//! configured. An explicit ignored Linux rail below also uses real client UIDs.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::error::Error;
use std::os::unix::fs::MetadataExt as _;
#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

use quanta_index_contract::TextQuerySyntax;
use quanta_index_ipc::{
    GROUP_DIRECTORY_MODE, GROUP_SOCKET_MODE, PRIVATE_SOCKET_MODE, SharedSocketAccess,
    SocketAccessPolicy,
};
use quanta_index_searchd::app::{SocketAccessPolicies, SocketRole};
use quanta_index_searchd_harness::E2eRuntime;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn expect_eq<T: PartialEq + std::fmt::Debug>(what: &str, observed: &T, expected: &T) -> TestResult {
    if observed == expected {
        Ok(())
    } else {
        Err(format!("{what}: observed {observed:?}, expected {expected:?}").into())
    }
}

fn mode_of(path: &Path) -> TestResult<u32> {
    Ok(std::fs::symlink_metadata(path)?.mode() & 0o7777)
}

fn gid_of(path: &Path) -> TestResult<u32> {
    Ok(std::fs::symlink_metadata(path)?.gid())
}

fn self_gid() -> u32 {
    rustix::process::getegid().as_raw()
}

/// A gid this process is not a member of, by the same reading of
/// `getgroups` the daemon uses.
fn a_gid_this_process_is_not_in() -> TestResult<u32> {
    let mut members: BTreeSet<u32> = rustix::process::getgroups()?
        .into_iter()
        .map(rustix::process::Gid::as_raw)
        .collect();
    let _inserted = members.insert(self_gid());
    (1_u32..=u32::MAX)
        .find(|gid| !members.contains(gid))
        .ok_or_else(|| "this process is a member of every gid".into())
}

fn group_shared(gid: u32) -> SocketAccessPolicy {
    SocketAccessPolicy::Shared(SharedSocketAccess::new(Some(gid), BTreeSet::new()))
}

fn scrape_gauges(rt: &mut E2eRuntime) -> TestResult<BTreeMap<String, f64>> {
    Ok(rt
        .metrics_snapshot()?
        .gauges
        .into_iter()
        .map(|gauge| (gauge.name, gauge.value))
        .collect())
}

fn scrape_counters(rt: &mut E2eRuntime) -> TestResult<BTreeMap<String, u64>> {
    Ok(rt
        .metrics_snapshot()?
        .counters
        .into_iter()
        .map(|counter| (counter.name, counter.value))
        .collect())
}

fn gauge(gauges: &BTreeMap<String, f64>, name: &str) -> TestResult<f64> {
    gauges
        .get(name)
        .copied()
        .ok_or_else(|| format!("gauge `{name}` is in the scrape: {gauges:?}").into())
}

/// A daemon whose query socket is shared with this process's primary
/// group serves a query over it.
///
/// Control and ingest stay private on disk, and the daemon reports
/// exactly those policies in its boot inventory and its scrape.
#[test]
fn a_group_shared_query_socket_serves_and_is_reported_while_the_others_stay_private() -> TestResult
{
    let gid = self_gid();
    let policies = SocketAccessPolicies::new(
        group_shared(gid),
        SocketAccessPolicy::Private,
        SocketAccessPolicy::Private,
    );
    let mut rt = E2eRuntime::boot_with_socket_access(policies.clone())?;
    rt.ingest_text("repo-shared", "src/alpha.rs", "needle alpha")?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let served = rt.query_text(TextQuerySyntax::Native, "needle", 10);
    if let Some(error) = served.typed_error {
        return Err(format!("a query over the shared socket failed typed: {error:?}").into());
    }
    expect_eq(
        "hits over the shared socket",
        &served.candidate_ids.len(),
        &1,
    )?;

    // On disk: the shared socket is `0660` of the group, the private ones
    // `0600`, and the directory the daemon created is `0710` of the group.
    let (query, control, ingest) = rt
        .socket_paths()
        .ok_or("the driver is running after a served query")?;
    expect_eq("query socket mode", &mode_of(query)?, &GROUP_SOCKET_MODE)?;
    expect_eq("query socket gid", &gid_of(query)?, &gid)?;
    expect_eq(
        "control socket mode",
        &mode_of(control)?,
        &PRIVATE_SOCKET_MODE,
    )?;
    expect_eq(
        "ingest socket mode",
        &mode_of(ingest)?,
        &PRIVATE_SOCKET_MODE,
    )?;
    let directory = rt
        .socket_directory()
        .ok_or("a shared policy places the sockets in a directory of their own")?;
    expect_eq(
        "socket directory mode",
        &mode_of(directory)?,
        &GROUP_DIRECTORY_MODE,
    )?;
    expect_eq("socket directory gid", &gid_of(directory)?, &gid)?;

    // The boot inventory names every policy as configured.
    let inventory = rt
        .boot_inventory()
        .ok_or("the harness holds the boot inventory while the driver runs")?;
    expect_eq(
        "boot inventory policies",
        &inventory.socket_access,
        &policies,
    )?;
    for role in SocketRole::ALL {
        expect_eq(
            &format!("{} policy by role", role.as_str()),
            inventory.socket_access.for_role(role),
            policies.for_role(role),
        )?;
    }

    // The scrape carries the same policies as gauges: shared flags, listed
    // uids, and a gid only for the socket that names a group.
    let gauges = scrape_gauges(&mut rt)?;
    expect_eq(
        "query shared",
        &gauge(&gauges, "boot_socket_query_shared")?,
        &1.0,
    )?;
    expect_eq(
        "query group gid",
        &gauge(&gauges, "boot_socket_query_group_gid")?,
        &f64::from(gid),
    )?;
    expect_eq(
        "query listed uids",
        &gauge(&gauges, "boot_socket_query_allowed_uids")?,
        &0.0,
    )?;
    for role in ["control", "ingest"] {
        expect_eq(
            &format!("{role} shared"),
            &gauge(&gauges, &format!("boot_socket_{role}_shared"))?,
            &0.0,
        )?;
        expect_eq(
            &format!("{role} listed uids"),
            &gauge(&gauges, &format!("boot_socket_{role}_allowed_uids"))?,
            &0.0,
        )?;
        let gid_gauge = format!("boot_socket_{role}_group_gid");
        if gauges.contains_key(&gid_gauge) {
            return Err(format!("a private socket reports no group gid: {gid_gauge}").into());
        }
    }
    // Nothing was refused: every connection this test made is the owner's.
    let counters = scrape_counters(&mut rt)?;
    for plane in ["query", "control", "ingest"] {
        for suffix in ["peer_refused_total", "peer_credentials_unreadable_total"] {
            let name = format!("ipc_{plane}_{suffix}");
            let observed = counters
                .get(&name)
                .copied()
                .ok_or_else(|| format!("counter `{name}` is in the scrape"))?;
            expect_eq(&name, &observed, &0)?;
        }
    }
    Ok(())
}

/// A same-process daemon reopen reuses the configured shared socket namespace
/// and serves the persisted generation after the directory is recreated.
#[test]
fn shared_socket_namespace_survives_same_process_reopen() -> TestResult {
    let policies = SocketAccessPolicies::new(
        group_shared(self_gid()),
        SocketAccessPolicy::Private,
        SocketAccessPolicy::Private,
    );
    let mut rt = E2eRuntime::boot_with_socket_access(policies)?;
    let directory = rt
        .socket_directory()
        .ok_or("shared socket policy must reserve a directory")?
        .to_path_buf();
    rt.ingest_text("repo-shared", "src/reopen.rs", "reopen_needle")?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let before = rt.query_text(TextQuerySyntax::Native, "reopen_needle", 10);
    if before.typed_error.is_some() || before.candidate_ids.len() != 1 {
        return Err("shared socket query failed before reopen".into());
    }

    rt.try_reopen_in_place()?;
    expect_eq(
        "shared socket namespace after stop",
        &rt.socket_directory().map(Path::to_path_buf),
        &Some(directory.clone()),
    )?;
    if directory.exists() {
        return Err("shared socket directory was not removed on stop".into());
    }
    rt.start()?;
    expect_eq(
        "shared socket namespace after restart",
        &rt.socket_directory().map(Path::to_path_buf),
        &Some(directory.clone()),
    )?;
    if !directory.is_dir() {
        return Err("shared socket directory was not recreated on restart".into());
    }
    let after = rt.query_text(TextQuerySyntax::Native, "reopen_needle", 10);
    if after.typed_error.is_some() || after.candidate_ids.len() != 1 {
        return Err("shared socket query failed after reopen".into());
    }
    rt.stop()?;
    if directory.exists() {
        return Err("shared socket directory leaked after final stop".into());
    }
    Ok(())
}

/// A shared policy naming a group the daemon is not a member of refuses
/// boot with a typed reason, before any socket — or the directory for
/// them — exists.
#[test]
fn a_group_the_daemon_is_not_in_refuses_boot_before_any_socket_is_bound() -> TestResult {
    let outsider = a_gid_this_process_is_not_in()?;
    let policies = SocketAccessPolicies::new(
        group_shared(outsider),
        SocketAccessPolicy::Private,
        SocketAccessPolicy::Private,
    );
    let mut rt = E2eRuntime::boot_with_socket_access(policies)?;
    let refused = match rt.start() {
        Ok(()) => return Err("a group the daemon is not in must refuse boot".into()),
        Err(error) => format!("{error:#}"),
    };
    if !refused.contains("SOCKET_ACCESS_UNSATISFIABLE")
        || !refused.contains(&format!("gid {outsider}"))
    {
        return Err(format!("the refusal names the code and the group: {refused}").into());
    }
    let directory = rt
        .socket_directory()
        .ok_or("a shared policy names a socket directory")?;
    if directory.exists() {
        return Err(format!(
            "a refused boot leaves no socket directory behind: {}",
            directory.display()
        )
        .into());
    }
    if rt.boot_inventory().is_some() || rt.socket_paths().is_some() {
        return Err("a refused boot leaves no running driver".into());
    }
    Ok(())
}

/// Explicit Linux root-runner component proof. The helper below runs as a
/// different kernel UID; this is not a release-daemon or P11 host proof.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an explicit Linux root container with setpriv and SETUID/SETGID"]
fn linux_two_real_uids_enforce_operator_events_and_serve_listed_query() -> TestResult {
    use quanta_index_contract::{ProcessRequestEventPlaneV1, ProcessRequestEventStageV1};

    use crate::fail_closed_wait::{RealTicker, WaitError, wait_for};

    if rustix::process::geteuid().as_raw() != 0 {
        return Err("Linux real-UID proof requires a root runner; never skip it".into());
    }
    let other_uid = 65_534_u32;
    let listed = SocketAccessPolicy::Shared(SharedSocketAccess::new(
        None,
        [other_uid].into_iter().collect(),
    ));
    let policies = SocketAccessPolicies::new(listed.clone(), listed, SocketAccessPolicy::Private);
    let mut rt = E2eRuntime::boot_with_socket_access(policies)?;
    let outcome = (|| -> TestResult {
        rt.ingest_text("repo-real-uid", "src/linux_uid.rs", "needle real uid")?;
        let _generation = rt.seal()?;
        rt.activate_last_sealed_generation()?;
        let (query, control, _ingest) = rt.socket_paths().ok_or("missing live sockets")?;
        let query = query.to_path_buf();
        let control = control.to_path_buf();
        expect_eq("listed query socket mode", &mode_of(&query)?, &0o666)?;
        expect_eq("listed control socket mode", &mode_of(&control)?, &0o666)?;
        let directory = rt
            .socket_directory()
            .ok_or("shared socket directory absent")?;
        expect_eq("listed socket directory mode", &mode_of(directory)?, &0o711)?;
        let root = rt.state_root().to_path_buf();
        // An owner can read the bounded ring. The outsider's Admin request
        // must return only a typed refusal, leaving this query window intact.
        let before = rt.process_request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
        run_real_uid_probe(other_uid, "deny-events", &query, &control, &root)?;
        let after_denial = rt.process_request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
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
        run_real_uid_probe(other_uid, "query", &query, &control, &root)?;
        // A client may receive its response before the server records the
        // terminal event. Await that observable completion, not a fixed delay.
        let after_query = wait_for(
            &RealTicker::new(),
            Duration::from_secs(5),
            Duration::from_millis(10),
            "listed UID query terminal event",
            || rt.process_request_events(ProcessRequestEventPlaneV1::Query, 1024),
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
    let stopped = rt.stop();
    match (outcome, stopped) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
        (Err(probe), Err(cleanup)) => {
            Err(format!("real-UID probe failed: {probe}; daemon cleanup failed: {cleanup}").into())
        }
    }
}

/// The same test binary is copied outside any 0700 Cargo checkout so a
/// deprivileged OS process can execute the real SDK/IPC client code.
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
#[test]
#[ignore = "only the explicit Linux real-UID parent may launch this helper"]
fn linux_real_uid_client_helper() -> TestResult {
    use quanta_index_contract::{
        GenerationSelector, ProcessRequestEventPlaneV1, ProcessRequestEventsRequestV1,
        QueryConstraintSetV1, RepoId, RevisionId, SearchPlaneControlIpcRequest,
        SearchPlaneControlIpcRequestEnvelope, SearchPlaneControlIpcResponse,
        SearchPlaneControlIpcResponseEnvelope, SearchPlaneErrorCodeV2, SearchPlaneQueryIpcRequest,
        SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
        SearchPlaneQueryIpcResponseEnvelope, TextQueryRequest,
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
    let state_root = std::env::var("QI_E3_REAL_UID_STATE_ROOT")?;
    match std::fs::read_dir(state_root) {
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
        Err(error) => return Err(format!("private daemon root refusal differed: {error}").into()),
        Ok(_entries) => return Err("different UID traversed the private daemon state root".into()),
    }
    let query = std::env::var("QI_E3_REAL_UID_QUERY_SOCKET")?;
    let control = std::env::var("QI_E3_REAL_UID_CONTROL_SOCKET")?;
    match std::env::var("QI_E3_REAL_UID_MODE")?.as_str() {
        "deny-events" => {
            let response: SearchPlaneControlIpcResponseEnvelope = send_request(
                Path::new(&control),
                &SearchPlaneControlIpcRequestEnvelope {
                    request_id: 0xe305,
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
