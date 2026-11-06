//! Source-phase functional/cost parity on the existing real Kernel fixture.

use super::*;
use quanta_index_core::diagnostic_trace::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticCounterV1,
    DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1,
    DiagnosticRootKindV1, DiagnosticSpanIdV1, DiagnosticStageV1, DiagnosticTerminalV1,
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
    state: Mutex<FocusedEvents>,
    focus: EventFocus,
    rejected: AtomicU64,
}

#[derive(Clone, Copy, Debug)]
enum EventFocus {
    Source,
    Preflight,
}

impl EventFocus {
    fn includes(self, stage: DiagnosticStageV1) -> bool {
        match self {
            Self::Source => matches!(
                stage,
                DiagnosticStageV1::NativeSourceApply
                    | DiagnosticStageV1::NativeSourceOwnerPrepare
                    | DiagnosticStageV1::NativeSourcePathValidate
                    | DiagnosticStageV1::NativeSourcePathInstall
                    | DiagnosticStageV1::NativeSourceMapApply
                    | DiagnosticStageV1::NativeWorkSeal
                    | DiagnosticStageV1::BuildPreflight
            ),
            Self::Preflight => matches!(
                stage,
                DiagnosticStageV1::BuildPreflight
                    | DiagnosticStageV1::CatalogLockWait
                    | DiagnosticStageV1::CatalogLockHold
                    | DiagnosticStageV1::SqliteWork
            ),
        }
    }
}

// This fixture retains a bounded projection, not a complete trace. Count
// intentionally ignored events and every error terminal independently.
#[derive(Debug)]
struct FocusedEvents {
    rows: Vec<DiagnosticEmitV1>,
    selected: Vec<DiagnosticSpanIdV1>,
    ignored: u64,
    error_terminals: u64,
    aborted_terminals: u64,
}

impl BoundedEvents {
    fn new(focus: EventFocus) -> TestResult<Self> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(512)?;
        let mut selected = Vec::new();
        selected.try_reserve_exact(512)?;
        Ok(Self {
            state: Mutex::new(FocusedEvents {
                rows,
                selected,
                ignored: 0,
                error_terminals: 0,
                aborted_terminals: 0,
            }),
            focus,
            rejected: AtomicU64::new(0),
        })
    }
}
impl DiagnosticEventSinkV1 for BoundedEvents {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        let Ok(mut state) = self.state.try_lock() else {
            let _previous = self.rejected.fetch_add(1, Ordering::Relaxed);
            return false;
        };
        if matches!(
            event.kind,
            DiagnosticEventKindV1::SpanEnd {
                outcome: DiagnosticTerminalV1::Error { .. },
                ..
            }
        ) {
            state.error_terminals = state.error_terminals.saturating_add(1);
        }
        if matches!(
            event.kind,
            DiagnosticEventKindV1::SpanEnd {
                outcome: DiagnosticTerminalV1::Aborted,
                ..
            }
        ) {
            state.aborted_terminals = state.aborted_terminals.saturating_add(1);
        }
        let selected_begin = if let DiagnosticEventKindV1::SpanBegin { span_id, stage, .. } =
            event.kind
            && self.focus.includes(stage)
        {
            Some(span_id)
        } else {
            None
        };
        let retain = selected_begin.is_some()
            || matches!(event.kind, DiagnosticEventKindV1::Health { .. })
            || matches!(
                event.kind,
                DiagnosticEventKindV1::SpanEnd { span_id, .. }
                    if state.selected.contains(&span_id)
            )
            || matches!(
                event.kind,
                DiagnosticEventKindV1::Counter { span_id, .. }
                    if state.selected.contains(&span_id)
            );
        if !retain {
            state.ignored = state.ignored.saturating_add(1);
            return true;
        }
        if state.rows.len() >= 512 || (selected_begin.is_some() && state.selected.len() >= 512) {
            let _previous = self.rejected.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        if let Some(span_id) = selected_begin {
            state.selected.push(span_id);
        }
        state.rows.push(event);
        true
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
    let events = Arc::new(BoundedEvents::new(EventFocus::Source)?);
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([93; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))?;
    let root_id = root.span_id();
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
    let state = events.state.lock().map_err(|error| error.to_string())?;
    check!(
        state.ignored > 0,
        "source fixture did not record its focused projection"
    );
    let rows = &state.rows;
    let mut phases = Vec::new();
    let mut current_source_span = None;
    for row in rows {
        if let DiagnosticEventKindV1::SpanBegin {
            span_id,
            parent_span_id,
            stage,
            lane: DiagnosticLaneV1(3),
            ..
        } = row.kind
            && matches!(
                stage,
                DiagnosticStageV1::NativeSourceApply
                    | DiagnosticStageV1::NativeSourceOwnerPrepare
                    | DiagnosticStageV1::NativeSourcePathValidate
                    | DiagnosticStageV1::NativeSourcePathInstall
                    | DiagnosticStageV1::NativeSourceMapApply
            )
        {
            if stage == DiagnosticStageV1::NativeSourceApply {
                current_source_span = Some(span_id);
            } else {
                check_eq!(
                    parent_span_id,
                    current_source_span,
                    "source preparation and map phases must be sibling children of SourceApply"
                );
            }
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
                    | DiagnosticEventKindV1::OwnedRenewalRefusal { .. }
                    | DiagnosticEventKindV1::SyncAggregate { .. }
                    | DiagnosticEventKindV1::ObjectInstallAggregate { .. }
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
    // PathValidate. A late collision ends it before MapApply; there is no
    // separate path-install phase or durable partial root to report.
    check_eq!(
        phases,
        vec![
            (
                DiagnosticStageV1::NativeSourceApply,
                DiagnosticTerminalV1::Ok
            ),
            (
                DiagnosticStageV1::NativeSourceOwnerPrepare,
                DiagnosticTerminalV1::Ok
            ),
            (
                DiagnosticStageV1::NativeSourcePathValidate,
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
            (
                DiagnosticStageV1::NativeSourceOwnerPrepare,
                DiagnosticTerminalV1::Ok
            ),
            (
                DiagnosticStageV1::NativeSourcePathValidate,
                DiagnosticTerminalV1::Error { code: 1 }
            ),
        ],
        "source phase outcomes differ from validated scratch and map application scopes"
    );
    let source_spans = rows
        .iter()
        .enumerate()
        .filter_map(|(begin_at, row)| match row.kind {
            DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id,
                stage: DiagnosticStageV1::NativeSourceApply,
                lane: DiagnosticLaneV1(3),
                ..
            } => Some((begin_at, span_id, parent_span_id)),
            _ => None,
        })
        .collect::<Vec<_>>();
    check_eq!(
        source_spans.len(),
        2,
        "expected one SourceApply per recorded submit"
    );
    let (success_work, refused_work, _) = observed.get(1).ok_or("recorded source work absent")?;
    for (ordinal, (begin_at, span_id, parent)) in source_spans.into_iter().enumerate() {
        check_eq!(parent, Some(root_id), "SourceApply lost its operation root");
        let (end_at, outcome) = rows
            .iter()
            .enumerate()
            .find_map(|(at, row)| match row.kind {
                DiagnosticEventKindV1::SpanEnd {
                    span_id: ended,
                    outcome,
                } if ended == span_id => Some((at, outcome)),
                _ => None,
            })
            .ok_or("SourceApply terminal absent")?;
        check!(begin_at < end_at, "SourceApply ended before it began");
        let expected_outcome = if ordinal == 0 {
            DiagnosticTerminalV1::Ok
        } else {
            DiagnosticTerminalV1::Error { code: 1 }
        };
        check_eq!(outcome, expected_outcome, "SourceApply terminal changed");
        let operation_work = if ordinal == 0 {
            success_work
        } else {
            refused_work
        };
        for (key, ceiling) in [
            (DiagnosticCounterV1::WorkReads, operation_work.reads),
            (DiagnosticCounterV1::WorkWrites, operation_work.writes),
            (DiagnosticCounterV1::WorkBytes, operation_work.bytes),
            (DiagnosticCounterV1::WorkMutations, operation_work.mutations),
        ] {
            let matching = rows
                .iter()
                .enumerate()
                .filter_map(|(at, row)| match row.kind {
                    DiagnosticEventKindV1::Counter {
                        span_id: counter_span,
                        key: counter_key,
                        value,
                    } if counter_span == span_id && counter_key == key => Some((at, value)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            check_eq!(
                matching.len(),
                1,
                "SourceApply work counter is missing or repeated"
            );
            let (counter_at, value) = *matching.first().ok_or("SourceApply counter absent")?;
            check!(
                begin_at < counter_at && counter_at < end_at,
                "SourceApply counter must precede its terminal"
            );
            check!(
                value <= ceiling,
                "SourceApply work exceeds whole operation work"
            );
            if ordinal == 0 {
                check!(value > 0, "successful SourceApply work must be nonzero");
            }
        }
    }
    // Preserve the original coarse phase oracle and independently require the
    // two one-owner attempts' complete synchronous substep boundaries.
    for (expected_lane, expected_count) in [(30, 4), (31, 2), (32, 2), (33, 2), (34, 2)] {
        let children = rows
            .iter()
            .filter_map(|row| match row.kind {
                DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id,
                    stage: DiagnosticStageV1::NativeSourceOwnerPrepare,
                    lane: DiagnosticLaneV1(lane),
                    ..
                } if lane == expected_lane => Some((span_id, parent_span_id)),
                _ => None,
            })
            .collect::<Vec<_>>();
        check_eq!(
            children.len(),
            expected_count,
            "owner substep call count differs"
        );
        for (span_id, parent) in children {
            check!(
                rows.iter().any(|row| matches!(
                    row.kind,
                    DiagnosticEventKindV1::SpanBegin {
                        span_id: observed,
                        stage: DiagnosticStageV1::NativeSourceOwnerPrepare,
                        lane: DiagnosticLaneV1(3),
                        ..
                    } if Some(observed) == parent
                )),
                "owner substep lost the original coarse parent"
            );
            let terminals = rows
                .iter()
                .filter_map(|row| match row.kind {
                    DiagnosticEventKindV1::SpanEnd {
                        span_id: ended,
                        outcome,
                    } if ended == span_id => Some(outcome),
                    _ => None,
                })
                .collect::<Vec<_>>();
            check_eq!(
                terminals,
                vec![DiagnosticTerminalV1::Ok],
                "owner substep terminal differs"
            );
        }
    }
    let source_parent = rows
        .iter()
        .find_map(|row| {
            if let DiagnosticEventKindV1::SpanBegin {
                parent_span_id,
                stage: DiagnosticStageV1::NativeSourceApply,
                ..
            } = row.kind
            {
                Some(parent_span_id)
            } else {
                None
            }
        })
        .ok_or("successful SourceApply span is absent")?;
    let work_seals: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            if let DiagnosticEventKindV1::SpanBegin {
                span_id,
                parent_span_id,
                stage: DiagnosticStageV1::NativeWorkSeal,
                ..
            } = row.kind
            {
                Some((span_id, parent_span_id, row.ts_ns))
            } else {
                None
            }
        })
        .collect();
    check_eq!(
        work_seals.len(),
        1,
        "successful SourceAdvance must seal work once; refused preflight must not"
    );
    let (seal_id, seal_parent, seal_start) = *work_seals.first().ok_or("work seal absent")?;
    check_eq!(
        seal_parent,
        source_parent,
        "work seal must retain the SourceApply operation parent"
    );
    let seal_ends: Vec<_> = rows
        .iter()
        .filter_map(|row| {
            if let DiagnosticEventKindV1::SpanEnd { span_id, outcome } = row.kind
                && span_id == seal_id
            {
                Some((outcome, row.ts_ns))
            } else {
                None
            }
        })
        .collect();
    check_eq!(seal_ends.len(), 1, "work seal must close exactly once");
    let (seal_outcome, seal_end) = *seal_ends.first().ok_or("work seal end absent")?;
    check_eq!(
        seal_outcome,
        DiagnosticTerminalV1::Ok,
        "successful work seal must close successfully"
    );
    check!(seal_end >= seal_start, "work seal ended before it began");
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
    drop(state);
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum PreflightFailure {
    InvalidEventDigest,
    ZeroWork,
    Cancelled,
}

#[test]
fn source_preflight_failures_and_early_cancellation_preserve_trace_boundaries() -> TestResult {
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
            let events = Arc::new(BoundedEvents::new(EventFocus::Preflight)?);
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
            let state = events.state.lock().map_err(|error| error.to_string())?;
            let rows = &state.rows;
            if !record {
                check_eq!(
                    clock.0.load(Ordering::Relaxed),
                    0,
                    "Off preflight touched diagnostic clock"
                );
                check!(rows.is_empty(), "Off preflight emitted trace events");
                drop(state);
                continue;
            }
            check!(
                state.ignored > 0,
                "preflight fixture did not record its focused projection"
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
            let entered_preflight = !matches!(failure, PreflightFailure::Cancelled);
            check_eq!(
                preflight.len(),
                usize::from(entered_preflight),
                "preflight span differs from the actual submit entry order"
            );
            if let Some(span_id) = preflight.first() {
                let terminals: Vec<_> = rows
                    .iter()
                    .filter_map(|row| {
                        if let DiagnosticEventKindV1::SpanEnd {
                            span_id: ended,
                            outcome,
                        } = row.kind
                            && ended == *span_id
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
            }
            check_eq!(
                state.error_terminals,
                u64::from(entered_preflight),
                "preflight failure emitted a missing or additional error terminal"
            );
            check_eq!(
                state.aborted_terminals,
                0,
                "preflight failure aborted a selected or filtered span"
            );
            if matches!(failure, PreflightFailure::Cancelled) {
                let waits: Vec<_> = rows
                    .iter()
                    .filter_map(|row| {
                        if let DiagnosticEventKindV1::SpanBegin {
                            span_id,
                            stage: DiagnosticStageV1::CatalogLockWait,
                            ..
                        } = row.kind
                        {
                            Some(span_id)
                        } else {
                            None
                        }
                    })
                    .collect();
                check_eq!(waits.len(), 1, "cancelled inspect lacked its one lock wait");
                let wait = *waits.first().ok_or("cancelled lock wait missing")?;
                let terminals: Vec<_> = rows
                    .iter()
                    .filter_map(|row| {
                        if let DiagnosticEventKindV1::SpanEnd { span_id, outcome } = row.kind
                            && span_id == wait
                        {
                            Some(outcome)
                        } else {
                            None
                        }
                    })
                    .collect();
                check_eq!(
                    terminals,
                    vec![DiagnosticTerminalV1::Cancelled { code: 1 }],
                    "cancelled inspect lock wait had an incorrect terminal"
                );
                check!(
                    !rows.iter().any(|row| matches!(
                        row.kind,
                        DiagnosticEventKindV1::SpanBegin {
                            stage: DiagnosticStageV1::CatalogLockHold,
                            ..
                        }
                    )),
                    "cancelled inspect acquired a catalog lock"
                );
            }
            // Journal inspection may record lock wait/hold, but preflight
            // refusals must not enter measured SqliteWork admission scopes.
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
            drop(state);
        }
        check_eq!(
            observed.first(),
            observed.get(1),
            "Off/Record changed preflight error or logical usage"
        );
    }
    Ok(())
}
