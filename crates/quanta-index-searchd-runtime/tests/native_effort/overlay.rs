//! Real UDS effort stages over one selected view and live unsaved corpus.

use super::*;
use quanta_index_contract::native_protocol::{
    NativeEffortDone, NativeEffortReason, NativeIndexErrorCode, NativeOverlayCorpusStamp,
    NativeOverlayDocument, NativeOverlayEffortContext, NativeOverlayReply, NativeOverlayRequest,
    NativeOverlaySemanticResult, NativeOverlaySession, NativeOverlayTextResult,
    NativeOverlayTextSource,
};
use quanta_index_searchd_runtime::native::NativeInteractionRecorder;

fn second_owner() -> OwnerKey {
    OwnerKey {
        stable_id: StableOwnerId([7; 16]),
        ..owner()
    }
}

fn configure_overlay_service(service: &NativeIndexService) -> TestResult {
    service.configure_interaction_activity(Arc::new(NativeInteractionRecorder::new()))?;
    service.configure_overlay(2, 4, 1024, 4096, TEXT_INSTANCE)?;
    service.configure_effort(NativeEffortConfiguration {
        history: Arc::new(NativeEffortObservationHistory::new()),
        policy: Arc::new(policy()?),
        estimator: Arc::new(DeclaredCostEstimator),
        signals: Arc::new(UnknownSignals),
    })?;
    Ok(())
}

fn begin_overlay(client: &NativeUdsClient) -> TestResult<NativeOverlaySession> {
    let publisher = client.begin_interaction_publisher(Duration::from_secs(30), &budget())?;
    let session = NativeOverlaySession {
        workspace: workspace(),
        publisher,
        epoch: 1,
    };
    assert_eq!(
        client.overlay(NativeOverlayRequest::Begin { session }, &budget())?,
        NativeOverlayReply::Begun(session)
    );
    Ok(session)
}

fn update_document(
    client: &NativeUdsClient,
    document: NativeOverlayDocument,
    previous: Option<u64>,
    revision: u64,
    text: Option<&str>,
    path: Option<&str>,
) -> TestResult {
    assert_eq!(
        client.update_overlay_at_path(
            document,
            previous,
            revision,
            text.map(str::to_owned),
            path.map(str::to_owned),
            &budget(),
        )?,
        NativeOverlayReply::Updated { document, revision }
    );
    Ok(())
}

fn overlay_stream_query(
    selected: ViewRef,
    session: NativeOverlaySession,
    word: &str,
    expected_corpus: Option<Digest>,
) -> NativeEffortStreamQuery {
    let mut query = stream_query(selected);
    query.overlay = Some(NativeOverlayEffortContext {
        session,
        expected_corpus,
    });
    query.text_query.query_text = word.into();
    query
}

fn query_overlay_stream(
    client: &NativeUdsClient,
    query: NativeEffortStreamQuery,
) -> TestResult<(
    NativeOverlayTextResult,
    NativeOverlaySemanticResult,
    NativeEffortDone,
)> {
    let mut fast = None;
    let mut deep = None;
    let done = client.query_effort_stream(query, &budget(), &mut |stage| {
        match stage {
            NativeEffortStreamStage::OverlayFast(result) if fast.is_none() => {
                fast = Some(result.clone());
            }
            NativeEffortStreamStage::OverlayDeep(result) if fast.is_some() && deep.is_none() => {
                deep = Some(result.clone());
            }
            NativeEffortStreamStage::Fast(_)
            | NativeEffortStreamStage::Deep(_)
            | NativeEffortStreamStage::OverlayFast(_)
            | NativeEffortStreamStage::OverlayDeep(_) => {
                return Err(NativeUdsError::Protocol(
                    "overlay effort stage order or origin differs".into(),
                ));
            }
        }
        Ok(())
    })?;
    Ok((
        fast.ok_or("overlay Fast absent")?,
        deep.ok_or("overlay Deep absent")?,
        done,
    ))
}

#[test]
fn uds_overlay_unquoted_multi_token_keyword_matches_quoted_phrase() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    configure_overlay_service(&service)?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (selected, durable_source) = setup(&client, semantic)?;
    let session = begin_overlay(&client)?;
    let tombstone = NativeOverlayDocument {
        session,
        owner: owner(),
    };
    let target = NativeOverlayDocument {
        session,
        owner: second_owner(),
    };
    let other = NativeOverlayDocument {
        session,
        owner: OwnerKey {
            stable_id: StableOwnerId([8; 16]),
            ..owner()
        },
    };
    let gap = NativeOverlayDocument {
        session,
        owner: OwnerKey {
            stable_id: StableOwnerId([9; 16]),
            ..owner()
        },
    };
    update_document(&client, tombstone, None, 1, None, None)?;
    update_document(&client, target, None, 1, Some("row-0"), Some("b.rs"))?;
    update_document(&client, other, None, 1, Some("row-1"), Some("c.rs"))?;
    update_document(&client, gap, None, 1, Some("row gap 0"), Some("d.rs"))?;

    let (keyword_fast, keyword_deep, keyword_done) = query_overlay_stream(
        &client,
        overlay_stream_query(selected, session, "row-0", None),
    )?;
    assert_eq!(keyword_fast.view, selected);
    assert_eq!(keyword_deep.view, selected);
    assert_eq!(keyword_done.view, selected);
    assert!(keyword_done.deep_delivered);
    assert_eq!(keyword_done.corpus.as_ref(), Some(&keyword_fast.corpus));
    assert_eq!(keyword_deep.corpus, keyword_fast.corpus);
    assert!(keyword_fast.corpus.revisions.iter().any(|revision| {
        revision.owner == tombstone.owner
            && revision.revision == 1
            && revision.deleted
            && !revision.retired
    }));
    assert_eq!(keyword_fast.matched, 1);
    assert_eq!(keyword_fast.hits.len(), 1);
    assert!(keyword_fast.hits.first().is_some_and(|hit| {
        hit.owner == target.owner
            && hit.path == "b.rs"
            && hit.source == NativeOverlayTextSource::Overlay { revision: 1 }
    }));
    assert!(!keyword_fast.hits.iter().any(|hit| {
        hit.owner == tombstone.owner
            && hit.source == NativeOverlayTextSource::Durable(durable_source)
    }));

    let (phrase_fast, phrase_deep, phrase_done) = query_overlay_stream(
        &client,
        overlay_stream_query(
            selected,
            session,
            "\"row-0\"",
            Some(keyword_fast.corpus.identity),
        ),
    )?;
    assert_eq!(phrase_fast, keyword_fast);
    assert_eq!(phrase_deep, keyword_deep);
    assert_eq!(phrase_done, keyword_done);

    for (query, expected_owner) in [
        ("row-1", Some(other.owner)),
        ("0-row", None),
        ("row-0-gap", None),
        ("alpha", None),
    ] {
        let (fast, deep, done) = query_overlay_stream(
            &client,
            overlay_stream_query(selected, session, query, Some(keyword_fast.corpus.identity)),
        )?;
        assert_eq!(fast.view, selected);
        assert_eq!(deep.view, selected);
        assert_eq!(done.view, selected);
        assert!(done.deep_delivered);
        assert_eq!(fast.corpus, keyword_fast.corpus);
        assert_eq!(deep.corpus, keyword_fast.corpus);
        assert_eq!(done.corpus.as_ref(), Some(&keyword_fast.corpus));
        if let Some(expected) = expected_owner {
            assert_eq!(fast.matched, 1);
            assert_eq!(fast.hits.len(), 1);
            assert!(fast.hits.first().is_some_and(|hit| hit.owner == expected));
        } else {
            assert_eq!(fast.matched, 0);
            assert!(fast.hits.is_empty());
        }
    }
    assert_eq!(
        client.overlay(NativeOverlayRequest::Close { session }, &budget())?,
        NativeOverlayReply::Closed(session)
    );
    running.stop()?;
    Ok(())
}

#[test]
fn uds_overlay_effort_replaces_adds_and_deletes_on_one_selected_view() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    configure_overlay_service(&service)?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (selected, original_source) = setup(&client, semantic)?;
    let session = begin_overlay(&client)?;
    let old = NativeOverlayDocument {
        session,
        owner: owner(),
    };
    let added = NativeOverlayDocument {
        session,
        owner: second_owner(),
    };
    update_document(&client, old, None, 1, Some("beta"), Some("a.rs"))?;
    update_document(&client, added, None, 1, Some("alpha"), Some("b.rs"))?;

    let (fast, deep, done) = query_overlay_stream(
        &client,
        overlay_stream_query(selected, session, "alpha", None),
    )?;
    assert_eq!(fast.view, selected);
    assert_eq!(deep.view, selected);
    assert_eq!(done.view, selected);
    assert!(done.deep_delivered);
    assert_eq!(done.corpus.as_ref(), Some(&fast.corpus));
    assert_eq!(deep.corpus, fast.corpus);
    assert_eq!(fast.matched, 1);
    assert!(fast.hits.first().is_some_and(|hit| {
        hit.owner == added.owner
            && hit.path == "b.rs"
            && hit.source == NativeOverlayTextSource::Overlay { revision: 1 }
    }));
    assert!(deep.hits.first().is_some_and(|hit| {
        hit.owner == added.owner
            && hit.overlay.is_some_and(|origin| origin.revision == 1)
            && hit.chunk.is_none()
    }));
    assert!(!fast.hits.iter().any(|hit| {
        hit.owner == old.owner && hit.source == NativeOverlayTextSource::Durable(original_source)
    }));

    let (beta_fast, beta_deep, beta_done) = query_overlay_stream(
        &client,
        overlay_stream_query(selected, session, "beta", Some(fast.corpus.identity)),
    )?;
    assert_eq!(beta_fast.corpus, fast.corpus);
    assert_eq!(beta_deep.corpus, fast.corpus);
    assert_eq!(beta_done.corpus.as_ref(), Some(&fast.corpus));
    assert!(beta_fast.hits.first().is_some_and(|hit| {
        hit.owner == old.owner && hit.source == NativeOverlayTextSource::Overlay { revision: 1 }
    }));

    update_document(&client, old, Some(1), 2, None, None)?;
    let (deleted_fast, deleted_deep, deleted_done) = query_overlay_stream(
        &client,
        overlay_stream_query(selected, session, "alpha", None),
    )?;
    assert_eq!(deleted_fast.view, selected);
    assert_eq!(deleted_deep.corpus, deleted_fast.corpus);
    assert_eq!(deleted_done.corpus.as_ref(), Some(&deleted_fast.corpus));
    assert!(deleted_fast
        .corpus
        .revisions
        .iter()
        .any(|row| { row.owner == old.owner && row.revision == 2 && row.deleted && !row.retired }));
    assert_eq!(deleted_fast.matched, 1);
    assert!(deleted_fast
        .hits
        .first()
        .is_some_and(|hit| hit.owner == added.owner));
    assert!(!deleted_deep.hits.iter().any(|hit| hit.owner == old.owner));
    assert_eq!(
        client.overlay(NativeOverlayRequest::Close { session }, &budget())?,
        NativeOverlayReply::Closed(session)
    );
    let closed = client.query_effort_stream(
        overlay_stream_query(selected, session, "alpha", None),
        &budget(),
        &mut |_stage| Ok(()),
    );
    assert!(
        matches!(closed, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Conflict)
    );
    running.stop()?;
    Ok(())
}

#[test]
fn uds_overlay_lexical_scope_limits_deep_to_fast_or_distinct_text_universe() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let (service, semantic) = service(root.path(), &socket, Arc::clone(&calls))?;
    configure_overlay_service(&service)?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (selected, _) = setup(&client, semantic)?;
    let session = begin_overlay(&client)?;
    let replaced = NativeOverlayDocument {
        session,
        owner: owner(),
    };
    let added = NativeOverlayDocument {
        session,
        owner: second_owner(),
    };
    update_document(&client, replaced, None, 1, Some("beta"), Some("a.rs"))?;
    update_document(&client, added, None, 1, Some("alpha"), Some("b.rs"))?;

    // A beta semantic vector prefers the replaced owner without lexical
    // eligibility. The alpha Fast universe contains only the added owner.
    let mut same = overlay_stream_query(selected, session, "alpha", None);
    same.semantic_query.query_text = "beta".into();
    same.semantic_query.lexical_scope = Some(same.text_query.clone());
    let calls_before = calls.load(Ordering::SeqCst);
    let (same_fast, same_deep, same_done) = query_overlay_stream(&client, same)?;
    assert_eq!(same_fast.matched, 1);
    assert!(same_fast.hits.iter().all(|hit| hit.owner == added.owner));
    assert_eq!(same_deep.corpus, same_fast.corpus);
    assert_eq!(same_done.corpus.as_ref(), Some(&same_fast.corpus));
    assert_eq!(same_deep.hits.len(), 1);
    assert!(same_deep.hits.iter().all(|hit| {
        hit.owner == added.owner && hit.overlay.is_some_and(|origin| origin.revision == 1)
    }));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        calls_before.checked_add(2).ok_or("call count overflow")?
    );

    // Fast beta and Deep lexical alpha differ. A second Text stage must
    // replace Fast's hit universe without emitting another Fast frame.
    let mut distinct =
        overlay_stream_query(selected, session, "beta", Some(same_fast.corpus.identity));
    distinct.semantic_query.query_text = "beta".into();
    let mut lexical = distinct.text_query.clone();
    lexical.query_text = "alpha".into();
    distinct.semantic_query.lexical_scope = Some(lexical);
    let calls_before = calls.load(Ordering::SeqCst);
    let (different_fast, different_deep, different_done) = query_overlay_stream(&client, distinct)?;
    assert_eq!(different_fast.matched, 1);
    assert!(different_fast
        .hits
        .iter()
        .all(|hit| hit.owner == replaced.owner));
    assert_eq!(different_fast.corpus, same_fast.corpus);
    assert_eq!(different_deep.corpus, same_fast.corpus);
    assert_eq!(different_done.corpus.as_ref(), Some(&same_fast.corpus));
    assert_eq!(different_deep.hits.len(), 1);
    assert!(different_deep.hits.iter().all(|hit| {
        hit.owner == added.owner && hit.overlay.is_some_and(|origin| origin.revision == 1)
    }));
    assert_eq!(
        calls.load(Ordering::SeqCst),
        calls_before.checked_add(2).ok_or("call count overflow")?
    );

    // Deleting the old owner masks durable alpha. With only beta active,
    // scoped alpha is empty and no query or overlay embedding is requested.
    update_document(&client, replaced, Some(1), 2, None, None)?;
    update_document(&client, added, Some(1), 2, Some("beta"), Some("b.rs"))?;
    let mut empty = overlay_stream_query(selected, session, "beta", None);
    let mut lexical = empty.text_query.clone();
    lexical.query_text = "alpha".into();
    empty.semantic_query.lexical_scope = Some(lexical);
    let calls_before = calls.load(Ordering::SeqCst);
    let (empty_fast, empty_deep, empty_done) = query_overlay_stream(&client, empty)?;
    assert_eq!(empty_fast.matched, 1);
    assert!(empty_fast.hits.iter().all(|hit| hit.owner == added.owner));
    assert!(empty_fast.corpus.revisions.iter().any(|row| {
        row.owner == replaced.owner && row.revision == 2 && row.deleted && !row.retired
    }));
    assert!(empty_deep.hits.is_empty());
    assert_eq!(empty_deep.corpus, empty_fast.corpus);
    assert_eq!(empty_done.corpus.as_ref(), Some(&empty_fast.corpus));
    assert_eq!(calls.load(Ordering::SeqCst), calls_before);
    running.stop()?;
    Ok(())
}

#[test]
fn uds_overlay_distinct_lexical_scope_keeps_emitted_fast_on_optional_deep_denial() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut options = ServiceOptions::default();
    options.resources.execution.capacity.cpu_units = 6;
    options.resources.execution.interactive.cpu_units = 2;
    options.semantic_need = options.resources.execution.interactive;
    let (service, semantic) =
        service_with_options(root.path(), &socket, Arc::clone(&calls), options)?;
    configure_overlay_service(&service)?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket);
    let (selected, _) = setup(&client, semantic)?;
    let session = begin_overlay(&client)?;
    let document = NativeOverlayDocument {
        session,
        owner: owner(),
    };
    update_document(&client, document, None, 1, Some("beta"), Some("a.rs"))?;

    let mut query = overlay_stream_query(selected, session, "beta", None);
    let mut lexical = query.text_query.clone();
    lexical.query_text = "alpha".into();
    query.semantic_query.lexical_scope = Some(lexical);
    let baseline = calls.load(Ordering::SeqCst);
    let mut fast = None;
    let mut deep_seen = false;
    let done = client.query_effort_stream(query, &budget(), &mut |stage| {
        match stage {
            NativeEffortStreamStage::OverlayFast(result) if fast.is_none() => {
                fast = Some(result.clone());
            }
            NativeEffortStreamStage::OverlayDeep(_) => deep_seen = true,
            NativeEffortStreamStage::Fast(_)
            | NativeEffortStreamStage::Deep(_)
            | NativeEffortStreamStage::OverlayFast(_) => {
                return Err(NativeUdsError::Protocol(
                    "scoped overlay effort emitted an unexpected stage".into(),
                ));
            }
        }
        Ok(())
    })?;
    let fast = fast.ok_or("required Fast stage was not delivered")?;
    assert_eq!(fast.view, selected);
    assert_eq!(fast.matched, 1);
    assert!(fast.hits.iter().all(|hit| hit.owner == document.owner));
    assert!(!deep_seen);
    assert_eq!(done.reason, NativeEffortReason::ResourceDenied);
    assert!(!done.deep_delivered);
    assert_eq!(done.corpus.as_ref(), Some(&fast.corpus));
    assert_eq!(calls.load(Ordering::SeqCst), baseline);
    running.stop()?;
    Ok(())
}

#[test]
fn uds_overlay_effort_rejects_revision_change_during_provider_before_deep_delivery() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(ProviderGate::default());
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            provider_gate: Some(Arc::clone(&gate)),
            ..ServiceOptions::default()
        },
    )?;
    configure_overlay_service(&service)?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket.clone());
    let (selected, _) = setup(&client, semantic)?;
    let session = begin_overlay(&client)?;
    let document = NativeOverlayDocument {
        session,
        owner: owner(),
    };
    update_document(&client, document, None, 1, Some("beta"), Some("a.rs"))?;

    let (fast_tx, fast_rx) = mpsc::channel::<NativeOverlayCorpusStamp>();
    let worker = std::thread::spawn(move || {
        let client = NativeUdsClient::new(socket);
        let mut deep_seen = false;
        let result = client.query_effort_stream(
            overlay_stream_query(selected, session, "beta", None),
            &budget(),
            &mut |stage| {
                match stage {
                    NativeEffortStreamStage::OverlayFast(fast) => fast_tx
                        .send(fast.corpus.clone())
                        .map_err(|error| NativeUdsError::Protocol(error.to_string()))?,
                    NativeEffortStreamStage::OverlayDeep(_) => deep_seen = true,
                    NativeEffortStreamStage::Fast(_) | NativeEffortStreamStage::Deep(_) => {
                        return Err(NativeUdsError::Protocol(
                            "durable stage escaped overlay request".into(),
                        ));
                    }
                }
                Ok(())
            },
        );
        (result, deep_seen)
    });
    let captured = fast_rx.recv_timeout(Duration::from_secs(3))?;
    assert_eq!(captured.session, session);
    wait_until(|| gate.entered.load(Ordering::Acquire))?;
    update_document(&client, document, Some(1), 2, Some("alpha"), Some("a.rs"))?;
    gate.released.store(true, Ordering::Release);
    let (outcome, deep_seen) = worker.join().map_err(|_panic| "overlay worker panicked")?;
    assert!(!deep_seen);
    assert!(
        matches!(outcome, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Conflict)
    );
    running.stop()?;
    Ok(())
}

#[test]
fn uds_overlay_effort_original_cancellation_during_provider_blocks_deep_and_drains() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(ProviderGate::default());
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            provider_gate: Some(Arc::clone(&gate)),
            ..ServiceOptions::default()
        },
    )?;
    let resources = service.resource_observation();
    configure_overlay_service(&service)?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket.clone());
    let (selected, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let baseline_pins = pin_count(&catalog_path)?;
    let session = begin_overlay(&client)?;
    let document = NativeOverlayDocument {
        session,
        owner: owner(),
    };
    update_document(&client, document, None, 1, Some("beta"), Some("a.rs"))?;

    let original = budget();
    let worker_request = original.clone();
    let (fast_tx, fast_rx) = mpsc::channel::<NativeOverlayCorpusStamp>();
    let worker = std::thread::spawn(move || {
        let client = NativeUdsClient::new(socket);
        let mut deep_seen = false;
        let result = client.query_effort_stream(
            overlay_stream_query(selected, session, "beta", None),
            &worker_request,
            &mut |stage| {
                match stage {
                    NativeEffortStreamStage::OverlayFast(fast) => fast_tx
                        .send(fast.corpus.clone())
                        .map_err(|error| NativeUdsError::Protocol(error.to_string()))?,
                    NativeEffortStreamStage::OverlayDeep(_) => deep_seen = true,
                    NativeEffortStreamStage::Fast(_) | NativeEffortStreamStage::Deep(_) => {
                        return Err(NativeUdsError::Protocol(
                            "durable stage escaped overlay request".into(),
                        ));
                    }
                }
                Ok(())
            },
        );
        (result, deep_seen)
    });
    let captured = fast_rx.recv_timeout(Duration::from_secs(3))?;
    assert_eq!(captured.session, session);
    assert_eq!(captured.revisions.len(), 1);
    assert!(captured
        .revisions
        .first()
        .is_some_and(|revision| revision.owner == document.owner));
    wait_until(|| gate.entered.load(Ordering::Acquire))?;
    assert_eq!(
        pin_count(&catalog_path)?,
        baseline_pins.checked_add(1).ok_or("pin count overflow")?
    );
    original.cancel_handle().cancel();
    gate.released.store(true, Ordering::Release);
    let (outcome, deep_seen) = worker.join().map_err(|_panic| "overlay worker panicked")?;
    assert!(original.is_cancelled());
    assert!(!deep_seen);
    assert!(matches!(
        outcome,
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Cancelled
    ));
    wait_until(|| {
        pin_count(&catalog_path).is_ok_and(|count| count == baseline_pins)
            && resources.observe().is_ok_and(|observation| {
                observation.reserved == ResourceNeed::default()
                    && observation.interactive.active == 0
                    && observation.enrichment.active == 0
            })
    })?;
    assert_eq!(
        client.overlay(NativeOverlayRequest::Close { session }, &budget())?,
        NativeOverlayReply::Closed(session)
    );
    running.stop()?;
    Ok(())
}

#[derive(Clone, Copy)]
enum LateCorpusInvalidation {
    CloseSession,
    EndPublisher,
}

fn overlay_effort_rejects_late_corpus_invalidation(action: LateCorpusInvalidation) -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(ProviderGate::default());
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            provider_gate: Some(Arc::clone(&gate)),
            ..ServiceOptions::default()
        },
    )?;
    let resources = service.resource_observation();
    configure_overlay_service(&service)?;
    let running = RunningService::start(service);
    let client = NativeUdsClient::new(socket.clone());
    let (selected, _) = setup(&client, semantic)?;
    let catalog_path = catalog_path(root.path());
    let baseline_pins = pin_count(&catalog_path)?;
    let session = begin_overlay(&client)?;
    let document = NativeOverlayDocument {
        session,
        owner: owner(),
    };
    update_document(&client, document, None, 1, Some("beta"), Some("a.rs"))?;

    let (fast_tx, fast_rx) = mpsc::channel::<NativeOverlayCorpusStamp>();
    let worker = std::thread::spawn(move || {
        let client = NativeUdsClient::new(socket);
        let mut deep_seen = false;
        let result = client.query_effort_stream(
            overlay_stream_query(selected, session, "beta", None),
            &budget(),
            &mut |stage| {
                match stage {
                    NativeEffortStreamStage::OverlayFast(fast) => fast_tx
                        .send(fast.corpus.clone())
                        .map_err(|error| NativeUdsError::Protocol(error.to_string()))?,
                    NativeEffortStreamStage::OverlayDeep(_) => deep_seen = true,
                    NativeEffortStreamStage::Fast(_) | NativeEffortStreamStage::Deep(_) => {
                        return Err(NativeUdsError::Protocol(
                            "durable stage escaped overlay request".into(),
                        ));
                    }
                }
                Ok(())
            },
        );
        (result, deep_seen)
    });
    let captured = fast_rx.recv_timeout(Duration::from_secs(3))?;
    assert_eq!(captured.session, session);
    assert!(captured.revisions.iter().any(|row| {
        row.owner == document.owner && row.revision == 1 && !row.deleted && !row.retired
    }));
    wait_until(|| gate.entered.load(Ordering::Acquire))?;
    let expected_pins = baseline_pins.checked_add(1).ok_or("pin count overflow")?;
    assert_eq!(pin_count(&catalog_path)?, expected_pins);
    match action {
        LateCorpusInvalidation::CloseSession => {
            assert_eq!(
                client.overlay(NativeOverlayRequest::Close { session }, &budget())?,
                NativeOverlayReply::Closed(session)
            );
        }
        LateCorpusInvalidation::EndPublisher => {
            client.end_interaction_publisher(session.publisher, 1, &budget())?;
        }
    }
    gate.released.store(true, Ordering::Release);
    let (outcome, deep_seen) = worker.join().map_err(|_panic| "overlay worker panicked")?;
    assert!(!deep_seen);
    assert!(matches!(
        outcome,
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Conflict
    ));
    wait_until(|| {
        pin_count(&catalog_path).is_ok_and(|count| count == baseline_pins)
            && resources.observe().is_ok_and(|observation| {
                observation.reserved == ResourceNeed::default()
                    && observation.interactive.active == 0
                    && observation.enrichment.active == 0
            })
    })?;
    running.stop()?;
    Ok(())
}

#[test]
fn uds_overlay_effort_close_during_provider_refuses_deep_and_drains() -> TestResult {
    overlay_effort_rejects_late_corpus_invalidation(LateCorpusInvalidation::CloseSession)
}

#[test]
fn uds_overlay_effort_end_publisher_during_provider_refuses_deep_and_drains() -> TestResult {
    overlay_effort_rejects_late_corpus_invalidation(LateCorpusInvalidation::EndPublisher)
}
