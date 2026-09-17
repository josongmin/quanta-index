//! Structural query route entry: lowering, pinning the read, seeding,
//! evaluation, page selection, projection.
//!
//! A page is cut from one epoch-named structural snapshot (QI-BB-020 W2):
//! a fresh walk pins the current epoch, a continuation the epoch its
//! cursor names, and every leaf of the query — the pinned universe, each
//! parse-tree match the producer executes, each symbol projection — reads
//! that one snapshot. A cursor whose epoch is no longer retained is
//! refused typed; it is never served from a newer snapshot.
//!
//! Evaluation materializes the whole match set on purpose: a boolean tree
//! of `match { ... }` leaves is decided by set algebra over every leaf's
//! matches, so no leaf can be cut at the page. The page is then selected
//! after the cursor with the bounded keyset collector (QI-BB-025 W4), so
//! the window's count is exact and only the page's rows are projected.

use std::sync::Arc;

use quanta_index_contract::{
    AuxEpochV1, LqQuery, QueryResultWindowV1, SearchPlaneStructuralQueryResponse,
    StructuralCandidate, StructuralCursorV1, StructuralQueryRequest,
};
use quanta_index_core::{CoreError, RequestBudgetV1, StructuralService, validate_query_top_k};

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::errors::structural_invalid_request;
use crate::query_dispatcher::keyset_page::{KeysetPageCollector, StreamEnd};
use crate::query_dispatcher::routes::structural::buckets::StructuralCandidateBuckets;
use crate::query_dispatcher::routes::structural::eval::{
    StructuralEvalContext, evaluate_structural_expr, extract_structural_requested_lang,
    structural_expr_has_non_structural_leaf, structural_expr_has_structural_leaf,
    structural_expr_is_pure_negative_root,
};
use crate::query_dispatcher::routes::structural::lexical_leaves::LexicalSubexprEvaluator;
use crate::query_dispatcher::routes::structural::lowering::{
    extract_structural_filters, lower_structural_query_request,
};
use crate::query_dispatcher::routes::structural::projection::project_structural_page;
use crate::query_dispatcher::routes::structural::read::StructuralRead;
use crate::query_dispatcher::routes::structural::universe::build_pinned_structural_universe;

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
        // The whole query — the pinned universe, every parse-tree leaf the
        // producer executes, every symbol projection — reads one structural
        // snapshot, taken here (QI-BB-020 W2): the cursor's epoch for a
        // continuation, the current one for a fresh walk.
        let structural =
            self.structural_read(&pin, request.cursor.as_ref().map(|cursor| cursor.aux_epoch))?;
        let read = StructuralRead {
            pin: &pin,
            epoch: structural.epoch,
            state: structural.state.as_ref(),
        };
        let page = self.execute_structural_page(
            read,
            &lowered,
            request.text_query.top_k,
            request.cursor.as_ref(),
            budget,
        )?;
        Ok(SearchPlaneStructuralQueryResponse {
            generation: pin,
            results: page.results,
            window: page.window,
            read_epoch: structural.epoch,
            examined: page.examined,
            next_cursor: page.next_cursor,
        })
    }

    /// Evaluate the structural expression against the pinned read and
    /// select the page after `cursor`.
    fn execute_structural_page(
        &self,
        read: StructuralRead<'_>,
        lowered: &LqQuery,
        top_k: u32,
        cursor: Option<&StructuralCursorV1>,
        budget: &RequestBudgetV1,
    ) -> Result<StructuralPage, CoreError> {
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
            Some(build_pinned_structural_universe(
                read.pin,
                read.state,
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
                read,
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
            read,
            &lowered.expr,
            requested_lang.as_deref(),
            &executable_filters,
            &lowered.options,
            seed.as_ref(),
            lexical_eval.as_ref(),
        )?;
        select_structural_page(candidates, read.epoch, top_k, cursor)
    }
}

/// One page of structural results in candidate-id order.
#[derive(Debug)]
struct StructuralPage {
    results: Vec<StructuralCandidate>,
    window: QueryResultWindowV1,
    examined: u64,
    next_cursor: Option<StructuralCursorV1>,
}

/// Cut the page of `top_k` matched candidates after `cursor` from the
/// evaluated match set (QI-BB-025 W4).
///
/// Every matched candidate is walked so the window's count is exact; only
/// the page's buckets are projected into rows, so the response is
/// proportional to the page. `epoch` is the snapshot the match set was
/// evaluated against; the page's continuation names it.
fn select_structural_page(
    mut candidates: StructuralCandidateBuckets,
    epoch: AuxEpochV1,
    top_k: u32,
    cursor: Option<&StructuralCursorV1>,
) -> Result<StructuralPage, CoreError> {
    let mut collector =
        KeysetPageCollector::new(top_k, cursor.map(|cursor| cursor.candidate_id.clone()))?;
    for candidate_id in candidates.keys() {
        collector.examined_one();
        collector.offer(candidate_id);
    }
    let page = collector.finish(StreamEnd::Exhausted)?;
    let results = project_structural_page(&mut candidates, &page.keys)?;
    let next_cursor = page.next_key.map(|candidate_id| StructuralCursorV1 {
        candidate_id,
        aux_epoch: epoch,
    });
    Ok(StructuralPage {
        results,
        window: page.window,
        examined: page.examined,
        next_cursor,
    })
}
