use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticCounterV1,
    DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticHealthCodeV1,
    DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticSpanIdV1, DiagnosticStageV1,
    DiagnosticTerminalV1,
};

#[derive(Debug, Default)]
struct Clock(AtomicU64);

impl Clock {
    fn set(&self, value: u64) {
        self.0.store(value, Ordering::Relaxed);
    }
}

impl DiagnosticClockV1 for Clock {
    fn now_ns(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Default)]
struct TickClock(AtomicU64);

impl DiagnosticClockV1 for TickClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Debug, Default)]
struct OneShotPanicClock {
    ticks: AtomicU64,
    armed: AtomicBool,
    panics: AtomicU64,
}

impl OneShotPanicClock {
    fn arm(&self) {
        self.armed.store(true, Ordering::Release);
    }
}

impl DiagnosticClockV1 for OneShotPanicClock {
    fn now_ns(&self) -> u64 {
        if self.armed.swap(false, Ordering::AcqRel) {
            let _previous = self.panics.fetch_add(1, Ordering::Relaxed);
            std::panic::resume_unwind(Box::new("one-shot diagnostic clock fault"));
        }
        self.ticks.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Debug, Default)]
struct Events(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for Events {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut rows) = self.0.try_lock() else {
            return false;
        };
        if rows.len() >= 65_536 {
            return false;
        }
        rows.push(event);
        true
    }
}

fn rows(events: &Events) -> Result<Vec<DiagnosticEmitV1>, Box<dyn std::error::Error>> {
    let guard = events
        .0
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let rows = guard.clone();
    drop(guard);
    Ok(rows)
}

fn write_span(rows: &[DiagnosticEmitV1]) -> Result<DiagnosticSpanIdV1, &'static str> {
    rows.iter()
        .find_map(|row| {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                stage: DiagnosticStageV1::SqliteWork,
                lane: DiagnosticLaneV1(22),
                ..
            } = row.kind
            {
                Some(span_id)
            } else {
                None
            }
        })
        .ok_or("missing lane22 write aggregate")
}

fn counter(
    rows: &[DiagnosticEmitV1],
    span: DiagnosticSpanIdV1,
    key: DiagnosticCounterV1,
) -> Option<u64> {
    rows.iter().find_map(|row| {
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
}

fn health(rows: &[DiagnosticEmitV1], code: DiagnosticHealthCodeV1) -> bool {
    rows.iter().any(|row| matches!(row.kind, DiagnosticEventKindV1::Health { code: observed } if observed == code))
}

#[test]
fn record_write_envelopes_have_fixed_clock_gold_and_original_work_counts() -> TestResult {
    let owner = NonZeroU64::new(91).ok_or("zero owner")?;
    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([91; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = diagnostic::Scope::enter(owner, Some(&child));
        clock.set(10);
        let before =
            diagnostic::write_timing::CallbackTiming::begin(owner, 7, EventKind::BeforeWrite)
                .ok_or("missing BeforeWrite token")?;
        clock.set(15);
        before.finish(&Ok(None));
        // The actual lower result is counted independently at AfterWrite.
        diagnostic::write(
            owner,
            rusqlite::ffi::SQLITE_OPEN_WAL,
            4096,
            rusqlite::ffi::SQLITE_OK,
        );
        clock.set(21);
        let after =
            diagnostic::write_timing::CallbackTiming::begin(owner, 7, EventKind::AfterWrite)
                .ok_or("missing AfterWrite token")?;
        clock.set(30);
        after.finish(&Err(Refusal::Io));

        clock.set(40);
        let refused =
            diagnostic::write_timing::CallbackTiming::begin(owner, 8, EventKind::BeforeWrite)
                .ok_or("missing refused BeforeWrite token")?;
        clock.set(44);
        refused.finish(&Err(Refusal::Busy));
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = rows(&events)?;
    let span = write_span(&rows)?;
    for (key, gold) in [
        (DiagnosticCounterV1::SqliteWriteAdmissionCalls, 2),
        (DiagnosticCounterV1::SqliteWriteAdmissionRefusals, 1),
        (DiagnosticCounterV1::SqliteWriteAdmissionElapsedNs, 9),
        (DiagnosticCounterV1::SqliteWriteLowerEnvelopeElapsedNs, 6),
        (DiagnosticCounterV1::SqliteWritePostCallbackElapsedNs, 9),
        (DiagnosticCounterV1::SqliteWritePostCallbackRefusals, 1),
        (DiagnosticCounterV1::SqliteWriteCalls, 1),
        (DiagnosticCounterV1::SqliteWriteRequestedBytes, 4096),
        (DiagnosticCounterV1::SqliteWriteFailures, 0),
    ] {
        require(
            counter(&rows, span, key) == Some(gold),
            "write timing or original work gold changed",
        )?;
    }
    require(
        !rows
            .iter()
            .any(|row| matches!(row.kind, DiagnosticEventKindV1::Health { .. })),
        "valid write callback pair emitted diagnostic health",
    )
}

#[test]
fn off_and_foreign_write_callbacks_do_not_sample_or_bleed_into_record() -> TestResult {
    let owner = NonZeroU64::new(92).ok_or("zero owner")?;
    let foreign = NonZeroU64::new(93).ok_or("zero foreign owner")?;
    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([92; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = diagnostic::Scope::enter(owner, Some(&child));
        let before = clock.0.load(Ordering::Relaxed);
        require(
            diagnostic::write_timing::CallbackTiming::begin(foreign, 4, EventKind::BeforeWrite)
                .is_none(),
            "foreign owner acquired timing token",
        )?;
        {
            let _off = diagnostic::Scope::enter(owner, None);
            require(
                diagnostic::write_timing::CallbackTiming::begin(owner, 4, EventKind::BeforeWrite)
                    .is_none(),
                "nested Off acquired timing token",
            )?;
        }
        require(
            clock.0.load(Ordering::Relaxed) == before,
            "Off or foreign callback sampled diagnostic clock",
        )?;
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = rows(&events)?;
    require(
        write_span(&rows).is_err(),
        "foreign or Off callback opened a Record write aggregate",
    )?;
    require(
        !health(&rows, DiagnosticHealthCodeV1::ForeignCaptureSpan),
        "idle nested Off incorrectly invalidated Record",
    )
}

#[test]
fn mismatched_or_unpaired_write_pair_invalidates_only_timing_series() -> TestResult {
    let owner = NonZeroU64::new(94).ok_or("zero owner")?;
    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([94; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = diagnostic::Scope::enter(owner, Some(&child));
        clock.set(5);
        let before =
            diagnostic::write_timing::CallbackTiming::begin(owner, 7, EventKind::BeforeWrite)
                .ok_or("missing BeforeWrite token")?;
        clock.set(6);
        before.finish(&Ok(None));
        require(
            diagnostic::write_timing::CallbackTiming::begin(owner, 8, EventKind::AfterWrite)
                .is_none(),
            "wrong-handle AfterWrite accepted",
        )?;
        diagnostic::write(
            owner,
            rusqlite::ffi::SQLITE_OPEN_MAIN_DB,
            42,
            rusqlite::ffi::SQLITE_OK,
        );
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = rows(&events)?;
    let span = write_span(&rows)?;
    require(
        health(&rows, DiagnosticHealthCodeV1::ForeignCaptureSpan),
        "handle mismatch lacked health",
    )?;
    require(
        counter(&rows, span, DiagnosticCounterV1::SqliteWriteAdmissionCalls).is_none(),
        "partial timing series was emitted",
    )?;
    require(
        counter(&rows, span, DiagnosticCounterV1::SqliteWriteCalls) == Some(1),
        "timing corruption hid independent native write count",
    )
}

#[test]
fn unfinished_reentrant_regressed_or_transitioned_write_has_health() -> TestResult {
    for case in 0_u8..5 {
        let owner_id = u64::from(case)
            .checked_add(100)
            .ok_or("owner id overflow")?;
        let owner = NonZeroU64::new(owner_id).ok_or("zero owner")?;
        let capture_byte = case.checked_add(100).ok_or("capture id overflow")?;
        let clock = Arc::new(Clock::default());
        let events = Arc::new(Events::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([capture_byte; 16]),
            clock.clone(),
            events.clone(),
        );
        let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
        let child = context.child_context(&root);
        {
            let _scope = diagnostic::Scope::enter(owner, Some(&child));
            clock.set(10);
            let before =
                diagnostic::write_timing::CallbackTiming::begin(owner, 7, EventKind::BeforeWrite)
                    .ok_or("missing initial BeforeWrite token")?;
            match case {
                0 => {
                    clock.set(11);
                    before.finish(&Ok(None));
                    // Scope ends without the required AfterWrite.
                }
                1 => {
                    require(
                        diagnostic::write_timing::CallbackTiming::begin(
                            owner,
                            7,
                            EventKind::BeforeWrite,
                        )
                        .is_none(),
                        "reentrant BeforeWrite acquired a second timer",
                    )?;
                    clock.set(11);
                    before.finish(&Err(Refusal::Io));
                }
                2 => {
                    clock.set(9);
                    before.finish(&Err(Refusal::Io));
                }
                3 => {
                    clock.set(11);
                    before.finish(&Ok(None));
                    {
                        let _off = diagnostic::Scope::enter(owner, None);
                        require(
                            diagnostic::write_timing::CallbackTiming::begin(
                                owner,
                                7,
                                EventKind::AfterWrite,
                            )
                            .is_none(),
                            "nested Off acquired AfterWrite timer",
                        )?;
                    }
                }
                4 => {
                    clock.set(11);
                    before.finish(&Ok(None));
                    diagnostic::checkpoint_start(owner, 7);
                    diagnostic::checkpoint_done(owner, 7);
                }
                _ => return Err("unexpected write timing case".into()),
            }
        }
        let _finished = root.finish(DiagnosticTerminalV1::Ok);
        let rows = rows(&events)?;
        let span = write_span(&rows)?;
        let expected = match case {
            0 | 4 => DiagnosticHealthCodeV1::MissingParentSpan,
            2 => DiagnosticHealthCodeV1::ClockRegressed,
            1 | 3 => DiagnosticHealthCodeV1::ForeignCaptureSpan,
            _ => return Err("unexpected write timing case".into()),
        };
        require(
            health(&rows, expected),
            "invalid callback lacked exact health",
        )?;
        require(
            counter(&rows, span, DiagnosticCounterV1::SqliteWriteAdmissionCalls).is_none(),
            "invalid callback published partial timing counters",
        )?;
    }
    Ok(())
}

#[test]
fn real_sqlite_record_write_timing_preserves_committed_data() -> TestResult {
    let fixture = fixture(64 * 1024 * 1024)?;
    let admission: Arc<dyn CatalogIoAdmission> = fixture.vfs.clone();
    let catalog =
        SqliteCatalog::open_with_io_admission(&fixture.root, Duration::from_secs(1), admission)?;
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([95; 16]),
        Arc::new(TickClock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = fixture.vfs.enter_scope(Some(&child));
        let writer = Connection::open_with_flags_and_vfs(
            catalog.path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        writer.execute_batch("CREATE TABLE write_timing_gold(value INTEGER); INSERT INTO write_timing_gold VALUES(17);")?;
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let observed: i64 = {
        let _off = fixture.vfs.enter_scope(None);
        let reader = Connection::open_with_flags_and_vfs(
            catalog.path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        reader.query_row("SELECT value FROM write_timing_gold", [], |row| row.get(0))?
    };
    require(observed == 17, "Record timing changed committed data")?;
    let rows = rows(&events)?;
    let span = write_span(&rows)?;
    let admissions = counter(&rows, span, DiagnosticCounterV1::SqliteWriteAdmissionCalls)
        .ok_or("missing admission count")?;
    let native = counter(&rows, span, DiagnosticCounterV1::SqliteWriteCalls)
        .ok_or("missing native count")?;
    require(
        admissions > 0 && admissions == native,
        "accepted native writes must have one BeforeWrite and one AfterWrite callback",
    )?;
    let admission_refusals = counter(
        &rows,
        span,
        DiagnosticCounterV1::SqliteWriteAdmissionRefusals,
    )
    .ok_or("missing admission refusal count")?;
    let post_refusals = counter(
        &rows,
        span,
        DiagnosticCounterV1::SqliteWritePostCallbackRefusals,
    )
    .ok_or("missing post-callback refusal count")?;
    require(
        admission_refusals == 0 && post_refusals == 0,
        "successful SQLite write was reported as a VFS refusal",
    )?;
    let mut partition = 0_u64;
    for key in [
        DiagnosticCounterV1::SqliteWalWriteCalls,
        DiagnosticCounterV1::SqliteMainWriteCalls,
        DiagnosticCounterV1::SqliteOtherWriteCalls,
    ] {
        partition = partition
            .checked_add(counter(&rows, span, key).ok_or("missing native write partition")?)
            .ok_or("native write partition overflow")?;
    }
    require(partition == native, "native write partition changed")?;
    for key in [
        DiagnosticCounterV1::SqliteWriteAdmissionElapsedNs,
        DiagnosticCounterV1::SqliteWriteLowerEnvelopeElapsedNs,
        DiagnosticCounterV1::SqliteWritePostCallbackElapsedNs,
    ] {
        require(
            counter(&rows, span, key).is_some_and(|elapsed| elapsed > 0),
            "real Record path lacked a positive write interval",
        )?;
    }
    require(
        !rows
            .iter()
            .any(|row| matches!(row.kind, DiagnosticEventKindV1::Health { .. })),
        "real SQLite Record timing invalidated capture",
    )
}

#[test]
fn one_shot_write_clock_panic_does_not_refuse_or_lose_sqlite_work() -> TestResult {
    let fixture = fixture(64 * 1024 * 1024)?;
    let admission: Arc<dyn CatalogIoAdmission> = fixture.vfs.clone();
    let catalog =
        SqliteCatalog::open_with_io_admission(&fixture.root, Duration::from_secs(1), admission)?;
    let events = Arc::new(Events::default());
    let clock = Arc::new(OneShotPanicClock::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([96; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _scope = fixture.vfs.enter_scope(Some(&child));
        let writer = Connection::open_with_flags_and_vfs(
            catalog.path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        writer.execute_batch(
            "PRAGMA synchronous=FULL; \
             CREATE TABLE write_clock_panic_gold(value INTEGER); \
             INSERT INTO write_clock_panic_gold VALUES(0);",
        )?;
        // Settle warmup writes with the product's FULL sync policy before
        // arming the fault. The warm WAL UPDATE reaches xWrite before its sync.
        clock.arm();
        writer.execute_batch("UPDATE write_clock_panic_gold SET value=23;")?;
    }
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let observed: i64 = {
        let _off = fixture.vfs.enter_scope(None);
        let reader = Connection::open_with_flags_and_vfs(
            catalog.path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE,
            fixture.vfs.name(),
        )?;
        reader.query_row("SELECT value FROM write_clock_panic_gold", [], |row| {
            row.get(0)
        })?
    };
    require(observed == 23, "clock panic changed committed SQLite work")?;
    require(
        clock.panics.load(Ordering::Relaxed) == 1 && !clock.armed.load(Ordering::Relaxed),
        "diagnostic clock panicked more than once or fault was not reached",
    )?;
    let rows = rows(&events)?;
    let span = write_span(&rows)?;
    require(
        health(&rows, DiagnosticHealthCodeV1::MissingParentSpan),
        "clock panic did not invalidate timing capture",
    )?;
    require(
        counter(&rows, span, DiagnosticCounterV1::SqliteWriteAdmissionCalls).is_none(),
        "clock panic published a partial write timing series",
    )?;
    require(
        counter(&rows, span, DiagnosticCounterV1::SqliteWriteCalls).is_some_and(|value| value > 0),
        "clock panic hid original native write observation",
    )
}
