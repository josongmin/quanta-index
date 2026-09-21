use std::sync::{Arc, RwLock};

use quanta_index_contract::{SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse};
use quanta_index_core::RequestBudgetV1;

use crate::observability::BoundedQueryObsStore;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::tests::support::common::{
    TestResult, default_query_embedder, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::RejectLexicalOpener;
use crate::query_dispatcher::tests::support::repo_map::{
    StubRepoMapQueryPort, into_repo_map_query_response, repo_map_request,
};
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;
use crate::{Ledger, SnapshotRegistries, SnapshotRegistryPolicy};

#[test]
fn repo_map_dispatcher_branch_delegates_to_repo_map_query_port() -> TestResult {
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        Arc::new(RwLock::new(Ledger::default())),
        test_activation_catalog()?,
    );

    let response = into_repo_map_query_response(dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::RepoMapQuery(repo_map_request()),
        &RequestBudgetV1::unbounded(),
    ))?;

    if response.repo_id.as_str() != "repo-map-ipc" {
        return Err(format!("unexpected repo id: {}", response.repo_id.as_str()).into());
    }
    if response.revision_id.as_str() != "rev-map-ipc" {
        return Err(format!("unexpected revision id: {}", response.revision_id.as_str()).into());
    }
    if response.manifest_generation.get() != 9 {
        return Err(format!(
            "unexpected manifest generation: {}",
            response.manifest_generation.get()
        )
        .into());
    }
    if response.snapshot_meta.snapshot_id != "dispatch-snapshot" {
        return Err(
            format!("unexpected snapshot id: {}", response.snapshot_meta.snapshot_id).into()
        );
    }
    if response.entries.len() != 1 {
        return Err(format!("unexpected entry count: {}", response.entries.len()).into());
    }
    let first_entry = response
        .entries
        .first()
        .ok_or_else(|| "expected one repo-map entry".to_string())?;
    if first_entry.owner_path != "src/lib.rs" {
        return Err(format!("unexpected owner path: {}", first_entry.owner_path).into());
    }
    Ok(())
}

#[test]
fn repo_map_dispatch_emits_closed_obs_metrics() -> TestResult {
    let obs_sink = Arc::new(BoundedQueryObsStore::default());
    let dispatcher = SearchPlaneDispatcher::new_with_obs(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        Arc::new(RwLock::new(Ledger::default())),
        test_activation_catalog()?,
        default_query_embedder(),
        obs_sink.clone(),
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::RepoMapQuery(repo_map_request()),
        &RequestBudgetV1::unbounded(),
    );
    match response {
        SearchPlaneQueryIpcResponse::RepoMapQuery(_) => {}
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected RepoMapQuery response, got {other:?}").into());
        }
    }

    let names = obs_sink
        .snapshot()
        .into_iter()
        .map(|sample| sample.name.into_string())
        .collect::<Vec<_>>();
    let expected = vec![
        "lq_query_intake_total".to_string(),
        "lq_planner_total".to_string(),
        "lq_engine_fanout_count".to_string(),
        "lq_merge_result_count".to_string(),
        "lq_route_repo_map_latency_ms".to_string(),
        "lq_route_repo_map_served_total".to_string(),
    ];
    if names != expected {
        return Err(format!("unexpected repo-map obs metric names: {names:?}").into());
    }
    let errors = obs_sink.errors();
    if !errors.is_empty() {
        return Err(format!("unexpected repo-map obs errors: {errors:?}").into());
    }
    let samples = obs_sink.snapshot();
    for sample in &samples {
        if sample.dimensions.ticket_id.as_ref() != "LXE-10"
            || sample.dimensions.wave_id.as_ref() != "8"
            || sample.dimensions.tenant_id.as_ref() != "local"
            || sample.dimensions.repo_id.as_ref() != "repo-map-ipc"
            || sample.dimensions.generation_id != 9
        {
            return Err(
                format!("unexpected repo-map obs dimensions: {:?}", sample.dimensions).into()
            );
        }
    }
    Ok(())
}
