//! `dsl_cold_matrix` — cold-start single-query probe for the Layer-3 DSL
//! latency matrix (RFC-DSL-Benchmarking §3.2), and the assembler of its
//! artifact.
//!
//! Each `--scenario` invocation is **one fresh process = one true cold
//! sample**. The `tools/benchmark/run_dsl_cold_matrix.py` orchestrator drives
//! this binary `--scenario <id>` K times per scenario, then pipes every
//! sample into one `--assemble` invocation, which aggregates the percentiles
//! and writes the one `BenchArtifactV1` (QI-BB-010): the exact head of a
//! clean worktree, the fixture corpus digest, the run configuration digest,
//! the host and the assembling process's peak RSS. The orchestrator never
//! writes an artifact and never stamps a head.
//!
//! The probe never aggregates and never fabricates a number: it boots, seeds
//! the scenario's fixture, seals, activates, times exactly one query, prints
//! one JSON sample, and exits. A query that the runtime rejects is reported
//! with its typed error code and a null result count, never a guessed
//! latency.
//!
//! Subcommands:
//!   --list                        print the scenario authority as a JSON array
//!   --scenario <id>               run one cold sample and print a JSON object
//!   --assemble --out <path> --samples <n>
//!                                 read a JSON array of samples on stdin and
//!                                 write the artifact

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::Result as AnyResult;
use quanta_index_searchd_harness::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, GitHeadV1, HostV1, LatencySummary,
    PhaseDurationsV1, ResourceUsageV1, ResultShape, config_digest, corpus_digest,
    model_revision_of,
};
use quanta_index_searchd_harness::bench_support::{
    QueryOutcome, ScenarioTruthMode, bench_row, fixture_corpus_files, prepare_cold_runtime,
    run_scenario_query, validate_scenario_outcome,
};
use quanta_index_searchd_harness::scenarios::{DslBenchScenario, SCENARIOS, scenario_by_id};
use quanta_index_searchd_harness::{E2eErrorCode, E2eRuntime};

const DIMENSION: &str = "dsl-cold";

/// Print one line of machine-readable output to stdout.
///
/// This binary's contract is to emit exactly one JSON document per run, so
/// stdout printing is the intended interface rather than incidental logging.
#[expect(
    clippy::print_stdout,
    reason = "the cold-matrix probe emits its JSON sample on stdout by design"
)]
fn emit_stdout(value: &serde_json::Value) {
    println!("{value}");
}

/// Print a diagnostic line to stderr.
///
/// Operator-facing errors go to stderr so the orchestrator can separate them
/// from the JSON sample on stdout.
#[expect(
    clippy::print_stderr,
    reason = "the cold-matrix probe reports operator errors on stderr by design"
)]
fn emit_stderr(message: &str) {
    eprintln!("{message}");
}

fn print_list() {
    let rows: Vec<serde_json::Value> = SCENARIOS
        .iter()
        .map(|scenario| {
            serde_json::json!({
                "scenario_id": scenario.id,
                "route_family": scenario.route_family.as_str(),
                "syntax": scenario.syntax.as_str(),
                "expected_shape": scenario.expected_shape.as_str(),
            })
        })
        .collect();
    emit_stdout(&serde_json::Value::Array(rows));
}

fn run_scenario(scenario: &DslBenchScenario) -> ExitCode {
    let mut runtime = match prepare_cold_runtime(scenario) {
        Ok(runtime) => runtime,
        Err(err) => {
            emit_stderr(&format!(
                "dsl_cold_matrix: prepare failed for {}: {err:#}",
                scenario.id
            ));
            return ExitCode::FAILURE;
        }
    };
    match measured_sample(&mut runtime, scenario) {
        Ok(sample) => {
            emit_stdout(&sample);
            ExitCode::SUCCESS
        }
        Err(err) => {
            emit_stderr(&format!(
                "dsl_cold_matrix: scenario {}: {err:#}",
                scenario.id
            ));
            ExitCode::FAILURE
        }
    }
}

/// Time exactly one cold query and build its JSON sample.
fn measured_sample(
    runtime: &mut E2eRuntime,
    scenario: &DslBenchScenario,
) -> AnyResult<serde_json::Value> {
    let started = Instant::now();
    let outcome = run_scenario_query(runtime, scenario);
    let first_query_ms = started.elapsed().as_secs_f64() * 1000.0;
    validate_scenario_outcome(scenario, ScenarioTruthMode::IsolatedFixture, &outcome)?;
    Ok(serde_json::json!({
        "scenario_id": scenario.id,
        "route_family": scenario.route_family.as_str(),
        "syntax": scenario.syntax.as_str(),
        "mode": BenchMode::Cold.as_str(),
        "result_shape": outcome.result_shape.as_str(),
        "first_query_ms": first_query_ms,
        "result_count": outcome.result_count,
        "typed_error_code": outcome.typed_error_code.map(E2eErrorCode::as_str),
        "engine_touched": outcome.engine_touched,
        "early_stop_reason": outcome.early_stop_reason,
        "model_revision": model_revision_of(runtime.embedder_profile()),
    }))
}

/// One `--scenario` sample as the assembler reads it back.
struct ColdSample {
    scenario_id: String,
    first_query_ms: Option<f64>,
    outcome: QueryOutcome,
    model_revision: Option<String>,
}

fn string_field(sample: &serde_json::Value, key: &str) -> AnyResult<String> {
    sample
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("sample has no string `{key}`: {sample}"))
}

fn optional_string_field(sample: &serde_json::Value, key: &str) -> AnyResult<Option<String>> {
    match sample.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(text)) => Ok(Some(text.clone())),
        Some(other) => Err(anyhow::anyhow!("sample `{key}` is not a string: {other}")),
    }
}

fn optional_error_code_field(
    sample: &serde_json::Value,
    key: &str,
) -> AnyResult<Option<E2eErrorCode>> {
    optional_string_field(sample, key)?
        .map(|value| {
            E2eErrorCode::from_code_str(&value)
                .ok_or_else(|| anyhow::anyhow!("sample `{key}` has unknown error code {value:?}"))
        })
        .transpose()
}

fn result_shape_of(text: &str) -> AnyResult<ResultShape> {
    Ok(match text {
        "candidates" => ResultShape::Candidates,
        "commits" => ResultShape::Commits,
        "diff_paths" => ResultShape::DiffPaths,
        "typed_error" => ResultShape::TypedError,
        "empty" => ResultShape::Empty,
        other => {
            return Err(anyhow::anyhow!(
                "sample result_shape {other:?} is not a shape"
            ));
        }
    })
}

fn parse_sample(sample: &serde_json::Value) -> AnyResult<ColdSample> {
    let first_query_ms =
        match sample.get("first_query_ms") {
            None | Some(serde_json::Value::Null) => None,
            Some(value) => Some(value.as_f64().ok_or_else(|| {
                anyhow::anyhow!("sample first_query_ms is not a number: {value}")
            })?),
        };
    if first_query_ms.is_some_and(|value| !value.is_finite() || value < 0.0) {
        return Err(anyhow::anyhow!(
            "sample first_query_ms is not finite non-negative: {sample}"
        ));
    }
    let engine_touched = sample
        .get("engine_touched")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("sample has no engine_touched array: {sample}"))?
        .iter()
        .map(|engine| {
            engine
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| anyhow::anyhow!("engine_touched entry is not a string: {engine}"))
        })
        .collect::<AnyResult<Vec<_>>>()?;
    let result_count = match sample.get("result_count") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("sample result_count is not a count: {value}"))?,
        ),
    };
    Ok(ColdSample {
        scenario_id: string_field(sample, "scenario_id")?,
        first_query_ms,
        outcome: QueryOutcome {
            result_shape: result_shape_of(&string_field(sample, "result_shape")?)?,
            result_count,
            typed_error_code: optional_error_code_field(sample, "typed_error_code")?,
            engine_touched,
            early_stop_reason: optional_string_field(sample, "early_stop_reason")?,
        },
        model_revision: optional_string_field(sample, "model_revision")?,
    })
}

/// Read every sample on stdin and write the cold artifact.
///
/// Every scenario in the authority must have exactly `samples` valid samples.
/// A missing, early-stopped, mislabeled or semantically wrong sample is refused.
fn assemble(out: &Path, samples_per_scenario: usize) -> AnyResult<()> {
    let git_head = GitHeadV1::resolve(Path::new("."))?;
    let host = HostV1::observe()?;
    let mut stdin = String::new();
    let _read = std::io::stdin().read_to_string(&mut stdin)?;
    let samples: Vec<serde_json::Value> = serde_json::from_str(&stdin)?;
    let mut by_scenario: BTreeMap<String, Vec<ColdSample>> = BTreeMap::new();
    for sample in &samples {
        let parsed = parse_sample(sample)?;
        let scenario = scenario_by_id(&parsed.scenario_id).ok_or_else(|| {
            anyhow::anyhow!("sample names unknown scenario {:?}", parsed.scenario_id)
        })?;
        for (field, expected) in [
            ("mode", BenchMode::Cold.as_str()),
            ("route_family", scenario.route_family.as_str()),
            ("syntax", scenario.syntax.as_str()),
        ] {
            let actual = string_field(sample, field)?;
            if actual != expected {
                return Err(anyhow::anyhow!(
                    "scenario {} sample {field}={actual:?}, expected {expected:?}",
                    scenario.id
                ));
            }
        }
        validate_scenario_outcome(
            scenario,
            ScenarioTruthMode::IsolatedFixture,
            &parsed.outcome,
        )?;
        if parsed.first_query_ms.is_none() {
            return Err(anyhow::anyhow!(
                "scenario {} sample has null first_query_ms",
                scenario.id
            ));
        }
        by_scenario
            .entry(parsed.scenario_id.clone())
            .or_default()
            .push(parsed);
    }
    let mut rows = Vec::with_capacity(SCENARIOS.len());
    let mut model_revision: Option<Option<String>> = None;
    for scenario in SCENARIOS {
        let Some(scenario_samples) = by_scenario.remove(scenario.id) else {
            return Err(anyhow::anyhow!("no samples for scenario {}", scenario.id));
        };
        let Some(last) = scenario_samples.last() else {
            return Err(anyhow::anyhow!("no samples for scenario {}", scenario.id));
        };
        for sample in &scenario_samples {
            if let Some(previous) = &model_revision {
                if previous != &sample.model_revision {
                    return Err(anyhow::anyhow!(
                        "scenario {} sample model_revision differs from other samples",
                        scenario.id
                    ));
                }
            } else {
                model_revision = Some(sample.model_revision.clone());
            }
        }
        if scenario_samples.len() != samples_per_scenario {
            return Err(anyhow::anyhow!(
                "scenario {} has {} samples, expected {samples_per_scenario}",
                scenario.id,
                scenario_samples.len()
            ));
        }
        let latencies = scenario_samples
            .iter()
            .map(|sample| {
                sample.first_query_ms.ok_or_else(|| {
                    anyhow::anyhow!(
                        "scenario {} sample has null first_query_ms without early_stop_reason",
                        scenario.id
                    )
                })
            })
            .collect::<AnyResult<Vec<f64>>>()?;
        let latency = LatencySummary::from_samples_ms(&latencies)
            .ok_or_else(|| anyhow::anyhow!("scenario {} has no latencies", scenario.id))?;
        rows.push(bench_row(
            scenario,
            clone_outcome(&last.outcome),
            Some(latency),
        ));
    }
    let artifact = BenchArtifactV1 {
        dimension: DIMENSION.to_string(),
        mode: BenchMode::Cold,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: corpus_digest(
                DIMENSION,
                &SCENARIOS
                    .iter()
                    .flat_map(|scenario| fixture_corpus_files(scenario.fixture))
                    .collect::<Vec<_>>(),
            ),
            config_digest: config_digest(
                DIMENSION,
                &[
                    ("samples_per_scenario", samples_per_scenario.to_string()),
                    ("scenario_count", SCENARIOS.len().to_string()),
                ],
            ),
            model_revision: model_revision
                .ok_or_else(|| anyhow::anyhow!("no model revision samples"))?,
        },
        host,
        resources: ResourceUsageV1::observe_self()?,
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows,
        detail: serde_json::json!({
            "samples_per_scenario": samples_per_scenario,
            "sample_source": "one fresh process per sample",
        }),
    };
    artifact.write_to(out)?;
    Ok(())
}

fn clone_outcome(outcome: &QueryOutcome) -> QueryOutcome {
    QueryOutcome {
        result_shape: outcome.result_shape,
        result_count: outcome.result_count,
        typed_error_code: outcome.typed_error_code,
        engine_touched: outcome.engine_touched.clone(),
        early_stop_reason: outcome.early_stop_reason.clone(),
    }
}

fn parse_assemble_args(rest: &[String]) -> AnyResult<(PathBuf, usize)> {
    let mut out: Option<PathBuf> = None;
    let mut samples: Option<usize> = None;
    let mut it = rest.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--out" => {
                out = Some(PathBuf::from(
                    it.next()
                        .ok_or_else(|| anyhow::anyhow!("--out requires a path"))?,
                ));
            }
            "--samples" => {
                let raw = it
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--samples requires a count"))?;
                let parsed = raw
                    .parse::<usize>()
                    .map_err(|err| anyhow::anyhow!("--samples {raw:?}: {err}"))?;
                if parsed == 0 {
                    return Err(anyhow::anyhow!("--samples must be at least 1"));
                }
                samples = Some(parsed);
            }
            other => return Err(anyhow::anyhow!("unknown --assemble argument {other:?}")),
        }
    }
    Ok((
        out.ok_or_else(|| anyhow::anyhow!("--assemble requires --out <path>"))?,
        samples.ok_or_else(|| anyhow::anyhow!("--assemble requires --samples <n>"))?,
    ))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((flag, rest)) if flag.as_str() == "--list" && rest.is_empty() => {
            print_list();
            ExitCode::SUCCESS
        }
        Some((flag, rest)) if flag.as_str() == "--scenario" => {
            let Some(id) = rest.first() else {
                emit_stderr("dsl_cold_matrix: --scenario requires an <id>");
                return ExitCode::FAILURE;
            };
            scenario_by_id(id).map_or_else(
                || {
                    emit_stderr(&format!("dsl_cold_matrix: unknown scenario id `{id}`"));
                    ExitCode::FAILURE
                },
                run_scenario,
            )
        }
        Some((flag, rest)) if flag.as_str() == "--assemble" => {
            match parse_assemble_args(rest).and_then(|(out, samples)| assemble(&out, samples)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    emit_stderr(&format!("dsl_cold_matrix: assemble failed: {err:#}"));
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            emit_stderr(
                "usage: dsl_cold_matrix (--list | --scenario <id> | --assemble --out <path> --samples <n>)",
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::SearchPlaneErrorCodeV2;

    #[test]
    fn child_sample_error_code_decoder_rejects_unknown_and_retired_codes() {
        for local in [
            "HARNESS_START",
            "HARNESS_ROUTE_MISMATCH",
            "IPC_TRANSPORT",
            "UNEXPECTED_RESPONSE",
        ] {
            assert!(
                SearchPlaneErrorCodeV2::from_wire_str(local).is_none(),
                "harness and daemon code namespaces must remain disjoint"
            );
        }
        let wire = serde_json::json!({"typed_error_code": "NOT_READY"});
        assert_eq!(
            optional_error_code_field(&wire, "typed_error_code").expect("closed wire code"),
            Some(E2eErrorCode::Remote(SearchPlaneErrorCodeV2::NotReady))
        );
        let harness = serde_json::json!({"typed_error_code": "HARNESS_START"});
        assert_eq!(
            optional_error_code_field(&harness, "typed_error_code").expect("harness code"),
            Some(E2eErrorCode::HarnessStart)
        );
        for stale in ["BAD_REQUEST", "UNREGISTERED_ERROR"] {
            let sample = serde_json::json!({"typed_error_code": stale});
            assert!(optional_error_code_field(&sample, "typed_error_code").is_err());
        }
    }
}
