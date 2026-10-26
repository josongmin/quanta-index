//! Real renewal SQL failure through the service's existing admitted VFS.
#![expect(
    clippy::panic_in_result_fn,
    reason = "fixed lifecycle assertions are independent behavior oracles"
)]

use crate as native_runtime;
#[path = "../tests/common/native_owned_pump_fixture.rs"]
mod fixture;
use crate::{native::NativeIndexService, native_scratch_io::NativeScratchIo};
use fixture::{
    ProviderGate, ReleaseProvider, ServiceThreadGuard, TestResult, advance, config, configure,
    limits, private_root, registry, resources, workspace,
};
use quanta_index_catalog::{CATALOG_FILE_NAME, CatalogIoAdmission};
use quanta_index_contract::{
    SearchPlaneErrorCodeV2,
    native_protocol::{NativeIndexRequest, NativeIndexResponse},
};
use quanta_index_core::CoreError;
use quanta_index_core::RequestBudgetV1;
use quanta_index_kernel::AdapterRegistry;
use quanta_index_sdk::native_uds::NativeUdsClient;
use quanta_index_searchd::app::runtime::StateRootLease;
use std::{
    path::Path,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

fn read_catalog(root: &Path) -> rusqlite::Result<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(
        root.join("catalog").join(CATALOG_FILE_NAME),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
}

fn write_catalog(io: &NativeScratchIo, root: &Path, sql: &str) -> TestResult {
    let _funding = io.enter_maintenance_scope()?;
    let _scope = io.enter_scope(None);
    let database = rusqlite::Connection::open_with_flags_and_vfs(
        root.join("catalog").join(CATALOG_FILE_NAME),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        io.vfs_name(),
    )?;
    database.busy_timeout(Duration::from_secs(2))?;
    let written = database.execute_batch(sql);
    let closed = database.close().map_err(|(_connection, error)| error);
    io.drain_retirements()?;
    written?;
    closed?;
    Ok(())
}

#[test]
fn owned_renewal_storage_failure_returns_error_after_provider_quiescence() -> TestResult {
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
    let mut pump_config = config();
    pump_config.new_assignment.lease_ms = 6_000;
    pump_config.new_assignment.total_timeout_ms = 30_000;
    let service = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        resources(),
        Some(pump_config),
    )?;
    let observation = service.resource_observation();
    let io = Arc::clone(&service.dispatcher.io);
    let shutdown = service.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(service.run(Duration::from_secs(60)));
    });
    let mut guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    // This guard drops before the service guard on any early assertion failure,
    // releasing the deliberately uncooperative provider before joining it.
    let _release = ReleaseProvider(Arc::clone(&gate));
    let client = NativeUdsClient::new(socket.clone());
    let configured = configure(&client)?;
    // The same named VFS and maintenance reserve own this real SQL fault.
    write_catalog(
        &io,
        root.path(),
        "CREATE TRIGGER fail_owned_renewal
         BEFORE UPDATE ON native_owned_work_v1
         WHEN OLD.active=1 AND NEW.active=1
         BEGIN SELECT RAISE(ABORT, 'owned-renewal-storage-fault'); END;",
    )?;
    let _pending = advance(&client, configured, 2, 1)?;
    let _intent = entered_rx.recv_timeout(Duration::from_secs(5))?;
    let database = read_catalog(root.path())?;
    let active: i64 = database.query_row(
        "SELECT active FROM native_owned_work_v1 WHERE active=1",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(active, 1);
    database.close().map_err(|(_connection, error)| error)?;
    // The provider's own request budget reports cancellation after the real
    // catalog renewal hits the trigger; no elapsed-time guess marks success.
    cancelled_rx.recv_timeout(Duration::from_secs(10))?;
    // Remove the injected schema object before retirement and cold validation.
    // All handles close while the service still owns the active provider.
    write_catalog(&io, root.path(), "DROP TRIGGER fail_owned_renewal")?;
    drop(io);
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
    let outcome = finished_rx.recv_timeout(Duration::from_secs(5))?;
    let error = outcome.err().ok_or("renewal storage error was swallowed")?;
    assert!(error.to_string().contains("owned-renewal-storage-fault"));
    guard.stop()?;
    assert_eq!(observation.observe()?.reserved.cpu_units, 0);
    let database = read_catalog(root.path())?;
    let inactive: i64 =
        database.query_row("SELECT active FROM native_owned_work_v1", [], |row| {
            row.get(0)
        })?;
    assert_eq!(inactive, 0);
    database.close().map_err(|(_connection, error)| error)?;
    let reopened = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        resources(),
        None,
    )?;
    drop(reopened);
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    Ok(())
}

#[test]
fn fatal_catalog_failure_stops_listener_and_returns_pump_error() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let service = NativeIndexService::bind_with_registry_and_pump(
        root.path(),
        &socket,
        limits(),
        AdapterRegistry::new(),
        resources(),
        Some(config()),
    )?;
    let io = Arc::clone(&service.dispatcher.io);
    let shutdown = service.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let outcome = service.run(Duration::from_secs(60));
        let _sent = finished_tx.send(outcome);
    });
    let mut guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    let client = NativeUdsClient::new(socket);
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &RequestBudgetV1::for_duration(Duration::from_secs(5)),
    )?;
    assert!(matches!(created, NativeIndexResponse::Created(_)));
    write_catalog(&io, root.path(), "DROP TABLE native_work_v1")?;
    drop(io);
    let stopped = finished_rx.recv_timeout(Duration::from_secs(5));
    guard.stop()?;
    let result = stopped.map_err(|error| format!("pump did not stop the listener: {error}"))?;
    let error = result
        .err()
        .ok_or("corrupt catalog must fail the owned pump")?;
    assert!(error.to_string().contains("native_work_v1"));
    Ok(())
}
