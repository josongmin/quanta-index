//! Scheduled arrivals over the real query IPC socket.
//!
//! Arrival timestamps are
//! fixed before dispatch; queueing and scheduler lag remain in end-to-end
//! latency, so a slow daemon cannot silently reduce the offered load.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result as AnyResult, ensure};
use quanta_index_contract::{
    GenerationPin, QueryConstraintSetV1, SearchPlaneErrorCodeV2, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, TextQueryRequest, TextQuerySyntax, TextRankUnit,
};
use quanta_index_ipc::{ClientIoPolicy, IpcError, send_request};
use quanta_index_searchd_harness::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    LatencySummary, PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest,
    corpus_digest, model_revision_of,
};
use quanta_index_searchd_harness::scale::{
    ScaleTier, ScopedOracle, generate_corpus, generate_scoped_corpus, params_for,
    preflight_scoped_corpus, repo_query_token, scoped_corpus_digest,
};
use quanta_index_searchd_harness::{E2eRuntime, E2eTextChunkSpec};
use serde_json::{Value, json};

pub(crate) const DIMENSION: &str = "open-loop";
const QUERY: &str = "scale_needle_token";
const TOP_K: u32 = 10;
const MAX_TOTAL_REQUESTS: u128 = 100_000;
const MIN_DELIVERY_RATIO: f64 = 0.95;

/// Arrival process used to schedule requests independently of completions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArrivalModel {
    /// Legacy fixed-spacing diagnostic schedule.
    DeterministicPeriodic,
    /// Seeded exponential inter-arrivals, the qualification default.
    SeededPoisson,
}

impl ArrivalModel {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::DeterministicPeriodic => "deterministic_periodic",
            Self::SeededPoisson => "seeded_poisson",
        }
    }
}

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
}

impl Config {
    pub(crate) fn validate(&self) -> AnyResult<()> {
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

fn scheduled_count(duration: Duration, rate: u32) -> AnyResult<u128> {
    ensure!(rate > 0, "offered QPS must be positive");
    let scaled = duration
        .as_nanos()
        .checked_mul(u128::from(rate))
        .context("offered request count overflow")?;
    Ok(scaled.div_ceil(1_000_000_000))
}

fn scheduled_offset(index: u128, rate: u32) -> AnyResult<Duration> {
    let scaled = index
        .checked_mul(1_000_000_000)
        .context("scheduled offset overflow")?;
    let nanos = scaled
        .checked_div(u128::from(rate))
        .context("offered QPS must be positive")?;
    Ok(Duration::from_nanos(u64::try_from(nanos)?))
}

/// Small deterministic PRNG sufficient for schedule generation. It is local to
/// the benchmark contract so an ambient thread RNG cannot alter a replay.
fn next_random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn poisson_offsets(duration: Duration, rate: u32, seed: u64) -> AnyResult<Vec<Duration>> {
    let expected = scheduled_count(duration, rate)?;
    let maximum = expected
        .checked_mul(2)
        .and_then(|value| value.checked_add(128))
        .context("Poisson schedule limit overflow")?;
    let mut state = seed ^ u64::from(rate).rotate_left(17);
    let mut elapsed_secs = 0.0_f64;
    let duration_secs = duration.as_secs_f64();
    let mut offsets = Vec::new();
    loop {
        if elapsed_secs >= duration_secs {
            break;
        }
        // Map 53 random bits into (0, 1] so ln is finite and zero intervals
        // cannot create duplicate scheduled arrivals.
        let random = next_random(&mut state) >> 11;
        let high = u32::try_from(random >> 21).context("Poisson random high bits overflow")?;
        let low = u32::try_from(random & ((1_u64 << 21) - 1))
            .context("Poisson random low bits overflow")?;
        let unit = (f64::from(high) * 2_097_152.0 + f64::from(low)) / 9_007_199_254_740_992.0;
        let open_unit = unit.clamp(f64::MIN_POSITIVE, 1.0);
        elapsed_secs += -open_unit.ln() / f64::from(rate);
        if elapsed_secs >= duration_secs {
            break;
        }
        if u128::try_from(offsets.len()).context("Poisson schedule length overflow")? >= maximum {
            anyhow::bail!("Poisson schedule exceeded its bounded replay limit");
        }
        offsets.push(Duration::from_secs_f64(elapsed_secs));
    }
    Ok(offsets)
}

fn scheduled_offsets(config: &Config, rate: u32) -> AnyResult<Vec<Duration>> {
    match config.arrival_model {
        ArrivalModel::DeterministicPeriodic => {
            let count = scheduled_count(config.duration, rate)?;
            (0..count)
                .map(|index| scheduled_offset(index, rate))
                .collect()
        }
        ArrivalModel::SeededPoisson => poisson_offsets(config.duration, rate, config.seed),
    }
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
    observed.into_iter().eq(expected.iter().map(|row| {
        (
            row.candidate_id.as_str(),
            row.source_repo_id.as_str(),
            row.repo_relative_path.as_str(),
        )
    }))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Outcome {
    Served { result_count: u64 },
    InvalidResult,
    TypedError { code: SearchPlaneErrorCodeV2 },
    Timeout,
    TransportError { kind: String },
    QueueDeadline,
}

#[derive(Clone, Debug)]
struct Completion {
    outcome: Outcome,
    elapsed_ms: Option<f64>,
    dispatch_lag_ms: f64,
}

#[derive(Clone, Copy, Debug)]
struct OfferedRequest {
    id: u64,
    scheduled: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct LoadPoint {
    pub target_qps: u32,
    pub offered: u64,
    pub served: u64,
    pub typed_errors: u64,
    pub unexpected_typed_errors: u64,
    pub timeouts: u64,
    pub transport_errors: u64,
    pub transport_error_kinds: BTreeMap<String, u64>,
    pub invalid_results: u64,
    pub dropped_queue_full: u64,
    pub dropped_scheduler_late: u64,
    pub dropped_deadline: u64,
    pub offered_qps: f64,
    pub achieved_qps: f64,
    pub drain_secs: f64,
    /// Scheduled arrival to response; includes scheduler lag, queue wait and IPC.
    pub latency: Option<LatencySummary>,
    pub max_dispatch_lag_ms: f64,
    pub last_result_count: Option<u64>,
    pub error_codes: Vec<String>,
    pub saturated: bool,
}

impl LoadPoint {
    fn detail(&self) -> Value {
        json!({
            "target_qps": self.target_qps,
            "offered": self.offered,
            "served": self.served,
            "typed_errors": self.typed_errors,
            "unexpected_typed_errors": self.unexpected_typed_errors,
            "timeouts": self.timeouts,
            "transport_errors": self.transport_errors,
            "transport_error_kinds": self.transport_error_kinds,
            "invalid_results": self.invalid_results,
            "dropped_queue_full": self.dropped_queue_full,
            "dropped_scheduler_late": self.dropped_scheduler_late,
            "dropped_deadline": self.dropped_deadline,
            "offered_qps": self.offered_qps,
            "achieved_qps": self.achieved_qps,
            "drain_secs": self.drain_secs,
            "latency": self.latency,
            "max_dispatch_lag_ms": self.max_dispatch_lag_ms,
            "error_codes": self.error_codes,
            "saturated": self.saturated,
        })
    }
}

fn request(pin: &GenerationPin) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: QUERY.to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(pin.clone()),
        generation_selector: None,
        top_k: TOP_K,
        cursor: None,
    })
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
        IpcError::ZeroRequestId => "zero_request_id".to_string(),
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
    task: OfferedRequest,
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
            SearchPlaneQueryIpcResponse::Error(error) => Outcome::TypedError { code: error.code },
            SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
            | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
            | SearchPlaneQueryIpcResponse::Text(_)
            | SearchPlaneQueryIpcResponse::Symbol(_)
            | SearchPlaneQueryIpcResponse::Semantic(_)
            | SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_)
            | SearchPlaneQueryIpcResponse::Hybrid(_)
            | SearchPlaneQueryIpcResponse::HybridSeed(_)
            | SearchPlaneQueryIpcResponse::History(_)
            | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
            | SearchPlaneQueryIpcResponse::Structural(_)
            | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
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

fn increment_count(counter: &mut u64, label: &str) -> AnyResult<()> {
    *counter = counter
        .checked_add(1)
        .with_context(|| format!("{label} overflow"))?;
    Ok(())
}

fn summarize(
    rate: u32,
    duration: Duration,
    offered: u64,
    dropped_queue_full: u64,
    dropped_scheduler_late: u64,
    completions: Vec<Completion>,
    drain_secs: f64,
) -> AnyResult<LoadPoint> {
    ensure!(offered > 0, "open-loop load point offered no requests");
    ensure!(
        drain_secs.is_finite() && drain_secs > 0.0,
        "measurement window must be finite and positive"
    );
    let accounted = u64::try_from(completions.len())?
        .checked_add(dropped_queue_full)
        .and_then(|count| count.checked_add(dropped_scheduler_late))
        .context("offered request accounting overflow")?;
    ensure!(
        accounted == offered,
        "offered requests were lost before accounting"
    );
    let mut served = 0_u64;
    let mut typed_errors = 0_u64;
    let mut unexpected_typed_errors = 0_u64;
    let mut timeouts = 0_u64;
    let mut transport_errors = 0_u64;
    let mut transport_error_kinds = BTreeMap::<String, u64>::new();
    let mut invalid_results = 0_u64;
    let mut dropped_deadline = 0_u64;
    let mut last_result_count = None;
    let mut codes = Vec::new();
    let mut elapsed = Vec::new();
    let mut max_dispatch_lag_ms = 0.0_f64;
    for completion in completions {
        ensure!(
            completion.dispatch_lag_ms.is_finite() && completion.dispatch_lag_ms >= 0.0,
            "dispatch lag must be finite and nonnegative"
        );
        max_dispatch_lag_ms = max_dispatch_lag_ms.max(completion.dispatch_lag_ms);
        if let Some(ms) = completion.elapsed_ms {
            ensure!(
                ms.is_finite() && ms >= 0.0,
                "completion latency must be finite and nonnegative"
            );
            elapsed.push(ms);
        }
        match completion.outcome {
            Outcome::Served { result_count } => {
                increment_count(&mut served, "served count")?;
                last_result_count = Some(result_count);
            }
            Outcome::TypedError { code } => {
                increment_count(&mut typed_errors, "typed error count")?;
                if code != SearchPlaneErrorCodeV2::ServerOverloaded {
                    increment_count(&mut unexpected_typed_errors, "unexpected typed error count")?;
                }
                codes.push(code.as_wire_str().to_string());
            }
            Outcome::Timeout => increment_count(&mut timeouts, "timeout count")?,
            Outcome::TransportError { kind } => {
                increment_count(&mut transport_errors, "transport error count")?;
                increment_count(
                    transport_error_kinds.entry(kind).or_default(),
                    "transport error kind count",
                )?;
            }
            Outcome::InvalidResult => {
                increment_count(&mut invalid_results, "invalid result count")?;
            }
            Outcome::QueueDeadline => {
                increment_count(&mut dropped_deadline, "queue deadline count")?;
            }
        }
    }
    codes.sort();
    codes.dedup();
    let offered_qps = f64::from(u32::try_from(offered)?) / duration.as_secs_f64();
    let achieved_qps = f64::from(u32::try_from(served)?) / drain_secs.max(duration.as_secs_f64());
    ensure!(
        offered_qps.is_finite() && offered_qps > 0.0 && achieved_qps.is_finite(),
        "open-loop throughput is not finite"
    );
    let saturated = dropped_queue_full > 0
        || dropped_scheduler_late > 0
        || dropped_deadline > 0
        || typed_errors > 0
        || timeouts > 0
        || transport_errors > 0
        || invalid_results > 0
        || achieved_qps / offered_qps < MIN_DELIVERY_RATIO;
    Ok(LoadPoint {
        target_qps: rate,
        offered,
        served,
        typed_errors,
        unexpected_typed_errors,
        timeouts,
        transport_errors,
        transport_error_kinds,
        invalid_results,
        dropped_queue_full,
        dropped_scheduler_late,
        dropped_deadline,
        offered_qps,
        achieved_qps,
        drain_secs,
        latency: LatencySummary::from_samples_ms(&elapsed),
        max_dispatch_lag_ms,
        last_result_count,
        error_codes: codes,
        saturated,
    })
}

fn measure_point(
    socket: &Path,
    pin: &GenerationPin,
    expected_rows: &[ExpectedRow],
    config: &Config,
    rate: u32,
) -> AnyResult<LoadPoint> {
    let offsets = scheduled_offsets(config, rate)?;
    let offered = u64::try_from(offsets.len())?;
    ensure!(
        offered > 0,
        "open-loop schedule offered no requests at {rate} QPS"
    );
    let (sender, receiver) = mpsc::sync_channel::<OfferedRequest>(config.queue_capacity);
    let shared_receiver = Arc::new(Mutex::new(receiver));
    let (results_sender, results_receiver) = mpsc::channel::<Completion>();
    let mut handles = Vec::with_capacity(config.workers);
    for _ in 0..config.workers {
        let shared_receiver = Arc::clone(&shared_receiver);
        let results_sender = results_sender.clone();
        let socket = PathBuf::from(socket);
        let pin = pin.clone();
        let expected_rows = expected_rows.to_vec();
        let timeout = config.request_timeout;
        handles.push(thread::spawn(move || {
            loop {
                let next = match shared_receiver.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => return,
                };
                match next {
                    Ok(task) => {
                        if results_sender
                            .send(dispatch(&socket, &pin, &expected_rows, task, timeout))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        }));
    }
    drop(results_sender);
    let started = Instant::now();
    let mut dropped_queue_full = 0_u64;
    let mut dropped_scheduler_late = 0_u64;
    for (index, offset) in offsets.into_iter().enumerate() {
        let scheduled = started
            .checked_add(offset)
            .context("scheduled arrival instant overflow")?;
        if let Some(wait) = scheduled.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        if Instant::now().saturating_duration_since(scheduled) >= config.request_timeout {
            // The scheduler itself missed this deadline. No request was sent.
            increment_count(&mut dropped_scheduler_late, "scheduler-late drop count")?;
            continue;
        }
        match sender.try_send(OfferedRequest {
            id: u64::try_from(index)?
                .checked_add(1)
                .context("request id overflow")?,
            scheduled,
        }) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => {
                increment_count(&mut dropped_queue_full, "full-queue drop count")?;
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                anyhow::bail!("all load workers disconnected during scheduling");
            }
        }
    }
    drop(sender);
    for handle in handles {
        handle
            .join()
            .map_err(|panic| anyhow::anyhow!("load worker panicked: {panic:?}"))?;
    }
    let drain_secs = started.elapsed().as_secs_f64();
    let completions: Vec<Completion> = results_receiver.into_iter().collect();
    summarize(
        rate,
        config.duration,
        offered,
        dropped_queue_full,
        dropped_scheduler_late,
        completions,
        drain_secs,
    )
}

#[derive(Clone, Debug)]
pub(crate) struct Report {
    pub config: Config,
    pub corpus_digest: String,
    pub model_revision: Option<String>,
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

pub(crate) fn run(config: Config) -> AnyResult<Report> {
    config.validate()?;
    let legacy =
        (config.tier == ScaleTier::Small).then(|| generate_corpus(ScaleTier::Small, config.seed));
    let scoped = if config.tier == ScaleTier::Small {
        None
    } else {
        let files = generate_scoped_corpus(config.tier, config.seed)?;
        let oracle = ScopedOracle::from_source(&files, config.tier)?;
        let _admission = preflight_scoped_corpus(&files)?;
        Some((files, oracle))
    };
    let (source_paths, corpus_digest) = if let Some(corpus) = &legacy {
        (
            source_fixture_paths(corpus)?,
            corpus_digest(DIMENSION, corpus),
        )
    } else if let Some((files, _)) = &scoped {
        let source_paths = files
            .iter()
            .map(|file| format!("{}/{}", file.source_repo_id, file.repo_relative_path))
            .collect::<BTreeSet<_>>();
        ensure!(
            source_paths.len() == files.len(),
            "open-loop scoped source identity repeats"
        );
        let digest = scoped_corpus_digest(DIMENSION, files);
        (source_paths, digest)
    } else {
        anyhow::bail!("open-loop has no prepared source fixture");
    };

    let mut runtime = E2eRuntime::boot()?;
    let model_revision = model_revision_of(runtime.embedder_profile());
    if let Some(corpus) = &legacy {
        let serving_owner = runtime.repo();
        for (path, content) in corpus {
            runtime.ingest_text(serving_owner.as_str(), path, content)?;
        }
    } else if let Some((files, _)) = &scoped {
        let chunks = files
            .iter()
            .map(|file| {
                [E2eTextChunkSpec {
                    content: &file.content,
                    start_line: 1,
                    end_line: 2,
                    source_repo_id: Some(&file.source_repo_id),
                }]
            })
            .collect::<Vec<_>>();
        let batch_files = files
            .iter()
            .zip(&chunks)
            .map(|(file, chunk)| (file.repo_relative_path.as_str(), chunk.as_slice()))
            .collect::<Vec<_>>();
        let _ids = runtime.ingest_text_files_one_batch(&batch_files)?;
        let _wire = runtime.preview_pending_search_corpus_wire_bytes()?;
    }
    let sealed = runtime.seal()?;
    runtime.activate_last_sealed_generation()?;
    let pin = GenerationPin::new(runtime.repo(), runtime.revision(), sealed);
    let primed = runtime.query_text(TextQuerySyntax::Native, QUERY, TOP_K);
    ensure!(
        primed.typed_error.is_none() && !primed.candidates.is_empty(),
        "fixture must serve a nonempty lexical answer before timing"
    );
    if let Some((_, oracle)) = &scoped {
        for repo_index in 0..params_for(config.tier).repo_count {
            let source_repo_id = format!("repo{repo_index}");
            let response = runtime.query_text(
                TextQuerySyntax::Native,
                &repo_query_token(repo_index),
                TOP_K,
            );
            ensure!(
                response.typed_error.is_none(),
                "open-loop source-repository probe returned a typed error: {source_repo_id}"
            );
            oracle.verify_page(Some(&source_repo_id), &response.candidates)?;
        }
    }
    let preflight = runtime.query_once(|_| request(&pin))?;
    let expected_rows = match preflight {
        SearchPlaneQueryIpcResponse::Text(page)
            if page.generation == pin && page.rank_unit == TextRankUnit::Chunk =>
        {
            let observed = page
                .results
                .iter()
                .map(|row| {
                    let path = if config.tier == ScaleTier::Small {
                        row.repo_relative_path.as_str().to_string()
                    } else {
                        format!("{}/{}", row.source_repo_id, row.repo_relative_path)
                    };
                    (row.candidate_id.clone(), path)
                })
                .collect::<Vec<_>>();
            if config.tier == ScaleTier::Small {
                ensure!(
                    page.results
                        .iter()
                        .all(|row| row.source_repo_id == pin.repo_id.as_str()),
                    "open-loop baseline returned a foreign source repository"
                );
            }
            fixture_candidate_ids(
                &source_paths,
                observed
                    .iter()
                    .map(|(id, path)| (id.as_str(), path.as_str())),
            )?;
            page.results
                .iter()
                .map(|row| ExpectedRow {
                    candidate_id: row.candidate_id.clone(),
                    source_repo_id: row.source_repo_id.clone(),
                    repo_relative_path: row.repo_relative_path.clone(),
                })
                .collect()
        }
        SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::Text(_)
        | SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::Semantic(_)
        | SearchPlaneQueryIpcResponse::SemanticWorkBoundedV1(_)
        | SearchPlaneQueryIpcResponse::Hybrid(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::Error(_) => {
            anyhow::bail!("pinned IPC query failed correctness preflight")
        }
    };
    let (socket, _, _) = runtime
        .socket_paths()
        .context("query socket is not running")?;
    let socket = socket.to_path_buf();
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
    Ok(Report {
        config,
        corpus_digest,
        model_revision,
        points,
    })
}

pub(crate) fn artifact(
    report: &Report,
    git_head: GitHeadV1,
    host: HostV1,
) -> AnyResult<BenchArtifactV1> {
    let resources = ResourceUsageV1::observe_self()?;
    let config = &report.config;
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
                    ("query", QUERY.to_string()),
                    ("top_k", TOP_K.to_string()),
                ],
            ),
            model_revision: report.model_revision.clone(),
        },
        host,
        resources,
        phases: PhaseDurationsV1::default(),
        disk_amplification: None,
        rows,
        detail: json!({
            "passed": report.passed(),
            "arrival_model": config.arrival_model.as_str(),
            "latency_origin": "scheduled_arrival",
            "latency_sample_scope": "completed_requests_including_errors_and_timeouts_excluding_drops",
            "duration_ms": config.duration.as_millis(),
            "request_timeout_ms": config.request_timeout.as_millis(),
            "queue_capacity": config.queue_capacity,
            "workers": config.workers,
            "tier": config.tier.as_str(),
            "serving_owner_count": 1,
            "source_repo_count": quanta_index_searchd_harness::scale::params_for(config.tier).repo_count,
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

    #[test]
    fn zero_request_id_keeps_its_protocol_failure_kind() {
        assert_eq!(transport_kind(&IpcError::ZeroRequestId), "zero_request_id");
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
        wrong_path[0].1 = "foreign.rs".to_string();
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
        duplicate_id[1].0 = duplicate_id[0].0.clone();
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
        duplicate_path[1].1 = duplicate_path[0].1.clone();
        assert!(
            fixture_candidate_ids(
                &paths,
                duplicate_path
                    .iter()
                    .map(|(id, path)| (id.as_str(), path.as_str()))
            )
            .is_err()
        );
        let mut missing_needle = corpus.clone();
        missing_needle[0].1 = missing_needle[0].1.replace(QUERY, "absent");
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
        wrong_repo[0].1 = "repo4/src/file_0.rs".to_string();
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
    fn nonfinite_completed_samples_cannot_enter_latency_summary() {
        assert!(
            summarize(
                1,
                Duration::from_secs(1),
                1,
                0,
                0,
                vec![Completion {
                    outcome: Outcome::Served { result_count: 1 },
                    elapsed_ms: Some(f64::NAN),
                    dispatch_lag_ms: 0.0,
                }],
                1.0,
            )
            .is_err()
        );
        assert!(
            summarize(
                1,
                Duration::from_secs(1),
                1,
                0,
                0,
                vec![Completion {
                    outcome: Outcome::Served { result_count: 1 },
                    elapsed_ms: Some(1.0),
                    dispatch_lag_ms: f64::INFINITY,
                }],
                1.0,
            )
            .is_err()
        );
    }

    #[test]
    fn slow_answers_keep_the_original_arrival_in_the_tail() -> AnyResult<()> {
        let point = summarize(
            10,
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
                    outcome: Outcome::Served { result_count: 2 },
                    elapsed_ms: Some(700.0),
                    dispatch_lag_ms: 600.0,
                },
            ],
            1.0,
        )?;
        assert_eq!(
            point.latency.context("latency missing")?.p99_ms.to_bits(),
            700.0_f64.to_bits()
        );
        assert_eq!(point.max_dispatch_lag_ms.to_bits(), 600.0_f64.to_bits());
        assert!(!point.saturated);
        Ok(())
    }

    #[test]
    fn every_offered_request_must_be_accounted() {
        assert!(
            summarize(
                10,
                Duration::from_secs(1),
                2,
                0,
                0,
                vec![Completion {
                    outcome: Outcome::Timeout,
                    elapsed_ms: Some(10.0),
                    dispatch_lag_ms: 0.0
                },],
                1.0
            )
            .is_err()
        );
    }

    #[test]
    fn overload_is_saturation_but_other_typed_errors_are_correctness_failures() -> AnyResult<()> {
        let point = summarize(
            3,
            Duration::from_secs(1),
            3,
            1,
            0,
            vec![
                Completion {
                    outcome: Outcome::TypedError {
                        code: SearchPlaneErrorCodeV2::ServerOverloaded,
                    },
                    elapsed_ms: Some(5.0),
                    dispatch_lag_ms: 0.0,
                },
                Completion {
                    outcome: Outcome::TypedError {
                        code: SearchPlaneErrorCodeV2::CatalogBusy,
                    },
                    elapsed_ms: Some(6.0),
                    dispatch_lag_ms: 0.0,
                },
            ],
            1.0,
        )?;
        assert_eq!(point.typed_errors, 2);
        assert_eq!(point.unexpected_typed_errors, 1);
        assert_eq!(point.dropped_queue_full, 1);
        assert!(point.saturated);
        Ok(())
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
                },
                corpus_digest: String::new(),
                model_revision: None,
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
            },
            corpus_digest: String::new(),
            model_revision: None,
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
        })?;
        assert_eq!(report.points.len(), 1);
        let point = report.points.first().context("missing load point")?;
        assert_eq!(point.offered, 2);
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
}
