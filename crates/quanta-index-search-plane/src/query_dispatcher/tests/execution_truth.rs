//! W10-R1 execution-truth oracle: every engine list on every response must
//! match the backend calls the recording doubles actually saw.
//!
//! These tests dispatch through real routes over recording adapters and
//! assert the full chain per response: adapter call counts, the
//! explanation's `engines_executed` / `engines_touched`, the window lane
//! traces' executed/contributed split, and the emitted fanout value. A
//! `force_empty` plan invokes nothing and must report nothing executed;
//! an executed zero-hit lane still counts everywhere.

use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    EngineTouched, ExplainCandidateV1, HybridCandidateV1, HybridLaneContributionV1, HybridLaneV1,
    HybridQueryRequest, HybridSeedQueryRequest, QueryConstraintSetV1, SearchExplanation,
    SearchPlaneExplainQueryRequest, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
    SemanticCorpusKindV1, SemanticQueryRequest, SemanticSeedCorpusBudgetV1, SymbolQueryRequest,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{RequestBudgetV1, RequestCorrelationV1};

use crate::observability::BoundedQueryObsStore;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::execution_trace::{LaneExecutionRecorderV1, LaneExecutionSummaryV1};
use crate::query_dispatcher::tests::support::common::{
    TestResult, candidate, dispatcher_with_obs, ready_pin,
};
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState,
};
use crate::query_dispatcher::tests::support::semantic::{
    RecordingSemanticOpener, RecordingSemanticState,
};

type BoxError = Box<dyn std::error::Error>;

/// A dispatcher over recording lanes, with the lane states and the metric
/// sink it records into.
struct TruthLanes {
    dispatcher: SearchPlaneDispatcher,
    lexical: Arc<Mutex<RecordingLexicalState>>,
    semantic: Arc<Mutex<RecordingSemanticState>>,
    obs: Arc<BoundedQueryObsStore>,
}

fn truth_dispatcher(
    lexical_rows: Vec<quanta_index_contract::LexicalCandidate>,
    semantic_rows: Vec<quanta_index_contract::LexicalCandidate>,
) -> Result<TruthLanes, BoxError> {
    truth_dispatcher_full(lexical_rows, semantic_rows, None, None)
}

/// Raw-state adjustment before the dispatcher is built.
type StateTune = dyn Fn(&mut RecordingLexicalState, &mut RecordingSemanticState);

/// A dispatcher over recording lanes with full double control.
///
/// The dense engine answers `semantic_rows`, the lexical plan admits
/// exactly `admitted` (or everything when `None`), and `tune` adjusts the
/// raw states before the dispatcher is built.
fn truth_dispatcher_full(
    lexical_rows: Vec<quanta_index_contract::LexicalCandidate>,
    semantic_rows: Vec<quanta_index_contract::LexicalCandidate>,
    admitted: Option<&[&str]>,
    tune: Option<&StateTune>,
) -> Result<TruthLanes, BoxError> {
    let lexical = Arc::new(Mutex::new(RecordingLexicalState {
        admitted_ids: admitted.map(|ids| ids.iter().map(|id| (*id).to_string()).collect()),
        ..RecordingLexicalState::default()
    }));
    let semantic = Arc::new(Mutex::new(RecordingSemanticState {
        constrained_search_results: Some(semantic_rows),
        ..RecordingSemanticState::default()
    }));
    {
        let mut lexical_guard = lexical
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        let mut semantic_guard = semantic
            .lock()
            .map_err(|err| format!("semantic state poisoned: {err}"))?;
        if let Some(tune) = tune {
            tune(&mut lexical_guard, &mut semantic_guard);
        }
    }
    let obs = Arc::new(BoundedQueryObsStore::default());
    let dispatcher = dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&lexical),
            results: lexical_rows,
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&semantic),
        }),
        obs.clone(),
    )?;
    Ok(TruthLanes {
        dispatcher,
        lexical,
        semantic,
        obs,
    })
}

/// Every lexical backend call the double saw, by method.
fn lexical_calls(lanes: &TruthLanes) -> Result<(usize, usize, usize, usize), BoxError> {
    let state = lanes
        .lexical
        .lock()
        .map_err(|err| format!("lexical state poisoned: {err}"))?;
    Ok((
        state.search_top_ks.len(),
        state.admission_calls.len(),
        state.presence_checks.len(),
        state.explained_candidates.len(),
    ))
}

/// Every semantic backend call the double saw: constrained searches,
/// hit searches, corpus searches, scoped searches, exact scores.
fn semantic_calls(lanes: &TruthLanes) -> Result<(usize, usize, usize, usize, usize), BoxError> {
    let state = lanes
        .semantic
        .lock()
        .map_err(|err| format!("semantic state poisoned: {err}"))?;
    Ok((
        state.search_top_ks.len(),
        state.search_hit_vectors.len(),
        state.corpus_searches.len(),
        state.scoped_vectors.len(),
        state.scored_candidates.len(),
    ))
}

fn total_lexical(lanes: &TruthLanes) -> Result<usize, BoxError> {
    let (searches, admissions, presence, explained) = lexical_calls(lanes)?;
    Ok(searches
        .saturating_add(admissions)
        .saturating_add(presence)
        .saturating_add(explained))
}

fn total_semantic(lanes: &TruthLanes) -> Result<usize, BoxError> {
    let (constrained, hits, corpora, scoped_searches, exact_scores) = semantic_calls(lanes)?;
    // `search_hits_for_corpus*` records both its hit vector and its corpus
    // entry: one backend call, counted once.
    Ok(constrained
        .saturating_add(hits)
        .saturating_add(scoped_searches)
        .saturating_add(exact_scores)
        .saturating_add(corpora.saturating_sub(hits.min(corpora))))
}

/// Executed and post-filter contribution fanouts, independently observed.
fn emitted_fanout(lanes: &TruthLanes) -> (Option<f64>, Option<f64>) {
    let samples = lanes.obs.snapshot();
    let value = |name: &str| {
        samples
            .iter()
            .find(|sample| &*sample.name == name)
            .map(|sample| sample.value)
    };
    (
        value("lq_engine_fanout_count"),
        value("lq_lane_contribution_count"),
    )
}

fn engines_of(explanation: &SearchExplanation) -> (&[EngineTouched], &[EngineTouched]) {
    (
        explanation.engines_executed.as_slice(),
        explanation.engines_touched.as_slice(),
    )
}

/// Assert the full truth chain for one response.
///
/// The explanation's engine sets must equal the adapter-observed
/// invocation/contribution sets, every lane trace must agree engine by
/// engine, and the emitted fanout must equal the executed engine count.
fn assert_truth_chain(
    what: &str,
    explanation: &SearchExplanation,
    lanes: &[(String, bool, bool)],
    fanout: (Option<f64>, Option<f64>),
    lexical_invocations: usize,
    semantic_invocations: usize,
    expect_lexical_contributed: bool,
    expect_semantic_contributed: bool,
) -> Result<(), BoxError> {
    let (executed, touched) = engines_of(explanation);
    let mut expect_executed = Vec::new();
    if lexical_invocations > 0 {
        expect_executed.push(EngineTouched::Lexical);
    }
    if semantic_invocations > 0 {
        expect_executed.push(EngineTouched::Semantic);
    }
    if executed != expect_executed.as_slice() {
        return Err(format!(
            "{what}: engines_executed {executed:?} != invoked engines {expect_executed:?}"
        )
        .into());
    }
    let mut expect_touched = Vec::new();
    if expect_lexical_contributed {
        expect_touched.push(EngineTouched::Lexical);
    }
    if expect_semantic_contributed {
        expect_touched.push(EngineTouched::Semantic);
    }
    if touched != expect_touched.as_slice() {
        return Err(format!(
            "{what}: engines_touched {touched:?} != contributed engines {expect_touched:?}"
        )
        .into());
    }
    for (name, executed, contributed) in lanes {
        let (expect_executed, expect_contributed) = if name.ends_with("lexical") {
            (lexical_invocations > 0, expect_lexical_contributed)
        } else if name.ends_with("dense") {
            (semantic_invocations > 0, expect_semantic_contributed)
        } else {
            return Err(format!("{what}: unknown lane {name}").into());
        };
        if *executed != expect_executed || *contributed != expect_contributed {
            return Err(format!(
                "{what}: lane {name} executed={executed} contributed={contributed}, want \
                 executed={expect_executed} contributed={expect_contributed}"
            )
            .into());
        }
    }
    let expect_fanout = f64::from(
        u32::try_from(expect_executed.len())
            .map_err(|err| format!("executed engine count overflow: {err}"))?,
    );
    if fanout.0 != Some(expect_fanout) {
        return Err(format!("{what}: executed fanout {:?} != {expect_fanout}", fanout.0).into());
    }
    let expect_contributed = f64::from(
        u32::try_from(expect_touched.len())
            .map_err(|err| format!("contributed lane count overflow: {err}"))?,
    );
    if fanout.1 != Some(expect_contributed) {
        return Err(format!(
            "{what}: contribution fanout {:?} != {expect_contributed}",
            fanout.1
        )
        .into());
    }
    Ok(())
}

fn lane_flags(
    response: &SearchPlaneQueryIpcResponse,
) -> Result<Vec<(String, bool, bool)>, BoxError> {
    use quanta_index_contract::LaneTraceV1;
    fn flags(lanes: &[LaneTraceV1]) -> Vec<(String, bool, bool)> {
        lanes
            .iter()
            .map(|lane| (lane.lane().to_string(), lane.executed(), lane.contributed()))
            .collect()
    }
    match response {
        SearchPlaneQueryIpcResponse::Semantic(response) => {
            Ok(flags(response.window.coverage().lanes()))
        }
        SearchPlaneQueryIpcResponse::Hybrid(response) => {
            Ok(flags(response.window.coverage().lanes()))
        }
        SearchPlaneQueryIpcResponse::HybridSeed(response) => {
            Ok(flags(response.window.coverage().lanes()))
        }
        // The explain response carries no window: its truth chain is the
        // explanation plus the adapter counts, asserted by the caller.
        SearchPlaneQueryIpcResponse::Explain(_) => Ok(Vec::new()),
        SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::Error(_) => {
            Err("execution truth covers the windowed and explain responses only".into())
        }
    }
}

fn rust_constraints() -> QueryConstraintSetV1 {
    QueryConstraintSetV1::from_languages([
        quanta_index_contract::lex::LanguageCode::new("rust").expect("valid language")
    ])
}

fn text_query(query_text: &str, constraints: QueryConstraintSetV1) -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: query_text.to_string(),
        constraints,
        generation: Some(ready_pin()),
        generation_selector: None,
        top_k: 10,
        cursor: None,
    }
}

// CASE-COVERS: direct lexical and symbol routes derive fanout from actual
// backend calls, including a successful force_empty response with no call.
#[test]
fn direct_lexical_routes_report_actual_backend_activity() -> TestResult {
    for (label, query, constraints, rows, expected_calls, expected_fanout) in [
        (
            "force_empty",
            "lang:python needle",
            rust_constraints(),
            vec![candidate("lex-a", 1.0)],
            0,
            (Some(0.0), Some(0.0)),
        ),
        (
            "zero_hit",
            "needle",
            QueryConstraintSetV1::unconstrained(),
            Vec::new(),
            1,
            (Some(1.0), Some(0.0)),
        ),
        (
            "hit",
            "needle",
            QueryConstraintSetV1::unconstrained(),
            vec![candidate("lex-a", 1.0)],
            1,
            (Some(1.0), Some(1.0)),
        ),
    ] {
        let lanes = truth_dispatcher(rows.clone(), Vec::new())?;
        let response = lanes.dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(text_query(query, constraints.clone())),
            &RequestBudgetV1::unbounded(),
        );
        if !matches!(response, SearchPlaneQueryIpcResponse::Text(_)) {
            return Err(format!("{label}: expected text response, got {response:?}").into());
        }
        let calls = lanes
            .lexical
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?
            .search_top_ks
            .len();
        if calls != expected_calls || emitted_fanout(&lanes) != expected_fanout {
            return Err(format!(
                "{label}: text calls={calls}, fanout={:?}; expected calls={expected_calls}, fanout={expected_fanout:?}",
                emitted_fanout(&lanes),
            )
            .into());
        }

        let lanes = truth_dispatcher(rows, Vec::new())?;
        let response = lanes.dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Symbol(SymbolQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: query.to_string(),
                constraints,
                generation: Some(ready_pin()),
                generation_selector: None,
                top_k: 10,
                cursor: None,
            }),
            &RequestBudgetV1::unbounded(),
        );
        if !matches!(response, SearchPlaneQueryIpcResponse::Symbol(_)) {
            return Err(format!("{label}: expected symbol response, got {response:?}").into());
        }
        let calls = lanes
            .lexical
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?
            .symbol_top_ks
            .len();
        if calls != expected_calls || emitted_fanout(&lanes) != expected_fanout {
            return Err(format!(
                "{label}: symbol calls={calls}, fanout={:?}; expected calls={expected_calls}, fanout={expected_fanout:?}",
                emitted_fanout(&lanes),
            )
            .into());
        }
    }
    Ok(())
}

fn hybrid_request(
    query_text: &str,
    constraints: QueryConstraintSetV1,
) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
        text_query: text_query(query_text, constraints),
        semantic_query_text: "needle".to_string(),
        generation: Some(ready_pin()),
        generation_selector: None,
        top_k: 10,
    })
}

fn semantic_request(
    scope: Option<TextQueryRequest>,
    constraints: QueryConstraintSetV1,
) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
        query_text: "needle".to_string(),
        constraints,
        generation: Some(ready_pin()),
        generation_selector: None,
        lexical_scope: scope,
        top_k: 10,
    })
}

fn seed_request(
    query_text: &str,
    constraints: QueryConstraintSetV1,
    corpora: Vec<SemanticSeedCorpusBudgetV1>,
) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::HybridSeed(HybridSeedQueryRequest {
        text_query: text_query(query_text, constraints),
        semantic_query_text: "needle".to_string(),
        generation: Some(ready_pin()),
        generation_selector: None,
        dense_corpora: corpora,
        top_k: 10,
    })
}

fn explain_request(
    candidate: ExplainCandidateV1,
    text_query: Option<TextQueryRequest>,
    semantic_query_text: Option<&str>,
) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Explain(SearchPlaneExplainQueryRequest {
        generation: ready_pin(),
        candidate,
        text_query,
        semantic_query_text: semantic_query_text.map(str::to_string),
    })
}

// CASE-COVERS: W10-R1 — the recorder counts invocations, never plans.
#[test]
fn recorder_counts_backend_calls_and_nothing_else() {
    let recorder = LaneExecutionRecorderV1::new();
    assert_eq!(recorder.summary(), LaneExecutionSummaryV1::default());
    recorder.record_lexical_invocation();
    recorder.record_lexical_invocation();
    recorder.record_semantic_invocation();
    let summary = recorder.summary();
    assert_eq!(summary.lexical_invocations, 2);
    assert_eq!(summary.semantic_invocations, 1);
    assert!(!summary.lexical_contributed);
    assert!(!summary.semantic_contributed);
    assert_eq!(
        summary.executed_engines(),
        vec![EngineTouched::Lexical, EngineTouched::Semantic]
    );
    assert!(summary.touched_engines().is_empty());
    recorder.record_semantic_contribution();
    let summary = recorder.summary();
    assert_eq!(summary.touched_engines(), vec![EngineTouched::Semantic]);
}

// CASE-COVERS: W10-R1 — unscoped semantic truth over hits and zero hits.
#[test]
fn semantic_unscoped_truth_chain() -> TestResult {
    for (label, semantic_rows, expect_touched) in [
        ("hits", vec![candidate("sem-a", 1.0)], true),
        ("zero-hit", Vec::new(), false),
    ] {
        let lanes = truth_dispatcher(vec![candidate("lex-a", 1.0)], semantic_rows)?;
        let response = lanes.dispatcher.dispatch(
            semantic_request(None, QueryConstraintSetV1::unconstrained()),
            &RequestBudgetV1::unbounded(),
        );
        let SearchPlaneQueryIpcResponse::Semantic(semantic) = &response else {
            return Err(format!("{label}: expected Semantic response, got {response:?}").into());
        };
        let lexical = total_lexical(&lanes)?;
        let sem = total_semantic(&lanes)?;
        if lexical != 0 || sem != 1 {
            return Err(format!(
                "{label}: unscoped semantic must invoke once (semantic), saw lexical={lexical} semantic={sem}"
            )
            .into());
        }
        assert_truth_chain(
            label,
            &semantic.explanation,
            &lane_flags(&response)?,
            emitted_fanout(&lanes),
            lexical,
            sem,
            false,
            expect_touched,
        )?;
    }
    Ok(())
}

// CASE-COVERS: W10-R1 — scoped semantic truth, including a force_empty
// scope that invokes no lexical backend.
#[test]
fn semantic_scoped_truth_chain() -> TestResult {
    // A live scope: both backends invoked, both contribute.
    let lanes = truth_dispatcher(vec![candidate("lex-a", 1.0)], vec![candidate("sem-a", 1.0)])?;
    let scope = text_query("scope", QueryConstraintSetV1::unconstrained());
    let response = lanes.dispatcher.dispatch(
        semantic_request(Some(scope), QueryConstraintSetV1::unconstrained()),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::Semantic(semantic) = &response else {
        return Err(format!("expected Semantic response, got {response:?}").into());
    };
    let lexical = total_lexical(&lanes)?;
    let sem = total_semantic(&lanes)?;
    if lexical != 1 || sem != 1 {
        return Err(format!(
            "scoped semantic must invoke both backends once, saw lexical={lexical} semantic={sem}"
        )
        .into());
    }
    assert_truth_chain(
        "scoped",
        &semantic.explanation,
        &lane_flags(&response)?,
        emitted_fanout(&lanes),
        lexical,
        sem,
        true,
        true,
    )?;

    // A contradictory scope: the lexical backend is never invoked, so the
    // response reports the semantic engine alone.
    let lanes = truth_dispatcher(vec![candidate("lex-a", 1.0)], vec![candidate("sem-a", 1.0)])?;
    let scope = text_query("lang:python scope", rust_constraints());
    let response = lanes.dispatcher.dispatch(
        semantic_request(Some(scope), rust_constraints()),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::Semantic(semantic) = &response else {
        return Err(format!("expected Semantic response, got {response:?}").into());
    };
    let lexical = total_lexical(&lanes)?;
    let sem = total_semantic(&lanes)?;
    if lexical != 0 || sem != 1 {
        return Err(format!(
            "force_empty scope must invoke semantic only, saw lexical={lexical} semantic={sem}"
        )
        .into());
    }
    assert_truth_chain(
        "force_empty scope",
        &semantic.explanation,
        &lane_flags(&response)?,
        emitted_fanout(&lanes),
        lexical,
        sem,
        false,
        true,
    )?;
    Ok(())
}

// CASE-COVERS: W10-R1 — hybrid truth over hits, zero hits, and force_empty.
#[test]
fn hybrid_truth_chain() -> TestResult {
    for (label, lexical_rows, semantic_rows, expect_contributed) in [
        (
            "hits",
            vec![candidate("lex-a", 1.0)],
            vec![candidate("sem-a", 1.0)],
            (true, true),
        ),
        ("zero-hit", Vec::new(), Vec::new(), (false, false)),
    ] {
        let lanes = truth_dispatcher(lexical_rows, semantic_rows)?;
        let response = lanes.dispatcher.dispatch(
            hybrid_request("needle", QueryConstraintSetV1::unconstrained()),
            &RequestBudgetV1::unbounded(),
        );
        let SearchPlaneQueryIpcResponse::Hybrid(hybrid) = &response else {
            return Err(format!("{label}: expected Hybrid response, got {response:?}").into());
        };
        let lexical = total_lexical(&lanes)?;
        let sem = total_semantic(&lanes)?;
        if lexical != 1 || sem != 1 {
            return Err(format!(
                "{label}: hybrid must invoke both backends once, saw lexical={lexical} semantic={sem}"
            )
            .into());
        }
        assert_truth_chain(
            label,
            &hybrid.explanation,
            &lane_flags(&response)?,
            emitted_fanout(&lanes),
            lexical,
            sem,
            expect_contributed.0,
            expect_contributed.1,
        )?;
    }

    // A contradictory plan invokes nothing and reports nothing executed.
    let lanes = truth_dispatcher(vec![candidate("lex-a", 1.0)], vec![candidate("sem-a", 1.0)])?;
    let response = lanes.dispatcher.dispatch(
        hybrid_request("lang:python needle", rust_constraints()),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::Hybrid(hybrid) = &response else {
        return Err(format!("expected Hybrid response, got {response:?}").into());
    };
    let lexical = total_lexical(&lanes)?;
    let sem = total_semantic(&lanes)?;
    if lexical != 0 || sem != 0 {
        return Err(format!(
            "force_empty hybrid must invoke nothing, saw lexical={lexical} semantic={sem}"
        )
        .into());
    }
    assert_truth_chain(
        "force_empty",
        &hybrid.explanation,
        &lane_flags(&response)?,
        emitted_fanout(&lanes),
        0,
        0,
        false,
        false,
    )?;
    if hybrid
        .window
        .coverage()
        .lanes()
        .iter()
        .any(quanta_index_contract::LaneTraceV1::executed)
    {
        return Err("force_empty hybrid must leave every lane trace unexecuted".into());
    }
    Ok(())
}

// CASE-COVERS: W10-R1 — every admission fetch and every filter round
// counts; the engine sets still collapse to the two lanes.
#[test]
fn hybrid_filtered_refill_counts_every_backend_call() -> TestResult {
    // A deep dense page against a small admitted set: the first fetch
    // fills completely while staying starved, so the lane must refill.
    let mut dense_rows = vec![candidate("alpha", 0.9)];
    for index in 0..99 {
        dense_rows.push(candidate(
            Box::leak(format!("other-{index}").into_boxed_str()),
            0.8,
        ));
    }
    let lanes = truth_dispatcher_full(
        vec![candidate("alpha", 1.0)],
        dense_rows,
        Some(&["alpha"]),
        None,
    )?;
    let response = lanes.dispatcher.dispatch(
        hybrid_request("type:file needle", QueryConstraintSetV1::unconstrained()),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::Hybrid(hybrid) = &response else {
        return Err(format!("expected Hybrid response, got {response:?}").into());
    };
    let (searches, admissions, _, _) = lexical_calls(&lanes)?;
    let (constrained, _, _, _, _) = semantic_calls(&lanes)?;
    if searches != 1 || admissions == 0 {
        return Err(format!(
            "filtered hybrid must run the lexical lane once and evaluate filters, \
             saw searches={searches} admissions={admissions}"
        )
        .into());
    }
    if constrained < 2 {
        return Err(format!(
            "a starved dense lane must refill: saw {constrained} semantic fetches"
        )
        .into());
    }
    let lexical = total_lexical(&lanes)?;
    let sem = total_semantic(&lanes)?;
    assert_truth_chain(
        "refill",
        &hybrid.explanation,
        &lane_flags(&response)?,
        emitted_fanout(&lanes),
        lexical,
        sem,
        true,
        true,
    )?;
    Ok(())
}

// CASE-COVERS: W10-R1 — seed truth over the global lane, per-corpus lanes,
// and force_empty.
#[test]
fn hybrid_seed_truth_chain() -> TestResult {
    // Per-corpus lanes: one semantic invocation per corpus plus the
    // lexical lane.
    let lanes = truth_dispatcher(vec![candidate("lex-a", 1.0)], Vec::new())?;
    let corpora = vec![
        SemanticSeedCorpusBudgetV1 {
            corpus_kind: SemanticCorpusKindV1::SymbolCard,
            top_k: 7,
        },
        SemanticSeedCorpusBudgetV1 {
            corpus_kind: SemanticCorpusKindV1::ClusterCard,
            top_k: 7,
        },
    ];
    let response = lanes.dispatcher.dispatch(
        seed_request("needle", QueryConstraintSetV1::unconstrained(), corpora),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::HybridSeed(seed) = &response else {
        return Err(format!("expected HybridSeed response, got {response:?}").into());
    };
    let lexical = total_lexical(&lanes)?;
    let sem = total_semantic(&lanes)?;
    if lexical != 1 || sem != 2 {
        return Err(format!(
            "two-corpus seed must invoke lexical once and semantic twice, \
             saw lexical={lexical} semantic={sem}"
        )
        .into());
    }
    assert_truth_chain(
        "corpora",
        &seed.explanation,
        &lane_flags(&response)?,
        emitted_fanout(&lanes),
        lexical,
        sem,
        true,
        true,
    )?;

    // A contradictory plan invokes nothing and reports nothing executed.
    let lanes = truth_dispatcher(vec![candidate("lex-a", 1.0)], Vec::new())?;
    let response = lanes.dispatcher.dispatch(
        seed_request("lang:python needle", rust_constraints(), Vec::new()),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::HybridSeed(seed) = &response else {
        return Err(format!("expected HybridSeed response, got {response:?}").into());
    };
    let lexical = total_lexical(&lanes)?;
    let sem = total_semantic(&lanes)?;
    if lexical != 0 || sem != 0 {
        return Err(format!(
            "force_empty seed must invoke nothing, saw lexical={lexical} semantic={sem}"
        )
        .into());
    }
    assert_truth_chain(
        "force_empty",
        &seed.explanation,
        &lane_flags(&response)?,
        emitted_fanout(&lanes),
        0,
        0,
        false,
        false,
    )?;
    Ok(())
}

fn hybrid_candidate_for(id: &str) -> HybridCandidateV1 {
    HybridCandidateV1 {
        candidate: candidate(id, 1.25),
        fused_score: 1.0 / (60.0 + 1.0) + 1.0 / (60.0 + 2.0),
        contributions: vec![
            HybridLaneContributionV1 {
                lane: HybridLaneV1::Lexical,
                rank: 1,
                raw_score: 1.25,
            },
            HybridLaneContributionV1 {
                lane: HybridLaneV1::Dense,
                rank: 2,
                raw_score: 0.5,
            },
        ],
    }
}

// CASE-COVERS: W10-R1 — explain presence is one lexical lookup.
#[test]
fn explain_presence_truth_chain() -> TestResult {
    let lanes = truth_dispatcher(vec![candidate("alpha", 1.25)], Vec::new())?;
    let response = lanes.dispatcher.dispatch(
        explain_request(
            ExplainCandidateV1::Lexical(candidate("alpha", 1.25)),
            None,
            None,
        ),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::Explain(explain) = &response else {
        return Err(format!("expected Explain response, got {response:?}").into());
    };
    let lexical = total_lexical(&lanes)?;
    let sem = total_semantic(&lanes)?;
    if lexical != 1 || sem != 0 {
        return Err(format!(
            "presence must invoke lexical once, saw lexical={lexical} semantic={sem}"
        )
        .into());
    }
    assert_truth_chain(
        "presence",
        &explain.explanation,
        &lane_flags(&response)?,
        emitted_fanout(&lanes),
        lexical,
        sem,
        true,
        false,
    )?;
    Ok(())
}

// CASE-COVERS: W10-R1 — an explain refusal invokes no backend at all.
#[test]
fn explain_preflight_refusal_invokes_nothing() -> TestResult {
    let lanes = truth_dispatcher(vec![candidate("alpha", 1.25)], Vec::new())?;
    let response = lanes.dispatcher.dispatch(
        explain_request(
            ExplainCandidateV1::Hybrid(hybrid_candidate_for("alpha")),
            None,
            None,
        ),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::Error(error) = response else {
        return Err(format!("expected typed refusal, got {response:?}").into());
    };
    if error.code != quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest {
        return Err(format!("unexpected refusal code: {:?}", error.code).into());
    }
    let lexical = total_lexical(&lanes)?;
    let sem = total_semantic(&lanes)?;
    if lexical != 0 || sem != 0 {
        return Err(format!(
            "refused explain must invoke nothing, saw lexical={lexical} semantic={sem}"
        )
        .into());
    }
    Ok(())
}

// CASE-COVERS: W10-R1 — a hybrid explain records the trace, the exact
// dense score, and the re-run lanes.
#[test]
fn explain_hybrid_truth_chain() -> TestResult {
    let tune = |_lexical: &mut RecordingLexicalState, semantic: &mut RecordingSemanticState| {
        _ = semantic.stored_cosines.insert("alpha".to_string(), 0.5f32);
    };
    let lanes = truth_dispatcher_full(
        vec![candidate("alpha", 1.25)],
        vec![candidate("beta", 0.9), candidate("alpha", 0.5)],
        None,
        Some(&tune),
    )?;
    let response = lanes.dispatcher.dispatch(
        explain_request(
            ExplainCandidateV1::Hybrid(hybrid_candidate_for("alpha")),
            Some(text_query("scope", QueryConstraintSetV1::unconstrained())),
            Some("scope"),
        ),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::Explain(explain) = &response else {
        return Err(format!("expected Explain response, got {response:?}").into());
    };
    let (searches, _admissions, _presence, explained) = lexical_calls(&lanes)?;
    let (_constrained, _hits, _corpora, _scoped, scored) = semantic_calls(&lanes)?;
    // The lexical trace plus the re-run lexical lane; the exact dense
    // score plus at least the re-run dense fetch.
    if explained != 1 || searches != 1 {
        return Err(format!(
            "hybrid explain must trace once and re-run the lexical lane once, \
             saw explained={explained} searches={searches}"
        )
        .into());
    }
    if scored != 1 {
        return Err(format!("hybrid explain must score exactly once, saw {scored}").into());
    }
    let lexical = total_lexical(&lanes)?;
    let sem = total_semantic(&lanes)?;
    assert_truth_chain(
        "hybrid explain",
        &explain.explanation,
        &lane_flags(&response)?,
        emitted_fanout(&lanes),
        lexical,
        sem,
        true,
        true,
    )?;
    Ok(())
}

// CASE-COVERS: W10-R2 — one budget correlation reaches every route's
// explanation unchanged, and an uncorrelated budget stays 0 everywhere.
// The adapter no longer stamps: what the builders resolve IS the id.
#[test]
fn one_correlation_reaches_every_route_explanation() -> TestResult {
    fn explanation_id(response: &SearchPlaneQueryIpcResponse) -> Result<u64, BoxError> {
        match response {
            SearchPlaneQueryIpcResponse::Semantic(r) => Ok(r.explanation.request_id),
            SearchPlaneQueryIpcResponse::Hybrid(r) => Ok(r.explanation.request_id),
            SearchPlaneQueryIpcResponse::HybridSeed(r) => Ok(r.explanation.request_id),
            SearchPlaneQueryIpcResponse::Explain(r) => Ok(r.explanation.request_id),
            SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
            | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
            | SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
            | SearchPlaneQueryIpcResponse::Error(_) => {
                Err(format!("expected an explanation-carrying response, got {response:?}").into())
            }
        }
    }
    let corpora = vec![SemanticSeedCorpusBudgetV1 {
        corpus_kind: SemanticCorpusKindV1::SymbolCard,
        top_k: 7,
    }];
    let requests = |corpora: Vec<SemanticSeedCorpusBudgetV1>| {
        vec![
            (
                "semantic",
                semantic_request(None, QueryConstraintSetV1::unconstrained()),
            ),
            (
                "hybrid",
                hybrid_request("needle", QueryConstraintSetV1::unconstrained()),
            ),
            (
                "seed",
                seed_request("needle", QueryConstraintSetV1::unconstrained(), corpora),
            ),
            (
                "explain",
                explain_request(
                    ExplainCandidateV1::Lexical(candidate("alpha", 1.25)),
                    None,
                    None,
                ),
            ),
        ]
    };
    let Some(correlation) = RequestCorrelationV1::from_raw(77) else {
        return Err("77 is a nonzero test id".to_string().into());
    };
    for (name, request) in requests(corpora.clone()) {
        let lanes = truth_dispatcher(
            vec![candidate("alpha", 1.25)],
            vec![candidate("sem-a", 1.0)],
        )?;
        let budget = RequestBudgetV1::unbounded().with_correlation(correlation);
        let response = lanes.dispatcher.dispatch(request, &budget);
        let id = explanation_id(&response)?;
        if id != 77 {
            return Err(format!("{name}: correlated dispatch must carry 77, got {id}").into());
        }
    }
    for (name, request) in requests(corpora) {
        let lanes = truth_dispatcher(
            vec![candidate("alpha", 1.25)],
            vec![candidate("sem-a", 1.0)],
        )?;
        let response = lanes
            .dispatcher
            .dispatch(request, &RequestBudgetV1::unbounded());
        let id = explanation_id(&response)?;
        if id != 0 {
            return Err(format!("{name}: uncorrelated dispatch must stay 0, got {id}").into());
        }
    }
    Ok(())
}
