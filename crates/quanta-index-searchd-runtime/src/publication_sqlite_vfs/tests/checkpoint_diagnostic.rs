use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticCounterV1,
    DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticHealthCodeV1,
    DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticSpanIdV1, DiagnosticStageV1,
    DiagnosticTerminalV1,
};

// Exercise the same timing boundaries as RouterCallback while keeping the
// existing independent lower-write count and byte gold unchanged.
fn observe_write(owner: NonZeroU64, flags: i32, amount: i64, sqlite_result: i32) {
    use self::diagnostic::write_timing::CallbackTiming;

    if let Some(timing) = CallbackTiming::begin(owner, 1, EventKind::BeforeWrite) {
        timing.finish(&Ok(None));
    }
    let timing = CallbackTiming::begin(owner, 1, EventKind::AfterWrite);
    diagnostic::write(owner, flags, amount, sqlite_result);
    if let Some(timing) = timing {
        timing.finish(&Ok(None));
    }
}

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

fn span_ids(
    rows: &[DiagnosticEmitV1],
    lane: u16,
) -> Vec<(DiagnosticSpanIdV1, Option<DiagnosticSpanIdV1>)> {
    rows.iter()
        .filter_map(|row| {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id,
                stage: DiagnosticStageV1::SqliteWork,
                lane: DiagnosticLaneV1(observed),
                ..
            } = row.kind
                && observed == lane
            {
                Some((span_id, parent_span_id))
            } else {
                None
            }
        })
        .collect()
}

fn counter(rows: &[DiagnosticEmitV1], span: DiagnosticSpanIdV1, key: DiagnosticCounterV1) -> u64 {
    rows.iter()
        .filter_map(|row| {
            if let DiagnosticEventKindV1::Counter {
                span_id,
                key: observed,
                value,
            } = row.kind
                && span_id == span
                && observed == key
            {
                Some(value)
            } else {
                None
            }
        })
        .sum()
}

#[test]
fn wal_sync_failure_done_without_start_keeps_original_failure_capture() -> TestResult {
    let owner = NonZeroU64::new(57).ok_or("zero owner")?;
    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([57; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = diagnostic::Scope::enter(owner, Some(&child));
        diagnostic::before(owner, 73, rusqlite::ffi::SQLITE_OPEN_WAL, || {
            diagnostic::MutationClass::NoMutation
        });
        diagnostic::after(
            owner,
            73,
            rusqlite::ffi::SQLITE_IOERR_FSYNC,
            rusqlite::ffi::quanta_capacity_vfs::NativeSyncObservation::default(),
            true,
        );
        let before_done = clock.0.load(Ordering::Relaxed);
        let emitted_before_done = events
            .0
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .len();
        // Bundled walCheckpoint calls DONE unconditionally after its copy loop,
        // but skips START when this preceding WAL sync fails.
        diagnostic::checkpoint_done(owner, 73);
        require(
            clock.0.load(Ordering::Relaxed) == before_done
                && events
                    .0
                    .lock()
                    .map_err(|error| std::io::Error::other(error.to_string()))?
                    .len()
                    == emitted_before_done,
            "DONE without START emitted a fabricated backfill or health event",
        )?;
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let wal_syncs = span_ids(&rows, 11);
    require(
        wal_syncs.len() == 1 && span_ids(&rows, 25).is_empty(),
        "WAL error was relabeled as backfill",
    )?;
    let wal_sync = wal_syncs.first().ok_or("missing WAL sync span")?.0;
    require(
        rows.iter().any(|row| {
            matches!(row.kind, DiagnosticEventKindV1::SpanEnd {
            span_id,
            outcome: DiagnosticTerminalV1::Error { code },
        } if span_id == wal_sync && code == rusqlite::ffi::SQLITE_IOERR_FSYNC.unsigned_abs())
        }),
        "failed WAL xSync lost its original lane11 terminal",
    )?;
    require(
        !rows
            .iter()
            .any(|row| matches!(row.kind, DiagnosticEventKindV1::Health { .. })),
        "supported WAL sync failure invalidated diagnostic capture",
    )
}

#[test]
fn checkpoint_window_segments_writes_without_duplication_or_losing_sync() -> TestResult {
    let owner = NonZeroU64::new(51).ok_or("zero owner")?;
    let foreign = NonZeroU64::new(52).ok_or("zero foreign owner")?;
    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([51; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    let scope = diagnostic::Scope::enter(owner, Some(&child));
    observe_write(
        owner,
        rusqlite::ffi::SQLITE_OPEN_WAL,
        7,
        rusqlite::ffi::SQLITE_OK,
    );
    let before_mask = clock.0.load(Ordering::Relaxed);
    {
        let _off = diagnostic::Scope::enter(owner, None);
        diagnostic::checkpoint_start(owner, 99);
        diagnostic::checkpoint_done(owner, 99);
    }
    diagnostic::checkpoint_start(foreign, 99);
    diagnostic::checkpoint_done(foreign, 99);
    require(
        clock.0.load(Ordering::Relaxed) == before_mask,
        "Off/foreign checkpoint marker consumed a clock",
    )?;
    diagnostic::checkpoint_start(owner, 99);
    diagnostic::checkpoint_start(owner, 99);
    observe_write(
        owner,
        rusqlite::ffi::SQLITE_OPEN_MAIN_DB,
        4096,
        rusqlite::ffi::SQLITE_OK,
    );
    diagnostic::before(owner, 99, rusqlite::ffi::SQLITE_OPEN_MAIN_DB, || {
        diagnostic::MutationClass::Overwrite
    });
    diagnostic::after(
        owner,
        99,
        rusqlite::ffi::SQLITE_OK,
        rusqlite::ffi::quanta_capacity_vfs::NativeSyncObservation::default(),
        true,
    );
    diagnostic::checkpoint_done(owner, 100);
    observe_write(
        owner,
        rusqlite::ffi::SQLITE_OPEN_MAIN_DB,
        2048,
        rusqlite::ffi::SQLITE_OK,
    );
    diagnostic::checkpoint_done(owner, 99);
    observe_write(
        owner,
        rusqlite::ffi::SQLITE_OPEN_WAL,
        9,
        rusqlite::ffi::SQLITE_OK,
    );
    drop(scope);
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let windows = span_ids(&rows, 25);
    require(
        windows.len() == 1,
        "duplicate marker created another backfill window",
    )?;
    let window = windows.first().ok_or("missing backfill window")?.0;
    let segments = span_ids(&rows, 22);
    require(
        segments.len() == 3,
        "write aggregate did not split at both markers",
    )?;
    let [before, inside, after] = segments.as_slice() else {
        return Err(std::io::Error::other("backfill segment shape changed").into());
    };
    require(
        inside.1 == Some(window) && before.1 == after.1,
        "backfill writes lack the lane25 parent or original context was not restored",
    )?;
    let total = segments
        .iter()
        .map(|(span, _)| counter(&rows, *span, DiagnosticCounterV1::SqliteWriteRequestedBytes))
        .sum::<u64>();
    require(
        total == 6160,
        "segmented requested bytes were lost or counted twice",
    )?;
    require(
        segments
            .iter()
            .map(|(span, _)| counter(&rows, *span, DiagnosticCounterV1::SqliteWriteCalls))
            .sum::<u64>()
            == 4,
        "segmented write calls were lost or counted twice",
    )?;
    require(
        counter(&rows, inside.0, DiagnosticCounterV1::SqliteMainWriteCalls) == 2
            && counter(
                &rows,
                inside.0,
                DiagnosticCounterV1::SqliteWriteRequestedBytes,
            ) == 6144,
        "main writes were not attributed solely to the backfill child",
    )?;
    require(
        span_ids(&rows, 12)
            .iter()
            .any(|(_, parent)| *parent == Some(window)),
        "native sync observation stopped during a backfill window",
    )?;
    require(
        rows.iter().any(|row| matches!(row.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome: DiagnosticTerminalV1::Ok } if span_id == window)),
        "paired markers did not end their observation window",
    )?;
    require(
        rows.iter()
            .filter(|row| {
                matches!(
                    row.kind,
                    DiagnosticEventKindV1::Health {
                        code: DiagnosticHealthCodeV1::MissingParentSpan
                            | DiagnosticHealthCodeV1::ForeignCaptureSpan
                    }
                )
            })
            .count()
            == 2,
        "duplicate or foreign checkpoint marker lacked health",
    )
}

#[derive(Debug)]
struct ReenteringEvents {
    owner: NonZeroU64,
    rows: Mutex<Vec<DiagnosticEmitV1>>,
    calls: AtomicU64,
    mask_failures: AtomicU64,
    inject_write: AtomicBool,
}
impl DiagnosticEventSinkV1 for ReenteringEvents {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let _current = diagnostic::context(self.owner);
        if self.inject_write.swap(false, Ordering::Relaxed) {
            observe_write(
                self.owner,
                rusqlite::ffi::SQLITE_OPEN_MAIN_DB,
                5,
                rusqlite::ffi::SQLITE_OK,
            );
        }
        {
            let _masked = diagnostic::Scope::enter(self.owner, None);
            if diagnostic::context(self.owner).is_some() {
                let _previous = self.mask_failures.fetch_add(1, Ordering::Relaxed);
                return false;
            }
        }
        let Ok(mut rows) = self.rows.try_lock() else {
            return false;
        };
        rows.push(event);
        let _previous = self.calls.fetch_add(1, Ordering::Relaxed);
        true
    }
}

#[test]
fn checkpoint_transition_sink_reentry_and_nested_scope_restore_owner() -> TestResult {
    let owner = NonZeroU64::new(55).ok_or("zero owner")?;
    let sink = Arc::new(ReenteringEvents {
        owner,
        rows: Mutex::new(Vec::new()),
        calls: AtomicU64::new(0),
        mask_failures: AtomicU64::new(0),
        inject_write: AtomicBool::new(false),
    });
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([55; 16]),
        Arc::new(Clock::default()),
        sink.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    let outer = diagnostic::Scope::enter(owner, Some(&child));
    diagnostic::checkpoint_start(owner, 51);
    {
        let _nested = diagnostic::Scope::enter(owner, Some(&child));
        diagnostic::checkpoint_start(owner, 52);
        observe_write(
            owner,
            rusqlite::ffi::SQLITE_OPEN_MAIN_DB,
            3,
            rusqlite::ffi::SQLITE_OK,
        );
        diagnostic::checkpoint_done(owner, 52);
    }
    observe_write(
        owner,
        rusqlite::ffi::SQLITE_OPEN_MAIN_DB,
        5,
        rusqlite::ffi::SQLITE_OK,
    );
    diagnostic::checkpoint_done(owner, 51);
    drop(outer);
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    require(
        sink.calls.load(Ordering::Relaxed) > 0
            && sink.mask_failures.load(Ordering::Relaxed) == 0
            && diagnostic::context(owner).is_none(),
        "checkpoint transition held a RefCell borrow across a sink callback",
    )?;
    let rows = sink
        .rows
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let windows = span_ids(&rows, 25);
    require(
        windows.len() == 2,
        "nested Record scope damaged checkpoint windows",
    )?;
    require(
        windows.iter().all(|(span, _)| rows.iter().any(|row| {
            matches!(row.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome: DiagnosticTerminalV1::Ok } if span_id == *span)
        })),
        "nested Record scope lost a paired checkpoint marker",
    )?;
    require(
        !rows
            .iter()
            .any(|row| matches!(row.kind, DiagnosticEventKindV1::Health { .. })),
        "checkpoint transition or nested restoration invalidated capture",
    )
}

#[test]
fn unsupported_reentrant_write_marks_transition_capture_unhealthy() -> TestResult {
    let owner = NonZeroU64::new(56).ok_or("zero owner")?;
    let sink = Arc::new(ReenteringEvents {
        owner,
        rows: Mutex::new(Vec::new()),
        calls: AtomicU64::new(0),
        mask_failures: AtomicU64::new(0),
        inject_write: AtomicBool::new(false),
    });
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([56; 16]),
        Arc::new(Clock::default()),
        sink.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = diagnostic::Scope::enter(owner, Some(&child));
        sink.inject_write.store(true, Ordering::Relaxed);
        diagnostic::checkpoint_start(owner, 60);
        diagnostic::checkpoint_done(owner, 60);
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = sink
        .rows
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    require(
        rows.iter().any(|row| {
            matches!(
                row.kind,
                DiagnosticEventKindV1::Health {
                    code: DiagnosticHealthCodeV1::MissingParentSpan
                }
            )
        }),
        "unsupported transition reentry silently lost a write",
    )
}

#[test]
fn unpaired_checkpoint_start_aborts_only_its_observation_window() -> TestResult {
    let owner = NonZeroU64::new(53).ok_or("zero owner")?;
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([53; 16]),
        Arc::new(Clock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = diagnostic::Scope::enter(owner, Some(&child));
        diagnostic::checkpoint_start(owner, 1);
        observe_write(
            owner,
            rusqlite::ffi::SQLITE_OPEN_MAIN_DB,
            4,
            rusqlite::ffi::SQLITE_OK,
        );
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let windows = span_ids(&rows, 25);
    require(windows.len() == 1, "unpaired START lacked a window")?;
    let window = windows.first().ok_or("missing unpaired window")?.0;
    require(
        rows.iter().any(|row| matches!(row.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome: DiagnosticTerminalV1::Aborted } if span_id == window)),
        "unpaired START was labeled complete",
    )
}

#[test]
fn real_automatic_checkpoint_retains_sqlite_results_and_parents_main_writes() -> TestResult {
    let mut results = Vec::new();
    for record in [false, true] {
        let fixture = fixture(64 * 1024 * 1024)?;
        let admission: Arc<dyn CatalogIoAdmission> = fixture.vfs.clone();
        let catalog = SqliteCatalog::open_with_io_admission(
            &fixture.root,
            Duration::from_secs(1),
            admission,
        )?;
        let clock = Arc::new(Clock::default());
        let events = Arc::new(Events::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([54; 16]),
            clock.clone(),
            events.clone(),
        );
        let root = if record {
            Some(context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?)
        } else {
            None
        };
        let selected = root.as_ref().map(|root| context.child_context(root));
        let observed = {
            let _scope = fixture.vfs.enter_scope(selected.as_ref());
            let writer = Connection::open_with_flags_and_vfs(
                catalog.path(),
                OpenFlags::SQLITE_OPEN_READ_WRITE,
                fixture.vfs.name(),
            )?;
            writer.execute_batch(
                "PRAGMA synchronous=FULL;
                 PRAGMA fullfsync=ON;
                 PRAGMA foreign_keys=ON;
                 PRAGMA wal_autocheckpoint=1;
                 CREATE TABLE checkpoint_diagnostic_gold(v INTEGER);
                 INSERT INTO checkpoint_diagnostic_gold VALUES(1);
                 INSERT INTO checkpoint_diagnostic_gold VALUES(2);
                 INSERT INTO checkpoint_diagnostic_gold VALUES(3);",
            )?;
            let journal: String = writer.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
            let synchronous: i64 = writer.query_row("PRAGMA synchronous", [], |row| row.get(0))?;
            let fullfsync: i64 = writer.query_row("PRAGMA fullfsync", [], |row| row.get(0))?;
            let foreign_keys: i64 =
                writer.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
            let autocheckpoint: i64 =
                writer.query_row("PRAGMA wal_autocheckpoint", [], |row| row.get(0))?;
            let sum: i64 =
                writer.query_row("SELECT sum(v) FROM checkpoint_diagnostic_gold", [], |row| {
                    row.get(0)
                })?;
            let count: i64 = writer.query_row(
                "SELECT count(*) FROM checkpoint_diagnostic_gold",
                [],
                |row| row.get(0),
            )?;
            (
                journal,
                synchronous,
                fullfsync,
                foreign_keys,
                autocheckpoint,
                sum,
                count,
            )
        };
        if let Some(root) = root {
            let _finished = root.finish(DiagnosticTerminalV1::Ok);
        }
        require(
            observed.0.eq_ignore_ascii_case("wal")
                && observed.1 == 2
                && observed.2 == 1
                && observed.3 == 1
                && observed.4 == 1
                && observed.5 == 6
                && observed.6 == 3,
            "automatic checkpoint fixture changed Catalog durability or rows",
        )?;
        if record {
            let rows = events
                .0
                .lock()
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let windows = span_ids(&rows, 25);
            require(
                !windows.is_empty(),
                "real automatic checkpoint emitted no backfill markers",
            )?;
            let segments = span_ids(&rows, 22);
            let child_segments: Vec<_> = segments
                .iter()
                .filter(|(_, parent)| windows.iter().any(|(window, _)| *parent == Some(*window)))
                .collect();
            require(
                child_segments.iter().any(|(span, _)| {
                    counter(&rows, *span, DiagnosticCounterV1::SqliteMainWriteCalls) > 0
                }),
                "real backfill main writes were not parented to lane25",
            )?;
            let calls = segments
                .iter()
                .map(|(span, _)| counter(&rows, *span, DiagnosticCounterV1::SqliteWriteCalls))
                .sum::<u64>();
            let classified = [
                DiagnosticCounterV1::SqliteWalWriteCalls,
                DiagnosticCounterV1::SqliteMainWriteCalls,
                DiagnosticCounterV1::SqliteOtherWriteCalls,
            ]
            .into_iter()
            .map(|key| {
                segments
                    .iter()
                    .map(|(span, _)| counter(&rows, *span, key))
                    .sum::<u64>()
            })
            .sum::<u64>();
            require(
                calls > 0 && calls == classified,
                "segmented real write counters duplicated or omitted a class",
            )?;
            require(
                !rows
                    .iter()
                    .any(|row| matches!(row.kind, DiagnosticEventKindV1::Health { .. })),
                "real checkpoint invalidated capture",
            )?;
        } else {
            require(
                events
                    .0
                    .lock()
                    .map_err(|error| std::io::Error::other(error.to_string()))?
                    .is_empty()
                    && clock.0.load(Ordering::Relaxed) == 0,
                "Off checkpoint emitted a diagnostic event or clock",
            )?;
        }
        results.push(observed);
        drop(catalog);
        fixture.vfs.drain_retirements()?;
        require(
            fixture.common.is_quiescent()?,
            "checkpoint fixture retained native handles",
        )?;
    }
    require(
        matches!(results.as_slice(), [off, record] if off == record),
        "Record changed SQLite's observed result",
    )
}
