//! Diagnostic spans shared by native lexical operations.

use quanta_index_core::{
    DiagnosticContextV1, DiagnosticHealthCodeV1, DiagnosticLaneV1, DiagnosticSpanGuardV1,
    DiagnosticStageV1, DiagnosticTerminalV1,
};

pub(crate) fn enter_span(
    context: &DiagnosticContextV1,
    stage: DiagnosticStageV1,
) -> Option<DiagnosticSpanGuardV1> {
    enter_span_on_lane(context, stage, DiagnosticLaneV1(2))
}

pub(crate) fn enter_span_on_lane(
    context: &DiagnosticContextV1,
    stage: DiagnosticStageV1,
    lane: DiagnosticLaneV1,
) -> Option<DiagnosticSpanGuardV1> {
    context.enter(stage, lane).map_or_else(
        |trace_error| {
            context.health(match trace_error {
                quanta_index_core::DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                    DiagnosticHealthCodeV1::CounterOverflow
                }
                quanta_index_core::DiagnosticTraceErrorV1::ClockRegressed => {
                    DiagnosticHealthCodeV1::ClockRegressed
                }
                quanta_index_core::DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                    DiagnosticHealthCodeV1::ForeignCaptureSpan
                }
                quanta_index_core::DiagnosticTraceErrorV1::MissingParentSpan => {
                    DiagnosticHealthCodeV1::MissingParentSpan
                }
            });
            None
        },
        Some,
    )
}

/// Borrow the open span for bounded counters before its terminal event.
pub(crate) fn observe_result_with_span_on_lane<T, E>(
    diagnostic: Option<&DiagnosticContextV1>,
    stage: DiagnosticStageV1,
    lane: DiagnosticLaneV1,
    operation: impl FnOnce(Option<&DiagnosticContextV1>, Option<&DiagnosticSpanGuardV1>) -> Result<T, E>,
) -> Result<T, E> {
    let span = diagnostic.and_then(|context| enter_span_on_lane(context, stage, lane));
    let child = diagnostic
        .zip(span.as_ref())
        .map(|(context, span)| context.child_context(span));
    let result = operation(child.as_ref(), span.as_ref());
    if let Some(span) = span {
        let outcome = DiagnosticTerminalV1::from_result(&result);
        let _finished = span.finish(outcome);
    }
    result
}
