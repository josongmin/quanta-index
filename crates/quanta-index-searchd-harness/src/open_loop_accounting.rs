//! Protocol-neutral open-loop completion accounting shared by query transports.
//! Endpoint adapters own response binding and typed error classification.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result as AnyResult, ensure};
use serde_json::{Value, json};

use crate::artifact::LatencySummary;

const MIN_DELIVERY_RATIO: f64 = 0.95;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    Served { result_count: u64 },
    InvalidResult,
    Overloaded { code: String },
    TypedError { code: String },
    Timeout,
    TransportError { kind: String },
    QueueDeadline,
}

#[derive(Clone, Debug)]
pub struct Completion {
    pub outcome: Outcome,
    pub elapsed_ms: Option<f64>,
    pub dispatch_lag_ms: f64,
}

#[derive(Clone, Debug)]
pub struct LoadPoint {
    pub target_qps: u32,
    pub offered: u64,
    pub served: u64,
    pub typed_errors: u64,
    pub unexpected_typed_errors: u64,
    pub timeouts: u64,
    pub transport_errors: u64,
    pub transport_error_kinds: BTreeMap<String, u64>,
    pub invalid_results: u64,
    pub dropped_queue_full: u64,
    pub dropped_scheduler_late: u64,
    pub dropped_deadline: u64,
    pub offered_qps: f64,
    pub achieved_qps: f64,
    pub drain_secs: f64,
    /// Scheduled arrival to response; includes scheduler lag, queue wait and IPC.
    pub latency: Option<LatencySummary>,
    pub max_dispatch_lag_ms: f64,
    pub last_result_count: Option<u64>,
    pub error_codes: Vec<String>,
    pub saturated: bool,
}

impl LoadPoint {
    pub fn detail(&self) -> Value {
        json!({
            "target_qps": self.target_qps,
            "offered": self.offered,
            "served": self.served,
            "typed_errors": self.typed_errors,
            "unexpected_typed_errors": self.unexpected_typed_errors,
            "timeouts": self.timeouts,
            "transport_errors": self.transport_errors,
            "transport_error_kinds": self.transport_error_kinds,
            "invalid_results": self.invalid_results,
            "dropped_queue_full": self.dropped_queue_full,
            "dropped_scheduler_late": self.dropped_scheduler_late,
            "dropped_deadline": self.dropped_deadline,
            "offered_qps": self.offered_qps,
            "achieved_qps": self.achieved_qps,
            "drain_secs": self.drain_secs,
            "latency": self.latency,
            "max_dispatch_lag_ms": self.max_dispatch_lag_ms,
            "error_codes": self.error_codes,
            "saturated": self.saturated,
        })
    }
}

fn increment_count(counter: &mut u64, label: &str) -> AnyResult<()> {
    *counter = counter
        .checked_add(1)
        .with_context(|| format!("{label} overflow"))?;
    Ok(())
}

pub fn summarize(
    rate: u32,
    duration: Duration,
    offered: u64,
    dropped_queue_full: u64,
    dropped_scheduler_late: u64,
    completions: Vec<Completion>,
    drain_secs: f64,
) -> AnyResult<LoadPoint> {
    ensure!(offered > 0, "open-loop load point offered no requests");
    ensure!(
        drain_secs.is_finite() && drain_secs > 0.0,
        "measurement window must be finite and positive"
    );
    let accounted = u64::try_from(completions.len())?
        .checked_add(dropped_queue_full)
        .and_then(|count| count.checked_add(dropped_scheduler_late))
        .context("offered request accounting overflow")?;
    ensure!(
        accounted == offered,
        "offered requests were lost before accounting"
    );
    let mut served = 0_u64;
    let mut typed_errors = 0_u64;
    let mut unexpected_typed_errors = 0_u64;
    let mut timeouts = 0_u64;
    let mut transport_errors = 0_u64;
    let mut transport_error_kinds = BTreeMap::<String, u64>::new();
    let mut invalid_results = 0_u64;
    let mut dropped_deadline = 0_u64;
    let mut last_result_count = None;
    let mut codes = Vec::new();
    let mut elapsed = Vec::new();
    let mut max_dispatch_lag_ms = 0.0_f64;
    for completion in completions {
        ensure!(
            completion.dispatch_lag_ms.is_finite() && completion.dispatch_lag_ms >= 0.0,
            "dispatch lag must be finite and nonnegative"
        );
        max_dispatch_lag_ms = max_dispatch_lag_ms.max(completion.dispatch_lag_ms);
        if let Some(ms) = completion.elapsed_ms {
            ensure!(
                ms.is_finite() && ms >= 0.0,
                "completion latency must be finite and nonnegative"
            );
            elapsed.push(ms);
        }
        match completion.outcome {
            Outcome::Served { result_count } => {
                increment_count(&mut served, "served count")?;
                last_result_count = Some(result_count);
            }
            Outcome::Overloaded { code } => {
                increment_count(&mut typed_errors, "typed error count")?;
                codes.push(code);
            }
            Outcome::TypedError { code } => {
                increment_count(&mut typed_errors, "typed error count")?;
                increment_count(&mut unexpected_typed_errors, "unexpected typed error count")?;
                codes.push(code);
            }
            Outcome::Timeout => increment_count(&mut timeouts, "timeout count")?,
            Outcome::TransportError { kind } => {
                increment_count(&mut transport_errors, "transport error count")?;
                increment_count(
                    transport_error_kinds.entry(kind).or_default(),
                    "transport error kind count",
                )?;
            }
            Outcome::InvalidResult => {
                increment_count(&mut invalid_results, "invalid result count")?;
            }
            Outcome::QueueDeadline => {
                increment_count(&mut dropped_deadline, "queue deadline count")?;
            }
        }
    }
    codes.sort();
    codes.dedup();
    let offered_qps = f64::from(u32::try_from(offered)?) / duration.as_secs_f64();
    let achieved_qps = f64::from(u32::try_from(served)?) / drain_secs.max(duration.as_secs_f64());
    ensure!(
        offered_qps.is_finite() && offered_qps > 0.0 && achieved_qps.is_finite(),
        "open-loop throughput is not finite"
    );
    let saturated = dropped_queue_full > 0
        || dropped_scheduler_late > 0
        || dropped_deadline > 0
        || typed_errors > 0
        || timeouts > 0
        || transport_errors > 0
        || invalid_results > 0
        || achieved_qps / offered_qps < MIN_DELIVERY_RATIO;
    Ok(LoadPoint {
        target_qps: rate,
        offered,
        served,
        typed_errors,
        unexpected_typed_errors,
        timeouts,
        transport_errors,
        transport_error_kinds,
        invalid_results,
        dropped_queue_full,
        dropped_scheduler_late,
        dropped_deadline,
        offered_qps,
        achieved_qps,
        drain_secs,
        latency: LatencySummary::from_samples_ms(&elapsed),
        max_dispatch_lag_ms,
        last_result_count,
        error_codes: codes,
        saturated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonfinite_completed_samples_cannot_enter_latency_summary() {
        assert!(
            summarize(
                1,
                Duration::from_secs(1),
                1,
                0,
                0,
                vec![Completion {
                    outcome: Outcome::Served { result_count: 1 },
                    elapsed_ms: Some(f64::NAN),
                    dispatch_lag_ms: 0.0,
                }],
                1.0,
            )
            .is_err()
        );
        assert!(
            summarize(
                1,
                Duration::from_secs(1),
                1,
                0,
                0,
                vec![Completion {
                    outcome: Outcome::Served { result_count: 1 },
                    elapsed_ms: Some(1.0),
                    dispatch_lag_ms: f64::INFINITY,
                }],
                1.0,
            )
            .is_err()
        );
    }

    #[test]
    fn slow_answers_keep_the_original_arrival_in_the_tail() -> AnyResult<()> {
        let point = summarize(
            10,
            Duration::from_secs(1),
            2,
            0,
            0,
            vec![
                Completion {
                    outcome: Outcome::Served { result_count: 2 },
                    elapsed_ms: Some(5.0),
                    dispatch_lag_ms: 0.0,
                },
                Completion {
                    outcome: Outcome::Served { result_count: 2 },
                    elapsed_ms: Some(700.0),
                    dispatch_lag_ms: 600.0,
                },
            ],
            1.0,
        )?;
        assert_eq!(
            point.latency.context("latency missing")?.p99_ms.to_bits(),
            700.0_f64.to_bits()
        );
        assert_eq!(point.max_dispatch_lag_ms.to_bits(), 600.0_f64.to_bits());
        assert!(!point.saturated);
        Ok(())
    }

    #[test]
    fn every_offered_request_must_be_accounted() {
        assert!(
            summarize(
                10,
                Duration::from_secs(1),
                2,
                0,
                0,
                vec![Completion {
                    outcome: Outcome::Timeout,
                    elapsed_ms: Some(10.0),
                    dispatch_lag_ms: 0.0
                },],
                1.0
            )
            .is_err()
        );
    }

    #[test]
    fn overload_is_saturation_but_other_typed_errors_are_correctness_failures() -> AnyResult<()> {
        let point = summarize(
            3,
            Duration::from_secs(1),
            3,
            1,
            0,
            vec![
                Completion {
                    outcome: Outcome::Overloaded {
                        code: "SERVER_OVERLOADED".to_owned(),
                    },
                    elapsed_ms: Some(5.0),
                    dispatch_lag_ms: 0.0,
                },
                Completion {
                    outcome: Outcome::TypedError {
                        code: "CATALOG_BUSY".to_owned(),
                    },
                    elapsed_ms: Some(6.0),
                    dispatch_lag_ms: 0.0,
                },
            ],
            1.0,
        )?;
        assert_eq!(point.typed_errors, 2);
        assert_eq!(point.unexpected_typed_errors, 1);
        assert_eq!(point.dropped_queue_full, 1);
        assert!(point.saturated);
        Ok(())
    }
}
