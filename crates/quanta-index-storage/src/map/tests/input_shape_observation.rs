use super::*;

use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
    DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticRootKindV1, RequestBudgetV1,
};
use quanta_index_core::index::WorkBudgetDenial;
use std::sync::Arc;

type TestResult = Result<(), Box<dyn std::error::Error>>;

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
        let Ok(mut events) = self.0.try_lock() else {
            return false;
        };
        events.push(event);
        true
    }
}

#[derive(Clone, Copy)]
enum Tail {
    Complete,
    Error,
    WorkDenial,
}

#[derive(Clone, Copy)]
enum Layout {
    OneRoot,
    TwoRootBoundary,
    ResidentPair,
    NonEmptyBase,
}

struct Input {
    rows: std::vec::IntoIter<OrdinalMapEdit>,
    yielded: usize,
    tail: Tail,
}

impl MapEditCursor for Input {
    fn next_edit(&mut self, budget: &mut WorkBudget) -> Result<Option<OrdinalMapEdit>, IndexError> {
        if self.yielded == 2 {
            match self.tail {
                Tail::Complete => {}
                Tail::Error => {
                    return Err(IndexError::InvalidInput("injected late input error".into()));
                }
                Tail::WorkDenial => {
                    budget.charge(WorkUsage {
                        bytes: 256 * 1024 * 1024,
                        ..WorkUsage::default()
                    })?;
                    return Err(IndexError::BackendViolation(
                        "unrefused test Work charge".into(),
                    ));
                }
            }
        }
        let next = self.rows.next();
        if next.is_some() {
            self.yielded += 1;
        }
        Ok(next)
    }
}

struct Run {
    result: Result<Vec<MapRoot>, IndexError>,
    usage: WorkUsage,
    denial: Option<WorkBudgetDenial>,
    events: Vec<DiagnosticEmitV1>,
    lookups: Option<[Option<MapValue>; 4]>,
    first: ObjectRef,
    second: ObjectRef,
}

fn run(record: bool, tail: Tail, layout: Layout) -> Result<Run, Box<dyn std::error::Error>> {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let sink = RecordingSink::default();
    let mut setup = budget()?;
    let first = value(&store, b"shape-first", &mut setup)?;
    let second = value(&store, b"shape-second", &mut setup)?;
    let old = matches!(layout, Layout::NonEmptyBase)
        .then(|| value(&store, b"shape-old", &mut setup))
        .transpose()?;
    let base = if matches!(layout, Layout::NonEmptyBase) {
        store
            .apply(
                MapRoot(None),
                &[MapEdit::objects(b"a".to_vec(), None, old)],
                &sink,
                &mut setup,
            )?
            .root
    } else {
        MapRoot(None)
    };
    let empty = MapValue::inline(b"", [])?;
    let aliased = MapValue::inline(b"xy", [first, second, first, second, first])?;
    let rows = [
        MapEdit {
            key: b"a".to_vec(),
            expected: old.map(Into::into),
            next: None,
        },
        MapEdit::objects(b"b".to_vec(), None, Some(first)),
        MapEdit {
            key: b"c".to_vec(),
            expected: None,
            next: Some(empty),
        },
        MapEdit {
            key: b"d".to_vec(),
            expected: None,
            next: Some(aliased),
        },
    ]
    .into_iter()
    .enumerate()
    .filter(|(index, _)| !matches!(layout, Layout::ResidentPair) || *index == 1 || *index == 3)
    .map(|(index, edit)| {
        let root = match layout {
            Layout::OneRoot => 0,
            Layout::NonEmptyBase => 0,
            Layout::TwoRootBoundary => u32::from(index >= 2),
            Layout::ResidentPair => u32::from(index >= 2),
        };
        OrdinalMapEdit::unscoped(root, edit)
    })
    .collect::<Result<Vec<_>, _>>()?;
    let mut input = Input {
        rows: rows.into_iter(),
        yielded: 0,
        tail,
    };

    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([81; 16]),
        clock as Arc<dyn DiagnosticClockV1>,
        Arc::clone(&events) as Arc<dyn DiagnosticEventSinkV1>,
    );
    let root = record
        .then(|| context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1)))
        .transpose()?;
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let request = match root.as_ref() {
        Some(root) => request.with_diagnostic_context(context.child_context(root)),
        None => request,
    };
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 256 * 1024 * 1024,
            mutations: 100_000,
        },
        request,
    );
    let bases: &[MapRoot] = match layout {
        Layout::OneRoot => &[MapRoot(None)],
        Layout::NonEmptyBase => &[base],
        Layout::TwoRootBoundary | Layout::ResidentPair => &[MapRoot(None), MapRoot(None)],
    };
    let result: Result<Vec<MapRoot>, IndexError> = store
        .apply_stream(bases, &mut input, &sink, &mut work)
        .map(|roots| roots.into_iter().collect::<Vec<MapRoot>>());
    let usage = work.usage();
    let denial = work.first_denial();
    let lookups = if matches!(layout, Layout::NonEmptyBase) {
        if let Some(root) = result.as_ref().ok().and_then(|roots| roots.first()).copied() {
            let mut verify = budget()?;
            Some([
                store.lookup(root, b"a", &mut verify)?,
                store.lookup(root, b"b", &mut verify)?,
                store.lookup(root, b"c", &mut verify)?,
                store.lookup(root, b"d", &mut verify)?,
            ])
        } else {
            None
        }
    } else {
        None
    };
    if let Some(root) = root {
        let _finished = root.finish(DiagnosticTerminalV1::from_result(&result));
    }
    let events = events
        .0
        .lock()
        .map_err(|_| assertion_error("shape event sink poisoned".into()))?
        .clone();
    Ok(Run {
        result,
        usage,
        denial,
        events,
        lookups,
        first,
        second,
    })
}

fn map_counter(run: &Run, key: DiagnosticCounterV1) -> Result<u64, Box<dyn std::error::Error>> {
    let spans = run
        .events
        .iter()
        .filter_map(|event| match event.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                stage: DiagnosticStageV1::MapApply,
                lane: DiagnosticLaneV1(0),
                ..
            } => Some(span_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    check_eq!(spans.len(), 1);
    let span = spans[0];
    let counters = run
        .events
        .iter()
        .filter_map(|event| match event.kind {
            DiagnosticEventKindV1::Counter {
                span_id,
                key: observed,
                value,
            } if span_id == span && observed == key => Some(value),
            _ => None,
        })
        .collect::<Vec<_>>();
    check_eq!(counters.len(), 1, "counter {key:?} missing or duplicated");
    let end = run.events.iter().position(|event| {
        matches!(event.kind, DiagnosticEventKindV1::SpanEnd { span_id, .. } if span_id == span)
    });
    check!(end.is_some());
    Ok(counters[0])
}

fn check_shape(run: &Run, expected: [u64; 6]) -> TestResult {
    for (key, expected) in [
        DiagnosticCounterV1::MapInputNextAbsentValues,
        DiagnosticCounterV1::MapInputNextObjectValues,
        DiagnosticCounterV1::MapInputNextInlineValues,
        DiagnosticCounterV1::MapInputNextInlineDataBytes,
        DiagnosticCounterV1::MapInputNextInlineChildRefs,
        DiagnosticCounterV1::MapInputNextWireBytes,
    ]
    .into_iter()
    .zip(expected)
    {
        check_eq!(map_counter(run, key)?, expected, "counter {key:?}");
    }
    Ok(())
}

#[test]
fn empty_stream_reports_fixed_mixed_next_value_shapes_without_changing_work() -> TestResult {
    let off = run(false, Tail::Complete, Layout::OneRoot)?;
    let recorded = run(true, Tail::Complete, Layout::OneRoot)?;
    check_eq!(off.usage, recorded.usage);
    check_eq!(off.denial, recorded.denial);
    check_eq!(off.denial, None);
    check!(off.events.is_empty());
    check_eq!(off.result.as_ref().ok(), recorded.result.as_ref().ok());
    check!(recorded.result.is_ok());
    // None differs from a present zero-data/zero-child Inline value. The
    // five child slots deliberately contain repeated physical references.
    check_shape(&recorded, [1, 1, 2, 2, 5, 201])?;
    check_eq!(map_counter(&recorded, DiagnosticCounterV1::MapRootBuilds)?, 1);
    Ok(())
}

#[test]
fn failed_empty_build_counts_reached_root_and_only_yielded_input_prefix() -> TestResult {
    for (tail, expected_denial) in [
        (Tail::Error, None),
        (Tail::WorkDenial, Some(WorkBudgetDenial::Bytes)),
    ] {
        let off = run(false, tail, Layout::OneRoot)?;
        let recorded = run(true, tail, Layout::OneRoot)?;
        check_eq!(off.usage, recorded.usage);
        check_eq!(off.denial, recorded.denial);
        check_eq!(off.denial, expected_denial);
        check!(off.events.is_empty());
        check_eq!(format!("{:?}", off.result), format!("{:?}", recorded.result));
        check!(recorded.result.is_err());
        check_eq!(map_counter(&recorded, DiagnosticCounterV1::MapRootBuilds)?, 1);
        check_shape(&recorded, [1, 1, 0, 0, 0, 33])?;
    }
    Ok(())
}

#[test]
fn two_root_lookahead_and_scalar_prefetch_count_each_yielded_value_once() -> TestResult {
    let off = run(false, Tail::Complete, Layout::TwoRootBoundary)?;
    let recorded = run(true, Tail::Complete, Layout::TwoRootBoundary)?;
    check_eq!(off.usage, recorded.usage);
    check_eq!(off.denial, recorded.denial);
    check_eq!(off.result.as_ref().ok(), recorded.result.as_ref().ok());
    check_eq!(recorded.result.as_ref().ok().map(Vec::len), Some(2));
    check_shape(&recorded, [1, 1, 2, 2, 5, 201])?;
    check_eq!(map_counter(&recorded, DiagnosticCounterV1::MapRootBuilds)?, 2);
    Ok(())
}

#[test]
fn resident_pair_has_no_scratch_backed_map_apply_shape_packet() -> TestResult {
    let off = run(false, Tail::Complete, Layout::ResidentPair)?;
    let recorded = run(true, Tail::Complete, Layout::ResidentPair)?;
    check_eq!(off.usage, recorded.usage);
    check_eq!(off.denial, recorded.denial);
    check_eq!(off.result.as_ref().ok(), recorded.result.as_ref().ok());
    check_eq!(recorded.result.as_ref().ok().map(Vec::len), Some(2));
    check!(recorded.events.iter().all(|event| {
        !matches!(
            event.kind,
            DiagnosticEventKindV1::SpanBegin {
                stage: DiagnosticStageV1::MapApply,
                lane: DiagnosticLaneV1(0),
                ..
            }
        ) && !matches!(
            event.kind,
            DiagnosticEventKindV1::Counter {
                key: DiagnosticCounterV1::MapInputNextAbsentValues
                    | DiagnosticCounterV1::MapInputNextObjectValues
                    | DiagnosticCounterV1::MapInputNextInlineValues
                    | DiagnosticCounterV1::MapInputNextInlineDataBytes
                    | DiagnosticCounterV1::MapInputNextInlineChildRefs
                    | DiagnosticCounterV1::MapInputNextWireBytes,
                ..
            }
        )
    }));
    Ok(())
}

#[test]
fn shape_counter_overflow_omits_entire_family_and_reports_health() -> TestResult {
    let clock = Arc::new(Clock::default());
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([82; 16]),
        clock as Arc<dyn DiagnosticClockV1>,
        Arc::clone(&events) as Arc<dyn DiagnosticEventSinkV1>,
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30))
        .with_diagnostic_context(context.child_context(&root));
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 4096,
            mutations: 100,
        },
        request,
    );
    let mut observation = MapApplyObservation::new(&work);
    let mut shape = MapInputShape::new(true);
    // Every prior Object contributed exactly 33 canonical wire bytes.
    shape.counts = Some([
        0,
        558_992_244_657_865_200,
        0,
        0,
        0,
        18_446_744_073_709_551_600,
    ]);
    let position = OrdinalMapEdit::unscoped(
        0,
        MapEdit::objects(
            b"a".to_vec(),
            None,
            Some(ObjectRef(quanta_index_core::index::Digest([9; 32]))),
        ),
    )?;
    let mut input = TestEditCursor(vec![position].into_iter());
    let mut cursor = ShapeObservedCursor {
        input: &mut input,
        shape: &mut shape,
        diagnostic: observation.context.clone(),
    };
    check!(cursor.next_edit(&mut work)?.is_some());
    drop(cursor);
    check!(shape.counts.is_none());
    observation.finish(&Ok::<(), IndexError>(()), shape);
    let _finished = root.finish(DiagnosticTerminalV1::Ok);
    let rows = events
        .0
        .lock()
        .map_err(|_| assertion_error("shape event sink poisoned".into()))?;
    check_eq!(
        rows.iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::Health {
                        code: DiagnosticHealthCodeV1::CounterOverflow
                    }
                )
            })
            .count(),
        1
    );
    check_eq!(
        rows.iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::Counter {
                        key: DiagnosticCounterV1::MapInputNextAbsentValues
                            | DiagnosticCounterV1::MapInputNextObjectValues
                            | DiagnosticCounterV1::MapInputNextInlineValues
                            | DiagnosticCounterV1::MapInputNextInlineDataBytes
                            | DiagnosticCounterV1::MapInputNextInlineChildRefs
                            | DiagnosticCounterV1::MapInputNextWireBytes,
                        ..
                    }
                )
            })
            .count(),
        0
    );
    Ok(())
}

#[test]
fn nonempty_base_delta_counts_original_cursor_once_without_scratch_replay() -> TestResult {
    let off = run(false, Tail::Complete, Layout::NonEmptyBase)?;
    let recorded = run(true, Tail::Complete, Layout::NonEmptyBase)?;
    check_eq!(off.usage, recorded.usage);
    check_eq!(off.denial, recorded.denial);
    check_eq!(off.denial, None);
    check_eq!(off.result.as_ref().ok(), recorded.result.as_ref().ok());
    check_eq!(off.lookups, recorded.lookups);
    check_eq!(
        recorded.lookups,
        Some([
            None,
            Some(recorded.first.into()),
            Some(MapValue::inline(b"", [])?),
            Some(MapValue::inline(
                b"xy",
                [
                    recorded.first,
                    recorded.second,
                    recorded.first,
                    recorded.second,
                    recorded.first,
                ],
            )?),
        ])
    );
    check_shape(&recorded, [1, 1, 2, 2, 5, 201])?;
    check_eq!(map_counter(&recorded, DiagnosticCounterV1::MapRootBuilds)?, 1);
    Ok(())
}
