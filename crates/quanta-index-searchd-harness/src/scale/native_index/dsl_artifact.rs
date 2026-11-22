//! Convert a pinned Native DSL diagnostic matrix to the common benchmark envelope.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Read as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use nix::fcntl::OFlag;
use quanta_index_contract::{
    HistoryQueryRequest,
    index::ViewRef,
    native_protocol::{
        NativeDslPageV1, NativeDslQueryV1, NativeHistoryPage, NativeReadSessionHistoryCutV1,
    },
};
use serde::Deserialize as _;
use serde_json::{Value, json};

use super::dsl_history::{HISTORY_SAMPLES_SCHEMA, history_query, verify_sample};
use super::dsl_retained::{
    retained_config_digest, retained_dsl_query, retained_fixture_digest, verify_retained_page,
};
use super::dsl_text::{NATIVE_DSL_TEXT_REPOSITORY, native_text_fixture_digest};
use super::executable::NativeExecutable;
use crate::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, GitHeadV1, HostV1, LatencySummary,
    PhaseDurationsV1, ResourceUsageV1, config_digest, corpus_digest,
};
use crate::history_fixture::{HISTORY_LEDGER, history_fixture_corpus_files};
use crate::scenarios::{
    DslBenchScenario, FixtureKind, LEXICAL_CORPUS, NATIVE_TEXT_FILE_SCENARIOS, SCENARIOS,
    scenario_by_id,
};

const MATRIX_SCHEMA: &str = "native-dsl-matrix-v1";
const MAX_MATRIX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MatrixQualification {
    Diagnostic,
    Authority,
}

impl MatrixQualification {
    fn as_str(self) -> &'static str {
        match self {
            Self::Diagnostic => "diagnostic",
            Self::Authority => "authority",
        }
    }
}

struct ValidatedMatrix {
    qualification: MatrixQualification,
    mode: BenchMode,
    head: GitHeadV1,
    runner_sha256: String,
    daemon_sha256: String,
    history_fixture_sha256: String,
    text_fixture_sha256: String,
    text_config_sha256: String,
    history_config_sha256: String,
    retained_fixture_sha256: BTreeMap<String, String>,
    retained_config_sha256: BTreeMap<String, String>,
    timing: ReportTiming,
    text_query_deadline_ms: f64,
    reports_per_scenario: usize,
    samples: BTreeMap<String, Vec<f64>>,
    report_count: usize,
    daemon_peak_rss_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ReportTiming {
    passes: u64,
    samples_per_pass: u64,
    prime_queries: u64,
    cooldown_ms: f64,
    pass_settle_ms: f64,
}

/// Assemble a validated common-schema artifact. Authority additionally requires
/// complete Linux daemon RSS and the registered per-row sample floor.
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
    validate_qualification_host(valid.qualification, &host)?;
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
    let _read = file
        .take(MAX_MATRIX_BYTES + 1)
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

fn samples_for_report(
    report: &Value,
    mode: BenchMode,
    id: &str,
) -> Result<(Vec<f64>, ReportTiming)> {
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
    Ok((
        samples,
        ReportTiming {
            passes,
            samples_per_pass: per_pass,
            prime_queries: prime,
            cooldown_ms,
            pass_settle_ms,
        },
    ))
}

fn finite_nonnegative(report: &Value, field: &str, id: &str) -> Result<f64> {
    report
        .get(field)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .with_context(|| format!("Native DSL {id} has missing or invalid {field}"))
}

fn validate_history_report(
    report: &Value,
    scenario: &DslBenchScenario,
    binding: &mut Option<NativeReadSessionHistoryCutV1>,
) -> Result<()> {
    ensure!(
        required_str(report, "result_shape")? == scenario.expected_shape.as_str(),
        "History result shape differs"
    );
    let query =
        HistoryQueryRequest::deserialize(report.get("query").context("missing History query")?)
            .context("decode History query")?;
    ensure!(
        query == history_query(scenario),
        "History query differs from canonical scenario"
    );
    let witness = NativeReadSessionHistoryCutV1::deserialize(
        report
            .get("published")
            .context("missing History publication witness")?,
    )
    .context("decode History publication witness")?;
    let page = NativeHistoryPage::deserialize(
        report
            .get("observed_page")
            .context("missing observed History SDK page")?,
    )
    .context("decode observed History SDK page")?;
    page.validate_reply(
        witness.session,
        witness.view,
        witness.instance,
        &query,
        None,
    )
    .map_err(anyhow::Error::msg)?;
    let generation = &witness.cut.generation;
    let fixture = HISTORY_LEDGER.build_batch(
        generation.repo_id.clone(),
        generation.revision_id.clone(),
        generation.generation,
    )?;
    verify_sample(&page, &witness, &fixture, scenario.expected_shape)?;
    ensure!(
        scenario.expected_count == Some(page.matched),
        "History count differs from scenario gold"
    );
    if let Some(previous) = binding {
        // Every process opens its own session against the one retained fixture.
        ensure!(
            previous.view == witness.view
                && previous.instance == witness.instance
                && previous.member == witness.member
                && previous.cut == witness.cut,
            "History reports name different published members or cuts"
        );
    } else {
        *binding = Some(witness);
    }
    Ok(())
}

fn validate_retained_report(report: &Value, scenario: &DslBenchScenario) -> Result<()> {
    let query =
        NativeDslQueryV1::deserialize(report.get("query").context("missing retained DSL query")?)
            .context("decode retained DSL query")?;
    ensure!(
        query == retained_dsl_query(scenario)?,
        "retained DSL query differs from canonical scenario"
    );
    let query_digest = query.digest().map_err(anyhow::Error::msg)?;
    let published = required_array(report, "published")?;
    ensure!(
        u64::try_from(published.len())?
            == report
                .get("passes")
                .and_then(Value::as_u64)
                .context("missing retained DSL passes")?,
        "retained DSL publication count differs"
    );
    for item in published {
        let view = ViewRef::deserialize(item.get("view").context("missing retained DSL view")?)
            .context("decode retained DSL view")?;
        let page =
            NativeDslPageV1::deserialize(item.get("page").context("missing retained DSL page")?)
                .context("decode retained DSL page")?;
        ensure!(
            page.query == query_digest,
            "retained DSL page query differs from the registered query"
        );
        verify_retained_page(scenario, view, &page)?;
    }
    Ok(())
}

fn validate_matrix(input: &Value) -> Result<ValidatedMatrix> {
    ensure!(
        required_str(input, "schema")? == MATRIX_SCHEMA,
        "wrong Native DSL matrix schema"
    );
    let qualification = match required_str(input, "qualification")? {
        "diagnostic" => MatrixQualification::Diagnostic,
        "authority" => MatrixQualification::Authority,
        other => anyhow::bail!("unsupported Native DSL qualification {other}"),
    };
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
                    .filter(|s| s.fixture == FixtureKind::LexicalCorpus
                        && s.expected_typed_error_code == Some("PARSE_FAIL"))
                    .count()
                + SCENARIOS
                    .iter()
                    .filter(|s| s.fixture == FixtureKind::HistoryLedger)
                    .count()
                + SCENARIOS
                    .iter()
                    .filter(|s| matches!(
                        s.fixture,
                        FixtureKind::RuntimeCatalog | FixtureKind::StructuralTree
                    ))
                    .count(),
        "Native DSL matrix must contain the complete Native Text, error, History and retained roster"
    );
    let mut expected = BTreeMap::new();
    for (id, paths) in NATIVE_TEXT_FILE_SCENARIOS {
        ensure!(
            expected.insert(*id, ("text", Some(*paths))).is_none(),
            "duplicate Native DSL registry ID"
        );
    }
    for scenario in SCENARIOS.iter().filter(|s| {
        s.fixture == FixtureKind::LexicalCorpus && s.expected_typed_error_code == Some("PARSE_FAIL")
    }) {
        ensure!(
            expected.insert(scenario.id, ("error", None)).is_none(),
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
    for scenario in SCENARIOS.iter().filter(|s| {
        matches!(
            s.fixture,
            FixtureKind::RuntimeCatalog | FixtureKind::StructuralTree
        )
    }) {
        ensure!(
            expected.insert(scenario.id, ("retained", None)).is_none(),
            "duplicate Native DSL registry ID"
        );
    }
    ensure!(
        NATIVE_TEXT_FILE_SCENARIOS.len() == 9 && expected.len() == 33,
        "Native DSL supported roster changed; review artifact assembly"
    );
    let mut seen = BTreeSet::new();
    let mut all_samples = BTreeMap::new();
    let mut report_count = 0;
    let mut text_fixture = None;
    let mut text_config = None;
    let mut history_config = None;
    let mut retained_fixture = BTreeMap::new();
    let mut retained_config = BTreeMap::new();
    let mut history_binding = None;
    let mut common_timing = None;
    let mut text_query_deadline_ms = None;
    let mut cold_repeats = None;
    let mut daemon_peak_rss_bytes = Some(0_u64);
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
            daemon_peak_rss_bytes =
                accumulate_daemon_peak_rss(daemon_peak_rss_bytes, wrapper, kind == "error", id)?;
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
            let expected_schema = if kind == "history" {
                HISTORY_SAMPLES_SCHEMA.to_owned()
            } else {
                format!("native-dsl-{kind}-samples-v1")
            };
            ensure!(
                required_str(report, "schema")? == expected_schema,
                "Native DSL {id} report schema differs"
            );
            ensure!(
                required_str(report, "scenario_id")? == id,
                "Native DSL report scenario differs"
            );
            if kind == "text" || kind == "error" {
                ensure!(
                    required_str(report, "query_text")? == scenario.query.resolve().as_ref(),
                    "Native DSL {id} Text query differs"
                );
                ensure!(
                    required_str(report, "query_syntax")? == scenario.syntax.as_str(),
                    "Native DSL {id} syntax differs"
                );
                if kind == "text" {
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
                } else {
                    let count = report
                        .get("passes")
                        .and_then(Value::as_u64)
                        .and_then(|passes| {
                            report
                                .get("samples_per_pass")
                                .and_then(Value::as_u64)
                                .and_then(|samples| passes.checked_mul(samples))
                        })
                        .context("Native DSL error sample count is missing or overflows")?;
                    ensure!(
                        required_str(report, "result_shape")? == scenario.expected_shape.as_str()
                            && required_str(report, "typed_error_code")?
                                == scenario
                                    .expected_typed_error_code
                                    .context("missing error gold")?
                            && report.get("error_count").and_then(Value::as_u64) == Some(count)
                            && report
                                .get("engine_touched")
                                .and_then(Value::as_array)
                                .is_some_and(Vec::is_empty)
                            && required_str(report, "measurement_scope")? == "client_preflight"
                            && report.get("matched_files").is_none()
                            && report.get("result_unit").is_none(),
                        "Native DSL {id} error oracle or measurement scope differs"
                    );
                }
                let fixture = required_str(report, "fixture_sha256")?;
                let config = required_str(report, "config_template_sha256")?;
                ensure!(
                    required_str(report, "source_repo_id")? == NATIVE_DSL_TEXT_REPOSITORY
                        && fixture == native_text_fixture_digest()?,
                    "Native DSL {id} Text fixture bytes or repository differ from canonical corpus"
                );
                raw_sha256(config, "Text config_template_sha256")?;
                retain_equal(&mut text_fixture, fixture, "Text fixture")?;
                retain_equal(&mut text_config, config, "Text config")?;
                let deadline = finite_nonnegative(report, "query_deadline_ms", id)?;
                ensure!(deadline > 0.0, "Native DSL {id} has zero query deadline");
                if let Some(previous) = text_query_deadline_ms {
                    ensure!(
                        previous == deadline,
                        "Native DSL Text query deadline differs across reports"
                    );
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
            } else if kind == "history" {
                validate_history_report(report, scenario, &mut history_binding)
                    .with_context(|| format!("Native DSL {id} History evidence"))?;
            } else {
                let fixture = required_str(report, "fixture_sha256")?;
                let config = required_str(report, "config_template_sha256")?;
                sha256(fixture, "retained fixture_sha256")?;
                ensure!(
                    config == retained_config_digest()?,
                    "Native DSL {id} retained config differs from the canonical workload"
                );
                ensure!(
                    fixture == retained_fixture_digest(scenario.fixture)?,
                    "Native DSL {id} retained fixture differs from canonical source"
                );
                let key = scenario.fixture.as_str().to_owned();
                retain_equal(
                    retained_fixture.entry(key.clone()).or_insert(None),
                    fixture,
                    "retained fixture",
                )?;
                retain_equal(
                    retained_config.entry(key).or_insert(None),
                    config,
                    "retained config",
                )?;
                validate_retained_report(report, scenario)
                    .with_context(|| format!("Native DSL {id} retained evidence"))?;
            }
            let (report_samples, timing) = samples_for_report(report, mode, id)?;
            if let Some(previous) = common_timing {
                ensure!(
                    previous == timing,
                    "Native DSL timing differs across reports"
                );
            } else {
                common_timing = Some(timing);
            }
            samples.extend(report_samples);
        }
        ensure!(
            all_samples.insert(id.to_owned(), samples).is_none(),
            "duplicate Native DSL sample row {id}"
        );
    }
    ensure!(
        seen.len() == expected.len(),
        "Native DSL matrix is missing supported scenarios"
    );
    if qualification == MatrixQualification::Authority {
        ensure!(
            daemon_peak_rss_bytes.is_some(),
            "Native DSL authority requires measured RSS for every daemon-backed report"
        );
        let sample_floor = if mode == BenchMode::Warm { 200 } else { 20 };
        for (id, samples) in &all_samples {
            ensure!(
                samples.len() >= sample_floor,
                "Native DSL authority {id} requires at least {sample_floor} samples"
            );
        }
    }
    Ok(ValidatedMatrix {
        qualification,
        mode,
        head,
        runner_sha256,
        daemon_sha256,
        history_fixture_sha256,
        text_fixture_sha256: text_fixture.context("missing Text fixture binding")?,
        text_config_sha256: text_config.context("missing Text config binding")?,
        history_config_sha256: history_config.context("missing History config binding")?,
        retained_fixture_sha256: retained_fixture
            .into_iter()
            .map(|(kind, value)| Ok((kind, value.context("missing retained fixture binding")?)))
            .collect::<Result<_>>()?,
        retained_config_sha256: retained_config
            .into_iter()
            .map(|(kind, value)| Ok((kind, value.context("missing retained config binding")?)))
            .collect::<Result<_>>()?,
        timing: common_timing.context("missing Native DSL timing")?,
        text_query_deadline_ms: text_query_deadline_ms.context("missing Text query deadline")?,
        reports_per_scenario: cold_repeats.unwrap_or(1),
        samples: all_samples,
        report_count,
        daemon_peak_rss_bytes,
    })
}

fn validate_qualification_host(qualification: MatrixQualification, host: &HostV1) -> Result<()> {
    if qualification == MatrixQualification::Authority {
        ensure!(
            host.os == "linux",
            "Native DSL authority requires a Linux assembler host"
        );
    }
    Ok(())
}

fn accumulate_daemon_peak_rss(
    current: Option<u64>,
    report: &Value,
    client_error: bool,
    id: &str,
) -> Result<Option<u64>> {
    let peak = match report.get("daemon_peak_rss_bytes") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_u64()
                .filter(|bytes| *bytes > 0)
                .with_context(|| format!("Native DSL {id} daemon RSS is invalid"))?,
        ),
    };
    if client_error {
        ensure!(
            peak.is_none(),
            "Native DSL {id} client error has daemon RSS"
        );
        return Ok(current);
    }
    Ok(current
        .zip(peak)
        .map(|(measured, observed)| measured.max(observed)))
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
    let resources = valid
        .daemon_peak_rss_bytes
        .map_or(resources, |peak_rss_bytes| ResourceUsageV1 {
            peak_rss_bytes,
        });
    let dimension = format!("dsl-{}", valid.mode.as_str());
    let measured_ids = valid.samples.keys().cloned().collect::<Vec<_>>();
    let remaining_ids = SCENARIOS
        .iter()
        .filter(|s| !valid.samples.contains_key(s.id))
        .map(|s| s.id)
        .collect::<Vec<_>>();
    let error_ids = SCENARIOS
        .iter()
        .filter(|scenario| {
            scenario.expected_typed_error_code == Some("PARSE_FAIL")
                && valid.samples.contains_key(scenario.id)
        })
        .map(|scenario| scenario.id)
        .collect::<BTreeSet<_>>();
    let measurement_scope_by_scenario = error_ids
        .iter()
        .map(|id| ((*id).to_owned(), "client_preflight"))
        .collect::<BTreeMap<_, _>>();
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
                error_count: if error_ids.contains(scenario.id) {
                    samples.map_or(0, |values| values.len() as u64)
                } else {
                    0
                },
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
                typed_error_code: if error_ids.contains(scenario.id) {
                    scenario.expected_typed_error_code.map(str::to_owned)
                } else {
                    None
                },
                engine_touched: if samples.is_some() && !error_ids.contains(scenario.id) {
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
    let mut corpus_files = LEXICAL_CORPUS
        .iter()
        .map(|(path, content)| ((*path).to_owned(), (*content).to_owned()))
        .collect::<Vec<_>>();
    corpus_files.extend(history_fixture_corpus_files());
    let corpus_binding = json!({
        "source_bytes_sha256": corpus_digest(&dimension, &corpus_files),
        "text_fixture_sha256": valid.text_fixture_sha256,
        "retained_fixture_sha256": valid.retained_fixture_sha256,
    });
    let corpus_digest = format!(
        "sha256:{}",
        super::hex_sha256(corpus_binding.to_string().as_bytes())
    );
    let config_digest = config_digest(
        &dimension,
        &[
            ("text_config_sha256", valid.text_config_sha256.clone()),
            ("history_config_sha256", valid.history_config_sha256.clone()),
            (
                "retained_config_sha256",
                serde_json::to_string(&valid.retained_config_sha256)
                    .expect("validated retained config map"),
            ),
            ("mode", valid.mode.as_str().into()),
            ("passes", valid.timing.passes.to_string()),
            (
                "samples_per_pass",
                valid.timing.samples_per_pass.to_string(),
            ),
            ("prime_queries", valid.timing.prime_queries.to_string()),
            ("cooldown_ms", valid.timing.cooldown_ms.to_string()),
            ("pass_settle_ms", valid.timing.pass_settle_ms.to_string()),
            (
                "query_deadline_ms",
                valid.text_query_deadline_ms.to_string(),
            ),
            (
                "reports_per_scenario",
                valid.reports_per_scenario.to_string(),
            ),
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
            "qualification": valid.qualification.as_str(),
            "source_schema": MATRIX_SCHEMA,
            "native_matrix_sha256": native_matrix_sha256,
            "native_matrix": native_matrix,
            "native_subset": { "measured": measured_ids.len(), "total": SCENARIOS.len(),
                "measured_ids": measured_ids, "remaining_ids": remaining_ids,
                "remaining_disposition": "pending_owner_disposition",
                "measurement_scope_by_scenario": measurement_scope_by_scenario },
            "report_count": valid.report_count,
            "runner_sha256": valid.runner_sha256,
            "daemon_sha256": valid.daemon_sha256,
            "history_fixture_sha256": valid.history_fixture_sha256,
            "text_fixture_sha256": valid.text_fixture_sha256,
            "text_config_sha256": valid.text_config_sha256,
            "history_config_sha256": valid.history_config_sha256,
            "retained_fixture_sha256": valid.retained_fixture_sha256,
            "retained_config_sha256": valid.retained_config_sha256,
            "resource_scope": if valid.daemon_peak_rss_bytes.is_some() {
                "maximum Linux daemon VmHWM across daemon-backed reports"
            } else {
                "assembler process RUSAGE_SELF only; complete daemon RSS unavailable in matrix"
            },
            "host_scope": "assembler host; pinned frozen runner invokes assembly after measurement",
            "corpus_digest_scope": "canonical Text and History source bytes plus retained RuntimeCatalog/StructuralTree fixture digests, independent of executable",
            "timing": {"passes": valid.timing.passes, "samples_per_pass": valid.timing.samples_per_pass,
                "prime_queries": valid.timing.prime_queries, "cooldown_ms": valid.timing.cooldown_ms,
                "pass_settle_ms": valid.timing.pass_settle_ms, "query_deadline_ms": valid.text_query_deadline_ms,
                "reports_per_scenario": valid.reports_per_scenario},
            "sample_scope": "SDK latency samples; error rows measure client preflight and have no daemon query; per-sample oracle enforced by pinned Native runner",
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;
    use quanta_index_contract::{
        AuxEpochV1, CommitCandidate, DiffCandidate, HistoryOrderV1, ManifestGeneration,
        QueryResultWindowV1, QueryResultWindowV2, RepoId, RevisionId,
        index::{
            AuxiliaryGenerationKeyV1, CollectionId, Digest, HistorySourceCutV1, IndexInstanceId,
            ObjectRef, SlotId, TenantId, ViewRef, WorkspaceKey, WorktreeId,
        },
        native_protocol::{NativeHistoryPage, NativeReadSessionHistoryCutV1, NativeReadSessionId},
    };
    use serde_json::{Value, json};
    use tempfile::TempDir;

    use super::super::dsl_retained::{candidate_gold, runtime_metadata_source};
    use super::{
        HISTORY_SAMPLES_SCHEMA, MAX_MATRIX_BYTES, MatrixQualification, NATIVE_DSL_TEXT_REPOSITORY,
        build_artifact, history_query, native_text_fixture_digest, read_matrix_bytes,
        retained_config_digest, retained_dsl_query, retained_fixture_digest, samples_for_report,
        validate_matrix, validate_qualification_host,
    };
    use crate::artifact::{BenchArtifactV1, BenchMode, HostV1, ResourceUsageV1, ResultShape};
    use crate::history_fixture::HISTORY_LEDGER;
    use crate::scenarios::{DslBenchScenario, FixtureKind, NATIVE_TEXT_FILE_SCENARIOS, SCENARIOS};

    fn history_page_fixture(
        shape: ResultShape,
    ) -> (NativeReadSessionHistoryCutV1, NativeHistoryPage) {
        let fixture = HISTORY_LEDGER
            .build_batch(
                RepoId::new("history-fixture").expect("static repository"),
                RevisionId::new("history-rev").expect("static revision"),
                ManifestGeneration::new(1),
            )
            .expect("static History fixture");
        let witness = NativeReadSessionHistoryCutV1 {
            session: NativeReadSessionId([1; 32]),
            view: ViewRef {
                workspace: WorkspaceKey {
                    tenant: TenantId([1; 16]),
                    collection: CollectionId([2; 16]),
                    worktree: WorktreeId([3; 16]),
                    slot: SlotId([4; 16]),
                },
                epoch: 1,
                sequence: 2,
                digest: Digest([3; 32]),
            },
            instance: IndexInstanceId([4; 16]),
            member: ObjectRef(Digest([5; 32])),
            cut: HistorySourceCutV1 {
                generation: AuxiliaryGenerationKeyV1 {
                    repo_id: fixture.repo_id.clone(),
                    revision_id: fixture.revision_id.clone(),
                    generation: fixture.generation,
                },
                durable_epoch: AuxEpochV1::new(1),
                snapshot_digest: Digest([6; 32]),
                replay_start: Digest([7; 32]),
            },
        };
        let commit = fixture.commits.first().expect("fixture commit");
        let diff = fixture.diff_hunks.first().expect("fixture diff hunk");
        let page = NativeHistoryPage {
            session: witness.session,
            view: witness.view,
            instance: witness.instance,
            member: witness.member,
            order: HistoryOrderV1::Recency,
            commits: (shape == ResultShape::Commits)
                .then(|| CommitCandidate {
                    sha: commit.sha,
                    parent_ids: commit.parents.clone(),
                    committed_at_unix_s: 0,
                    author: commit.author.to_string(),
                    committer: commit.committer.to_string(),
                    message: commit.message.to_string(),
                    is_merge: commit.is_merge,
                    tags: commit.tags.iter().map(ToString::to_string).collect(),
                    score: None,
                })
                .into_iter()
                .collect(),
            diffs: (shape == ResultShape::DiffPaths)
                .then(|| DiffCandidate {
                    repo_relative_path: diff.file_path.to_string(),
                    hunk_header: diff.record.hunk_header.to_string(),
                    side: diff.record.side,
                    line_start: diff.record.byte_start,
                    line_end: diff.record.byte_end,
                    snippet: diff.record.touched_text.to_string(),
                    score: None,
                })
                .into_iter()
                .collect(),
            matched: 1,
            remaining: 1,
            examined: 1,
            read_epoch: witness.cut.durable_epoch,
            window: QueryResultWindowV2::exact_probe(1),
            after: None,
        };
        (witness, page)
    }

    fn retained_report(scenario: &DslBenchScenario, prime: u64) -> Value {
        let (witness, _) = history_page_fixture(ResultShape::Commits);
        let query = retained_dsl_query(scenario).expect("canonical retained query");
        let ids = candidate_gold(scenario.id).expect("canonical retained IDs");
        let rows = if scenario.fixture == FixtureKind::RuntimeCatalog {
            let rows = ids
                .iter()
                .map(|id| {
                    let (path, snippet) =
                        runtime_metadata_source(id).expect("RuntimeMetadata fixture source");
                    json!({"candidate_id": id, "path": path, "source_repo_id": "repo-e2e",
                    "start_line": 1, "end_line": 1, "snippet": snippet})
                })
                .collect::<Vec<_>>();
            json!({"RuntimeMetadata": rows})
        } else {
            json!({"Structural": [{
                "candidate": {"candidate_id": "e2e-5-src/tree.rs", "bindings": [{
                    "metavariable": "name", "start_byte": 14, "end_byte": 33,
                    "start_line": 2, "end_line": 2
                }]},
                "match_range": {"byte_start": 11, "byte_end": 37, "line_start": 2, "line_end": 2},
                "source_repo_id": "repo-e2e", "path": "src/tree.rs",
                "pattern_kind": null, "matched_node": null
            }]})
        };
        json!({
            "schema": "native-dsl-retained-samples-v1", "scenario_id": scenario.id,
            "query": query, "daemon_sha256": "2".repeat(64),
            "config_template_sha256": retained_config_digest().expect("canonical retained config"),
            "fixture_sha256": retained_fixture_digest(scenario.fixture).expect("canonical retained fixture"),
            "published": [{"view": witness.view, "page": {"view": witness.view,
                "projection": witness.member, "session": witness.session,
                "query": query.digest().expect("query digest"), "rows": rows, "after": null,
                "window": QueryResultWindowV1::exact(u32::try_from(ids.len()).unwrap())}}],
            "passes": 1, "samples_per_pass": 1, "prime_queries": prime,
            "cooldown_ms": 0.0, "pass_settle_ms": 0.0,
            "samples_ms": [1.0],
            "latency": {"p50_ms": 1.0, "p95_ms": 1.0, "p99_ms": 1.0, "samples": 1},
        })
    }

    fn complete_matrix_without_errors(mode: BenchMode) -> serde_json::Value {
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
                "fixture_sha256": native_text_fixture_digest().expect("canonical Text fixture"),
                "source_repo_id": NATIVE_DSL_TEXT_REPOSITORY,
                "query_deadline_ms": 1000.0,
                "passes": 1, "samples_per_pass": 1, "prime_queries": prime,
                "cooldown_ms": 0.0, "pass_settle_ms": 0.0,
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
            let (witness, page) = history_page_fixture(scenario.expected_shape);
            let report = json!({
                "schema": HISTORY_SAMPLES_SCHEMA, "scenario_id": scenario.id,
                "query": history_query(scenario),
                "result_shape": scenario.expected_shape.as_str(),
                "published": witness, "observed_page": page,
                "passes": 1, "samples_per_pass": 1, "prime_queries": prime,
                "cooldown_ms": 0.0, "pass_settle_ms": 0.0,
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
        for scenario in SCENARIOS.iter().filter(|scenario| {
            matches!(
                scenario.fixture,
                FixtureKind::RuntimeCatalog | FixtureKind::StructuralTree
            )
        }) {
            scenarios.push(json!({"scenario_id": scenario.id, "kind": "retained",
                "reports": [retained_report(scenario, prime)]}));
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

    fn complete_matrix(mode: BenchMode) -> serde_json::Value {
        let mut matrix = complete_matrix_without_errors(mode);
        let prime = if mode == BenchMode::Cold { 0 } else { 1 };
        for scenario in SCENARIOS
            .iter()
            .filter(|scenario| scenario.expected_typed_error_code == Some("PARSE_FAIL"))
        {
            let report = json!({
                "schema": "native-dsl-error-samples-v1", "scenario_id": scenario.id,
                "query_text": scenario.query.resolve(), "query_syntax": scenario.syntax.as_str(),
                "result_shape": scenario.expected_shape.as_str(),
                "typed_error_code": "PARSE_FAIL", "error_count": 1,
                "engine_touched": [], "measurement_scope": "client_preflight",
                "daemon_sha256": "2".repeat(64),
                "config_template_sha256": "3".repeat(64),
                "fixture_sha256": native_text_fixture_digest().expect("canonical Text fixture"),
                "source_repo_id": NATIVE_DSL_TEXT_REPOSITORY,
                "query_deadline_ms": 1000.0,
                "passes": 1, "samples_per_pass": 1, "prime_queries": prime,
                "cooldown_ms": 0.0, "pass_settle_ms": 0.0,
                "published": [{"view": {}, "source_cut": {}, "operations": [{"submit_count": 1, "completion": "immediate_ack"}]}],
                "samples_ms": [1.0],
                "latency": {"p50_ms": 1.0, "p95_ms": 1.0, "p99_ms": 1.0, "samples": 1},
            });
            matrix["scenarios"]
                .as_array_mut()
                .expect("fixed scenario array")
                .push(json!({"scenario_id": scenario.id, "kind": "error", "reports": [report]}));
        }
        matrix
    }

    fn authority_matrix(mode: BenchMode) -> Value {
        let mut matrix = complete_matrix(mode);
        matrix["qualification"] = json!("authority");
        for row in matrix["scenarios"].as_array_mut().expect("scenario rows") {
            let client_error = row["kind"] == "error";
            let history = row["kind"] == "history";
            if mode == BenchMode::Cold {
                let report = row["reports"][0].clone();
                row["reports"] = json!(vec![report; 20]);
            }
            for wrapper in row["reports"].as_array_mut().expect("SDK reports") {
                if !client_error {
                    wrapper["daemon_peak_rss_bytes"] = json!(4096);
                }
                if mode == BenchMode::Warm {
                    let report = if history {
                        &mut wrapper["history"]
                    } else {
                        wrapper
                    };
                    report["samples_per_pass"] = json!(200);
                    report["samples_ms"] = json!(vec![1.0; 200]);
                    report["latency"]["samples"] = json!(200);
                    if client_error {
                        report["error_count"] = json!(200);
                    }
                }
            }
        }
        matrix
    }

    fn artifact_for_test(matrix: serde_json::Value) -> BenchArtifactV1 {
        let valid = validate_matrix(&matrix).expect("valid complete Native subset");
        build_artifact(
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
        )
    }

    #[test]
    fn complete_native_matrix_measures_all_registry_rows() {
        for mode in [BenchMode::Cold, BenchMode::Warm] {
            let matrix = complete_matrix(mode);
            let valid = validate_matrix(&matrix).expect("valid complete Native matrix");
            assert_eq!(valid.samples.len(), 33);
            let artifact = artifact_for_test(matrix);
            assert_eq!(artifact.rows.len(), 33);
            assert!(
                artifact
                    .rows
                    .iter()
                    .all(|row| row.latency.is_some() && row.early_stop_reason.is_none())
            );
            assert_eq!(artifact.detail["qualification"], "diagnostic");
        }
    }

    #[test]
    fn authority_matrix_requires_complete_daemon_rss_sample_floor_and_linux_host() {
        for mode in [BenchMode::Warm, BenchMode::Cold] {
            let matrix = authority_matrix(mode);
            let valid = validate_matrix(&matrix).expect("complete authority matrix");
            assert_eq!(valid.qualification, MatrixQualification::Authority);
            assert_eq!(valid.daemon_peak_rss_bytes, Some(4096));
            let mut linux = HostV1 {
                os: "linux".into(),
                arch: "x86_64".into(),
                cpu_count: 1,
                mem_bytes: 1,
                hostname_hash: "unit-test".into(),
            };
            assert!(validate_qualification_host(valid.qualification, &linux).is_ok());
            linux.os = "macos".into();
            assert!(validate_qualification_host(valid.qualification, &linux).is_err());
            assert!(validate_qualification_host(MatrixQualification::Diagnostic, &linux).is_ok());
            let artifact = artifact_for_test(matrix.clone());
            assert_eq!(artifact.detail["qualification"], "authority");
            assert_eq!(artifact.resources.peak_rss_bytes, 4096);
            assert_eq!(
                artifact.detail["resource_scope"],
                "maximum Linux daemon VmHWM across daemon-backed reports"
            );

            let mut missing_rss = matrix.clone();
            let daemon_row = missing_rss["scenarios"]
                .as_array_mut()
                .expect("scenario rows")
                .iter_mut()
                .find(|row| row["kind"] != "error")
                .expect("daemon-backed row");
            daemon_row["reports"][0]["daemon_peak_rss_bytes"] = Value::Null;
            assert!(
                validate_matrix(&missing_rss)
                    .err()
                    .expect("authority must reject missing measured RSS")
                    .to_string()
                    .contains("measured RSS")
            );
            missing_rss["qualification"] = json!("diagnostic");
            assert!(validate_matrix(&missing_rss).is_ok());

            let mut underfloor = matrix.clone();
            if mode == BenchMode::Warm {
                for row in underfloor["scenarios"]
                    .as_array_mut()
                    .expect("scenario rows")
                {
                    let client_error = row["kind"] == "error";
                    let history = row["kind"] == "history";
                    let wrapper = &mut row["reports"][0];
                    let report = if history {
                        &mut wrapper["history"]
                    } else {
                        wrapper
                    };
                    report["samples_per_pass"] = json!(199);
                    report["samples_ms"] = json!(vec![1.0; 199]);
                    report["latency"]["samples"] = json!(199);
                    if client_error {
                        report["error_count"] = json!(199);
                    }
                }
            } else {
                for row in underfloor["scenarios"]
                    .as_array_mut()
                    .expect("scenario rows")
                {
                    assert!(
                        row["reports"]
                            .as_array_mut()
                            .expect("SDK reports")
                            .pop()
                            .is_some()
                    );
                }
            }
            assert!(
                validate_matrix(&underfloor)
                    .err()
                    .expect("authority must reject samples below the floor")
                    .to_string()
                    .contains("at least")
            );
        }
        let mut unknown = authority_matrix(BenchMode::Warm);
        unknown["qualification"] = json!("unverified");
        assert!(validate_matrix(&unknown).is_err());
    }

    #[test]
    fn only_complete_daemon_rss_can_replace_assembler_resources() {
        let mut matrix = complete_matrix(BenchMode::Warm);
        let rows = matrix["scenarios"].as_array_mut().expect("scenario rows");
        let mut daemon_row = None;
        let mut error_row = None;
        for (index, row) in rows.iter_mut().enumerate() {
            if row["kind"] == "error" {
                error_row = Some(index);
            } else {
                daemon_row = Some(index);
                row["reports"][0]["daemon_peak_rss_bytes"] = json!(4096);
            }
        }
        let daemon_row = daemon_row.expect("daemon row");
        let error_row = error_row.expect("client error row");
        let rss_field = "daemon_peak_rss_bytes";
        matrix["scenarios"][daemon_row]["reports"][0][rss_field] = json!(8192);
        let artifact = artifact_for_test(matrix.clone());
        assert_eq!(artifact.resources.peak_rss_bytes, 8192);
        assert_eq!(
            artifact.detail["resource_scope"],
            "maximum Linux daemon VmHWM across daemon-backed reports"
        );

        matrix["scenarios"][daemon_row]["reports"][0][rss_field] = Value::Null;
        let artifact = artifact_for_test(matrix.clone());
        assert_eq!(artifact.resources.peak_rss_bytes, 1);
        assert_eq!(
            validate_matrix(&matrix).unwrap().daemon_peak_rss_bytes,
            None
        );
        matrix["scenarios"][daemon_row]["reports"][0][rss_field] = json!(0);
        assert!(validate_matrix(&matrix).is_err());
        matrix["scenarios"][daemon_row]["reports"][0][rss_field] = json!(8192);
        matrix["scenarios"][error_row]["reports"][0][rss_field] = json!(4096);
        assert!(validate_matrix(&matrix).is_err());
    }

    #[test]
    fn retained_report_refuses_unrelated_view_query_or_gold() {
        let matrix = complete_matrix(BenchMode::Cold);
        assert!(validate_matrix(&matrix).is_ok());
        let row = matrix["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .position(|row| row["scenario_id"] == "structural.tree_match.native")
            .unwrap();
        let mut changed = matrix.clone();
        changed["scenarios"][row]["reports"][0]["published"][0]["page"]["view"]["sequence"] =
            json!(7);
        assert!(
            validate_matrix(&changed).is_err(),
            "accepted another published view"
        );
        let mut changed = matrix.clone();
        changed["scenarios"][row]["reports"][0]["config_template_sha256"] = json!("5".repeat(64));
        assert!(
            validate_matrix(&changed).is_err(),
            "accepted a different retained workload"
        );
        let mut changed = matrix.clone();
        changed["scenarios"][row]["reports"][0]["published"][0]["page"]["query"] =
            json!(Digest([9; 32]));
        assert!(
            validate_matrix(&changed).is_err(),
            "accepted an unrelated query digest"
        );
        for (field, replacement, expected_error) in [
            (
                "/candidate/bindings/0/end_byte",
                json!(32),
                "Structural capture differs",
            ),
            (
                "/source_repo_id",
                json!("foreign-repo"),
                "Structural source or match range differs",
            ),
            (
                "/match_range/byte_end",
                json!(36),
                "Structural source or match range differs",
            ),
        ] {
            let mut changed = matrix.clone();
            let hit = &mut changed["scenarios"][row]["reports"][0]["published"][0]["page"]["rows"]
                ["Structural"][0];
            *hit.pointer_mut(field).expect("known Structural field") = replacement;
            let error = validate_matrix(&changed)
                .err()
                .expect("altered Structural row");
            assert!(format!("{error:#}").contains(expected_error));
        }
    }

    #[test]
    fn retained_runtime_metadata_refuses_changed_payload_with_the_right_candidate_id() {
        let matrix = complete_matrix(BenchMode::Cold);
        assert!(validate_matrix(&matrix).is_ok());
        let index = matrix["scenarios"]
            .as_array()
            .expect("scenario rows")
            .iter()
            .position(|row| row["scenario_id"] == "runtime.changed.sourcegraph")
            .expect("canonical RuntimeMetadata scenario");
        for (field, value) in [
            ("path", json!("src/stale.rs")),
            ("source_repo_id", json!("foreign-repo")),
            ("start_line", json!(2)),
            ("end_line", json!(2)),
            ("snippet", json!("fn unrelated() {}")),
        ] {
            let mut changed = matrix.clone();
            let row = &mut changed["scenarios"][index]["reports"][0]["published"][0]["page"]["rows"]
                ["RuntimeMetadata"][0];
            row[field] = value;
            let error = validate_matrix(&changed).err().expect("altered payload");
            assert!(format!("{error:#}").contains("RuntimeMetadata payload differs"));
        }
    }

    #[test]
    fn history_samples_require_observed_rows_and_one_retained_source() {
        let matrix = complete_matrix(BenchMode::Cold);
        assert!(validate_matrix(&matrix).is_ok());
        let index = matrix["scenarios"]
            .as_array()
            .expect("scenario array")
            .iter()
            .position(|row| row["scenario_id"] == "history.since_commit.native")
            .expect("canonical History scenario");
        for (field, replacement) in [
            ("/observed_page", Value::Null),
            ("/observed_page/matched", json!(0)),
            (
                "/observed_page/commits/0/message",
                json!("foreign revision"),
            ),
            ("/published/member", json!(vec![6_u8; 32])),
        ] {
            let mut invalid = matrix.clone();
            let report = &mut invalid["scenarios"][index]["reports"][0]["history"];
            *report.pointer_mut(field).expect("known report field") = replacement;
            assert!(
                validate_matrix(&invalid).is_err(),
                "accepted altered {field}"
            );
        }
        let mut mixed = matrix.clone();
        let report = &mut mixed["scenarios"][index]["reports"][0]["history"];
        report["published"]["view"]["sequence"] = json!(3);
        report["observed_page"]["view"]["sequence"] = json!(3);
        let error = validate_matrix(&mixed)
            .err()
            .expect("must reject mixed view");
        assert!(format!("{error:#}").contains("different published members or cuts"));
    }

    #[test]
    fn repository_predicate_requires_its_sourcegraph_query_and_exact_file_gold() {
        let valid = complete_matrix(BenchMode::Cold);
        assert!(validate_matrix(&valid).is_ok());
        let position = valid["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .position(|row| row["scenario_id"] == "lexical.repo_has_file.sourcegraph")
            .expect("repository predicate is in the maintained Native roster");
        assert_eq!(
            valid["scenarios"][position]["reports"][0]["matched_files"],
            json!(["src/lib.rs"])
        );
        for (field, forged) in [
            ("query_text", json!("needle")),
            ("query_syntax", json!("native")),
            ("matched_files", json!(["src/other.rs"])),
        ] {
            let mut matrix = valid.clone();
            matrix["scenarios"][position]["reports"][0][field] = forged;
            assert!(validate_matrix(&matrix).is_err(), "accepted forged {field}");
        }
        let mut omitted = valid;
        let _removed = omitted["scenarios"]
            .as_array_mut()
            .unwrap()
            .remove(position);
        assert!(validate_matrix(&omitted).is_err());
    }

    #[test]
    fn native_error_gold_is_measured_as_client_preflight_without_engine_claim() {
        for mode in [BenchMode::Cold, BenchMode::Warm] {
            let matrix = complete_matrix(mode);
            let valid = validate_matrix(&matrix).expect("33 supported Native DSL scenarios");
            assert_eq!(valid.samples.len(), 33);
            let artifact = artifact_for_test(matrix);
            assert_eq!(artifact.rows.len(), 33);
            assert_eq!(artifact.detail["qualification"], "diagnostic");
            assert_eq!(artifact.detail["native_subset"]["measured"], 33);
            assert_eq!(
                artifact.detail["native_subset"]["remaining_ids"]
                    .as_array()
                    .unwrap()
                    .len(),
                0
            );
            for scenario in SCENARIOS
                .iter()
                .filter(|scenario| scenario.expected_typed_error_code == Some("PARSE_FAIL"))
            {
                let row = artifact
                    .rows
                    .iter()
                    .find(|row| row.scenario_id == scenario.id)
                    .expect("measured error row");
                assert!(row.latency.is_some(), "{} missing SDK latency", scenario.id);
                assert_eq!(row.error_count, 1, "{} error count", scenario.id);
                assert_eq!(
                    row.typed_error_code.as_deref(),
                    Some("PARSE_FAIL"),
                    "{} code",
                    scenario.id
                );
                assert!(
                    row.engine_touched.is_empty(),
                    "{} claimed daemon execution",
                    scenario.id
                );
                assert_eq!(row.result_count, None, "{} has result rows", scenario.id);
                assert_eq!(
                    row.early_stop_reason, None,
                    "{} is still pending",
                    scenario.id
                );
                assert_eq!(
                    artifact.detail["native_subset"]["measurement_scope_by_scenario"][scenario.id],
                    "client_preflight"
                );
            }
        }
    }

    #[test]
    fn native_error_reports_refuse_missing_or_forged_gold_and_binary() {
        let valid = complete_matrix(BenchMode::Cold);
        assert!(
            validate_matrix(&valid).is_ok(),
            "fixed 33-row input is rejected"
        );
        let error_index = valid["scenarios"]
            .as_array()
            .unwrap()
            .iter()
            .position(|row| row["kind"] == "error")
            .unwrap();

        for (field, forged) in [
            ("typed_error_code", json!("INVALID_INPUT")),
            ("error_count", json!(0)),
            ("engine_touched", json!(["native"])),
            ("measurement_scope", json!("daemon_query")),
            ("query_text", json!("needle")),
            ("daemon_sha256", json!("9".repeat(64))),
        ] {
            let mut matrix = valid.clone();
            matrix["scenarios"][error_index]["reports"][0][field] = forged;
            assert!(validate_matrix(&matrix).is_err(), "accepted forged {field}");
        }
        let mut missing = valid;
        let omitted = missing["scenarios"]
            .as_array_mut()
            .unwrap()
            .remove(error_index);
        assert_eq!(omitted["kind"], "error");
        assert!(
            validate_matrix(&missing).is_err(),
            "accepted missing error row"
        );
    }

    #[test]
    fn rejects_same_count_foreign_scenario() {
        let mut matrix = complete_matrix(BenchMode::Cold);
        matrix["scenarios"][0]["scenario_id"] = json!("foreign.case");
        assert!(validate_matrix(&matrix).is_err());
    }

    #[test]
    fn executable_digest_does_not_change_corpus_or_config_digest() {
        let first = artifact_for_test(complete_matrix(BenchMode::Warm));
        let mut second_matrix = complete_matrix(BenchMode::Warm);
        second_matrix["runner_sha256"] = json!(format!("sha256:{}", "9".repeat(64)));
        let second = artifact_for_test(second_matrix);
        assert_eq!(
            first.provenance.corpus_digest,
            second.provenance.corpus_digest
        );
        assert_eq!(
            first.provenance.config_digest,
            second.provenance.config_digest
        );
    }

    #[test]
    fn refuses_timing_drift_between_scenarios() {
        let mut matrix = complete_matrix(BenchMode::Warm);
        matrix["scenarios"][0]["reports"][0]["cooldown_ms"] = json!(5.0);
        assert!(validate_matrix(&matrix).is_err());
    }

    #[test]
    fn matrix_reader_requires_bounded_regular_input() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let path = temp.path().join("matrix.json");
        fs::write(&path, b"{}")?;
        assert_eq!(read_matrix_bytes(&path)?.as_slice(), b"{}");
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
    fn native_text_fixture_rejects_missing_or_foreign_repository_identity() {
        let valid = complete_matrix(BenchMode::Cold);
        assert!(validate_matrix(&valid).is_ok());
        for repository in [
            None,
            Some(json!("different-fixture-repository")),
            Some(serde_json::Value::Null),
        ] {
            let mut matrix = valid.clone();
            let report = matrix["scenarios"]
                .as_array_mut()
                .expect("fixture scenarios")
                .iter_mut()
                .find(|row| row["kind"] == "text")
                .expect("Text fixture row")["reports"][0]
                .as_object_mut()
                .expect("Text fixture report");
            if let Some(repository) = repository {
                let _previous = report.insert("source_repo_id".into(), repository);
            } else {
                let _previous = report.remove("source_repo_id");
            }
            assert!(
                validate_matrix(&matrix).is_err(),
                "fixture repository identity must be bound independently of its paths"
            );
        }
    }

    #[test]
    fn rejects_null_publication_witnesses() {
        let mut text = complete_matrix(BenchMode::Cold);
        let text_row = text["scenarios"]
            .as_array_mut()
            .expect("fixture scenarios")
            .iter_mut()
            .find(|row| row["kind"] == "text")
            .expect("Text fixture row");
        text_row["reports"][0]["published"][0]["view"] = serde_json::Value::Null;
        assert!(validate_matrix(&text).is_err());

        let mut history = complete_matrix(BenchMode::Cold);
        let history_row = history["scenarios"]
            .as_array_mut()
            .expect("fixture scenarios")
            .iter_mut()
            .find(|row| row["kind"] == "history")
            .expect("History fixture row");
        history_row["reports"][0]["history"]["published"]["cut"] = serde_json::Value::Null;
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
            "cooldown_ms": 0.0, "pass_settle_ms": 0.0,
            "samples_ms": [1.0, 2.0],
            "latency": {"p50_ms": 1.0, "p95_ms": 2.0, "p99_ms": 2.0, "samples": 2},
        });
        assert!(samples_for_report(&report, BenchMode::Cold, "case").is_err());
    }

    #[test]
    fn rejects_summary_that_differs_from_raw_samples() {
        let report = json!({
            "passes": 1, "samples_per_pass": 1, "prime_queries": 0,
            "cooldown_ms": 0.0, "pass_settle_ms": 0.0,
            "samples_ms": [1.0],
            "latency": {"p50_ms": 2.0, "p95_ms": 2.0, "p99_ms": 2.0, "samples": 1},
        });
        assert!(samples_for_report(&report, BenchMode::Cold, "case").is_err());
    }
}
