//! Request-scoped observation of the existing paired managed install path.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
    DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1,
    DiagnosticStageV1, DiagnosticTerminalV1, RequestBudgetV1,
    index::{
        BootstrapRef, CollectionId, IngressRef, ObjectNamespaceBinding, OperationKey, ProducerId,
        ReclaimCandidate, ReclaimDrainPage, ReclaimPermit, ReclaimTarget, ReclaimWorkPage,
        RetiringRef, SlotId, StreamId, TenantId, WorkLimits, WorkspaceKey, WorktreeId,
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

#[derive(Debug, Default)]
struct Events(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for Events {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut rows) = self.0.try_lock() else {
            return false;
        };
        if rows.len() == 16 {
            return false;
        }
        rows.push(event);
        true
    }
}

struct TraceAuthority {
    fail_confirmation: bool,
    inputs: usize,
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
                ObjectInstallScope::Attempt(_) => false,
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
            }
        }
    }
    Ok(())
}
