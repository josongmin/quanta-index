//! L1 regressions at the real dispatcher boundary, using an instrumented port.
//!
//! Registration in tests/mod.rs is coordinated with L0; no production validator
//! or replacement window arithmetic is implemented by these fixtures.

use std::sync::{Arc, Mutex};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ExactRepoRelativePathV1, LqCase, QueryConstraintSetV1, SearchPlaneErrorCodeV2,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, SymbolQueryRequest, TextQueryRequest,
    TextQuerySyntax,
};
use quanta_index_core::RequestBudgetV1;

use crate::observability::NoopQueryObsSink;
use crate::query_dispatcher::tests::support::common::{
    TestResult, dispatcher_with_obs, ipc_error_from, ready_pin,
};
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState,
};
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;

fn symbol_request(text: &str, language: Option<&str>) -> Result<SymbolQueryRequest, String> {
    let constraints = match language {
        Some(language) => QueryConstraintSetV1::from_languages([
            LanguageCode::new(language).map_err(str::to_string)?
        ]),
        None => QueryConstraintSetV1::unconstrained(),
    };
    Ok(SymbolQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: text.into(),
        constraints,
        generation: Some(ready_pin()),
        generation_selector: None,
        top_k: 3,
        cursor: None,
    })
}

#[test]
fn native_exact_symbol_case_and_typed_file_reach_one_search_plan() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: Vec::new(),
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(NoopQueryObsSink),
    )?;
    let mut request = symbol_request("symbol.local_name.exact(writeContentType) case:yes", None)?;
    request.constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
        ExactRepoRelativePathV1::new("render/render.go").map_err(str::to_string)?,
    );
    let (_page, _execution) =
        dispatcher.symbol_with_execution(request, &RequestBudgetV1::unbounded())?;
    let calls = state.lock().map_err(|error| error.to_string())?;
    if calls.primitive_queries.len() != 1
        || calls.primitive_queries[0].options.case != Some(LqCase::Sensitive)
        || calls.symbol_top_ks != [4]
        || calls.symbol_constraints.len() != 1
        || calls.symbol_constraints[0]
            .repo_relative_path_exact
            .as_ref()
            .map(ExactRepoRelativePathV1::as_str)
            != Some("render/render.go")
    {
        return Err(format!(
            "Native exact-case/file dispatch: primitive_cases={:?} symbol_top_ks={:?} symbol_constraints={:?}",
            calls
                .primitive_queries
                .iter()
                .map(|query| query.options.case)
                .collect::<Vec<_>>(),
            calls.symbol_top_ks,
            calls.symbol_constraints,
        )
        .into());
    }
    Ok(())
}

#[test]
fn symbol_conflicts_reject_before_snapshot_open_and_language_empty() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: Vec::new(),
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(NoopQueryObsSink),
    )?;
    for language in [None, Some("rust")] {
        for conflict in [
            "select:file",
            "select:path",
            "select:content",
            "select:repo",
            "type:file",
            "type:symbol select:file",
        ] {
            for mode in ["", "index:no "] {
                for term in ["needle", "absent_term"] {
                    let text = format!("{mode}lang:python {conflict} {term}");
                    let request = symbol_request(&text, language)?;
                    let (code, message) = ipc_error_from(dispatcher.dispatch(
                        SearchPlaneQueryIpcRequest::Symbol(request),
                        &RequestBudgetV1::unbounded(),
                    ))?;
                    if code != SearchPlaneErrorCodeV2::InvalidRequest {
                        return Err(format!("{text}: {code}: {message}").into());
                    }
                }
            }
        }
    }
    let calls = state.lock().map_err(|error| error.to_string())?;
    let invoked = !calls.opened_pins.is_empty() || !calls.symbol_top_ks.is_empty();
    drop(calls);
    if invoked {
        return Err("pure domain conflict acquired a snapshot or invoked search".into());
    }
    Ok(())
}

#[test]
fn contradictory_languages_do_not_hide_zero_count() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: Vec::new(),
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(NoopQueryObsSink),
    )?;
    let symbol = symbol_request("lang:python count:0 needle", Some("rust"))?;
    for request in [
        SearchPlaneQueryIpcRequest::Symbol(symbol.clone()),
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest::from(symbol)),
    ] {
        let (code, message) =
            ipc_error_from(dispatcher.dispatch(request, &RequestBudgetV1::unbounded()))?;
        if code != SearchPlaneErrorCodeV2::LexFilterInvalidCount {
            return Err(
                format!("count:0 changed meaning on empty scope: {code}: {message}").into(),
            );
        }
    }
    let calls = state.lock().map_err(|error| error.to_string())?;
    let invoked = !calls.opened_pins.is_empty()
        || !calls.symbol_top_ks.is_empty()
        || !calls.search_top_ks.is_empty();
    drop(calls);
    if invoked {
        return Err("pure invalid count reached snapshot/search".into());
    }
    Ok(())
}

// Native planner coverage pins the same phrase's rejection independently.
// This regression requires shared primitive admission before LogicalEmpty;
// authoring it alone does not establish a reproduced dispatcher defect.
#[test]
fn contradictory_languages_do_not_hide_tokenless_phrase() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: Vec::new(),
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(NoopQueryObsSink),
    )?;
    let request = TextQueryRequest::from(symbol_request("lang:python \"!!!\"", Some("rust"))?);
    let (code, message) = ipc_error_from(dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(request),
        &RequestBudgetV1::unbounded(),
    ))?;
    if code != SearchPlaneErrorCodeV2::LexTextQueryNoTokens {
        return Err(
            format!("tokenless phrase changed meaning on empty scope: {code}: {message}").into(),
        );
    }
    let calls = state.lock().map_err(|error| error.to_string())?;
    if calls.primitive_queries.len() != 1 {
        return Err("dispatcher did not invoke exactly one primitive admission".into());
    }
    let invoked = !calls.opened_pins.is_empty() || !calls.search_top_ks.is_empty();
    drop(calls);
    if invoked {
        return Err("pure invalid phrase reached snapshot/search".into());
    }
    Ok(())
}

#[test]
fn l1_real_adapter_admits_primitives_before_language_empty_or_generation_open() -> TestResult {
    let dir = tempfile::tempdir()?;
    let absent_root = dir.path().join("unopened-lexical");
    let dispatcher = dispatcher_with_obs(
        Arc::new(quanta_index_lexical::LexicalAdapter::with_state_root(
            absent_root.clone(),
        )),
        Arc::new(RejectSemanticOpener),
        Arc::new(NoopQueryObsSink),
    )?;
    for manual in ["", "index:no "] {
        for language in [None, Some("rust")] {
            for (primitive, expected) in [
                ("...", SearchPlaneErrorCodeV2::LexTextQueryNoTokens),
                ("\"!!!\"", SearchPlaneErrorCodeV2::LexTextQueryNoTokens),
                ("content:!!!", SearchPlaneErrorCodeV2::LexTextQueryNoTokens),
                (
                    "repo.has.content(\"!!!\")",
                    SearchPlaneErrorCodeV2::LexTextQueryNoTokens,
                ),
                (
                    "file.has.content(\"!!!\")",
                    SearchPlaneErrorCodeV2::LexTextQueryNoTokens,
                ),
                (
                    "patterntype:regexp [",
                    SearchPlaneErrorCodeV2::LexRegexDialectParseError,
                ),
            ] {
                let query = format!("{manual}lang:python {primitive}");
                let request = TextQueryRequest::from(symbol_request(&query, language)?);
                let (code, message) = ipc_error_from(dispatcher.dispatch(
                    SearchPlaneQueryIpcRequest::Text(request),
                    &RequestBudgetV1::unbounded(),
                ))?;
                if code != expected {
                    return Err(
                        format!("{query}, {language:?}: {code} != {expected}: {message}").into(),
                    );
                }
            }
            let request = symbol_request(&format!("{manual}lang:python ..."), language)?;
            let (code, message) = ipc_error_from(dispatcher.dispatch(
                SearchPlaneQueryIpcRequest::Symbol(request),
                &RequestBudgetV1::unbounded(),
            ))?;
            if code != SearchPlaneErrorCodeV2::LexTextQueryNoTokens {
                return Err(format!("symbol primitive admission: {code}: {message}").into());
            }
        }
        let request = TextQueryRequest::from(symbol_request(
            &format!("{manual}lang:python needle"),
            Some("rust"),
        )?);
        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(request),
            &RequestBudgetV1::unbounded(),
        );
        let SearchPlaneQueryIpcResponse::Text(response) = response else {
            return Err(format!(
                "valid contradiction tried opening absent generation: {response:?}"
            )
            .into());
        };
        if !response.results.is_empty() || !response.explanation.engines_executed.is_empty() {
            return Err("logical empty acquired execution facts".into());
        }
    }
    if absent_root.exists() {
        return Err("pure validation materialized adapter state".into());
    }
    Ok(())
}

#[test]
fn structural_exact_all_callers_propagate_bounded_count_refusal() -> TestResult {
    use crate::query_dispatcher::tests::support::lexical::recording_lexical_candidate;
    use crate::query_dispatcher::tests::support::structural::{
        PatternRoutingStructuralProducer, ready_ledger_with_structural_boolean_chunks,
        structural_dispatcher_mixed,
    };

    // The instrumented port pins routing/error propagation. Native L1 tests
    // independently prove the complete-set port's refusal and positive controls.
    let dispatcher = structural_dispatcher_mixed(
        Arc::new(PatternRoutingStructuralProducer::new()),
        Arc::new(RecordingLexicalOpener {
            state: Arc::new(Mutex::new(RecordingLexicalState::default())),
            results: vec![recording_lexical_candidate("chunk-a")],
        }),
        ready_ledger_with_structural_boolean_chunks(),
    )?;
    for leaf in ["needle", "symbol.has.name(needle)"] {
        for count in ["", "count:all ", "count:1 "] {
            let text = format!("{count}{leaf} AND match {{ alpha }}");
            let request = quanta_index_contract::StructuralQueryRequest {
                text_query: TextQueryRequest::from(symbol_request(&text, None)?),
                cursor: None,
            };
            let response = dispatcher.dispatch(
                SearchPlaneQueryIpcRequest::Structural(request),
                &RequestBudgetV1::unbounded(),
            );
            if count == "count:1 " {
                let (code, message) = ipc_error_from(response)?;
                if code != SearchPlaneErrorCodeV2::LexFilterInvalidCount {
                    return Err(format!("{text}: wrong all-port refusal {code}: {message}").into());
                }
            } else if let SearchPlaneQueryIpcResponse::Structural(page) = response {
                let ids: Vec<_> = page
                    .results
                    .iter()
                    .map(|row| row.candidate_id.as_str())
                    .collect();
                if ids != ["chunk-a"] {
                    return Err(format!("{text}: complete-set control returned {ids:?}").into());
                }
            } else {
                return Err(format!(
                    "{text}: expected complete structural control, got {response:?}"
                )
                .into());
            }
        }
    }
    Ok(())
}

#[test]
fn language_empty_cannot_hide_unsupported_symbol_text_or_domain_override() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: Vec::new(),
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(NoopQueryObsSink),
    )?;
    for mode in ["", "index:no "] {
        for language in [None, Some("rust")] {
            for term in ["needle", "absent_term"] {
                for (projection, expected) in [
                    ("", SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo),
                    ("select:file ", SearchPlaneErrorCodeV2::InvalidRequest),
                ] {
                    let text = format!("{mode}lang:python {projection}patterntype:regexp {term}.*");
                    let request = symbol_request(&text, language)?;
                    let (code, message) = ipc_error_from(dispatcher.dispatch(
                        SearchPlaneQueryIpcRequest::Symbol(request),
                        &RequestBudgetV1::unbounded(),
                    ))?;
                    if code != expected {
                        return Err(format!("{text}: {code} != {expected}: {message}").into());
                    }
                }
                let text = format!("{mode}lang:python select:symbol patterntype:regexp {term}.*");
                let request = TextQueryRequest::from(symbol_request(&text, language)?);
                let (code, message) = ipc_error_from(dispatcher.dispatch(
                    SearchPlaneQueryIpcRequest::Text(request),
                    &RequestBudgetV1::unbounded(),
                ))?;
                if code != SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo {
                    return Err(format!("{text}: {code}: {message}").into());
                }
            }
        }
    }
    let calls = state.lock().map_err(|error| error.to_string())?;
    let invoked = !calls.opened_pins.is_empty()
        || !calls.symbol_top_ks.is_empty()
        || !calls.search_top_ks.is_empty();
    drop(calls);
    if invoked {
        return Err("unsupported Symbol text reached snapshot/search".into());
    }
    Ok(())
}

#[test]
fn valid_language_contradiction_has_no_executed_lane_or_backend_search() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: Vec::new(),
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(NoopQueryObsSink),
    )?;
    let request = symbol_request("lang:python needle", Some("rust"))?;
    let (symbols, execution) =
        dispatcher.symbol_with_execution(request.clone(), &RequestBudgetV1::unbounded())?;
    if !symbols.results.is_empty()
        || symbols.next_cursor.is_some()
        || !execution.executed_engines().is_empty()
        || symbols
            .window
            .coverage()
            .lanes()
            .iter()
            .any(quanta_index_contract::LaneTraceV1::executed)
    {
        return Err(format!("logical empty claimed Symbol execution: {symbols:?}").into());
    }
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest::from(request)),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::Text(text) = response else {
        return Err(format!("valid logical empty Text request refused: {response:?}").into());
    };
    if !text.results.is_empty()
        || text.next_cursor.is_some()
        || !text.explanation.engines_executed.is_empty()
        || text
            .window
            .coverage()
            .lanes()
            .iter()
            .any(quanta_index_contract::LaneTraceV1::executed)
    {
        return Err(format!("logical empty claimed Text execution: {text:?}").into());
    }
    let calls = state.lock().map_err(|error| error.to_string())?;
    // A future capability gate may open the immutable view, but it must not
    // execute backend search for the independently contradictory language set.
    let invoked = !calls.symbol_top_ks.is_empty() || !calls.search_top_ks.is_empty();
    drop(calls);
    if invoked {
        return Err("logical empty invoked backend search".into());
    }
    Ok(())
}

fn count_dispatcher(
    cardinality: u32,
    snippet_bytes: usize,
) -> Result<crate::query_dispatcher::dispatcher::SearchPlaneDispatcher, Box<dyn std::error::Error>>
{
    let results = (0..cardinality)
        .rev()
        .map(|id| {
            let mut row = crate::query_dispatcher::tests::support::common::candidate(
                &format!("symbol-{id:02}"),
                1.0,
            );
            row.snippet = "x".repeat(snippet_bytes);
            row
        })
        .collect();
    dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::new(Mutex::new(RecordingLexicalState::default())),
            results,
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(NoopQueryObsSink),
    )
}

fn walk_symbol_pages(
    dispatcher: &crate::query_dispatcher::dispatcher::SearchPlaneDispatcher,
    mut request: SymbolQueryRequest,
    cardinality: u32,
) -> TestResult {
    use quanta_index_contract::CandidateCountV1;
    let mut seen = Vec::new();
    let mut finished = false;
    for _ in 0..=cardinality {
        let (page, _) =
            dispatcher.symbol_with_execution(request.clone(), &RequestBudgetV1::unbounded())?;
        let remaining = u64::from(cardinality)
            .checked_sub(u64::try_from(seen.len())?)
            .ok_or("walk exceeded fixture cardinality")?;
        match page.window.candidate_count() {
            CandidateCountV1::Exact(total) if total != remaining => {
                return Err(format!("exact count {total} != fixture remaining {remaining}").into());
            }
            CandidateCountV1::AtLeast(lower) if lower > remaining => {
                return Err(
                    format!("lower bound {lower} exceeds fixture remaining {remaining}").into(),
                );
            }
            CandidateCountV1::Exact(_) | CandidateCountV1::AtLeast(_) => {}
        }
        let has_more = u64::try_from(page.results.len())? < remaining;
        if page.window.has_more() != Some(has_more) || page.next_cursor.is_some() != has_more {
            return Err(format!("window lost or invented continuation: {page:?}").into());
        }
        seen.extend(page.results.into_iter().map(|row| row.candidate_id));
        request.cursor = page.next_cursor;
        if request.cursor.is_none() {
            finished = true;
            break;
        }
    }
    let expected: Vec<_> = (0..cardinality)
        .map(|id| format!("symbol-{id:02}"))
        .collect();
    if !finished || seen != expected {
        return Err(format!("cursor walk differs from fixture: {seen:?} != {expected:?}").into());
    }
    Ok(())
}

#[test]
fn count_windows_preserve_exact_cardinality_and_complete_cursor_walks() -> TestResult {
    for cardinality in [0, 1, 3, 5, 6] {
        let dispatcher = count_dispatcher(cardinality, 8)?;
        for count in ["", "count:1 ", "count:3 ", "count:5 ", "count:all "] {
            for top_k in [1, 3, 5, 8] {
                let mut request = symbol_request(&format!("{count}needle"), None)?;
                request.top_k = top_k;
                walk_symbol_pages(&dispatcher, request, cardinality)?;
            }
        }
    }
    Ok(())
}

#[test]
fn cursor_walk_distinguishes_equal_paths_and_ids_in_different_sources() -> TestResult {
    use quanta_index_contract::{RepoId, RevisionId, SourceFileKey, SourceFileRevision};
    let mut results = Vec::new();
    for source in ["source-b", "source-a"] {
        for id in ["shared-1", "shared-0"] {
            let mut candidate = crate::query_dispatcher::tests::support::common::candidate(id, 1.0);
            candidate.source_repo_id = RepoId::new(source)?;
            candidate.source = Some(SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new(source)?,
                    repo_relative_path: candidate.repo_relative_path.clone(),
                },
                revision_id: RevisionId::new("source-revision")?,
                source_sha256: [1; 32],
            });
            results.push(candidate);
        }
    }
    let dispatcher = dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::new(Mutex::new(RecordingLexicalState::default())),
            results,
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(NoopQueryObsSink),
    )?;
    let expected = vec![
        ("source-a".to_string(), "shared-0".to_string()),
        ("source-a".to_string(), "shared-1".to_string()),
        ("source-b".to_string(), "shared-0".to_string()),
        ("source-b".to_string(), "shared-1".to_string()),
    ];
    for count in ["", "count:1 ", "count:all "] {
        for top_k in [1, 2] {
            let mut request = symbol_request(&format!("{count}needle"), None)?;
            request.top_k = top_k;
            let mut observed = Vec::new();
            for _ in 0..4 {
                let (page, _) = dispatcher
                    .symbol_with_execution(request.clone(), &RequestBudgetV1::unbounded())?;
                for candidate in page.results {
                    let source = candidate.source.ok_or("fixture lost source identity")?;
                    observed.push((
                        source.file.source_repo_id.as_str().to_string(),
                        candidate.candidate_id,
                    ));
                }
                request.cursor = page.next_cursor;
                if request.cursor.is_none() {
                    break;
                }
            }
            if request.cursor.is_some() || observed != expected {
                return Err(
                    format!("cross-source cursor omitted/repeated a row: {observed:?}").into(),
                );
            }
        }
    }
    Ok(())
}

#[test]
fn clipped_count_page_continues_from_last_retained_row_without_omissions() -> TestResult {
    use crate::query_dispatcher::response_budget::ResponsePayloadBudget;
    let dispatcher = count_dispatcher(5, 2048)?;
    let mut request = symbol_request("count:5 needle", None)?;
    request.top_k = 2;
    let (two_rows, _) =
        dispatcher.symbol_with_execution(request.clone(), &RequestBudgetV1::unbounded())?;
    let byte_cap = quanta_index_ipc::cbor_payload_len(&two_rows)?;
    let dispatcher = dispatcher.with_response_budget(ResponsePayloadBudget::new(byte_cap)?);
    request.top_k = 5;
    let (clipped, _) =
        dispatcher.symbol_with_execution(request.clone(), &RequestBudgetV1::unbounded())?;
    if clipped.results.is_empty()
        || clipped.results.len() >= 5
        || quanta_index_ipc::cbor_payload_len(&clipped)? > byte_cap
    {
        return Err(format!("expected a fitting strict prefix: {clipped:?}").into());
    }
    walk_symbol_pages(&dispatcher, request, 5)
}

#[test]
fn cursor_cannot_replay_into_a_new_language_contradiction() -> TestResult {
    let dispatcher = count_dispatcher(5, 8)?;
    let mut request = symbol_request("lang:rust needle", Some("rust"))?;
    let (page, _) =
        dispatcher.symbol_with_execution(request.clone(), &RequestBudgetV1::unbounded())?;
    request.cursor = Some(page.next_cursor.ok_or("fixture must continue")?);
    request.query_text = "lang:python needle".into();
    let error = dispatcher
        .symbol_with_execution(request, &RequestBudgetV1::unbounded())
        .err()
        .ok_or("changed logical plan accepted prior cursor")?;
    let (code, _) = error.into_search_plane_wire();
    if code != SearchPlaneErrorCodeV2::CursorContextMismatch {
        return Err(format!("wrong changed-plan refusal: {code}").into());
    }
    Ok(())
}

#[test]
fn sourcegraph_symbol_projection_conflict_uses_the_same_pure_admission() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let dispatcher = dispatcher_with_obs(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: Vec::new(),
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(NoopQueryObsSink),
    )?;
    for language in [None, Some("rust")] {
        for term in ["needle", "absent_term"] {
            let mut request = symbol_request(&format!("lang:python select:file {term}"), language)?;
            request.syntax = TextQuerySyntax::Sourcegraph;
            let (code, _) = ipc_error_from(dispatcher.dispatch(
                SearchPlaneQueryIpcRequest::Symbol(request),
                &RequestBudgetV1::unbounded(),
            ))?;
            if code != SearchPlaneErrorCodeV2::InvalidRequest {
                return Err(format!("Sourcegraph projection conflict: {code}").into());
            }
        }
    }
    let calls = state.lock().map_err(|error| error.to_string())?;
    let invoked = !calls.opened_pins.is_empty() || !calls.symbol_top_ks.is_empty();
    drop(calls);
    if invoked {
        return Err("Sourcegraph pure conflict reached snapshot/search".into());
    }
    Ok(())
}
