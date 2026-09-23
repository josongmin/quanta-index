use std::sync::{Arc, Mutex, RwLock};

use quanta_index_contract::{
    CurrentGenerationRequest, GenerationPin, GenerationSelector, HybridQueryRequest,
    ManifestGeneration, RepoId, RevisionId, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcResponse, SearchPlaneTrackKind, SemanticQueryRequest, TextQueryRequest,
    TextQuerySyntax,
};
use quanta_index_core::RequestBudgetV1;
use tempfile::tempdir;

use crate::observability::NoopQueryObsSink;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::selection::resolve_joint_active_selection;
use crate::query_dispatcher::semantic_query::{
    resolve_hybrid_request_selection, resolve_semantic_request_selection,
};
use crate::query_dispatcher::tests::support::common::{
    TestResult, corpus_generation, default_query_embedder, ipc_error_from, ready_ledger,
    test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::RejectLexicalOpener;
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapSnapshotPort;
use crate::query_dispatcher::tests::support::semantic::{
    FixedModelQueryEmbedder, RecordingSemanticOpener, RecordingSemanticState, RejectSemanticOpener,
    UnavailableTestQueryEmbedder, semantic_focus_request,
};
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;
use crate::{
    ActivationCatalog, Ledger, PreparedSearchCorpusGenerationV1, SEARCH_OWNED_SEMANTIC_DIMENSION,
    SnapshotRegistries, SnapshotRegistryPolicy,
};

fn require_joint_selection(condition: bool, message: &'static str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

#[test]
fn joint_active_selection_uses_one_composite_head_and_checks_explicit_pin() -> TestResult {
    let dir = tempdir()?;
    let catalog = ActivationCatalog::open(dir.keep())?;
    let repo = RepoId::new("joint-active-repo")?;
    let revision = RevisionId::new("joint-active-revision")?;
    let first = corpus_generation(
        repo.clone(),
        revision.clone(),
        ManifestGeneration::new(9),
        "joint-digest-9",
    )?;
    let first_prepared = PreparedSearchCorpusGenerationV1::new(first.clone(), None)?;
    let _first_activation =
        catalog.activate_prepared_search_corpus_generation_v1(&first_prepared)?;
    let selector = GenerationSelector::Active {
        repo_id: repo.clone(),
        revision_id: revision.clone(),
    };
    let pinned_first =
        GenerationPin::new(repo.clone(), revision.clone(), ManifestGeneration::new(9));
    let first_selection = resolve_joint_active_selection(
        &catalog,
        Some(&selector),
        Some(&selector),
        Some(&pinned_first),
        "hybrid",
    )?
    .ok_or("matching active selectors must resolve together")?;
    require_joint_selection(
        first_selection.pin == pinned_first,
        "wrong first active pin",
    )?;
    require_joint_selection(
        first_selection.expected_manifest_digest.as_deref() == Some("joint-digest-9"),
        "wrong first active digest",
    )?;

    let second = corpus_generation(
        repo,
        revision.clone(),
        ManifestGeneration::new(10),
        "joint-digest-10",
    )?;
    let _second_activation = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(second, Some(first))?,
    )?;
    let latest =
        resolve_joint_active_selection(&catalog, Some(&selector), Some(&selector), None, "hybrid")?
            .ok_or("active selectors must resolve after activation")?;
    require_joint_selection(
        latest.pin.manifest_generation == ManifestGeneration::new(10),
        "wrong advanced active generation",
    )?;
    require_joint_selection(
        latest.expected_manifest_digest.as_deref() == Some("joint-digest-10"),
        "wrong advanced active digest",
    )?;
    let text_scope = TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "needle".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(latest.pin.clone()),
        generation_selector: Some(selector.clone()),
        top_k: 5,
        cursor: None,
    };
    let semantic_request = SemanticQueryRequest {
        query_text: "semantic".to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(latest.pin.clone()),
        generation_selector: Some(selector.clone()),
        lexical_scope: Some(text_scope.clone()),
        top_k: 5,
    };
    let semantic_selection = resolve_semantic_request_selection(&catalog, &semantic_request)?;
    require_joint_selection(
        semantic_selection.pin == latest.pin
            && semantic_selection.expected_manifest_digest == latest.expected_manifest_digest,
        "semantic scope did not select the composite active head",
    )?;
    let hybrid_request = HybridQueryRequest {
        text_query: text_scope,
        semantic_query_text: "semantic".to_string(),
        generation: Some(latest.pin.clone()),
        generation_selector: Some(selector.clone()),
        top_k: 5,
    };
    let hybrid_selection = resolve_hybrid_request_selection(&catalog, &hybrid_request)?;
    require_joint_selection(
        hybrid_selection.pin == latest.pin
            && hybrid_selection.expected_manifest_digest == latest.expected_manifest_digest,
        "hybrid did not select the composite active head",
    )?;
    let mut stale_hybrid = hybrid_request;
    stale_hybrid.text_query.generation = Some(pinned_first.clone());
    require_joint_selection(
        resolve_hybrid_request_selection(&catalog, &stale_hybrid).is_err(),
        "hybrid accepted stale lexical pin",
    )?;
    require_joint_selection(
        resolve_joint_active_selection(
            &catalog,
            Some(&selector),
            Some(&selector),
            Some(&pinned_first),
            "hybrid",
        )
        .is_err(),
        "joint selection accepted stale lexical pin",
    )?;
    let foreign = GenerationSelector::Active {
        repo_id: RepoId::new("foreign-repo")?,
        revision_id: revision,
    };
    require_joint_selection(
        resolve_joint_active_selection(&catalog, Some(&selector), Some(&foreign), None, "hybrid")
            .is_err(),
        "joint selection accepted a foreign active pair",
    )?;
    Ok(())
}

#[test]
fn query_plane_resolves_only_catalog_active_generation() -> TestResult {
    let dir = tempdir()?;
    let activation_catalog = Arc::new(ActivationCatalog::open(dir.keep())?);
    let repo_id = RepoId::new("repo-map-ipc")?;
    let revision_id = RevisionId::new("rev-map-ipc")?;
    let active = corpus_generation(
        repo_id.clone(),
        revision_id.clone(),
        ManifestGeneration::new(9),
        "activation-digest-9",
    )?;
    let prepared = PreparedSearchCorpusGenerationV1::new(active, None)?;
    let _activation =
        activation_catalog.activate_prepared_search_corpus_generation_v1(&prepared)?;
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        Arc::clone(&activation_catalog),
    );
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::ResolveActiveGeneration(CurrentGenerationRequest {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track: SearchPlaneTrackKind::Lexical,
        }),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(snapshot) = response else {
        return Err(format!("expected catalog snapshot, got {response:?}").into());
    };
    if snapshot.repo_id != repo_id
        || snapshot.revision_id != revision_id
        || snapshot.track != SearchPlaneTrackKind::Lexical
        || snapshot.manifest_generation != ManifestGeneration::new(9)
        || snapshot.manifest_digest != "activation-digest-9"
    {
        return Err(format!("wrong catalog snapshot: {snapshot:?}").into());
    }
    let selection = resolve_semantic_request_selection(
        activation_catalog.as_ref(),
        &SemanticQueryRequest {
            query_text: "semantic text".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(
                repo_id.clone(),
                revision_id.clone(),
                ManifestGeneration::new(9),
            )),
            generation_selector: Some(GenerationSelector::Active {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
            }),
            lexical_scope: None,
            top_k: 5,
        },
    )?;
    if selection.expected_manifest_digest.as_deref() != Some("activation-digest-9") {
        return Err("pin plus Active must preserve semantic manifest authority".into());
    }
    let next = corpus_generation(
        repo_id.clone(),
        revision_id.clone(),
        ManifestGeneration::new(10),
        "activation-digest-10",
    )?;
    let next_prepared =
        PreparedSearchCorpusGenerationV1::new(next, Some(prepared.candidate().clone()))?;
    let _next_activation =
        activation_catalog.activate_prepared_search_corpus_generation_v1(&next_prepared)?;
    let stale = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(
                repo_id.clone(),
                revision_id.clone(),
                ManifestGeneration::new(9),
            )),
            generation_selector: Some(GenerationSelector::Active {
                repo_id,
                revision_id: revision_id.clone(),
            }),
            top_k: 5,
            cursor: None,
        }),
        &RequestBudgetV1::unbounded(),
    );
    let (code, _) = ipc_error_from(stale).map_err(Box::<dyn std::error::Error>::from)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest {
        return Err(format!("stale Active+pin must be INVALID_REQUEST, got {code}").into());
    }
    let missing = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::ResolveActiveGeneration(CurrentGenerationRequest {
            repo_id: RepoId::new("missing")?,
            revision_id,
            track: SearchPlaneTrackKind::Lexical,
        }),
        &RequestBudgetV1::unbounded(),
    );
    let (code, _) = ipc_error_from(missing).map_err(Box::<dyn std::error::Error>::from)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::NotReady {
        return Err(format!("missing active entry must be NOT_READY, got {code}").into());
    }
    Ok(())
}

// CASE-COVERS: query-time semantic model-identity enforcement (SEM_MODEL_MISMATCH).
#[test]
fn ensure_query_model_matches_index_v1_fails_closed_on_model_drift() {
    use crate::query_dispatcher::semantic_query::ensure_query_model_matches_index_v1;
    use quanta_index_contract::lex::LexicalErrorCode;
    use quanta_index_core::CoreError;

    let expect_model_mismatch = |err: CoreError| {
        #[expect(
            clippy::wildcard_enum_match_arm,
            reason = "the test intentionally rejects every non-typed model-mismatch error"
        )]
        match err {
            CoreError::Typed { code, .. } => assert_eq!(
                code,
                quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    LexicalErrorCode::SemModelMismatch
                ),
                "model drift must surface SEM_MODEL_MISMATCH"
            ),
            other => panic!("expected SemModelMismatch typed error, got {other:?}"),
        }
    };

    // POSITIVE: identical model id + revision => Ok (matching path proceeds).
    assert!(
        ensure_query_model_matches_index_v1(
            "search-owned-hash-text-v1",
            "r1",
            "search-owned-hash-text-v1",
            Some("r1"),
            "semantic",
        )
        .is_ok()
    );
    assert!(ensure_query_model_matches_index_v1("m", "2", "m", Some("2"), "hybrid").is_ok());

    // ORIGINAL TRIGGER: same dimension is irrelevant — a different model id
    // (the future same-dim engine swap) MUST fail closed, not silently rank.
    expect_model_mismatch(
        ensure_query_model_matches_index_v1(
            "neural-768-v2",
            "r1",
            "search-owned-hash-text-v1",
            Some("r1"),
            "semantic",
        )
        .unwrap_err(),
    );

    // EDGE: same id, revision drift must also fail closed (QI-BB-028).
    expect_model_mismatch(
        ensure_query_model_matches_index_v1("m", "1", "m", Some("2"), "hybrid seed").unwrap_err(),
    );

    // CORNER: an index sealed without a revision cannot be compared and
    // is refused rather than assumed to match.
    expect_model_mismatch(
        ensure_query_model_matches_index_v1("m", "1", "m", None, "semantic").unwrap_err(),
    );
}

#[test]
fn semantic_dispatch_embeds_query_text() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&state),
        }),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "focus alpha".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(
                RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev-map-ipc")
                    .expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(9),
            )),
            generation_selector: None,
            lexical_scope: None,
            top_k: 3,
        }),
        &RequestBudgetV1::unbounded(),
    );

    match response {
        SearchPlaneQueryIpcResponse::Semantic(semantic) => {
            if semantic.results.len() != 1 {
                return Err(format!(
                    "expected one semantic result, got {}",
                    semantic.results.len()
                )
                .into());
            }
        }
        other @ (SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::Error(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | quanta_index_contract::SearchPlaneQueryIpcResponse::RuntimeMetadata(_)) => {
            return Err(format!("expected Semantic response, got {other:?}").into());
        }
    }

    let (search_vectors, scoped_vectors) = {
        let guard = state
            .lock()
            .map_err(|err| format!("semantic state poisoned: {err}"))?;
        (guard.search_vectors.clone(), guard.scoped_vectors.clone())
    };
    let expected = default_query_embedder().embed_query(
        "focus alpha",
        &quanta_index_core::RequestBudgetV1::unbounded(),
    )?;
    if search_vectors.as_slice() != [expected] {
        return Err(format!("unexpected semantic vectors: {search_vectors:?}").into());
    }
    if !scoped_vectors.is_empty() {
        return Err(format!("unexpected scoped vectors: {scoped_vectors:?}").into());
    }
    Ok(())
}

// CASE-COVERS: query-time model gate WIRING — a model-id drift at the dispatcher
// boundary fails closed BEFORE the searcher runs (proves the gate is invoked at
// the call site, not just that the helper logic is correct).
#[test]
fn semantic_dispatch_rejects_model_identity_drift_v1() -> TestResult {
    let state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let dispatcher = SearchPlaneDispatcher::new_with_obs(
        Arc::new(RejectLexicalOpener),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&state),
        }),
        SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
        Arc::new(FixedModelQueryEmbedder {
            model_id: "neural-768-v2",
            model_revision: "r1",
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
        }),
        Arc::new(NoopQueryObsSink),
    );

    match dispatcher.semantic(&semantic_focus_request(), &RequestBudgetV1::unbounded()) {
        Err(quanta_index_core::CoreError::Typed { code, .. }) => {
            let expected = quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                quanta_index_contract::lex::LexicalErrorCode::SemModelMismatch,
            );
            if code != expected {
                return Err(format!("expected SEM_MODEL_MISMATCH, got code {code}").into());
            }
        }
        other => {
            return Err(format!("model drift must fail closed, got {other:?}").into());
        }
    }

    // The gate runs AFTER embed but BEFORE the searcher: no vectors reach the
    // searcher, so a mismatched-model query never produces a (garbage) ranking.
    let guard = state
        .lock()
        .map_err(|err| format!("semantic state poisoned: {err}"))?;
    if !guard.search_vectors.is_empty() || !guard.scoped_vectors.is_empty() {
        return Err(format!(
            "model drift must reject before invoking the searcher; search={:?} scoped={:?}",
            guard.search_vectors, guard.scoped_vectors
        )
        .into());
    }
    drop(guard);
    Ok(())
}

// CASE-COVERS: embed-before-model-gate ORDER — an unavailable embedder surfaces
// SEM_PROVIDER_UNAVAILABLE (from embed), NOT SEM_MODEL_MISMATCH, proving the gate
// is placed after embed so the provider-unavailable rail keeps its own error.
#[test]
fn semantic_dispatch_unavailable_embedder_keeps_provider_error_before_model_gate_v1() -> TestResult
{
    let state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let dispatcher = SearchPlaneDispatcher::new_with_obs(
        Arc::new(RejectLexicalOpener),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&state),
        }),
        SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ready_ledger(),
        test_activation_catalog()?,
        Arc::new(UnavailableTestQueryEmbedder),
        Arc::new(NoopQueryObsSink),
    );

    match dispatcher.semantic(&semantic_focus_request(), &RequestBudgetV1::unbounded()) {
        Err(quanta_index_core::CoreError::Typed { code, .. }) => {
            let provider = quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                quanta_index_contract::lex::LexicalErrorCode::SemProviderUnavailable,
            );
            if code != provider {
                return Err(format!(
                    "unavailable embedder must surface SEM_PROVIDER_UNAVAILABLE (embed precedes the model gate), got {code}"
                )
                .into());
            }
        }
        other => {
            return Err(format!("unavailable embedder must fail closed, got {other:?}").into());
        }
    }
    Ok(())
}

#[test]
fn semantic_dispatch_rejects_active_digest_mismatch_with_exact_code() -> TestResult {
    let dir = tempdir()?;
    let activation_catalog = Arc::new(ActivationCatalog::open(dir.keep())?);
    let active = corpus_generation(
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
        ManifestGeneration::new(9),
        "activation-digest-9",
    )?;
    let prepared = PreparedSearchCorpusGenerationV1::new(active, None)?;
    let activation = activation_catalog.activate_prepared_search_corpus_generation_v1(&prepared)?;
    if activation.active.manifest_generation() != ManifestGeneration::new(9) {
        return Err("expected active composite generation 9".into());
    }
    let mut ledger = Ledger::default();
    let repo_id =
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy");
    let revision_id =
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy");
    ledger.record_track_materialized(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Semantic,
        ManifestGeneration::new(9),
        Some("observed-digest-9"),
    );
    ledger.record_track_seal_with_digest(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Semantic,
        ManifestGeneration::new(9),
        "observed-digest-9",
    );
    ledger.record_historically_sealed_search_corpus(
        &repo_id,
        &revision_id,
        ManifestGeneration::new(9),
        "observed-digest-9",
    );
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        Arc::new(RwLock::new(ledger)),
        activation_catalog,
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "focus alpha".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: Some(GenerationSelector::Active {
                repo_id: RepoId::new("repo-map-ipc")
                    .expect("static fixture ID satisfies canonical policy"),
                revision_id: RevisionId::new("rev-map-ipc")
                    .expect("static fixture ID satisfies canonical policy"),
            }),
            lexical_scope: None,
            top_k: 3,
        }),
        &RequestBudgetV1::unbounded(),
    );

    let (code, message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::SemanticManifestDigestMismatch {
        return Err(format!("unexpected semantic mismatch code: {code}").into());
    }
    if !message.contains("expected=activation-digest-9")
        || !message.contains("observed=observed-digest-9")
    {
        return Err(format!("unexpected semantic mismatch message: {message}").into());
    }
    Ok(())
}

#[test]
fn semantic_dispatch_rejects_unsealed_pinned_generation_with_exact_code() -> TestResult {
    let mut ledger = Ledger::default();
    let repo_id =
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy");
    let revision_id =
        RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy");
    ledger.record_track_materialized(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Semantic,
        ManifestGeneration::new(9),
        Some("manifest-digest-9"),
    );
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RejectLexicalOpener),
        Arc::new(RejectSemanticOpener),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        Arc::new(RwLock::new(ledger)),
        test_activation_catalog()?,
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: "focus alpha".to_string(),
            constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
            generation: Some(GenerationPin::new(
                RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev-map-ipc")
                    .expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(9),
            )),
            generation_selector: None,
            lexical_scope: None,
            top_k: 3,
        }),
        &RequestBudgetV1::unbounded(),
    );

    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::SemanticGenerationNotSealed {
        return Err(format!("unexpected semantic unsealed code: {code}").into());
    }
    Ok(())
}
