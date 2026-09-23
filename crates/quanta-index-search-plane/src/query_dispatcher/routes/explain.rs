//! Explain route: exact presence lookup and per-candidate score explanation
//! under the plan that ranked it.
//!
//! A lexical row is traced through the lexical engine. A hybrid row is
//! re-derived lane by lane against the index (QI-BB-022): the lexical lane
//! through the engine's own trace, the dense lane by embedding the dense
//! query and scoring the candidate's stored vector exactly, and the fusion
//! by re-running both bounded lanes under the same plan and reciprocal
//! rank fusion. Nothing the payload carries is used as an oracle for
//! itself; every carried number is compared with what the index says now.

use std::collections::BTreeSet;

use quanta_index_contract::{
    CandidatePresenceV1, ExplainCandidateV1, ExplanationRow, HybridCandidateV1, HybridLaneV1,
    LexicalCandidate, LqOptions, LqYesNoOnly, PlannerStage, PlannerTraceEntry, SearchExplanation,
    SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse, TextQueryRequest,
};
use quanta_index_core::{
    CoreError, ExplainQueryPort, HybridFilterPlanV1, HybridOrchestratorPolicy,
    LexicalCandidateExplanationV1, LexicalPageSpec, LexicalPolicy, LexicalScoreEngineV1,
    LexicalScoreTraceV1, LexicalSearcher, QueryRouteV1, RequestBudgetV1, SemanticSearcher,
    validate_query_top_k,
};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::dense_admission::admit_dense_lane_v1;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::execution_trace::{LaneExecutionRecorderV1, LaneExecutionSummaryV1};
use crate::query_dispatcher::planning::prepare_language_query_v1;
use crate::query_dispatcher::ranking::stabilize_ranked_candidates;
use crate::query_dispatcher::read_view::{ReadViewRequestV1, attach_read_view_trace};
use crate::query_dispatcher::window::hybrid_probe_top_k_v1;

impl SearchPlaneDispatcher {
    /// Explain one candidate (QI-BB-022): an exact presence lookup, and when
    /// the request names the query, the score the lexical engine emits for
    /// exactly this candidate under the plan that ranked it. A hybrid
    /// candidate is additionally re-derived on its dense lane and its
    /// fusion, and every carried number is reconciled against the index.
    fn explain(
        &self,
        request: SearchPlaneExplainQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError> {
        budget.checkpoint("explain:entry")?;
        request
            .validate_v1()
            .map_err(|err| CoreError::InvalidContract(format!("explain: {err}")))?;
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
        let candidate_id = row.candidate_id.as_str();
        let Some(text_query) = request.text_query else {
            let ExplainCandidateV1::Lexical(_) = &request.candidate else {
                return Err(CoreError::InvalidContract(
                    "explain: a hybrid candidate is explained under the query it was fused for; text_query is required"
                        .to_string(),
                ));
            };
            // A presence lookup has no plan: the view is the lexical track
            // alone.
            let view = self.acquire_read_view(
                &ReadViewRequestV1::declare("explain", QueryRouteV1::Explain, None, &pin),
                budget,
            )?;
            budget.checkpoint("explain:presence")?;
            // Invocation truth (W10-R1): the exact lookup is the one
            // lexical backend call on this path.
            let execution = LaneExecutionRecorderV1::new();
            execution.record_lexical_invocation();
            let presence = view.lexical()?.candidate_presence(candidate_id)?;
            execution.record_lexical_contribution();
            let mut explanation = build_presence_explanation(
                candidate_id,
                presence,
                &execution.summary(),
                budget.response_request_id(),
            );
            attach_read_view_trace(&mut explanation, view.identity());
            return Ok(SearchPlaneExplainQueryResponse {
                generation: pin,
                presence,
                explanation,
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
        // would have refused either. For a hybrid row this is the fused
        // `top_k` the hybrid ran with, which sizes the lanes re-run below.
        let accepted_top_k = validate_query_top_k(text_query.top_k)?;
        let pinned_query = TextQueryRequest {
            generation: Some(pin.clone()),
            ..text_query
        };
        budget.checkpoint("explain:plan")?;
        let (explanation, presence) = match &request.candidate {
            ExplainCandidateV1::Lexical(candidate) => {
                let planned =
                    self.plan_lexical_text_query(&pinned_query, QueryRouteV1::Explain, budget)?;
                if planned.pin != pin {
                    return Err(CoreError::InvalidContract(format!(
                        "explain: the query rebinds to generation {} but the candidate is at {}",
                        planned.pin.manifest_generation.get(),
                        pin.manifest_generation.get()
                    )));
                }
                let view = self.acquire_read_view(
                    &ReadViewRequestV1::new("explain", &pin, planned.domains),
                    budget,
                )?;
                let searcher = view.lexical()?;
                budget.checkpoint("explain:score")?;
                let execution = LaneExecutionRecorderV1::new();
                let explained = trace_lexical_candidate(
                    searcher.as_ref(),
                    &planned.query,
                    &planned.constraints,
                    planned.force_empty,
                    candidate_id,
                    budget,
                    &execution,
                )?;
                execution.record_lexical_contribution();
                let mut explanation = build_lexical_score_explanation(
                    candidate,
                    &planned.query,
                    &explained,
                    &execution.summary(),
                    budget.response_request_id(),
                )?;
                attach_read_view_trace(&mut explanation, view.identity());
                (explanation, presence_of(&explained))
            }
            ExplainCandidateV1::Hybrid(hybrid) => {
                let Some(semantic_query_text) = request.semantic_query_text.as_deref() else {
                    return Err(CoreError::InvalidContract(
                        "explain: a hybrid candidate is explained under the dense query it was fused for; semantic_query_text is required"
                            .to_string(),
                    ));
                };
                let lanes = HybridLanePlanV1::prepare(&pinned_query)?;
                let view = self.acquire_read_view(
                    &ReadViewRequestV1::declare(
                        "explain",
                        QueryRouteV1::Hybrid,
                        Some(&lanes.query),
                        &pin,
                    ),
                    budget,
                )?;
                let lex_searcher = view.lexical()?;
                let sem_searcher = view.semantic()?;
                budget.checkpoint("explain:score")?;
                // Invocation truth (W10-R1): the lexical trace, the exact
                // dense score, and the re-run lanes each record at their
                // backend call. Embedding is provider work, not a semantic
                // invocation, and records nothing here.
                let execution = LaneExecutionRecorderV1::new();
                let explained = trace_lexical_candidate(
                    lex_searcher.as_ref(),
                    &lanes.query,
                    &lanes.constraints,
                    lanes.force_empty,
                    candidate_id,
                    budget,
                    &execution,
                )?;
                let query_vector = self.embed_and_gate_query(
                    semantic_query_text,
                    sem_searcher.as_ref(),
                    "explain",
                    budget,
                )?;
                budget.checkpoint("explain:dense")?;
                execution.record_semantic_invocation();
                let dense_score =
                    sem_searcher.score_candidate(candidate_id, &query_vector, budget)?;
                let rederived = rederive_hybrid_lanes(
                    lex_searcher.as_ref(),
                    sem_searcher.as_ref(),
                    &lanes,
                    &query_vector,
                    accepted_top_k,
                    candidate_id,
                    budget,
                    &execution,
                )?;
                execution.record_lexical_contribution();
                execution.record_semantic_contribution();
                let mut explanation = build_hybrid_score_explanation(
                    hybrid,
                    &lanes.query,
                    &explained,
                    &HybridDenseDerivationV1 {
                        cosine: dense_score,
                        ranks: rederived,
                    },
                    &execution.summary(),
                    budget.response_request_id(),
                )?;
                explanation.planner_trace.push(PlannerTraceEntry {
                    stage: PlannerStage::Plan,
                    detail: format!("hybrid.filters={}", lanes.filter_plan),
                });
                attach_read_view_trace(&mut explanation, view.identity());
                (explanation, presence_of(&explained))
            }
        };
        Ok(SearchPlaneExplainQueryResponse {
            generation: pin,
            presence,
            explanation,
        })
    }
}

/// The two lanes' plan exactly as the hybrid route prepares it.
///
/// See `routes/hybrid.rs`: the lowered text query, its filter
/// classification for the dense lane, and the language-prepared query and
/// constraints both lanes run under. Kept step for step with the route so
/// the lanes an explain re-runs are the lanes the hybrid ran.
struct HybridLanePlanV1 {
    filter_plan: HybridFilterPlanV1,
    query: quanta_index_contract::LqQuery,
    constraints: quanta_index_contract::QueryConstraintSetV1,
    force_empty: bool,
}

impl HybridLanePlanV1 {
    fn prepare(text_query: &TextQueryRequest) -> Result<Self, CoreError> {
        let lexical_query = lower_lexical_text_query(text_query)?;
        let filter_plan = HybridFilterPlanV1::plan(&lexical_query)?;
        let prepared = prepare_language_query_v1(lexical_query, &text_query.constraints)?;
        LexicalPolicy::validate_query(&prepared.query)?;
        Ok(Self {
            filter_plan,
            query: prepared.query,
            constraints: prepared.constraints,
            force_empty: prepared.force_empty,
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

/// The lexical engine's trace of one candidate under the plan, or its
/// presence when the plan is a contradiction and matches nothing.
fn trace_lexical_candidate(
    searcher: &dyn LexicalSearcher,
    query: &quanta_index_contract::LqQuery,
    constraints: &quanta_index_contract::QueryConstraintSetV1,
    force_empty: bool,
    candidate_id: &str,
    budget: &RequestBudgetV1,
    execution: &LaneExecutionRecorderV1,
) -> Result<LexicalCandidateExplanationV1, CoreError> {
    // Both arms invoke the backend exactly once: a presence lookup under
    // a contradiction, the engine trace otherwise.
    execution.record_lexical_invocation();
    if force_empty {
        return Ok(match searcher.candidate_presence(candidate_id)? {
            CandidatePresenceV1::Indexed => LexicalCandidateExplanationV1::NotMatched {
                reason: "the plan is a contradiction and matches nothing".to_string(),
            },
            CandidatePresenceV1::NotIndexed => LexicalCandidateExplanationV1::NotIndexed,
        });
    }
    searcher.explain_candidate(query, constraints, candidate_id, budget)
}

/// The typed presence a lexical trace decides.
const fn presence_of(explained: &LexicalCandidateExplanationV1) -> CandidatePresenceV1 {
    match explained {
        LexicalCandidateExplanationV1::NotIndexed => CandidatePresenceV1::NotIndexed,
        LexicalCandidateExplanationV1::NotMatched { .. }
        | LexicalCandidateExplanationV1::Matched(_) => CandidatePresenceV1::Indexed,
    }
}

/// Where the two re-run lanes place the candidate.
///
/// Its 1-based rank among each lane's distinct identities (the rank the
/// fusion counts), or `None` when the bounded lane did not reach it; and
/// its position in the fused page the hybrid would serve for this `top_k`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RederivedHybridRanksV1 {
    lexical: Option<u32>,
    dense: Option<u32>,
    fused_page_position: Option<u32>,
}

impl RederivedHybridRanksV1 {
    /// The lane ranks in lane order, as the fusion sums them.
    fn ranks_in_lane_order(self) -> impl Iterator<Item = u32> {
        self.lexical.into_iter().chain(self.dense)
    }

    fn rank_of(self, lane: HybridLaneV1) -> Option<u32> {
        match lane {
            HybridLaneV1::Lexical => self.lexical,
            HybridLaneV1::Dense => self.dense,
        }
    }
}

/// The dense lane and the fusion as the index re-derives them for one
/// candidate.
struct HybridDenseDerivationV1 {
    /// The exact cosine of the candidate's stored vector against the
    /// embedded dense query; `None` when the generation stores no vector
    /// for it.
    cosine: Option<f32>,
    ranks: RederivedHybridRanksV1,
}

/// Re-run both bounded lanes exactly as the hybrid route runs them.
///
/// Same plan, same constraints, same internal over-fetch for `top_k`, same
/// dense admission under the plan's exact filters, same budget; then place
/// the candidate in each lane and in the fused page.
fn rederive_hybrid_lanes(
    lex_searcher: &dyn LexicalSearcher,
    sem_searcher: &dyn SemanticSearcher,
    lanes: &HybridLanePlanV1,
    query_vector: &[f32],
    top_k: u32,
    candidate_id: &str,
    budget: &RequestBudgetV1,
    execution: &LaneExecutionRecorderV1,
) -> Result<RederivedHybridRanksV1, CoreError> {
    if lanes.force_empty {
        return Ok(RederivedHybridRanksV1 {
            lexical: None,
            dense: None,
            fused_page_position: None,
        });
    }
    let internal_top_k = hybrid_probe_top_k_v1(top_k)?;
    budget.checkpoint("explain:lexical-lane")?;
    execution.record_lexical_invocation();
    let mut lex_rows = lex_searcher
        .search_constrained(
            &lanes.query,
            &lanes.constraints,
            &LexicalPageSpec::first(internal_top_k),
            budget,
        )?
        .candidates;
    stabilize_ranked_candidates(&mut lex_rows);
    let dense = admit_dense_lane_v1(
        &lanes.filter_plan,
        lex_searcher,
        &lanes.constraints,
        internal_top_k,
        budget,
        execution,
        |candidate: &LexicalCandidate| candidate.candidate_id.as_str(),
        |fetch_size| {
            budget.checkpoint("explain:dense-lane")?;
            execution.record_semantic_invocation();
            sem_searcher.search_constrained(query_vector, &lanes.constraints, fetch_size, budget)
        },
    )?;
    let mut sem_rows = dense.rows;
    stabilize_ranked_candidates(&mut sem_rows);
    budget.checkpoint("explain:fuse")?;
    let fused = HybridOrchestratorPolicy::fuse_rrf_candidates(&lex_rows, &sem_rows, top_k)?;
    let fused_page_position = fused
        .iter()
        .position(|row| row.candidate.candidate_id == candidate_id)
        .map(|index| checked_rank(index, "fused page"))
        .transpose()?;
    Ok(RederivedHybridRanksV1 {
        lexical: distinct_rank_of(&lex_rows, candidate_id)?,
        dense: distinct_rank_of(&sem_rows, candidate_id)?,
        fused_page_position,
    })
}

/// The 1-based rank of `candidate_id` among a lane's distinct identities,
/// which is the rank the fusion counts (a lane's repeated identities are
/// dropped), or `None` when the lane did not reach it.
fn distinct_rank_of(
    rows: &[LexicalCandidate],
    candidate_id: &str,
) -> Result<Option<u32>, CoreError> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for row in rows {
        if !seen.insert(row.candidate_id.as_str()) {
            continue;
        }
        if row.candidate_id == candidate_id {
            return checked_rank(seen.len().saturating_sub(1), "lane").map(Some);
        }
    }
    Ok(None)
}

/// A 0-based position as the 1-based `u32` rank a contribution carries.
fn checked_rank(index: usize, what: &str) -> Result<u32, CoreError> {
    u32::try_from(index.saturating_add(1)).map_err(|err| {
        CoreError::Storage(format!("explain: {what} position does not fit a rank: {err}"))
    })
}

/// Relative tolerance under which a candidate's carried score is the score
/// this plan emits for it.
const EXPLAIN_SCORE_TOLERANCE: f32 = 1e-5;

/// Absolute tolerance under which a carried cosine is the stored vector's
/// cosine: the dense lane's refine step reports the exact distance, so the
/// two differ by float rounding only.
const EXPLAIN_COSINE_TOLERANCE: f32 = 1e-4;

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
    execution: &LaneExecutionSummaryV1,
    request_id: u64,
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
        // Single-sourced (W10-R1): both engine lists derive from the
        // observed invocation truth.
        engines_touched: execution.touched_engines(),
        engines_executed: execution.executed_engines(),
        // W10-R2: the route's budget correlation; 0 only off-transport.
        request_id,
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

/// The plan's filter leaves, one trace entry each, so an unmatched
/// candidate names the filters that stood between it and the page.
///
/// The lexical engine reports a non-match as one fact; which leaf excluded
/// the candidate is the plan's to list, and it is listed in plan order.
fn plan_filter_trace_v1(query: &quanta_index_contract::LqQuery) -> Vec<PlannerTraceEntry> {
    let mut entries = vec![PlannerTraceEntry {
        stage: PlannerStage::Plan,
        detail: format!("explain.plan_filters={}", query.filters.len()),
    }];
    entries.extend(
        query
            .filters
            .iter()
            .enumerate()
            .map(|(index, filter)| PlannerTraceEntry {
                stage: PlannerStage::Plan,
                detail: format!("explain.plan_filter[{index}]={filter:?}"),
            }),
    );
    entries
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
    query: &quanta_index_contract::LqQuery,
    explained: &LexicalCandidateExplanationV1,
    execution: &LaneExecutionSummaryV1,
    request_id: u64,
) -> Result<SearchExplanation, CoreError> {
    let candidate_id = candidate.candidate_id.as_str();
    let carried_score = candidate.score;
    let mut planner_trace = scored_trace_head_v1("lexical_score_trace", explained);
    let (contributions, summary) = match explained {
        LexicalCandidateExplanationV1::NotIndexed => (
            Vec::new(),
            format!("candidate {candidate_id} is NOT present in the lexical index (exact lookup)"),
        ),
        LexicalCandidateExplanationV1::NotMatched { reason } => {
            planner_trace.extend(plan_filter_trace_v1(query));
            (
                Vec::new(),
                format!(
                    "candidate {candidate_id} is present in the lexical index but the query does not match it: {reason}{}",
                    plan_filter_prose_v1(query)
                ),
            )
        }
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
        // Single-sourced (W10-R1): both engine lists derive from the
        // observed invocation truth.
        engines_touched: execution.touched_engines(),
        engines_executed: execution.executed_engines(),
        // W10-R2: the route's budget correlation; 0 only off-transport.
        request_id,
        early_stop_reason: None,
        contributions,
        ranker_weights_hash: ranker_weights_hash_v1(&query.options, RankerFusionV1::None),
        strategy: "lexical_score_trace".to_string(),
        summary,
    })
}

/// The plan's filter leaves in prose, for an unmatched candidate's
/// summary; empty when the plan has none.
fn plan_filter_prose_v1(query: &quanta_index_contract::LqQuery) -> String {
    if query.filters.is_empty() {
        return String::new();
    }
    let leaves = query
        .filters
        .iter()
        .map(|filter| format!("{filter:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(" (plan filters: {leaves})")
}

/// Narrow an RRF term or sum to the `f32` an explanation row carries.
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

/// Whether the dense lane's re-derivation agrees with a hybrid row's
/// provenance.
///
/// A carried dense contribution must be the stored vector's cosine within
/// [`EXPLAIN_COSINE_TOLERANCE`]. A row the dense lane did not carry is
/// consistent when the generation stores no vector for it, or when the
/// re-run dense lane does not reach it; it is not when the re-run lane
/// ranks it, since the lane should then have carried it.
fn hybrid_dense_lane_reconciles(
    hybrid: &HybridCandidateV1,
    derived: &HybridDenseDerivationV1,
) -> bool {
    match (hybrid.contribution(HybridLaneV1::Dense), derived.cosine) {
        (Some(dense), Some(cosine)) => (dense.raw_score - cosine).abs() <= EXPLAIN_COSINE_TOLERANCE,
        (Some(_), None) => false,
        (None, None) => true,
        (None, Some(_)) => derived.ranks.dense.is_none(),
    }
}

/// Whether the re-derived fusion reproduces the carried provenance.
///
/// Every carried lane rank is the re-run lane's rank, no re-run lane ranks
/// a candidate that lane did not carry, and the carried `fused_score` is
/// the RRF of the re-derived ranks.
fn hybrid_fusion_reconciles(hybrid: &HybridCandidateV1, ranks: RederivedHybridRanksV1) -> bool {
    let lanes_agree = [HybridLaneV1::Lexical, HybridLaneV1::Dense]
        .into_iter()
        .all(|lane| {
            hybrid
                .contribution(lane)
                .map(|contribution| contribution.rank)
                == ranks.rank_of(lane)
        });
    let rederived = HybridOrchestratorPolicy::rrf_score(ranks.ranks_in_lane_order());
    lanes_agree && rederived.total_cmp(&hybrid.fused_score).is_eq()
}

/// The explanation of a hybrid candidate's score (QI-BB-022), re-derived
/// against the index on every axis.
///
/// Three reconciliations are traced, each against what the index says
/// now, never against the payload alone: `explain.score_reconciled` (the
/// lexical lane's emitted score against the carried lexical raw score),
/// `explain.dense_reconciled` (the stored vector's exact cosine against
/// the carried dense raw score) and `explain.fused_reconciled` (the ranks
/// of both re-run lanes and their RRF against the carried ranks and
/// `fused_score`). A forged payload fails on the axis it forged.
///
/// The rows are, in order: `lexical.<engine>` when the plan matches the
/// candidate (the emitted lexical score, in the engine's units),
/// `dense.cosine` when the generation stores a vector for it (the exact
/// cosine, re-derived), then one `hybrid.rrf.<lane>` row per re-run lane
/// that reached it, carrying that lane's RRF term `1 / (k + rank)`. The
/// `hybrid.rrf.*` rows sum to the re-derived fused score; the lane score
/// rows are in their own units and are not summed — see
/// [`ExplanationRow`].
fn build_hybrid_score_explanation(
    hybrid: &HybridCandidateV1,
    query: &quanta_index_contract::LqQuery,
    explained: &LexicalCandidateExplanationV1,
    derived: &HybridDenseDerivationV1,
    execution: &LaneExecutionSummaryV1,
    request_id: u64,
) -> Result<SearchExplanation, CoreError> {
    let mut report = HybridTraceReportV1::open(hybrid, explained);
    report.lexical_lane(hybrid, query, explained)?;
    report.dense_lane(hybrid, derived);
    report.fusion(hybrid, derived.ranks);
    Ok(report.close(&query.options, execution, request_id))
}

/// The hybrid explanation under assembly: one method per lane, then the
/// fusion, each appending its rows, trace entries and summary fragment.
struct HybridTraceReportV1 {
    planner_trace: Vec<PlannerTraceEntry>,
    contributions: Vec<ExplanationRow>,
    summary: Vec<String>,
}

impl HybridTraceReportV1 {
    fn open(hybrid: &HybridCandidateV1, explained: &LexicalCandidateExplanationV1) -> Self {
        Self {
            planner_trace: scored_trace_head_v1("hybrid_score_trace", explained),
            contributions: Vec::with_capacity(4),
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
        query: &quanta_index_contract::LqQuery,
        explained: &LexicalCandidateExplanationV1,
    ) -> Result<(), CoreError> {
        let candidate_id = hybrid.candidate.candidate_id.as_str();
        match explained {
            LexicalCandidateExplanationV1::NotIndexed => {
                self.summary
                    .push(" NOT present in the lexical index (exact lookup)".to_string());
            }
            LexicalCandidateExplanationV1::NotMatched { reason } => {
                self.planner_trace.extend(plan_filter_trace_v1(query));
                self.summary.push(format!(
                    " present in the lexical index but the query does not match it: {reason}{}",
                    plan_filter_prose_v1(query)
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

    /// The dense lane: the stored vector's exact cosine against the
    /// embedded dense query, and whether the carried dense contribution is
    /// that cosine.
    fn dense_lane(&mut self, hybrid: &HybridCandidateV1, derived: &HybridDenseDerivationV1) {
        if let Some(cosine) = derived.cosine {
            self.contributions.push(ExplanationRow {
                signal_name: "dense.cosine".into(),
                signal_value: cosine,
                weight: 1.0,
                contribution: cosine,
            });
            self.merge_entry(format!("explain.dense_lane=rederived; cosine={cosine:.6}"));
            self.summary.push(format!(
                "; its stored vector scores cosine {cosine:.6} against the dense query"
            ));
        } else {
            self.merge_entry("explain.dense_lane=absent".to_string());
            self.summary
                .push("; the generation stores no vector for it".to_string());
        }
        let reconciled = hybrid_dense_lane_reconciles(hybrid, derived);
        self.merge_entry(format!("explain.dense_reconciled={reconciled}"));
        self.summary.push(
            match (hybrid.contribution(HybridLaneV1::Dense), reconciled) {
                (Some(_), true) => "; the dense lane's carried raw score is this cosine",
                (Some(_), false) => "; the dense lane's carried raw score is NOT this cosine",
                (None, true) => "; the dense lane did not see it, consistent with the index",
                (None, false) => {
                    "; the dense lane did not see it although the re-run dense lane ranks it"
                }
            }
            .to_string(),
        );
    }

    /// The fusion: the re-run lanes' ranks as RRF rows, the fused page
    /// position, and whether the carried ranks and `fused_score` are the
    /// re-derived ones.
    fn fusion(&mut self, hybrid: &HybridCandidateV1, ranks: RederivedHybridRanksV1) {
        for lane in [HybridLaneV1::Lexical, HybridLaneV1::Dense] {
            if let Some(rank) = ranks.rank_of(lane) {
                let term =
                    narrow_rrf_score(HybridOrchestratorPolicy::rrf_score(std::iter::once(rank)));
                self.contributions.push(ExplanationRow {
                    signal_name: format!("hybrid.rrf.{}", lane.as_code_str()).into_boxed_str(),
                    signal_value: 1.0,
                    weight: term,
                    contribution: term,
                });
            }
        }
        let rederived = HybridOrchestratorPolicy::rrf_score(ranks.ranks_in_lane_order());
        let reconciled = hybrid_fusion_reconciles(hybrid, ranks);
        self.merge_entry(format!(
            "explain.rrf_k={}; carried_ranks={}; rederived_ranks={}",
            HybridOrchestratorPolicy::rrf_k(),
            carried_ranks_detail(hybrid),
            rederived_ranks_detail(ranks)
        ));
        self.merge_entry(format!("explain.fused_rederived={rederived:.9}"));
        self.merge_entry(format!(
            "explain.fused_page_position={}",
            ranks
                .fused_page_position
                .map_or_else(|| "beyond_top_k".to_string(), |position| position.to_string())
        ));
        self.merge_entry(format!("explain.fused_reconciled={reconciled}"));
        self.summary.push(if reconciled {
            format!(
                "; rrf over the re-run lanes is {rederived:.9}, the carried fused score at the carried ranks"
            )
        } else {
            format!(
                "; rrf over the re-run lanes is {rederived:.9} at ranks {}, NOT the carried fused score {:.9} at ranks {}",
                rederived_ranks_detail(ranks),
                hybrid.fused_score,
                carried_ranks_detail(hybrid)
            )
        });
    }

    fn close(
        self,
        options: &LqOptions,
        execution: &LaneExecutionSummaryV1,
        request_id: u64,
    ) -> SearchExplanation {
        SearchExplanation {
            planner_trace: self.planner_trace,
            // Single-sourced (W10-R1): both engine lists derive from the
            // observed invocation truth.
            engines_touched: execution.touched_engines(),
            engines_executed: execution.executed_engines(),
            // W10-R2: the route's budget correlation; 0 only off-transport.
            request_id,
            early_stop_reason: None,
            contributions: self.contributions,
            ranker_weights_hash: ranker_weights_hash_v1(options, RankerFusionV1::Rrf),
            strategy: "hybrid_score_trace".to_string(),
            summary: self.summary.concat(),
        }
    }
}

/// The carried ranks as `lane#rank`, comma-separated, for the trace.
fn carried_ranks_detail(hybrid: &HybridCandidateV1) -> String {
    hybrid
        .contributions
        .iter()
        .map(|contribution| format!("{}#{}", contribution.lane.as_code_str(), contribution.rank))
        .collect::<Vec<_>>()
        .join(",")
}

/// The re-derived ranks as `lane#rank`, comma-separated, for the trace;
/// `lane#absent` for a lane that did not reach the candidate.
fn rederived_ranks_detail(ranks: RederivedHybridRanksV1) -> String {
    [HybridLaneV1::Lexical, HybridLaneV1::Dense]
        .into_iter()
        .map(|lane| {
            format!(
                "{}#{}",
                lane.as_code_str(),
                ranks
                    .rank_of(lane)
                    .map_or_else(|| "absent".to_string(), |rank| rank.to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}
