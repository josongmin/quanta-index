//! P09: real supervised daemon and control UDS process-readiness proof.

use std::error::Error;
use std::os::unix::net::UnixListener;
use std::time::Duration;

use quanta_index_contract::{
    GenerationPin, ProcessReadinessReasonV1, ProcessReadinessV1, ProcessRequestEventPlaneV1,
    ProcessRequestEventStageV1, QueryConstraintSetV1, SearchPlaneControlIpcResponse,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
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
fn operator_ring_correlates_queue_backend_and_terminal_by_request_id() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo-events", "src/events.rs", "needle events")?;
    let _sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let before = rt.process_request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
    let served = rt.query_text(TextQuerySyntax::Native, "needle", 5);
    if let Some(error) = served.typed_error {
        return Err(format!("fixture query failed: {error:?}").into());
    }
    let after = rt.process_request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
    if after.process_instance != before.process_instance {
        return Err("query ring process instance changed during the probe".into());
    }
    let outcome = after.events.iter().find(|event| {
        event.sequence >= before.next_sequence
            && event.stage == ProcessRequestEventStageV1::BackendOutcome
            && event.route.as_deref() == Some("query.text")
    });
    let Some(outcome) = outcome else {
        return Err("query backend outcome missing from bounded operator ring".into());
    };
    let stages: Vec<_> = after
        .events
        .iter()
        .filter(|event| event.request_id == outcome.request_id)
        .map(|event| event.stage)
        .collect();
    for required in [
        ProcessRequestEventStageV1::QueueAdmitted,
        ProcessRequestEventStageV1::BackendStarted,
        ProcessRequestEventStageV1::BackendReturned,
        ProcessRequestEventStageV1::BackendOutcome,
        ProcessRequestEventStageV1::ResponseWritten,
    ] {
        if !stages.contains(&required) {
            return Err(format!(
                "request {} lacks stage {required:?}: {stages:?}",
                outcome.request_id
            )
            .into());
        }
    }
    Ok(())
}

#[test]
fn operator_ring_correlates_provider_ticket_with_query_terminal() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    rt.ingest_text("repo-provider-events", "src/provider.rs", "needle provider")?;
    let _sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let before = rt.process_request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
    let served = rt.query_semantic("needle", 5, None);
    if let Some(error) = served.typed_error {
        return Err(format!("semantic fixture query failed: {error:?}").into());
    }
    let after = rt.process_request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
    let Some(outcome) = after.events.iter().find(|event| {
        event.sequence >= before.next_sequence
            && event.stage == ProcessRequestEventStageV1::BackendOutcome
            && event.route.as_deref() == Some("query.semantic")
    }) else {
        return Err("semantic backend outcome missing from operator ring".into());
    };
    let correlated: Vec<_> = after
        .events
        .iter()
        .filter(|event| event.request_id == outcome.request_id)
        .collect();
    let started = correlated
        .iter()
        .find(|event| event.stage == ProcessRequestEventStageV1::ProviderStarted)
        .ok_or("provider start missing for semantic request")?;
    let returned = correlated
        .iter()
        .find(|event| event.stage == ProcessRequestEventStageV1::ProviderReturned)
        .ok_or("provider return missing for semantic request")?;
    require_eq(&started.ticket_id, &returned.ticket_id, "provider ticket")?;
    if started.ticket_id.is_none()
        || !correlated
            .iter()
            .any(|event| event.stage == ProcessRequestEventStageV1::ResponseWritten)
    {
        return Err("provider ticket or terminal response missing".into());
    }
    Ok(())
}

#[test]
fn binary_daemon_exposes_one_correlated_query_without_payload() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let mut prepared = E2eRuntime::boot_in(&state_root)?;
    prepared.ingest_text("repo-binary-events", "src/events.rs", "needle events")?;
    let sealed = prepared.seal()?;
    prepared.activate_last_sealed_generation()?;
    let pin = GenerationPin::new(prepared.repo(), prepared.revision(), sealed);
    prepared.stop()?;

    let process = SearchdBinaryProcess::start(&state_root)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let before = client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
        let request_id = 0x5eed_u64;
        let query = SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "needle".to_owned(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 5,
                cursor: None,
            }),
        };
        let sockets = daemon_socket_paths(&state_root);
        let response: SearchPlaneQueryIpcResponseEnvelope = quanta_index_ipc::send_request(
            &sockets[0],
            &query,
            quanta_index_ipc::ClientIoPolicy::default(),
        )?;
        require_eq(
            &response.request_id,
            &request_id,
            "query response request ID",
        )?;
        let text = match response.payload {
            SearchPlaneQueryIpcResponse::Text(text) => text,
            other => return Err(format!("binary query returned wrong response: {other:?}").into()),
        };
        if text.results.is_empty() {
            return Err("binary query returned no fixture result".into());
        }
        let after = client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
        require_eq(
            &after.process_instance,
            &before.process_instance,
            "process instance",
        )?;
        let Some(outcome) = after.events.iter().find(|event| {
            event.sequence >= before.next_sequence
                && event.request_id.get() == request_id
                && event.stage == ProcessRequestEventStageV1::BackendOutcome
                && event.route.as_deref() == Some("query.text")
        }) else {
            return Err("binary query outcome absent from operator ring".into());
        };
        for stage in [
            ProcessRequestEventStageV1::QueueAdmitted,
            ProcessRequestEventStageV1::BackendStarted,
            ProcessRequestEventStageV1::BackendReturned,
            ProcessRequestEventStageV1::ResponseWritten,
        ] {
            if !after
                .events
                .iter()
                .any(|event| event.request_id == outcome.request_id && event.stage == stage)
            {
                return Err(format!("binary query {} lacks {stage:?}", outcome.request_id).into());
            }
        }

        let semantic_before = client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
        let semantic_id = 0x5eee_u64;
        let semantic_query = SearchPlaneQueryIpcRequestEnvelope {
            request_id: semantic_id,
            payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
                query_text: "needle".to_owned(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(pin),
                generation_selector: None,
                lexical_scope: None,
                top_k: 5,
            }),
        };
        let semantic_response: SearchPlaneQueryIpcResponseEnvelope =
            quanta_index_ipc::send_request(
                &sockets[0],
                &semantic_query,
                quanta_index_ipc::ClientIoPolicy::default(),
            )?;
        require_eq(
            &semantic_response.request_id,
            &semantic_id,
            "semantic response request ID",
        )?;
        match semantic_response.payload {
            SearchPlaneQueryIpcResponse::Semantic(_) => {}
            other => return Err(format!("binary semantic query refused: {other:?}").into()),
        }
        let semantic_after = client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
        let correlated: Vec<_> = semantic_after
            .events
            .iter()
            .filter(|event| {
                event.sequence >= semantic_before.next_sequence
                    && event.request_id.get() == semantic_id
            })
            .collect();
        for stage in [
            ProcessRequestEventStageV1::QueueAdmitted,
            ProcessRequestEventStageV1::BackendStarted,
            ProcessRequestEventStageV1::BackendOutcome,
            ProcessRequestEventStageV1::BackendReturned,
            ProcessRequestEventStageV1::ResponseWritten,
        ] {
            if !correlated.iter().any(|event| {
                event.stage == stage
                    && (stage != ProcessRequestEventStageV1::BackendOutcome
                        || event.route.as_deref() == Some("query.semantic"))
            }) {
                return Err(format!("binary semantic query {semantic_id} lacks {stage:?}").into());
            }
        }
        let started = correlated
            .iter()
            .find(|event| event.stage == ProcessRequestEventStageV1::ProviderStarted)
            .ok_or("binary semantic provider start missing")?;
        let returned = correlated
            .iter()
            .find(|event| event.stage == ProcessRequestEventStageV1::ProviderReturned)
            .ok_or("binary semantic provider return missing")?;
        require_eq(
            &started.ticket_id,
            &returned.ticket_id,
            "binary provider ticket",
        )?;
        if started.ticket_id.is_none()
            || !correlated
                .iter()
                .any(|event| event.stage == ProcessRequestEventStageV1::ResponseWritten)
        {
            return Err("binary semantic provider ticket or terminal response missing".into());
        }
        Ok(())
    })();
    let stopped = process.stop();
    outcome.and(stopped)
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
