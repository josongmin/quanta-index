//! Lexical text and symbol query routes.

use quanta_index_contract::{
    QueryResultWindowV1, SearchPlaneTrackKind, SymbolQueryRequest, SymbolQueryResponse,
    TextQueryRequest, TextQueryResponse,
};
use quanta_index_core::{
    CoreError, LexicalPolicy, LexicalQueryPort, QueryRouteV1, RequestBudgetV1, validate_query_top_k,
};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::planning::{
    prepare_language_query_v1, query_selects_file_owner_projection,
};
use crate::query_dispatcher::ranking::stabilize_ranked_candidates;
use crate::query_dispatcher::read_view::ReadViewRequestV1;
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
        let planned = self.plan_lexical_text_query(request, QueryRouteV1::Lexical)?;
        let wants_file_owner_projection = query_selects_file_owner_projection(&planned.query);
        if planned.force_empty {
            return Ok(TextQueryResponse {
                generation: planned.pin,
                results: Vec::new(),
                window: QueryResultWindowV1::exact(0),
                file_owner_rows: wants_file_owner_projection.then(Vec::new),
            });
        }
        let view = self.acquire_read_view(&ReadViewRequestV1::new(
            "lexical",
            &planned.pin,
            planned.domains,
        ))?;
        let searcher = view.lexical()?;
        let fetch_top_k = lexical_fetch_limit_v1(&planned.query, request.top_k)?;
        budget.checkpoint("lexical:search")?;
        let mut page = searcher.search_constrained(
            &planned.query,
            &planned.constraints,
            fetch_top_k,
            budget,
        )?;
        budget.checkpoint("lexical:project")?;
        stabilize_ranked_candidates(&mut page.candidates);
        let window = lexical_page_window_v1(&mut page, request.top_k, fetch_top_k)?;
        let results = page.candidates;
        let file_owner_rows = if wants_file_owner_projection {
            Some(searcher.project_file_owners(&results)?)
        } else {
            None
        };
        Ok(TextQueryResponse {
            generation: planned.pin,
            results,
            window,
            file_owner_rows,
        })
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
        let lexical_request = TextQueryRequest {
            syntax: request.syntax,
            query_text: request.query_text,
            constraints: request.constraints,
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k: request.top_k,
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
            });
        }
        let view = self.acquire_read_view(&ReadViewRequestV1::declare(
            "symbol",
            QueryRouteV1::Symbol,
            Some(&prepared_language.query),
            &pin,
        ))?;
        let searcher = view.lexical()?;
        // The symbol port has no count collector yet, so a `count` option
        // still yields a probe-derived (at-least) window here.
        budget.checkpoint("symbol:search")?;
        let mut results = searcher.search_symbols_constrained(
            &prepared_language.query,
            &prepared_language.constraints,
            probe_top_k_v1(request.top_k)?,
            budget,
        )?;
        let window = finalize_probe_window_v1(&mut results, request.top_k)?;
        Ok(SymbolQueryResponse {
            generation: pin,
            results,
            window,
        })
    }
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
