//! Native file query samples for the existing DSL matrix executables.

use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, ensure};
use quanta_index_contract::TextQueryRequest;
use quanta_index_contract::index::{Digest, SourceTextAdmission, ViewSelection};
use quanta_index_core::RequestBudgetV1;
use quanta_index_sdk::native_uds::{BorrowedSourceInput, NativeUdsClient};
use serde_json::{Value, json};

use super::retained_read::{QUERY_DEADLINE, text_query};
use super::workload::{FIXED_FOREGROUND_WORK_LIMITS, NativeForegroundWorkload, NativeWorkloadMode};
use super::{
    NativeDaemon, NativeExecutable, PublicationAudit, PublicationLabel, advance_staged, budget,
    cold_source_mutations, configure_text_index, create_workspace, hex_sha256, native_config,
    operation, text_spec,
};
use crate::artifact::{LatencySummary, corpus_digest_refs};
use crate::scenarios::{LEXICAL_CORPUS, NATIVE_TEXT_FILE_SCENARIOS, TOP_K, scenario_by_id};

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
    let (_, expected_paths) = NATIVE_TEXT_FILE_SCENARIOS
        .iter()
        .copied()
        .find(|(id, _)| *id == scenario_id)
        .with_context(|| format!("scenario {scenario_id} has no Native file oracle"))?;
    let scenario = scenario_by_id(scenario_id).context("Native file scenario is unregistered")?;
    let mut request = text_query(&scenario.query.resolve(), TOP_K);
    request.syntax = scenario.syntax.into();
    let executable = NativeExecutable::capture(daemon_bin)?;
    let workload = NativeForegroundWorkload {
        limits: FIXED_FOREGROUND_WORK_LIMITS,
        mode: NativeWorkloadMode::Fixed,
    };
    let mut samples_ms = Vec::new();
    let mut published = Vec::new();
    for _ in 0..timing.passes {
        let (samples, pass) = measure_pass(&executable, workload, &request, expected_paths, timing)
            .with_context(|| format!("Native file DSL scenario {scenario_id}"))?;
        samples_ms.extend(samples);
        published.push(pass);
    }
    let latency = LatencySummary::from_samples_ms(&samples_ms).context("empty DSL sample set")?;
    let canonical_config = native_config(
        Path::new("/fixture-state"),
        Path::new("/fixture.sock"),
        workload,
    );
    Ok(json!({
        "schema": "native-dsl-text-samples-v1",
        "scenario_id": scenario_id,
        "query_text": request.query_text,
        "query_syntax": scenario.syntax.as_str(),
        "result_unit": "file",
        "matched_files": expected_paths,
        "daemon_sha256": executable.sha256(),
        "config_template_sha256": hex_sha256(&serde_json::to_vec(&canonical_config)?),
        "fixture_sha256": corpus_digest_refs("native-dsl-text", LEXICAL_CORPUS.iter().copied()),
        "query_deadline_ms": QUERY_DEADLINE.as_secs_f64() * 1000.0,
        "passes": timing.passes,
        "samples_per_pass": timing.samples,
        "prime_queries": timing.prime_queries,
        "cooldown_ms": timing.cooldown.as_secs_f64() * 1000.0,
        "pass_settle_ms": timing.pass_settle.as_secs_f64() * 1000.0,
        "published": published,
        "samples_ms": samples_ms,
        "latency": latency,
    }))
}

fn measure_pass(
    executable: &NativeExecutable,
    workload: NativeForegroundWorkload,
    request: &TextQueryRequest,
    expected_paths: &[&str],
    timing: NativeDslTiming,
) -> Result<(Vec<f64>, Value)> {
    let temp = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let socket = temp.path().join("native.sock");
    let config = temp.path().join("native.json");
    let config_bytes = serde_json::to_vec(&native_config(
        &temp.path().join("state"),
        &socket,
        workload,
    ))?;
    fs::write(&config, &config_bytes)?;
    fs::set_permissions(&config, fs::Permissions::from_mode(0o600))?;
    let stderr = temp.path().join("daemon.stderr");
    // No recording: this runner times the SDK call with diagnostics disabled.
    let daemon = NativeDaemon::start(executable, &config, &socket, None, 0, &stderr)?;
    let run = (|| -> Result<(Vec<f64>, Value)> {
        let client = NativeUdsClient::new(socket);
        let mut publication = PublicationAudit::default();
        let genesis = create_workspace(&client)?;
        let configured = configure_text_index(&client, genesis, &mut publication)?;
        let ingress = client.begin_input(operation(2), &budget())?;
        let inputs = LEXICAL_CORPUS
            .iter()
            .map(|&(path, text)| BorrowedSourceInput {
                path,
                bytes: text.as_bytes(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
            })
            .collect::<Vec<_>>();
        let refs = client.stage_sources_borrowed(ingress, &inputs, &budget())?;
        ensure!(
            refs.len() == inputs.len(),
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
            let result =
                client.search_text(ViewSelection::Exact(view), instance, request, &budget)?;
            let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
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
            Ok(latency_ms)
        };
        let samples_ms = timing.sample_pass(sample)?;
        Ok((
            samples_ms,
            json!({"view": view, "source_cut": selected.source,
            "config_sha256": hex_sha256(&config_bytes),
            "operations": publication.detail()}),
        ))
    })();
    match (run, daemon.stop(&mut None)) {
        (Ok(samples), Ok(())) => {
            ensure!(
                fs::read(&config)? == config_bytes,
                "Native DSL config changed during measurement"
            );
            Ok(samples)
        }
        (Err(cause), Ok(())) | (Ok(_), Err(cause)) => Err(cause),
        (Err(cause), Err(drain)) => {
            Err(cause.context(format!("Native daemon drain failed: {drain:#}")))
        }
    }
}
