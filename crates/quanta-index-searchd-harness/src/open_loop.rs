//! Scheduled arrivals over the real query IPC socket. Arrival timestamps are
//! fixed before dispatch; queueing and scheduler lag remain in end-to-end
//! latency, so a slow daemon cannot silently reduce the offered load.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result as AnyResult, ensure};
use quanta_index_contract::{
    GenerationPin, QueryConstraintSetV1, SearchPlaneErrorCodeV2, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::{ClientIoPolicy, IpcError, send_request};
use quanta_index_searchd_harness::E2eRuntime;
use quanta_index_searchd_harness::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    LatencySummary, PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest,
    corpus_digest, model_revision_of,
};
use quanta_index_searchd_harness::scale::{ScaleTier, generate_corpus};
use serde_json::{Value, json};

pub(crate) const DIMENSION: &str = "open-loop";
const REPO: &str = "repo-open-loop";
const QUERY: &str = "scale_needle_token";
const TOP_K: u32 = 10;
const MAX_TOTAL_REQUESTS: u128 = 100_000;
const MIN_DELIVERY_RATIO: f64 = 0.95;

#[derive(Clone, Debug)]
pub(crate) struct Config {
    pub seed: u64,
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
        }
        ensure!(
            total <= MAX_TOTAL_REQUESTS,
            "at most {MAX_TOTAL_REQUESTS} requests may be offered"
        );
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
    let nanos = index
        .checked_mul(1_000_000_000)
        .context("scheduled offset overflow")?
        / u128::from(rate);
    Ok(Duration::from_nanos(u64::try_from(nanos)?))
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

#[derive(Clone, Debug)]
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
        _ => "policy_or_socket".to_string(),
    }
}

fn dispatch(
    socket: &Path,
    pin: &GenerationPin,
    expected_ids: &[String],
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
                    && page
                        .results
                        .iter()
                        .map(|row| &row.candidate_id)
                        .eq(expected_ids.iter()) =>
            {
                Outcome::Served {
                    result_count: u64::try_from(page.results.len()).unwrap_or(u64::MAX),
                }
            }
            SearchPlaneQueryIpcResponse::Error(error) => Outcome::TypedError { code: error.code },
            _ => Outcome::InvalidResult,
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

fn summarize(
    rate: u32,
    duration: Duration,
    offered: u64,
    dropped_queue_full: u64,
    dropped_scheduler_late: u64,
    mut completions: Vec<Completion>,
    drain_secs: f64,
) -> AnyResult<LoadPoint> {
    ensure!(drain_secs > 0.0, "measurement window must be positive");
    ensure!(
        u64::try_from(completions.len())? + dropped_queue_full + dropped_scheduler_late == offered,
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
    for completion in completions.drain(..) {
        max_dispatch_lag_ms = max_dispatch_lag_ms.max(completion.dispatch_lag_ms);
        if let Some(ms) = completion.elapsed_ms {
            elapsed.push(ms);
        }
        match completion.outcome {
            Outcome::Served { result_count } => {
                served += 1;
                last_result_count = Some(result_count);
            }
            Outcome::TypedError { code } => {
                typed_errors += 1;
                if code != SearchPlaneErrorCodeV2::ServerOverloaded {
                    unexpected_typed_errors += 1;
                }
                codes.push(code.as_wire_str().to_string());
            }
            Outcome::Timeout => timeouts += 1,
            Outcome::TransportError { kind } => {
                transport_errors += 1;
                *transport_error_kinds.entry(kind).or_default() += 1;
            }
            Outcome::InvalidResult => invalid_results += 1,
            Outcome::QueueDeadline => dropped_deadline += 1,
        }
    }
    codes.sort();
    codes.dedup();
    let offered_qps = f64::from(u32::try_from(offered)?) / duration.as_secs_f64();
    let achieved_qps = f64::from(u32::try_from(served)?) / drain_secs.max(duration.as_secs_f64());
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
    expected_ids: &[String],
    config: &Config,
    rate: u32,
) -> AnyResult<LoadPoint> {
    let offered = u64::try_from(scheduled_count(config.duration, rate)?)?;
    let (sender, receiver) = mpsc::sync_channel::<OfferedRequest>(config.queue_capacity);
    let shared_receiver = Arc::new(Mutex::new(receiver));
    let (results_sender, results_receiver) = mpsc::channel::<Completion>();
    let mut handles = Vec::with_capacity(config.workers);
    for _ in 0..config.workers {
        let shared_receiver = Arc::clone(&shared_receiver);
        let results_sender = results_sender.clone();
        let socket = PathBuf::from(socket);
        let pin = pin.clone();
        let expected_ids = expected_ids.to_vec();
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
                            .send(dispatch(&socket, &pin, &expected_ids, task, timeout))
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
    for index in 0..offered {
        let scheduled = started + scheduled_offset(u128::from(index), rate)?;
        if let Some(wait) = scheduled.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
        if Instant::now().saturating_duration_since(scheduled) >= config.request_timeout {
            // The scheduler itself missed this deadline. No request was sent.
            dropped_scheduler_late += 1;
            continue;
        }
        match sender.try_send(OfferedRequest {
            id: index + 1,
            scheduled,
        }) {
            Ok(()) => {}
            Err(mpsc::TrySendError::Full(_)) => dropped_queue_full += 1,
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
        self.points.iter().all(|point| {
            point.invalid_results == 0
                && point.unexpected_typed_errors == 0
                && point.transport_errors == 0
        })
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
    let corpus = generate_corpus(ScaleTier::Small, config.seed);
    let corpus_digest = corpus_digest(DIMENSION, &corpus);
    let mut runtime = E2eRuntime::boot()?;
    let model_revision = model_revision_of(runtime.embedder_profile());
    for (path, content) in &corpus {
        runtime.ingest_text(REPO, path, content)?;
    }
    let sealed = runtime.seal()?;
    runtime.activate_last_sealed_generation()?;
    let pin = GenerationPin::new(runtime.repo(), runtime.revision(), sealed);
    let primed = runtime.query_text(TextQuerySyntax::Native, QUERY, TOP_K);
    ensure!(
        primed.typed_error.is_none() && !primed.candidates.is_empty(),
        "fixture must serve a nonempty lexical answer before timing"
    );
    let preflight = runtime.query_once(|_| request(&pin))?;
    let expected_ids = match preflight {
        SearchPlaneQueryIpcResponse::Text(page)
            if page.generation == pin && !page.results.is_empty() =>
        {
            page.results
                .into_iter()
                .map(|row| row.candidate_id)
                .collect::<Vec<_>>()
        }
        _ => anyhow::bail!("pinned IPC query failed correctness preflight"),
    };
    let (socket, _, _) = runtime
        .socket_paths()
        .context("query socket is not running")?;
    let socket = socket.to_path_buf();
    let mut points = Vec::with_capacity(config.rates_qps.len());
    for rate in &config.rates_qps {
        points.push(measure_point(&socket, &pin, &expected_ids, &config, *rate)?);
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
        .map(|point| BenchRowV1 {
            scenario_id: format!("open_loop.lexical.qps{}", point.target_qps),
            route_family: RouteFamily::Lexical,
            syntax: BenchSyntax::Native,
            result_shape: ResultShape::Candidates,
            latency: point.latency,
            qps: Some(point.achieved_qps),
            error_count: point.typed_errors + point.transport_errors + point.invalid_results,
            timeout_count: point.timeouts,
            result_count: point.last_result_count,
            typed_error_code: point.error_codes.first().cloned(),
            engine_touched: vec!["lexical".to_string()],
            early_stop_reason: point
                .latency
                .is_none()
                .then(|| "all_offered_requests_dropped".to_string()),
        })
        .collect();
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
                    ("tier", ScaleTier::Small.as_str().to_string()),
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
            "arrival_model": "deterministic_periodic",
            "latency_origin": "scheduled_arrival",
            "latency_sample_scope": "completed_requests_including_errors_and_timeouts_excluding_drops",
            "duration_ms": config.duration.as_millis(),
            "request_timeout_ms": config.request_timeout.as_millis(),
            "queue_capacity": config.queue_capacity,
            "workers": config.workers,
            "saturation_onset_qps": report.saturation_onset_qps(),
            "points": report.points.iter().map(LoadPoint::detail).collect::<Vec<_>>(),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_has_fixed_arrivals_independent_of_completion() -> AnyResult<()> {
        assert_eq!(scheduled_count(Duration::from_millis(250), 10)?, 3);
        assert_eq!(scheduled_offset(0, 10)?, Duration::ZERO);
        assert_eq!(scheduled_offset(2, 10)?, Duration::from_millis(200));
        Ok(())
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
        assert_eq!(point.latency.context("latency missing")?.p99_ms, 700.0);
        assert_eq!(point.max_dispatch_lag_ms, 600.0);
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
        assert_eq!(point.transport_error_kinds["io::ConnectionReset"], 1);
        assert!(
            !Report {
                config: Config {
                    seed: 1,
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
    fn bounded_real_runtime_smoke() -> AnyResult<()> {
        let report = run(Config {
            seed: 1,
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
