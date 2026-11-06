use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticCounterV1,
    DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticHealthCodeV1,
    DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticSpanIdV1, DiagnosticStageV1,
    DiagnosticTerminalV1,
};
use rusqlite::ffi::quanta_capacity_vfs::{EventKind, NativeSyncObservation, Refusal};

#[derive(Debug, Default)]
struct Clock {
    ticks: AtomicU64,
    armed: AtomicBool,
    panics: AtomicU64,
}

impl DiagnosticClockV1 for Clock {
    fn now_ns(&self) -> u64 {
        if self.armed.swap(false, Ordering::AcqRel) {
            let _previous = self.panics.fetch_add(1, Ordering::Relaxed);
            std::panic::resume_unwind(Box::new("lazy scratch clock fault"));
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

impl Events {
    fn rows(&self) -> Result<Vec<DiagnosticEmitV1>, Box<dyn std::error::Error>> {
        Ok(self
            .0
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .clone())
    }
}

fn lane_spans(
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

fn has_health(rows: &[DiagnosticEmitV1], code: DiagnosticHealthCodeV1) -> bool {
    rows.iter().any(|row| {
        matches!(row.kind, DiagnosticEventKindV1::Health { code: observed } if observed == code)
    })
}

#[test]
fn first_refused_scratch_write_has_admission_but_no_reached_write() -> TestResult {
    let owner = NonZeroU64::new(71).ok_or("zero owner")?;
    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(DiagnosticCaptureIdV1([71; 16]), clock, events.clone());
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let root_id = root.span_id();
    let child = context.child_context(&root);
    {
        let _scope = diagnostic::Scope::enter_scratch(owner, Some(&child));
        let refusal: Result<Option<CString>, Refusal> = Err(Refusal::Busy);
        let timing =
            diagnostic::write_timing::CallbackTiming::begin(owner, 17, EventKind::BeforeWrite)
                .ok_or("first refused BeforeWrite lacked timing")?;
        timing.finish(&refusal);
        require(
            matches!(refusal, Err(Refusal::Busy)),
            "diagnostics changed original refusal",
        )?;
    }
    root.finish(DiagnosticTerminalV1::Ok)?;
    let rows = events.rows()?;
    let spans = lane_spans(&rows, 22);
    require(
        spans.len() == 1,
        "first refusal did not create exactly one lane22",
    )?;
    let (span, parent) = spans.first().copied().ok_or("missing lane22")?;
    require(parent == Some(root_id), "refusal used a foreign parent")?;
    for (key, gold) in [
        (DiagnosticCounterV1::SqliteWriteAdmissionCalls, 1),
        (DiagnosticCounterV1::SqliteWriteAdmissionRefusals, 1),
        (DiagnosticCounterV1::SqliteWriteCalls, 0),
        (DiagnosticCounterV1::SqliteWriteRequestedBytes, 0),
    ] {
        require(
            counter(&rows, span, key) == Some(gold),
            "refusal counter differs from fixed gold",
        )?;
    }
    require(
        rows.iter().any(|row| {
            matches!(row.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome: DiagnosticTerminalV1::Ok } if span_id == span)
        }) && !rows.iter().any(|row| matches!(row.kind, DiagnosticEventKindV1::Health { .. })),
        "refused write left an open or invalid aggregate",
    )
}

#[test]
fn first_scratch_write_clock_panic_does_not_refuse_real_sqlite_update() -> TestResult {
    let fixture = fixture(64 * 1024 * 1024)?;
    let path = fixture.root.join("lazy-scratch.sqlite");
    let writer = {
        let _off = fixture.vfs.enter_scratch_scope(None);
        let writer = Connection::open_with_flags_and_vfs(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            fixture.vfs.name(),
        )?;
        writer.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; CREATE TABLE gold(v INTEGER NOT NULL); INSERT INTO gold VALUES(7);")?;
        writer
    };
    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([72; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let child = context.child_context(&root);
    {
        let _record = fixture.vfs.enter_scratch_scope(Some(&child));
        let before = clock.ticks.load(Ordering::Relaxed);
        clock.armed.store(true, Ordering::Release);
        writer.execute_batch("UPDATE gold SET v=23;")?;
        require(
            clock.panics.load(Ordering::Relaxed) == 1,
            "first write did not exercise the observer panic",
        )?;
        require(
            clock.ticks.load(Ordering::Relaxed) > before,
            "panic recovery did not record capture health",
        )?;
    }
    root.finish(DiagnosticTerminalV1::Ok)?;
    let observed: i64 = {
        let _off = fixture.vfs.enter_scratch_scope(None);
        let observed = writer.query_row("SELECT v FROM gold", [], |row| row.get(0))?;
        drop(writer);
        observed
    };
    require(
        observed == 23,
        "observer panic changed committed SQLite work",
    )?;
    let rows = events.rows()?;
    require(
        has_health(&rows, DiagnosticHealthCodeV1::MissingParentSpan)
            && lane_spans(&rows, 22).is_empty(),
        "failed lane activation fabricated a healthy write aggregate",
    )?;
    Ok(())
}

#[derive(Debug)]
struct ReenteringEvents {
    owner: NonZeroU64,
    fired: AtomicBool,
    acquired: AtomicBool,
    rows: Mutex<Vec<DiagnosticEmitV1>>,
}

impl DiagnosticEventSinkV1 for ReenteringEvents {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        if matches!(
            event.kind,
            DiagnosticEventKindV1::SpanBegin {
                stage: DiagnosticStageV1::SqliteWork,
                lane: DiagnosticLaneV1(22),
                ..
            }
        ) && !self.fired.swap(true, Ordering::AcqRel)
            && diagnostic::write_timing::CallbackTiming::begin(
                self.owner,
                31,
                EventKind::BeforeWrite,
            )
            .is_some()
        {
            self.acquired.store(true, Ordering::Release);
        }
        let Ok(mut rows) = self.rows.try_lock() else {
            return false;
        };
        if rows.len() >= 65_536 {
            return false;
        }
        rows.push(event);
        true
    }
}

#[test]
fn reentrant_first_scratch_write_does_not_install_duplicate_lane() -> TestResult {
    let owner = NonZeroU64::new(73).ok_or("zero owner")?;
    let events = Arc::new(ReenteringEvents {
        owner,
        fired: AtomicBool::new(false),
        acquired: AtomicBool::new(false),
        rows: Mutex::new(Vec::new()),
    });
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([73; 16]),
        Arc::new(Clock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let root_id = root.span_id();
    let child = context.child_context(&root);
    {
        let _scope = diagnostic::Scope::enter_scratch(owner, Some(&child));
        require(
            diagnostic::write_timing::CallbackTiming::begin(owner, 31, EventKind::BeforeWrite)
                .is_none(),
            "reentered first write acquired a valid timing token",
        )?;
    }
    root.finish(DiagnosticTerminalV1::Ok)?;
    let rows = events
        .rows
        .lock()
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let spans = lane_spans(&rows, 22);
    let (span, parent) = spans.first().copied().ok_or("missing attempted lane22")?;
    require(
        events.fired.load(Ordering::Acquire)
            && !events.acquired.load(Ordering::Acquire)
            && spans.len() == 1
            && parent == Some(root_id)
            && rows.iter().any(|row| matches!(row.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome: DiagnosticTerminalV1::Aborted } if span_id == span))
            && has_health(&rows, DiagnosticHealthCodeV1::MissingParentSpan),
        "reentrant observer opened a duplicate or silently completed lane22",
    )
}

#[test]
fn sync_only_scratch_scope_preserves_parent_and_no_false_health() -> TestResult {
    let owner = NonZeroU64::new(74).ok_or("zero owner")?;
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([74; 16]),
        Arc::new(Clock::default()),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let root_id = root.span_id();
    let child = context.child_context(&root);
    {
        let _scope = diagnostic::Scope::enter_scratch(owner, Some(&child));
        diagnostic::before(owner, 41, rusqlite::ffi::SQLITE_OPEN_WAL, || {
            diagnostic::MutationClass::NoMutation
        });
        diagnostic::after(
            owner,
            41,
            rusqlite::ffi::SQLITE_OK,
            NativeSyncObservation::default(),
            true,
        );
    }
    root.finish(DiagnosticTerminalV1::Ok)?;
    let rows = events.rows()?;
    let writes = lane_spans(&rows, 22);
    let syncs = lane_spans(&rows, 11);
    let (write_id, parent) = writes.first().copied().ok_or("missing sync-only parent")?;
    let (sync_id, sync_parent) = syncs.first().copied().ok_or("missing sync-only span")?;
    require(
        writes.len() == 1
            && syncs.len() == 1
            && parent == Some(root_id)
            && sync_parent == Some(root_id)
            && counter(&rows, write_id, DiagnosticCounterV1::SqliteWriteCalls) == Some(0)
            && counter(&rows, sync_id, DiagnosticCounterV1::SqliteSyncNoMutation) == Some(1)
            && rows.iter().any(|row| matches!(row.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome: DiagnosticTerminalV1::Ok } if span_id == write_id))
            && rows.iter().any(|row| matches!(row.kind, DiagnosticEventKindV1::SpanEnd { span_id, outcome: DiagnosticTerminalV1::Ok } if span_id == sync_id))
            && !rows.iter().any(|row| matches!(row.kind, DiagnosticEventKindV1::Health { .. })),
        "sync-only scratch path lost its parent or invented a write/health",
    )
}
