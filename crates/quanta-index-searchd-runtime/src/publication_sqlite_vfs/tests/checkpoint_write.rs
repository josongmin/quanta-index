use super::*;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticCounterV1,
    DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1,
    DiagnosticRootKindV1, DiagnosticTerminalV1,
};

#[derive(Debug, Default)]
struct Clock(AtomicU64);

impl DiagnosticClockV1 for Clock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Debug, Default)]
struct Events(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for Events {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut rows) = self.0.try_lock() else {
            return false;
        };
        if rows.len() >= 8192 {
            return false;
        }
        rows.push(event);
        true
    }
}

fn rows(connection: &Connection) -> rusqlite::Result<Vec<(i64, i64)>> {
    let mut statement =
        connection.prepare("SELECT n,length(payload) FROM checkpoint_gold ORDER BY n")?;
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

// The committed WAL page images are an independent byte oracle for the main
// database after a complete backfill. The test does not ask the new helper for
// expected bytes or assume a failed write leaves no physical prefix.
fn wal_page_images(
    path: &Path,
    page_size: usize,
) -> Result<BTreeMap<u32, Vec<u8>>, Box<dyn std::error::Error>> {
    let wal = fs::read(path)?;
    let header = wal.get(..32).ok_or("WAL header is missing")?;
    let encoded_size = u32::from_be_bytes(
        header
            .get(8..12)
            .ok_or("WAL page size is missing")?
            .try_into()?,
    );
    require(
        usize::try_from(encoded_size)? == page_size,
        "WAL page size differs",
    )?;
    let frame_size = page_size
        .checked_add(24)
        .ok_or("WAL frame width overflow")?;
    let frames = wal.get(32..).ok_or("WAL frame body is missing")?;
    require(
        !frames.is_empty() && frames.len().checked_rem(frame_size) == Some(0),
        "WAL has no complete frames",
    )?;
    let mut images = BTreeMap::new();
    for frame in frames.chunks_exact(frame_size) {
        let page = u32::from_be_bytes(
            frame
                .get(..4)
                .ok_or("WAL page number is missing")?
                .try_into()?,
        );
        require(page != 0, "WAL contains page zero")?;
        let _previous = images.insert(
            page,
            frame.get(24..).ok_or("WAL page image is missing")?.to_vec(),
        );
    }
    Ok(images)
}

fn with_main_write_count<T>(
    fixture: &Fixture,
    operation: impl FnOnce() -> rusqlite::Result<T>,
) -> Result<(T, u64), Box<dyn std::error::Error>> {
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([41; 16]),
        Arc::new(Clock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    let scope = fixture.vfs.enter_scope(Some(&child));
    let result = operation();
    drop(scope);
    let _finished = root.finish(if result.is_ok() {
        DiagnosticTerminalV1::Ok
    } else {
        DiagnosticTerminalV1::Error { code: 1 }
    });
    let result = result?;
    let observed = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut calls = 0_u64;
    for event in observed.iter() {
        if let DiagnosticEventKindV1::Counter { key, value, .. } = event.kind
            && key == DiagnosticCounterV1::SqliteMainWriteCalls
        {
            calls = calls
                .checked_add(value)
                .ok_or("main write count overflow")?;
        }
    }
    drop(observed);
    Ok((result, calls))
}

type CheckpointCounts = (i64, i64, i64);

fn checkpoint_with_main_write_count(
    fixture: &Fixture,
    connection: &Connection,
) -> Result<(CheckpointCounts, u64), Box<dyn std::error::Error>> {
    with_main_write_count(fixture, || {
        connection.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
    })
}

#[test]
fn native_checkpoint_batch_bytes_and_refusals_match_scalar() -> TestResult {
    quanta_capacity_vfs::test_checkpoint_batch_protocol().map_err(|error| {
        std::io::Error::other(format!("native checkpoint batch oracle: {error:?}")).into()
    })
}

#[test]
fn real_checkpoint_backfills_exact_wal_images_and_keeps_8k_scalar() -> TestResult {
    for page_size in [1024_usize, 4096, 8192] {
        let fixture = fixture_with_boot_input_and_mode(
            |root| {
                let connection = Connection::open(root.join("checkpoint.sqlite"))?;
                connection.execute_batch(&format!(
                    "PRAGMA page_size={page_size}; CREATE TABLE checkpoint_gold(n INTEGER PRIMARY KEY,payload BLOB);"
                ))?;
                Ok(64 * 1024 * 1024)
            },
            FundingPolicy::HeldParent,
        )?;
        let allowance = fixture
            .common
            .reserve_publication_allowance(16 * 1024 * 1024, 8)?;
        let funding = fixture.vfs.enter_funded_scope(Arc::clone(&allowance))?;
        let path = fixture.root.join("checkpoint.sqlite");
        let writer = Connection::open_with_flags_and_vfs(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        writer.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA fullfsync=ON;",
        )?;
        let automatic: i64 = writer.query_row("PRAGMA wal_autocheckpoint", [], |row| row.get(0))?;
        require(automatic == 1000, "default WAL auto-checkpoint changed")?;
        writer.execute_batch("PRAGMA wal_autocheckpoint=0; PRAGMA wal_checkpoint(TRUNCATE);")?;
        let actual_page_size: i64 = writer.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        require(
            usize::try_from(actual_page_size)? == page_size,
            "page size changed",
        )?;

        let old_reader = Connection::open_with_flags_and_vfs(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        old_reader.execute_batch("BEGIN;")?;
        require(rows(&old_reader)?.is_empty(), "old snapshot is not empty")?;
        writer.execute_batch(
            "BEGIN IMMEDIATE; INSERT INTO checkpoint_gold VALUES(7,zeroblob(262144)); COMMIT;",
        )?;
        let wal = path.with_file_name("checkpoint.sqlite-wal");
        let images = wal_page_images(&wal, page_size)?;
        require(
            images.len() > 8,
            "fixture did not create more than eight DB pages",
        )?;
        let has_adjacent_eight = images
            .keys()
            .copied()
            .collect::<Vec<_>>()
            .windows(8)
            .any(|run| {
                run.windows(2).all(|pair| {
                    pair.first().and_then(|page| page.checked_add(1)) == pair.get(1).copied()
                })
            });
        require(
            has_adjacent_eight,
            "fixture has no adjacent eight-page checkpoint run",
        )?;
        let mut expected = vec![(7_i64, 262_144_i64)];
        require(rows(&writer)? == expected, "committed row differs")?;
        require(
            rows(&old_reader)?.is_empty(),
            "old reader saw later WAL frames",
        )?;

        let ((busy, logged, checkpointed), held_calls) =
            checkpoint_with_main_write_count(&fixture, &writer)?;
        require(
            busy == 0 && logged > 0 && checkpointed == 0 && held_calls == 0,
            "checkpoint passed a held old WAL snapshot",
        )?;
        old_reader.execute_batch("COMMIT;")?;
        drop(old_reader);
        if page_size == 4096 {
            allowance.resize_available(1, 4)?;
            let refused = writer.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            });
            require(
                matches!(
                    &refused,
                    Err(rusqlite::Error::SqliteFailure(error, _))
                        if error.code == rusqlite::ErrorCode::DiskFull
                ),
                &format!("checkpoint did not retain original capacity refusal: {refused:?}"),
            )?;
            require(
                rows(&writer)? == expected,
                "refused backfill changed committed rows",
            )?;
            // A new connection must still recover the committed WAL view.
            // No claim is made that an unsuccessful write changed zero bytes.
            let after_refusal = Connection::open_with_flags_and_vfs(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE,
                fixture.vfs.name(),
            )?;
            require(
                rows(&after_refusal)? == expected,
                "fresh reader lost committed WAL data after refused backfill",
            )?;
            drop(after_refusal);
            allowance.resize_available(16 * 1024 * 1024, 8)?;
        }
        let ((busy, logged, checkpointed), main_calls) =
            checkpoint_with_main_write_count(&fixture, &writer)?;
        require(
            busy == 0 && logged > 0 && checkpointed == logged,
            "released reader did not permit complete checkpoint",
        )?;
        let main = fs::read(&path)?;
        for (page, image) in &images {
            let slot = usize::try_from(page.checked_sub(1).ok_or("page zero")?)?
                .checked_mul(page_size)
                .ok_or("main page offset overflow")?;
            let end = slot
                .checked_add(page_size)
                .ok_or("main page end overflow")?;
            require(
                main.get(slot..end) == Some(image.as_slice()),
                "checkpoint main page differs from committed WAL image",
            )?;
        }
        let page_count = u64::try_from(images.len())?;
        if page_size <= 4096 {
            require(
                main_calls > 0 && main_calls < page_count,
                "eligible checkpoint did not reduce real main xWrite calls",
            )?;
        } else {
            require(
                main_calls == page_count,
                "8KiB checkpoint did not retain scalar main xWrite calls",
            )?;
        }
        writeln!(
            std::io::stderr().lock(),
            "native-checkpoint-gold page_size={page_size} eligible_pages={page_count} main_calls={main_calls}"
        )?;
        let integrity: String = writer.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        require(integrity == "ok", "checkpoint damaged SQLite integrity")?;
        if page_size == 4096 {
            writer.execute_batch(
                "PRAGMA wal_checkpoint(TRUNCATE); PRAGMA wal_autocheckpoint=1000;",
            )?;
            let ((), automatic_calls) = with_main_write_count(&fixture, || {
                writer.execute_batch(
                    "BEGIN IMMEDIATE; INSERT INTO checkpoint_gold VALUES(8,zeroblob(4194304)); COMMIT;",
                )
            })?;
            require(
                automatic_calls > 0,
                "commit did not execute automatic checkpoint",
            )?;
            let ((busy, logged, checkpointed), remaining_calls) =
                checkpoint_with_main_write_count(&fixture, &writer)?;
            require(
                busy == 0 && logged >= 1000 && checkpointed == logged && remaining_calls == 0,
                "default 1000-frame automatic checkpoint left unbackfilled pages",
            )?;
            expected.push((8, 4_194_304));
            require(
                rows(&writer)? == expected,
                "automatic checkpoint changed row gold",
            )?;
        }
        writer.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        drop(writer);
        let cold = Connection::open_with_flags_and_vfs(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        require(
            rows(&cold)? == expected,
            "cold reopen changed checkpoint gold",
        )?;
        let cold_integrity: String =
            cold.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        require(
            cold_integrity == "ok",
            "cold reopen damaged SQLite integrity",
        )?;
        drop(cold);
        fixture.vfs.drain_retirements()?;
        let before_cleanup = fixture.authority.snapshot()?;
        if before_cleanup.active_tickets != 0 {
            let pending = fixture.authority.pending_batch(None, 2)?;
            require(
                before_cleanup.active_tickets == 1
                    && before_cleanup.uncertain_pending == 1
                    && pending.len() == 1
                    && matches!(
                        pending.first().map(|charge| charge.kind),
                        Some(quanta_index_core::publication_capacity::PendingChargeKind::Create)
                    )
                    && fs::metadata(&wal)?.len() == 0,
                &format!(
                    "cold reopen left unexpected checkpoint custody: {before_cleanup:?}, {pending:?}"
                ),
            )?;
            // The existing binder verifies the retired original inode,
            // sole link, zero-byte birth and absence of pins before removal.
            fixture.common.discard_retired_empty_native_create(&wal)?;
        }
        drop(funding);
        drop(allowance);
        let settled = fixture.authority.snapshot()?;
        require(
            settled.active_tickets == 0
                && settled.active_allowances == 0
                && settled.uncertain_pending == 0
                && !settled.poisoned
                && fixture.common.is_quiescent()?,
            "checkpoint retained original capacity custody",
        )?;
    }
    Ok(())
}
