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

#[cfg(not(target_os = "linux"))]
#[test]
fn retirement_precheck_returns_unknown_for_an_unadmitted_resume() -> TestResult {
    use crate::{native::NativeRetirementAuthorityV2, native_product::retirement_approval_v2};
    use quanta_index_contract::index::{
        Digest, HistoryAppendV1, ObjectRef, RetiredProjectionPolicyRefV1, UpdateKind,
        UpdateRequest, ViewRef,
    };
    use quanta_index_core::index::{IndexError, OWNED_WORK_PRODUCER, SubmitOutcome, WorkBudget};

    let root = private_root()?;
    let service = NativeIndexService::bind_with_registry(
        root.path(),
        &root.path().join("native.sock"),
        limits(),
        AdapterRegistry::new(),
        &resources(),
    )?;
    // Exercise the Runtime gate with a real catalog using the non-Linux
    // capability type. Installed Linux signature verification is upstream.
    let retirement = NativeRetirementAuthorityV2 {
        approval: retirement_approval_v2::VerifiedApprovalV2,
        retired_projections: Vec::new(),
        retired_instances: vec![fixture::spec().instance],
        _index_memory: service.dispatcher.resources.reserve_response_memory(1)?,
        policy_ref: RetiredProjectionPolicyRefV1 {
            object: ObjectRef(Digest([1; 32])),
            sha256: Digest([2; 32]),
            identity_count: 1,
        },
    };
    let mut budget = WorkBudget::with_request(limits(), RequestBudgetV1::unbounded());
    let key = fixture::operation(1);
    let reply = service.dispatcher.check_retired_request_v2(
        &retirement,
        &NativeIndexRequest::Resume(key),
        &mut budget,
    )?;
    assert!(matches!(reply, Some(NativeIndexResponse::Submitted(reply))
        if reply.operation == key && reply.payload == SubmitOutcome::Unknown(key)));

    let reserved = fixture::operation(2);
    let reserved = quanta_index_contract::index::OperationKey {
        producer: OWNED_WORK_PRODUCER,
        ..reserved
    };
    let rejected = service.dispatcher.check_retired_request_v2(
        &retirement,
        &NativeIndexRequest::Resume(reserved),
        &mut budget,
    );
    assert!(matches!(rejected, Err(IndexError::InvalidInput(message))
        if message == "reserved owned work producer"));
    let cut = serde_json::from_value(serde_json::json!({
        "generation": {"repo_id": "repo-a", "revision_id": "rev-a", "generation": 1},
        "durable_epoch": 2,
        "snapshot_digest": vec![7_u8; 32],
        "replay_start": vec![8_u8; 32]
    }))?;
    let history = UpdateRequest {
        operation: fixture::operation(3),
        expected_view: ViewRef {
            workspace: workspace(),
            epoch: 1,
            sequence: 0,
            digest: Digest([0; 32]),
        },
        kind: UpdateKind::HistoryAppend(HistoryAppendV1 {
            instance: fixture::spec().instance,
            expected: cut,
            input: ObjectRef(Digest([3; 32])),
            profile: ObjectRef(Digest([4; 32])),
        }),
    };
    let denied = service.dispatcher.check_retired_request_v2(
        &retirement,
        &NativeIndexRequest::Submit(history),
        &mut budget,
    );
    assert!(matches!(denied, Err(IndexError::InvalidInput(message))
        if message == "retired projection or instance identity"));
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[test]
fn retired_source_terminal_replays_after_request_retirement_and_checks_published_profile()
-> TestResult {
    use crate::{native::NativeRetirementAuthorityV2, native_product::retirement_approval_v2};
    use quanta_index_contract::index::{
        Digest, ObjectRef, ProjectionId, RetiredProjectionPolicyRefV1, SourceOperationMode,
    };
    use quanta_index_core::index::{
        IndexError, WorkBudget, effort::ReservationGuard, ports::OperationJournalPort,
    };

    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (entered, _observed) = mpsc::sync_channel(2);
    let gate = Arc::new(ProviderGate {
        entered,
        cancellation_notice: None,
        released: Mutex::new(true),
        wake: Condvar::new(),
        calls: AtomicUsize::new(0),
    });
    let service = NativeIndexService::bind_with_registry(
        root.path(),
        &socket,
        limits(),
        registry(&gate)?,
        &resources(),
    )?;
    let dispatcher = Arc::clone(&service.dispatcher);
    let io = Arc::clone(&dispatcher.io);
    let shutdown = service.shutdown_handle();
    let (finished_tx, finished_rx) = mpsc::sync_channel(1);
    let runner = thread::spawn(move || {
        let _sent = finished_tx.send(service.run(Duration::from_millis(100)));
    });
    let mut guard = ServiceThreadGuard {
        shutdown,
        runner: Some(runner),
    };
    let client = NativeUdsClient::new(socket);
    let configured = configure(&client)?;
    let published = advance(&client, configured, 2, 1)?;
    let key = fixture::operation(2);
    let saved = dispatcher.catalog.operation_request(
        key,
        &WorkBudget::with_request(limits(), RequestBudgetV1::unbounded()),
    )?;
    assert_eq!(saved.source_mode, SourceOperationMode::Advance);

    let request_hex: String = saved
        .request
        .0
        .0
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    write_catalog(
        &io,
        root.path(),
        &format!(
            "DELETE FROM native_operation_object_v1 WHERE object=x'{request_hex}';
         DELETE FROM native_object_edge_v1 WHERE parent=x'{request_hex}';
         DELETE FROM native_object_v1 WHERE object=x'{request_hex}';"
        ),
    )?;
    std::fs::remove_file(
        root.path()
            .join("native-objects")
            .join("objects")
            .join(&request_hex[..2])
            .join(&request_hex),
    )?;

    let retirement = |retired_instances| -> TestResult<NativeRetirementAuthorityV2> {
        Ok(NativeRetirementAuthorityV2 {
            approval: retirement_approval_v2::VerifiedApprovalV2,
            retired_projections: vec![ProjectionId([99; 16])],
            retired_instances,
            _index_memory: dispatcher.resources.reserve_response_memory(1)?,
            policy_ref: RetiredProjectionPolicyRefV1 {
                object: ObjectRef(Digest([1; 32])),
                sha256: Digest([2; 32]),
                identity_count: 1,
            },
        })
    };
    let reservation = ReservationGuard::try_acquire(
        Arc::clone(&dispatcher.control_scheduler),
        dispatcher.control_need,
    )
    .ok_or("control admission refused")?;
    let mut work = WorkBudget::with_request(limits(), RequestBudgetV1::unbounded());
    work.bind_memory_scope(reservation.memory_scope());
    let permitted = dispatcher.check_retired_request_v2(
        &retirement(Vec::new())?,
        &NativeIndexRequest::Resume(key),
        &mut work,
    )?;
    assert!(
        matches!(permitted, Some(NativeIndexResponse::Submitted(reply))
        if reply.operation == key && reply.payload == quanta_index_contract::index::SubmitOutcome::Terminal(
            quanta_index_contract::index::OperationStatus::Published(published)
        ))
    );
    let denied = dispatcher.check_retired_request_v2(
        &retirement(vec![fixture::spec().instance])?,
        &NativeIndexRequest::Resume(key),
        &mut work,
    );
    assert!(matches!(denied, Err(IndexError::InvalidInput(message))
        if message == "retired projection or instance identity"));

    // A refused standalone History request also retains its exact target
    // instance after the request body is gone. The refusal published no view.
    let history_key = fixture::operation(3);
    let ingress = client.begin_input(history_key, &RequestBudgetV1::unbounded())?;
    let input = client.stage_projection_output_with_children(
        ingress,
        b"{}",
        &[],
        &RequestBudgetV1::unbounded(),
    )?;
    let profile = client.read_view(
        quanta_index_contract::index::ViewSelection::Exact(published),
        &RequestBudgetV1::unbounded(),
    )?.profile;
    let cut = serde_json::from_value(serde_json::json!({
        "generation": {"repo_id": "repo-a", "revision_id": "rev-a", "generation": 1},
        "durable_epoch": 2,
        "snapshot_digest": vec![7_u8; 32],
        "replay_start": vec![8_u8; 32]
    }))?;
    let outcome = client.submit(
        quanta_index_contract::index::UpdateRequest {
            operation: history_key,
            expected_view: published,
            kind: quanta_index_contract::index::UpdateKind::HistoryAppend(
                quanta_index_contract::index::HistoryAppendV1 {
                    instance: fixture::spec().instance,
                    expected: cut,
                    input,
                    profile,
                },
            ),
        },
        &RequestBudgetV1::unbounded(),
    )?;
    let quanta_index_contract::index::SubmitOutcome::Terminal(history_status) = outcome else {
        return Err("standalone History refusal was not terminal".into());
    };
    assert!(matches!(history_status, quanta_index_contract::index::OperationStatus::Refused(_)));
    assert_eq!(
        dispatcher.catalog.authenticated_terminal_history_request_v1(
            history_key, &history_status, &mut work,
        )?,
        Some(fixture::spec().instance)
    );
    let saved_history = dispatcher.catalog.operation_request(history_key, &work)?;
    let history_hex: String = saved_history.request.0.0.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    write_catalog(
        &io,
        root.path(),
        &format!(
            "DELETE FROM native_operation_object_v1 WHERE object=x'{history_hex}';
             DELETE FROM native_object_edge_v1 WHERE parent=x'{history_hex}';
             DELETE FROM native_object_v1 WHERE object=x'{history_hex}';"
        ),
    )?;
    std::fs::remove_file(
        root.path().join("native-objects").join("objects")
            .join(&history_hex[..2]).join(&history_hex),
    )?;
    let replay = dispatcher.check_retired_request_v2(
        &retirement(Vec::new())?,
        &NativeIndexRequest::Resume(history_key),
        &mut work,
    )?;
    assert!(matches!(replay, Some(NativeIndexResponse::Submitted(reply))
        if reply.operation == history_key && reply.payload == quanta_index_contract::index::SubmitOutcome::Terminal(history_status)));
    let denied = dispatcher.check_retired_request_v2(
        &retirement(vec![fixture::spec().instance])?,
        &NativeIndexRequest::Resume(history_key),
        &mut work,
    );
    assert!(matches!(denied, Err(IndexError::InvalidInput(message))
        if message == "retired projection or instance identity"));
    drop(work);
    drop(reservation);
    drop(io);
    drop(dispatcher);
    guard.stop()?;
    finished_rx.recv_timeout(Duration::from_secs(5))??;
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
        &resources(),
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
        &resources(),
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
        &resources(),
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
