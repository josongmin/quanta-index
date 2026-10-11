//! Server-side monotonic timings attached to the query response that owns them.
//! These observations never drive ranking, admission, or deadline decisions.

use std::borrow::Cow;
use std::time::{Duration, Instant};

use quanta_index_contract::{QueryStageKindV1, QueryStageTimingV1};
use quanta_index_core::{
    DiagnosticContextV1, DiagnosticLaneV1, DiagnosticStageV1, DiagnosticTerminalV1, RequestBudgetV1,
};

/// Rebind only diagnostics. A cloned budget shares its cancellation, deadline,
/// and usage ledgers with the request. The disabled path borrows the original.
pub(super) fn budget_under_diagnostic<'a>(
    budget: &'a RequestBudgetV1,
    child: Option<&DiagnosticContextV1>,
) -> Cow<'a, RequestBudgetV1> {
    child.map_or_else(
        || Cow::Borrowed(budget),
        |child| Cow::Owned(budget.clone().with_diagnostic_context(child.clone())),
    )
}

/// Pass the exact span child to work delegated by a stage. No thread-local
/// context or timestamp inference is used for ancestry.
pub(super) fn observe_query_result_scoped<T, E>(
    diagnostic: Option<&DiagnosticContextV1>,
    stage: DiagnosticStageV1,
    action: impl FnOnce(Option<&DiagnosticContextV1>) -> Result<T, E>,
) -> Result<T, E> {
    let span = diagnostic.and_then(|context| {
        context.enter(stage, DiagnosticLaneV1(1)).map_or_else(
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
    });
    let child = diagnostic
        .zip(span.as_ref())
        .map(|(context, span)| context.child_context(span));
    let result = action(child.as_ref());
    if let Some(span) = span {
        let outcome = DiagnosticTerminalV1::from_result(&result);
        let _trace_completion = span.finish(outcome);
    }
    result
}

/// Startup-bound stage observation policy. Operational metrics and request
/// deadlines remain active under both policies; neither policy affects ranking.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum QueryStageObservationPolicy {
    #[default]
    Enabled,
    Disabled,
}

impl QueryStageObservationPolicy {
    /// Reject aliases and empty values rather than changing observation silently.
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "enabled" => Ok(Self::Enabled),
            "disabled" => Ok(Self::Disabled),
            _ => Err("query stage observation must be enabled or disabled"),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    pub(super) fn start(self) -> Option<Instant> {
        match self {
            Self::Enabled => Some(Instant::now()),
            Self::Disabled => None,
        }
    }
}

/// Disabled observations allocate no stage storage and carry no invented zeros.
pub(super) struct StageTimings(Option<Vec<QueryStageTimingV1>>);

impl StageTimings {
    pub(super) fn new(policy: QueryStageObservationPolicy, capacity: usize) -> Self {
        Self(match policy {
            QueryStageObservationPolicy::Enabled => Some(Vec::with_capacity(capacity)),
            QueryStageObservationPolicy::Disabled => None,
        })
    }

    pub(super) fn push(&mut self, timing: Option<QueryStageTimingV1>) {
        if let (Some(stages), Some(timing)) = (&mut self.0, timing) {
            stages.push(timing);
        }
    }

    pub(super) fn record_elapsed(
        &mut self,
        stage: QueryStageKindV1,
        started: Option<Instant>,
        calls: u32,
        returned_candidates: Option<usize>,
    ) {
        if let (Some(stages), Some(started)) = (&mut self.0, started) {
            stages.push(measured(
                stage,
                started.elapsed(),
                calls,
                returned_candidates,
            ));
        }
    }

    pub(super) fn finish(self) -> Option<Vec<QueryStageTimingV1>> {
        self.0
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).map_or(u64::MAX, |value| value)
}

fn count(value: usize) -> u64 {
    u64::try_from(value).map_or(u64::MAX, |value| value)
}

pub(super) fn measured(
    stage: QueryStageKindV1,
    duration: Duration,
    calls: u32,
    returned_candidates: Option<usize>,
) -> QueryStageTimingV1 {
    QueryStageTimingV1 {
        stage,
        elapsed_ns: nanos(duration),
        calls,
        returned_candidates: returned_candidates.map(count),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        QueryStageObservationPolicy, StageTimings, budget_under_diagnostic,
        observe_query_result_scoped,
    };
    use quanta_index_contract::QueryStageKindV1;
    use quanta_index_core::{DiagnosticStageV1, RequestBudgetV1};

    #[test]
    fn lexical_observation_cbor_shape_fits_fixed_page_reserve() {
        use crate::query_dispatcher::response_budget::LEXICAL_STAGE_RESERVE_BYTES;
        use quanta_index_contract::QueryStageTimingV1;
        let stages = [
            QueryStageKindV1::LexicalPrepare,
            QueryStageKindV1::LexicalReadView,
            QueryStageKindV1::LexicalSearch,
            QueryStageKindV1::LexicalProject,
        ]
        .map(|stage| QueryStageTimingV1 {
            stage,
            elapsed_ns: u64::MAX,
            calls: u32::MAX,
            returned_candidates: Some(u64::MAX),
        });
        let bytes =
            quanta_index_ipc::cbor_payload_len(&Some(stages)).expect("bounded stage shape encodes");
        assert_eq!(LEXICAL_STAGE_RESERVE_BYTES, 512);
        assert!(
            bytes < LEXICAL_STAGE_RESERVE_BYTES,
            "lexical stage bytes exceed fixed reserve: {bytes}"
        );
    }

    #[test]
    fn disabled_stage_observation_has_no_clock_storage_or_measurement() {
        let policy = QueryStageObservationPolicy::Disabled;
        assert_eq!(policy.start(), None);
        let mut stages = StageTimings::new(policy, 7);
        stages.record_elapsed(QueryStageKindV1::HybridPrepare, policy.start(), 1, None);
        assert!(stages.finish().is_none());
        assert_eq!(policy.as_str(), "disabled");
        let budget = RequestBudgetV1::unbounded();
        let borrowed = budget_under_diagnostic(&budget, None);
        assert!(matches!(borrowed, std::borrow::Cow::Borrowed(_)));
        assert!(std::ptr::eq(borrowed.as_ref(), std::ptr::from_ref(&budget)));
        let result: Result<(), ()> =
            observe_query_result_scoped(None, DiagnosticStageV1::QueryPlan, |child| {
                assert!(child.is_none());
                Ok(())
            });
        assert!(result.is_ok());
    }

    #[test]
    fn enabled_stage_observation_retains_real_measurements() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::{Arc, Mutex};

        use quanta_index_core::{
            DiagnosticCaptureIdV1, DiagnosticClockV1, DiagnosticContextV1, DiagnosticEmitV1,
            DiagnosticEventKindV1, DiagnosticEventSinkV1, DiagnosticLaneV1, DiagnosticRootKindV1,
            DiagnosticTerminalV1,
        };

        #[derive(Debug)]
        struct Clock(AtomicU64);
        impl DiagnosticClockV1 for Clock {
            fn now_ns(&self) -> u64 {
                self.0.fetch_add(1, Ordering::Relaxed)
            }
        }
        #[derive(Debug, Default)]
        struct Sink(Mutex<Vec<DiagnosticEmitV1>>);
        impl DiagnosticEventSinkV1 for Sink {
            fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
                self.0.lock().expect("trace sink mutex").push(event);
                true
            }
        }

        let policy = QueryStageObservationPolicy::default();
        let started = policy.start();
        assert!(started.is_some());
        let mut stages = StageTimings::new(policy, 4);
        stages.record_elapsed(QueryStageKindV1::LexicalPrepare, started, 1, None);
        let stages = stages
            .finish()
            .expect("enabled observations allocate storage");
        assert_eq!(stages.len(), 1);
        assert_eq!(stages.first().map(|stage| stage.calls), Some(1));
        assert_eq!(policy.as_str(), "enabled");

        let sink = Arc::new(Sink::default());
        let context = DiagnosticContextV1::new(
            DiagnosticCaptureIdV1([51; 16]),
            Arc::new(Clock(AtomicU64::new(1))),
            sink.clone(),
        );
        let request = context
            .enter_root(DiagnosticRootKindV1::Request, DiagnosticLaneV1(1))
            .expect("request root");
        let request_context = context.child_context(&request);
        let budget = RequestBudgetV1::unbounded().with_diagnostic_context(request_context);
        let result: Result<(), ()> = observe_query_result_scoped(
            budget.diagnostic_context(),
            DiagnosticStageV1::QueryPlan,
            |plan_child| {
                let plan_budget = budget_under_diagnostic(&budget, plan_child);
                observe_query_result_scoped(
                    plan_budget.diagnostic_context(),
                    DiagnosticStageV1::QueryCandidate,
                    |candidate_child| {
                        let candidate_budget =
                            budget_under_diagnostic(plan_budget.as_ref(), candidate_child);
                        let verify = candidate_budget
                            .diagnostic_context()
                            .expect("candidate child context")
                            .enter(DiagnosticStageV1::QueryVerify, DiagnosticLaneV1(2))
                            .expect("verify child");
                        verify.finish(DiagnosticTerminalV1::Ok).expect("verify end");
                        Ok(())
                    },
                )
            },
        );
        assert!(result.is_ok());
        request
            .finish(DiagnosticTerminalV1::Ok)
            .expect("request end");

        let events = sink.0.lock().expect("trace sink mutex");
        let begin = |stage| {
            events.iter().find_map(|event| {
                if let DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id,
                    stage: observed,
                    ..
                } = event.kind
                {
                    return (observed == stage).then_some((span_id, parent_span_id));
                }
                None
            })
        };
        let (request_id, _) = begin(DiagnosticStageV1::Request).expect("request span");
        let (plan_id, plan_parent) = begin(DiagnosticStageV1::QueryPlan).expect("plan span");
        let (candidate_id, candidate_parent) =
            begin(DiagnosticStageV1::QueryCandidate).expect("candidate span");
        let (_, verify_parent) = begin(DiagnosticStageV1::QueryVerify).expect("verify span");
        assert_eq!(plan_parent, Some(request_id));
        assert_eq!(candidate_parent, Some(plan_id));
        assert_eq!(verify_parent, Some(candidate_id));
    }
}
