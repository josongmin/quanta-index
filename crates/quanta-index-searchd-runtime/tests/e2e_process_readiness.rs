//! P09: real supervised daemon and control UDS process-readiness proof.

use std::error::Error;
use std::os::unix::net::UnixListener;
use std::time::Duration;

use quanta_index_contract::{
    ProcessReadinessReasonV1, ProcessReadinessV1, SearchPlaneControlIpcResponse,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest,
};
use quanta_index_core::GenerationStorageKeyV1;
use quanta_index_searchd_harness::E2eRuntime;

use crate::fail_closed_wait::{RealTicker, WaitError, wait_for};
use crate::searchd_binary_process::{SearchdBinaryProcess, daemon_socket_paths};

type TestResult = Result<(), Box<dyn Error>>;

fn require_eq<T: std::fmt::Debug + PartialEq>(actual: &T, expected: &T, field: &str) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{field}: expected {expected:?}, got {actual:?}").into())
    }
}

fn wait_until_ready(rt: &mut E2eRuntime) -> Result<ProcessReadinessV1, Box<dyn Error>> {
    match wait_for(
        &RealTicker::new(),
        Duration::from_secs(5),
        Duration::from_millis(10),
        "supervised process readiness",
        || rt.process_readiness(),
        |report| report.ready,
        |_| false,
    ) {
        Ok(report) => Ok(report),
        Err(WaitError::Terminal(error)) => Err(error.into()),
        Err(WaitError::Timeout(timeout)) => Err(Box::new(timeout)),
    }
}

fn wait_until_not_ready(rt: &mut E2eRuntime) -> Result<ProcessReadinessV1, Box<dyn Error>> {
    match wait_for(
        &RealTicker::new(),
        Duration::from_secs(5),
        Duration::from_millis(10),
        "active backend root loss",
        || rt.process_readiness(),
        |report| !report.ready,
        |_| false,
    ) {
        Ok(report) => Ok(report),
        Err(WaitError::Terminal(error)) => Err(error.into()),
        Err(WaitError::Timeout(timeout)) => Err(Box::new(timeout)),
    }
}

#[test]
fn zero_active_repositories_are_ready_only_with_all_supervised_children() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let report = wait_until_ready(&mut rt)?;
    require_eq(&report.active_repositories, &0, "active repositories")?;
    require_eq(
        &report.active_candidate_integrity,
        &None,
        "active integrity",
    )?;
    require_eq(&report.components.query_plane, &true, "query plane")?;
    require_eq(&report.components.control_plane, &true, "control plane")?;
    require_eq(&report.components.ingest_plane, &true, "ingest plane")?;
    require_eq(
        &report.components.maintenance_heartbeat,
        &true,
        "maintenance heartbeat",
    )?;
    require_eq(&report.not_ready_reasons, &Vec::new(), "not-ready reasons")?;
    Ok(())
}

#[test]
fn zero_active_repositories_do_not_require_existing_track_roots() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    for track in ["lexical", "semantic"] {
        let root = rt.state_root().join("indexes").join(track);
        if root.exists() {
            let hidden = rt
                .state_root()
                .join("indexes")
                .join(format!("{track}-hidden"));
            std::fs::rename(&root, &hidden)?;
            let report = wait_until_ready(&mut rt);
            std::fs::rename(&hidden, &root)?;
            require_eq(&report?.ready, &true, "zero-active readiness")?;
        }
    }
    require_eq(
        &wait_until_ready(&mut rt)?.ready,
        &true,
        "zero-active readiness",
    )?;
    Ok(())
}

#[test]
fn active_repository_requires_physical_candidate_proof() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo-readiness", "src/ready.rs", "readiness probe")?;
    let _sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let report = wait_until_ready(&mut rt)?;
    require_eq(&report.active_repositories, &1, "active repositories")?;
    require_eq(
        &report.active_candidate_integrity,
        &Some(true),
        "active integrity",
    )?;
    require_eq(
        &report
            .not_ready_reasons
            .contains(&ProcessReadinessReasonV1::ActiveCandidateIntegrityFailed),
        &false,
        "active integrity failure reason",
    )?;
    Ok(())
}

#[test]
fn lost_active_track_root_invalidates_backend_readiness_and_restores() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo-backend-loss", "src/ready.rs", "fn ready() {}")?;
    let _sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    require_eq(
        &wait_until_ready(&mut rt)?.ready,
        &true,
        "initial readiness",
    )?;

    for track in ["lexical", "semantic"] {
        let root = rt.state_root().join("indexes").join(track);
        let hidden = rt
            .state_root()
            .join("indexes")
            .join(format!("{track}-hidden"));
        std::fs::rename(&root, &hidden)?;
        let outcome = (|| -> TestResult {
            let report = wait_until_not_ready(&mut rt)?;
            if !report
                .not_ready_reasons
                .contains(&ProcessReadinessReasonV1::RequiredBackendOpenUnproven)
            {
                return Err(format!("{track} root loss lacks backend reason: {report:?}").into());
            }
            Ok(())
        })();
        std::fs::rename(&hidden, &root)?;
        outcome?;
        require_eq(
            &wait_until_ready(&mut rt)?.ready,
            &true,
            "restored readiness",
        )?;
    }
    Ok(())
}

#[test]
fn binary_daemon_detects_lost_active_backend_root() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let mut prepared = E2eRuntime::boot_in(&state_root)?;
    prepared.ingest_text("repo-binary-backend-loss", "src/ready.rs", "fn ready() {}")?;
    let _sealed = prepared.seal()?;
    prepared.activate_last_sealed_generation()?;
    prepared.stop()?;

    let process = SearchdBinaryProcess::start(&state_root)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let ready = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "binary daemon active backend ready",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| true,
        )?;
        require_eq(&ready.active_repositories, &1, "binary active repositories")?;

        let root = state_root.join("indexes/lexical");
        let hidden = state_root.join("indexes/lexical-hidden");
        std::fs::rename(&root, &hidden)?;
        let lost = (|| -> TestResult {
            let report = wait_for(
                &RealTicker::new(),
                Duration::from_secs(20),
                Duration::from_millis(50),
                "binary daemon active backend loss",
                || client.observability().process_readiness(),
                |report| !report.ready,
                |_| true,
            )?;
            if !report
                .not_ready_reasons
                .contains(&ProcessReadinessReasonV1::RequiredBackendOpenUnproven)
            {
                return Err(format!("binary root loss lacks backend reason: {report:?}").into());
            }
            Ok(())
        })();
        std::fs::rename(&hidden, &root)?;
        lost?;
        let restored = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "binary daemon active backend restored",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| true,
        )?;
        require_eq(&restored.ready, &true, "binary restored readiness")
    })();
    let stopped = process.stop();
    outcome.and(stopped)
}

#[test]
fn reactivated_generation_reproves_physical_authority_after_aba() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo-readiness", "src/ready.rs", "fn first() {}")?;
    let _sealed = rt.seal()?;
    let first = rt
        .last_sealed_search_corpus_identity()
        .ok_or("first sealed generation has an identity")?;
    rt.activate_last_sealed_generation()?;
    let first_head = rt
        .active_search_corpus_head()?
        .ok_or("first generation is active")?;
    require_eq(&wait_until_ready(&mut rt)?.ready, &true, "first readiness")?;

    rt.ingest_text("repo-readiness", "src/ready.rs", "fn second() {}")?;
    let _sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let second_head = rt
        .active_search_corpus_head()?
        .ok_or("second generation is active")?;
    if second_head.generation == first {
        return Err("second activation did not change the generation".into());
    }
    // Do not poll readiness at B: the last cached physical proof must still
    // be for A when the catalog returns to A.
    let rollback =
        rt.rollback_search_corpus_cas_raw(SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: second_head,
            target: first.clone(),
        })?;
    if !matches!(
        rollback,
        SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
    ) {
        return Err(format!("rollback to first generation failed: {rollback:?}").into());
    }
    let returned_head = rt
        .active_search_corpus_head()?
        .ok_or("first generation is active again")?;
    require_eq(&returned_head.generation, &first, "returned generation")?;
    if returned_head.activation_token == first_head.activation_token {
        return Err("A -> B -> A must issue a new activation token".into());
    }

    let storage_key = GenerationStorageKeyV1::for_repo_revision(&rt.repo(), &rt.revision());
    let manifest = std::fs::canonicalize(rt.state_root())?
        .join("indexes/lexical")
        .join(storage_key.as_str())
        .join(format!("g{}", first.lexical.manifest_generation.get()))
        .join("text-authority/manifest.cbor");
    let original = std::fs::read(&manifest)?;
    let mut corrupted = original.clone();
    let last = corrupted.last_mut().ok_or("sealed manifest is empty")?;
    *last ^= 0xff;
    std::fs::write(&manifest, corrupted)?;
    let damaged_result = (|| -> TestResult {
        let report = rt.process_readiness()?;
        require_eq(
            &report.ready,
            &false,
            "damaged returned generation readiness",
        )?;
        require_eq(
            &report.active_candidate_integrity,
            &Some(false),
            "damaged returned generation physical proof",
        )?;
        if !report
            .not_ready_reasons
            .contains(&ProcessReadinessReasonV1::ActiveCandidateIntegrityFailed)
        {
            return Err(format!("missing active integrity reason: {report:?}").into());
        }
        Ok(())
    })();
    std::fs::write(&manifest, original)?;
    damaged_result?;
    require_eq(
        &wait_until_ready(&mut rt)?.ready,
        &true,
        "restored readiness",
    )?;
    Ok(())
}

#[test]
fn surviving_control_socket_reports_lost_or_replaced_plane_path_not_ready() -> TestResult {
    for (lost_plane, reason) in [
        (0, ProcessReadinessReasonV1::QueryPlaneUnhealthy),
        (2, ProcessReadinessReasonV1::IngestPlaneUnhealthy),
    ] {
        let parent = quanta_index_searchd_harness::private_tempdir()?;
        let state_root = parent.path().join("state");
        let process = SearchdBinaryProcess::start(&state_root)?;
        let outcome = (|| -> TestResult {
            let client = process.connect()?;
            let ready = wait_for(
                &RealTicker::new(),
                Duration::from_secs(5),
                Duration::from_millis(10),
                "daemon binary ready before socket path loss",
                || client.observability().process_readiness(),
                |report| report.ready,
                |_| true,
            )?;
            require_eq(&ready.ready, &true, "initial readiness")?;

            let sockets = daemon_socket_paths(&state_root);
            let socket_path = sockets
                .get(lost_plane)
                .ok_or("lost plane index has no socket path")?;
            std::fs::remove_file(socket_path)?;
            // Replacing query.sock with another valid socket must not pass
            // an existence/type check; only the daemon's bound inode counts.
            let replacement = (lost_plane == 0)
                .then(|| UnixListener::bind(socket_path))
                .transpose()?;
            let report = client.observability().process_readiness()?;
            require_eq(&report.ready, &false, "readiness after socket path loss")?;
            if !report.not_ready_reasons.contains(&reason) {
                return Err(format!("lost plane {lost_plane} lacks {reason:?}: {report:?}").into());
            }
            drop(replacement);
            Ok(())
        })();
        let stopped = process.stop();
        outcome.and(stopped)?;
    }
    Ok(())
}
