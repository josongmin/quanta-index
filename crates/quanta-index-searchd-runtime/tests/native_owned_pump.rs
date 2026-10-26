//! Service-owned native pump lifecycle and failure supervision.
#![expect(
    clippy::panic_in_result_fn,
    reason = "integration assertions are independent observable oracles"
)]

use quanta_index_catalog::CATALOG_FILE_NAME;
use quanta_index_contract::{
    SearchPlaneErrorCodeV2,
    index::{
        OperationKey, OperationStatus, OwnerKey, ProducerId, SlotId, SourceTextAdmission,
        StableOwnerId, StreamId, UpdateKind, UpdateRequest, ViewSelection, WorkClaim, WorkIntent,
        WorkScope, WorkStatus, WorkspaceKey,
    },
    native_protocol::{NativeIndexErrorCode, NativeIndexRequest, NativeIndexResponse},
};
use quanta_index_core::{
    CoreError,
    index::{ObjectRef, WorkBudget, WorkLimits, ports::ImmutableObjectPort},
};
use quanta_index_kernel::{AdapterRegistry, QueryAdapterRegistry};
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};
use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd_runtime::native::{
    NATIVE_MARKER_NAME, NativeIndexService, NativePumpDecision, NativePumpObservation,
    NativePumpPolicy, NativePumpScope, NativeReclaimConfig, NativeServiceCapabilities,
};
use quanta_index_storage::FileObjectStore;
use std::{
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use quanta_index_searchd_runtime as native_runtime;
#[path = "common/native_owned_pump_fixture.rs"]
mod fixture;
use fixture::{
    ProviderGate, ReleaseProvider, ServiceThreadGuard, TestResult, advance, advance_in, budget,
    config, configure, configure_in, limits, operation, operation_in, owner, private_root,
    published, registry, registry_with_provider, resources, workspace,
};

fn other_workspace() -> WorkspaceKey {
    WorkspaceKey {
        slot: SlotId([7; 16]),
        ..workspace()
    }
}

fn third_workspace() -> WorkspaceKey {
    WorkspaceKey {
        slot: SlotId([8; 16]),
        ..workspace()
    }
}

fn read_catalog(root: &Path) -> Result<rusqlite::Connection, rusqlite::Error> {
    rusqlite::Connection::open_with_flags(
        root.join("catalog").join(CATALOG_FILE_NAME),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
}

#[test]
fn invalid_pump_scope_is_rejected_before_native_format_creation() -> TestResult {
    let root = private_root()?;
    let mut invalid = config();
    invalid.scopes.push(NativePumpScope {
        workspace: workspace(),
        epoch: 1,
    });
    let result = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &root.path().join("native.sock"),
        limits(),
        AdapterRegistry::new(),
        resources(),
        Some(invalid),
    );
    assert!(result.is_err());
    assert!(!root.path().join(NATIVE_MARKER_NAME).exists());
    assert_eq!(std::fs::read_dir(root.path())?.count(), 0);
    Ok(())
}

#[test]
fn owned_pump_refuses_capacity_that_can_starve_foreground_control_before_creation() -> TestResult {
    let root = private_root()?;
    let mut too_small = resources();
    too_small.execution.capacity.cpu_units = 4;
    let result = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &root.path().join("native.sock"),
        limits(),
        AdapterRegistry::new(),
        too_small,
        Some(config()),
    );
    assert!(result.is_err());
    assert_eq!(std::fs::read_dir(root.path())?.count(), 0);
    Ok(())
}

#[test]
fn declared_owner_provider_demand_is_checked_before_native_state_creation() -> TestResult {
    let (entered_tx, _entered_rx) = mpsc::sync_channel(1);
    let gate = Arc::new(ProviderGate {
        entered: entered_tx,
        cancellation_notice: None,
        released: Mutex::new(true),
        wake: Condvar::new(),
        calls: AtomicUsize::new(0),
    });
    let zero_root = private_root()?;
    let zero = NativeIndexService::bind_with_registry_and_pump(
        zero_root.path(),
        &zero_root.path().join("native.sock"),
        limits(),
        registry_with_provider(&gate, 0)?,
        resources(),
        Some(config()),
    )?;
    drop(zero);

    let missing_root = private_root()?;
    let missing = NativeIndexService::bind_with_registry_and_pump(
        missing_root.path(),
        &missing_root.path().join("native.sock"),
        limits(),
        registry_with_provider(&gate, 1)?,
        resources(),
        Some(config()),
    );
    assert!(missing.is_err());
    assert_eq!(std::fs::read_dir(missing_root.path())?.count(), 0);

    let provider_root = private_root()?;
    let mut reserved = resources();
    reserved.execution.capacity.provider_units = 1;
    reserved.execution.enrichment.provider_units = 1;
    let present = NativeIndexService::bind_with_registry_and_pump(
        provider_root.path(),
        &provider_root.path().join("native.sock"),
        limits(),
        registry_with_provider(&gate, 1)?,
        reserved,
        Some(config()),
    )?;
    drop(present);
    Ok(())
}

#[derive(Debug)]
struct PanickingCadence;

impl NativePumpPolicy for PanickingCadence {
    fn decide(&self, _observation: &NativePumpObservation) -> NativePumpDecision {
        std::panic::resume_unwind(Box::new("test cadence panic"))
    }
}

#[derive(Debug)]
struct ArmedPanickingCadence(Arc<AtomicBool>);

impl NativePumpPolicy for ArmedPanickingCadence {
    fn decide(&self, _observation: &NativePumpObservation) -> NativePumpDecision {
        if self.0.load(Ordering::Acquire) {
            std::panic::resume_unwind(Box::new("test active cadence panic"));
        }
        NativePumpDecision::Poll
    }
}

#[test]
fn cadence_panic_after_grant_cancels_and_settles_active_owned_work() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (cancelled_tx, cancelled_rx) = mpsc::sync_channel(1);
    let gate = Arc::new(ProviderGate {
        entered: entered_tx,
        cancellation_notice: Some(cancelled_tx),
        released: Mutex::new(false),
        wake: Condvar::new(),
        calls: AtomicUsize::new(0),
    });
    let armed = Arc::new(AtomicBool::new(false));
    let mut pump_config = config();
    pump_config.cadence = Arc::new(ArmedPanickingCadence(Arc::clone(&armed)));
    let service = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        resources(),
        Some(pump_config),
    )?;
    let shutdown = service.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(service.run(Duration::from_secs(60)));
    });
    let mut guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    let _release = ReleaseProvider(Arc::clone(&gate));
    let client = NativeUdsClient::new(socket);
    let configured = configure(&client)?;
    let _pending = advance(&client, configured, 2, 1)?;
    let intent = entered_rx.recv_timeout(Duration::from_secs(5))?;
    assert!(matches!(
        client.request(NativeIndexRequest::WorkStatus(intent), &budget())?,
        NativeIndexResponse::WorkStatus(WorkStatus::Claimed(_))
    ));
    armed.store(true, Ordering::Release);
    cancelled_rx.recv_timeout(Duration::from_secs(5))?;
    gate.release();
    let result = finished_rx.recv_timeout(Duration::from_secs(5))?;
    guard.stop()?;
    let error = result
        .err()
        .ok_or("active cadence panic was not returned")?;
    assert!(
        error
            .to_string()
            .contains("native owned work pump panicked")
    );
    let connection = read_catalog(root.path())?;
    let active: i64 =
        connection.query_row("SELECT active FROM native_owned_work_v1", [], |row| {
            row.get(0)
        })?;
    assert_eq!(active, 0);
    connection.close().map_err(|(_connection, error)| error)?;
    Ok(())
}

#[test]
fn cadence_panic_is_supervised_and_releases_the_service_root() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let mut pump_config = config();
    pump_config.cadence = Arc::new(PanickingCadence);
    let service = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        AdapterRegistry::new(),
        resources(),
        Some(pump_config),
    )?;
    let error = service
        .run(Duration::from_secs(60))
        .err()
        .ok_or("cadence panic was not propagated")?;
    assert!(error.to_string().contains("owned work pump panicked"));
    let reopened = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        AdapterRegistry::new(),
        resources(),
        None,
    )?;
    drop(reopened);
    Ok(())
}

fn native_object_file(
    root: &std::path::Path,
    reference: ObjectRef,
) -> TestResult<std::path::PathBuf> {
    let bytes = (reference.0).0;
    let mut name = String::with_capacity(64);
    for byte in bytes {
        std::fmt::Write::write_fmt(&mut name, format_args!("{byte:02x}"))?;
    }
    Ok(root
        .join("native-objects")
        .join("objects")
        .join(format!(
            "{:02x}",
            bytes.first().copied().unwrap_or_default()
        ))
        .join(name))
}

fn native_object_absent_nofollow(path: &std::path::Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Ok(_) => Ok(false),
        Err(error) => Err(error),
    }
}

#[test]
fn owned_worker_reserves_declared_provider_units_below_class_envelope() -> TestResult {
    for provider_units in [0, 1] {
        let root = private_root()?;
        let socket = root.path().join("native.sock");
        let (entered_tx, entered_rx) = mpsc::sync_channel(1);
        let gate = Arc::new(ProviderGate {
            entered: entered_tx,
            cancellation_notice: None,
            released: Mutex::new(false),
            wake: Condvar::new(),
            calls: AtomicUsize::new(0),
        });
        let mut reserved = resources();
        reserved.execution.capacity.provider_units = 1;
        reserved.execution.enrichment.provider_units = 1;
        let service = NativeIndexService::bind_with_registry_and_pump(
            root.path(),
            &socket,
            limits(),
            registry_with_provider(&gate, provider_units)?,
            reserved,
            Some(config()),
        )?;
        let observed = service.resource_observation();
        let shutdown = service.shutdown_handle();
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);
        let runner = thread::spawn(move || {
            let _sent = finished_tx.send(service.run(Duration::from_secs(60)));
        });
        let mut guard = ServiceThreadGuard {
            shutdown,
            runner: Some(runner),
        };
        let _release = ReleaseProvider(Arc::clone(&gate));
        let client = NativeUdsClient::new(socket);
        let configured = configure(&client)?;
        let _pending = advance(&client, configured, 2, 1)?;
        let intent = entered_rx.recv_timeout(Duration::from_secs(5))?;
        assert_eq!(observed.observe()?.reserved.provider_units, provider_units);
        gate.release();
        let _completed = wait_completed(&client, &intent)?;
        guard.stop()?;
        finished_rx.recv_timeout(Duration::from_secs(5))??;
        assert_eq!(observed.observe()?.reserved.provider_units, 0);
    }
    Ok(())
}

fn wait_completed(
    client: &NativeUdsClient,
    intent: &WorkIntent,
) -> TestResult<quanta_index_core::index::ViewRef> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or("deadline overflow")?;
    loop {
        let response = client
            .request(NativeIndexRequest::WorkStatus(*intent), &budget())
            .map_err(|error| format!("completion status: {error}"))?;
        let NativeIndexResponse::WorkStatus(status) = response else {
            return Err(format!("wrong work-status response: {response:?}").into());
        };
        match status {
            WorkStatus::Completed(view) => return Ok(view),
            WorkStatus::Blocked(reason) => {
                return Err(format!("owned work blocked: {reason:?}").into());
            }
            WorkStatus::Superseded => return Err("owned work was unexpectedly superseded".into()),
            WorkStatus::Unknown | WorkStatus::Pending | WorkStatus::Claimed(_) => {
                if Instant::now() >= deadline {
                    return Err(format!("owned work did not complete: {status:?}").into());
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

fn claim_owner_in(
    client: &NativeUdsClient,
    workspace: WorkspaceKey,
    selected_owner: OwnerKey,
) -> TestResult<WorkClaim> {
    let reply = client.request(
        NativeIndexRequest::ReadyWork {
            workspace,
            epoch: 1,
            limit: 16,
        },
        &budget(),
    )?;
    let NativeIndexResponse::ReadyWork(ready) = reply else {
        return Err(format!("expected ready work, got {reply:?}").into());
    };
    let intent = ready
        .into_iter()
        .find(|intent| {
            matches!(intent.scope, WorkScope::Owner { owner, .. } if owner == selected_owner)
        })
        .ok_or("manual owner work missing")?;
    let reply = client.request(
        NativeIndexRequest::ClaimWork {
            intent,
            lease_millis: 60_000,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Claimed(claim) = reply else {
        return Err(format!("expected manual claim, got {reply:?}").into());
    };
    Ok(claim)
}

#[test]
fn two_direct_enrichment_calls_leave_ipc_ingress_for_core() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (entered_tx, entered_rx) = mpsc::sync_channel(2);
    let gate = Arc::new(ProviderGate {
        entered: entered_tx,
        cancellation_notice: None,
        released: Mutex::new(false),
        wake: Condvar::new(),
        calls: AtomicUsize::new(0),
    });
    let mut pump_config = config();
    pump_config.max_in_flight = 2;
    let mut capacity = resources();
    capacity.execution.capacity.cpu_units = 6;
    capacity.execution.capacity.io_bytes = 6_144;
    capacity.execution.capacity.memory_bytes = capacity
        .execution
        .enrichment
        .memory_bytes
        .checked_mul(7)
        .ok_or("test retained memory capacity overflow")?;
    let service = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        capacity,
        Some(pump_config),
    )?;
    let worker_observation = service.worker_observation();
    let shutdown = service.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(service.run(Duration::from_secs(60)));
    });
    let mut guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    let _release = ReleaseProvider(Arc::clone(&gate));
    let client = NativeUdsClient::new(socket.clone());
    let mut materializations = Vec::new();
    for (workspace, owner_id) in [(other_workspace(), 1), (third_workspace(), 2)] {
        let configured = configure_in(&client, workspace)?;
        let pending = advance_in(&client, workspace, configured, 2, owner_id)?;
        let claim = claim_owner_in(&client, workspace, owner(owner_id))?;
        let operation = OperationKey {
            producer: ProducerId([41; 16]),
            stream: StreamId([42; 16]),
            sequence: 1,
            ..operation_in(workspace, 1)
        };
        let materialize_socket = socket.clone();
        materializations.push(thread::spawn(move || {
            NativeUdsClient::new(materialize_socket).submit(
                UpdateRequest {
                    operation,
                    expected_view: pending,
                    kind: UpdateKind::Materialize { claim },
                },
                &budget(),
            )
        }));
    }
    let _first_intent = entered_rx.recv_timeout(Duration::from_secs(5))?;
    let _second_intent = entered_rx.recv_timeout(Duration::from_secs(5))?;
    assert_eq!(
        worker_observation
            .observe(Duration::from_secs(1))?
            .enrichment
            .active,
        2
    );
    let _core_configured = configure_in(&client, workspace())?;
    gate.release();
    for materialize in materializations {
        let outcome = materialize
            .join()
            .map_err(|_panic| "direct materialize thread panicked")??;
        let _published = published(outcome)?;
    }
    guard.stop()?;
    finished_rx.recv_timeout(Duration::from_secs(5))??;
    assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn batch_one_dispatches_distinct_scopes_while_first_provider_is_blocked() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (entered_tx, entered_rx) = mpsc::sync_channel(2);
    let gate = Arc::new(ProviderGate {
        entered: entered_tx,
        cancellation_notice: None,
        released: Mutex::new(false),
        wake: Condvar::new(),
        calls: AtomicUsize::new(0),
    });
    let mut pump_config = config();
    pump_config.scopes.push(NativePumpScope {
        workspace: other_workspace(),
        epoch: 1,
    });
    pump_config.batch_limit = 1;
    pump_config.max_in_flight = 2;
    let mut capacity = resources();
    capacity.execution.capacity.cpu_units = 6;
    capacity.execution.capacity.io_bytes = 6_144;
    capacity.execution.capacity.memory_bytes = capacity
        .execution
        .enrichment
        .memory_bytes
        .checked_mul(7)
        .ok_or("test retained memory capacity overflow")?;
    let service = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        capacity,
        Some(pump_config),
    )?;
    let worker_observation = service.worker_observation();
    let resource_observation = service.resource_observation();
    let shutdown = service.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(service.run(Duration::from_secs(60)));
    });
    let mut guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    let _release = ReleaseProvider(Arc::clone(&gate));
    let client = NativeUdsClient::new(socket);
    let first = configure_in(&client, workspace())?;
    let second = configure_in(&client, other_workspace())?;
    let _first_pending = advance_in(&client, workspace(), first, 2, 1)?;
    let _second_pending = advance_in(&client, other_workspace(), second, 2, 2)?;
    let entered_one = entered_rx.recv_timeout(Duration::from_secs(5))?;
    let entered_two = entered_rx.recv_timeout(Duration::from_secs(5))?;
    let active = worker_observation.observe(Duration::from_secs(1))?;
    assert_eq!(active.enrichment.active, 2);
    assert_eq!(resource_observation.observe()?.enrichment.active, 2);
    gate.release();
    for mut intent in [entered_one, entered_two] {
        let WorkScope::Owner { owner, .. } = intent.scope else {
            return Err("provider reported a non-owner work scope".into());
        };
        if owner.stable_id == StableOwnerId([2; 16]) {
            intent.workspace = other_workspace();
        }
        let _completed = wait_completed(&client, &intent)?;
    }
    guard.stop()?;
    finished_rx.recv_timeout(Duration::from_secs(5))??;
    assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
    assert_eq!(resource_observation.observe()?.reserved.cpu_units, 0);
    Ok(())
}

#[test]
fn empty_first_scope_does_not_spend_batch_one_dispatch_slot() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let gate = Arc::new(ProviderGate {
        entered: entered_tx,
        cancellation_notice: None,
        released: Mutex::new(false),
        wake: Condvar::new(),
        calls: AtomicUsize::new(0),
    });
    let seed = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        resources(),
        None,
    )?;
    let seed_shutdown = seed.shutdown_handle();
    let (seed_tx, seed_rx) = mpsc::sync_channel(1);
    let seed_runner = thread::spawn(move || {
        let _sent = seed_tx.send(seed.run(Duration::from_millis(20)));
    });
    let mut seed_guard = ServiceThreadGuard {
        shutdown: seed_shutdown,
        runner: Some(seed_runner),
    };
    let client = NativeUdsClient::new(socket.clone());
    let configured = configure_in(&client, other_workspace())?;
    let _pending = advance_in(&client, other_workspace(), configured, 2, 2)?;
    seed_guard.stop()?;
    seed_rx.recv_timeout(Duration::from_secs(5))??;

    let mut pump_config = config();
    pump_config.scopes.push(NativePumpScope {
        workspace: other_workspace(),
        epoch: 1,
    });
    pump_config.batch_limit = 1;
    pump_config.poll_interval = Duration::from_secs(60);
    let service = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        resources(),
        Some(pump_config),
    )?;
    let shutdown = service.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(service.run(Duration::from_secs(60)));
    });
    let mut guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    let _release = ReleaseProvider(Arc::clone(&gate));
    let mut intent = entered_rx.recv_timeout(Duration::from_secs(5))?;
    intent.workspace = other_workspace();
    gate.release();
    let _completed = wait_completed(&client, &intent)?;
    guard.stop()?;
    finished_rx.recv_timeout(Duration::from_secs(5))??;
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn completed_owned_run_wakes_same_scope_backlog_before_long_poll() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (entered_tx, entered_rx) = mpsc::sync_channel(2);
    let gate = Arc::new(ProviderGate {
        entered: entered_tx,
        cancellation_notice: None,
        released: Mutex::new(false),
        wake: Condvar::new(),
        calls: AtomicUsize::new(0),
    });
    let seed = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        resources(),
        None,
    )?;
    let seed_shutdown = seed.shutdown_handle();
    let (seed_tx, seed_rx) = mpsc::sync_channel(1);
    let seed_runner = thread::spawn(move || {
        let _sent = seed_tx.send(seed.run(Duration::from_millis(20)));
    });
    let mut seed_guard = ServiceThreadGuard {
        shutdown: seed_shutdown,
        runner: Some(seed_runner),
    };
    let client = NativeUdsClient::new(socket.clone());
    let configured = configure(&client)?;
    let first_pending = advance(&client, configured, 2, 1)?;
    let _second_pending = advance(&client, first_pending, 3, 2)?;
    seed_guard.stop()?;
    seed_rx.recv_timeout(Duration::from_secs(5))??;

    let mut pump_config = config();
    pump_config.batch_limit = 1;
    pump_config.poll_interval = Duration::from_secs(60);
    let service = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        resources(),
        Some(pump_config),
    )?;
    let shutdown = service.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(service.run(Duration::from_secs(60)));
    });
    let mut guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    let _release = ReleaseProvider(Arc::clone(&gate));
    let first_intent = entered_rx.recv_timeout(Duration::from_secs(5))?;
    gate.release();
    let _first_completed = wait_completed(&client, &first_intent)?;
    let second_intent = entered_rx.recv_timeout(Duration::from_secs(5))?;
    let _second_completed = wait_completed(&client, &second_intent)?;
    guard.stop()?;
    finished_rx.recv_timeout(Duration::from_secs(5))??;
    assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn native_owned_pump_renews_and_completes_without_external_claim_or_submit() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (entered_tx, entered_rx) = mpsc::sync_channel(4);
    let gate = Arc::new(ProviderGate {
        entered: entered_tx,
        cancellation_notice: None,
        released: Mutex::new(false),
        wake: Condvar::new(),
        calls: AtomicUsize::new(0),
    });
    let pump_config = config();
    // Exercise the actual product lease. The deterministic renewal tests cover
    // subsecond cadence; a shared-host 200ms fixture can legitimately fence the
    // first operation and test retry instead of this same-receipt contract.
    let provider_hold = Duration::from_millis(pump_config.new_assignment.lease_ms)
        .checked_add(Duration::from_millis(250))
        .ok_or("renewal fixture hold overflow")?;
    let service = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        resources(),
        Some(pump_config),
    )?;
    let resource_observation = service.resource_observation();
    let shutdown = service.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(service.run(Duration::from_secs(60)));
    });
    let mut guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    let _release = ReleaseProvider(Arc::clone(&gate));
    let client = NativeUdsClient::new(socket.clone());
    let configured = configure(&client).map_err(|error| format!("configure: {error}"))?;
    let pending_view = advance(&client, configured, 2, 1)
        .map_err(|error| format!("first source advance: {error}"))?;
    let intent = entered_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| format!("first provider entry: {error}"))?;
    let NativeIndexResponse::WorkStatus(WorkStatus::Claimed(initial)) = client
        .request(NativeIndexRequest::WorkStatus(intent), &budget())
        .map_err(|error| format!("claim/renew status: {error}"))?
    else {
        return Err("expected claim".into());
    };
    std::thread::sleep(provider_hold);
    let NativeIndexResponse::WorkStatus(WorkStatus::Claimed(renewed)) = client
        .request(NativeIndexRequest::WorkStatus(intent), &budget())
        .map_err(|error| format!("claim/renew status: {error}"))?
    else {
        return Err("expected renewed claim".into());
    };
    assert_eq!(renewed.fence, initial.fence);
    assert!(renewed.lease_until_ms > initial.lease_until_ms);
    assert!(matches!(
        client.request(NativeIndexRequest::RenewWork { claim: renewed, lease_millis: 300_000 }, &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
    ));
    // Bind the original receipt while this fixture has only its first owner.
    // A later source advance may create another active assignment; an
    // unqualified active-row query would then select a different work item.
    let database = read_catalog(root.path())?;
    let payload: Vec<u8> = database.query_row(
        "SELECT payload FROM native_owned_work_v1 WHERE active=1",
        [],
        |row| row.get(0),
    )?;
    database.close().map_err(|(_connection, error)| error)?;
    let assignment: quanta_index_core::index::OwnedWorkAssignment =
        serde_json::from_slice(&payload)?;
    let key = assignment.request.operation;
    assert!(matches!(
        &assignment.request.kind,
        UpdateKind::Materialize { claim }
            if claim.intent == intent && claim.fence == initial.fence
    ));
    // Core publication and Control status proceed while Enrichment is blocked.
    let second_view = advance(&client, pending_view, 3, 2)
        .map_err(|error| format!("second source advance: {error}"))?;
    assert_eq!(
        client
            .status(operation(3), &budget())
            .map_err(|error| format!("core receipt: {error}"))?,
        OperationStatus::Published(second_view)
    );
    assert!(resource_observation.observe()?.reserved.cpu_units > 0);
    assert!(matches!(
        client.request(NativeIndexRequest::Resume(key), &budget()),
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::InvalidInput
    ));
    gate.release();
    let completed = wait_completed(&client, &intent)?;
    let original_receipt = client
        .status(key, &budget())
        .map_err(|error| format!("owned receipt: {error}"))?;
    if original_receipt != OperationStatus::Published(completed) {
        // Read diagnostics only after a failure. Extra catalog/UDS requests
        // during the active lease would change the race being diagnosed.
        let current_work = client.request(NativeIndexRequest::WorkStatus(intent), &budget());
        let database = read_catalog(root.path())?;
        let catalog_now: Result<i64, _> = database.query_row(
            "SELECT last_ms FROM native_work_clock_v1 WHERE singleton=1",
            [],
            |row| row.get(0),
        );
        let current_assignment: Result<Vec<u8>, _> =
            database.query_row("SELECT payload FROM native_owned_work_v1", [], |row| {
                row.get(0)
            });
        database.close().map_err(|(_connection, error)| error)?;
        let active_assignment_read = current_assignment
            .as_ref()
            .map(Vec::len)
            .map_err(ToString::to_string);
        let decoded_assignment = current_assignment
            .as_ref()
            .map_err(ToString::to_string)
            .and_then(|payload| {
                serde_json::from_slice::<quanta_index_core::index::OwnedWorkAssignment>(payload)
                    .map_err(|error| error.to_string())
            });
        return Err(format!(
            "original owned receipt {original_receipt:?}, expected Published({completed:?}); \
             initial_claim={initial:?}, observed_renewal={renewed:?}, \
             current_work={current_work:?}, catalog_now={catalog_now:?}, \
             active_assignment={decoded_assignment:?}, \
             active_assignment_read={active_assignment_read:?}"
        )
        .into());
    }
    assert_eq!(
        client
            .read_view(ViewSelection::Exact(pending_view), &budget())
            .map_err(|error| format!("historical read: {error}"))?
            .identity,
        pending_view
    );
    let second_intent = entered_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| format!("second provider entry: {error}"))?;
    let _second_completed = wait_completed(&client, &second_intent)?;
    guard.stop()?;
    finished_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| format!("service completion: {error}"))??;
    assert_eq!(resource_observation.observe()?.reserved.cpu_units, 0);
    assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
    // Durable original receipt is available with no new provider execution.
    let reopened = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        resources(),
        None,
    )?;
    let shutdown = reopened.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(reopened.run(Duration::from_millis(20)));
    });
    let mut reopen_guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    assert_eq!(
        client
            .status(key, &budget())
            .map_err(|error| format!("owned receipt: {error}"))?,
        OperationStatus::Published(completed)
    );
    assert_eq!(
        client.request(NativeIndexRequest::WorkStatus(intent), &budget())?,
        NativeIndexResponse::WorkStatus(WorkStatus::Completed(completed))
    );
    reopen_guard.stop()?;
    finished_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| format!("service completion: {error}"))??;
    assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
    Ok(())
}

#[test]
fn native_owned_pump_renews_while_automatic_reclaim_deletes_discarded_input() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let gate = Arc::new(ProviderGate {
        entered: entered_tx,
        cancellation_notice: None,
        released: Mutex::new(false),
        wake: Condvar::new(),
        calls: AtomicUsize::new(0),
    });
    let mut pump_config = config();
    pump_config.new_assignment.lease_ms = 500;
    pump_config.new_assignment.total_timeout_ms = 20_000;
    let mut reserved = resources();
    reserved.execution.control.io_bytes = 128 * 1024 * 1024;
    reserved.execution.control.memory_bytes = 8 * 1024 * 1024;
    reserved.execution.capacity.cpu_units = 6;
    reserved.execution.capacity.io_bytes = 512 * 1024 * 1024;
    reserved.execution.capacity.memory_bytes = 32 * 1024 * 1024;
    let gc_limits = WorkLimits {
        reads: 1_000_000,
        writes: 1_000_000,
        bytes: 512 * 1024 * 1024,
        mutations: 1_000_000,
    };
    let reclaim = NativeReclaimConfig {
        interval: Duration::from_millis(20),
        max_steps_per_round: 1,
        child_limit: 32,
        work_limits: gc_limits,
        round_timeout: Duration::from_secs(5),
        head_probe_every: 4,
    };
    let service = NativeIndexService::bind_with_capabilities_and_maintenance(
        root.path(),
        &socket,
        limits(),
        NativeServiceCapabilities {
            builders: registry(&gate)?,
            literal: QueryAdapterRegistry::new(),
            path: QueryAdapterRegistry::new(),
            text: QueryAdapterRegistry::new(),
            semantic: QueryAdapterRegistry::new(),
            pump: Some(pump_config),
            provider_attempt_pool: None,
        },
        reserved,
        Some(reclaim),
    )?;
    let observed = service.resource_observation();
    let shutdown = service.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(service.run(Duration::from_secs(60)));
    });
    let mut guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    let _release = ReleaseProvider(Arc::clone(&gate));
    let client = NativeUdsClient::new(socket);
    let configured = configure(&client)?;
    let _pending = advance(&client, configured, 2, 1)?;
    let intent = entered_rx.recv_timeout(Duration::from_secs(5))?;
    let NativeIndexResponse::WorkStatus(WorkStatus::Claimed(initial)) =
        client.request(NativeIndexRequest::WorkStatus(intent), &budget())?
    else {
        return Err("owned provider was not claimed before reclaim".into());
    };
    let database = read_catalog(root.path())?;
    let assignment_before: Vec<u8> = database.query_row(
        "SELECT payload FROM native_owned_work_v1 WHERE active=1",
        [],
        |row| row.get(0),
    )?;
    database.close().map_err(|(_connection, error)| error)?;
    let assignment_before: quanta_index_core::index::OwnedWorkAssignment =
        serde_json::from_slice(&assignment_before)?;

    let discarded_body = b"owned pump discarded input";
    let discarded_ingress = client.begin_input(operation(401), &budget())?;
    let discarded_record = client.stage_source(
        discarded_ingress,
        "discarded.rs".into(),
        discarded_body,
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let held_body = b"owned pump retained input";
    let held_ingress = client.begin_input(operation(402), &budget())?;
    let held_record = client.stage_source(
        held_ingress,
        "retained.rs".into(),
        held_body,
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let identity_root = private_root()?;
    let identity = FileObjectStore::open(identity_root.path().join("identity-objects"))?;
    let discarded_content = identity
        .identify(
            discarded_body,
            &[],
            &mut WorkBudget::with_request(gc_limits, budget()),
        )?
        .reference;
    let held_content = identity
        .identify(
            held_body,
            &[],
            &mut WorkBudget::with_request(gc_limits, budget()),
        )?
        .reference;
    let discarded_record_file = native_object_file(root.path(), discarded_record)?;
    let discarded_content_file = native_object_file(root.path(), discarded_content)?;
    let held_record_file = native_object_file(root.path(), held_record)?;
    let held_content_file = native_object_file(root.path(), held_content)?;
    assert!(discarded_record_file.is_file());
    assert!(discarded_content_file.is_file());
    assert!(held_record_file.is_file());
    assert!(held_content_file.is_file());
    client.discard_input(discarded_ingress, &budget())?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(5))
        .ok_or("reclaim deadline overflow")?;
    while (!native_object_absent_nofollow(&discarded_record_file)?
        || !native_object_absent_nofollow(&discarded_content_file)?)
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        native_object_absent_nofollow(&discarded_record_file)?,
        "automatic GC retained discarded record"
    );
    assert!(
        native_object_absent_nofollow(&discarded_content_file)?,
        "automatic GC retained discarded body"
    );
    assert!(
        held_record_file.is_file(),
        "automatic GC deleted held record"
    );
    assert!(
        held_content_file.is_file(),
        "automatic GC deleted held body"
    );

    thread::sleep(Duration::from_millis(600));
    let NativeIndexResponse::WorkStatus(WorkStatus::Claimed(renewed)) =
        client.request(NativeIndexRequest::WorkStatus(intent), &budget())?
    else {
        return Err("owned claim did not survive automatic reclaim".into());
    };
    assert_eq!(renewed.fence, initial.fence);
    assert!(renewed.lease_until_ms > initial.lease_until_ms);
    let database = read_catalog(root.path())?;
    let assignment_after: Vec<u8> = database.query_row(
        "SELECT payload FROM native_owned_work_v1 WHERE active=1",
        [],
        |row| row.get(0),
    )?;
    database.close().map_err(|(_connection, error)| error)?;
    let assignment_after: quanta_index_core::index::OwnedWorkAssignment =
        serde_json::from_slice(&assignment_after)?;
    assert_eq!(
        assignment_after.request.operation,
        assignment_before.request.operation
    );
    assert_eq!(assignment_after.dispatches, assignment_before.dispatches);
    assert!(held_record_file.is_file());
    assert!(held_content_file.is_file());
    gate.release();
    let completed = wait_completed(&client, &intent)?;
    assert_eq!(
        client.status(assignment_before.request.operation, &budget())?,
        OperationStatus::Published(completed)
    );
    guard.stop()?;
    finished_rx.recv_timeout(Duration::from_secs(5))??;
    assert_eq!(observed.observe()?.reserved.cpu_units, 0);
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn owned_shutdown_keeps_resources_and_root_until_provider_quiescence_then_resumes() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (entered_tx, entered_rx) = mpsc::sync_channel(4);
    let gate = Arc::new(ProviderGate {
        entered: entered_tx,
        cancellation_notice: None,
        released: Mutex::new(false),
        wake: Condvar::new(),
        calls: AtomicUsize::new(0),
    });
    let mut pump_config = config();
    pump_config.new_assignment.lease_ms = 10_000;
    pump_config.new_assignment.total_timeout_ms = 30_000;
    let service = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        resources(),
        Some(pump_config.clone()),
    )?;
    let observation = service.resource_observation();
    let shutdown = service.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(service.run(Duration::from_millis(20)));
    });
    let mut guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    let _release = ReleaseProvider(Arc::clone(&gate));
    let client = NativeUdsClient::new(socket.clone());
    let configured = configure(&client).map_err(|error| format!("configure: {error}"))?;
    let _pending = advance(&client, configured, 2, 1)?;
    let intent = entered_rx.recv_timeout(Duration::from_secs(5))?;
    let database = read_catalog(root.path())?;
    let payload: Vec<u8> = database.query_row(
        "SELECT payload FROM native_owned_work_v1 WHERE active=1",
        [],
        |row| row.get(0),
    )?;
    let before: quanta_index_core::index::OwnedWorkAssignment = serde_json::from_slice(&payload)?;
    database.close().map_err(|(_connection, error)| error)?;
    assert_eq!(before.dispatches, 1);
    let key = before.request.operation;
    guard.shutdown.trigger();
    // Cancellation cannot release the reservation or namespace while this
    // deliberately uncooperative provider still owns preparation handles.
    assert!(matches!(
        finished_rx.recv_timeout(Duration::from_millis(150)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    assert!(observation.observe()?.reserved.cpu_units > 0);
    let locked = StateRootLease::acquire(root.path())
        .err()
        .ok_or("state root was released before provider quiescence")?;
    assert!(matches!(
        locked,
        CoreError::Typed {
            code: SearchPlaneErrorCodeV2::StateRootInUse,
            ..
        }
    ));
    gate.release();
    finished_rx.recv_timeout(Duration::from_secs(5))??;
    guard.stop()?;
    assert_eq!(observation.observe()?.reserved.cpu_units, 0);
    let database = read_catalog(root.path())?;
    let payload: Vec<u8> = database.query_row(
        "SELECT payload FROM native_owned_work_v1 WHERE active=0",
        [],
        |row| row.get(0),
    )?;
    database.close().map_err(|(_connection, error)| error)?;
    let stopped: quanta_index_core::index::OwnedWorkAssignment = serde_json::from_slice(&payload)?;
    assert_eq!(stopped.request, before.request);
    assert_eq!(stopped.dispatches, 1);
    assert_eq!(stopped.deadline_ms, before.deadline_ms);
    // New configuration cannot replenish the durable allowance.
    pump_config.new_assignment.max_dispatches = 64;
    let reopened = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        resources(),
        Some(pump_config),
    )?;
    let shutdown = reopened.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(reopened.run(Duration::from_millis(20)));
    });
    let mut reopened_guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    assert_eq!(entered_rx.recv_timeout(Duration::from_secs(5))?, intent);
    let completed = wait_completed(&client, &intent)?;
    assert_eq!(
        client
            .status(key, &budget())
            .map_err(|error| format!("owned receipt: {error}"))?,
        OperationStatus::Published(completed)
    );
    let database = read_catalog(root.path())?;
    let payload: Vec<u8> =
        database.query_row("SELECT payload FROM native_owned_work_v1", [], |row| {
            row.get(0)
        })?;
    database.close().map_err(|(_connection, error)| error)?;
    let after: quanta_index_core::index::OwnedWorkAssignment = serde_json::from_slice(&payload)?;
    assert_eq!(after.request, before.request);
    assert_eq!(after.policy, before.policy);
    assert_eq!(after.deadline_ms, before.deadline_ms);
    assert_eq!(after.dispatches, 2);
    reopened_guard.stop()?;
    finished_rx.recv_timeout(Duration::from_secs(5))??;
    assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
    Ok(())
}
#[test]
fn owned_scan_is_independent_of_foreground_request_limits() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let gate = Arc::new(ProviderGate {
        entered: entered_tx,
        cancellation_notice: None,
        released: Mutex::new(true),
        wake: Condvar::new(),
        calls: AtomicUsize::new(0),
    });
    // Prepare a real pending work row under ordinary foreground limits.
    let first = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        resources(),
        None,
    )?;
    let shutdown = first.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(first.run(Duration::from_millis(20)));
    });
    let mut first_guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    let client = NativeUdsClient::new(socket.clone());
    let configured = configure(&client)?;
    let _pending_view = advance(&client, configured, 2, 1)?;
    first_guard.stop()?;
    finished_rx.recv_timeout(Duration::from_secs(5))??;

    // The foreground byte allowance is valid but smaller than a WorkRow.
    // The pump's bounded Catalog scan must still acquire and publish it.
    let foreground = WorkLimits {
        reads: 1,
        writes: 1,
        bytes: 1,
        mutations: 1,
    };
    let second = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        foreground,
        registry(&gate)?,
        resources(),
        Some(config()),
    )?;
    let shutdown = second.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(second.run(Duration::from_millis(20)));
    });
    let mut second_guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    let intent = entered_rx.recv_timeout(Duration::from_secs(5))?;
    let completed = wait_completed(&client, &intent)?;
    assert_eq!(
        client.request(NativeIndexRequest::WorkStatus(intent), &budget())?,
        NativeIndexResponse::WorkStatus(WorkStatus::Completed(completed))
    );
    second_guard.stop()?;
    finished_rx.recv_timeout(Duration::from_secs(5))??;
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    Ok(())
}
