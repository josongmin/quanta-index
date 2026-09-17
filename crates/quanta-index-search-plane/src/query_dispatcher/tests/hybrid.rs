use std::sync::{Arc, Mutex, RwLock};

use quanta_index_contract::{
    HybridQueryRequest, ManifestGeneration, QueryConstraintSetV1, RepoId, RevisionId,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, SearchPlaneTrackKind,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::RequestBudgetV1;

use crate::Ledger;
use crate::observability::BoundedQueryObsStore;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::selection::make_pin;
use crate::query_dispatcher::tests::support::common::{
    TestResult, candidate, default_query_embedder, dispatcher_with_obs, ipc_error_from,
    ready_ledger, ready_pin, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState, RejectLexicalOpener, StubLexicalOpener,
};
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapQueryPort;
use crate::query_dispatcher::tests::support::semantic::{
    RecordingSemanticOpener, RecordingSemanticState, RejectSemanticOpener, exact_dense_lane,
};
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

// CASE-COVERS: hybrid explanation honesty over two independent lanes.
#[test]
fn build_hybrid_response_explanation_reports_honest_lane_contribution_v1() {
    use crate::query_dispatcher::semantic_query::build_hybrid_response_explanation;
    use quanta_index_contract::EngineTouched;
    // args: (lexical_hits, semantic_hits, fused_universe, fused, top_k, stop, dense lane)
    // Both lanes contributed -> genuine RRF over both engines, and the
    // trace says the lanes are independent (QI-BB-018).
    let both = build_hybrid_response_explanation(2, 2, 3, 2, 100, None, &exact_dense_lane());
    assert_eq!(both.strategy, "rrf", "both-lane hybrid must stay rrf");
    assert_eq!(
        both.engines_touched,
        vec![EngineTouched::Lexical, EngineTouched::Semantic],
        "both-lane hybrid must report symmetric rrf over both engines"
    );
    assert!(
        both.planner_trace.iter().any(|e| e.detail
            == "hybrid.lanes=independent; lexical_hits=2; semantic_hits=2; fused_universe=3"),
        "hybrid trace must expose the independent lanes and the fused universe: {:?}",
        both.planner_trace
    );

    // Lexical found candidates but the dense lane matched none -> must
    // NOT claim a symmetric rrf fusion; it is lexical-only and the
    // Semantic engine is not touched.
    let lex_only = build_hybrid_response_explanation(3, 0, 3, 3, 100, None, &exact_dense_lane());
    assert_eq!(
        lex_only.strategy, "lexical_only",
        "semantic-empty hybrid must report lexical_only, not rrf"
    );
    assert_eq!(
        lex_only.engines_touched,
        vec![EngineTouched::Lexical],
        "semantic-empty hybrid must not over-claim the Semantic engine"
    );

    // No lane found anything: honest "empty", no engines claimed.
    let empty = build_hybrid_response_explanation(0, 0, 0, 0, 100, None, &exact_dense_lane());
    assert_eq!(empty.strategy, "empty", "no-hit hybrid must report empty");
    assert!(
        empty.engines_touched.is_empty(),
        "empty hybrid must claim no engines, got {:?}",
        empty.engines_touched
    );

    // Dense-only recall is a real outcome now: the lexical lane found
    // nothing but the dense lane did.
    let semantic_only =
        build_hybrid_response_explanation(0, 1, 1, 1, 100, None, &exact_dense_lane());
    assert_eq!(semantic_only.strategy, "semantic_only");
    assert_eq!(semantic_only.engines_touched, vec![EngineTouched::Semantic]);
}

#[test]
fn hybrid_dispatch_embeds_semantic_query_text() -> TestResult {
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
        quanta_index_contract::ExactRepoRelativePathV1::new("src/lib.rs")
            .map_err(str::to_string)?,
    );
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&lexical_state),
            results: vec![candidate("alpha", 1.0), candidate("beta", 0.9)],
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&semantic_state),
        }),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );

    let pin = make_pin(
        RepoId::new("repo-map-ipc"),
        RevisionId::new("rev-map-ipc"),
        ManifestGeneration::new(9),
    );
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: "scope".to_string(),
                constraints: constraints.clone(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 2,
            },
            semantic_query_text: "scope alpha".to_string(),
            generation: Some(pin),
            generation_selector: None,
            top_k: 2,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => {
            if hybrid.results.is_empty() {
                return Err("expected non-empty hybrid results".into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected Hybrid response, got {other:?}").into());
        }
    }

    let (scoped_vectors, search_vectors, search_constraints) = {
        let guard = semantic_state
            .lock()
            .map_err(|err| format!("semantic state poisoned: {err}"))?;
        (
            guard.scoped_vectors.clone(),
            guard.search_vectors.clone(),
            guard.search_constraints.clone(),
        )
    };
    let expected = default_query_embedder().embed_query("scope alpha")?;
    // QI-BB-018: the dense lane is independent of the lexical hits — one
    // unscoped search over the query vector, under the request's
    // constraints; never a search scoped to the lexical ids.
    if !scoped_vectors.is_empty() {
        return Err(format!(
            "hybrid must not scope the dense lane to lexical hits: {scoped_vectors:?}"
        )
        .into());
    }
    if search_vectors.as_slice() != [expected] {
        return Err(format!("unexpected dense lane vectors: {search_vectors:?}").into());
    }
    if search_constraints.as_slice() != [constraints.clone()] {
        return Err(format!(
            "hybrid dense lane lost exact-path constraints: {search_constraints:?}"
        )
        .into());
    }
    let searched_constraints = {
        let guard = lexical_state
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        guard.searched_constraints.clone()
    };
    if searched_constraints.as_slice() != [constraints] {
        return Err(format!(
            "hybrid lexical leg lost exact-path constraints: {searched_constraints:?}"
        )
        .into());
    }
    Ok(())
}

#[test]
fn hybrid_dispatch_rejects_unsealed_semantic_generation_with_exact_code() -> TestResult {
    let mut ledger = Ledger::default();
    let repo_id = RepoId::new("repo-map-ipc");
    let revision_id = RevisionId::new("rev-map-ipc");
    ledger.record_track_materialized(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Lexical,
        ManifestGeneration::new(9),
        None,
    );
    ledger.record_track_seal(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Lexical,
        ManifestGeneration::new(9),
    );
    ledger.record_track_materialized(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Semantic,
        ManifestGeneration::new(9),
        Some("manifest-digest-9"),
    );
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        Arc::new(RwLock::new(ledger)),
        test_activation_catalog()?,
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "scope".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 2,
            },
            semantic_query_text: "scope alpha".to_string(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 2,
        }),
        &RequestBudgetV1::unbounded(),
    );

    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != crate::readiness::ERR_SEMANTIC_GENERATION_NOT_SEALED {
        return Err(format!("unexpected hybrid unsealed code: {code}").into());
    }
    Ok(())
}

#[test]
fn hybrid_dispatch_emits_closed_obs_metrics() -> TestResult {
    let obs_sink = Arc::new(BoundedQueryObsStore::default());
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let dispatcher = dispatcher_with_obs(
        Arc::new(StubLexicalOpener {
            results: vec![candidate("lex-a", 1.0), candidate("lex-b", 0.5)],
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&semantic_state),
        }),
        obs_sink.clone(),
    )?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "scope".to_string(),
                constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 1,
            },
            semantic_query_text: "scope alpha".to_string(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 1,
        }),
        &RequestBudgetV1::unbounded(),
    );
    match response {
        SearchPlaneQueryIpcResponse::Hybrid(_) | SearchPlaneQueryIpcResponse::HybridSeed(_) => {}
        other @ (SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Hybrid response, got {other:?}").into());
        }
    }

    let names = obs_sink
        .snapshot()
        .into_iter()
        .map(|sample| sample.name.into_string())
        .collect::<Vec<_>>();
    // The closed metric set for one cold hybrid dispatch. The two
    // snapshot samples are cold opens because the registry starts empty;
    // a warm dispatch would report `..._hit_total` in their place.
    let expected = vec![
        "lq_query_intake_total".to_string(),
        "lq_snapshot_lexical_cold_open_ms".to_string(),
        "lq_snapshot_semantic_cold_open_ms".to_string(),
        "lq_planner_total".to_string(),
        "lq_engine_fanout_count".to_string(),
        "lq_merge_result_count".to_string(),
        "lq_early_stop_total".to_string(),
        "lq_route_hybrid_latency_ms".to_string(),
        "lq_route_hybrid_served_total".to_string(),
    ];
    if names != expected {
        return Err(format!("unexpected obs metric names: {names:?}").into());
    }
    let errors = obs_sink.errors();
    if !errors.is_empty() {
        return Err(format!("unexpected obs errors: {errors:?}").into());
    }
    let samples = obs_sink.snapshot();
    for sample in &samples {
        if sample.dimensions.ticket_id.as_ref() != "LXE-10"
            || sample.dimensions.wave_id.as_ref() != "8"
            || sample.dimensions.tenant_id.as_ref() != "local"
            || sample.dimensions.repo_id.as_ref() != "repo-map-ipc"
            || sample.dimensions.generation_id != 9
        {
            return Err(format!("unexpected obs dimensions: {:?}", sample.dimensions).into());
        }
        if sample.name.contains("scope") || sample.name.contains("1.0 0.0") {
            return Err(format!("metric name leaked query content: {}", sample.name).into());
        }
    }
    Ok(())
}
