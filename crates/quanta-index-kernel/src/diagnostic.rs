//! Optional spans over actual Native build and publication calls.

use quanta_index_core::{
    DiagnosticContextV1, DiagnosticHealthCodeV1, DiagnosticLaneV1, DiagnosticSpanGuardV1,
    DiagnosticStageV1, DiagnosticTerminalV1, DiagnosticTraceErrorV1,
    index::{AttemptContext, IndexError, OperationStatus, WorkBudget, WorkUsage},
};

/// This lexical observer is dropped before returning, including on unwind.
/// It owns no budget or claim and cannot travel with an asynchronous future.
pub(crate) fn catalog_call<
    C: quanta_index_core::index::ports::CatalogDiagnosticPort + ?Sized,
    T,
>(
    catalog: &C,
    budget: &WorkBudget,
    call: impl FnOnce() -> T,
) -> T {
    let _scope = catalog.enter_diagnostic_scope(budget.request().diagnostic_context());
    call()
}

pub(crate) fn begin(
    budget: &WorkBudget,
    stage: DiagnosticStageV1,
) -> Option<DiagnosticSpanGuardV1> {
    let context = budget.request().diagnostic_context()?;
    // Managed-install COMMIT already uses SqliteWork/lane3. Keep whole
    // Catalog API envelopes distinct so aggregation cannot label them COMMITs.
    let lane = if stage == DiagnosticStageV1::SqliteWork {
        DiagnosticLaneV1(14)
    } else {
        DiagnosticLaneV1(3)
    };
    begin_in_lane(context, stage, lane)
}

/// Borrow the original budget while its diagnostic children belong to `span`.
///
/// The budget's request is restored before the phase span is finished or
/// dropped. Off performs no request clone or diagnostic clock read.
pub(crate) fn phase_budget<'a>(
    budget: &'a mut WorkBudget,
    span: Option<&DiagnosticSpanGuardV1>,
) -> impl std::ops::DerefMut<Target = WorkBudget> + 'a {
    let child = span.and_then(|span| {
        budget
            .request()
            .diagnostic_context()
            .map(|context| context.child_context(span))
    });
    budget.scoped_diagnostic_context(child)
}

/// Private attribution within the synchronous source-owner preparation loop.
/// These lanes do not change the stage, its failure owner or the original work budget.
#[derive(Clone, Copy)]
pub(crate) enum SourceOwnerStep {
    ManifestCursor,
    BaseLookup,
    RecordGet,
    RecordDecodePathEdges,
    BodyGetValidate,
}

impl SourceOwnerStep {
    const fn lane(self) -> DiagnosticLaneV1 {
        DiagnosticLaneV1(match self {
            Self::ManifestCursor => 30,
            Self::BaseLookup => 31,
            Self::RecordGet => 32,
            Self::RecordDecodePathEdges => 33,
            Self::BodyGetValidate => 34,
        })
    }
}

/// Observe exactly one synchronous call, restoring the caller's diagnostic parent
/// before finishing the child span. Off adds no clock read or budget checkpoint.
pub(crate) fn source_owner_step<T>(
    budget: &mut WorkBudget,
    step: SourceOwnerStep,
    call: impl FnOnce(&mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    let Some(context) = budget.request().diagnostic_context() else {
        return call(budget);
    };
    let Some(span) = begin_in_lane(
        context,
        DiagnosticStageV1::NativeSourceOwnerPrepare,
        step.lane(),
    ) else {
        return call(budget);
    };
    let result = {
        let mut scoped = phase_budget(budget, Some(&span));
        call(&mut scoped)
    };
    finish(Some(span), result)
}

/// Distinguish the two owner materialization APIs without changing execution.
///
/// Lanes23/24 identify prepared-result lookup and result staging; lane14
/// remains the envelope for other existing Catalog API calls.
pub(crate) fn begin_sqlite_call(
    budget: &WorkBudget,
    lane: DiagnosticLaneV1,
) -> Option<DiagnosticSpanGuardV1> {
    let context = budget.request().diagnostic_context()?;
    begin_in_lane(context, DiagnosticStageV1::SqliteWork, lane)
}

fn begin_in_lane(
    context: &DiagnosticContextV1,
    stage: DiagnosticStageV1,
    lane: DiagnosticLaneV1,
) -> Option<DiagnosticSpanGuardV1> {
    match context.enter(stage, lane) {
        Ok(span) => Some(span),
        Err(error) => {
            context.health(match error {
                DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                    DiagnosticHealthCodeV1::CounterOverflow
                }
                DiagnosticTraceErrorV1::ClockRegressed => DiagnosticHealthCodeV1::ClockRegressed,
                DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                    DiagnosticHealthCodeV1::ForeignCaptureSpan
                }
                DiagnosticTraceErrorV1::MissingParentSpan => {
                    DiagnosticHealthCodeV1::MissingParentSpan
                }
            });
            None
        }
    }
}

pub(crate) fn finish<T, E>(
    span: Option<DiagnosticSpanGuardV1>,
    result: Result<T, E>,
) -> Result<T, E> {
    if let Some(span) = span {
        let _finished = span.finish(DiagnosticTerminalV1::from_result(&result));
    }
    result
}

/// Keep a bounded failure boundary even when trace capture is Off. This does
/// not time a stage or replace the actual work error with a diagnostic error.
pub(crate) fn finish_work<T>(
    budget: &WorkBudget,
    stage: DiagnosticStageV1,
    span: Option<DiagnosticSpanGuardV1>,
    result: Result<T, IndexError>,
) -> Result<T, IndexError> {
    if result.is_err() {
        budget.record_failed_stage(stage);
    }
    finish(span, result)
}

/// One coarse execution boundary with accepted logical work deltas.
///
/// Counters are neither physical I/O nor failed attempted charges. Off records no
/// clocks or counters. This owns no execution grant and never changes results.
pub(crate) struct WorkStage {
    stage: DiagnosticStageV1,
    span: Option<DiagnosticSpanGuardV1>,
    before: Option<WorkUsage>,
}

impl WorkStage {
    pub(crate) fn begin(budget: &WorkBudget, stage: DiagnosticStageV1) -> Self {
        let span = begin(budget, stage);
        let before = span.as_ref().map(|_span| budget.usage());
        Self {
            stage,
            span,
            before,
        }
    }

    /// Execute synchronous work with the original budget borrowed under this
    /// phase. Restore its request before emitting the phase's terminal event.
    pub(crate) fn run<T>(
        self,
        context: &mut AttemptContext<'_>,
        work: impl FnOnce(&mut AttemptContext<'_>) -> Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        let result = {
            let mut phase_budget = phase_budget(context.budget, self.span.as_ref());
            let mut phase_context = AttemptContext {
                attempt: context.attempt,
                objects: context.objects,
                maps: context.maps,
                custody: context.custody,
                scratch: context.scratch,
                budget: &mut phase_budget,
            };
            work(&mut phase_context)
        };
        self.finish(context.budget, result)
    }

    pub(crate) fn span(&self) -> Option<&DiagnosticSpanGuardV1> {
        self.span.as_ref()
    }

    pub(crate) fn finish<T>(
        self,
        budget: &WorkBudget,
        result: Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        if let (Some(span), Some(before)) = (&self.span, self.before) {
            budget.record_diagnostic_usage_delta(before, span);
        }
        finish_work(budget, self.stage, self.span, result)
    }
}

pub(crate) fn finish_publication(
    span: Option<DiagnosticSpanGuardV1>,
    result: Result<OperationStatus, IndexError>,
) -> Result<OperationStatus, IndexError> {
    if let Some(span) = span {
        let terminal = match &result {
            Ok(OperationStatus::Published(_)) => DiagnosticTerminalV1::Ok,
            Ok(OperationStatus::Pending(_) | OperationStatus::Unknown) => {
                DiagnosticTerminalV1::Aborted
            }
            Ok(OperationStatus::Refused(_)) | Err(_) => DiagnosticTerminalV1::Error { code: 1 },
        };
        let _finished = span.finish(terminal);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    };
    use std::time::{Duration, Instant};

    use quanta_index_core::index::{WorkLimits, WorkUsage};
    use quanta_index_core::{
        DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
        DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1,
        DiagnosticTerminalV1, RequestBudgetV1,
    };

    use super::{DiagnosticSpanGuardV1, DiagnosticStageV1, IndexError, WorkBudget, finish_work};

    #[derive(Debug, Default)]
    struct StepClock(AtomicU64);
    impl DiagnosticClockV1 for StepClock {
        fn now_ns(&self) -> u64 {
            self.0.fetch_add(1, Ordering::Relaxed)
        }
    }

    #[derive(Debug, Default)]
    struct CpuStepClock {
        wall: AtomicU64,
        cpu: AtomicU64,
    }
    impl DiagnosticClockV1 for CpuStepClock {
        fn now_ns(&self) -> u64 {
            self.wall.fetch_add(1, Ordering::Relaxed)
        }

        fn thread_cpu_now_ns(&self) -> Option<u64> {
            Some(self.cpu.fetch_add(7, Ordering::Relaxed))
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

    #[test]
    fn source_owner_step_lanes_are_closed() -> Result<(), Box<dyn std::error::Error>> {
        let observed = [
            super::SourceOwnerStep::ManifestCursor,
            super::SourceOwnerStep::BaseLookup,
            super::SourceOwnerStep::RecordGet,
            super::SourceOwnerStep::RecordDecodePathEdges,
            super::SourceOwnerStep::BodyGetValidate,
        ]
        .map(|step| step.lane().0);
        if observed != [30, 31, 32, 33, 34] {
            return Err("owner substep lanes changed".into());
        }
        Ok(())
    }

    #[test]
    fn source_owner_step_preserves_off_and_record_refusal_and_cpu_parent()
    -> Result<(), Box<dyn std::error::Error>> {
        for record in [false, true] {
            let events = Arc::new(Events::default());
            let clock = Arc::new(CpuStepClock::default());
            let context = DiagnosticContextV1::new(
                DiagnosticCaptureIdV1([96; 16]),
                clock.clone(),
                events.clone(),
            );
            let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
            let request = RequestBudgetV1::for_duration(Duration::from_secs(5));
            let request = if record {
                request.with_diagnostic_context(context.child_context(&root))
            } else {
                request
            };
            let mut work = WorkBudget::with_request(
                WorkLimits {
                    reads: 1,
                    writes: 0,
                    bytes: 0,
                    mutations: 0,
                },
                request,
            );
            let original = work.request().clone();
            let outer = super::begin(&work, DiagnosticStageV1::NativeSourceOwnerPrepare);
            let outer_id = outer.as_ref().map(DiagnosticSpanGuardV1::span_id);
            let before_clock = clock.wall.load(Ordering::Relaxed);
            let mut calls = 0_u8;
            let mut success_calls = 0_u8;
            let primary = {
                let mut scoped = super::phase_budget(&mut work, outer.as_ref());
                let success = super::source_owner_step(
                    &mut scoped,
                    super::SourceOwnerStep::ManifestCursor,
                    |_budget| {
                        success_calls = success_calls.saturating_add(1);
                        Ok(42_u8)
                    },
                );
                if success != Ok(42) {
                    return Err("owner substep changed successful result".into());
                }
                super::source_owner_step(
                    &mut scoped,
                    super::SourceOwnerStep::BaseLookup,
                    |budget| {
                        calls = calls.saturating_add(1);
                        budget.charge(WorkUsage {
                            reads: 1,
                            ..WorkUsage::default()
                        })?;
                        budget.charge(WorkUsage {
                            reads: 1,
                            ..WorkUsage::default()
                        })
                    },
                )
            };
            if calls != 1
                || success_calls != 1
                || primary != Err(IndexError::ResourceLimited)
                || work.usage().reads != 1
                || work.first_denial() != Some(quanta_index_core::index::WorkBudgetDenial::Reads)
                || !work.request().same_request_as(&original)
                || !format!("{work:?}").contains("failed_stage: None")
                || (!record && clock.wall.load(Ordering::Relaxed) != before_clock)
            {
                return Err("owner substep changed original work or refusal".into());
            }
            let result = super::finish_work(
                &work,
                DiagnosticStageV1::NativeSourceOwnerPrepare,
                outer,
                primary,
            );
            if result != Err(IndexError::ResourceLimited)
                || !format!("{work:?}").contains("failed_stage: Some(NativeSourceOwnerPrepare)")
            {
                return Err("owner substep changed enclosing failure stage".into());
            }
            root.finish(DiagnosticTerminalV1::from_result(&result))?;
            let rows = events.0.lock().map_err(|_poison| "owner events poisoned")?;
            let step = rows.iter().find_map(|event| match event.kind {
                DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id,
                    stage: DiagnosticStageV1::NativeSourceOwnerPrepare,
                    lane: DiagnosticLaneV1(31),
                    ..
                } => Some((span_id, parent_span_id)),
                _ => None,
            });
            if record {
                let (step_id, parent) = step.ok_or("owner substep span absent")?;
                if parent != outer_id
                    || rows
                        .iter()
                        .filter(|event| {
                            matches!(
                                event.kind,
                                DiagnosticEventKindV1::SpanBegin {
                                    lane: DiagnosticLaneV1(31),
                                    ..
                                }
                            )
                        })
                        .count()
                        != 1
                    || !rows.iter().any(|event| {
                        matches!(event.kind,
                            DiagnosticEventKindV1::Counter {
                                span_id,
                                key: quanta_index_core::DiagnosticCounterV1::ThreadCpuNs,
                                value: 7,
                            } if span_id == step_id
                        )
                    })
                {
                    return Err("owner substep parent or thread CPU differs".into());
                }
            } else if step.is_some() {
                return Err("Off opened an owner substep span".into());
            }
        }
        Ok(())
    }

    #[test]
    fn source_owner_step_unwind_restores_parent_and_aborts_span()
    -> Result<(), Box<dyn std::error::Error>> {
        let events = Arc::new(Events::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([97; 16]),
            Arc::new(StepClock::default()),
            events.clone(),
        );
        let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 1,
                writes: 0,
                bytes: 0,
                mutations: 0,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(5))
                .with_diagnostic_context(context.child_context(&root)),
        );
        let original = work.request().clone();
        let outer = super::begin(&work, DiagnosticStageV1::NativeSourceOwnerPrepare);
        let outer_id = outer.as_ref().ok_or("owner parent span absent")?.span_id();
        {
            let mut scoped = super::phase_budget(&mut work, outer.as_ref());
            let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _result: Result<(), IndexError> = super::source_owner_step(
                    &mut scoped,
                    super::SourceOwnerStep::RecordGet,
                    |budget| {
                        budget.charge(WorkUsage {
                            reads: 1,
                            ..WorkUsage::default()
                        })?;
                        std::panic::resume_unwind(Box::new("owner substep unwind"));
                    },
                );
            }));
            if panicked.is_ok() || scoped.usage().reads != 1 {
                return Err("owner substep unwind lost work".into());
            }
            let after = super::begin(&scoped, DiagnosticStageV1::ManagedObjectReserve);
            super::finish(after, Ok::<(), IndexError>(()))?;
        }
        if !work.request().same_request_as(&original) {
            return Err("owner substep unwind changed request".into());
        }
        super::finish(outer, Ok::<(), IndexError>(()))?;
        root.finish(DiagnosticTerminalV1::Ok)?;
        let rows = events.0.lock().map_err(|_poison| "owner events poisoned")?;
        let step = rows.iter().find_map(|event| match event.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id,
                lane: DiagnosticLaneV1(32),
                ..
            } => Some((span_id, parent_span_id)),
            _ => None,
        });
        let (step_id, parent) = step.ok_or("unwound owner substep absent")?;
        if parent != Some(outer_id)
            || !rows.iter().any(|event| {
                matches!(event.kind,
                    DiagnosticEventKindV1::SpanEnd {
                        span_id,
                        outcome: DiagnosticTerminalV1::Aborted,
                    } if span_id == step_id
                )
            })
            || !rows.iter().any(|event| {
                matches!(event.kind,
                    DiagnosticEventKindV1::SpanBegin {
                        stage: DiagnosticStageV1::ManagedObjectReserve,
                        parent_span_id: Some(parent_span_id),
                        ..
                    } if parent_span_id == outer_id
                )
            })
        {
            return Err("owner substep unwind lost terminal or restored parent".into());
        }
        Ok(())
    }

    #[test]
    fn catalog_api_lane_separates_managed_commit_and_preserves_other_stages()
    -> Result<(), Box<dyn std::error::Error>> {
        let events = Arc::new(Events::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([91; 16]),
            Arc::new(StepClock::default()),
            events.clone(),
        );
        let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
        let child = context.child_context(&root);
        let work = WorkBudget::with_request(
            WorkLimits {
                reads: 1,
                writes: 1,
                bytes: 1,
                mutations: 1,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(5))
                .with_diagnostic_context(child.clone()),
        );
        let api = super::begin(&work, DiagnosticStageV1::SqliteWork);
        if super::finish(api, Err::<(), _>(IndexError::Conflict)) != Err(IndexError::Conflict) {
            return Err("Catalog API observation replaced the original error".into());
        }
        // The existing Catalog managed-install contract reserves lane3 for COMMIT.
        let commit = child.enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(3))?;
        let _terminal = commit.finish(DiagnosticTerminalV1::Ok);
        let finalize = super::begin(&work, DiagnosticStageV1::Finalize);
        super::finish(finalize, Ok::<(), IndexError>(()))?;
        let _terminal = root.finish(DiagnosticTerminalV1::Ok);
        let rows = events
            .0
            .lock()
            .map_err(|_poison| "diagnostic events poisoned")?;
        let stages: Vec<_> = rows
            .iter()
            .filter_map(|row| {
                if let DiagnosticEventKindV1::SpanBegin { stage, lane, .. } = row.kind {
                    (stage != DiagnosticStageV1::Operation).then_some((stage, lane.0))
                } else {
                    None
                }
            })
            .collect();
        drop(rows);
        if stages
            != [
                (DiagnosticStageV1::SqliteWork, 14),
                (DiagnosticStageV1::SqliteWork, 3),
                (DiagnosticStageV1::Finalize, 3),
            ]
            || work.usage() != WorkUsage::default()
        {
            return Err(format!(
                "Catalog API/COMMIT attribution aliases or changed usage: {stages:?}"
            )
            .into());
        }
        Ok(())
    }

    fn budget() -> Result<WorkBudget, IndexError> {
        Ok(WorkBudget::new(
            WorkLimits {
                reads: 1,
                writes: 1,
                bytes: 1,
                mutations: 1,
            },
            Instant::now()
                .checked_add(Duration::from_secs(10))
                .ok_or(IndexError::ResourceLimited)?,
        ))
    }

    #[test]
    fn phase_budget_parents_managed_children_and_restores_worker_context()
    -> Result<(), Box<dyn std::error::Error>> {
        for stage in [
            DiagnosticStageV1::NativeSourceApply,
            DiagnosticStageV1::NativeSourceOwnerPrepare,
            DiagnosticStageV1::NativeSourcePathValidate,
            DiagnosticStageV1::NativeSourceMapApply,
            DiagnosticStageV1::BuildPrepare,
            DiagnosticStageV1::Finalize,
            DiagnosticStageV1::SemanticPrepare,
            DiagnosticStageV1::NativeCoverageScan,
            DiagnosticStageV1::NativeCoverageMapApply,
            DiagnosticStageV1::NativeSearchablePrepare,
            DiagnosticStageV1::NativeCoverageSave,
            DiagnosticStageV1::NativeMemberAssemble,
        ] {
            let events = Arc::new(Events::default());
            let context = DiagnosticContextV1::new(
                DiagnosticCaptureIdV1([93; 16]),
                Arc::new(StepClock::default()),
                events.clone(),
            );
            let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
            let root_id = root.span_id();
            let request = RequestBudgetV1::for_duration(Duration::from_secs(5))
                .with_diagnostic_context(context.child_context(&root));
            let mut work = WorkBudget::with_request(
                WorkLimits {
                    reads: 1,
                    writes: 1,
                    bytes: 1,
                    mutations: 1,
                },
                request,
            );
            let phase = super::begin(&work, stage);
            let phase_id = phase.as_ref().ok_or("phase span absent")?.span_id();
            let mut suspended = Box::pin(async {
                let scoped = super::phase_budget(&mut work, phase.as_ref());
                let managed = super::begin(&scoped, DiagnosticStageV1::ManagedObjectReserve);
                super::finish(managed, Ok::<(), IndexError>(()))?;
                std::future::pending::<()>().await;
                Ok::<(), IndexError>(())
            });
            let waker = futures::task::noop_waker();
            let mut poll_context = std::task::Context::from_waker(&waker);
            if !matches!(
                std::future::Future::poll(suspended.as_mut(), &mut poll_context),
                std::task::Poll::Pending
            ) {
                return Err("phase future unexpectedly completed".into());
            }
            drop(suspended);
            super::finish(phase, Ok::<(), IndexError>(()))?;
            let after = super::begin(&work, DiagnosticStageV1::ManagedObjectConfirm);
            super::finish(after, Ok::<(), IndexError>(()))?;
            root.finish(DiagnosticTerminalV1::Ok)?;
            let rows = events
                .0
                .lock()
                .map_err(|_error| "diagnostic events poisoned")?;
            let parents: Vec<_> = rows
                .iter()
                .filter_map(|event| {
                    if let DiagnosticEventKindV1::SpanBegin {
                        stage,
                        parent_span_id,
                        ..
                    } = event.kind
                    {
                        Some((stage, parent_span_id))
                    } else {
                        None
                    }
                })
                .collect();
            drop(rows);
            if parents
                != [
                    (DiagnosticStageV1::Operation, None),
                    (stage, Some(root_id)),
                    (DiagnosticStageV1::ManagedObjectReserve, Some(phase_id)),
                    (DiagnosticStageV1::ManagedObjectConfirm, Some(root_id)),
                ]
                || work.usage() != WorkUsage::default()
            {
                return Err(format!("managed phase ancestry or usage changed: {parents:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn source_phase_refusal_retains_original_budget_and_child_parent()
    -> Result<(), Box<dyn std::error::Error>> {
        let events = Arc::new(Events::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([94; 16]),
            Arc::new(StepClock::default()),
            events.clone(),
        );
        let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
        let root_id = root.span_id();
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 1,
                writes: 1,
                bytes: 1,
                mutations: 1,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(5))
                .with_diagnostic_context(context.child_context(&root)),
        );
        let original = work.request().clone();
        let stage = super::WorkStage::begin(&work, DiagnosticStageV1::NativeCoverageMapApply);
        let stage_id = stage.span().ok_or("source phase span absent")?.span_id();
        let primary = {
            let mut scoped = super::phase_budget(&mut work, stage.span());
            if !scoped.request().same_request_as(&original) {
                return Err("phase replaced the original request".into());
            }
            let managed = super::begin(&scoped, DiagnosticStageV1::ManagedObjectReserve);
            let refused = scoped.charge(WorkUsage {
                reads: 2,
                ..WorkUsage::default()
            });
            super::finish_work(
                &scoped,
                DiagnosticStageV1::ManagedObjectReserve,
                managed,
                refused,
            )
        };
        let result = stage.finish(&work, primary.clone());
        if primary != Err(IndexError::ResourceLimited)
            || result != primary
            || !work.request().same_request_as(&original)
            || work.request().deadline() != original.deadline()
            || work.usage() != WorkUsage::default()
            || !format!("{work:?}").contains("first_denial: \"reads\"")
            || !format!("{work:?}").contains("failed_stage: Some(ManagedObjectReserve)")
        {
            return Err("source phase changed refusal, authority or original work".into());
        }
        root.finish(DiagnosticTerminalV1::from_result(&result))?;
        let rows = events.0.lock().map_err(|_error| "events poisoned")?;
        let parents: Vec<_> = rows
            .iter()
            .filter_map(|event| {
                if let DiagnosticEventKindV1::SpanBegin {
                    stage,
                    parent_span_id,
                    ..
                } = event.kind
                {
                    Some((stage, parent_span_id))
                } else {
                    None
                }
            })
            .collect();
        drop(rows);
        if parents
            != [
                (DiagnosticStageV1::Operation, None),
                (DiagnosticStageV1::NativeCoverageMapApply, Some(root_id)),
                (DiagnosticStageV1::ManagedObjectReserve, Some(stage_id)),
            ]
        {
            return Err(format!("source refusal lost diagnostic ancestry: {parents:?}").into());
        }
        Ok(())
    }

    #[test]
    fn work_stage_reports_only_accepted_deltas_and_preserves_off_refusals()
    -> Result<(), Box<dyn std::error::Error>> {
        for record in [false, true] {
            for refuse in [false, true] {
                let events = Arc::new(Events::default());
                let clock = Arc::new(StepClock::default());
                let context = DiagnosticContextV1::new(
                    DiagnosticCaptureIdV1([92; 16]),
                    clock.clone(),
                    events.clone(),
                );
                let root =
                    context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))?;
                let child = context.child_context(&root);
                let request = RequestBudgetV1::for_duration(Duration::from_secs(5));
                let request = if record {
                    request.with_diagnostic_context(child)
                } else {
                    request
                };
                let mut work = WorkBudget::with_request(
                    WorkLimits {
                        reads: 10,
                        writes: 10,
                        bytes: 10,
                        mutations: 10,
                    },
                    request,
                );
                work.charge(WorkUsage {
                    reads: 1,
                    ..WorkUsage::default()
                })?;
                let original_request = work.request().clone();
                let clock_before = clock.0.load(Ordering::Relaxed);
                let stage = super::WorkStage::begin(&work, DiagnosticStageV1::NativeCoverageScan);
                let stage_id = stage.span().map(DiagnosticSpanGuardV1::span_id);
                let (accepted, result) = {
                    let mut scoped = super::phase_budget(&mut work, stage.span());
                    let child = super::begin(&scoped, DiagnosticStageV1::ManagedObjectReserve);
                    super::finish(child, Ok::<(), IndexError>(()))?;
                    scoped.charge(WorkUsage {
                        reads: 2,
                        writes: 1,
                        bytes: 3,
                        mutations: 1,
                    })?;
                    let accepted = scoped.usage();
                    let result = if refuse {
                        scoped.charge(WorkUsage {
                            reads: 10,
                            ..WorkUsage::default()
                        })
                    } else {
                        Ok(())
                    };
                    (accepted, result)
                };
                let expected = result.clone();
                let actual = stage.finish(&work, result);
                if actual != expected
                    || work.usage() != accepted
                    || !work.request().same_request_as(&original_request)
                    || work.request().deadline() != original_request.deadline()
                {
                    return Err("stage observation replaced result or work usage".into());
                }
                if !record && clock.0.load(Ordering::Relaxed) != clock_before {
                    return Err("Off stage read the diagnostic clock".into());
                }
                root.finish(DiagnosticTerminalV1::from_result(&actual))?;
                let rows = events.0.lock().map_err(|_error| "events poisoned")?;
                let child_parent = rows.iter().find_map(|row| {
                    if let DiagnosticEventKindV1::SpanBegin {
                        stage: DiagnosticStageV1::ManagedObjectReserve,
                        parent_span_id,
                        ..
                    } = row.kind
                    {
                        Some(parent_span_id)
                    } else {
                        None
                    }
                });
                if record && child_parent != Some(stage_id) {
                    return Err("stage child did not inherit the phase parent".into());
                }
                let counters: Vec<_> = rows
                    .iter()
                    .filter_map(|row| {
                        if let DiagnosticEventKindV1::Counter { key, value, .. } = row.kind {
                            Some((key, value))
                        } else {
                            None
                        }
                    })
                    .collect();
                let wanted = if record {
                    vec![
                        (quanta_index_core::DiagnosticCounterV1::WorkReads, 2),
                        (quanta_index_core::DiagnosticCounterV1::WorkWrites, 1),
                        (quanta_index_core::DiagnosticCounterV1::WorkBytes, 3),
                        (quanta_index_core::DiagnosticCounterV1::WorkMutations, 1),
                    ]
                } else {
                    vec![]
                };
                if counters != wanted {
                    return Err(format!(
                        "stage counters include prior or refused work: {counters:?}"
                    )
                    .into());
                }
                if refuse && !format!("{work:?}").contains("failed_stage: Some(NativeCoverageScan)")
                {
                    return Err("Off/Record lost the actual inner refusal boundary".into());
                }
                drop(rows);
            }
        }
        Ok(())
    }

    #[test]
    fn off_success_keeps_diagnostics_and_original_budget_unchanged() -> Result<(), IndexError> {
        let budget = budget()?;
        let before = format!("{budget:?}");
        let result = finish_work(&budget, DiagnosticStageV1::BuildPrepare, None, Ok(42));
        if result != Ok(42)
            || before != format!("{budget:?}")
            || budget.request().diagnostic_context().is_some()
        {
            return Err(IndexError::BackendViolation(
                "Off success changed work or diagnostics".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn off_failure_keeps_primary_error_usage_and_first_inner_stage() -> Result<(), IndexError> {
        let mut budget = budget()?;
        let primary = budget.charge(WorkUsage {
            reads: 2,
            ..WorkUsage::default()
        });
        let inner = finish_work(
            &budget,
            DiagnosticStageV1::SemanticPrepare,
            None,
            primary.clone(),
        );
        let outer = finish_work(
            &budget,
            DiagnosticStageV1::BuildPrepare,
            None,
            inner.clone(),
        );
        let diagnostic = format!("{budget:?}");
        if primary != Err(IndexError::ResourceLimited)
            || inner != primary
            || outer != primary
            || budget.usage() != WorkUsage::default()
            || !diagnostic.contains("failed_stage: Some(SemanticPrepare)")
            || !diagnostic.contains("first_denial: \"reads\"")
        {
            return Err(IndexError::BackendViolation(
                "failure annotation replaced original refusal".into(),
            ));
        }
        if budget.checkpoint().is_err() {
            return Err(IndexError::BackendViolation(
                "diagnostic consumed execution authority".into(),
            ));
        }
        Ok(())
    }

    #[test]
    fn stage_failure_does_not_manufacture_a_budget_denial() -> Result<(), IndexError> {
        let budget = budget()?;
        let primary = IndexError::Storage("actual storage refusal".into());
        let result: Result<(), IndexError> = finish_work(
            &budget,
            DiagnosticStageV1::Finalize,
            None,
            Err(primary.clone()),
        );
        let diagnostic = format!("{budget:?}");
        if result != Err(primary)
            || !diagnostic.contains("failed_stage: Some(Finalize)")
            || !diagnostic.contains("first_denial: \"none\"")
            || budget.usage() != WorkUsage::default()
        {
            return Err(IndexError::BackendViolation(
                "storage error manufactured budget denial".into(),
            ));
        }
        Ok(())
    }
}
