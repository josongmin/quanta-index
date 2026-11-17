//! Refusal or cancellation of any packet admission precedes durable effects.

use super::*;
use quanta_index_core::index::{
    MemoryLease, MemoryLeaseOwner, MemoryLifetime, MemoryScope, MemoryScopePort,
};

#[path = "prepared_leaf_run_tests.rs"]
mod prepared_leaf_run_tests;

struct Admission {
    calls: AtomicU64,
    live: Arc<AtomicU64>,
    refuse_at: u64,
    cancel: Option<quanta_index_core::CancelHandleV1>,
}

struct Release(Arc<AtomicU64>);

impl MemoryLeaseOwner for Release {
    fn release(&self, bytes: u64) {
        let _previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
    }
}

impl MemoryScopePort for Admission {
    fn reserve(&self, bytes: u64, _lifetime: MemoryLifetime) -> Result<MemoryLease, IndexError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.refuse_at {
            if let Some(cancel) = &self.cancel {
                cancel.cancel();
            } else {
                return Err(IndexError::ResourceLimited);
            }
        }
        let _previous = self.live.fetch_add(bytes, Ordering::SeqCst);
        Ok(MemoryLease::new(
            bytes,
            Arc::new(Release(Arc::clone(&self.live))),
        ))
    }
}

fn attempt_sink() -> ManagedSink {
    ManagedSink(ObjectInstallScope::Attempt(AttemptRef {
        operation: OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            sequence: 1,
        },
        attempt: AttemptId([7; 16]),
        fence: 1,
    }))
}

fn budget(writes: u64, bytes: u64) -> WorkBudget {
    WorkBudget::with_request(
        WorkLimits {
            reads: 100_000,
            writes,
            bytes,
            mutations: 100_000,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    )
}

struct Fixture {
    store: FileObjectStore,
    probe: Arc<PairProbe>,
    observer: Arc<ObjectIoObservation>,
    _directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Self::configured(PairProbe::default(), false)
    }

    fn configured(
        probe: PairProbe,
        fail_confirmation: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::with_inputs(probe, fail_confirmation, 1)
    }

    fn with_inputs(
        probe: PairProbe,
        fail_confirmation: bool,
        inputs: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let scratch = Arc::new(FileScratchSpace::open_standalone(
            directory.path().join("scratch"),
            1024 * 1024,
        )?);
        let probe = Arc::new(probe);
        let authority: Arc<dyn ManagedObjectAuthority> = Arc::new(TraceAuthority {
            fail_confirmation,
            inputs,
            pair: Some(Arc::clone(&probe)),
        });
        let observer = Arc::new(ObjectIoObservation::default());
        let store = FileObjectStore::open_managed_with_scratch(
            directory.path().join("store"),
            scratch,
            authority,
        )?
        .with_io_observation(Arc::clone(&observer));
        Ok(Self {
            store,
            probe,
            observer,
            _directory: directory,
        })
    }

    fn require_no_effect(
        &self,
        objects: &[&PreparedObject],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let events = self
            .probe
            .events
            .lock()
            .map_err(|_error| "probe poisoned")?;
        let observed = self.observer.snapshot()?;
        if !events.is_empty()
            || observed.reserve_calls != 0
            || observed.confirm_calls != 0
            || observed.write_calls != 0
            || observed.file_sync_calls != 0
            || objects
                .iter()
                .any(|object| self.store.path_for(object.registration.reference).exists())
        {
            return Err("packet admission crossed a durable installation effect".into());
        }
        Ok(())
    }
}

fn observe_refused_child_group(
    record: bool,
) -> Result<(WorkUsage, Vec<DiagnosticEmitV1>), Box<dyn std::error::Error>> {
    let fixture = Fixture::with_inputs(PairProbe::default(), false, 0)?;
    let mut setup = budget(100, 1_000_000);
    let child = fixture
        .store
        .prepare_object(b"protected child", &[], &mut setup)?;
    let unguarded = ObjectRef(Digest([0xe0; 32]));
    let children = [child.registration.reference, unguarded];
    let left = fixture
        .store
        .prepare_object(b"left", &children, &mut setup)?;
    let right = fixture
        .store
        .prepare_object(b"right", &children, &mut setup)?;
    assert!([&child, &left, &right].iter().all(|object| {
        FileObjectStore::gate_index(object.registration.reference)
            != FileObjectStore::gate_index(unguarded)
    }));

    let observed = observe_refused_run(&fixture, &[&[&child], &[&left, &right]], 2, record)?;
    assert!(
        fixture
            .store
            .path_for(child.registration.reference)
            .exists()
    );
    assert!(!fixture.store.path_for(left.registration.reference).exists());
    assert!(
        !fixture
            .store
            .path_for(right.registration.reference)
            .exists()
    );
    Ok(observed)
}

fn observe_refused_run(
    fixture: &Fixture,
    groups: &[&[&PreparedObject]],
    reads: u64,
    record: bool,
) -> Result<(WorkUsage, Vec<DiagnosticEmitV1>), Box<dyn std::error::Error>> {
    let events = Arc::new(Events::with_limit(128));
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([42; 16]),
        Arc::new(StepClock::default()),
        events.clone(),
    );
    let mut request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let root = if record {
        let span = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))?;
        request = request.with_diagnostic_context(context.child_context(&span));
        Some(span)
    } else {
        None
    };
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads,
            writes: 100,
            bytes: 1_000_000,
            mutations: 100,
        },
        request,
    );
    let live = Arc::new(AtomicU64::new(0));
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Admission {
        calls: AtomicU64::new(0),
        live: live.clone(),
        refuse_at: u64::MAX,
        cancel: None,
    }))));
    let result = fixture
        .store
        .install_prepared_run(groups, &attempt_sink(), &mut work);
    assert!(matches!(result, Err(IndexError::ResourceLimited)));
    assert_eq!(work.usage().reads, reads);
    assert_eq!(live.load(Ordering::SeqCst), 0);
    if let Some(root) = root {
        root.finish(DiagnosticTerminalV1::from_result(&result))?;
    }
    let rows = events
        .0
        .lock()
        .map_err(|_| "trace events poisoned")?
        .clone();
    Ok((work.usage(), rows))
}

#[test]
fn managed_run_observes_cross_group_child_overlap_at_the_refused_prefix()
-> Result<(), Box<dyn std::error::Error>> {
    let run = |record| {
        let fixture = Fixture::with_inputs(PairProbe::default(), false, 0)?;
        let mut setup = budget(100, 1_000_000);
        let shared = fixture.store.prepare_object(b"shared", &[], &mut setup)?;
        let fresh = fixture.store.prepare_object(b"fresh", &[], &mut setup)?;
        let first =
            fixture
                .store
                .prepare_object(b"first", &[shared.registration.reference], &mut setup)?;
        let last = fixture.store.prepare_object(
            b"last",
            &[shared.registration.reference, fresh.registration.reference],
            &mut setup,
        )?;
        // The two leaves are installed locally in group one. Only the shared
        // leaf has an external path proof from group two. Group three checks
        // that leaf successfully before running out of reads on the fresh leaf.
        let observed = observe_refused_run(
            &fixture,
            &[&[&shared, &fresh], &[&first], &[&last]],
            3,
            record,
        )?;
        assert!(
            fixture
                .store
                .path_for(first.registration.reference)
                .exists()
        );
        assert!(!fixture.store.path_for(last.registration.reference).exists());
        Ok::<_, Box<dyn std::error::Error>>(observed)
    };
    let (off_work, off_events) = run(false)?;
    let (record_work, events) = run(true)?;
    assert_eq!(record_work, off_work);
    assert!(off_events.is_empty());
    let failed_span = events
        .iter()
        .find_map(|event| match event.kind {
            DiagnosticEventKindV1::Counter {
                span_id,
                key: DiagnosticCounterV1::ManagedObjectCrossGroupExternalChildOverlap,
                value: 1,
            } => Some(span_id),
            _ => None,
        })
        .ok_or("refused group did not report its prior-group child overlap")?;
    for key in [
        DiagnosticCounterV1::ManagedObjectCrossGroupRepeatedChildPathChecks,
        DiagnosticCounterV1::ManagedObjectChildPathChecks,
    ] {
        assert!(events.iter().any(|event| matches!(event.kind,
            DiagnosticEventKindV1::Counter { span_id, key: observed, value: 1 }
            if span_id == failed_span && observed == key)));
    }
    let refused = DiagnosticTerminalV1::from_result::<(), _>(&Err(IndexError::ResourceLimited));
    assert!(events.iter().any(|event| matches!(event.kind,
        DiagnosticEventKindV1::SpanEnd { span_id, outcome, .. }
        if span_id == failed_span && outcome == refused)));
    Ok(())
}

#[test]
fn managed_run_reuses_a_guarded_child_check_only_in_one_physical_group()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::with_inputs(PairProbe::default(), false, 0)?;
    let mut setup = budget(100, 1_000_000);
    let child = fixture
        .store
        .prepare_object(b"guarded child", &[], &mut setup)?;
    let children = [child.registration.reference];
    let left = fixture
        .store
        .prepare_object(b"left", &children, &mut setup)?;
    let right = fixture
        .store
        .prepare_object(b"right", &children, &mut setup)?;
    let later = fixture
        .store
        .prepare_object(b"later", &children, &mut setup)?;
    // One Catalog reserve read and one actual child check in each parent
    // group. The later group must obtain a fresh physical proof.
    let mut work = WorkBudget::with_request(
        WorkLimits {
            reads: 3,
            writes: 100,
            bytes: 1_000_000,
            mutations: 100,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    let live = Arc::new(AtomicU64::new(0));
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Admission {
        calls: AtomicU64::new(0),
        live: live.clone(),
        refuse_at: u64::MAX,
        cancel: None,
    }))));
    fixture.store.install_prepared_run(
        &[&[&child], &[&left, &right], &[&later]],
        &attempt_sink(),
        &mut work,
    )?;
    assert_eq!(work.usage().reads, 3);
    assert_eq!(live.load(Ordering::SeqCst), 0);
    for object in [&left, &right, &later] {
        let installed = fixture
            .store
            .get(object.registration.reference, &mut setup)?;
        assert_eq!(
            installed.registration.children,
            object.registration.children
        );
    }
    Ok(())
}

#[test]
fn managed_run_checks_a_different_child_even_when_its_gate_and_slot_collide()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = Fixture::with_inputs(PairProbe::default(), false, 0)?;
    let mut setup = budget(100, 1_000_000);
    let child = fixture
        .store
        .prepare_object(b"guarded child", &[], &mut setup)?;
    let reference = child.registration.reference;
    let mut missing_digest = (reference.0).0;
    missing_digest[31] ^= 1;
    let missing = ObjectRef(Digest(missing_digest));
    let left = fixture
        .store
        .prepare_object(b"left", &[reference], &mut setup)?;
    let right = fixture
        .store
        .prepare_object(b"right", &[missing], &mut setup)?;
    let mut work = budget(100, 1_000_000);
    let live = Arc::new(AtomicU64::new(0));
    work.bind_memory_scope(Some(MemoryScope::new(Arc::new(Admission {
        calls: AtomicU64::new(0),
        live: live.clone(),
        refuse_at: u64::MAX,
        cancel: None,
    }))));
    let result = fixture.store.install_prepared_run(
        &[&[&child], &[&left, &right]],
        &attempt_sink(),
        &mut work,
    );
    assert_eq!(
        result,
        Err(IndexError::InvalidInput(
            "object references a missing child".into()
        ))
    );
    assert_eq!(work.usage().reads, 3);
    assert_eq!(live.load(Ordering::SeqCst), 0);
    assert!(
        !fixture
            .store
            .path_for(right.registration.reference)
            .exists()
    );
    Ok(())
}

#[test]
fn managed_run_observes_exact_external_child_shape_before_a_read_refusal()
-> Result<(), Box<dyn std::error::Error>> {
    let (off_work, off_events) = observe_refused_child_group(false)?;
    let (recorded_work, recorded_events) = observe_refused_child_group(true)?;
    assert_eq!(recorded_work, off_work);
    assert!(off_events.is_empty());
    let mut observed = Vec::new();
    for event in &recorded_events {
        if let DiagnosticEventKindV1::Counter {
            span_id,
            key,
            value,
        } = event.kind
            && matches!(
                key,
                DiagnosticCounterV1::ManagedObjectExternalChildReferences
                    | DiagnosticCounterV1::ManagedObjectUniqueExternalChildren
                    | DiagnosticCounterV1::ManagedObjectGuardedRepeatedExternalChildren
            )
        {
            observed.push((span_id, key, value));
        }
    }
    let [
        (span_id, total_key, total),
        (unique_span, unique_key, unique),
        (guarded_span, guarded_key, guarded),
    ] = observed.as_slice()
    else {
        return Err("expected one complete planned child shape".into());
    };
    assert_eq!((*span_id, *span_id), (*unique_span, *guarded_span));
    assert_eq!(
        (*total_key, *total),
        (DiagnosticCounterV1::ManagedObjectExternalChildReferences, 4)
    );
    assert_eq!(
        (*unique_key, *unique),
        (DiagnosticCounterV1::ManagedObjectUniqueExternalChildren, 2)
    );
    assert_eq!(
        (*guarded_key, *guarded),
        (
            DiagnosticCounterV1::ManagedObjectGuardedRepeatedExternalChildren,
            1
        )
    );
    assert!(recorded_events.iter().any(|event| {
        matches!(event.kind, DiagnosticEventKindV1::SpanEnd { span_id: observed, outcome, .. }
            if observed == *span_id && outcome == DiagnosticTerminalV1::from_result::<(), _>(&Err(IndexError::ResourceLimited)))
    }));
    Ok(())
}

#[test]
fn later_packet_allocation_refusal_or_cancellation_has_no_install_effect()
-> Result<(), Box<dyn std::error::Error>> {
    for cancel in [false, true] {
        // Inject through every metadata/path allocation of a two-packet run.
        // None may observe a preceding reserve, write, sync or confirmation.
        for refuse_at in 1..=4 {
            let fixture = Fixture::new()?;
            let mut preparation = budget(100_000, 32 * 1024 * 1024);
            let first = fixture
                .store
                .prepare_object(b"first", &[], &mut preparation)?;
            let second = fixture
                .store
                .prepare_object(b"second", &[], &mut preparation)?;
            let mut execution = budget(100_000, 32 * 1024 * 1024);
            let live = Arc::new(AtomicU64::new(0));
            let admission = Arc::new(Admission {
                calls: AtomicU64::new(0),
                live: Arc::clone(&live),
                refuse_at,
                cancel: cancel.then(|| execution.request().cancel_handle()),
            });
            execution.bind_memory_scope(Some(MemoryScope::new(admission.clone())));
            let result = fixture.store.install_prepared_run(
                &[&[&first], &[&second]],
                &attempt_sink(),
                &mut execution,
            );
            let expected = if cancel {
                IndexError::Cancelled
            } else {
                IndexError::ResourceLimited
            };
            if result != Err(expected)
                || admission.calls.load(Ordering::SeqCst) != refuse_at
                || live.load(Ordering::SeqCst) != 0
            {
                return Err("run changed admission refusal or retained a temporary grant".into());
            }
            fixture.require_no_effect(&[&first, &second])?;
        }
    }
    Ok(())
}

#[test]
fn funded_run_keeps_synced_files_and_releases_grants_on_confirmation_failure()
-> Result<(), Box<dyn std::error::Error>> {
    for fail_confirmation in [false, true] {
        let fixture = Fixture::configured(PairProbe::default(), fail_confirmation)?;
        let mut preparation = budget(100_000, 32 * 1024 * 1024);
        let first = fixture
            .store
            .prepare_object(b"first", &[], &mut preparation)?;
        let second = fixture
            .store
            .prepare_object(b"second", &[], &mut preparation)?;
        let mut execution = budget(100_000, 32 * 1024 * 1024);
        let live = Arc::new(AtomicU64::new(0));
        execution.bind_memory_scope(Some(MemoryScope::new(Arc::new(Admission {
            calls: AtomicU64::new(0),
            live: Arc::clone(&live),
            refuse_at: u64::MAX,
            cancel: None,
        }))));
        let result = fixture.store.install_prepared_run(
            &[&[&first], &[&second]],
            &attempt_sink(),
            &mut execution,
        );
        let expected = if fail_confirmation {
            Err(IndexError::BackendViolation(
                "injected managed confirmation failure".into(),
            ))
        } else {
            Ok(())
        };
        let events = fixture
            .probe
            .events
            .lock()
            .map_err(|_error| "probe poisoned")?;
        let observed = fixture.observer.snapshot()?;
        if result != expected
            || events.as_slice() != ["reserve", "advance", "confirm"]
            || live.load(Ordering::SeqCst) != 0
            || observed.write_calls != 2
            || observed.file_sync_calls != 2
            || observed.failed_confirms != u64::from(fail_confirmation)
            || !fixture
                .store
                .path_for(first.registration.reference)
                .exists()
            || !fixture
                .store
                .path_for(second.registration.reference)
                .exists()
        {
            return Err(
                "confirmation result lost synced object custody or temporary admission".into(),
            );
        }
    }
    Ok(())
}

#[test]
fn cancellation_after_first_sync_leaves_successor_unwritten_and_releases_grants()
-> Result<(), Box<dyn std::error::Error>> {
    let mut execution = budget(100_000, 32 * 1024 * 1024);
    let fixture = Fixture::configured(
        PairProbe {
            cancel_on_advance: Some(execution.request().cancel_handle()),
            ..PairProbe::default()
        },
        false,
    )?;
    let mut preparation = budget(100_000, 32 * 1024 * 1024);
    let first = fixture
        .store
        .prepare_object(b"first", &[], &mut preparation)?;
    let second = fixture
        .store
        .prepare_object(b"second", &[], &mut preparation)?;
    let live = Arc::new(AtomicU64::new(0));
    execution.bind_memory_scope(Some(MemoryScope::new(Arc::new(Admission {
        calls: AtomicU64::new(0),
        live: Arc::clone(&live),
        refuse_at: u64::MAX,
        cancel: None,
    }))));
    let result = fixture.store.install_prepared_run(
        &[&[&first], &[&second]],
        &attempt_sink(),
        &mut execution,
    );
    let events = fixture
        .probe
        .events
        .lock()
        .map_err(|_error| "probe poisoned")?;
    let observed = fixture.observer.snapshot()?;
    if result != Err(IndexError::Cancelled)
        || events.as_slice() != ["reserve"]
        || live.load(Ordering::SeqCst) != 0
        || observed.write_calls != 1
        || observed.file_sync_calls != 1
        || !fixture
            .store
            .path_for(first.registration.reference)
            .exists()
        || fixture
            .store
            .path_for(second.registration.reference)
            .exists()
    {
        return Err("cancellation crossed successor installation or lost the synced prefix".into());
    }
    Ok(())
}
