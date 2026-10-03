//! Ranked lexical pages at the dispatcher (QI-BB-005 보완 #4/#5).
//!
//! A text page continues exactly when more rows exist and names its last
//! row as the cursor; a request carrying the cursor reaches the searcher
//! and is refused typed when it names another generation; a page past the
//! response byte budget is cut at the last row that fits and continued by
//! that row, and a first row that alone does not fit is refused typed
//! before anything is encoded.

use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    CandidateCountV1, ContinuationTokenV2, ERR_RESULT_TOO_LARGE, FileOwnerProjectionRow,
    LexicalCandidate, LexicalCursor, ManifestGeneration, PlannerStage, PlannerTraceEntry,
    QueryConstraintSetV1, QueryResultWindowV2, RepoRelativePath, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SymbolQueryResponse, TextQueryRequest, TextQueryResponse,
    TextQuerySyntax,
};
use quanta_index_core::{QueryRouteV1, RequestBudgetV1};

use crate::observability::NoopQueryObsSink;
use crate::query_dispatcher::continuation::CursorRequestContextV2;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::response_budget::{
    RankedPage, ResponsePayloadBudget, fit_ranked_page,
};
use crate::query_dispatcher::tests::support::common::{
    TestResult, candidate, dispatcher_with_obs, ipc_error_from, ready_pin, symbol_candidate,
};
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState, StubLexicalOpener,
};
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;

/// Rows tied on score, path and lines, so the page order is the id order.
fn rows(count: usize, snippet_bytes: usize) -> Vec<LexicalCandidate> {
    (0..count)
        .map(|index| {
            let mut row = candidate(&format!("cand-{index:02}"), 1.0);
            row.snippet = "x".repeat(snippet_bytes);
            row
        })
        .collect()
}

fn request(top_k: u32, cursor: Option<ContinuationTokenV2>) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "needle".to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(ready_pin()),
        generation_selector: None,
        top_k,
        cursor,
    })
}

fn stub_dispatcher(
    results: Vec<LexicalCandidate>,
) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>> {
    dispatcher_with_obs(
        Arc::new(StubLexicalOpener { results }),
        Arc::new(RejectSemanticOpener),
        Arc::new(NoopQueryObsSink),
    )
}

fn text(response: SearchPlaneQueryIpcResponse) -> Result<TextQueryResponse, String> {
    match response {
        SearchPlaneQueryIpcResponse::Text(page) => Ok(page),
        other @ (SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            Err(format!("expected a text page, got {other:?}"))
        }
    }
}

fn ids(rows: &[LexicalCandidate]) -> Vec<String> {
    rows.iter().map(|row| row.candidate_id.clone()).collect()
}

/// Walk every page, checking each continuation names its last row.
fn walk(
    dispatcher: &SearchPlaneDispatcher,
    top_k: u32,
) -> Result<Vec<Vec<String>>, Box<dyn std::error::Error>> {
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut cursor: Option<ContinuationTokenV2> = None;
    for _ in 0..64 {
        let page = text(dispatcher.dispatch(
            request(top_k, cursor.clone()),
            &RequestBudgetV1::unbounded(),
        ))?;
        pages.push(ids(&page.results));
        if page.window.has_more() == Some(true) {
            let token = page
                .next_cursor
                .as_ref()
                .ok_or("continued page has no token")?;
            let opened = dispatcher.cursors()?.open::<LexicalCursor>(token)?;
            let expected = page.results.last().ok_or("continued page has no row")?;
            if opened.boundary
                != LexicalCursor::at(ManifestGeneration::new(9), expected.order_key())
            {
                return Err("the signed continuation boundary is not the last row".into());
            }
        } else if page.next_cursor.is_some() {
            return Err("a final page carries a continuation token".into());
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(pages),
        }
    }
    Err("the walk did not end".into())
}

#[test]
fn a_page_continues_exactly_when_more_rows_exist() -> TestResult {
    let dispatcher = stub_dispatcher(rows(5, 8))?;
    let pages = walk(&dispatcher, 2)?;
    let expected: Vec<Vec<String>> = vec![
        vec!["cand-00".into(), "cand-01".into()],
        vec!["cand-02".into(), "cand-03".into()],
        vec!["cand-04".into()],
    ];
    if pages != expected {
        return Err(format!("pages drifted: {pages:?}").into());
    }
    Ok(())
}

#[test]
fn code_search_pages_bind_the_overlap_ranking_order() -> TestResult {
    let mut files = rows(3, 8);
    for (index, file) in files.iter_mut().enumerate() {
        file.repo_relative_path = RepoRelativePath::new(format!("src/{index:02}.rs"));
    }
    let dispatcher = stub_dispatcher(files)?;
    let mut first_request = request(2, None);
    let SearchPlaneQueryIpcRequest::Text(first_text) = &mut first_request else {
        return Err("text fixture drifted".into());
    };
    first_text.syntax = TextQuerySyntax::CodeSearch;
    let first = text(dispatcher.dispatch(first_request.clone(), &RequestBudgetV1::unbounded()))?;
    let token = first.next_cursor.clone().ok_or("first page continues")?;
    let opened = dispatcher.cursors()?.open::<LexicalCursor>(&token)?;
    if opened.binding().order
        != "code_search_file_overlap_score_v1_desc_source_repo_path_line_candidate"
    {
        return Err(format!(
            "unexpected code search cursor order: {}",
            opened.binding().order
        )
        .into());
    }

    if let SearchPlaneQueryIpcRequest::Text(request_text) = &mut first_request {
        request_text.cursor = Some(token);
    }
    let second = text(dispatcher.dispatch(first_request.clone(), &RequestBudgetV1::unbounded()))?;
    if ids(&first.results) != ["cand-00", "cand-01"]
        || ids(&second.results) != ["cand-02"]
        || second.next_cursor.is_some()
    {
        return Err(
            format!("code search pages drifted: first={first:?}, second={second:?}").into(),
        );
    }

    let SearchPlaneQueryIpcRequest::Text(request_text) = &mut first_request else {
        return Err("text fixture drifted".into());
    };
    let planning_request = TextQueryRequest {
        cursor: None,
        ..request_text.clone()
    };
    let planned = dispatcher.plan_lexical_text_query(
        &planning_request,
        QueryRouteV1::Lexical,
        &RequestBudgetV1::unbounded(),
    )?;
    let old_context = CursorRequestContextV2 {
        route: quanta_index_contract::CursorRouteV2::Lexical,
        pin: &planned.pin,
        query: &planned.query,
        constraints: &planned.constraints,
        order: "code_search_file_score_v1_desc_source_repo_path_line_candidate",
        cap: 2,
    };
    let old_token = dispatcher
        .cursors()?
        .mint(&opened.boundary, &old_context, Vec::new())?;
    request_text.cursor = Some(old_token);
    let (code, _) =
        ipc_error_from(dispatcher.dispatch(first_request, &RequestBudgetV1::unbounded()))?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::CursorContextMismatch {
        return Err(format!("old scoring cursor answered {code}").into());
    }
    Ok(())
}

#[test]
fn typo_cursor_uses_its_own_order_and_rejects_exact_reuse() -> TestResult {
    let mut files = rows(3, 8);
    for (index, file) in files.iter_mut().enumerate() {
        file.repo_relative_path = RepoRelativePath::new(format!("src/{index:02}.rs"));
    }
    let dispatcher = stub_dispatcher(files)?;
    let mut request = request(2, None);
    let SearchPlaneQueryIpcRequest::Text(text_request) = &mut request else {
        return Err("text fixture drifted".into());
    };
    text_request.syntax = TextQuerySyntax::CodeSearch;
    text_request.query_text = "typo:needlx".into();
    let first = text(dispatcher.dispatch(request.clone(), &RequestBudgetV1::unbounded()))?;
    let token = first.next_cursor.ok_or("typo page continues")?;
    let opened = dispatcher.cursors()?.open::<LexicalCursor>(&token)?;
    if opened.binding().order
        != "code_search_identifier_typo_osa1_v1_desc_source_repo_path_line_candidate"
    {
        return Err(format!("wrong typo cursor order: {}", opened.binding().order).into());
    }
    let SearchPlaneQueryIpcRequest::Text(text_request) = &mut request else {
        return Err("text fixture drifted".into());
    };
    text_request.cursor = Some(token);
    text_request.query_text = "needlx".into();
    let (code, _) = ipc_error_from(dispatcher.dispatch(request, &RequestBudgetV1::unbounded()))?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::CursorContextMismatch {
        return Err(format!("exact query reused typo cursor: {code}").into());
    }
    Ok(())
}

#[test]
fn the_cursor_reaches_the_searcher_and_another_generations_is_refused_first() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: rows(3, 8),
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(NoopQueryObsSink),
    )?;
    let first = text(dispatcher.dispatch(request(2, None), &RequestBudgetV1::unbounded()))?;
    let cursor = first.next_cursor.ok_or("first page continues")?;
    let decoded = dispatcher
        .cursors()?
        .open::<LexicalCursor>(&cursor)?
        .boundary;
    let _page = text(dispatcher.dispatch(
        request(2, Some(cursor.clone())),
        &RequestBudgetV1::unbounded(),
    ))?;
    let afters = state
        .lock()
        .map_err(|err| format!("state: {err}"))?
        .search_afters
        .clone();
    if afters != vec![None, Some(decoded)] {
        return Err(format!("the searcher saw {afters:?}").into());
    }

    let mut foreign = request(2, Some(cursor.clone()));
    let SearchPlaneQueryIpcRequest::Text(foreign_request) = &mut foreign else {
        return Err("text request fixture drifted".into());
    };
    let mut foreign_pin = ready_pin();
    foreign_pin.manifest_generation = ManifestGeneration::new(10);
    foreign_request.generation = Some(foreign_pin);
    let (code, _message) =
        ipc_error_from(dispatcher.dispatch(foreign, &RequestBudgetV1::unbounded()))?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::CursorContextMismatch {
        return Err(format!("a foreign cursor answered `{code}`").into());
    }
    let (code, _) = ipc_error_from(dispatcher.dispatch(
        request(1, Some(cursor.clone())),
        &RequestBudgetV1::unbounded(),
    ))?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::CursorContextMismatch {
        return Err(format!("a changed page cap answered `{code}`").into());
    }
    let mut changed_query = request(2, Some(cursor.clone()));
    let SearchPlaneQueryIpcRequest::Text(text) = &mut changed_query else {
        return Err("text request fixture drifted".into());
    };
    text.query_text = "different query".to_string();
    let (code, _) =
        ipc_error_from(dispatcher.dispatch(changed_query, &RequestBudgetV1::unbounded()))?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::CursorContextMismatch {
        return Err(format!("a changed query answered `{code}`").into());
    }
    let mut tampered = cursor.as_str().as_bytes().to_vec();
    let first = tampered.first_mut().ok_or("nonempty token")?;
    *first = if *first == b'A' { b'B' } else { b'A' };
    let tampered = ContinuationTokenV2::new(String::from_utf8(tampered)?)?;
    let (code, _) = ipc_error_from(
        dispatcher.dispatch(request(2, Some(tampered)), &RequestBudgetV1::unbounded()),
    )?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::CursorInvalid {
        return Err(format!("a tampered token answered `{code}`").into());
    }
    let searches = state
        .lock()
        .map_err(|err| format!("state: {err}"))?
        .search_top_ks
        .len();
    if searches != 2 {
        return Err(format!("the refusal came after a search: {searches} searches").into());
    }
    Ok(())
}

/// A page past the byte budget is cut at the last row that fits, stays
/// under the budget as encoded, and the cursor walk still visits every row
/// once, in order.
#[test]
fn a_page_past_the_byte_budget_is_cut_and_continued() -> TestResult {
    let all = rows(6, 1_000);
    let budget = ResponsePayloadBudget::new(3_500)?;
    let dispatcher = stub_dispatcher(all.clone())?.with_response_budget(budget);
    let first = text(dispatcher.dispatch(request(6, None), &RequestBudgetV1::unbounded()))?;
    let encoded = quanta_index_ipc::cbor_payload_len(&first)?;
    if encoded > budget.max_payload_bytes() {
        return Err(format!("the cut page encodes to {encoded} bytes").into());
    }
    if first.results.is_empty()
        || first.results.len() >= all.len()
        || first.window.has_more() != Some(true)
    {
        return Err(format!(
            "six 1 KB rows cannot fit 3.5 KB, but one can: {} rows, has_more {:?}",
            first.results.len(),
            first.window.has_more()
        )
        .into());
    }
    let pages = walk(&dispatcher, 6)?;
    let walked: Vec<String> = pages.into_iter().flatten().collect();
    if walked != ids(&all) {
        return Err(format!("the byte-cut walk drifted: {walked:?}").into());
    }
    Ok(())
}

#[expect(
    clippy::indexing_slicing,
    reason = "rows(3, 8) builds exactly three rows; indices 0..3 and prefix ..2 are in range"
)]
#[test]
fn a_large_later_row_cannot_refuse_a_fitting_prefix() -> TestResult {
    let mut all = rows(3, 8);
    all[2].snippet = "x".repeat(8_000);
    let budget = ResponsePayloadBudget::new(3_500)?;
    let dispatcher = stub_dispatcher(all.clone())?.with_response_budget(budget);
    let first = text(dispatcher.dispatch(request(3, None), &RequestBudgetV1::unbounded()))?;
    if ids(&first.results) != ids(&all[..2])
        || first.window.has_more() != Some(true)
        || first.next_cursor.is_none()
    {
        return Err(format!("a fitting prefix was not continued: {first:?}").into());
    }
    let encoded = quanta_index_ipc::cbor_payload_len(&first)?;
    if encoded > budget.max_payload_bytes() {
        return Err(format!("the prefix encodes to {encoded} bytes").into());
    }
    Ok(())
}

#[test]
fn a_large_later_owner_projection_cannot_refuse_a_fitting_paired_prefix() -> TestResult {
    let results = rows(3, 8);
    let file_owner_rows = results
        .iter()
        .enumerate()
        .map(|(index, candidate)| FileOwnerProjectionRow {
            source_repo_id: candidate.source_repo_id.clone(),
            candidate_id: candidate.candidate_id.clone(),
            repo_id: candidate.repo_id.clone(),
            revision_id: candidate.revision_id.clone(),
            manifest_generation: candidate.manifest_generation,
            repo_relative_path: candidate.repo_relative_path.clone(),
            owners: if index == 2 {
                vec!["owner".repeat(1_600)]
            } else {
                Vec::new()
            },
        })
        .collect();
    let page = TextQueryResponse {
        rank_unit: quanta_index_contract::TextRankUnit::Chunk,
        explanation: quanta_index_contract::SearchExplanation::empty(),
        generation: ready_pin(),
        results,
        window: QueryResultWindowV2::pageable(3, CandidateCountV1::Exact(3), false, vec![])?,
        file_owner_rows: Some(file_owner_rows),
        next_cursor: None,
    };
    let budget = ResponsePayloadBudget::new(3_500)?;
    let cut = fit_ranked_page(page, budget, |_boundary| {
        Ok(ContinuationTokenV2::new("opaque").expect("nonempty fixture token"))
    })?;
    if cut.results.len() != 2
        || cut.file_owner_rows.as_ref().map(Vec::len) != Some(2)
        || cut.window.has_more() != Some(true)
        || cut.next_cursor.is_none()
    {
        return Err(format!("paired prefix was not continued: {cut:?}").into());
    }
    let encoded = quanta_index_ipc::cbor_payload_len(&cut)?;
    if encoded > budget.max_payload_bytes() {
        return Err(format!("paired prefix encodes to {encoded} bytes").into());
    }
    Ok(())
}

#[test]
fn a_fitting_first_row_is_not_refused_by_cursor_reservation() -> TestResult {
    let dispatcher = stub_dispatcher(rows(2, 1_000))?;
    // Size an independently requested one-row continued page with the current
    // wire shape and signed cursor. Timing values are excluded from pagination
    // authority and replaced by the contract's fixed observation reserve.
    let mut one = text(dispatcher.dispatch(request(1, None), &RequestBudgetV1::unbounded()))?;
    if one.results.len() != 1 || one.next_cursor.is_none() {
        return Err("the reference page must contain one row and a real cursor".into());
    }
    one.explanation.stage_timings = None;
    let limit = quanta_index_ipc::cbor_payload_len(&one)?
        .checked_add(crate::query_dispatcher::response_budget::LEXICAL_STAGE_RESERVE_BYTES)
        .ok_or("reference response size overflow")?;
    let budget = ResponsePayloadBudget::new(limit)?;
    let mut whole = text(dispatcher.dispatch(request(2, None), &RequestBudgetV1::unbounded()))?;
    whole.explanation.stage_timings = None;
    let whole_bytes = quanta_index_ipc::cbor_payload_len(&whole)?
        .checked_add(crate::query_dispatcher::response_budget::LEXICAL_STAGE_RESERVE_BYTES)
        .ok_or("whole response size overflow")?;
    if whole.results.len() != 2 || whole_bytes <= limit {
        return Err("the uncut two-row reference must exceed the one-row budget".into());
    }
    let dispatcher = dispatcher.with_response_budget(budget);
    let first = text(dispatcher.dispatch(request(2, None), &RequestBudgetV1::unbounded()))?;
    if first.results.len() != 1 || first.window.has_more() != Some(true) {
        return Err(format!("the first fitting row was not continued: {first:?}").into());
    }
    let encoded = quanta_index_ipc::cbor_payload_len(&first)?;
    if encoded > budget.max_payload_bytes() {
        return Err(format!("the prefix encodes to {encoded} bytes").into());
    }
    Ok(())
}

#[test]
fn code_search_clock_policy_keeps_tight_page_and_continuation_identical() -> TestResult {
    let results = rows(3, 400);
    let mut explanation = quanta_index_contract::SearchExplanation::empty();
    explanation.planner_trace = [
        "code_search.execution.scope=ordinary_exhaustive_page_v1;exploration_complete=true",
        "code_search.execution.mode=ordinary",
        "code_search.execution.posting_probes=3",
    ]
    .into_iter()
    .map(|detail| PlannerTraceEntry {
        stage: PlannerStage::Merge,
        detail: detail.to_string(),
    })
    .collect();
    let off = TextQueryResponse {
        rank_unit: quanta_index_contract::TextRankUnit::Chunk,
        explanation,
        generation: ready_pin(),
        results,
        window: QueryResultWindowV2::pageable(3, CandidateCountV1::Exact(3), false, vec![])?,
        file_owner_rows: None,
        next_cursor: None,
    };
    let mut on = off.clone();
    for (name, value) in [
        ("candidate_ns", 1_u64),
        ("sort_page_ns", u64::MAX),
        ("preview_ns", 42_u64),
    ] {
        on.explanation.planner_trace.push(PlannerTraceEntry {
            stage: PlannerStage::Merge,
            detail: format!("code_search.execution.{name}={value}"),
        });
    }
    if on.budget_encoded_len()? != off.budget_encoded_len()? {
        return Err("CodeSearch on/off clocks changed the page budget".into());
    }
    let token = ContinuationTokenV2::new("same-cursor")?;
    let mut one = off.clone();
    one.cut(
        1,
        crate::query_dispatcher::window::cut_pageable_window_v2(&off.window, 1)?,
        token.clone(),
    );
    let limit = one.budget_encoded_len()?;
    if off.budget_encoded_len()? <= limit {
        return Err("the uncut fixture must exceed its one-row budget".into());
    }
    let budget = ResponsePayloadBudget::new(limit)?;
    let fitted_on = fit_ranked_page(on, budget, |_cursor| Ok(token.clone()))?;
    let fitted_off = fit_ranked_page(off, budget, |_cursor| Ok(token.clone()))?;
    if ids(&fitted_on.results) != ids(&fitted_off.results)
        || fitted_on.results.len() != 1
        || fitted_on.window != fitted_off.window
        || fitted_on.next_cursor != fitted_off.next_cursor
        || fitted_on.next_cursor != Some(token)
        || quanta_index_ipc::cbor_payload_len(&fitted_on)? > limit
        || quanta_index_ipc::cbor_payload_len(&fitted_off)? > limit
    {
        return Err("CodeSearch clocks changed a tight page or continuation".into());
    }
    let mut duplicate = fitted_on;
    let clock = duplicate
        .explanation
        .planner_trace
        .last()
        .ok_or("clock trace disappeared")?
        .clone();
    duplicate.explanation.planner_trace.push(clock);
    if duplicate.budget_encoded_len().is_ok() {
        return Err("duplicated CodeSearch clock passed the budget contract".into());
    }
    Ok(())
}

#[test]
fn a_shorter_prefix_can_exceed_the_budget_when_its_cursor_is_larger() -> TestResult {
    let results = rows(4, 8);
    let large_token = ContinuationTokenV2::new("L".repeat(3_000))?;
    let page = TextQueryResponse {
        rank_unit: quanta_index_contract::TextRankUnit::Chunk,
        explanation: quanta_index_contract::SearchExplanation::empty(),
        generation: ready_pin(),
        results,
        window: QueryResultWindowV2::pageable(4, CandidateCountV1::AtLeast(5), true, vec![])?,
        file_owner_rows: None,
        next_cursor: Some(large_token.clone()),
    };
    let budget = ResponsePayloadBudget::new(2_500)?;
    let mut two = page.clone();
    two.cut(
        2,
        crate::query_dispatcher::window::cut_pageable_window_v2(&page.window, 2)?,
        large_token,
    );
    if quanta_index_ipc::cbor_payload_len(&two)? <= budget.max_payload_bytes() {
        return Err("the two-row fixture must not fit with its large cursor".into());
    }
    let fitted = fit_ranked_page(page, budget, |cursor| {
        Ok(
            ContinuationTokenV2::new(if cursor.candidate_id == "cand-02" {
                "short".to_string()
            } else {
                "L".repeat(3_000)
            })
            .expect("nonempty fixture token"),
        )
    })?;
    if fitted.results.len() != 3
        || fitted.window.has_more() != Some(true)
        || fitted.next_cursor.as_ref().map(ContinuationTokenV2::as_str) != Some("short")
        || quanta_index_ipc::cbor_payload_len(&fitted)? > budget.max_payload_bytes()
    {
        return Err(format!("the longest fitting prefix was not selected: {fitted:?}").into());
    }
    Ok(())
}

#[expect(
    clippy::indexing_slicing,
    reason = "the len check short-circuits the same condition before either index runs"
)]
#[test]
fn symbol_page_budget_cut_keeps_ranked_prefix_window_and_cursor() -> TestResult {
    let results = (0..4)
        .map(|index| {
            let mut row = symbol_candidate(&format!("symbol-{index:02}"), 1.0);
            row.snippet = "x".repeat(400);
            row
        })
        .collect();
    let page = SymbolQueryResponse {
        generation: ready_pin(),
        results,
        window: QueryResultWindowV2::pageable(4, CandidateCountV1::Exact(4), false, vec![])?,
        next_cursor: None,
    };
    let token = ContinuationTokenV2::new("opaque")?;
    let mut two = page.clone();
    two.cut(
        2,
        crate::query_dispatcher::window::cut_pageable_window_v2(&page.window, 2)?,
        token.clone(),
    );
    let mut three = page.clone();
    three.cut(
        3,
        crate::query_dispatcher::window::cut_pageable_window_v2(&page.window, 3)?,
        token.clone(),
    );
    let budget = ResponsePayloadBudget::new(quanta_index_ipc::cbor_payload_len(&two)?)?;
    if quanta_index_ipc::cbor_payload_len(&three)? <= budget.max_payload_bytes() {
        return Err("the three-symbol prefix must exceed the two-symbol budget".into());
    }

    let fitted = fit_ranked_page(page, budget, |_cursor| Ok(token.clone()))?;
    if fitted.results.len() != 2
        || fitted.results[0].candidate_id != "symbol-00"
        || fitted.results[1].candidate_id != "symbol-01"
        || fitted.window.returned() != 2
        || fitted.window.candidate_count() != CandidateCountV1::Exact(4)
        || fitted.window.has_more() != Some(true)
        || fitted.next_cursor != Some(token)
    {
        return Err(format!("symbol prefix contract was not preserved: {fitted:?}").into());
    }
    Ok(())
}

#[test]
fn a_first_row_larger_than_the_budget_is_refused_typed() -> TestResult {
    let dispatcher =
        stub_dispatcher(rows(2, 1_000))?.with_response_budget(ResponsePayloadBudget::new(200)?);
    let (code, _message) =
        ipc_error_from(dispatcher.dispatch(request(2, None), &RequestBudgetV1::unbounded()))?;
    if code != ERR_RESULT_TOO_LARGE {
        return Err(format!("an oversized first row answered `{code}`").into());
    }
    Ok(())
}

/// A cursor riding on the text query of a route that ranks no pages is
/// refused typed, never ignored: the runtime-metadata route keeps its own
/// keyset cursor, and its text leaf does not continue.
#[test]
fn a_cursor_on_a_route_that_does_not_page_is_refused() -> TestResult {
    use crate::query_dispatcher::tests::support::runtime_metadata::{
        ready_runtime_metadata_ledger, runtime_metadata_dispatcher_with_ledger,
        runtime_query_request,
    };
    let dispatcher = runtime_metadata_dispatcher_with_ledger(ready_runtime_metadata_ledger(1, 1))?;
    let SearchPlaneQueryIpcRequest::RuntimeMetadata(mut runtime) =
        runtime_query_request(TextQuerySyntax::Native, "dirty:yes")
    else {
        return Err("the helper builds a runtime-metadata request".into());
    };
    runtime.text_query.cursor = Some(ContinuationTokenV2::new("signed-nested-text")?);
    let (code, _message) = ipc_error_from(dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::RuntimeMetadata(runtime),
        &RequestBudgetV1::unbounded(),
    ))?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::QueryCursorUnsupported {
        return Err(format!("a text-leaf cursor answered `{code}`").into());
    }
    Ok(())
}
