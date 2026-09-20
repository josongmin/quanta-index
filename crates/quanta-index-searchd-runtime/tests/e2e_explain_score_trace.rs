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

use crate::e2e_harness;
use quanta_index_contract::{
    CandidatePresenceV1, HybridLaneV1, LexicalCandidate, PlannerStage, SearchExplanation,
    TextQuerySyntax,
};

use e2e_harness::E2eRuntime;

type TestResult = Result<(), Box<dyn Error>>;

const SCORE_TOLERANCE: f32 = 1e-5;
const PLAIN_QUERY: &str = "needle";
const BOOSTED_QUERY: &str = "needle boost:2.5";

fn ingest_fixture(rt: &mut E2eRuntime) -> TestResult {
    rt.ingest_text("repo", "src/dense.rs", "needle needle needle haystack")?;
    rt.ingest_text(
        "repo",
        "src/sparse.rs",
        "needle in a very long haystack of many other words",
    )?;
    rt.ingest_text("repo", "src/twice.rs", "needle haystack needle")?;
    rt.ingest_text("repo", "src/other.rs", "nothing relevant here")?;
    let _sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(())
}

fn page(rt: &mut E2eRuntime, query: &str) -> Result<Vec<LexicalCandidate>, Box<dyn Error>> {
    let result = rt.query_text(TextQuerySyntax::Sourcegraph, query, 10);
    if let Some(error) = result.typed_error {
        return Err(format!("query `{query}` refused: {error}").into());
    }
    Ok(result.candidates)
}

fn explained(
    rt: &mut E2eRuntime,
    candidate: LexicalCandidate,
    query: &str,
) -> Result<(CandidatePresenceV1, SearchExplanation), Box<dyn Error>> {
    let explain = rt.explain_candidate_under_query(candidate, TextQuerySyntax::Sourcegraph, query);
    if let Some(error) = explain.typed_error {
        return Err(format!("explain refused: {error}").into());
    }
    Ok((
        explain
            .presence
            .ok_or("a served explain carries presence")?,
        explain
            .explanation
            .ok_or("a served explain carries an explanation")?,
    ))
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

#[test]
fn every_page_candidate_explains_to_its_carried_score_in_page_order() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    ingest_fixture(&mut rt)?;
    let candidates = page(&mut rt, PLAIN_QUERY)?;
    if candidates.len() != 3 {
        return Err(format!("three documents carry the needle: {candidates:?}").into());
    }
    let mut hashes = Vec::new();
    let mut previous_score: Option<f32> = None;
    for candidate in candidates {
        let carried = candidate.score;
        let (presence, explanation) = explained(&mut rt, candidate.clone(), PLAIN_QUERY)?;
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

#[test]
fn a_boost_is_the_weight_and_moves_the_page_the_trace_and_the_weights_hash_together() -> TestResult
{
    let mut rt = E2eRuntime::boot()?;
    ingest_fixture(&mut rt)?;
    let plain = page(&mut rt, PLAIN_QUERY)?;
    let boosted = page(&mut rt, BOOSTED_QUERY)?;
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
    let (_, plain_explanation) = explained(&mut rt, plain_top.clone(), PLAIN_QUERY)?;
    let (_, boosted_explanation) = explained(&mut rt, boosted_top.clone(), BOOSTED_QUERY)?;
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
    let (_, cross) = explained(&mut rt, boosted_top, PLAIN_QUERY)?;
    if !trace_says(&cross, "explain.score_reconciled=false")
        || (contribution_sum(&cross) - plain_top.score).abs() > SCORE_TOLERANCE
    {
        return Err(format!("a cross-plan explain must say so: {cross:?}").into());
    }
    Ok(())
}

#[test]
fn presence_is_a_typed_exact_lookup_and_a_non_match_is_not_absence() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    ingest_fixture(&mut rt)?;
    let candidates = page(&mut rt, PLAIN_QUERY)?;
    let Some(hit) = candidates.first().cloned() else {
        return Err("a hit".into());
    };
    // Presence only: no query named, no score traced.
    let presence_only = rt.explain_candidate(hit.clone());
    if presence_only.presence != Some(CandidatePresenceV1::Indexed)
        || presence_only
            .explanation
            .as_ref()
            .is_none_or(|explanation| {
                explanation.strategy != "presence_lookup" || !explanation.contributions.is_empty()
            })
    {
        return Err(format!("presence-only explain: {:?}", presence_only.explanation).into());
    }
    // A never-ingested id is not indexed, under either mode.
    let mut ghost = hit;
    ghost.candidate_id = "never-ingested".to_string();
    let absent = rt.explain_candidate(ghost.clone());
    if absent.presence != Some(CandidatePresenceV1::NotIndexed) {
        return Err(format!("a ghost is not indexed: {:?}", absent.explanation).into());
    }
    let (presence, explanation) = explained(&mut rt, ghost, PLAIN_QUERY)?;
    if presence != CandidatePresenceV1::NotIndexed
        || !explanation.contributions.is_empty()
        || !trace_says(&explanation, "explain.candidate_indexed=false")
    {
        return Err(format!("a ghost under a query: {explanation:?}").into());
    }
    // The document without the needle is indexed but unmatched — never
    // reported as absent.
    let other_page = page(&mut rt, "nothing")?;
    let Some(other) = other_page.first().cloned() else {
        return Err("the other document is a hit for its own text".into());
    };
    let (presence, explanation) = explained(&mut rt, other, PLAIN_QUERY)?;
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
#[test]
fn a_hybrid_both_lane_candidate_is_rederived_against_the_index_on_every_axis() -> TestResult {
    const HYBRID_TOP_K: u32 = 10;
    let mut rt = E2eRuntime::boot()?;
    ingest_fixture(&mut rt)?;
    // The same text drives both lanes, so the needle documents are seen by
    // both: the lexical lane by term, the dense lane by the hashed vector.
    let hybrid = rt.query_hybrid(
        TextQuerySyntax::Sourcegraph,
        PLAIN_QUERY,
        PLAIN_QUERY,
        HYBRID_TOP_K,
    );
    if let Some(error) = hybrid.typed_error {
        return Err(format!("hybrid query refused: {error}").into());
    }
    let Some(row) = hybrid
        .hybrid_candidates
        .iter()
        .find(|row| {
            row.contribution(HybridLaneV1::Lexical).is_some()
                && row.contribution(HybridLaneV1::Dense).is_some()
        })
        .cloned()
    else {
        return Err(format!("a row both lanes saw: {:?}", hybrid.hybrid_candidates).into());
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
    let page = page(&mut rt, PLAIN_QUERY)?;
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
    let semantic = rt.query_semantic(PLAIN_QUERY, 10, None);
    if let Some(error) = semantic.typed_error {
        return Err(format!("semantic query refused: {error}").into());
    }
    let Some(semantic_position) = semantic
        .candidates
        .iter()
        .position(|candidate| candidate.candidate_id == row.candidate.candidate_id)
    else {
        return Err("the semantic page carries the row".into());
    };
    let Some(semantic_row) = semantic.candidates.get(semantic_position) else {
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

    let explain = rt.explain_hybrid_candidate_under_queries(
        row.clone(),
        TextQuerySyntax::Sourcegraph,
        PLAIN_QUERY,
        PLAIN_QUERY,
        HYBRID_TOP_K,
    );
    if let Some(error) = explain.typed_error {
        return Err(format!("hybrid explain refused: {error}").into());
    }
    let (Some(presence), Some(explanation)) = (explain.presence, explain.explanation) else {
        return Err("a served explain carries presence and an explanation".into());
    };
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
        let explain = rt.explain_hybrid_candidate_under_queries(
            forged,
            TextQuerySyntax::Sourcegraph,
            PLAIN_QUERY,
            PLAIN_QUERY,
            HYBRID_TOP_K,
        );
        let Some(explanation) = explain.explanation else {
            return Err(format!("{what} forgery explain: {:?}", explain.typed_error).into());
        };
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
#[test]
fn an_unmatched_candidate_names_the_plan_filters_that_excluded_it() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    ingest_fixture(&mut rt)?;
    let candidates = page(&mut rt, PLAIN_QUERY)?;
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
    let (presence, explanation) = explained(&mut rt, sparse, filtered_query)?;
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
    let candidates = page(&mut rt, PLAIN_QUERY)?;
    let Some(sparse) = candidates
        .iter()
        .find(|candidate| candidate.repo_relative_path.as_str().ends_with("sparse.rs"))
        .cloned()
    else {
        return Err("the sparse document is a plain hit".into());
    };
    let (_, explanation) = explained(&mut rt, sparse, PLAIN_QUERY)?;
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
