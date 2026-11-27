//! SDK-backed relevance producer over a pinned, separately launched Legacy daemon.

use std::collections::BTreeSet;

use anyhow::{Context as _, Result};
use quanta_index_contract::{
    BatchPublishReceipt, ChunkId, ChunkRecord, EngineTouched, ExecutionOutcomeV2, GenerationPin,
    ManifestGeneration, MetricsSnapshotV1, PlannerStage, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusActiveHeadV1, SearchExplanation, SemanticQueryResponse, lex::LanguageCode,
};
use quanta_index_embed::{OpenAiEmbedStatsSnapshot, OpenAiRequestSample, REQUEST_SAMPLE_CAPACITY};
use quanta_index_sdk::{QuantaIndex, SearchCorpusBatch};
use quanta_index_searchd::app::SemanticEmbedderProfile;
use quanta_index_searchd::app::config::ProviderWorkBudgetConfig;

use crate::legacy_reference::{
    DEFAULT_HISTORY_MAX_GENERATIONS, LegacySearchdBinary, RelevanceProviderInputs,
    SearchdBinaryProcess, legacy_source_replacement_from_chunks, publish_legacy_source_batch,
};
use crate::private_tempdir;

use super::{
    JUDGED_QUERIES, LEXICAL_RELEVANCE_CORPUS, OpenAiAbSourceEvidence, OverlapCapture, QueryScore,
    RelevanceReport, RelevanceRoute, SEMANTIC_GATED_QUERIES, SEMANTIC_JUDGED_QUERIES,
    SOURCEGRAPH_OVERLAP_BUCKETS, SemanticAbCase, SemanticCaseMetrics, SemanticFixturePiece,
    SemanticIntentKind, TOP_K, aggregate_routes, collapse_same_repo_path, model_revision_of,
    openai_provider_stats_json, score_query, score_semantic_case, semantic_fixture_files,
    summarize_semantic_quality,
};
use crate::artifact::BenchSyntax;

const SERVING_REPO: &str = "repo-e2e";
const SERVING_REVISION: &str = "rev-e2e";

fn fixture_identity() -> Result<(RepoId, RevisionId)> {
    Ok((
        RepoId::new(SERVING_REPO)?,
        RevisionId::new(SERVING_REVISION)?,
    ))
}

/// Preserve the former E2eRuntime's one-based chunk IDs and input order.
fn fixture_chunks(files: &[(&str, Vec<SemanticFixturePiece>)]) -> Result<Vec<ChunkRecord>> {
    let rust = LanguageCode::new("rust").map_err(anyhow::Error::msg)?;
    let mut chunks = Vec::new();
    for (path, pieces) in files {
        anyhow::ensure!(
            path.ends_with(".rs"),
            "relevance fixture language changed: {path}"
        );
        for piece in pieces {
            let id = chunks.len().saturating_add(1);
            chunks.push(ChunkRecord {
                chunk_id: ChunkId::new(format!("e2e-{id}-{path}")),
                repo_relative_path: RepoRelativePath::new(*path),
                language: rust.clone(),
                start_byte: 0,
                end_byte: u32::try_from(piece.text.len())?,
                start_line: piece.start_line,
                end_line: piece.end_line,
                text: piece.text.to_owned().into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: None,
            });
        }
    }
    Ok(chunks)
}

fn with_fixture_process<T>(
    binary: &LegacySearchdBinary,
    profile: &SemanticEmbedderProfile,
    openai_ab_inputs: Option<&RelevanceProviderInputs>,
    evaluate: impl FnOnce(&QuantaIndex) -> Result<T>,
) -> Result<(T, u64)> {
    let state = private_tempdir()?;
    let mut command = binary.relevance_benchmark_command(
        state.path(),
        DEFAULT_HISTORY_MAX_GENERATIONS,
        profile,
        openai_ab_inputs,
    )?;
    if openai_ab_inputs.is_some() {
        let _configured = command.env("QUANTA_INDEX_EMBED_REQUEST_SAMPLE_METRICS", "true");
    }
    let process = SearchdBinaryProcess::start_with_pinned_command(state.path(), command)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let result = (|| {
        let client = process
            .connect()
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let value = evaluate(&client)?;
        let rss = process.observe_peak_rss(&client)?;
        Ok::<_, anyhow::Error>((value, rss))
    })();
    let stopped = process.stop().map_err(|error| anyhow::anyhow!("{error}"));
    let verified = binary.verify().map_err(|error| anyhow::anyhow!("{error}"));
    let output = result?;
    stopped?;
    verified?;
    Ok(output)
}

fn publish_fixture(
    client: &QuantaIndex,
    chunks: Vec<ChunkRecord>,
) -> Result<(SearchCorpusActiveHeadV1, BatchPublishReceipt)> {
    let (repo, revision) = fixture_identity()?;
    let batch = legacy_source_replacement_from_chunks(&repo, &revision, 1, chunks)
        .map_err(|error| anyhow::anyhow!("relevance fixture Source batch: {error}"))?;
    let (head, receipt) = publish_legacy_source_batch(client, &batch, None)
        .map_err(|error| anyhow::anyhow!("relevance fixture publish and activate: {error}"))?;
    anyhow::ensure!(
        head.generation.lexical.manifest_generation == ManifestGeneration::new(1),
        "relevance fixture active generation differs"
    );
    Ok((head, receipt))
}

fn run_fixture<T>(
    binary: &LegacySearchdBinary,
    profile: &SemanticEmbedderProfile,
    chunks: Vec<ChunkRecord>,
    evaluate: impl FnOnce(&QuantaIndex, &GenerationPin) -> Result<T>,
) -> Result<(T, u64)> {
    with_fixture_process(binary, profile, None, |client| {
        let (head, _) = publish_fixture(client, chunks)?;
        let track = head.generation.lexical;
        let pin = GenerationPin::new(track.repo_id, track.revision_id, track.manifest_generation);
        evaluate(client, &pin)
    })
}

fn text_order(
    client: &QuantaIndex,
    pin: &GenerationPin,
    syntax: BenchSyntax,
    query: &str,
) -> Result<Vec<String>> {
    let builder = client.lexical().query();
    let response = match syntax {
        BenchSyntax::Native => builder.native(query),
        BenchSyntax::Sourcegraph => builder.sourcegraph(query),
    }
    .pinned(pin.clone())
    .top_k(TOP_K)
    .execute()?;
    anyhow::ensure!(
        response.generation == *pin,
        "relevance text query selected another generation"
    );
    Ok(response
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str().to_owned())
        .collect())
}

fn semantic_response(
    client: &QuantaIndex,
    pin: &GenerationPin,
    query: &str,
) -> Result<SemanticQueryResponse> {
    // No lexical scope: the pure vector route is the judged behavior.
    let response = client
        .semantic()
        .query()
        .text(query)
        .pinned(pin.clone())
        .top_k(TOP_K)
        .execute()?;
    anyhow::ensure!(
        response.generation == *pin,
        "relevance semantic query selected another generation"
    );
    Ok(response)
}

fn project_semantic_order(response: &SemanticQueryResponse) -> Vec<String> {
    collapse_same_repo_path(
        response
            .results
            .iter()
            .map(|candidate| {
                (
                    candidate.repo_id.as_str().to_owned(),
                    candidate.repo_relative_path.as_str().to_owned(),
                )
            })
            .collect(),
    )
}

fn semantic_order(client: &QuantaIndex, pin: &GenerationPin, query: &str) -> Result<Vec<String>> {
    Ok(project_semantic_order(&semantic_response(
        client, pin, query,
    )?))
}

pub(super) struct OpenAiAbFixture {
    batch: SearchCorpusBatch,
    batch_digest: String,
    paths: BTreeSet<&'static str>,
}

impl OpenAiAbFixture {
    pub(super) fn prepare() -> Result<Self> {
        let files = semantic_fixture_files();
        let paths = files.iter().map(|(path, _)| *path).collect();
        let (repo, revision) = fixture_identity()?;
        let batch =
            legacy_source_replacement_from_chunks(&repo, &revision, 1, fixture_chunks(&files)?)
                .map_err(|error| anyhow::anyhow!("OpenAI A/B Source batch: {error}"))?;
        let batch_digest = batch.batch_digest()?;
        Ok(Self {
            batch,
            batch_digest,
            paths,
        })
    }

    pub(super) fn source_batch_digest(&self) -> String {
        format!("sha256:{}", self.batch_digest)
    }
}

pub(super) struct OpenAiAbCapture {
    pub(super) cases: Vec<SemanticAbCase>,
    pub(super) provider_stats: OpenAiEmbedStatsSnapshot,
    pub(super) source_evidence: OpenAiAbSourceEvidence,
    pub(super) peak_daemon_rss_bytes: u64,
}

pub(super) fn run_openai_ab(
    hash_profile: &SemanticEmbedderProfile,
    openai_profile: &SemanticEmbedderProfile,
    openai_inputs: &RelevanceProviderInputs,
    fixture: &OpenAiAbFixture,
) -> Result<OpenAiAbCapture> {
    let SemanticEmbedderProfile::OpenAi {
        model,
        model_revision,
        tuning,
        ..
    } = openai_profile
    else {
        anyhow::bail!("OpenAI A/B requires an OpenAI arm");
    };
    let max_batch = u64::try_from(tuning.max_batch)?;
    let retry_cap = tuning
        .max_retries
        .min(openai_inputs.budget.retry_attempts_cap);
    let binary = LegacySearchdBinary::from_environment()
        .map_err(|error| anyhow::anyhow!("OpenAI A/B pinned Legacy executable: {error}"))?;
    // Publish the approved Source batch unchanged in both arms.
    let publish = |client: &QuantaIndex| -> Result<SearchCorpusActiveHeadV1> {
        let (head, receipt) = publish_legacy_source_batch(client, &fixture.batch, None)
            .map_err(|error| anyhow::anyhow!("OpenAI A/B Source publish and activate: {error}"))?;
        anyhow::ensure!(
            receipt.batch_digest == fixture.batch_digest,
            "OpenAI A/B publication did not attest the submitted Source batch"
        );
        Ok(head)
    };
    let hash_model = format!(
        "{}@{}",
        quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_MODEL_ID,
        quanta_index_search_plane::SEARCH_OWNED_SEMANTIC_MODEL_REVISION
    );
    let ((hash_cases, hash_head), hash_rss) =
        with_fixture_process(&binary, hash_profile, None, |client| {
            let head = publish(client)?;
            let cases = score_ab_arm(client, &head, &hash_model, &fixture.paths)?;
            Ok((cases, head))
        })
        .context("Hash A/B arm")?;
    let openai_model = format!("openai:{model}@{model_revision}");
    let ((openai_cases, openai_head, published, provider_stats), openai_rss) =
        with_fixture_process(&binary, openai_profile, Some(openai_inputs), |client| {
            let scrape = || {
                let snapshot = client.observability().metrics_snapshot()?;
                let stats = openai_provider_stats(&snapshot, max_batch)?;
                verify_ab_spend(&snapshot, &openai_inputs.budget, stats.http_request_count)?;
                Ok::<_, anyhow::Error>(stats)
            };
            let before = scrape().context(
                "OpenAI A/B requires complete provider telemetry before Source publication",
            )?;
            anyhow::ensure!(
                before.total_texts_observed == 0 && before.http_request_count == 0,
                "OpenAI A/B provider telemetry is not fresh"
            );
            let head = publish(client)?;
            let published =
                scrape().context("OpenAI A/B provider telemetry after Source publication")?;
            anyhow::ensure!(
                successful_ab_http_requests(&before, &published, retry_cap)? > 0,
                "OpenAI A/B Source publication did not observe a successful provider request"
            );
            let cases = score_ab_arm(client, &head, &openai_model, &fixture.paths)?;
            let stats = scrape().context("OpenAI A/B provider telemetry after queries")?;
            verify_ab_provider_progress(&published, &stats)?;
            verify_ab_query_observations(
                &published,
                &stats,
                cases.len(),
                tuning.cache_enabled,
                retry_cap,
            )?;
            Ok((cases, head, published, stats))
        })
        .context("OpenAI A/B arm")?;
    let cases = SEMANTIC_JUDGED_QUERIES
        .iter()
        .zip(hash_cases)
        .zip(openai_cases)
        .map(|((query, hash), openai)| SemanticAbCase {
            id: query.id,
            intent_kind: query.intent_kind,
            intent: query.intent,
            query: query.query,
            on_topic_path: query.on_topic_path,
            hard_negative_path: query.hard_negative_path,
            hash,
            openai,
        })
        .collect();
    Ok(OpenAiAbCapture {
        cases,
        provider_stats,
        source_evidence: OpenAiAbSourceEvidence {
            legacy_daemon_sha256: binary.sha256().to_owned(),
            source_batch_digest: fixture.source_batch_digest(),
            hash: hash_head,
            openai: openai_head,
            openai_publication_http_requests: published.http_request_count,
            openai_publication_stats: openai_provider_stats_json(&published),
        },
        peak_daemon_rss_bytes: hash_rss.max(openai_rss),
    })
}

fn score_ab_arm(
    client: &QuantaIndex,
    head: &SearchCorpusActiveHeadV1,
    profile: &str,
    fixture_paths: &BTreeSet<&str>,
) -> Result<Vec<SemanticCaseMetrics>> {
    let track = &head.generation.semantic;
    let pin = GenerationPin::new(
        track.repo_id.clone(),
        track.revision_id.clone(),
        track.manifest_generation,
    );
    let pin_detail = format!(
        "{}@{}#{}",
        pin.repo_id.as_str(),
        pin.revision_id.as_str(),
        pin.manifest_generation.get()
    );
    let cases = SEMANTIC_JUDGED_QUERIES
        .iter()
        .map(|query| {
            let response = semantic_response(client, &pin, query.query)
                .with_context(|| format!("A/B semantic query `{}`", query.id))?;
            verify_ab_read_view(
                &response.explanation,
                &pin_detail,
                &track.manifest_digest,
                profile,
            )
            .with_context(|| format!("A/B retained view for `{}`", query.id))?;
            let order = ab_semantic_order(&response, fixture_paths)
                .with_context(|| format!("A/B semantic result for `{}`", query.id))?;
            score_semantic_case(query, order)
        })
        .collect::<Result<Vec<_>>>()?;
    anyhow::ensure!(
        client
            .generations()
            .active_head(pin.repo_id, pin.revision_id)?
            .as_ref()
            == Some(head),
        "OpenAI A/B fixture activation changed while evaluating the retained generation"
    );
    Ok(cases)
}

fn ab_semantic_order(
    response: &SemanticQueryResponse,
    fixture_paths: &BTreeSet<&str>,
) -> Result<Vec<String>> {
    anyhow::ensure!(
        response.selected_active_head.is_none(),
        "A/B semantic query did not use its explicit pin"
    );
    if let ExecutionOutcomeV2::InterruptedPartial { reason } = response.window.outcome() {
        anyhow::bail!("A/B semantic query was interrupted: {}", reason.as_str());
    }
    anyhow::ensure!(
        response.explanation.engines_executed.as_slice() == [EngineTouched::Semantic],
        "A/B query did not execute the semantic engine alone"
    );
    anyhow::ensure!(
        response.window.returned() <= TOP_K,
        "A/B semantic response exceeded the requested top-k"
    );
    let pin = &response.generation;
    for candidate in &response.results {
        anyhow::ensure!(
            candidate.repo_id == pin.repo_id
                && candidate.revision_id == pin.revision_id
                && candidate.manifest_generation == pin.manifest_generation,
            "A/B candidate `{}` belongs to another generation",
            candidate.candidate_id
        );
        anyhow::ensure!(
            candidate.source_repo_id == pin.repo_id
                && fixture_paths.contains(candidate.repo_relative_path.as_str()),
            "A/B candidate `{}` is outside the published fixture",
            candidate.candidate_id
        );
    }
    Ok(project_semantic_order(response))
}

fn verify_ab_read_view(
    explanation: &SearchExplanation,
    pin: &str,
    digest: &str,
    profile: &str,
) -> Result<()> {
    for (prefix, expected) in [
        ("read_view.domains=", "semantic"),
        ("read_view.pin=", pin),
        ("read_view.semantic_artifact=", digest),
        ("read_view.profile=", profile),
    ] {
        let mut matches = explanation.planner_trace.iter().filter_map(|entry| {
            entry
                .detail
                .strip_prefix(prefix)
                .map(|value| (entry.stage, value))
        });
        anyhow::ensure!(
            matches.next() == Some((PlannerStage::Plan, expected)) && matches.next().is_none(),
            "missing or mismatched A/B read view evidence for {prefix}{expected}"
        );
    }
    Ok(())
}

const REQUEST_SAMPLE_PREFIX: &str = "embed_provider_request_sample_";

fn provider_counter(snapshot: &MetricsSnapshotV1, name: &str) -> Result<u64> {
    snapshot
        .counters
        .iter()
        .find(|point| point.name == name)
        .map(|point| point.value)
        .with_context(|| format!("missing provider counter `{name}`"))
}

fn verify_ab_spend(
    snapshot: &MetricsSnapshotV1,
    budget: &ProviderWorkBudgetConfig,
    requests: u64,
) -> Result<()> {
    let per_attempt = budget
        .openai_max_request_cost_micros
        .context("missing OpenAI A/B request cost bound")?
        .get();
    let expected_spend = requests
        .checked_mul(per_attempt)
        .context("OpenAI A/B attempt cost overflow")?;
    anyhow::ensure!(
        expected_spend <= budget.total_cost_micros_ceiling,
        "OpenAI A/B exceeded the approved spend ceiling"
    );
    for (name, expected) in [
        ("embed_provider_cost_micros_per_attempt_bound", per_attempt),
        (
            "embed_provider_cost_micros_ceiling",
            budget.total_cost_micros_ceiling,
        ),
        (
            "embed_provider_retry_attempts_cap",
            u64::from(budget.retry_attempts_cap),
        ),
        (
            "embed_provider_inflight_requests_cap",
            budget.inflight_requests_cap,
        ),
        (
            "embed_provider_inflight_bytes_cap",
            budget.inflight_bytes_cap,
        ),
        ("embed_provider_charged_cost_micros_total", expected_spend),
    ] {
        anyhow::ensure!(
            provider_counter(snapshot, name)? == expected,
            "OpenAI A/B daemon budget metric `{name}` differs from approved work"
        );
    }
    Ok(())
}

/// Decode the opt-in provider window from the daemon's public metrics scrape.
/// Numbered gauges are dense and oldest-first. The monotonic maxima cover
/// the full process lifetime, including requests evicted from the window.
fn openai_provider_stats(
    snapshot: &MetricsSnapshotV1,
    max_batch: u64,
) -> Result<OpenAiEmbedStatsSnapshot> {
    let counter = |name| provider_counter(snapshot, name);
    let gauge_integer = |name: &str| -> Result<u64> {
        let value = snapshot
            .gauges
            .iter()
            .find(|point| point.name == name)
            .with_context(|| format!("missing provider gauge `{name}`"))?
            .value;
        let integer = value
            .to_string()
            .parse::<u64>()
            .with_context(|| format!("provider gauge `{name}` must be a nonnegative integer"))?;
        anyhow::ensure!(
            integer < (1_u64 << 53),
            "provider gauge `{name}` exceeds the exact integer range"
        );
        Ok(integer)
    };
    let capacity = u64::try_from(REQUEST_SAMPLE_CAPACITY)?;
    anyhow::ensure!(
        gauge_integer("embed_provider_request_samples_capacity")? == capacity,
        "provider request sample capacity differs"
    );
    let http_request_count = counter("embed_provider_http_requests_total")?;
    let request_samples_dropped = counter("embed_provider_request_samples_dropped_total")?;
    let sample_count = http_request_count.min(capacity);
    anyhow::ensure!(
        http_request_count.checked_sub(request_samples_dropped) == Some(sample_count),
        "provider request sample accounting is incomplete"
    );
    let gauge_count = snapshot
        .gauges
        .iter()
        .filter(|point| point.name.starts_with(REQUEST_SAMPLE_PREFIX))
        .count();
    anyhow::ensure!(
        gauge_count == usize::try_from(sample_count)?.saturating_mul(2),
        "provider request sample window differs"
    );
    let request_samples = (0..sample_count)
        .map(|slot| {
            let texts_submitted =
                gauge_integer(&format!("{REQUEST_SAMPLE_PREFIX}{slot}_texts_submitted"))?;
            let estimated_tokens =
                gauge_integer(&format!("{REQUEST_SAMPLE_PREFIX}{slot}_estimated_tokens"))?;
            anyhow::ensure!(
                texts_submitted > 0,
                "provider request sample {slot} is empty"
            );
            Ok(OpenAiRequestSample {
                texts_submitted,
                estimated_tokens,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let stats = OpenAiEmbedStatsSnapshot {
        total_texts_observed: counter("embed_provider_texts_observed_total")?,
        cache_hits: counter("embed_provider_cache_hits_total")?,
        distinct_miss_texts: counter("embed_provider_distinct_miss_texts_total")?,
        http_request_count,
        retry_count: counter("embed_provider_retries_total")?,
        retryable_status_count: counter("embed_provider_http_failures_total")?,
        transport_error_count: counter("embed_provider_transport_failures_total")?,
        max_request_texts: counter("embed_provider_max_request_texts")?,
        max_estimated_tokens: counter("embed_provider_max_estimated_tokens")?,
        request_samples_dropped,
        request_samples,
    };
    anyhow::ensure!(
        stats
            .cache_hits
            .checked_add(stats.distinct_miss_texts)
            .is_some_and(|observed| observed <= stats.total_texts_observed),
        "provider cache hits and distinct misses exceed observed texts"
    );
    let failures = stats
        .retryable_status_count
        .checked_add(stats.transport_error_count)
        .filter(|&failures| failures <= stats.http_request_count)
        .context("provider failures exceed observed requests")?;
    anyhow::ensure!(
        stats.retry_count <= failures,
        "provider retries exceed retryable failures"
    );
    anyhow::ensure!(
        stats.max_request_texts <= max_batch,
        "provider max request texts {} exceeds configured max_batch {max_batch}",
        stats.max_request_texts
    );
    for (maximum, window_max) in [
        (
            stats.max_request_texts,
            stats
                .request_samples
                .iter()
                .map(|sample| sample.texts_submitted)
                .max()
                .unwrap_or(0),
        ),
        (
            stats.max_estimated_tokens,
            stats
                .request_samples
                .iter()
                .map(|sample| sample.estimated_tokens)
                .max()
                .unwrap_or(0),
        ),
    ] {
        anyhow::ensure!(
            window_max <= maximum && (request_samples_dropped > 0 || window_max == maximum),
            "provider request maxima disagree with the observed sample window"
        );
    }
    Ok(stats)
}

fn verify_ab_provider_progress(
    published: &OpenAiEmbedStatsSnapshot,
    queried: &OpenAiEmbedStatsSnapshot,
) -> Result<()> {
    for (label, before, after) in [
        (
            "requests",
            published.http_request_count,
            queried.http_request_count,
        ),
        (
            "observed texts",
            published.total_texts_observed,
            queried.total_texts_observed,
        ),
        ("cache hits", published.cache_hits, queried.cache_hits),
        (
            "distinct misses",
            published.distinct_miss_texts,
            queried.distinct_miss_texts,
        ),
        ("retries", published.retry_count, queried.retry_count),
        (
            "retryable statuses",
            published.retryable_status_count,
            queried.retryable_status_count,
        ),
        (
            "transport errors",
            published.transport_error_count,
            queried.transport_error_count,
        ),
        (
            "max request texts",
            published.max_request_texts,
            queried.max_request_texts,
        ),
        (
            "max estimated tokens",
            published.max_estimated_tokens,
            queried.max_estimated_tokens,
        ),
    ] {
        anyhow::ensure!(
            after >= before,
            "OpenAI A/B provider {label} decreased after Source publication"
        );
    }
    // Both scrapes have complete bounded windows. Publication samples still
    // retained after the queries must be the identical prefix of the new window.
    let evicted = queried
        .request_samples_dropped
        .checked_sub(published.request_samples_dropped)
        .context("OpenAI A/B dropped request samples decreased after Source publication")?;
    let overlap = published
        .request_samples
        .iter()
        .skip(usize::try_from(evicted)?);
    let retained = overlap.len();
    anyhow::ensure!(
        queried.request_samples.iter().take(retained).eq(overlap),
        "OpenAI A/B provider request window changed a retained publication sample"
    );
    let added = queried
        .http_request_count
        .saturating_sub(published.http_request_count);
    if added <= u64::try_from(queried.request_samples.len())? {
        let expected_maxima = queried.request_samples.iter().skip(retained).fold(
            (published.max_request_texts, published.max_estimated_tokens),
            |(texts, tokens), sample| {
                (
                    texts.max(sample.texts_submitted),
                    tokens.max(sample.estimated_tokens),
                )
            },
        );
        anyhow::ensure!(
            (queried.max_request_texts, queried.max_estimated_tokens) == expected_maxima,
            "OpenAI A/B provider maxima differ from publication and the complete query sample suffix"
        );
    }
    Ok(())
}

fn ab_counter_delta(before: u64, after: u64) -> Result<u64> {
    after
        .checked_sub(before)
        .context("OpenAI A/B provider telemetry decreased")
}

fn successful_ab_http_requests(
    before: &OpenAiEmbedStatsSnapshot,
    after: &OpenAiEmbedStatsSnapshot,
    retry_cap: u32,
) -> Result<u64> {
    let attempts = ab_counter_delta(before.http_request_count, after.http_request_count)?;
    let retries = ab_counter_delta(before.retry_count, after.retry_count)?;
    let retryable = ab_counter_delta(before.retryable_status_count, after.retryable_status_count)?;
    let transport = ab_counter_delta(before.transport_error_count, after.transport_error_count)?;
    anyhow::ensure!(
        retryable.checked_add(transport) == Some(retries),
        "OpenAI A/B completed phase contains an HTTP failure without a retry"
    );
    let successes = attempts
        .checked_sub(retries)
        .context("OpenAI A/B retries exceed attempts")?;
    anyhow::ensure!(
        retries <= successes.saturating_mul(u64::from(retry_cap)),
        "OpenAI A/B completed phase exceeds the approved retry limit"
    );
    Ok(successes)
}

fn verify_ab_query_observations(
    published: &OpenAiEmbedStatsSnapshot,
    queried: &OpenAiEmbedStatsSnapshot,
    case_count: usize,
    cache_enabled: bool,
    retry_cap: u32,
) -> Result<()> {
    let count = u64::try_from(case_count)?;
    let expected_successes = if cache_enabled {
        let hits = ab_counter_delta(published.cache_hits, queried.cache_hits)?;
        let misses = ab_counter_delta(published.distinct_miss_texts, queried.distinct_miss_texts)?;
        let texts = ab_counter_delta(published.total_texts_observed, queried.total_texts_observed)?;
        anyhow::ensure!(
            texts == count && hits.checked_add(misses) == Some(count),
            "OpenAI A/B cache observations do not account for every query"
        );
        misses
    } else {
        anyhow::ensure!(
            queried.total_texts_observed == 0
                && queried.cache_hits == 0
                && queried.distinct_miss_texts == 0,
            "OpenAI A/B disabled cache reported cache observations"
        );
        count
    };
    anyhow::ensure!(
        successful_ab_http_requests(published, queried, retry_cap)? == expected_successes,
        "OpenAI A/B HTTP observations do not account for every uncached query"
    );
    let attempts = ab_counter_delta(published.http_request_count, queried.http_request_count)?;
    let sampled_attempts = usize::try_from(attempts).unwrap_or(usize::MAX);
    anyhow::ensure!(
        queried
            .request_samples
            .iter()
            .rev()
            .take(sampled_attempts)
            .all(|sample| sample.texts_submitted == 1),
        "OpenAI A/B query request samples must contain one text per HTTP attempt"
    );
    Ok(())
}

pub(super) fn run(profile: SemanticEmbedderProfile) -> Result<RelevanceReport> {
    anyhow::ensure!(
        !matches!(profile, SemanticEmbedderProfile::Unavailable),
        "relevance evaluation requires an active semantic embedder"
    );
    let binary =
        LegacySearchdBinary::from_environment().map_err(|error| anyhow::anyhow!("{error}"))?;
    let daemon_sha = binary.sha256().to_owned();
    let lexical_files = LEXICAL_RELEVANCE_CORPUS
        .iter()
        .map(|(path, text)| {
            (
                *path,
                vec![SemanticFixturePiece {
                    text: *text,
                    start_line: 1,
                    end_line: 2,
                }],
            )
        })
        .collect::<Vec<_>>();
    let lexical_chunks = fixture_chunks(&lexical_files)?;
    let ((mut queries, overlap), lexical_rss) = run_fixture(
        &binary,
        &SemanticEmbedderProfile::hash_dev(),
        lexical_chunks,
        |client, pin| {
            let mut queries = Vec::with_capacity(
                JUDGED_QUERIES
                    .len()
                    .saturating_add(SEMANTIC_GATED_QUERIES.len()),
            );
            for query in JUDGED_QUERIES {
                anyhow::ensure!(
                    query.route == RelevanceRoute::Lexical,
                    "relevance lexical fixture has a non-lexical route"
                );
                let order = text_order(client, pin, query.syntax, query.query)
                    .with_context(|| format!("relevance query `{}`", query.id))?;
                queries.push(score_query(query, order)?);
            }
            let overlap = SOURCEGRAPH_OVERLAP_BUCKETS
                .iter()
                .map(|bucket| {
                    let (quanta_ordering, quanta_capture_error) =
                        match text_order(client, pin, bucket.syntax, bucket.query) {
                            Ok(order) => (order, None),
                            Err(error) => (Vec::new(), Some(error.to_string())),
                        };
                    OverlapCapture {
                        bucket: bucket.bucket,
                        query_family: bucket.query_family,
                        query: bucket.query,
                        syntax: bucket.syntax,
                        sourcegraph_surface: bucket.sourcegraph_surface,
                        quanta_ordering,
                        quanta_capture_error,
                    }
                })
                .collect();
            Ok((queries, overlap))
        },
    )?;

    let semantic_chunks = fixture_chunks(&semantic_fixture_files())?;
    let learned = !profile.is_dev();
    let model_revision = model_revision_of(&profile);
    let ((semantic_queries, semantic_quality), semantic_rss) =
        run_fixture(&binary, &profile, semantic_chunks, |client, pin| {
            let mut semantic_queries = Vec::new();
            for query in SEMANTIC_GATED_QUERIES {
                anyhow::ensure!(
                    query.route == RelevanceRoute::Semantic,
                    "relevance semantic fixture has a non-semantic route"
                );
                let order = semantic_order(client, pin, query.query)
                    .with_context(|| format!("relevance query `{}`", query.id))?;
                semantic_queries.push(score_query(query, order)?);
            }
            let quality = if learned {
                let mut cases: Vec<SemanticCaseMetrics> = Vec::new();
                for query in SEMANTIC_JUDGED_QUERIES
                    .iter()
                    .filter(|query| query.intent_kind == SemanticIntentKind::Paraphrase)
                {
                    let order = semantic_order(client, pin, query.query)
                        .with_context(|| format!("relevance query `{}`", query.id))?;
                    let metrics = score_semantic_case(query, order.clone())?;
                    semantic_queries.push(QueryScore {
                        id: query.id,
                        route: RelevanceRoute::Semantic,
                        intent: query.intent,
                        query: query.query,
                        produced_order: order,
                        mrr_at_10: metrics.mrr_at_10,
                        ndcg_at_10: metrics.ndcg_at_10,
                        recall_at_20: metrics.recall_at_20,
                        failures: Vec::new(),
                    });
                    cases.push(metrics);
                }
                Some(summarize_semantic_quality(&cases))
            } else {
                None
            };
            Ok((semantic_queries, quality))
        })?;
    queries.extend(semantic_queries);
    let mut routes = aggregate_routes(&queries);
    if let Some(quality) = &semantic_quality
        && let Some(route) = routes
            .iter_mut()
            .find(|route| route.route == RelevanceRoute::Semantic)
    {
        route.passed &= quality.passed;
    }
    let passed = queries.iter().all(QueryScore::passed) && routes.iter().all(|route| route.passed);
    Ok(RelevanceReport {
        queries,
        routes,
        overlap,
        passed,
        model_revision,
        semantic_quality,
        peak_daemon_rss_bytes: lexical_rss.max(semantic_rss),
        legacy_daemon_sha256: daemon_sha,
    })
}

#[cfg(test)]
mod tests {
    use quanta_index_contract::{
        CandidateCountV1, CoverageV1, ExaminedUniverseV1, InterruptedReasonV2, LexicalCandidate,
        MetricCounterV1, MetricGaugeV1, PlannerTraceEntry, QueryResultWindowV2,
    };

    use super::*;

    #[test]
    fn openai_ab_retained_view_rejects_altered_evidence() -> Result<()> {
        let mut explanation = SearchExplanation::empty();
        explanation.planner_trace = [
            "read_view.domains=semantic",
            "read_view.epochs=-",
            "read_view.pin=repo-e2e@rev-e2e#1",
            "read_view.semantic_artifact=lex-seal:1",
            "read_view.profile=openai:fixture@v2",
        ]
        .into_iter()
        .map(|detail| PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail: detail.to_owned(),
        })
        .collect();
        let pin = "repo-e2e@rev-e2e#1";
        let digest = "lex-seal:1";
        let profile = "openai:fixture@v2";
        verify_ab_read_view(&explanation, pin, digest, profile)?;
        for (wrong_pin, wrong_digest, wrong_profile) in [
            ("repo-e2e@rev-e2e#2", digest, profile),
            (pin, "different-seal", profile),
            (pin, digest, "openai:fixture@v3"),
        ] {
            anyhow::ensure!(
                verify_ab_read_view(&explanation, wrong_pin, wrong_digest, wrong_profile).is_err(),
                "altered retained identity was accepted"
            );
        }
        explanation.planner_trace.push(PlannerTraceEntry {
            stage: PlannerStage::Plan,
            detail: "read_view.semantic_artifact=lex-seal:1".to_owned(),
        });
        anyhow::ensure!(
            verify_ab_read_view(&explanation, pin, digest, profile).is_err(),
            "duplicate retained identity was accepted"
        );
        Ok(())
    }

    #[test]
    fn openai_ab_ranked_page_rejects_foreign_rows_and_interruption() -> Result<()> {
        let repo = RepoId::new("repo-e2e")?;
        let revision = RevisionId::new("rev-e2e")?;
        let generation = ManifestGeneration::new(1);
        let path = "auth/token_refresh.rs";
        let row = LexicalCandidate {
            source_repo_id: repo.clone(),
            source: None,
            preview: None,
            candidate_id: "auth-chunk-1".to_owned(),
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            manifest_generation: generation,
            repo_relative_path: RepoRelativePath::new(path),
            lines: Some(quanta_index_contract::LexicalLineRange {
                start_line: 1,
                end_line: 2,
            }),
            excerpt: Some(quanta_index_contract::LexicalMatchExcerpt {
                snippet: String::new(),
                snippet_hit_offset: None,
                highlights: Vec::new(),
            }),
            score: 1.0,
            native_source: None,
        };
        let second = LexicalCandidate {
            candidate_id: "auth-chunk-2".to_owned(),
            lines: Some(quanta_index_contract::LexicalLineRange {
                start_line: 3,
                end_line: 4,
            }),
            excerpt: Some(quanta_index_contract::LexicalMatchExcerpt {
                snippet: String::new(),
                snippet_hit_offset: None,
                highlights: Vec::new(),
            }),
            score: 0.5,
            ..row.clone()
        };
        let response = SemanticQueryResponse {
            generation: GenerationPin::new(repo, revision, generation),
            selected_active_head: None,
            results: vec![row.clone(), second.clone()],
            window: QueryResultWindowV2::exact_probe(2),
            explanation: SearchExplanation {
                engines_touched: vec![EngineTouched::Semantic],
                engines_executed: vec![EngineTouched::Semantic],
                ..SearchExplanation::empty()
            },
        };
        let fixture_paths = BTreeSet::from([path]);
        anyhow::ensure!(
            ab_semantic_order(&response, &fixture_paths)? == [path],
            "both in-fixture chunks must contribute one file rank"
        );

        let zero_hits = SemanticQueryResponse {
            results: Vec::new(),
            window: QueryResultWindowV2::exact_probe(0),
            explanation: SearchExplanation {
                engines_executed: vec![EngineTouched::Semantic],
                ..SearchExplanation::empty()
            },
            ..response.clone()
        };
        anyhow::ensure!(ab_semantic_order(&zero_hits, &fixture_paths)?.is_empty());
        let skipped = SemanticQueryResponse {
            window: QueryResultWindowV2::logical_empty("semantic.dense"),
            explanation: SearchExplanation::empty(),
            ..zero_hits
        };
        let skipped: SemanticQueryResponse =
            serde_json::from_value(serde_json::to_value(skipped)?)?;
        let refused = ab_semantic_order(&skipped, &fixture_paths)
            .err()
            .context("A/B scored a skipped backend as a successful empty page")?;
        anyhow::ensure!(
            refused
                .to_string()
                .contains("did not execute the semantic engine alone")
        );

        for (foreign_row, reason) in [
            (
                LexicalCandidate {
                    manifest_generation: ManifestGeneration::new(2),
                    ..row.clone()
                },
                "belongs to another generation",
            ),
            (
                LexicalCandidate {
                    revision_id: RevisionId::new("rev-stale")?,
                    ..row.clone()
                },
                "belongs to another generation",
            ),
            (
                LexicalCandidate {
                    source_repo_id: RepoId::new("outside")?,
                    ..row.clone()
                },
                "is outside the published fixture",
            ),
            (
                LexicalCandidate {
                    repo_relative_path: RepoRelativePath::new("other/file.rs"),
                    ..row
                },
                "is outside the published fixture",
            ),
        ] {
            let mixed = SemanticQueryResponse {
                results: vec![foreign_row, second.clone()],
                ..response.clone()
            };
            // Each bad row is wire-valid and carries the same outer pin as
            // the successful case. The A/B fixture must still refuse it.
            let decoded: SemanticQueryResponse =
                serde_json::from_value(serde_json::to_value(mixed)?)?;
            let refused = ab_semantic_order(&decoded, &fixture_paths)
                .err()
                .context("A/B scored a foreign row")?;
            anyhow::ensure!(
                refused.to_string().contains(reason),
                "unexpected refusal: {refused}"
            );
        }

        let partial = SemanticQueryResponse {
            window: QueryResultWindowV2::new(
                2,
                CandidateCountV1::AtLeast(2),
                ExecutionOutcomeV2::InterruptedPartial {
                    reason: InterruptedReasonV2::Deadline,
                },
                CoverageV1::new(ExaminedUniverseV1::AtLeast(2), None, Vec::new()),
                None,
            )
            .map_err(anyhow::Error::msg)?,
            ..response
        };
        let refused = ab_semantic_order(&partial, &fixture_paths)
            .err()
            .context("A/B scored interrupted results")?;
        anyhow::ensure!(
            refused.to_string().contains("interrupted: deadline"),
            "unexpected refusal: {refused}"
        );
        Ok(())
    }

    fn ab_provider_snapshot() -> MetricsSnapshotV1 {
        let counters = [
            ("embed_provider_texts_observed_total", 400),
            ("embed_provider_cache_hits_total", 100),
            ("embed_provider_distinct_miss_texts_total", 250),
            ("embed_provider_http_requests_total", 257),
            ("embed_provider_retries_total", 7),
            ("embed_provider_http_failures_total", 5),
            ("embed_provider_transport_failures_total", 2),
            ("embed_provider_max_request_texts", 23),
            ("embed_provider_max_estimated_tokens", 900),
            ("embed_provider_request_samples_dropped_total", 1),
        ]
        .into_iter()
        .map(|(name, value)| MetricCounterV1 {
            name: name.to_owned(),
            value,
        })
        .collect();
        let gauges = (0_u32..256)
            .rev()
            .flat_map(|slot| {
                [
                    MetricGaugeV1 {
                        name: format!("embed_provider_request_sample_{slot}_texts_submitted"),
                        value: 1.0,
                    },
                    MetricGaugeV1 {
                        name: format!("embed_provider_request_sample_{slot}_estimated_tokens"),
                        value: f64::from(slot.saturating_add(5)),
                    },
                ]
            })
            .chain([MetricGaugeV1 {
                name: "embed_provider_request_samples_capacity".to_owned(),
                value: 256.0,
            }])
            .collect();
        MetricsSnapshotV1 {
            counters,
            gauges,
            ..MetricsSnapshotV1::default()
        }
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "fixed SDK telemetry gold is compared directly"
    )]
    fn openai_ab_provider_stats_preserve_order_and_evicted_maxima() -> Result<()> {
        let snapshot = ab_provider_snapshot();
        let wire: MetricsSnapshotV1 = serde_json::from_value(serde_json::to_value(snapshot)?)?;
        let stats = openai_provider_stats(&wire, 23)?;
        assert_eq!(stats.http_request_count, 257);
        assert_eq!(stats.request_samples_dropped, 1);
        assert_eq!(stats.request_samples.len(), 256);
        assert_eq!(stats.max_request_texts, 23);
        assert_eq!(stats.max_estimated_tokens, 900);
        assert_eq!(stats.total_texts_observed, 400);
        assert_eq!(stats.cache_hits, 100);
        assert_eq!(stats.distinct_miss_texts, 250);
        assert_eq!(stats.retry_count, 7);
        assert_eq!(stats.retryable_status_count, 5);
        assert_eq!(stats.transport_error_count, 2);
        assert!(
            stats
                .request_samples
                .iter()
                .all(|sample| sample.texts_submitted == 1)
        );
        assert!(
            stats
                .request_samples
                .iter()
                .map(|sample| sample.estimated_tokens)
                .eq(5..=260)
        );
        Ok(())
    }

    #[test]
    fn openai_ab_provider_stats_reject_missing_or_inconsistent_export() -> Result<()> {
        let original = ab_provider_snapshot();
        let refused = openai_provider_stats(&original, 22)
            .err()
            .context("an evicted request that exceeded the configured batch cap was accepted")?;
        anyhow::ensure!(
            refused
                .to_string()
                .contains("max request texts 23 exceeds configured max_batch 22")
        );
        let mut missing_counter = original.clone();
        missing_counter
            .counters
            .retain(|point| point.name != "embed_provider_distinct_miss_texts_total");
        let mut missing_capability = original.clone();
        missing_capability
            .gauges
            .retain(|point| point.name != "embed_provider_request_samples_capacity");
        let mut missing_sample = original.clone();
        missing_sample
            .gauges
            .retain(|point| point.name != "embed_provider_request_sample_1_estimated_tokens");
        let mut fractional_sample = original.clone();
        fractional_sample
            .gauges
            .iter_mut()
            .find(|point| point.name == "embed_provider_request_sample_1_texts_submitted")
            .context("fixture sample")?
            .value = 1.5;
        let mut false_retry_count = original.clone();
        false_retry_count
            .counters
            .iter_mut()
            .find(|point| point.name == "embed_provider_retries_total")
            .context("fixture retries")?
            .value = 8;
        let mut false_failure_count = original.clone();
        false_failure_count
            .counters
            .iter_mut()
            .find(|point| point.name == "embed_provider_http_failures_total")
            .context("fixture HTTP failures")?
            .value = 256;
        let mut false_maximum = original;
        false_maximum
            .counters
            .iter_mut()
            .find(|point| point.name == "embed_provider_max_estimated_tokens")
            .context("fixture maximum")?
            .value = 259;
        for (snapshot, reason) in [
            (
                missing_counter,
                "missing provider counter `embed_provider_distinct_miss_texts_total`",
            ),
            (
                missing_capability,
                "missing provider gauge `embed_provider_request_samples_capacity`",
            ),
            (missing_sample, "provider request sample window differs"),
            (fractional_sample, "must be a nonnegative integer"),
            (false_maximum, "provider request maxima disagree"),
            (
                false_retry_count,
                "provider retries exceed retryable failures",
            ),
            (
                false_failure_count,
                "provider failures exceed observed requests",
            ),
        ] {
            let refused = openai_provider_stats(&snapshot, 23)
                .err()
                .context("bad provider scrape was accepted")?;
            anyhow::ensure!(
                refused.to_string().contains(reason),
                "Unexpected refusal: {refused:#}"
            );
        }
        Ok(())
    }

    #[test]
    fn openai_ab_provider_progress_keeps_publication_samples_and_maxima() -> Result<()> {
        let published = openai_provider_stats(&ab_provider_snapshot(), 23)?;
        let mut queried = published.clone();
        queried.http_request_count = 258;
        queried.request_samples_dropped = 2;
        queried.total_texts_observed = 401;
        queried.distinct_miss_texts = 251;
        queried.request_samples.rotate_left(1);
        queried
            .request_samples
            .last_mut()
            .context("fixture query sample")?
            .estimated_tokens = 261;
        verify_ab_provider_progress(&published, &queried)?;

        let mut new_maximum = queried.clone();
        new_maximum.max_estimated_tokens = 901;
        let refused = verify_ab_provider_progress(&published, &new_maximum)
            .err()
            .context("an unobserved increase in the lifetime maximum was accepted")?;
        anyhow::ensure!(refused.to_string().contains("complete query sample suffix"));
        new_maximum
            .request_samples
            .last_mut()
            .context("fixture query sample")?
            .estimated_tokens = 901;
        verify_ab_provider_progress(&published, &new_maximum)?;

        let mut evicted_query_peak = published.clone();
        evicted_query_peak.http_request_count = 514;
        evicted_query_peak.request_samples_dropped = 258;
        evicted_query_peak.total_texts_observed = 657;
        evicted_query_peak.distinct_miss_texts = 507;
        evicted_query_peak.max_estimated_tokens = 1000;
        verify_ab_provider_progress(&published, &evicted_query_peak)?;

        let mut changed_sample = queried.clone();
        changed_sample
            .request_samples
            .first_mut()
            .context("fixture retained sample")?
            .estimated_tokens = 7;
        let refused = verify_ab_provider_progress(&published, &changed_sample)
            .err()
            .context("changed publication sample was accepted")?;
        anyhow::ensure!(
            refused
                .to_string()
                .contains("changed a retained publication sample")
        );

        queried.max_estimated_tokens = 261;
        let refused = verify_ab_provider_progress(&published, &queried)
            .err()
            .context("lost publication maximum was accepted")?;
        anyhow::ensure!(
            refused
                .to_string()
                .contains("max estimated tokens decreased")
        );
        Ok(())
    }

    #[test]
    fn fixture_ids_and_split_match_prior_runtime() -> Result<()> {
        let files = semantic_fixture_files();
        let chunks = fixture_chunks(&files)?;
        let (first_path, _) = files.first().context("empty semantic fixture")?;
        let first_chunk = chunks.first().context("empty semantic fixture chunks")?;
        assert_eq!(first_chunk.chunk_id.as_str(), format!("e2e-1-{first_path}"));
        let refresh = chunks
            .iter()
            .filter(|chunk| chunk.repo_relative_path.as_str() == "auth/token_refresh.rs")
            .map(|chunk| (chunk.start_line, chunk.end_line))
            .collect::<Vec<_>>();
        assert_eq!(refresh, [(1, 2), (3, 4)]);
        let (_, revision) = fixture_identity()?;
        assert_eq!(revision.as_str(), SERVING_REVISION);
        Ok(())
    }

    #[test]
    fn openai_ab_rejects_missing_query_observations() -> Result<()> {
        let mut published = openai_provider_stats(&ab_provider_snapshot(), 23)?;
        anyhow::ensure!(verify_ab_query_observations(&published, &published, 13, true, 1).is_err());

        let mut cached = published.clone();
        cached.total_texts_observed = cached.total_texts_observed.saturating_add(13);
        cached.cache_hits = cached.cache_hits.saturating_add(13);
        verify_ab_query_observations(&published, &cached, 13, true, 1)?;

        published.total_texts_observed = 0;
        published.cache_hits = 0;
        published.distinct_miss_texts = 0;
        anyhow::ensure!(
            verify_ab_query_observations(&published, &published, 13, false, 1).is_err()
        );
        let mut queried = published.clone();
        queried.http_request_count = queried.http_request_count.saturating_add(13);
        queried.retryable_status_count = queried.retryable_status_count.saturating_add(1);
        queried.retry_count = queried.retry_count.saturating_add(1);
        anyhow::ensure!(
            verify_ab_query_observations(&published, &queried, 13, false, 1).is_err(),
            "a failed attempt cannot stand in for a successful query"
        );
        queried.http_request_count = queried.http_request_count.saturating_add(1);
        verify_ab_query_observations(&published, &queried, 13, false, 1)?;
        let refusal = verify_ab_query_observations(&published, &queried, 13, false, 0)
            .err()
            .context("a run with zero retries approved accepted a retry")?;
        anyhow::ensure!(
            refusal
                .to_string()
                .contains("exceeds the approved retry limit")
        );
        queried
            .request_samples
            .last_mut()
            .context("query sample")?
            .texts_submitted = 2;
        anyhow::ensure!(verify_ab_query_observations(&published, &queried, 13, false, 1).is_err());
        Ok(())
    }

    #[test]
    fn openai_ab_requires_daemon_budget_attestation_before_publication() -> Result<()> {
        let mut snapshot = ab_provider_snapshot();
        let budget = ProviderWorkBudgetConfig {
            total_cost_micros_ceiling: 600,
            retry_attempts_cap: 1,
            openai_max_request_cost_micros: std::num::NonZeroU64::new(2),
            ..Default::default()
        };
        anyhow::ensure!(verify_ab_spend(&snapshot, &budget, 0).is_err());
        for (name, value) in [
            ("embed_provider_cost_micros_per_attempt_bound", 2),
            ("embed_provider_cost_micros_ceiling", 600),
            ("embed_provider_retry_attempts_cap", 1),
            (
                "embed_provider_inflight_requests_cap",
                budget.inflight_requests_cap,
            ),
            (
                "embed_provider_inflight_bytes_cap",
                budget.inflight_bytes_cap,
            ),
            ("embed_provider_charged_cost_micros_total", 0),
        ] {
            snapshot.counters.push(MetricCounterV1 {
                name: name.into(),
                value,
            });
        }
        verify_ab_spend(&snapshot, &budget, 0)?;
        anyhow::ensure!(verify_ab_spend(&snapshot, &budget, 1).is_err());
        Ok(())
    }

    #[test]
    #[ignore = "requires QUANTA_INDEX_LEGACY_SEARCHD_BIN and matching SHA256"]
    fn pinned_hash_reference_publishes_and_scores_the_full_roster() -> Result<()> {
        let report =
            super::super::run_relevance_report_with_profile(SemanticEmbedderProfile::hash_dev())
                .context("pinned Legacy relevance run")?;
        assert_eq!(
            report.queries.len(),
            JUDGED_QUERIES.len() + SEMANTIC_GATED_QUERIES.len()
        );
        let expected_ids = JUDGED_QUERIES
            .iter()
            .chain(SEMANTIC_GATED_QUERIES)
            .map(|query| query.id)
            .collect::<std::collections::BTreeSet<_>>();
        let observed_ids = report
            .queries
            .iter()
            .map(|query| query.id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(observed_ids, expected_ids);
        assert_eq!(report.overlap.len(), SOURCEGRAPH_OVERLAP_BUCKETS.len());
        assert!(report.semantic_quality.is_none());
        assert_eq!(report.legacy_daemon_sha256.len(), 64);
        assert!(report.peak_daemon_rss_bytes > 0);
        assert!(report.passed, "judged mechanical relevance route failed");

        let gated = SEMANTIC_GATED_QUERIES
            .first()
            .context("hash gate lacks its fixed exact-token query")?;
        let first = report
            .queries
            .iter()
            .find(|score| score.id == gated.id)
            .context("hash gate was not executed")?;
        let corpus_paths = semantic_fixture_files()
            .into_iter()
            .map(|(path, _)| path)
            .collect::<std::collections::BTreeSet<_>>();
        assert!(!first.produced_order.is_empty());
        assert!(
            first
                .produced_order
                .iter()
                .all(|path| corpus_paths.contains(path.as_str())),
            "semantic response contained an out-of-fixture or raw chunk identity"
        );
        assert_eq!(
            first
                .produced_order
                .iter()
                .filter(|path| path.as_str() == "auth/token_refresh.rs")
                .count(),
            1,
            "split auth file must occupy one file rank"
        );
        let unique = first
            .produced_order
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(unique.len(), first.produced_order.len());

        // A second independent state root proves byte-identical hash ranking
        // across generations, while repeated queries in that process prove
        // same-generation stability for both exact and paraphrase cases.
        let binary = LegacySearchdBinary::from_environment()
            .map_err(|error| anyhow::anyhow!("pinned Legacy binary: {error}"))?;
        let (second, _rss) = run_fixture(
            &binary,
            &SemanticEmbedderProfile::hash_dev(),
            fixture_chunks(&semantic_fixture_files())?,
            |client, pin| {
                let first_again = semantic_order(client, pin, gated.query)?;
                let second_again = semantic_order(client, pin, gated.query)?;
                anyhow::ensure!(
                    first_again == second_again,
                    "same-generation hash query changed order"
                );
                let mut exact_count = 0_usize;
                let mut paraphrase_count = 0_usize;
                for query in SEMANTIC_JUDGED_QUERIES {
                    let order_a = semantic_order(client, pin, query.query)?;
                    let order_b = semantic_order(client, pin, query.query)?;
                    anyhow::ensure!(order_a == order_b, "hash query changed order: {}", query.id);
                    anyhow::ensure!(
                        order_a
                            .iter()
                            .all(|path| corpus_paths.contains(path.as_str())),
                        "semantic query returned an out-of-fixture path: {}",
                        query.id
                    );
                    match query.intent_kind {
                        SemanticIntentKind::ExactToken => {
                            exact_count = exact_count.saturating_add(1);
                            anyhow::ensure!(
                                order_a.first().map(String::as_str) == Some(query.on_topic_path),
                                "exact-token query did not rank its target first: {}",
                                query.id
                            );
                        }
                        SemanticIntentKind::Paraphrase => {
                            paraphrase_count = paraphrase_count.saturating_add(1);
                        }
                    }
                }
                anyhow::ensure!(exact_count > 0, "semantic exact-token fixture is empty");
                anyhow::ensure!(paraphrase_count > 0, "semantic paraphrase fixture is empty");
                Ok(first_again)
            },
        )
        .context("second pinned hash semantic run")?;
        assert_eq!(first.produced_order, second);
        let second_metrics = score_query(gated, second)?;
        assert_eq!(
            first.recall_at_20.to_bits(),
            second_metrics.recall_at_20.to_bits(),
            "cross-process hash recall must be bit-identical"
        );
        Ok(())
    }
}
