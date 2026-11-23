//! Scheduled arrivals over the real query IPC socket.
//!
//! Arrival timestamps are
//! fixed before dispatch; queueing and scheduler lag remain in end-to-end
//! latency, so a slow daemon cannot silently reduce the offered load.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};
use std::time::{Duration, Instant};

use anyhow::{Context, Result as AnyResult, ensure};
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, QueryConstraintSetV1, RepoId, RepoRelativePath,
    RevisionId, SearchPlaneErrorCodeV2, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, TextQueryRequest, TextQuerySyntax, TextRankUnit,
    lex::LanguageCode,
};
use quanta_index_ipc::{ClientIoPolicy, IpcError, send_request};
use quanta_index_sdk::{ConnectOptions, QuantaIndex, SearchCorpusBatch};
use quanta_index_searchd_harness::ScaleStageError;
use quanta_index_searchd_harness::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest, corpus_digest,
    model_revision_of,
};
use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, SearchdBinaryProcess, daemon_socket_paths, legacy_source_batch,
    legacy_source_replacement_from_chunks, publish_legacy_source_batch, request_when_ready,
};
use quanta_index_searchd_harness::offered_load::{
    self, MAX_TOTAL_REQUESTS, OfferedConfig, OfferedTask, scheduled_count,
};
use quanta_index_searchd_harness::open_loop_accounting::{
    Completion, LoadPoint, Outcome, summarize,
};
use quanta_index_searchd_harness::private_tempdir;
use quanta_index_searchd_harness::scale_disk_census::DiskObservationMode;
use quanta_index_searchd_harness::scale_provenance::scoped_corpus_digest;
use quanta_index_searchd_harness::scale_runtime_policy::{
    DEFAULT_HISTORY_MAX_GENERATIONS, HARNESS_HISTORY_MAX_REVISION_PAIRS, ScaleRuntimeConfig,
    daemon_scale_capacity_environment,
};
use quanta_index_searchd_harness::source_fixture::{
    ScaleTier, ScopedFile, ScopedOracle, generate_corpus, generate_scoped_corpus, params_for,
    preflight_scoped_corpus, repo_query_token,
};
use serde_json::{Value, json};

pub(crate) const DIMENSION: &str = "open-loop";
const BACKEND: &str = "legacy-reference";
const QUERY: &str = "scale_needle_token";
const TOP_K: u32 = 10;

pub(crate) use quanta_index_searchd_harness::offered_load::ArrivalModel;
#[cfg(test)]
use quanta_index_searchd_harness::offered_load::scheduled_offset;

#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub seed: u64,
    pub tier: ScaleTier,
    pub arrival_model: ArrivalModel,
    pub rates_qps: Vec<u32>,
    pub duration: Duration,
    pub workers: usize,
    pub queue_capacity: usize,
    pub request_timeout: Duration,
    /// Explicit per-pair history budget; `None` uses the scale-supported 1 GiB bound.
    pub history_max_bytes: Option<u64>,
    /// Explicit total retained-history budget; `None` uses the scale-supported 2 GiB bound.
    /// An explicit total requires a pair override.
    pub history_max_total_bytes: Option<u64>,
}

impl Config {
    fn offered_config(&self, rate_qps: u32) -> OfferedConfig {
        OfferedConfig {
            seed: self.seed,
            arrival_model: self.arrival_model,
            rate_qps,
            duration: self.duration,
            workers: self.workers,
            queue_capacity: self.queue_capacity,
            request_timeout: self.request_timeout,
        }
    }

    fn history_config(&self) -> ScaleRuntimeConfig {
        ScaleRuntimeConfig {
            disk_observation: DiskObservationMode::BoundaryOnly,
            client_timeout: None,
            history_max_bytes: self.history_max_bytes,
            history_max_total_bytes: self.history_max_total_bytes,
        }
    }

    fn effective_history_max_bytes(&self) -> AnyResult<u64> {
        self.history_config().effective_history_max_bytes()
    }

    pub(crate) fn runtime_policy_json(&self) -> AnyResult<Value> {
        let mut policy = self.history_config().execution_json()?;
        let object = policy
            .as_object_mut()
            .context("open-loop runtime policy must be a JSON object")?;
        let _previous = object.insert(
            "history_max_generations".to_string(),
            json!(DEFAULT_HISTORY_MAX_GENERATIONS),
        );
        Ok(policy)
    }

    pub(crate) fn history_policy_json(&self) -> AnyResult<Value> {
        let policy = self.history_config();
        Ok(json!({
            "history_max_generations": DEFAULT_HISTORY_MAX_GENERATIONS,
            "history_max_bytes": self.effective_history_max_bytes()?,
            "requested_history_max_bytes": self.history_max_bytes,
            "history_max_total_bytes": policy.effective_history_max_total_bytes()?,
            "requested_history_max_total_bytes": self.history_max_total_bytes,
            "history_policy_id": policy.history_policy_id()?,
            "history_max_revision_pairs": HARNESS_HISTORY_MAX_REVISION_PAIRS,
        }))
    }

    pub(crate) fn validate(&self) -> AnyResult<()> {
        let _effective_history_max_bytes = self.effective_history_max_bytes()?;
        ensure!(
            !self.rates_qps.is_empty(),
            "at least one offered QPS is required"
        );
        ensure!(
            self.duration >= Duration::from_millis(100) && self.duration <= Duration::from_secs(60),
            "duration must be 100 ms..60 s"
        );
        ensure!(
            self.request_timeout >= Duration::from_millis(50)
                && self.request_timeout <= Duration::from_secs(30),
            "request timeout must be 50 ms..30 s"
        );
        ensure!((1..=128).contains(&self.workers), "workers must be 1..128");
        ensure!(
            (1..=4096).contains(&self.queue_capacity),
            "queue capacity must be 1..4096"
        );
        let mut previous = 0;
        let mut total = 0_u128;
        for rate in &self.rates_qps {
            ensure!(
                *rate > previous,
                "offered QPS values must be positive and strictly increasing"
            );
            previous = *rate;
            total = total
                .checked_add(scheduled_count(self.duration, *rate)?)
                .context("total offered requests overflow")?;
            ensure!(
                total <= MAX_TOTAL_REQUESTS,
                "at most {MAX_TOTAL_REQUESTS} requests may be offered"
            );
            ensure!(
                !scheduled_offsets(self, *rate)?.is_empty(),
                "open-loop schedule offers no requests at {rate} QPS"
            );
        }
        Ok(())
    }
}

fn scheduled_offsets(config: &Config, rate: u32) -> AnyResult<Vec<Duration>> {
    offered_load::scheduled_offsets(&config.offered_config(rate))
}

/// One generated chunk per source file carries the planted exact query.
/// The source fixture, rather than an earlier engine response, defines which
/// paths may appear in a correct baseline page.
fn source_fixture_paths(corpus: &[(String, String)]) -> AnyResult<BTreeSet<String>> {
    ensure!(!corpus.is_empty(), "open-loop source fixture is empty");
    ensure!(
        corpus.iter().all(|(_, content)| content.contains(QUERY)),
        "open-loop source fixture lacks the planted query"
    );
    let paths: BTreeSet<String> = corpus.iter().map(|(path, _)| path.clone()).collect();
    ensure!(
        paths.len() == corpus.len(),
        "open-loop source fixture repeats a file path"
    );
    Ok(paths)
}

fn fixture_candidate_ids<'a>(
    paths: &BTreeSet<String>,
    rows: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> AnyResult<Vec<String>> {
    let rows: Vec<_> = rows.into_iter().collect();
    let expected = paths.len().min(usize::try_from(TOP_K)?);
    ensure!(
        rows.len() == expected,
        "open-loop baseline returned {} candidates; source fixture requires {expected}",
        rows.len()
    );
    let mut ids = BTreeSet::new();
    let mut seen_paths = BTreeSet::new();
    for (candidate_id, path) in &rows {
        ensure!(
            paths.contains(*path),
            "open-loop baseline contains a foreign source path"
        );
        ensure!(
            seen_paths.insert(*path),
            "open-loop baseline repeats a source file"
        );
        ensure!(
            !candidate_id.is_empty() && ids.insert(*candidate_id),
            "open-loop baseline has an empty or repeated candidate ID"
        );
    }
    Ok(rows.into_iter().map(|(id, _)| id.to_string()).collect())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpectedRow {
    candidate_id: String,
    source_repo_id: String,
    repo_relative_path: String,
}

fn matches_expected_rows<'a>(
    expected: &[ExpectedRow],
    observed: impl IntoIterator<Item = (&'a str, &'a str, &'a str)>,
) -> bool {
    let mut rows = observed.into_iter();
    expected.iter().all(|row| {
        rows.next().is_some_and(|observed| {
            observed
                == (
                    row.candidate_id.as_str(),
                    row.source_repo_id.as_str(),
                    row.repo_relative_path.as_str(),
                )
        })
    }) && rows.next().is_none()
}

fn request(pin: &GenerationPin) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Text(text_request(QUERY, pin))
}

fn text_request(query: &str, pin: &GenerationPin) -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: query.to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(pin.clone()),
        generation_selector: None,
        top_k: TOP_K,
        cursor: None,
    }
}

fn transport_kind(error: &IpcError) -> String {
    match error {
        IpcError::Io(err) => format!("io::{:?}", err.kind()),
        IpcError::Truncated => "truncated".to_string(),
        IpcError::Oversized(_) => "oversized".to_string(),
        IpcError::EmptyFrame => "empty_frame".to_string(),
        IpcError::Encode(_) => "encode".to_string(),
        IpcError::Decode(_) => "decode".to_string(),
        IpcError::IngressSaturated { .. } => "ingress_saturated".to_string(),
        IpcError::FrameMemoryLimited { .. } => "frame_memory_limited".to_string(),
        IpcError::ZeroRequestId => "zero_request_id".to_string(),
        IpcError::PeerBindingRefused(_) => "peer_binding_refused".to_string(),
        IpcError::Timeout { .. }
        | IpcError::InvalidClientIoTimeout
        | IpcError::ClientIoDeadlineElapsed
        | IpcError::ReadinessTimeout { .. }
        | IpcError::InvalidAdmissionPolicy
        | IpcError::SocketInUse(_)
        | IpcError::SocketPathInsecure { .. }
        | IpcError::SocketAccessUnsatisfiable { .. } => "policy_or_socket".to_string(),
    }
}

fn dispatch(
    socket: &Path,
    pin: &GenerationPin,
    expected_rows: &[ExpectedRow],
    task: OfferedTask,
    timeout: Duration,
) -> Completion {
    let now = Instant::now();
    let lag = now.saturating_duration_since(task.scheduled);
    let lag_ms = lag.as_secs_f64() * 1000.0;
    let Some(deadline) = task.scheduled.checked_add(timeout) else {
        return Completion {
            outcome: Outcome::QueueDeadline,
            elapsed_ms: None,
            dispatch_lag_ms: lag_ms,
        };
    };
    if now >= deadline {
        return Completion {
            outcome: Outcome::QueueDeadline,
            elapsed_ms: None,
            dispatch_lag_ms: lag_ms,
        };
    }
    let envelope = SearchPlaneQueryIpcRequestEnvelope {
        request_id: task.id,
        diagnostic_attempt: quanta_index_contract::ipc::DiagnosticAttemptFieldV1::Absent,
        payload: request(pin),
    };
    let outcome = match ClientIoPolicy::try_with_deadline(deadline).and_then(|policy| {
        send_request::<_, SearchPlaneQueryIpcResponseEnvelope>(socket, &envelope, policy)
    }) {
        Ok(response) if response.request_id != task.id => Outcome::InvalidResult,
        Ok(response) => match response.payload {
            SearchPlaneQueryIpcResponse::Text(page)
                if page.generation == *pin
                    && page.rank_unit == TextRankUnit::Chunk
                    && matches_expected_rows(
                        expected_rows,
                        page.results.iter().map(|row| {
                            (
                                row.candidate_id.as_str(),
                                row.source_repo_id.as_str(),
                                row.repo_relative_path.as_str(),
                            )
                        }),
                    ) =>
            {
                u64::try_from(page.results.len()).map_or(Outcome::InvalidResult, |result_count| {
                    Outcome::Served { result_count }
                })
            }
            SearchPlaneQueryIpcResponse::Error(error)
                if error.code == SearchPlaneErrorCodeV2::ServerOverloaded =>
            {
                Outcome::Overloaded {
                    code: error.code.as_wire_str().to_string(),
                }
            }
            SearchPlaneQueryIpcResponse::Error(error) => Outcome::TypedError {
                code: error.code.as_wire_str().to_string(),
            },
            SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
            | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
            | SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::HybridFileV1(_)
            | SearchPlaneQueryIpcResponse::GoTestTraceFileV1(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)

            | SearchPlaneQueryIpcResponse::Explain(_)
            | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_) => Outcome::InvalidResult,
        },
        Err(IpcError::Timeout { .. } | IpcError::ClientIoDeadlineElapsed) => Outcome::Timeout,
        Err(error) => Outcome::TransportError {
            kind: transport_kind(&error),
        },
    };
    Completion {
        outcome,
        elapsed_ms: Some(task.scheduled.elapsed().as_secs_f64() * 1000.0),
        dispatch_lag_ms: lag_ms,
    }
}

fn measure_point(
    socket: &Path,
    pin: &GenerationPin,
    expected_rows: &[ExpectedRow],
    config: &Config,
    rate: u32,
) -> AnyResult<LoadPoint> {
    let socket = PathBuf::from(socket);
    let pin = pin.clone();
    let expected_rows = expected_rows.to_vec();
    let cancel = Arc::new(AtomicBool::new(false));
    let run = offered_load::run_point(
        &config.offered_config(rate),
        &cancel,
        move |task, timeout| dispatch(&socket, &pin, &expected_rows, task, timeout),
    )?;
    summarize(
        rate,
        config.duration,
        run.offered,
        run.dropped_queue_full,
        run.dropped_scheduler_late,
        run.completions
            .into_iter()
            .map(|entry| entry.value)
            .collect(),
        run.drain_secs,
    )
}

#[derive(Clone, Debug)]
pub(crate) struct Report {
    pub config: Config,
    pub corpus_digest: String,
    pub model_revision: Option<String>,
    pub daemon_sha256: String,
    pub resources: ResourceUsageV1,
    pub points: Vec<LoadPoint>,
}

impl Report {
    pub(crate) fn passed(&self) -> bool {
        // A load ladder must establish one healthy operating point. Above its
        // saturation boundary the listener may refuse connections at its hard
        // cap; those transport failures are capacity evidence, not proof that
        // a served response was incorrect. They remain counted in every row.
        self.points.first().is_some_and(|point| !point.saturated)
            && self
                .points
                .iter()
                .all(|point| point.invalid_results == 0 && point.unexpected_typed_errors == 0)
    }

    pub(crate) fn saturation_onset_qps(&self) -> Option<u32> {
        self.points
            .iter()
            .find(|point| point.saturated)
            .map(|point| point.target_qps)
    }
}

fn scoped_batch(
    repo: &RepoId,
    revision: &RevisionId,
    files: &[ScopedFile],
) -> AnyResult<SearchCorpusBatch> {
    let language = LanguageCode::new("rust").map_err(anyhow::Error::msg)?;
    let chunks = (1_u64..)
        .zip(files)
        .map(|(id, file)| {
            let path = &file.repo_relative_path;
            Ok(ChunkRecord {
                chunk_id: ChunkId::new(format!("e2e-{id}-{path}")),
                repo_relative_path: RepoRelativePath::new(path.as_str()),
                language: language.clone(),
                start_byte: 0,
                end_byte: u32::try_from(file.content.len())?,
                start_line: 1,
                end_line: 2,
                text: file.content.to_owned().into_boxed_str(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: Some(RepoId::new(file.source_repo_id.as_str())?),
            })
        })
        .collect::<AnyResult<_>>()?;
    legacy_source_replacement_from_chunks(repo, revision, 1, chunks)
        .map_err(|error| anyhow::anyhow!("prepare scoped Source: {error}"))
}

pub(crate) fn run(config: Config) -> AnyResult<Report> {
    config.validate()?;
    let binary = LegacySearchdBinary::from_environment()
        .map_err(|error| anyhow::anyhow!("select pinned old-Q open-loop daemon: {error}"))?;
    let repo = RepoId::new("repo-e2e")?;
    let revision = RevisionId::new("rev-e2e")?;
    let (batch, source_paths, corpus_digest, scoped_oracle) = if config.tier == ScaleTier::Small {
        let corpus = generate_corpus(ScaleTier::Small, config.seed);
        let source_paths = source_fixture_paths(&corpus)?;
        let digest = corpus_digest(DIMENSION, &corpus);
        let files = (1_u64..)
            .zip(&corpus)
            .map(|(id, (path, content))| (path.as_str(), content.as_str(), id))
            .collect::<Vec<_>>();
        let batch = legacy_source_batch(&repo, &revision, 1, None, &files)
            .map_err(|error| anyhow::anyhow!("prepare small Source: {error}"))?;
        (batch, source_paths, digest, None)
    } else {
        let files = generate_scoped_corpus(config.tier, config.seed)?;
        let oracle = ScopedOracle::from_source(&files, config.tier)?;
        let _admission = preflight_scoped_corpus(&files)
            .map_err(|error| ScaleStageError::source_admission(&error))?;
        let source_paths = files
            .iter()
            .map(|file| format!("{}/{}", file.source_repo_id, file.repo_relative_path))
            .collect::<BTreeSet<_>>();
        ensure!(
            source_paths.len() == files.len(),
            "open-loop scoped source identity repeats"
        );
        let digest = scoped_corpus_digest(DIMENSION, &files);
        let batch = scoped_batch(&repo, &revision, &files)?;
        (batch, source_paths, digest, Some(oracle))
    };

    let root = private_tempdir()?;
    let policy = config.history_config();
    let client_timeout = Duration::from_millis(policy.effective_timeout_ms()?);
    let model = quanta_index_searchd::app::SemanticEmbedderProfile::hash_dev();
    let mut command = binary
        .benchmark_command(root.path(), DEFAULT_HISTORY_MAX_GENERATIONS)
        .map_err(|error| anyhow::anyhow!("prepare pinned old-Q open-loop daemon: {error}"))?;
    let _configured = command
        .envs(policy.daemon_history_environment()?)
        .envs(daemon_scale_capacity_environment());
    let process = SearchdBinaryProcess::start_with_pinned_command(root.path(), command)
        .map_err(|error| ScaleStageError::operation("runtime_boot", &anyhow::anyhow!("{error}")))?;
    let observation = (|| -> AnyResult<_> {
        let client = QuantaIndex::connect_legacy(
            ConnectOptions::from_state_root(root.path()).with_request_io_timeout(client_timeout),
        )?;
        // The SDK applies its exact encoded upload admission before sampling.
        let (head, _receipt) =
            publish_legacy_source_batch(&client, &batch, None).map_err(|error| {
                ScaleStageError::operation("build_seal", &anyhow::anyhow!("{error}"))
            })?;
        let pin = GenerationPin::new(repo, revision, head.generation.lexical.manifest_generation);
        let primed =
            request_when_ready(|| client.lexical().query_request(text_request(QUERY, &pin)))?;
        ensure!(
            primed.generation == pin && !primed.results.is_empty(),
            "fixture must serve a nonempty pinned lexical answer before timing"
        );
        if let Some(oracle) = &scoped_oracle {
            for repo_index in 0..params_for(config.tier).repo_count {
                let source_repo_id = format!("repo{repo_index}");
                let response = client
                    .lexical()
                    .query_request(text_request(&repo_query_token(repo_index), &pin))?;
                ensure!(
                    response.generation == pin,
                    "open-loop scoped probe generation changed"
                );
                let _result_count = oracle.verify_page(Some(&source_repo_id), &response.results)?;
            }
        }
        let page = client.lexical().query_request(text_request(QUERY, &pin))?;
        ensure!(
            page.generation == pin && page.rank_unit == TextRankUnit::Chunk,
            "pinned IPC query failed correctness preflight"
        );
        if config.tier == ScaleTier::Small {
            ensure!(
                page.results
                    .iter()
                    .all(|row| row.source_repo_id == pin.repo_id),
                "open-loop baseline returned a foreign source repository"
            );
        }
        let observed = page
            .results
            .iter()
            .map(|row| {
                let path = if config.tier == ScaleTier::Small {
                    row.repo_relative_path.as_str().to_string()
                } else {
                    format!(
                        "{}/{}",
                        row.source_repo_id.as_str(),
                        row.repo_relative_path.as_str()
                    )
                };
                (row.candidate_id.as_str(), path)
            })
            .collect::<Vec<_>>();
        let _validated_ids = fixture_candidate_ids(
            &source_paths,
            observed.iter().map(|(id, path)| (*id, path.as_str())),
        )?;
        let expected_rows = page
            .results
            .into_iter()
            .map(|row| ExpectedRow {
                candidate_id: row.candidate_id,
                source_repo_id: row.source_repo_id.as_str().to_string(),
                repo_relative_path: row.repo_relative_path.as_str().to_string(),
            })
            .collect::<Vec<_>>();
        let [socket, _, _] = daemon_socket_paths(root.path());
        let mut points = Vec::with_capacity(config.rates_qps.len());
        for rate in &config.rates_qps {
            points.push(measure_point(
                &socket,
                &pin,
                &expected_rows,
                &config,
                *rate,
            )?);
        }
        let peak_rss_bytes = process.observe_peak_rss(&client)?;
        Ok((points, ResourceUsageV1 { peak_rss_bytes }))
    })();
    let stopped = process
        .stop()
        .map_err(|error| anyhow::anyhow!("stop pinned old-Q open-loop daemon: {error}"));
    let (points, resources) = match (observation, stopped) {
        (Ok(value), Ok(())) => value,
        (Err(primary), Ok(())) | (Ok(_), Err(primary)) => return Err(primary),
        (Err(primary), Err(cleanup)) => return Err(primary.context(format!("{cleanup:#}"))),
    };
    binary
        .verify()
        .map_err(|error| anyhow::anyhow!("verify pinned old-Q open-loop daemon: {error}"))?;
    Ok(Report {
        config,
        corpus_digest,
        model_revision: model_revision_of(&model),
        daemon_sha256: binary.sha256().to_owned(),
        resources,
        points,
    })
}

pub(crate) fn artifact(
    report: &Report,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<BenchArtifactV1> {
    let config = &report.config;
    let history_policy = config.history_policy_json()?;
    let rows = report
        .points
        .iter()
        .map(|point| -> AnyResult<BenchRowV1> {
            let error_count = point
                .typed_errors
                .checked_add(point.transport_errors)
                .and_then(|count| count.checked_add(point.invalid_results))
                .context("open-loop artifact error count overflow")?;
            Ok(BenchRowV1 {
                scenario_id: format!("open_loop.lexical.qps{}", point.target_qps),
                route_family: RouteFamily::Lexical,
                syntax: BenchSyntax::Native,
                result_shape: ResultShape::Candidates,
                latency: point.latency,
                qps: Some(point.achieved_qps),
                error_count,
                timeout_count: point.timeouts,
                result_count: point.last_result_count,
                typed_error_code: point.error_codes.first().cloned(),
                engine_touched: vec!["lexical".to_string()],
                early_stop_reason: point
                    .latency
                    .is_none()
                    .then(|| "all_offered_requests_dropped".to_string()),
            })
        })
        .collect::<AnyResult<Vec<_>>>()?;
    Ok(BenchArtifactV1 {
        dimension: DIMENSION.to_string(),
        mode: BenchMode::Warm,
        concurrency: u32::try_from(config.workers)?,
        provenance: BenchProvenanceV1 {
            git_head,
            corpus_digest: report.corpus_digest.clone(),
            config_digest: config_digest(
                DIMENSION,
                &[
                    ("backend", BACKEND.to_owned()),
                    ("daemon_sha256", report.daemon_sha256.clone()),
                    ("seed", config.seed.to_string()),
                    ("arrival_model", config.arrival_model.as_str().to_string()),
                    (
                        "rates_qps",
                        config
                            .rates_qps
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(","),
                    ),
                    ("duration_ms", config.duration.as_millis().to_string()),
                    ("workers", config.workers.to_string()),
                    ("queue_capacity", config.queue_capacity.to_string()),
                    (
                        "request_timeout_ms",
                        config.request_timeout.as_millis().to_string(),
                    ),
                    ("tier", config.tier.as_str().to_string()),
                    (
                        "history_max_generations",
                        DEFAULT_HISTORY_MAX_GENERATIONS.to_string(),
                    ),
                    (
                        "history_max_bytes",
                        config.effective_history_max_bytes()?.to_string(),
                    ),
                    (
                        "requested_history_max_bytes",
                        format!("{:?}", config.history_max_bytes),
                    ),
                    (
                        "requested_history_max_total_bytes",
                        format!("{:?}", config.history_max_total_bytes),
                    ),
                    (
                        "history_policy_id",
                        config.history_config().history_policy_id()?.to_string(),
                    ),
                    (
                        "history_max_total_bytes",
                        config
                            .history_config()
                            .effective_history_max_total_bytes()?
                            .to_string(),
                    ),
                    (
                        "history_max_revision_pairs",
                        HARNESS_HISTORY_MAX_REVISION_PAIRS.to_string(),
                    ),
                    ("runtime_policy", config.runtime_policy_json()?.to_string()),
                    ("query", QUERY.to_string()),
                    ("top_k", TOP_K.to_string()),
                ],
            ),
            model_revision: report.model_revision.clone(),
        },
        host,
        resources: report.resources,
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows,
        detail: json!({
            "backend": BACKEND,
            "daemon_sha256": report.daemon_sha256,
            "peak_rss_scope": "daemon_process",
            "passed": report.passed(),
            "arrival_model": config.arrival_model.as_str(),
            "latency_origin": "scheduled_arrival",
            "latency_sample_scope": "completed_requests_including_errors_and_timeouts_excluding_drops",
            "duration_ms": config.duration.as_millis(),
            "request_timeout_ms": config.request_timeout.as_millis(),
            "queue_capacity": config.queue_capacity,
            "workers": config.workers,
            "tier": config.tier.as_str(),
            "history_policy": history_policy,
            "runtime_policy": config.runtime_policy_json()?,
            "serving_owner_count": 1,
            "source_repo_count": params_for(config.tier).repo_count,
            "saturation_onset_qps": report.saturation_onset_qps(),
            "points": report.points.iter().map(LoadPoint::detail).collect::<Vec<_>>(),
        }),
    })
}

#[cfg(test)]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions intentionally fail while fixture setup uses the Result operator"
)]
mod tests {
    use super::*;

    fn required_json<'a>(value: &'a Value, pointer: &str) -> AnyResult<&'a Value> {
        value
            .pointer(pointer)
            .with_context(|| format!("missing JSON field {pointer}"))
    }

    #[test]
    fn zero_request_id_keeps_its_protocol_failure_kind() {
        assert_eq!(transport_kind(&IpcError::ZeroRequestId), "zero_request_id");
        assert_eq!(
            transport_kind(&IpcError::PeerBindingRefused(
                quanta_index_ipc::PeerBindingRefusalV1::ForeignProcess,
            )),
            "peer_binding_refused"
        );
    }

    #[test]
    fn schedule_has_fixed_arrivals_independent_of_completion() -> AnyResult<()> {
        assert_eq!(scheduled_count(Duration::from_millis(250), 10)?, 3);
        assert_eq!(scheduled_offset(0, 10)?, Duration::ZERO);
        assert_eq!(scheduled_offset(2, 10)?, Duration::from_millis(200));
        Ok(())
    }

    #[test]
    fn seeded_poisson_schedule_is_replayable_and_not_fixed_spacing() -> AnyResult<()> {
        let config = Config {
            seed: 7,
            tier: ScaleTier::Small,
            arrival_model: ArrivalModel::SeededPoisson,
            rates_qps: vec![10],
            duration: Duration::from_secs(1),
            workers: 1,
            queue_capacity: 1,
            request_timeout: Duration::from_secs(1),
            history_max_bytes: None,
            history_max_total_bytes: None,
        };
        let first = scheduled_offsets(&config, 10)?;
        assert_eq!(first, scheduled_offsets(&config, 10)?);
        assert!(!first.is_empty());
        assert!(
            first
                .windows(2)
                .any(|pair| pair
                    .first()
                    .zip(pair.get(1))
                    .is_some_and(|(earlier, later)| {
                        later.checked_sub(*earlier) != Some(Duration::from_millis(100))
                    }))
        );
        Ok(())
    }

    #[test]
    fn empty_seeded_arrivals_cannot_be_a_healthy_load_point() -> AnyResult<()> {
        let empty = Config {
            seed: 0,
            tier: ScaleTier::Small,
            arrival_model: ArrivalModel::SeededPoisson,
            rates_qps: vec![1],
            duration: Duration::from_millis(100),
            workers: 1,
            queue_capacity: 1,
            request_timeout: Duration::from_secs(1),
            history_max_bytes: None,
            history_max_total_bytes: None,
        };
        assert!(scheduled_offsets(&empty, 1)?.is_empty());
        assert!(empty.validate().is_err());
        assert!(summarize(1, empty.duration, 0, 0, 0, Vec::new(), 0.1).is_err());

        let admitted = Config {
            rates_qps: vec![3],
            duration: Duration::from_secs(10),
            ..empty
        };
        admitted.validate()?;
        assert!(!scheduled_offsets(&admitted, 3)?.is_empty());
        Ok(())
    }

    #[test]
    fn source_fixture_is_independent_of_the_engine_baseline() -> AnyResult<()> {
        let corpus = generate_corpus(ScaleTier::Small, 0);
        let paths = source_fixture_paths(&corpus)?;
        assert_eq!(paths.len(), 16);
        let rows = paths
            .iter()
            .take(10)
            .enumerate()
            .map(|(index, path)| (format!("id-{index}"), path.clone()))
            .collect::<Vec<_>>();
        let observed = rows.iter().map(|(id, path)| (id.as_str(), path.as_str()));
        assert_eq!(fixture_candidate_ids(&paths, observed)?.len(), 10);
        assert!(
            fixture_candidate_ids(
                &paths,
                rows.iter()
                    .take(9)
                    .map(|(id, path)| (id.as_str(), path.as_str()))
            )
            .is_err()
        );
        let mut wrong_path = rows.clone();
        wrong_path
            .first_mut()
            .context("missing first candidate row")?
            .1 = "foreign.rs".to_string();
        assert!(
            fixture_candidate_ids(
                &paths,
                wrong_path
                    .iter()
                    .map(|(id, path)| (id.as_str(), path.as_str()))
            )
            .is_err()
        );
        let mut duplicate_id = rows.clone();
        let first_id = duplicate_id
            .first()
            .context("missing first candidate row")?
            .0
            .clone();
        duplicate_id
            .get_mut(1)
            .context("missing second candidate row")?
            .0 = first_id;
        assert!(
            fixture_candidate_ids(
                &paths,
                duplicate_id
                    .iter()
                    .map(|(id, path)| (id.as_str(), path.as_str()))
            )
            .is_err()
        );
        let mut duplicate_path = rows.clone();
        let first_path = duplicate_path
            .first()
            .context("missing first candidate row")?
            .1
            .clone();
        duplicate_path
            .get_mut(1)
            .context("missing second candidate row")?
            .1 = first_path;
        assert!(
            fixture_candidate_ids(
                &paths,
                duplicate_path
                    .iter()
                    .map(|(id, path)| (id.as_str(), path.as_str()))
            )
            .is_err()
        );
        let mut missing_needle = corpus;
        let first_file = missing_needle
            .first_mut()
            .context("missing first source file")?;
        first_file.1 = first_file.1.replace(QUERY, "absent");
        assert!(source_fixture_paths(&missing_needle).is_err());
        Ok(())
    }

    #[test]
    fn medium_preflight_preserves_source_repo_identity_for_same_relative_path() -> AnyResult<()> {
        let files = generate_scoped_corpus(ScaleTier::Medium, 7)?;
        let _oracle = ScopedOracle::from_source(&files, ScaleTier::Medium)?;
        let _admission = preflight_scoped_corpus(&files)?;
        let paths = files
            .iter()
            .map(|file| format!("{}/{}", file.source_repo_id, file.repo_relative_path))
            .collect::<BTreeSet<_>>();
        assert!(paths.contains("repo0/src/file_0.rs"));
        assert!(paths.contains("repo1/src/file_0.rs"));
        assert_eq!(paths.len(), 256);
        let rows = paths
            .iter()
            .take(10)
            .enumerate()
            .map(|(index, path)| (format!("id-{index}"), path.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            fixture_candidate_ids(
                &paths,
                rows.iter().map(|(id, path)| (id.as_str(), path.as_str()))
            )?
            .len(),
            10
        );
        let mut wrong_repo = rows;
        wrong_repo
            .first_mut()
            .context("missing first candidate row")?
            .1 = "repo4/src/file_0.rs".to_string();
        assert!(
            fixture_candidate_ids(
                &paths,
                wrong_repo
                    .iter()
                    .map(|(id, path)| (id.as_str(), path.as_str()))
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn measured_response_rejects_identity_change_with_same_candidate_ids() {
        let expected = vec![ExpectedRow {
            candidate_id: "candidate-0".to_string(),
            source_repo_id: "repo0".to_string(),
            repo_relative_path: "src/file_0.rs".to_string(),
        }];
        assert!(matches_expected_rows(
            &expected,
            [("candidate-0", "repo0", "src/file_0.rs")]
        ));
        assert!(!matches_expected_rows(
            &expected,
            [("candidate-0", "repo1", "src/file_0.rs")]
        ));
        assert!(!matches_expected_rows(
            &expected,
            [("candidate-0", "repo0", "src/foreign.rs")]
        ));
    }

    #[test]
    fn transport_failures_keep_their_kind() -> AnyResult<()> {
        let point = summarize(
            1,
            Duration::from_secs(1),
            1,
            0,
            0,
            vec![Completion {
                outcome: Outcome::TransportError {
                    kind: "io::ConnectionReset".to_string(),
                },
                elapsed_ms: Some(5.0),
                dispatch_lag_ms: 0.0,
            }],
            1.0,
        )?;
        assert_eq!(point.transport_errors, 1);
        assert_eq!(
            point.transport_error_kinds.get("io::ConnectionReset"),
            Some(&1)
        );
        assert!(
            !Report {
                config: Config {
                    seed: 1,
                    tier: ScaleTier::Small,
                    arrival_model: ArrivalModel::SeededPoisson,
                    rates_qps: vec![1],
                    duration: Duration::from_secs(1),
                    workers: 1,
                    queue_capacity: 1,
                    request_timeout: Duration::from_secs(1),
                    history_max_bytes: None,
                    history_max_total_bytes: None,
                },
                corpus_digest: String::new(),
                model_revision: None,
                daemon_sha256: "a".repeat(64),
                resources: ResourceUsageV1 {
                    peak_rss_bytes: 16_384
                },
                points: vec![point],
            }
            .passed()
        );
        Ok(())
    }

    #[test]
    fn transport_refusal_after_a_healthy_point_is_capacity_evidence() -> AnyResult<()> {
        let healthy = summarize(
            1,
            Duration::from_secs(1),
            1,
            0,
            0,
            vec![Completion {
                outcome: Outcome::Served { result_count: 2 },
                elapsed_ms: Some(5.0),
                dispatch_lag_ms: 0.0,
            }],
            1.0,
        )?;
        let overloaded = summarize(
            2,
            Duration::from_secs(1),
            2,
            0,
            0,
            vec![
                Completion {
                    outcome: Outcome::Served { result_count: 2 },
                    elapsed_ms: Some(5.0),
                    dispatch_lag_ms: 0.0,
                },
                Completion {
                    outcome: Outcome::TransportError {
                        kind: "truncated".to_string(),
                    },
                    elapsed_ms: Some(10.0),
                    dispatch_lag_ms: 0.0,
                },
            ],
            1.0,
        )?;
        let report = Report {
            config: Config {
                seed: 1,
                tier: ScaleTier::Small,
                arrival_model: ArrivalModel::SeededPoisson,
                rates_qps: vec![1, 2],
                duration: Duration::from_secs(1),
                workers: 1,
                queue_capacity: 1,
                request_timeout: Duration::from_secs(1),
                history_max_bytes: None,
                history_max_total_bytes: None,
            },
            corpus_digest: String::new(),
            model_revision: None,
            daemon_sha256: "a".repeat(64),
            resources: ResourceUsageV1 {
                peak_rss_bytes: 16_384,
            },
            points: vec![healthy, overloaded],
        };
        assert!(report.passed());
        assert_eq!(
            report
                .points
                .get(1)
                .context("missing saturated load point")?
                .transport_error_kinds
                .get("truncated"),
            Some(&1)
        );
        assert_eq!(report.saturation_onset_qps(), Some(2));
        Ok(())
    }

    #[test]
    fn bounded_real_runtime_smoke() -> AnyResult<()> {
        let report = run(Config {
            seed: 1,
            tier: ScaleTier::Small,
            // This smoke test verifies the bounded executor. Keep its offered
            // count deterministic; qualification runs use SeededPoisson.
            arrival_model: ArrivalModel::DeterministicPeriodic,
            rates_qps: vec![10],
            duration: Duration::from_millis(200),
            workers: 2,
            queue_capacity: 2,
            request_timeout: Duration::from_secs(2),
            history_max_bytes: None,
            history_max_total_bytes: None,
        })?;
        assert_eq!(report.points.len(), 1);
        let point = report.points.first().context("missing load point")?;
        assert_eq!(point.offered, 2);
        assert_eq!(point.served, 2);
        assert_eq!(
            point.served
                + point.typed_errors
                + point.timeouts
                + point.transport_errors
                + point.invalid_results
                + point.dropped_queue_full
                + point.dropped_scheduler_late
                + point.dropped_deadline,
            2
        );
        Ok(())
    }

    #[test]
    fn history_budget_uses_scale_policy_bounds_and_reaches_real_seal() -> AnyResult<()> {
        let config = Config {
            seed: 1,
            tier: ScaleTier::Small,
            arrival_model: ArrivalModel::DeterministicPeriodic,
            rates_qps: vec![10],
            duration: Duration::from_millis(200),
            workers: 2,
            queue_capacity: 2,
            request_timeout: Duration::from_secs(2),
            history_max_bytes: Some(1),
            history_max_total_bytes: None,
        };
        assert_eq!(config.effective_history_max_bytes()?, 1);
        let error = run(config.clone()).expect_err("one byte cannot retain the planted index");
        let stage = error
            .downcast_ref::<ScaleStageError>()
            .context("typed seal stage")?;
        assert_eq!(stage.stage, "build_seal");
        assert!(
            stage
                .message
                .contains("SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED")
        );

        let mut boundary = config;
        boundary.history_max_bytes = Some(0);
        assert!(boundary.validate().is_err(), "zero pair budget must refuse");

        // Scale's supported default total is 2 GiB, independently of the
        // generic harness default. Equality is admissible; one byte more is not.
        boundary.history_max_bytes = Some(2_147_483_648);
        assert_eq!(boundary.effective_history_max_bytes()?, 2_147_483_648);
        boundary.validate()?;
        boundary.history_max_bytes = Some(2_147_483_649);
        assert!(
            boundary.validate().is_err(),
            "pair above default total must refuse"
        );

        // An explicit total selects its own bound and still constrains the pair.
        boundary.history_max_total_bytes = Some(536_870_912);
        boundary.history_max_bytes = Some(536_870_912);
        assert_eq!(boundary.effective_history_max_bytes()?, 536_870_912);
        boundary.validate()?;
        boundary.history_max_bytes = Some(536_870_913);
        assert!(
            boundary.validate().is_err(),
            "pair above explicit total must refuse"
        );
        Ok(())
    }

    #[test]
    fn open_loop_artifact_binds_requested_and_effective_history() -> AnyResult<()> {
        let point = summarize(
            10,
            Duration::from_secs(1),
            1,
            0,
            0,
            vec![Completion {
                outcome: Outcome::Served { result_count: 10 },
                elapsed_ms: Some(1.0),
                dispatch_lag_ms: 0.0,
            }],
            1.0,
        )?;
        let mut report = Report {
            config: Config {
                seed: 7,
                tier: ScaleTier::Small,
                arrival_model: ArrivalModel::DeterministicPeriodic,
                rates_qps: vec![10],
                duration: Duration::from_secs(1),
                workers: 1,
                queue_capacity: 8,
                request_timeout: Duration::from_secs(1),
                history_max_bytes: None,
                history_max_total_bytes: None,
            },
            corpus_digest: "sha256:fixed-source".to_string(),
            model_revision: None,
            daemon_sha256: "a".repeat(64),
            resources: ResourceUsageV1 {
                peak_rss_bytes: 16_384,
            },
            points: vec![point],
        };
        let head = GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567")?;
        let host = HostV1 {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            cpu_count: 4,
            mem_bytes: 1 << 30,
            hostname_hash: "sha256:host".to_string(),
        };
        let default = artifact(&report, head.clone(), host.clone())?.to_json()?;
        assert_eq!(
            required_json(&default, "/detail/backend")?,
            &json!("legacy-reference")
        );
        assert_eq!(
            required_json(&default, "/detail/daemon_sha256")?,
            &json!("a".repeat(64))
        );
        assert_eq!(
            required_json(&default, "/resources/peak_rss_bytes")?,
            &json!(16_384)
        );
        for (name, expected) in [
            ("query_dispatch_budget_ms", json!(600_000)),
            ("client_request_timeout_ms", json!(600_000)),
            ("ingest_max_text_bytes", json!(134_217_728)),
            ("source_publication_max_bytes", json!(536_870_912)),
            ("process_memory_ceiling_bytes", json!(4_294_967_296_u64)),
            ("history_max_generations", json!(8)),
        ] {
            ensure!(
                required_json(&default, &format!("/detail/runtime_policy/{name}"))? == &expected,
                "open-loop runtime policy {name} differs from supported capacity"
            );
        }
        ensure!(
            required_json(&default, "/detail/history_policy/history_max_bytes")?
                == &json!(1_073_741_824),
            "default history max bytes changed"
        );
        ensure!(
            required_json(&default, "/detail/history_policy/history_max_total_bytes")?
                == &json!(2_147_483_648_u64),
            "default total history max bytes changed"
        );
        ensure!(
            required_json(&default, "/detail/history_policy/history_policy_id")?
                == &json!("scale-supported-v1"),
            "default history policy changed"
        );
        ensure!(
            required_json(&default, "/detail/history_policy/history_max_generations")? == &json!(8),
            "default history generations changed"
        );
        ensure!(
            required_json(
                &default,
                "/detail/history_policy/requested_history_max_bytes"
            )? == &Value::Null,
            "default history request must be null"
        );
        ensure!(
            required_json(
                &default,
                "/detail/history_policy/requested_history_max_total_bytes"
            )? == &Value::Null,
            "default total history request must be null"
        );
        report.config.history_max_bytes = Some(268_435_456);
        let explicit = artifact(&report, head.clone(), host.clone())?.to_json()?;
        ensure!(
            required_json(&explicit, "/detail/history_policy/history_max_bytes")?
                == &json!(268_435_456),
            "explicit history max bytes changed"
        );
        ensure!(
            required_json(
                &explicit,
                "/detail/history_policy/requested_history_max_bytes"
            )? == &json!(268_435_456),
            "explicit requested history bytes changed"
        );
        ensure!(
            required_json(&default, "/provenance/config_digest")?
                != required_json(&explicit, "/provenance/config_digest")?,
            "history override did not change config digest"
        );
        report.config.history_max_bytes = Some(300_000_000);
        report.config.history_max_total_bytes = Some(600_000_000);
        let diagnostic = artifact(&report, head.clone(), host.clone())?.to_json()?;
        for (pointer, expected) in [
            (
                "/detail/history_policy/history_max_bytes",
                json!(300_000_000),
            ),
            (
                "/detail/history_policy/requested_history_max_bytes",
                json!(300_000_000),
            ),
            (
                "/detail/history_policy/history_max_total_bytes",
                json!(600_000_000),
            ),
            (
                "/detail/history_policy/requested_history_max_total_bytes",
                json!(600_000_000),
            ),
            (
                "/detail/history_policy/history_policy_id",
                json!("explicit-pair-total-diagnostic-v1"),
            ),
        ] {
            ensure!(
                required_json(&diagnostic, pointer)? == &expected,
                "{pointer} changed"
            );
        }
        ensure!(
            required_json(&diagnostic, "/provenance/config_digest")?
                != required_json(&explicit, "/provenance/config_digest")?,
            "total override did not change config digest"
        );
        report.config.history_max_total_bytes = Some(700_000_000);
        let wider_total = artifact(&report, head, host)?.to_json()?;
        ensure!(
            required_json(&diagnostic, "/provenance/config_digest")?
                != required_json(&wider_total, "/provenance/config_digest")?,
            "total-only change did not change config digest"
        );
        Ok(())
    }
}
