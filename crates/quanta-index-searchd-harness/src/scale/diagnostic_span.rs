//! Trace fault handling shared by scale benchmark observers.

use quanta_index_core::{
    DiagnosticContextV1, DiagnosticHealthCodeV1, DiagnosticSpanGuardV1, DiagnosticTraceErrorV1,
};

// Sampling and measurement continue when trace spans fail, while the capture
// records the typed fault for the final trace verdict.
pub(crate) fn optional_diagnostic_span(
    context: &DiagnosticContextV1,
    result: Result<DiagnosticSpanGuardV1, DiagnosticTraceErrorV1>,
) -> Option<DiagnosticSpanGuardV1> {
    match result {
        Ok(span) => Some(span),
        Err(error) => {
            let code = match error {
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
            };
            context.health(code);
            None
        }
    }
}
