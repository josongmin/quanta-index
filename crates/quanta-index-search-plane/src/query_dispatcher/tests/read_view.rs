//! One `QueryReadViewV1` per request (plan §5.6 / §7.1).
//!
//! A route opens exactly the domains its plan declares, a required domain
//! the generation lacks is refused typed before any lane executes, and
//! the view never mixes generations.
//!
//! The oracles are the recording doubles' call logs (which handles were
//! opened, which lanes ran) and ledgers that hold *no* auxiliary state at
//! all: a route that served from such a ledger provably demanded no
//! auxiliary domain.

use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use quanta_index_contract::{
    AuxEpochV1, GenerationPin, HybridQueryRequest, ManifestGeneration, PlannerStage, RepoId,
    RevisionId, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcResponse, SearchPlaneTrackKind,
    TextQueryRequest, TextQuerySyntax,
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
use crate::query_dispatcher::tests::support::common::{
    TestResult, activation_catalog_with_generations, candidate, corpus_generation, ipc_error_from,
    ready_ledger, ready_pin, test_activation_catalog,
};
use crate::query_dispatcher::tests::support::lexical::{
    RecordingLexicalOpener, RecordingLexicalState, StubLexicalSearcher,
};
use crate::query_dispatcher::tests::support::repo_map::StubRepoMapQueryPort;
use crate::query_dispatcher::tests::support::semantic::{
    RecordingSemanticOpener, RecordingSemanticState,
};
use crate::query_dispatcher::tests::support::structural::FailClosedStructuralProducer;

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
        Arc::new(StubRepoMapQueryPort),
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
        "read_view.lexical_artifact=stub-lexical-digest",
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
        Arc::new(StubRepoMapQueryPort),
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
        Arc::new(StubRepoMapQueryPort),
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
        ("repo:has.meta(team:core) needle", RepoMetadataAuthorityV1::Meta),
        ("repo:has.topic(security) needle", RepoMetadataAuthorityV1::Topic),
        ("repo:has.description(\"search\") needle", RepoMetadataAuthorityV1::Description),
        ("file:has.contributor(alice) needle", RepoMetadataAuthorityV1::Contributor),
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
    let request =
        ReadViewRequestV1::new("history", &pin, RequiredDomainsV1::of(ReadDomainV1::History));
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
    let request =
        ReadViewRequestV1::new("lexical", &pin, RequiredDomainsV1::of(ReadDomainV1::LexicalTrack));
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
        ("semantic_manifest_digest", view.semantic_manifest_digest().map(|_| ())),
    ] {
        match outcome {
            Err(CoreError::Typed { code, .. }) if code == READ_VIEW_DOMAIN_UNDECLARED_CODE => {}
            other => {
                return Err(
                    format!("{name}: an undeclared domain is refused, got {other:?}").into()
                );
            }
        }
    }
    Ok(())
}
