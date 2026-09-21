//! QI-BB-018 보완 #3 — every DSL filter binds both hybrid lanes, end to end.
//!
//! Three chunks share the dense query's words, so the dense lane ranks all
//! of them for every query here; what differs is the filter. The oracle is
//! the fixture itself: the set of chunks a filter admits is known from the
//! paths and languages they were ingested under, and a hybrid page may
//! carry exactly that set — a dense-only row the filter excludes is a leak,
//! whichever lane put it there. The same queries run on the hybrid-seed
//! route, whose entities are the same chunks.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::error::Error;

use crate::e2e_harness;
use quanta_index_contract::{
    CandidateCountV1, HybridSeedQueryRequest, PlannerStage, QueryConstraintSetV1,
    QueryResultWindowV1, SearchExplanation, SearchPlaneErrorCodeV2, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, TextQueryRequest, TextQuerySyntax,
};

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

/// The dense query: every fixture chunk carries these words.
const DENSE_QUERY: &str = "needle focus";

/// The three chunks and the id each was ingested under.
struct Fixture {
    alpha: String,
    beta: String,
    gamma: String,
}

impl Fixture {
    fn all(&self) -> BTreeSet<&str> {
        [self.alpha.as_str(), self.beta.as_str(), self.gamma.as_str()]
            .into_iter()
            .collect()
    }
}

/// alpha `src/lib.rs` (rust), beta `src/main.rs` (rust), gamma `src/lib.py`
/// (python): all three carry the lexical needle and the dense words.
fn ingest_fixture(rt: &mut E2eRuntime) -> Result<Fixture, Box<dyn Error>> {
    let alpha = rt.ingest_text_with_candidate_id("repo", "src/lib.rs", "needle focus lib alpha")?;
    let beta = rt.ingest_text_with_candidate_id("repo", "src/main.rs", "needle focus main beta")?;
    let gamma = rt.ingest_text_with_candidate_id("repo", "src/lib.py", "needle focus lib gamma")?;
    let _sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(Fixture { alpha, beta, gamma })
}

struct HybridPage {
    ids: Vec<String>,
    window: QueryResultWindowV1,
    explanation: SearchExplanation,
}

/// One hybrid query: the lexical lane runs `text_query`, the dense lane
/// [`DENSE_QUERY`]; a typed refusal is an `Err`.
fn hybrid_page(rt: &mut E2eRuntime, text_query: &str) -> Result<HybridPage, Box<dyn Error>> {
    hybrid_page_in(rt, TextQuerySyntax::Sourcegraph, text_query)
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every response but the one the route serves is the same test failure"
)]
fn hybrid_page_in(
    rt: &mut E2eRuntime,
    syntax: TextQuerySyntax,
    text_query: &str,
) -> Result<HybridPage, Box<dyn Error>> {
    let response = hybrid_response(rt, syntax, text_query)?;
    match response {
        SearchPlaneQueryIpcResponse::Hybrid(hybrid) => Ok(HybridPage {
            ids: hybrid
                .results
                .iter()
                .map(|row| row.candidate.candidate_id.clone())
                .collect(),
            window: hybrid.window,
            explanation: hybrid.explanation,
        }),
        SearchPlaneQueryIpcResponse::Error(error) => {
            Err(format!("hybrid `{text_query}` refused: {} {}", error.code, error.message).into())
        }
        other => Err(format!("hybrid `{text_query}`: unexpected response {other:?}").into()),
    }
}

fn hybrid_response(
    rt: &mut E2eRuntime,
    syntax: TextQuerySyntax,
    text_query: &str,
) -> Result<SearchPlaneQueryIpcResponse, Box<dyn Error>> {
    let response = rt.query_once(|pin| {
        SearchPlaneQueryIpcRequest::Hybrid(quanta_index_contract::HybridQueryRequest {
            text_query: TextQueryRequest {
                syntax,
                query_text: text_query.to_string(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: pin.clone(),
                generation_selector: None,
                top_k: 50,
                cursor: None,
            },
            semantic_query_text: DENSE_QUERY.to_string(),
            generation: pin,
            generation_selector: None,
            top_k: 10,
        })
    })?;
    Ok(response)
}

/// The typed code a hybrid query is refused with, or `Err` when it serves.
fn hybrid_refusal(
    rt: &mut E2eRuntime,
    syntax: TextQuerySyntax,
    text_query: &str,
) -> Result<SearchPlaneErrorCodeV2, Box<dyn Error>> {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "any served response variant is the same refusal-expectation failure"
    )]
    match hybrid_response(rt, syntax, text_query)? {
        SearchPlaneQueryIpcResponse::Error(error) => Ok(error.code),
        other => Err(format!("hybrid `{text_query}` served or returned {other:?}").into()),
    }
}

/// One hybrid-seed query over the global dense lane: the seeded entity ids
/// in seed order, and the explanation.
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "every response but the one the route serves is the same test failure"
)]
fn hybrid_seed_page(
    rt: &mut E2eRuntime,
    text_query: &str,
) -> Result<(Vec<String>, QueryResultWindowV1, SearchExplanation), Box<dyn Error>> {
    let response = rt.query_once(|pin| {
        SearchPlaneQueryIpcRequest::HybridSeed(HybridSeedQueryRequest {
            text_query: TextQueryRequest {
                syntax: TextQuerySyntax::Sourcegraph,
                query_text: text_query.to_string(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: pin.clone(),
                generation_selector: None,
                top_k: 50,
                cursor: None,
            },
            semantic_query_text: DENSE_QUERY.to_string(),
            generation: pin,
            generation_selector: None,
            dense_corpora: Vec::new(),
            top_k: 10,
        })
    })?;
    match response {
        SearchPlaneQueryIpcResponse::HybridSeed(seed) => Ok((
            seed.seed_candidates
                .iter()
                .map(|candidate| candidate.entity_id.clone())
                .collect(),
            seed.window,
            seed.explanation,
        )),
        SearchPlaneQueryIpcResponse::Error(error) => {
            Err(format!("hybrid-seed `{text_query}` refused: {} {}", error.code, error.message)
                .into())
        }
        other => Err(format!("hybrid-seed `{text_query}`: unexpected response {other:?}").into()),
    }
}

fn trace_detail(explanation: &SearchExplanation, prefix: &str) -> Option<String> {
    explanation
        .planner_trace
        .iter()
        .filter(|entry| matches!(entry.stage, PlannerStage::Plan | PlannerStage::ExecFanout))
        .find(|entry| entry.detail.starts_with(prefix))
        .map(|entry| entry.detail.clone())
}

fn as_set(ids: &[String]) -> BTreeSet<&str> {
    ids.iter().map(String::as_str).collect()
}

/// The fixture is what the tests assume: without a filter, the dense lane
/// ranks all three chunks, so any later exclusion is the filter's doing.
///
/// This first query also waits for the sealed generation to serve, so the
/// one-shot queries after it need no readiness retry.
fn assert_dense_lane_ranks_all_three(rt: &mut E2eRuntime, fixture: &Fixture) -> TestResult {
    let page = rt.query_hybrid(TextQuerySyntax::Sourcegraph, "needle", DENSE_QUERY, 10);
    if let Some(error) = page.typed_error {
        return Err(format!("the unfiltered hybrid query refused: {error}").into());
    }
    if as_set(&page.candidate_ids) != fixture.all() {
        return Err(format!(
            "the unfiltered hybrid page must carry all three: {:?}",
            page.candidate_ids
        )
        .into());
    }
    let explanation = page
        .explanation
        .ok_or("a served hybrid page carries an explanation")?;
    let lanes =
        trace_detail(&explanation, "hybrid.lanes=").ok_or("the hybrid trace names its lanes")?;
    if !lanes.contains("semantic_hits=3") {
        return Err(format!("the dense lane must rank all three chunks: {lanes}").into());
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — `file:` binds the dense lane: only the
// chunk in that file is served, though the dense lane ranks all three.
fn verify_file_filter(rt: &mut E2eRuntime, fixture: &Fixture) -> TestResult {
    let page = hybrid_page(rt, "file:src/lib.rs needle")?;
    if page.ids != [fixture.alpha.clone()] {
        return Err(format!(
            "file:src/lib.rs must serve alpha alone, got {:?} (beta={}, gamma={})",
            page.ids, fixture.beta, fixture.gamma
        )
        .into());
    }
    if page.window != QueryResultWindowV1::exact(1) {
        return Err(format!("expected an exact one-row window: {:?}", page.window).into());
    }
    let filters = trace_detail(&page.explanation, "hybrid.filters=")
        .ok_or("the hybrid trace states the push-down class per filter")?;
    if filters != "hybrid.filters=exact:file" {
        return Err(format!("file must be classed exact: {filters}").into());
    }
    let admission = trace_detail(&page.explanation, "hybrid.dense_admission=")
        .ok_or("the hybrid trace states the dense admission outcome")?;
    if admission != "hybrid.dense_admission=exhausted; examined=3; admitted=1" {
        return Err(
            format!("the dense lane must examine all three and admit one: {admission}").into()
        );
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — `repo:` excluding the generation answers
// an honest empty window, not the dense rows and not an error.
//
// The lexical repo predicate is a whole-term regex over the document's
// repo id, so `repo:other` names exactly the repo `other`; the fixture's
// generation belongs to another repo and no lane may answer from it.
fn verify_repo_filter(rt: &mut E2eRuntime, _fixture: &Fixture) -> TestResult {
    let page = hybrid_page(rt, "repo:other needle")?;
    if !page.ids.is_empty() {
        return Err(format!("a repo the query excluded must not be served: {:?}", page.ids).into());
    }
    if page.window != QueryResultWindowV1::exact(0)
        || page.window.candidate_count() != CandidateCountV1::Exact(0)
    {
        return Err(format!("expected an honest empty window: {:?}", page.window).into());
    }
    if page.explanation.strategy != "empty" || !page.explanation.engines_touched.is_empty() {
        return Err(format!(
            "an empty page claims no lane: {} {:?}",
            page.explanation.strategy, page.explanation.engines_touched
        )
        .into());
    }
    let filters = trace_detail(&page.explanation, "hybrid.filters=")
        .ok_or("the hybrid trace states the push-down class per filter")?;
    if filters != "hybrid.filters=exact:repo" {
        return Err(format!("repo must be classed exact: {filters}").into());
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — `lang:` is pushed down typed and `type:`
// evaluated exactly; each class is named in the trace.
fn verify_lang_and_type_filters(rt: &mut E2eRuntime, fixture: &Fixture) -> TestResult {
    let page = hybrid_page(rt, "lang:python needle")?;
    if page.ids != [fixture.gamma.clone()] {
        return Err(format!("lang:python must serve gamma alone: {:?}", page.ids).into());
    }
    let filters = trace_detail(&page.explanation, "hybrid.filters=")
        .ok_or("the hybrid trace states the push-down class per filter")?;
    if filters != "hybrid.filters=pushdown:lang" {
        return Err(format!("lang must be classed pushdown: {filters}").into());
    }

    // `type:file` admits text chunks: all three; `type:symbol` admits
    // symbol documents, of which the fixture has none — the dense chunks
    // are not symbols and must not stand in.
    let page = hybrid_page(rt, "type:file needle")?;
    if as_set(&page.ids) != fixture.all() {
        return Err(format!("type:file must serve every chunk: {:?}", page.ids).into());
    }
    let filters = trace_detail(&page.explanation, "hybrid.filters=")
        .ok_or("the hybrid trace states the push-down class per filter")?;
    if filters != "hybrid.filters=exact:type:file" {
        return Err(format!("type must be classed exact: {filters}").into());
    }
    let page = hybrid_page(rt, "type:symbol needle")?;
    if !page.ids.is_empty() {
        return Err(format!("type:symbol must not serve chunks: {:?}", page.ids).into());
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 / item 3 — a filter or option no lane can
// apply to dense rows is refused typed, never served lexical-only; and the
// native grammar has no negated filter, so `-file:` never reaches a lane.
fn verify_refusals(rt: &mut E2eRuntime, _fixture: &Fixture) -> TestResult {
    for query in ["select:file needle", "count:2 needle", "type:path needle"] {
        let code = hybrid_refusal(rt, TextQuerySyntax::Sourcegraph, query)?;
        if code.as_wire_str() != "HYBRID_FILTER_UNSUPPORTED" {
            return Err(
                format!("`{query}` must refuse HYBRID_FILTER_UNSUPPORTED, got {code}").into()
            );
        }
    }
    // The native grammar refuses a dash before a filter at parse time
    // (`NOT/- without expression`): a negated filter is not a filter either
    // lane could apply, and the refusal is the parser's.
    let code = hybrid_refusal(rt, TextQuerySyntax::Native, "-file:src/lib.rs needle")?;
    if code.as_wire_str() != "PARSE_FAIL" {
        return Err(format!("a negated filter must be refused by the grammar, got {code}").into());
    }
    Ok(())
}

// CASE-COVERS: QI-BB-018 보완 #3 — the hybrid-seed route binds its dense
// lane under the same contract, on the same fixture.
fn verify_hybrid_seed_filters(rt: &mut E2eRuntime, fixture: &Fixture) -> TestResult {
    let (seeds, _window, explanation) = hybrid_seed_page(rt, "needle")?;
    if as_set(&seeds) != fixture.all() {
        return Err(format!("the unfiltered seed set must carry all three: {seeds:?}").into());
    }
    let admission = trace_detail(&explanation, "hybrid_seed.dense_admission[global]=")
        .ok_or("the seed trace states the dense admission outcome")?;
    if admission != "hybrid_seed.dense_admission[global]=not_needed" {
        return Err(format!("no filter, no admission: {admission}").into());
    }

    let (seeds, window, explanation) = hybrid_seed_page(rt, "file:src/lib.rs needle")?;
    if seeds != [fixture.alpha.clone()] {
        return Err(format!("file:src/lib.rs must seed alpha alone: {seeds:?}").into());
    }
    if window != QueryResultWindowV1::exact(1) {
        return Err(format!("expected an exact one-row window: {window:?}").into());
    }
    let admission = trace_detail(&explanation, "hybrid_seed.dense_admission[global]=")
        .ok_or("the seed trace states the dense admission outcome")?;
    if admission != "hybrid_seed.dense_admission[global]=exhausted; examined=3; admitted=1" {
        return Err(format!("the seed dense lane must admit alpha alone: {admission}").into());
    }
    let filters = trace_detail(&explanation, "hybrid_seed.filters=")
        .ok_or("the seed trace states the push-down class per filter")?;
    if filters != "hybrid_seed.filters=exact:file" {
        return Err(format!("file must be classed exact: {filters}").into());
    }

    let (seeds, window, _explanation) = hybrid_seed_page(rt, "repo:other needle")?;
    if !seeds.is_empty() || window != QueryResultWindowV1::exact(0) {
        return Err(format!("an excluded repo must seed nothing: {seeds:?} {window:?}").into());
    }

    let (seeds, _window, _explanation) = hybrid_seed_page(rt, "lang:python needle")?;
    if seeds != [fixture.gamma.clone()] {
        return Err(format!("lang:python must seed gamma alone: {seeds:?}").into());
    }

    match hybrid_seed_page(rt, "select:file needle") {
        Ok((seeds, _, _)) => {
            return Err(
                format!("select:file must be refused on hybrid-seed, seeded {seeds:?}").into()
            );
        }
        Err(err) if err.to_string().contains("HYBRID_FILTER_UNSUPPORTED") => {}
        Err(err) => return Err(format!("unexpected hybrid-seed refusal: {err}").into()),
    }
    Ok(())
}

#[test]
fn hybrid_filters_share_one_indexed_fixture() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    let fixture = ingest_fixture(&mut rt)?;
    assert_dense_lane_ranks_all_three(&mut rt, &fixture)?;

    let verify_file_filter_fn: fn(&mut E2eRuntime, &Fixture) -> TestResult = verify_file_filter;
    for (name, verify) in [
        ("file_filter", verify_file_filter_fn),
        ("repo_filter", verify_repo_filter),
        ("lang_and_type_filters", verify_lang_and_type_filters),
        ("typed_refusals", verify_refusals),
        ("hybrid_seed_filters", verify_hybrid_seed_filters),
    ] {
        verify(&mut rt, &fixture)
            .map_err(|error| -> Box<dyn Error> { format!("{name}: {error}").into() })?;
    }
    Ok(())
}
