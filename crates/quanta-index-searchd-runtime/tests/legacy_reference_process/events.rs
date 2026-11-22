//! Pinned old-Q request-event compatibility tests.
//!
//! These exercise the Legacy public SDK/control and query sockets. They do not
//! qualify the Native daemon or grant its diagnostic routes Legacy semantics.

use std::error::Error;
use std::path::Path;
use std::time::Duration;

use quanta_index_contract::{
    GenerationPin, ManifestGeneration, ProcessRequestEventPlaneV1, ProcessRequestEventStageV1,
    ProcessRequestEventsRequestV1, ProcessRequestEventsV1, QueryConstraintSetV1, RepoId,
    RevisionId, SearchPlaneControlIpcRequest, SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneControlIpcResponse, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_sdk::QuantaIndex;
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, daemon_socket_paths, publish_legacy_source,
};

use crate::fail_closed_wait::{RealTicker, wait_for};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn require_eq<T: std::fmt::Debug + PartialEq>(actual: &T, expected: &T, field: &str) -> TestResult {
    if actual == expected {
        Ok(())
    } else {
        Err(format!("{field}: expected {expected:?}, got {actual:?}").into())
    }
}

fn publish_fixture(client: &QuantaIndex, path: &str, body: &str) -> TestResult<GenerationPin> {
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let (_head, receipt) =
        publish_legacy_source(client, &repo, &revision, 1, None, &[(path, body, 1)], None)?;
    if !receipt.applied || receipt.durable_sequence == 0 {
        return Err("old-Q fixture publication lacks a durable receipt".into());
    }
    Ok(GenerationPin::new(
        repo,
        revision,
        ManifestGeneration::new(1),
    ))
}

fn with_published_reference(
    path: &str,
    body: &str,
    test: impl FnOnce(&QuantaIndex, &Path, &GenerationPin) -> TestResult,
) -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let pin = publish_fixture(&client, path, body)?;
        test(&client, &state_root, &pin)
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}

fn with_reopened_reference(
    path: &str,
    body: &str,
    test: impl FnOnce(&QuantaIndex, &Path, &GenerationPin) -> TestResult,
) -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let preparation = binary.start(&state_root, 8)?;
    let published = (|| -> TestResult<GenerationPin> {
        let client = preparation.connect()?;
        publish_fixture(&client, path, body)
    })();
    let preparation_stopped = preparation.stop();
    let pin = published?;
    preparation_stopped?;
    binary.verify()?;

    // The observed ring and search backend belong to a fresh OS process,
    // reopened from the generation published by the preparation process.
    let process = binary.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        test(&client, &state_root, &pin)
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}

fn text_query(
    socket: &Path,
    request_id: u64,
    pin: &GenerationPin,
    top_k: u32,
) -> TestResult<SearchPlaneQueryIpcResponseEnvelope> {
    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id,
        diagnostic_attempt: quanta_index_contract::ipc::DiagnosticAttemptFieldV1::Absent,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".to_owned(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k,
            cursor: None,
        }),
    };
    Ok(quanta_index_ipc::send_request(
        socket,
        &request,
        quanta_index_ipc::ClientIoPolicy::default(),
    )?)
}

fn semantic_query(
    socket: &Path,
    request_id: u64,
    pin: &GenerationPin,
) -> TestResult<SearchPlaneQueryIpcResponseEnvelope> {
    let request = SearchPlaneQueryIpcRequestEnvelope {
        request_id,
        diagnostic_attempt: quanta_index_contract::ipc::DiagnosticAttemptFieldV1::Absent,
        payload: SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "needle".to_owned(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: None,
            top_k: 5,
        }),
    };
    Ok(quanta_index_ipc::send_request(
        socket,
        &request,
        quanta_index_ipc::ClientIoPolicy::default(),
    )?)
}

fn ingest_failure_event_tail(control_socket: &Path) -> TestResult<ProcessRequestEventsV1> {
    let request = SearchPlaneControlIpcRequestEnvelope {
        request_id: u64::MAX,
        diagnostic_attempt: quanta_index_contract::ipc::DiagnosticAttemptFieldV1::Absent,
        payload: SearchPlaneControlIpcRequest::ProcessRequestEventsV1(
            ProcessRequestEventsRequestV1 {
                plane: ProcessRequestEventPlaneV1::Ingest,
                limit: 64,
            },
        ),
    };
    let policy = quanta_index_ipc::ClientIoPolicy::try_new(Duration::from_secs(5))?;
    let response: SearchPlaneControlIpcResponseEnvelope =
        quanta_index_ipc::send_request(control_socket, &request, policy)?;
    require_eq(
        &response.request_id,
        &request.request_id,
        "failure event request ID",
    )?;
    let SearchPlaneControlIpcResponse::ProcessRequestEventsV1(events) = response.payload else {
        return Err(format!(
            "failure event request returned wrong response: {:?}",
            response.payload
        )
        .into());
    };
    require_eq(
        &events.plane,
        &ProcessRequestEventPlaneV1::Ingest,
        "failure event plane",
    )?;
    Ok(events)
}

#[test]
fn operator_ring_correlates_queue_backend_and_terminal_by_request_id() -> TestResult {
    with_published_reference("src/events.rs", "needle events", |client, root, pin| {
        let before = client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
        let ingest_tail = ingest_failure_event_tail(&daemon_socket_paths(root)[1])?;
        require_eq(
            &ingest_tail.process_instance,
            &before.process_instance,
            "failure diagnostic process instance",
        )?;
        if ingest_tail.events.len() > 64
            || !ingest_tail
                .events
                .iter()
                .any(|event| event.stage == ProcessRequestEventStageV1::ResponseWritten)
        {
            return Err("bounded failure diagnostic lacks an ingest terminal response".into());
        }
        let request_id = 0x5ea0_u64;
        let served = text_query(&daemon_socket_paths(root)[0], request_id, pin, 5)?;
        require_eq(&served.request_id, &request_id, "fixture query request ID")?;
        if !matches!(&served.payload, SearchPlaneQueryIpcResponse::Text(_)) {
            return Err(format!("fixture query failed: {:?}", served.payload).into());
        }
        let after = client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
        if after.process_instance != before.process_instance {
            return Err("query ring process instance changed during the probe".into());
        }
        let outcome = after.events.iter().find(|event| {
            event.sequence >= before.next_sequence
                && event.request_id.get() == request_id
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
    })
}

#[test]
fn operator_ring_correlates_provider_ticket_with_query_terminal() -> TestResult {
    with_published_reference("src/provider.rs", "needle provider", |client, root, pin| {
        let before = client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
        let request_id = 0x5ea1_u64;
        let served = semantic_query(&daemon_socket_paths(root)[0], request_id, pin)?;
        require_eq(&served.request_id, &request_id, "semantic query request ID")?;
        if !matches!(&served.payload, SearchPlaneQueryIpcResponse::Semantic(_)) {
            return Err(format!("semantic fixture query failed: {:?}", served.payload).into());
        }
        let after = client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
        let Some(outcome) = after.events.iter().find(|event| {
            event.sequence >= before.next_sequence
                && event.request_id.get() == request_id
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
    })
}

#[test]
fn binary_daemon_exposes_one_correlated_query_without_payload() -> TestResult {
    with_reopened_reference("src/events.rs", "needle events", |client, root, pin| {
        let before = client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
        let sockets = daemon_socket_paths(root);
        let request_id = 0x5eed_u64;
        let response = text_query(&sockets[0], request_id, pin, 5)?;
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
        let semantic_response = semantic_query(&sockets[0], semantic_id, pin)?;
        require_eq(
            &semantic_response.request_id,
            &semantic_id,
            "semantic response request ID",
        )?;
        if !matches!(
            &semantic_response.payload,
            SearchPlaneQueryIpcResponse::Semantic(_)
        ) {
            return Err(format!(
                "binary semantic query refused: {:?}",
                semantic_response.payload
            )
            .into());
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
    })
}

#[test]
fn binary_restart_replaces_request_event_instance_and_discards_prior_window() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let first = binary.start(&state_root, 8)?;
    let client = first.connect()?;
    let pin = publish_fixture(&client, "src/restart.rs", "needle restart")?;
    let request_id = 0x5ee1_u64;
    let query = text_query(&daemon_socket_paths(&state_root)[0], request_id, &pin, 5)?;
    if query.request_id != request_id
        || !matches!(&query.payload, SearchPlaneQueryIpcResponse::Text(_))
    {
        return Err(format!("first binary query did not serve: {query:?}").into());
    }
    let before = client
        .observability()
        .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
    if !before.events.iter().any(|event| {
        event.request_id.get() == request_id
            && event.stage == ProcessRequestEventStageV1::BackendOutcome
    }) {
        return Err("first process did not record the query outcome".into());
    }
    drop(client);
    first.stop()?;

    let second = binary.start(&state_root, 8)?;
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
    outcome.and(stopped)?;
    binary.verify()
}

#[test]
fn binary_query_ring_reports_wrap_loss_and_retains_the_latest_request() -> TestResult {
    with_reopened_reference("src/wrap.rs", "needle ring wrap", |client, root, pin| {
        let before = client
            .observability()
            .request_events(ProcessRequestEventPlaneV1::Query, 1024)?;
        let socket = &daemon_socket_paths(root)[0];
        let first_id = 0x7100_u64;
        let request_count = 300_u64;
        for offset in 0..request_count {
            let request_id = first_id + offset;
            let response = text_query(socket, request_id, pin, 1)?;
            if response.request_id != request_id
                || !matches!(&response.payload, SearchPlaneQueryIpcResponse::Text(_))
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
        let previous_dropped = before.dropped_after;
        if after.dropped_before <= previous_dropped
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
    })
}
