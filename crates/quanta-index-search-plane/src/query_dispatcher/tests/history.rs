use std::sync::{Arc, RwLock};

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::{
    LqExpr, LqFilter, LqLeaf, LqType, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneQueryIpcResponse, UpsertCommit,
};
use quanta_index_core::{CoreError, RequestBudgetV1};

use crate::observability::BoundedQueryObsStore;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::errors::{
    ERR_HISTORY_GENERATION_NOT_READY, ERR_HISTORY_PRODUCER_UNAVAILABLE,
    ERR_HISTORY_SHARD_UNAVAILABLE,
};
use crate::query_dispatcher::routes::history::validate_history_query;
use crate::query_dispatcher::tests::support::common::{
    TestResult, assert_closed_obs_metrics, default_query_embedder, encode_cbor, ipc_error_from,
    manual_query, ready_ledger, ready_pin, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::history::{
    history_commit_record, history_dispatcher_with_ledger, history_query_request,
    ledger_with_history_ops,
};
use crate::query_dispatcher::tests::support::lexical::RejectLexicalOpener;
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapSnapshotPort;
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;
use crate::{Ledger, SnapshotRegistries, SnapshotRegistryPolicy};

#[test]
fn history_dispatch_success_emits_closed_obs_metrics() -> TestResult {
    let obs_sink = Arc::new(BoundedQueryObsStore::default());
    let commit_payload = encode_cbor(&history_commit_record())?;
    let dispatcher = SearchPlaneDispatcher::new_with_obs(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ledger_with_history_ops(vec![LexicalChannelOp::UpsertCommit(UpsertCommit {
            repo_id: RepoId::new("repo-map-ipc")
                .expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-map-ipc")
                .expect("static fixture ID satisfies canonical policy"),
            generation: ManifestGeneration::new(9),
            payload: commit_payload,
        })])?,
        test_activation_catalog()?,
        default_query_embedder(),
        obs_sink.clone(),
    );

    let response = dispatcher.dispatch(
        history_query_request("type:commit fix"),
        &RequestBudgetV1::unbounded(),
    );
    match response {
        SearchPlaneQueryIpcResponse::History(history) => {
            if history.generation != ready_pin()
                || history.commits.len() != 1
                || !history.diffs.is_empty()
            {
                return Err(format!("unexpected history response: {history:?}").into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected History response, got {other:?}").into());
        }
    }

    assert_closed_obs_metrics(
        &obs_sink,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
            "lq_route_history_examined_candidates_total",
            "lq_route_history_latency_ms",
            "lq_route_history_served_total",
        ],
    )
}

#[test]
fn history_dispatch_unavailable_emits_closed_obs_metric() -> TestResult {
    let obs_sink = Arc::new(BoundedQueryObsStore::default());
    let dispatcher = SearchPlaneDispatcher::new_with_obs(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
        default_query_embedder(),
        obs_sink.clone(),
    );

    let response = dispatcher.dispatch(
        history_query_request("type:commit fix"),
        &RequestBudgetV1::unbounded(),
    );
    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != ERR_HISTORY_PRODUCER_UNAVAILABLE {
        return Err(format!("expected {ERR_HISTORY_PRODUCER_UNAVAILABLE}, got {code}").into());
    }

    assert_closed_obs_metrics(
        &obs_sink,
        &[
            "lq_query_intake_total",
            "lq_typed_error_unavailable_total",
            "lq_route_history_latency_ms",
            "lq_route_history_errors_total",
        ],
    )
}

#[test]
fn history_dispatch_rejects_missing_type_with_invalid_request() -> TestResult {
    let dispatcher = history_dispatcher_with_ledger(ready_ledger())?;
    let response = dispatcher.dispatch(history_query_request("fix"), &RequestBudgetV1::unbounded());
    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest {
        return Err(format!("expected INVALID_REQUEST, got {code}").into());
    }
    if !message.contains("explicit `type:commit` or `type:diff` is required") {
        return Err(format!("unexpected missing-type rejection message: {message}").into());
    }
    Ok(())
}

#[test]
fn history_dispatch_rejects_commit_file_filter_with_invalid_request() -> TestResult {
    let dispatcher = history_dispatcher_with_ledger(ready_ledger())?;
    let response = dispatcher.dispatch(
        history_query_request("type:commit file:src/lib.rs fix"),
        &RequestBudgetV1::unbounded(),
    );
    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest {
        return Err(format!("expected INVALID_REQUEST, got {code}").into());
    }
    if !message.contains("`file:` and `diff.*` filters require `type:diff`") {
        return Err(format!("unexpected commit-file rejection message: {message}").into());
    }
    Ok(())
}

#[test]
fn history_dispatch_rejects_commit_diff_filter_with_invalid_request() -> TestResult {
    let dispatcher = history_dispatcher_with_ledger(ready_ledger())?;
    let response = dispatcher.dispatch(
        history_query_request("type:commit diff.added:history fix"),
        &RequestBudgetV1::unbounded(),
    );
    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest {
        return Err(format!("expected INVALID_REQUEST, got {code}").into());
    }
    if !message.contains("`file:` and `diff.*` filters require `type:diff`") {
        return Err(format!("unexpected commit-diff rejection message: {message}").into());
    }
    Ok(())
}

#[test]
fn history_dispatch_rejects_predicate_leaf_with_not_implemented() -> TestResult {
    let dispatcher = history_dispatcher_with_ledger(ready_ledger())?;
    let response = dispatcher.dispatch(
        history_query_request("type:commit file.contains('fix')"),
        &RequestBudgetV1::unbounded(),
    );
    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::NotImplemented {
        return Err(format!("expected NOT_IMPLEMENTED, got {code}").into());
    }
    if !message.contains("history: predicate leaves are not executable on this route") {
        return Err(format!("unexpected history predicate rejection message: {message}").into());
    }
    Ok(())
}

#[test]
fn history_validate_rejects_content_regex_leaf_upfront() -> TestResult {
    let query = manual_query(
        LqExpr::Leaf(LqLeaf::Keyword("fix".to_string())),
        vec![
            LqFilter::Type {
                kind: LqType::Commit,
            },
            LqFilter::Content {
                leaf: LqLeaf::Regex("fix".to_string()),
            },
        ],
    );
    match validate_history_query(&query) {
        Err(CoreError::NotImplemented(message))
            if message.contains("history: regex leaves are not executable") =>
        {
            Ok(())
        }
        other => Err(format!("expected regex content reject, got {other:?}").into()),
    }
}

#[test]
fn history_dispatch_maps_generation_not_ready_before_lexical_materialization() -> TestResult {
    let dispatcher = history_dispatcher_with_ledger(Arc::new(RwLock::new(Ledger::default())))?;

    let response = dispatcher.dispatch(
        history_query_request("type:commit fix"),
        &RequestBudgetV1::unbounded(),
    );

    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != ERR_HISTORY_GENERATION_NOT_READY {
        return Err(format!("expected {ERR_HISTORY_GENERATION_NOT_READY}, got {code}").into());
    }
    Ok(())
}

#[test]
fn history_dispatch_maps_producer_unavailable_after_lexical_ready() -> TestResult {
    let dispatcher = history_dispatcher_with_ledger(ready_ledger())?;

    let response = dispatcher.dispatch(
        history_query_request("type:commit fix"),
        &RequestBudgetV1::unbounded(),
    );

    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != ERR_HISTORY_PRODUCER_UNAVAILABLE {
        return Err(format!("expected {ERR_HISTORY_PRODUCER_UNAVAILABLE}, got {code}").into());
    }
    Ok(())
}

#[test]
fn history_dispatch_maps_shard_unavailable_for_missing_diff_shard() -> TestResult {
    let commit_payload = encode_cbor(&history_commit_record())?;
    let dispatcher = history_dispatcher_with_ledger(ledger_with_history_ops(vec![
        LexicalChannelOp::UpsertCommit(UpsertCommit {
            repo_id: RepoId::new("repo-map-ipc")
                .expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev-map-ipc")
                .expect("static fixture ID satisfies canonical policy"),
            generation: ManifestGeneration::new(9),
            payload: commit_payload,
        }),
    ])?)?;

    let response = dispatcher.dispatch(
        history_query_request("type:diff history"),
        &RequestBudgetV1::unbounded(),
    );

    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != ERR_HISTORY_SHARD_UNAVAILABLE {
        return Err(format!("expected {ERR_HISTORY_SHARD_UNAVAILABLE}, got {code}").into());
    }
    Ok(())
}
