//! Controlled UDS overlap: a held optional provider cannot consume protected
//! Fast and foreground-control capacity or dispatch a second Deep stage.

use super::*;
use quanta_index_contract::native_protocol::{NativeEffortReason, NativeIndexErrorCode};
use quanta_index_searchd_runtime::native::NativeInteractionRecorder;

struct ReleaseProviderOnDrop(Arc<ProviderGate>);

impl Drop for ReleaseProviderOnDrop {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::Release);
    }
}

fn provider_limited_resources() -> NativeServiceResources {
    let mut plan = resources();
    // Two Deep executions fit the CPU, IO and memory envelopes alongside
    // protected control/interactive/core floors and retained-data headroom.
    // Only the provider capacity remains at the two-class protected floor.
    plan.execution.capacity = ResourceNeed {
        cpu_units: 8,
        io_bytes: 8 * 64 * 1024 * 1024,
        memory_bytes: 16 * 64 * 1024 * 1024,
        provider_units: 2,
    };
    plan
}

#[test]
fn held_deep_preserves_independent_fast_and_foreground_control() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(ProviderGate {
        max_wait: Duration::from_secs(15),
        ..ProviderGate::default()
    });
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            resources: provider_limited_resources(),
            provider_gate: Some(Arc::clone(&gate)),
            ..ServiceOptions::default()
        },
    )?;
    let resources = service.resource_observation();
    service.configure_interaction_activity(Arc::new(NativeInteractionRecorder::new()))?;
    service.configure_effort(NativeEffortConfiguration {
        history: Arc::new(NativeEffortObservationHistory::new()),
        policy: Arc::new(policy()?),
        estimator: Arc::new(DeclaredCostEstimator),
        signals: Arc::new(UnknownSignals),
    })?;
    let running = RunningService::start(service);
    let _release_gate = ReleaseProviderOnDrop(Arc::clone(&gate));
    let client = NativeUdsClient::new(socket.clone());
    let (view, _) = setup(&client, semantic)?;
    let baseline_calls = calls.load(Ordering::SeqCst);
    let catalog_path = catalog_path(root.path());
    let baseline_pins = pin_count(&catalog_path)?;

    let (fast_tx, fast_rx) = mpsc::channel();
    let held_socket = socket;
    let held = std::thread::spawn(move || {
        let client = NativeUdsClient::new(held_socket);
        let mut stages = Vec::new();
        let result = client.query_effort_stream(stream_query(view), &budget(), &mut |stage| {
            match stage {
                NativeEffortStreamStage::Fast(result) => {
                    stages.push(1_u8);
                    fast_tx
                        .send(result.view)
                        .map_err(|error| NativeUdsError::Protocol(error.to_string()))?;
                }
                NativeEffortStreamStage::Deep(result) => {
                    if result.view != view {
                        return Err(NativeUdsError::Protocol("Deep changed view".into()));
                    }
                    stages.push(2);
                }
                NativeEffortStreamStage::OverlayFast(_)
                | NativeEffortStreamStage::OverlayDeep(_) => {
                    return Err(NativeUdsError::Protocol(
                        "durable query returned overlay stage".into(),
                    ));
                }
            }
            Ok(())
        });
        (result, stages)
    });
    assert_eq!(fast_rx.recv_timeout(Duration::from_secs(5))?, view);
    let rendezvous_deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or("provider rendezvous deadline overflow")?;
    while !gate.entered.load(Ordering::Acquire) {
        if Instant::now() >= rendezvous_deadline {
            return Err("held Deep never entered provider".into());
        }
        std::thread::yield_now();
    }
    let held_calls = baseline_calls
        .checked_add(1)
        .ok_or("provider count overflow")?;
    assert_eq!(calls.load(Ordering::SeqCst), held_calls);
    let held_resources = resources.observe()?;
    assert_eq!(held_resources.enrichment.active, 1);
    assert_eq!(held_resources.enrichment.reserved.provider_units, 1);
    assert_eq!(held_resources.enrichment.reserved.cpu_units, 1);
    assert_eq!(
        pin_count(&catalog_path)?,
        baseline_pins.checked_add(1).ok_or("pin count overflow")?
    );

    // These are separate UDS connections. Their completion is a functional
    // progress witness while the admitted Deep provider is still held.
    let mut fast_query = stream_query(view);
    fast_query.preference = NativeEffortPreference::Fast;
    let mut independent_fast = Vec::new();
    let fast_done = client.query_effort_stream(fast_query, &budget(), &mut |stage| {
        match stage {
            NativeEffortStreamStage::Fast(result) => {
                independent_fast.push(result.view);
            }
            NativeEffortStreamStage::Deep(_)
            | NativeEffortStreamStage::OverlayFast(_)
            | NativeEffortStreamStage::OverlayDeep(_) => {
                return Err(NativeUdsError::Protocol(
                    "independent Fast returned unexpected stage".into(),
                ));
            }
        }
        Ok(())
    })?;
    assert_eq!(independent_fast, vec![view]);
    assert_eq!(fast_done.view, view);
    assert!(!fast_done.deep_delivered);
    assert_eq!(fast_done.reason, NativeEffortReason::ExplicitFast);

    let publisher = client.begin_interaction_publisher(Duration::from_secs(10), &budget())?;
    client.report_interaction_activity(publisher, 1, &budget())?;
    client.renew_interaction_publisher(publisher, 2, &budget())?;
    client.end_interaction_publisher(publisher, 3, &budget())?;

    // The second query may publish Fast, but its optional Deep must be
    // refused before invoking the provider while the first Deep holds charge.
    let mut extra_stages = Vec::new();
    let extra_done = client.query_effort_stream(stream_query(view), &budget(), &mut |stage| {
        match stage {
            NativeEffortStreamStage::Fast(result) => extra_stages.push(result.view),
            NativeEffortStreamStage::Deep(_)
            | NativeEffortStreamStage::OverlayFast(_)
            | NativeEffortStreamStage::OverlayDeep(_) => {
                return Err(NativeUdsError::Protocol(
                    "saturated Deep started provider stage".into(),
                ));
            }
        }
        Ok(())
    })?;
    assert_eq!(extra_stages, vec![view]);
    assert_eq!(extra_done.view, view);
    assert_eq!(extra_done.reason, NativeEffortReason::ResourceDenied);
    assert!(!extra_done.deep_delivered);
    assert_eq!(calls.load(Ordering::SeqCst), held_calls);
    assert!(gate.entered.load(Ordering::Acquire));
    let still_held = resources.observe()?;
    assert_eq!(still_held.enrichment.active, 1);
    assert_eq!(still_held.enrichment.reserved.provider_units, 1);

    gate.released.store(true, Ordering::Release);
    let (held_result, held_stages) = held.join().map_err(|_panic| "held Deep panicked")?;
    let held_done = held_result?;
    assert_eq!(held_stages, vec![1, 2]);
    assert!(held_done.deep_delivered);
    assert_eq!(held_done.view, view);
    wait_until(|| {
        pin_count(&catalog_path).is_ok_and(|pins| pins == baseline_pins)
            && resources.observe().is_ok_and(|state| {
                state.reserved == ResourceNeed::default()
                    && state.interactive.active == 0
                    && state.enrichment.active == 0
                    && state.control.active == 0
            })
    })?;
    running.stop()?;
    Ok(())
}

#[test]
fn cancelled_noncooperative_provider_remains_charged_until_it_exits() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(ProviderGate {
        max_wait: Duration::from_secs(15),
        cooperative: false,
        ..ProviderGate::default()
    });
    let (service, semantic) = service_with_options(
        root.path(),
        &socket,
        Arc::clone(&calls),
        ServiceOptions {
            resources: provider_limited_resources(),
            provider_gate: Some(Arc::clone(&gate)),
            ..ServiceOptions::default()
        },
    )?;
    let resources = service.resource_observation();
    service.configure_effort(NativeEffortConfiguration {
        history: Arc::new(NativeEffortObservationHistory::new()),
        policy: Arc::new(policy()?),
        estimator: Arc::new(DeclaredCostEstimator),
        signals: Arc::new(UnknownSignals),
    })?;
    let running = RunningService::start(service);
    let _release_gate = ReleaseProviderOnDrop(Arc::clone(&gate));
    let client = NativeUdsClient::new(socket.clone());
    let (view, _) = setup(&client, semantic)?;
    let baseline_calls = calls.load(Ordering::SeqCst);
    let catalog_path = catalog_path(root.path());
    let baseline_pins = pin_count(&catalog_path)?;
    let original = RequestBudgetV1::for_duration(Duration::from_secs(20));
    let cancel = original.cancel_handle();
    let (fast_tx, fast_rx) = mpsc::channel();
    let (terminal_tx, terminal_rx) = mpsc::channel();
    let held_socket = socket;
    let held = std::thread::spawn(move || {
        let client = NativeUdsClient::new(held_socket);
        let outcome = client.query_effort_stream(stream_query(view), &original, &mut |stage| {
            match stage {
                NativeEffortStreamStage::Fast(result) => fast_tx
                    .send(result.view)
                    .map_err(|error| NativeUdsError::Protocol(error.to_string()))?,
                NativeEffortStreamStage::Deep(_)
                | NativeEffortStreamStage::OverlayFast(_)
                | NativeEffortStreamStage::OverlayDeep(_) => {
                    return Err(NativeUdsError::Protocol(
                        "cancelled query returned a later stage".into(),
                    ));
                }
            }
            Ok(())
        });
        let _sent = terminal_tx.send(outcome);
    });
    assert_eq!(fast_rx.recv_timeout(Duration::from_secs(5))?, view);
    let rendezvous_deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or("provider rendezvous deadline overflow")?;
    while !gate.entered.load(Ordering::Acquire) {
        if Instant::now() >= rendezvous_deadline {
            return Err("noncooperative provider was not entered".into());
        }
        std::thread::yield_now();
    }
    let held_calls = baseline_calls
        .checked_add(1)
        .ok_or("provider count overflow")?;
    assert_eq!(calls.load(Ordering::SeqCst), held_calls);
    cancel.cancel();
    let cancelled = terminal_rx.recv_timeout(Duration::from_secs(5))?;
    assert!(matches!(
        cancelled,
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Cancelled
    ));
    held.join()
        .map_err(|_panic| "cancelled UDS worker panicked")?;

    // The client has exited, but the provider has not. No reservation or
    // provider unit may be refunded at the cancellation boundary.
    assert!(!gate.released.load(Ordering::Acquire));
    let charged = resources.observe()?;
    assert_eq!(charged.enrichment.active, 1);
    assert_eq!(charged.enrichment.reserved.cpu_units, 1);
    assert_eq!(charged.enrichment.reserved.provider_units, 1);
    assert_eq!(
        pin_count(&catalog_path)?,
        baseline_pins.checked_add(1).ok_or("pin count overflow")?
    );
    let mut extra_fast = Vec::new();
    let extra = client.query_effort_stream(stream_query(view), &budget(), &mut |stage| {
        match stage {
            NativeEffortStreamStage::Fast(result) => extra_fast.push(result.view),
            NativeEffortStreamStage::Deep(_)
            | NativeEffortStreamStage::OverlayFast(_)
            | NativeEffortStreamStage::OverlayDeep(_) => {
                return Err(NativeUdsError::Protocol(
                    "charged provider admitted a second Deep".into(),
                ));
            }
        }
        Ok(())
    })?;
    assert_eq!(extra_fast, vec![view]);
    assert_eq!(extra.reason, NativeEffortReason::ResourceDenied);
    assert!(!extra.deep_delivered);
    assert_eq!(calls.load(Ordering::SeqCst), held_calls);

    gate.released.store(true, Ordering::Release);
    wait_until(|| {
        pin_count(&catalog_path).is_ok_and(|pins| pins == baseline_pins)
            && resources.observe().is_ok_and(|state| {
                state.reserved == ResourceNeed::default()
                    && state.enrichment.active == 0
                    && state.interactive.active == 0
            })
    })?;
    running.stop()?;
    Ok(())
}
