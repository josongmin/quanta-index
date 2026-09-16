//! G0-C — catalog storage engine probe, `SQLite` candidate (W0 decision gate).
//!
//! W2 wants one durable catalog for operation journal, replay receipts,
//! snapshot/artifact/activation rows and per-key auxiliary epochs, with short
//! publish transactions and an invisible staging epoch. Before a storage crate
//! is admitted, the engine has to demonstrate the properties that design leans
//! on, against the exact linked version, not from documentation:
//!
//! 1. which engine version is linked and which pragmas are actually in effect;
//! 2. commit latency shape for the auxiliary workload (short per-key
//!    transactions) — a *shape* on this host, not a benchmark;
//! 3. a reader inside its own transaction keeps a consistent snapshot while a
//!    writer commits (WAL isolation);
//! 4. a second writer meeting a held write lock gets a typed busy error, never
//!    a silent no-op or a silently applied write;
//! 5. work in an uncommitted transaction is absent after a crash-shaped
//!    disconnect, and committed work is present;
//! 6. an export taken while a writer keeps committing is internally consistent
//!    and passes the engine's own integrity check;
//! 7. whether content bit-rot inside a page is detected. (It is not: `SQLite`
//!    has no page checksums, so `integrity_check` verifies b-tree structure
//!    only and corrupted cell content is served as-is. This is the evidence
//!    that catalog rows must carry their own digests.)
//!
//! (5) uses a real child process that aborts mid-transaction. Leaking a
//! transaction object and dropping the connection normally is *not* a crash:
//! the engine's close path rolls back cleanly, which proves nothing.
//!
//! Emits `G0C-EVIDENCE` lines; run with `-- --nocapture` for the gate ADR.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode, OpenFlags, TransactionBehavior, params};

type ProbeResult = Result<(), Box<dyn Error>>;

const AUX_KEYS: usize = 256;
const AUX_UPDATES_PER_KEY: usize = 8;
/// Rows in the export fixture: sized so the backup spans many pages.
const EXPORT_KEYS: usize = 20_000;

fn open(path: &Path) -> Result<Connection, Box<dyn Error>> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )?;
    // The pragmas W2 would run on every connection. Each is read back below
    // rather than assumed, because `PRAGMA journal_mode` is a request the
    // engine may decline.
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.busy_timeout(Duration::from_millis(50))?;
    Ok(connection)
}

/// The auxiliary-authority shape: one row per `(domain, key)` carrying an
/// epoch and an opaque payload digest, rewritten in place per key.
fn create_schema(connection: &Connection) -> ProbeResult {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS aux_rows (
             domain TEXT NOT NULL,
             key TEXT NOT NULL,
             epoch INTEGER NOT NULL,
             payload_digest TEXT NOT NULL,
             PRIMARY KEY (domain, key)
         ) WITHOUT ROWID;",
    )?;
    Ok(())
}

fn upsert(connection: &Connection, key: usize, epoch: usize) -> ProbeResult {
    let _rows = connection.execute(
        "INSERT INTO aux_rows (domain, key, epoch, payload_digest)
         VALUES ('history', ?1, ?2, ?3)
         ON CONFLICT(domain, key) DO UPDATE SET
             epoch = excluded.epoch,
             payload_digest = excluded.payload_digest",
        params![
            format!("k{key:05}"),
            i64::try_from(epoch)?,
            format!("sha256:{key:05}:{epoch:03}")
        ],
    )?;
    Ok(())
}

fn row_count(connection: &Connection) -> Result<i64, Box<dyn Error>> {
    Ok(connection.query_row("SELECT count(*) FROM aux_rows", [], |row| row.get(0))?)
}

fn percentile_micros(samples: &mut [Duration], pct: usize) -> u128 {
    if samples.is_empty() {
        return 0;
    }
    samples.sort_unstable();
    let rank = pct.saturating_mul(samples.len()).div_ceil(100).max(1);
    samples
        .get(rank.min(samples.len()).saturating_sub(1))
        .map_or(0, std::time::Duration::as_micros)
}

#[expect(
    clippy::print_stdout,
    reason = "the probe's whole purpose is to emit machine-readable gate evidence for the G0-C ADR"
)]
fn evidence(label: &str, fields: &[(&str, String)]) {
    let rendered: Vec<String> = fields
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    println!("G0C-EVIDENCE sqlite {label} {}", rendered.join(" "));
}

/// (1) Linked engine version and the pragmas actually in effect.
#[test]
fn linked_engine_version_and_effective_pragmas() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let connection = open(&dir.path().join("catalog.sqlite"))?;
    let version: String = connection.query_row("SELECT sqlite_version()", [], |row| row.get(0))?;
    let journal_mode: String =
        connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
    let synchronous: i64 = connection.pragma_query_value(None, "synchronous", |row| row.get(0))?;
    let foreign_keys: i64 =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    let busy_timeout: i64 =
        connection.pragma_query_value(None, "busy_timeout", |row| row.get(0))?;
    let page_size: i64 = connection.pragma_query_value(None, "page_size", |row| row.get(0))?;
    evidence(
        "engine",
        &[
            ("sqlite_version", version),
            ("rusqlite_version", rusqlite::version().to_string()),
            ("journal_mode", journal_mode.clone()),
            ("synchronous", synchronous.to_string()),
            ("foreign_keys", foreign_keys.to_string()),
            ("busy_timeout_ms", busy_timeout.to_string()),
            ("page_size", page_size.to_string()),
        ],
    );
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(
            format!("WAL was requested but the effective journal_mode is {journal_mode}").into(),
        );
    }
    if foreign_keys != 1 {
        return Err("foreign_keys pragma was not honored".into());
    }
    Ok(())
}

/// (2) Commit latency shape for short per-key transactions.
///
/// Numbers from this host are a shape, not a budget: other builds were running
/// on the same machine when the gate was taken. What the gate needs is the
/// order of magnitude and the ratio between per-key and batched commits.
#[test]
fn per_key_transaction_commit_latency_shape() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let mut connection = open(&dir.path().join("catalog.sqlite"))?;
    create_schema(&connection)?;

    let mut per_key: Vec<Duration> = Vec::with_capacity(AUX_KEYS * AUX_UPDATES_PER_KEY);
    for epoch in 0..AUX_UPDATES_PER_KEY {
        for key in 0..AUX_KEYS {
            let started = Instant::now();
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            upsert(&transaction, key, epoch)?;
            transaction.commit()?;
            per_key.push(started.elapsed());
        }
    }

    let mut batched: Vec<Duration> = Vec::with_capacity(AUX_UPDATES_PER_KEY);
    for epoch in AUX_UPDATES_PER_KEY..AUX_UPDATES_PER_KEY.saturating_mul(2) {
        let started = Instant::now();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for key in 0..AUX_KEYS {
            upsert(&transaction, key, epoch)?;
        }
        transaction.commit()?;
        batched.push(started.elapsed());
    }

    // `synchronous=NORMAL` under WAL does not fsync per commit; a power loss
    // can drop the most recent commits (never corrupt). A catalog that is the
    // authority for "this was committed" needs the FULL figure too.
    connection.pragma_update(None, "synchronous", "FULL")?;
    let mut per_key_full: Vec<Duration> = Vec::with_capacity(AUX_KEYS);
    for key in 0..AUX_KEYS {
        let started = Instant::now();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        upsert(&transaction, key, 99)?;
        transaction.commit()?;
        per_key_full.push(started.elapsed());
    }

    // On macOS `fsync` does not flush the drive cache; SQLite only issues
    // `F_FULLFSYNC` with `fullfsync=ON`. Without it the FULL figure above is
    // not comparable to an engine that does a real durable sync per commit.
    connection.pragma_update(None, "fullfsync", "ON")?;
    let fullfsync: i64 = connection.pragma_query_value(None, "fullfsync", |row| row.get(0))?;
    let mut per_key_fullfsync: Vec<Duration> = Vec::with_capacity(AUX_KEYS);
    for key in 0..AUX_KEYS {
        let started = Instant::now();
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        upsert(&transaction, key, 100)?;
        transaction.commit()?;
        per_key_fullfsync.push(started.elapsed());
    }

    let final_rows = row_count(&connection)?;
    evidence(
        "commit_latency",
        &[
            ("per_key_commits", per_key.len().to_string()),
            (
                "per_key_p50_us",
                percentile_micros(&mut per_key, 50).to_string(),
            ),
            (
                "per_key_p95_us",
                percentile_micros(&mut per_key, 95).to_string(),
            ),
            (
                "per_key_p99_us",
                percentile_micros(&mut per_key, 99).to_string(),
            ),
            ("batched_commits", batched.len().to_string()),
            ("batched_rows_per_commit", AUX_KEYS.to_string()),
            ("sync_full_per_key_commits", per_key_full.len().to_string()),
            (
                "sync_full_per_key_p50_us",
                percentile_micros(&mut per_key_full, 50).to_string(),
            ),
            (
                "sync_full_per_key_p95_us",
                percentile_micros(&mut per_key_full, 95).to_string(),
            ),
            (
                "sync_full_per_key_p99_us",
                percentile_micros(&mut per_key_full, 99).to_string(),
            ),
            (
                "batched_p50_us",
                percentile_micros(&mut batched, 50).to_string(),
            ),
            (
                "batched_p95_us",
                percentile_micros(&mut batched, 95).to_string(),
            ),
            ("fullfsync_effective", fullfsync.to_string()),
            (
                "fullfsync_per_key_commits",
                per_key_fullfsync.len().to_string(),
            ),
            (
                "fullfsync_per_key_p50_us",
                percentile_micros(&mut per_key_fullfsync, 50).to_string(),
            ),
            (
                "fullfsync_per_key_p95_us",
                percentile_micros(&mut per_key_fullfsync, 95).to_string(),
            ),
            (
                "fullfsync_per_key_p99_us",
                percentile_micros(&mut per_key_fullfsync, 99).to_string(),
            ),
            ("final_rows", final_rows.to_string()),
        ],
    );
    if usize::try_from(final_rows)? != AUX_KEYS {
        return Err(format!("upserts must converge to one row per key, got {final_rows}").into());
    }
    Ok(())
}

/// (3) A reader inside its own transaction keeps a consistent snapshot.
#[test]
fn reader_transaction_keeps_a_consistent_snapshot_across_writer_commits() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("catalog.sqlite");
    let writer = open(&path)?;
    create_schema(&writer)?;
    for key in 0..8 {
        upsert(&writer, key, 0)?;
    }

    let mut reader = open(&path)?;
    let snapshot = reader.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let seen_before: i64 =
        snapshot.query_row("SELECT count(*) FROM aux_rows", [], |row| row.get(0))?;

    for key in 8..64 {
        upsert(&writer, key, 0)?;
    }
    let writer_total = row_count(&writer)?;
    let seen_during: i64 =
        snapshot.query_row("SELECT count(*) FROM aux_rows", [], |row| row.get(0))?;
    snapshot.commit()?;
    let seen_after = row_count(&reader)?;

    evidence(
        "wal_snapshot",
        &[
            ("seen_before_writes", seen_before.to_string()),
            ("writer_total_after_writes", writer_total.to_string()),
            ("seen_inside_open_snapshot", seen_during.to_string()),
            ("seen_after_snapshot_closed", seen_after.to_string()),
        ],
    );
    if seen_before != 8 || seen_during != 8 {
        return Err("open reader snapshot must not observe concurrent commits".into());
    }
    if writer_total != 64 || seen_after != 64 {
        return Err("closing the snapshot must expose the committed writes".into());
    }
    Ok(())
}

/// (4) A second writer meeting a held write lock gets a typed busy error.
#[test]
fn contended_writer_receives_typed_busy_not_silent_outcome() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("catalog.sqlite");
    let mut holder = open(&path)?;
    create_schema(&holder)?;
    let held = holder.transaction_with_behavior(TransactionBehavior::Immediate)?;
    upsert(&held, 1, 1)?;

    let mut contender = open(&path)?;
    let started = Instant::now();
    let attempt = contender.transaction_with_behavior(TransactionBehavior::Immediate);
    let waited = started.elapsed();
    let outcome = match &attempt {
        Ok(_) => "acquired".to_string(),
        Err(err) => format!("{err:?}").replace(' ', "_"),
    };
    let is_busy = matches!(
        &attempt,
        Err(rusqlite::Error::SqliteFailure(failure, _))
            if matches!(failure.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    );
    drop(attempt);
    held.rollback()?;
    let after_release = contender.transaction_with_behavior(TransactionBehavior::Immediate);
    let acquired_after_release = after_release.is_ok();
    drop(after_release);

    evidence(
        "busy",
        &[
            ("contender_outcome", outcome),
            ("contender_waited_ms", waited.as_millis().to_string()),
            ("typed_busy", is_busy.to_string()),
            ("acquired_after_release", acquired_after_release.to_string()),
        ],
    );
    if !is_busy {
        return Err("contended writer must fail with a typed busy/locked error".into());
    }
    if !acquired_after_release {
        return Err("contender must acquire the write lock once it is released".into());
    }
    if row_count(&contender)? != 0 {
        return Err("the rolled-back holder transaction must leave no rows".into());
    }
    Ok(())
}

/// Child-process entry: commit 16 rows, open a second transaction with 16
/// more, then abort without unwinding. Selected by `G0C_CRASH_CHILD_DB`.
///
/// This is a `#[test]` so it lives in the same binary; the parent re-executes
/// the binary with the env var set and this test's name as the filter.
#[test]
fn crash_child_entrypoint() -> ProbeResult {
    let Ok(db) = std::env::var("G0C_CRASH_CHILD_DB") else {
        // Not the child: nothing to do. The parent test below is the one that
        // asserts; this entry only ever runs meaningfully under re-execution.
        return Ok(());
    };
    let mut connection = open(Path::new(&db))?;
    create_schema(&connection)?;
    let committed = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for key in 0..16 {
        upsert(&committed, key, 1)?;
    }
    committed.commit()?;
    let doomed = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for key in 16..32 {
        upsert(&doomed, key, 1)?;
    }
    // Die with the transaction open. `abort` skips destructors, so neither
    // the transaction nor the connection gets a chance to roll back or close.
    std::process::abort();
}

/// (5) Uncommitted work is absent after a real process crash.
#[test]
fn uncommitted_transaction_is_absent_after_child_process_crash() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("catalog.sqlite");
    let status = std::process::Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("crash_child_entrypoint")
        .arg("--nocapture")
        .env("G0C_CRASH_CHILD_DB", &path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    let wal_bytes_after_crash = match std::fs::metadata(dir.path().join("catalog.sqlite-wal")) {
        Ok(meta) => meta.len(),
        // No WAL file at all is a legitimate post-crash state (nothing spilled).
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => 0,
        Err(err) => return Err(err.into()),
    };

    let reopened = open(&path)?;
    let rows = row_count(&reopened)?;
    let integrity: String = reopened.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    evidence(
        "crash_recovery",
        &[
            ("child_exit", format!("{status:?}").replace(' ', "_")),
            ("child_crashed", (!status.success()).to_string()),
            ("committed_rows_expected", "16".to_string()),
            ("rows_after_reopen", rows.to_string()),
            ("wal_bytes_after_crash", wal_bytes_after_crash.to_string()),
            ("integrity_check", integrity.clone()),
        ],
    );
    if status.success() {
        return Err("crash child must not exit cleanly; the probe did not crash".into());
    }
    if rows != 16 {
        return Err(format!("expected only the 16 committed rows after reopen, got {rows}").into());
    }
    if integrity != "ok" {
        return Err(format!("integrity check after crash reopen: {integrity}").into());
    }
    Ok(())
}

/// (6) An export taken while a writer keeps committing is consistent.
///
/// The engine's online backup API (`sqlite3_backup_step`) restarts from the
/// beginning whenever a *different* connection writes to the source, so under
/// continuous writes it never converges — an earlier revision of this probe
/// looped forever on exactly that. `VACUUM INTO` instead runs inside one read
/// transaction and copies that snapshot, which is what W7's frozen export
/// needs. This test uses `VACUUM INTO` and records the backup-API behavior as
/// the reason.
#[test]
fn export_during_writes_is_internally_consistent() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("catalog.sqlite");
    let export_path = dir.path().join("export.sqlite");
    let writer = open(&path)?;
    create_schema(&writer)?;
    for key in 0..EXPORT_KEYS {
        upsert(&writer, key, 0)?;
    }
    writer.pragma_update(None, "wal_checkpoint", "TRUNCATE")?;

    // Writer keeps committing on its own thread while a second connection
    // exports. `stop` is a handshake, not a timer.
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let writer_path = path.clone();
    let background = std::thread::spawn(move || -> Result<u64, String> {
        let mut writer = open(&writer_path).map_err(|err| err.to_string())?;
        let mut commits = 0_u64;
        loop {
            if stop_rx.try_recv().is_ok() {
                return Ok(commits);
            }
            let transaction = writer
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|err| err.to_string())?;
            for key in 0..8 {
                upsert(&transaction, key, 7).map_err(|err| err.to_string())?;
            }
            transaction.commit().map_err(|err| err.to_string())?;
            commits = commits.saturating_add(1);
        }
    });

    let exporter = open(&path)?;
    let started = Instant::now();
    let _rows_affected = exporter.execute(
        "VACUUM INTO ?1",
        params![export_path.to_string_lossy().into_owned()],
    )?;
    let export_took = started.elapsed();
    stop_tx.send(())?;
    let background_commits = background
        .join()
        .map_err(|panic| format!("writer thread panicked: {panic:?}"))??;

    let export = Connection::open(&export_path)?;
    let export_integrity: String =
        export.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    let export_rows = row_count(&export)?;
    let export_epoch_max: i64 =
        export.query_row("SELECT max(epoch) FROM aux_rows", [], |row| row.get(0))?;
    let source_rows = row_count(&writer)?;
    evidence(
        "export",
        &[
            ("method", "VACUUM_INTO".to_string()),
            (
                "backup_api_note",
                "sqlite3_backup_step restarts when another connection writes; does not converge under continuous writes".replace(' ', "_"),
            ),
            ("export_ms", export_took.as_millis().to_string()),
            ("writer_commits_during_export", background_commits.to_string()),
            ("export_integrity", export_integrity.clone()),
            ("export_rows", export_rows.to_string()),
            ("export_epoch_max", export_epoch_max.to_string()),
            ("source_rows", source_rows.to_string()),
        ],
    );
    if export_integrity != "ok" {
        return Err(format!("export failed integrity check: {export_integrity}").into());
    }
    if usize::try_from(export_rows)? != EXPORT_KEYS {
        return Err("export must hold every key exactly once".into());
    }
    if background_commits == 0 {
        return Err(
            "the writer never committed during the export; the probe proved nothing".into(),
        );
    }
    Ok(())
}

/// (7) Content bit-rot inside a page is served, not detected.
///
/// `SQLite` pages carry no checksum. `PRAGMA integrity_check` walks b-tree
/// structure and will pass a page whose cell *content* has been altered. The
/// probe overwrites bytes of a known payload digest in the main file and shows
/// the engine returning the altered value with a clean integrity report. This
/// is the evidence that the catalog's rows must carry their own digests.
#[test]
fn content_bit_rot_is_served_with_a_clean_integrity_check() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("catalog.sqlite");
    let original = "sha256:00042:000";
    {
        let connection = open(&path)?;
        create_schema(&connection)?;
        for key in 0..AUX_KEYS {
            upsert(&connection, key, 0)?;
        }
        connection.pragma_update(None, "wal_checkpoint", "TRUNCATE")?;
    }
    let mut bytes = std::fs::read(&path)?;
    let needle = original.as_bytes();
    // Rewrite the digest's hex tail in place at every occurrence: same length,
    // different content. Superseded page images can hold stale copies, so
    // hitting only the first match may miss the live cell.
    let offsets: Vec<usize> = bytes
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle)
        .map(|(offset, _)| offset)
        .collect();
    if offsets.is_empty() {
        return Err("payload digest must be present verbatim in the main file".into());
    }
    for offset in &offsets {
        let tail = offset.saturating_add(needle.len()).saturating_sub(3);
        for slot in tail..tail.saturating_add(3) {
            if let Some(byte) = bytes.get_mut(slot) {
                *byte = b'f';
            }
        }
    }
    std::fs::write(&path, &bytes)?;

    let connection = open(&path)?;
    let integrity: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    let served: String = connection.query_row(
        "SELECT payload_digest FROM aux_rows WHERE domain = 'history' AND key = 'k00042'",
        [],
        |row| row.get(0),
    )?;
    evidence(
        "content_bit_rot",
        &[
            ("integrity_check", integrity.replace(' ', "_")),
            ("occurrences_corrupted", offsets.len().to_string()),
            ("written", original.to_string()),
            ("served", served.clone()),
            ("served_matches_written", (served == original).to_string()),
        ],
    );
    if integrity != "ok" {
        return Err(format!(
            "unexpected: integrity check caught content-only corruption ({integrity}); revisit the ADR"
        )
        .into());
    }
    if served == original {
        return Err("the corruption did not land on the row; the probe proved nothing".into());
    }
    Ok(())
}
