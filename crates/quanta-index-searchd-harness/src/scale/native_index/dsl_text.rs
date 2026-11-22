//! Native file query samples for the existing DSL matrix executables.

use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, ensure};
use quanta_index_contract::index::{Digest, SourceInput, SourceTextAdmission, ViewSelection};
use quanta_index_contract::native_protocol::{NativeIndexRequest, NativeIndexResponse};
use quanta_index_contract::{RepoId, TextQueryRequest};
use quanta_index_core::RequestBudgetV1;
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};
use serde_json::{Value, json};

use super::retained_read::{QUERY_DEADLINE, text_query};
use super::workload::{FIXED_FOREGROUND_WORK_LIMITS, NativeForegroundWorkload, NativeWorkloadMode};
use super::{
    NativeExecutable, NativeSession, PublicationAudit, PublicationLabel, advance_staged, budget,
    cold_source_mutations, configure_text_index, create_workspace, hex_sha256, native_config,
    operation, text_spec,
};
use crate::artifact::{LatencySummary, corpus_digest_refs};
use crate::scenarios::{
    FixtureKind, LEXICAL_CORPUS, NATIVE_TEXT_FILE_SCENARIOS, TOP_K, scenario_by_id,
};

#[derive(Clone, Copy)]
enum NativeTextOracle {
    Files(&'static [&'static str]),
    ParseFail,
}

pub(super) const NATIVE_DSL_TEXT_REPOSITORY: &str = "native-dsl-fixture";

/// Repository membership is an input, independent of a source's path or body.
pub(super) fn native_text_fixture_digest() -> Result<String> {
    let binding = json!({
        "source_bytes_sha256": corpus_digest_refs("native-dsl-text", LEXICAL_CORPUS.iter().copied()),
        "source_repo_id": NATIVE_DSL_TEXT_REPOSITORY,
    });
    Ok(format!(
        "sha256:{}",
        hex_sha256(&serde_json::to_vec(&binding)?)
    ))
}

/// Each pass uses a new process and publishes the shared fixture before measuring.
#[derive(Clone, Copy)]
pub struct NativeDslTiming {
    pub passes: usize,
    pub samples: usize,
    pub prime_queries: usize,
    pub cooldown: Duration,
    pub pass_settle: Duration,
}

impl NativeDslTiming {
    /// One measured query in a newly started process, with no priming.
    pub const PROCESS_COLD: Self = Self {
        passes: 1,
        samples: 1,
        prime_queries: 0,
        cooldown: Duration::ZERO,
        pass_settle: Duration::ZERO,
    };

    pub(super) fn sample_pass(self, mut sample: impl FnMut() -> Result<f64>) -> Result<Vec<f64>> {
        for _ in 0..self.prime_queries {
            let _warmup_latency_ms = sample()?;
            thread::sleep(self.pass_settle);
        }
        let mut samples_ms = Vec::new();
        for _ in 0..self.samples {
            thread::sleep(self.cooldown);
            samples_ms.push(sample()?);
        }
        Ok(samples_ms)
    }
}

/// Measure the public SDK call for one supported DSL file scenario.
///
/// This emits Native file diagnostics. Its result unit and schema keep the
/// samples separate from the Legacy warm/cold comparison artifacts.
pub fn measure_native_dsl_text(
    daemon_bin: &Path,
    scenario_id: &str,
    timing: NativeDslTiming,
) -> Result<Value> {
    ensure!(
        timing.passes > 0 && timing.samples > 0,
        "no DSL samples requested"
    );
    let measured_queries = timing
        .passes
        .checked_mul(timing.samples)
        .context("Native DSL measured query count overflow")?;
    let expected_paths = NATIVE_TEXT_FILE_SCENARIOS
        .iter()
        .copied()
        .find(|(id, _)| *id == scenario_id)
        .map(|(_, paths)| paths);
    let scenario = scenario_by_id(scenario_id).context("Native file scenario is unregistered")?;
    let oracle = match (expected_paths, scenario.expected_typed_error_code) {
        (Some(paths), None) => NativeTextOracle::Files(paths),
        (None, Some("PARSE_FAIL")) if scenario.fixture == FixtureKind::LexicalCorpus => {
            NativeTextOracle::ParseFail
        }
        _ => anyhow::bail!("scenario {scenario_id} has no supported Native Text oracle"),
    };
    let mut request = text_query(&scenario.query.resolve(), TOP_K);
    request.syntax = scenario.syntax.into();
    let executable = NativeExecutable::capture(daemon_bin)?;
    let workload = NativeForegroundWorkload {
        limits: FIXED_FOREGROUND_WORK_LIMITS,
        mode: NativeWorkloadMode::Fixed,
    };
    let mut samples_ms = Vec::new();
    let mut published = Vec::new();
    let mut daemon_peak_rss_bytes = None;
    for _ in 0..timing.passes {
        let (samples, pass, peak_rss_bytes) =
            measure_pass(&executable, workload, &request, oracle, timing)
                .with_context(|| format!("Native file DSL scenario {scenario_id}"))?;
        samples_ms.extend(samples);
        published.push(pass);
        daemon_peak_rss_bytes = daemon_peak_rss_bytes.max(peak_rss_bytes);
    }
    let latency = LatencySummary::from_samples_ms(&samples_ms).context("empty DSL sample set")?;
    let canonical_config = native_config(
        Path::new("/fixture-state"),
        Path::new("/fixture.sock"),
        workload,
    );
    let mut report = json!({
        "schema": if matches!(oracle, NativeTextOracle::ParseFail) {
            "native-dsl-error-samples-v1"
        } else {
            "native-dsl-text-samples-v1"
        },
        "scenario_id": scenario_id,
        "query_text": request.query_text,
        "query_syntax": scenario.syntax.as_str(),
        "daemon_sha256": executable.sha256(),
        "daemon_peak_rss_bytes": daemon_peak_rss_bytes,
        "config_template_sha256": hex_sha256(&serde_json::to_vec(&canonical_config)?),
        "fixture_sha256": native_text_fixture_digest()?,
        "source_repo_id": NATIVE_DSL_TEXT_REPOSITORY,
        "query_deadline_ms": QUERY_DEADLINE.as_secs_f64() * 1000.0,
        "passes": timing.passes,
        "samples_per_pass": timing.samples,
        "prime_queries": timing.prime_queries,
        "cooldown_ms": timing.cooldown.as_secs_f64() * 1000.0,
        "pass_settle_ms": timing.pass_settle.as_secs_f64() * 1000.0,
        "published": published,
        "samples_ms": samples_ms,
        "latency": latency,
    });
    match oracle {
        NativeTextOracle::Files(paths) => {
            report["result_unit"] = json!("file");
            report["matched_files"] = json!(paths);
        }
        NativeTextOracle::ParseFail => {
            report["result_shape"] = json!(scenario.expected_shape.as_str());
            report["typed_error_code"] = json!("PARSE_FAIL");
            report["error_count"] = json!(measured_queries);
            report["engine_touched"] = json!([]);
            report["measurement_scope"] = json!("client_preflight");
        }
    }
    Ok(report)
}

fn measure_pass(
    executable: &NativeExecutable,
    workload: NativeForegroundWorkload,
    request: &TextQueryRequest,
    oracle: NativeTextOracle,
    timing: NativeDslTiming,
) -> Result<(Vec<f64>, Value, Option<u64>)> {
    // No recording: this runner times the SDK call with diagnostics disabled.
    let daemon = NativeSession::start_owned_pinned(executable, |root, socket| {
        Ok(native_config(root, socket, workload))
    })?;
    let config_bytes = daemon.config_bytes().to_vec();
    let run = (|| -> Result<_> {
        let client = NativeUdsClient::new(daemon.socket_path().to_path_buf());
        let mut publication = PublicationAudit::default();
        let genesis = create_workspace(&client)?;
        let configured = configure_text_index(&client, genesis, &mut publication)?;
        let ingress = client.begin_input(operation(2), &budget())?;
        let inputs = LEXICAL_CORPUS
            .iter()
            .map(|&(path, text)| SourceInput {
                path: path.into(),
                bytes: text.as_bytes().to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
                language: None,
            })
            .collect::<Vec<_>>();
        let input_count = inputs.len();
        let repository = RepoId::new(NATIVE_DSL_TEXT_REPOSITORY)?;
        let NativeIndexResponse::StagedSources(refs) = client.request(
            NativeIndexRequest::StageRepositorySources {
                ingress,
                repository,
                inputs,
            },
            &budget(),
        )?
        else {
            anyhow::bail!("Native DSL repository fixture staging returned an unexpected reply");
        };
        ensure!(
            refs.len() == input_count,
            "Native DSL fixture staging is incomplete"
        );
        let view = advance_staged(
            &client,
            ingress,
            configured,
            cold_source_mutations(&refs)?,
            1,
            PublicationLabel {
                phase: "full_event_ack",
                cycle: None,
            },
            &mut publication,
        )?;
        let selected = client.read_view(ViewSelection::Exact(view), &budget())?;
        let query_text = request.query_text.as_str();
        let instance = text_spec().instance;
        let sample = || -> Result<f64> {
            let request = request.clone();
            let budget = RequestBudgetV1::for_duration(QUERY_DEADLINE);
            let started = Instant::now();
            let result = client.search_text(ViewSelection::Exact(view), instance, request, &budget);
            let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
            match oracle {
                NativeTextOracle::Files(expected_paths) => {
                    let result = result?;
                    let mut paths = result
                        .hits
                        .iter()
                        .map(|hit| hit.path.as_str())
                        .collect::<Vec<_>>();
                    paths.sort_unstable();
                    ensure!(
                        result.view == view
                            && result.exhaustive
                            && result.matched == u64::try_from(expected_paths.len())?
                            && paths.as_slice() == expected_paths,
                        "Native DSL file oracle mismatch for {:?}: matched {}, paths {paths:?}",
                        query_text,
                        result.matched,
                    );
                }
                NativeTextOracle::ParseFail => match result {
                    Err(NativeUdsError::QueryRejected { code, message }) => {
                        ensure!(
                            code.as_wire_str() == "PARSE_FAIL" && !message.is_empty(),
                            "Native DSL typed parse oracle mismatch for {query_text:?}: {code}: {message}"
                        );
                    }
                    Err(other) => anyhow::bail!(
                        "Native DSL expected local PARSE_FAIL for {query_text:?}, got {other}"
                    ),
                    Ok(_) => anyhow::bail!(
                        "Native DSL expected local PARSE_FAIL for {query_text:?}, got success"
                    ),
                },
            }
            Ok(latency_ms)
        };
        let samples_ms = timing.sample_pass(sample)?;
        let peak_rss_bytes = match oracle {
            NativeTextOracle::Files(_) => daemon.linux_peak_rss_bytes()?,
            NativeTextOracle::ParseFail => None,
        };
        Ok((
            samples_ms,
            json!({"view": view, "source_cut": selected.source,
            "config_sha256": hex_sha256(&config_bytes),
            "operations": publication.detail()}),
            peak_rss_bytes,
        ))
    })();
    match (
        run,
        daemon.stop_with(|config| {
            ensure!(
                fs::read(config)? == config_bytes,
                "Native DSL config changed during measurement"
            );
            Ok(())
        }),
    ) {
        (Ok(samples), Ok(())) => Ok(samples),
        (Err(cause), Ok(())) | (Ok(_), Err(cause)) => Err(cause),
        (Err(cause), Err(drain)) => {
            Err(cause.context(format!("Native daemon drain failed: {drain:#}")))
        }
    }
}
