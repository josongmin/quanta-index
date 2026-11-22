//! Pinned old-Q shared-socket policy through CLI configuration and real IPC.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};

use quanta_index_contract::{GenerationPin, ManifestGeneration, RepoId, RevisionId};
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, SearchdBinaryProcess, daemon_socket_paths, publish_legacy_source,
};
use quanta_index_searchd_harness::private_tempdir;

use crate::legacy_start_probe::wait_for_refusal;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn a_group_shared_query_socket_serves_and_is_reported_while_the_others_stay_private() -> TestResult
{
    let gid = rustix::process::getegid().as_raw();
    // The CLI places the sockets under the state root. The group must be
    // able to traverse the ancestors, including on hosts with a private TMPDIR.
    let root = tempfile::tempdir_in("/tmp")?;
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o711))?;
    let binary = LegacySearchdBinary::from_environment()?;
    let mut command = binary.benchmark_command(root.path(), 8)?;
    let _configured = command.env(
        "QUANTA_INDEX_QUERY_SOCKET_ACCESS",
        format!("shared:group={gid}"),
    );
    let daemon = SearchdBinaryProcess::start_with_pinned_command(root.path(), command)?;
    let client = daemon.connect()?;
    let pin = GenerationPin::new(
        RepoId::new("repo-shared")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    );
    let (_active, _receipt) = publish_legacy_source(
        &client,
        &pin.repo_id,
        &pin.revision_id,
        pin.manifest_generation.get(),
        None,
        &[("src/alpha.rs", "needle alpha", 1)],
        None,
    )?;
    let answer = client
        .lexical()
        .query()
        .native("needle")
        .pinned(pin.clone())
        .top_k(10)
        .execute()?;
    let [hit] = answer.results.as_slice() else {
        return Err(format!("expected one hit through the group socket: {answer:?}").into());
    };
    if answer.generation != pin || hit.repo_relative_path.as_str() != "src/alpha.rs" {
        return Err(
            format!("the group socket served another row or generation: {answer:?}").into(),
        );
    }

    let [query, control, ingest] = daemon_socket_paths(root.path());
    for (path, mode, group) in [
        (&query, 0o660, Some(gid)),
        (&control, 0o600, None),
        (&ingest, 0o600, None),
    ] {
        let stat = fs::symlink_metadata(path)?;
        if !stat.file_type().is_socket()
            || stat.mode() & 0o7777 != mode
            || group.is_some_and(|expected| stat.gid() != expected)
        {
            return Err(
                format!("unexpected socket permissions or gid for {path:?}: {stat:?}").into(),
            );
        }
    }
    let directory = fs::symlink_metadata(root.path().join("search-plane"))?;
    if !directory.is_dir() || directory.mode() & 0o7777 != 0o710 || directory.gid() != gid {
        return Err(format!("unexpected group socket directory: {directory:?}").into());
    }

    // The boot inventory projects the policy of each socket into these
    // gauges. For this fixture they cover the group, sharing and UID fields.
    let metrics = client.observability().metrics_snapshot()?;
    for (role, shared, group) in [
        ("query", 1.0_f64, Some(f64::from(gid))),
        ("control", 0.0, None),
        ("ingest", 0.0, None),
    ] {
        for (suffix, expected) in [
            ("shared", Some(shared)),
            ("allowed_uids", Some(0.0)),
            ("group_gid", group),
        ] {
            let name = format!("boot_socket_{role}_{suffix}");
            let actual = metrics.gauges.iter().find(|point| point.name == name);
            if actual.map(|point| point.value.to_bits()) != expected.map(f64::to_bits) {
                return Err(format!("expected {name}={expected:?}, got {actual:?}").into());
            }
        }
        for suffix in ["peer_refused_total", "peer_credentials_unreadable_total"] {
            let name = format!("ipc_{role}_{suffix}");
            let actual = metrics.counters.iter().find(|point| point.name == name);
            if actual.map(|point| point.value) != Some(0) {
                return Err(format!("expected {name}=0, got {actual:?}").into());
            }
        }
    }
    drop(client);
    daemon.stop()?;
    binary.verify()
}

#[test]
fn outsider_group_refuses_before_any_socket_is_bound() -> TestResult {
    let mut groups: BTreeSet<u32> = rustix::process::getgroups()?
        .into_iter()
        .map(rustix::process::Gid::as_raw)
        .collect();
    let _inserted = groups.insert(rustix::process::getegid().as_raw());
    let outsider = (1_u32..=u32::MAX)
        .find(|gid| !groups.contains(gid))
        .ok_or("this process is a member of every gid")?;
    let root = private_tempdir()?;
    let binary = LegacySearchdBinary::from_environment()?;
    let mut command = binary.command()?;
    let _configured = command.arg("serve").arg("--state-root").arg(root.path());
    let _configured = command.apply_searchd_env(8);
    let _configured = command.env(
        "QUANTA_INDEX_QUERY_SOCKET_ACCESS",
        format!("shared:group={outsider}"),
    );
    let output = wait_for_refusal(&binary, command)?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.code() != Some(70)
        || !output.stdout.is_empty()
        || !stderr.contains("SOCKET_ACCESS_UNSATISFIABLE")
        || !stderr.contains(&format!("gid {outsider}"))
    {
        return Err(format!(
            "old-Q socket refusal differs: exit={:?}, stdout={:?}, stderr={stderr}",
            output.status.code(),
            output.stdout
        )
        .into());
    }
    if root.path().join("search-plane").exists()
        || daemon_socket_paths(root.path())
            .into_iter()
            .any(|path| path.exists())
        || root.path().join("indexes").exists()
    {
        return Err("refused old-Q boot created socket or publication state".into());
    }
    Ok(())
}
