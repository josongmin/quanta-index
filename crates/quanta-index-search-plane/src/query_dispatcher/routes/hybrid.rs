//! Hybrid (lexical + semantic RRF) query route and the shared fusion core.

use std::collections::BTreeSet;

use quanta_index_contract::{
    EarlyStopReason, HybridQueryRequest, HybridQueryResponse, LexicalCandidate, TextQueryRequest,
};
use quanta_index_core::{
    CoreError, HybridFilterPlanV1, HybridOrchestratorPolicy, HybridQueryPort, LexicalPolicy,
    QueryRouteV1, RequestBudgetV1,
};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::dense_admission::admit_dense_lane_v1;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::planning::prepare_language_query_v1;
use crate::query_dispatcher::ranking::stabilize_ranked_candidates;
use crate::query_dispatcher::read_view::{ReadViewRequestV1, attach_read_view_trace};
use crate::query_dispatcher::selection::SemanticSelection;
use crate::query_dispatcher::semantic_query::{
    HybridFilterTraceV1, HybridFusion, HybridLaneTallyV1, build_hybrid_response_explanation,
    resolve_hybrid_request_selection,
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
        let lexical_query = lower_lexical_text_query(text_query)?;
        let filter_plan = HybridFilterPlanV1::plan(&lexical_query)?;
        let prepared_language = prepare_language_query_v1(lexical_query, &text_query.constraints)?;
        LexicalPolicy::validate_query(&prepared_language.query)?;
        let view = self.acquire_read_view(
            &ReadViewRequestV1::declare(
                plane,
                QueryRouteV1::Hybrid,
                Some(&prepared_language.query),
                &pin,
            )
            .with_semantic_manifest_digest(selection.expected_manifest_digest.as_deref()),
        )?;
        let lex_searcher = view.lexical()?;
        let sem_searcher = view.semantic()?;
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
        // Independent dense lane under the same constraints and, per
        // candidate, the same exact filters; never scoped to the lexical
        // hits.
        let dense = admit_dense_lane_v1(
            &filter_plan,
            lex_searcher.as_ref(),
            &prepared_language.constraints,
            internal_top_k,
            budget,
            |candidate: &LexicalCandidate| candidate.candidate_id.as_str(),
            |fetch_size| {
                if prepared_language.force_empty {
                    return Ok(Vec::new());
                }
                budget.checkpoint("hybrid:semantic")?;
                sem_searcher.search_constrained(
                    &query_vector,
                    &prepared_language.constraints,
                    fetch_size,
                    budget,
                )
            },
        )?;
        let filter_trace = HybridFilterTraceV1 {
            filters: format!("hybrid.filters={filter_plan}"),
            admission: vec![dense.trace_detail("hybrid.dense_admission")],
        };
        let mut sem_results = dense.rows;
        budget.checkpoint("hybrid:fuse")?;
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
        let early_stop_reason = if fused_universe_size > fused.len() {
            Some(EarlyStopReason::CountReached)
        } else {
            None
        };
        let mut explanation = build_hybrid_response_explanation(
            &HybridLaneTallyV1 {
                lexical_hits: lex_results.len(),
                semantic_hits: sem_results.len(),
                fused_universe: fused_universe_size,
                fused_hits: fused.len(),
            },
            internal_top_k,
            early_stop_reason,
            &sem_searcher.dense_lane(),
            &filter_trace,
        );
        attach_read_view_trace(&mut explanation, view.identity());
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
