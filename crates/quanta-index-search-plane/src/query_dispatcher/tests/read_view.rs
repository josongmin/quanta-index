//! One `QueryReadViewV2` per request (plan §5.6 / §7.1).
//!
//! A route opens exactly the domains its plan declares, a required domain
//! the generation lacks is refused typed before any lane executes, and
//! the view never mixes generations.
//!
//! The oracles are the recording doubles' call logs (which handles were
//! opened, which lanes ran) and ledgers that hold *no* auxiliary state at
//! all: a route that served from such a ledger provably demanded no
//! auxiliary domain.

use std::sync::{Arc, Barrier, Mutex, RwLock};
use std::time::Instant;

use quanta_index_contract::{
    AuxEpochV1, GenerationPin, GenerationSelector, HybridQueryRequest, ManifestGeneration,
    PlannerStage, RepoId, RevisionId, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse,
    SearchPlaneTrackKind, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_core::{
    CoreError, READ_VIEW_DOMAIN_UNDECLARED_CODE, READ_VIEW_GENERATION_MIX_CODE, ReadDomainV1,
    RepoMetadataAuthoritiesV1, RepoMetadataAuthorityV1, RequestBudgetV1, RequiredDomainsV1,
};

use crate::Ledger;
use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::query_dispatcher::errors::{
    ERR_HISTORY_GENERATION_NOT_READY, ERR_HISTORY_PRODUCER_UNAVAILABLE,
};
use crate::query_dispatcher::read_view::{ReadViewRequestV1, assemble_for_test};
use crate::query_dispatcher::selection::resolve_optional_selection;
use crate::query_dispatcher::tests::support::common::{
    TestResult, activation_catalog_with_generations, candidate, corpus_generation, ipc_error_from,
    ready_ledger, ready_pin, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState, StubLexicalSearcher,
};
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapSnapshotPort;
use crate::query_dispatcher::tests::support::semantic::{
    RecordingSemanticOpener, RecordingSemanticState,
};
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;
use crate::{PreparedSearchCorpusGenerationV1, SearchCorpusHistoryRetentionReceiptV1};

/// A dispatcher over recording lexical and semantic openers and `ledger`.
fn recording_dispatcher(
    lexical_state: &Arc<Mutex<RecordingLexicalState>>,
    semantic_state: &Arc<Mutex<RecordingSemanticState>>,
    ledger: Arc<RwLock<Ledger>>,
) -> Result<SearchPlaneDispatcher, Box<dyn std::error::Error>> {
    Ok(SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(lexical_state),
            results: vec![candidate("alpha", 1.0)],
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(semantic_state),
        }),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        ledger,
        test_activation_catalog()?,
    ))
}

fn text_request(query_text: &str, pin: GenerationPin) -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Sourcegraph,
        query_text: query_text.to_string(),
        constraints: quanta_index_contract::QueryConstraintSetV1::unconstrained(),
        generation: Some(pin),
        generation_selector: None,
        top_k: 5,
        cursor: None,
    }
}

/// The catalog selection and the ledger serving check are separate. Force
/// the exact G1 selection -> G2/G3 activation -> G1 retirement schedule
/// without relying on a scheduler race. This fixes the current refusal
/// boundary; it does not assert that a selected G1 is guaranteed to serve.
#[test]
fn active_selection_reaped_before_view_acquisition_refuses_without_opening_g1() -> TestResult {
    let g1 = ready_pin();
    let catalog = test_activation_catalog()?;
    let g1_generation = corpus_generation(
        g1.repo_id.clone(),
        g1.revision_id.clone(),
        g1.manifest_generation,
        "manifest-digest-9",
    )?;
    let g1_head = catalog.activate_prepared_search_corpus_generation_v1(
        &PreparedSearchCorpusGenerationV1::new(g1_generation, None)?,
    )?;
    let selector = GenerationSelector::Active {
        repo_id: g1.repo_id.clone(),
        revision_id: g1.revision_id.clone(),
    };
    let selected = resolve_optional_selection(
        &catalog,
        None,
        Some(&selector),
        SearchPlaneTrackKind::Lexical,
        "lexical",
    )?
    .ok_or("active selection returned no generation")?;
    assert_eq!(selected, g1);

    let ledger = ready_ledger();
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&lexical_state),
            results: vec![candidate("alpha", 1.0)],
        }),
        Arc::new(RecordingSemanticOpener {
            state: semantic_state,
        }),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        Arc::clone(&ledger),
        Arc::clone(&catalog),
    );
    let release_retention = Arc::new(Barrier::new(2));
    let retention_done = Arc::new(Barrier::new(2));
    let mutation = {
        let catalog = Arc::clone(&catalog);
        let ledger = Arc::clone(&ledger);
        let repo = g1.repo_id.clone();
        let revision = g1.revision_id.clone();
        let release_retention = Arc::clone(&release_retention);
        let retention_done = Arc::clone(&retention_done);
        std::thread::spawn(move || -> Result<(), String> {
            release_retention.wait();
            let mut previous = g1_head.active;
            for generation in [10, 11] {
                let next = corpus_generation(
                    repo.clone(),
                    revision.clone(),
                    ManifestGeneration::new(generation),
                    &format!("manifest-digest-{generation}"),
                )
                .map_err(|error| error.to_string())?;
                previous = catalog
                    .activate_prepared_search_corpus_generation_v1(
                        &PreparedSearchCorpusGenerationV1::new(next, Some(previous))
                            .map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())?
                    .active;
            }
            let receipt = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
                &repo,
                &revision,
                [ManifestGeneration::new(10), ManifestGeneration::new(11)],
            );
            ledger
                .write()
                .map_err(|error| error.to_string())?
                .apply_search_corpus_history_retention_receipt_v1(
                    &repo,
                    &revision,
                    ManifestGeneration::new(11),
                    &receipt,
                )
                .map_err(|error| error.to_string())?;
            retention_done.wait();
            Ok(())
        })
    };
    release_retention.wait();
    // Never wait forever if the mutation failed before signalling the second
    // barrier: join its terminal result before trying to acquire the view.
    // The completion barrier is only reached on success; use the thread's
    // completion itself as the second deterministic rendezvous.
    let _ = retention_done;
    mutation.join().map_err(|_| "retention thread panicked")??;
    let request = ReadViewRequestV1::new(
        "lexical",
        &selected,
        RequiredDomainsV1::of(ReadDomainV1::LexicalTrack),
    );
    let refusal = dispatcher
        .acquire_read_view(&request, &RequestBudgetV1::unbounded())
        .err()
        .ok_or("reaped G1 unexpectedly acquired a read view")?;
    if !matches!(refusal, CoreError::Typed { .. } | CoreError::NotReady(_)) {
        return Err(format!("unexpected reaped-generation refusal: {refusal:?}").into());
    }
    let opened = lexical_state.lock().map_err(|error| error.to_string())?;
    if !opened.opened_pins.is_empty() {
        return Err("retired G1 reached the lexical opener".into());
    }
    Ok(())
}

/// What the doubles recorded: lexical opens, lexical searches, semantic
/// opens, semantic searches.
struct LaneTally {
    lexical_opens: usize,
    lexical_searches: usize,
    semantic_opens: usize,
    semantic_searches: usize,
}

fn tally(
    lexical_state: &Arc<Mutex<RecordingLexicalState>>,
    semantic_state: &Arc<Mutex<RecordingSemanticState>>,
) -> Result<LaneTally, Box<dyn std::error::Error>> {
    let lexical = lexical_state
        .lock()
        .map_err(|err| format!("lexical state poisoned: {err}"))?;
    let semantic = semantic_state
        .lock()
        .map_err(|err| format!("semantic state poisoned: {err}"))?;
    Ok(LaneTally {
        lexical_opens: lexical.opened_pins.len(),
        lexical_searches: lexical
            .search_top_ks
            .len()
            .saturating_add(lexical.symbol_top_ks.len()),
        semantic_opens: semantic.cluster_membership_opened_pins.len(),
        semantic_searches: semantic
            .search_vectors
            .len()
            .saturating_add(semantic.scoped_vectors.len())
            .saturating_add(semantic.search_hit_vectors.len()),
    })
}

/// A plain keyword query declares the lexical track only: it opens the
/// lexical handle once, never the semantic one, and it serves from a
/// ledger that holds no auxiliary authority at all.
#[test]
fn a_plain_lexical_query_opens_the_lexical_snapshot_only() -> TestResult {
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let dispatcher = recording_dispatcher(&lexical_state, &semantic_state, ready_ledger())?;

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(text_request("needle", ready_pin())),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::Text(page) = response else {
        return Err(format!("a plain lexical query is served, got {response:?}").into());
    };
    if page.results.len() != 1 {
        return Err(format!("the stub's one hit is served: {:?}", page.results).into());
    }
    let tally = tally(&lexical_state, &semantic_state)?;
    if tally.lexical_opens != 1 || tally.lexical_searches != 1 {
        return Err(format!(
            "one lexical open and one search, got opens={} searches={}",
            tally.lexical_opens, tally.lexical_searches
        )
        .into());
    }
    if tally.semantic_opens != 0 || tally.semantic_searches != 0 {
        return Err(format!(
            "the semantic track is never opened for a lexical plan: opens={} searches={}",
            tally.semantic_opens, tally.semantic_searches
        )
        .into());
    }
    Ok(())
}

/// A hybrid query declares both tracks: each handle is opened exactly
/// once, and the response trace names the view's domains and epochs.
#[test]
fn a_hybrid_query_opens_both_tracks_once_and_names_them_in_its_trace() -> TestResult {
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let dispatcher = recording_dispatcher(&lexical_state, &semantic_state, ready_ledger())?;

    let pin = ready_pin();
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: text_request("needle", pin.clone()),
            semantic_query_text: "needle".to_string(),
            generation: Some(pin),
            generation_selector: None,
            top_k: 5,
        }),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::Hybrid(hybrid) = response else {
        return Err(format!("a hybrid query is served, got {response:?}").into());
    };
    let tally = tally(&lexical_state, &semantic_state)?;
    if tally.lexical_opens != 1 || tally.semantic_opens != 1 {
        return Err(format!(
            "each track is opened once: lexical={} semantic={}",
            tally.lexical_opens, tally.semantic_opens
        )
        .into());
    }
    let details: Vec<&str> = hybrid
        .explanation
        .planner_trace
        .iter()
        .filter(|entry| entry.stage == PlannerStage::Plan)
        .map(|entry| entry.detail.as_str())
        .collect();
    let expected_head = [
        "read_view.domains=lexical,semantic",
        "read_view.epochs=-",
        "read_view.pin=repo-map-ipc@rev-map-ipc#9",
        "read_view.lexical_artifact=manifest-digest-9",
        "read_view.normalizer=2.0",
        "read_view.semantic_artifact=manifest-digest-9",
    ];
    if details.get(..expected_head.len()) != Some(&expected_head[..]) {
        return Err(format!("the trace opens with the view's identity: {details:?}").into());
    }
    if !details
        .iter()
        .any(|detail| detail.starts_with("read_view.profile="))
    {
        return Err(format!("the trace names the semantic profile: {details:?}").into());
    }
    Ok(())
}

#[test]
fn lexical_read_view_refuses_an_opened_handle_with_a_different_seal() -> TestResult {
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState {
        manifest_digest: Some("different-physical-seal".to_string()),
        ..RecordingLexicalState::default()
    }));
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let dispatcher = recording_dispatcher(&lexical_state, &semantic_state, ready_ledger())?;
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(text_request("needle", ready_pin())),
        &RequestBudgetV1::unbounded(),
    );
    let (code, _message) = ipc_error_from(response)?;
    if code != quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch {
        return Err(format!("wrong lexical identity refusal: {code:?}").into());
    }
    let state = lexical_state
        .lock()
        .map_err(|error| format!("lexical state poisoned: {error}"))?;
    if state.opened_pins.len() != 1 || !state.search_top_ks.is_empty() {
        return Err("lexical identity refusal did not stop before search".into());
    }
    drop(state);
    Ok(())
}

#[test]
fn semantic_read_view_refuses_an_opened_handle_with_a_different_seal() -> TestResult {
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState {
        manifest_digest: Some("different-physical-seal".to_string()),
        ..RecordingSemanticState::default()
    }));
    let dispatcher = recording_dispatcher(&lexical_state, &semantic_state, ready_ledger())?;
    let pin = ready_pin();
    let request = ReadViewRequestV1::new(
        "semantic",
        &pin,
        RequiredDomainsV1::of(ReadDomainV1::SemanticTrack),
    );
    let Err(error) = dispatcher.acquire_read_view(&request, &RequestBudgetV1::unbounded()) else {
        return Err("a handle with a different physical seal was admitted".into());
    };
    if !matches!(
        &error,
        CoreError::Typed { code, message }
            if *code == quanta_index_contract::SearchPlaneErrorCodeV2::SemanticManifestDigestMismatch
                && message.contains("expected=manifest-digest-9")
                && message.contains("observed=different-physical-seal")
    ) {
        return Err(format!("wrong semantic identity refusal: {error:?}").into());
    }
    let state = semantic_state
        .lock()
        .map_err(|error| format!("semantic state poisoned: {error}"))?;
    if state.cluster_membership_opened_pins.len() != 1 || !state.search_vectors.is_empty() {
        return Err("semantic identity refusal did not stop before search".into());
    }
    drop(state);
    Ok(())
}

/// The lexical fixture generation of `ledger_with_lexical_only`: sealed
/// lexical track, no history authority.
fn lexical_only_ledger() -> Arc<RwLock<Ledger>> {
    let mut ledger = Ledger::default();
    let repo_id =
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy");
    let revision_id = RevisionId::new("2222222222222222222222222222222222222222")
        .expect("static fixture ID satisfies canonical policy");
    ledger.record_track_materialized(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Lexical,
        ManifestGeneration::new(9),
        None,
    );
    ledger.record_track_seal(
        &repo_id,
        &revision_id,
        SearchPlaneTrackKind::Lexical,
        ManifestGeneration::new(9),
    );
    ledger.record_historically_sealed_search_corpus(
        &repo_id,
        &revision_id,
        ManifestGeneration::new(9),
        "manifest-digest-9",
    );
    Arc::new(RwLock::new(ledger))
}

/// A plan that reads the history authority on a generation that has
/// none is refused with the history domain's code before the lexical
/// handle is opened, let alone searched.
#[test]
fn a_history_read_on_a_generation_without_history_is_refused_before_any_lane() -> TestResult {
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let pin = GenerationPin::new(
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("2222222222222222222222222222222222222222")
            .expect("static fixture ID satisfies canonical policy"),
        ManifestGeneration::new(9),
    );
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&lexical_state),
            results: vec![candidate("alpha", 1.0)],
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&semantic_state),
        }),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        lexical_only_ledger(),
        activation_catalog_with_generations(&[corpus_generation(
            pin.repo_id.clone(),
            pin.revision_id.clone(),
            pin.manifest_generation,
            "head-lex",
        )?])?,
    );

    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(text_request(
            "rev:at.time(1970-01-01T00:00:00.150Z) needle",
            pin.clone(),
        )),
        &RequestBudgetV1::unbounded(),
    );
    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    // The lexical track is materialized at the pin, so the absent history
    // is the producer's, not a not-yet-materialized generation.
    if code != ERR_HISTORY_PRODUCER_UNAVAILABLE {
        return Err(format!("expected {ERR_HISTORY_PRODUCER_UNAVAILABLE}, got {code}").into());
    }
    let tally = tally(&lexical_state, &semantic_state)?;
    if tally.lexical_opens != 0 || tally.lexical_searches != 0 {
        return Err(format!(
            "no lane runs when a required domain is missing: opens={} searches={}",
            tally.lexical_opens, tally.lexical_searches
        )
        .into());
    }

    // The same query serves once the plan no longer reads history.
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(text_request("needle", pin)),
        &RequestBudgetV1::unbounded(),
    );
    if !matches!(response, SearchPlaneQueryIpcResponse::Text(_)) {
        return Err(format!("the plain query serves: {response:?}").into());
    }
    Ok(())
}

/// A generation the lexical track has not reached yet answers the
/// not-yet-materialized history code.
#[test]
fn a_history_read_ahead_of_the_lexical_track_is_not_ready() -> TestResult {
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState::default()));
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let pin = GenerationPin::new(
        RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
        RevisionId::new("2222222222222222222222222222222222222222")
            .expect("static fixture ID satisfies canonical policy"),
        ManifestGeneration::new(12),
    );
    let dispatcher = SearchPlaneDispatcher::new(
        Arc::new(RecordingLexicalOpener {
            state: Arc::clone(&lexical_state),
            results: Vec::new(),
        }),
        Arc::new(RecordingSemanticOpener {
            state: Arc::clone(&semantic_state),
        }),
        Arc::new(StubRepoMapSnapshotPort::default()),
        Arc::new(FailClosedStructuralProducer),
        lexical_only_ledger(),
        test_activation_catalog()?,
    );
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(text_request(
            "rev:at.time(1970-01-01T00:00:00.150Z) needle",
            pin,
        )),
        &RequestBudgetV1::unbounded(),
    );
    let (code, _message) = ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
    if code != ERR_HISTORY_GENERATION_NOT_READY {
        return Err(format!("expected {ERR_HISTORY_GENERATION_NOT_READY}, got {code}").into());
    }
    let tally = tally(&lexical_state, &semantic_state)?;
    if tally.lexical_opens != 0 {
        return Err("no handle is opened for a refused view".into());
    }
    Ok(())
}

/// A predicate over a source-repo metadata authority the handle did not
/// materialize is refused with that authority's code.
///
/// The handle is opened (its identity is what proves the absence) and no
/// lane runs.
#[test]
fn a_repo_metadata_predicate_without_its_authority_is_refused_before_any_lane() -> TestResult {
    let lexical_state = Arc::new(Mutex::new(RecordingLexicalState {
        repo_metadata: Some(
            RepoMetadataAuthoritiesV1::NONE.with(RepoMetadataAuthorityV1::FileOwnership),
        ),
        ..RecordingLexicalState::default()
    }));
    let semantic_state = Arc::new(Mutex::new(RecordingSemanticState::default()));
    let dispatcher = recording_dispatcher(&lexical_state, &semantic_state, ready_ledger())?;

    let cases = [
        (
            "repo:has.commit.after(2020-01-01) needle",
            RepoMetadataAuthorityV1::CommitRecency,
        ),
        (
            "repo:has.meta(team:core) needle",
            RepoMetadataAuthorityV1::Meta,
        ),
        (
            "repo:has.topic(security) needle",
            RepoMetadataAuthorityV1::Topic,
        ),
        (
            "repo:has.description(\"search\") needle",
            RepoMetadataAuthorityV1::Description,
        ),
        (
            "file:has.contributor(alice) needle",
            RepoMetadataAuthorityV1::Contributor,
        ),
    ];
    for (query_text, authority) in cases {
        let response = dispatcher.dispatch(
            SearchPlaneQueryIpcRequest::Text(text_request(query_text, ready_pin())),
            &RequestBudgetV1::unbounded(),
        );
        let (code, message) =
            ipc_error_from(response).map_err(Box::<dyn std::error::Error>::from)?;
        if code != authority.unavailable_code() {
            return Err(format!(
                "{query_text}: expected {}, got {code}: {message}",
                authority.unavailable_code()
            )
            .into());
        }
    }
    let tally = tally(&lexical_state, &semantic_state)?;
    if tally.lexical_searches != 0 {
        return Err(format!(
            "no lane runs for a refused authority, saw {} searches",
            tally.lexical_searches
        )
        .into());
    }

    // The authority the handle does hold serves.
    let response = dispatcher.dispatch(
        SearchPlaneQueryIpcRequest::Text(text_request("select:file.owners needle", ready_pin())),
        &RequestBudgetV1::unbounded(),
    );
    let SearchPlaneQueryIpcResponse::Text(page) = response else {
        return Err(format!("the held authority serves, got {response:?}").into());
    };
    if page.file_owner_rows.is_none() {
        return Err("the ownership projection is served".into());
    }
    Ok(())
}

fn other_generation_history_read(
    pin: &GenerationPin,
) -> Result<crate::readiness::AuxRead<crate::readiness::HistoryAuthorityState>, CoreError> {
    let mut ledger = Ledger::default();
    let other = ManifestGeneration::new(pin.manifest_generation.get().saturating_add(1));
    ledger.apply_history_batch(
        &quanta_index_contract::HistoryIngestBatch {
            repo_id: pin.repo_id.clone(),
            revision_id: pin.revision_id.clone(),
            generation: other,
            manifest_digest: None,
            batch_digest: "other-generation".to_string(),
            commits: Vec::new(),
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: Vec::new(),
        },
        Instant::now(),
    )?;
    ledger
        .history_read_at(&pin.repo_id, &pin.revision_id, other, None, Instant::now())?
        .ok_or_else(|| CoreError::Storage("other generation history missing".to_string()))
}

/// A history snapshot of another generation offered as the pin's history
/// domain is refused as a mix; the view never claims parts of different
/// generations are one dependency vector.
#[test]
fn a_mixed_generation_aux_state_is_refused_typed() -> TestResult {
    let pin = ready_pin();
    let request = ReadViewRequestV1::new(
        "history",
        &pin,
        RequiredDomainsV1::of(ReadDomainV1::History),
    );
    let mixed = other_generation_history_read(&pin)?;
    match assemble_for_test(&request, Some(mixed), None, None, None) {
        Err(CoreError::Typed { code, message }) if code == READ_VIEW_GENERATION_MIX_CODE => {
            if !message.contains("belongs to generation 10") {
                return Err(format!("the refusal names the offered generation: {message}").into());
            }
        }
        Err(other) => return Err(format!("a mix is refused typed, got {other:?}").into()),
        Ok(view) => {
            return Err(format!(
                "a mix is refused typed, got a view over {}",
                view.identity().domains
            )
            .into());
        }
    }

    // The pin's own snapshot assembles, and its epoch is named.
    let own = {
        let ledger = ready_ledger();
        let mut guard = ledger
            .write()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        guard.apply_history_batch(
            &quanta_index_contract::HistoryIngestBatch {
                repo_id: pin.repo_id.clone(),
                revision_id: pin.revision_id.clone(),
                generation: pin.manifest_generation,
                manifest_digest: None,
                batch_digest: "own-generation".to_string(),
                commits: Vec::new(),
                refs: Vec::new(),
                tags: Vec::new(),
                diff_hunks: Vec::new(),
            },
            Instant::now(),
        )?;
        guard
            .history_read_at(
                &pin.repo_id,
                &pin.revision_id,
                pin.manifest_generation,
                None,
                Instant::now(),
            )?
            .ok_or("own generation history missing")?
    };
    let view = assemble_for_test(&request, Some(own), None, None, None)?;
    if view.identity().aux_epochs.get(&ReadDomainV1::History) != Some(&AuxEpochV1::new(1)) {
        return Err(format!(
            "the identity names the history epoch: {:?}",
            view.identity().aux_epochs
        )
        .into());
    }
    Ok(())
}

/// A route that reaches for a domain its plan did not declare is refused
/// typed rather than handed a late acquisition.
#[test]
fn a_route_reaching_past_its_declaration_is_refused() -> TestResult {
    let pin = ready_pin();
    let request = ReadViewRequestV1::new(
        "lexical",
        &pin,
        RequiredDomainsV1::of(ReadDomainV1::LexicalTrack),
    );
    let view = assemble_for_test(
        &request,
        None,
        None,
        None,
        Some(Arc::new(StubLexicalSearcher {
            manifest_digest: None,
            results: Vec::new(),
        })),
    )?;
    if view.lexical().is_err() {
        return Err("the declared domain is held".into());
    }
    for (name, outcome) in [
        ("semantic", view.semantic().map(|_| ())),
        ("history", view.history().map(|_| ())),
        ("history_text", view.history_text().map(|_| ())),
        ("runtime", view.runtime().map(|_| ())),
        ("structural", view.structural().map(|_| ())),
        (
            "semantic_manifest_digest",
            view.semantic_manifest_digest().map(|_| ()),
        ),
    ] {
        match outcome {
            Err(CoreError::Typed { code, .. }) if code == READ_VIEW_DOMAIN_UNDECLARED_CODE => {}
            other => {
                return Err(
                    format!("{name}: an undeclared domain is refused, got {other:?}").into(),
                );
            }
        }
    }
    Ok(())
}
