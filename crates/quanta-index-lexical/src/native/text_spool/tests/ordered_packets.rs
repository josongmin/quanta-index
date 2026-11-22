use super::*;
use quanta_index_core::index::{
    MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort, WorkBudgetDenial,
};
use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticCounterV1,
    DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1,
    DiagnosticRootKindV1, DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1,
    RequestBudgetV1,
};
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering as AtomicOrdering},
};

#[derive(Debug, Default)]
struct PacketClock(AtomicU64);

impl DiagnosticClockV1 for PacketClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, AtomicOrdering::SeqCst)
    }
}

#[derive(Debug, Default)]
struct PacketSink(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for PacketSink {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut events) = self.0.try_lock() else {
            return false;
        };
        events.push(event);
        true
    }
}

#[derive(Clone)]
struct OrderedPacketEnvelope {
    live: Arc<AtomicU64>,
    limit: u64,
}

impl MemoryScopePort for OrderedPacketEnvelope {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        // The existing single-packet fixture keeps its exact narrow bound;
        // the multipage fixture supplies a separate explicit envelope.
        let _previous = self
            .live
            .fetch_update(AtomicOrdering::SeqCst, AtomicOrdering::SeqCst, |used| {
                used.checked_add(bytes).filter(|total| *total <= self.limit)
            })
            .map_err(|_used| IndexError::ResourceLimited)?;
        Ok(MemoryLease::new(bytes, Arc::new(self.clone())))
    }
}

impl MemoryLeaseOwner for OrderedPacketEnvelope {
    fn release(&self, bytes: u64) {
        let _previous = self.live.fetch_sub(bytes, AtomicOrdering::SeqCst);
    }
}

fn packet_trace_budget(
    record: bool,
) -> Result<(WorkBudget, Arc<PacketSink>, Option<DiagnosticSpanGuardV1>), IndexError> {
    let sink = Arc::new(PacketSink::default());
    let mut operation = None;
    let diagnostic = if record {
        let trace = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([71; 16]),
            Arc::new(PacketClock::default()),
            sink.clone(),
        );
        let root = trace
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let child = trace.child_context(&root);
        operation = Some(root);
        Some(child)
    } else {
        None
    };
    let request = RequestBudgetV1::for_duration(Duration::from_secs(10));
    let request = if let Some(diagnostic) = diagnostic {
        request.with_diagnostic_context(diagnostic)
    } else {
        request
    };
    let work = WorkBudget::with_request(
        WorkLimits {
            reads: 1000,
            writes: 1000,
            bytes: 10_000_000,
            mutations: 1000,
        },
        request,
    );
    Ok((work, sink, operation))
}

fn run_packet_trace(
    indices: [usize; 5],
    record: bool,
) -> Result<(WorkUsage, Vec<DiagnosticEmitV1>), IndexError> {
    let (mut work, sink, operation) = packet_trace_budget(record)?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let rows = fixed_rows();
    for index in indices {
        spool.posting_batch(&rows[index..=index], &mut work)?;
    }
    check_fixed_rows(&spool, &mut work)?;
    let usage = work.usage();
    drop(spool);
    if let Some(operation) = operation {
        operation
            .finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    }
    let events = sink
        .0
        .lock()
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?
        .clone();
    Ok((usage, events))
}

fn cold_row(term: &[u8], owner: u8, budget: &WorkBudget) -> Result<PostingScratchRow, IndexError> {
    Ok(PostingScratchRow {
        term: PostingTermKey::copy(term, budget)?,
        owner: [owner; 32],
        expected: None,
        next: Some(object(owner + 10)),
    })
}

fn ordered_packet_path(
    cold: bool,
) -> Result<(WorkUsage, Vec<u8>, Vec<DiagnosticEmitV1>), IndexError> {
    let (mut work, sink, operation) = packet_trace_budget(true)?;
    let live = Arc::new(AtomicU64::new(0));
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(OrderedPacketEnvelope {
        live: Arc::clone(&live),
        limit: 128 * 1024 + 1_772 * 512 + 4_096,
    }))));
    let spool = if cold {
        TextSpool::new_cold_postings(&TestScratch, &mut work)?
    } else {
        TextSpool::new(&TestScratch, &mut work)?
    };
    let keys = [b"\0a", b"\0b", b"\0c", b"\0d", b"\0e"];
    let rows = keys
        .iter()
        .map(|term| cold_row(term.as_slice(), 1, &work))
        .collect::<Result<Vec<_>, _>>()?;
    spool.posting_batch_rows(&rows, &mut work)?;

    let terms = spool.posting_term_page(None, &mut work)?;
    check_eq!(terms.len(), keys.len());
    for (actual, expected) in terms.iter().zip(keys) {
        check_eq!(actual.as_slice(), expected.as_slice());
    }
    let mut cursor = spool.posting_cursor(terms);
    for root in 0..keys.len() {
        let actual = cursor.next_edit(&mut work)?.ok_or(IndexError::Conflict)?;
        check_eq!(
            actual.root(),
            u32::try_from(root).map_err(|_error| IndexError::ResourceLimited)?
        );
        check_eq!(actual.key(), &[1; 32]);
        check_eq!(actual.expected()?, None);
        check_eq!(actual.next()?, Some(object(11)));
    }
    check!(cursor.next_edit(&mut work)?.is_none());
    drop(cursor);
    let payload: Vec<u8> = spool
        .db
        .scoped(&work)?
        .query_row(
            "SELECT payload FROM posting_packet WHERE phase=0 AND run=0 AND page=0",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    let usage = work.usage();
    drop(rows);
    drop(spool);
    check_eq!(live.load(AtomicOrdering::SeqCst), 0);
    if let Some(operation) = operation {
        operation
            .finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    }
    let events = sink
        .0
        .lock()
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?
        .clone();
    Ok((usage, payload, events))
}

#[test]
fn ordered_cold_and_general_packets_keep_wire_work_and_original_envelope() -> Result<(), IndexError>
{
    let (general_work, general_payload, general_events) = ordered_packet_path(false)?;
    let (cold_work, cold_payload, cold_events) = ordered_packet_path(true)?;
    check_eq!(cold_payload, general_payload);
    check_eq!(cold_work, general_work);
    for event in general_events.iter().chain(&cold_events) {
        match event.kind {
            DiagnosticEventKindV1::SpanBegin { stage, lane, .. }
                if stage == DiagnosticStageV1::NativeTextPostingEdit =>
            {
                check!(!matches!(lane.0, 36 | 37 | 39));
            }
            DiagnosticEventKindV1::Health { .. } => {
                return Err(IndexError::BackendViolation(
                    "ordered packet emitted trace health".into(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn run_cold_owner_packets(record: bool) -> Result<(WorkUsage, Vec<DiagnosticEmitV1>), IndexError> {
    let sink = Arc::new(PacketSink::default());
    let mut operation = None;
    let diagnostic = if record {
        let trace = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([72; 16]),
            Arc::new(PacketClock::default()),
            sink.clone(),
        );
        let root = trace
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        let child = trace.child_context(&root);
        operation = Some(root);
        Some(child)
    } else {
        None
    };
    let request = RequestBudgetV1::for_duration(Duration::from_secs(60));
    let request = if let Some(diagnostic) = diagnostic {
        request.with_diagnostic_context(diagnostic)
    } else {
        request
    };
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 10_000,
            writes: 10_000,
            bytes: 100_000_000,
            mutations: 10_000,
        },
        request,
    );
    let spool = TextSpool::new_cold_postings(&TestScratch, &mut work)?;
    let mut batch = Vec::new();
    for owner in [1_u8, 2] {
        for index in 0..1_800_u16 {
            let term = format!("\0t{index:04}");
            batch.push(cold_row(term.as_bytes(), owner, &work)?);
            if batch.len() == quanta_index_scratch_sqlite::MAX_WIDE_WRITE_BATCH_ROWS {
                spool.posting_batch_rows(&batch, &mut work)?;
                batch.clear();
            }
        }
    }
    if !batch.is_empty() {
        spool.posting_batch_rows(&batch, &mut work)?;
    }

    let mut range = PackedRangeCursor::new(&spool, b"\0t0000", &mut work)?;
    let phase_zero: (i64, i64, i64) = spool
        .db
        .scoped(&work)?
        .query_row(
            "SELECT COUNT(*),COUNT(DISTINCT run),COALESCE(MAX(page),-1) \
             FROM posting_packet WHERE phase=0",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| sql(&error))?;
    // Owner boundaries do not flush a run: 3,600 rows pack as
    // 1,772 + 1,772 + 56. The 128-row input batch crosses the owner boundary.
    check_eq!(phase_zero, (3, 3, 0));
    for index in 0..1_800_u16 {
        let term = format!("\0t{index:04}");
        for owner in [1_u8, 2] {
            let row = range.next(&mut work)?.ok_or(IndexError::ResourceLimited)?;
            check_eq!(row.term.as_slice(), term.as_bytes());
            check_eq!(row.owner, [owner; 32]);
            check_eq!(row.expected, None);
            check_eq!(row.next, Some(object(owner + 10)));
        }
    }
    check!(range.next(&mut work)?.is_none());
    let stable = work.usage();
    check!(range.next(&mut work)?.is_none());
    check_eq!(work.usage(), stable);
    drop(range);
    drop(spool);
    if let Some(operation) = operation {
        operation
            .finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    }
    let events = sink
        .0
        .lock()
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?
        .clone();
    Ok((work.usage(), events))
}

fn run_ordered_cold_pages(record: bool) -> Result<(WorkUsage, Vec<DiagnosticEmitV1>), IndexError> {
    const TERMS: u16 = 200;
    const OWNERS: u8 = 18;
    let (setup, sink, operation) = packet_trace_budget(record)?;
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 10_000,
            writes: 10_000,
            bytes: 100_000_000,
            mutations: 10_000,
        },
        setup.request().clone(),
    );
    let live = Arc::new(AtomicU64::new(0));
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(OrderedPacketEnvelope {
        live: Arc::clone(&live),
        limit: 4 * 1024 * 1024,
    }))));
    let spool = TextSpool::new_cold_postings(&TestScratch, &mut work)?;
    let before = work.usage();
    check!(spool.prepare_ordered_cold_run_parent(&mut work)?);
    let guard = spool
        .begin_ordered_cold_run(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut batch = Vec::new();
    for index in 0..TERMS {
        let term = format!("\0t{index:04}");
        for owner in 1..=OWNERS {
            batch.push(cold_row(term.as_bytes(), owner, &work)?);
            if batch.len() == 64 {
                spool.posting_batch_rows(&batch, &mut work)?;
                batch.clear();
            }
        }
    }
    if !batch.is_empty() {
        spool.posting_batch_rows(&batch, &mut work)?;
    }
    guard.finish(&mut work)?;
    let after = work.usage();

    let pages: Vec<(i64, i64, Vec<u8>)> = {
        let db = spool.db.scoped(&work)?;
        let mut statement = db
            .prepare("SELECT run,page,payload FROM posting_packet WHERE phase=0 ORDER BY run,page")
            .map_err(|error| sql(&error))?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|error| sql(&error))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql(&error))?
    };
    check!(pages.len() >= 2);
    for (index, (run, page, payload)) in pages.iter().enumerate() {
        check_eq!(*run, 0);
        check_eq!(
            *page,
            i64::try_from(index).map_err(|_error| IndexError::ResourceLimited)?
        );
        check_eq!(payload.get(..8), Some(b"QITXPR03".as_slice()));
    }
    // Every edit retains its original logical write charge; each physical
    // phase-0 page adds exactly one scratch write.
    check_eq!(
        after.writes.checked_sub(before.writes),
        Some(
            u64::from(TERMS) * u64::from(OWNERS)
                + u64::try_from(pages.len()).map_err(|_error| IndexError::ResourceLimited)?,
        )
    );

    let mut range = PackedRangeCursor::new(&spool, b"\0t0000", &mut work)?;
    for index in 0..TERMS {
        let term = format!("\0t{index:04}");
        for owner in 1..=OWNERS {
            let row = range.next(&mut work)?.ok_or(IndexError::Conflict)?;
            check_eq!(row.term.as_slice(), term.as_bytes());
            check_eq!(row.owner, [owner; 32]);
            check_eq!(row.expected, None);
            check_eq!(row.next, Some(object(owner + 10)));
        }
    }
    check!(range.next(&mut work)?.is_none());
    let stable = work.usage();
    check!(range.next(&mut work)?.is_none());
    check_eq!(work.usage(), stable);
    drop(range);
    drop(spool);
    check_eq!(live.load(AtomicOrdering::SeqCst), 0);
    if let Some(operation) = operation {
        operation
            .finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    }
    let events = sink
        .0
        .lock()
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?
        .clone();
    Ok((work.usage(), events))
}

#[test]
fn ordered_cold_run_groups_multiple_pages_and_preserves_full_merge_output() -> Result<(), IndexError>
{
    let (off, off_events) = run_ordered_cold_pages(false)?;
    let (record, events) = run_ordered_cold_pages(true)?;
    check_eq!(off, record);
    check!(off_events.is_empty());
    check!(
        events
            .iter()
            .all(|event| !matches!(event.kind, DiagnosticEventKindV1::Health { .. }))
    );
    Ok(())
}

#[test]
fn ordered_cold_run_optional_credit_miss_keeps_original_spool_usable() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new_cold_postings(&TestScratch, &mut work)?;
    let epoch = spool.db.change_count()?;
    check!(!spool.prepare_ordered_cold_run_parent(&mut work)?);
    check_eq!(spool.db.change_count()?, epoch);
    check_eq!(work.first_denial(), None);
    spool.check_posting_usable()?;
    spool.posting_batch_rows(&[cold_row(b"\0fallback", 1, &work)?], &mut work)?;
    let mut range = PackedRangeCursor::new(&spool, b"\0fallback", &mut work)?;
    let row = range.next(&mut work)?.ok_or(IndexError::Conflict)?;
    check_eq!(row.term.as_slice(), b"\0fallback");
    check_eq!(row.owner, [1; 32]);
    check_eq!(row.next, Some(object(11)));
    check!(range.next(&mut work)?.is_none());
    Ok(())
}

fn funded_ordered_run_work() -> Result<(WorkBudget, Arc<AtomicU64>), IndexError> {
    let (setup, _sink, _operation) = packet_trace_budget(false)?;
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 10_000,
            writes: 10_000,
            bytes: 100_000_000,
            mutations: 10_000,
        },
        setup.request().clone(),
    );
    let live = Arc::new(AtomicU64::new(0));
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(OrderedPacketEnvelope {
        live: Arc::clone(&live),
        limit: 4 * 1024 * 1024,
    }))));
    Ok((work, live))
}

#[test]
fn abandoned_ordered_cold_run_refuses_partial_page_publication() -> Result<(), IndexError> {
    let (mut work, live) = funded_ordered_run_work()?;
    let spool = TextSpool::new_cold_postings(&TestScratch, &mut work)?;
    let epoch = spool.db.change_count()?;
    check!(spool.prepare_ordered_cold_run_parent(&mut work)?);
    let guard = spool
        .begin_ordered_cold_run(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let mut batch = Vec::new();
    for index in 0..100_u16 {
        let term = format!("\0t{index:04}");
        for owner in 1..=18_u8 {
            batch.push(cold_row(term.as_bytes(), owner, &work)?);
            if batch.len() == 64 {
                spool.posting_batch_rows(&batch, &mut work)?;
                batch.clear();
            }
        }
    }
    if !batch.is_empty() {
        spool.posting_batch_rows(&batch, &mut work)?;
    }
    check!(spool.db.change_count()? > epoch);
    let count: i64 = spool
        .db
        .scoped(&work)?
        .query_row(
            "SELECT COUNT(*) FROM posting_packet WHERE phase=0",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    check_eq!(count, 1);

    let mut denied = WorkBudget::with_request(
        WorkLimits {
            reads: 10_000,
            writes: 0,
            bytes: 100_000_000,
            mutations: 10_000,
        },
        work.request().clone(),
    );
    let next = [cold_row(b"\0tz", 1, &denied)?];
    check_eq!(
        spool.posting_batch_rows(&next, &mut denied).err(),
        Some(IndexError::ResourceLimited)
    );
    check_eq!(denied.first_denial(), Some(WorkBudgetDenial::Writes));
    drop(guard);
    check!(matches!(
        spool.check_posting_usable(),
        Err(IndexError::BackendViolation(message))
            if message == "posting scratch packet was abandoned"
    ));
    check!(PackedRangeCursor::new(&spool, b"\0t0000", &mut work).is_err());
    drop(spool);
    check_eq!(live.load(AtomicOrdering::SeqCst), 0);
    Ok(())
}

#[test]
fn cancelled_ordered_cold_run_poisoned_without_sql_effect() -> Result<(), IndexError> {
    let (mut work, live) = funded_ordered_run_work()?;
    let spool = TextSpool::new_cold_postings(&TestScratch, &mut work)?;
    let epoch = spool.db.change_count()?;
    check!(spool.prepare_ordered_cold_run_parent(&mut work)?);
    let guard = spool
        .begin_ordered_cold_run(&mut work)?
        .ok_or(IndexError::ResourceLimited)?;
    let row = [cold_row(b"\0a", 1, &work)?];
    work.request().cancel_handle().cancel();
    check!(matches!(
        spool.posting_batch_rows(&row, &mut work),
        Err(IndexError::Cancelled)
    ));
    check_eq!(spool.db.change_count()?, epoch);
    drop(guard);
    check!(spool.check_posting_usable().is_err());
    drop(spool);
    check_eq!(live.load(AtomicOrdering::SeqCst), 0);
    Ok(())
}

#[test]
fn ordered_cold_run_rejects_missing_extra_or_corrupt_second_page() -> Result<(), IndexError> {
    for corruption in 0..6_u8 {
        let (mut work, _live) = funded_ordered_run_work()?;
        let spool = TextSpool::new_cold_postings(&TestScratch, &mut work)?;
        check!(spool.prepare_ordered_cold_run_parent(&mut work)?);
        let guard = spool
            .begin_ordered_cold_run(&mut work)?
            .ok_or(IndexError::ResourceLimited)?;
        let mut batch = Vec::new();
        for index in 0..100_u16 {
            let term = format!("\0t{index:04}");
            for owner in 1..=18_u8 {
                batch.push(cold_row(term.as_bytes(), owner, &work)?);
                if batch.len() == 64 {
                    spool.posting_batch_rows(&batch, &mut work)?;
                    batch.clear();
                }
            }
        }
        if !batch.is_empty() {
            spool.posting_batch_rows(&batch, &mut work)?;
        }
        guard.finish(&mut work)?;
        let db = spool.db.scoped(&work)?;
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM posting_packet WHERE phase=0",
                [],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        check_eq!(count, 2);
        let changed = match corruption {
            0 => db.execute(
                "UPDATE posting_packet SET page=2 WHERE phase=0 AND run=0 AND page=1",
                [],
            ),
            1 => db.execute(
                "DELETE FROM posting_packet WHERE phase=0 AND run=0 AND page=1",
                [],
            ),
            2 => db.execute(
                "INSERT INTO posting_packet \
                 SELECT phase,run,2,first_term,first_owner,last_term,last_owner,payload \
                 FROM posting_packet WHERE phase=0 AND run=0 AND page=1",
                [],
            ),
            3 => db.execute(
                "UPDATE posting_packet SET first_term=x'00ff' \
                 WHERE phase=0 AND run=0 AND page=1",
                [],
            ),
            4 => db.execute(
                "UPDATE posting_packet SET first_owner=zeroblob(32) \
                 WHERE phase=0 AND run=0 AND page=1",
                [],
            ),
            _ => db.execute(
                "UPDATE posting_packet SET payload=zeroblob(length(payload)) \
                 WHERE phase=0 AND run=0 AND page=1",
                [],
            ),
        }
        .map_err(|error| sql(&error))?;
        check_eq!(changed, 1);
        drop(db);
        let result = PackedRangeCursor::new(&spool, b"\0t0000", &mut work).and_then(|mut range| {
            for _ in 0..=1_800 {
                if range.next(&mut work)?.is_none() {
                    return Ok(());
                }
            }
            Err(IndexError::Conflict)
        });
        check!(matches!(result, Err(IndexError::CorruptObject(_))));
    }
    Ok(())
}

#[test]
fn cold_owner_runs_merge_shared_terms_without_packet_reorder() -> Result<(), IndexError> {
    let (off, off_events) = run_cold_owner_packets(false)?;
    let (record, events) = run_cold_owner_packets(true)?;
    check_eq!(off, record);
    check!(off_events.is_empty());
    let mut cold_sort_observed = false;
    for event in events {
        match event.kind {
            DiagnosticEventKindV1::SpanBegin { stage, lane, .. }
                if stage == DiagnosticStageV1::NativeTextPostingEdit =>
            {
                check!(!matches!(lane.0, 36 | 37));
                cold_sort_observed |= lane.0 == 39;
            }
            DiagnosticEventKindV1::Health { .. } => {
                return Err(IndexError::BackendViolation(
                    "cold owner run emitted trace health".into(),
                ));
            }
            _ => {}
        }
    }
    check!(cold_sort_observed);

    let mut work = budget()?;
    let spool = TextSpool::new_cold_postings(&TestScratch, &mut work)?;
    let first = [cold_row(b"\0a", 2, &work)?];
    spool.posting_batch_rows(&first, &mut work)?;
    let decreasing_owner = [cold_row(b"\0b", 1, &work)?];
    let result = spool
        .posting_batch_rows(&decreasing_owner, &mut work)
        .and_then(|()| PackedRangeCursor::new(&spool, b"\0a", &mut work).map(|_| ()));
    check!(matches!(result, Err(IndexError::CorruptObject(_))));

    let mut work = budget()?;
    let spool = TextSpool::new_cold_postings(&TestScratch, &mut work)?;
    let repeated = [cold_row(b"\0a", 1, &work)?, cold_row(b"\0a", 1, &work)?];
    let result = spool
        .posting_batch_rows(&repeated, &mut work)
        .and_then(|()| PackedRangeCursor::new(&spool, b"\0a", &mut work).map(|_| ()));
    check!(matches!(result, Err(IndexError::CorruptObject(_))));
    Ok(())
}

#[test]
fn cold_prefix_merge_retries_refused_write_and_rejects_duplicate_key() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new_cold_postings(&TestScratch, &mut work)?;
    spool.posting_batch_rows(
        &[
            cold_row(b"\0b", 1, &work)?,
            cold_row(b"\0c", 1, &work)?,
            cold_row(b"\0a", 2, &work)?,
            cold_row(b"\0b", 2, &work)?,
        ],
        &mut work,
    )?;
    let mut denied = WorkBudget::with_request(
        WorkLimits {
            writes: 0,
            reads: 1_000,
            bytes: 10_000_000,
            mutations: 1_000,
        },
        work.request().clone(),
    );
    let epoch = spool.db.change_count()?;
    check_eq!(
        spool.posting_term_page(None, &mut denied).err(),
        Some(IndexError::ResourceLimited)
    );
    check_eq!(denied.first_denial(), Some(WorkBudgetDenial::Writes));
    check!(denied.usage().bytes > 0);
    check_eq!(spool.db.change_count()?, epoch);

    let mut range = PackedRangeCursor::new(&spool, b"\0a", &mut work)?;
    for (term, owner) in [(b"\0a", 2), (b"\0b", 1), (b"\0b", 2), (b"\0c", 1)] {
        let row = range.next(&mut work)?.ok_or(IndexError::Conflict)?;
        check_eq!(row.term.as_slice(), term.as_slice());
        check_eq!(row.owner, [owner; 32]);
        check_eq!(row.expected, None);
        check_eq!(row.next, Some(object(owner + 10)));
    }
    check!(range.next(&mut work)?.is_none());

    let mut work = budget()?;
    let spool = TextSpool::new_cold_postings(&TestScratch, &mut work)?;
    spool.posting_batch_rows(
        &[
            cold_row(b"\0b", 1, &work)?,
            cold_row(b"\0c", 1, &work)?,
            cold_row(b"\0a", 1, &work)?,
            cold_row(b"\0b", 1, &work)?,
        ],
        &mut work,
    )?;
    let epoch = spool.db.change_count()?;
    check!(matches!(
        spool.posting_term_page(None, &mut work),
        Err(IndexError::CorruptObject(message))
            if message == "posting scratch packet: duplicate cold posting key"
    ));
    check_eq!(spool.db.change_count()?, epoch);
    Ok(())
}

#[test]
fn cold_prefix_comparison_charges_once_per_suffix_term_before_write() -> Result<(), IndexError> {
    let mut work = budget()?;
    let spool = TextSpool::new_cold_postings(&TestScratch, &mut work)?;
    // b,z are already ordered in the packet. Four later owners share one
    // inverted term a, so only one comparison with prefix b is required.
    let rows = [
        cold_row(b"\0b", 1, &work)?,
        cold_row(b"\0z", 1, &work)?,
        cold_row(b"\0a", 2, &work)?,
        cold_row(b"\0a", 3, &work)?,
        cold_row(b"\0a", 4, &work)?,
        cold_row(b"\0a", 5, &work)?,
    ];
    spool.posting_batch_rows(&rows, &mut work)?;
    let epoch = spool.db.change_count()?;
    let mut denied = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000,
            writes: 0,
            bytes: 10_000_000,
            mutations: 1_000,
        },
        work.request().clone(),
    );
    check_eq!(
        spool.posting_term_page(None, &mut denied).err(),
        Some(IndexError::ResourceLimited)
    );
    check_eq!(denied.first_denial(), Some(WorkBudgetDenial::Writes));
    // One unique-term sort: 2 key bytes + 2 index bytes. Six compact rows
    // use 10 bytes each; five owners and five references use 320 bytes.
    // Prefix b is compared with suffix term a once, for 2 more bytes.
    check_eq!(
        denied.usage(),
        WorkUsage {
            bytes: 4 + 6 * 10 + 320 + 2,
            ..WorkUsage::default()
        }
    );
    check_eq!(spool.db.change_count()?, epoch);

    // The denied insert is before a physical effect. The retained packet
    // must still seal and expose the independent term/owner golden order.
    let terms = spool.posting_term_page(None, &mut work)?;
    check_eq!(terms.len(), 3);
    for (actual, expected) in terms.iter().zip([b"\0a", b"\0b", b"\0z"]) {
        check_eq!(actual.as_slice(), expected.as_slice());
    }
    let mut cursor = spool.posting_cursor(terms);
    for (root, owner) in [(0_u32, 2_u8), (0, 3), (0, 4), (0, 5), (1, 1), (2, 1)] {
        let actual = cursor.next_edit(&mut work)?.ok_or(IndexError::Conflict)?;
        check_eq!(actual.root(), root);
        check_eq!(actual.key(), &[owner; 32]);
        check_eq!(actual.expected()?, None);
        check_eq!(actual.next()?, Some(object(owner + 10)));
    }
    check!(cursor.next_edit(&mut work)?.is_none());
    Ok(())
}

fn check_packet_copy_trace(events: &[DiagnosticEmitV1], unordered: bool) -> Result<(), IndexError> {
    let mut begins = BTreeMap::new();
    let mut ends = BTreeMap::new();
    let mut bytes = BTreeMap::new();
    for event in events {
        match event.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id,
                stage,
                lane,
                ..
            } => {
                check!(
                    begins
                        .insert(span_id, (parent_span_id, stage, lane))
                        .is_none()
                );
            }
            DiagnosticEventKindV1::SpanEnd { span_id, outcome } => {
                check!(ends.insert(span_id, outcome).is_none());
            }
            DiagnosticEventKindV1::Counter {
                span_id,
                key: DiagnosticCounterV1::WorkBytes,
                value,
            } => {
                check!(bytes.insert(span_id, value).is_none());
            }
            DiagnosticEventKindV1::Health { .. } => {
                return Err(IndexError::BackendViolation(
                    "packet observation emitted trace health".into(),
                ));
            }
            _ => {}
        }
    }
    let mut phases = BTreeMap::new();
    for (span_id, (_, stage, lane)) in &begins {
        if *stage == DiagnosticStageV1::NativeTextPostingEdit && (36..=38).contains(&lane.0) {
            check!(phases.insert(lane.0, *span_id).is_none());
        }
    }
    if !unordered {
        check!(phases.is_empty());
        return Ok(());
    }
    check_eq!(phases.len(), 3);
    let packet = *phases.get(&38).ok_or(IndexError::ResourceLimited)?;
    let parent = begins.get(&packet).ok_or(IndexError::ResourceLimited)?.0;
    check!(parent.is_some());
    for (lane, gold) in [(36_u16, 365_u64), (37_u16, 50_u64)] {
        let span = *phases.get(&lane).ok_or(IndexError::ResourceLimited)?;
        check_eq!(
            begins.get(&span).ok_or(IndexError::ResourceLimited)?.0,
            Some(packet)
        );
        check_eq!(ends.get(&span), Some(&DiagnosticTerminalV1::Ok));
        check_eq!(bytes.get(&span), Some(&gold));
    }
    check_eq!(ends.get(&packet), Some(&DiagnosticTerminalV1::Ok));
    Ok(())
}

fn check_packet_refusal_trace(events: &[DiagnosticEmitV1]) -> Result<(), IndexError> {
    let mut begins = BTreeMap::new();
    let mut ends = BTreeMap::new();
    let mut bytes = BTreeMap::new();
    let mut packets = Vec::new();
    let mut probe = None;
    for event in events {
        match event.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id,
                stage,
                lane,
                ..
            } => {
                check!(
                    begins
                        .insert(span_id, (parent_span_id, stage, lane))
                        .is_none()
                );
                if stage == DiagnosticStageV1::NativeTextPostingEdit {
                    match lane.0 {
                        38 => packets.push(span_id),
                        39 => check!(probe.replace(span_id).is_none()),
                        _ => {}
                    }
                }
            }
            DiagnosticEventKindV1::SpanEnd { span_id, outcome } => {
                check!(ends.insert(span_id, outcome).is_none());
            }
            DiagnosticEventKindV1::Counter {
                span_id,
                key: DiagnosticCounterV1::WorkBytes,
                value,
            } => {
                check!(bytes.insert(span_id, value).is_none());
            }
            DiagnosticEventKindV1::Health { .. } => {
                return Err(IndexError::BackendViolation(
                    "packet refusal emitted trace health".into(),
                ));
            }
            _ => {}
        }
    }
    check_eq!(packets.len(), 2);
    let parent = begins
        .get(&packets[0])
        .ok_or(IndexError::ResourceLimited)?
        .0;
    check!(parent.is_some());
    check_eq!(
        begins
            .get(&packets[1])
            .ok_or(IndexError::ResourceLimited)?
            .0,
        parent
    );
    let probe = probe.ok_or(IndexError::ResourceLimited)?;
    check_eq!(
        begins.get(&probe).ok_or(IndexError::ResourceLimited)?.0,
        parent
    );
    check_eq!(ends.get(&probe), Some(&DiagnosticTerminalV1::Ok));
    for (index, packet) in packets.into_iter().enumerate() {
        let expected = if index == 0 {
            DiagnosticTerminalV1::Error { code: 1 }
        } else {
            DiagnosticTerminalV1::Ok
        };
        check_eq!(ends.get(&packet), Some(&expected));
        let mut children = BTreeMap::new();
        for (span, (parent, stage, lane)) in &begins {
            if *parent == Some(packet)
                && *stage == DiagnosticStageV1::NativeTextPostingEdit
                && matches!(lane.0, 36 | 37)
            {
                check!(children.insert(lane.0, *span).is_none());
            }
        }
        check_eq!(children.len(), 2);
        for (lane, gold) in [(36_u16, 365_u64), (37_u16, 50_u64)] {
            let child = children.get(&lane).ok_or(IndexError::ResourceLimited)?;
            check_eq!(ends.get(child), Some(&DiagnosticTerminalV1::Ok));
            check_eq!(bytes.get(child), Some(&gold));
        }
        if index == 0 {
            // Refused SQL insertion accepts the sort, body copy and dictionary
            // copy, but not the rejected packet-write charge.
            check_eq!(bytes.get(&packet), Some(&575));
        }
    }
    Ok(())
}

fn run_packet_refusal_trace(
    record: bool,
) -> Result<(WorkUsage, WorkUsage, WorkUsage, Vec<DiagnosticEmitV1>), IndexError> {
    let (mut work, sink, operation) = packet_trace_budget(record)?;
    let spool = TextSpool::new(&TestScratch, &mut work)?;
    let rows = fixed_rows();
    for index in [3, 2, 4, 0, 1] {
        spool.posting_batch(&rows[index..=index], &mut work)?;
    }
    let producer_usage = work.usage();
    let epoch = spool.db.change_count()?;
    let original = work.request().clone();
    let mut denied = WorkBudget::with_request(
        WorkLimits {
            reads: 1000,
            writes: 0,
            bytes: 10_000_000,
            mutations: 1000,
        },
        original.clone(),
    );
    check_eq!(
        spool.posting_term_page(None, &mut denied).err(),
        Some(IndexError::ResourceLimited)
    );
    check_eq!(denied.first_denial(), Some(WorkBudgetDenial::Writes));
    check_eq!(
        denied.usage(),
        WorkUsage {
            bytes: 575,
            ..WorkUsage::default()
        }
    );
    check_eq!(spool.db.change_count()?, epoch);
    check_eq!(work.usage(), producer_usage);
    check!(denied.request().same_request_as(&original));
    check_eq!(denied.request().deadline(), original.deadline());
    check_eq!(denied.request().diagnostic_context().is_some(), record);
    if let Some(diagnostic) = denied.request().diagnostic_context() {
        let probe = diagnostic
            .enter(
                DiagnosticStageV1::NativeTextPostingEdit,
                DiagnosticLaneV1(39),
            )
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        probe
            .finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    }
    let denied_usage = denied.usage();
    check_fixed_rows(&spool, &mut work)?;
    let final_usage = work.usage();
    drop(spool);
    if let Some(operation) = operation {
        operation
            .finish(DiagnosticTerminalV1::Ok)
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    }
    let events = sink
        .0
        .lock()
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?
        .clone();
    Ok((producer_usage, denied_usage, final_usage, events))
}

fn fixed_rows() -> [(Vec<u8>, MapEdit); 5] {
    [
        edit(b"\0a", 3, None, Some(object(10))),
        edit(b"\0b", 1, None, Some(object(11))),
        edit(b"\0b", 2, None, Some(object(10))),
        edit(b"\x01a", 1, None, Some(object(11))),
        edit(b"\x01a", 3, None, Some(object(10))),
    ]
}

fn check_fixed_rows(spool: &TextSpool, work: &mut WorkBudget) -> Result<(), IndexError> {
    let terms = spool.posting_term_page(None, work)?;
    check_eq!(terms.len(), 3);
    for (term, expected) in terms.iter().zip([b"\0a", b"\0b", b"\x01a"]) {
        check_eq!(term.as_slice(), expected.as_slice());
    }
    let mut cursor = spool.posting_cursor(terms);
    for ((_, expected), root) in fixed_rows().into_iter().zip([0, 1, 1, 2, 2]) {
        let actual = cursor.next_edit(work)?.ok_or(IndexError::Conflict)?;
        check_eq!(actual.root(), root);
        check_eq!(actual.key(), expected.key);
        check_eq!(actual.expected()?, expected.expected);
        check_eq!(actual.next()?, expected.next);
    }
    check!(cursor.next_edit(work)?.is_none());
    Ok(())
}

#[test]
fn ordered_initial_posting_packet_avoids_sort_and_row_recopy() -> Result<(), IndexError> {
    let mut observed = Vec::new();
    for indices in [[0, 1, 2, 3, 4], [3, 2, 4, 0, 1]] {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        let rows = fixed_rows();
        for index in indices {
            spool.posting_batch(&rows[index..=index], &mut work)?;
        }
        check_fixed_rows(&spool, &mut work)?;
        observed.push(work.usage());
    }
    // Five two-byte terms with absent priors and object values. Sorting
    // previously visited 73 expanded bytes per row, then copied each
    // ten-byte compact row into a second packet. Logical edit admission,
    // initial copies, dictionaries, SQL writes and readback stay identical.
    check_eq!(observed[0].reads, observed[1].reads);
    check_eq!(observed[0].writes, observed[1].writes);
    check_eq!(observed[0].mutations, observed[1].mutations);
    check_eq!(observed[1].bytes - observed[0].bytes, 5 * (73 + 10));
    Ok(())
}

#[test]
fn unordered_packet_observation_separates_sort_and_real_row_copy() -> Result<(), IndexError> {
    for (indices, unordered) in [([0, 1, 2, 3, 4], false), ([3, 2, 4, 0, 1], true)] {
        let (off_usage, off_events) = run_packet_trace(indices, false)?;
        let (recorded_usage, recorded_events) = run_packet_trace(indices, true)?;
        check_eq!(off_usage, recorded_usage);
        check!(off_events.is_empty());
        check_packet_copy_trace(&recorded_events, unordered)?;
    }
    Ok(())
}

#[test]
fn unordered_packet_write_refusal_records_error_and_retries_same_rows() -> Result<(), IndexError> {
    let (off_producer, off_denied, off_final, off_events) = run_packet_refusal_trace(false)?;
    let (record_producer, record_denied, record_final, record_events) =
        run_packet_refusal_trace(true)?;
    check_eq!(off_producer, record_producer);
    check_eq!(off_denied, record_denied);
    check_eq!(off_final, record_final);
    check!(off_events.is_empty());
    check_packet_refusal_trace(&record_events)
}

#[test]
fn initial_posting_packet_write_denial_retries_physical_rows_after_offset_sort()
-> Result<(), IndexError> {
    for indices in [[0, 1, 2, 3, 4], [3, 2, 4, 0, 1]] {
        let mut work = budget()?;
        let spool = TextSpool::new(&TestScratch, &mut work)?;
        let rows = fixed_rows();
        for index in indices {
            spool.posting_batch(&rows[index..=index], &mut work)?;
        }
        let epoch = spool.db.change_count()?;
        let mut denied = WorkBudget::new(
            WorkLimits {
                reads: 1000,
                writes: 0,
                bytes: 10_000_000,
                mutations: 1000,
            },
            work.request().deadline(),
        );
        check!(matches!(
            spool.posting_term_page(None, &mut denied),
            Err(IndexError::ResourceLimited)
        ));
        check_eq!(denied.usage().writes, 0);
        check_eq!(spool.db.change_count()?, epoch);
        check_fixed_rows(&spool, &mut work)?;
    }
    Ok(())
}

/// The second phase-0 run has 600 ordered prefix rows, then two suffix owners
/// sharing 572 terms. Its literal witnesses and back references must survive
/// both the authenticated packet reader and the final cross-run merge.
#[test]
fn cold_back_reference_multi_run_wire_work_and_eof() -> Result<(), IndexError> {
    use sha2::{Digest as _, Sha256};

    let mut off = None;
    for record in [false, true] {
        let sink = Arc::new(PacketSink::default());
        let mut operation = None;
        let request = RequestBudgetV1::for_duration(Duration::from_secs(60));
        let request = if record {
            let trace = DiagnosticContextV1::new(
                DiagnosticCaptureIdV1([74; 16]),
                Arc::new(PacketClock::default()),
                sink.clone(),
            );
            let root = trace
                .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
            let request = request.with_diagnostic_context(trace.child_context(&root));
            operation = Some(root);
            request
        } else {
            request
        };
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 20_000,
                writes: 20_000,
                bytes: 100_000_000,
                mutations: 20_000,
            },
            request,
        );
        let spool = TextSpool::new_cold_postings(&TestScratch, &mut work)?;
        let mut batch = Vec::new();
        for (owner, count) in [(1_u8, 1_772_u16), (2, 600), (3, 600), (4, 572)] {
            for index in 0..count {
                let term = format!("\0t{index:04}");
                batch.push(cold_row(term.as_bytes(), owner, &work)?);
                if batch.len() == quanta_index_scratch_sqlite::MAX_WIDE_WRITE_BATCH_ROWS {
                    spool.posting_batch_rows(&batch, &mut work)?;
                    batch.clear();
                }
            }
        }
        if !batch.is_empty() {
            spool.posting_batch_rows(&batch, &mut work)?;
        }
        let mut range = PackedRangeCursor::new(&spool, b"\0t0000", &mut work)?;
        let db = spool.db.scoped(&work)?;
        let run_count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM posting_packet WHERE phase=0",
                [],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        check_eq!(run_count, 2);
        for (run, magic, size, row_end, dictionaries) in [
            (0_i64, b"QITXPR02", 24_922_usize, 24_858_u32, 1_u16),
            (1, b"QITXPR03", 21_618, 21_426, 3),
        ] {
            let payload: Vec<u8> = db
                .query_row(
                    "SELECT payload FROM posting_packet WHERE phase=0 AND run=?1 AND page=0",
                    [run],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            check_eq!(payload.len(), size);
            check_eq!(&payload[..8], magic);
            check_eq!(&payload[8..10], &1_772_u16.to_be_bytes());
            check_eq!(&payload[42..46], &row_end.to_be_bytes());
            check_eq!(&payload[46..48], &dictionaries.to_be_bytes());
            check_eq!(&payload[48..50], &dictionaries.to_be_bytes());
            let digest = Sha256::digest(&payload[42..]);
            check_eq!(&payload[10..42], digest.as_slice());
        }
        let final_phase = spool
            .postings
            .final_phase()?
            .ok_or(IndexError::ResourceLimited)?;
        check!(final_phase > 0);
        let final_first: Vec<u8> = db
            .query_row(
                "SELECT payload FROM posting_packet WHERE phase=?1 AND run=0 AND page=0",
                [final_phase],
                |row| row.get(0),
            )
            .map_err(|error| sql(&error))?;
        // The final seekable stream can reuse the existing cold grouped
        // format. Its first term has four owners, so the page must contain
        // back references to the first literal.
        check_eq!(&final_first[..8], b"QITXPR03");
        drop(db);
        let mut middle = PackedRangeCursor::new(&spool, b"\0t0001", &mut work)?;
        for owner in [1_u8, 2, 3, 4] {
            let row = middle.next(&mut work)?.ok_or(IndexError::Conflict)?;
            check_eq!(row.term.as_slice(), b"\0t0001");
            check_eq!(row.owner, [owner; 32]);
        }
        drop(middle);
        for index in 0..1_772_u16 {
            let term = format!("\0t{index:04}");
            for (owner, count) in [(1_u8, 1_772_u16), (2, 600), (3, 600), (4, 572)] {
                if index >= count {
                    continue;
                }
                let row = range.next(&mut work)?.ok_or(IndexError::Conflict)?;
                check_eq!(row.term.as_slice(), term.as_bytes());
                check_eq!(row.owner, [owner; 32]);
                check_eq!(row.expected, None);
                check_eq!(row.next, Some(object(owner + 10)));
            }
        }
        check!(range.next(&mut work)?.is_none());
        let eof_usage = work.usage();
        check!(range.next(&mut work)?.is_none());
        check_eq!(work.usage(), eof_usage);
        drop(range);
        drop(spool);
        if let Some(root) = operation {
            root.finish(DiagnosticTerminalV1::Ok)
                .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
        }
        let events = sink
            .0
            .lock()
            .map_err(|error| IndexError::BackendViolation(error.to_string()))?
            .clone();
        if record {
            let span = events
                .iter()
                .find_map(|event| match event.kind {
                    DiagnosticEventKindV1::SpanBegin {
                        span_id,
                        stage: DiagnosticStageV1::NativeTextPostingEdit,
                        lane: DiagnosticLaneV1(38),
                        ..
                    } => Some(span_id),
                    _ => None,
                })
                .ok_or(IndexError::Conflict)?;
            let mut bytes = None;
            for event in &events {
                if let DiagnosticEventKindV1::Counter {
                    span_id,
                    key: DiagnosticCounterV1::WorkBytes,
                    value,
                } = event.kind
                    && span_id == span
                {
                    check!(bytes.replace(value).is_none());
                }
            }
            // 600 sorted keys, 600 equal prefix comparisons and 599 greater
            // prefix comparisons, the compact body, dictionaries and SQL row.
            check_eq!(bytes, Some(102_206));
            check_eq!(off, Some(work.usage()));
        } else {
            check!(events.is_empty());
            off = Some(work.usage());
        }
        check_eq!(work.first_denial(), None);
    }
    Ok(())
}
