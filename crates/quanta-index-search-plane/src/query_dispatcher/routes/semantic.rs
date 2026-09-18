//! Semantic query route and the shared embed -> model-identity gate.

use std::collections::BTreeSet;

use quanta_index_contract::{EarlyStopReason, SemanticQueryRequest, SemanticQueryResponse};
use quanta_index_core::{
    CoreError, LexicalPolicy, RequestBudgetV1, SemanticPolicy, SemanticQueryPort, SemanticSearcher,
    validate_query_top_k,
};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::planning::prepare_language_query_v1;
use crate::query_dispatcher::ranking::stabilize_ranked_candidates;
use crate::query_dispatcher::semantic_query::{
    SemanticScopeV1, build_semantic_response_explanation, ensure_query_model_matches_index_v1,
    prefix_semantic_query_error, resolve_semantic_request_selection,
};
use crate::query_dispatcher::window::{finalize_probe_window_v1, probe_top_k_v1, top_k_limit};

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
    ) -> Result<Vec<f32>, CoreError> {
        let query_vector = self
            .query_embedder
            .embed_query(query_text)
            .map_err(|err| prefix_semantic_query_error(plane, err))?;
        ensure_query_model_matches_index_v1(
            self.query_embedder.model_id(),
            self.query_embedder.model_revision(),
            sem_searcher.index_model_id(),
            sem_searcher.index_model_revision(),
            plane,
        )?;
        Ok(query_vector)
    }

    pub(crate) fn semantic(
        &self,
        request: &SemanticQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SemanticQueryResponse, CoreError> {
        budget.checkpoint("semantic:entry")?;
        SemanticPolicy::validate_top_k(request.top_k)?;
        let selection =
            resolve_semantic_request_selection(self.activation_catalog.as_ref(), request)?;
        let pin = selection.pin.clone();
        self.validate_semantic_selection(&selection, "semantic")?;
        let mut effective_constraints = request.constraints.clone();
        let scope = if let Some(scope) = request.lexical_scope.as_ref() {
            // QI-BB-004: the scope's `top_k` is the lexical candidate cap the
            // contract promises. It is validated under the shared public
            // gate, the lexical lane is asked for exactly that many ranked
            // candidates, and the semantic allowlist is those ids and no
            // more — never a full-recall materialization of the scope query.
            let scope_cap = validate_query_top_k(scope.top_k)?;
            if scope.constraints != request.constraints {
                return Err(CoreError::InvalidContract(
                    "semantic: lexical scope constraints must equal outer semantic constraints"
                        .to_string(),
                ));
            }
            let lowered_scope = lower_lexical_text_query(scope)?;
            let prepared_language = prepare_language_query_v1(lowered_scope, &request.constraints)?;
            LexicalPolicy::validate_query(&prepared_language.query)?;
            effective_constraints = prepared_language.constraints.clone();
            let lex_materialized =
                self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
            LexicalPolicy::validate_query_against_readiness(
                pin.manifest_generation,
                lex_materialized,
            )?;
            let searcher =
                self.acquire_lexical(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
            budget.checkpoint("semantic:scope")?;
            let mut scoped = if prepared_language.force_empty {
                Vec::new()
            } else {
                searcher
                    .search_constrained(
                        &prepared_language.query,
                        &prepared_language.constraints,
                        scope_cap,
                        budget,
                    )?
                    .candidates
            };
            if scoped.len() > top_k_limit(scope_cap) {
                return Err(CoreError::InvalidContract(format!(
                    "semantic: lexical scope adapter returned {} candidates for a cap of {scope_cap}",
                    scoped.len()
                )));
            }
            stabilize_ranked_candidates(&mut scoped);
            Some(SemanticScopeV1 {
                requested_cap: scope_cap,
                candidate_ids: scoped
                    .into_iter()
                    .map(|candidate| candidate.candidate_id)
                    .collect::<BTreeSet<_>>(),
            })
        } else {
            None
        };
        let scope_candidate_ids = scope.as_ref().map(|scope| &scope.candidate_ids);
        let searcher =
            self.acquire_semantic(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        budget.checkpoint("semantic:embed")?;
        let query_vector =
            self.embed_and_gate_query(request.query_text.as_str(), searcher.as_ref(), "semantic")?;
        let probe_top_k = probe_top_k_v1(request.top_k)?;
        budget.checkpoint("semantic:search")?;
        let mut results = if let Some(scope_ids) = scope_candidate_ids {
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
        budget.checkpoint("semantic:project")?;
        let window = finalize_probe_window_v1(&mut results, request.top_k)?;
        let early_stop_reason = scope_candidate_ids.and_then(|scope_ids| {
            let limit = top_k_limit(request.top_k);
            if scope_ids.len() > results.len() && results.len() == limit {
                Some(EarlyStopReason::CountReached)
            } else {
                None
            }
        });
        let explanation = build_semantic_response_explanation(
            scope.as_ref(),
            results.len(),
            early_stop_reason,
            &searcher.dense_lane(),
        );
        Ok(SemanticQueryResponse {
            generation: pin,
            results,
            window,
            explanation,
        })
    }
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
