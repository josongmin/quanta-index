//! Shared stage diagnostics without a backend dependency.

use quanta_index_core::{
    DiagnosticContextV1, DiagnosticLaneV1, DiagnosticSpanGuardV1, DiagnosticStageV1,
    DiagnosticTerminalV1,
};

pub(crate) fn begin_build_stage(
    diagnostic: Option<&DiagnosticContextV1>,
    stage: DiagnosticStageV1,
) -> Option<DiagnosticSpanGuardV1> {
    diagnostic.and_then(|context| {
        context.enter(stage, DiagnosticLaneV1(3)).map_or_else(
            |trace_error| {
                context.health(match trace_error {
                    quanta_index_core::DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                        quanta_index_core::DiagnosticHealthCodeV1::CounterOverflow
                    }
                    quanta_index_core::DiagnosticTraceErrorV1::ClockRegressed => {
                        quanta_index_core::DiagnosticHealthCodeV1::ClockRegressed
                    }
                    quanta_index_core::DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                        quanta_index_core::DiagnosticHealthCodeV1::ForeignCaptureSpan
                    }
                    quanta_index_core::DiagnosticTraceErrorV1::MissingParentSpan => {
                        quanta_index_core::DiagnosticHealthCodeV1::MissingParentSpan
                    }
                });
                None
            },
            Some,
        )
    })
}

pub(crate) fn finish_build_stage(span: Option<DiagnosticSpanGuardV1>, success: bool) {
    if let Some(span) = span {
        let outcome = if success {
            DiagnosticTerminalV1::Ok
        } else {
            DiagnosticTerminalV1::Error { code: 1 }
        };
        let _trace_completion = span.finish(outcome);
    }
}
