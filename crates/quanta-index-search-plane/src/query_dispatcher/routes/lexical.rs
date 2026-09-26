//! Lexical text and symbol query routes.

use std::time::Instant;

use quanta_index_contract::{
    CursorRouteV2, EngineTouched, GenerationPin, LexicalCursor, LexicalRowOrderKey,
    QueryResultWindowV1, QueryStageKindV1, QueryStageTimingV1, SearchExplanation,
    SearchPlaneTrackKind, SymbolQueryRequest, SymbolQueryResponse, TextQueryRequest,
    TextQueryResponse, validate_lexical_page_v1,
};
use quanta_index_core::{
    CoreError, LexicalPageSpec, LexicalPolicy, LexicalQueryPort, QueryRouteV1, RequestBudgetV1,
    validate_query_top_k,
};

use crate::lower_lexical_text_query;
use crate::query_dispatcher::continuation::{CursorRequestContextV2, require_token_pin};
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::execution_trace::{LaneExecutionRecorderV1, LaneExecutionSummaryV1};
use crate::query_dispatcher::planning::{
    prepare_language_query_v1, query_selects_file_owner_projection,
};
use crate::query_dispatcher::read_view::ReadViewRequestV1;
use crate::query_dispatcher::response_budget::fit_ranked_page;
use crate::query_dispatcher::selection::resolve_optional_selection;
use crate::query_dispatcher::stage_timing::elapsed;
use crate::query_dispatcher::window::{
    finalize_probe_window_v1, lexical_fetch_limit_v1, lexical_page_window_v1, pageable_window_v2,
    probe_top_k_v1,
};

const LEXICAL_CURSOR_ORDER_V2: &str = "score_desc_path_line_candidate_v1";

fn lexical_explanation(
    budget: &RequestBudgetV1,
    execution: &LaneExecutionSummaryV1,
    stage_timings: Vec<QueryStageTimingV1>,
) -> SearchExplanation {
    let mut explanation = SearchExplanation::empty();
    explanation.request_id = budget.response_request_id();
    explanation.engines_executed = execution.executed_engines();
    explanation.engines_touched = execution.touched_engines();
    explanation.strategy = "lexical".to_string();
    explanation.stage_timings = Some(stage_timings);
    explanation
}

impl SearchPlaneDispatcher {
    /// Lower the request, acquire the view its plan declares, and forward
    /// it to the pinned lexical searcher.
    fn lexical(
        &self,
        request: &TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<TextQueryResponse, CoreError> {
        self.lexical_with_execution(request, budget)
            .map(|(response, _)| response)
    }

    pub(in crate::query_dispatcher) fn lexical_with_execution(
        &self,
        request: &TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<(TextQueryResponse, LaneExecutionSummaryV1), CoreError> {
        let execution = LaneExecutionRecorderV1::new();
        budget.checkpoint("lexical:entry")?;
        let prepare_started = Instant::now();
        let _accepted_top_k = validate_query_top_k(request.top_k)?;
        let opened = request
            .cursor
            .as_ref()
            .map(|token| self.cursors()?.open::<LexicalCursor>(token))
            .transpose()?;
        if let Some(opened) = &opened {
            require_token_pin(
                request.generation.as_ref(),
                request.generation_selector.as_ref(),
                &opened.binding().pin,
            )?;
        }
        // The cursor positions the page; the plan is the query's alone.
        let pageless = TextQueryRequest {
            generation: opened
                .as_ref()
                .map(|cursor| cursor.binding().pin.clone())
                .or_else(|| request.generation.clone()),
            generation_selector: opened
                .as_ref()
                .map_or_else(|| request.generation_selector.clone(), |_| None),
            cursor: None,
            ..request.clone()
        };
        let planned = self.plan_lexical_text_query(&pageless, QueryRouteV1::Lexical, budget)?;
        let cursor_context = CursorRequestContextV2 {
            route: CursorRouteV2::Lexical,
            pin: &planned.pin,
            query: &planned.query,
            constraints: &planned.constraints,
            order: LEXICAL_CURSOR_ORDER_V2,
            cap: request.top_k,
        };
        if let Some(opened) = &opened {
            self.cursors()?
                .require_context(opened, &cursor_context, Vec::new())?;
        }
        let mut stage_timings = vec![elapsed(
            QueryStageKindV1::LexicalPrepare,
            prepare_started,
            1,
            None,
        )];
        let wants_file_owner_projection = query_selects_file_owner_projection(&planned.query);
        if planned.force_empty {
            let project_started = Instant::now();
            let window = pageable_window_v2(QueryResultWindowV1::exact(0), "lexical")?;
            stage_timings.push(elapsed(
                QueryStageKindV1::LexicalProject,
                project_started,
                1,
                Some(0),
            ));
            let summary = execution.summary();
            return Ok((
                TextQueryResponse {
                    generation: planned.pin.clone(),
                    results: Vec::new(),
                    window,
                    explanation: lexical_explanation(budget, &summary, stage_timings),
                    file_owner_rows: wants_file_owner_projection.then(Vec::new),
                    next_cursor: None,
                },
                summary,
            ));
        }
        let view_started = Instant::now();
        let view = self.acquire_read_view(
            &ReadViewRequestV1::new("lexical", &planned.pin, planned.domains),
            budget,
        )?;
        stage_timings.push(elapsed(
            QueryStageKindV1::LexicalReadView,
            view_started,
            1,
            None,
        ));
        let searcher = view.lexical()?;
        let fetch_top_k = lexical_fetch_limit_v1(&planned.query, request.top_k)?;
        budget.checkpoint("lexical:search")?;
        execution.record_lexical_invocation();
        let search_started = Instant::now();
        let mut page = searcher.search_constrained(
            &planned.query,
            &planned.constraints,
            &LexicalPageSpec {
                fetch: fetch_top_k,
                after: continuation(opened.as_ref().map(|cursor| &cursor.boundary), &planned.pin)?,
            },
            budget,
        )?;
        stage_timings.push(elapsed(
            QueryStageKindV1::LexicalSearch,
            search_started,
            1,
            Some(page.candidates.len()),
        ));
        budget.checkpoint("lexical:project")?;
        let project_started = Instant::now();
        let window = lexical_page_window_v1(&mut page, request.top_k, fetch_top_k)?;
        let results = page.candidates;
        let next_boundary = next_cursor(
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
        let public_window = pageable_window_v2(window, "lexical")?;
        let next_cursor = next_boundary
            .as_ref()
            .map(|boundary| self.cursors()?.mint(boundary, &cursor_context, Vec::new()))
            .transpose()?;
        stage_timings.push(elapsed(
            QueryStageKindV1::LexicalProject,
            project_started,
            1,
            Some(results.len()),
        ));
        let mut explanation = lexical_explanation(budget, &execution.summary(), stage_timings);
        if !results.is_empty() {
            // Reserve the maximal explanation shape before response-budget
            // fitting. A truncated page can only remove this contribution.
            explanation.engines_touched.push(EngineTouched::Lexical);
        }
        let mut response = fit_ranked_page(
            TextQueryResponse {
                generation: planned.pin.clone(),
                results,
                window: public_window,
                explanation,
                file_owner_rows,
                next_cursor,
            },
            self.response_budget,
            |boundary| self.cursors()?.mint(boundary, &cursor_context, Vec::new()),
        )?;
        if !response.results.is_empty() {
            execution.record_lexical_contribution();
        }
        let summary = execution.summary();
        response.explanation.engines_touched = summary.touched_engines();
        let final_stage = response
            .explanation
            .stage_timings
            .as_mut()
            .and_then(|stages| stages.last_mut())
            .ok_or_else(|| CoreError::InvalidContract("lexical project stage missing".to_string()))?;
        final_stage.returned_candidates = Some(
            u64::try_from(response.results.len()).map_or(u64::MAX, |count| count),
        );
        Ok((response, summary))
    }

    pub(in crate::query_dispatcher) fn symbol_with_execution(
        &self,
        request: SymbolQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<(SymbolQueryResponse, LaneExecutionSummaryV1), CoreError> {
        let execution = LaneExecutionRecorderV1::new();
        budget.checkpoint("symbol:entry")?;
        let _accepted_top_k = validate_query_top_k(request.top_k)?;
        let opened = request
            .cursor
            .as_ref()
            .map(|token| self.cursors()?.open::<LexicalCursor>(token))
            .transpose()?;
        let pin = if let Some(opened) = &opened {
            require_token_pin(
                request.generation.as_ref(),
                request.generation_selector.as_ref(),
                &opened.binding().pin,
            )?;
            opened.binding().pin.clone()
        } else {
            resolve_optional_selection(
                self.activation_catalog.as_ref(),
                request.generation.clone(),
                request.generation_selector.as_ref(),
                SearchPlaneTrackKind::Lexical,
                "symbol",
            )?
            .ok_or_else(|| {
                CoreError::InvalidContract("symbol: generation selector required".to_string())
            })?
        };
        let after = continuation(opened.as_ref().map(|cursor| &cursor.boundary), &pin)?;
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
        let cursor_context = CursorRequestContextV2 {
            route: CursorRouteV2::Symbol,
            pin: &pin,
            query: &prepared_language.query,
            constraints: &prepared_language.constraints,
            order: LEXICAL_CURSOR_ORDER_V2,
            cap: lexical_request.top_k,
        };
        if let Some(opened) = &opened {
            self.cursors()?
                .require_context(opened, &cursor_context, Vec::new())?;
        }
        if prepared_language.force_empty {
            return Ok((
                SymbolQueryResponse {
                    generation: pin.clone(),
                    results: Vec::new(),
                    window: pageable_window_v2(QueryResultWindowV1::exact(0), "symbol")?,
                    next_cursor: None,
                },
                execution.summary(),
            ));
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
        execution.record_lexical_invocation();
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
        let next_boundary = next_cursor(
            &window,
            &pin,
            results
                .iter()
                .map(quanta_index_contract::SymbolCandidate::order_key),
        )?;
        let public_window = pageable_window_v2(window, "symbol")?;
        let next_cursor = next_boundary
            .as_ref()
            .map(|boundary| self.cursors()?.mint(boundary, &cursor_context, Vec::new()))
            .transpose()?;
        let response = fit_ranked_page(
            SymbolQueryResponse {
                generation: pin.clone(),
                results,
                window: public_window,
                next_cursor,
            },
            self.response_budget,
            |boundary| self.cursors()?.mint(boundary, &cursor_context, Vec::new()),
        )?;
        if !response.results.is_empty() {
            execution.record_lexical_contribution();
        }
        Ok((response, execution.summary()))
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
            code: quanta_index_contract::SearchPlaneErrorCodeV2::QueryCursorGenerationMismatch,
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
