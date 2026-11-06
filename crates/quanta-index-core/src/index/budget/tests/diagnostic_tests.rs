use super::*;
use std::sync::Mutex;

#[derive(Debug, Default)]
struct UsageSink(Mutex<Vec<crate::DiagnosticEmitV1>>);

impl crate::DiagnosticEventSinkV1 for UsageSink {
    fn try_emit(&self, event: crate::DiagnosticEmitV1) -> bool {
        let Ok(mut events) = self.0.try_lock() else {
            return false;
        };
        events.push(event);
        true
    }
}

fn trace() -> (
    crate::DiagnosticContextV1,
    Arc<DiagnosticClockProbe>,
    Arc<UsageSink>,
) {
    let clock = Arc::new(DiagnosticClockProbe::default());
    let sink = Arc::new(UsageSink::default());
    let diagnostic = crate::DiagnosticContextV1::new(
        crate::DiagnosticCaptureIdV1([83; 16]),
        clock.clone(),
        sink.clone(),
    );
    (diagnostic, clock, sink)
}

fn recorded(sink: &UsageSink) -> Result<Vec<crate::DiagnosticEmitV1>, IndexError> {
    Ok(sink
        .0
        .lock()
        .map_err(|_error| IndexError::BackendViolation("usage sink poisoned".into()))?
        .clone())
}

const WORK_KEYS: [crate::DiagnosticCounterV1; 4] = [
    crate::DiagnosticCounterV1::WorkReads,
    crate::DiagnosticCounterV1::WorkWrites,
    crate::DiagnosticCounterV1::WorkBytes,
    crate::DiagnosticCounterV1::WorkMutations,
];

fn check_work_family(
    events: &[crate::DiagnosticEmitV1],
    span_id: crate::DiagnosticSpanIdV1,
    expected: [u64; 4],
) -> Result<(), IndexError> {
    let end = events
        .iter()
        .position(|event| {
            matches!(
                event.kind,
                crate::DiagnosticEventKindV1::SpanEnd { span_id: ended, .. }
                    if ended == span_id
            )
        })
        .ok_or_else(|| IndexError::BackendViolation("work span terminal absent".into()))?;
    let mut observed: [Option<u64>; 4] = [None; 4];
    for (event_index, event) in events.iter().enumerate() {
        if let crate::DiagnosticEventKindV1::Counter {
            span_id: owner,
            key,
            value,
        } = event.kind
            && WORK_KEYS.contains(&key)
        {
            check_eq!(owner, span_id);
            check!(event_index < end);
            let slot = WORK_KEYS
                .iter()
                .position(|known| *known == key)
                .and_then(|index| observed.get_mut(index))
                .ok_or_else(|| IndexError::BackendViolation("work key slot absent".into()))?;
            check!(slot.replace(value).is_none());
        }
    }
    check_eq!(observed, expected.map(Some));
    Ok(())
}

#[test]
fn diagnostic_usage_delta_emits_fixed_accepted_work_before_terminal() -> Result<(), IndexError> {
    let (diagnostic, clock, sink) = trace();
    let span = diagnostic
        .enter_root(
            crate::DiagnosticRootKindV1::Operation,
            crate::DiagnosticLaneV1(0),
        )
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let span_id = span.span_id();
    let authority = provider_port();
    let mut budget = WorkBudget::with_request(
        limits(),
        crate::RequestBudgetV1::for_duration(Duration::from_secs(30))
            .with_diagnostic_context(diagnostic),
    );
    budget.bind_provider_attempt_custody(&authority)?;
    let request = budget.request().clone();
    let before = budget.usage();
    budget.charge(WorkUsage {
        reads: 2,
        writes: 1,
        bytes: 7,
        mutations: 3,
    })?;
    let accepted = budget.usage();
    check_eq!(
        budget.charge(WorkUsage {
            reads: 2,
            ..WorkUsage::default()
        }),
        Err(IndexError::ResourceLimited)
    );
    check_eq!(budget.usage(), accepted);
    let clock_before = clock.0.load(Ordering::Relaxed);
    budget.record_diagnostic_usage_delta(before, &span);
    check_eq!(
        clock.0.load(Ordering::Relaxed).checked_sub(clock_before),
        Some(4)
    );
    check_eq!(budget.usage(), accepted);
    check!(budget.request().same_request_as(&request));
    check_eq!(budget.request().deadline(), request.deadline());
    check!(matches!(
        budget.request().begin_provider_attempt_v1(),
        Ok(Some(_))
    ));
    check_eq!(budget.first_denial(), Some(WorkBudgetDenial::Reads));
    span.finish(crate::DiagnosticTerminalV1::Error { code: 1 })
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    check_work_family(&recorded(&sink)?, span_id, [2, 1, 7, 3])
}

#[test]
fn diagnostic_usage_delta_emits_four_observed_zeros() -> Result<(), IndexError> {
    let (diagnostic, clock, sink) = trace();
    let span = diagnostic
        .enter_root(
            crate::DiagnosticRootKindV1::Operation,
            crate::DiagnosticLaneV1(0),
        )
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let span_id = span.span_id();
    let budget = WorkBudget::with_request(
        limits(),
        crate::RequestBudgetV1::for_duration(Duration::from_secs(30))
            .with_diagnostic_context(diagnostic),
    );
    let before = budget.usage();
    let request = budget.request().clone();
    let clock_before = clock.0.load(Ordering::Relaxed);
    budget.record_diagnostic_usage_delta(before, &span);
    check_eq!(
        clock.0.load(Ordering::Relaxed).checked_sub(clock_before),
        Some(4)
    );
    check_eq!(budget.usage(), WorkUsage::default());
    check!(budget.request().same_request_as(&request));
    span.finish(crate::DiagnosticTerminalV1::Ok)
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    check_work_family(&recorded(&sink)?, span_id, [0, 0, 0, 0])
}

#[test]
fn diagnostic_usage_delta_underflow_reports_health_without_partial_family() -> Result<(), IndexError>
{
    let (diagnostic, _clock, sink) = trace();
    let span = diagnostic
        .enter_root(
            crate::DiagnosticRootKindV1::Operation,
            crate::DiagnosticLaneV1(0),
        )
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let budget = WorkBudget::with_request(
        limits(),
        crate::RequestBudgetV1::for_duration(Duration::from_secs(30))
            .with_diagnostic_context(diagnostic),
    );
    let request = budget.request().clone();
    budget.record_diagnostic_usage_delta(
        WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        },
        &span,
    );
    check_eq!(budget.usage(), WorkUsage::default());
    check!(budget.request().same_request_as(&request));
    span.finish(crate::DiagnosticTerminalV1::Ok)
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let events = recorded(&sink)?;
    check_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.kind,
                crate::DiagnosticEventKindV1::Health {
                    code: crate::DiagnosticHealthCodeV1::CounterOverflow
                }
            ))
            .count(),
        1
    );
    check!(events.iter().all(|event| !matches!(
        event.kind,
        crate::DiagnosticEventKindV1::Counter { key, .. } if WORK_KEYS.contains(&key)
    )));
    Ok(())
}

#[test]
fn diagnostic_usage_delta_off_does_not_emit_or_read_clock() -> Result<(), IndexError> {
    let (diagnostic, clock, sink) = trace();
    let span = diagnostic
        .enter_root(
            crate::DiagnosticRootKindV1::Operation,
            crate::DiagnosticLaneV1(0),
        )
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let mut budget = WorkBudget::with_request(
        limits(),
        crate::RequestBudgetV1::for_duration(Duration::from_secs(30)),
    );
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let accepted = budget.usage();
    let request = budget.request().clone();
    let events_before = recorded(&sink)?.len();
    let clock_before = clock.0.load(Ordering::Relaxed);
    budget.record_diagnostic_usage_delta(WorkUsage::default(), &span);
    check_eq!(recorded(&sink)?.len(), events_before);
    check_eq!(clock.0.load(Ordering::Relaxed), clock_before);
    check_eq!(budget.usage(), accepted);
    check!(budget.request().same_request_as(&request));
    span.finish(crate::DiagnosticTerminalV1::Ok)
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    check!(recorded(&sink)?.iter().all(|event| !matches!(
        event.kind,
        crate::DiagnosticEventKindV1::Counter { key, .. } if WORK_KEYS.contains(&key)
    )));
    Ok(())
}

#[test]
fn diagnostic_usage_delta_preserves_foreign_span_refusal() -> Result<(), IndexError> {
    let (diagnostic, _clock, sink) = trace();
    let (foreign, _foreign_clock, foreign_sink) = trace();
    let span = foreign
        .enter_root(
            crate::DiagnosticRootKindV1::Operation,
            crate::DiagnosticLaneV1(0),
        )
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let budget = WorkBudget::with_request(
        limits(),
        crate::RequestBudgetV1::for_duration(Duration::from_secs(30))
            .with_diagnostic_context(diagnostic),
    );
    let request = budget.request().clone();
    budget.record_diagnostic_usage_delta(WorkUsage::default(), &span);
    check_eq!(budget.usage(), WorkUsage::default());
    check!(budget.request().same_request_as(&request));
    span.finish(crate::DiagnosticTerminalV1::Ok)
        .map_err(|error| IndexError::BackendViolation(error.to_string()))?;
    let events = recorded(&sink)?;
    check!(events.iter().any(|event| matches!(
        event.kind,
        crate::DiagnosticEventKindV1::Health {
            code: crate::DiagnosticHealthCodeV1::ForeignCaptureSpan
        }
    )));
    check!(events.iter().all(|event| !matches!(
        event.kind,
        crate::DiagnosticEventKindV1::Counter { key, .. } if WORK_KEYS.contains(&key)
    )));
    check!(recorded(&foreign_sink)?.iter().all(|event| !matches!(
        event.kind,
        crate::DiagnosticEventKindV1::Counter { key, .. } if WORK_KEYS.contains(&key)
    )));
    Ok(())
}
