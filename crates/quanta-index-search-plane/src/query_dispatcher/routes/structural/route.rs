//! Structural query route entry: lowering, seeding, evaluation, projection.

use std::sync::Arc;

use quanta_index_contract::{
    GenerationPin, LqQuery, QueryResultWindowV1, SearchPlaneStructuralQueryResponse,
    StructuralQueryRequest,
};
use quanta_index_core::{CoreError, RequestBudgetV1, StructuralService, validate_query_top_k};

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::errors::structural_invalid_request;
use crate::query_dispatcher::routes::structural::eval::{
    StructuralEvalContext, evaluate_structural_expr, extract_structural_requested_lang,
    structural_expr_has_non_structural_leaf, structural_expr_has_structural_leaf,
    structural_expr_is_pure_negative_root,
};
use crate::query_dispatcher::routes::structural::lexical_leaves::LexicalSubexprEvaluator;
use crate::query_dispatcher::routes::structural::lowering::{
    extract_structural_filters, lower_structural_query_request,
};
use crate::query_dispatcher::routes::structural::projection::project_structural_query_results;
use crate::query_dispatcher::routes::structural::universe::build_pinned_structural_universe;
use crate::query_dispatcher::window::{exact_total_window_v1, top_k_limit};

impl SearchPlaneDispatcher {
    pub(crate) fn structural(
        &self,
        request: &StructuralQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneStructuralQueryResponse, CoreError> {
        budget.checkpoint("structural:entry")?;
        let _accepted_top_k = validate_query_top_k(request.text_query.top_k)?;
        let (pin, lowered) =
            lower_structural_query_request(self.activation_catalog.as_ref(), request)?;
        let (results, window) =
            self.execute_structural_results(&pin, &lowered, request.text_query.top_k, budget)?;
        Ok(SearchPlaneStructuralQueryResponse {
            generation: pin,
            results,
            window,
        })
    }

    /// Evaluate the structural expression and cut the page.
    ///
    /// Evaluation materializes the whole match set, so the window carries
    /// its exact size (QI-BB-025) and `has_more` says the page cut it.
    fn execute_structural_results(
        &self,
        pin: &GenerationPin,
        lowered: &LqQuery,
        top_k: u32,
        budget: &RequestBudgetV1,
    ) -> Result<
        (
            Vec<quanta_index_contract::StructuralCandidate>,
            QueryResultWindowV1,
        ),
        CoreError,
    > {
        if !structural_expr_has_structural_leaf(&lowered.expr) {
            return Err(structural_invalid_request(
                "query must include at least one structural `match { ... }` leaf",
            ));
        }
        let has_lexical = structural_expr_has_non_structural_leaf(&lowered.expr);
        let requested_lang = extract_structural_requested_lang(&lowered.expr, has_lexical)?;
        let (requested_lang, executable_filters) =
            extract_structural_filters(lowered, requested_lang.as_deref())?;
        let seed = if structural_expr_is_pure_negative_root(&lowered.expr) {
            let structural_state = self
                .ledger
                .read()
                .map_err(|_poisoned| {
                    CoreError::Storage("search-plane ledger poisoned".to_string())
                })?
                .structural_snapshot(&pin.repo_id, &pin.revision_id, pin.manifest_generation)
                .ok_or_else(|| {
                    CoreError::NotReady(format!(
                        "structural: generation {} chunk authority is not materialized",
                        pin.manifest_generation.get()
                    ))
                })?;
            Some(build_pinned_structural_universe(
                pin,
                &structural_state,
                requested_lang.as_deref(),
                &executable_filters,
            )?)
        } else {
            None
        };
        let service = StructuralService::new(Arc::clone(&self.structural_producer));
        let mut ctx = StructuralEvalContext::default();
        let lexical_eval = if has_lexical {
            Some(LexicalSubexprEvaluator {
                dispatcher: self,
                pin,
                query: lowered,
                budget,
            })
        } else {
            None
        };
        budget.checkpoint("structural:execute")?;
        let candidates = evaluate_structural_expr(
            &mut ctx,
            &service,
            pin,
            &lowered.expr,
            requested_lang.as_deref(),
            &executable_filters,
            &lowered.options,
            seed.as_ref(),
            lexical_eval.as_ref(),
        )?;
        let mut results = project_structural_query_results(candidates);
        let total = u64::try_from(results.len()).map_err(|err| {
            CoreError::InvalidContract(format!("structural match set overflow: {err}"))
        })?;
        results.truncate(top_k_limit(top_k));
        let window = exact_total_window_v1(results.len(), total)?;
        Ok((results, window))
    }
}
