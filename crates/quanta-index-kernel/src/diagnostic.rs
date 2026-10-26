//! Optional spans over actual Native build and publication calls.

use quanta_index_core::{
    DiagnosticContextV1, DiagnosticCounterV1, DiagnosticHealthCodeV1, DiagnosticLaneV1,
    DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1, DiagnosticTraceErrorV1,
    index::{IndexError, OperationStatus, WorkBudget, WorkUsage},
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
    context: Option<DiagnosticContextV1>,
    before: Option<WorkUsage>,
}

impl WorkStage {
    pub(crate) fn begin(budget: &WorkBudget, stage: DiagnosticStageV1) -> Self {
        let span = begin(budget, stage);
        let before = span.as_ref().map(|_span| budget.usage());
        let context = span
            .as_ref()
            .and_then(|_span| budget.request().diagnostic_context().cloned());
        Self {
            stage,
            span,
            context,
            before,
        }
    }

    pub(crate) fn finish<T>(
        self,
        budget: &WorkBudget,
        result: Result<T, IndexError>,
    ) -> Result<T, IndexError> {
        if let (Some(context), Some(span), Some(before)) = (&self.context, &self.span, self.before)
        {
            let after = budget.usage();
            for (key, end, start) in [
                (DiagnosticCounterV1::WorkReads, after.reads, before.reads),
                (DiagnosticCounterV1::WorkWrites, after.writes, before.writes),
                (DiagnosticCounterV1::WorkBytes, after.bytes, before.bytes),
                (
                    DiagnosticCounterV1::WorkMutations,
                    after.mutations,
                    before.mutations,
                ),
            ] {
                if let Some(value) = end.checked_sub(start) {
                    context.counter(span, key, value);
                } else {
                    context.health(DiagnosticHealthCodeV1::CounterOverflow);
                }
            }
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

    use super::{DiagnosticStageV1, IndexError, WorkBudget, finish_work};

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
            DiagnosticStageV1::BuildPrepare,
            DiagnosticStageV1::Finalize,
            DiagnosticStageV1::SemanticPrepare,
            DiagnosticStageV1::NativeCoverageSave,
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
                let clock_before = clock.0.load(Ordering::Relaxed);
                let stage = super::WorkStage::begin(&work, DiagnosticStageV1::NativeCoverageScan);
                work.charge(WorkUsage {
                    reads: 2,
                    writes: 1,
                    bytes: 3,
                    mutations: 1,
                })?;
                let accepted = work.usage();
                let result = if refuse {
                    work.charge(WorkUsage {
                        reads: 10,
                        ..WorkUsage::default()
                    })
                } else {
                    Ok(())
                };
                let expected = result.clone();
                let actual = stage.finish(&work, result);
                if actual != expected || work.usage() != accepted {
                    return Err("stage observation replaced result or work usage".into());
                }
                if !record && clock.0.load(Ordering::Relaxed) != clock_before {
                    return Err("Off stage read the diagnostic clock".into());
                }
                root.finish(DiagnosticTerminalV1::from_result(&actual))?;
                let rows = events.0.lock().map_err(|_error| "events poisoned")?;
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
