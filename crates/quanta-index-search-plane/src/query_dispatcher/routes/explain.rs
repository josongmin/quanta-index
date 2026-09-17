//! Explain route: exact presence lookup and per-candidate score explanation
//! under the plan that ranked it.

use quanta_index_contract::{
    CandidatePresenceV1, EngineTouched, ExplanationRow, LqOptions, LqYesNoOnly, PlannerStage,
    PlannerTraceEntry, SearchExplanation, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse, TextQueryRequest,
};
use quanta_index_core::{
    CoreError, ExplainQueryPort, LexicalCandidateExplanationV1, LexicalPolicy,
    LexicalScoreEngineV1, RequestBudgetV1, validate_query_top_k,
};

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;

impl SearchPlaneDispatcher {
    /// Explain one candidate (QI-BB-022): an exact presence lookup, and when
    /// the request names the query, the score the lexical engine emits for
    /// exactly this candidate under the plan that ranked it.
    fn explain(
        &self,
        request: SearchPlaneExplainQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError> {
        budget.checkpoint("explain:entry")?;
        let pin = request.generation;
        if request.candidate.manifest_generation != pin.manifest_generation {
            return Err(CoreError::InvalidContract(format!(
                "explain: candidate manifest_generation {} != pin {}",
                request.candidate.manifest_generation.get(),
                pin.manifest_generation.get()
            )));
        }
        if request.candidate.repo_id != pin.repo_id
            || request.candidate.revision_id != pin.revision_id
        {
            return Err(CoreError::InvalidContract(
                "explain: candidate (repo, revision) does not match pin".to_string(),
            ));
        }
        let materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let searcher =
            self.acquire_lexical(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let candidate_id = request.candidate.candidate_id.as_str();
        let Some(text_query) = request.text_query else {
            budget.checkpoint("explain:presence")?;
            let presence = searcher.candidate_presence(candidate_id)?;
            return Ok(SearchPlaneExplainQueryResponse {
                generation: pin,
                presence,
                explanation: build_presence_explanation(candidate_id, presence),
            });
        };
        // The query names its generation at most once, and it is this one.
        if text_query.generation_selector.is_some() {
            return Err(CoreError::InvalidContract(
                "explain: text_query must not carry a generation selector; the explain pins its generation"
                    .to_string(),
            ));
        }
        if text_query
            .generation
            .as_ref()
            .is_some_and(|query_pin| *query_pin != pin)
        {
            return Err(CoreError::InvalidContract(
                "explain: text_query generation does not match the explain pin".to_string(),
            ));
        }
        // The query is the one the search accepted, `top_k` included; the
        // trace does not page, but it does not accept a request the search
        // would have refused either.
        let _accepted_top_k = validate_query_top_k(text_query.top_k)?;
        let pinned_query = TextQueryRequest {
            generation: Some(pin.clone()),
            ..text_query
        };
        budget.checkpoint("explain:plan")?;
        let planned = self.plan_lexical_text_query(&pinned_query)?;
        if planned.pin != pin {
            return Err(CoreError::InvalidContract(format!(
                "explain: the query rebinds to generation {} but the candidate is at {}",
                planned.pin.manifest_generation.get(),
                pin.manifest_generation.get()
            )));
        }
        budget.checkpoint("explain:score")?;
        let explained = if planned.force_empty {
            match searcher.candidate_presence(candidate_id)? {
                CandidatePresenceV1::Indexed => LexicalCandidateExplanationV1::NotMatched {
                    reason: "the plan is a contradiction and matches nothing".to_string(),
                },
                CandidatePresenceV1::NotIndexed => LexicalCandidateExplanationV1::NotIndexed,
            }
        } else {
            searcher.explain_candidate(
                &planned.query,
                &planned.constraints,
                candidate_id,
                budget,
            )?
        };
        let presence = match explained {
            LexicalCandidateExplanationV1::NotIndexed => CandidatePresenceV1::NotIndexed,
            LexicalCandidateExplanationV1::NotMatched { .. }
            | LexicalCandidateExplanationV1::Matched(_) => CandidatePresenceV1::Indexed,
        };
        let explanation = build_lexical_score_explanation(
            candidate_id,
            request.candidate.score,
            &planned.query.options,
            &explained,
        )?;
        Ok(SearchPlaneExplainQueryResponse {
            generation: pin,
            presence,
            explanation,
        })
    }
}

impl ExplainQueryPort for SearchPlaneDispatcher {
    fn explain_query(
        &self,
        request: SearchPlaneExplainQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError> {
        self.explain(request, budget)
    }
}

/// Relative tolerance under which a candidate's carried score is the score
/// this plan emits for it.
const EXPLAIN_SCORE_TOLERANCE: f32 = 1e-5;

/// The explanation of a presence-only explain: what the lookup found and
/// nothing about scores, since no query was named.
fn build_presence_explanation(
    candidate_id: &str,
    presence: CandidatePresenceV1,
) -> SearchExplanation {
    let indexed = presence == CandidatePresenceV1::Indexed;
    SearchExplanation {
        planner_trace: vec![
            PlannerTraceEntry {
                stage: PlannerStage::Plan,
                detail: "explain.mode=presence_lookup".to_string(),
            },
            PlannerTraceEntry {
                stage: PlannerStage::Merge,
                detail: format!("explain.candidate_indexed={indexed}"),
            },
        ],
        engines_touched: vec![EngineTouched::Lexical],
        early_stop_reason: None,
        contributions: Vec::new(),
        ranker_weights_hash: [0u8; 32],
        strategy: "presence_lookup".to_string(),
        summary: if indexed {
            format!("candidate {candidate_id} is present in the lexical index (exact lookup)")
        } else {
            format!(
                "candidate {candidate_id} is NOT present in the lexical index (exact lookup: stale, removed, or never indexed)"
            )
        },
    }
}

/// The ranker inputs a lexical plan scores with, pinned as one digest: the
/// engine the plan runs on and the boost it applies. Two explanations with
/// equal hashes were scored under the same weights.
fn lexical_ranker_weights_hash_v1(options: &LqOptions) -> [u8; 32] {
    use sha2::Digest as _;
    let engine = if matches!(options.index_mode, Some(LqYesNoOnly::No)) {
        LexicalScoreEngineV1::UnindexedScan
    } else {
        LexicalScoreEngineV1::Bm25
    };
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"quanta-index lexical ranker weights v1\n");
    hasher.update(b"engine=");
    hasher.update(engine.as_str().as_bytes());
    hasher.update(b"\nboost_millis=");
    match options.boost_millis {
        Some(millis) => hasher.update(millis.to_string().as_bytes()),
        None => hasher.update(b"none"),
    }
    hasher.update(b"\n");
    hasher.finalize().into()
}

/// The explanation of a scored explain: one contribution row per signal,
/// summing to the emitted score, and whether the candidate's carried score
/// is that score.
fn build_lexical_score_explanation(
    candidate_id: &str,
    carried_score: f32,
    options: &LqOptions,
    explained: &LexicalCandidateExplanationV1,
) -> Result<SearchExplanation, CoreError> {
    let mut planner_trace = vec![PlannerTraceEntry {
        stage: PlannerStage::Plan,
        detail: "explain.mode=lexical_score_trace".to_string(),
    }];
    let (indexed, matched) = match explained {
        LexicalCandidateExplanationV1::NotIndexed => (false, false),
        LexicalCandidateExplanationV1::NotMatched { .. } => (true, false),
        LexicalCandidateExplanationV1::Matched(_) => (true, true),
    };
    planner_trace.push(PlannerTraceEntry {
        stage: PlannerStage::Merge,
        detail: format!("explain.candidate_indexed={indexed}"),
    });
    planner_trace.push(PlannerTraceEntry {
        stage: PlannerStage::Merge,
        detail: format!("explain.candidate_matched={matched}"),
    });
    let (contributions, summary) = match explained {
        LexicalCandidateExplanationV1::NotIndexed => (
            Vec::new(),
            format!("candidate {candidate_id} is NOT present in the lexical index (exact lookup)"),
        ),
        LexicalCandidateExplanationV1::NotMatched { reason } => (
            Vec::new(),
            format!(
                "candidate {candidate_id} is present in the lexical index but the query does not match it: {reason}"
            ),
        ),
        LexicalCandidateExplanationV1::Matched(trace) => {
            if !trace.emitted_score.is_finite() {
                return Err(CoreError::Storage(format!(
                    "explain: the lexical engine emitted a non-finite score for {candidate_id}"
                )));
            }
            let tolerance = EXPLAIN_SCORE_TOLERANCE * carried_score.abs().max(1.0);
            let reconciled = (trace.emitted_score - carried_score).abs() <= tolerance;
            planner_trace.push(PlannerTraceEntry {
                stage: PlannerStage::Merge,
                detail: format!("explain.score_reconciled={reconciled}"),
            });
            let rows = vec![ExplanationRow {
                signal_name: format!("lexical.{}", trace.engine.as_str()).into_boxed_str(),
                signal_value: trace.engine_score,
                weight: trace.boost_factor,
                contribution: trace.emitted_score,
            }];
            let summary = if reconciled {
                format!(
                    "candidate {candidate_id} is present and scores {:.6} under the query ({} {:.6} x boost {:.3}); the candidate's carried score is this score",
                    trace.emitted_score,
                    trace.engine.as_str(),
                    trace.engine_score,
                    trace.boost_factor
                )
            } else {
                format!(
                    "candidate {candidate_id} is present and scores {:.6} under the query ({} {:.6} x boost {:.3}); the candidate's carried score {carried_score:.6} is not this plan's score (fused or scored under another plan)",
                    trace.emitted_score,
                    trace.engine.as_str(),
                    trace.engine_score,
                    trace.boost_factor
                )
            };
            (rows, summary)
        }
    };
    Ok(SearchExplanation {
        planner_trace,
        engines_touched: vec![EngineTouched::Lexical],
        early_stop_reason: None,
        contributions,
        ranker_weights_hash: lexical_ranker_weights_hash_v1(options),
        strategy: "lexical_score_trace".to_string(),
        summary,
    })
}
