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

use crate::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, GitHeadV1, HostV1, LatencySummary,
    PhaseDurationsV1, ResourceUsageV1, RouteFamily, config_digest, corpus_digest,
    model_revision_of,
};
use crate::bench_support::{
    QueryOutcome, ScenarioTruthMode, bench_row, prepare_warm_runtime, run_scenario_query,
    validate_scenario_outcome, warm_fixture_corpus_files,
};
use crate::harness::E2eRuntime;
use crate::scenarios::{DslBenchScenario, SCENARIOS};

/// The artifact dimension this rail writes.
pub const DIMENSION: &str = "tail";

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

/// Look up the declared budget for a deterministic DSL-scenario route family.
///
/// Semantic and hybrid requests have independent request shapes and no DSL
/// scenario oracle. They must not silently inherit lexical thresholds.
#[must_use]
pub fn budget_for(route: RouteFamily) -> Option<RouteTailBudget> {
    Some(match route {
        RouteFamily::Lexical => BUDGET_LEXICAL,
        RouteFamily::History => BUDGET_HISTORY,
        RouteFamily::RuntimeCatalog => BUDGET_RUNTIME_CATALOG,
        RouteFamily::Structural => BUDGET_STRUCTURAL,
        RouteFamily::Adversarial => BUDGET_ADVERSARIAL,
        RouteFamily::Semantic
        | RouteFamily::Hybrid
        | RouteFamily::Symbol
        | RouteFamily::RepoMap => return None,
    })
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
    /// The artifact row for the representative scenario: the golden-validated
    /// outcome plus the measured latency.
    pub row: BenchRowV1,
}

/// First scenario carrying the given route family.
///
/// Returns `None` when no scenario declares this route — surfaced upstream as a
/// rail error rather than silently dropping a budgeted route.
fn representative_scenario(route: RouteFamily) -> Option<&'static DslBenchScenario> {
    SCENARIOS
        .iter()
        .find(|scenario| scenario.route_family == route)
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
        validate_scenario_outcome(scenario, ScenarioTruthMode::SharedWarmFixture, &truth).map_err(
            |err| {
                anyhow::anyhow!(
                    "tail: route `{}` scenario `{}` failed golden validation before timing: {err}",
                    budget.route.as_str(),
                    scenario.id
                )
            },
        )?;
        let mut samples_ms: Vec<f64> = Vec::with_capacity(TAIL_SAMPLES);
        for _ in 0..TAIL_SAMPLES {
            let started = Instant::now();
            let _outcome = run_scenario_query(rt, scenario);
            samples_ms.push(elapsed_ms(started));
        }
        let latency = LatencySummary::from_samples_ms(&samples_ms).ok_or_else(|| {
            anyhow::anyhow!(
                "tail: route `{}` collected no samples",
                budget.route.as_str()
            )
        })?;
        out.push(RouteTailMeasurement {
            route: budget.route,
            scenario_id: scenario.id,
            sample_count: samples_ms.len(),
            // `LatencySummary` is the artifact's percentile SSOT. Reusing it
            // keeps the diagnostic detail and emitted `BenchRowV1` identical.
            p50_ms: latency.p50_ms,
            p95_ms: latency.p95_ms,
            p99_ms: latency.p99_ms,
            result_count: truth.result_count,
            row: bench_row(scenario, truth, Some(latency)),
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
    /// The embedder the warm fixture was built under, for the provenance.
    pub model_revision: Option<String>,
}

/// Run the full tail rail against a freshly booted warm runtime.
pub fn run_tail_report() -> AnyResult<TailReport> {
    let mut rt = prepare_warm_runtime()?;
    let model_revision = model_revision_of(rt.embedder_profile());
    let measurements = measure_route_tails(&mut rt)?;
    let passed = measurements.len() == ROUTE_TAIL_BUDGETS.len();
    Ok(TailReport {
        measurements,
        passed,
        model_revision,
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
        "kind": "quanta-index-tail-budget-manifest",
        "manifest_schema_version": 1,
        "dimension": "tail",
        "host_class": "macbook-advisory",
        "policy_note": "per-route budgets are route-aware (no global threshold); p50 is the blocking-candidate signal and p95/p99 are explicit advisory thresholds. This increment documents the budgets and records verdicts as advisory on this host; the canonical blocking enforcement is the Linux perf runner's.",
        "routes": ROUTE_TAIL_BUDGETS.iter().map(budget_json).collect::<Vec<_>>(),
    })
}

fn measurement_json(measurement: &RouteTailMeasurement) -> AnyResult<Value> {
    let budget = budget_for(measurement.route).ok_or_else(|| {
        anyhow::anyhow!(
            "tail measurement has no DSL route budget: {:?}",
            measurement.route
        )
    })?;
    Ok(json!({
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
    }))
}

/// The dimension-specific detail of the tail artifact: every measured
/// route against its budget, and the rail's blocking signal.
pub fn detail_json(report: &TailReport) -> AnyResult<Value> {
    let routes = report
        .measurements
        .iter()
        .map(measurement_json)
        .collect::<AnyResult<Vec<_>>>()?;
    Ok(json!({
        "passed": report.passed,
        "blocking_signal": "route correctness (golden-validated before timing); latency budgets are advisory on this host this increment",
        "routes": routes,
    }))
}

/// The tail artifact: one `BenchArtifactV1` whose rows are the measured
/// routes' representative scenarios and whose provenance names the head,
/// the warm fixture corpus, the sample count and the embedder.
pub fn artifact(
    report: &TailReport,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<BenchArtifactV1> {
    Ok(BenchArtifactV1 {
        dimension: DIMENSION.to_string(),
        mode: BenchMode::Warm,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: corpus_digest(DIMENSION, &warm_fixture_corpus_files()),
            config_digest: config_digest(
                DIMENSION,
                &[
                    ("samples", TAIL_SAMPLES.to_string()),
                    ("route_count", ROUTE_TAIL_BUDGETS.len().to_string()),
                ],
            ),
            model_revision: report.model_revision.clone(),
        },
        host,
        resources: ResourceUsageV1::observe_self()?,
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows: report
            .measurements
            .iter()
            .map(|measurement| measurement.row.clone())
            .collect(),
        detail: detail_json(report)?,
    })
}

/// Write the two canonical tail artifacts under `dir`:
/// `route_budgets.json` (the declared manifest) and `summary.json` (the
/// `BenchArtifactV1`).
pub fn write_artifacts(
    report: &TailReport,
    dir: &std::path::Path,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<()> {
    crate::artifact::write_json_pretty(&dir.join("route_budgets.json"), &route_budgets_json())?;
    artifact(report, git_head, host)?.write_to(&dir.join("summary.json"))?;
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
            let budget = budget_for(route).expect("DSL route has a tail budget");
            assert_eq!(budget.route, route, "budget routed to wrong family");
            assert!(
                budget.p50_ms <= budget.p95_ms && budget.p95_ms <= budget.p99_ms,
                "route {} budgets must be monotonic p50<=p95<=p99",
                route.as_str()
            );
        }
        assert_eq!(ROUTE_TAIL_BUDGETS.len(), 5, "exactly five route budgets");
        assert!(budget_for(RouteFamily::Semantic).is_none());
        assert!(budget_for(RouteFamily::Hybrid).is_none());
        assert!(budget_for(RouteFamily::Symbol).is_none());
        assert!(budget_for(RouteFamily::RepoMap).is_none());
    }

    #[test]
    fn tail_detail_reuses_artifact_nearest_rank_percentiles() {
        let sample_count = u32::try_from(TAIL_SAMPLES).expect("bounded fixture sample count");
        let samples: Vec<f64> = (1..=sample_count).map(f64::from).collect();
        let summary =
            LatencySummary::from_samples_ms(&samples).expect("tail samples are non-empty");
        assert_eq!(summary.p50_ms, 32.0);
        assert_eq!(summary.p95_ms, 61.0);
        assert_eq!(summary.p99_ms, 64.0);
    }

    #[test]
    fn route_budgets_json_is_well_formed() {
        let value = route_budgets_json();
        assert_eq!(value["kind"], "quanta-index-tail-budget-manifest");
        assert_eq!(value["manifest_schema_version"], 1);
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

    fn sample_report() -> TailReport {
        TailReport {
            measurements: vec![RouteTailMeasurement {
                route: RouteFamily::Lexical,
                scenario_id: "test.lexical",
                sample_count: 64,
                p50_ms: 1.0,
                p95_ms: 2.0,
                p99_ms: 3.0,
                result_count: Some(4),
                row: BenchRowV1 {
                    scenario_id: "test.lexical".to_string(),
                    route_family: RouteFamily::Lexical,
                    syntax: crate::artifact::BenchSyntax::Native,
                    result_shape: crate::artifact::ResultShape::Candidates,
                    latency: LatencySummary::from_samples_ms(&[1.0, 2.0, 3.0]),
                    qps: None,
                    error_count: 0,
                    timeout_count: 0,
                    result_count: Some(4),
                    typed_error_code: None,
                    engine_touched: vec!["Lexical".to_string()],
                    early_stop_reason: None,
                },
            }],
            passed: true,
            model_revision: Some("model@rev:d16".to_string()),
        }
    }

    #[test]
    fn detail_json_records_passed_and_per_route_advisory() {
        let value = detail_json(&sample_report()).expect("valid route budget");
        assert_eq!(value["passed"], true);
        let routes = value["routes"].as_array().expect("routes array");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0]["route"], "lexical");
        assert_eq!(routes[0]["advisory_within_budget"]["p50"], true);
    }

    #[test]
    fn the_artifact_is_a_provenanced_envelope_over_the_route_rows() {
        let head = GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567").expect("a head");
        let host = HostV1 {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            cpu_count: 4,
            mem_bytes: 1 << 30,
            hostname_hash: "sha256:host".to_string(),
        };
        let artifact = artifact(&sample_report(), head, host).expect("the process is observable");
        let value = artifact.to_json().expect("serializes");
        assert_eq!(value["schema_version"], 2);
        assert_eq!(value["dimension"], "tail");
        assert_eq!(value["mode"], "warm");
        assert_eq!(value["concurrency"], 1);
        assert_eq!(
            value["provenance"]["git_head"],
            "0123456789abcdef0123456789abcdef01234567"
        );
        assert_eq!(value["provenance"]["model_revision"], "model@rev:d16");
        assert_eq!(value["rows"].as_array().map(Vec::len), Some(1));
        assert_eq!(value["rows"][0]["scenario_id"], "test.lexical");
        assert_eq!(value["detail"]["routes"][0]["route"], "lexical");
    }
}
