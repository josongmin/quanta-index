//! G0-C — catalog storage engine probe, `redb` alternative (W0 decision gate).
//!
//! An ADR that names no alternative is a rubber stamp. `redb` is the obvious
//! embedded-Rust candidate: pure Rust (no C build), MVCC, single writer,
//! crash-safe by design. This target runs the same questions as the `SQLite`
//! probe where they apply, so the two can be compared on evidence:
//!
//! 1. engine identity;
//! 2. commit latency shape for the per-key auxiliary workload;
//! 3. a reader keeps a consistent snapshot while a writer commits;
//! 4. what a second writer meeting a held write transaction experiences;
//! 5. uncommitted work is absent after a real child-process crash;
//! 6. content bit-rot inside the file: detected, or served?
//!
//! Emits `G0C-EVIDENCE redb ...` lines; run with `-- --nocapture`.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use redb::{Database, ReadableDatabase as _, ReadableTableMetadata as _, TableDefinition};

type ProbeResult = Result<(), Box<dyn Error>>;

const AUX_KEYS: usize = 256;
const AUX_UPDATES_PER_KEY: usize = 8;
const AUX: TableDefinition<'_, &str, &str> = TableDefinition::new("aux_rows");

fn open(path: &Path) -> Result<Database, Box<dyn Error>> {
    Ok(Database::create(path)?)
}

fn key_for(key: usize) -> String {
    format!("history\u{1f}k{key:05}")
}

fn digest_for(key: usize, epoch: usize) -> String {
    format!("sha256:{key:05}:{epoch:03}")
}

fn upsert_many(db: &Database, keys: impl Iterator<Item = usize>, epoch: usize) -> ProbeResult {
    let write = db.begin_write()?;
    {
        let mut table = write.open_table(AUX)?;
        for key in keys {
            let _prior = table.insert(key_for(key).as_str(), digest_for(key, epoch).as_str())?;
        }
    }
    write.commit()?;
    Ok(())
}

fn row_count(db: &Database) -> Result<u64, Box<dyn Error>> {
    let read = db.begin_read()?;
    match read.open_table(AUX) {
        Ok(table) => Ok(table.len()?),
        Err(redb::TableError::TableDoesNotExist(_)) => Ok(0),
        Err(err) => Err(err.into()),
    }
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
    println!("G0C-EVIDENCE redb {label} {}", rendered.join(" "));
}

/// (1) Engine identity.
#[test]
fn engine_identity() {
    evidence(
        "engine",
        &[
            ("crate", "redb".to_string()),
            ("version", env!("CARGO_PKG_VERSION").to_string()),
            ("native_build", "none (pure Rust)".to_string()),
        ],
    );
}

/// (2) Commit latency shape for the per-key auxiliary workload.
///
/// `redb` fsyncs on every commit by default (`Durability::Immediate`), so this
/// is comparable to the `SQLite` `synchronous=FULL` figure, not to `NORMAL`.
#[test]
fn per_key_transaction_commit_latency_shape() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let db = open(&dir.path().join("catalog.redb"))?;

    let mut per_key: Vec<Duration> = Vec::with_capacity(AUX_KEYS * AUX_UPDATES_PER_KEY);
    for epoch in 0..AUX_UPDATES_PER_KEY {
        for key in 0..AUX_KEYS {
            let started = Instant::now();
            upsert_many(&db, std::iter::once(key), epoch)?;
            per_key.push(started.elapsed());
        }
    }
    let mut batched: Vec<Duration> = Vec::with_capacity(AUX_UPDATES_PER_KEY);
    for epoch in AUX_UPDATES_PER_KEY..AUX_UPDATES_PER_KEY.saturating_mul(2) {
        let started = Instant::now();
        upsert_many(&db, 0..AUX_KEYS, epoch)?;
        batched.push(started.elapsed());
    }
    let final_rows = row_count(&db)?;
    evidence(
        "commit_latency",
        &[
            (
                "durability",
                "Immediate (fsync per commit; redb default)".replace(' ', "_"),
            ),
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
            (
                "batched_p50_us",
                percentile_micros(&mut batched, 50).to_string(),
            ),
            (
                "batched_p95_us",
                percentile_micros(&mut batched, 95).to_string(),
            ),
            ("final_rows", final_rows.to_string()),
        ],
    );
    if usize::try_from(final_rows)? != AUX_KEYS {
        return Err(format!("upserts must converge to one row per key, got {final_rows}").into());
    }
    Ok(())
}

/// (3) A reader keeps a consistent snapshot while a writer commits (MVCC).
#[test]
fn reader_transaction_keeps_a_consistent_snapshot_across_writer_commits() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let db = open(&dir.path().join("catalog.redb"))?;
    upsert_many(&db, 0..8, 0)?;

    let snapshot = db.begin_read()?;
    let seen_before = snapshot.open_table(AUX)?.len()?;
    upsert_many(&db, 8..64, 0)?;
    let seen_during = snapshot.open_table(AUX)?.len()?;
    drop(snapshot);
    let seen_after = row_count(&db)?;

    evidence(
        "mvcc_snapshot",
        &[
            ("seen_before_writes", seen_before.to_string()),
            ("seen_inside_open_snapshot", seen_during.to_string()),
            ("seen_after_snapshot_closed", seen_after.to_string()),
        ],
    );
    if seen_before != 8 || seen_during != 8 || seen_after != 64 {
        return Err("open reader snapshot must not observe concurrent commits".into());
    }
    Ok(())
}

/// (4) A second writer meeting a held write transaction.
///
/// `redb` has one writer; `begin_write` on a second handle blocks until the
/// first commits or aborts. There is no timeout API: the probe measures that
/// the contender is parked and released, not a typed busy error.
#[test]
fn contended_writer_blocks_until_release() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("catalog.redb");
    let db = std::sync::Arc::new(open(&path)?);
    upsert_many(&db, 0..4, 0)?;

    let held = db.begin_write()?;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let contender_db = std::sync::Arc::clone(&db);
    let contender = thread::spawn(move || -> Result<(), String> {
        entered_tx
            .send(Instant::now())
            .map_err(|err| err.to_string())?;
        let write = contender_db.begin_write().map_err(|err| err.to_string())?;
        acquired_tx
            .send(Instant::now())
            .map_err(|err| err.to_string())?;
        write.abort().map_err(|err| err.to_string())?;
        Ok(())
    });
    let entered_at = entered_rx.recv_timeout(Duration::from_secs(10))?;
    // Prove the contender is parked: it must not have acquired before release.
    let premature = acquired_rx.recv_timeout(Duration::from_millis(200));
    let release_at = Instant::now();
    held.abort()?;
    let acquired_at = acquired_rx.recv_timeout(Duration::from_secs(10))?;
    contender
        .join()
        .map_err(|panic| format!("contender panicked: {panic:?}"))??;

    evidence(
        "contention",
        &[
            ("timeout_api", "none".to_string()),
            ("acquired_before_release", premature.is_ok().to_string()),
            (
                "parked_ms_before_release",
                release_at
                    .duration_since(entered_at)
                    .as_millis()
                    .to_string(),
            ),
            (
                "acquired_ms_after_release",
                acquired_at
                    .duration_since(release_at)
                    .as_millis()
                    .to_string(),
            ),
        ],
    );
    if premature.is_ok() {
        return Err("second writer acquired while the first held the write lock".into());
    }
    Ok(())
}

/// Child-process entry for (5). Selected by `G0C_REDB_CRASH_CHILD_DB`.
#[test]
fn crash_child_entrypoint() -> ProbeResult {
    let Ok(db_path) = std::env::var("G0C_REDB_CRASH_CHILD_DB") else {
        return Ok(());
    };
    let db = open(Path::new(&db_path))?;
    upsert_many(&db, 0..16, 1)?;
    let doomed = db.begin_write()?;
    {
        let mut table = doomed.open_table(AUX)?;
        for key in 16..32 {
            let _prior = table.insert(key_for(key).as_str(), digest_for(key, 1).as_str())?;
        }
    }
    std::process::abort();
}

/// (5) Uncommitted work is absent after a real process crash.
#[test]
fn uncommitted_transaction_is_absent_after_child_process_crash() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("catalog.redb");
    let status = std::process::Command::new(std::env::current_exe()?)
        .arg("--exact")
        .arg("crash_child_entrypoint")
        .arg("--nocapture")
        .env("G0C_REDB_CRASH_CHILD_DB", &path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    let mut reopened = open(&path)?;
    let rows = row_count(&reopened)?;
    let check = reopened.check_integrity();
    evidence(
        "crash_recovery",
        &[
            ("child_crashed", (!status.success()).to_string()),
            ("committed_rows_expected", "16".to_string()),
            ("rows_after_reopen", rows.to_string()),
            ("integrity_check", format!("{check:?}").replace(' ', "_")),
        ],
    );
    if status.success() {
        return Err("crash child must not exit cleanly; the probe did not crash".into());
    }
    if rows != 16 {
        return Err(format!("expected only the 16 committed rows after reopen, got {rows}").into());
    }
    Ok(())
}

/// (6) Content bit-rot inside the file: detected, or served?
///
/// `redb` checksums its b-tree pages (XXH3), so altered content should be
/// refused at open or read time rather than returned.
#[test]
fn content_bit_rot_is_detected_or_served() -> ProbeResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("catalog.redb");
    let original = digest_for(42, 0);
    {
        let db = open(&path)?;
        upsert_many(&db, 0..AUX_KEYS, 0)?;
    }
    let mut bytes = std::fs::read(&path)?;
    let needle = original.as_bytes();
    let offset = bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .ok_or("payload digest must be present verbatim in the file")?;
    let tail = offset.saturating_add(needle.len()).saturating_sub(3);
    for slot in tail..tail.saturating_add(3) {
        if let Some(byte) = bytes.get_mut(slot) {
            *byte = b'f';
        }
    }
    std::fs::write(&path, &bytes)?;

    let outcome: Result<String, String> = (|| {
        let db = Database::open(&path).map_err(|err| format!("open:{err}"))?;
        let read = db.begin_read().map_err(|err| format!("begin_read:{err}"))?;
        let table = read
            .open_table(AUX)
            .map_err(|err| format!("open_table:{err}"))?;
        let value = table
            .get(key_for(42).as_str())
            .map_err(|err| format!("get:{err}"))?
            .map(|guard| guard.value().to_string())
            .ok_or_else(|| "get:missing".to_string())?;
        Ok(value)
    })();
    let integrity = Database::open(&path)
        .and_then(|mut db| db.check_integrity())
        .map_or_else(|err| format!("err:{err}"), |ok| format!("ok:{ok}"));

    evidence(
        "content_bit_rot",
        &[
            ("written", original.clone()),
            (
                "read_outcome",
                match &outcome {
                    Ok(value) => format!("served:{value}"),
                    Err(err) => format!("refused:{err}"),
                }
                .replace(' ', "_"),
            ),
            ("integrity_check", integrity.replace(' ', "_")),
            (
                "served_altered_content",
                matches!(&outcome, Ok(value) if value != &original).to_string(),
            ),
        ],
    );
    // Either outcome is admissible evidence; what the gate forbids is served
    // *altered* content with a clean integrity report.
    if let Ok(value) = &outcome
        && value != &original
        && integrity.starts_with("ok:true")
    {
        return Err("altered content was served with a clean integrity check".into());
    }
    Ok(())
}
