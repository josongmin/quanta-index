//! Independent SQL effects and admission checks for normalized posting values.

use super::*;
use std::cell::Cell;

use quanta_index_core::index::{
    MapEdit, MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
};
use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticCounterV1,
    DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticHealthCodeV1,
    DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticStageV1, DiagnosticTerminalV1,
    RequestBudgetV1,
};
use rusqlite::StatementStatus;
use std::sync::Mutex;

#[derive(Debug, Default)]
struct PhaseClock {
    wall: AtomicU64,
    cpu: AtomicU64,
}

impl DiagnosticClockV1 for PhaseClock {
    fn now_ns(&self) -> u64 {
        self.wall.fetch_add(1, Ordering::Relaxed)
    }

    fn thread_cpu_now_ns(&self) -> Option<u64> {
        Some(self.cpu.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Default)]
struct PhaseEvents(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for PhaseEvents {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut events) = self.0.lock() else {
            return false;
        };
        events.push(event);
        true
    }
}

type PhaseCapture = (
    DiagnosticContextV1,
    Arc<PhaseClock>,
    Arc<PhaseEvents>,
    quanta_index_core::DiagnosticSpanGuardV1,
);

fn phase_capture(id: u8) -> Result<PhaseCapture, Box<dyn std::error::Error>> {
    let clock = Arc::new(PhaseClock::default());
    let events = Arc::new(PhaseEvents::default());
    let root_context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([id; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = root_context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    Ok((root_context.child_context(&root), clock, events, root))
}

fn phase_work(context: DiagnosticContextV1) -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 16 * 1024 * 1024,
            mutations: 100_000,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(10)).with_diagnostic_context(context),
    )
}

fn fixture() -> Result<(TestAllocation, Connection), IndexError> {
    let allocation = TestAllocation::new(256 * 1024, 4 * 1024 * 1024, false)?;
    let db = allocation.open()?;
    configure(&db, &allocation)?;
    db.execute_batch(
        "CREATE TABLE posting(term BLOB NOT NULL,k BLOB NOT NULL,p INTEGER,n INTEGER,
             PRIMARY KEY(term,k)) WITHOUT ROWID;
         CREATE TABLE effects(id INTEGER PRIMARY KEY);",
    )
    .map_err(|error| sql(&error))?;
    Ok((allocation, db))
}

fn rows(count: u16) -> Vec<(Vec<u8>, MapEdit)> {
    (0..count)
        .map(|index| {
            (
                index.to_be_bytes().to_vec(),
                MapEdit {
                    key: vec![17; 32],
                    expected: Some(ObjectRef(Digest([7; 32])).into()),
                    next: Some(ObjectRef(Digest([9; 32])).into()),
                },
            )
        })
        .collect()
}

fn posting_sql_oracle(mask: u8, count: usize) -> Result<String, IndexError> {
    let mut query = String::from("INSERT INTO posting(term,k,p,n) VALUES ");
    let mut shared = [None; 4];
    let mut next = 0_usize;
    for (column, bit) in (1..4).zip([1_u8, 2, 4]) {
        if mask & bit != 0 {
            next = next.checked_add(1).ok_or(IndexError::ResourceLimited)?;
            *shared.get_mut(column).ok_or(IndexError::ResourceLimited)? = Some(next);
        }
    }
    for slot in 0..count {
        if slot != 0 {
            query.push(',');
        }
        if mask == 0 {
            query.push_str("(?,?,?,?)");
            continue;
        }
        query.push('(');
        for column in 0..4 {
            if column != 0 {
                query.push(',');
            }
            let parameter =
                if let Some(parameter) = shared.get(column).ok_or(IndexError::ResourceLimited)? {
                    *parameter
                } else {
                    next = next.checked_add(1).ok_or(IndexError::ResourceLimited)?;
                    next
                };
            query.push('?');
            query.push_str(&parameter.to_string());
        }
        query.push(')');
    }
    Ok(query)
}

#[test]
fn posting_sql_shape_all_masks_and_lengths_keep_wire_and_fee()
-> Result<(), Box<dyn std::error::Error>> {
    for mask in 0_u8..8 {
        for rows_count in [1_usize, 64, 80, 127, 128] {
            let (allocation, db) = fixture()?;
            let mut input = rows(u16::try_from(rows_count)?);
            for (slot, (_term, edit)) in input.iter_mut().enumerate() {
                let ordinal = u8::try_from(slot)?;
                edit.key = vec![if mask & 1 != 0 { 17 } else { ordinal }; 32];
                let prior_byte = if mask & 2 != 0 {
                    1
                } else {
                    ordinal.checked_add(1).ok_or(IndexError::ResourceLimited)?
                };
                let next_byte = if mask & 4 != 0 {
                    128
                } else {
                    ordinal
                        .checked_add(128)
                        .ok_or(IndexError::ResourceLimited)?
                };
                edit.expected = Some(ObjectRef(Digest([prior_byte; 32])).into());
                edit.next = Some(ObjectRef(Digest([next_byte; 32])).into());
            }
            let mut work = work()?;
            let before_execute = Cell::new(false);
            let reservations = [256_u64; 128];
            let written = write_posting_edit_fields_batch(
                &db,
                &allocation,
                reservations
                    .get(..rows_count)
                    .ok_or(IndexError::ResourceLimited)?,
                |slot| {
                    let (term, edit) = input.get(slot).ok_or(IndexError::ResourceLimited)?;
                    Ok((term.as_slice(), edit.into()))
                },
                |value, _budget| {
                    let reference = value.as_object_ref()?;
                    let ordinal = *reference.0.0.first().ok_or(IndexError::ResourceLimited)?;
                    i64::from(ordinal)
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)
                },
                || before_execute.set(true),
                &mut work,
            );
            if rows_count > MAX_WRITE_BATCH_ROWS
                && max_supported_wide_edit_batch_rows(&db)? != MAX_WIDE_WRITE_BATCH_ROWS
            {
                check!(matches!(written, Err(IndexError::ResourceLimited)));
                check!(!before_execute.get());
                check_eq!(count(&db, "posting")?, 0);
                continue;
            }
            written?;
            check!(before_execute.get());
            let actual_mask = if rows_count == 1 { 0 } else { mask };
            let query = posting_sql_oracle(actual_mask, rows_count)?;
            let plain = posting_sql_oracle(0, rows_count)?;
            let row_bytes = u64::try_from(rows_count)?
                .checked_mul(164)
                .ok_or(IndexError::ResourceLimited)?;
            let sql_bytes = u64::try_from(query.len().max(plain.len()))?;
            let expected_bytes = 4096_u64
                .checked_add(row_bytes)
                .and_then(|bytes| bytes.checked_add(sql_bytes))
                .ok_or(IndexError::ResourceLimited)?;
            check_eq!(work.usage().writes, u64::try_from(rows_count)?);
            check_eq!(work.usage().bytes, expected_bytes);
            let statement = db.prepare_cached(&query).map_err(|error| sql(&error))?;
            check_eq!(statement.get_status(StatementStatus::Run), 1);
            drop(statement);
            let mut statement = db.prepare("SELECT term,k,p,n FROM posting ORDER BY term,k")?;
            let actual = statement.query_map([], |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })?;
            for (slot, row) in actual.enumerate() {
                let (term, owner, prior, next) = row?;
                let (expected_term, edit) = input.get(slot).ok_or(IndexError::ResourceLimited)?;
                check_eq!(term.as_slice(), expected_term.as_slice());
                check_eq!(owner.as_slice(), edit.key.as_slice());
                let prior_ref = edit
                    .expected
                    .as_ref()
                    .ok_or(IndexError::ResourceLimited)?
                    .as_object_ref()?;
                let next_ref = edit
                    .next
                    .as_ref()
                    .ok_or(IndexError::ResourceLimited)?
                    .as_object_ref()?;
                let prior_id =
                    i64::from(*prior_ref.0.0.first().ok_or(IndexError::ResourceLimited)?)
                        .checked_add(1)
                        .ok_or(IndexError::ResourceLimited)?;
                let next_id = i64::from(*next_ref.0.0.first().ok_or(IndexError::ResourceLimited)?)
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
                check_eq!(prior, Some(prior_id));
                check_eq!(next, Some(next_id));
            }
            check_eq!(count(&db, "posting")?, i64::try_from(rows_count)?);
        }
    }
    Ok(())
}

#[test]
fn posting_full_phases_record_rows_and_preserve_off_work() -> Result<(), Box<dyn std::error::Error>>
{
    let (context, clock, events, _root) = phase_capture(61)?;
    let rows = rows(2);
    let (allocation, db) = fixture()?;
    let mut observed = phase_work(context.clone());
    let executed = Cell::new(false);
    write_posting_edit_fields_batch(
        &db,
        &allocation,
        &[256; 2],
        |slot| {
            let (term, edit) = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
            Ok((term.as_slice(), edit.into()))
        },
        |value, _budget| {
            let reference = value.as_object_ref()?;
            Ok(i64::from(
                *reference.0.0.first().ok_or(IndexError::ResourceLimited)?,
            ))
        },
        || executed.set(true),
        &mut observed,
    )?;
    check!(executed.get());
    check_eq!(count(&db, "posting")?, 2);
    let recorded = events
        .0
        .lock()
        .map_err(|error| format!("phase event lock poisoned: {error}"))?
        .clone();
    let phases = recorded
        .iter()
        .filter_map(|event| {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                stage: DiagnosticStageV1::NativeTextPostingEdit,
                lane: DiagnosticLaneV1(lane),
                ..
            } = event.kind
                && (36..=39).contains(&lane)
            {
                Some((lane, span_id))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    check_eq!(phases.len(), 4);
    for (expected_lane, (lane, span_id)) in (36..=39).zip(&phases) {
        check_eq!(*lane, expected_lane);
        check!(recorded.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::SpanEnd {
                span_id: ended,
                outcome: DiagnosticTerminalV1::Ok,
            } if ended == *span_id
        )));
        check!(recorded.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::Counter {
                span_id: counted,
                key: DiagnosticCounterV1::ThreadCpuNs,
                ..
            } if counted == *span_id
        )));
    }
    let full_execute = phases.get(3).ok_or(IndexError::ResourceLimited)?.1;
    for (key, value) in [
        (DiagnosticCounterV1::SqliteRows, 2),
        (DiagnosticCounterV1::SqliteBatches, 1),
    ] {
        check!(recorded.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::Counter {
                span_id,
                key: observed_key,
                value: observed_value,
            } if span_id == full_execute && observed_key == key && observed_value == value
        )));
    }
    for (sampled_lane, parent) in [(34, phases.get(1)), (35, phases.get(3))] {
        let parent = parent.ok_or(IndexError::ResourceLimited)?.1;
        check!(recorded.iter().any(|event| matches!(
            event.kind,
            DiagnosticEventKindV1::SpanBegin {
                parent_span_id: Some(parent_id),
                lane: DiagnosticLaneV1(lane),
                ..
            } if lane == sampled_lane && parent_id == parent
        )));
    }

    let (off_allocation, off_db) = fixture()?;
    let mut off = phase_work(context);
    let before_wall = clock.wall.load(Ordering::Relaxed);
    let before_cpu = clock.cpu.load(Ordering::Relaxed);
    {
        let mut masked = off.scoped_diagnostic_override(None);
        write_posting_edit_fields_batch(
            &off_db,
            &off_allocation,
            &[256; 2],
            |slot| {
                let (term, edit) = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
                Ok((term.as_slice(), edit.into()))
            },
            |value, _budget| {
                let reference = value.as_object_ref()?;
                Ok(i64::from(
                    *reference.0.0.first().ok_or(IndexError::ResourceLimited)?,
                ))
            },
            || {},
            &mut masked,
        )?;
    }
    check_eq!(clock.wall.load(Ordering::Relaxed), before_wall);
    check_eq!(clock.cpu.load(Ordering::Relaxed), before_cpu);
    check_eq!(off.usage(), observed.usage());
    check_eq!(count(&off_db, "posting")?, 2);
    Ok(())
}

#[test]
fn posting_span_admission_failure_preserves_write_and_reports_health()
-> Result<(), Box<dyn std::error::Error>> {
    let (context, _clock, events, _root) = phase_capture(63)?;
    let (_foreign_context, _foreign_clock, _foreign_events, foreign_root) = phase_capture(64)?;
    let invalid_parent = context.child_context(&foreign_root);
    events
        .0
        .lock()
        .map_err(|error| format!("phase event lock poisoned: {error}"))?
        .clear();
    let rows = rows(2);
    let (allocation, db) = fixture()?;
    let mut observed = phase_work(invalid_parent);
    let executed = Cell::new(false);
    write_posting_edit_fields_batch(
        &db,
        &allocation,
        &[256; 2],
        |slot| {
            let (term, edit) = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
            Ok((term.as_slice(), edit.into()))
        },
        |value, _budget| {
            let reference = value.as_object_ref()?;
            Ok(i64::from(
                *reference.0.0.first().ok_or(IndexError::ResourceLimited)?,
            ))
        },
        || executed.set(true),
        &mut observed,
    )?;
    check!(executed.get());
    check_eq!(count(&db, "posting")?, 2);
    let recorded = events
        .0
        .lock()
        .map_err(|error| format!("phase event lock poisoned: {error}"))?
        .clone();
    check!(recorded.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::Health {
            code: DiagnosticHealthCodeV1::ForeignCaptureSpan
        }
    )));
    check!(!recorded.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::SpanBegin {
            lane: DiagnosticLaneV1(36..=39),
            ..
        }
    )));

    let (off_allocation, off_db) = fixture()?;
    let mut off = work()?;
    write_posting_edit_fields_batch(
        &off_db,
        &off_allocation,
        &[256; 2],
        |slot| {
            let (term, edit) = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
            Ok((term.as_slice(), edit.into()))
        },
        |value, _budget| {
            let reference = value.as_object_ref()?;
            Ok(i64::from(
                *reference.0.0.first().ok_or(IndexError::ResourceLimited)?,
            ))
        },
        || {},
        &mut off,
    )?;
    check_eq!(off.usage(), observed.usage());
    check_eq!(count(&off_db, "posting")?, 2);
    Ok(())
}

#[test]
fn posting_preexecute_callback_error_closes_binding_phase_without_execute()
-> Result<(), Box<dyn std::error::Error>> {
    let (context, _clock, events, _root) = phase_capture(62)?;
    let (allocation, db) = fixture()?;
    let rows = rows(2);
    let mut work = phase_work(context);
    let calls = Cell::new(0_u8);
    let interned = Cell::new(0_u8);
    let executed = Cell::new(false);
    let result = write_posting_edit_fields_batch(
        &db,
        &allocation,
        &[256; 2],
        |slot| {
            calls.set(
                calls
                    .get()
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            if calls.get() == 4 {
                return Err(IndexError::Storage("binding callback refusal".into()));
            }
            let (term, edit) = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
            Ok((term.as_slice(), edit.into()))
        },
        |_value, _budget| {
            interned.set(
                interned
                    .get()
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?,
            );
            Ok(1)
        },
        || executed.set(true),
        &mut work,
    );
    check!(
        matches!(result, Err(IndexError::Storage(ref message)) if message == "binding callback refusal")
    );
    check_eq!(calls.get(), 4);
    check_eq!(interned.get(), 0);
    check!(!executed.get());
    check_eq!(count(&db, "posting")?, 0);
    let recorded = events
        .0
        .lock()
        .map_err(|error| format!("phase event lock poisoned: {error}"))?
        .clone();
    let binding = recorded.iter().find_map(|event| {
        if let DiagnosticEventKindV1::SpanBegin {
            span_id,
            lane: DiagnosticLaneV1(38),
            ..
        } = event.kind
        {
            Some(span_id)
        } else {
            None
        }
    });
    let binding = binding.ok_or("missing binding phase")?;
    check!(recorded.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::SpanEnd {
            span_id,
            outcome: DiagnosticTerminalV1::Error { .. },
        } if span_id == binding
    )));
    check!(!recorded.iter().any(|event| matches!(
        event.kind,
        DiagnosticEventKindV1::SpanBegin {
            lane: DiagnosticLaneV1(39),
            ..
        }
    )));
    Ok(())
}

fn work() -> Result<WorkBudget, IndexError> {
    Ok(WorkBudget::new(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 16 * 1024 * 1024,
            mutations: 100_000,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    ))
}

fn count(db: &Connection, table: &str) -> Result<i64, IndexError> {
    let query = match table {
        "posting" => "SELECT count(*) FROM posting",
        "effects" => "SELECT count(*) FROM effects",
        _ => return Err(IndexError::InvalidInput("unknown test table".into())),
    };
    db.query_row(query, [], |row| row.get(0))
        .map_err(|error| sql(&error))
}

#[test]
fn normalized_values_preserve_all_rows_and_share_one_id_binding_per_column()
-> Result<(), Box<dyn std::error::Error>> {
    let (allocation, db) = fixture()?;
    let rows = rows(128);
    let mut work = work()?;
    let interned = Cell::new(0_u64);
    write_posting_edit_fields_batch(
        &db,
        &allocation,
        &[256; 128],
        |slot| {
            let (term, edit) = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
            Ok((term.as_slice(), edit.into()))
        },
        |value, _budget| {
            interned.set(interned.get().saturating_add(1));
            if value == &MapValue::from(ObjectRef(Digest([7; 32]))) {
                Ok(7)
            } else if value == &MapValue::from(ObjectRef(Digest([9; 32]))) {
                Ok(9)
            } else {
                Err(IndexError::BackendViolation(
                    "unexpected canonical value".into(),
                ))
            }
        },
        || {},
        &mut work,
    )?;
    check_eq!(interned.get(), 2);
    check_eq!(work.usage().writes, 128);
    let query = format!(
        "INSERT INTO posting(term,k,p,n) VALUES {}",
        (4..132)
            .map(|parameter| format!("(?{parameter},?1,?2,?3)"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let statement = db.prepare_cached(&query).map_err(|error| sql(&error))?;
    check_eq!(statement.get_status(StatementStatus::Run), 1);
    drop(statement);
    let mut statement = db.prepare("SELECT term,k,p,n FROM posting ORDER BY term,k")?;
    let actual: Vec<_> = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    check_eq!(actual.len(), 128);
    for (index, (term, owner, prior, next)) in actual.into_iter().enumerate() {
        check_eq!(term, u16::try_from(index)?.to_be_bytes());
        check_eq!(owner, vec![17; 32]);
        check_eq!(prior, 7);
        check_eq!(next, 9);
    }
    Ok(())
}

#[test]
fn mixed_owner_packet_reuses_two_value_ids_without_cross_call_state()
-> Result<(), Box<dyn std::error::Error>> {
    let (allocation, db) = fixture()?;
    let mut work = work()?;
    for pass in 0_u16..2 {
        let mut input = rows(128);
        for (slot, (term, edit)) in input.iter_mut().enumerate() {
            let ordinal = u16::try_from(slot)?
                .checked_add(pass.checked_mul(128).ok_or(IndexError::ResourceLimited)?)
                .ok_or(IndexError::ResourceLimited)?;
            *term = ordinal.to_be_bytes().to_vec();
            edit.key = vec![if slot < 64 { 17 } else { 19 }; 32];
            edit.next = Some(ObjectRef(Digest([if slot % 2 == 0 { 9 } else { 11 }; 32])).into());
        }
        let interned = Cell::new(0_u64);
        write_posting_edit_fields_batch(
            &db,
            &allocation,
            &[256; 128],
            |slot| {
                let (term, edit) = input.get(slot).ok_or(IndexError::ResourceLimited)?;
                Ok((term.as_slice(), edit.into()))
            },
            |value, _budget| {
                interned.set(interned.get().saturating_add(1));
                for id in [7_u8, 9, 11] {
                    if value == &MapValue::from(ObjectRef(Digest([id; 32]))) {
                        return Ok(i64::from(id));
                    }
                }
                Err(IndexError::BackendViolation(
                    "unexpected canonical value".into(),
                ))
            },
            || {},
            &mut work,
        )?;
        // The shared prior is probed once; the alternating next pair is probed
        // once each. Each new call starts empty despite retaining the SQLite DB.
        check_eq!(interned.get(), 3);
    }
    let mut statement = db.prepare("SELECT term,k,p,n FROM posting ORDER BY term,k")?;
    let actual: Vec<_> = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    check_eq!(actual.len(), 256);
    for (slot, (term, owner, prior, next)) in actual.into_iter().enumerate() {
        check_eq!(term, u16::try_from(slot)?.to_be_bytes());
        check_eq!(owner, vec![if slot % 128 < 64 { 17 } else { 19 }; 32]);
        check_eq!(prior, 7);
        check_eq!(next, if slot % 2 == 0 { 9 } else { 11 });
    }
    Ok(())
}

#[test]
fn late_input_failure_or_cancellation_never_starts_value_interning()
-> Result<(), Box<dyn std::error::Error>> {
    for cancelled in [false, true] {
        let (allocation, db) = fixture()?;
        let rows = rows(8);
        let calls = Cell::new(0_u64);
        let interned = Cell::new(0_u64);
        let executed = Cell::new(false);
        let mut work = work()?;
        let cancel = work.request().cancel_handle();
        let result = write_posting_edit_fields_batch(
            &db,
            &allocation,
            &[256; 8],
            |slot| {
                calls.set(calls.get().saturating_add(1));
                if calls.get() == 13 {
                    if cancelled {
                        cancel.cancel();
                    } else {
                        return Err(IndexError::Storage("preexecute sentinel".into()));
                    }
                }
                let (term, edit) = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
                Ok((term.as_slice(), edit.into()))
            },
            |_value, _budget| {
                interned.set(interned.get().saturating_add(1));
                let _changed = db
                    .execute("INSERT INTO effects DEFAULT VALUES", [])
                    .map_err(|error| sql(&error))?;
                Ok(1)
            },
            || executed.set(true),
            &mut work,
        );
        check!(if cancelled {
            matches!(result, Err(IndexError::Cancelled))
        } else {
            matches!(result, Err(IndexError::Storage(ref message)) if message == "preexecute sentinel")
        });
        check_eq!(interned.get(), 0);
        check!(!executed.get());
        check_eq!(count(&db, "effects")?, 0);
        check_eq!(count(&db, "posting")?, 0);
    }
    Ok(())
}

#[test]
fn intern_refusal_and_invalid_id_preserve_error_and_never_execute_posting()
-> Result<(), Box<dyn std::error::Error>> {
    for id in [None, Some(0), Some(-1)] {
        let (allocation, db) = fixture()?;
        let rows = rows(1);
        let mut work = work()?;
        let executed = Cell::new(false);
        let result = write_posting_edit_fields_batch(
            &db,
            &allocation,
            &[256],
            |slot| {
                let (term, edit) = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
                Ok((term.as_slice(), edit.into()))
            },
            |_value, _budget| {
                id.map_or_else(
                    || Err(IndexError::Storage("dictionary sentinel".into())),
                    Ok,
                )
            },
            || executed.set(true),
            &mut work,
        );
        check!(if id.is_none() {
            matches!(result, Err(IndexError::Storage(ref message)) if message == "dictionary sentinel")
        } else {
            matches!(result, Err(IndexError::BackendViolation(_)))
        });
        check_eq!(count(&db, "posting")?, 0);
        check!(!executed.get());
    }
    Ok(())
}

#[test]
fn null_values_remain_null_and_never_invoke_the_dictionary()
-> Result<(), Box<dyn std::error::Error>> {
    let (allocation, db) = fixture()?;
    let mut rows = rows(2);
    for (_, edit) in &mut rows {
        edit.expected = None;
    }
    rows.get_mut(1).ok_or(IndexError::ResourceLimited)?.1.next = None;
    let mut work = work()?;
    let interned = Cell::new(0_u64);
    write_posting_edit_fields_batch(
        &db,
        &allocation,
        &[256; 2],
        |slot| {
            let (term, edit) = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
            Ok((term.as_slice(), edit.into()))
        },
        |_value, _budget| {
            interned.set(interned.get().saturating_add(1));
            Ok(9)
        },
        || {},
        &mut work,
    )?;
    check_eq!(interned.get(), 1);
    let mut statement = db.prepare("SELECT p,n FROM posting ORDER BY term,k")?;
    let actual: Vec<_> = statement
        .query_map([], |row| {
            Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?))
        })?
        .collect::<Result<_, _>>()?;
    check_eq!(actual, vec![(None, Some(9)), (None, None)]);
    Ok(())
}

struct Refund(Arc<AtomicU64>);
impl MemoryLeaseOwner for Refund {
    fn release(&self, bytes: u64) {
        let _previous = self.0.fetch_add(bytes, Ordering::Relaxed);
    }
}

struct DenySecondLease {
    calls: AtomicU64,
    admitted: AtomicU64,
    refunded: Arc<AtomicU64>,
}
impl MemoryScopePort for DenySecondLease {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        if self.calls.fetch_add(1, Ordering::Relaxed) != 0 {
            return Err(IndexError::ResourceLimited);
        }
        self.admitted.store(bytes, Ordering::Relaxed);
        Ok(MemoryLease::new(
            bytes,
            Arc::new(Refund(Arc::clone(&self.refunded))),
        ))
    }
}

#[test]
fn value_binding_metadata_is_admitted_before_any_intern_or_mutation()
-> Result<(), Box<dyn std::error::Error>> {
    let (allocation, db) = fixture()?;
    let rows = rows(1);
    let mut work = work()?;
    let memory = Arc::new(DenySecondLease {
        calls: AtomicU64::new(0),
        admitted: AtomicU64::new(0),
        refunded: Arc::new(AtomicU64::new(0)),
    });
    work.bind_memory_scope(Some(MemoryScope::new(memory.clone())));
    let interned = Cell::new(0_u64);
    let result = write_posting_edit_fields_batch(
        &db,
        &allocation,
        &[256],
        |slot| {
            let (term, edit) = rows.get(slot).ok_or(IndexError::ResourceLimited)?;
            Ok((term.as_slice(), edit.into()))
        },
        |_value, _budget| {
            interned.set(interned.get().saturating_add(1));
            Ok(1)
        },
        || {},
        &mut work,
    );
    check!(matches!(result, Err(IndexError::ResourceLimited)));
    check_eq!(memory.calls.load(Ordering::Relaxed), 2);
    check_eq!(
        memory.admitted.load(Ordering::Relaxed),
        memory.refunded.load(Ordering::Relaxed)
    );
    check_eq!(interned.get(), 0);
    check_eq!(count(&db, "posting")?, 0);
    Ok(())
}
