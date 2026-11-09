//! Request-scoped observation of the existing paired managed install path.

use std::{
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
    DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1,
    DiagnosticStageV1, DiagnosticTerminalV1, RequestBudgetV1,
    index::{
        AttemptId, AttemptRef, BootstrapRef, CollectionId, IngressRef, ObjectNamespaceBinding,
        OperationKey, ProducerId, ReclaimCandidate, ReclaimDrainPage, ReclaimPermit, ReclaimTarget,
        ReclaimWorkPage, RetiringRef, SlotId, StreamId, TenantId, WorkLimits, WorkspaceKey,
        WorktreeId,
        ports::{CatalogReclaimPort, ManagedObjectInstallPort},
    },
};

use super::*;

#[derive(Debug, Default)]
struct StepClock(AtomicU64);

impl DiagnosticClockV1 for StepClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

#[path = "write_preparation/tests/root_validation.rs"]
mod root_validation;
#[cfg(target_os = "macos")]
#[path = "scalar_sync_tests.rs"]
mod scalar_sync_tests;

#[derive(Debug, Default)]
struct Events(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for Events {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut rows) = self.0.try_lock() else {
            return false;
        };
        if rows.len() == 64 {
            return false;
        }
        rows.push(event);
        true
    }
}

struct TraceAuthority {
    fail_confirmation: bool,
    inputs: usize,
    pair: Option<Arc<PairProbe>>,
}

#[derive(Default)]
struct PairProbe {
    events: Mutex<Vec<&'static str>>,
    fail_advance: bool,
    fail_advance_at: Option<u64>,
    advances: AtomicU64,
    expected_gates: Mutex<Option<(Weak<FileObjectStore>, [bool; 256])>>,
}

impl PairProbe {
    fn record(&self, event: &'static str) -> Result<(), IndexError> {
        if let Some((store, expected)) = self
            .expected_gates
            .lock()
            .map_err(|_error| IndexError::BackendViolation("gate probe poisoned".into()))?
            .as_ref()
        {
            let store = store
                .upgrade()
                .ok_or_else(|| IndexError::BackendViolation("gate probe store dropped".into()))?;
            for (index, held) in expected.iter().enumerate() {
                if store.gates[index].try_lock().is_err() != *held {
                    return Err(IndexError::BackendViolation(
                        "prepared run did not hold exactly its gate union".into(),
                    ));
                }
            }
        }
        self.events
            .lock()
            .map_err(|_error| IndexError::BackendViolation("pair probe poisoned".into()))?
            .push(event);
        Ok(())
    }
}

fn catalog_child(
    budget: &mut WorkBudget,
    lane: DiagnosticLaneV1,
    work: impl FnOnce(&mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let span = budget
        .request()
        .diagnostic_context()
        .map(|context| context.enter(DiagnosticStageV1::SqliteWork, lane))
        .transpose()
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let result = work(budget);
    if let Some(span) = span {
        let _finished = span.finish(DiagnosticTerminalV1::from_result(&result));
    }
    result
}

impl TraceAuthority {
    fn expected(&self, scope: ObjectInstallScope, objects: &[RegisteredObject]) -> bool {
        objects.len() == self.inputs
            && match scope {
                ObjectInstallScope::Bootstrap(bootstrap) => {
                    self.inputs == 1
                        && objects
                            .first()
                            .is_some_and(|object| bootstrap.profile == object.reference)
                }
                ObjectInstallScope::Ingress(stage) => self.inputs == 3 && stage.fence == 1,
                ObjectInstallScope::Attempt(_) => self.inputs == 1 && self.pair.is_some(),
            }
    }
}

impl ManagedObjectInstallPort for TraceAuthority {
    fn reserve_managed(
        &self,
        scope: ObjectInstallScope,
        objects: &[RegisteredObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if !self.expected(scope, objects) {
            return Err(IndexError::BackendViolation(
                "unexpected managed batch".into(),
            ));
        }
        if let Some(probe) = self.pair.as_ref() {
            probe.record("reserve")?;
        }
        catalog_child(budget, DiagnosticLaneV1(4), |budget| {
            budget.charge(WorkUsage {
                reads: 1,
                ..WorkUsage::default()
            })
        })
    }

    fn confirm_managed(
        &self,
        scope: ObjectInstallScope,
        objects: &[RegisteredObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if !self.expected(scope, objects) {
            return Err(IndexError::BackendViolation(
                "unexpected managed batch".into(),
            ));
        }
        if let Some(probe) = self.pair.as_ref() {
            probe.record("confirm")?;
        }
        catalog_child(budget, DiagnosticLaneV1(6), |budget| {
            budget.charge(WorkUsage {
                writes: 1,
                ..WorkUsage::default()
            })?;
            if self.fail_confirmation {
                Err(IndexError::BackendViolation(
                    "injected managed confirmation failure".into(),
                ))
            } else {
                Ok(())
            }
        })
    }

    fn supports_atomic_attempt_advance(&self) -> bool {
        self.pair.is_some()
    }

    fn advance_attempt_managed(
        &self,
        attempt: AttemptRef,
        confirmed: &[RegisteredObject],
        next: &[RegisteredObject],
        budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        if !self.expected(ObjectInstallScope::Attempt(attempt), confirmed)
            || !self.expected(ObjectInstallScope::Attempt(attempt), next)
        {
            return Err(IndexError::BackendViolation(
                "paired attempt registration differs".into(),
            ));
        }
        let probe = self
            .pair
            .as_ref()
            .ok_or_else(|| IndexError::BackendViolation("pair probe absent".into()))?;
        let advance = probe.advances.fetch_add(1, Ordering::Relaxed) + 1;
        if probe.fail_advance || probe.fail_advance_at == Some(advance) {
            probe.record("advance_failed")?;
            return Err(IndexError::BackendViolation(
                "injected atomic advance failure".into(),
            ));
        }
        budget.checkpoint()?;
        probe.record("advance")
    }
}

fn unused_reclaim<T>() -> Result<T, IndexError> {
    Err(IndexError::BackendViolation(
        "managed trace fixture has no reclaim work".into(),
    ))
}

impl CatalogReclaimPort for TraceAuthority {
    fn reclaim_work_page(
        &self,
        _after: Option<ObjectRef>,
        _limit: u32,
        _budget: &mut WorkBudget,
    ) -> Result<ReclaimWorkPage, IndexError> {
        unused_reclaim()
    }

    fn retire_candidate(
        &self,
        _candidate: ReclaimCandidate,
        _budget: &mut WorkBudget,
    ) -> Result<Option<RetiringRef>, IndexError> {
        unused_reclaim()
    }

    fn enter_reclaiming(
        &self,
        _retiring: RetiringRef,
        _target: ReclaimTarget,
        _budget: &mut WorkBudget,
    ) -> Result<ReclaimPermit, IndexError> {
        unused_reclaim()
    }

    fn validate_reclaiming(
        &self,
        _permit: ReclaimPermit,
        _budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        unused_reclaim()
    }

    fn mark_unlinked(
        &self,
        _permit: ReclaimPermit,
        _budget: &mut WorkBudget,
    ) -> Result<(), IndexError> {
        unused_reclaim()
    }

    fn drain_unlinked(
        &self,
        _permit: ReclaimPermit,
        _limit: u32,
        _budget: &mut WorkBudget,
    ) -> Result<ReclaimDrainPage, IndexError> {
        unused_reclaim()
    }
}

impl ManagedObjectAuthority for TraceAuthority {
    fn bind_namespace(&self, _binding: ObjectNamespaceBinding) -> Result<(), IndexError> {
        Ok(())
    }
}

struct ManagedSink(ObjectInstallScope);

impl ObjectWriteSink for ManagedSink {
    fn managed_scope(&self) -> Option<ObjectInstallScope> {
        Some(self.0)
    }

    fn reserve(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
        unused_reclaim()
    }

    fn installed(&self, _object: &RegisteredObject) -> Result<(), IndexError> {
        unused_reclaim()
    }

    fn reserve_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
        unused_reclaim()
    }

    fn installed_batch(&self, _objects: &[RegisteredObject]) -> Result<(), IndexError> {
        unused_reclaim()
    }
}

struct Run {
    result: Result<RegisteredObject, IndexError>,
    usage: WorkUsage,
    clock_calls: u64,
    events: Vec<DiagnosticEmitV1>,
}

fn run_managed(
    record: bool,
    fail_confirmation: bool,
    inputs: usize,
) -> Result<Run, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let scratch = Arc::new(FileScratchSpace::open_standalone(
        directory.path().join("scratch"),
        1024 * 1024,
    )?);
    let authority: Arc<dyn ManagedObjectAuthority> = Arc::new(TraceAuthority {
        fail_confirmation,
        inputs,
        pair: None,
    });
    let store = FileObjectStore::open_managed_with_scratch(
        directory.path().join("store"),
        scratch,
        authority,
    )?;
    let clock = Arc::new(StepClock::default());
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([81; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = if record {
        Some(context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))?)
    } else {
        None
    };
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let request = match root.as_ref() {
        Some(root) => request.with_diagnostic_context(context.child_context(root)),
        None => request,
    };
    let mut budget = WorkBudget::with_request(
        WorkLimits {
            reads: 100,
            writes: 100,
            bytes: 1_000_000,
            mutations: 100,
        },
        request,
    );
    let profile = store
        .identify(b"managed-trace", &[], &mut budget)?
        .reference;
    let workspace = WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    };
    let scope = if inputs == 1 {
        ObjectInstallScope::Bootstrap(BootstrapRef {
            workspace,
            epoch: 1,
            fence: 1,
            profile,
        })
    } else {
        ObjectInstallScope::Ingress(IngressRef {
            operation: OperationKey {
                workspace,
                epoch: 1,
                producer: ProducerId([5; 16]),
                stream: StreamId([6; 16]),
                stream_epoch: 1,
                sequence: 2,
            },
            fence: 1,
        })
    };
    let sink = ManagedSink(scope);
    let result = if inputs == 1 {
        store.install(b"managed-trace", &[], &sink, &mut budget)
    } else {
        let rows =
            [b"managed-trace".as_slice(), b"second", b"third"].map(|payload| ObjectBatchInput {
                payload: payload.to_vec(),
                children: Vec::new(),
            });
        store
            .install_batch(&rows, &sink, &mut budget)
            .and_then(|objects| {
                if objects.len() != 3 {
                    return Err(IndexError::BackendViolation(
                        "managed batch lost output".into(),
                    ));
                }
                objects.into_iter().next().ok_or_else(|| {
                    IndexError::BackendViolation("managed batch returned no objects".into())
                })
            })
    };
    let usage = budget.usage();
    if let Some(root) = root {
        let _finished = root.finish(DiagnosticTerminalV1::from_result(&result));
    }
    let rows = events
        .0
        .lock()
        .map_err(|_error| IndexError::BackendViolation("trace events poisoned".into()))?
        .clone();
    Ok(Run {
        result,
        usage,
        clock_calls: clock.0.load(Ordering::Relaxed),
        events: rows,
    })
}

#[test]
fn managed_reserve_confirm_trace_preserves_off_cost_and_closes_error()
-> Result<(), Box<dyn std::error::Error>> {
    for count in [1, 3] {
        let expected_inputs = u64::try_from(count)?;
        let off = run_managed(false, false, count)?;
        let recorded = run_managed(true, false, count)?;
        let same_reference = matches!(
            (&off.result, &recorded.result),
            (Ok(left), Ok(right)) if left.reference == right.reference
        );
        if !same_reference
            || off.usage != recorded.usage
            || off.clock_calls != 0
            || !off.events.is_empty()
        {
            return Err(IndexError::BackendViolation(
                "managed trace changed Off result or cost".into(),
            )
            .into());
        }
        let off_failed = run_managed(false, true, count)?;
        let failed = run_managed(true, true, count)?;
        let same_failure = matches!(
            (&off_failed.result, &failed.result),
            (
                Err(IndexError::BackendViolation(off_message)),
                Err(IndexError::BackendViolation(recorded_message))
            ) if off_message == "injected managed confirmation failure"
                && off_message == recorded_message
        );
        if !same_failure
            || off_failed.usage != failed.usage
            || off_failed.clock_calls != 0
            || !off_failed.events.is_empty()
        {
            return Err(IndexError::BackendViolation(
                "managed trace changed Off authority failure".into(),
            )
            .into());
        }
        for (run, confirmation) in [
            (&recorded, DiagnosticTerminalV1::Ok),
            (&failed, DiagnosticTerminalV1::Error { code: 1 }),
        ] {
            let mut reserve = None;
            let mut confirm = None;
            let mut reserve_parent = None;
            let mut confirm_parent = None;
            let mut roots = Vec::new();
            for event in &run.events {
                if let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id,
                    stage,
                    ..
                } = event.kind
                {
                    if stage == DiagnosticStageV1::Operation && parent_span_id.is_none() {
                        roots.push(span_id);
                    } else if stage == DiagnosticStageV1::ManagedObjectReserve {
                        reserve = Some(span_id);
                        reserve_parent = parent_span_id;
                    } else if stage == DiagnosticStageV1::ManagedObjectConfirm {
                        confirm = Some(span_id);
                        confirm_parent = parent_span_id;
                    }
                }
            }
            let [root] = roots.as_slice() else {
                return Err(IndexError::BackendViolation(
                    "managed trace has no unique operation root".into(),
                )
                .into());
            };
            let (Some(reserve), Some(confirm)) = (reserve, confirm) else {
                return Err(
                    IndexError::BackendViolation("managed trace omitted a stage".into()).into(),
                );
            };
            if reserve_parent != Some(*root) || confirm_parent != Some(*root) {
                return Err(IndexError::BackendViolation(
                    "managed authority budget did not restore the operation parent".into(),
                )
                .into());
            }
            let ended = |span_id, terminal| {
                run.events.iter().filter(|event| {
            matches!(event.kind, DiagnosticEventKindV1::SpanEnd { span_id: observed, outcome } if observed == span_id && outcome == terminal)
        }).count()
            };
            if ended(reserve, DiagnosticTerminalV1::Ok) != 1 || ended(confirm, confirmation) != 1 {
                return Err(IndexError::BackendViolation(
                    "managed trace left a span open or mislabeled".into(),
                )
                .into());
            }
            let children = run
                .events
                .iter()
                .filter_map(|event| {
                    if let DiagnosticEventKindV1::SpanBegin {
                        span_id,
                        parent_span_id,
                        stage: DiagnosticStageV1::SqliteWork,
                        lane,
                        ..
                    } = event.kind
                    {
                        Some((span_id, parent_span_id, lane))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            if children.len() != 2 {
                return Err(IndexError::BackendViolation(
                    "managed authority omitted or duplicated catalog children".into(),
                )
                .into());
            }
            for (lane, parent, terminal) in [
                (DiagnosticLaneV1(4), reserve, DiagnosticTerminalV1::Ok),
                (DiagnosticLaneV1(6), confirm, confirmation),
            ] {
                let mut matching = children
                    .iter()
                    .filter(|(_, observed_parent, observed_lane)| {
                        *observed_parent == Some(parent) && *observed_lane == lane
                    });
                let (Some(child), None) = (matching.next(), matching.next()) else {
                    return Err(IndexError::BackendViolation(
                        "managed authority catalog child lost parent or terminal".into(),
                    )
                    .into());
                };
                if ended(child.0, terminal) != 1 {
                    return Err(IndexError::BackendViolation(
                        "managed authority catalog child lost parent or terminal".into(),
                    )
                    .into());
                }
            }
            for span_id in [reserve, confirm] {
                let inputs = run
                    .events
                    .iter()
                    .filter(|event| {
                        matches!(event.kind, DiagnosticEventKindV1::Counter {
                    span_id: observed,
                    key: DiagnosticCounterV1::ManagedObjectInputs,
                    value,
                } if observed == span_id && value == expected_inputs)
                    })
                    .count();
                if inputs != 1 {
                    return Err(IndexError::BackendViolation(
                        "managed trace omitted or duplicated reached input count".into(),
                    )
                    .into());
                }
                let terminal_at = run
                    .events
                    .iter()
                    .position(|event| {
                        matches!(event.kind, DiagnosticEventKindV1::SpanEnd { span_id: observed, .. } if observed == span_id)
                    })
                    .ok_or_else(|| {
                        IndexError::BackendViolation("managed stage terminal missing".into())
                    })?;
                for (key, expected) in [
                    (
                        DiagnosticCounterV1::WorkReads,
                        u64::from(span_id == reserve),
                    ),
                    (
                        DiagnosticCounterV1::WorkWrites,
                        u64::from(span_id == confirm),
                    ),
                    (DiagnosticCounterV1::WorkBytes, 0),
                    (DiagnosticCounterV1::WorkMutations, 0),
                ] {
                    let counters = run
                        .events
                        .iter()
                        .enumerate()
                        .filter_map(|(index, event)| {
                            if let DiagnosticEventKindV1::Counter {
                                span_id: observed,
                                key: observed_key,
                                value,
                            } = event.kind
                                && observed == span_id
                                && observed_key == key
                            {
                                Some((index, value))
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>();
                    if counters.as_slice() != [(counters.first().map_or(usize::MAX, |entry| entry.0), expected)]
                        || counters[0].0 >= terminal_at
                    {
                        return Err(IndexError::BackendViolation(
                            "managed work counter missing, duplicated, or after terminal".into(),
                        )
                        .into());
                    }
                }
            }
        }
    }
    Ok(())
}

#[test]
fn managed_attempt_pair_holds_first_install_until_atomic_advance()
-> Result<(), Box<dyn std::error::Error>> {
    for fail_advance in [false, true] {
        let directory = tempfile::tempdir()?;
        let scratch = Arc::new(FileScratchSpace::open_standalone(
            directory.path().join("scratch"),
            1024 * 1024,
        )?);
        let probe = Arc::new(PairProbe {
            fail_advance,
            ..PairProbe::default()
        });
        let authority: Arc<dyn ManagedObjectAuthority> = Arc::new(TraceAuthority {
            fail_confirmation: false,
            inputs: 1,
            pair: Some(Arc::clone(&probe)),
        });
        let observer = Arc::new(ObjectIoObservation::default());
        let store = FileObjectStore::open_managed_with_scratch(
            directory.path().join("store"),
            scratch,
            authority,
        )?
        .with_io_observation(Arc::clone(&observer));
        let clock = Arc::new(StepClock::default());
        let events = Arc::new(Events::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([82; 16]),
            clock,
            Arc::clone(&events) as Arc<dyn DiagnosticEventSinkV1>,
        );
        let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))?;
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 10_000,
                writes: 10_000,
                bytes: 10_000_000,
                mutations: 10_000,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30))
                .with_diagnostic_context(context.child_context(&root)),
        );
        let first = store.prepare_object(b"first pair child", &[], &mut budget)?;
        let next = store.prepare_object(
            b"second pair parent",
            &[first.registration.reference],
            &mut budget,
        )?;
        let attempt = AttemptRef {
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
        };
        let sink = ManagedSink(ObjectInstallScope::Attempt(attempt));
        if !store.can_run_prepared_installs(&sink) {
            return Err("atomic managed pair was not selected".into());
        }
        let result = store.install_prepared_run(&[&[&first], &[&next]], &sink, &mut budget);
        let expected_log = if fail_advance {
            &["reserve", "advance_failed"][..]
        } else {
            &["reserve", "advance", "confirm"][..]
        };
        let log = probe
            .events
            .lock()
            .map_err(|_error| "pair probe poisoned")?;
        if log.as_slice() != expected_log
            || !store.path_for(first.registration.reference).exists()
            || store.path_for(next.registration.reference).exists() == fail_advance
            || (result.is_err() != fail_advance)
        {
            return Err("paired install crossed a failed advance or changed its order".into());
        }
        drop(log);
        let expected_packets = if fail_advance { 1 } else { 2 };
        root.finish(DiagnosticTerminalV1::from_result(&result))?;
        let trace = events.0.lock().map_err(|_error| "trace events poisoned")?;
        let packets = trace
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::SpanBegin {
                        stage: DiagnosticStageV1::ManagedObjectInstall,
                        ..
                    }
                )
            })
            .count();
        let advances = trace
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::SpanBegin {
                        stage: DiagnosticStageV1::ManagedObjectAdvance,
                        ..
                    }
                )
            })
            .count();
        if packets != expected_packets || advances != 1 {
            return Err("paired install trace lost a physical or advance boundary".into());
        }
        let writes = observer.snapshot()?.write_calls;
        if writes != u64::try_from(expected_packets)? {
            return Err("paired install wrote an object outside the reached group".into());
        }
    }
    Ok(())
}

#[test]
fn contended_install_gate_union_releases_its_prefix() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let store = FileObjectStore::open(directory.path().join("store"))?;
    let budget = WorkBudget::with_request(
        WorkLimits {
            reads: 0,
            writes: 0,
            bytes: 0,
            mutations: 0,
        },
        RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    let mut needed = [false; 256];
    needed[1] = true;
    needed[3] = true;

    let busy = store.lock_gate(3, &budget)?;
    if !matches!(store.try_lock_install_gate_union(&needed, &budget)?, Err(3))
        || store.try_lock_gate(1)?.is_none()
    {
        return Err("contended install union retained an earlier gate".into());
    }
    drop(busy);

    let guards = store
        .try_lock_install_gate_union(&needed, &budget)?
        .map_err(|_| "available install gates were not acquired")?;
    if guards[1].is_none() || guards[3].is_none() || store.try_lock_gate(2)?.is_none() {
        return Err("install gate union acquired the wrong gates".into());
    }
    Ok(())
}

#[test]
fn managed_attempt_run_holds_union_and_stops_at_failed_advance()
-> Result<(), Box<dyn std::error::Error>> {
    for fail_advance_at in [None, Some(2), Some(3)] {
        let directory = tempfile::tempdir()?;
        let scratch = Arc::new(FileScratchSpace::open_standalone(
            directory.path().join("scratch"),
            1024 * 1024,
        )?);
        let probe = Arc::new(PairProbe {
            fail_advance_at,
            ..PairProbe::default()
        });
        let authority: Arc<dyn ManagedObjectAuthority> = Arc::new(TraceAuthority {
            fail_confirmation: false,
            inputs: 1,
            pair: Some(Arc::clone(&probe)),
        });
        let observer = Arc::new(ObjectIoObservation::default());
        let store = Arc::new(
            FileObjectStore::open_managed_with_scratch(
                directory.path().join("store"),
                scratch,
                authority,
            )?
            .with_io_observation(Arc::clone(&observer)),
        );
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 10_000,
                writes: 10_000,
                bytes: 10_000_000,
                mutations: 10_000,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        let first = store.prepare_object(b"run-1", &[], &mut budget)?;
        let second =
            store.prepare_object(b"run-2", &[first.registration.reference], &mut budget)?;
        let third =
            store.prepare_object(b"run-3", &[second.registration.reference], &mut budget)?;
        let fourth =
            store.prepare_object(b"run-4", &[third.registration.reference], &mut budget)?;
        let objects = [&first, &second, &third, &fourth];
        let groups: [&[&PreparedObject]; 4] = [
            std::slice::from_ref(&objects[0]),
            std::slice::from_ref(&objects[1]),
            std::slice::from_ref(&objects[2]),
            std::slice::from_ref(&objects[3]),
        ];
        let attempt = AttemptRef {
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
        };
        let sink = ManagedSink(ObjectInstallScope::Attempt(attempt));
        if fail_advance_at.is_none() {
            let mut oversized_wire =
                store.prepare_object(b"run aggregate bound", &[], &mut budget)?;
            oversized_wire.registration = RegisteredObject::unscoped(
                oversized_wire.registration.reference,
                MAX_BATCH_BYTES,
                Vec::new(),
            );
            if !matches!(
                store.install_prepared_run(&[groups[0], &[&oversized_wire]], &sink, &mut budget),
                Err(IndexError::ResourceLimited)
            ) {
                return Err("prepared run accepted an oversized aggregate".into());
            }
            let five = [groups[0], groups[1], groups[2], groups[3], groups[0]];
            let empty: &[&PreparedObject] = &[];
            let empty_group = [groups[0], empty];
            let invalid_tail = [&fourth, &fourth];
            let duplicate = [groups[0], groups[1], groups[2], &invalid_tail];
            let oversized = [&first; MAX_INSTALL_BATCH_INPUTS + 1];
            let too_many_inputs = [&oversized[..], groups[1]];
            for invalid in [
                &groups[..1],
                &five[..],
                &empty_group[..],
                &duplicate[..],
                &too_many_inputs[..],
            ] {
                if store
                    .install_prepared_run(invalid, &sink, &mut budget)
                    .is_ok()
                {
                    return Err("prepared run accepted an invalid packet bound".into());
                }
            }
            if !probe
                .events
                .lock()
                .map_err(|_error| "run probe poisoned")?
                .is_empty()
                || observer.snapshot()?.write_calls != 0
            {
                return Err("invalid prepared run crossed the first effect".into());
            }
        }
        let mut expected_gates = [false; 256];
        for object in objects {
            expected_gates[FileObjectStore::gate_index(object.registration.reference)] = true;
        }
        *probe
            .expected_gates
            .lock()
            .map_err(|_error| "gate probe poisoned")? =
            Some((Arc::downgrade(&store), expected_gates));
        let result = store.install_prepared_run(&groups, &sink, &mut budget);
        let reached = usize::try_from(fail_advance_at.unwrap_or(4))?;
        let expected_events: &[&str] = match fail_advance_at {
            None => &["reserve", "advance", "advance", "advance", "confirm"],
            Some(2) => &["reserve", "advance", "advance_failed"],
            Some(3) => &["reserve", "advance", "advance", "advance_failed"],
            _ => return Err("unexpected advance failure fixture".into()),
        };
        let events = probe.events.lock().map_err(|_error| "run probe poisoned")?;
        let sample = observer.snapshot()?;
        let reached_count = u64::try_from(reached)?;
        let failed = u64::from(fail_advance_at.is_some());
        let reserve_attempts = reached_count
            .checked_add(failed)
            .ok_or(IndexError::ResourceLimited)?;
        if events.as_slice() != expected_events
            || result.is_err() != fail_advance_at.is_some()
            || sample.write_calls != reached_count
            || sample.reserve_calls != reserve_attempts
            || sample.reserve_objects != reserve_attempts
            || sample.confirm_calls != reached_count
            || sample.confirm_objects != reached_count
            || sample.failed_reserves != failed
            || sample.failed_confirms != failed
            || objects.iter().enumerate().any(|(index, object)| {
                store.path_for(object.registration.reference).exists() != (index < reached)
            })
        {
            return Err(
                "prepared run crossed an unreachable packet or changed Catalog order".into(),
            );
        }
    }
    Ok(())
}

#[test]
fn managed_batch_physical_trace_is_one_packet_aggregate() -> Result<(), Box<dyn std::error::Error>>
{
    let off = run_managed(false, false, 3)?;
    let recorded = run_managed(true, false, 3)?;
    if !matches!((&off.result, &recorded.result), (Ok(left), Ok(right)) if left == right)
        || off.usage != recorded.usage
        || off.clock_calls != 0
        || !off.events.is_empty()
    {
        return Err(IndexError::BackendViolation(
            "physical packet observation changed Off product result or budget".into(),
        )
        .into());
    }
    let packet = recorded.events.iter().find_map(|event| {
        if let DiagnosticEventKindV1::SpanBegin {
            span_id,
            stage: DiagnosticStageV1::ManagedObjectInstall,
            ..
        } = event.kind
        {
            Some(span_id)
        } else {
            None
        }
    });
    let Some(packet) = packet else {
        return Err(IndexError::BackendViolation("physical packet span is absent".into()).into());
    };
    let expected = [
        quanta_index_core::DiagnosticObjectInstallPhaseV1::ChildValidation,
        quanta_index_core::DiagnosticObjectInstallPhaseV1::NamespaceValidation,
        quanta_index_core::DiagnosticObjectInstallPhaseV1::WriterCreate,
        quanta_index_core::DiagnosticObjectInstallPhaseV1::Write,
        quanta_index_core::DiagnosticObjectInstallPhaseV1::FileSync,
        quanta_index_core::DiagnosticObjectInstallPhaseV1::PublishAndRetire,
        quanta_index_core::DiagnosticObjectInstallPhaseV1::DirectorySync,
        quanta_index_core::DiagnosticObjectInstallPhaseV1::Settlement,
    ];
    for phase in expected {
        let expected_calls = if matches!(
            phase,
            quanta_index_core::DiagnosticObjectInstallPhaseV1::Write
                | quanta_index_core::DiagnosticObjectInstallPhaseV1::FileSync
        ) {
            3
        } else {
            0
        };
        let count = recorded
            .events
            .iter()
            .filter(|event| {
                matches!(event.kind, DiagnosticEventKindV1::ObjectInstallAggregate {
                span_id, phase: observed, calls, elapsed_sum_ns, thread_cpu_ns: None, failures: 0,
            } if span_id == packet
                && observed == phase
                && calls > 0
                && (expected_calls == 0 || calls == expected_calls)
                && elapsed_sum_ns > 0)
            })
            .count();
        if count != 1 {
            return Err(IndexError::BackendViolation(
                "physical packet omitted or duplicated one reached phase".into(),
            )
            .into());
        }
    }
    #[cfg(target_os = "macos")]
    if recorded
        .events
        .iter()
        .filter(|event| {
            matches!(event.kind, DiagnosticEventKindV1::ObjectInstallAggregate {
            span_id, phase: quanta_index_core::DiagnosticObjectInstallPhaseV1::Barrier,
            calls: 1, failures: 0, ..
        } if span_id == packet)
        })
        .count()
        != 1
    {
        return Err(
            IndexError::BackendViolation("physical packet barrier is absent".into()).into(),
        );
    }
    if recorded
        .events
        .iter()
        .filter(|event| {
            matches!(event.kind, DiagnosticEventKindV1::SpanEnd {
            span_id, outcome: DiagnosticTerminalV1::Ok,
        } if span_id == packet)
        })
        .count()
        != 1
    {
        return Err(IndexError::BackendViolation("physical packet was not closed".into()).into());
    }
    Ok(())
}
