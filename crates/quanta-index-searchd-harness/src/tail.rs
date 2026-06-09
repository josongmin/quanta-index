//! Latency-tail rail (J7Q-04): route-aware p50/p95/p99 budgets + measured tails.
//!
//! This module owns three things, all checked-in and route-local:
//!
//! - a **route-tail budget manifest** — one [`RouteTailBudget`] per
//!   [`RouteFamily`], declaring a `p50` / `p95` / `p99` millisecond budget. The
//!   budgets are the SSOT a reviewer reads to answer "what is an acceptable tail
//!   for the structural route", reviewable by eye against the emitted
//!   `route_budgets.json`. There is deliberately NO single global threshold;
//!   each route family carries its own (per the ticket No-Go);
//! - a **measured warm tail** — [`measure_route_tails`] boots one warm
//!   [`E2eRuntime`] (every fixture, one sealed+active generation), then for each
//!   route family runs its representative scenario [`TAIL_SAMPLES`] times,
//!   collecting per-query wall-times into a sorted sample and reading p50/p95/p99
//!   off it. Each row carries route-local diagnostic metadata (sample count,
//!   candidate count, scenario id) sufficient to explain a tail cliff;
//! - a **two-layer verdict** that keeps the blocking and advisory signals
//!   separate. The BLOCKING layer is correctness: every measured route must
//!   golden-validate (correct shape / count / typed-error) before it is timed —
//!   a route that errors or returns the wrong result is a rail failure, never a
//!   fast-but-wrong "pass". The latency layer is, in this first increment,
//!   ADVISORY on this host: the macbook wall-clock is variance-prone, so p50 is
//!   compared as the blocking-candidate signal and p95/p99 as explicit advisory
//!   thresholds, with the canonical blocking enforcement owned by the Linux perf
//!   runner. This matches the ticket's "document route-family tail budgets before
//!   turning any new threshold hard".
//!
//! Fail-closed posture: a route whose representative query rejects, errors, or
//! returns the wrong golden shape is a rail error (typed error -> `Err`), never a
//! zero-latency pass over a broken route.

use std::time::Instant;

use anyhow::Result as AnyResult;
use serde_json::{Value, json};

use crate::artifact::RouteFamily;
use crate::bench_support::{
    QueryOutcome, ScenarioTruthMode, prepare_warm_runtime, run_scenario_query,
    validate_scenario_outcome,
};
use crate::harness::E2eRuntime;
use crate::scenarios::{DslBenchScenario, SCENARIOS};

/// Per-query wall-time samples collected per route for percentile estimation.
///
/// Sized so p95/p99 land on distinct ranks (a 64-sample slice puts p99 at rank
/// 62) while the whole sweep stays sub-second on the tiny warm fixtures.
pub const TAIL_SAMPLES: usize = 64;

/// A route family's declared tail budget, in milliseconds.
///
/// `p50_ms` is the blocking-candidate signal (the most stable percentile);
/// `p95_ms` / `p99_ms` are explicit advisory thresholds. Budgets are intentionally
/// generous for a warm developer machine — they catch gross route-local
/// regressions without flaking on host variance; the canonical tight enforcement
/// is the Linux perf runner's.
#[derive(Clone, Copy, Debug)]
pub struct RouteTailBudget {
    pub route: RouteFamily,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
}

// Per-route budgets as named consts so [`budget_for`] resolves each through a
// total match (no fallible/indexed lookup) while [`ROUTE_TAIL_BUDGETS`] stays the
// single ordered source the manifest artifact serializes. Structural carries a
// wider budget (tree-pattern matching is heavier) and Adversarial a tighter one
// (its representative query must reject fast, not serve), so the manifest is
// genuinely route-aware rather than a global cap.
const BUDGET_LEXICAL: RouteTailBudget = RouteTailBudget {
    route: RouteFamily::Lexical,
    p50_ms: 30.0,
    p95_ms: 80.0,
    p99_ms: 150.0,
};
const BUDGET_HISTORY: RouteTailBudget = RouteTailBudget {
    route: RouteFamily::History,
    p50_ms: 30.0,
    p95_ms: 80.0,
    p99_ms: 150.0,
};
const BUDGET_RUNTIME_CATALOG: RouteTailBudget = RouteTailBudget {
    route: RouteFamily::RuntimeCatalog,
    p50_ms: 30.0,
    p95_ms: 80.0,
    p99_ms: 150.0,
};
const BUDGET_STRUCTURAL: RouteTailBudget = RouteTailBudget {
    route: RouteFamily::Structural,
    p50_ms: 40.0,
    p95_ms: 100.0,
    p99_ms: 200.0,
};
const BUDGET_ADVERSARIAL: RouteTailBudget = RouteTailBudget {
    route: RouteFamily::Adversarial,
    p50_ms: 20.0,
    p95_ms: 50.0,
    p99_ms: 100.0,
};

/// The checked-in route-tail budget manifest: one row per [`RouteFamily`], in
/// declaration order.
pub const ROUTE_TAIL_BUDGETS: &[RouteTailBudget] = &[
    BUDGET_LEXICAL,
    BUDGET_HISTORY,
    BUDGET_RUNTIME_CATALOG,
    BUDGET_STRUCTURAL,
    BUDGET_ADVERSARIAL,
];

/// Look up the declared budget for a route family.
///
/// Total by construction: the match is exhaustive over [`RouteFamily`], so every
/// route resolves to its declared budget without a fallible lookup.
#[must_use]
pub fn budget_for(route: RouteFamily) -> RouteTailBudget {
    match route {
        RouteFamily::Lexical => BUDGET_LEXICAL,
        RouteFamily::History => BUDGET_HISTORY,
        RouteFamily::RuntimeCatalog => BUDGET_RUNTIME_CATALOG,
        RouteFamily::Structural => BUDGET_STRUCTURAL,
        RouteFamily::Adversarial => BUDGET_ADVERSARIAL,
    }
}

// ---------------------------------------------------------------------------
// Percentiles (nearest-rank over an ascending-sorted sample).
// ---------------------------------------------------------------------------

/// Nearest-rank percentile of an ascending-sorted latency slice.
///
/// `q` is a percentile in `0..=100`; the index is `floor(q * (n - 1) / 100)`,
/// which is always in range for a non-empty slice. Saturating index math keeps
/// it total; `get` keeps the read panic-free.
#[expect(
    clippy::integer_division,
    reason = "nearest-rank index is an intentional floor over a small (<= TAIL_SAMPLES) sorted slice; q <= 100 and n >= 1 keep the index in range"
)]
fn percentile_ms(sorted_ms: &[f64], q: usize) -> f64 {
    let n = sorted_ms.len();
    if n == 0 {
        return 0.0;
    }
    let idx = q.saturating_mul(n.saturating_sub(1)) / 100;
    sorted_ms.get(idx).copied().unwrap_or(0.0)
}

/// Convert an elapsed `Instant` span to milliseconds.
fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

// ---------------------------------------------------------------------------
// Measurement.
// ---------------------------------------------------------------------------

/// Measured tail for one route family.
#[derive(Clone, Debug)]
pub struct RouteTailMeasurement {
    pub route: RouteFamily,
    pub scenario_id: &'static str,
    pub sample_count: usize,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    /// Result count the representative query returned (route-local diagnostic),
    /// or `None` for a typed-error (reject) route.
    pub result_count: Option<u64>,
}

/// First scenario carrying the given route family.
///
/// Returns `None` when no scenario declares this route — surfaced upstream as a
/// rail error rather than silently dropping a budgeted route.
fn representative_scenario(route: RouteFamily) -> Option<&'static DslBenchScenario> {
    SCENARIOS.iter().find(|scenario| scenario.route_family == route)
}

/// Boot one warm runtime and measure each budgeted route's tail.
///
/// Fail-closed: a route with no representative scenario, or whose representative
/// query does not golden-validate under the shared warm fixture, is a rail error.
pub fn measure_route_tails(rt: &mut E2eRuntime) -> AnyResult<Vec<RouteTailMeasurement>> {
    let mut out: Vec<RouteTailMeasurement> = Vec::with_capacity(ROUTE_TAIL_BUDGETS.len());
    for budget in ROUTE_TAIL_BUDGETS {
        let Some(scenario) = representative_scenario(budget.route) else {
            return Err(anyhow::anyhow!(
                "tail: no representative scenario for route `{}`",
                budget.route.as_str()
            ));
        };
        // Correctness is the blocking gate: validate the route serves the right
        // golden result BEFORE timing, so a fast-but-wrong route cannot pass.
        let truth: QueryOutcome = run_scenario_query(rt, scenario);
        validate_scenario_outcome(scenario, ScenarioTruthMode::SharedWarmFixture, &truth)
            .map_err(|err| {
                anyhow::anyhow!(
                    "tail: route `{}` scenario `{}` failed golden validation before timing: {err}",
                    budget.route.as_str(),
                    scenario.id
                )
            })?;
        let mut samples_ms: Vec<f64> = Vec::with_capacity(TAIL_SAMPLES);
        for _ in 0..TAIL_SAMPLES {
            let started = Instant::now();
            let _outcome = run_scenario_query(rt, scenario);
            samples_ms.push(elapsed_ms(started));
        }
        samples_ms.sort_by(f64::total_cmp);
        out.push(RouteTailMeasurement {
            route: budget.route,
            scenario_id: scenario.id,
            sample_count: samples_ms.len(),
            p50_ms: percentile_ms(&samples_ms, 50),
            p95_ms: percentile_ms(&samples_ms, 95),
            p99_ms: percentile_ms(&samples_ms, 99),
            result_count: truth.result_count,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Verdicts (correctness = blocking; latency = advisory this increment).
// ---------------------------------------------------------------------------

/// Whether one measured percentile is within its declared budget.
fn within(measured_ms: f64, budget_ms: f64) -> bool {
    measured_ms <= budget_ms
}

/// The full tail report: one row per route plus the rail verdict.
#[derive(Clone, Debug)]
pub struct TailReport {
    pub measurements: Vec<RouteTailMeasurement>,
    /// `true` once every budgeted route golden-validated and was measured. The
    /// latency budgets are advisory in this increment, so they do not flip this.
    pub passed: bool,
}

/// Run the full tail rail against a freshly booted warm runtime.
pub fn run_tail_report() -> AnyResult<TailReport> {
    let mut rt = prepare_warm_runtime()?;
    let measurements = measure_route_tails(&mut rt)?;
    let passed = measurements.len() == ROUTE_TAIL_BUDGETS.len();
    Ok(TailReport {
        measurements,
        passed,
    })
}

// ---------------------------------------------------------------------------
// Artifact emission (manual json!, mirrors the relevance/scale rails).
// ---------------------------------------------------------------------------

fn budget_json(budget: &RouteTailBudget) -> Value {
    json!({
        "route": budget.route.as_str(),
        "p50_ms": budget.p50_ms,
        "p95_ms": budget.p95_ms,
        "p99_ms": budget.p99_ms,
        "p50_class": "blocking-candidate",
        "p95_class": "advisory",
        "p99_class": "advisory",
    })
}

/// The checked-in route-tail budget manifest, serialized.
#[must_use]
pub fn route_budgets_json() -> Value {
    json!({
        "schema_version": 1,
        "dimension": "tail",
        "host_class": "macbook-advisory",
        "policy_note": "per-route budgets are route-aware (no global threshold); p50 is the blocking-candidate signal and p95/p99 are explicit advisory thresholds. This increment documents the budgets and records verdicts as advisory on this host; the canonical blocking enforcement is the Linux perf runner's.",
        "routes": ROUTE_TAIL_BUDGETS.iter().map(budget_json).collect::<Vec<_>>(),
    })
}

fn measurement_json(measurement: &RouteTailMeasurement) -> Value {
    let budget = budget_for(measurement.route);
    json!({
        "route": measurement.route.as_str(),
        "scenario_id": measurement.scenario_id,
        "sample_count": measurement.sample_count,
        "p50_ms": measurement.p50_ms,
        "p95_ms": measurement.p95_ms,
        "p99_ms": measurement.p99_ms,
        "result_count": measurement.result_count,
        "budget": {
            "p50_ms": budget.p50_ms,
            "p95_ms": budget.p95_ms,
            "p99_ms": budget.p99_ms,
        },
        // Advisory this increment: recorded per percentile, never flips rail pass.
        "advisory_within_budget": {
            "p50": within(measurement.p50_ms, budget.p50_ms),
            "p95": within(measurement.p95_ms, budget.p95_ms),
            "p99": within(measurement.p99_ms, budget.p99_ms),
        },
    })
}

/// Build the tail summary value over every measured route.
#[must_use]
pub fn summary_json(report: &TailReport, git_rev: &str) -> Value {
    json!({
        "schema_version": 1,
        "dimension": "tail",
        "git_rev": git_rev,
        "host_class": "macbook-advisory",
        "passed": report.passed,
        "blocking_signal": "route correctness (golden-validated before timing); latency budgets are advisory on this host this increment",
        "routes": report.measurements.iter().map(measurement_json).collect::<Vec<_>>(),
    })
}

/// Write the two canonical tail artifacts under `dir`:
/// `route_budgets.json` and `summary.json`.
pub fn write_artifacts(report: &TailReport, dir: &std::path::Path, git_rev: &str) -> AnyResult<()> {
    crate::artifact::write_json_pretty(&dir.join("route_budgets.json"), &route_budgets_json())?;
    crate::artifact::write_json_pretty(&dir.join("summary.json"), &summary_json(report, git_rev))?;
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    clippy::float_cmp,
    reason = "tests index JSON/slices whose shape this module constructs (out-of-range is a legit test failure), and assert exact integer-valued percentile picks (nearest-rank returns a stored slice element, no arithmetic, so exact f64 equality is correct)"
)]
mod tests {
    //! Budget-manifest + percentile invariants.
    //!
    //! These never boot a runtime: they pin the route-aware budget table and the
    //! pure percentile reader. The seeded end-to-end measurement is exercised by
    //! the `tail_matrix` rail under the daemon lane.
    use super::*;

    #[test]
    fn budget_manifest_has_one_row_per_route() {
        for route in [
            RouteFamily::Lexical,
            RouteFamily::History,
            RouteFamily::RuntimeCatalog,
            RouteFamily::Structural,
            RouteFamily::Adversarial,
        ] {
            let budget = budget_for(route);
            assert_eq!(budget.route, route, "budget routed to wrong family");
            assert!(
                budget.p50_ms <= budget.p95_ms && budget.p95_ms <= budget.p99_ms,
                "route {} budgets must be monotonic p50<=p95<=p99",
                route.as_str()
            );
        }
        assert_eq!(ROUTE_TAIL_BUDGETS.len(), 5, "exactly five route budgets");
    }

    #[test]
    fn percentile_is_nearest_rank_and_in_range() {
        let sorted: Vec<f64> = (0..100).map(f64::from).collect();
        assert_eq!(percentile_ms(&sorted, 0), 0.0);
        assert_eq!(percentile_ms(&sorted, 50), 49.0, "floor(50*99/100)=49");
        assert_eq!(percentile_ms(&sorted, 99), 98.0, "floor(99*99/100)=98");
        assert_eq!(percentile_ms(&sorted, 100), 99.0, "top rank");
    }

    #[test]
    fn percentile_empty_is_zero() {
        assert_eq!(percentile_ms(&[], 95), 0.0);
    }

    #[test]
    fn route_budgets_json_is_well_formed() {
        let value = route_budgets_json();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["dimension"], "tail");
        let routes = value["routes"].as_array().expect("routes is an array");
        assert_eq!(routes.len(), 5);
        for row in routes {
            assert!(row["route"].is_string());
            assert!(row["p50_ms"].is_number());
            assert_eq!(row["p50_class"], "blocking-candidate");
            assert_eq!(row["p95_class"], "advisory");
        }
    }

    #[test]
    fn summary_json_records_passed_and_per_route_advisory() {
        let report = TailReport {
            measurements: vec![RouteTailMeasurement {
                route: RouteFamily::Lexical,
                scenario_id: "test.lexical",
                sample_count: 64,
                p50_ms: 1.0,
                p95_ms: 2.0,
                p99_ms: 3.0,
                result_count: Some(4),
            }],
            passed: true,
        };
        let value = summary_json(&report, "deadbeef");
        assert_eq!(value["dimension"], "tail");
        assert_eq!(value["git_rev"], "deadbeef");
        assert_eq!(value["passed"], true);
        let routes = value["routes"].as_array().expect("routes array");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0]["route"], "lexical");
        assert_eq!(routes[0]["advisory_within_budget"]["p50"], true);
    }
}
