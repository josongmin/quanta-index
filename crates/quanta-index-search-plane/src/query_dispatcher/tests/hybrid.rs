use std::sync::{Arc, Mutex, RwLock};

use quanta_index_contract::{
    ExecutionOutcomeV2, GoTestTraceFileQueryRequestV1, GoTestTraceFrameClassV1, HybridQueryRequest,
    ManifestGeneration, QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId,
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
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapSnapshotPort;
use crate::query_dispatcher::tests::support::semantic::{
    RecordingSemanticOpener, RecordingSemanticState, RejectSemanticOpener, exact_dense_lane,
};
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

fn go_file_candidate(
    id: &str,
    path: &str,
    line: u32,
    score: f32,
) -> quanta_index_contract::LexicalCandidate {
    let mut row = candidate(id, score);
    row.repo_relative_path = RepoRelativePath::new(path);
    row.start_line = line;
    row.end_line = line;
    row
}

fn go_file_request(failure_excerpt: &str, owner: Option<RepoId>) -> GoTestTraceFileQueryRequestV1 {
    GoTestTraceFileQueryRequestV1 {
        query: HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "handler failure".to_string(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 8,
                cursor: None,
            },
            semantic_query_text: "handler failure".to_string(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 8,
        },
        failure_excerpt: failure_excerpt.to_string(),
        failure_source_repo_id: owner,
        failure_module_context: None,
        top_k: 5,
    }
}

#[test]
fn go_test_trace_dense_attestation_requires_an_executed_scan() -> TestResult {
    use quanta_index_core::{
        DenseIndexBuildV1, DenseIndexEffortV1, DenseIndexTrainingV1, DenseIndexV1,
        DenseLaneAttestationV1, DenseLaneContractV1,
    };

    let sealed_approximate = DenseLaneContractV1 {
        index: DenseIndexV1::Approximate {
            effort: DenseIndexEffortV1 {
                index_kind: "ivf_hnsw_sq".to_string(),
                partitions: 1,
                nprobes: 1,
                ef_floor: 16,
                ef_per_candidate: 2,
                refine_factor: 2,
            },
            lineage: DenseIndexTrainingV1 {
                trained_at_generation: 9,
                trained_rows: 1,
                appended_rows: 0,
                deleted_rows: 0,
            },
            build: DenseIndexBuildV1 {
                hnsw_m: 16,
                hnsw_ef_construction: 64,
                appended_segments: Vec::new(),
            },
        },
        attestation: DenseLaneAttestationV1::Sealed,
    };
    for force_empty in [true, false] {
        let lexical = Arc::new(Mutex::new(RecordingLexicalState::default()));
        let semantic = Arc::new(Mutex::new(RecordingSemanticState {
            constrained_search_results: Some(vec![go_file_candidate(
                "dense-go",
                "src/handler.go",
                42,
                1.0,
            )]),
            dense_lane: Some(sealed_approximate.clone()),
            ..RecordingSemanticState::default()
        }));
        let dispatcher = dispatcher_with_obs(
            Arc::new(RecordingLexicalOpener {
                state: Arc::clone(&lexical),
                results: vec![go_file_candidate("lex-go", "src/handler.go", 42, 1.0)],
            }),
            Arc::new(RecordingSemanticOpener {
                state: Arc::clone(&semantic),
            }),
            Arc::new(BoundedQueryObsStore::default()),
        )?;
        let mut request = go_file_request("test failure", None);
        request.query.text_query.syntax = TextQuerySyntax::Sourcegraph;
        request.query.text_query.query_text = "lang:python handler".to_string();
        request.query.text_query.constraints =
            QueryConstraintSetV1::from_languages([quanta_index_contract::lex::LanguageCode::new(
                if force_empty { "rust" } else { "python" },
            )
            .map_err(str::to_string)?]);
        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::GoTestTraceFileV1(request.clone()),
            &RequestBudgetV1::unbounded(),
        );
        let SearchPlaneQueryIpcResponse::GoTestTraceFileV1(files) = response else {
            return Err(format!("force_empty={force_empty}: {response:?}").into());
        };
        files.validate_against_v1(&request)?;
        let dense_trace = files
            .explanation
            .planner_trace
            .iter()
            .find(|entry| entry.detail.starts_with("dense.index="))
            .ok_or("missing dense lane trace")?;
        let expected_trace = if force_empty {
            sealed_approximate.trace_detail()
        } else {
            "dense.index=exact; dense.attestation=exact_scan_of_sealed_rows".to_string()
        };
        let lexical_calls = lexical
            .lock()
            .map_err(|error| error.to_string())?
            .search_top_ks
            .len();
        let semantic_calls = semantic
            .lock()
            .map_err(|error| error.to_string())?
            .search_top_ks
            .len();
        let expected_calls = usize::from(!force_empty);
        if dense_trace.detail != expected_trace
            || lexical_calls != expected_calls
            || semantic_calls != expected_calls
            || (force_empty && !files.explanation.engines_executed.is_empty())
            || (!force_empty
                && files.explanation.engines_executed
                    != [
                        quanta_index_contract::EngineTouched::Lexical,
                        quanta_index_contract::EngineTouched::Semantic,
                    ])
        {
            return Err(format!(
                "force_empty={force_empty}: dense trace={dense_trace:?}, expected={expected_trace:?}, lexical_calls={lexical_calls}, semantic_calls={semantic_calls}, executed={:?}",
                files.explanation.engines_executed,
            )
            .into());
        }
    }
    Ok(())
}

#[test]
fn hybrid_fetch_floor_reaches_both_backends_and_keeps_force_empty_idle() -> TestResult {
    use crate::HybridFetchFloorPolicy;
    for floor in [
        HybridFetchFloorPolicy::Floor25,
        HybridFetchFloorPolicy::Floor50,
        HybridFetchFloorPolicy::Floor100,
    ] {
        for (top_k, force_empty) in [(1, false), (10, false), (100, false), (10, true)] {
            let lexical = Arc::new(Mutex::new(RecordingLexicalState::default()));
            let semantic = Arc::new(Mutex::new(RecordingSemanticState {
                constrained_search_results: Some(vec![candidate("dense-a", 1.0)]),
                ..RecordingSemanticState::default()
            }));
            let dispatcher = dispatcher_with_obs(
                Arc::new(RecordingLexicalOpener {
                    state: Arc::clone(&lexical),
                    results: vec![candidate("lex-a", 1.0)],
                }),
                Arc::new(RecordingSemanticOpener {
                    state: Arc::clone(&semantic),
                }),
                Arc::new(BoundedQueryObsStore::default()),
            )?;
            if dispatcher.hybrid_fetch_floor() != HybridFetchFloorPolicy::Floor100 {
                return Err("default dispatcher must preserve production floor 100".into());
            }
            let dispatcher = dispatcher.with_hybrid_fetch_floor(floor);
            if dispatcher.hybrid_fetch_floor() != floor {
                return Err("dispatcher did not retain the selected floor".into());
            }
            let constraints = if force_empty {
                QueryConstraintSetV1::from_languages([
                    quanta_index_contract::lex::LanguageCode::new("rust")
                        .map_err(str::to_string)?,
                ])
            } else {
                QueryConstraintSetV1::unconstrained()
            };
            let response = dispatcher.dispatch(
                SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
                    text_query: TextQueryRequest {
                        syntax: TextQuerySyntax::Sourcegraph,
                        query_text: if force_empty {
                            "lang:python needle"
                        } else {
                            "needle"
                        }
                        .to_string(),
                        constraints,
                        generation: Some(ready_pin()),
                        generation_selector: None,
                        top_k,
                        cursor: None,
                    },
                    semantic_query_text: "needle".to_string(),
                    generation: Some(ready_pin()),
                    generation_selector: None,
                    top_k,
                }),
                &RequestBudgetV1::unbounded(),
            );
            let SearchPlaneQueryIpcResponse::Hybrid(response) = response else {
                return Err(format!(
                    "floor={floor:?}, k={top_k}: expected hybrid success, got {response:?}"
                )
                .into());
            };
            let expected = if force_empty {
                Vec::new()
            } else {
                vec![floor.get().max(top_k.saturating_add(1))]
            };
            let lexical_fetches = lexical
                .lock()
                .map_err(|error| error.to_string())?
                .search_top_ks
                .clone();
            let semantic_fetches = semantic
                .lock()
                .map_err(|error| error.to_string())?
                .search_top_ks
                .clone();
            if lexical_fetches != expected
                || semantic_fetches != expected
                || response.generation != ready_pin()
            {
                return Err(format!("floor={floor:?}, k={top_k}: wrong fetch/pin; lexical={lexical_fetches:?}, semantic={semantic_fetches:?}, expected={expected:?}").into());
            }
            if force_empty
                && (!response.results.is_empty()
                    || !response.explanation.engines_executed.is_empty())
            {
                return Err("force-empty floor variant invoked a lane or returned rows".into());
            }
        }
    }
    Ok(())
}

/// The explanation of one hybrid execution with the given lane tally, the
/// given observed invocation truth, and no DSL filter.
fn hybrid_explanation(
    lexical_hits: usize,
    semantic_hits: usize,
    fused_universe: usize,
    fused_hits: usize,
    execution: crate::query_dispatcher::execution_trace::LaneExecutionSummaryV1,
) -> quanta_index_contract::SearchExplanation {
    hybrid_explanation_with_id(
        lexical_hits,
        semantic_hits,
        fused_universe,
        fused_hits,
        execution,
        // Off-transport honesty fixture: no budget, no correlation.
        0,
    )
}

/// [`hybrid_explanation`] with an explicit request id, for the W10-R2
/// correlation tests: the builder echoes whatever the route resolved.
fn hybrid_explanation_with_id(
    lexical_hits: usize,
    semantic_hits: usize,
    fused_universe: usize,
    fused_hits: usize,
    execution: crate::query_dispatcher::execution_trace::LaneExecutionSummaryV1,
    request_id: u64,
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
            execution,
        },
        100,
        None,
        &exact_dense_lane(),
        &HybridFilterTraceV1 {
            filters: "hybrid.filters=none".to_string(),
            admission: vec!["hybrid.dense_admission=not_needed".to_string()],
        },
        request_id,
    )
}

// CASE-COVERS: hybrid explanation honesty over two independent lanes.
#[test]
fn build_hybrid_response_explanation_reports_honest_lane_contribution_v1() {
    use crate::query_dispatcher::execution_trace::LaneExecutionSummaryV1;
    use quanta_index_contract::EngineTouched;
    // args: (lexical_hits, semantic_hits, fused_universe, fused, execution)
    // Both lanes contributed -> genuine RRF over both engines, and the
    // trace says the lanes are independent (QI-BB-018).
    let both = hybrid_explanation(
        2,
        2,
        3,
        2,
        LaneExecutionSummaryV1 {
            lexical_invocations: 1,
            semantic_invocations: 1,
            lexical_contributed: true,
            semantic_contributed: true,
        },
    );
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
    let lex_only = hybrid_explanation(
        3,
        0,
        3,
        3,
        LaneExecutionSummaryV1 {
            lexical_invocations: 1,
            semantic_invocations: 1,
            lexical_contributed: true,
            semantic_contributed: false,
        },
    );
    assert_eq!(
        lex_only.strategy, "lexical_only",
        "semantic-empty hybrid must report lexical_only, not rrf"
    );
    assert_eq!(
        lex_only.engines_touched,
        vec![EngineTouched::Lexical],
        "semantic-empty hybrid must not over-claim the Semantic engine"
    );

    // No lane found anything: honest "empty", no engines claimed. The
    // summary is a `force_empty` execution: nothing invoked, nothing
    // contributed.
    let empty = hybrid_explanation(0, 0, 0, 0, LaneExecutionSummaryV1::default());
    assert_eq!(empty.strategy, "empty", "no-hit hybrid must report empty");
    assert!(
        empty.engines_touched.is_empty(),
        "empty hybrid must claim no engines, got {:?}",
        empty.engines_touched
    );

    // Dense-only recall is a real outcome now: the lexical lane found
    // nothing but the dense lane did.
    let semantic_only = hybrid_explanation(
        0,
        1,
        1,
        1,
        LaneExecutionSummaryV1 {
            lexical_invocations: 1,
            semantic_invocations: 1,
            lexical_contributed: false,
            semantic_contributed: true,
        },
    );
    assert_eq!(semantic_only.strategy, "semantic_only");
    assert_eq!(semantic_only.engines_touched, vec![EngineTouched::Semantic]);

    // W10-R1: executed is invocation truth, not plan shape. The three
    // ran-lanes cases report both engines executed — including the
    // zero-hit lanes the fanout metric must count — while the
    // `force_empty` case reports none.
    for (name, explanation, executed) in [
        (
            "both",
            &both,
            vec![EngineTouched::Lexical, EngineTouched::Semantic],
        ),
        (
            "lex_only",
            &lex_only,
            vec![EngineTouched::Lexical, EngineTouched::Semantic],
        ),
        ("empty", &empty, Vec::new()),
        (
            "semantic_only",
            &semantic_only,
            vec![EngineTouched::Lexical, EngineTouched::Semantic],
        ),
    ] {
        assert_eq!(
            explanation.engines_executed, executed,
            "{name} hybrid must report exactly its invoked engines"
        );
        // Builders emit 0; only the transport adapter stamps a real id.
        assert_eq!(
            explanation.request_id, 0,
            "{name} hybrid built off-transport must carry request_id 0"
        );
    }
}

// CASE-COVERS: W10-R1 builder purity — engine lists follow the summary,
// never the hit counts.
#[test]
fn hybrid_builder_derives_engines_from_summary_not_hits() {
    use crate::query_dispatcher::execution_trace::LaneExecutionSummaryV1;
    use quanta_index_contract::EngineTouched;
    // Hits without invocations (impossible through a route, possible as a
    // builder input): no engine may be reported executed.
    let phantom = hybrid_explanation(5, 5, 9, 4, LaneExecutionSummaryV1::default());
    assert!(
        phantom.engines_executed.is_empty(),
        "hits must never imply execution: {:?}",
        phantom.engines_executed
    );
    assert!(
        phantom.engines_touched.is_empty(),
        "hits must never imply contribution: {:?}",
        phantom.engines_touched
    );
    // Invocations without contribution: executed but not touched.
    let zero_hit = hybrid_explanation(
        0,
        0,
        0,
        0,
        LaneExecutionSummaryV1 {
            lexical_invocations: 3,
            semantic_invocations: 2,
            lexical_contributed: false,
            semantic_contributed: false,
        },
    );
    assert_eq!(
        zero_hit.engines_executed,
        vec![EngineTouched::Lexical, EngineTouched::Semantic]
    );
    assert!(zero_hit.engines_touched.is_empty());
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
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );

    let pin = make_pin(
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
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
                cursor: None,
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
        other @ (SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::GoTestTraceFileV1(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_)
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
    let expected = default_query_embedder().embed_query(
        "scope alpha",
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
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
    let repo_id =
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy");
    let revision_id =
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy");
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
    ledger.record_historically_sealed_search_corpus(
        &repo_id,
        &revision_id,
        ManifestGeneration::new(9),
        "manifest-digest-9",
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
        Arc::new(StubRepoMapSnapshotPort::default()),
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
                cursor: None,
            },
            semantic_query_text: "scope alpha".to_string(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 2,
        }),
        &RequestBudgetV1::unbounded(),
    );

    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::SemanticGenerationNotSealed {
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
                cursor: None,
            },
            semantic_query_text: "scope alpha".to_string(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 1,
        }),
        &RequestBudgetV1::unbounded(),
    );
    match response {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => {
            let stages = hybrid
                .explanation
                .stage_timings
                .as_ref()
                .ok_or("missing server timings")?;
            let names = stages
                .iter()
                .map(|stage| stage.stage.as_str())
                .collect::<Vec<_>>();
            let expected_count = u64::try_from(hybrid.results.len())?;
            if names
                != [
                    "hybrid.prepare",
                    "hybrid.read_view",
                    "hybrid.lexical_search",
                    "hybrid.embedding",
                    "hybrid.dense_fetch",
                    "hybrid.dense_admission",
                    "hybrid.fusion",
                ]
                || stages.last().and_then(|stage| stage.returned_candidates) != Some(expected_count)
            {
                return Err(format!("hybrid stage provenance is incomplete: {stages:?}").into());
            }
        }
        SearchPlaneQueryIpcResponse::HybridSeed(_) => {
            return Err("hybrid query returned hybrid seed response".into());
        }
        other @ (SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::GoTestTraceFileV1(_)
        | SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_)
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
        "lq_lane_contribution_count".to_string(),
        "lq_merge_result_count".to_string(),
        "lq_early_stop_total".to_string(),
        "lq_route_hybrid_examined_candidates_total".to_string(),
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
        Arc::new(StubRepoMapSnapshotPort::default()),
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
                cursor: None,
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

// CASE-COVERS: W10-R2 — the builder echoes the route-resolved request id
// verbatim and keeps 0 for the off-transport fixture.
#[test]
fn build_hybrid_response_explanation_echoes_the_routed_request_id() {
    use crate::query_dispatcher::execution_trace::LaneExecutionSummaryV1;
    let carried = hybrid_explanation_with_id(2, 2, 3, 2, LaneExecutionSummaryV1::default(), 42);
    assert_eq!(
        carried.request_id, 42,
        "the builder must echo the routed id, got {}",
        carried.request_id
    );
    let off_transport = hybrid_explanation(2, 2, 3, 2, LaneExecutionSummaryV1::default());
    assert_eq!(
        off_transport.request_id, 0,
        "the off-transport fixture must stay 0, got {}",
        off_transport.request_id
    );
}

// CASE-COVERS: P02 — the public Go route ranks distinct files from the two
// original chunk lanes. Equal chunk IDs in different files stay distinct.
#[test]
fn go_test_trace_dispatch_ranks_files_without_chunk_fusion() -> TestResult {
    let lexical = vec![
        go_file_candidate("shared-id", "src/handler.go", 42, 5.0),
        go_file_candidate("handler-next", "src/handler.go", 80, 4.0),
        go_file_candidate("helper-lex", "src/helper.go", 12, 3.0),
        go_file_candidate("test-lex", "src/handler_test.go", 7, 2.0),
        go_file_candidate("doc-lex", "README.md", 1, 1.0),
    ];
    let dense = vec![
        go_file_candidate("shared-id", "src/other.go", 9, 0.9),
        go_file_candidate("helper-dense", "src/helper.go", 20, 0.8),
        go_file_candidate("handler-dense", "src/handler.go", 41, 0.7),
        go_file_candidate("other-next", "src/other.go", 15, 0.6),
        go_file_candidate("test-dense", "src/handler_test.go", 8, 0.5),
    ];
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState {
        constrained_search_results: Some(dense),
        ..RecordingSemanticState::default()
    }));
    let dispatcher = dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&lexical_state),
            results: lexical,
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&semantic_state),
        }),
        Arc::new(BoundedQueryObsStore::default()),
    )?;
    let request = go_file_request("src/handler.go:42: panic", Some(ready_pin().repo_id));
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::GoTestTraceFileV1(request.clone()),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::GoTestTraceFileV1(files) = response else {
        return Err(format!("expected GoTestTraceFileV1 response, got {response:?}").into());
    };
    files.validate_against_v1(&request)?;
    let paths = files
        .results
        .iter()
        .map(|hit| hit.file.repo_relative_path.as_str())
        .collect::<Vec<_>>();
    if paths
        != [
            "src/handler.go",
            "src/helper.go",
            "src/handler_test.go",
            "src/other.go",
            "README.md",
        ]
    {
        return Err(format!("Go file rank changed: {paths:?}").into());
    }
    if files.frame_class != GoTestTraceFrameClassV1::Exact
        || !files.results[0].source_frame
        || files.results[1..].iter().any(|hit| hit.source_frame)
        || files.lexical_window.returned() != 5
        || files.dense_window.returned() != 5
        || files.lexical_window.outcome() != ExecutionOutcomeV2::ExactExhausted
        || files.dense_window.outcome() != ExecutionOutcomeV2::ExactExhausted
    {
        return Err(format!("Go frame or original chunk windows changed: {files:?}").into());
    }
    let handler = &files.results[0];
    let helper = &files.results[1];
    let other = &files.results[3];
    let handler_lex = handler
        .lexical
        .as_ref()
        .ok_or("handler lost lexical witness")?;
    let handler_dense = handler.dense.as_ref().ok_or("handler lost dense witness")?;
    let helper_lex = helper
        .lexical
        .as_ref()
        .ok_or("helper lost lexical witness")?;
    let helper_dense = helper.dense.as_ref().ok_or("helper lost dense witness")?;
    let other_dense = other.dense.as_ref().ok_or("other lost dense witness")?;
    if (
        handler_lex.file_rank,
        handler_lex.chunk_rank,
        handler_lex.candidate.candidate_id.as_str(),
        handler_lex.candidate.start_line,
    ) != (1, 1, "shared-id", 42)
        || (
            handler_dense.file_rank,
            handler_dense.chunk_rank,
            handler_dense.candidate.candidate_id.as_str(),
            handler_dense.candidate.start_line,
        ) != (3, 3, "handler-dense", 41)
        || (
            helper_lex.file_rank,
            helper_lex.chunk_rank,
            helper_lex.candidate.candidate_id.as_str(),
        ) != (2, 3, "helper-lex")
        || (
            helper_dense.file_rank,
            helper_dense.chunk_rank,
            helper_dense.candidate.candidate_id.as_str(),
        ) != (2, 2, "helper-dense")
        || (
            other_dense.file_rank,
            other_dense.chunk_rank,
            other_dense.candidate.candidate_id.as_str(),
            other_dense.candidate.start_line,
        ) != (1, 1, "shared-id", 9)
        || other.lexical.is_some()
    {
        return Err(format!(
            "Go lane witnesses were fused or reordered: {:?}",
            files.results
        )
        .into());
    }
    let explanation = &files.explanation;
    if explanation.strategy != "go_test_trace_file_rrf"
        || !explanation.planner_trace.iter().any(|entry| {
            entry.stage == quanta_index_contract::PlannerStage::Merge
                && entry.detail == "go_test_trace.file_results=5"
        })
        || explanation
            .planner_trace
            .iter()
            .any(|entry| entry.detail.starts_with("hybrid.fused_results="))
    {
        return Err(format!("Go explanation describes chunk fusion: {explanation:?}").into());
    }
    let stages = explanation
        .stage_timings
        .as_deref()
        .ok_or("Go route has no server stage timings")?;
    if stages
        .iter()
        .any(|stage| stage.stage.as_str() == "hybrid.fusion")
        || explanation.early_stop_reason.is_some()
    {
        return Err(format!(
            "Go route claimed chunk fusion or unknown file exhaustion: {explanation:?}"
        )
        .into());
    }
    let wire = encode_cbor(&files)?;
    let decoded: quanta_index_contract::GoTestTraceFileQueryResponseV1 =
        quanta_index_ipc::decode_cbor_payload(&wire)?;
    if decoded != files {
        return Err("Go file projection changed on the public wire".into());
    }
    Ok(())
}

#[test]
fn go_test_trace_dispatch_keeps_frame_class_and_repo_binding() -> TestResult {
    let dispatcher = dispatcher_with_obs(
        Arc::new(StubLexicalOpener {
            results: vec![go_file_candidate("source", "src/handler.go", 42, 1.0)],
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::new(Mutex::new(RecordingSemanticState {
                constrained_search_results: Some(Vec::new()),
                ..RecordingSemanticState::default()
            })),
        }),
        Arc::new(BoundedQueryObsStore::default()),
    )?;
    let cases = [
        (
            "src/handler.go:42: panic",
            Some(ready_pin().repo_id),
            GoTestTraceFrameClassV1::Exact,
            true,
        ),
        (
            "src/handler.go:42: panic",
            None,
            GoTestTraceFrameClassV1::Exact,
            false,
        ),
        (
            "src/handler.go:42: panic",
            Some(RepoId::new("other-repo")?),
            GoTestTraceFrameClassV1::Exact,
            false,
        ),
        (
            "src/handler.go:42: panic src/other.go:9: panic",
            Some(ready_pin().repo_id),
            GoTestTraceFrameClassV1::Ambiguous,
            false,
        ),
        (
            "src/handler_test.go:42: panic",
            Some(ready_pin().repo_id),
            GoTestTraceFrameClassV1::Unresolved,
            false,
        ),
    ];
    for (failure, owner, frame_class, source_frame) in cases {
        let request = go_file_request(failure, owner);
        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::GoTestTraceFileV1(request.clone()),
            &RequestBudgetV1::unbounded(),
        );
        let SearchPlaneQueryIpcResponse::GoTestTraceFileV1(files) = response else {
            return Err(format!("expected Go file response, got {response:?}").into());
        };
        files.validate_against_v1(&request)?;
        if files.frame_class != frame_class
            || files.results.len() != 1
            || files.results[0].source_frame != source_frame
        {
            return Err(
                format!("failure={failure:?}: frame owner/class changed: {files:?}").into(),
            );
        }
    }
    Ok(())
}

#[test]
fn go_test_trace_dispatch_stabilizes_source_file_ties_before_chunk_cap() -> TestResult {
    let canonical = go_file_candidate("a-tie", "src/a.go", 1, 0.5);
    let mut foreign = canonical.clone();
    foreign.source_repo_id = RepoId::new("z-other-source")?;
    let rows = vec![
        go_file_candidate("one", "src/one.go", 1, 1.0),
        go_file_candidate("two", "src/two.go", 1, 0.9),
        go_file_candidate("three", "src/three.go", 1, 0.8),
        go_file_candidate("four", "src/four.go", 1, 0.7),
        // Both lanes return z and the foreign a first. Only the a in the
        // lower source repo belongs above the chunk cap.
        go_file_candidate("z-tie", "src/z.go", 1, 0.5),
        foreign,
        canonical.clone(),
    ];
    let dispatcher = dispatcher_with_obs(
        Arc::new(StubLexicalOpener {
            results: rows.clone(),
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::new(Mutex::new(RecordingSemanticState {
                constrained_search_results: Some(rows),
                ..RecordingSemanticState::default()
            })),
        }),
        Arc::new(BoundedQueryObsStore::default()),
    )?;
    let mut request = go_file_request("test failure", None);
    request.query.top_k = 5;
    request.query.text_query.top_k = 5;
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::GoTestTraceFileV1(request.clone()),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::GoTestTraceFileV1(files) = response else {
        return Err(format!("expected Go file response, got {response:?}").into());
    };
    files.validate_against_v1(&request)?;
    let boundary = files.results.iter().find(|hit| {
        hit.file.source_repo_id == canonical.source_repo_id
            && hit.file.repo_relative_path == canonical.repo_relative_path
    });
    if files.lexical_window.returned() != 5
        || files.dense_window.returned() != 5
        || files.dense_window.outcome() != (ExecutionOutcomeV2::LowerBound { continuation: true })
        || !boundary.is_some_and(|hit| hit.lexical.is_some() && hit.dense.is_some())
        || files.results.iter().any(|hit| {
            hit.file.source_repo_id.as_str() == "z-other-source"
                || hit.file.repo_relative_path.as_str() == "src/z.go"
        })
    {
        return Err(format!("Go file tie was cut in backend order: {files:?}").into());
    }
    Ok(())
}

#[test]
fn go_test_trace_dispatch_counts_only_filter_exclusions_after_dense_refill() -> TestResult {
    let mut admitted = std::collections::BTreeSet::new();
    let mut dense = Vec::new();
    for index in 0_u16..50 {
        let id = format!("row-{index}");
        let directory = if index < 5 { "docs" } else { "src" };
        if index >= 5 {
            let _inserted = admitted.insert(id.clone());
        }
        // All scores tie. The last admitted row belongs first by canonical
        // order and must survive even the internal depth cut of 25 rows.
        let path = if index == 49 {
            "src/00-last.go".to_string()
        } else {
            format!("{directory}/file-{index}.go")
        };
        dense.push(go_file_candidate(&id, &path, 1, 1.0));
    }
    let semantic = Arc::new(Mutex::new(RecordingSemanticState {
        constrained_search_results: Some(dense),
        ..RecordingSemanticState::default()
    }));
    let dispatcher = dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::new(Mutex::new(RecordingLexicalState {
                admitted_ids: Some(admitted),
                ..RecordingLexicalState::default()
            })),
            results: Vec::new(),
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&semantic),
        }),
        Arc::new(BoundedQueryObsStore::default()),
    )?
    .with_hybrid_fetch_floor(crate::HybridFetchFloorPolicy::Floor25);
    let mut request = go_file_request("test failure", None);
    request.query.text_query.syntax = TextQuerySyntax::Sourcegraph;
    request.query.text_query.query_text = "file:src/ handler".to_string();
    request.query.text_query.top_k = 5;
    request.query.top_k = 5;
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::GoTestTraceFileV1(request.clone()),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::GoTestTraceFileV1(files) = response else {
        return Err(format!("expected Go file response, got {response:?}").into());
    };
    files.validate_against_v1(&request)?;
    let fetches = semantic
        .lock()
        .map_err(|error| format!("semantic state poisoned: {error}"))?
        .search_top_ks
        .clone();
    let [lane] = files.dense_window.coverage().lanes() else {
        return Err("expected exactly one Go dense lane".into());
    };
    if fetches != [quanta_index_core::HybridOrchestratorPolicy::dense_admission_examine_ceiling()]
        || files.dense_window.returned() != 5
        || lane.filtered_out() != 5
        || lane.cost() != Some(50)
        || files.results.len() != 5
        || files
            .results
            .first()
            .is_none_or(|hit| hit.file.repo_relative_path.as_str() != "src/00-last.go")
        || files
            .results
            .iter()
            .any(|hit| !hit.file.repo_relative_path.as_str().starts_with("src/"))
    {
        return Err(format!("unexpected Go dense refill {fetches:?}: {files:?}").into());
    }
    Ok(())
}
