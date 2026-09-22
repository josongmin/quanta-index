use std::sync::{Arc, Mutex};

use quanta_index_contract::{ClusterMembershipBatchReadRequestV1, ManifestGeneration};
use quanta_index_core::{CoreError, RequestBudgetV1};

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::tests::support::common::{
    TestResult, ready_ledger, ready_pin, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::RejectLexicalOpener;
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapQueryPort;
use crate::query_dispatcher::tests::support::semantic::{
    RecordingSemanticOpener, RecordingSemanticState,
    available_cluster_membership_batch_response_v1, cluster_membership_batch_request_v1,
};
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

#[test]
fn cluster_membership_dispatch_rejects_invalid_request_before_semantic_open_v1() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&state),
        }),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );
    let request = ClusterMembershipBatchReadRequestV1 {
        generation: ready_pin(),
        items: Vec::new(),
    };

    match dispatcher.cluster_membership_batch_read(&request, &RequestBudgetV1::unbounded()) {
        Err(CoreError::InvalidContract(message)) if message.contains("must not be empty") => {}
        other => {
            return Err(format!("expected invalid-contract rejection, got {other:?}").into());
        }
    }

    let reached_storage = {
        let guard = state
            .lock()
            .map_err(|err| format!("semantic state poisoned: {err}"))?;
        !guard.cluster_membership_opened_pins.is_empty()
            || !guard.cluster_membership_requests.is_empty()
    };
    if reached_storage {
        return Err("invalid membership request reached semantic storage".into());
    }
    Ok(())
}

#[test]
fn cluster_membership_dispatch_opens_one_pinned_generation_and_preserves_authority_v1() -> TestResult
{
    let request = cluster_membership_batch_request_v1();
    let expected = available_cluster_membership_batch_response_v1(&request);
    let state = Arc::new(Mutex::new(RecordingSemanticState {
        cluster_membership_response: Some(expected.clone()),
        ..RecordingSemanticState::default()
    }));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&state),
        }),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );

    let observed =
        dispatcher.cluster_membership_batch_read(&request, &RequestBudgetV1::unbounded())?;
    if observed != expected {
        return Err(format!("membership authority drifted: {observed:?}").into());
    }

    let (opened_pins, recorded_requests) = {
        let guard = state
            .lock()
            .map_err(|err| format!("semantic state poisoned: {err}"))?;
        (
            guard.cluster_membership_opened_pins.clone(),
            guard.cluster_membership_requests.clone(),
        )
    };
    if opened_pins.as_slice()
        != [(
            request.generation.repo_id.clone(),
            request.generation.revision_id.clone(),
            request.generation.manifest_generation,
        )]
    {
        return Err(
            format!("membership read must open its exact pin once: {opened_pins:?}").into(),
        );
    }
    if recorded_requests.as_slice() != [request] {
        return Err(format!(
            "membership request must reach the searcher exactly once: {recorded_requests:?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn cluster_membership_dispatch_rejects_forged_or_stale_searcher_authority_v1() -> TestResult {
    let request = cluster_membership_batch_request_v1();
    let mut forged_response = available_cluster_membership_batch_response_v1(&request);
    let Some(quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(snapshot)) =
        forged_response.outcomes.first_mut()
    else {
        return Err("fixture must contain one available membership".into());
    };
    snapshot.authority_digest = "forged-authority".to_string();

    let mut stale_response = available_cluster_membership_batch_response_v1(&request);
    let Some(quanta_index_contract::ClusterMembershipReadOutcomeV1::Available(snapshot)) =
        stale_response.outcomes.first_mut()
    else {
        return Err("fixture must contain one available membership".into());
    };
    snapshot.generation.manifest_generation = ManifestGeneration::new(8);

    for (case, response) in [("forged", forged_response), ("stale", stale_response)] {
        let state = Arc::new(Mutex::new(RecordingSemanticState {
            cluster_membership_response: Some(response),
            ..RecordingSemanticState::default()
        }));
        let dispatcher = SearchPlaneDispatcher::new(
            Arc::new(RejectLexicalOpener),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&state),
            }),
            Arc::new(StubRepoMapQueryPort),
            Arc::new(FailClosedStructuralProducer),
            ready_ledger(),
            test_activation_catalog()?,
        );

        match dispatcher.cluster_membership_batch_read(&request, &RequestBudgetV1::unbounded()) {
            Err(CoreError::InvalidContract(message)) if message.contains("invalid authority") => {}
            other => {
                return Err(format!(
                    "{case} membership authority must fail InvalidContract, got {other:?}"
                )
                .into());
            }
        }
        let guard = state
            .lock()
            .map_err(|err| format!("semantic state poisoned: {err}"))?;
        if guard.cluster_membership_opened_pins.len() != 1
            || guard.cluster_membership_requests.as_slice() != [request.clone()]
        {
            return Err(format!(
                "{case} authority must be rejected after exactly one storage read"
            )
            .into());
        }
    }
    Ok(())
}
