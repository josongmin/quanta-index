//! Lexical text and symbol query routes.

use quanta_index_contract::{
    CODE_SEARCH_IDENTIFIER_TYPO_PREDICATE, CODE_SEARCH_SYMBOL_COMPONENTS_PREDICATE, CursorRouteV2,
    EngineTouched, GenerationPin, LexicalCursor, LexicalRowOrderKey, LqExpr, LqLeaf, LqPatternType,
    LqQuery, PlannerStage, PlannerTraceEntry, QueryResultWindowV1, QueryResultWindowV2,
    QueryStageKindV1, QueryStageTimingV1, SearchExplanation, SearchPlaneTrackKind,
    SymbolQueryRequest, SymbolQueryResponse, TextQueryRequest, TextQueryResponse,
    validate_lexical_page_v1,
};
use quanta_index_core::{
    CodeSearchExecutionStatsV1, CoreError, LexicalEndpoint, LexicalPageSpec, LexicalPolicy,
    LexicalQueryPort, QueryRouteV1, RequestBudgetV1, validate_query_top_k,
};

use crate::lower_lexical_text_query;
use crate::lowering::reject_code_search_on_nonlexical_route;
use crate::query_dispatcher::continuation::{
    CursorRequestContextV2, require_cursor_on_nonempty_plan, require_token_pin,
};
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::execution_trace::{LaneExecutionRecorderV1, LaneExecutionSummaryV1};
use crate::query_dispatcher::planning::{
    prepare_language_query_v1, query_selects_file_owner_projection, text_rank_unit,
};
use crate::query_dispatcher::read_view::ReadViewRequestV1;
use crate::query_dispatcher::response_budget::fit_ranked_page;
use crate::query_dispatcher::selection::resolve_optional_selection;
use crate::query_dispatcher::stage_timing::StageTimings;
use crate::query_dispatcher::window::{
    lexical_fetch_limit_v1, lexical_page_window_v1, pageable_window_v2,
};

const LEXICAL_CURSOR_ORDER_V2: &str = "score_desc_source_repo_path_line_candidate_v2";
// The signed cursor context must change when CodeSearch scoring changes,
// even if the sealed generation and query text remain identical.
pub(super) const CODE_SEARCH_CURSOR_ORDER: &str =
    "code_search_file_overlap_score_v1_desc_source_repo_path_line_candidate";
const CODE_SEARCH_TYPO_CURSOR_ORDER: &str =
    "code_search_identifier_typo_osa1_v1_desc_source_repo_path_line_candidate";
const CODE_SEARCH_COMPONENT_CURSOR_ORDER: &str =
    "code_search_symbol_components_v1_desc_source_repo_path_line_candidate";

pub(super) fn code_search_rank_order(query: &LqQuery) -> &'static str {
    if is_code_search_typo(query) {
        CODE_SEARCH_TYPO_CURSOR_ORDER
    } else if is_code_search_components(query) {
        CODE_SEARCH_COMPONENT_CURSOR_ORDER
    } else {
        CODE_SEARCH_CURSOR_ORDER
    }
}

fn is_code_search_typo(query: &LqQuery) -> bool {
    if query.options.pattern_type != LqPatternType::CodeSearch {
        return false;
    }
    let leaf = match &query.expr {
        LqExpr::Leaf(leaf) => Some(leaf),
        LqExpr::All(parts) => match parts.as_slice() {
            [LqExpr::Leaf(leaf)] => Some(leaf),
            _ => None,
        },
        LqExpr::Empty | LqExpr::Not(_) | LqExpr::Any(_) => None,
    };
    matches!(leaf, Some(LqLeaf::Predicate { name, .. }) if name == CODE_SEARCH_IDENTIFIER_TYPO_PREDICATE)
}

fn is_code_search_components(query: &LqQuery) -> bool {
    if query.options.pattern_type != LqPatternType::CodeSearch {
        return false;
    }
    let leaf = match &query.expr {
        LqExpr::Leaf(leaf) => Some(leaf),
        LqExpr::All(parts) => match parts.as_slice() {
            [LqExpr::Leaf(leaf)] => Some(leaf),
            _ => None,
        },
        LqExpr::Empty | LqExpr::Not(_) | LqExpr::Any(_) => None,
    };
    matches!(leaf, Some(LqLeaf::Predicate { name, .. }) if name == CODE_SEARCH_SYMBOL_COMPONENTS_PREDICATE)
}

fn lexical_explanation(
    budget: &RequestBudgetV1,
    execution: &LaneExecutionSummaryV1,
    stage_timings: Option<Vec<QueryStageTimingV1>>,
) -> SearchExplanation {
    let mut explanation = SearchExplanation::empty();
    explanation.request_id = budget.response_request_id();
    explanation.engines_executed = execution.executed_engines();
    explanation.engines_touched = execution.touched_engines();
    explanation.strategy = "lexical".to_string();
    explanation.stage_timings = stage_timings;
    explanation
}

fn code_search_execution_trace(
    stats: CodeSearchExecutionStatsV1,
    fetched: usize,
    exact_total: Option<u64>,
) -> Result<Vec<PlannerTraceEntry>, CoreError> {
    if Some(stats.cursor_eligible_files) != exact_total
        || u64::try_from(fetched).ok() != Some(stats.fetched_files)
        || stats.cursor_eligible_files > stats.verified_matching_files
        || stats.fetched_files > stats.cursor_eligible_files
        || stats.verified_matching_files > stats.final_candidate_visits
        || stats.literal_verified_files > stats.literal_source_verification_attempts
    {
        return Err(CoreError::InvalidContract(
            "lexical: contradictory code-search work counts".into(),
        ));
    }
    let mut entries = vec![PlannerTraceEntry {
        stage: PlannerStage::Merge,
        detail: "code_search.execution.scope=ordinary_exhaustive_page_v1;exploration_complete=true"
            .into(),
    }];
    entries.extend(
        [
            (
                "literal_source_verification_attempts",
                stats.literal_source_verification_attempts,
            ),
            ("literal_verified_files", stats.literal_verified_files),
            ("final_candidate_visits", stats.final_candidate_visits),
            ("verified_matching_files", stats.verified_matching_files),
            ("cursor_eligible_files", stats.cursor_eligible_files),
            ("fetched_files", stats.fetched_files),
        ]
        .into_iter()
        .map(|(name, value)| PlannerTraceEntry {
            stage: PlannerStage::Merge,
            detail: format!("code_search.execution.{name}={value}"),
        }),
    );
    Ok(entries)
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
        let prepare_started = self.query_stage_observation.start();
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
            order: if planned.query.options.pattern_type == LqPatternType::CodeSearch {
                code_search_rank_order(&planned.query)
            } else {
                LEXICAL_CURSOR_ORDER_V2
            },
            cap: request.top_k,
        };
        if let Some(opened) = &opened {
            self.cursors()?
                .require_context(opened, &cursor_context, Vec::new())?;
        }
        let mut stage_timings = StageTimings::new(self.query_stage_observation, 4);
        stage_timings.record_elapsed(QueryStageKindV1::LexicalPrepare, prepare_started, 1, None);
        let wants_file_owner_projection = query_selects_file_owner_projection(&planned.query);
        let rank_unit = text_rank_unit(&planned.query);
        require_cursor_on_nonempty_plan(opened.is_some(), planned.force_empty)?;
        if planned.force_empty {
            let project_started = self.query_stage_observation.start();
            let window = QueryResultWindowV2::logical_empty("lexical");
            stage_timings.record_elapsed(
                QueryStageKindV1::LexicalProject,
                project_started,
                1,
                Some(0),
            );
            let summary = execution.summary();
            return Ok((
                TextQueryResponse {
                    generation: planned.pin.clone(),
                    rank_unit,
                    results: Vec::new(),
                    window,
                    explanation: lexical_explanation(budget, &summary, stage_timings.finish()),
                    file_owner_rows: wants_file_owner_projection.then(Vec::new),
                    next_cursor: None,
                },
                summary,
            ));
        }
        let view_started = self.query_stage_observation.start();
        let view = self.acquire_read_view(
            &ReadViewRequestV1::new("lexical", &planned.pin, planned.domains),
            budget,
        )?;
        stage_timings.record_elapsed(QueryStageKindV1::LexicalReadView, view_started, 1, None);
        let searcher = view.lexical()?;
        let fetch_top_k = lexical_fetch_limit_v1(&planned.query, request.top_k)?;
        budget.checkpoint("lexical:search")?;
        execution.record_lexical_invocation();
        let search_started = self.query_stage_observation.start();
        let mut page = searcher.search_constrained(
            &planned.query,
            &planned.constraints,
            &LexicalPageSpec {
                fetch: fetch_top_k,
                after: continuation(opened.as_ref().map(|cursor| &cursor.boundary), &planned.pin)?,
            },
            budget,
        )?;
        stage_timings.record_elapsed(
            QueryStageKindV1::LexicalSearch,
            search_started,
            1,
            Some(page.candidates.len()),
        );
        budget.checkpoint("lexical:project")?;
        let project_started = self.query_stage_observation.start();
        if page.code_search_stats.is_some()
            && planned.query.options.pattern_type != LqPatternType::CodeSearch
        {
            return Err(CoreError::InvalidContract(
                "lexical: code-search counts on a different engine".into(),
            ));
        }
        let code_search_trace = page
            .code_search_stats
            .map(|stats| {
                code_search_execution_trace(stats, page.candidates.len(), page.exact_total)
            })
            .transpose()?;
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
        stage_timings.record_elapsed(
            QueryStageKindV1::LexicalProject,
            project_started,
            1,
            Some(results.len()),
        );
        let mut explanation =
            lexical_explanation(budget, &execution.summary(), stage_timings.finish());
        if let Some(trace) = code_search_trace {
            explanation.planner_trace.extend(trace);
            // Reserve before fitting. Updating to a shorter returned prefix
            // cannot enlarge the serialized explanation after fitting.
            explanation.planner_trace.push(PlannerTraceEntry {
                stage: PlannerStage::Merge,
                detail: format!("code_search.execution.returned_files={}", results.len()),
            });
        }
        if !results.is_empty() {
            // Reserve the maximal explanation shape before response-budget
            // fitting. A truncated page can only remove this contribution.
            explanation.engines_touched.push(EngineTouched::Lexical);
        }
        let mut response = fit_ranked_page(
            TextQueryResponse {
                generation: planned.pin.clone(),
                rank_unit,
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
        if let Some(entry) = response.explanation.planner_trace.iter_mut().find(|entry| {
            entry
                .detail
                .starts_with("code_search.execution.returned_files=")
        }) {
            entry.detail = format!(
                "code_search.execution.returned_files={}",
                response.results.len()
            );
        }
        if let Some(stages) = response.explanation.stage_timings.as_mut() {
            let final_stage = stages.last_mut().ok_or_else(|| {
                CoreError::InvalidContract("lexical project stage missing".to_string())
            })?;
            final_stage.returned_candidates =
                Some(u64::try_from(response.results.len()).map_or(u64::MAX, |count| count));
        }
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
        // The page token is authenticated below. Query lowering accepts only
        // the query inputs, as on the Text route, not its continuation token.
        let pageless = TextQueryRequest {
            cursor: None,
            ..TextQueryRequest::from(request.clone())
        };
        reject_code_search_on_nonlexical_route(pageless.syntax, "symbol")?;
        let lowered = lower_lexical_text_query(&pageless)?;
        let validated =
            LexicalPolicy::plan_query(&lowered, &request.constraints, LexicalEndpoint::Symbol)?;
        self.lex_opener
            .preflight_query_primitives(&validated, budget)?;
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
        let prepared_language = prepare_language_query_v1(lowered, &lexical_request.constraints)?;
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
        require_cursor_on_nonempty_plan(opened.is_some(), prepared_language.force_empty)?;
        if prepared_language.force_empty {
            return Ok((
                SymbolQueryResponse {
                    generation: pin.clone(),
                    results: Vec::new(),
                    window: QueryResultWindowV2::logical_empty("symbol"),
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
        // The producer carries count facts separately from capped rows.
        let fetch_top_k = lexical_fetch_limit_v1(&prepared_language.query, lexical_request.top_k)?;
        budget.checkpoint("symbol:search")?;
        execution.record_lexical_invocation();
        let mut page = searcher.search_symbols_constrained(
            &prepared_language.query,
            &prepared_language.constraints,
            &LexicalPageSpec {
                fetch: fetch_top_k,
                after,
            },
            budget,
        )?;
        let window = lexical_page_window_v1(&mut page, lexical_request.top_k, fetch_top_k)?;
        let results = page.candidates;
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

#[cfg(test)]
mod typed_cursor_tests {
    use super::{
        CODE_SEARCH_COMPONENT_CURSOR_ORDER, CODE_SEARCH_CURSOR_ORDER,
        CODE_SEARCH_TYPO_CURSOR_ORDER, is_code_search_components, is_code_search_typo,
    };
    use crate::lowering::lower_code_search_query_text;

    #[test]
    fn code_search_execution_counts_reject_contradictory_page_provenance() {
        use super::{CodeSearchExecutionStatsV1, code_search_execution_trace};
        let stats = CodeSearchExecutionStatsV1 {
            literal_source_verification_attempts: 3,
            literal_verified_files: 2,
            final_candidate_visits: 2,
            verified_matching_files: 2,
            cursor_eligible_files: 1,
            fetched_files: 1,
        };
        assert_eq!(
            code_search_execution_trace(stats, 1, Some(1))
                .expect("fixed counts")
                .len(),
            7
        );
        assert!(code_search_execution_trace(stats, 2, Some(1)).is_err());
        assert!(code_search_execution_trace(stats, 1, Some(2)).is_err());
        assert!(code_search_execution_trace(stats, 1, None).is_err());
        let invalid = CodeSearchExecutionStatsV1 {
            verified_matching_files: 3,
            ..stats
        };
        assert!(code_search_execution_trace(invalid, 1, Some(1)).is_err());
    }

    #[test]
    fn typo_cursor_order_is_distinct_from_exact_code_search() {
        let typo = lower_code_search_query_text("typo:load_jsom").expect("typo query");
        let exact = lower_code_search_query_text("load_jsom").expect("exact query");
        assert!(is_code_search_typo(&typo));
        assert!(!is_code_search_typo(&exact));
        assert_ne!(CODE_SEARCH_TYPO_CURSOR_ORDER, CODE_SEARCH_CURSOR_ORDER);
    }

    #[test]
    fn component_cursor_order_is_distinct_from_other_file_modes() {
        let components =
            lower_code_search_query_text("components:\"clean up\"").expect("component query");
        let ordinary = lower_code_search_query_text("clean up").expect("ordinary query");
        let typo = lower_code_search_query_text("typo:load_jsom").expect("typo query");
        assert!(is_code_search_components(&components));
        assert!(!is_code_search_components(&ordinary));
        assert!(!is_code_search_components(&typo));
        assert_ne!(CODE_SEARCH_COMPONENT_CURSOR_ORDER, CODE_SEARCH_CURSOR_ORDER);
        assert_ne!(
            CODE_SEARCH_COMPONENT_CURSOR_ORDER,
            CODE_SEARCH_TYPO_CURSOR_ORDER
        );
    }
}
