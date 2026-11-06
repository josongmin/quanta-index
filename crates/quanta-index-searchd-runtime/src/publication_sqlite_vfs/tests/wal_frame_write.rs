use super::*;
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
        if rows.len() >= 4096 {
            return false;
        }
        rows.push(event);
        true
    }
}

fn gold_rows(connection: &Connection) -> rusqlite::Result<Vec<(i64, i64)>> {
    let mut query = connection.prepare("SELECT n,length(payload) FROM gold ORDER BY n")?;
    let rows = query.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    rows.collect()
}

#[test]
fn native_frame_bytes_sync_points_and_first_errors_match_original_writes() -> TestResult {
    quanta_capacity_vfs::test_packed_wal_frame()
        .map_err(|error| std::io::Error::other(format!("native WAL frame oracle: {error:?}")))?;
    Ok(())
}

#[test]
fn native_append_batch_bytes_sync_boundaries_and_first_errors_match_scalar() -> TestResult {
    quanta_capacity_vfs::test_wal_append_batch()
        .map_err(|error| std::io::Error::other(format!("native WAL append oracle: {error:?}")))?;
    Ok(())
}

#[test]
fn real_wal_packs_4k_frames_and_preserves_8k_fallback_and_custody() -> TestResult {
    for page_size in [1024_u64, 4096, 8192] {
        let fixture = fixture_with_boot_input_and_mode(
            |root| {
                // Independent pre-boot database: inventory must account its
                // existing inode before any controlled SQLite operation.
                let connection = Connection::open(root.join("frames.sqlite"))?;
                connection.execute_batch(&format!(
                    "PRAGMA page_size={page_size}; CREATE TABLE gold(n INTEGER PRIMARY KEY, payload BLOB);"
                ))?;
                Ok(64 * 1024 * 1024)
            },
            FundingPolicy::HeldParent,
        )?;
        let allowance = fixture
            .common
            .reserve_publication_allowance(16 * 1024 * 1024, 8)?;
        let funding = fixture.vfs.enter_funded_scope(Arc::clone(&allowance))?;
        let path = fixture.root.join("frames.sqlite");
        let writer = Connection::open_with_flags_and_vfs(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA synchronous=FULL; PRAGMA fullfsync=ON; PRAGMA wal_checkpoint(TRUNCATE);")?;
        let actual_page_size: i64 = writer.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        require(
            u64::try_from(actual_page_size)? == page_size,
            "page size gold differs",
        )?;

        // Keep an older WAL snapshot live while the writer commits. A second
        // connection must later read the committed frames without relying on
        // a last-close checkpoint to move them into the main file.
        let old_reader = Connection::open_with_flags_and_vfs(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        old_reader.execute_batch("BEGIN;")?;
        require(
            gold_rows(&old_reader)?.is_empty(),
            "old WAL snapshot is not empty",
        )?;

        let sink = Arc::new(Events::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([117; 16]),
            Arc::new(Clock::default()),
            sink.clone(),
        );
        let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
        let child = context.child_context(&root);
        let scope = fixture.vfs.enter_scope(Some(&child));
        writer.execute_batch("BEGIN IMMEDIATE; INSERT INTO gold VALUES(1,zeroblob(7000)),(2,zeroblob(7000)),(3,zeroblob(7000)),(4,zeroblob(7000)),(5,zeroblob(7000)),(6,zeroblob(7000)),(7,zeroblob(7000)),(8,zeroblob(7000)); COMMIT;")?;
        let wal_bytes = fs::metadata(path.with_file_name("frames.sqlite-wal"))?.len();
        let frame_size = page_size.checked_add(24).ok_or("frame width overflow")?;
        let frame_bytes = wal_bytes.checked_sub(32).ok_or("WAL header absent")?;
        require(frame_bytes % frame_size == 0, "WAL has partial frame bytes")?;
        let frames = frame_bytes
            .checked_div(frame_size)
            .ok_or("WAL frame width was zero")?;
        require(frames > 1, "WAL did not contain multiple real frames")?;
        let original_calls = frames
            .checked_mul(if page_size <= 4096 { 1 } else { 2 })
            .and_then(|calls| calls.checked_add(1))
            .ok_or("WAL write call gold overflow")?;
        drop(scope);
        let _finished = root.finish(DiagnosticTerminalV1::Ok);
        let rows = sink
            .0
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        let mut calls = 0_u64;
        let mut requested = 0_u64;
        for event in rows.iter() {
            if let DiagnosticEventKindV1::Counter { key, value, .. } = event.kind {
                if key == DiagnosticCounterV1::SqliteWalWriteCalls {
                    calls = calls.checked_add(value).ok_or("WAL call count overflow")?;
                }
                if key == DiagnosticCounterV1::SqliteWriteRequestedBytes {
                    requested = requested
                        .checked_add(value)
                        .ok_or("WAL requested-byte count overflow")?;
                }
            }
        }
        if page_size <= 4096 {
            // The fresh append contains a WAL header and multiple complete
            // frames. Fewer calls than one per frame proves actual grouping;
            // exact bulk amount/width is checked by the independent C oracle.
            require(
                calls > 1 && calls < original_calls,
                &format!(
                    "WAL bulk calls {calls} did not reduce {original_calls} for {page_size}-byte pages"
                ),
            )?;
        } else {
            require(
                calls == original_calls,
                &format!(
                    "WAL fallback calls {calls} != {original_calls} for {page_size}-byte pages"
                ),
            )?;
        }
        writeln!(
            std::io::stderr().lock(),
            "native-wal-frame-gold page_size={page_size} frames={frames} calls={calls} requested={requested}"
        )?;
        require(
            requested == wal_bytes,
            "packing changed requested physical bytes",
        )?;
        require(
            !rows
                .iter()
                .any(|event| matches!(event.kind, DiagnosticEventKindV1::Health { .. })),
            "frame packing invalidated diagnostics",
        )?;
        drop(rows);
        let values: (i64, i64) = writer.query_row(
            "SELECT count(*),sum(length(payload)) FROM gold",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        require(values == (8, 56_000), "committed frame data changed")?;
        let expected_rows = (1_i64..=8).map(|n| (n, 7000_i64)).collect::<Vec<_>>();
        require(
            gold_rows(&writer)? == expected_rows,
            "committed SQL rows changed",
        )?;
        let integrity: String = writer.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        require(integrity == "ok", "packed frame database integrity failed")?;
        require(
            gold_rows(&old_reader)?.is_empty(),
            "old WAL snapshot saw new frames",
        )?;
        writer.execute_batch(
            "BEGIN IMMEDIATE; INSERT INTO gold VALUES(9,zeroblob(7000)); ROLLBACK;",
        )?;
        require(
            gold_rows(&writer)? == expected_rows,
            "rollback changed committed SQL rows",
        )?;
        drop(writer);
        let reopened = Connection::open_with_flags_and_vfs(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        require(
            gold_rows(&reopened)? == expected_rows,
            "reopened WAL rows changed",
        )?;
        let reopened_integrity: String =
            reopened.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        require(reopened_integrity == "ok", "reopened WAL integrity failed")?;
        old_reader.execute_batch("COMMIT;")?;
        drop(old_reader);
        reopened.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        if page_size == 4096 {
            // This is a real VFS admission refusal on a fresh WAL append.
            // Leave enough held capacity for its 32-byte header, but not a
            // complete multi-page body. No synthetic IO error is injected.
            allowance.resize_available(4096, 4)?;
            let refused = reopened.execute_batch(
                "BEGIN IMMEDIATE;
                 INSERT INTO gold VALUES(9,zeroblob(262144));
                 COMMIT;",
            );
            require(
                matches!(
                    &refused,
                    Err(rusqlite::Error::SqliteFailure(error, _))
                        if error.code == rusqlite::ErrorCode::DiskFull
                ),
                &format!("fresh WAL append did not refuse as SQLite FULL: {refused:?}"),
            )?;
            if !reopened.is_autocommit() {
                reopened.execute_batch("ROLLBACK;")?;
            }
            require(
                gold_rows(&reopened)? == expected_rows,
                "capacity refusal changed committed SQL rows",
            )?;
            allowance.resize_available(16 * 1024 * 1024, 8)?;
            reopened.execute_batch(
                "BEGIN IMMEDIATE;
                 INSERT INTO gold VALUES(9,zeroblob(7));
                 DELETE FROM gold WHERE n=9;
                 COMMIT;",
            )?;
            require(
                gold_rows(&reopened)? == expected_rows,
                "same-connection retry changed fixed SQL gold",
            )?;
            reopened.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        drop(reopened);
        if page_size == 4096 {
            let cold = Connection::open_with_flags_and_vfs(
                &path,
                OpenFlags::SQLITE_OPEN_READ_WRITE,
                fixture.vfs.name(),
            )?;
            require(
                gold_rows(&cold)? == expected_rows,
                "cold reopen after capacity refusal changed fixed SQL gold",
            )?;
            let integrity: String =
                cold.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
            require(
                integrity == "ok",
                "cold reopen after capacity refusal failed integrity check",
            )?;
            drop(cold);
        }
        fixture.vfs.drain_retirements()?;
        if page_size == 4096 {
            let before_cleanup = fixture.authority.snapshot()?;
            if before_cleanup.active_tickets != 0 {
                let pending = fixture.authority.pending_batch(None, 2)?;
                require(
                    before_cleanup.active_tickets == 1
                        && before_cleanup.uncertain_pending == 1
                        && pending.len() == 1
                        && matches!(
                            pending.first().map(|charge| charge.kind),
                            Some(
                                quanta_index_core::publication_capacity::PendingChargeKind::Create
                            )
                        ),
                    &format!(
                        "cold reopen left an unexpected capacity ticket: {before_cleanup:?}, {pending:?}, {:?}",
                        fixture.common.require_quiescent("WAL fault close")
                    ),
                )?;
                let wal_path = path.with_file_name("frames.sqlite-wal");
                let wal_bytes = fs::metadata(&wal_path).map_err(|error| {
                    std::io::Error::other(format!(
                        "pending empty WAL birth was not present at {wal_path:?}: {error}; {:?}",
                        fixture.common.require_quiescent("WAL fault close")
                    ))
                })?.len();
                require(
                    wal_bytes == 0,
                    &format!(
                        "pending WAL birth contains {wal_bytes} bytes; {:?}",
                        fixture.common.require_quiescent("WAL fault close")
                    ),
                )?;
                // The capacity binder independently verifies the original
                // retired native FD, zero-byte birth, sole link and no pins.
                fixture
                    .common
                    .discard_retired_empty_native_create(&wal_path)?;
            }
        }
        drop(funding);
        drop(allowance);
        let retired = fixture.authority.snapshot()?;
        require(
            retired.active_tickets == 0
                && retired.active_allowances == 0
                && retired.uncertain_pending == 0
                && !retired.poisoned
                && fixture.common.is_quiescent()?,
            &format!(
                "frame write custody did not retire: {retired:?}, {:?}",
                fixture.common.require_quiescent("WAL fault final")
            ),
        )?;
    }
    Ok(())
}
