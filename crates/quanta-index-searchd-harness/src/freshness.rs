//! Workspace mutation to lexical visibility through real filesystem operations
//! and a pinned Legacy daemon's public SDK front doors.
//!
//! The producer is explicit: after each filesystem mutation this rail reads the
//! changed file and publishes it. It does not measure a filesystem watcher.
//! `receipt_to_visible_ms` starts when `publish_outcome()` returns its durable
//! Source receipt. It includes activation and ends at the first correct,
//! generation-pinned query. Every transition checks presence, absence, response generation,
//! and every candidate's generation.

use std::fs;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context as _, Result as AnyResult, anyhow, ensure};
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, ManifestGeneration, QueryConstraintSetV1, RepoId,
    RepoRelativePath, RevisionId, SearchCorpusActiveHeadV1, SourceFileKey, TextQueryRequest,
    TextQueryResponse, TextQuerySyntax, TextRankUnit, lex::LanguageCode,
};
use quanta_index_sdk::{PublishedBatchEvidence, QuantaIndex, SearchCorpusBatch};
use quanta_index_searchd::app::SemanticEmbedderProfile;
use quanta_index_searchd_harness::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    LatencySummary, PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest,
    corpus_digest, model_revision_of,
};
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, SearchdBinaryProcess, legacy_source_delta_from_chunks,
    legacy_source_replacement_from_chunks,
};
use quanta_index_searchd_harness::{fixture_source_scope_v1, private_tempdir};
use serde_json::{Value, json};
use tempfile::TempDir;

const DIMENSION: &str = "freshness";
const REPO: &str = "repo-e2e";
const REVISION: &str = "rev-e2e";
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
    pub daemon_sha256: String,
    pub peak_daemon_rss_bytes: u64,
}

/// Distinct local staging, acknowledged publication, activation and read clocks.
/// The serialized metric names predate this split; only `sealed_at` is a
/// durable Source receipt boundary.
struct TransitionClock {
    mutation_started: Instant,
    ingest_started: Instant,
    staged_at: Instant,
    seal_started: Instant,
    sealed_at: Instant,
    activation_started: Instant,
    activated_at: Instant,
    query_started: Instant,
    visible_at: Instant,
}

impl TransitionClock {
    fn measure(self, generation: ManifestGeneration) -> TransitionMeasurement {
        TransitionMeasurement {
            mutation_to_visible_ms: between_ms(self.mutation_started, self.visible_at),
            receipt_to_visible_ms: between_ms(self.sealed_at, self.visible_at),
            ingest_ms: between_ms(self.ingest_started, self.staged_at),
            ingest_through_seal_ms: between_ms(self.ingest_started, self.sealed_at),
            seal_ms: between_ms(self.seal_started, self.sealed_at),
            activation_ms: between_ms(self.activation_started, self.activated_at),
            first_query_ms: between_ms(self.query_started, self.visible_at),
            generation: generation.get(),
        }
    }
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

fn between_ms(started: Instant, ended: Instant) -> f64 {
    ended.duration_since(started).as_secs_f64() * 1_000.0
}

fn repo() -> AnyResult<RepoId> {
    Ok(RepoId::new(REPO)?)
}

fn revision() -> AnyResult<RevisionId> {
    Ok(RevisionId::new(REVISION)?)
}

fn pin(generation: ManifestGeneration) -> AnyResult<GenerationPin> {
    Ok(GenerationPin::new(repo()?, revision()?, generation))
}

fn source_chunk(id: &str, path: &str, content: &str) -> AnyResult<ChunkRecord> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust").map_err(anyhow::Error::msg)?,
        start_byte: 0,
        end_byte: u32::try_from(content.len())?,
        start_line: 1,
        end_line: 2,
        text: content.to_owned().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn source_file(path: &str) -> AnyResult<SourceFileKey> {
    let file = SourceFileKey {
        source_repo_id: repo()?,
        repo_relative_path: RepoRelativePath::new(path),
    };
    file.validate().map_err(|error| anyhow!(error))?;
    Ok(file)
}

fn pinned_text_page(
    client: &QuantaIndex,
    syntax: TextQuerySyntax,
    generation: ManifestGeneration,
    token: &str,
) -> AnyResult<(GenerationPin, TextQueryResponse)> {
    let expected_pin = pin(generation)?;
    let page = client.lexical().query_request(TextQueryRequest {
        syntax,
        query_text: token.to_owned(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(expected_pin.clone()),
        generation_selector: None,
        top_k: TOP_K,
        cursor: None,
    })?;
    ensure!(
        page.generation == expected_pin,
        "freshness: {token} response pin differs: {:?}",
        page.generation
    );
    Ok((expected_pin, page))
}

fn expect_paths(
    client: &QuantaIndex,
    generation: ManifestGeneration,
    token: &str,
    paths: &[&str],
) -> AnyResult<()> {
    let (expected_pin, page) =
        pinned_text_page(client, TextQuerySyntax::Native, generation, token)?;
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

/// Check the product file result against the same canonical synthetic source
/// issuer that staged ingest uses. Native chunk visibility alone cannot prove
/// the file result's source revision identity.
fn expect_code_search_files(
    client: &QuantaIndex,
    generation: ManifestGeneration,
    token: &str,
    files: &[(&str, &str)],
) -> AnyResult<()> {
    let (expected_pin, page) =
        pinned_text_page(client, TextQuerySyntax::CodeSearch, generation, token)?;
    ensure!(
        page.rank_unit == TextRankUnit::File,
        "freshness: CodeSearch rank unit differs for {token}"
    );
    ensure!(
        page.results.len() == files.len() && page.next_cursor.is_none(),
        "freshness: CodeSearch file count or continuation differs for {token}"
    );
    let mut actual = Vec::with_capacity(page.results.len());
    for candidate in &page.results {
        let path = candidate.repo_relative_path.as_str();
        let (_, content) = files
            .iter()
            .find(|(expected_path, _)| *expected_path == path)
            .ok_or_else(|| anyhow!("freshness: unexpected CodeSearch path {path}"))?;
        let source = candidate
            .source
            .as_ref()
            .ok_or_else(|| anyhow!("freshness: CodeSearch file {path} has no source identity"))?;
        let expected_scope = fixture_source_scope_v1(
            source_file(path)?,
            revision()?,
            vec![source_chunk("source-sha-oracle", path, content)?],
            Vec::new(),
        )?;
        ensure!(
            expected_scope
                .coverage
                .source
                .file
                .repo_relative_path
                .as_str()
                == path,
            "freshness: synthetic source issuer returned an unexpected scope"
        );
        let expected_sha = expected_scope.coverage.source.source_sha256;
        ensure!(
            candidate.candidate_id.starts_with("file:")
                && candidate.repo_id == expected_pin.repo_id
                && candidate.revision_id == expected_pin.revision_id
                && candidate.manifest_generation == generation,
            "freshness: CodeSearch file {path} has incoherent identity: {candidate:?}"
        );
        ensure!(
            candidate.source_repo_id == expected_pin.repo_id
                && source.file.source_repo_id == expected_pin.repo_id
                && source.file.repo_relative_path == candidate.repo_relative_path
                && source.revision_id == expected_pin.revision_id,
            "freshness: CodeSearch file {path} has incoherent source revision"
        );
        ensure!(
            source.source_sha256 == expected_sha,
            "freshness: CodeSearch file {path} has stale source SHA"
        );
        actual.push(path);
    }
    actual.sort_unstable();
    actual.dedup();
    ensure!(
        actual.len() == files.len(),
        "freshness: CodeSearch returned duplicate files for {token}"
    );
    Ok(())
}

fn publish_activate_and_observe(
    client: &QuantaIndex,
    batch: &SearchCorpusBatch,
    expected_active: SearchCorpusActiveHeadV1,
    mutation_started: Instant,
    ingest_started: Instant,
    staged_at: Instant,
    token: &str,
    expected_paths: &[&str],
) -> AnyResult<(TransitionMeasurement, SearchCorpusActiveHeadV1)> {
    ensure!(
        client
            .generations()
            .active_head(batch.repo_id().clone(), batch.revision_id().clone())?
            == Some(expected_active.clone()),
        "freshness: Source head changed before delta publication"
    );
    let seal_started = Instant::now();
    let outcome = client.search_corpus().publish_outcome(batch)?;
    ensure!(
        outcome.receipt.applied && outcome.receipt.durable_sequence > 0,
        "freshness: Source publication has no durable applied receipt"
    );
    let sealed_at = Instant::now();
    let activation_started = Instant::now();
    let activation = client.search_corpus().activate_published(
        &PublishedBatchEvidence::from(&outcome),
        Some(expected_active),
    )?;
    let activated_at = Instant::now();
    let query_started = Instant::now();
    expect_paths(client, batch.generation(), token, expected_paths)?;
    let visible_at = Instant::now();
    let active = client
        .generations()
        .active_head(batch.repo_id().clone(), batch.revision_id().clone())?
        .ok_or_else(|| anyhow!("freshness: activated generation has no observed head"))?;
    ensure!(
        active == activation.active
            && active.generation.lexical.manifest_generation == batch.generation()
            && active.generation.semantic.manifest_generation == batch.generation()
            && active.generation.lexical.manifest_digest == batch.manifest_digest(),
        "freshness: activated Source head differs from the published batch"
    );
    Ok((
        TransitionClock {
            mutation_started,
            ingest_started,
            staged_at,
            seal_started,
            sealed_at,
            activation_started,
            activated_at,
            query_started,
            visible_at,
        }
        .measure(batch.generation()),
        active,
    ))
}

/// One independent workspace and daemon. All generations are retained so
/// historical pins can be checked after the final activation.
pub(crate) fn run_one(binary: &LegacySearchdBinary) -> AnyResult<(FreshnessSample, u64)> {
    let workspace = TempDir::new().context("freshness: create workspace")?;
    let state = private_tempdir().context("freshness: create private daemon state")?;
    let src = workspace.path().join("src");
    fs::create_dir(&src).context("freshness: create src directory")?;
    fs::write(workspace.path().join(UPDATE_PATH), OLD_CONTENT)?;
    fs::write(workspace.path().join(RENAME_FROM), RENAME_CONTENT)?;

    let process = SearchdBinaryProcess::start_with_pinned_command(
        state.path(),
        binary
            .benchmark_command(state.path(), HISTORY_GENERATIONS)
            .map_err(|error| anyhow!("freshness: pinned Legacy command: {error}"))?,
    )
    .map_err(|error| anyhow!("freshness: start pinned Legacy daemon: {error}"))?;
    let client = process
        .connect()
        .map_err(|error| anyhow!("freshness: connect SDK: {error}"))?;
    let serving_repo = repo()?;
    let serving_revision = revision()?;
    let base_started = Instant::now();
    let old_chunk = source_chunk(
        "freshness-old-1",
        UPDATE_PATH,
        &fs::read_to_string(workspace.path().join(UPDATE_PATH))?,
    )?;
    let rename_chunk = source_chunk(
        "freshness-rename-1",
        RENAME_FROM,
        &fs::read_to_string(workspace.path().join(RENAME_FROM))?,
    )?;
    let base_batch = legacy_source_replacement_from_chunks(
        &serving_repo,
        &serving_revision,
        1,
        vec![old_chunk.clone(), rename_chunk.clone()],
    )
    .map_err(|error| anyhow!("freshness: build base Source: {error}"))?;
    ensure!(
        client
            .generations()
            .active_head(serving_repo.clone(), serving_revision.clone())?
            .is_none(),
        "freshness: new daemon already has an active Source head"
    );
    let base_outcome = client.search_corpus().publish_outcome(&base_batch)?;
    ensure!(
        base_outcome.receipt.applied && base_outcome.receipt.durable_sequence > 0,
        "freshness: base Source publication has no durable applied receipt"
    );
    let base_build_ms = elapsed_ms(base_started);
    let base_activation = client
        .search_corpus()
        .activate_published(&PublishedBatchEvidence::from(&base_outcome), None)?;
    let base = base_batch.generation();
    let mut active = client
        .generations()
        .active_head(serving_repo.clone(), serving_revision.clone())?
        .ok_or_else(|| anyhow!("freshness: base Source activation has no head"))?;
    ensure!(
        active == base_activation.active
            && active.generation.lexical.manifest_generation == base
            && active.generation.semantic.manifest_generation == base
            && active.generation.lexical.manifest_digest == base_batch.manifest_digest(),
        "freshness: base Source active head differs"
    );
    expect_paths(&client, base, OLD_TOKEN, &[UPDATE_PATH])?;
    expect_paths(&client, base, RENAME_TOKEN, &[RENAME_FROM])?;
    expect_code_search_files(&client, base, OLD_TOKEN, &[(UPDATE_PATH, OLD_CONTENT)])?;
    expect_code_search_files(
        &client,
        base,
        RENAME_TOKEN,
        &[(RENAME_FROM, RENAME_CONTENT)],
    )?;

    let update_started = Instant::now();
    fs::write(workspace.path().join(UPDATE_PATH), NEW_CONTENT)?;
    let ingest_started = Instant::now();
    let updated_chunk = source_chunk(
        "freshness-new-2",
        UPDATE_PATH,
        &fs::read_to_string(workspace.path().join(UPDATE_PATH))?,
    )?;
    let update_batch = legacy_source_delta_from_chunks(
        &serving_repo,
        &serving_revision,
        2,
        std::slice::from_ref(&old_chunk),
        &[],
        vec![updated_chunk.clone()],
    )
    .map_err(|error| anyhow!("freshness: build update delta: {error}"))?;
    let staged_at = Instant::now();
    let (update, update_active) = publish_activate_and_observe(
        &client,
        &update_batch,
        active,
        update_started,
        ingest_started,
        staged_at,
        NEW_TOKEN,
        &[UPDATE_PATH],
    )?;
    active = update_active;
    let updated_generation = ManifestGeneration::new(update.generation);
    expect_paths(&client, updated_generation, OLD_TOKEN, &[])?;
    expect_paths(&client, updated_generation, RENAME_TOKEN, &[RENAME_FROM])?;
    expect_code_search_files(
        &client,
        updated_generation,
        NEW_TOKEN,
        &[(UPDATE_PATH, NEW_CONTENT)],
    )?;
    expect_code_search_files(&client, updated_generation, OLD_TOKEN, &[])?;

    let delete_started = Instant::now();
    fs::remove_file(workspace.path().join(UPDATE_PATH))?;
    let ingest_started = Instant::now();
    let delete_batch = legacy_source_delta_from_chunks(
        &serving_repo,
        &serving_revision,
        3,
        std::slice::from_ref(&updated_chunk),
        &[source_file(UPDATE_PATH)?],
        Vec::new(),
    )
    .map_err(|error| anyhow!("freshness: build delete delta: {error}"))?;
    let staged_at = Instant::now();
    let (delete, delete_active) = publish_activate_and_observe(
        &client,
        &delete_batch,
        active,
        delete_started,
        ingest_started,
        staged_at,
        NEW_TOKEN,
        &[],
    )?;
    active = delete_active;
    let deleted_generation = ManifestGeneration::new(delete.generation);
    expect_paths(&client, deleted_generation, OLD_TOKEN, &[])?;
    expect_paths(&client, deleted_generation, RENAME_TOKEN, &[RENAME_FROM])?;
    expect_code_search_files(&client, deleted_generation, NEW_TOKEN, &[])?;
    expect_code_search_files(
        &client,
        deleted_generation,
        RENAME_TOKEN,
        &[(RENAME_FROM, RENAME_CONTENT)],
    )?;

    let rename_started = Instant::now();
    fs::rename(
        workspace.path().join(RENAME_FROM),
        workspace.path().join(RENAME_TO),
    )?;
    let ingest_started = Instant::now();
    let moved_chunk = source_chunk(
        "freshness-rename-4",
        RENAME_TO,
        &fs::read_to_string(workspace.path().join(RENAME_TO))?,
    )?;
    let rename_batch = legacy_source_delta_from_chunks(
        &serving_repo,
        &serving_revision,
        4,
        std::slice::from_ref(&rename_chunk),
        &[source_file(RENAME_FROM)?],
        vec![moved_chunk],
    )
    .map_err(|error| anyhow!("freshness: build rename delta: {error}"))?;
    let staged_at = Instant::now();
    let (rename, rename_active) = publish_activate_and_observe(
        &client,
        &rename_batch,
        active,
        rename_started,
        ingest_started,
        staged_at,
        RENAME_TOKEN,
        &[RENAME_TO],
    )?;
    active = rename_active;
    ensure!(
        !workspace.path().join(RENAME_FROM).exists(),
        "freshness: rename source still exists"
    );
    ensure!(
        !workspace.path().join(UPDATE_PATH).exists(),
        "freshness: deleted file still exists"
    );
    // Historical pins must remain immutable across update/delete/rename.
    expect_paths(&client, base, OLD_TOKEN, &[UPDATE_PATH])?;
    expect_paths(&client, base, RENAME_TOKEN, &[RENAME_FROM])?;
    expect_paths(&client, updated_generation, NEW_TOKEN, &[UPDATE_PATH])?;
    expect_paths(&client, deleted_generation, RENAME_TOKEN, &[RENAME_FROM])?;
    let final_generation = ManifestGeneration::new(rename.generation);
    expect_paths(&client, final_generation, OLD_TOKEN, &[])?;
    expect_paths(&client, final_generation, NEW_TOKEN, &[])?;
    expect_code_search_files(
        &client,
        final_generation,
        RENAME_TOKEN,
        &[(RENAME_TO, RENAME_CONTENT)],
    )?;
    expect_code_search_files(&client, final_generation, NEW_TOKEN, &[])?;
    expect_code_search_files(
        &client,
        final_generation,
        "path:moved.rs",
        &[(RENAME_TO, RENAME_CONTENT)],
    )?;
    expect_code_search_files(&client, final_generation, "path:rename.rs", &[])?;

    // Restart the same pinned executable over the durable state root. Current
    // and historical file results must retain their source revision and pin.
    let live_peak = process.observe_peak_rss(&client)?;
    process
        .stop()
        .map_err(|error| anyhow!("freshness: stop daemon: {error}"))?;
    let reopened = SearchdBinaryProcess::start_with_pinned_command(
        state.path(),
        binary
            .benchmark_command(state.path(), HISTORY_GENERATIONS)
            .map_err(|error| anyhow!("freshness: pinned Legacy restart command: {error}"))?,
    )
    .map_err(|error| anyhow!("freshness: restart pinned Legacy daemon: {error}"))?;
    let reopened_client = reopened
        .connect()
        .map_err(|error| anyhow!("freshness: reconnect SDK: {error}"))?;
    ensure!(
        reopened_client
            .generations()
            .active_head(serving_repo, serving_revision)?
            == Some(active),
        "freshness: daemon restart changed the active Source head"
    );
    expect_code_search_files(
        &reopened_client,
        final_generation,
        RENAME_TOKEN,
        &[(RENAME_TO, RENAME_CONTENT)],
    )?;
    expect_code_search_files(&reopened_client, final_generation, NEW_TOKEN, &[])?;
    expect_code_search_files(
        &reopened_client,
        base,
        OLD_TOKEN,
        &[(UPDATE_PATH, OLD_CONTENT)],
    )?;
    expect_code_search_files(
        &reopened_client,
        updated_generation,
        NEW_TOKEN,
        &[(UPDATE_PATH, NEW_CONTENT)],
    )?;
    expect_code_search_files(&reopened_client, deleted_generation, NEW_TOKEN, &[])?;
    let reopened_peak = reopened.observe_peak_rss(&reopened_client)?;
    reopened
        .stop()
        .map_err(|error| anyhow!("freshness: stop restarted daemon: {error}"))?;
    binary
        .verify()
        .map_err(|error| anyhow!("freshness: pinned binary changed: {error}"))?;

    Ok((
        FreshnessSample {
            base_build_ms,
            update,
            delete,
            rename,
        },
        live_peak.max(reopened_peak),
    ))
}

pub(crate) fn run(samples: u32) -> AnyResult<FreshnessReport> {
    ensure!(
        (1..=MAX_SAMPLES).contains(&samples),
        "freshness: samples must be in 1..={MAX_SAMPLES}"
    );
    let binary = LegacySearchdBinary::from_environment()
        .map_err(|error| anyhow!("freshness: pinned Legacy input: {error}"))?;
    let mut measured = Vec::with_capacity(usize::try_from(samples)?);
    let mut peak_daemon_rss_bytes = 0;
    for _ in 0..samples {
        let (sample, peak) = run_one(&binary)?;
        peak_daemon_rss_bytes = peak_daemon_rss_bytes.max(peak);
        measured.push(sample);
    }
    Ok(FreshnessReport {
        samples: measured,
        model_revision: model_revision_of(&SemanticEmbedderProfile::hash_dev()),
        daemon_sha256: binary.sha256().to_owned(),
        peak_daemon_rss_bytes,
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
    ensure!(
        report.peak_daemon_rss_bytes > 0,
        "freshness: daemon RSS is absent"
    );
    ensure!(
        report.daemon_sha256.len() == 64,
        "freshness: pinned daemon SHA is absent"
    );
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
                    ("legacy_daemon_sha256", report.daemon_sha256.clone()),
                    (
                        "producer",
                        "explicit-file-read-and-sdk-source-delta".to_string(),
                    ),
                    (
                        "receipt_clock",
                        "validated-sdk-source-receipt-before-activation-v3".to_string(),
                    ),
                ],
            ),
            model_revision: report.model_revision.clone(),
        },
        host,
        resources: ResourceUsageV1 {
            peak_rss_bytes: report.peak_daemon_rss_bytes,
        },
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
            // Every transition asserts exact visible paths and pinned generation;
            // a stale hit aborts before an artifact can be emitted.
            "stale_hits": 0,
            "timing_contract": {
                "mutation_to_visible_ms": "filesystem mutation start to first correct pinned query after activation",
                "receipt_to_visible_ms": "validated durable SDK Source receipt to first correct pinned query after activation; excludes batch construction and publication",
                "ingest_ms": "file read and local SDK batch construction; rename includes source tombstone and replacement",
                "ingest_through_seal_ms": "file read and local SDK batch construction start to validated durable SDK Source receipt",
                "producer": "explicit file read, SDK Source delta publication, and SDK activation; no watcher is measured",
                "comparability": "receipt_to_visible_ms starts after validated durable SDK receipt; earlier local-stage and seal-based artifacts are not comparable"
            },
            "legacy_daemon_sha256": report.daemon_sha256,
            "resource_scope": "peak RSS of pinned Legacy daemon processes across all samples and restarts",
            "sample_count": report.samples.len(),
            "samples": report.samples.iter().map(FreshnessSample::to_json).collect::<Vec<_>>(),
            "correctness": ["update replaces old token", "delete removes new token", "rename moves path", "historical generation pins remain coherent", "CodeSearch file identity and source SHA remain coherent through update/delete/rename and daemon runtime reopen"]
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
    use std::time::Duration;

    #[test]
    fn receipt_clock_excludes_staging_and_seal_but_includes_activation() {
        let origin = Instant::now();
        let at = |milliseconds| origin + Duration::from_millis(milliseconds);
        let measured = TransitionClock {
            mutation_started: at(0),
            ingest_started: at(2),
            staged_at: at(5),
            seal_started: at(6),
            sealed_at: at(16),
            activation_started: at(18),
            activated_at: at(21),
            query_started: at(23),
            visible_at: at(30),
        }
        .measure(ManifestGeneration::new(2));
        for (observed, expected) in [
            (measured.mutation_to_visible_ms, 30.0),
            (measured.receipt_to_visible_ms, 14.0),
            (measured.ingest_ms, 3.0),
            (measured.ingest_through_seal_ms, 14.0),
            (measured.seal_ms, 10.0),
            (measured.activation_ms, 3.0),
            (measured.first_query_ms, 7.0),
        ] {
            assert!(
                (observed - expected).abs() < 1e-9,
                "clock split observed {observed} ms, expected {expected} ms"
            );
        }
        assert_eq!(measured.generation, 2);
    }

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
            envelope.pointer("/detail/stale_hits") == Some(&json!(0)),
            "observed stale-hit accounting drift"
        );
        ensure!(
            envelope.pointer("/detail/timing_contract/receipt_to_visible_ms")
                == Some(&json!(
                    "validated durable SDK Source receipt to first correct pinned query after activation; excludes batch construction and publication"
                )),
            "receipt timing contract drift"
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
