//! Real-service WAL reclaim around a held `SQLite` snapshot and funded GC.

use super::*;
use quanta_index_searchd_runtime::native::NativeWalReclaimPolicy;
use rusqlite::{Connection, OpenFlags};

const WAL_RECLAIM_TEST_INTERVAL: Duration = Duration::from_millis(20);
const WAL_RECLAIM_TEST_DEADLINE: Duration = Duration::from_secs(10);

fn wal_len(path: &std::path::Path) -> TestResult<u64> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error.into()),
    }
}

fn wait_until(
    deadline: Instant,
    mut predicate: impl FnMut() -> TestResult<bool>,
) -> TestResult<bool> {
    loop {
        if predicate()? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn reclaim_with_held_sqlite_reader(
    payload_bytes: usize,
    gc_reads: u64,
    wal_policy: NativeWalReclaimPolicy,
) -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let resources = mixed_resources();
    let initial = NativeIndexService::bind_with_capabilities_and_maintenance(
        root.path(),
        &socket,
        work_limits(),
        mixed_capabilities(&resources)?,
        &resources,
        None,
    )
    .map_err(|error| format!("bind WAL fixture preparation service: {error:#}"))?;
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
        operation(982),
        "discarded-wal.rs",
        b"discarded while WAL reader is live",
    )?;
    let (_retained, survivor) = stage_source(
        &client,
        operation(983),
        "survivor-wal.rs",
        b"survives WAL reader and reclaim",
    )?;
    let record_file = native_object_file(root.path(), record)?;
    let survivor_file = native_object_file(root.path(), survivor)?;
    let survivor_wire = std::fs::read(&survivor_file)?;
    client.discard_input(discarded, &budget())?;
    running
        .stop()
        .map_err(|error| format!("stop WAL fixture preparation service: {error}"))?;
    assert!(!native_object_absent_nofollow(&record_file)?);

    // The writer closes before service bind. The read transaction keeps the
    // original snapshot alive, so the WAL cannot be truncated by a new owner.
    let database = root.path().join("catalog/catalog-v1.sqlite");
    let wal = root.path().join("catalog/catalog-v1.sqlite-wal");
    let writer = Connection::open(&database)?;
    writer.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
         CREATE TABLE wal_reclaim_reader_gold(id INTEGER PRIMARY KEY, body BLOB);",
    )?;
    assert_eq!(
        writer.execute(
            "INSERT INTO wal_reclaim_reader_gold VALUES(7, zeroblob(?1))",
            [i64::try_from(payload_bytes)?],
        )?,
        1
    );
    writer.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    // The large body is already allocated in the main DB. A later same-size
    // update fills WAL without requiring main-file growth from a 512 KiB
    // maintenance parent when the held reader eventually releases.
    assert!(std::fs::metadata(&database)?.len() >= u64::try_from(payload_bytes)?);
    let reader = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    reader.execute_batch("BEGIN")?;
    assert_eq!(
        reader.query_row("SELECT sum(id) FROM wal_reclaim_reader_gold", [], |row| {
            row.get::<_, i64>(0)
        })?,
        7
    );
    assert_eq!(
        writer.execute(
            "UPDATE wal_reclaim_reader_gold SET body=?1 WHERE id=7",
            [vec![b'Z'; payload_bytes]],
        )?,
        1
    );
    assert_eq!(
        writer.execute("INSERT INTO wal_reclaim_reader_gold VALUES(11, X'0B')", [])?,
        1
    );
    drop(writer);
    let initial_wal = wal_len(&wal)?;
    let max_fundable_wal = resources
        .execution
        .control
        .io_bytes
        .checked_div(4)
        .ok_or("WAL IO divisor was zero")?;
    assert!(
        initial_wal >= wal_policy.trigger_bytes,
        "fixture never crossed the WAL trigger"
    );
    if gc_reads == 16_384 {
        let checkpoint_reads = initial_wal
            .checked_add(511)
            .and_then(|bytes| bytes.checked_div(512))
            .ok_or("WAL work estimate overflow")?;
        assert!(
            checkpoint_reads > gc_reads,
            "fixture WAL fits the claimed unfunded read allowance"
        );
    }
    assert!(
        initial_wal < max_fundable_wal,
        "WAL must be fundable by the unchanged control IO class"
    );

    let mut reclaim = test_reclaim_config();
    reclaim.work_limits.reads = gc_reads;
    reclaim.wal_reclaim = wal_policy;
    let service = NativeIndexService::bind_with_capabilities_and_maintenance(
        root.path(),
        &socket,
        work_limits(),
        mixed_capabilities(&resources)?,
        &resources,
        Some(reclaim),
    )
    .map_err(|error| format!("bind automatic WAL reclaim service: {error:#}"))?;
    let running = NativeServiceGuard::start(service);
    let drain_deadline = Instant::now()
        .checked_add(WAL_RECLAIM_TEST_DEADLINE)
        .ok_or("WAL GC deadline overflow")?;
    assert!(
        wait_until(drain_deadline, || Ok(native_object_absent_nofollow(
            &record_file
        )?))?,
        "funded object reclaim stalled while a SQLite reader held the WAL"
    );
    assert_eq!(std::fs::read(&survivor_file)?, survivor_wire);
    assert_eq!(
        reader.query_row("SELECT sum(id) FROM wal_reclaim_reader_gold", [], |row| {
            row.get::<_, i64>(0)
        })?,
        7,
        "live reader lost its old snapshot"
    );
    assert_eq!(
        reader.query_row(
            "SELECT hex(substr(body,1,1)) FROM wal_reclaim_reader_gold WHERE id=7",
            [],
            |row| row.get::<_, String>(0),
        )?,
        "00",
        "live reader observed a later body"
    );
    assert!(wal_len(&wal)? > 0, "live reader allowed WAL truncation");
    // End only the snapshot. Keep the raw connection open throughout the
    // observation so its last-close checkpoint cannot satisfy this oracle.
    reader.execute_batch("ROLLBACK")?;

    if gc_reads == work_limits().reads {
        let reclaim_deadline = Instant::now()
            .checked_add(WAL_RECLAIM_TEST_DEADLINE)
            .ok_or("WAL truncation deadline overflow")?;
        let reduced_below = initial_wal
            .checked_div(2)
            .ok_or("WAL reduction divisor was zero")?;
        assert!(
            wait_until(reclaim_deadline, || Ok(wal_len(&wal)? < reduced_below))?,
            "WAL allocation did not shrink after the held snapshot ended"
        );
    } else {
        // 12 MiB WAL needs more than 16,384 admitted 512-byte read units.
        // The same round can reclaim the tiny discarded object, but the
        // optional checkpoint must remain deferred under this fixed budget.
        std::thread::sleep(Duration::from_millis(200));
        let not_reclaimed = initial_wal
            .checked_div(2)
            .ok_or("WAL retention divisor was zero")?;
        assert!(
            wal_len(&wal)? >= not_reclaimed,
            "unfunded optional checkpoint bypassed the existing work limit"
        );
    }
    // Observation is complete. Release the external SQLite handle before
    // requesting checked native main/SHM retirement at service shutdown.
    drop(reader);
    running
        .stop()
        .map_err(|error| format!("stop automatic WAL reclaim service: {error}"))?;
    let verify = Connection::open_with_flags(&database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    assert_eq!(
        verify.query_row(
            "SELECT sum(id), sum(length(body)) FROM wal_reclaim_reader_gold",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?,
        (
            18,
            i64::try_from(payload_bytes)?
                .checked_add(1)
                .ok_or("WAL gold payload length overflow")?
        )
    );
    assert_eq!(
        verify.query_row(
            "SELECT hex(substr(body,1,1)) FROM wal_reclaim_reader_gold WHERE id=7",
            [],
            |row| row.get::<_, String>(0),
        )?,
        "5A"
    );
    drop(verify);
    if gc_reads == 16_384 {
        // This arm deliberately leaves an unfunded WAL. With all product
        // connections closed, the test-owned ordinary SQLite connection
        // performs its normal last-close cleanup before the separate capacity
        // census boots a new service. This is fixture cleanup, not a product
        // WAL reclaim result or a use of the service's maintenance budget.
        let cleanup = Connection::open(&database)
            .map_err(|error| format!("open test-owned WAL cleanup connection: {error}"))?;
        cleanup
            .close()
            .map_err(|(_connection, error)| format!("close test-owned WAL cleanup: {error}"))?;
        assert_eq!(wal_len(&wal)?, 0, "test-owned last close retained WAL");
    }
    mixed_quiescent_census(root.path(), &socket, &resources)
        .map_err(|error| format!("rebind for WAL physical census: {error}"))?;
    Ok(())
}

#[test]
fn held_sqlite_reader_does_not_preempt_gc_and_wal_reclaims_after_release() -> TestResult {
    reclaim_with_held_sqlite_reader(
        4_194_304,
        work_limits().reads,
        NativeWalReclaimPolicy {
            trigger_bytes: 1,
            min_interval: WAL_RECLAIM_TEST_INTERVAL,
        },
    )
}

#[test]
fn optional_wal_reclaim_respects_remaining_gc_read_budget() -> TestResult {
    reclaim_with_held_sqlite_reader(
        12_582_912,
        16_384,
        NativeWalReclaimPolicy {
            trigger_bytes: 1,
            min_interval: WAL_RECLAIM_TEST_INTERVAL,
        },
    )
}

#[test]
fn product_wal_policy_preserves_gc_and_reclaims_a_funded_wal() -> TestResult {
    reclaim_with_held_sqlite_reader(
        12_582_912,
        work_limits().reads,
        NativeWalReclaimPolicy {
            trigger_bytes: 8_388_608,
            min_interval: Duration::from_secs(1),
        },
    )
}
