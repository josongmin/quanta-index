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
    TestResult, candidate, default_query_embedder, dispatcher_with_obs, encode_cbor,
    ipc_error_from, ready_ledger, ready_pin, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState, RejectLexicalOpener, StubLexicalOpener,
};
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapQueryPort;
use crate::query_dispatcher::tests::support::semantic::{
    RecordingSemanticOpener, RecordingSemanticState, RejectSemanticOpener, exact_dense_lane,
};
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

/// The explanation of one hybrid execution with the given lane tally and
/// no DSL filter.
fn hybrid_explanation(
    lexical_hits: usize,
    semantic_hits: usize,
    fused_universe: usize,
    fused_hits: usize,
) -> quanta_index_contract::SearchExplanation {
    use crate::query_dispatcher::semantic_query::{
        HybridFilterTraceV1, HybridLaneTallyV1, build_hybrid_response_explanation,
    };
    build_hybrid_response_explanation(
        &HybridLaneTallyV1 {
            lexical_hits,
            semantic_hits,
            fused_universe,
            fused_hits,
        },
        100,
        None,
        &exact_dense_lane(),
        &HybridFilterTraceV1 {
            filters: "hybrid.filters=none".to_string(),
            admission: vec!["hybrid.dense_admission=not_needed".to_string()],
        },
    )
}

// CASE-COVERS: hybrid explanation honesty over two independent lanes.
#[test]
fn build_hybrid_response_explanation_reports_honest_lane_contribution_v1() {
    use quanta_index_contract::EngineTouched;
    // args: (lexical_hits, semantic_hits, fused_universe, fused)
    // Both lanes contributed -> genuine RRF over both engines, and the
    // trace says the lanes are independent (QI-BB-018).
    let both = hybrid_explanation(2, 2, 3, 2);
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

    // The filter contract's trace entries ride along (QI-BB-018 보완 #3).
    for detail in ["hybrid.filters=none", "hybrid.dense_admission=not_needed"] {
        assert!(
            both.planner_trace.iter().any(|e| e.detail == detail),
            "hybrid trace must carry {detail}: {:?}",
            both.planner_trace
        );
    }

    // Lexical found candidates but the dense lane matched none -> must
    // NOT claim a symmetric rrf fusion; it is lexical-only and the
    // Semantic engine is not touched.
    let lex_only = hybrid_explanation(3, 0, 3, 3);
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
    let empty = hybrid_explanation(0, 0, 0, 0);
    assert_eq!(empty.strategy, "empty", "no-hit hybrid must report empty");
    assert!(
        empty.engines_touched.is_empty(),
        "empty hybrid must claim no engines, got {:?}",
        empty.engines_touched
    );

    // Dense-only recall is a real outcome now: the lexical lane found
    // nothing but the dense lane did.
    let semantic_only = hybrid_explanation(0, 1, 1, 1);
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

// CASE-COVERS: QI-BB-022 — every fused row carries the lanes that saw it,
// with the rank and raw score each lane emitted, and the RRF score that
// ranked it; the fused order is exactly the id-level fusion's order.
#[test]
fn hybrid_rows_carry_per_lane_provenance_and_the_fused_score() -> TestResult {
    use quanta_index_contract::{HybridLaneContributionV1, HybridLaneV1};
    use quanta_index_core::HybridOrchestratorPolicy;
    // Lexical: alpha, beta, gamma. Dense: beta, delta, alpha. Distinct
    // scores per lane so lane stabilization keeps these orders.
    let lexical = vec![
        candidate("alpha", 3.0),
        candidate("beta", 2.0),
        candidate("gamma", 1.0),
    ];
    let dense = vec![
        candidate("beta", 0.9),
        candidate("delta", 0.8),
        candidate("alpha", 0.7),
    ];
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState {
        constrained_search_results: Some(dense.clone()),
        ..RecordingSemanticState::default()
    }));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(StubLexicalOpener {
            results: lexical.clone(),
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&semantic_state),
        }),
        Arc::new(StubRepoMapQueryPort),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "scope".to_string(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 10,
            },
            semantic_query_text: "scope alpha".to_string(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 10,
        }),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::Hybrid(hybrid) = response else {
        return Err(format!("expected Hybrid response, got {response:?}").into());
    };
    // The fused order is the id-level fusion's order, unchanged by the
    // provenance.
    let ids = hybrid
        .results
        .iter()
        .map(|row| row.candidate.candidate_id.clone())
        .collect::<Vec<_>>();
    let lexical_ids = lexical
        .iter()
        .map(|row| row.candidate_id.clone())
        .collect::<Vec<_>>();
    let dense_ids = dense
        .iter()
        .map(|row| row.candidate_id.clone())
        .collect::<Vec<_>>();
    let expected = HybridOrchestratorPolicy::fuse_rrf_ids(&lexical_ids, &dense_ids, 10);
    if ids != expected {
        return Err(format!("fused order {ids:?} != id fusion {expected:?}").into());
    }
    if ids != ["beta", "alpha", "delta", "gamma"] {
        return Err(format!("unexpected fused order {ids:?}").into());
    }
    // An independent RRF oracle over the mocks' lane positions.
    let rrf = |ranks: &[u32]| -> f64 {
        ranks
            .iter()
            .map(|rank| 1.0 / (60.0 + f64::from(*rank)))
            .sum()
    };
    let row = |id: &str| {
        hybrid
            .results
            .iter()
            .find(|row| row.candidate.candidate_id == id)
            .ok_or_else(|| format!("{id} fused"))
    };
    // beta: both lanes saw it — the lexical row (score 2.0) is carried, the
    // contributions are the lanes' own ranks and raw scores, in lane order.
    let beta = row("beta")?;
    if beta.contributions
        != [
            HybridLaneContributionV1 {
                lane: HybridLaneV1::Lexical,
                rank: 2,
                raw_score: 2.0,
            },
            HybridLaneContributionV1 {
                lane: HybridLaneV1::Dense,
                rank: 1,
                raw_score: 0.9,
            },
        ]
        || beta.candidate.score.to_bits() != 2.0_f32.to_bits()
        || beta.fused_score.to_bits() != rrf(&[2, 1]).to_bits()
    {
        return Err(format!("beta provenance: {beta:?}").into());
    }
    // delta: dense-only — exactly the dense contribution, the dense row.
    let delta = row("delta")?;
    if delta.contributions
        != [HybridLaneContributionV1 {
            lane: HybridLaneV1::Dense,
            rank: 2,
            raw_score: 0.8,
        }]
        || delta.candidate.score.to_bits() != 0.8_f32.to_bits()
        || delta.fused_score.to_bits() != rrf(&[2]).to_bits()
    {
        return Err(format!("delta provenance: {delta:?}").into());
    }
    // gamma: lexical-only.
    let gamma = row("gamma")?;
    if gamma.contributions
        != [HybridLaneContributionV1 {
            lane: HybridLaneV1::Lexical,
            rank: 3,
            raw_score: 1.0,
        }]
        || gamma.fused_score.to_bits() != rrf(&[3]).to_bits()
    {
        return Err(format!("gamma provenance: {gamma:?}").into());
    }
    // The wire holds the same rows: the response encodes, and decodes back
    // to itself, under the contract's ranking-order check.
    let bytes = encode_cbor(&hybrid)?;
    let decoded: quanta_index_contract::HybridQueryResponse =
        quanta_index_ipc::decode_cbor_payload(&bytes)?;
    if decoded != hybrid {
        return Err("the hybrid response must round-trip through the wire".into());
    }
    Ok(())
}
