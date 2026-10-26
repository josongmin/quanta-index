//! Fixed-cap pressure with a retained read ticket and concurrent foreground
//! staging. Individual pin and capacity laws live in the parent.

use super::*;
use quanta_index_core::index::ports::CatalogExactHistoryPort as _;
use quanta_index_searchd_runtime::native::{NativeRetentionConfig, NativeRetentionScope};

const PRESSURE_BYTES: usize = 128 * 1024;
const PRESSURE_MAX_STAGES: u8 = 48;
const PRESSURE_CYCLES: u64 = 2;
const PRESSURE_SPARE_BYTES: u64 = 4 * 1024 * 1024;
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);
const EXTENDED_MIN_CYCLES: u64 = 4;
const EXTENDED_MAX_CYCLES: u64 = 128;
const EXTENDED_MIN_ACTIVE: Duration = Duration::from_secs(36);
const EXTENDED_PIN_CUTOFF: Duration = Duration::from_secs(90);
// Original 10s phase watchdogs are checked between RPCs; each RPC keeps the
// parent's 30s deadline. Headroom and the pin cutoff are cooperative checks,
// not syscall preemption. Offline validation must still prove the pin live.
const EXTENDED_CYCLE_HEADROOM: Duration = Duration::from_secs(45);

#[derive(Clone, Copy)]
enum PressureProfile {
    TwoCycles,
    Extended,
}

#[derive(Default)]
struct PressureSummary {
    cycles: u64,
    accepted: u64,
    recoveries: u64,
    physical_refusals: u64,
    max_discarded_debt: usize,
    max_drain_wall: Duration,
    max_logical_bytes: u64,
}

struct PressureCycleResult {
    accepted: u64,
    discarded_debt: usize,
    drain_wall: Duration,
    max_logical_bytes: u64,
}

impl PressureSummary {
    fn record(&mut self, cycle: &PressureCycleResult) -> TestResult {
        self.cycles = self
            .cycles
            .checked_add(1)
            .ok_or("pressure cycle overflow")?;
        self.accepted = self
            .accepted
            .checked_add(cycle.accepted)
            .ok_or("pressure accepted count overflow")?;
        self.recoveries = self
            .recoveries
            .checked_add(1)
            .ok_or("pressure recovery count overflow")?;
        self.physical_refusals = self
            .physical_refusals
            .checked_add(1)
            .ok_or("pressure refusal count overflow")?;
        self.max_discarded_debt = self.max_discarded_debt.max(cycle.discarded_debt);
        self.max_drain_wall = self.max_drain_wall.max(cycle.drain_wall);
        self.max_logical_bytes = self.max_logical_bytes.max(cycle.max_logical_bytes);
        Ok(())
    }
}

fn pressure_body(cycle: u64, index: u8, kind: u8) -> Vec<u8> {
    let mut body = Vec::with_capacity(PRESSURE_BYTES);
    body.push(kind);
    body.extend_from_slice(&cycle.to_le_bytes());
    body.push(index);
    body.resize(PRESSURE_BYTES, kind);
    body
}

fn present_native_objects(root: &std::path::Path, objects: &[ObjectRef]) -> TestResult<usize> {
    let mut present = 0_usize;
    for object in objects {
        if !native_object_absent_nofollow(&native_object_file(root, *object)?)? {
            present = present
                .checked_add(1)
                .ok_or("physical debt count overflow")?;
        }
    }
    Ok(present)
}

fn check_pin_cutoff(deadline: Option<Instant>, phase: &str) -> TestResult {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(format!("extended pressure exceeded pin cutoff during {phase}").into());
    }
    Ok(())
}

#[test]
fn oversized_wal_does_not_block_manual_object_reclaim() -> TestResult {
    oversized_wal_reclaim(false)
}

#[test]
fn oversized_wal_does_not_block_automatic_object_reclaim() -> TestResult {
    oversized_wal_reclaim(true)
}

fn oversized_wal_reclaim(automatic: bool) -> TestResult {
    use rusqlite::{Connection, OpenFlags};

    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let resources = mixed_resources();
    let initial = NativeIndexService::bind_with_capabilities_and_maintenance(
        root.path(),
        &socket,
        work_limits(),
        mixed_capabilities(resources)?,
        resources,
        None,
    )?;
    let running = NativeServiceGuard::start(initial);
    let client = NativeUdsClient::new(socket.clone());
    assert!(matches!(
        client.request(
            NativeIndexRequest::CreateWorkspace {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?,
        NativeIndexResponse::Created(_)
    ));
    let (discarded, record) = stage_source(
        &client,
        operation(980),
        "discarded.rs",
        b"discarded WAL gold",
    )?;
    let (_retained, survivor) =
        stage_source(&client, operation(981), "survivor.rs", b"retained WAL gold")?;
    let record_file = native_object_file(root.path(), record)?;
    let survivor_file = native_object_file(root.path(), survivor)?;
    let survivor_wire = std::fs::read(&survivor_file)?;
    client.discard_input(discarded, &budget())?;
    running.stop()?;
    assert!(
        !native_object_absent_nofollow(&record_file)?,
        "discarded record absent before oversized WAL reclaim fixture"
    );

    // Establish a real SQLite snapshot and WAL with the service closed.
    // The raw writer ends before the admitted service binds; only its
    // read-only snapshot remains across boot, then closes before reclaim.
    let database = root.path().join("catalog/catalog-v1.sqlite");
    let writer = Connection::open(&database)?;
    writer.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
         CREATE TABLE physical_wal_gold(id INTEGER PRIMARY KEY, body BLOB);
         INSERT INTO physical_wal_gold VALUES(7, zeroblob(18874368));
         PRAGMA wal_checkpoint(TRUNCATE);",
    )?;
    let wal = database.with_file_name("catalog-v1.sqlite-wal");
    assert!(std::fs::metadata(&database)?.len() >= 18_874_368);
    assert_eq!(std::fs::metadata(&wal)?.len(), 0);
    let reader = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    reader.execute_batch("BEGIN")?;
    let old_sum: i64 = reader.query_row("SELECT sum(id) FROM physical_wal_gold", [], |row| {
        row.get(0)
    })?;
    assert_eq!(old_sum, 7);
    assert_eq!(
        writer.execute(
            "UPDATE physical_wal_gold SET body=?1 WHERE id=7",
            [vec![b'Z'; 18_874_368]]
        )?,
        1
    );
    assert_eq!(
        writer.execute("INSERT INTO physical_wal_gold VALUES(11, X'0B')", [])?,
        1
    );
    drop(writer);
    let original_io = resources.execution.control.io_bytes;
    let threshold = original_io.checked_div(4).ok_or("zero WAL IO divisor")?;
    assert!(std::fs::metadata(&wal)?.len() > threshold);
    let service = NativeIndexService::bind_with_capabilities_and_maintenance(
        root.path(),
        &socket,
        work_limits(),
        mixed_capabilities(resources)?,
        resources,
        automatic.then(test_reclaim_config),
    )?;
    let maintenance = service.maintenance_handle();
    let running = NativeServiceGuard::start(service);
    assert!(std::fs::metadata(&wal)?.len() > threshold);
    assert_eq!(
        reader.query_row("SELECT sum(id) FROM physical_wal_gold", [], |row| {
            row.get::<_, i64>(0)
        })?,
        7
    );
    assert_eq!(
        reader.query_row(
            "SELECT hex(substr(body,1,1)) FROM physical_wal_gold WHERE id=7",
            [],
            |row| row.get::<_, String>(0)
        )?,
        "00"
    );
    drop(reader);
    // Releasing the snapshot does not shrink the physical WAL. Reclaim must
    // progress under its unchanged original IO reservation, without needing
    // a later foreground request or daemon restart to reduce this WAL first.
    assert!(std::fs::metadata(&wal)?.len() > threshold);
    if automatic {
        let deadline = Instant::now()
            .checked_add(SETTLE_TIMEOUT)
            .ok_or("oversized WAL reclaim deadline overflow")?;
        while !native_object_absent_nofollow(&record_file)? && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    } else {
        let mut after = None;
        for _step in 0..16 {
            let mut budget = WorkBudget::with_request(work_limits(), budget());
            let step = maintenance
                .reclaim_step(after, 1, &mut budget)
                .map_err(|error| {
                    format!(
                        "oversized WAL manual reclaim failed: {error:?}; usage={:?}",
                        budget.usage()
                    )
                })?;
            if native_object_absent_nofollow(&record_file)? {
                break;
            }
            after = if step.has_more {
                step.resume_after
            } else {
                None
            };
        }
    }
    assert!(
        native_object_absent_nofollow(&record_file)?,
        "oversized WAL blocked {} object reclaim",
        if automatic { "automatic" } else { "manual" }
    );
    assert_eq!(std::fs::read(&survivor_file)?, survivor_wire);
    drop(maintenance);
    running.stop()?;
    let verify = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let gold: (i64, i64) = verify.query_row(
        "SELECT sum(id), sum(length(body)) FROM physical_wal_gold",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(gold, (18, 18_874_369));
    assert_eq!(
        verify.query_row(
            "SELECT hex(substr(body,1,1)) FROM physical_wal_gold WHERE id=7",
            [],
            |row| row.get::<_, String>(0)
        )?,
        "5A"
    );
    drop(verify);
    mixed_quiescent_census(root.path(), &socket, resources)
}

fn mixed_capabilities(resources: NativeServiceResources) -> TestResult<NativeServiceCapabilities> {
    let mut capabilities = reclaim_capabilities(None)?;
    let path_need = ResourceNeed {
        provider_units: 0,
        ..resources.execution.interactive
    };
    capabilities.path.register(
        descriptors().file_map,
        path_need,
        NativePathReader::new(path_need)?,
    )?;
    Ok(capabilities)
}

fn mixed_retention() -> NativeRetentionConfig {
    NativeRetentionConfig {
        scopes: vec![NativeRetentionScope {
            workspace: workspace(),
            epoch: 1,
        }],
        sequence_window: 1,
        page_limit: 8,
        interval: Duration::from_millis(20),
        round_timeout: Duration::from_secs(2),
        work_limits: work_limits(),
        head_probe_every: 2,
    }
}

fn mixed_resources() -> NativeServiceResources {
    let mut resources = test_resources();
    // One additional maintenance class must be funded before binding.
    resources.execution.capacity.cpu_units = 5;
    resources.execution.capacity.io_bytes = 320 * 1024 * 1024;
    resources.execution.capacity.memory_bytes = (5 + 1) * 64 * 1024 * 1024;
    resources
}

fn mixed_service(
    root: &std::path::Path,
    socket: &std::path::Path,
    resources: NativeServiceResources,
    reclaim: Option<NativeReclaimConfig>,
    retention: NativeRetentionConfig,
) -> TestResult<NativeIndexService> {
    Ok(NativeIndexService::bind_with_retention(
        root,
        socket,
        work_limits(),
        mixed_capabilities(resources)?,
        resources,
        reclaim,
        Some(retention),
    )?)
}

fn mixed_quiescent_census(
    root: &std::path::Path,
    socket: &std::path::Path,
    resources: NativeServiceResources,
) -> TestResult {
    // A retention-enabled bind starts its owned maintenance pump before it
    // returns. The directory walk and capacity snapshot are separate reads,
    // so compare them only while no background writer can mutate the root.
    let service = NativeIndexService::bind_with_retention(
        root,
        socket,
        work_limits(),
        mixed_capabilities(resources)?,
        resources,
        None,
        None,
    )?;
    check_native_physical_census(&service, root)?;
    drop(service);
    Ok(())
}

fn await_reclaim_headroom<T>(
    phase: &str,
    deadline: Instant,
    mut request: impl FnMut() -> Result<T, NativeUdsError>,
) -> TestResult<T> {
    loop {
        match request() {
            Ok(result) => return Ok(result),
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::ResourceLimited
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(format!("{phase}: {error:?}").into()),
        }
    }
}

fn pressure_cycle(
    client: &NativeUdsClient,
    root: &std::path::Path,
    identity: &FileObjectStore,
    cap_bytes: u64,
    maintenance_reserved_bytes: u64,
    cycle: u64,
    pin_cutoff: Option<Instant>,
) -> TestResult<PressureCycleResult> {
    check_pin_cutoff(pin_cutoff, "cycle start")?;
    let operation_sequence = 7000_u64
        .checked_add(
            cycle
                .checked_mul(100)
                .ok_or("pressure operation overflow")?,
        )
        .ok_or("pressure operation overflow")?;
    let ingress = client
        .begin_input(operation(operation_sequence), &budget())
        .map_err(|error| format!("pressure cycle {cycle} begin input: {error:?}"))?;
    let mut discarded_objects = Vec::new();
    let mut accepted = 0_u8;
    let mut refused = false;
    let mut max_logical_bytes = native_regular_file_bytes(&native_regular_files(root)?)?;
    let deadline = Instant::now()
        .checked_add(SETTLE_TIMEOUT)
        .ok_or("pressure stage deadline overflow")?;
    let mut index = 0_u8;
    while index < PRESSURE_MAX_STAGES {
        if Instant::now() >= deadline {
            return Err(format!("pressure cycle {cycle} exceeded its stage deadline").into());
        }
        // Every cycle uses distinct bodies, so stale objects or dedup cannot
        // turn a physical-pressure assertion into a no-growth replay.
        let body = pressure_body(cycle, index, 1);
        let next_body = identity.identify(
            &body,
            &[],
            &mut WorkBudget::with_request(work_limits(), budget()),
        )?;
        let next_body_wire_bytes = next_body.bytes;
        if !discarded_objects.contains(&next_body.reference) {
            discarded_objects.push(next_body.reference);
        }
        let result = client.stage_sources(
            ingress,
            vec![SourceInput {
                path: format!("pressure-{cycle}-{index}.rs"),
                bytes: body,
                text: SourceTextAdmission::Excluded,
            }],
            &budget(),
        );
        let files = native_regular_files(root)?;
        let bytes = native_regular_file_bytes(&files)?;
        max_logical_bytes = max_logical_bytes.max(bytes);
        assert!(
            bytes <= cap_bytes,
            "pressure cycle {cycle} exceeded physical cap: {bytes}"
        );
        let body_headroom = cap_bytes
            .checked_sub(bytes)
            .ok_or("physical census exceeded cap")?
            // Foreground begin_create must first restore this full reserve.
            // If even that cannot fit, no bytes are available to its body.
            .saturating_sub(maintenance_reserved_bytes);
        match result {
            Ok(staged) => {
                assert_eq!(staged.len(), 1);
                for object in staged {
                    if !discarded_objects.contains(&object) {
                        discarded_objects.push(object);
                    }
                }
                accepted = accepted.checked_add(1).ok_or("stage count overflow")?;
                index = index.checked_add(1).ok_or("stage index overflow")?;
            }
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::ResourceLimited =>
            {
                // A prior cycle's discarded probe may still be reclaiming.
                // Require new foreground progress in this cycle before its
                // next physical-full observation can close the cycle.
                if accepted > 0 && body_headroom < next_body_wire_bytes {
                    refused = true;
                    break;
                }
                // Other execution or in-flight funding can also report
                // ResourceLimited. Retry until this independent census proves
                // the next full body cannot fit in the physical allowance.
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(format!("pressure cycle {cycle} stage {index}: {error:?}").into());
            }
        }
    }
    assert!(accepted > 0, "pressure cycle {cycle} admitted no full body");
    assert!(refused, "pressure cycle {cycle} never reached its cap");
    check_pin_cutoff(pin_cutoff, "physical-full staging")?;
    assert!(
        present_native_objects(root, &discarded_objects)? > 0,
        "pressure cycle {cycle} created no physical objects"
    );
    client
        .discard_input(ingress, &budget())
        .map_err(|error| format!("pressure cycle {cycle} discard staged input: {error:?}"))?;
    let mut discarded_debt = present_native_objects(root, &discarded_objects)?;

    // Reclaim uses the original service-owned maintenance class. A successful
    // full-size stage under the unchanged cap is the foreground recovery gold.
    let probe_sequence = operation_sequence
        .checked_add(50)
        .ok_or("pressure probe operation overflow")?;
    // Discard releases logical custody; automatic physical GC is asynchronous.
    // BeginInput also writes Catalog pages and must share the same recovery
    // deadline as the full-size stage, rather than assuming immediate space.
    let recovery_deadline = Instant::now()
        .checked_add(SETTLE_TIMEOUT)
        .ok_or("pressure recovery deadline overflow")?;
    let probe = await_reclaim_headroom(
        &format!("pressure cycle {cycle} begin recovery probe"),
        recovery_deadline,
        || client.begin_input(operation(probe_sequence), &budget()),
    )?;
    check_pin_cutoff(pin_cutoff, "recovery ingress")?;
    let probe_body = pressure_body(cycle, 0, 2);
    let staged = await_reclaim_headroom(
        &format!("pressure cycle {cycle} full-size recovery stage"),
        recovery_deadline,
        || {
            client.stage_sources(
                probe,
                vec![SourceInput {
                    path: format!("recovered-{cycle}.rs"),
                    bytes: probe_body.clone(),
                    text: SourceTextAdmission::Excluded,
                }],
                &budget(),
            )
        },
    )?;
    assert_eq!(staged.len(), 1);
    for object in staged {
        if !discarded_objects.contains(&object) {
            discarded_objects.push(object);
        }
    }
    let recovery_body = identity
        .identify(
            &probe_body,
            &[],
            &mut WorkBudget::with_request(work_limits(), budget()),
        )?
        .reference;
    if !discarded_objects.contains(&recovery_body) {
        discarded_objects.push(recovery_body);
    }
    let recovered_bytes = native_regular_file_bytes(&native_regular_files(root)?)?;
    max_logical_bytes = max_logical_bytes.max(recovered_bytes);
    assert!(recovered_bytes <= cap_bytes);
    check_pin_cutoff(pin_cutoff, "full-size recovery")?;
    client
        .discard_input(probe, &budget())
        .map_err(|error| format!("pressure cycle {cycle} discard recovery probe: {error:?}"))?;
    discarded_debt = discarded_debt.max(present_native_objects(root, &discarded_objects)?);
    // Finish this cycle's physical debt while the old read pin is still live.
    // The later last-pin watchdog must measure that release, not unrelated
    // discarded staging work from earlier cycles. Both phases keep 10s.
    let drain_deadline = Instant::now()
        .checked_add(SETTLE_TIMEOUT)
        .ok_or("discarded-object drain deadline overflow")?;
    let drain_started = Instant::now();
    while !native_objects_absent_nofollow(root, &discarded_objects)? {
        if Instant::now() >= drain_deadline {
            return Err(format!("pressure cycle {cycle} discarded objects did not drain").into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    check_pin_cutoff(pin_cutoff, "discarded-object drain")?;
    Ok(PressureCycleResult {
        accepted: u64::from(accepted),
        discarded_debt,
        drain_wall: drain_started.elapsed(),
        max_logical_bytes,
    })
}

#[test]
fn native_mixed_pin_pressure_reclaims_under_same_cap_after_last_release() -> TestResult {
    native_mixed_pin_pressure(PressureProfile::TwoCycles)
}

#[test]
#[ignore = "opt-in fixed-cap long-pin pressure; bounded diagnostic, not a steady-state SLO"]
fn native_extended_pin_pressure_reclaims_under_same_cap_after_last_release() -> TestResult {
    native_mixed_pin_pressure(PressureProfile::Extended)
}

fn native_mixed_pin_pressure(pressure_profile: PressureProfile) -> TestResult {
    use quanta_index_core::index::ports::CatalogReadPort as _;

    let root = private_root()?;
    let identity_root = private_root()?;
    let identity = FileObjectStore::open(identity_root.path().join("identity-objects"))?;
    let socket = root.path().join("native.sock");
    let resources = mixed_resources();
    let retention = mixed_retention();
    let reclaim = NativeReclaimConfig {
        max_steps_per_round: 4,
        ..test_reclaim_config()
    };
    let running = NativeServiceGuard::start(mixed_service(
        root.path(),
        &socket,
        resources,
        Some(reclaim),
        retention.clone(),
    )?);
    let client = NativeUdsClient::new(socket.clone());
    let created = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = created else {
        return Err(format!("mixed pressure genesis: {created:?}").into());
    };
    let profile_ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        profile_ingress,
        IndexProfile {
            instances: vec![literal_spec(), file_spec()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let (first_ingress, first_record) =
        stage_source(&client, operation(2), "first.rs", b"old pinned source")?;
    let old_view = advance_one(
        &client,
        first_ingress,
        configured,
        owner(1),
        first_record,
        1,
    )?;
    let old_paths = client.query_paths(
        ViewSelection::Exact(old_view),
        file_spec().instance,
        "",
        None,
        8,
        &budget(),
    )?;
    assert_eq!(old_paths.view, old_view);
    let old_entry = old_paths.entries.first().ok_or("old path entry absent")?;
    assert_eq!(old_entry.source, first_record);
    let record_file = native_object_file(root.path(), first_record)?;
    let body_file = native_object_file(root.path(), old_entry.bytes)?;
    let record_wire = std::fs::read(&record_file)?;
    let body_wire = std::fs::read(&body_file)?;
    running.stop()?;

    // The independent OS-backed session is established while the admitted
    // service is closed, then held across every cap-pressure cycle.
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    let session = catalog.open_read_session()?;
    let pin = catalog.acquire_ticket(
        session.as_ref(),
        ViewSelection::Exact(old_view),
        Duration::from_secs(120),
        &WorkBudget::with_request(work_limits(), budget()),
    )?;
    let pin_acquired_at = Instant::now();
    let pin_cutoff = match pressure_profile {
        PressureProfile::TwoCycles => None,
        PressureProfile::Extended => Some(
            pin_acquired_at
                .checked_add(EXTENDED_PIN_CUTOFF)
                .ok_or("extended pin cutoff overflow")?,
        ),
    };
    assert_eq!(pin.view.identity, old_view);
    drop(catalog);
    mixed_quiescent_census(root.path(), &socket, resources)?;
    let service = mixed_service(
        root.path(),
        &socket,
        resources,
        Some(reclaim),
        retention.clone(),
    )?;
    let running = NativeServiceGuard::start(service);

    let (second_ingress, second_record) = stage_source(
        &client,
        operation(3),
        "second.rs",
        b"current survivor source",
    )?;
    let second_view = advance_one(
        &client,
        second_ingress,
        old_view,
        owner(2),
        second_record,
        2,
    )?;
    let retract = client.begin_input(operation(4), &budget())?;
    let prior = client.read_view(ViewSelection::Exact(second_view), &budget())?;
    let owners = client.stage_mutations(
        retract,
        [OwnerMutation {
            owner: owner(1),
            expected: Some(first_record),
            next: None,
        }],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(1).producer,
        stream: operation(1).stream,
        stream_epoch: 1,
        predecessor: 2,
        sequence: 3,
        payload: transition.owner_digest()?,
    });
    let current = published(&client.submit(
        UpdateRequest {
            operation: operation(4),
            expected_view: second_view,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    let floor_deadline = Instant::now()
        .checked_add(SETTLE_TIMEOUT)
        .ok_or("retention floor deadline overflow")?;
    loop {
        match client.read_view(ViewSelection::Exact(old_view), &budget()) {
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable =>
            {
                break;
            }
            Ok(view) if view.identity == old_view => {}
            other => return Err(format!("old-view retention probe: {other:?}").into()),
        }
        if Instant::now() >= floor_deadline {
            return Err("retention floor did not advance while old ticket was live".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let current_paths = client.query_paths(
        ViewSelection::Exact(current),
        file_spec().instance,
        "",
        None,
        8,
        &budget(),
    )?;
    assert_eq!(current_paths.view, current);
    assert_eq!(current_paths.entries.len(), 1);
    assert!(!current_paths.has_more);
    let current_entry = current_paths.entries.first().ok_or("current path absent")?;
    assert_eq!(current_entry.path, "second.rs");
    assert_eq!(current_entry.owner, owner(2));
    assert_eq!(current_entry.source, second_record);
    let current_body_file = native_object_file(root.path(), current_entry.bytes)?;
    assert!(
        std::fs::symlink_metadata(&current_body_file)?
            .file_type()
            .is_file()
    );
    let current_body_wire = std::fs::read(&current_body_file)?;
    running.stop()?;

    // Complete the COW source transitions before choosing the tight staging
    // cap. The pin remains live, and the cap then stays fixed for pressure,
    // recovery, release, and reopen. COW publication itself can need more
    // transient physical headroom than the foreground source-stage probe.
    let baseline = native_regular_files(root.path())?;
    let cap_bytes = native_regular_file_bytes(&baseline)?
        .checked_add(PRESSURE_SPARE_BYTES)
        .ok_or("mixed pressure byte cap overflow")?;
    let mut capped_resources = resources;
    capped_resources.physical.inventory.max_bytes = cap_bytes;
    mixed_quiescent_census(root.path(), &socket, capped_resources)?;
    let service = mixed_service(
        root.path(),
        &socket,
        capped_resources,
        Some(reclaim),
        retention.clone(),
    )?;
    let running = NativeServiceGuard::start(service);

    let pressure_started = Instant::now();
    let mut summary = PressureSummary::default();
    loop {
        let active = pressure_started.elapsed();
        let complete = match pressure_profile {
            PressureProfile::TwoCycles => summary.cycles >= PRESSURE_CYCLES,
            PressureProfile::Extended => {
                summary.cycles >= EXTENDED_MIN_CYCLES && active >= EXTENDED_MIN_ACTIVE
            }
        };
        if complete {
            break;
        }
        if matches!(pressure_profile, PressureProfile::Extended) {
            if summary.cycles >= EXTENDED_MAX_CYCLES {
                return Err(
                    "extended pressure reached cycle bound before active-time floor".into(),
                );
            }
            let remaining = pin_cutoff
                .ok_or("extended pin cutoff absent")?
                .saturating_duration_since(Instant::now());
            if remaining < EXTENDED_CYCLE_HEADROOM {
                return Err(format!(
                    "extended pressure lacks one bounded cycle plus pin-release headroom: cycles={}, active={active:?}, remaining={remaining:?}",
                    summary.cycles
                )
                .into());
            }
        }
        let cycle = summary.cycles;
        let pressure = pressure_cycle(
            &client,
            root.path(),
            &identity,
            cap_bytes,
            capped_resources.physical.maintenance_reserved_bytes,
            cycle,
            pin_cutoff,
        );
        let cycle_result = match pressure {
            Ok(cycle_result) => cycle_result,
            Err(error) => {
                // Preserve the owned pump's join error when foreground recovery
                // fails; guard Drop cannot return that independent failure.
                let resources = running.resources.observe();
                let shutdown = running.stop();
                let files = native_regular_files(root.path())?;
                let disk_bytes = native_regular_file_bytes(&files)?;
                let connection = rusqlite::Connection::open_with_flags(
                    root.path().join("catalog/catalog-v1.sqlite"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )?;
                let counts: (i64, i64, i64, i64) = connection.query_row(
                    "SELECT (SELECT COUNT(*) FROM native_gc_queue_v1),
                        (SELECT COUNT(*) FROM native_gc_object_v1 WHERE phase=4),
                        (SELECT COUNT(*) FROM native_ingress_object_v1),
                        (SELECT COUNT(*) FROM native_ingress_intent_v1)",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;
                return Err(format!(
                    "pressure cycle {cycle}: {error}; service shutdown: {shutdown:?}; resources={resources:?}; disk={disk_bytes}/{cap_bytes}; queue/deleted/ingress/intents={counts:?}; prior_cycles={}; prior_accepted={}"
                    , summary.cycles, summary.accepted
                ).into());
            }
        };
        summary.record(&cycle_result)?;
        assert_eq!(std::fs::read(&record_file)?, record_wire);
        assert_eq!(std::fs::read(&body_file)?, body_wire);
        assert_eq!(pin.view.identity, old_view);
        let served_deadline = Instant::now()
            .checked_add(SETTLE_TIMEOUT)
            .ok_or("post-discard serving deadline overflow")?;
        let served = await_reclaim_headroom(
            &format!("pressure cycle {cycle} read current view"),
            served_deadline,
            || client.read_view(ViewSelection::Exact(current), &budget()),
        )?;
        assert_eq!(served.identity, current);
        check_pin_cutoff(pin_cutoff, "post-cycle pinned view")?;
    }
    let pressure_active_duration = pressure_started.elapsed();
    running.stop()?;
    mixed_quiescent_census(root.path(), &socket, capped_resources)?;
    check_pin_cutoff(pin_cutoff, "offline pin validation")?;

    // Explicit release is performed offline so the raw Catalog writer never
    // competes with the admitted service's SQLite VFS.
    let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
    assert_eq!(
        catalog.exact_history_floor(workspace(), 1)?,
        current.sequence
    );
    catalog.validate_ticket(session.as_ref(), pin.lease)?;
    catalog.release_ticket(session.as_ref(), pin.lease)?;
    let pin_held_duration = pin_acquired_at.elapsed();
    check_pin_cutoff(pin_cutoff, "pin release")?;
    drop(session);
    drop(catalog);
    mixed_quiescent_census(root.path(), &socket, capped_resources)?;
    let service = mixed_service(
        root.path(),
        &socket,
        capped_resources,
        Some(reclaim),
        retention,
    )?;
    let running = NativeServiceGuard::start(service);
    let reclaim_deadline = Instant::now()
        .checked_add(SETTLE_TIMEOUT)
        .ok_or("last-pin reclaim deadline overflow")?;
    while (!native_object_absent_nofollow(&record_file)?
        || !native_object_absent_nofollow(&body_file)?)
        && Instant::now() < reclaim_deadline
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    if !native_object_absent_nofollow(&record_file)? || !native_object_absent_nofollow(&body_file)?
    {
        let shutdown = running.stop();
        let connection = rusqlite::Connection::open_with_flags(
            root.path().join("catalog/catalog-v1.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let old_sequence = i64::try_from(old_view.sequence)?;
        let state: (i64, i64, i64, i64) = connection.query_row(
            "SELECT (SELECT COUNT(*) FROM native_view_v1 WHERE sequence=?1),
                    (SELECT COUNT(*) FROM native_pin_v1 WHERE sequence=?1),
                    (SELECT COUNT(*) FROM native_gc_queue_v1),
                    (SELECT COUNT(*) FROM native_gc_object_v1 WHERE phase=4)",
            [old_sequence],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        let mut references = Vec::new();
        for (table, column) in [
            ("native_view_root_v1", "object"),
            ("native_operation_object_v1", "object"),
            ("native_ingress_intent_v1", "object"),
            ("native_ingress_object_v1", "object"),
            ("native_object_edge_v1", "child"),
            ("native_object_install_edge_v1", "child"),
        ] {
            let count: i64 = connection.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {column}=?1"),
                [first_record.0.0.as_slice()],
                |row| row.get(0),
            )?;
            references.push((table, count));
        }
        return Err(format!("last-pin GC did not finish: old views/pins/queue/deleted={state:?}; record references={references:?}; shutdown={shutdown:?}").into());
    }
    let surviving_paths = client.query_paths(
        ViewSelection::Exact(current),
        file_spec().instance,
        "",
        None,
        8,
        &budget(),
    )?;
    assert_eq!(surviving_paths.view, current);
    assert_eq!(surviving_paths.entries.len(), 1);
    assert!(!surviving_paths.has_more);
    let surviving_entry = surviving_paths
        .entries
        .first()
        .ok_or("surviving path absent")?;
    assert_eq!(surviving_entry.path, "second.rs");
    assert_eq!(surviving_entry.owner, owner(2));
    assert_eq!(surviving_entry.source, second_record);
    assert_eq!(surviving_entry.bytes, current_entry.bytes);
    assert!(
        std::fs::symlink_metadata(&current_body_file)?
            .file_type()
            .is_file()
    );
    assert_eq!(std::fs::read(&current_body_file)?, current_body_wire);
    running.stop()?;
    mixed_quiescent_census(root.path(), &socket, capped_resources)?;
    if matches!(pressure_profile, PressureProfile::Extended) {
        use std::io::Write as _;

        // Keep diagnostic sink errors in the test result instead of panicking.
        writeln!(
            std::io::stdout().lock(),
            "extended physical pressure: cycles={} active_ms={} accepted={} recoveries={} physical_refusals={} max_sampled_discarded_objects={} max_drain_ms={} max_sampled_logical_bytes={} fixed_cap_bytes={} pin_held_ms={} scope=bounded_functional_stress_not_steady_state_or_peak",
            summary.cycles,
            pressure_active_duration.as_millis(),
            summary.accepted,
            summary.recoveries,
            summary.physical_refusals,
            summary.max_discarded_debt,
            summary.max_drain_wall.as_millis(),
            summary.max_logical_bytes,
            cap_bytes,
            pin_held_duration.as_millis(),
        )?;
    }
    Ok(())
}
