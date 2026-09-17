//! Explain route: exact presence lookup and per-candidate score explanation
//! under the plan that ranked it.

use quanta_index_contract::{
    CandidatePresenceV1, EngineTouched, ExplainCandidateV1, ExplanationRow, HybridCandidateV1,
    HybridLaneContributionV1, HybridLaneV1, LexicalCandidate, LqOptions, LqYesNoOnly, PlannerStage,
    PlannerTraceEntry, SearchExplanation, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse, TextQueryRequest,
};
use quanta_index_core::{
    CoreError, ExplainQueryPort, HybridOrchestratorPolicy, LexicalCandidateExplanationV1,
    LexicalPolicy, LexicalScoreEngineV1, LexicalScoreTraceV1, RequestBudgetV1,
    validate_query_top_k,
};

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;

impl SearchPlaneDispatcher {
    /// Explain one candidate (QI-BB-022): an exact presence lookup, and when
    /// the request names the query, the score the lexical engine emits for
    /// exactly this candidate under the plan that ranked it. A hybrid
    /// candidate is additionally reconciled against the lane provenance and
    /// the RRF score it carries.
    fn explain(
        &self,
        request: SearchPlaneExplainQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError> {
        budget.checkpoint("explain:entry")?;
        let pin = request.generation;
        let row = request.candidate.lexical_row();
        if row.manifest_generation != pin.manifest_generation {
            return Err(CoreError::InvalidContract(format!(
                "explain: candidate manifest_generation {} != pin {}",
                row.manifest_generation.get(),
                pin.manifest_generation.get()
            )));
        }
        if row.repo_id != pin.repo_id || row.revision_id != pin.revision_id {
            return Err(CoreError::InvalidContract(
                "explain: candidate (repo, revision) does not match pin".to_string(),
            ));
        }
        let materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, materialized)?;
        let searcher =
            self.acquire_lexical(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let candidate_id = row.candidate_id.as_str();
        let Some(text_query) = request.text_query else {
            let ExplainCandidateV1::Lexical(_) = &request.candidate else {
                return Err(CoreError::InvalidContract(
                    "explain: a hybrid candidate is explained under the query it was fused for; text_query is required"
                        .to_string(),
                ));
            };
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
        let explanation = match &request.candidate {
            ExplainCandidateV1::Lexical(candidate) => {
                build_lexical_score_explanation(candidate, &planned.query.options, &explained)?
            }
            ExplainCandidateV1::Hybrid(candidate) => {
                build_hybrid_score_explanation(candidate, &planned.query.options, &explained)?
            }
        };
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

/// Whether `emitted` is `carried` within [`EXPLAIN_SCORE_TOLERANCE`],
/// relative to the carried magnitude (absolute below one).
fn scores_reconcile(emitted: f32, carried: f32) -> bool {
    let tolerance = EXPLAIN_SCORE_TOLERANCE * carried.abs().max(1.0);
    (emitted - carried).abs() <= tolerance
}

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

/// The fusion a ranker weights hash pins beside the lexical plan: none for
/// a lexical explain, the RRF constant for a hybrid one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RankerFusionV1 {
    None,
    Rrf,
}

/// The ranker inputs a plan scores with, pinned as one digest.
///
/// The digest covers the engine the lexical plan runs on, the boost it
/// applies, and the fusion over it. Two explanations with equal hashes
/// were scored under the same weights.
fn ranker_weights_hash_v1(options: &LqOptions, fusion: RankerFusionV1) -> [u8; 32] {
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
    if fusion == RankerFusionV1::Rrf {
        hasher.update(b"fusion=rrf\nrrf_k=");
        hasher.update(HybridOrchestratorPolicy::rrf_k().to_string().as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().into()
}

/// The trace entries every scored explain opens with: its mode, and whether
/// the candidate is indexed and matched under the plan.
fn scored_trace_head_v1(
    mode: &str,
    explained: &LexicalCandidateExplanationV1,
) -> Vec<PlannerTraceEntry> {
    let (indexed, matched) = match explained {
        LexicalCandidateExplanationV1::NotIndexed => (false, false),
        LexicalCandidateExplanationV1::NotMatched { .. } => (true, false),
        LexicalCandidateExplanationV1::Matched(_) => (true, true),
    };
    vec![
        PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail: format!("explain.mode={mode}"),
        },
        PlannerTraceEntry {
            stage: PlannerStage::Merge,
            detail: format!("explain.candidate_indexed={indexed}"),
        },
        PlannerTraceEntry {
            stage: PlannerStage::Merge,
            detail: format!("explain.candidate_matched={matched}"),
        },
    ]
}

/// The one contribution row of a matched lexical trace: the engine's own
/// score, the plan's boost as the weight, and the emitted score.
fn lexical_trace_row_v1(
    candidate_id: &str,
    trace: &LexicalScoreTraceV1,
) -> Result<ExplanationRow, CoreError> {
    if !trace.emitted_score.is_finite() {
        return Err(CoreError::Storage(format!(
            "explain: the lexical engine emitted a non-finite score for {candidate_id}"
        )));
    }
    Ok(ExplanationRow {
        signal_name: format!("lexical.{}", trace.engine.as_str()).into_boxed_str(),
        signal_value: trace.engine_score,
        weight: trace.boost_factor,
        contribution: trace.emitted_score,
    })
}

/// How the explain describes a matched lexical trace in prose.
fn lexical_trace_prose_v1(trace: &LexicalScoreTraceV1) -> String {
    format!(
        "scores {:.6} under the query ({} {:.6} x boost {:.3})",
        trace.emitted_score,
        trace.engine.as_str(),
        trace.engine_score,
        trace.boost_factor
    )
}

/// The explanation of a scored explain: one contribution row per signal,
/// summing to the emitted score, and whether the candidate's carried score
/// is that score.
fn build_lexical_score_explanation(
    candidate: &LexicalCandidate,
    options: &LqOptions,
    explained: &LexicalCandidateExplanationV1,
) -> Result<SearchExplanation, CoreError> {
    let candidate_id = candidate.candidate_id.as_str();
    let carried_score = candidate.score;
    let mut planner_trace = scored_trace_head_v1("lexical_score_trace", explained);
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
            let row = lexical_trace_row_v1(candidate_id, trace)?;
            let reconciled = scores_reconcile(trace.emitted_score, carried_score);
            planner_trace.push(PlannerTraceEntry {
                stage: PlannerStage::Merge,
                detail: format!("explain.score_reconciled={reconciled}"),
            });
            let prose = lexical_trace_prose_v1(trace);
            let summary = if reconciled {
                format!(
                    "candidate {candidate_id} is present and {prose}; the candidate's carried score is this score"
                )
            } else {
                format!(
                    "candidate {candidate_id} is present and {prose}; the candidate's carried score {carried_score:.6} is not this plan's score (fused or scored under another plan)"
                )
            };
            (vec![row], summary)
        }
    };
    Ok(SearchExplanation {
        planner_trace,
        engines_touched: vec![EngineTouched::Lexical],
        early_stop_reason: None,
        contributions,
        ranker_weights_hash: ranker_weights_hash_v1(options, RankerFusionV1::None),
        strategy: "lexical_score_trace".to_string(),
        summary,
    })
}

/// Narrow an RRF score to the `f32` an explanation row carries.
///
/// An RRF score is a sum of at most two terms `1 / (k + rank)` with `k`
/// positive, so it lies in `(0, 2 / k]`; narrowing cannot overflow, it only
/// rounds.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "an RRF score is within (0, 2 / k]; narrowing to f32 only rounds"
)]
fn narrow_rrf_score(score: f64) -> f32 {
    score as f32
}

/// The RRF score recomputed from a hybrid row's carried ranks, and whether
/// it is the row's carried `fused_score`.
struct RrfReconciliationV1 {
    recomputed: f64,
    reconciled: bool,
    /// The carried ranks as `lane#rank`, comma-separated, for the trace.
    ranks_detail: String,
}

/// Recompute a hybrid row's RRF score from the ranks it carries.
///
/// The ranks are summed in lane order with the fusion's own arithmetic, so
/// a genuine row reproduces its `fused_score` exactly; the wire carries the
/// f64 the plane sorted by, so no tolerance is owed.
fn rrf_reconciliation_v1(hybrid: &HybridCandidateV1) -> RrfReconciliationV1 {
    let ranks = hybrid
        .contributions
        .iter()
        .map(|contribution| contribution.rank);
    let recomputed = HybridOrchestratorPolicy::rrf_score(ranks);
    RrfReconciliationV1 {
        recomputed,
        reconciled: recomputed.total_cmp(&hybrid.fused_score).is_eq(),
        ranks_detail: hybrid
            .contributions
            .iter()
            .map(|contribution| {
                format!("{}#{}", contribution.lane.as_code_str(), contribution.rank)
            })
            .collect::<Vec<_>>()
            .join(","),
    }
}

/// Whether the lexical lane's trace agrees with a hybrid row's provenance.
///
/// It agrees when the emitted score is the carried lexical raw score, or
/// when the plan does not match a row the lexical lane did not see.
fn hybrid_lexical_lane_reconciles(
    hybrid: &HybridCandidateV1,
    explained: &LexicalCandidateExplanationV1,
) -> bool {
    match (hybrid.contribution(HybridLaneV1::Lexical), explained) {
        (Some(lexical), LexicalCandidateExplanationV1::Matched(trace)) => {
            scores_reconcile(trace.emitted_score, lexical.raw_score)
        }
        (
            None,
            LexicalCandidateExplanationV1::NotIndexed
            | LexicalCandidateExplanationV1::NotMatched { .. },
        ) => true,
        (
            Some(_),
            LexicalCandidateExplanationV1::NotIndexed
            | LexicalCandidateExplanationV1::NotMatched { .. },
        )
        | (None, LexicalCandidateExplanationV1::Matched(_)) => false,
    }
}

/// The explanation of a hybrid candidate's score (QI-BB-022).
///
/// The lexical lane is traced under the plan and reconciled with the
/// carried lexical contribution, the dense contribution is the row the
/// fusion carried, and the RRF score is recomputed from the carried ranks
/// against the carried `fused_score`.
///
/// The rows are, in order: `lexical.<engine>` when the plan matches the
/// candidate (its emitted score, this plane's authority), `dense.cosine`
/// when the dense lane saw it (the carried raw score; the explain does not
/// re-run the dense lane), and `hybrid.rrf` (the recomputed RRF score, the
/// ranking key). Two kinds of reconciliation are traced:
/// `explain.score_reconciled` for the lexical lane and
/// `explain.fused_reconciled` for the RRF arithmetic.
fn build_hybrid_score_explanation(
    hybrid: &HybridCandidateV1,
    options: &LqOptions,
    explained: &LexicalCandidateExplanationV1,
) -> Result<SearchExplanation, CoreError> {
    let mut report = HybridTraceReportV1::open(hybrid, explained);
    report.lexical_lane(hybrid, explained)?;
    if let Some(dense) = hybrid.contribution(HybridLaneV1::Dense) {
        report.dense_lane(dense);
    }
    report.fused_score(hybrid);
    Ok(report.close(options))
}

/// The hybrid explanation under assembly: one method per lane, then the
/// fused row, each appending its rows, trace entries and summary fragment.
struct HybridTraceReportV1 {
    planner_trace: Vec<PlannerTraceEntry>,
    contributions: Vec<ExplanationRow>,
    summary: Vec<String>,
}

impl HybridTraceReportV1 {
    fn open(hybrid: &HybridCandidateV1, explained: &LexicalCandidateExplanationV1) -> Self {
        Self {
            planner_trace: scored_trace_head_v1("hybrid_score_trace", explained),
            contributions: Vec::with_capacity(3),
            summary: vec![format!(
                "hybrid candidate {}:",
                hybrid.candidate.candidate_id
            )],
        }
    }

    fn merge_entry(&mut self, detail: String) {
        self.planner_trace.push(PlannerTraceEntry {
            stage: PlannerStage::Merge,
            detail,
        });
    }

    /// The lexical lane: the plan's own trace of the candidate, and whether
    /// it agrees with the lexical contribution the row carries.
    fn lexical_lane(
        &mut self,
        hybrid: &HybridCandidateV1,
        explained: &LexicalCandidateExplanationV1,
    ) -> Result<(), CoreError> {
        let candidate_id = hybrid.candidate.candidate_id.as_str();
        match explained {
            LexicalCandidateExplanationV1::NotIndexed => {
                self.summary
                    .push(" NOT present in the lexical index (exact lookup)".to_string());
            }
            LexicalCandidateExplanationV1::NotMatched { reason } => {
                self.summary.push(format!(
                    " present in the lexical index but the query does not match it: {reason}"
                ));
            }
            LexicalCandidateExplanationV1::Matched(trace) => {
                self.contributions
                    .push(lexical_trace_row_v1(candidate_id, trace)?);
                self.summary
                    .push(format!(" present and {}", lexical_trace_prose_v1(trace)));
            }
        }
        let reconciled = hybrid_lexical_lane_reconciles(hybrid, explained);
        self.merge_entry(format!("explain.score_reconciled={reconciled}"));
        self.summary.push(
            match (hybrid.contribution(HybridLaneV1::Lexical), reconciled) {
                (Some(_), true) => "; the lexical lane's carried raw score is this score",
                (Some(_), false) => {
                    "; the lexical lane's carried raw score is NOT this plan's score"
                }
                (None, true) => "; the lexical lane did not see it, consistent with the plan",
                (None, false) => "; the lexical lane did not see it although the plan matches it",
            }
            .to_string(),
        );
        Ok(())
    }

    /// The dense lane: the contribution as the fusion carried it. The
    /// explain does not re-run the dense lane, and says so.
    fn dense_lane(&mut self, dense: &HybridLaneContributionV1) {
        self.contributions.push(ExplanationRow {
            signal_name: "dense.cosine".into(),
            signal_value: dense.raw_score,
            weight: 1.0,
            contribution: dense.raw_score,
        });
        self.merge_entry(format!(
            "explain.dense_lane=carried; rank={}; raw_score={:.6}",
            dense.rank, dense.raw_score
        ));
        self.summary.push(format!(
            "; the dense lane ranked it #{} at cosine {:.6} (carried, not re-derived)",
            dense.rank, dense.raw_score
        ));
    }

    /// The fused row: the RRF of the carried ranks, against the carried
    /// fused score.
    fn fused_score(&mut self, hybrid: &HybridCandidateV1) {
        let rrf = rrf_reconciliation_v1(hybrid);
        self.contributions.push(ExplanationRow {
            signal_name: "hybrid.rrf".into(),
            signal_value: narrow_rrf_score(rrf.recomputed),
            weight: 1.0,
            contribution: narrow_rrf_score(rrf.recomputed),
        });
        self.merge_entry(format!(
            "explain.rrf_k={}; ranks={}",
            HybridOrchestratorPolicy::rrf_k(),
            rrf.ranks_detail
        ));
        self.merge_entry(format!("explain.fused_reconciled={}", rrf.reconciled));
        self.summary.push(if rrf.reconciled {
            format!(
                "; rrf over the carried ranks is {:.9}, the carried fused score",
                rrf.recomputed
            )
        } else {
            format!(
                "; rrf over the carried ranks is {:.9}, NOT the carried fused score {:.9}",
                rrf.recomputed, hybrid.fused_score
            )
        });
    }

    fn close(self, options: &LqOptions) -> SearchExplanation {
        SearchExplanation {
            planner_trace: self.planner_trace,
            engines_touched: vec![EngineTouched::Lexical],
            early_stop_reason: None,
            contributions: self.contributions,
            ranker_weights_hash: ranker_weights_hash_v1(options, RankerFusionV1::Rrf),
            strategy: "hybrid_score_trace".to_string(),
            summary: self.summary.concat(),
        }
    }
}
