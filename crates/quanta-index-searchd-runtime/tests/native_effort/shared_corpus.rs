//! Cancelling one original request cannot revoke another reader of the same
//! authenticated overlay corpus.

use super::*;
use quanta_index_contract::native_protocol::{
    NativeEffortDone, NativeIndexErrorCode, NativeOverlayCorpusStamp, NativeOverlayDocument,
    NativeOverlayEffortContext, NativeOverlayReply, NativeOverlayRequest, NativeOverlaySession,
};
use quanta_index_searchd_runtime::native::NativeInteractionRecorder;

struct ReleaseGateOnDrop(Arc<ProviderGate>);

impl Drop for ReleaseGateOnDrop {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::Release);
    }
}

fn overlay_query(selected: ViewRef, session: NativeOverlaySession) -> NativeEffortStreamQuery {
    let mut query = stream_query(selected);
    query.overlay = Some(NativeOverlayEffortContext {
        session,
        expected_corpus: None,
    });
    query.text_query.query_text = "beta".into();
    query
}

fn query_with_fast_signal(
    socket: std::path::PathBuf,
    selected: ViewRef,
    session: NativeOverlaySession,
    request: &RequestBudgetV1,
    fast: &mpsc::Sender<NativeOverlayCorpusStamp>,
) -> (Result<NativeEffortDone, NativeUdsError>, bool) {
    let client = NativeUdsClient::new(socket);
    let mut saw_deep = false;
    let mut fast_corpus = None;
    let outcome =
        client.query_effort_stream(overlay_query(selected, session), request, &mut |stage| {
            match stage {
                NativeEffortStreamStage::OverlayFast(result) => {
                    fast_corpus = Some(result.corpus.clone());
                    fast.send(result.corpus.clone())
                        .map_err(|error| NativeUdsError::Protocol(error.to_string()))?;
                }
                NativeEffortStreamStage::OverlayDeep(result) => {
                    saw_deep = true;
                    if result.view != selected
                        || result.corpus.session != session
                        || fast_corpus.as_ref() != Some(&result.corpus)
                    {
                        return Err(NativeUdsError::Protocol(
                            "Deep changed selected view or overlay session".into(),
                        ));
                    }
                }
                NativeEffortStreamStage::Fast(_) | NativeEffortStreamStage::Deep(_) => {
                    return Err(NativeUdsError::Protocol(
                        "durable stage escaped overlay request".into(),
                    ));
                }
            }
            Ok(())
        });
    (outcome, saw_deep)
}

#[test]
fn independent_overlay_effort_requests_share_corpus_but_not_cancellation() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(ProviderGate {
        max_wait: Duration::from_secs(15),
        ..ProviderGate::default()
    });
    let mut plan = resources();
    plan.execution.capacity = ResourceNeed {
        cpu_units: 6,
        io_bytes: 6 * 64 * 1024 * 1024,
        memory_bytes: 7 * 64 * 1024 * 1024,
        provider_units: 3,
    };
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            resources: plan,
            provider_gate: Some(Arc::clone(&gate)),
            ..ServiceOptions::default()
        },
    )?;
    let resources = service.resource_observation();
    service.configure_interaction_activity(Arc::new(NativeInteractionRecorder::new()))?;
    service.configure_overlay(2, 4, 1024, 4096, TEXT_INSTANCE)?;
    service.configure_effort(NativeEffortConfiguration {
        history: Arc::new(NativeEffortObservationHistory::new()),
        policy: Arc::new(policy()?),
        estimator: Arc::new(DeclaredCostEstimator),
        signals: Arc::new(UnknownSignals),
    })?;
    let running = RunningService::start(service);
    let _release_gate = ReleaseGateOnDrop(Arc::clone(&gate));
    let client = NativeUdsClient::new(socket.clone());
    let (selected, _) = setup(&client, semantic)?;
    let setup_calls = calls.load(Ordering::SeqCst);
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
    let document = NativeOverlayDocument {
        session,
        owner: owner(),
    };
    assert_eq!(
        client.update_overlay_at_path(
            document,
            None,
            1,
            Some("beta".into()),
            Some("a.rs".into()),
            &budget(),
        )?,
        NativeOverlayReply::Updated {
            document,
            revision: 1,
        }
    );
    let catalog_path = catalog_path(root.path());
    let baseline_pins = pin_count(&catalog_path)?;

    let cancel_budget = RequestBudgetV1::for_duration(Duration::from_secs(20));
    let cancel_handle = cancel_budget.cancel_handle();
    let survivor_budget = RequestBudgetV1::for_duration(Duration::from_secs(20));
    let (fast_tx, fast_rx) = mpsc::channel();
    let cancelled_socket = socket.clone();
    let cancelled_fast = fast_tx.clone();
    let cancelled = std::thread::spawn(move || {
        query_with_fast_signal(
            cancelled_socket,
            selected,
            session,
            &cancel_budget,
            &cancelled_fast,
        )
    });
    let surviving = std::thread::spawn(move || {
        query_with_fast_signal(socket, selected, session, &survivor_budget, &fast_tx)
    });
    let rendezvous_deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or("rendezvous deadline overflow")?;
    let first =
        fast_rx.recv_timeout(rendezvous_deadline.saturating_duration_since(Instant::now()))?;
    let second =
        fast_rx.recv_timeout(rendezvous_deadline.saturating_duration_since(Instant::now()))?;
    assert_eq!(first, second);
    assert_eq!(first.session, session);

    let expected_provider_calls = setup_calls
        .checked_add(2)
        .ok_or("provider count overflow")?;
    while calls.load(Ordering::SeqCst) < expected_provider_calls {
        if Instant::now() >= rendezvous_deadline {
            return Err("both requests did not reach provider before gate deadline".into());
        }
        std::thread::yield_now();
    }
    assert!(gate.entered.load(Ordering::Acquire));
    assert_eq!(
        pin_count(&catalog_path)?,
        baseline_pins.checked_add(2).ok_or("pin count overflow")?
    );
    cancel_handle.cancel();
    gate.released.store(true, Ordering::Release);

    let (cancelled_outcome, cancelled_deep) = cancelled
        .join()
        .map_err(|_panic| "cancelled effort worker panicked")?;
    let (surviving_outcome, surviving_deep) = surviving
        .join()
        .map_err(|_panic| "surviving effort worker panicked")?;
    assert!(!cancelled_deep);
    assert!(
        matches!(cancelled_outcome, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Cancelled)
    );
    assert!(surviving_deep);
    let done = surviving_outcome?;
    assert_eq!(done.view, selected);
    assert!(done.deep_delivered);
    assert_eq!(done.corpus.as_ref(), Some(&first));
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
