//! QI-BB-022 — explaining one candidate is an exact lookup plus the score
//! the engine emits for it under the plan, never a ranked re-search.
//!
//! The oracles are the ranked search itself (every candidate it returns
//! must explain to exactly its emitted score, in the same order), an exact
//! id lookup that does not depend on how many other documents match, and
//! the plan's own boost, which must show up as the weight and move the
//! emitted score with it.

#![forbid(unsafe_code)]

use std::error::Error;

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    BatchIngestMode, CandidatePresenceV1, ChunkId, ChunkRecord, LQ_VERSION_TAG, LqExpr, LqLeaf,
    LqOptions, LqQuery, LqSpan, LqYesNoOnly, ManifestGeneration, QueryConstraintSetV1, RepoId,
    RepoRelativePath, RevisionId, SearchCorpusIngestBatch, UpsertChunk,
};
use quanta_index_core::{
    CoreError, LexicalCandidateExplanationV1, LexicalIndexOpenPort, LexicalPageSpec,
    LexicalScoreEngineV1, LexicalScoreTraceV1, RequestBudgetV1, SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;

type TestResult = Result<(), Box<dyn Error>>;

const SCORE_TOLERANCE: f32 = 1e-5;

fn repo() -> RepoId {
    RepoId::new("explain-repo")
}

fn revision() -> RevisionId {
    RevisionId::new("explain-rev")
}

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn upsert(chunk_id: &str, text: &str) -> Result<LexicalChannelOp, Box<dyn Error>> {
    let record = ChunkRecord {
        chunk_id: ChunkId::new(chunk_id),
        repo_relative_path: RepoRelativePath::new(format!("src/{chunk_id}.txt")),
        language: LanguageCode::new("text")
            .map_err(|err| -> Box<dyn Error> { format!("language: {err}").into() })?,
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 0,
        end_line: 0,
        text: text.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    };
    let mut payload = Vec::new();
    ciborium::into_writer(&record, &mut payload)?;
    Ok(LexicalChannelOp::UpsertChunk(UpsertChunk {
        repo_id: repo(),
        revision_id: revision(),
        generation: generation(),
        chunk_id: ChunkId::new(chunk_id),
        payload,
    }))
}

/// Build and seal one generation from `ops` (the mutation-only build port
/// followed by the digest-carrying seal).
fn seal(adapter: &LexicalAdapter, ops: &[LexicalChannelOp]) -> Result<(), CoreError> {
    quanta_index_core::LexicalIndexBuildPort::build(
        adapter,
        &repo(),
        &revision(),
        generation(),
        ops,
    )?;
    SearchCorpusBatchBuildPort::build_batch(
        adapter,
        &SearchCorpusIngestBatch {
            repo_id: repo(),
            revision_id: revision(),
            generation: generation(),
            base_generation: None,
            manifest_digest: "manifest:explain".to_string(),
            batch_digest: "batch:explain".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: Vec::new(),
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: true,
        },
    )
}

fn keyword_query(terms: &[&str]) -> LqQuery {
    let leaves: Vec<LqExpr> = terms
        .iter()
        .map(|term| LqExpr::Leaf(LqLeaf::Keyword((*term).to_string())))
        .collect();
    let expr = match leaves.len() {
        1 => leaves.into_iter().next().map_or(LqExpr::Empty, |leaf| leaf),
        _ => LqExpr::Any(leaves),
    };
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr,
        filters: Vec::new(),
        options: LqOptions::defaults(),
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn matched(
    explanation: LexicalCandidateExplanationV1,
) -> Result<LexicalScoreTraceV1, Box<dyn Error>> {
    match explanation {
        LexicalCandidateExplanationV1::Matched(trace) => Ok(trace),
        other @ (LexicalCandidateExplanationV1::NotIndexed
        | LexicalCandidateExplanationV1::NotMatched { .. }) => {
            Err(format!("expected a matched trace, got {other:?}").into())
        }
    }
}

#[test]
fn every_ranked_candidate_explains_to_exactly_its_emitted_score_in_rank_order() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    // Non-matching documents sit before the first match, between matches
    // and after the last one, so the one-document scorer is exercised at
    // every position relative to the plan's match set.
    seal(
        &adapter,
        &[
            upsert("first-other", "nothing relevant at all")?,
            upsert("dense", "needle needle needle haystack")?,
            upsert(
                "sparse",
                "needle in a very long haystack of many other words here",
            )?,
            upsert("other", "still nothing relevant")?,
            upsert("twice", "needle haystack needle")?,
            upsert("last-other", "nothing relevant at the end")?,
        ],
    )?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let constraints = QueryConstraintSetV1::unconstrained();
    let query = keyword_query(&["needle"]);
    let page = searcher.search_constrained(
        &query,
        &constraints,
        &LexicalPageSpec::first(10),
        &RequestBudgetV1::unbounded(),
    )?;
    if page.candidates.len() != 3 {
        return Err(format!("three documents contain the needle: {:?}", page.candidates).into());
    }
    let mut previous: Option<f32> = None;
    for candidate in &page.candidates {
        let trace = matched(searcher.explain_candidate(
            &query,
            &constraints,
            &candidate.candidate_id,
            &RequestBudgetV1::unbounded(),
        )?)?;
        if trace.engine != LexicalScoreEngineV1::Bm25
            || (trace.emitted_score - candidate.score).abs() > SCORE_TOLERANCE
            || trace
                .engine_score
                .mul_add(trace.boost_factor, -trace.emitted_score)
                .abs()
                > SCORE_TOLERANCE
            || (trace.boost_factor - 1.0).abs() > SCORE_TOLERANCE
        {
            return Err(format!(
                "{} scored {} in the page but explains to {trace:?}",
                candidate.candidate_id, candidate.score
            )
            .into());
        }
        if let Some(previous) = previous
            && trace.emitted_score > previous + SCORE_TOLERANCE
        {
            return Err(format!(
                "explained scores must follow the page order: {} after {previous}",
                trace.emitted_score
            )
            .into());
        }
        previous = Some(trace.emitted_score);
    }
    // The documents without the needle are indexed but unmatched, never
    // "absent", wherever they sit relative to the matches — and so is every
    // document under a plan that matches nothing in the segment at all.
    let nothing = keyword_query(&["zebra"]);
    for (plan, unmatched) in [
        (&query, "first-other"),
        (&query, "other"),
        (&query, "last-other"),
        (&nothing, "first-other"),
        (&nothing, "dense"),
    ] {
        match searcher.explain_candidate(
            plan,
            &constraints,
            unmatched,
            &RequestBudgetV1::unbounded(),
        )? {
            LexicalCandidateExplanationV1::NotMatched { .. } => {}
            other @ (LexicalCandidateExplanationV1::NotIndexed
            | LexicalCandidateExplanationV1::Matched(_)) => {
                return Err(format!("{unmatched} is an indexed non-match, got {other:?}").into());
            }
        }
    }
    Ok(())
}

#[test]
fn a_boost_in_the_plan_is_the_weight_and_scales_the_emitted_score() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    seal(
        &adapter,
        &[
            upsert("a", "needle haystack")?,
            upsert("b", "needle needle")?,
        ],
    )?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let constraints = QueryConstraintSetV1::unconstrained();
    let plain = keyword_query(&["needle"]);
    let mut boosted = plain.clone();
    boosted.options.boost_millis = Some(2_500);
    let plain_trace = matched(searcher.explain_candidate(
        &plain,
        &constraints,
        "b",
        &RequestBudgetV1::unbounded(),
    )?)?;
    let boosted_trace = matched(searcher.explain_candidate(
        &boosted,
        &constraints,
        "b",
        &RequestBudgetV1::unbounded(),
    )?)?;
    if (boosted_trace.boost_factor - 2.5).abs() > SCORE_TOLERANCE
        || (boosted_trace.engine_score - plain_trace.engine_score).abs() > SCORE_TOLERANCE
        || plain_trace
            .emitted_score
            .mul_add(-2.5, boosted_trace.emitted_score)
            .abs()
            > 1e-4
    {
        return Err(format!("boost must be the weight over an unchanged engine score: {plain_trace:?} vs {boosted_trace:?}").into());
    }
    // And the page agrees: the emitted score under the boosted plan is what
    // the boosted search returns.
    let page = searcher.search_constrained(
        &boosted,
        &constraints,
        &LexicalPageSpec::first(10),
        &RequestBudgetV1::unbounded(),
    )?;
    let Some(top) = page
        .candidates
        .iter()
        .find(|candidate| candidate.candidate_id == "b")
    else {
        return Err("b is a hit".into());
    };
    if (top.score - boosted_trace.emitted_score).abs() > SCORE_TOLERANCE {
        return Err(format!(
            "page score {} != explained {}",
            top.score, boosted_trace.emitted_score
        )
        .into());
    }
    Ok(())
}

#[test]
fn presence_is_an_exact_lookup_independent_of_the_corpus_around_the_candidate() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    // 300 short documents outrank one long one on the same term: a top-50
    // re-search would never see the long one.
    let mut ops = Vec::new();
    for index in 0..300_u32 {
        ops.push(upsert(&format!("short-{index}"), "needle")?);
    }
    let filler = "word ".repeat(400);
    ops.push(upsert("buried", &format!("needle {filler}"))?);
    seal(&adapter, &ops)?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let constraints = QueryConstraintSetV1::unconstrained();
    let query = keyword_query(&["needle"]);
    let page = searcher.search_constrained(
        &query,
        &constraints,
        &LexicalPageSpec::first(50),
        &RequestBudgetV1::unbounded(),
    )?;
    if page
        .candidates
        .iter()
        .any(|candidate| candidate.candidate_id == "buried")
    {
        return Err("the fixture must bury the candidate below the page".into());
    }
    let trace = matched(searcher.explain_candidate(
        &query,
        &constraints,
        "buried",
        &RequestBudgetV1::unbounded(),
    )?)?;
    if trace.emitted_score <= 0.0 {
        return Err(format!("the buried candidate is matched with a real score: {trace:?}").into());
    }
    if searcher.candidate_presence("buried")? != CandidatePresenceV1::Indexed {
        return Err("the buried candidate is indexed".into());
    }
    if searcher.candidate_presence("never-ingested")? != CandidatePresenceV1::NotIndexed {
        return Err("an unknown id is not indexed".into());
    }
    match searcher.explain_candidate(
        &query,
        &constraints,
        "never-ingested",
        &RequestBudgetV1::unbounded(),
    )? {
        LexicalCandidateExplanationV1::NotIndexed => Ok(()),
        other @ (LexicalCandidateExplanationV1::NotMatched { .. }
        | LexicalCandidateExplanationV1::Matched(_)) => {
            Err(format!("an unknown id explains as NotIndexed, got {other:?}").into())
        }
    }
}

#[test]
fn an_unindexed_scan_explains_through_the_same_per_document_matcher() -> TestResult {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    seal(
        &adapter,
        &[
            upsert("hit", "needle haystack")?,
            upsert("miss", "haystack only")?,
        ],
    )?;
    let searcher = adapter.open(&repo(), &revision(), generation())?;
    let constraints = QueryConstraintSetV1::unconstrained();
    let mut query = keyword_query(&["needle"]);
    query.options.index_mode = Some(LqYesNoOnly::No);
    query.options.boost_millis = Some(3_000);
    let page = searcher.search_constrained(
        &query,
        &constraints,
        &LexicalPageSpec::first(10),
        &RequestBudgetV1::unbounded(),
    )?;
    let Some(hit) = page
        .candidates
        .iter()
        .find(|candidate| candidate.candidate_id == "hit")
    else {
        return Err(format!("the scan returns the hit: {:?}", page.candidates).into());
    };
    let trace = matched(searcher.explain_candidate(
        &query,
        &constraints,
        "hit",
        &RequestBudgetV1::unbounded(),
    )?)?;
    if trace.engine != LexicalScoreEngineV1::UnindexedScan
        || (trace.engine_score - 1.0).abs() > SCORE_TOLERANCE
        || (trace.emitted_score - hit.score).abs() > SCORE_TOLERANCE
        || (trace.emitted_score - 3.0).abs() > 1e-4
    {
        return Err(format!(
            "scan trace must be 1.0 x boost = page score: {trace:?} vs {}",
            hit.score
        )
        .into());
    }
    match searcher.explain_candidate(&query, &constraints, "miss", &RequestBudgetV1::unbounded())? {
        LexicalCandidateExplanationV1::NotMatched { .. } => Ok(()),
        other @ (LexicalCandidateExplanationV1::NotIndexed
        | LexicalCandidateExplanationV1::Matched(_)) => {
            Err(format!("a scan non-match is NotMatched, got {other:?}").into())
        }
    }
}
