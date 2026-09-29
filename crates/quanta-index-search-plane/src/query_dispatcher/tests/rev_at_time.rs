use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    GenerationPin, LqFilter, ManifestGeneration, RepoId, RevisionId, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::RequestBudgetV1;

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::errors::ERR_HISTORY_INVALID_TIMEREF;
use crate::query_dispatcher::tests::support::common::{
    TestResult, activation_catalog_with_generations, candidate, corpus_generation, ipc_error_from,
    test_activation_catalog,
};
use crate::query_dispatcher::tests::support::history::ledger_with_rev_at_time_history;
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState, StubLexicalOpener,
};
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapSnapshotPort;
use crate::query_dispatcher::tests::support::semantic::RejectSemanticOpener;
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

#[test]
fn lexical_dispatch_rebinds_rev_at_time_to_reachable_ancestor() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let activation_catalog = activation_catalog_with_generations(&[
        corpus_generation(
            RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("1111111111111111111111111111111111111111")
                .expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(7),
            "ancestor-lex",
        )?,
        corpus_generation(
            RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("2222222222222222222222222222222222222222")
                .expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(9),
            "head-lex",
        )?,
    ])?;
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&state),
            results: vec![candidate("ancestor-hit", 1.0)],
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ledger_with_rev_at_time_history()?,
        activation_catalog,
    );

    let request = TextQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "rev:at.time(1970-01-01T00:00:00.150Z) foo".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(GenerationPin::new(
            RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("2222222222222222222222222222222222222222")
                .expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(9),
        )),
        generation_selector: None,
        top_k: 5,
        cursor: None,
    };
    let expected_ancestor = GenerationPin::new(
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("1111111111111111111111111111111111111111")
            .expect("static fixture ID satisfies canonical policy"),
        ManifestGeneration::new(7),
    );
    let resolved = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(request.clone()),
        &RequestBudgetV1::unbounded(),
    );
    if !matches!(
        &resolved,
        SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(pin) if pin == &expected_ancestor
    ) {
        return Err(format!("expected ancestor resolution, got {resolved:?}").into());
    }
    let mut before_history = request.clone();
    before_history.query_text = "rev:at.time(1970-01-01T00:00:00.050Z) foo".to_string();
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(request),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Text(text) => {
            if text.generation != expected_ancestor {
                return Err(format!("unexpected rebound generation: {:?}", text.generation).into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)) => {
            return Err(format!("expected Text response, got {other:?}").into());
        }
    }

    let empty_resolution = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(before_history.clone()),
        &RequestBudgetV1::unbounded(),
    );
    if !matches!(
        &empty_resolution,
        SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(pin)
            if pin.manifest_generation == ManifestGeneration::new(9)
                && pin.revision_id.as_str() == "2222222222222222222222222222222222222222"
    ) {
        return Err(format!("expected head resolution, got {empty_resolution:?}").into());
    }
    let empty_response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(before_history),
        &RequestBudgetV1::unbounded(),
    );
    if !matches!(
        &empty_response,
        SearchPlaneQueryIpcResponse::Text(text)
            if text.results.is_empty()
                && text.generation.manifest_generation == ManifestGeneration::new(9)
    ) {
        return Err(format!("expected empty head response, got {empty_response:?}").into());
    }

    let guard = state
        .lock()
        .map_err(|err| format!("lexical state poisoned: {err}"))?;
    if guard.opened_pins.as_slice()
        != [(
            RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("1111111111111111111111111111111111111111")
                .expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(7),
        )]
    {
        return Err(format!("unexpected opened pins: {:?}", guard.opened_pins).into());
    }
    let Some(last_query) = guard.searched_queries.last() else {
        return Err("expected search query after rev:at.time rebind".into());
    };
    if last_query
        .filters
        .iter()
        .any(|filter| matches!(filter, LqFilter::Rev { .. }))
    {
        return Err(format!(
            "rev filters must be consumed before lexical execution: {last_query:?}"
        )
        .into());
    }
    drop(guard);
    Ok(())
}

#[test]
fn lexical_dispatch_rejects_rev_at_time_invalid_timeref() -> TestResult {
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(StubLexicalOpener {
            results: Vec::new(),
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ledger_with_rev_at_time_history()?,
        test_activation_catalog()?,
    );

    let request = TextQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: "rev:at.time(definitely-not-a-timeref) foo".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(GenerationPin::new(
            RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("2222222222222222222222222222222222222222")
                .expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(9),
        )),
        generation_selector: None,
        top_k: 5,
        cursor: None,
    };
    for payload in [
        SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(request.clone()),
        SearchPlaneQueryIpcRequest::Text(request),
    ] {
        let response = dispatcher.dispatch(payload, &RequestBudgetV1::unbounded());
        let (code, _message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != ERR_HISTORY_INVALID_TIMEREF {
            return Err(format!("expected {ERR_HISTORY_INVALID_TIMEREF}, got {code}").into());
        }
    }
    Ok(())
}

#[test]
fn lexical_dispatch_rejects_rev_at_time_when_rebound_generation_is_unactivated() -> TestResult {
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(StubLexicalOpener {
            results: Vec::new(),
        }),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ledger_with_rev_at_time_history()?,
        activation_catalog_with_generations(&[corpus_generation(
            RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("2222222222222222222222222222222222222222")
                .expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(9),
            "head-lex",
        )?])?,
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Sourcegraph,
            query_text: "rev:at.time(1970-01-01T00:00:00.150Z) foo".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(
                RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("2222222222222222222222222222222222222222")
                    .expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(9),
            )),
            generation_selector: None,
            top_k: 5,
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );

    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::NotReady {
        return Err(format!("expected NOT_READY, got {code}").into());
    }
    if !message.contains("no active Lexical generation") {
        return Err(format!("unexpected rev:at.time not-ready message: {message}").into());
    }
    Ok(())
}
