//! Lexical text and symbol query routes.

use quanta_index_contract::{
    GenerationPin, LexicalCursor, LexicalRowOrderKey, QUERY_CURSOR_GENERATION_MISMATCH_CODE,
    QueryResultWindowV1, SearchPlaneTrackKind, SymbolQueryRequest, SymbolQueryResponse,
    TextQueryRequest, TextQueryResponse, validate_lexical_page_v1,
};
use quanta_index_core::{
    CoreError, LexicalPageSpec, LexicalPolicy, LexicalQueryPort, QueryRouteV1, RequestBudgetV1,
    validate_query_top_k,
};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::planning::{
    prepare_language_query_v1, query_selects_file_owner_projection,
};
use crate::query_dispatcher::read_view::ReadViewRequestV1;
use crate::query_dispatcher::response_budget::fit_ranked_page;
use crate::query_dispatcher::selection::resolve_optional_selection;
use crate::query_dispatcher::window::{
    finalize_probe_window_v1, lexical_fetch_limit_v1, lexical_page_window_v1, probe_top_k_v1,
};

impl SearchPlaneDispatcher {
    /// Lower the request, acquire the view its plan declares, and forward
    /// it to the pinned lexical searcher.
    fn lexical(
        &self,
        request: &TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<TextQueryResponse, CoreError> {
        budget.checkpoint("lexical:entry")?;
        let _accepted_top_k = validate_query_top_k(request.top_k)?;
        // The cursor positions the page; the plan is the query's alone.
        let pageless = TextQueryRequest {
            cursor: None,
            ..request.clone()
        };
        let planned = self.plan_lexical_text_query(&pageless, QueryRouteV1::Lexical, budget)?;
        let wants_file_owner_projection = query_selects_file_owner_projection(&planned.query);
        if planned.force_empty {
            return Ok(TextQueryResponse {
                generation: planned.pin,
                results: Vec::new(),
                window: QueryResultWindowV1::exact(0),
                file_owner_rows: wants_file_owner_projection.then(Vec::new),
                next_cursor: None,
            });
        }
        let view = self.acquire_read_view(
            &ReadViewRequestV1::new("lexical", &planned.pin, planned.domains),
            budget,
        )?;
        let searcher = view.lexical()?;
        let fetch_top_k = lexical_fetch_limit_v1(&planned.query, request.top_k)?;
        budget.checkpoint("lexical:search")?;
        let mut page = searcher.search_constrained(
            &planned.query,
            &planned.constraints,
            &LexicalPageSpec {
                fetch: fetch_top_k,
                after: continuation(request.cursor.as_ref(), &planned.pin)?,
            },
            budget,
        )?;
        budget.checkpoint("lexical:project")?;
        let window = lexical_page_window_v1(&mut page, request.top_k, fetch_top_k)?;
        let results = page.candidates;
        let next_cursor = next_cursor(
            &window,
            &planned.pin,
            results
                .iter()
                .map(quanta_index_contract::LexicalCandidate::order_key),
        )?;
        let file_owner_rows = if wants_file_owner_projection {
            Some(searcher.project_file_owners(&results)?)
        } else {
            None
        };
        fit_ranked_page(
            TextQueryResponse {
                generation: planned.pin,
                results,
                window,
                file_owner_rows,
                next_cursor,
            },
            self.response_budget,
        )
    }

    pub(crate) fn symbol(
        &self,
        request: SymbolQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SymbolQueryResponse, CoreError> {
        budget.checkpoint("symbol:entry")?;
        let _accepted_top_k = validate_query_top_k(request.top_k)?;
        let pin = resolve_optional_selection(
            self.activation_catalog.as_ref(),
            request.generation.clone(),
            request.generation_selector.as_ref(),
            SearchPlaneTrackKind::Lexical,
            "symbol",
        )?
        .ok_or_else(|| {
            CoreError::InvalidContract("symbol: generation selector required".to_string())
        })?;
        let after = continuation(request.cursor.as_ref(), &pin)?;
        let lexical_request = TextQueryRequest {
            generation: Some(pin.clone()),
            generation_selector: None,
            cursor: None,
            ..TextQueryRequest::from(request)
        };
        let lowered = lower_lexical_text_query(&lexical_request)?;
        let prepared_language = prepare_language_query_v1(lowered, &lexical_request.constraints)?;
        LexicalPolicy::validate_query_with_constraints(
            &prepared_language.query,
            &prepared_language.constraints,
        )?;
        if prepared_language.force_empty {
            return Ok(SymbolQueryResponse {
                generation: pin,
                results: Vec::new(),
                window: QueryResultWindowV1::exact(0),
                next_cursor: None,
            });
        }
        let view = self.acquire_read_view(
            &ReadViewRequestV1::declare(
                "symbol",
                QueryRouteV1::Symbol,
                Some(&prepared_language.query),
                &pin,
            ),
            budget,
        )?;
        let searcher = view.lexical()?;
        // The symbol port has no count collector yet, so a `count` option
        // still yields a probe-derived (at-least) window here.
        budget.checkpoint("symbol:search")?;
        let mut results = searcher.search_symbols_constrained(
            &prepared_language.query,
            &prepared_language.constraints,
            &LexicalPageSpec {
                fetch: probe_top_k_v1(lexical_request.top_k)?,
                after,
            },
            budget,
        )?;
        let window = finalize_probe_window_v1(&mut results, lexical_request.top_k)?;
        let next_cursor = next_cursor(
            &window,
            &pin,
            results
                .iter()
                .map(quanta_index_contract::SymbolCandidate::order_key),
        )?;
        fit_ranked_page(
            SymbolQueryResponse {
                generation: pin,
                results,
                window,
                next_cursor,
            },
            self.response_budget,
        )
    }
}

/// The request's cursor as the page boundary, refused typed when it was cut
/// from another generation than the one the request resolved to: a score
/// only compares within the ranking that made it.
fn continuation(
    cursor: Option<&LexicalCursor>,
    pin: &GenerationPin,
) -> Result<Option<LexicalCursor>, CoreError> {
    match cursor {
        None => Ok(None),
        Some(cursor) if cursor.manifest_generation == pin.manifest_generation => {
            Ok(Some(cursor.clone()))
        }
        Some(cursor) => Err(CoreError::Typed {
            code: QUERY_CURSOR_GENERATION_MISMATCH_CODE.to_string(),
            message: format!(
                "the cursor was cut from generation {} but the request resolves to generation {}; pin the page's generation to continue it",
                cursor.manifest_generation.get(),
                pin.manifest_generation.get()
            ),
        }),
    }
}

/// The cursor a page continues from, after checking the adapter answered
/// in page order: the last row when more exist, none when the page is the
/// end.
fn next_cursor<'a>(
    window: &QueryResultWindowV1,
    pin: &GenerationPin,
    rows: impl Iterator<Item = LexicalRowOrderKey<'a>> + Clone,
) -> Result<Option<LexicalCursor>, CoreError> {
    let cursor = window
        .has_more()
        .then(|| rows.clone().last())
        .flatten()
        .map(|last| LexicalCursor::at(pin.manifest_generation, last));
    validate_lexical_page_v1(window, rows, pin.manifest_generation, cursor.as_ref()).map_err(
        |defect| CoreError::InvalidContract(format!("lexical page from the adapter: {defect}")),
    )?;
    Ok(cursor)
}

impl LexicalQueryPort for SearchPlaneDispatcher {
    fn lexical_query(
        &self,
        request: TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<TextQueryResponse, CoreError> {
        self.lexical(&request, budget)
    }
}
