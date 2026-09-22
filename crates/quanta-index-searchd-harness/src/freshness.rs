//! Workspace mutation to lexical visibility, through real filesystem operations
//! and the harness's ingest, seal, activation and query UDS front doors.
//!
//! The producer is explicit: after each filesystem mutation this rail reads the
//! changed file and publishes it. It does not measure a filesystem watcher.
//! `receipt_to_visible_ms` starts when the final ingest call returns a
//! successful response and ends at the first correct, generation-pinned query;
//! it includes seal and activation. Every transition checks both presence and
//! absence, response generation, and every candidate's generation.

use std::fs;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context as _, Result as AnyResult, anyhow, ensure};
use quanta_index_contract::{GenerationPin, ManifestGeneration, TextQuerySyntax};
use quanta_index_searchd_harness::E2eRuntime;
use quanta_index_searchd_harness::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    LatencySummary, PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest,
    corpus_digest, model_revision_of,
};
use serde_json::{Value, json};
use tempfile::TempDir;

const DIMENSION: &str = "freshness";
const REPO: &str = "freshness-workspace";
const UPDATE_PATH: &str = "src/update.rs";
const RENAME_FROM: &str = "src/rename.rs";
const RENAME_TO: &str = "src/moved.rs";
const OLD_TOKEN: &str = "freshness_old_anchor";
const NEW_TOKEN: &str = "freshness_new_anchor";
const RENAME_TOKEN: &str = "freshness_rename_anchor";
const OLD_CONTENT: &str = "fn old() { let freshness_old_anchor = 1; }\n";
const NEW_CONTENT: &str = "fn new() { let freshness_new_anchor = 2; }\n";
const RENAME_CONTENT: &str = "fn moved() { let freshness_rename_anchor = 3; }\n";
const TOP_K: u32 = 16;
const MAX_SAMPLES: u32 = 64;
const HISTORY_GENERATIONS: usize = 8;

#[derive(Clone, Debug)]
pub(crate) struct TransitionMeasurement {
    pub mutation_to_visible_ms: f64,
    pub receipt_to_visible_ms: f64,
    pub ingest_ms: f64,
    pub ingest_through_seal_ms: f64,
    pub seal_ms: f64,
    pub activation_ms: f64,
    pub first_query_ms: f64,
    pub generation: u64,
}

impl TransitionMeasurement {
    fn to_json(&self) -> Value {
        json!({
            "mutation_to_visible_ms": self.mutation_to_visible_ms,
            "receipt_to_visible_ms": self.receipt_to_visible_ms,
            "ingest_ms": self.ingest_ms,
            "ingest_through_seal_ms": self.ingest_through_seal_ms,
            "seal_ms": self.seal_ms,
            "activation_ms": self.activation_ms,
            "first_query_ms": self.first_query_ms,
            "generation": self.generation,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FreshnessSample {
    pub base_build_ms: f64,
    pub update: TransitionMeasurement,
    pub delete: TransitionMeasurement,
    pub rename: TransitionMeasurement,
}

impl FreshnessSample {
    fn to_json(&self) -> Value {
        json!({
            "base_build_ms": self.base_build_ms,
            "update": self.update.to_json(),
            "delete": self.delete.to_json(),
            "rename": self.rename.to_json(),
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FreshnessReport {
    pub samples: Vec<FreshnessSample>,
    pub model_revision: Option<String>,
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

fn between_ms(started: Instant, ended: Instant) -> f64 {
    ended.duration_since(started).as_secs_f64() * 1_000.0
}

fn pin(rt: &E2eRuntime, generation: ManifestGeneration) -> GenerationPin {
    GenerationPin::new(rt.repo(), rt.revision(), generation)
}

fn expect_paths(
    rt: &mut E2eRuntime,
    generation: ManifestGeneration,
    token: &str,
    paths: &[&str],
) -> AnyResult<()> {
    let expected_pin = pin(rt, generation);
    let page = rt
        .query_text_page(
            TextQuerySyntax::Native,
            token,
            TOP_K,
            Some(expected_pin.clone()),
            None,
        )?
        .served(token)?;
    ensure!(
        page.generation == expected_pin,
        "freshness: {token} response generation {:?}, expected {:?}",
        page.generation,
        expected_pin
    );
    let mut actual = Vec::with_capacity(page.results.len());
    for candidate in &page.results {
        ensure!(
            candidate.repo_id == expected_pin.repo_id
                && candidate.revision_id == expected_pin.revision_id
                && candidate.manifest_generation == generation,
            "freshness: {token} candidate {} has incoherent repo/revision/generation",
            candidate.candidate_id
        );
        ensure!(
            candidate.snippet.contains(token),
            "freshness: {token} candidate {} has no matching snippet",
            candidate.candidate_id
        );
        actual.push(candidate.repo_relative_path.as_str().to_string());
    }
    actual.sort();
    let mut expected = paths
        .iter()
        .map(|path| (*path).to_string())
        .collect::<Vec<_>>();
    expected.sort();
    ensure!(
        actual == expected,
        "freshness: {token} generation {} paths {actual:?}, expected {expected:?}",
        generation.get()
    );
    Ok(())
}

fn seal_activate_and_observe(
    rt: &mut E2eRuntime,
    mutation_started: Instant,
    ingest_started: Instant,
    receipt_at: Instant,
    token: &str,
    expected_paths: &[&str],
) -> AnyResult<TransitionMeasurement> {
    let seal_started = Instant::now();
    let generation = rt.seal()?;
    let sealed_at = Instant::now();
    let seal_ms = between_ms(seal_started, sealed_at);
    let ingest_ms = between_ms(ingest_started, receipt_at);
    let ingest_through_seal_ms = between_ms(ingest_started, sealed_at);
    let activation_started = Instant::now();
    rt.activate_last_sealed_generation()?;
    let activation_ms = elapsed_ms(activation_started);
    let query_started = Instant::now();
    expect_paths(rt, generation, token, expected_paths)?;
    let visible_at = Instant::now();
    let first_query_ms = between_ms(query_started, visible_at);
    let receipt_to_visible_ms = between_ms(receipt_at, visible_at);
    let mutation_to_visible_ms = between_ms(mutation_started, visible_at);
    Ok(TransitionMeasurement {
        mutation_to_visible_ms,
        receipt_to_visible_ms,
        ingest_ms,
        ingest_through_seal_ms,
        seal_ms,
        activation_ms,
        first_query_ms,
        generation: generation.get(),
    })
}

/// One independent workspace and daemon. All generations are retained so
/// historical pins can be checked after the final activation.
pub(crate) fn run_one() -> AnyResult<(FreshnessSample, Option<String>)> {
    let workspace = TempDir::new().context("freshness: create workspace")?;
    let src = workspace.path().join("src");
    fs::create_dir(&src).context("freshness: create src directory")?;
    fs::write(workspace.path().join(UPDATE_PATH), OLD_CONTENT)?;
    fs::write(workspace.path().join(RENAME_FROM), RENAME_CONTENT)?;

    let mut rt = E2eRuntime::boot_with_history_max_generations(HISTORY_GENERATIONS)?;
    let model_revision = model_revision_of(rt.embedder_profile());
    let base_started = Instant::now();
    for path in [UPDATE_PATH, RENAME_FROM] {
        rt.ingest_text(
            REPO,
            path,
            &fs::read_to_string(workspace.path().join(path))?,
        )?;
    }
    let base = rt.seal()?;
    let base_build_ms = elapsed_ms(base_started);
    rt.activate_last_sealed_generation()?;
    expect_paths(&mut rt, base, OLD_TOKEN, &[UPDATE_PATH])?;
    expect_paths(&mut rt, base, RENAME_TOKEN, &[RENAME_FROM])?;

    let update_started = Instant::now();
    fs::write(workspace.path().join(UPDATE_PATH), NEW_CONTENT)?;
    let ingest_started = Instant::now();
    rt.ingest_text(
        REPO,
        UPDATE_PATH,
        &fs::read_to_string(workspace.path().join(UPDATE_PATH))?,
    )?;
    let receipt_at = Instant::now();
    let update = seal_activate_and_observe(
        &mut rt,
        update_started,
        ingest_started,
        receipt_at,
        NEW_TOKEN,
        &[UPDATE_PATH],
    )?;
    let updated_generation = ManifestGeneration::new(update.generation);
    expect_paths(&mut rt, updated_generation, OLD_TOKEN, &[])?;
    expect_paths(&mut rt, updated_generation, RENAME_TOKEN, &[RENAME_FROM])?;

    let delete_started = Instant::now();
    fs::remove_file(workspace.path().join(UPDATE_PATH))?;
    let ingest_started = Instant::now();
    rt.delete_chunk_for_path(UPDATE_PATH)?;
    let receipt_at = Instant::now();
    let delete = seal_activate_and_observe(
        &mut rt,
        delete_started,
        ingest_started,
        receipt_at,
        NEW_TOKEN,
        &[],
    )?;
    let deleted_generation = ManifestGeneration::new(delete.generation);
    expect_paths(&mut rt, deleted_generation, OLD_TOKEN, &[])?;
    expect_paths(&mut rt, deleted_generation, RENAME_TOKEN, &[RENAME_FROM])?;

    let rename_started = Instant::now();
    fs::rename(
        workspace.path().join(RENAME_FROM),
        workspace.path().join(RENAME_TO),
    )?;
    let ingest_started = Instant::now();
    rt.delete_chunk_for_path(RENAME_FROM)?;
    rt.ingest_text(
        REPO,
        RENAME_TO,
        &fs::read_to_string(workspace.path().join(RENAME_TO))?,
    )?;
    let receipt_at = Instant::now();
    let rename = seal_activate_and_observe(
        &mut rt,
        rename_started,
        ingest_started,
        receipt_at,
        RENAME_TOKEN,
        &[RENAME_TO],
    )?;
    ensure!(
        !workspace.path().join(RENAME_FROM).exists(),
        "freshness: rename source still exists"
    );
    ensure!(
        !workspace.path().join(UPDATE_PATH).exists(),
        "freshness: deleted file still exists"
    );
    // Historical pins must remain immutable across update/delete/rename.
    expect_paths(&mut rt, base, OLD_TOKEN, &[UPDATE_PATH])?;
    expect_paths(&mut rt, base, RENAME_TOKEN, &[RENAME_FROM])?;
    expect_paths(&mut rt, updated_generation, NEW_TOKEN, &[UPDATE_PATH])?;
    expect_paths(&mut rt, deleted_generation, RENAME_TOKEN, &[RENAME_FROM])?;
    let final_generation = ManifestGeneration::new(rename.generation);
    expect_paths(&mut rt, final_generation, OLD_TOKEN, &[])?;
    expect_paths(&mut rt, final_generation, NEW_TOKEN, &[])?;

    Ok((
        FreshnessSample {
            base_build_ms,
            update,
            delete,
            rename,
        },
        model_revision,
    ))
}

pub(crate) fn run(samples: u32) -> AnyResult<FreshnessReport> {
    ensure!(
        (1..=MAX_SAMPLES).contains(&samples),
        "freshness: samples must be in 1..={MAX_SAMPLES}"
    );
    let mut measured = Vec::with_capacity(usize::try_from(samples)?);
    let mut model_revision = None;
    for _ in 0..samples {
        let (sample, model) = run_one()?;
        if let Some(previous) = &model_revision {
            ensure!(
                previous == &model,
                "freshness: embedding model changed during run"
            );
        } else {
            model_revision = Some(model);
        }
        measured.push(sample);
    }
    Ok(FreshnessReport {
        samples: measured,
        model_revision: model_revision.ok_or_else(|| anyhow!("freshness: no samples"))?,
    })
}

fn row(
    name: &str,
    samples: &[FreshnessSample],
    select: impl Fn(&FreshnessSample) -> &TransitionMeasurement,
    result_count: u64,
) -> AnyResult<BenchRowV1> {
    let elapsed = samples
        .iter()
        .map(|sample| select(sample).receipt_to_visible_ms)
        .collect::<Vec<_>>();
    let latency = LatencySummary::from_samples_ms(&elapsed)
        .ok_or_else(|| anyhow!("freshness: {name} has no timing samples"))?;
    Ok(BenchRowV1 {
        scenario_id: format!("freshness.{name}.receipt_to_visible"),
        route_family: RouteFamily::Lexical,
        syntax: BenchSyntax::Native,
        result_shape: if result_count == 0 {
            ResultShape::Empty
        } else {
            ResultShape::Candidates
        },
        latency: Some(latency),
        qps: None,
        error_count: 0,
        timeout_count: 0,
        result_count: Some(result_count),
        typed_error_code: None,
        engine_touched: vec!["Lexical".to_string()],
        early_stop_reason: None,
    })
}

pub(crate) fn artifact(
    report: &FreshnessReport,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<BenchArtifactV1> {
    ensure!(!report.samples.is_empty(), "freshness: empty report");
    let base_build = report
        .samples
        .iter()
        .map(|s| s.base_build_ms)
        .collect::<Vec<_>>();
    let build_ms = LatencySummary::from_samples_ms(&base_build)
        .ok_or_else(|| anyhow!("freshness: no base build sample"))?
        .p50_ms;
    let update_build = report
        .samples
        .iter()
        .map(|s| s.update.ingest_through_seal_ms)
        .collect::<Vec<_>>();
    let update_ms = LatencySummary::from_samples_ms(&update_build)
        .ok_or_else(|| anyhow!("freshness: no update build sample"))?
        .p50_ms;
    Ok(BenchArtifactV1 {
        dimension: DIMENSION.to_string(),
        mode: BenchMode::Warm,
        concurrency: 1,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: corpus_digest(
                DIMENSION,
                &[
                    (UPDATE_PATH.to_string(), OLD_CONTENT.to_string()),
                    (RENAME_FROM.to_string(), RENAME_CONTENT.to_string()),
                    (UPDATE_PATH.to_string(), NEW_CONTENT.to_string()),
                    (RENAME_TO.to_string(), RENAME_CONTENT.to_string()),
                ],
            ),
            config_digest: config_digest(
                DIMENSION,
                &[
                    ("samples", report.samples.len().to_string()),
                    ("top_k", TOP_K.to_string()),
                    ("history_max_generations", HISTORY_GENERATIONS.to_string()),
                    ("producer", "explicit-file-read-and-uds-ingest".to_string()),
                ],
            ),
            model_revision: report.model_revision.clone(),
        },
        host,
        resources: ResourceUsageV1::observe_self()?,
        phases: PhaseDurationsV1 {
            build_ms: Some(build_ms),
            update_ms: Some(update_ms),
            gc_ms: None,
        },
        disk_amplification: None,
        rows: vec![
            row("update", &report.samples, |s| &s.update, 1)?,
            row("delete", &report.samples, |s| &s.delete, 0)?,
            row("rename", &report.samples, |s| &s.rename, 1)?,
        ],
        detail: json!({
            "passed": true,
            "timing_contract": {
                "mutation_to_visible_ms": "filesystem mutation start to first correct pinned query after activation",
                "receipt_to_visible_ms": "last successful ingest response return to first correct pinned query after activation",
                "ingest_ms": "file read and UDS ingest to final successful response; rename includes tombstone and replacement",
                "ingest_through_seal_ms": "file read and UDS ingest start to validated seal return",
                "producer": "explicit file read and UDS ingest; no watcher is measured"
            },
            "sample_count": report.samples.len(),
            "samples": report.samples.iter().map(FreshnessSample::to_json).collect::<Vec<_>>(),
            "correctness": ["update replaces old token", "delete removes new token", "rename moves path", "historical generation pins remain coherent"]
        }),
    })
}

pub(crate) fn write_artifact(
    report: &FreshnessReport,
    out_dir: &Path,
    head: GitHeadV1,
    host: HostV1,
) -> AnyResult<()> {
    artifact(report, head, host)?.write_to(&out_dir.join("summary.json"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_mutations_are_visible_only_in_their_generation() -> AnyResult<()> {
        let report = run(1)?;
        let sample = report
            .samples
            .first()
            .ok_or_else(|| anyhow!("missing sample"))?;
        ensure!(sample.update.generation == 2, "update generation drift");
        ensure!(sample.delete.generation == 3, "delete generation drift");
        ensure!(sample.rename.generation == 4, "rename generation drift");
        for transition in [&sample.update, &sample.delete, &sample.rename] {
            ensure!(
                transition.mutation_to_visible_ms >= transition.receipt_to_visible_ms,
                "mutation time shorter than receipt time"
            );
        }
        let head = GitHeadV1::parse("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")?;
        let envelope = artifact(&report, head, HostV1::observe()?)?.to_json()?;
        ensure!(
            envelope.pointer("/schema_version") == Some(&json!(2)),
            "artifact schema drift"
        );
        ensure!(
            envelope.pointer("/dimension") == Some(&json!("freshness")),
            "artifact dimension drift"
        );
        ensure!(
            envelope.pointer("/detail/passed") == Some(&json!(true)),
            "correctness verdict drift"
        );
        ensure!(
            envelope.pointer("/rows/1/result_shape") == Some(&json!("empty")),
            "delete result shape drift"
        );
        ensure!(
            envelope.pointer("/rows/2/latency/samples") == Some(&json!(1)),
            "rename sample count drift"
        );
        Ok(())
    }
}
