use super::*;
use std::{
    collections::BTreeSet,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticCounterV1,
    DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1,
    DiagnosticRootKindV1, DiagnosticSpanIdV1, DiagnosticStageV1, DiagnosticTerminalV1,
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

impl Events {
    fn rows(&self) -> Result<Vec<DiagnosticEmitV1>, Box<dyn std::error::Error>> {
        Ok(self
            .0
            .lock()
            .map_err(|error| {
                std::io::Error::other(format!("scratch diagnostic sink poisoned: {error}"))
            })?
            .clone())
    }
}

#[test]
fn scratch_sqlite_read_scopes_emit_no_write_events_until_a_real_write() -> TestResult {
    let (_directory, _root, authority, common, _io, space) = disposal_fixture()?;
    let baseline = authority.snapshot()?;
    let mut work_budget = budget()?;
    let allocation = space.allocate(
        ScratchRequest {
            initial_bytes: 256 * 1024,
            max_bytes: 2 * 1024 * 1024,
        },
        &mut work_budget,
    )?;
    let connection = ScratchConnection::open(
        &allocation.path().join("read-observer.sqlite"),
        allocation.as_ref(),
        &work_budget,
    )?;
    {
        let db = connection.scoped(&work_budget)?;
        db.execute_batch(
            "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; \
             CREATE TABLE values_test(v INTEGER PRIMARY KEY); \
             INSERT INTO values_test VALUES(2),(3);",
        )?;
    }
    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let trace = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([74; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = trace.enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(0))?;
    let phase = trace.child_context(&root).enter(
        DiagnosticStageV1::NativeSourceOwnerPrepare,
        DiagnosticLaneV1(0),
    )?;
    let phase_id = phase.span_id();
    let clock_before = clock.0.load(Ordering::Relaxed);
    let events_before = events.rows()?.len();
    {
        let phase_budget = work_budget.scoped_diagnostic_context(Some(trace.child_context(&phase)));
        // Fixed input cardinality; this is an event-count oracle, not a timing
        // threshold. SQL still runs through the actual admitted native VFS.
        for _ordinal in 0..4096 {
            let db = connection.scoped(&phase_budget)?;
            let sum: i64 = db.query_row("SELECT SUM(v) FROM values_test", [], |row| row.get(0))?;
            require(sum == 5, "read-only scratch SQL changed fixed rows")?;
        }
        require(
            clock.0.load(Ordering::Relaxed) == clock_before
                && events.rows()?.len() == events_before,
            "read-only scratch scopes created write diagnostics",
        )?;
        let db = connection.scoped(&phase_budget)?;
        db.execute_batch("INSERT INTO values_test VALUES(7);")?;
        let sum: i64 = db.query_row("SELECT SUM(v) FROM values_test", [], |row| row.get(0))?;
        require(sum == 12, "observed scratch write changed fixed SQL gold")?;
    }
    phase.finish(DiagnosticTerminalV1::Ok)?;
    root.finish(DiagnosticTerminalV1::Ok)?;
    let rows = events.rows()?;
    let mut writes = BTreeSet::new();
    let mut observed = BTreeSet::new();
    let mut ended = BTreeSet::new();
    for row in &rows {
        match row.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id,
                stage: DiagnosticStageV1::SqliteWork,
                lane: DiagnosticLaneV1(22),
                ..
            } => {
                require(
                    parent_span_id == Some(phase_id) && writes.insert(span_id),
                    "lazy scratch write span has a wrong or duplicate phase",
                )?;
            }
            DiagnosticEventKindV1::SpanEnd { span_id, .. } => {
                require(ended.insert(span_id), "duplicate diagnostic span end")?;
            }
            DiagnosticEventKindV1::Counter {
                span_id,
                key: DiagnosticCounterV1::SqliteWriteCalls,
                value,
            } if value > 0 => {
                let _inserted = observed.insert(span_id);
            }
            DiagnosticEventKindV1::Health { .. } => {
                return Err(std::io::Error::other("lazy scratch diagnostic health failed").into());
            }
            DiagnosticEventKindV1::SpanBegin { .. }
            | DiagnosticEventKindV1::Counter { .. }
            | DiagnosticEventKindV1::Link { .. }
            | DiagnosticEventKindV1::RequestIdentity { .. }
            | DiagnosticEventKindV1::PhaseIdentity { .. }
            | DiagnosticEventKindV1::AnnRegionObservation { .. }
            | DiagnosticEventKindV1::SqliteSyncObservation { .. }
            | DiagnosticEventKindV1::OwnedRenewalRefusal { .. }
            | DiagnosticEventKindV1::SyncAggregate { .. }
            | DiagnosticEventKindV1::ObjectInstallAggregate { .. }
            | DiagnosticEventKindV1::ObjectKindAggregate { .. }
            | DiagnosticEventKindV1::AdoptionGroupAggregate { .. }
            | DiagnosticEventKindV1::Bm25CensusAggregate { .. }
            | DiagnosticEventKindV1::Bm25BoundaryAggregate { .. }
            | DiagnosticEventKindV1::Bm25LiveBuildAggregate { .. } => {}
        }
    }
    require(
        writes.len() == 1 && writes == observed && writes.is_subset(&ended),
        "one real scratch mutation lacked exactly one closed write aggregate",
    )?;
    for expected in [
        DiagnosticCounterV1::SqliteWriteAdmissionCalls,
        DiagnosticCounterV1::SqliteWriteAdmissionRefusals,
        DiagnosticCounterV1::SqliteWriteAdmissionElapsedNs,
        DiagnosticCounterV1::SqliteWriteLowerEnvelopeElapsedNs,
        DiagnosticCounterV1::SqliteWritePostCallbackElapsedNs,
        DiagnosticCounterV1::SqliteWritePostCallbackRefusals,
        DiagnosticCounterV1::SqliteWriteCalls,
        DiagnosticCounterV1::SqliteWriteRequestedBytes,
        DiagnosticCounterV1::SqliteWriteFailures,
        DiagnosticCounterV1::SqliteWalWriteCalls,
        DiagnosticCounterV1::SqliteMainWriteCalls,
        DiagnosticCounterV1::SqliteOtherWriteCalls,
    ] {
        let count = rows
            .iter()
            .filter(|row| {
                matches!(row.kind, DiagnosticEventKindV1::Counter { span_id, key, .. }
                    if writes.contains(&span_id) && key == expected)
            })
            .count();
        require(
            count == 1,
            "lazy scratch aggregate has missing or duplicate counters",
        )?;
    }
    let after = events.rows()?.len();
    let clock_after = clock.0.load(Ordering::Relaxed);
    drop(connection);
    drop(allocation);
    require(
        events.rows()?.len() == after && clock.0.load(Ordering::Relaxed) == clock_after,
        "scratch close emitted a false observed-write claim",
    )?;
    require(
        authority.snapshot()? == baseline,
        "read/write scratch diagnostics changed capacity custody",
    )?;
    drop(space);
    require(
        common.is_quiescent()?,
        "lazy scratch retained physical custody",
    )
}

#[test]
fn scratch_sqlite_rebinds_each_budget_phase_and_restores_nested_off_after_unwind() -> TestResult {
    let (_directory, _root, authority, common, _io, space) = disposal_fixture()?;
    let baseline = authority.snapshot()?;
    let mut work_budget = budget()?;
    let allocation = space.allocate(
        ScratchRequest {
            initial_bytes: 256 * 1024,
            max_bytes: 2 * 1024 * 1024,
        },
        &mut work_budget,
    )?;
    let path = allocation.path().join("phase.sqlite");

    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let trace = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([73; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = trace.enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(0))?;
    let owner = trace.child_context(&root).enter(
        DiagnosticStageV1::NativeSourceOwnerPrepare,
        DiagnosticLaneV1(0),
    )?;
    let owner_id = owner.span_id();
    let connection = {
        let phase_budget = work_budget.scoped_diagnostic_context(Some(trace.child_context(&owner)));
        let connection = ScratchConnection::open(&path, allocation.as_ref(), &phase_budget)?;
        {
            let db = connection.scoped(&phase_budget)?;
            db.execute_batch(
                "PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; \
                 CREATE TABLE values_test(v INTEGER PRIMARY KEY); \
                 INSERT INTO values_test VALUES(2);",
            )?;
        }
        connection
    };
    owner.finish(DiagnosticTerminalV1::Ok)?;

    let coverage = trace.child_context(&root).enter(
        DiagnosticStageV1::NativeCoverageMapApply,
        DiagnosticLaneV1(0),
    )?;
    let coverage_id = coverage.span_id();
    {
        let phase_budget =
            work_budget.scoped_diagnostic_context(Some(trace.child_context(&coverage)));
        let db = connection.scoped(&phase_budget)?;
        db.execute_batch("INSERT INTO values_test VALUES(3);")?;

        // A separate request with no diagnostic context must mask the live
        // connection's outer Record scope for actual SQLite writes.
        let off_budget = budget()?;
        let off_clock_before = clock.0.load(Ordering::Relaxed);
        let off_events_before = events.rows()?.len();
        {
            let off = connection.scoped(&off_budget)?;
            off.execute_batch("INSERT INTO values_test VALUES(5);")?;
            require(
                clock.0.load(Ordering::Relaxed) == off_clock_before
                    && events.rows()?.len() == off_events_before,
                "nested Off scratch write observed the Record clock or sink",
            )?;
        }
        require(
            clock.0.load(Ordering::Relaxed) == off_clock_before
                && events.rows()?.len() == off_events_before,
            "nested Off scope Drop observed the Record clock or sink",
        )?;
        db.execute_batch("INSERT INTO values_test VALUES(7);")?;

        // A SQL error must not strand the connection in its prior observation.
        require(
            db.execute_batch("INSERT INTO values_test VALUES(7);")
                .is_err(),
            "duplicate fixed key unexpectedly succeeded",
        )?;
        let unwind_clock_before = clock.0.load(Ordering::Relaxed);
        let unwind_events_before = events.rows()?.len();
        let off = connection.scoped(&off_budget)?;
        let unwound = catch_unwind(AssertUnwindSafe(move || {
            let _off = off;
            std::panic::resume_unwind(Box::new("scratch diagnostic scope unwind probe"));
        }));
        require(unwound.is_err(), "scope unwind probe did not panic")?;
        require(
            clock.0.load(Ordering::Relaxed) == unwind_clock_before
                && events.rows()?.len() == unwind_events_before,
            "unwound Off scope observed the Record clock or sink",
        )?;
        db.execute_batch("INSERT INTO values_test VALUES(11);")?;
        let sum: i64 = db.query_row("SELECT SUM(v) FROM values_test", [], |row| row.get(0))?;
        require(sum == 28, "real scratch SQL rows differ from fixed gold")?;
    }
    coverage.finish(DiagnosticTerminalV1::Ok)?;
    root.finish(DiagnosticTerminalV1::Ok)?;
    let before_drop_clock = clock.0.load(Ordering::Relaxed);
    let before_drop_events = events.rows()?.len();
    drop(connection); // Drop intentionally emits no diagnostic I/O claim.
    drop(allocation);
    require(
        clock.0.load(Ordering::Relaxed) == before_drop_clock
            && events.rows()?.len() == before_drop_events,
        "scratch Drop emitted a false complete-I/O diagnostic claim",
    )?;
    require(
        authority.snapshot()? == baseline,
        "scratch close/retirement changed physical capacity custody",
    )?;
    drop(space);
    require(
        common.is_quiescent()?,
        "scratch space retained physical custody",
    )?;

    let rows = events.rows()?;
    let mut write_ids = BTreeSet::<DiagnosticSpanIdV1>::new();
    let mut owner_ids = BTreeSet::new();
    let mut coverage_ids = BTreeSet::new();
    let mut reached_writes = BTreeSet::new();
    let mut ended = BTreeSet::<DiagnosticSpanIdV1>::new();
    for row in &rows {
        if let DiagnosticEventKindV1::SpanBegin {
            span_id,
            parent_span_id,
            stage: DiagnosticStageV1::SqliteWork,
            lane: DiagnosticLaneV1(22),
            ..
        } = row.kind
        {
            require(write_ids.insert(span_id), "duplicate scratch write span")?;
            if parent_span_id == Some(owner_id) {
                let _inserted = owner_ids.insert(span_id);
            } else if parent_span_id == Some(coverage_id) {
                let _inserted = coverage_ids.insert(span_id);
            } else {
                return Err(std::io::Error::other(
                    "scratch write retained an old or unrelated diagnostic parent",
                )
                .into());
            }
        } else if let DiagnosticEventKindV1::SpanEnd { span_id, .. } = row.kind {
            require(ended.insert(span_id), "duplicate diagnostic span end")?;
        } else if let DiagnosticEventKindV1::Counter {
            span_id,
            key: DiagnosticCounterV1::SqliteWriteCalls,
            value,
        } = row.kind
            && value > 0
        {
            let _inserted = reached_writes.insert(span_id);
        } else if let DiagnosticEventKindV1::Health { .. } = row.kind {
            return Err(std::io::Error::other("scratch diagnostic capture health failed").into());
        }
    }
    require(
        write_ids.is_subset(&ended)
            && owner_ids.iter().any(|id| reached_writes.contains(id))
            && coverage_ids.iter().any(|id| reached_writes.contains(id)),
        "real Scratch SQLite writes lacked closed lane22 spans under both current phases",
    )
}
