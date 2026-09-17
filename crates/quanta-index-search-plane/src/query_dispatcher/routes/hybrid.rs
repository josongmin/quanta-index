//! Hybrid (lexical + semantic RRF) query route and the shared fusion core.

use std::collections::BTreeSet;

use quanta_index_contract::{
    EarlyStopReason, HybridQueryRequest, HybridQueryResponse, TextQueryRequest,
};
use quanta_index_core::{
    CoreError, HybridOrchestratorPolicy, HybridQueryPort, LexicalPolicy, RequestBudgetV1,
};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::planning::prepare_language_query_v1;
use crate::query_dispatcher::ranking::stabilize_ranked_candidates;
use crate::query_dispatcher::selection::SemanticSelection;
use crate::query_dispatcher::semantic_query::{
    HybridFusion, build_hybrid_response_explanation, resolve_hybrid_request_selection,
};
use crate::query_dispatcher::window::{fused_window_v1, hybrid_probe_top_k_v1};

impl SearchPlaneDispatcher {
    /// The hybrid route: two independent, bounded lanes fused by RRF
    /// (QI-BB-018).
    ///
    /// The lexical lane runs the lowered text query; the dense lane runs the
    /// embedded semantic query over the whole generation under the same
    /// pushed-down constraints. Their union is fused, so a document the
    /// lexical lane never saw can enter the top-k on dense relevance alone —
    /// this is hybrid recall, not a dense re-rank of lexical recall.
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
        let lex_materialized = self.snapshot_lex_materialized(&pin.repo_id, &pin.revision_id)?;
        LexicalPolicy::validate_query_against_readiness(pin.manifest_generation, lex_materialized)?;
        self.validate_semantic_selection(selection, plane)?;

        let lex_searcher =
            self.acquire_lexical(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let sem_searcher =
            self.acquire_semantic(&pin.repo_id, &pin.revision_id, pin.manifest_generation)?;
        let lexical_query = lower_lexical_text_query(text_query)?;
        let prepared_language = prepare_language_query_v1(lexical_query, &text_query.constraints)?;
        LexicalPolicy::validate_query(&prepared_language.query)?;
        let internal_top_k = hybrid_probe_top_k_v1(top_k)?;
        budget.checkpoint("hybrid:lexical")?;
        let mut lex_results = if prepared_language.force_empty {
            Vec::new()
        } else {
            lex_searcher
                .search_constrained(
                    &prepared_language.query,
                    &prepared_language.constraints,
                    internal_top_k,
                    budget,
                )?
                .candidates
        };
        stabilize_ranked_candidates(&mut lex_results);
        budget.checkpoint("hybrid:embed")?;
        let query_vector =
            self.embed_and_gate_query(semantic_query_text, sem_searcher.as_ref(), plane)?;
        budget.checkpoint("hybrid:semantic")?;
        // Independent dense lane under the same constraints, never scoped to
        // the lexical hits.
        let mut sem_results = if prepared_language.force_empty {
            Vec::new()
        } else {
            sem_searcher.search_constrained(
                &query_vector,
                &prepared_language.constraints,
                internal_top_k,
            )?
        };
        budget.checkpoint("hybrid:fuse")?;
        stabilize_ranked_candidates(&mut sem_results);
        let internal_limit = usize::try_from(internal_top_k).map_err(|err| {
            CoreError::InvalidContract(format!("hybrid: internal top_k overflow: {err}"))
        })?;
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
        let fused = HybridOrchestratorPolicy::fuse_rrf(&lex_results, &sem_results, top_k);
        let early_stop_reason = if fused_universe_size > fused.len() {
            Some(EarlyStopReason::CountReached)
        } else {
            None
        };
        let explanation = build_hybrid_response_explanation(
            lex_results.len(),
            sem_results.len(),
            fused_universe_size,
            fused.len(),
            internal_top_k,
            early_stop_reason,
            &sem_searcher.dense_lane(),
        );
        let window = fused_window_v1(top_k, fused.len(), fused_universe_size, lane_limit_reached)?;
        Ok(HybridFusion {
            pin,
            fused,
            window,
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
            generation: fusion.pin,
            results: fusion.fused,
            window: fusion.window,
            explanation: fusion.explanation,
        })
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
