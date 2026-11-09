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
    CoreError, DenseIndexV1, DenseLaneAttestationV1, DenseLaneContractV1, HybridOrchestratorPolicy,
    HybridQueryPort, LexicalPageSpec, QueryRouteV1, RequestBudgetV1, TraceFileEvidenceV1,
    TraceFileLaneWitnessV1, TraceFileRankPolicyV1, TraceSourceFrameClassV1,
};

use crate::query_dispatcher::dense_admission::admit_dense_lane_v1;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::execution_trace::{LaneExecutionRecorderV1, LaneExecutionSummaryV1};
use crate::query_dispatcher::ranking::stabilize_ranked_candidates;
use crate::query_dispatcher::read_view::{
    QueryReadViewV2, ReadViewRequestV1, attach_read_view_trace,
};
use crate::query_dispatcher::selection::SemanticSelection;
use crate::query_dispatcher::semantic_query::{
    GoTraceFileTallyV1, HybridFilterTraceV1, HybridFusion, HybridLaneTallyV1,
    build_go_trace_file_response_explanation, build_hybrid_response_explanation,
    resolve_hybrid_request_selection,
};
use crate::query_dispatcher::stage_timing::{StageTimings, measured};
use crate::query_dispatcher::window::{fused_window_v2, hybrid_probe_top_k_v1, lane_count_u64};

/// Independently fetched chunk lanes. Neither file ranking nor chunk fusion
/// has run, so each route can apply only its own ranking and window contract.
struct HybridLaneCandidates {
    pin: quanta_index_contract::GenerationPin,
    lexical_rows: Vec<LexicalCandidate>,
    dense_rows: Vec<LexicalCandidate>,
    dense_outcome: quanta_index_core::DenseAdmissionOutcomeV1,
    dense_examined: usize,
    dense_admitted: usize,
    internal_top_k: u32,
    execution: LaneExecutionSummaryV1,
    filter_trace: HybridFilterTraceV1,
    dense_lane: DenseLaneContractV1,
    view: QueryReadViewV2,
    stage_timings: StageTimings,
}

#[derive(Clone, Copy)]
enum HybridCandidateRoute<'a> {
    Chunk(&'a str),
    GoTraceFile,
}

fn order_go_file_lane(rows: &mut [LexicalCandidate]) {
    rows.sort_by(|left, right| left.order_key().order(&right.order_key()));
}

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
        let mut lanes = self.execute_hybrid_candidates(
            &selection,
            &request.query.text_query,
            &request.query.semantic_query_text,
            request.query.top_k,
            HybridCandidateRoute::GoTraceFile,
            budget,
        )?;
        let lane_top_k = request.query.top_k;
        let lane_limit = usize::try_from(lane_top_k).map_err(|error| {
            CoreError::InvalidContract(format!("trace lane limit exceeds usize: {error}"))
        })?;
        let internal_limit = usize::try_from(lanes.internal_top_k).map_err(|error| {
            CoreError::InvalidContract(format!("trace probe limit exceeds usize: {error}"))
        })?;
        let lexical_count = lanes.lexical_rows.len();
        let dense_count = lanes.dense_rows.len();
        let lexical_window = fused_window_v2(
            lane_top_k,
            lexical_count.min(lane_limit),
            lexical_count,
            lexical_count == internal_limit,
            None,
            lanes.internal_top_k,
            vec![
                LaneTraceV1::new(
                    "go_test_trace.lexical",
                    lanes.execution.lexical_executed(),
                    lanes.execution.lexical_contributed,
                )
                .with_candidates(CandidateCountV1::AtLeast(lane_count_u64(lexical_count)?)),
            ],
        )?;
        let dense_window = fused_window_v2(
            lane_top_k,
            dense_count.min(lane_limit),
            dense_count,
            dense_count == internal_limit,
            Some(lanes.dense_outcome),
            lanes.internal_top_k,
            vec![
                LaneTraceV1::new(
                    "go_test_trace.dense",
                    lanes.execution.semantic_executed(),
                    lanes.execution.semantic_contributed,
                )
                .with_candidates(CandidateCountV1::AtLeast(lane_count_u64(dense_count)?))
                .with_filtered_out(lane_count_u64(
                    lanes.dense_examined.saturating_sub(lanes.dense_admitted),
                )?)
                .with_cost(lane_count_u64(lanes.dense_examined)?),
            ],
        )?;
        lanes.lexical_rows.truncate(lane_limit);
        lanes.dense_rows.truncate(lane_limit);
        let evidence = match (
            request.failure_source_repo_id.clone(),
            request.failure_module_context.as_ref(),
        ) {
            (Some(owner), Some(module)) => TraceFileEvidenceV1::from_go_test_module_cache_panic(
                &request.failure_excerpt,
                owner,
                module,
            ),
            (owner, None) => {
                TraceFileEvidenceV1::from_go_test_failure_text(&request.failure_excerpt, owner)
            }
            (None, Some(_)) => {
                return Err(CoreError::InvalidContract(
                    "Go module context requires failure-source owner".to_string(),
                ));
            }
        };
        budget.checkpoint("go-test-trace-file:rank")?;
        let ranked = TraceFileRankPolicyV1::rank_go_test_failure(
            &lanes.pin,
            &lanes.lexical_rows,
            &lanes.dense_rows,
            &evidence,
            request.top_k,
        )?;
        let mut explanation = build_go_trace_file_response_explanation(
            &GoTraceFileTallyV1 {
                lexical_hits: lexical_count,
                semantic_hits: dense_count,
                file_hits: ranked.len(),
                execution: lanes.execution,
            },
            lanes.internal_top_k,
            &lanes.dense_lane,
            &lanes.filter_trace,
            budget.response_request_id(),
        );
        // The current timing enum names chunk fusion, not file ranking.
        // Retain only the shared stages actually executed by this route.
        explanation.stage_timings = lanes.stage_timings.finish();
        attach_read_view_trace(&mut explanation, lanes.view.identity());
        let frame_class = match evidence.frame_class() {
            TraceSourceFrameClassV1::Unresolved => GoTestTraceFrameClassV1::Unresolved,
            TraceSourceFrameClassV1::Exact => GoTestTraceFrameClassV1::Exact,
            TraceSourceFrameClassV1::Ambiguous => GoTestTraceFrameClassV1::Ambiguous,
        };
        let response = GoTestTraceFileQueryResponseV1 {
            generation: lanes.pin,
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
            explanation,
        };
        response
            .validate_against_v1(request)
            .map_err(CoreError::InvalidContract)?;
        Ok(response)
    }

    /// Fetch both bounded chunk lanes with the same pinned view, exact DSL
    /// filters, dense admission, budget, and execution observations.
    fn execute_hybrid_candidates(
        &self,
        selection: &SemanticSelection,
        text_query: &TextQueryRequest,
        semantic_query_text: &str,
        top_k: u32,
        route: HybridCandidateRoute<'_>,
        budget: &RequestBudgetV1,
    ) -> Result<HybridLaneCandidates, CoreError> {
        let plane = match route {
            HybridCandidateRoute::Chunk(plane) => plane,
            HybridCandidateRoute::GoTraceFile => "go test trace file",
        };
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
        match route {
            HybridCandidateRoute::Chunk(_) => stabilize_ranked_candidates(&mut lex_results),
            HybridCandidateRoute::GoTraceFile => order_go_file_lane(&mut lex_results),
        }
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
        // Exact file ranking must see canonical ties before Top-K. Under an
        // exact lexical admission filter, keep one bounded ranked prefix for
        // the whole request instead of scanning the generation at every
        // widening admission round.
        let go_exact_admission = matches!(route, HybridCandidateRoute::GoTraceFile)
            && filter_plan.admission_query().is_some();
        let mut go_ranked_candidates: Option<Vec<LexicalCandidate>> = None;
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
                let mut fetch_started = None;
                let fetched = match route {
                    HybridCandidateRoute::Chunk(_) => {
                        execution.record_semantic_invocation();
                        fetch_started = self.query_stage_observation.start();
                        sem_searcher.search_constrained(
                            &query_vector,
                            &prepared_language.constraints,
                            fetch_size,
                            budget,
                        )?
                    }
                    HybridCandidateRoute::GoTraceFile => {
                        if !go_exact_admission {
                            execution.record_semantic_invocation();
                            fetch_started = self.query_stage_observation.start();
                            sem_searcher.search_constrained_canonical_ties(
                                &query_vector,
                                &prepared_language.constraints,
                                fetch_size,
                                budget,
                            )?
                        } else {
                            if go_ranked_candidates.is_none() {
                                execution.record_semantic_invocation();
                                fetch_started = self.query_stage_observation.start();
                                go_ranked_candidates =
                                    Some(sem_searcher.search_constrained_canonical_ties(
                                        &query_vector,
                                        &prepared_language.constraints,
                                        HybridOrchestratorPolicy::dense_admission_examine_ceiling(),
                                        budget,
                                    )?);
                            }
                            let ranked = go_ranked_candidates.as_ref().ok_or_else(|| {
                                CoreError::InvalidContract(
                                    "Go canonical candidates were not fetched".into(),
                                )
                            })?;
                            let depth = usize::try_from(fetch_size).map_err(|error| {
                                CoreError::InvalidContract(format!(
                                    "Go canonical fetch depth exceeds usize: {error}"
                                ))
                            })?;
                            ranked[..ranked.len().min(depth)].to_vec()
                        }
                    }
                };
                if let Some(fetch_started) = fetch_started {
                    dense_fetch_duration =
                        dense_fetch_duration.saturating_add(fetch_started.elapsed());
                    dense_fetch_calls = dense_fetch_calls.saturating_add(1);
                    dense_fetched_rows = dense_fetched_rows.saturating_add(
                        go_ranked_candidates
                            .as_ref()
                            .map_or(fetched.len(), Vec::len),
                    );
                }
                Ok(fetched)
            },
        )?;
        if dense_fetch_calls > 0 {
            // Count backend output, including repeat reads by ordinary Hybrid
            // refills. Go exact admission reads its ranked prefix once.
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
        let sem_results = dense.rows;
        // Contribution, same rule the builder uses: a lane contributed iff
        // it returned rows, independent of the route's later ranking unit.
        if !lex_results.is_empty() {
            execution.record_lexical_contribution();
        }
        if !sem_results.is_empty() {
            execution.record_semantic_contribution();
        }
        let execution = execution.summary();
        let mut dense_lane = sem_searcher.dense_lane();
        if matches!(route, HybridCandidateRoute::GoTraceFile)
            && execution.semantic_executed()
            && matches!(dense_lane.index, DenseIndexV1::Approximate { .. })
        {
            dense_lane.index = DenseIndexV1::Exact;
            dense_lane.attestation = DenseLaneAttestationV1::ExactScanOfSealedRows;
        }
        Ok(HybridLaneCandidates {
            pin,
            lexical_rows: lex_results,
            dense_rows: sem_results,
            dense_outcome,
            dense_examined,
            dense_admitted,
            internal_top_k,
            execution,
            filter_trace,
            dense_lane,
            view,
            stage_timings,
        })
    }

    /// The ordinary hybrid route: two independent, bounded lanes fused by
    /// chunk RRF (QI-BB-018).
    ///
    /// The lexical lane runs the lowered text query; the dense lane embeds
    /// the semantic query over the whole generation under the same pushed-down
    /// constraints. Their union allows dense-only hits. Each fused row carries
    /// its RRF score and per-lane rank and raw score (QI-BB-022).
    ///
    /// Every DSL filter binds both lanes: `lang:` is pushed down typed, exact
    /// filters admit each dense candidate through the lexical plan, and an
    /// unsupported filter refuses the query before either lane runs. The
    /// window counts the bounded lanes' union and names capped dense admission.
    fn execute_hybrid_fusion(
        &self,
        selection: &SemanticSelection,
        text_query: &TextQueryRequest,
        semantic_query_text: &str,
        top_k: u32,
        plane: &str,
        budget: &RequestBudgetV1,
    ) -> Result<HybridFusion, CoreError> {
        let mut lanes = self.execute_hybrid_candidates(
            selection,
            text_query,
            semantic_query_text,
            top_k,
            HybridCandidateRoute::Chunk(plane),
            budget,
        )?;
        budget.checkpoint("hybrid:fuse")?;
        let fuse_started = self.query_stage_observation.start();
        stabilize_ranked_candidates(&mut lanes.dense_rows);
        let internal_limit = usize::try_from(lanes.internal_top_k).map_err(|err| {
            CoreError::InvalidContract(format!("hybrid: internal top_k overflow: {err}"))
        })?;
        // A lane at its full internal depth proves at least one row past
        // the page exists; a filtered dense lane reaches that depth only
        // when the admission loop filled it.
        let lane_limit_reached =
            lanes.lexical_rows.len() == internal_limit || lanes.dense_rows.len() == internal_limit;
        let fused_universe_size = lanes
            .lexical_rows
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .chain(
                lanes
                    .dense_rows
                    .iter()
                    .map(|candidate| candidate.candidate_id.as_str()),
            )
            .collect::<BTreeSet<_>>()
            .len();
        let fused = HybridOrchestratorPolicy::fuse_rrf_candidates(
            &lanes.lexical_rows,
            &lanes.dense_rows,
            top_k,
        )?;
        lanes.stage_timings.record_elapsed(
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
        let mut explanation = build_hybrid_response_explanation(
            &HybridLaneTallyV1 {
                lexical_hits: lanes.lexical_rows.len(),
                semantic_hits: lanes.dense_rows.len(),
                fused_universe: fused_universe_size,
                fused_hits: fused.len(),
                execution: lanes.execution,
            },
            lanes.internal_top_k,
            early_stop_reason,
            &lanes.dense_lane,
            &lanes.filter_trace,
            budget.response_request_id(),
        );
        explanation.stage_timings = lanes.stage_timings.finish();
        attach_read_view_trace(&mut explanation, lanes.view.identity());
        let window_v2 = fused_window_v2(
            top_k,
            fused.len(),
            fused_universe_size,
            lane_limit_reached,
            Some(lanes.dense_outcome),
            lanes.internal_top_k,
            hybrid_lane_traces(
                &lanes.execution,
                lanes.lexical_rows.len(),
                lanes.dense_rows.len(),
                lanes.dense_examined,
                lanes.dense_admitted,
            )?,
        )?;
        Ok(HybridFusion {
            pin: lanes.pin,
            fused,
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
