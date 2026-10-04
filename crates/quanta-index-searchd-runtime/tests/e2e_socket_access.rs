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
//! and this rail proves the daemon binds, serves and reports as configured.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::error::Error;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

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
