//! Hybrid (lexical + semantic RRF) query route and the shared fusion core.

use std::collections::BTreeSet;
use std::time::Duration;

use quanta_index_contract::{
    CandidateCountV1, EarlyStopReason, GoTestTraceFileHitV1, GoTestTraceFileLaneWitnessV1,
    GoTestTraceFileQueryRequestV1, GoTestTraceFileQueryResponseV1, GoTestTraceFrameClassV1,
    HybridQueryRequest, HybridQueryResponse, LaneTraceV1, LexicalCandidate, QueryStageKindV1,
    TextQueryRequest,
};
use quanta_index_core::{
    CoreError, HybridOrchestratorPolicy, HybridQueryPort, LexicalPageSpec, QueryRouteV1,
    RequestBudgetV1, TraceFileEvidenceV1, TraceFileLaneWitnessV1, TraceFileRankPolicyV1,
    TraceSourceFrameClassV1,
};

use crate::query_dispatcher::dense_admission::admit_dense_lane_v1;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::execution_trace::{LaneExecutionRecorderV1, LaneExecutionSummaryV1};
use crate::query_dispatcher::ranking::stabilize_ranked_candidates;
use crate::query_dispatcher::read_view::{ReadViewRequestV1, attach_read_view_trace};
use crate::query_dispatcher::selection::SemanticSelection;
use crate::query_dispatcher::semantic_query::{
    HybridFilterTraceV1, HybridFusion, HybridLaneTallyV1, build_hybrid_response_explanation,
    resolve_hybrid_request_selection,
};
use crate::query_dispatcher::stage_timing::{StageTimings, measured};
use crate::query_dispatcher::window::{fused_window_v2, hybrid_probe_top_k_v1, lane_count_u64};

/// Lane traces for the fused window.
///
/// Lexical and dense lanes carry their executed/contributed split from the
/// observed invocation truth, candidate counts, and the dense admission
/// examination count as the dense lane's cost observation.
fn hybrid_lane_traces(
    execution: &LaneExecutionSummaryV1,
    lexical_hits: usize,
    dense_hits: usize,
    dense_examined: usize,
    dense_admitted: usize,
) -> Result<Vec<quanta_index_contract::LaneTraceV1>, CoreError> {
    use quanta_index_contract::{CandidateCountV1, LaneTraceV1};
    let excluded = dense_examined.saturating_sub(dense_admitted);
    Ok(vec![
        LaneTraceV1::new(
            "hybrid.lexical",
            execution.lexical_executed(),
            execution.lexical_contributed,
        )
        .with_candidates(CandidateCountV1::AtLeast(lane_count_u64(lexical_hits)?)),
        LaneTraceV1::new(
            "hybrid.dense",
            execution.semantic_executed(),
            execution.semantic_contributed,
        )
        .with_candidates(CandidateCountV1::AtLeast(lane_count_u64(dense_hits)?))
        .with_filtered_out(lane_count_u64(excluded)?)
        .with_cost(lane_count_u64(dense_examined)?),
    ])
}

impl SearchPlaneDispatcher {
    pub(crate) fn go_test_trace_file(
        &self,
        request: &GoTestTraceFileQueryRequestV1,
        budget: &RequestBudgetV1,
    ) -> Result<GoTestTraceFileQueryResponseV1, CoreError> {
        request.validate_v1().map_err(CoreError::InvalidContract)?;
        budget.checkpoint("go-test-trace-file:entry")?;
        let selection =
            resolve_hybrid_request_selection(self.activation_catalog.as_ref(), &request.query)?;
        let mut fusion = self.execute_hybrid_fusion(
            &selection,
            &request.query.text_query,
            &request.query.semantic_query_text,
            request.query.top_k,
            "go test trace file",
            budget,
        )?;
        let lane_top_k = request.query.top_k;
        let lane_limit = usize::try_from(lane_top_k).map_err(|error| {
            CoreError::InvalidContract(format!("trace lane limit exceeds usize: {error}"))
        })?;
        let internal_limit = usize::try_from(fusion.internal_top_k).map_err(|error| {
            CoreError::InvalidContract(format!("trace probe limit exceeds usize: {error}"))
        })?;
        let lexical_count = fusion.lexical_rows.len();
        let dense_count = fusion.dense_rows.len();
        let lexical_window = fused_window_v2(
            lane_top_k,
            lexical_count.min(lane_limit),
            lexical_count,
            lexical_count == internal_limit,
            None,
            fusion.internal_top_k,
            vec![
                LaneTraceV1::new(
                    "go_test_trace.lexical",
                    fusion.execution.lexical_executed(),
                    fusion.execution.lexical_contributed,
                )
                .with_candidates(CandidateCountV1::AtLeast(lane_count_u64(lexical_count)?)),
            ],
        )?;
        let dense_window = fused_window_v2(
            lane_top_k,
            dense_count.min(lane_limit),
            dense_count,
            dense_count == internal_limit,
            Some(fusion.dense_outcome),
            fusion.internal_top_k,
            vec![
                LaneTraceV1::new(
                    "go_test_trace.dense",
                    fusion.execution.semantic_executed(),
                    fusion.execution.semantic_contributed,
                )
                .with_candidates(CandidateCountV1::AtLeast(lane_count_u64(dense_count)?))
                .with_filtered_out(lane_count_u64(
                    fusion.dense_examined.saturating_sub(dense_count),
                )?)
                .with_cost(lane_count_u64(fusion.dense_examined)?),
            ],
        )?;
        fusion.lexical_rows.truncate(lane_limit);
        fusion.dense_rows.truncate(lane_limit);
        let evidence = TraceFileEvidenceV1::from_go_test_failure_text(
            &request.failure_excerpt,
            request.failure_source_repo_id.clone(),
        );
        budget.checkpoint("go-test-trace-file:rank")?;
        let ranked = TraceFileRankPolicyV1::rank_go_test_failure(
            &fusion.pin,
            &fusion.lexical_rows,
            &fusion.dense_rows,
            &evidence,
            request.top_k,
        )?;
        let frame_class = match evidence.frame_class() {
            TraceSourceFrameClassV1::Unresolved => GoTestTraceFrameClassV1::Unresolved,
            TraceSourceFrameClassV1::Exact => GoTestTraceFrameClassV1::Exact,
            TraceSourceFrameClassV1::Ambiguous => GoTestTraceFrameClassV1::Ambiguous,
        };
        let response = GoTestTraceFileQueryResponseV1 {
            generation: fusion.pin,
            selected_active_head: selection.active_head,
            results: ranked
                .into_iter()
                .map(|hit| GoTestTraceFileHitV1 {
                    file: hit.key.file,
                    lexical: hit.lexical.map(trace_witness),
                    dense: hit.dense.map(trace_witness),
                    file_rrf_score: hit.file_rrf_score,
                    production_source: hit.production_source,
                    source_frame: hit.source_frame,
                })
                .collect(),
            lexical_window,
            dense_window,
            frame_class,
            explanation: fusion.explanation,
        };
        response
            .validate_against_v1(request)
            .map_err(CoreError::InvalidContract)?;
        Ok(response)
    }

    /// The hybrid route: two independent, bounded lanes fused by RRF
    /// (QI-BB-018).
    ///
    /// The lexical lane runs the lowered text query; the dense lane runs the
    /// embedded semantic query over the whole generation under the same
    /// pushed-down constraints. Their union is fused, so a document the
    /// lexical lane never saw can enter the top-k on dense relevance alone —
    /// this is hybrid recall, not a dense re-rank of lexical recall. Every
    /// fused row carries its RRF score and the rank and raw score each lane
    /// gave it (QI-BB-022), so a caller can see which lane put it there and
    /// an explain can reconcile it lane by lane.
    ///
    /// Every DSL filter binds both lanes (QI-BB-018 보완 #3): `lang:` is
    /// pushed down typed, the filters the contract classes as exact admit
    /// each dense candidate through the lexical plan, and a filter no lane
    /// can apply refuses the query typed before either lane runs. The
    /// window counts the bounded lanes' union: a dense lane the admission
    /// loop capped is named as such in the trace.
    fn execute_hybrid_fusion(
        &self,
        selection: &SemanticSelection,
        text_query: &TextQueryRequest,
        semantic_query_text: &str,
        top_k: u32,
        plane: &str,
        budget: &RequestBudgetV1,
    ) -> Result<HybridFusion, CoreError> {
        let pin = selection.pin.clone();
        let mut stage_timings = StageTimings::new(self.query_stage_observation, 7);
        let prepare_started = self.query_stage_observation.start();
        let (filter_plan, prepared_language) =
            self.plan_hybrid_lexical_query(text_query, budget)?;
        stage_timings.record_elapsed(QueryStageKindV1::HybridPrepare, prepare_started, 1, None);
        let view_started = self.query_stage_observation.start();
        let view = self.acquire_read_view(
            &ReadViewRequestV1::declare(
                plane,
                QueryRouteV1::Hybrid,
                Some(&prepared_language.query),
                &pin,
            )
            .with_semantic_manifest_digest(selection.expected_manifest_digest.as_deref()),
            budget,
        )?;
        stage_timings.record_elapsed(QueryStageKindV1::HybridReadView, view_started, 1, None);
        let lex_searcher = view.lexical()?;
        let sem_searcher = view.semantic()?;
        let internal_top_k = hybrid_probe_top_k_v1(top_k, self.hybrid_fetch_floor)?;
        // Invocation truth (W10-R1): only backend calls record. A
        // `force_empty` plan invokes nothing and records nothing.
        let execution = LaneExecutionRecorderV1::new();
        budget.checkpoint("hybrid:lexical")?;
        let lexical_started = self.query_stage_observation.start();
        let mut lex_results = if prepared_language.force_empty {
            Vec::new()
        } else {
            execution.record_lexical_invocation();
            lex_searcher
                .search_constrained(
                    &prepared_language.query,
                    &prepared_language.constraints,
                    &LexicalPageSpec::first(internal_top_k),
                    budget,
                )?
                .candidates
        };
        if !prepared_language.force_empty {
            stage_timings.record_elapsed(
                QueryStageKindV1::HybridLexicalSearch,
                lexical_started,
                1,
                Some(lex_results.len()),
            );
        }
        stabilize_ranked_candidates(&mut lex_results);
        budget.checkpoint("hybrid:embed")?;
        let embed_started = self.query_stage_observation.start();
        let query_vector =
            self.embed_and_gate_query(semantic_query_text, sem_searcher.as_ref(), plane, budget)?;
        stage_timings.record_elapsed(QueryStageKindV1::HybridEmbedding, embed_started, 1, None);
        // Independent dense lane under the same constraints and, per
        // candidate, the same exact filters; never scoped to the lexical
        // hits.
        let admission_started = self.query_stage_observation.start();
        let mut dense_fetch_duration = Duration::ZERO;
        let mut dense_fetch_calls = 0_u32;
        let mut dense_fetched_rows = 0_usize;
        let dense = admit_dense_lane_v1(
            &filter_plan,
            lex_searcher.as_ref(),
            &prepared_language.constraints,
            internal_top_k,
            budget,
            &execution,
            |candidate: &LexicalCandidate| candidate.candidate_id.as_str(),
            |fetch_size| {
                if prepared_language.force_empty {
                    return Ok(Vec::new());
                }
                budget.checkpoint("hybrid:semantic")?;
                execution.record_semantic_invocation();
                let fetch_started = self.query_stage_observation.start();
                let fetched = sem_searcher.search_constrained(
                    &query_vector,
                    &prepared_language.constraints,
                    fetch_size,
                    budget,
                )?;
                if let Some(fetch_started) = fetch_started {
                    dense_fetch_duration =
                        dense_fetch_duration.saturating_add(fetch_started.elapsed());
                    dense_fetch_calls = dense_fetch_calls.saturating_add(1);
                    dense_fetched_rows = dense_fetched_rows.saturating_add(fetched.len());
                }
                Ok(fetched)
            },
        )?;
        if dense_fetch_calls > 0 {
            // Refills can return the same row more than once. This count is
            // cumulative backend output, not unique admitted candidates.
            stage_timings.push(Some(measured(
                QueryStageKindV1::HybridDenseFetch,
                dense_fetch_duration,
                dense_fetch_calls,
                Some(dense_fetched_rows),
            )));
        }
        stage_timings.record_elapsed(
            QueryStageKindV1::HybridDenseAdmission,
            admission_started,
            1,
            Some(dense.rows.len()),
        );
        let filter_trace = HybridFilterTraceV1 {
            filters: format!("hybrid.filters={filter_plan}"),
            admission: vec![dense.trace_detail("hybrid.dense_admission")],
        };
        let dense_outcome = dense.outcome;
        let dense_examined = dense.examined;
        let dense_admitted = dense.admitted;
        let mut sem_results = dense.rows;
        budget.checkpoint("hybrid:fuse")?;
        let fuse_started = self.query_stage_observation.start();
        stabilize_ranked_candidates(&mut sem_results);
        let internal_limit = usize::try_from(internal_top_k).map_err(|err| {
            CoreError::InvalidContract(format!("hybrid: internal top_k overflow: {err}"))
        })?;
        // A lane at its full internal depth proves at least one row past
        // the page exists; a filtered dense lane reaches that depth only
        // when the admission loop filled it.
        let lane_limit_reached =
            lex_results.len() == internal_limit || sem_results.len() == internal_limit;
        let fused_universe_size = lex_results
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .chain(
                sem_results
                    .iter()
                    .map(|candidate| candidate.candidate_id.as_str()),
            )
            .collect::<BTreeSet<_>>()
            .len();
        let fused =
            HybridOrchestratorPolicy::fuse_rrf_candidates(&lex_results, &sem_results, top_k)?;
        stage_timings.record_elapsed(
            QueryStageKindV1::HybridFusion,
            fuse_started,
            1,
            Some(fused.len()),
        );
        let early_stop_reason = if fused_universe_size > fused.len() {
            Some(EarlyStopReason::CountReached)
        } else {
            None
        };
        // Contribution, same rule the builder used to apply inline: a lane
        // contributed iff it returned rows.
        if !lex_results.is_empty() {
            execution.record_lexical_contribution();
        }
        if !sem_results.is_empty() {
            execution.record_semantic_contribution();
        }
        let summary = execution.summary();
        let mut explanation = build_hybrid_response_explanation(
            &HybridLaneTallyV1 {
                lexical_hits: lex_results.len(),
                semantic_hits: sem_results.len(),
                fused_universe: fused_universe_size,
                fused_hits: fused.len(),
                execution: summary,
            },
            internal_top_k,
            early_stop_reason,
            &sem_searcher.dense_lane(),
            &filter_trace,
            budget.response_request_id(),
        );
        explanation.stage_timings = stage_timings.finish();
        attach_read_view_trace(&mut explanation, view.identity());
        let window_v2 = fused_window_v2(
            top_k,
            fused.len(),
            fused_universe_size,
            lane_limit_reached,
            Some(dense_outcome),
            internal_top_k,
            hybrid_lane_traces(
                &summary,
                lex_results.len(),
                sem_results.len(),
                dense_examined,
                dense_admitted,
            )?,
        )?;
        Ok(HybridFusion {
            pin,
            fused,
            lexical_rows: lex_results,
            dense_rows: sem_results,
            dense_outcome,
            dense_examined,
            internal_top_k,
            execution: summary,
            window_v2,
            explanation,
        })
    }

    fn hybrid(
        &self,
        request: &HybridQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<HybridQueryResponse, CoreError> {
        budget.checkpoint("hybrid:entry")?;
        HybridOrchestratorPolicy::validate_top_k(request.top_k)?;
        let selection =
            resolve_hybrid_request_selection(self.activation_catalog.as_ref(), request)?;
        let fusion = self.execute_hybrid_fusion(
            &selection,
            &request.text_query,
            request.semantic_query_text.as_str(),
            request.top_k,
            "hybrid",
            budget,
        )?;
        Ok(HybridQueryResponse {
            selected_active_head: selection.active_head,
            generation: fusion.pin,
            results: fusion.fused,
            window: fusion.window_v2,
            explanation: fusion.explanation,
        })
    }
}

fn trace_witness(witness: TraceFileLaneWitnessV1) -> GoTestTraceFileLaneWitnessV1 {
    GoTestTraceFileLaneWitnessV1 {
        file_rank: witness.file_rank,
        chunk_rank: witness.chunk_rank,
        candidate: witness.candidate,
    }
}

impl HybridQueryPort for SearchPlaneDispatcher {
    fn hybrid_query(
        &self,
        request: HybridQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<HybridQueryResponse, CoreError> {
        self.hybrid(&request, budget)
    }
}
