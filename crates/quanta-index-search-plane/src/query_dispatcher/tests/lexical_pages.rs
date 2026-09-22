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
    LexicalCandidate, LexicalCursor, ManifestGeneration, QueryConstraintSetV1, QueryResultWindowV2,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, TextQueryRequest, TextQueryResponse,
    TextQuerySyntax,
};
use quanta_index_core::RequestBudgetV1;

use crate::observability::NoopQueryObsSink;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::response_budget::{
    RankedPage, ResponsePayloadBudget, fit_ranked_page,
};
use crate::query_dispatcher::tests::support::common::{
    TestResult, candidate, dispatcher_with_obs, ipc_error_from, ready_pin,
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
        other @ (SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
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
    let budget = ResponsePayloadBudget::new(2_300)?;
    let dispatcher = stub_dispatcher(rows(2, 1_000))?.with_response_budget(budget);
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
fn a_shorter_prefix_can_exceed_the_budget_when_its_cursor_is_larger() -> TestResult {
    let results = rows(4, 8);
    let large_token = ContinuationTokenV2::new("L".repeat(3_000))?;
    let page = TextQueryResponse {
        generation: ready_pin(),
        results,
        window: QueryResultWindowV2::pageable(4, CandidateCountV1::AtLeast(5), true, vec![])?,
        file_owner_rows: None,
        next_cursor: Some(large_token.clone()),
    };
    let budget = ResponsePayloadBudget::new(2_500)?;
    let two = page.clone().cut(
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
