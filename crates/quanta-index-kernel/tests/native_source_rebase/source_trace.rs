//! Source-phase functional/cost parity on the existing real Kernel fixture.

use super::*;
use quanta_index_core::diagnostic_trace::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
    DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1,
    DiagnosticStageV1, DiagnosticTerminalV1,
};
use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Default)]
struct StepClock(AtomicU64);
impl DiagnosticClockV1 for StepClock {
    fn now_ns(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
}

#[derive(Debug)]
struct BoundedEvents {
    rows: Mutex<Vec<DiagnosticEmitV1>>,
    rejected: AtomicU64,
}
impl BoundedEvents {
    fn new() -> TestResult<Self> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(512)?;
        Ok(Self {
            rows: Mutex::new(rows),
            rejected: AtomicU64::new(0),
        })
    }
}
impl DiagnosticEventSinkV1 for BoundedEvents {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        if let Ok(mut rows) = self.rows.try_lock()
            && rows.len() < 512
        {
            rows.push(event);
            true
        } else {
            let _previous = self.rejected.fetch_add(1, Ordering::Relaxed);
            false
        }
    }
}

fn traced_request(context: Option<DiagnosticContextV1>) -> quanta_index_core::RequestBudgetV1 {
    let request_budget = quanta_index_core::RequestBudgetV1::for_duration(Duration::from_secs(30));
    match context {
        Some(context) => request_budget.with_diagnostic_context(context),
        None => request_budget,
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "test-only synchronous driver exercises the real Kernel submit future"
)]
fn observed_submit(
    fixture: &Fixture,
    request: UpdateRequest,
    mut work: WorkBudget,
) -> (
    Result<SubmitOutcome, IndexError>,
    quanta_index_core::index::WorkUsage,
) {
    let outcome = futures::executor::block_on(fixture.engine.submit(request, &mut work));
    (outcome, work.usage())
}

#[test]
fn source_trace_preserves_publication_cost_and_closes_refused_preflight() -> TestResult {
    let clock = Arc::new(StepClock::default());
    let events = Arc::new(BoundedEvents::new()?);
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([93; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))?;
    let context = context.child_context(&root);
    let mut observed = Vec::new();
    for record in [false, true] {
        let before_clock = clock.0.load(Ordering::Relaxed);
        let directory = tempfile::tempdir()?;
        let fixture = Fixture::open(directory.path())?;
        let configured = fixture.configure()?;
        let (_, request, _) = fixture.capture(
            key(5, 6, 2),
            configured,
            owner(5, 1),
            None,
            "alpha.rs",
            b"alpha unique",
            1,
        )?;
        let trace = record.then(|| context.clone());
        let (outcome, success_work) = observed_submit(
            &fixture,
            request,
            budget_with_request(traced_request(trace)),
        );
        let published = published(outcome?)?;
        let cut = fixture
            .engine
            .pin_view(ViewSelection::Exact(published), &budget())?
            .view()
            .source;
        let (_, collision, _) = fixture.capture(
            key(7, 8, 2),
            published,
            owner(7, 1),
            None,
            "alpha.rs",
            b"beta unique",
            1,
        )?;
        let trace = record.then(|| context.clone());
        let (outcome, refused_work) = observed_submit(
            &fixture,
            collision,
            budget_with_request(traced_request(trace)),
        );
        let outcome = outcome?;
        check_eq!(
            outcome,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict)),
            "source trace changed path-collision refusal"
        );
        check_eq!(
            latest_view(&fixture)?,
            published,
            "refused trace operation advanced head"
        );
        observed.push((success_work, refused_work, cut));
        if !record {
            check_eq!(
                clock.0.load(Ordering::Relaxed),
                before_clock,
                "Off source path touched the diagnostic clock"
            );
        }
    }
    check_eq!(
        observed.first(),
        observed.get(1),
        "Off/Record changed source cuts or logical work"
    );
    root.finish(DiagnosticTerminalV1::Ok)?;
    check_eq!(
        events.rejected.load(Ordering::Relaxed),
        0,
        "test trace lost events"
    );
    let rows = events.rows.lock().map_err(|error| error.to_string())?;
    let mut phases = Vec::new();
    for row in rows.iter() {
        if let DiagnosticEventKindV1::SpanBegin { span_id, stage, .. } = row.kind
            && matches!(
                stage,
                DiagnosticStageV1::NativeSourceApply
                    | DiagnosticStageV1::NativeSourcePathInstall
                    | DiagnosticStageV1::NativeSourceMapApply
            )
        {
            let ends: Vec<_> = rows
                .iter()
                .filter_map(|row| match row.kind {
                    DiagnosticEventKindV1::SpanEnd {
                        span_id: ended,
                        outcome,
                    } if ended == span_id => Some(outcome),
                    DiagnosticEventKindV1::SpanBegin { .. }
                    | DiagnosticEventKindV1::SpanEnd { .. }
                    | DiagnosticEventKindV1::RequestIdentity { .. }
                    | DiagnosticEventKindV1::PhaseIdentity { .. }
                    | DiagnosticEventKindV1::Counter { .. }
                    | DiagnosticEventKindV1::AnnRegionObservation { .. }
                    | DiagnosticEventKindV1::SqliteSyncObservation { .. }
                    | DiagnosticEventKindV1::SyncAggregate { .. }
                    | DiagnosticEventKindV1::ObjectKindAggregate { .. }
                    | DiagnosticEventKindV1::AdoptionGroupAggregate { .. }
                    | DiagnosticEventKindV1::Bm25CensusAggregate { .. }
                    | DiagnosticEventKindV1::Bm25BoundaryAggregate { .. }
                    | DiagnosticEventKindV1::Bm25LiveBuildAggregate { .. }
                    | DiagnosticEventKindV1::Link { .. }
                    | DiagnosticEventKindV1::Health { .. } => None,
                })
                .collect();
            check_eq!(
                ends.len(),
                1,
                "source phase has missing or repeated terminal"
            );
            phases.push((stage, *ends.first().ok_or("missing source phase terminal")?));
        }
    }
    // Inline path values are written into attempt-local scratch during
    // SourceApply. A late collision ends that stage before MapApply; there is
    // no separate path-install phase or durable partial root to report.
    check_eq!(
        phases,
        vec![
            (
                DiagnosticStageV1::NativeSourceApply,
                DiagnosticTerminalV1::Ok
            ),
            (
                DiagnosticStageV1::NativeSourceMapApply,
                DiagnosticTerminalV1::Ok
            ),
            (
                DiagnosticStageV1::NativeSourceApply,
                DiagnosticTerminalV1::Error { code: 1 }
            ),
        ],
        "source phase outcomes differ from validated scratch and map application scopes"
    );
    let preflight: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                stage: DiagnosticStageV1::BuildPreflight,
                ..
            } = row.kind
            {
                Some(span_id)
            } else {
                None
            }
        })
        .collect();
    check_eq!(
        preflight.len(),
        2,
        "successful and path-conflict submit require two preflight scopes"
    );
    for span_id in preflight {
        let terminals: Vec<_> = rows
            .iter()
            .filter_map(|row| {
                if let DiagnosticEventKindV1::SpanEnd {
                    span_id: ended,
                    outcome,
                } = row.kind
                    && ended == span_id
                {
                    Some(outcome)
                } else {
                    None
                }
            })
            .collect();
        check_eq!(
            terminals.as_slice(),
            &[DiagnosticTerminalV1::Ok],
            "successful preflight scope has missing or incorrect terminal"
        );
    }
    check!(
        !rows
            .iter()
            .any(|row| matches!(row.kind, DiagnosticEventKindV1::Health { .. })),
        "source trace recorded health failure"
    );
    drop(rows);
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum PreflightFailure {
    InvalidEventDigest,
    ZeroWork,
    Cancelled,
}

#[test]
fn source_preflight_failures_close_error_span_before_catalog_admission() -> TestResult {
    for failure in [
        PreflightFailure::InvalidEventDigest,
        PreflightFailure::ZeroWork,
        PreflightFailure::Cancelled,
    ] {
        let mut observed = Vec::new();
        for record in [false, true] {
            let directory = tempfile::tempdir()?;
            let fixture = Fixture::open(directory.path())?;
            let configured = fixture.configure()?;
            let operation = key(21, 22, 2);
            let (_, mut request, _) = fixture.capture(
                operation,
                configured,
                owner(21, 1),
                None,
                "preflight.rs",
                b"preflight source",
                1,
            )?;
            if matches!(failure, PreflightFailure::InvalidEventDigest) {
                let UpdateKind::SourceAdvance(transition) = &mut request.kind else {
                    return Err("preflight fixture lost source transition".into());
                };
                let correct = transition.owner_digest()?;
                let wrong = Digest([99; 32]);
                check!(
                    wrong != correct,
                    "invalid event digest fixture matches owner digest"
                );
                let event = transition
                    .events
                    .first_mut()
                    .ok_or("preflight fixture lost source event")?;
                event.payload = wrong;
            }

            let clock = Arc::new(StepClock::default());
            let events = Arc::new(BoundedEvents::new()?);
            let context = DiagnosticContextV1::new(
                DiagnosticCaptureIdV1([94; 16]),
                clock.clone(),
                events.clone(),
            );
            let root = if record {
                Some(context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(4))?)
            } else {
                None
            };
            let trace = root.as_ref().map(|root| context.child_context(root));
            let request_budget = traced_request(trace);
            if matches!(failure, PreflightFailure::Cancelled) {
                request_budget.cancel_handle().cancel();
            }
            let work = if matches!(failure, PreflightFailure::ZeroWork) {
                WorkBudget::with_request(
                    WorkLimits {
                        reads: 0,
                        writes: 0,
                        bytes: 0,
                        mutations: 0,
                    },
                    request_budget,
                )
            } else {
                budget_with_request(request_budget)
            };
            let (result, usage) = observed_submit(&fixture, request, work);
            match failure {
                PreflightFailure::InvalidEventDigest => {
                    check!(
                        matches!(&result, Err(IndexError::InvalidInput(_))),
                        "invalid event digest did not fail in preflight"
                    );
                }
                PreflightFailure::ZeroWork => {
                    check_eq!(
                        result,
                        Err(IndexError::ResourceLimited),
                        "zero work budget changed preflight error"
                    );
                    check_eq!(
                        usage,
                        quanta_index_core::index::WorkUsage::default(),
                        "zero work budget charged usage"
                    );
                }
                PreflightFailure::Cancelled => {
                    check_eq!(
                        result,
                        Err(IndexError::Cancelled),
                        "pre-cancelled request changed preflight error"
                    );
                    check_eq!(
                        usage,
                        quanta_index_core::index::WorkUsage::default(),
                        "pre-cancelled request charged usage"
                    );
                }
            }
            check_eq!(
                latest_view(&fixture)?,
                configured,
                "preflight failure advanced source head"
            );
            check_eq!(
                fixture.engine.status(
                    operation,
                    quanta_index_core::index::ports::OperationInspectAccess::Request(&budget())
                )?,
                OperationStatus::Unknown,
                "preflight failure admitted a journal operation"
            );
            observed.push((result, usage));

            if let Some(root) = root {
                root.finish(DiagnosticTerminalV1::Ok)?;
            }
            check_eq!(
                events.rejected.load(Ordering::Relaxed),
                0,
                "preflight trace lost events"
            );
            let rows = events.rows.lock().map_err(|error| error.to_string())?;
            if !record {
                check_eq!(
                    clock.0.load(Ordering::Relaxed),
                    0,
                    "Off preflight touched diagnostic clock"
                );
                check!(rows.is_empty(), "Off preflight emitted trace events");
                drop(rows);
                continue;
            }
            let preflight: Vec<_> = rows
                .iter()
                .filter_map(|row| {
                    if let DiagnosticEventKindV1::SpanBegin {
                        span_id,
                        stage: DiagnosticStageV1::BuildPreflight,
                        ..
                    } = row.kind
                    {
                        Some(span_id)
                    } else {
                        None
                    }
                })
                .collect();
            check_eq!(
                preflight.len(),
                1,
                "preflight failure did not emit exactly one span"
            );
            let span_id = *preflight.first().ok_or("preflight span missing")?;
            let terminals: Vec<_> = rows
                .iter()
                .filter_map(|row| {
                    if let DiagnosticEventKindV1::SpanEnd {
                        span_id: ended,
                        outcome,
                    } = row.kind
                        && ended == span_id
                    {
                        Some(outcome)
                    } else {
                        None
                    }
                })
                .collect();
            check_eq!(
                terminals,
                vec![DiagnosticTerminalV1::Error { code: 1 }],
                "preflight error span has a missing or incorrect terminal"
            );
            check_eq!(
                rows.iter()
                    .filter(|row| matches!(
                        row.kind,
                        DiagnosticEventKindV1::SpanEnd {
                            outcome: DiagnosticTerminalV1::Error { .. },
                            ..
                        }
                    ))
                    .count(),
                1,
                "preflight failure emitted another error terminal"
            );
            // Earlier journal inspection is outside the measured admission APIs.
            check!(
                !rows.iter().any(|row| matches!(
                    row.kind,
                    DiagnosticEventKindV1::SpanBegin {
                        stage: DiagnosticStageV1::SqliteWork,
                        ..
                    }
                )),
                "preflight failure reached SqliteWork"
            );
            check!(
                !rows.iter().any(|row| matches!(
                    row.kind,
                    DiagnosticEventKindV1::SpanEnd {
                        outcome: DiagnosticTerminalV1::Aborted,
                        ..
                    } | DiagnosticEventKindV1::Health { .. }
                )),
                "preflight failure emitted drop or health event"
            );
            drop(rows);
        }
        check_eq!(
            observed.first(),
            observed.get(1),
            "Off/Record changed preflight error or logical usage"
        );
    }
    Ok(())
}
