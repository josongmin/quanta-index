// ------------------------------------------------------------------
// LXE-02 / LXE-09 wiring tests.
//
// These exercise the new planner short-circuit and the structural
// domain-port routing path. They are additive — existing dispatcher
// tests remain unchanged.
// ------------------------------------------------------------------

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use quanta_index_contract::lex::{SymbolKindCode, SymbolKindFamily};
use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoRelativePath, RevisionId, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SymbolCandidate, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::RequestBudgetV1;
use quanta_index_core::domains::structural::{StructuralError, StructuralReadiness};

use crate::observability::BoundedQueryObsStore;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::routes::structural::lexical_leaves::symbol_hits_to_structural_buckets;
use crate::query_dispatcher::tests::support::common::{
    TestResult, assert_closed_obs_metrics, default_query_embedder, ipc_error_from, ready_pin,
    test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState, RejectLexicalOpener, recording_lexical_candidate,
};
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapQueryPort;
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;
use crate::query_dispatcher::tests::support::structural::{
    PatternRoutingStructuralProducer, RecordingStructuralProducer,
    ready_ledger_with_structural_boolean_chunks, ready_ledger_with_structural_universe,
    structural_dispatcher_mixed, structural_dispatcher_with_producer,
    structural_dispatcher_with_producer_and_ledger, structural_match_candidate,
    structural_state_for_test_chunks,
};
use crate::{SnapshotRegistries, SnapshotRegistryPolicy};

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "`assert!`/`assert_eq!` invariant checks in a Result-returning test; a failed assertion is the intended test failure"
)]
fn symbol_hits_project_into_all_overlapping_chunks_deterministically()
-> Result<(), Box<dyn std::error::Error>> {
    let structural_state = structural_state_for_test_chunks(&[
        (
            "chunk-symbol-left",
            "src/symbol.rs",
            "fn ParityTypeSymbol() {}",
        ),
        (
            "chunk-symbol-right",
            "src/symbol.rs",
            "fn ParityTypeSymbol() {}",
        ),
    ])?;
    let buckets = symbol_hits_to_structural_buckets(
        vec![SymbolCandidate {
            candidate_id: "symbol-hit-1".to_string(),
            repo_id: RepoId::new("repo-map-ipc"),
            revision_id: RevisionId::new("rev-map-ipc"),
            manifest_generation: ManifestGeneration::new(9),
            repo_relative_path: RepoRelativePath::new("src/symbol.rs"),
            start_line: 1,
            end_line: 1,
            score: 1.0,
            snippet: "ParityTypeSymbol".to_string(),
            symbol_kind: SymbolKindCode::from_code_str("function")
                .expect("function symbol kind code"),
            symbol_kind_family: Some(SymbolKindFamily::Callable),
        }],
        &structural_state,
    );
    let projected_ids = buckets.keys().cloned().collect::<Vec<_>>();
    assert_eq!(
        projected_ids,
        vec![
            "chunk-symbol-left".to_string(),
            "chunk-symbol-right".to_string(),
        ]
    );
    for bucket in buckets.values() {
        assert_eq!(
            bucket.len(),
            1,
            "each overlapping chunk must receive exactly one projected structural bucket"
        );
    }
    Ok(())
}

#[test]
fn structural_dispatch_success_emits_closed_obs_metrics() -> TestResult {
    let obs_sink = Arc::new(BoundedQueryObsStore::default());
    let dispatcher = SearchPlaneDispatcher::new_with_obs(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(RecordingStructuralProducer::ready_with(vec![
            structural_match_candidate("chunk-tree"),
        ])),
        ready_ledger_with_structural_universe(),
        test_activation_catalog()?,
        default_query_embedder(),
        obs_sink.clone(),
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { :[x] }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 4,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );
    match response {
        SearchPlaneQueryIpcResponse::Structural(results) => {
            if results.generation != ready_pin() || results.results.len() != 1 {
                return Err(format!("unexpected structural response: {results:?}").into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Structural response, got {other:?}").into());
        }
    }

    assert_closed_obs_metrics(
        &obs_sink,
        &[
            "lq_query_intake_total",
            "lq_planner_total",
            "lq_engine_fanout_count",
            "lq_merge_result_count",
            "lq_route_structural_latency_ms",
            "lq_route_structural_served_total",
        ],
    )
}

#[test]
fn structural_dispatch_routes_happy_path_through_structural_service() -> TestResult {
    let producer = Arc::new(RecordingStructuralProducer::ready_with(vec![
        structural_match_candidate("chunk-tree"),
    ]));
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { :[x] }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 4,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Structural(results) => {
            if results.generation != ready_pin() {
                return Err(format!(
                    "expected structural generation {:?}, got {:?}",
                    ready_pin(),
                    results.generation
                )
                .into());
            }
            if results.results.len() != 1 {
                return Err(format!(
                    "expected 1 structural candidate, got {}",
                    results.results.len()
                )
                .into());
            }
            let Some(first) = results.results.first() else {
                return Err("expected structural results to contain one candidate".into());
            };
            if first.candidate_id != "chunk-tree" {
                return Err(format!("expected candidate_id=chunk-tree, got {first:?}").into());
            }
            // The page names the structural epoch it pinned (QI-BB-020
            // W2): one chunk install, so epoch 1 — and the producer executed
            // the leaf against that same epoch.
            if results.read_epoch != quanta_index_contract::AuxEpochV1::new(1) {
                return Err(format!(
                    "the structural page reads epoch 1, got {:?}",
                    results.read_epoch
                )
                .into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Structural response, got {other:?}").into());
        }
    }
    let consulted = producer.readiness_calls.load(Ordering::SeqCst);
    if consulted != 1 {
        return Err(format!(
            "expected structural readiness to be consulted exactly once, got {consulted} call(s)"
        )
        .into());
    }
    let executed = producer.execute_calls.load(Ordering::SeqCst);
    if executed != 1 {
        return Err(format!(
            "expected structural execute to be consulted exactly once, got {executed} call(s)"
        )
        .into());
    }
    let executed_epochs = producer
        .executed_epochs
        .lock()
        .map_err(|err| format!("recorder poisoned: {err}"))?
        .clone();
    if executed_epochs != vec![quanta_index_contract::AuxEpochV1::new(1)] {
        return Err(format!(
            "the leaf executes at the epoch the route pinned, got {executed_epochs:?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_maps_generation_not_ready() -> TestResult {
    let producer = Arc::new(RecordingStructuralProducer::new(
        StructuralReadiness::GenerationNotReady,
    ));
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { :[x] }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 4,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != "STR_GENERATION_NOT_READY" {
        return Err(format!("expected STR_GENERATION_NOT_READY, got {code}").into());
    }
    if producer.execute_calls.load(Ordering::SeqCst) != 0 {
        return Err("execute must not run when readiness is GenerationNotReady".into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_maps_shard_unavailable() -> TestResult {
    let producer = Arc::new(RecordingStructuralProducer::new(
        StructuralReadiness::ShardUnavailable,
    ));
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { :[x] }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 4,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != "STR_SHARD_UNAVAILABLE" {
        return Err(format!("expected STR_SHARD_UNAVAILABLE, got {code}").into());
    }
    if producer.execute_calls.load(Ordering::SeqCst) != 0 {
        return Err("execute must not run when readiness is ShardUnavailable".into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_maps_lang_not_supported() -> TestResult {
    let producer = Arc::new(RecordingStructuralProducer::ready_with_error(
        StructuralError::LangNotSupported("java".to_string()),
    ));
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "lang:java match { :[x] }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 4,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != "STR_LANG_NOT_SUPPORTED" {
        return Err(format!("expected STR_LANG_NOT_SUPPORTED, got {code}").into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_routes_repo_and_file_filters_to_producer() -> TestResult {
    let producer = Arc::new(RecordingStructuralProducer::ready_with(vec![
        structural_match_candidate("chunk-tree"),
    ]));
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "repo:repo-map-ipc file:src/lib.rs match { :[x] }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 4,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Structural(results) => {
            if results.generation != ready_pin() {
                return Err(format!(
                    "expected structural generation {:?}, got {:?}",
                    ready_pin(),
                    results.generation
                )
                .into());
            }
            if results.results.len() != 1 {
                return Err(format!(
                    "expected 1 structural candidate, got {}",
                    results.results.len()
                )
                .into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Structural response, got {other:?}").into());
        }
    }
    if producer.readiness_calls.load(Ordering::SeqCst) != 1 {
        return Err("producer readiness must run for executable structural filters".into());
    }
    if producer.execute_calls.load(Ordering::SeqCst) != 1 {
        return Err("producer execute must run for executable structural filters".into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_rejects_non_executable_filters_before_consulting_producer() -> TestResult {
    let producer = Arc::new(RecordingStructuralProducer::ready_with(vec![
        structural_match_candidate("chunk-tree"),
    ]));
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "select:repo match { :[x] }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 4,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != "STR_INVALID_REQUEST" {
        return Err(format!("expected STR_INVALID_REQUEST, got {code}").into());
    }
    if !message.contains("filter `select` is not executable") {
        return Err(format!("expected select-filter rejection message, got {message}").into());
    }
    if producer.readiness_calls.load(Ordering::SeqCst) != 0 {
        return Err("producer readiness must not run for invalid structural filters".into());
    }
    if producer.execute_calls.load(Ordering::SeqCst) != 0 {
        return Err("producer execute must not run for invalid structural filters".into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_routes_sourcegraph_structural_subset_to_producer() -> TestResult {
    let producer = Arc::new(RecordingStructuralProducer::ready_with(vec![
        structural_match_candidate("chunk-tree"),
    ]));
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

    let response = dispatcher.dispatch(SearchPlaneQueryIpcRequest::Structural(
        quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text:
                    r#"repo:repo-map-ipc path:src/lib.rs lang:rust patterntype:structural "function_item { { identifier :[x] } }""#
                        .to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 4,
            },
            cursor: None,
        },
    ), &RequestBudgetV1::unbounded());

    match response {
        SearchPlaneQueryIpcResponse::Structural(results) => {
            if results.generation != ready_pin() {
                return Err(format!(
                    "expected structural generation {:?}, got {:?}",
                    ready_pin(),
                    results.generation
                )
                .into());
            }
            if results.results.len() != 1 {
                return Err(format!(
                    "expected 1 structural candidate from SG structural route, got {}",
                    results.results.len()
                )
                .into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!(
                "expected Structural response for SG structural route, got {other:?}"
            )
            .into());
        }
    }
    if producer.readiness_calls.load(Ordering::SeqCst) != 1 {
        return Err("producer readiness must run for SG structural subset".into());
    }
    if producer.execute_calls.load(Ordering::SeqCst) != 1 {
        return Err("producer execute must run for SG structural subset".into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_rejects_typed_hole_kind_with_exact_code() -> TestResult {
    let producer = Arc::new(RecordingStructuralProducer::ready_with(vec![
        structural_match_candidate("chunk-tree"),
    ]));
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { function_item { { :[name.lambda] } } }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 4,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != "STR_HOLE_KIND_UNSUPPORTED" {
        return Err(format!("expected STR_HOLE_KIND_UNSUPPORTED, got {code}").into());
    }
    if !message.contains("typed hole kind `lambda`") {
        return Err(format!("expected typed-hole rejection message, got {message}").into());
    }
    if producer.readiness_calls.load(Ordering::SeqCst) != 0 {
        return Err("producer readiness must not run for typed-hole rejection".into());
    }
    if producer.execute_calls.load(Ordering::SeqCst) != 0 {
        return Err("producer execute must not run for typed-hole rejection".into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_executes_structural_boolean_and_with_canonical_projection() -> TestResult {
    let producer = Arc::new(PatternRoutingStructuralProducer::new());
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { alpha } AND match { beta }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 10,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Structural(results) => {
            if results.results.len() != 1 {
                return Err(format!(
                    "expected 1 structural candidate from boolean AND, got {}",
                    results.results.len()
                )
                .into());
            }
            let candidate = results
                .results
                .first()
                .ok_or_else(|| "missing structural candidate after size check".to_string())?;
            if candidate.candidate_id != "chunk-shared" {
                return Err(format!("expected chunk-shared, got {candidate:?}").into());
            }
            let start_bytes: Vec<u32> = candidate
                .bindings
                .iter()
                .map(|binding| binding.start_byte)
                .collect();
            if start_bytes != vec![5, 30] {
                return Err(format!(
                    "expected canonical merged bindings [5, 30], got {start_bytes:?}"
                )
                .into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Structural response, got {other:?}").into());
        }
    }

    if producer.readiness_calls.load(Ordering::SeqCst) != 2 {
        return Err("boolean AND should consult readiness once per structural leaf".into());
    }
    if producer.execute_calls.load(Ordering::SeqCst) != 2 {
        return Err("boolean AND should execute once per structural leaf".into());
    }
    let scopes = producer.recorded_scopes()?;
    if scopes
        != vec![
            None,
            Some(vec!["chunk-a".to_string(), "chunk-shared".to_string()]),
        ]
    {
        return Err(format!("unexpected AND candidate scopes: {scopes:?}").into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_executes_structural_boolean_or() -> TestResult {
    let producer = Arc::new(PatternRoutingStructuralProducer::new());
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { alpha } OR match { beta }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 10,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Structural(results) => {
            let ids: Vec<&str> = results
                .results
                .iter()
                .map(|candidate| candidate.candidate_id.as_str())
                .collect();
            if ids != vec!["chunk-a", "chunk-shared"] {
                return Err(format!("expected OR ids [chunk-a, chunk-shared], got {ids:?}").into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Structural response, got {other:?}").into());
        }
    }

    if producer.execute_calls.load(Ordering::SeqCst) != 2 {
        return Err("boolean OR should execute once per structural leaf".into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_executes_bounded_not() -> TestResult {
    let producer = Arc::new(PatternRoutingStructuralProducer::new());
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { alpha } AND NOT match { gamma }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 10,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Structural(results) => {
            let ids: Vec<&str> = results
                .results
                .iter()
                .map(|candidate| candidate.candidate_id.as_str())
                .collect();
            if ids != vec!["chunk-shared"] {
                return Err(format!(
                    "expected bounded NOT to retain only chunk-shared, got {ids:?}"
                )
                .into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Structural response, got {other:?}").into());
        }
    }

    if producer.execute_calls.load(Ordering::SeqCst) != 2 {
        return Err("bounded NOT should execute once per structural leaf".into());
    }
    let scopes = producer.recorded_scopes()?;
    if scopes
        != vec![
            None,
            Some(vec!["chunk-a".to_string(), "chunk-shared".to_string()]),
        ]
    {
        return Err(format!("unexpected bounded-NOT candidate scopes: {scopes:?}").into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_memoizes_identical_leaf_execution() -> TestResult {
    let producer = Arc::new(PatternRoutingStructuralProducer::new());
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "match { alpha } OR match { alpha }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 10,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Structural(results) => {
            let ids: Vec<&str> = results
                .results
                .iter()
                .map(|candidate| candidate.candidate_id.as_str())
                .collect();
            if ids != vec!["chunk-a", "chunk-shared"] {
                return Err(format!(
                    "expected memoized OR ids [chunk-a, chunk-shared], got {ids:?}"
                )
                .into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Structural response, got {other:?}").into());
        }
    }

    if producer.readiness_calls.load(Ordering::SeqCst) != 1 {
        return Err("identical structural leaves should consult readiness once".into());
    }
    if producer.execute_calls.load(Ordering::SeqCst) != 1 {
        return Err("identical structural leaves should execute once".into());
    }
    let scopes = producer.recorded_scopes()?;
    if scopes != vec![None] {
        return Err(format!("unexpected memoized candidate scopes: {scopes:?}").into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_executes_mixed_lexical_and_structural_and() -> TestResult {
    let producer = Arc::new(PatternRoutingStructuralProducer::new());
    let lex_opener = Arc::new(RecordingLexicalOpener {
        state: Arc::new(Mutex::new(RecordingLexicalState::default())),
        results: vec![
            recording_lexical_candidate("chunk-a"),
            recording_lexical_candidate("chunk-shared"),
        ],
    });
    let dispatcher = structural_dispatcher_mixed(
        Arc::clone(&producer),
        lex_opener,
        ready_ledger_with_structural_boolean_chunks(),
    )?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "needle AND match { alpha }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 10,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Structural(results) => {
            let mut ids = results
                .results
                .iter()
                .map(|candidate| candidate.candidate_id.as_str())
                .collect::<Vec<_>>();
            ids.sort_unstable();
            if ids != ["chunk-a", "chunk-shared"] {
                return Err(format!(
                    "expected mixed AND to keep chunk-a and chunk-shared, got {ids:?}"
                )
                .into());
            }
            let shared = results
                .results
                .iter()
                .find(|candidate| candidate.candidate_id == "chunk-shared")
                .ok_or_else(|| "missing chunk-shared structural binding".to_string())?;
            let start_bytes: Vec<u32> = shared
                .bindings
                .iter()
                .map(|binding| binding.start_byte)
                .collect();
            if start_bytes != [5, 20] {
                return Err(format!(
                    "expected canonical merged bindings [5, 20], got {start_bytes:?}"
                )
                .into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Structural response, got {other:?}").into());
        }
    }

    if producer.readiness_calls.load(Ordering::SeqCst) != 1 {
        return Err("mixed AND should consult structural readiness once".into());
    }
    if producer.execute_calls.load(Ordering::SeqCst) != 1 {
        return Err("mixed AND should execute structural leaf once".into());
    }
    Ok(())
}

#[test]
fn structural_dispatch_executes_pure_negative_root_from_pinned_universe() -> TestResult {
    let producer = Arc::new(PatternRoutingStructuralProducer::new());
    let dispatcher = structural_dispatcher_with_producer_and_ledger(
        Arc::clone(&producer),
        ready_ledger_with_structural_boolean_chunks(),
    )?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "NOT match { alpha }".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 10,
            },
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Structural(results) => {
            if results.results.len() != 1 {
                return Err(format!(
                    "expected 1 pure-negative survivor, got {:?}",
                    results.results
                )
                .into());
            }
            let candidate = results
                .results
                .first()
                .ok_or_else(|| "missing pure-negative candidate".to_string())?;
            if candidate.candidate_id != "chunk-beta" {
                return Err(format!("expected chunk-beta, got {candidate:?}").into());
            }
            if !candidate.bindings.is_empty() {
                return Err("pure-negative universe placeholder must not invent bindings".into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Structural response, got {other:?}").into());
        }
    }

    if producer.readiness_calls.load(Ordering::SeqCst) != 1 {
        return Err("pure-negative root should consult structural readiness once".into());
    }
    if producer.execute_calls.load(Ordering::SeqCst) != 1 {
        return Err("pure-negative root should execute inner structural leaf once".into());
    }
    Ok(())
}

// ------------------------------------------------------------------
// QI-BB-025 W4 — keyset paging over the evaluated match set.
// ------------------------------------------------------------------

/// A structural page request of `top_k` after `cursor` over the fixture
/// pin.
fn structural_page_request(
    top_k: u32,
    cursor: Option<quanta_index_contract::StructuralCursorV1>,
) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Structural(quanta_index_contract::StructuralQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "match { :[x] }".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k,
        },
        cursor,
    })
}

fn structural_page(
    dispatcher: &SearchPlaneDispatcher,
    top_k: u32,
    cursor: Option<quanta_index_contract::StructuralCursorV1>,
) -> Result<quanta_index_contract::SearchPlaneStructuralQueryResponse, Box<dyn std::error::Error>> {
    match dispatcher.dispatch(
        structural_page_request(top_k, cursor),
        &RequestBudgetV1::unbounded(),
    ) {
        SearchPlaneQueryIpcResponse::Structural(page) => Ok(page),
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            Err(format!("expected a structural page, got {other:?}").into())
        }
    }
}

/// `count` matches whose candidate ids are offered to the route in a
/// scrambled order (neither ascending nor descending): the page order
/// must not depend on it.
fn scrambled_matches(count: u32) -> Vec<quanta_index_core::StructuralMatchCandidate> {
    let mut order: Vec<u32> = (0..count).rev().collect();
    let rotation = order.len().min(5);
    order.rotate_left(rotation);
    order
        .into_iter()
        .map(|index| structural_match_candidate(&format!("cand-{index:02}")))
        .collect()
}

/// Pages of `top_k` partition the match set in candidate-id order with no
/// gap and no overlap.
///
/// Every page carries the exact count of matches after its cursor,
/// examines the whole match set, and the last has no cursor.
#[test]
fn structural_pages_partition_the_match_set_in_candidate_id_order() -> TestResult {
    const MATCHES: u32 = 23;
    const TOP_K: u32 = 5;
    let producer = Arc::new(RecordingStructuralProducer::ready_with(scrambled_matches(
        MATCHES,
    )));
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;
    let mut walked: Vec<String> = Vec::new();
    let mut cursor: Option<quanta_index_contract::StructuralCursorV1> = None;
    let mut remaining = u64::from(MATCHES);
    for _page in 0..8 {
        let page = structural_page(&dispatcher, TOP_K, cursor.clone())?;
        if page.window.candidate_count()
            != quanta_index_contract::CandidateCountV1::Exact(remaining)
        {
            return Err(format!(
                "the count after the cursor is exact ({remaining}): {:?}",
                page.window
            )
            .into());
        }
        if page.examined != u64::from(MATCHES) {
            return Err(format!(
                "every matched candidate is walked, examined {}",
                page.examined
            )
            .into());
        }
        walked.extend(page.results.iter().map(|row| row.candidate_id.clone()));
        remaining = remaining.saturating_sub(u64::from(page.window.returned()));
        match (page.window.has_more(), page.next_cursor) {
            (true, Some(next)) => {
                if Some(next.candidate_id.as_str()) != walked.last().map(String::as_str)
                    || next.aux_epoch != page.read_epoch
                {
                    return Err(
                        format!("the cursor is the last row in the read epoch: {next:?}").into(),
                    );
                }
                cursor = Some(next);
            }
            (false, None) => break,
            (has_more, next) => {
                return Err(format!("has_more={has_more} and cursor={next:?} disagree").into());
            }
        }
    }
    let expected: Vec<String> = (0..MATCHES)
        .map(|index| format!("cand-{index:02}"))
        .collect();
    if walked != expected {
        return Err(format!("the walk is every match once, in order: {walked:?}").into());
    }
    Ok(())
}

/// A cursor key that names no match is a boundary: the page after it
/// starts at the first match past it, and one past the end is empty.
#[test]
fn a_forged_structural_cursor_is_a_boundary_not_a_lookup() -> TestResult {
    let producer = Arc::new(RecordingStructuralProducer::ready_with(scrambled_matches(
        6,
    )));
    let dispatcher = structural_dispatcher_with_producer(Arc::clone(&producer))?;
    let first = structural_page(&dispatcher, 6, None)?;
    let epoch = first.read_epoch;
    let forged = quanta_index_contract::StructuralCursorV1 {
        candidate_id: "cand-02-and-a-half".to_string(),
        aux_epoch: epoch,
    };
    let page = structural_page(&dispatcher, 2, Some(forged))?;
    let ids: Vec<&str> = page
        .results
        .iter()
        .map(|row| row.candidate_id.as_str())
        .collect();
    if ids != ["cand-03", "cand-04"]
        || page.window.candidate_count() != quanta_index_contract::CandidateCountV1::Exact(3)
        || !page.window.has_more()
    {
        return Err(format!("the page after the boundary: {ids:?} {:?}", page.window).into());
    }
    let past_the_end = quanta_index_contract::StructuralCursorV1 {
        candidate_id: "cand-99".to_string(),
        aux_epoch: epoch,
    };
    let empty = structural_page(&dispatcher, 2, Some(past_the_end))?;
    if !empty.results.is_empty()
        || empty.window.candidate_count() != quanta_index_contract::CandidateCountV1::Exact(0)
        || empty.next_cursor.is_some()
    {
        return Err(format!("nothing follows a boundary past the end: {empty:?}").into());
    }
    Ok(())
}

/// A continuation pins the epoch its cursor names for every leaf the
/// producer executes.
///
/// A cursor naming a pruned epoch is refused `AUX_EPOCH_EXPIRED`, one
/// naming an epoch never produced `AUX_EPOCH_UNKNOWN` — neither is
/// served from the current snapshot.
#[test]
fn a_structural_continuation_pins_its_cursor_epoch_or_is_refused() -> TestResult {
    use quanta_index_core::{AUX_EPOCH_EXPIRED_CODE, AUX_EPOCH_RETAIN, AUX_EPOCH_UNKNOWN_CODE};

    use crate::query_dispatcher::tests::support::structural::install_structural_test_chunk;

    let producer = Arc::new(RecordingStructuralProducer::ready_with(scrambled_matches(
        5,
    )));
    let ledger = ready_ledger_with_structural_universe();
    let dispatcher =
        structural_dispatcher_with_producer_and_ledger(Arc::clone(&producer), Arc::clone(&ledger))?;
    let first = structural_page(&dispatcher, 2, None)?;
    let cursor = first.next_cursor.ok_or("five matches continue")?;
    if cursor.aux_epoch != quanta_index_contract::AuxEpochV1::new(1) {
        return Err(format!("one chunk install is epoch 1: {cursor:?}").into());
    }

    // A structural mutation lands between the pages.
    {
        let mut guard = ledger
            .write()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        install_structural_test_chunk(&mut guard, "chunk-late", "src/late.rs", "fn late() {}")?;
    }
    let second = structural_page(&dispatcher, 2, Some(cursor.clone()))?;
    if second.read_epoch != cursor.aux_epoch {
        return Err(format!(
            "the continuation reads the cursor's epoch, read {:?}",
            second.read_epoch
        )
        .into());
    }
    let executed_epochs = producer
        .executed_epochs
        .lock()
        .map_err(|err| format!("recorder poisoned: {err}"))?
        .clone();
    if executed_epochs
        != vec![
            quanta_index_contract::AuxEpochV1::new(1),
            quanta_index_contract::AuxEpochV1::new(1),
        ]
    {
        return Err(format!(
            "every leaf of the continuation executes at the cursor's epoch: {executed_epochs:?}"
        )
        .into());
    }
    let fresh = structural_page(&dispatcher, 2, None)?;
    if fresh.read_epoch != quanta_index_contract::AuxEpochV1::new(2) {
        return Err(format!(
            "a fresh walk reads the current epoch 2: {:?}",
            fresh.read_epoch
        )
        .into());
    }

    let unknown = quanta_index_contract::StructuralCursorV1 {
        candidate_id: cursor.candidate_id.clone(),
        aux_epoch: quanta_index_contract::AuxEpochV1::new(99),
    };
    let (code, _message) = ipc_error_from(dispatcher.dispatch(
        structural_page_request(2, Some(unknown)),
        &RequestBudgetV1::unbounded(),
    ))
    .map_err(Box::<dyn std::error::Error>::from)?;
    if code != AUX_EPOCH_UNKNOWN_CODE {
        return Err(format!("an epoch never produced is refused unknown, got {code}").into());
    }

    {
        let mut guard = ledger
            .write()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        for step in 0..AUX_EPOCH_RETAIN {
            let name = format!("chunk-churn-{step:02}");
            install_structural_test_chunk(&mut guard, &name, &format!("src/{name}.rs"), "churn")?;
        }
    }
    let (code, _message) = ipc_error_from(dispatcher.dispatch(
        structural_page_request(2, Some(cursor)),
        &RequestBudgetV1::unbounded(),
    ))
    .map_err(Box::<dyn std::error::Error>::from)?;
    if code != AUX_EPOCH_EXPIRED_CODE {
        return Err(format!("a pruned epoch is refused expired, got {code}").into());
    }
    Ok(())
}
