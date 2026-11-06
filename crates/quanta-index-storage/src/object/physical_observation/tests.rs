use super::*;
use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticEmitV1, DiagnosticEventKindV1,
    DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1, RequestBudgetV1,
    index::{IndexError, WorkBudget, WorkLimits},
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
type RecordedCase = (Result<(), &'static str>, Vec<DiagnosticEmitV1>);

#[derive(Debug)]
struct FixedClock {
    wall: AtomicU64,
    cpu: AtomicU64,
    cpu_available: bool,
}

impl DiagnosticClockV1 for FixedClock {
    fn now_ns(&self) -> u64 {
        self.wall.load(Ordering::Relaxed)
    }

    fn thread_cpu_now_ns(&self) -> Option<u64> {
        self.cpu_available.then(|| self.cpu.load(Ordering::Relaxed))
    }
}

#[derive(Debug, Default)]
struct Events(Mutex<Vec<DiagnosticEmitV1>>);

impl DiagnosticEventSinkV1 for Events {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        self.0.try_lock().is_ok_and(|mut rows| {
            rows.push(event);
            true
        })
    }
}

fn recorded_case(
    wall_end: u64,
    cpu_end: u64,
    cpu_available: bool,
    fail: bool,
) -> TestResult<RecordedCase> {
    let (clock, events, root, mut observation) = fixture(cpu_available)?;
    clock.wall.store(100, Ordering::Relaxed);
    clock.cpu.store(50, Ordering::Relaxed);
    let result = observation.measure(DiagnosticObjectInstallPhaseV1::Write, || {
        clock.wall.store(wall_end, Ordering::Relaxed);
        clock.cpu.store(cpu_end, Ordering::Relaxed);
        if fail { Err("fixed failure") } else { Ok(()) }
    });
    clock.wall.store(200, Ordering::Relaxed);
    clock.cpu.store(100, Ordering::Relaxed);
    observation.finish(&result);
    root.finish(DiagnosticTerminalV1::Ok)?;
    Ok((result, recorded_events(&events)?))
}

type Fixture = (
    Arc<FixedClock>,
    Arc<Events>,
    DiagnosticSpanGuardV1,
    PhysicalObservation,
);

fn fixture(cpu_available: bool) -> TestResult<Fixture> {
    let clock = Arc::new(FixedClock {
        wall: AtomicU64::new(0),
        cpu: AtomicU64::new(0),
        cpu_available,
    });
    let events = Arc::new(Events::default());
    let context = DiagnosticContextV1::new(
        DiagnosticCaptureIdV1([71; 16]),
        clock.clone(),
        events.clone(),
    );
    let root = context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))?;
    let child = context.child_context(&root);
    let span = child.enter(DiagnosticStageV1::ManagedObjectInstall, DiagnosticLaneV1(3))?;
    let observation = PhysicalObservation {
        context: child,
        span: Some(span),
        cells: [PhaseCell::default(); 9],
        flushed: false,
    };
    Ok((clock, events, root, observation))
}

fn recorded_events(events: &Events) -> TestResult<Vec<DiagnosticEmitV1>> {
    Ok(events
        .0
        .lock()
        .map_err(|_error| "event sink poisoned")?
        .clone())
}

fn packet_span(events: &[DiagnosticEmitV1]) -> Option<quanta_index_core::DiagnosticSpanIdV1> {
    events.iter().find_map(|event| {
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
    })
}

#[test]
fn fixed_clock_cpu_regression_keeps_wall_and_failure_but_omits_cpu() -> TestResult {
    let (result, events) = recorded_case(125, 40, true, true)?;
    let packet = packet_span(&events).ok_or("physical packet span absent")?;
    if result != Err("fixed failure")
        || events
            .iter()
            .filter(|event| {
                matches!(event.kind, DiagnosticEventKindV1::SpanEnd {
                span_id, outcome: DiagnosticTerminalV1::Error { code: 1 },
            } if span_id == packet)
            })
            .count()
            != 1
        || events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::Health {
                        code: DiagnosticHealthCodeV1::CounterRegressed,
                    }
                )
            })
            .count()
            != 1
        || events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::ObjectInstallAggregate {
                        phase: DiagnosticObjectInstallPhaseV1::Write,
                        calls: 1,
                        elapsed_sum_ns: 25,
                        thread_cpu_ns: None,
                        failures: 1,
                        ..
                    }
                )
            })
            .count()
            != 1
    {
        return Err("CPU regression was treated as valid or lost the failed wall sample".into());
    }
    Ok(())
}

#[test]
fn fixed_clock_two_reached_calls_sum_exact_wall_and_cpu() -> TestResult {
    let (clock, events, root, mut observation) = fixture(true)?;
    clock.wall.store(100, Ordering::Relaxed);
    clock.cpu.store(50, Ordering::Relaxed);
    let first = observation.measure(DiagnosticObjectInstallPhaseV1::Write, || {
        clock.wall.store(120, Ordering::Relaxed);
        clock.cpu.store(55, Ordering::Relaxed);
        Ok::<(), IndexError>(())
    });
    first?;
    clock.wall.store(140, Ordering::Relaxed);
    clock.cpu.store(60, Ordering::Relaxed);
    let second = observation.measure(DiagnosticObjectInstallPhaseV1::Write, || {
        clock.wall.store(170, Ordering::Relaxed);
        clock.cpu.store(72, Ordering::Relaxed);
        Ok::<(), IndexError>(())
    });
    clock.wall.store(200, Ordering::Relaxed);
    clock.cpu.store(100, Ordering::Relaxed);
    observation.finish(&second);
    second?;
    root.finish(DiagnosticTerminalV1::Ok)?;
    let rows = recorded_events(&events)?;
    let packet = packet_span(&rows).ok_or("physical packet span absent")?;
    if rows
        .iter()
        .filter(|event| {
            matches!(event.kind, DiagnosticEventKindV1::ObjectInstallAggregate {
            span_id,
            phase: DiagnosticObjectInstallPhaseV1::Write,
            calls: 2,
            elapsed_sum_ns: 50,
            thread_cpu_ns: Some(17),
            failures: 0,
        } if span_id == packet)
        })
        .count()
        != 1
    {
        return Err("physical phase did not sum fixed independent wall/CPU samples".into());
    }
    Ok(())
}

#[test]
fn dropped_packet_emits_reached_aggregate_and_aborted_terminal() -> TestResult {
    let (clock, events, root, mut observation) = fixture(true)?;
    clock.wall.store(100, Ordering::Relaxed);
    clock.cpu.store(50, Ordering::Relaxed);
    observation.measure(DiagnosticObjectInstallPhaseV1::Write, || {
        clock.wall.store(120, Ordering::Relaxed);
        clock.cpu.store(60, Ordering::Relaxed);
        Ok::<(), IndexError>(())
    })?;
    clock.wall.store(130, Ordering::Relaxed);
    clock.cpu.store(70, Ordering::Relaxed);
    drop(observation);
    root.finish(DiagnosticTerminalV1::Ok)?;
    let rows = recorded_events(&events)?;
    let packet = packet_span(&rows).ok_or("physical packet span absent")?;
    if rows
        .iter()
        .filter(|event| {
            matches!(event.kind, DiagnosticEventKindV1::ObjectInstallAggregate {
            span_id,
            phase: DiagnosticObjectInstallPhaseV1::Write,
            calls: 1,
            elapsed_sum_ns: 20,
            thread_cpu_ns: Some(10),
            failures: 0,
        } if span_id == packet)
        })
        .count()
        != 1
        || rows
            .iter()
            .filter(|event| {
                matches!(event.kind, DiagnosticEventKindV1::SpanEnd {
                span_id, outcome: DiagnosticTerminalV1::Aborted,
            } if span_id == packet)
            })
            .count()
            != 1
    {
        return Err("unfinished packet lost reached phase or Aborted terminal".into());
    }
    Ok(())
}

#[test]
fn cancelled_checkpoint_keeps_product_refusal_and_failed_phase() -> TestResult {
    let (clock, events, root, mut observation) = fixture(true)?;
    let request = RequestBudgetV1::for_duration(Duration::from_secs(30));
    let budget = WorkBudget::with_request(
        WorkLimits {
            reads: 1,
            writes: 1,
            bytes: 1,
            mutations: 1,
        },
        request,
    );
    clock.wall.store(100, Ordering::Relaxed);
    clock.cpu.store(50, Ordering::Relaxed);
    let result = observation.measure(DiagnosticObjectInstallPhaseV1::ChildValidation, || {
        budget.request().cancel_handle().cancel();
        clock.wall.store(120, Ordering::Relaxed);
        clock.cpu.store(60, Ordering::Relaxed);
        budget.checkpoint()
    });
    clock.wall.store(200, Ordering::Relaxed);
    clock.cpu.store(100, Ordering::Relaxed);
    observation.finish(&result);
    root.finish(DiagnosticTerminalV1::Ok)?;
    let rows = recorded_events(&events)?;
    let packet = packet_span(&rows).ok_or("physical packet span absent")?;
    if result != Err(IndexError::Cancelled)
        || rows
            .iter()
            .filter(|event| {
                matches!(event.kind, DiagnosticEventKindV1::ObjectInstallAggregate {
                span_id,
                phase: DiagnosticObjectInstallPhaseV1::ChildValidation,
                calls: 1,
                elapsed_sum_ns: 20,
                thread_cpu_ns: Some(10),
                failures: 1,
            } if span_id == packet)
            })
            .count()
            != 1
        || rows
            .iter()
            .filter(|event| {
                matches!(event.kind, DiagnosticEventKindV1::SpanEnd {
                span_id, outcome: DiagnosticTerminalV1::Error { code: 1 },
            } if span_id == packet)
            })
            .count()
            != 1
    {
        return Err("cancelled product checkpoint or diagnostic failure was changed".into());
    }
    Ok(())
}

#[test]
fn fixed_clock_wall_regression_marks_health_and_omits_invalid_phase() -> TestResult {
    let (result, events) = recorded_case(90, 60, true, false)?;
    if result.is_err()
        || events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::Health {
                        code: DiagnosticHealthCodeV1::ClockRegressed,
                    }
                )
            })
            .count()
            != 1
        || events.iter().any(|event| {
            matches!(
                event.kind,
                DiagnosticEventKindV1::ObjectInstallAggregate {
                    phase: DiagnosticObjectInstallPhaseV1::Write,
                    ..
                }
            )
        })
    {
        return Err("wall regression fabricated a valid physical phase".into());
    }
    Ok(())
}

#[test]
fn fixed_clock_unavailable_cpu_is_absent_without_health_failure() -> TestResult {
    let (result, events) = recorded_case(125, 60, false, false)?;
    if result.is_err()
        || events
            .iter()
            .any(|event| matches!(event.kind, DiagnosticEventKindV1::Health { .. }))
        || events
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    DiagnosticEventKindV1::ObjectInstallAggregate {
                        phase: DiagnosticObjectInstallPhaseV1::Write,
                        calls: 1,
                        elapsed_sum_ns: 25,
                        thread_cpu_ns: None,
                        failures: 0,
                        ..
                    }
                )
            })
            .count()
            != 1
    {
        return Err("unavailable CPU was reported as zero or unhealthy".into());
    }
    Ok(())
}
