use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn shared_map_node_is_counted_only_for_its_first_reachable_root() -> TestResult {
    use quanta_index_core::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
        DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticRootKindV1, RequestBudgetV1,
    };
    use std::sync::Arc;

    #[derive(Debug, Default)]
    struct StepClock(AtomicU64);
    impl DiagnosticClockV1 for StepClock {
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

    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let sink = RecordingSink::default();
    let mut setup = budget()?;
    let value = value(&store, b"shared map value", &mut setup)?;
    let before = physical_file_count(&directory)?;
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([71; 16]),
        Arc::new(StepClock::default()),
        Arc::clone(&events) as Arc<dyn DiagnosticEventSinkV1>,
    );
    let operation = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 100_000,
            writes: 100_000,
            bytes: 256 * 1024 * 1024,
            mutations: 100_000,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(30))
            .with_diagnostic_context(context.child_context(&operation)),
    );
    // Longer than the scalar-pair key bound, so both equal roots use the
    // scratch-backed two-root installation path measured by lanes 6 and 7.
    let key = vec![b'k'; quanta_index_core::index::ports::MAX_OBJECT_MAP_BATCH_KEY_BYTES + 1];
    let edit = MapEdit::objects(key.clone(), None, Some(value));
    let mut cursor = TestEditCursor(
        [
            OrdinalMapEdit::unscoped(0, edit.clone()),
            OrdinalMapEdit::unscoped(1, edit),
        ]
        .into_iter()
        .collect::<Vec<_>>()
        .into_iter(),
    );
    let roots = store.apply_stream(
        &[MapRoot(None), MapRoot(None)],
        &mut cursor,
        &sink,
        &mut work,
    )?;
    let [first, second] = roots.as_slice() else {
        return Err(assertion_error("expected two map roots".into()));
    };
    check_eq!(first, second);
    check_eq!(store.lookup_object(*first, &key, &mut work)?, Some(value));
    check_eq!(store.lookup_object(*second, &key, &mut work)?, Some(value));
    let node = first.0.ok_or("shared map node missing")?;
    check_eq!(
        store.get(node, &mut work)?.registration.children,
        vec![value]
    );
    check_eq!(physical_file_count(&directory)?, before + 1);
    check_eq!(
        sink.0
            .lock()
            .map_err(|_| std::io::Error::other("sink poisoned"))?
            .get(&node)
            .copied(),
        Some(true)
    );
    let rows = events
        .0
        .lock()
        .map_err(|_| std::io::Error::other("event sink poisoned"))?;
    for (lane, expected) in [(6, 1), (7, 0)] {
        let spans = rows
            .iter()
            .filter_map(|event| match event.kind {
                DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    stage: DiagnosticStageV1::MapApply,
                    lane: DiagnosticLaneV1(observed),
                    ..
                } if observed == lane => Some(span_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        check_eq!(spans.len(), 1);
        let span = spans[0];
        check_eq!(
            rows.iter()
                .filter(
                    |event| matches!(event.kind, DiagnosticEventKindV1::Counter {
                    span_id,
                    key: DiagnosticCounterV1::NewObjects,
                    value,
                } if span_id == span && value == expected)
                )
                .count(),
            1
        );
        check_eq!(
            rows.iter()
                .filter(
                    |event| matches!(event.kind, DiagnosticEventKindV1::SpanEnd {
                    span_id,
                    outcome: DiagnosticTerminalV1::Ok,
                } if span_id == span)
                )
                .count(),
            1
        );
    }
    Ok(())
}

#[test]
fn completed_group_matches_independent_set_with_bounded_work_and_refunds() -> TestResult {
    const MASK: usize = (MAX_INSTALL_BATCH_INPUTS * 2).next_power_of_two() - 1;
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut setup = budget()?;
    let mut objects = (0..MAX_INSTALL_BATCH_INPUTS)
        .map(|id| {
            store.prepare_object(
                &u64::try_from(id)
                    .map_err(|_error| IndexError::ResourceLimited)?
                    .to_be_bytes(),
                &[],
                &mut setup,
            )
        })
        .collect::<Result<Vec<_>, IndexError>>()?;
    let bucket = |reference: ObjectRef| {
        let [first, second, ..] = (reference.0).0;
        usize::from(u16::from_le_bytes([first, second])) & MASK
    };
    let first = objects
        .first()
        .ok_or("empty fixture")?
        .registration
        .reference;
    let mut collisions = Vec::new();
    for id in 10_000_u64..75_536 {
        let candidate = store.prepare_object(&id.to_be_bytes(), &[], &mut setup)?;
        if bucket(candidate.registration.reference) == bucket(first) {
            check!(candidate.registration.reference != first);
            collisions.push(candidate);
            if collisions.len() == 2 {
                break;
            }
        }
    }
    let absent = collisions.pop().ok_or("missing absent collision fixture")?;
    *objects.last_mut().ok_or("empty fixture")? =
        collisions.pop().ok_or("missing collision fixture")?;
    // A full physical group must not require the previous quadratic scan bill.
    let mut work = WorkBudget::new(
        WorkLimits {
            reads: 0,
            writes: 0,
            bytes: 128 * 1024,
            mutations: 0,
        },
        Instant::now()
            .checked_add(Duration::from_secs(10))
            .ok_or(IndexError::ResourceLimited)?,
    );
    let tracker = array_scope(MemoryLifetime::Temporary, Some(0), &mut work);
    let mut group = CompletedGroup::new();
    let mut model = std::collections::BTreeSet::new();
    for (index, object) in objects.iter().enumerate() {
        let reference = object.registration.reference;
        check_eq!(
            group.contains(reference, &objects, &mut work)?,
            model.contains(&reference)
        );
        group.insert(reference, index, &objects, &mut work)?;
        let _inserted = model.insert(reference);
    }
    for object in &objects {
        check_eq!(
            group.contains(object.registration.reference, &objects, &mut work)?,
            model.contains(&object.registration.reference)
        );
    }
    check!(!group.contains(absent.registration.reference, &objects, &mut work)?);
    group.clear(&mut work)?;
    for object in &objects {
        check!(!group.contains(object.registration.reference, &objects, &mut work)?);
    }
    check!(tracker.live.load(Ordering::Relaxed) > 0);
    drop(group);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    let mut denied = budget()?;
    let tracker = array_scope(MemoryLifetime::Temporary, None, &mut denied);
    let first = objects
        .first()
        .ok_or("empty fixture")?
        .registration
        .reference;
    let mut group = CompletedGroup::new();
    check!(matches!(
        group.insert(first, 0, &objects, &mut denied),
        Err(IndexError::ResourceLimited)
    ));
    check!(!group.contains(first, &objects, &mut denied)?);
    drop(group);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}

#[test]
fn resident_install_does_not_allocate_the_deferred_group_index() -> TestResult {
    let directory = TestDir::new();
    let store = FileObjectStore::open(&directory.0)?;
    let mut setup = budget()?;
    let object = value(&store, b"resident value", &mut setup)?;
    let mut work = budget()?;
    let table_bytes = 2usize
        .checked_mul(MAX_INSTALL_BATCH_INPUTS)
        .and_then(|slots| slots.checked_mul(std::mem::size_of::<usize>()))
        .ok_or(IndexError::ResourceLimited)?;
    let tracker = array_scope(
        MemoryLifetime::Temporary,
        Some(u64::try_from(table_bytes)?),
        &mut work,
    );
    let root = store
        .apply(
            MapRoot(None),
            &[MapEdit::objects(b"a".to_vec(), None, Some(object))],
            &RecordingSink::default(),
            &mut work,
        )?
        .root;
    check_eq!(store.lookup_object(root, b"a", &mut work)?, Some(object));
    check_eq!(tracker.refused.load(Ordering::Relaxed), 0);
    check_eq!(tracker.live.load(Ordering::Relaxed), 0);
    Ok(())
}
