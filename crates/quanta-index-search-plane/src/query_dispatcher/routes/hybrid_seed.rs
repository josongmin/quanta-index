//! Hybrid-seed query route: multi-corpus dense lanes fused with lexical recall.

use std::collections::BTreeSet;

use quanta_index_contract::{
    CandidateCountV1, EarlyStopReason, HybridSeedQueryRequest, HybridSeedQueryResponse, LaneTraceV1,
};
use quanta_index_core::{
    CoreError, DenseAdmissionOutcomeV1, HybridFilterPlanV1, HybridOrchestratorPolicy,
    LexicalPageSpec, LexicalPolicy, QueryRouteV1, RequestBudgetV1, SemanticSearchHitV1,
};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::dense_admission::admit_dense_lane_v1;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::planning::prepare_language_query_v1;
use crate::query_dispatcher::ranking::{
    stabilize_ranked_candidates, stabilize_semantic_seed_hits_v1,
};
use crate::query_dispatcher::read_view::{ReadViewRequestV1, attach_read_view_trace};
use crate::query_dispatcher::semantic_query::{
    HybridFilterTraceV1, SeedLaneTallyV1, build_hybrid_seed_candidates,
    build_hybrid_seed_response_explanation, canonical_dense_corpus_budgets_v1,
    resolve_hybrid_seed_request_selection,
};
use crate::query_dispatcher::window::{
    fused_window_v1, fused_window_v2, hybrid_probe_top_k_v1, lane_count_u64,
};

impl SearchPlaneDispatcher {
    /// The hybrid-seed route: one lexical lane and one dense lane per
    /// requested corpus (or one global dense lane), fused on entity.
    ///
    /// Every DSL filter binds every lane (QI-BB-018 보완 #3) under the same
    /// contract as the hybrid route: `lang:` pushed down typed, exact
    /// filters admitting each dense hit through the lexical plan, anything
    /// else refused typed before any lane runs. Each dense lane's admission
    /// outcome is its own trace entry.
    pub(crate) fn hybrid_seed(
        &self,
        request: &HybridSeedQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<HybridSeedQueryResponse, CoreError> {
        budget.checkpoint("hybrid-seed:entry")?;
        HybridOrchestratorPolicy::validate_top_k(request.top_k)?;
        let selection =
            resolve_hybrid_seed_request_selection(self.activation_catalog.as_ref(), request)?;
        let pin = selection.pin.clone();
        let lexical_query = lower_lexical_text_query(&request.text_query)?;
        let filter_plan = HybridFilterPlanV1::plan(&lexical_query)?;
        let prepared_language =
            prepare_language_query_v1(lexical_query, &request.text_query.constraints)?;
        LexicalPolicy::validate_query(&prepared_language.query)?;
        let view = self.acquire_read_view(
            &ReadViewRequestV1::declare(
                "hybrid seed",
                QueryRouteV1::HybridSeed,
                Some(&prepared_language.query),
                &pin,
            )
            .with_semantic_manifest_digest(selection.expected_manifest_digest.as_deref()),
            budget,
        )?;
        let manifest_digest = view.semantic_manifest_digest()?.to_string();
        let lex_searcher = view.lexical()?;
        let sem_searcher = view.semantic()?;
        let internal_top_k = hybrid_probe_top_k_v1(request.top_k)?;
        budget.checkpoint("hybrid-seed:lexical")?;
        let mut lex_results = if prepared_language.force_empty {
            Vec::new()
        } else {
            lex_searcher
                .search_constrained(
                    &prepared_language.query,
                    &prepared_language.constraints,
                    &LexicalPageSpec::first(internal_top_k),
                    budget,
                )?
                .candidates
        };
        stabilize_ranked_candidates(&mut lex_results);
        budget.checkpoint("hybrid-seed:embed")?;
        let query_vector = self.embed_and_gate_query(
            request.semantic_query_text.as_str(),
            sem_searcher.as_ref(),
            "hybrid seed",
            budget,
        )?;
        let internal_limit = usize::try_from(internal_top_k).map_err(|err| {
            CoreError::InvalidContract(format!("hybrid seed: internal top_k overflow: {err}"))
        })?;
        let primary_lane_limit_reached = lex_results.len() == internal_limit;
        // One dense lane per requested corpus (or one global lane), each a
        // single native search over the query vector per admission round
        // (QI-BB-019): there is no second, lexical-scoped dense search
        // behind the seed list.
        let dense_corpora = canonical_dense_corpus_budgets_v1(&request.dense_corpora)?;
        let mut unavailable_corpus_reasons = Vec::new();
        let mut semantic_lanes = Vec::new();
        let mut admission_traces = Vec::new();
        let mut dense_outcomes: Vec<DenseAdmissionOutcomeV1> = Vec::new();
        if prepared_language.force_empty {
            semantic_lanes.push(Vec::new());
        } else if dense_corpora.is_empty() {
            let lane = admit_dense_lane_v1(
                &filter_plan,
                lex_searcher.as_ref(),
                &prepared_language.constraints,
                internal_top_k,
                budget,
                |hit: &SemanticSearchHitV1| hit.candidate.candidate_id.as_str(),
                |fetch_size| {
                    budget.checkpoint("hybrid-seed:dense")?;
                    sem_searcher.search_hits_constrained(
                        &query_vector,
                        &prepared_language.constraints,
                        fetch_size,
                        budget,
                    )
                },
            )?;
            admission_traces.push(lane.trace_detail("hybrid_seed.dense_admission[global]"));
            dense_outcomes.push(lane.outcome);
            let mut hits = lane.rows;
            stabilize_semantic_seed_hits_v1(&mut hits);
            semantic_lanes.push(hits);
        } else {
            for corpus_budget in dense_corpora {
                let corpus_kind = corpus_budget.corpus_kind;
                let lane = admit_dense_lane_v1(
                    &filter_plan,
                    lex_searcher.as_ref(),
                    &prepared_language.constraints,
                    corpus_budget.top_k,
                    budget,
                    |hit: &SemanticSearchHitV1| hit.candidate.candidate_id.as_str(),
                    |fetch_size| {
                        // Each native call gets its own checkpoint.
                        budget.checkpoint("hybrid-seed:dense")?;
                        sem_searcher.search_hits_for_corpus_constrained(
                            &query_vector,
                            corpus_kind,
                            &prepared_language.constraints,
                            fetch_size,
                            budget,
                        )
                    },
                )?;
                admission_traces.push(lane.trace_detail(&format!(
                    "hybrid_seed.dense_admission[{}]",
                    corpus_kind.as_code_str()
                )));
                dense_outcomes.push(lane.outcome);
                // A corpus is unavailable when its lane returned no row to
                // examine, not when the filters admitted none.
                if lane.examined == 0 {
                    unavailable_corpus_reasons.push(format!(
                        "requested_semantic_corpus_unavailable:{}",
                        corpus_kind.as_code_str()
                    ));
                }
                let mut hits = lane.rows;
                stabilize_semantic_seed_hits_v1(&mut hits);
                semantic_lanes.push(hits);
            }
        }
        budget.checkpoint("hybrid-seed:fuse")?;
        let semantic_hits = semantic_lanes.iter().flatten().collect::<Vec<_>>();
        let lexical_entity_count = lex_results
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        let semantic_entity_count = semantic_hits
            .iter()
            .map(|hit| hit.owner_id.as_str())
            .collect::<BTreeSet<_>>()
            .len();
        let fused_entity_universe = lex_results
            .iter()
            .map(|candidate| candidate.candidate_id.as_str())
            .chain(semantic_hits.iter().map(|hit| hit.owner_id.as_str()))
            .collect::<BTreeSet<_>>()
            .len();
        let seed_candidates = build_hybrid_seed_candidates(
            &lex_results,
            &semantic_lanes,
            &unavailable_corpus_reasons,
            request.top_k,
        )?;
        let early_stop_reason = if fused_entity_universe > seed_candidates.len() {
            Some(EarlyStopReason::CountReached)
        } else {
            None
        };
        let mut explanation = build_hybrid_seed_response_explanation(
            &SeedLaneTallyV1 {
                lexical_hits: lex_results.len(),
                lexical_entities: lexical_entity_count,
                semantic_hits: semantic_hits.len(),
                semantic_entities: semantic_entity_count,
                fused_hits: seed_candidates.len(),
            },
            internal_top_k,
            &unavailable_corpus_reasons,
            early_stop_reason,
            &sem_searcher.dense_lane(),
            &HybridFilterTraceV1 {
                filters: format!("hybrid_seed.filters={filter_plan}"),
                admission: admission_traces,
            },
        );
        attach_read_view_trace(&mut explanation, view.identity());
        let window = fused_window_v1(
            request.top_k,
            seed_candidates.len(),
            fused_entity_universe,
            primary_lane_limit_reached,
        )?;
        // Aggregate dense admission: one capped lane caps the whole
        // window; a filled lane proves a continuation; exhausted lanes
        // prove only their own universe.
        let aggregate_outcome = if dense_outcomes.contains(&DenseAdmissionOutcomeV1::Capped) {
            Some(DenseAdmissionOutcomeV1::Capped)
        } else if dense_outcomes.contains(&DenseAdmissionOutcomeV1::Filled) {
            Some(DenseAdmissionOutcomeV1::Filled)
        } else if dense_outcomes.contains(&DenseAdmissionOutcomeV1::Exhausted) {
            Some(DenseAdmissionOutcomeV1::Exhausted)
        } else {
            None
        };
        let seed_lane_traces = vec![
            LaneTraceV1::new("hybrid_seed.lexical", true, !lex_results.is_empty()).with_candidates(
                CandidateCountV1::AtLeast(lane_count_u64(lexical_entity_count)?),
            ),
            LaneTraceV1::new("hybrid_seed.dense", true, semantic_entity_count > 0).with_candidates(
                CandidateCountV1::AtLeast(lane_count_u64(semantic_entity_count)?),
            ),
        ];
        let window_v2 = fused_window_v2(
            request.top_k,
            seed_candidates.len(),
            fused_entity_universe,
            primary_lane_limit_reached,
            aggregate_outcome,
            internal_top_k,
            seed_lane_traces,
        )?;
        Ok(HybridSeedQueryResponse {
            generation: pin,
            manifest_digest,
            seed_candidates,
            window,
            window_v2,
            explanation,
        })
    }
}
