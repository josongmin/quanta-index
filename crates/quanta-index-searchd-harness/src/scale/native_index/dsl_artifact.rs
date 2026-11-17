//! Convert a pinned Native DSL diagnostic matrix to the common benchmark envelope.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Read as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use nix::fcntl::OFlag;
use serde_json::{Value, json};

use super::executable::NativeExecutable;
use crate::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, GitHeadV1, HostV1, LatencySummary,
    PhaseDurationsV1, ResourceUsageV1, config_digest, corpus_digest, corpus_digest_refs,
};
use crate::history_fixture::history_fixture_corpus_files;
use crate::scenarios::{
    FixtureKind, LEXICAL_CORPUS, NATIVE_TEXT_FILE_SCENARIOS, SCENARIOS, scenario_by_id,
};

const MATRIX_SCHEMA: &str = "native-dsl-matrix-v1";
const MAX_MATRIX_BYTES: u64 = 16 * 1024 * 1024;

struct ValidatedMatrix {
    mode: BenchMode,
    head: GitHeadV1,
    runner_sha256: String,
    daemon_sha256: String,
    history_fixture_sha256: String,
    text_fixture_sha256: String,
    text_config_sha256: String,
    history_config_sha256: String,
    timing: ReportTiming,
    text_query_deadline_ms: f64,
    reports_per_scenario: usize,
    samples: BTreeMap<String, Vec<f64>>,
    report_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ReportTiming {
    passes: u64,
    samples_per_pass: u64,
    prime_queries: u64,
    cooldown_ms: f64,
    pass_settle_ms: f64,
}

/// Assemble a diagnostic common-schema artifact. This does not qualify the
/// remaining DSL scenarios or turn assembler-process RSS into daemon RSS.
pub fn assemble_native_dsl_artifact(
    input: &Path,
    output: &Path,
    expected_mode: BenchMode,
) -> Result<()> {
    ensure!(
        input != output,
        "Native DSL input and output paths must differ"
    );
    let bytes = read_matrix_bytes(input)?;
    let matrix: Value = serde_json::from_slice(&bytes).context("parse Native DSL matrix")?;
    let valid = validate_matrix(&matrix)?;
    ensure!(
        valid.mode == expected_mode,
        "Native DSL matrix mode differs from assembler binary"
    );

    let executable = NativeExecutable::capture(&std::env::current_exe()?)?;
    ensure!(
        valid.runner_sha256 == format!("sha256:{}", executable.sha256()),
        "Native DSL runner digest differs from the executing assembler"
    );
    let cwd = std::env::current_dir()?;
    let head = GitHeadV1::resolve(&cwd).context("Native DSL source checkout is not frozen")?;
    ensure!(
        head == valid.head,
        "Native DSL source HEAD differs from matrix capture"
    );
    let host = HostV1::observe()?;
    let resources = ResourceUsageV1::observe_self()?;
    let artifact = build_artifact(
        valid,
        host,
        resources,
        format!("sha256:{}", super::hex_sha256(&bytes)),
        matrix,
    );
    executable.verify()?;
    artifact
        .write_to(output)
        .context("write Native DSL artifact")?;
    Ok(())
}

fn read_matrix_bytes(input: &Path) -> Result<Vec<u8>> {
    // The matrix is a control document. O_NONBLOCK keeps a FIFO from hanging
    // the assembler; O_NOFOLLOW refuses a symlink before reading any bytes.
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NONBLOCK | OFlag::O_NOFOLLOW).bits())
        .open(input)
        .context("open Native DSL matrix")?;
    ensure!(
        file.metadata()?.is_file(),
        "Native DSL matrix must be a regular file"
    );
    let mut bytes = Vec::new();
    let _read = file.take(MAX_MATRIX_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("read Native DSL matrix")?;
    ensure!(
        u64::try_from(bytes.len())? <= MAX_MATRIX_BYTES,
        "Native DSL matrix exceeds {MAX_MATRIX_BYTES} bytes"
    );
    Ok(bytes)
}

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("Native DSL {key} is missing or not a string"))
}

fn required_array<'a>(value: &'a Value, key: &str) -> Result<&'a [Value]> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .with_context(|| format!("Native DSL {key} is missing or not an array"))
}

fn sha256(value: &str, key: &str) -> Result<()> {
    ensure!(
        value
            .strip_prefix("sha256:")
            .is_some_and(|hex| hex.len() == 64
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
        "Native DSL {key} must be a lowercase SHA-256 digest"
    );
    Ok(())
}

fn raw_sha256(value: &str, key: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "Native DSL {key} must be a lowercase SHA-256 hex digest"
    );
    Ok(())
}

fn samples_for_report(report: &Value, mode: BenchMode, id: &str) -> Result<(Vec<f64>, ReportTiming)> {
    let passes = report
        .get("passes")
        .and_then(Value::as_u64)
        .context("missing Native DSL passes")?;
    let per_pass = report
        .get("samples_per_pass")
        .and_then(Value::as_u64)
        .context("missing Native DSL samples_per_pass")?;
    let prime = report
        .get("prime_queries")
        .and_then(Value::as_u64)
        .context("missing Native DSL prime_queries")?;
    let cooldown_ms = finite_nonnegative(report, "cooldown_ms", id)?;
    let pass_settle_ms = finite_nonnegative(report, "pass_settle_ms", id)?;
    ensure!(
        passes > 0 && per_pass > 0,
        "Native DSL {id} has zero samples"
    );
    if mode == BenchMode::Cold {
        ensure!(
            (passes, per_pass, prime) == (1, 1, 0),
            "Native DSL {id} cold sample is not one unprimed query"
        );
    } else {
        ensure!(
            prime > 0,
            "Native DSL {id} warm sample has no priming query"
        );
    }
    let samples = required_array(report, "samples_ms")?;
    ensure!(
        u64::try_from(samples.len())?
            == passes
                .checked_mul(per_pass)
                .context("Native DSL sample count overflow")?,
        "Native DSL {id} sample count differs from pass structure"
    );
    let samples = samples
        .iter()
        .map(|v| {
            v.as_f64()
                .filter(|ms| ms.is_finite() && *ms >= 0.0)
                .with_context(|| format!("Native DSL {id} has invalid sample latency"))
        })
        .collect::<Result<Vec<_>>>()?;
    let expected = LatencySummary::from_samples_ms(&samples).context("empty Native DSL samples")?;
    let observed = report
        .get("latency")
        .context("missing Native DSL latency summary")?;
    for (key, actual) in [
        ("p50_ms", expected.p50_ms),
        ("p95_ms", expected.p95_ms),
        ("p99_ms", expected.p99_ms),
    ] {
        let stated = observed
            .get(key)
            .and_then(Value::as_f64)
            .with_context(|| format!("Native DSL {id} missing {key}"))?;
        ensure!(
            stated == actual,
            "Native DSL {id} {key} differs from raw samples"
        );
    }
    ensure!(
        observed.get("samples").and_then(Value::as_u64) == Some(u64::from(expected.samples)),
        "Native DSL {id} summary sample count differs"
    );
    Ok((samples, ReportTiming {
        passes, samples_per_pass: per_pass, prime_queries: prime,
        cooldown_ms, pass_settle_ms,
    }))
}

fn finite_nonnegative(report: &Value, field: &str, id: &str) -> Result<f64> {
    report.get(field).and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .with_context(|| format!("Native DSL {id} has missing or invalid {field}"))
}

fn validate_matrix(input: &Value) -> Result<ValidatedMatrix> {
    ensure!(
        required_str(input, "schema")? == MATRIX_SCHEMA,
        "wrong Native DSL matrix schema"
    );
    ensure!(
        required_str(input, "qualification")? == "diagnostic",
        "Native DSL input qualification must be diagnostic"
    );
    let mode = match required_str(input, "mode")? {
        "warm" => BenchMode::Warm,
        "cold" => BenchMode::Cold,
        other => anyhow::bail!("unsupported Native DSL mode {other}"),
    };
    let head = GitHeadV1::parse(required_str(input, "git_head")?)?;
    let runner_sha256 = required_str(input, "runner_sha256")?.to_owned();
    let daemon_sha256 = required_str(input, "daemon_sha256")?.to_owned();
    let history_fixture_sha256 = required_str(input, "history_fixture_sha256")?.to_owned();
    for (key, value) in [
        ("runner_sha256", &runner_sha256),
        ("daemon_sha256", &daemon_sha256),
        ("history_fixture_sha256", &history_fixture_sha256),
    ] {
        sha256(value, key)?;
    }
    let rows = required_array(input, "scenarios")?;
    ensure!(
        SCENARIOS.len() == 33,
        "canonical DSL roster changed; review Native subset disposition"
    );
    ensure!(
        rows.len()
            == NATIVE_TEXT_FILE_SCENARIOS.len()
                + SCENARIOS
                    .iter()
                    .filter(|s| s.fixture == FixtureKind::HistoryLedger)
                    .count(),
        "Native DSL matrix must contain the complete Native Text and History roster"
    );
    let mut expected = BTreeMap::new();
    for (id, paths) in NATIVE_TEXT_FILE_SCENARIOS {
        ensure!(
            expected.insert(*id, ("text", Some(*paths))).is_none(),
            "duplicate Native DSL registry ID"
        );
    }
    for scenario in SCENARIOS
        .iter()
        .filter(|s| s.fixture == FixtureKind::HistoryLedger)
    {
        ensure!(
            expected.insert(scenario.id, ("history", None)).is_none(),
            "duplicate Native DSL registry ID"
        );
    }
    ensure!(
        NATIVE_TEXT_FILE_SCENARIOS.len() == 7 && expected.len() == 14,
        "Native DSL supported roster changed; review artifact assembly"
    );
    let mut seen = BTreeSet::new();
    let mut all_samples = BTreeMap::new();
    let mut report_count = 0;
    let mut text_fixture = None;
    let mut text_config = None;
    let mut history_config = None;
    let mut common_timing = None;
    let mut text_query_deadline_ms = None;
    let mut cold_repeats = None;
    for row in rows {
        let id = required_str(row, "scenario_id")?;
        let kind = required_str(row, "kind")?;
        let (expected_kind, expected_paths) = expected
            .get(id)
            .with_context(|| format!("unknown Native DSL scenario {id}"))?;
        ensure!(
            kind == *expected_kind && seen.insert(id.to_owned()),
            "duplicate or wrong-kind Native DSL scenario {id}"
        );
        let scenario =
            scenario_by_id(id).context("Native DSL scenario missing from canonical registry")?;
        let reports = required_array(row, "reports")?;
        ensure!(!reports.is_empty(), "Native DSL {id} has no SDK reports");
        if mode == BenchMode::Warm {
            ensure!(
                reports.len() == 1,
                "Native DSL warm scenario {id} must have one report"
            );
        } else if let Some(count) = cold_repeats {
            ensure!(
                reports.len() == count,
                "Native DSL cold report count differs across scenarios"
            );
        } else {
            cold_repeats = Some(reports.len());
        }
        report_count += reports.len();
        let mut samples = Vec::new();
        for wrapper in reports {
            ensure!(
                required_str(wrapper, "daemon_sha256")?
                    == daemon_sha256.trim_start_matches("sha256:"),
                "Native DSL {id} daemon digest differs"
            );
            let report = if kind == "history" {
                ensure!(
                    required_str(wrapper, "schema")?
                        == format!("native-dsl-history-process-{}-v1", mode.as_str()),
                    "Native DSL {id} History process mode differs"
                );
                let config = required_str(wrapper, "config_sha256")?;
                raw_sha256(config, "History config_sha256")?;
                retain_equal(&mut history_config, config, "History config")?;
                wrapper
                    .get("history")
                    .context("missing Native History report")?
            } else {
                wrapper
            };
            ensure!(
                required_str(report, "schema")? == format!("native-dsl-{kind}-samples-v1"),
                "Native DSL {id} report schema differs"
            );
            ensure!(
                required_str(report, "scenario_id")? == id,
                "Native DSL report scenario differs"
            );
            if kind == "text" {
                ensure!(
                    required_str(report, "query_text")? == scenario.query.resolve().as_ref(),
                    "Native DSL {id} Text query differs"
                );
                ensure!(
                    required_str(report, "query_syntax")? == scenario.syntax.as_str(),
                    "Native DSL {id} syntax differs"
                );
                ensure!(
                    required_str(report, "result_unit")? == "file",
                    "Native DSL {id} result unit differs"
                );
                let paths = required_array(report, "matched_files")?;
                ensure!(
                    paths
                        .iter()
                        .map(Value::as_str)
                        .collect::<Option<Vec<_>>>()
                        .as_deref()
                        == *expected_paths,
                    "Native DSL {id} file oracle differs"
                );
                let fixture = required_str(report, "fixture_sha256")?;
                let config = required_str(report, "config_template_sha256")?;
                ensure!(
                    fixture
                        == corpus_digest_refs("native-dsl-text", LEXICAL_CORPUS.iter().copied()),
                    "Native DSL {id} Text fixture digest differs from canonical corpus"
                );
                raw_sha256(config, "Text config_template_sha256")?;
                retain_equal(&mut text_fixture, fixture, "Text fixture")?;
                retain_equal(&mut text_config, config, "Text config")?;
                let deadline = finite_nonnegative(report, "query_deadline_ms", id)?;
                ensure!(deadline > 0.0, "Native DSL {id} has zero query deadline");
                if let Some(previous) = text_query_deadline_ms {
                    ensure!(previous == deadline, "Native DSL Text query deadline differs across reports");
                } else {
                    text_query_deadline_ms = Some(deadline);
                }
                let published = required_array(report, "published")?;
                ensure!(
                    published.len()
                        == usize::try_from(
                            report
                                .get("passes")
                                .and_then(Value::as_u64)
                                .context("missing passes")?
                        )?,
                    "Native DSL {id} Text publication count differs"
                );
                ensure!(
                    published
                        .iter()
                        .all(|p| p.get("view").is_some_and(|value| !value.is_null())
                            && p.get("source_cut").is_some_and(|value| !value.is_null())
                            && p.get("operations").and_then(Value::as_array).is_some_and(
                                |ops| !ops.is_empty()
                                    && ops.iter().all(|operation| operation
                                        .get("submit_count")
                                        .and_then(Value::as_u64)
                                        == Some(1)
                                        && matches!(
                                            operation.get("completion").and_then(Value::as_str),
                                            Some("immediate_ack" | "reconciled_status")
                                        ))
                            )),
                    "Native DSL {id} missing Text publication witness"
                );
            } else {
                ensure!(
                    required_str(report, "result_shape")? == scenario.expected_shape.as_str(),
                    "Native DSL {id} History result shape differs"
                );
                let query = report
                    .get("query")
                    .context("missing Native History query")?;
                let text_query = query
                    .get("text_query")
                    .context("missing Native History text query")?;
                ensure!(
                    required_str(text_query, "query_text")? == scenario.query.resolve().as_ref()
                        && required_str(text_query, "syntax")? == scenario.syntax.as_str()
                        && text_query.get("top_k").and_then(Value::as_u64)
                            == Some(u64::from(crate::scenarios::TOP_K))
                        && required_str(query, "order")? == "recency"
                        && query.get("cursor").is_none_or(Value::is_null),
                    "Native DSL {id} History query differs from canonical scenario"
                );
                let published = report
                    .get("published")
                    .context("missing Native History publication witness")?;
                ensure!(
                    published.get("view").is_some_and(|value| !value.is_null())
                        && published.get("cut").is_some_and(|value| !value.is_null())
                        && published
                            .get("session")
                            .is_some_and(|value| !value.is_null()),
                    "Native DSL {id} incomplete History publication witness"
                );
            }
            let (report_samples, timing) = samples_for_report(report, mode, id)?;
            if let Some(previous) = common_timing {
                ensure!(previous == timing, "Native DSL timing differs across reports");
            } else {
                common_timing = Some(timing);
            }
            samples.extend(report_samples);
        }
        ensure!(all_samples.insert(id.to_owned(), samples).is_none(),
            "duplicate Native DSL sample row {id}");
    }
    ensure!(
        seen.len() == expected.len(),
        "Native DSL matrix is missing supported scenarios"
    );
    Ok(ValidatedMatrix {
        mode,
        head,
        runner_sha256,
        daemon_sha256,
        history_fixture_sha256,
        text_fixture_sha256: text_fixture.context("missing Text fixture binding")?,
        text_config_sha256: text_config.context("missing Text config binding")?,
        history_config_sha256: history_config.context("missing History config binding")?,
        timing: common_timing.context("missing Native DSL timing")?,
        text_query_deadline_ms: text_query_deadline_ms.context("missing Text query deadline")?,
        reports_per_scenario: cold_repeats.unwrap_or(1),
        samples: all_samples,
        report_count,
    })
}

fn retain_equal(slot: &mut Option<String>, value: &str, name: &str) -> Result<()> {
    if let Some(original) = slot {
        ensure!(
            original.as_str() == value,
            "Native DSL {name} digest differs across reports"
        );
    } else {
        *slot = Some(value.to_owned());
    }
    Ok(())
}

fn build_artifact(
    valid: ValidatedMatrix,
    host: HostV1,
    resources: ResourceUsageV1,
    native_matrix_sha256: String,
    native_matrix: Value,
) -> BenchArtifactV1 {
    let dimension = format!("dsl-{}", valid.mode.as_str());
    let measured_ids = valid.samples.keys().cloned().collect::<Vec<_>>();
    let remaining_ids = SCENARIOS
        .iter()
        .filter(|s| !valid.samples.contains_key(s.id))
        .map(|s| s.id)
        .collect::<Vec<_>>();
    let rows = SCENARIOS
        .iter()
        .map(|scenario| {
            let samples = valid.samples.get(scenario.id);
            BenchRowV1 {
                scenario_id: scenario.id.to_owned(),
                route_family: scenario.route_family,
                syntax: scenario.syntax,
                result_shape: scenario.expected_shape,
                latency: samples.and_then(|values| LatencySummary::from_samples_ms(values)),
                qps: None,
                error_count: 0,
                timeout_count: 0,
                result_count: samples.and_then(|_| {
                    if NATIVE_TEXT_FILE_SCENARIOS
                        .iter()
                        .any(|(id, _)| *id == scenario.id)
                    {
                        NATIVE_TEXT_FILE_SCENARIOS
                            .iter()
                            .find(|(id, _)| *id == scenario.id)
                            .map(|(_, paths)| paths.len() as u64)
                    } else {
                        scenario.expected_count
                    }
                }),
                typed_error_code: None,
                engine_touched: if samples.is_some() {
                    vec!["native".into()]
                } else {
                    Vec::new()
                },
                early_stop_reason: samples
                    .is_none()
                    .then(|| "pending_owner_disposition".into()),
            }
        })
        .collect();
    let corpus_digest = config_digest(
        "dsl-native-mixed-fixture",
        &[
            ("text_fixture_sha256", valid.text_fixture_sha256.clone()),
            (
                "history_fixture_sha256",
                valid.history_fixture_sha256.clone(),
            ),
        ],
    );
    let config_digest = config_digest(
        &dimension,
        &[
            ("runner_sha256", valid.runner_sha256.clone()),
            ("daemon_sha256", valid.daemon_sha256.clone()),
            ("text_config_sha256", valid.text_config_sha256.clone()),
            ("history_config_sha256", valid.history_config_sha256.clone()),
            ("mode", valid.mode.as_str().into()),
        ],
    );
    BenchArtifactV1 {
        dimension,
        mode: valid.mode,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head: valid.head,
            corpus_digest,
            config_digest,
            model_revision: None,
        },
        host,
        resources,
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows,
        detail: json!({
            "qualification": "diagnostic",
            "source_schema": MATRIX_SCHEMA,
            "native_matrix_sha256": native_matrix_sha256,
            "native_matrix": native_matrix,
            "native_subset": { "measured": measured_ids.len(), "total": SCENARIOS.len(),
                "measured_ids": measured_ids, "remaining_ids": remaining_ids,
                "remaining_disposition": "pending_owner_disposition" },
            "report_count": valid.report_count,
            "runner_sha256": valid.runner_sha256,
            "daemon_sha256": valid.daemon_sha256,
            "history_fixture_sha256": valid.history_fixture_sha256,
            "text_fixture_sha256": valid.text_fixture_sha256,
            "text_config_sha256": valid.text_config_sha256,
            "history_config_sha256": valid.history_config_sha256,
            "resource_scope": "assembler process RUSAGE_SELF only; daemon measurement RSS unavailable in matrix",
            "host_scope": "assembler host; pinned frozen runner invokes assembly after measurement",
            "corpus_digest_scope": "framed digest of pinned Text fixture and History fixture SHA-256 identifiers",
            "sample_scope": "SDK latency samples; per-sample oracle enforced by pinned Native runner",
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;
    use serde_json::json;
    use tempfile::TempDir;

    use super::{
        MAX_MATRIX_BYTES, build_artifact, read_matrix_bytes, samples_for_report, validate_matrix,
    };
    use crate::artifact::{BenchMode, HostV1, ResourceUsageV1, corpus_digest_refs};
    use crate::scenarios::{FixtureKind, LEXICAL_CORPUS, NATIVE_TEXT_FILE_SCENARIOS, SCENARIOS};

    fn complete_matrix(mode: BenchMode) -> serde_json::Value {
        let raw_daemon = "2".repeat(64);
        let raw_config = "3".repeat(64);
        let prime = if mode == BenchMode::Cold { 0 } else { 1 };
        let mut scenarios = Vec::new();
        for (id, paths) in NATIVE_TEXT_FILE_SCENARIOS {
            let scenario = SCENARIOS
                .iter()
                .find(|s| s.id == *id)
                .expect("registered Text scenario");
            let report = json!({
                "schema": "native-dsl-text-samples-v1", "scenario_id": id,
                "query_text": scenario.query.resolve(), "query_syntax": scenario.syntax.as_str(),
                "result_unit": "file", "matched_files": paths,
                "daemon_sha256": raw_daemon,
                "config_template_sha256": raw_config,
                "fixture_sha256": corpus_digest_refs("native-dsl-text", LEXICAL_CORPUS.iter().copied()),
                "passes": 1, "samples_per_pass": 1, "prime_queries": prime,
                "published": [{"view": {}, "source_cut": {}, "operations": [{"submit_count": 1, "completion": "immediate_ack"}]}],
                "samples_ms": [1.0],
                "latency": {"p50_ms": 1.0, "p95_ms": 1.0, "p99_ms": 1.0, "samples": 1},
            });
            scenarios.push(json!({"scenario_id": id, "kind": "text", "reports": [report]}));
        }
        for scenario in SCENARIOS
            .iter()
            .filter(|s| s.fixture == FixtureKind::HistoryLedger)
        {
            let report = json!({
                "schema": "native-dsl-history-samples-v1", "scenario_id": scenario.id,
                "query": {"text_query": {"query_text": scenario.query.resolve(),
                    "syntax": scenario.syntax.as_str(), "top_k": 10}, "order": "recency"},
                "result_shape": scenario.expected_shape.as_str(),
                "published": {"view": {}, "cut": {}, "session": {}},
                "passes": 1, "samples_per_pass": 1, "prime_queries": prime,
                "samples_ms": [1.0],
                "latency": {"p50_ms": 1.0, "p95_ms": 1.0, "p99_ms": 1.0, "samples": 1},
            });
            let wrapper = json!({
                "schema": format!("native-dsl-history-process-{}-v1", mode.as_str()),
                "daemon_sha256": raw_daemon, "config_sha256": raw_config,
                "history": report,
            });
            scenarios
                .push(json!({"scenario_id": scenario.id, "kind": "history", "reports": [wrapper]}));
        }
        json!({
            "schema": "native-dsl-matrix-v1", "qualification": "diagnostic",
            "mode": mode.as_str(), "git_head": "0123456789abcdef0123456789abcdef01234567",
            "runner_sha256": format!("sha256:{}", "1".repeat(64)),
            "daemon_sha256": format!("sha256:{raw_daemon}"),
            "history_fixture_sha256": format!("sha256:{}", "4".repeat(64)),
            "scenarios": scenarios,
        })
    }

    #[test]
    fn complete_native_subset_keeps_all_registry_rows_and_explicit_gaps() {
        for mode in [BenchMode::Cold, BenchMode::Warm] {
            let matrix = complete_matrix(mode);
            let valid = validate_matrix(&matrix).expect("valid complete Native subset");
            assert_eq!(valid.samples.len(), 14);
            let artifact = build_artifact(
                valid,
                HostV1 {
                    os: "unit-test".into(),
                    arch: "unit-test".into(),
                    cpu_count: 1,
                    mem_bytes: 1,
                    hostname_hash: "unit-test".into(),
                },
                ResourceUsageV1 { peak_rss_bytes: 1 },
                "sha256:unit-test".into(),
                matrix,
            );
            assert_eq!(artifact.rows.len(), 33);
            assert_eq!(
                artifact
                    .rows
                    .iter()
                    .filter(|row| row.latency.is_some())
                    .count(),
                14
            );
            assert_eq!(
                artifact
                    .rows
                    .iter()
                    .filter(
                        |row| row.early_stop_reason.as_deref() == Some("pending_owner_disposition")
                    )
                    .count(),
                19
            );
            assert_eq!(artifact.detail["qualification"], "diagnostic");
        }
    }

    #[test]
    fn rejects_same_count_foreign_scenario() {
        let mut matrix = complete_matrix(BenchMode::Cold);
        matrix["scenarios"][0]["scenario_id"] = json!("foreign.case");
        assert!(validate_matrix(&matrix).is_err());
    }

    #[test]
    fn matrix_reader_requires_bounded_regular_input() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let path = temp.path().join("matrix.json");
        fs::write(&path, b"{}")?;
        assert_eq!(read_matrix_bytes(&path)?, b"{}");
        let alias = temp.path().join("alias.json");
        symlink(&path, &alias)?;
        assert!(read_matrix_bytes(&alias).is_err());
        let fifo = temp.path().join("matrix.fifo");
        mkfifo(&fifo, Mode::S_IRUSR | Mode::S_IWUSR)?;
        assert!(read_matrix_bytes(&fifo).is_err());
        fs::write(&path, vec![0; usize::try_from(MAX_MATRIX_BYTES + 1)?])?;
        assert!(read_matrix_bytes(&path).is_err());
        Ok(())
    }

    #[test]
    fn rejects_null_publication_witnesses() {
        let mut text = complete_matrix(BenchMode::Cold);
        text["scenarios"][0]["reports"][0]["published"][0]["view"] = serde_json::Value::Null;
        assert!(validate_matrix(&text).is_err());

        let mut history = complete_matrix(BenchMode::Cold);
        history["scenarios"][7]["reports"][0]["history"]["published"]["cut"] =
            serde_json::Value::Null;
        assert!(validate_matrix(&history).is_err());
    }

    #[test]
    fn rejects_incomplete_native_roster() {
        let input = json!({
            "schema": "native-dsl-matrix-v1",
            "qualification": "diagnostic",
            "mode": "cold",
            "git_head": "0123456789abcdef0123456789abcdef01234567",
            "runner_sha256": format!("sha256:{}", "1".repeat(64)),
            "daemon_sha256": format!("sha256:{}", "2".repeat(64)),
            "history_fixture_sha256": format!("sha256:{}", "3".repeat(64)),
            "scenarios": [],
        });
        assert!(validate_matrix(&input).is_err());
    }

    #[test]
    fn rejects_cold_report_with_multiple_queries() {
        let report = json!({
            "passes": 1, "samples_per_pass": 2, "prime_queries": 0,
            "samples_ms": [1.0, 2.0],
            "latency": {"p50_ms": 1.0, "p95_ms": 2.0, "p99_ms": 2.0, "samples": 2},
        });
        assert!(samples_for_report(&report, BenchMode::Cold, "case").is_err());
    }

    #[test]
    fn rejects_summary_that_differs_from_raw_samples() {
        let report = json!({
            "passes": 1, "samples_per_pass": 1, "prime_queries": 0,
            "samples_ms": [1.0],
            "latency": {"p50_ms": 2.0, "p95_ms": 2.0, "p99_ms": 2.0, "samples": 1},
        });
        assert!(samples_for_report(&report, BenchMode::Cold, "case").is_err());
    }
}
