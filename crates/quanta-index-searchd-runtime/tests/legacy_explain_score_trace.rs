//! QI-BB-022 — explain traces the score a candidate earned under the query
//! that ranked it, and decides presence by exact lookup.
//!
//! The oracle for every trace is the search page itself: each candidate the
//! page carries must explain, under the same query, to one contribution row
//! whose sum is the carried score, in page order. A boost in the query must
//! appear as the row's weight and scale the page and the trace together,
//! and the ranker weights hash must change with it. Presence is a typed
//! field decided without a ranked re-search.

#![forbid(unsafe_code)]

use std::error::Error;
use std::time::Duration;

use quanta_index_contract::{
    CandidatePresenceV1, DiagnosticAttemptFieldV1, GenerationPin, HybridLaneV1, LexicalCandidate,
    ManifestGeneration, PlannerStage, QueryConstraintSetV1, RepoId, RevisionId, SearchExplanation,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::{ClientIoPolicy, send_request};
use quanta_index_sdk::QuantaIndex;
use quanta_index_searchd_harness::{
    legacy_reference::{LegacySearchdBinary, daemon_socket_paths, publish_legacy_source},
    private_tempdir,
};

use crate::fail_closed_wait::{RealTicker, wait_for};

type TestResult = Result<(), Box<dyn Error>>;

const SCORE_TOLERANCE: f32 = 1e-5;
const PLAIN_QUERY: &str = "needle";
const BOOSTED_QUERY: &str = "needle boost:2.5";

fn pin() -> Result<GenerationPin, Box<dyn Error>> {
    Ok(GenerationPin::new(
        RepoId::new("repo-e2e")?,
        RevisionId::new("rev-e2e")?,
        ManifestGeneration::new(1),
    ))
}

fn query_request(syntax: TextQuerySyntax, query: &str, top_k: u32) -> TextQueryRequest {
    TextQueryRequest {
        syntax,
        query_text: query.to_owned(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k,
        cursor: None,
    }
}

fn page(client: &QuantaIndex, query: &str) -> Result<Vec<LexicalCandidate>, Box<dyn Error>> {
    Ok(client
        .lexical()
        .query()
        .sourcegraph(query)
        .pinned(pin()?)
        .top_k(10)
        .execute()?
        .results)
}

fn explained(
    client: &QuantaIndex,
    candidate: LexicalCandidate,
    query: &str,
) -> Result<(CandidatePresenceV1, SearchExplanation), Box<dyn Error>> {
    let explain = client.search().explain_under_query(
        pin()?,
        candidate,
        query_request(TextQuerySyntax::Sourcegraph, query, 1),
    )?;
    Ok((explain.presence, explain.explanation))
}

fn trace_says(explanation: &SearchExplanation, detail: &str) -> bool {
    explanation
        .planner_trace
        .iter()
        .any(|entry| entry.stage == PlannerStage::Merge && entry.detail == detail)
}

fn contribution_sum(explanation: &SearchExplanation) -> f32 {
    explanation
        .contributions
        .iter()
        .map(|row| row.contribution)
        .sum()
}

fn verify_code_search_file_scores(
    client: &QuantaIndex,
    query_socket: &std::path::Path,
) -> TestResult {
    let pin = pin()?;
    let request_id = 0xe022;
    let native_response: SearchPlaneQueryIpcResponseEnvelope = send_request(
        query_socket,
        &SearchPlaneQueryIpcRequestEnvelope {
            request_id,
            diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
            payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
                generation: Some(pin.clone()),
                ..query_request(TextQuerySyntax::CodeSearch, PLAIN_QUERY, 10)
            }),
        },
        ClientIoPolicy::default(),
    )?;
    if native_response.request_id != request_id {
        return Err("CodeSearch raw IPC response ID differs".into());
    }
    let SearchPlaneQueryIpcResponse::Text(native_page) = native_response.payload else {
        return Err(format!("CodeSearch raw IPC refused: {:?}", native_response.payload).into());
    };
    let result = client
        .lexical()
        .query()
        .code_search(PLAIN_QUERY)
        .pinned(pin.clone())
        .top_k(10)
        .execute()?;
    if native_page.generation != pin
        || result.generation != pin
        || result.results != native_page.results
    {
        return Err("SDK must preserve the pinned native file page".into());
    }
    let request_trace = &result.explanation;
    for expected in [
        "code_search.execution.scope=ordinary_exhaustive_page_v1;exploration_complete=true",
        "code_search.execution.verified_matching_files=3",
        "code_search.execution.cursor_eligible_files=3",
        "code_search.execution.fetched_files=3",
        "code_search.execution.returned_files=3",
    ] {
        if !trace_says(request_trace, expected) {
            return Err(format!("CodeSearch public count missing: {expected}").into());
        }
    }
    let actual: Vec<_> = result
        .results
        .iter()
        .map(|row| (row.repo_relative_path.as_str(), row.score))
        .collect();
    // Fixed goldens from this source fixture, independent of explanation output.
    if actual
        != [
            ("src/dense.rs", 109.0),
            ("src/twice.rs", 107.0),
            ("src/sparse.rs", 105.0),
        ]
    {
        return Err(format!("source-file score goldens disagree: {actual:?}").into());
    }
    let bm25_top = page(client, PLAIN_QUERY)?
        .into_iter()
        .next()
        .ok_or("BM25 fixture hit")?;
    let (_, bm25) = explained(client, bm25_top, PLAIN_QUERY)?;
    for candidate in result.results {
        let carried = candidate.score;
        let presence = client.search().explain(pin.clone(), candidate.clone())?;
        if presence.presence != CandidatePresenceV1::Indexed {
            return Err("file candidate exact presence must be Indexed".into());
        }
        let explained = client.search().explain_under_query(
            pin.clone(),
            candidate,
            TextQueryRequest {
                syntax: TextQuerySyntax::CodeSearch,
                query_text: PLAIN_QUERY.into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: 1,
                cursor: None,
            },
        )?;
        let trace = explained.explanation;
        if explained.presence != CandidatePresenceV1::Indexed
            || trace.contributions.len() != 1
            || !matches!(
                trace.contributions.first(),
                Some(row) if row.signal_name.as_ref() == "lexical.code_search_file"
            )
            || contribution_sum(&trace).to_bits() != carried.to_bits()
            || !trace_says(&trace, "explain.code_search_score.boundary_and_path=100")
            || !trace_says(&trace, "explain.code_search_score.exact_case=5")
            || !trace_says(&trace, "explain.code_search_score.proximity=0")
            || !trace_says(&trace, "explain.score_reconciled=true")
            || !trace_says(
                &trace,
                &format!("explain.code_search_rank_study_v1.baseline={carried:.0};selected=true"),
            )
            || trace.ranker_weights_hash == bm25.ranker_weights_hash
        {
            return Err(
                format!("native file score must reach the public explanation: {trace:?}").into(),
            );
        }
    }
    Ok(())
}

fn verify_page_candidate_scores(client: &QuantaIndex) -> TestResult {
    let candidates = page(client, PLAIN_QUERY)?;
    if candidates.len() != 3 {
        return Err(format!("three documents carry the needle: {candidates:?}").into());
    }
    // The rank/trace equality below is self-referential. This fixed input
    // ordering is an independent oracle for the one-term BM25 page.
    let score_for = |path: &str| {
        candidates
            .iter()
            .find(|candidate| candidate.repo_relative_path.as_str() == path)
            .map(|candidate| candidate.score)
    };
    let (Some(dense), Some(twice), Some(sparse)) = (
        score_for("src/dense.rs"),
        score_for("src/twice.rs"),
        score_for("src/sparse.rs"),
    ) else {
        return Err(format!("fixture paths missing from page: {candidates:?}").into());
    };
    if !(dense.is_finite()
        && twice.is_finite()
        && sparse.is_finite()
        && dense > twice
        && twice > sparse
        && sparse > 0.0)
    {
        return Err(format!(
            "BM25 fixture order must be dense > twice > sparse > 0: {dense}, {twice}, {sparse}"
        )
        .into());
    }
    let mut hashes = Vec::new();
    let mut previous_score: Option<f32> = None;
    for candidate in candidates {
        let carried = candidate.score;
        let (presence, explanation) = explained(client, candidate.clone(), PLAIN_QUERY)?;
        if presence != CandidatePresenceV1::Indexed {
            return Err(format!("{} is indexed", candidate.candidate_id).into());
        }
        if explanation.strategy != "lexical_score_trace"
            || explanation.contributions.len() != 1
            || !trace_says(&explanation, "explain.candidate_matched=true")
            || !trace_says(&explanation, "explain.score_reconciled=true")
        {
            return Err(format!(
                "{} must explain as one reconciled lexical row: {explanation:?}",
                candidate.candidate_id
            )
            .into());
        }
        let Some(row) = explanation.contributions.first() else {
            return Err("one row".into());
        };
        if row.signal_name.as_ref() != "lexical.bm25"
            || row
                .signal_value
                .mul_add(row.weight, -row.contribution)
                .abs()
                > SCORE_TOLERANCE
            || (row.weight - 1.0).abs() > SCORE_TOLERANCE
            || (contribution_sum(&explanation) - carried).abs() > SCORE_TOLERANCE
        {
            return Err(format!(
                "{} carried {carried} but its row is {row:?}",
                candidate.candidate_id
            )
            .into());
        }
        if let Some(previous) = previous_score
            && carried > previous + SCORE_TOLERANCE
        {
            return Err("the page is ordered by the explained score".into());
        }
        previous_score = Some(carried);
        hashes.push(explanation.ranker_weights_hash);
    }
    let distinct: std::collections::BTreeSet<[u8; 32]> = hashes.iter().copied().collect();
    if hashes.contains(&[0u8; 32]) || distinct.len() != 1 {
        return Err(format!("one plan, one non-zero weights hash: {hashes:?}").into());
    }
    Ok(())
}

fn verify_boost(client: &QuantaIndex) -> TestResult {
    let plain = page(client, PLAIN_QUERY)?;
    let boosted = page(client, BOOSTED_QUERY)?;
    let Some(plain_top) = plain.first().cloned() else {
        return Err("a plain hit".into());
    };
    let Some(boosted_top) = boosted
        .iter()
        .find(|candidate| candidate.candidate_id == plain_top.candidate_id)
        .cloned()
    else {
        return Err("the same candidate under the boosted query".into());
    };
    if plain_top.score.mul_add(-2.5, boosted_top.score).abs() > 1e-4 {
        return Err(format!(
            "the page carries the boosted score: {} vs {} x 2.5",
            boosted_top.score, plain_top.score
        )
        .into());
    }
    let (_, plain_explanation) = explained(client, plain_top.clone(), PLAIN_QUERY)?;
    let (_, boosted_explanation) = explained(client, boosted_top.clone(), BOOSTED_QUERY)?;
    let (Some(plain_row), Some(boosted_row)) = (
        plain_explanation.contributions.first(),
        boosted_explanation.contributions.first(),
    ) else {
        return Err("one row each".into());
    };
    if (boosted_row.weight - 2.5).abs() > SCORE_TOLERANCE
        || (boosted_row.signal_value - plain_row.signal_value).abs() > SCORE_TOLERANCE
        || (boosted_row.contribution - boosted_top.score).abs() > SCORE_TOLERANCE
        || !trace_says(&boosted_explanation, "explain.score_reconciled=true")
    {
        return Err(format!(
            "the boost is the weight over the same engine score: {plain_row:?} vs {boosted_row:?}"
        )
        .into());
    }
    if plain_explanation.ranker_weights_hash == boosted_explanation.ranker_weights_hash {
        return Err("a different boost is a different weights vector".into());
    }
    // A candidate carried from the boosted page, explained under the plain
    // query, is not reconciled: the trace is honest about which plan it
    // scored under.
    let (_, cross) = explained(client, boosted_top, PLAIN_QUERY)?;
    if !trace_says(&cross, "explain.score_reconciled=false")
        || (contribution_sum(&cross) - plain_top.score).abs() > SCORE_TOLERANCE
    {
        return Err(format!("a cross-plan explain must say so: {cross:?}").into());
    }
    Ok(())
}

fn verify_presence(client: &QuantaIndex) -> TestResult {
    let candidates = page(client, PLAIN_QUERY)?;
    let Some(hit) = candidates.first().cloned() else {
        return Err("a hit".into());
    };
    // Presence only: no query named, no score traced.
    let presence_only = client.search().explain(pin()?, hit.clone())?;
    if presence_only.presence != CandidatePresenceV1::Indexed
        || presence_only.explanation.strategy != "presence_lookup"
        || !presence_only.explanation.contributions.is_empty()
    {
        return Err(format!("presence-only explain: {:?}", presence_only.explanation).into());
    }
    // A never-ingested id is not indexed, under either mode.
    let mut ghost = hit;
    ghost.candidate_id = "never-ingested".to_string();
    let absent = client.search().explain(pin()?, ghost.clone())?;
    if absent.presence != CandidatePresenceV1::NotIndexed {
        return Err(format!("a ghost is not indexed: {:?}", absent.explanation).into());
    }
    let (presence, explanation) = explained(client, ghost, PLAIN_QUERY)?;
    if presence != CandidatePresenceV1::NotIndexed
        || !explanation.contributions.is_empty()
        || !trace_says(&explanation, "explain.candidate_indexed=false")
    {
        return Err(format!("a ghost under a query: {explanation:?}").into());
    }
    // The document without the needle is indexed but unmatched — never
    // reported as absent.
    let other_page = page(client, "nothing")?;
    let Some(other) = other_page.first().cloned() else {
        return Err("the other document is a hit for its own text".into());
    };
    let (presence, explanation) = explained(client, other, PLAIN_QUERY)?;
    if presence != CandidatePresenceV1::Indexed
        || !trace_says(&explanation, "explain.candidate_matched=false")
        || !explanation.contributions.is_empty()
    {
        return Err(format!("an indexed non-match: {explanation:?}").into());
    }
    Ok(())
}

/// QI-BB-022: a hybrid row explains under both its queries, re-derived
/// lane by lane against the index.
///
/// The oracles are independent of the explain: the lexical page under the
/// same query (the lexical lane's raw score), the semantic page under the
/// same dense text (the dense lane's cosine and rank), and an RRF sum
/// recomputed here under k = 60 over the ranks the two pages give. A row
/// carried as the hybrid route emitted it reconciles on every axis; a row
/// forged on one axis — even one whose fused score is exactly the RRF of
/// the ranks it forged — fails on that axis and no other.
fn verify_hybrid_both_lane(client: &QuantaIndex) -> TestResult {
    const HYBRID_TOP_K: u32 = 10;
    // The same text drives both lanes, so the needle documents are seen by
    // both: the lexical lane by term, the dense lane by the hashed vector.
    let hybrid = client
        .search()
        .hybrid()
        .sourcegraph(PLAIN_QUERY)
        .semantic_text(PLAIN_QUERY)
        .pinned(pin()?)
        .top_k(HYBRID_TOP_K)
        .execute()?;
    let Some(row) = hybrid
        .results
        .iter()
        .find(|row| {
            row.contribution(HybridLaneV1::Lexical).is_some()
                && row.contribution(HybridLaneV1::Dense).is_some()
        })
        .cloned()
    else {
        return Err(format!("a row both lanes saw: {:?}", hybrid.results).into());
    };
    let Some(lexical) = row.contribution(HybridLaneV1::Lexical).cloned() else {
        return Err("the lexical contribution".into());
    };
    let Some(dense) = row.contribution(HybridLaneV1::Dense).cloned() else {
        return Err("the dense contribution".into());
    };
    let expected_fused: f64 = row
        .contributions
        .iter()
        .map(|contribution| 1.0 / (60.0 + f64::from(contribution.rank)))
        .sum();
    if row.fused_score.to_bits() != expected_fused.to_bits()
        || row.candidate.score.to_bits() != lexical.raw_score.to_bits()
    {
        return Err(format!("the row carries its own provenance: {row:?}").into());
    }
    // Independent oracles: the lexical page scores the row exactly as the
    // lexical lane did, and the semantic page ranks and scores it exactly
    // as the dense lane did.
    let page = page(client, PLAIN_QUERY)?;
    let Some(page_row) = page
        .iter()
        .find(|candidate| candidate.candidate_id == row.candidate.candidate_id)
    else {
        return Err("the lexical page carries the row".into());
    };
    if (page_row.score - lexical.raw_score).abs() > SCORE_TOLERANCE {
        return Err(format!(
            "the lexical raw score {} is the page score {}",
            lexical.raw_score, page_row.score
        )
        .into());
    }
    let semantic = client
        .semantic()
        .query()
        .text(PLAIN_QUERY)
        .pinned(pin()?)
        .top_k(10)
        .execute()?;
    let Some(semantic_position) = semantic
        .results
        .iter()
        .position(|candidate| candidate.candidate_id == row.candidate.candidate_id)
    else {
        return Err("the semantic page carries the row".into());
    };
    let Some(semantic_row) = semantic.results.get(semantic_position) else {
        return Err("the semantic row".into());
    };
    if u32::try_from(semantic_position.saturating_add(1))? != dense.rank
        || (semantic_row.score - dense.raw_score).abs() > 1e-4
    {
        return Err(format!(
            "the dense contribution {dense:?} is the semantic page's rank {} and score {}",
            semantic_position.saturating_add(1),
            semantic_row.score
        )
        .into());
    }

    let explain = client.search().explain_hybrid_under_queries(
        pin()?,
        row.clone(),
        query_request(TextQuerySyntax::Sourcegraph, PLAIN_QUERY, HYBRID_TOP_K),
        PLAIN_QUERY,
    )?;
    let presence = explain.presence;
    let explanation = explain.explanation;
    let rederived_ranks = format!(
        "explain.rrf_k=60; carried_ranks=lexical#{},dense#{}; rederived_ranks=lexical#{},dense#{}",
        lexical.rank, dense.rank, lexical.rank, dense.rank
    );
    if presence != CandidatePresenceV1::Indexed
        || explanation.strategy != "hybrid_score_trace"
        || !trace_says(&explanation, "explain.candidate_matched=true")
        || !trace_says(&explanation, "explain.score_reconciled=true")
        || !trace_says(&explanation, "explain.dense_reconciled=true")
        || !trace_says(&explanation, "explain.fused_reconciled=true")
        || !trace_says(&explanation, &rederived_ranks)
    {
        return Err(format!("hybrid explain must reconcile every axis: {explanation:?}").into());
    }
    let names = explanation
        .contributions
        .iter()
        .map(|entry| entry.signal_name.as_ref())
        .collect::<Vec<_>>();
    if names
        != [
            "lexical.bm25",
            "dense.cosine",
            "hybrid.rrf.lexical",
            "hybrid.rrf.dense",
        ]
    {
        return Err(format!("one score row and one RRF row per lane: {names:?}").into());
    }
    let [lexical_row, dense_row, rrf_lexical, rrf_dense] = explanation.contributions.as_slice()
    else {
        return Err("four rows".into());
    };
    // The composition rule: the lane score rows carry the lane scores in
    // their own units; the RRF rows sum to the fused score.
    let rrf_sum = f64::from(rrf_lexical.contribution) + f64::from(rrf_dense.contribution);
    if (lexical_row.contribution - lexical.raw_score).abs() > SCORE_TOLERANCE
        || (dense_row.contribution - dense.raw_score).abs() > 1e-4
        || (f64::from(rrf_lexical.contribution) - 1.0 / (60.0 + f64::from(lexical.rank))).abs()
            > 1e-8
        || (f64::from(rrf_dense.contribution) - 1.0 / (60.0 + f64::from(dense.rank))).abs() > 1e-8
        || (rrf_sum - row.fused_score).abs() > 1e-8
    {
        return Err(format!(
            "the rows carry the lane scores and the RRF terms: {:?}",
            explanation.contributions
        )
        .into());
    }

    // Forged on one axis at a time: each forgery fails its own axis and
    // no other, because every axis is compared with the index.
    let mut forged_lexical = row.clone();
    forged_lexical.candidate.score += 1.0;
    for contribution in &mut forged_lexical.contributions {
        if contribution.lane == HybridLaneV1::Lexical {
            contribution.raw_score += 1.0;
        }
    }
    let mut forged_dense = row.clone();
    for contribution in &mut forged_dense.contributions {
        if contribution.lane == HybridLaneV1::Dense {
            contribution.raw_score = (contribution.raw_score - 0.25).max(-1.0);
        }
    }
    // The fusion forgery is self-consistent: the ranks are moved and the
    // fused score is the RRF of the moved ranks, so a check against the
    // payload alone would pass it.
    let mut forged_fusion = row.clone();
    for contribution in &mut forged_fusion.contributions {
        if contribution.lane == HybridLaneV1::Dense {
            contribution.rank = contribution.rank.saturating_add(5);
        }
    }
    forged_fusion.fused_score = forged_fusion
        .contributions
        .iter()
        .map(|contribution| 1.0 / (60.0 + f64::from(contribution.rank)))
        .sum();
    for (forged, expected_axes, what) in [
        (forged_lexical, (false, true, true), "lexical"),
        (forged_dense, (true, false, true), "dense"),
        (forged_fusion, (true, true, false), "fusion"),
    ] {
        let explain = client.search().explain_hybrid_under_queries(
            pin()?,
            forged,
            query_request(TextQuerySyntax::Sourcegraph, PLAIN_QUERY, HYBRID_TOP_K),
            PLAIN_QUERY,
        )?;
        let explanation = explain.explanation;
        let axes = (
            trace_says(&explanation, "explain.score_reconciled=true"),
            trace_says(&explanation, "explain.dense_reconciled=true"),
            trace_says(&explanation, "explain.fused_reconciled=true"),
        );
        if axes != expected_axes {
            return Err(format!(
                "a {what} forgery must fail its own axis only: got {axes:?}, expected {expected_axes:?}: {explanation:?}"
            )
            .into());
        }
    }
    Ok(())
}

/// QI-BB-022: an indexed candidate the plan does not match names the
/// plan's filter leaves, so a caller sees which filters stood between the
/// document and the page rather than a bare "does not match".
fn verify_unmatched_candidate_filters(client: &QuantaIndex) -> TestResult {
    let candidates = page(client, PLAIN_QUERY)?;
    let Some(sparse) = candidates
        .iter()
        .find(|candidate| candidate.repo_relative_path.as_str().ends_with("sparse.rs"))
        .cloned()
    else {
        return Err("the sparse document is a plain hit".into());
    };
    // Under a file filter that names another document, the sparse hit is
    // indexed and unmatched, and the filter is listed.
    let filtered_query = "file:dense needle";
    let (presence, explanation) = explained(client, sparse, filtered_query)?;
    let filter_entries = explanation
        .planner_trace
        .iter()
        .filter(|entry| entry.stage == PlannerStage::Plan)
        .map(|entry| entry.detail.as_str())
        .filter(|detail| detail.starts_with("explain.plan_filter"))
        .collect::<Vec<_>>();
    if presence != CandidatePresenceV1::Indexed
        || !trace_says(&explanation, "explain.candidate_matched=false")
        || !explanation.contributions.is_empty()
        || filter_entries.first().copied() != Some("explain.plan_filters=1")
        || !filter_entries
            .get(1)
            .is_some_and(|detail| detail.starts_with("explain.plan_filter[0]=File"))
        || !explanation.summary.contains("plan filters: File")
    {
        return Err(format!(
            "an unmatched candidate names its plan filters: {filter_entries:?} / {explanation:?}"
        )
        .into());
    }
    // The same document under the plain query matches, and no filter is
    // listed for a match.
    let candidates = page(client, PLAIN_QUERY)?;
    let Some(sparse) = candidates
        .iter()
        .find(|candidate| candidate.repo_relative_path.as_str().ends_with("sparse.rs"))
        .cloned()
    else {
        return Err("the sparse document is a plain hit".into());
    };
    let (_, explanation) = explained(client, sparse, PLAIN_QUERY)?;
    if !trace_says(&explanation, "explain.candidate_matched=true")
        || explanation
            .planner_trace
            .iter()
            .any(|entry| entry.detail.starts_with("explain.plan_filter"))
    {
        return Err(format!("a match lists no filters: {explanation:?}").into());
    }
    Ok(())
}

#[test]
fn explain_score_traces_share_one_indexed_fixture() -> TestResult {
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let process = binary.start(&state_root, 8)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        let pin = pin()?;
        let _published = publish_legacy_source(
            &client,
            &pin.repo_id,
            &pin.revision_id,
            1,
            None,
            &[
                ("src/dense.rs", "needle needle needle haystack", 1),
                (
                    "src/sparse.rs",
                    "needle in a very long haystack of many other words",
                    2,
                ),
                ("src/twice.rs", "needle haystack needle", 3),
                ("src/other.rs", "nothing relevant here", 4),
            ],
            None,
        )?;
        let _ready = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "pinned old-Q Explain backend ready",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| true,
        )?;
        let [query_socket, _, _] = daemon_socket_paths(&state_root);
        verify_page_candidate_scores(&client).map_err(|error| -> Box<dyn Error> {
            format!("page_candidate_scores: {error}").into()
        })?;
        verify_code_search_file_scores(&client, &query_socket).map_err(
            |error| -> Box<dyn Error> { format!("code_search_file_scores: {error}").into() },
        )?;
        let verify_boost_fn: fn(&QuantaIndex) -> TestResult = verify_boost;
        for (name, verify) in [
            ("boost", verify_boost_fn),
            ("presence", verify_presence),
            ("hybrid_both_lane", verify_hybrid_both_lane),
            (
                "unmatched_candidate_filters",
                verify_unmatched_candidate_filters,
            ),
        ] {
            verify(&client)
                .map_err(|error| -> Box<dyn Error> { format!("{name}: {error}").into() })?;
        }
        Ok(())
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}
