//! Semantic query route and the shared embed -> model-identity gate.

use std::collections::BTreeSet;

use quanta_index_contract::{
    EarlyStopReason, QueryConstraintSetV1, QueryStageKindV1, SemanticQueryRequest,
    SemanticQueryResponse,
};
use quanta_index_core::{
    CoreError, DenseIndexV1, DenseLaneAttestationV1, LexicalPageSpec, QueryRouteV1,
    RequestBudgetV1, SemanticPolicy, SemanticQueryPort, SemanticSearcher, validate_query_top_k,
};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::execution_trace::LaneExecutionRecorderV1;
use crate::query_dispatcher::planning::PreparedLanguageQueryV1;
use crate::query_dispatcher::ranking::stabilize_ranked_candidates;
use crate::query_dispatcher::read_view::{ReadViewRequestV1, attach_read_view_trace};
use crate::query_dispatcher::semantic_query::{
    SemanticScopeV1, build_semantic_response_explanation, ensure_query_model_matches_index_v1,
    prefix_semantic_query_error, resolve_semantic_request_selection,
};
use crate::query_dispatcher::stage_timing::StageTimings;
use crate::query_dispatcher::window::{
    probe_top_k_v1, semantic_empty_scope_window_v2, semantic_window_v2, top_k_limit,
};

/// The lexical scope of a semantic query, lowered before anything is
/// acquired: its candidate cap and its prepared plan.
struct SemanticScopePlan {
    cap: u32,
    prepared: PreparedLanguageQueryV1,
}

impl SearchPlaneDispatcher {
    /// Embed a semantic query string and gate it against the opened index's
    /// model identity. This is the single place the embed → model-identity-gate
    /// invariant lives for the semantic, hybrid, and hybrid-seed paths — a query
    /// vector from a model that differs from the indexed one is not
    /// cosine-comparable and must fail closed here.
    pub(super) fn embed_and_gate_query(
        &self,
        query_text: &str,
        sem_searcher: &dyn SemanticSearcher,
        plane: &str,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<f32>, CoreError> {
        let query_vector = self
            .query_embedder
            .embed_query(query_text, budget)
            .map_err(|err| prefix_semantic_query_error(plane, err))?;
        ensure_query_model_matches_index_v1(
            self.query_embedder.model_id(),
            self.query_embedder.model_revision(),
            sem_searcher.index_model_id(),
            sem_searcher.index_model_revision(),
            plane,
        )?;
        sem_searcher.validate_query_vector(&query_vector)?;
        Ok(query_vector)
    }

    pub(crate) fn semantic(
        &self,
        request: &SemanticQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SemanticQueryResponse, CoreError> {
        budget.checkpoint("semantic:entry")?;
        SemanticPolicy::validate_top_k(request.top_k)?;
        if budget.semantic_work_consumed_v1()?.is_some() {
            let query_bytes = u64::try_from(request.query_text.len()).map_err(|error| {
                CoreError::InvalidContract(format!("semantic query text length overflow: {error}"))
            })?;
            budget.consume_semantic_work_v1(query_bytes.checked_add(1).ok_or_else(|| {
                CoreError::InvalidContract("semantic query text work overflow".into())
            })?)?;
        }
        let selection =
            resolve_semantic_request_selection(self.activation_catalog.as_ref(), request)?;
        let pin = selection.pin.clone();
        let mut stage_timings = StageTimings::new(self.query_stage_observation, 6);
        let prepare_started = self.query_stage_observation.start();
        let scope_plan = match request.lexical_scope.as_ref() {
            Some(scope) => Some(plan_semantic_scope(self, request, scope, budget)?),
            None => None,
        };
        let effective_constraints: QueryConstraintSetV1 = scope_plan.as_ref().map_or_else(
            || request.constraints.clone(),
            |plan| plan.prepared.constraints.clone(),
        );
        stage_timings.record_elapsed(QueryStageKindV1::SemanticPrepare, prepare_started, 1, None);
        let view_started = self.query_stage_observation.start();
        let view = self.acquire_read_view(
            &ReadViewRequestV1::declare(
                "semantic",
                QueryRouteV1::Semantic,
                scope_plan.as_ref().map(|plan| &plan.prepared.query),
                &pin,
            )
            .with_semantic_manifest_digest(selection.expected_manifest_digest.as_deref()),
            budget,
        )?;
        stage_timings.record_elapsed(QueryStageKindV1::SemanticReadView, view_started, 1, None);
        // Invocation truth (W10-R1): only backend calls record. A
        // `force_empty` scope invokes nothing and records nothing.
        let execution = LaneExecutionRecorderV1::new();
        let scope = match scope_plan.as_ref() {
            Some(plan) => {
                // QI-BB-004: the scope's `top_k` is the lexical candidate cap
                // the contract promises. The lexical lane is asked for exactly
                // that many ranked candidates, and the semantic allowlist is
                // those ids and no more — never a full-recall materialization
                // of the scope query.
                let searcher = view.lexical()?;
                budget.checkpoint("semantic:scope")?;
                let scope_started = self.query_stage_observation.start();
                let mut scoped = if plan.prepared.force_empty {
                    Vec::new()
                } else {
                    execution.record_lexical_invocation();
                    searcher
                        .search_constrained(
                            &plan.prepared.query,
                            &plan.prepared.constraints,
                            &LexicalPageSpec::first(plan.cap),
                            budget,
                        )?
                        .candidates
                };
                if !plan.prepared.force_empty {
                    stage_timings.record_elapsed(
                        QueryStageKindV1::SemanticLexicalScope,
                        scope_started,
                        1,
                        Some(scoped.len()),
                    );
                }
                if scoped.len() > top_k_limit(plan.cap) {
                    return Err(CoreError::InvalidContract(format!(
                        "semantic: lexical scope adapter returned {} candidates for a cap of {}",
                        scoped.len(),
                        plan.cap
                    )));
                }
                stabilize_ranked_candidates(&mut scoped);
                Some(SemanticScopeV1 {
                    requested_cap: plan.cap,
                    candidate_ids: scoped
                        .into_iter()
                        .map(|candidate| candidate.candidate_id)
                        .collect::<BTreeSet<_>>(),
                })
            }
            None => None,
        };
        let scope_candidate_ids = scope.as_ref().map(|scope| &scope.candidate_ids);
        let searcher = view.semantic()?;
        budget.checkpoint("semantic:embed")?;
        let embed_started = self.query_stage_observation.start();
        let query_vector = self.embed_and_gate_query(
            request.query_text.as_str(),
            searcher.as_ref(),
            "semantic",
            budget,
        )?;
        stage_timings.record_elapsed(QueryStageKindV1::SemanticEmbedding, embed_started, 1, None);
        // Embedding still admits the semantic input and checks model identity.
        // An empty lexical allowlist never issues a dense backend call, whether
        // constraints or an executed lexical search proved it empty.
        if scope_candidate_ids.is_some_and(BTreeSet::is_empty) {
            budget.checkpoint("semantic:project")?;
            let project_started = self.query_stage_observation.start();
            let mut explanation = build_semantic_response_explanation(
                scope.as_ref(),
                0,
                None,
                &searcher.dense_lane(),
                &execution.summary(),
                budget.response_request_id(),
            );
            attach_read_view_trace(&mut explanation, view.identity());
            stage_timings.record_elapsed(
                QueryStageKindV1::SemanticProject,
                project_started,
                1,
                Some(0),
            );
            explanation.stage_timings = stage_timings.finish();
            return Ok(SemanticQueryResponse {
                generation: pin,
                results: Vec::new(),
                window: semantic_empty_scope_window_v2(&execution.summary())?,
                explanation,
            });
        }
        let probe_top_k = probe_top_k_v1(request.top_k)?;
        budget.checkpoint("semantic:search")?;
        let search_started = self.query_stage_observation.start();
        execution.record_semantic_invocation();
        let mut results = if budget.semantic_work_consumed_v1()?.is_some() {
            if scope_candidate_ids.is_some() {
                return Err(CoreError::InvalidContract(
                    "bounded semantic search does not admit lexical scope".into(),
                ));
            }
            searcher.search_work_bounded_v1(
                &query_vector,
                &effective_constraints,
                probe_top_k,
                budget,
            )?
        } else if let Some(scope_ids) = scope_candidate_ids {
            searcher.search_scoped_constrained(
                &query_vector,
                scope_ids,
                &effective_constraints,
                probe_top_k,
                budget,
            )?
        } else {
            searcher.search_constrained(
                &query_vector,
                &effective_constraints,
                probe_top_k,
                budget,
            )?
        };
        if let Some(scope_ids) = scope_candidate_ids
            && let Some(outside) = results
                .iter()
                .find(|candidate| !scope_ids.contains(&candidate.candidate_id))
        {
            return Err(CoreError::InvalidContract(format!(
                "semantic: scoped adapter returned candidate outside lexical allowlist: {}",
                outside.candidate_id
            )));
        }
        stage_timings.record_elapsed(
            QueryStageKindV1::SemanticDenseSearch,
            search_started,
            1,
            Some(results.len()),
        );
        if budget.semantic_work_consumed_v1()?.is_some() {
            budget.consume_semantic_work_v1(u64::try_from(results.len()).map_err(|error| {
                CoreError::InvalidContract(format!("semantic result count overflow: {error}"))
            })?)?;
        }
        budget.checkpoint("semantic:project")?;
        let project_started = self.query_stage_observation.start();
        let observed = results.len();
        results.truncate(top_k_limit(request.top_k));
        // A scope contributes to the response only when one of its
        // candidates survives the dense search. An executed scope with no
        // final rows is still recorded as executed, not contributed.
        if scope.is_some() && !results.is_empty() {
            execution.record_lexical_contribution();
        }
        if !results.is_empty() {
            execution.record_semantic_contribution();
        }
        let summary = execution.summary();
        let dense_lane = if budget.semantic_work_consumed_v1()?.is_some() {
            let mut lane = searcher.dense_lane();
            if !matches!(lane.index, DenseIndexV1::Exact) {
                lane.attestation = DenseLaneAttestationV1::ExactScanOfSealedRows;
            }
            lane.index = DenseIndexV1::Exact;
            lane
        } else {
            searcher.dense_lane()
        };
        let window_v2 = semantic_window_v2(
            request.top_k,
            observed,
            scope_candidate_ids.map(BTreeSet::len),
            &dense_lane,
            &summary,
        )?;
        let early_stop_reason = scope_candidate_ids.and_then(|scope_ids| {
            let limit = top_k_limit(request.top_k);
            if scope_ids.len() > results.len() && results.len() == limit {
                Some(EarlyStopReason::CountReached)
            } else {
                None
            }
        });
        let mut explanation = build_semantic_response_explanation(
            scope.as_ref(),
            results.len(),
            early_stop_reason,
            &dense_lane,
            &summary,
            budget.response_request_id(),
        );
        attach_read_view_trace(&mut explanation, view.identity());
        stage_timings.record_elapsed(
            QueryStageKindV1::SemanticProject,
            project_started,
            1,
            Some(results.len()),
        );
        explanation.stage_timings = stage_timings.finish();
        Ok(SemanticQueryResponse {
            generation: pin,
            results,
            window: window_v2,
            explanation,
        })
    }
}

/// Validate and lower the lexical scope of a semantic request: the cap
/// under the shared public gate, constraints equal to the outer request's,
/// and the plan the lexical lane will run.
fn plan_semantic_scope(
    dispatcher: &SearchPlaneDispatcher,
    request: &SemanticQueryRequest,
    scope: &quanta_index_contract::TextQueryRequest,
    budget: &RequestBudgetV1,
) -> Result<SemanticScopePlan, CoreError> {
    let cap = validate_query_top_k(scope.top_k)?;
    if scope.constraints != request.constraints {
        return Err(CoreError::InvalidContract(
            "semantic: lexical scope constraints must equal outer semantic constraints".to_string(),
        ));
    }
    let lowered_scope = lower_lexical_text_query(scope)?;
    let prepared =
        dispatcher.prepare_lexical_language_query(lowered_scope, &request.constraints, budget)?;
    Ok(SemanticScopePlan { cap, prepared })
}

impl SemanticQueryPort for SearchPlaneDispatcher {
    fn semantic_query(
        &self,
        request: SemanticQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SemanticQueryResponse, CoreError> {
        self.semantic(&request, budget)
    }
}
