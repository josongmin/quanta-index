//! P09: real supervised daemon and control UDS process-readiness proof.

use std::error::Error;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::time::Duration;

use quanta_index_contract::{
    GenerationPin, MetricsSnapshotV1, ProcessReadinessReasonV1, ProcessReadinessV1,
    ProcessRequestEventPlaneV1, ProcessRequestEventStageV1, QueryConstraintSetV1,
    SearchPlaneControlIpcResponse, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope,
    SearchPlaneQueryIpcResponse, SearchPlaneQueryIpcResponseEnvelope,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SemanticQueryRequest, TextQueryRequest,
    TextQuerySyntax,
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

fn counter(snapshot: &MetricsSnapshotV1, name: &str) -> Option<u64> {
    snapshot
        .counters
        .iter()
        .find(|point| point.name == name)
        .map(|point| point.value)
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
        let SearchPlaneQueryIpcResponse::Text(text) = response.payload else {
            return Err(format!(
                "binary query returned wrong response: {:?}",
                response.payload
            )
            .into());
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
        let SearchPlaneQueryIpcResponse::Semantic(_) = semantic_response.payload else {
            return Err(format!(
                "binary semantic query refused: {:?}",
                semantic_response.payload
            )
            .into());
        };
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
fn binary_restart_replaces_request_event_instance_and_discards_prior_window() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let mut prepared = E2eRuntime::boot_in(&state_root)?;
    prepared.ingest_text("repo-restart-events", "src/restart.rs", "needle restart")?;
    let sealed = prepared.seal()?;
    prepared.activate_last_sealed_generation()?;
    let pin = GenerationPin::new(prepared.repo(), prepared.revision(), sealed);
    prepared.stop()?;

    let first = SearchdBinaryProcess::start(&state_root)?;
    let first_client = first.connect()?;
    let sockets = daemon_socket_paths(&state_root);
    let request_id = 0x5ee1_u64;
    let query = SearchPlaneQueryIpcRequestEnvelope {
        request_id,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".to_owned(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(pin),
            generation_selector: None,
            top_k: 5,
            cursor: None,
        }),
    };
    let result: SearchPlaneQueryIpcResponseEnvelope = quanta_index_ipc::send_request(
        &sockets[0],
        &query,
        quanta_index_ipc::ClientIoPolicy::default(),
    )?;
    if result.request_id != request_id
        || !matches!(result.payload, SearchPlaneQueryIpcResponse::Text(_))
    {
        return Err(format!("first binary query did not serve: {result:?}").into());
    }
    let before = first_client
        .observability()
        .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
    if !before.events.iter().any(|event| {
        event.request_id.get() == request_id
            && event.stage == ProcessRequestEventStageV1::BackendOutcome
    }) {
        return Err("first process did not record the query outcome".into());
    }
    drop(first_client);
    first.stop()?;

    let second = SearchdBinaryProcess::start(&state_root)?;
    let outcome = (|| -> TestResult {
        let client = second.connect()?;
        let after = client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
        if after.process_instance == before.process_instance {
            return Err("restart reused the prior request-event process identity".into());
        }
        if after
            .events
            .iter()
            .any(|event| event.request_id.get() == request_id)
        {
            return Err("restart disclosed an event from the prior process window".into());
        }
        if after.dropped_before > after.dropped_after || after.next_sequence == 0 {
            return Err("new process window has invalid loss or sequence bounds".into());
        }
        Ok(())
    })();
    let stopped = second.stop();
    outcome.and(stopped)
}

#[test]
fn binary_query_ring_reports_wrap_loss_and_retains_the_latest_request() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let mut prepared = E2eRuntime::boot_in(&state_root)?;
    prepared.ingest_text("repo-ring-wrap", "src/wrap.rs", "needle ring wrap")?;
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
        let socket = &daemon_socket_paths(&state_root)[0];
        let first_id = 0x7100_u64;
        let request_count = 300_u64;
        for offset in 0..request_count {
            let request_id = first_id + offset;
            let request = SearchPlaneQueryIpcRequestEnvelope {
                request_id,
                payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "needle".to_owned(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    top_k: 1,
                    cursor: None,
                }),
            };
            let response: SearchPlaneQueryIpcResponseEnvelope = quanta_index_ipc::send_request(
                socket,
                &request,
                quanta_index_ipc::ClientIoPolicy::default(),
            )?;
            if response.request_id != request_id
                || !matches!(response.payload, SearchPlaneQueryIpcResponse::Text(_))
            {
                return Err(format!("ring wrap query {request_id} failed: {response:?}").into());
            }
        }
        let after = wait_for(
            &RealTicker::new(),
            Duration::from_secs(5),
            Duration::from_millis(10),
            "latest query terminal event",
            || {
                client
                    .observability()
                    .request_events(ProcessRequestEventPlaneV1::Query, 1024)
            },
            |window| {
                window.events.iter().any(|event| {
                    event.request_id.get() == first_id + request_count - 1
                        && event.stage == ProcessRequestEventStageV1::ResponseWritten
                })
            },
            |_| false,
        )?;
        require_eq(
            &after.process_instance,
            &before.process_instance,
            "ring wrap process instance",
        )?;
        if after.dropped_before <= before.dropped_after
            || after.dropped_after < after.dropped_before
            || after
                .oldest_retained_sequence
                .is_none_or(|oldest| oldest <= before.next_sequence)
            || after.next_sequence <= after.oldest_retained_sequence.unwrap_or(0)
        {
            return Err(format!("ring wrap did not disclose bounded loss: {after:?}").into());
        }
        let last_id = first_id + request_count - 1;
        if after
            .events
            .iter()
            .any(|event| event.request_id.get() == first_id)
            || !after.events.iter().any(|event| {
                event.request_id.get() == last_id
                    && event.stage == ProcessRequestEventStageV1::ResponseWritten
            })
        {
            return Err("ring wrap retained an evicted request or lost the latest terminal".into());
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
fn binary_daemon_counts_inventory_read_failure_and_reopens_after_repair() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let mut prepared = E2eRuntime::boot_in(&state_root)?;
    prepared.ingest_text("repo-binary-inventory", "src/ready.rs", "needle inventory")?;
    let sealed = prepared.seal()?;
    prepared.activate_last_sealed_generation()?;
    let repo = prepared.repo();
    let revision = prepared.revision();
    let pin = GenerationPin::new(repo.clone(), revision.clone(), sealed);
    prepared.stop()?;

    let canonical = std::fs::canonicalize(&state_root)?;
    let identity = GenerationStorageKeyV1::for_repo_revision(&repo, &revision)
        .generation_dir(&canonical.join("indexes/lexical"), sealed)
        .join("search-corpus-generation-identity.cbor");
    let process = SearchdBinaryProcess::start(&state_root)?;
    let outcome =
        (|| -> TestResult {
            let client = process.connect()?;
            let sockets = daemon_socket_paths(&state_root);
            let query = |request_id| -> TestResult {
                let request = SearchPlaneQueryIpcRequestEnvelope {
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
                let response: SearchPlaneQueryIpcResponseEnvelope = quanta_index_ipc::send_request(
                    &sockets[0],
                    &request,
                    quanta_index_ipc::ClientIoPolicy::default(),
                )?;
                if !matches!(response.payload, SearchPlaneQueryIpcResponse::Text(_)) {
                    return Err(
                        format!("binary inventory query returned {:?}", response.payload).into(),
                    );
                }
                Ok(())
            };
            query(0x1a11)?;
            let before = client.observability().metrics_snapshot()?;
            let metric = "maintenance_inventory_admission_failures_total";
            let initial = counter(&before, metric).ok_or("inventory failure metric absent")?;
            if !before.gauges.iter().any(|point| {
                point.name == "snapshot_registry_lexical_entries" && point.value >= 1.0
            }) {
                return Err("lexical query did not leave a resident handle to revalidate".into());
            }

            let permissions = std::fs::metadata(&identity)?.permissions();
            std::fs::set_permissions(&identity, std::fs::Permissions::from_mode(0o000))?;
            let denied = (|| -> TestResult {
                let after = wait_for(
                    &RealTicker::new(),
                    Duration::from_secs(20),
                    Duration::from_millis(50),
                    "binary inventory read failure counter",
                    || client.observability().metrics_snapshot(),
                    |snapshot| counter(snapshot, metric).is_some_and(|value| value > initial),
                    |_| true,
                )?;
                if counter(&after, metric).is_none_or(|value| value <= initial) {
                    return Err("inventory read failure was not counted".into());
                }
                Ok(())
            })();
            std::fs::set_permissions(&identity, permissions)?;
            denied?;
            query(0x1a12)
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
#[test]
fn binary_daemon_restart_preserves_active_source_and_ranked_rows() -> TestResult {
    use sha2::Digest as _;

    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let source_path = "src/process_restart.rs";
    let source_text = "fn process_restart_needle() {}";
    let source_sha256: [u8; 32] =
        sha2::Sha256::digest(format!("{source_text}\n").as_bytes()).into();
    let mut prepared = E2eRuntime::boot_in(&state_root)?;
    let candidate_id =
        prepared.ingest_text_with_candidate_id("repo-process-restart", source_path, source_text)?;
    let sealed = prepared.seal()?;
    prepared.activate_last_sealed_generation()?;
    let pin = GenerationPin::new(prepared.repo(), prepared.revision(), sealed);
    prepared.stop()?;

    let observe =
        |process: &SearchdBinaryProcess,
         request_id: u64|
         -> Result<(String, Vec<quanta_index_contract::LexicalCandidate>), Box<dyn Error>> {
            let client = process.connect()?;
            let ready = wait_for(
                &RealTicker::new(),
                Duration::from_secs(20),
                Duration::from_millis(50),
                "binary daemon ready after process start",
                || client.observability().process_readiness(),
                |report| report.ready,
                |_| true,
            )?;
            require_eq(&ready.active_repositories, &1, "active repositories")?;
            let request = SearchPlaneQueryIpcRequestEnvelope {
                request_id,
                payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "process_restart_needle".to_owned(),
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
                &request,
                quanta_index_ipc::ClientIoPolicy::default(),
            )?;
            require_eq(
                &response.request_id,
                &request_id,
                "query response request ID",
            )?;
            let SearchPlaneQueryIpcResponse::Text(page) = response.payload else {
                return Err(format!("binary restart query returned {:?}", response.payload).into());
            };
            require_eq(&page.generation, &pin, "active query generation")?;
            if page.results.len() != 1 {
                return Err(format!("expected one source-backed row: {:?}", page.results).into());
            }
            let row = &page.results[0];
            require_eq(&row.candidate_id, &candidate_id, "source candidate ID")?;
            require_eq(&row.source_repo_id, &pin.repo_id, "source repository")?;
            require_eq(
                &row.repo_relative_path.as_str(),
                &source_path,
                "source relative path",
            )?;
            let source = row.source.as_ref().ok_or("missing source revision")?;
            require_eq(
                &source.source_sha256,
                &source_sha256,
                "source bytes SHA-256",
            )?;
            let events = client
                .observability()
                .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
            if !events.events.iter().any(|event| {
                event.request_id.get() == request_id
                    && event.stage == ProcessRequestEventStageV1::ResponseWritten
            }) {
                return Err("binary restart query has no terminal response event".into());
            }
            Ok((events.process_instance, page.results))
        };

    let first = SearchdBinaryProcess::start(&state_root)?;
    let first_observed = observe(&first, 0x5eed_01);
    let first_stopped = first.stop();
    let (first_instance, first_rows) = first_observed?;
    first_stopped?;

    let second = SearchdBinaryProcess::start(&state_root)?;
    let second_observed = observe(&second, 0x5eed_02);
    let second_stopped = second.stop();
    let (second_instance, second_rows) = second_observed?;
    second_stopped?;
    if first_instance == second_instance {
        return Err("binary process instance did not change after shutdown and restart".into());
    }
    require_eq(
        &second_rows,
        &first_rows,
        "ranked rows after binary restart",
    )
}
