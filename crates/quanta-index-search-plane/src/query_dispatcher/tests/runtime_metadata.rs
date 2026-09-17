use std::collections::BTreeSet;
use std::sync::Arc;

use quanta_index_contract::{
    ChunkId, LqExpr, LqFilter, LqLeaf, LqPredicateArg, ManifestGeneration, RepoId, RevisionId,
    RuntimeMetadataQueryRequest, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{CoreError, RequestBudgetV1};

use crate::observability::BoundedQueryObsStore;
use crate::query_dispatcher::errors::ERR_NOT_IMPLEMENTED;
use crate::query_dispatcher::routes::runtime_metadata::{
    runtime_generation_is_stale, runtime_seed_ids, validate_runtime_metadata_query,
};
use crate::query_dispatcher::tests::support::common::{
    TestResult, dispatcher_with_obs, ipc_error_from, manual_query, ready_ledger, ready_pin,
};
use crate::query_dispatcher::tests::support::lexical::RejectLexicalOpener;
use crate::query_dispatcher::tests::support::runtime_metadata::{
    ready_runtime_metadata_ledger, runtime_metadata_dispatcher_with_ledger, runtime_query_request,
};
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;

#[test]
fn runtime_metadata_dispatch_not_ready_emits_closed_obs_metric() -> TestResult {
    let obs_sink = Arc::new(BoundedQueryObsStore::default());
    let dispatcher = dispatcher_with_obs(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        obs_sink.clone(),
    )?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::RuntimeMetadata(RuntimeMetadataQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "changed:since=1970-01-01T00:00:00.010Z runtime".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 5,
            },
        }),
        &RequestBudgetV1::unbounded(),
    );
    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != "NOT_READY" {
        return Err(format!("expected NOT_READY, got {code}").into());
    }
    let names = obs_sink
        .snapshot()
        .into_iter()
        .map(|sample| sample.name.into_string())
        .collect::<Vec<_>>();
    let expected = vec![
        "lq_query_intake_total".to_string(),
        "lq_typed_error_not_ready_total".to_string(),
        "lq_route_runtime_metadata_latency_ms".to_string(),
        "lq_route_runtime_metadata_errors_total".to_string(),
    ];
    if names != expected {
        return Err(format!("unexpected runtime-metadata obs metric names: {names:?}").into());
    }
    let errors = obs_sink.errors();
    if !errors.is_empty() {
        return Err(format!("unexpected runtime-metadata obs errors: {errors:?}").into());
    }
    Ok(())
}

#[test]
fn runtime_metadata_dispatch_dirty_only_executes_like_dirty_yes() -> TestResult {
    let dispatcher =
        runtime_metadata_dispatcher_with_ledger(ready_runtime_metadata_ledger(100, 20))?;
    let response = dispatcher.dispatch(
        runtime_query_request(TextQuerySyntax::Native, "dirty:only todo"),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::RuntimeMetadata(response) = response else {
        return Err("expected RuntimeMetadata response".into());
    };
    let candidate_ids = response
        .results
        .into_iter()
        .map(|candidate| candidate.candidate_id)
        .collect::<Vec<_>>();
    if candidate_ids != ["chunk-dirty"] {
        return Err(format!("expected [\"chunk-dirty\"], got {candidate_ids:?}").into());
    }
    Ok(())
}

#[test]
fn runtime_metadata_dispatch_rejects_predicate_leaf_typed_error() -> TestResult {
    let dispatcher = runtime_metadata_dispatcher_with_ledger(ready_ledger())?;
    let response = dispatcher.dispatch(
        runtime_query_request(
            TextQuerySyntax::Native,
            "changed:since=1970-01-01T00:00:00.010Z file.contains('catalog_changed_needle')",
        ),
        &RequestBudgetV1::unbounded(),
    );
    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != ERR_NOT_IMPLEMENTED {
        return Err(format!("expected {ERR_NOT_IMPLEMENTED}, got {code}").into());
    }
    if !message.contains("runtime metadata: predicate leaves are not executable") {
        return Err(format!("unexpected predicate-leaf rejection message: {message}").into());
    }
    Ok(())
}

#[test]
fn runtime_metadata_validate_rejects_content_predicate_leaf_upfront() -> TestResult {
    let query = manual_query(
        LqExpr::Leaf(LqLeaf::Keyword("catalog".to_string())),
        vec![
            LqFilter::Changed {
                scope: "since=1970-01-01T00:00:00.010Z".to_string(),
            },
            LqFilter::Content {
                leaf: LqLeaf::Predicate {
                    name: "file.contains".to_string(),
                    args: vec![LqPredicateArg::RawString("catalog".to_string())],
                },
            },
        ],
    );
    match validate_runtime_metadata_query(&query) {
        Err(CoreError::NotImplemented(message))
            if message.contains("runtime metadata: predicate leaves are not executable") =>
        {
            Ok(())
        }
        other => Err(format!("expected predicate content reject, got {other:?}").into()),
    }
}

#[test]
fn runtime_generation_is_stale_requires_producer_head_ahead() -> TestResult {
    let ledger = ready_runtime_metadata_ledger(20, 20);
    let guard = ledger
        .read()
        .map_err(|err| format!("runtime metadata test ledger poisoned: {err}"))?;
    let runtime = guard
        .runtime_state(
            &RepoId::new("repo-map-ipc"),
            &RevisionId::new("rev-map-ipc"),
            ManifestGeneration::new(9),
        )
        .ok_or("missing runtime metadata state")?;
    let is_stale = runtime_generation_is_stale(runtime, 30)?;
    drop(guard);
    if is_stale {
        return Err(
            "stale relation unexpectedly matched when producer head did not advance".into(),
        );
    }
    Ok(())
}

#[test]
#[expect(
    clippy::significant_drop_tightening,
    reason = "guard borrows runtime and structural state used across the whole test"
)]
fn runtime_seed_ids_use_direct_catalog_sets() -> TestResult {
    let ledger = ready_runtime_metadata_ledger(100, 20);
    let guard = ledger
        .read()
        .map_err(|err| format!("runtime metadata test ledger poisoned: {err}"))?;
    let runtime = guard
        .runtime_state(
            &RepoId::new("repo-map-ipc"),
            &RevisionId::new("rev-map-ipc"),
            ManifestGeneration::new(9),
        )
        .ok_or("missing runtime metadata state")?;
    let structural = guard
        .structural_state(
            &RepoId::new("repo-map-ipc"),
            &RevisionId::new("rev-map-ipc"),
            ManifestGeneration::new(9),
        )
        .ok_or("missing structural state")?;

    let changed = runtime_seed_ids(
        &crate::lower_lexical_text_query(&TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "changed:since=1970-01-01T00:00:00.010Z catalog".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 5,
        })?,
        runtime,
        structural,
    )?;
    if changed != BTreeSet::from([ChunkId::new("chunk-changed")]) {
        return Err(format!("unexpected changed seed ids: {changed:?}").into());
    }

    let clean = runtime_seed_ids(
        &crate::lower_lexical_text_query(&TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "dirty:no todo".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 5,
        })?,
        runtime,
        structural,
    )?;
    if clean.contains(&ChunkId::new("chunk-dirty")) || !clean.contains(&ChunkId::new("chunk-clean"))
    {
        return Err(format!("unexpected clean-complement seed ids: {clean:?}").into());
    }

    let affected = runtime_seed_ids(
        &crate::lower_lexical_text_query(&TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "affected:rebuild=lexical catalog".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 5,
        })?,
        runtime,
        structural,
    )?;
    if affected != BTreeSet::from([ChunkId::new("chunk-changed")]) {
        return Err(format!("unexpected affected seed ids: {affected:?}").into());
    }
    Ok(())
}
