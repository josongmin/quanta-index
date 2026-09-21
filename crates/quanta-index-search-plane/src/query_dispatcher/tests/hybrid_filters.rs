//! QI-BB-018 보완 #3: every DSL filter binds both hybrid lanes.
//!
//! The dense lane used to run under the typed constraint set alone, so a
//! `file:` / `repo:` / `type:` filter the lexical lane honoured let
//! dense-only rows the query excluded into the fused result. These tests
//! drive the hybrid and hybrid-seed routes over recording doubles and pin
//! the contract: exact filters admit each dense candidate through the
//! lexical plan (and only the plan's filters, never its expression), the
//! dense lane refills until the admitted rows fill its depth or the engine
//! is exhausted, an unsupported filter is refused typed with zero lane
//! calls, and the explanation names the push-down class per filter.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    CandidateCountV1, HybridQueryRequest, HybridSeedQueryRequest, LexicalCandidate, LqExpr,
    LqFilter, QueryConstraintSetV1, SearchExplanation, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SemanticCorpusKindV1, SemanticSeedCorpusBudgetV1,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{HYBRID_FILTER_UNSUPPORTED_CODE, RequestBudgetV1};

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::tests::support::common::{
    TestResult, candidate, ipc_error_from, ready_ledger, ready_pin, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState,
};
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapSnapshotPort;
use crate::query_dispatcher::tests::support::semantic::{
    RecordingSemanticOpener, RecordingSemanticState,
};
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

type BoxError = Box<dyn std::error::Error>;

/// A dispatcher over recording lanes, with the lane states it records into.
struct FilteredLanes {
    dispatcher: SearchPlaneDispatcher,
    lexical: Arc<Mutex<RecordingLexicalState>>,
    semantic: Arc<Mutex<RecordingSemanticState>>,
}

/// One admission evaluation the lexical double saw: the filter-only plan
/// and the candidate ids it was asked about.
type AdmissionCall = (quanta_index_contract::LqQuery, BTreeSet<String>);

/// A dispatcher over recording lanes.
///
/// The lexical page is `lexical_rows`, the dense engine answers
/// `dense_rows` (cut to each fetch), and the lexical plan admits exactly
/// `admitted` (or everything when `None`).
fn filtered_dispatcher(
    lexical_rows: Vec<LexicalCandidate>,
    dense_rows: Vec<LexicalCandidate>,
    admitted: Option<&[&str]>,
) -> Result<FilteredLanes, BoxError> {
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState {
        admitted_ids: admitted.map(|ids| ids.iter().map(|id| (*id).to_string()).collect()),
        ..RecordingLexicalState::default()
    }));
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState {
        constrained_search_results: Some(dense_rows),
        ..RecordingSemanticState::default()
    }));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&lexical_state),
            results: lexical_rows,
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&semantic_state),
        }),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );
    Ok(FilteredLanes {
        dispatcher,
        lexical: lexical_state,
        semantic: semantic_state,
    })
}

fn hybrid_request(query_text: &str, top_k: u32) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: query_text.to_string(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 50,
            cursor: None,
        },
        semantic_query_text: "needle".to_string(),
        generation: Some(ready_pin()),
        generation_selector: None,
        top_k,
    })
}

fn hybrid_seed_request(
    query_text: &str,
    dense_corpora: Vec<SemanticSeedCorpusBudgetV1>,
    top_k: u32,
) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::HybridSeed(HybridSeedQueryRequest {
        text_query: TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: query_text.to_string(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(ready_pin()),
            generation_selector: None,
            top_k: 50,
            cursor: None,
        },
        semantic_query_text: "needle".to_string(),
        generation: Some(ready_pin()),
        generation_selector: None,
        dense_corpora,
        top_k,
    })
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every response but the one the route serves is the same test failure"
)]
fn hybrid_response(
    response: SearchPlaneQueryIpcResponse,
) -> Result<quanta_index_contract::HybridQueryResponse, BoxError> {
    match response {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => Ok(hybrid),
        other => Err(format!("expected Hybrid response, got {other:?}").into()),
    }
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every response but the one the route serves is the same test failure"
)]
fn hybrid_seed_response(
    response: SearchPlaneQueryIpcResponse,
) -> Result<quanta_index_contract::HybridSeedQueryResponse, BoxError> {
    match response {
        SearchPlaneQueryIpcResponse::HybridSeed(seed) => Ok(seed),
        other => Err(format!("expected HybridSeed response, got {other:?}").into()),
    }
}

fn trace_details(explanation: &SearchExplanation) -> Vec<&str> {
    explanation
        .planner_trace
        .iter()
        .map(|entry| entry.detail.as_str())
        .collect()
}

fn ids(set: &[&str]) -> BTreeSet<String> {
    set.iter().map(|id| (*id).to_string()).collect()
}

/// A dense row whose score falls with its rank.
fn ranked_dense_row(index: u16) -> LexicalCandidate {
    candidate(
        &format!("d{index:03}"),
        0.001_f32.mul_add(-f32::from(index), 1.0),
    )
}

/// The admission evaluations the lexical double saw, in call order.
fn admission_calls(
    state: &Arc<Mutex<RecordingLexicalState>>,
) -> Result<Vec<AdmissionCall>, BoxError> {
    Ok(state
        .lock()
        .map_err(|err| format!("lexical state poisoned: {err}"))?
        .admission_calls
        .clone())
}

fn dense_fetch_sizes(state: &Arc<Mutex<RecordingSemanticState>>) -> Result<Vec<u32>, BoxError> {
    Ok(state
        .lock()
        .map_err(|err| format!("semantic state poisoned: {err}"))?
        .search_top_ks
        .clone())
}

// CASE-COVERS: QI-BB-018 보완 #3 — a dense-only candidate the `file:` filter
// excludes never reaches the fused result; the admission plan is the
// query's filters alone.
#[test]
fn hybrid_dense_only_candidate_excluded_by_a_file_filter_never_appears() -> TestResult {
    // Lexical lane (`file:src/lib.rs needle`): alpha. Dense lane over the
    // whole generation: alpha, beta, gamma — beta and gamma live in other
    // files, which the lexical plan proves by admitting alpha only.
    let lanes = filtered_dispatcher(
        vec![candidate("alpha", 1.0)],
        vec![
            candidate("beta", 0.9),
            candidate("alpha", 0.8),
            candidate("gamma", 0.7),
        ],
        Some(&["alpha"]),
    )?;
    let response = hybrid_response(lanes.dispatcher.dispatch(
        hybrid_request("file:src/lib.rs needle", 10),
        &RequestBudgetV1::unbounded(),
    ))?;
    let fused = response
        .results
        .iter()
        .map(|row| row.candidate.candidate_id.as_str())
        .collect::<Vec<_>>();
    if fused != ["alpha"] {
        return Err(format!("dense rows the file filter excludes leaked: {fused:?}").into());
    }
    // alpha was seen by both lanes: the dense rank is its rank among the
    // admitted rows (1), not among the raw dense rows (2).
    let alpha = response
        .results
        .first()
        .ok_or("alpha row")?
        .contributions
        .iter()
        .map(|contribution| (contribution.lane, contribution.rank))
        .collect::<Vec<_>>();
    if alpha
        != [
            (quanta_index_contract::HybridLaneV1::Lexical, 1),
            (quanta_index_contract::HybridLaneV1::Dense, 1),
        ]
    {
        return Err(
            format!("alpha provenance must rank within the admitted lane: {alpha:?}").into(),
        );
    }
    if response.window.returned() != 1
        || response.window.candidate_count() != CandidateCountV1::Exact(1)
        || response.window.has_more()
    {
        return Err(format!(
            "window must count the admitted union: {:?}",
            response.window
        )
        .into());
    }
    // The plan the dense candidates were admitted through is the query's
    // exact filters with an empty expression: the dense lane is not asked
    // to match `needle`.
    let calls = admission_calls(&lanes.lexical)?;
    let [(plan, asked)] = calls.as_slice() else {
        return Err(format!("expected one admission round, got {}", calls.len()).into());
    };
    if plan.expr != LqExpr::Empty {
        return Err(format!("admission plan must carry no expression: {:?}", plan.expr).into());
    }
    if !matches!(plan.filters.as_slice(), [LqFilter::File { pattern, .. }] if pattern == "src/lib.rs")
    {
        return Err(format!(
            "admission plan must carry the file filter: {:?}",
            plan.filters
        )
        .into());
    }
    if *asked != ids(&["alpha", "beta", "gamma"]) {
        return Err(format!("every dense candidate must be evaluated: {asked:?}").into());
    }
    let details = trace_details(&response.explanation);
    for expected in [
        "hybrid.filters=exact:file",
        "hybrid.dense_admission=exhausted; examined=3; admitted=1",
    ] {
        if !details.contains(&expected) {
            return Err(format!("trace must state {expected}: {details:?}").into());
        }
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — `repo:` excluding the generation empties
// the dense lane too: an honest empty window, not an error and not the
// dense rows.
#[test]
fn hybrid_repo_filter_excluding_the_generation_empties_both_lanes() -> TestResult {
    let lanes = filtered_dispatcher(
        Vec::new(),
        vec![candidate("alpha", 0.9), candidate("beta", 0.8)],
        Some(&[]),
    )?;
    let response = hybrid_response(lanes.dispatcher.dispatch(
        hybrid_request("repo:^other$ needle", 10),
        &RequestBudgetV1::unbounded(),
    ))?;
    if !response.results.is_empty() {
        return Err(format!(
            "a repo the query excluded must not answer from the dense lane: {:?}",
            response.results
        )
        .into());
    }
    if response.window != quanta_index_contract::QueryResultWindowV1::exact(0) {
        return Err(format!("expected an honest empty window: {:?}", response.window).into());
    }
    if response.explanation.strategy != "empty" {
        return Err(format!(
            "expected an empty strategy: {}",
            response.explanation.strategy
        )
        .into());
    }
    let calls = admission_calls(&lanes.lexical)?;
    if calls.len() != 1
        || !matches!(
            calls.first().map(|(plan, _)| plan.filters.as_slice()),
            Some([LqFilter::Repo { pattern, .. }]) if pattern == "^other$"
        )
    {
        return Err(format!("the repo filter must reach the admission plan: {calls:?}").into());
    }
    let details = trace_details(&response.explanation);
    if !details.contains(&"hybrid.filters=exact:repo") {
        return Err(format!("trace must class repo as exact: {details:?}").into());
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — `type:file` and `lang:` bind the dense
// lane under their classes: lang pushed down typed, type evaluated exactly.
#[test]
fn hybrid_type_and_lang_filters_are_pushed_down_by_class() -> TestResult {
    let lanes = filtered_dispatcher(
        vec![candidate("alpha", 1.0)],
        vec![candidate("symbol-only", 0.9), candidate("alpha", 0.8)],
        Some(&["alpha"]),
    )?;
    let response = hybrid_response(lanes.dispatcher.dispatch(
        hybrid_request("type:file lang:rust needle", 10),
        &RequestBudgetV1::unbounded(),
    ))?;
    let fused = response
        .results
        .iter()
        .map(|row| row.candidate.candidate_id.as_str())
        .collect::<Vec<_>>();
    if fused != ["alpha"] {
        return Err(format!("a dense row that is not a text document leaked: {fused:?}").into());
    }
    let dense_constraints = lanes
        .semantic
        .lock()
        .map_err(|err| format!("semantic state poisoned: {err}"))?
        .search_constraints
        .clone();
    let rust = quanta_index_contract::lex::LanguageCode::new("rust").map_err(str::to_string)?;
    if dense_constraints.as_slice() != [QueryConstraintSetV1::from_languages([rust])] {
        return Err(format!("lang must reach the dense lane typed: {dense_constraints:?}").into());
    }
    let calls = admission_calls(&lanes.lexical)?;
    if !matches!(
        calls.first().map(|(plan, _)| plan.filters.as_slice()),
        Some([LqFilter::Type {
            kind: quanta_index_contract::LqType::File
        }])
    ) {
        return Err(format!("type must reach the admission plan alone: {calls:?}").into());
    }
    let details = trace_details(&response.explanation);
    if !details.contains(&"hybrid.filters=pushdown:lang; exact:type:file") {
        return Err(format!("trace must state the class per filter: {details:?}").into());
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — the dense lane refills until the admitted
// rows fill the page, asking about each identity once, and the window
// says more exist.
#[test]
fn hybrid_dense_lane_refills_until_admitted_rows_fill_top_k() -> TestResult {
    // 150 dense rows; the filter admits only the 50 the engine ranks past
    // the first fetch of 100 (top_k=2 -> internal depth 100).
    let dense = (0_u16..150).map(ranked_dense_row).collect::<Vec<_>>();
    let admitted = (100..150)
        .map(|index| format!("d{index:03}"))
        .collect::<Vec<_>>();
    let admitted_refs = admitted.iter().map(String::as_str).collect::<Vec<_>>();
    let lanes = filtered_dispatcher(Vec::new(), dense, Some(&admitted_refs))?;
    let response = hybrid_response(lanes.dispatcher.dispatch(
        hybrid_request("file:src/tail.rs needle", 2),
        &RequestBudgetV1::unbounded(),
    ))?;
    let fused = response
        .results
        .iter()
        .map(|row| row.candidate.candidate_id.as_str())
        .collect::<Vec<_>>();
    if fused != ["d100", "d101"] {
        return Err(format!("top_k must be filled from admitted rows: {fused:?}").into());
    }
    if response.window.returned() != 2
        || response.window.candidate_count() != CandidateCountV1::AtLeast(3)
        || !response.window.has_more()
    {
        return Err(format!(
            "window must say more admitted rows exist: {:?}",
            response.window
        )
        .into());
    }
    // One fetch of the depth admitted nothing; the refill doubled it and
    // the engine answered fewer rows than asked — exhausted.
    let fetches = dense_fetch_sizes(&lanes.semantic)?;
    if fetches != [100, 200] {
        return Err(format!("expected one refill at twice the depth: {fetches:?}").into());
    }
    let calls = admission_calls(&lanes.lexical)?;
    let asked = calls
        .iter()
        .map(|(_, asked)| asked.len())
        .collect::<Vec<_>>();
    if asked != [100, 50] {
        return Err(format!("each identity must be evaluated once: {asked:?}").into());
    }
    let details = trace_details(&response.explanation);
    if !details.contains(&"hybrid.dense_admission=exhausted; examined=150; admitted=50") {
        return Err(format!("trace must state the refill outcome: {details:?}").into());
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — a lane the filters fill at the first
// fetch neither refills nor over-claims; one the ceiling caps says so.
#[test]
fn hybrid_dense_lane_stops_when_filled_and_names_a_capped_lane() -> TestResult {
    let dense = (0_u16..120).map(ranked_dense_row).collect::<Vec<_>>();
    let lanes = filtered_dispatcher(Vec::new(), dense, None)?;
    let response = hybrid_response(lanes.dispatcher.dispatch(
        hybrid_request("file:src needle", 2),
        &RequestBudgetV1::unbounded(),
    ))?;
    if dense_fetch_sizes(&lanes.semantic)? != [100] {
        return Err("a filled lane must not refill".into());
    }
    let details = trace_details(&response.explanation);
    if !details.contains(&"hybrid.dense_admission=filled; examined=100; admitted=100") {
        return Err(format!("trace must state the filled lane: {details:?}").into());
    }
    if response.window.candidate_count() != CandidateCountV1::AtLeast(3) {
        return Err(format!("a filled lane proves more rows: {:?}", response.window).into());
    }

    // The engine always answers a full fetch and the filter admits nothing:
    // the loop reaches the ceiling and the trace says the lane is capped.
    let ceiling = quanta_index_core::HybridOrchestratorPolicy::dense_admission_examine_ceiling();
    let dense = (0..ceiling)
        .map(|index| candidate(&format!("d{index:05}"), 0.5))
        .collect::<Vec<_>>();
    let lanes = filtered_dispatcher(vec![candidate("lexical-only", 1.0)], dense, Some(&[]))?;
    let response = hybrid_response(lanes.dispatcher.dispatch(
        hybrid_request("file:src/nowhere.rs needle", 2),
        &RequestBudgetV1::unbounded(),
    ))?;
    let fused = response
        .results
        .iter()
        .map(|row| row.candidate.candidate_id.as_str())
        .collect::<Vec<_>>();
    if fused != ["lexical-only"] {
        return Err(format!("a capped lane admits nothing it examined: {fused:?}").into());
    }
    let fetches = dense_fetch_sizes(&lanes.semantic)?;
    if fetches.last() != Some(&ceiling) || fetches.iter().any(|size| *size > ceiling) {
        return Err(format!("refills must end at the ceiling: {fetches:?}").into());
    }
    let details = trace_details(&response.explanation);
    let capped = format!("hybrid.dense_admission=capped; examined={ceiling}; admitted=0");
    if !details.contains(&capped.as_str()) {
        return Err(format!("trace must name the capped lane: {details:?}").into());
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 / item 3 — a filter or option no lane can
// apply to dense rows is refused typed before any lane runs.
#[test]
fn hybrid_unsupported_filter_is_refused_typed_with_zero_lane_calls() -> TestResult {
    for query_text in [
        "select:file needle",
        "count:5 needle",
        "type:path needle",
        "rev:main needle",
    ] {
        let lanes = filtered_dispatcher(
            vec![candidate("alpha", 1.0)],
            vec![candidate("alpha", 0.9)],
            None,
        )?;
        let response = lanes.dispatcher.dispatch(
            hybrid_request(query_text, 10),
            &RequestBudgetV1::unbounded(),
        );
        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != HYBRID_FILTER_UNSUPPORTED_CODE {
            return Err(format!(
                "{query_text}: expected {HYBRID_FILTER_UNSUPPORTED_CODE}, got {code}: {message}"
            )
            .into());
        }
        let lexical = lanes
            .lexical
            .lock()
            .map_err(|err| format!("lexical state poisoned: {err}"))?;
        if !lexical.opened_pins.is_empty()
            || !lexical.search_top_ks.is_empty()
            || !lexical.admission_calls.is_empty()
        {
            return Err(format!("{query_text}: the lexical lane must not run").into());
        }
        drop(lexical);
        let semantic = lanes
            .semantic
            .lock()
            .map_err(|err| format!("semantic state poisoned: {err}"))?;
        if !semantic.search_vectors.is_empty()
            || !semantic.cluster_membership_opened_pins.is_empty()
        {
            return Err(format!("{query_text}: the dense lane must not run").into());
        }
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — the hybrid-seed route binds its dense
// lanes under the same contract: an excluded dense-only entity never
// seeds, and a corpus whose rows the filters excluded is not "unavailable".
#[test]
fn hybrid_seed_lanes_apply_the_same_filter_contract() -> TestResult {
    // Global lane: the double's `semantic-inline` hit, which the file
    // filter excludes; the lexical page is alpha alone.
    let lanes = filtered_dispatcher(vec![candidate("alpha", 1.0)], Vec::new(), Some(&["alpha"]))?;
    let response = hybrid_seed_response(lanes.dispatcher.dispatch(
        hybrid_seed_request("file:src/lib.rs needle", Vec::new(), 10),
        &RequestBudgetV1::unbounded(),
    ))?;
    let seeds = response
        .seed_candidates
        .iter()
        .map(|seed| seed.entity_id.as_str())
        .collect::<Vec<_>>();
    if seeds != ["alpha"] {
        return Err(format!("an excluded dense-only entity seeded: {seeds:?}").into());
    }
    let calls = admission_calls(&lanes.lexical)?;
    if calls.len() != 1 || calls.first().map(|(_, asked)| asked) != Some(&ids(&["semantic-inline"]))
    {
        return Err(format!("the global lane must be admitted per hit: {calls:?}").into());
    }
    let details = trace_details(&response.explanation);
    for expected in [
        "hybrid_seed.filters=exact:file",
        "hybrid_seed.dense_admission[global]=exhausted; examined=1; admitted=0",
    ] {
        if !details.contains(&expected) {
            return Err(format!("seed trace must state {expected}: {details:?}").into());
        }
    }

    // Corpus lanes: SymbolCard answers one hit the filter excludes,
    // RepositorySummary answers nothing. Only the empty lane is
    // unavailable; the excluded one was examined.
    let lanes = filtered_dispatcher(vec![candidate("alpha", 1.0)], Vec::new(), Some(&["alpha"]))?;
    let response = hybrid_seed_response(lanes.dispatcher.dispatch(
        hybrid_seed_request(
            "file:src/lib.rs needle",
            vec![
                SemanticSeedCorpusBudgetV1 {
                    corpus_kind: SemanticCorpusKindV1::SymbolCard,
                    top_k: 7,
                },
                SemanticSeedCorpusBudgetV1 {
                    corpus_kind: SemanticCorpusKindV1::RepositorySummary,
                    top_k: 11,
                },
            ],
            10,
        ),
        &RequestBudgetV1::unbounded(),
    ))?;
    let seeds = response
        .seed_candidates
        .iter()
        .map(|seed| seed.entity_id.as_str())
        .collect::<Vec<_>>();
    if seeds != ["alpha"] {
        return Err(format!("an excluded corpus entity seeded: {seeds:?}").into());
    }
    let degraded = response
        .seed_candidates
        .iter()
        .flat_map(|seed| seed.degraded_reasons.iter().map(String::as_str))
        .collect::<Vec<_>>();
    if degraded != ["requested_semantic_corpus_unavailable:RepositorySummary"] {
        return Err(
            format!("only the lane with nothing to examine is unavailable: {degraded:?}").into(),
        );
    }
    let details = trace_details(&response.explanation);
    for expected in [
        "hybrid_seed.dense_admission[SymbolCard]=exhausted; examined=1; admitted=0",
        "hybrid_seed.dense_admission[RepositorySummary]=exhausted; examined=0; admitted=0",
    ] {
        if !details.contains(&expected) {
            return Err(format!("seed trace must state {expected}: {details:?}").into());
        }
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — the hybrid-seed route refuses an
// unsupported filter typed with zero lane calls, like the hybrid route.
#[test]
fn hybrid_seed_unsupported_filter_is_refused_typed_with_zero_lane_calls() -> TestResult {
    let lanes = filtered_dispatcher(vec![candidate("alpha", 1.0)], Vec::new(), None)?;
    let response = lanes.dispatcher.dispatch(
        hybrid_seed_request("select:symbol needle", Vec::new(), 10),
        &RequestBudgetV1::unbounded(),
    );
    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != HYBRID_FILTER_UNSUPPORTED_CODE {
        return Err(format!("expected {HYBRID_FILTER_UNSUPPORTED_CODE}, got {code}").into());
    }
    if !lanes
        .lexical
        .lock()
        .map_err(|err| format!("lexical state poisoned: {err}"))?
        .opened_pins
        .is_empty()
    {
        return Err("the lexical lane must not open".into());
    }
    if !lanes
        .semantic
        .lock()
        .map_err(|err| format!("semantic state poisoned: {err}"))?
        .search_hit_vectors
        .is_empty()
    {
        return Err("the dense lane must not run".into());
    }
    Ok(())
}
