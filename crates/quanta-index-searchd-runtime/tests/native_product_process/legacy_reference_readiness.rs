//! External, pinned Legacy reference compatibility for process readiness.
//! These assertions do not qualify the Native daemon's readiness contract.

use std::{
    fs,
    os::unix::{fs::PermissionsExt as _, net::UnixListener},
    time::Duration,
};

use quanta_index_contract::ProcessReadinessReasonV1;
use quanta_index_searchd_harness::legacy_reference::daemon_socket_paths;

use super::{TestResult, require_equal};
use crate::{
    fail_closed_wait::{RealTicker, wait_for},
    legacy_searchd_binary::LegacySearchdBinary,
};

fn private_short_root() -> TestResult<tempfile::TempDir> {
    let root = tempfile::Builder::new().prefix("qlr-").tempdir_in("/tmp")?;
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))?;
    Ok(root)
}

#[test]
fn zero_active_repositories_are_ready_only_with_all_supervised_children() -> TestResult {
    let parent = private_short_root()?;
    let state_root = parent.path().join("state");
    let legacy = LegacySearchdBinary::from_environment()?;
    let process = legacy.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let report = wait_for(
            &RealTicker::new(),
            Duration::from_secs(5),
            Duration::from_millis(10),
            "external Legacy reference supervised process readiness",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| false,
        )?;
        require_equal(&report.active_repositories, &0, "active repositories")?;
        require_equal(
            &report.active_candidate_integrity,
            &None,
            "active integrity",
        )?;
        require_equal(&report.components.query_plane, &true, "query plane")?;
        require_equal(&report.components.control_plane, &true, "control plane")?;
        require_equal(&report.components.ingest_plane, &true, "ingest plane")?;
        require_equal(
            &report.components.maintenance_heartbeat,
            &true,
            "maintenance heartbeat",
        )?;
        require_equal(&report.not_ready_reasons, &Vec::new(), "not-ready reasons")?;
        Ok(())
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    legacy.verify()?;
    Ok(())
}

#[test]
fn zero_active_repositories_do_not_require_existing_track_roots() -> TestResult {
    let parent = private_short_root()?;
    let state_root = parent.path().join("state");
    let legacy = LegacySearchdBinary::from_environment()?;
    let process = legacy.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        for track in ["lexical", "semantic"] {
            let root = state_root.join("indexes").join(track);
            let hidden = state_root.join("indexes").join(format!("{track}-hidden"));
            let existed = root.exists();
            if existed {
                fs::rename(&root, &hidden)?;
            }
            let observed = (|| -> TestResult {
                require_equal(&root.exists(), &false, "hidden or absent track root")?;
                let report = wait_for(
                    &RealTicker::new(),
                    Duration::from_secs(5),
                    Duration::from_millis(10),
                    "external Legacy reference zero-active readiness without track root",
                    || client.observability().process_readiness(),
                    |report| report.ready,
                    |_| false,
                )?;
                require_equal(&report.ready, &true, "zero-active readiness")?;
                require_equal(&report.active_repositories, &0, "active repositories")?;
                require_equal(
                    &report.active_candidate_integrity,
                    &None,
                    "active integrity",
                )?;
                require_equal(&report.not_ready_reasons, &Vec::new(), "not-ready reasons")
            })();
            if existed {
                fs::rename(&hidden, &root)?;
            }
            observed?;
        }
        let report = wait_for(
            &RealTicker::new(),
            Duration::from_secs(5),
            Duration::from_millis(10),
            "external Legacy reference zero-active readiness after track restoration",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| false,
        )?;
        require_equal(&report.ready, &true, "restored zero-active readiness")?;
        require_equal(&report.active_repositories, &0, "active repositories")?;
        require_equal(
            &report.active_candidate_integrity,
            &None,
            "active integrity",
        )?;
        require_equal(&report.not_ready_reasons, &Vec::new(), "not-ready reasons")
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    legacy.verify()?;
    Ok(())
}

#[test]
fn surviving_control_socket_reports_lost_or_replaced_plane_path_not_ready() -> TestResult {
    let legacy = LegacySearchdBinary::from_environment()?;
    for (lost_plane, reason) in [
        (0, ProcessReadinessReasonV1::QueryPlaneUnhealthy),
        (2, ProcessReadinessReasonV1::IngestPlaneUnhealthy),
    ] {
        let parent = private_short_root()?;
        let state_root = parent.path().join("state");
        let process = legacy.start(&state_root, 8)?;
        let outcome = (|| -> TestResult {
            let client = process.connect()?;
            let ready = wait_for(
                &RealTicker::new(),
                Duration::from_secs(5),
                Duration::from_millis(10),
                "external Legacy reference ready before socket path loss",
                || client.observability().process_readiness(),
                |report| report.ready,
                |_| true,
            )?;
            require_equal(&ready.ready, &true, "initial readiness")?;

            let sockets = daemon_socket_paths(&state_root);
            let socket_path = sockets
                .get(lost_plane)
                .ok_or("lost plane index has no socket path")?;
            fs::remove_file(socket_path)?;
            // A valid replacement socket must not pass the bound-inode check.
            let replacement = (lost_plane == 0)
                .then(|| UnixListener::bind(socket_path))
                .transpose()?;
            let report = client.observability().process_readiness()?;
            require_equal(&report.ready, &false, "readiness after socket path loss")?;
            if !report.not_ready_reasons.contains(&reason) {
                return Err(format!("lost plane {lost_plane} lacks {reason:?}: {report:?}").into());
            }
            drop(replacement);
            Ok(())
        })();
        let stopped = process.stop();
        outcome.and(stopped)?;
        legacy.verify()?;
    }
    Ok(())
}
