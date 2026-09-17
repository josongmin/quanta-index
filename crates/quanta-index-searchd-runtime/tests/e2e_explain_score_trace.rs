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

use quanta_index_contract::{
    CandidatePresenceV1, HybridLaneV1, LexicalCandidate, PlannerStage, SearchExplanation,
    TextQuerySyntax,
};
use quanta_index_searchd_harness as e2e_harness;

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

/// QI-BB-022: a hybrid row explains under its query lane by lane.
///
/// The lexical lane traced under the plan is the carried lexical raw score
/// (`score_reconciled`), the dense lane is reported as carried, and the RRF
/// of the carried ranks — recomputed here under k = 60 as the independent
/// oracle — is the carried `fused_score` (`fused_reconciled`).
#[test]
fn a_hybrid_both_lane_candidate_explains_to_reconciled_lane_provenance() -> TestResult {
    let mut rt = E2eRuntime::boot()?;
    ingest_fixture(&mut rt)?;
    // The same text drives both lanes, so the needle documents are seen by
    // both: the lexical lane by term, the dense lane by the hashed vector.
    let hybrid = rt.query_hybrid(TextQuerySyntax::Sourcegraph, PLAIN_QUERY, PLAIN_QUERY, 10);
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
    // The lexical page under the same query scores the row exactly as the
    // lexical lane did — the raw score is a real lane score.
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
    let explain =
        rt.explain_candidate_under_query(row.clone(), TextQuerySyntax::Sourcegraph, PLAIN_QUERY);
    if let Some(error) = explain.typed_error {
        return Err(format!("hybrid explain refused: {error}").into());
    }
    let (Some(presence), Some(explanation)) = (explain.presence, explain.explanation) else {
        return Err("a served explain carries presence and an explanation".into());
    };
    if presence != CandidatePresenceV1::Indexed
        || explanation.strategy != "hybrid_score_trace"
        || !trace_says(&explanation, "explain.candidate_matched=true")
        || !trace_says(&explanation, "explain.score_reconciled=true")
        || !trace_says(&explanation, "explain.fused_reconciled=true")
        || !trace_says(
            &explanation,
            &format!(
                "explain.rrf_k=60; ranks=lexical#{},dense#{}",
                lexical.rank, dense.rank
            ),
        )
    {
        return Err(format!("hybrid explain must reconcile both axes: {explanation:?}").into());
    }
    let names = explanation
        .contributions
        .iter()
        .map(|entry| entry.signal_name.as_ref())
        .collect::<Vec<_>>();
    if names != ["lexical.bm25", "dense.cosine", "hybrid.rrf"] {
        return Err(format!("one row per lane plus the fused row: {names:?}").into());
    }
    let [lexical_row, dense_row, fused_row] = explanation.contributions.as_slice() else {
        return Err("three rows".into());
    };
    if (lexical_row.contribution - lexical.raw_score).abs() > SCORE_TOLERANCE
        || dense_row.contribution.to_bits() != dense.raw_score.to_bits()
        || (f64::from(fused_row.contribution) - expected_fused).abs() > 1e-8
    {
        return Err(format!(
            "the rows carry the lane scores: {:?}",
            explanation.contributions
        )
        .into());
    }
    // The explain is honest about a row whose provenance was not this
    // plan's: the same row carried with a stale lexical raw score is not
    // reconciled on the lexical axis, while its RRF arithmetic still is.
    let mut stale = row;
    stale.candidate.score += 1.0;
    for contribution in &mut stale.contributions {
        if contribution.lane == HybridLaneV1::Lexical {
            contribution.raw_score += 1.0;
        }
    }
    let explain =
        rt.explain_candidate_under_query(stale, TextQuerySyntax::Sourcegraph, PLAIN_QUERY);
    let Some(explanation) = explain.explanation else {
        return Err(format!("stale explain: {:?}", explain.typed_error).into());
    };
    if !trace_says(&explanation, "explain.score_reconciled=false")
        || !trace_says(&explanation, "explain.fused_reconciled=true")
    {
        return Err(format!("a stale lexical lane must say so: {explanation:?}").into());
    }
    Ok(())
}
