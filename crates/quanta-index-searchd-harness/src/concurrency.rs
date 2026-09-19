//! Concurrency rail (QI-BB-010 #4): 1 / 8 / 32 clients, a slow one, mixed routes.
//!
//! The rail drives 1 / 8 / 32 concurrent clients over the query socket, a
//! slow client mixed in, and mixed lexical / semantic / hybrid routes with a
//! count worst case — one `BenchArtifactV1` per client count.
//!
//! One daemon serves one sealed, activated generation of the seeded medium
//! scale corpus. For each client count the rail spawns that many client
//! threads; every fast client issues its share of requests round-robin over
//! the mixed route set through its own connection, while — for counts above
//! one — one extra *slow* client issues broad, page-maximum queries for the
//! whole window. Every request is timed on the client; the outcome is
//! classified as served, typed error (the daemon refused it, `OVERLOADED`
//! included) or timeout (the client's deadline passed). A transport failure
//! that is none of those is a rail error: the daemon is expected to answer
//! every request one way or another.
//!
//! What the artifact carries per client count: one row per route with
//! p50/p95/p99, QPS (served requests over the wall-clock window), error and
//! timeout counts, and the same for the slow client; `detail` adds the
//! head-of-line ratio (the fast clients' p50 with the slow client mixed in
//! over their p50 alone, `1.0` meaning no blocking) and the daemon's
//! dispatch-slot policy. Latency on this host is advisory (the host is
//! recorded, not idealised); the blocking signals are that every request was
//! answered and nothing timed out.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result as AnyResult;
use quanta_index_contract::{
    GenerationPin, HybridQueryRequest, QueryConstraintSetV1, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SemanticQueryRequest, TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::{ClientIoPolicy, IpcError, send_request};
use serde_json::{Value, json};

use crate::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    LatencySummary, PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest,
    corpus_digest, model_revision_of, saturating_u64,
};
use crate::harness::E2eRuntime;
use crate::scale::{ScaleTier, generate_corpus};

/// The artifact dimension this rail writes.
pub const DIMENSION: &str = "concurrency";

/// The client counts the finding names.
pub const CLIENT_COUNTS: [u32; 3] = [1, 8, 32];

/// Repo id of the seeded corpus.
const CONCURRENCY_REPO: &str = "repo-concurrency";

/// The token every generated file carries, so every fast query serves.
const FAST_QUERY_TOKEN: &str = "scale_needle_token";

/// The fast clients' page.
const FAST_TOP_K: u32 = 10;

/// The slow client's page: the public maximum, over a query that matches
/// every file, so each of its requests is the count/projection worst case.
const SLOW_TOP_K: u32 = quanta_index_contract::PUBLIC_TOP_K_MAX;

/// A query every generated file matches (the filler vocabulary), broad
/// enough that the slow client's page is the whole corpus.
const SLOW_QUERY: &str = "alpha OR beta OR gamma OR delta OR epsilon OR zeta OR eta OR theta";

/// How long one request may take before the client counts it as a timeout.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The mixed route set every fast client cycles through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MixedRoute {
    Lexical,
    Semantic,
    Hybrid,
    /// The lexical route with `count:yes`, the exact-count worst case.
    LexicalCount,
}

impl MixedRoute {
    pub const ALL: [Self; 4] = [
        Self::Lexical,
        Self::Semantic,
        Self::Hybrid,
        Self::LexicalCount,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
            Self::LexicalCount => "lexical_count",
        }
    }

    const fn route_family(self) -> RouteFamily {
        RouteFamily::Lexical
    }
}

fn text_request(pin: &GenerationPin, query_text: &str, top_k: u32) -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: query_text.to_string(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(pin.clone()),
        generation_selector: None,
        top_k,
        cursor: None,
    }
}

fn fast_request(route: MixedRoute, pin: &GenerationPin) -> SearchPlaneQueryIpcRequest {
    match route {
        MixedRoute::Lexical => {
            SearchPlaneQueryIpcRequest::Text(text_request(pin, FAST_QUERY_TOKEN, FAST_TOP_K))
        }
        MixedRoute::LexicalCount => SearchPlaneQueryIpcRequest::Text(text_request(
            pin,
            &format!("{FAST_QUERY_TOKEN} count:yes"),
            FAST_TOP_K,
        )),
        MixedRoute::Semantic => SearchPlaneQueryIpcRequest::Semantic(SemanticQueryRequest {
            query_text: FAST_QUERY_TOKEN.to_string(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            lexical_scope: None,
            top_k: FAST_TOP_K,
        }),
        MixedRoute::Hybrid => SearchPlaneQueryIpcRequest::Hybrid(HybridQueryRequest {
            text_query: text_request(pin, FAST_QUERY_TOKEN, FAST_TOP_K),
            semantic_query_text: FAST_QUERY_TOKEN.to_string(),
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k: FAST_TOP_K,
        }),
    }
}

fn slow_request(pin: &GenerationPin) -> SearchPlaneQueryIpcRequest {
    SearchPlaneQueryIpcRequest::Text(text_request(pin, SLOW_QUERY, SLOW_TOP_K))
}

/// How one request ended, as the client saw it.
#[derive(Clone, Debug, Eq, PartialEq)]
enum RequestOutcome {
    Served { result_count: u64 },
    TypedError { code: String },
    Timeout,
}

/// One timed request.
#[derive(Clone, Debug)]
struct RequestSample {
    route: Option<MixedRoute>,
    wall_ms: f64,
    outcome: RequestOutcome,
}

fn result_count_of(response: &SearchPlaneQueryIpcResponse) -> Option<u64> {
    let rows = match response {
        SearchPlaneQueryIpcResponse::Text(page) => page.results.len(),
        SearchPlaneQueryIpcResponse::Semantic(page) => page.results.len(),
        SearchPlaneQueryIpcResponse::Hybrid(page) => page.results.len(),
        SearchPlaneQueryIpcResponse::Symbol(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Error(_) => return None,
    };
    Some(saturating_u64(rows))
}

/// Issue one request and classify its outcome; a transport failure that is
/// neither an answer nor a timeout is the caller's error.
fn timed_request(
    socket: &Path,
    request_id: u64,
    payload: SearchPlaneQueryIpcRequest,
    route: Option<MixedRoute>,
) -> AnyResult<RequestSample> {
    let envelope = SearchPlaneQueryIpcRequestEnvelope {
        request_id,
        payload,
    };
    let policy = ClientIoPolicy::try_new(REQUEST_TIMEOUT)?;
    let started = Instant::now();
    let answer = send_request::<_, SearchPlaneQueryIpcResponseEnvelope>(socket, &envelope, policy);
    let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
    let outcome = match answer {
        Ok(response) => {
            if let SearchPlaneQueryIpcResponse::Error(error) = &response.payload {
                RequestOutcome::TypedError {
                    code: error.code.clone(),
                }
            } else {
                RequestOutcome::Served {
                    result_count: result_count_of(&response.payload).ok_or_else(|| {
                        anyhow::anyhow!("concurrency: a served answer of an unexpected kind")
                    })?,
                }
            }
        }
        Err(IpcError::Timeout { .. } | IpcError::ClientIoDeadlineElapsed) => {
            RequestOutcome::Timeout
        }
        Err(other) => {
            return Err(anyhow::anyhow!(
                "concurrency: request {request_id} failed at the transport: {other}"
            ));
        }
    };
    Ok(RequestSample {
        route,
        wall_ms,
        outcome,
    })
}

/// The outcome tallies and latency of one group of requests.
#[derive(Clone, Debug)]
pub struct GroupSummary {
    pub label: String,
    pub requests: u64,
    pub served: u64,
    pub error_count: u64,
    pub timeout_count: u64,
    /// Served requests per second of the measured window.
    pub qps: f64,
    /// Over every answered request (served or typed error).
    pub latency: Option<LatencySummary>,
    /// The typed codes seen, deduplicated.
    pub error_codes: Vec<String>,
    /// The result count of the last served request in the group.
    pub last_result_count: Option<u64>,
}

fn summarize(label: &str, samples: &[RequestSample], window_secs: f64) -> AnyResult<GroupSummary> {
    let served = samples
        .iter()
        .filter(|sample| matches!(sample.outcome, RequestOutcome::Served { .. }))
        .count();
    let timeouts = samples
        .iter()
        .filter(|sample| sample.outcome == RequestOutcome::Timeout)
        .count();
    let errors = samples
        .len()
        .saturating_sub(served)
        .saturating_sub(timeouts);
    let answered: Vec<f64> = samples
        .iter()
        .filter(|sample| sample.outcome != RequestOutcome::Timeout)
        .map(|sample| sample.wall_ms)
        .collect();
    let mut error_codes: Vec<String> = samples
        .iter()
        .filter_map(|sample| match &sample.outcome {
            RequestOutcome::TypedError { code } => Some(code.clone()),
            RequestOutcome::Served { .. } | RequestOutcome::Timeout => None,
        })
        .collect();
    error_codes.sort();
    error_codes.dedup();
    if window_secs <= 0.0 {
        return Err(anyhow::anyhow!("concurrency: a zero-length window"));
    }
    Ok(GroupSummary {
        label: label.to_string(),
        requests: u64::try_from(samples.len())?,
        served: u64::try_from(served)?,
        error_count: u64::try_from(errors)?,
        timeout_count: u64::try_from(timeouts)?,
        qps: f64::from(u32::try_from(served)?) / window_secs,
        latency: LatencySummary::from_samples_ms(&answered),
        error_codes,
        last_result_count: samples
            .iter()
            .rev()
            .find_map(|sample| match sample.outcome {
                RequestOutcome::Served { result_count } => Some(result_count),
                RequestOutcome::TypedError { .. } | RequestOutcome::Timeout => None,
            }),
    })
}

/// One measured client count.
#[derive(Clone, Debug)]
pub struct ConcurrencyMeasurement {
    pub clients: u32,
    pub requests_per_client: u32,
    /// Wall-clock length of the window, first request to last answer.
    pub window_secs: f64,
    /// One summary per mixed route over every fast client.
    pub routes: Vec<GroupSummary>,
    /// Every fast request together.
    pub fast: GroupSummary,
    /// The slow client, when one was mixed in (client counts above one).
    pub slow: Option<GroupSummary>,
}

/// Run one client count against the served generation.
fn measure_clients(
    socket: &Path,
    pin: &GenerationPin,
    clients: u32,
    requests_per_client: u32,
) -> AnyResult<ConcurrencyMeasurement> {
    let stop = Arc::new(AtomicBool::new(false));
    let window_started = Instant::now();
    let mut fast_handles = Vec::with_capacity(usize::try_from(clients)?);
    for client in 0..clients {
        let socket = socket.to_path_buf();
        let pin = pin.clone();
        fast_handles.push(thread::spawn(move || -> AnyResult<Vec<RequestSample>> {
            let mut samples = Vec::with_capacity(usize::try_from(requests_per_client)?);
            for index in 0..requests_per_client {
                let route = MixedRoute::ALL
                    .get(
                        usize::try_from(index)?
                            .checked_rem(MixedRoute::ALL.len())
                            .ok_or_else(|| {
                                anyhow::anyhow!("concurrency: the route set is empty")
                            })?,
                    )
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("concurrency: the route set is empty"))?;
                let request_id = u64::from(client)
                    .saturating_mul(1_000_000)
                    .saturating_add(u64::from(index));
                samples.push(timed_request(
                    &socket,
                    request_id,
                    fast_request(route, &pin),
                    Some(route),
                )?);
            }
            Ok(samples)
        }));
    }
    let slow_handle = (clients > 1).then(|| {
        let socket = socket.to_path_buf();
        let pin = pin.clone();
        let stop = Arc::clone(&stop);
        thread::spawn(move || -> AnyResult<Vec<RequestSample>> {
            let mut samples = Vec::new();
            let mut index = 0_u64;
            while !stop.load(Ordering::Acquire) {
                samples.push(timed_request(
                    &socket,
                    u64::MAX.saturating_sub(index),
                    slow_request(&pin),
                    None,
                )?);
                index = index.saturating_add(1);
            }
            Ok(samples)
        })
    });
    let mut fast_samples = Vec::new();
    for handle in fast_handles {
        let samples = handle
            .join()
            .map_err(|panic| anyhow::anyhow!("concurrency: a fast client panicked: {panic:?}"))??;
        fast_samples.extend(samples);
    }
    stop.store(true, Ordering::Release);
    let slow_samples = match slow_handle {
        Some(handle) => Some(handle.join().map_err(|panic| {
            anyhow::anyhow!("concurrency: the slow client panicked: {panic:?}")
        })??),
        None => None,
    };
    let window_secs = window_started.elapsed().as_secs_f64();
    let routes = MixedRoute::ALL
        .iter()
        .map(|route| {
            let samples: Vec<RequestSample> = fast_samples
                .iter()
                .filter(|sample| sample.route == Some(*route))
                .cloned()
                .collect();
            summarize(route.as_str(), &samples, window_secs)
        })
        .collect::<AnyResult<Vec<_>>>()?;
    let fast = summarize("fast", &fast_samples, window_secs)?;
    let slow = slow_samples
        .as_deref()
        .map(|samples| summarize("slow", samples, window_secs))
        .transpose()?;
    Ok(ConcurrencyMeasurement {
        clients,
        requests_per_client,
        window_secs,
        routes,
        fast,
        slow,
    })
}

/// The whole rail: the seeded generation, every client count, and the
/// facts the artifact's provenance names.
#[derive(Clone, Debug)]
pub struct ConcurrencyReport {
    pub tier: ScaleTier,
    pub seed: u64,
    pub requests_per_client: u32,
    pub measurements: Vec<ConcurrencyMeasurement>,
    pub model_revision: Option<String>,
    /// `true` when every request of every client count was answered (served
    /// or typed) and none timed out: the blocking signal on this host.
    pub passed: bool,
}

/// Seed the medium corpus, serve it, and measure every client count.
pub fn run_concurrency_report(seed: u64, requests_per_client: u32) -> AnyResult<ConcurrencyReport> {
    if requests_per_client == 0 {
        return Err(anyhow::anyhow!(
            "concurrency: requests_per_client must be at least 1"
        ));
    }
    let tier = ScaleTier::Medium;
    let corpus = generate_corpus(tier, seed);
    let mut rt = E2eRuntime::boot()?;
    let model_revision = model_revision_of(rt.embedder_profile());
    for (path, content) in &corpus {
        rt.ingest_text(CONCURRENCY_REPO, path, content)?;
    }
    let sealed = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    let pin = GenerationPin::new(rt.repo(), rt.revision(), sealed);
    // One served request of every route before timing, so readiness and
    // the cold open are not inside any window and a route the fixture
    // cannot serve is a rail error, not a row of typed errors. The first
    // probe waits for the activated generation to serve.
    let primed = rt.query_text(TextQuerySyntax::Native, FAST_QUERY_TOKEN, FAST_TOP_K);
    if let Some(error) = primed.typed_error {
        return Err(anyhow::anyhow!(
            "concurrency: the fixture does not serve: {} {}",
            error.code,
            error.message
        ));
    }
    let probes = MixedRoute::ALL
        .iter()
        .map(|route| (route.as_str(), fast_request(*route, &pin)))
        .chain(std::iter::once(("slow", slow_request(&pin))));
    for (label, request) in probes {
        if let SearchPlaneQueryIpcResponse::Error(error) = rt.query_once(|_| request)? {
            return Err(anyhow::anyhow!(
                "concurrency: {label} refused before timing: {} {}",
                error.code,
                error.message
            ));
        }
    }
    let Some((socket, _, _)) = rt.socket_paths() else {
        return Err(anyhow::anyhow!("concurrency: the daemon is not running"));
    };
    let socket = socket.to_path_buf();
    let mut measurements = Vec::with_capacity(CLIENT_COUNTS.len());
    for clients in CLIENT_COUNTS {
        measurements.push(measure_clients(
            &socket,
            &pin,
            clients,
            requests_per_client,
        )?);
    }
    let passed = measurements.iter().all(|measurement| {
        measurement.fast.timeout_count == 0
            && measurement
                .slow
                .as_ref()
                .is_none_or(|slow| slow.timeout_count == 0)
    });
    Ok(ConcurrencyReport {
        tier,
        seed,
        requests_per_client,
        measurements,
        model_revision,
        passed,
    })
}

// ---------------------------------------------------------------------------
// Artifact emission.
// ---------------------------------------------------------------------------

fn group_json(group: &GroupSummary) -> Value {
    json!({
        "label": group.label,
        "requests": group.requests,
        "served": group.served,
        "error_count": group.error_count,
        "timeout_count": group.timeout_count,
        "qps": group.qps,
        "latency": group.latency,
        "error_codes": group.error_codes,
        "last_result_count": group.last_result_count,
    })
}

/// The fast clients' p50 with the slow client mixed in over their p50
/// alone; `None` until both are measured.
fn head_of_line_ratio_p50(report: &ConcurrencyReport) -> Option<f64> {
    let alone = report
        .measurements
        .iter()
        .find(|measurement| measurement.slow.is_none())?
        .fast
        .latency?
        .p50_ms;
    let mixed = report
        .measurements
        .iter()
        .find(|measurement| measurement.slow.is_some())?
        .fast
        .latency?
        .p50_ms;
    if alone <= 0.0 {
        return None;
    }
    Some(mixed / alone)
}

fn measurement_json(measurement: &ConcurrencyMeasurement) -> Value {
    json!({
        "clients": measurement.clients,
        "requests_per_client": measurement.requests_per_client,
        "window_secs": measurement.window_secs,
        "routes": measurement.routes.iter().map(group_json).collect::<Vec<_>>(),
        "fast": group_json(&measurement.fast),
        "slow": measurement.slow.as_ref().map(group_json),
    })
}

/// The rail's detail: every client count, the head-of-line ratio and the
/// admission policy the daemon dispatched under.
#[must_use]
pub fn detail_json(report: &ConcurrencyReport) -> Value {
    json!({
        "passed": report.passed,
        "blocking_signal": "every request answered (served or typed) and none timed out; latency is advisory on this host",
        "corpus_tier": report.tier.as_str(),
        "seed": report.seed,
        "client_counts": CLIENT_COUNTS,
        "slow_client": {
            "query": SLOW_QUERY,
            "top_k": SLOW_TOP_K,
            "mixed_in_for_client_counts_above": 1,
        },
        "mixed_routes": MixedRoute::ALL.iter().map(|route| route.as_str()).collect::<Vec<_>>(),
        "head_of_line_ratio_p50": head_of_line_ratio_p50(report),
        "dispatch_policy": "ServerAdmissionPolicy::DEFAULT (4 dispatch slots, 2s queue wait)",
        "measurements": report.measurements.iter().map(measurement_json).collect::<Vec<_>>(),
    })
}

fn row(
    measurement: &ConcurrencyMeasurement,
    group: &GroupSummary,
    route: Option<MixedRoute>,
) -> BenchRowV1 {
    BenchRowV1 {
        scenario_id: format!("concurrency.c{}.{}", measurement.clients, group.label),
        route_family: route.map_or(RouteFamily::Lexical, MixedRoute::route_family),
        syntax: BenchSyntax::Native,
        result_shape: if group.served > 0 {
            ResultShape::Candidates
        } else if group.error_count > 0 {
            ResultShape::TypedError
        } else {
            ResultShape::Empty
        },
        latency: group.latency,
        qps: Some(group.qps),
        error_count: group.error_count,
        timeout_count: group.timeout_count,
        result_count: group.last_result_count,
        typed_error_code: group.error_codes.first().cloned(),
        engine_touched: Vec::new(),
        early_stop_reason: None,
    }
}

/// One artifact per client count, so a gate compares like with like: the
/// rows are the mixed routes' summaries, the fast aggregate and the slow
/// client's.
pub fn artifacts(
    report: &ConcurrencyReport,
    git_head: &GitHeadV1,
    host: &HostV1,
) -> AnyResult<Vec<(u32, BenchArtifactV1)>> {
    let corpus_digest = corpus_digest(DIMENSION, &generate_corpus(report.tier, report.seed));
    let resources = ResourceUsageV1::observe_self()?;
    report
        .measurements
        .iter()
        .map(|measurement| {
            let mut rows: Vec<BenchRowV1> = measurement
                .routes
                .iter()
                .zip(MixedRoute::ALL)
                .map(|(group, route)| row(measurement, group, Some(route)))
                .collect();
            rows.push(row(measurement, &measurement.fast, None));
            if let Some(slow) = &measurement.slow {
                rows.push(row(measurement, slow, None));
            }
            Ok((
                measurement.clients,
                BenchArtifactV1 {
                    dimension: DIMENSION.to_string(),
                    mode: BenchMode::Warm,
                    concurrency: measurement
                        .clients
                        .saturating_add(u32::from(measurement.slow.is_some())),
                    provenance: BenchProvenanceV1 {
                        git_head: git_head.clone(),
                        corpus_digest: corpus_digest.clone(),
                        config_digest: config_digest(
                            DIMENSION,
                            &[
                                ("tier", report.tier.as_str().to_string()),
                                ("seed", report.seed.to_string()),
                                ("clients", measurement.clients.to_string()),
                                (
                                    "requests_per_client",
                                    report.requests_per_client.to_string(),
                                ),
                                ("fast_top_k", FAST_TOP_K.to_string()),
                                ("slow_top_k", SLOW_TOP_K.to_string()),
                                (
                                    "request_timeout_secs",
                                    REQUEST_TIMEOUT.as_secs().to_string(),
                                ),
                            ],
                        ),
                        model_revision: report.model_revision.clone(),
                    },
                    host: host.clone(),
                    resources,
                    phases: PhaseDurationsV1::default(),
                    disk_amplification: None,
                    rows,
                    detail: detail_json(report),
                },
            ))
        })
        .collect()
}

/// Write `summary-c<clients>.json` per client count under `dir`.
pub fn write_artifacts(
    report: &ConcurrencyReport,
    dir: &Path,
    git_head: &GitHeadV1,
    host: &HostV1,
) -> AnyResult<()> {
    for (clients, artifact) in artifacts(report, git_head, host)? {
        artifact.write_to(&dir.join(format!("summary-c{clients}.json")))?;
    }
    Ok(())
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests read the serialized artifact by JSON path; a missing path fails the test"
)]
mod tests {
    //! The pure parts: outcome classification, tallies, rows and the
    //! head-of-line ratio. The measured rail runs under the daemon lane.
    use super::*;

    fn sample(route: Option<MixedRoute>, wall_ms: f64, outcome: RequestOutcome) -> RequestSample {
        RequestSample {
            route,
            wall_ms,
            outcome,
        }
    }

    #[test]
    fn tallies_split_served_typed_and_timeouts_and_qps_counts_served_only() {
        let samples = vec![
            sample(
                Some(MixedRoute::Lexical),
                1.0,
                RequestOutcome::Served { result_count: 3 },
            ),
            sample(
                Some(MixedRoute::Lexical),
                2.0,
                RequestOutcome::TypedError {
                    code: "OVERLOADED".to_string(),
                },
            ),
            sample(Some(MixedRoute::Lexical), 30_000.0, RequestOutcome::Timeout),
            sample(
                Some(MixedRoute::Lexical),
                3.0,
                RequestOutcome::Served { result_count: 5 },
            ),
        ];
        let summary = summarize("fast", &samples, 2.0).expect("a window");
        assert_eq!(summary.requests, 4);
        assert_eq!(summary.served, 2);
        assert_eq!(summary.error_count, 1);
        assert_eq!(summary.timeout_count, 1);
        assert!((summary.qps - 1.0).abs() < f64::EPSILON, "2 served over 2s");
        assert_eq!(
            summary.latency.map(|l| l.samples),
            Some(3),
            "timeouts are not latencies"
        );
        assert_eq!(summary.error_codes, vec!["OVERLOADED".to_string()]);
        assert_eq!(summary.last_result_count, Some(5));
        assert!(
            summarize("fast", &samples, 0.0).is_err(),
            "a zero window is refused"
        );
    }

    fn measurement(clients: u32, fast_p50: f64, slow: bool) -> ConcurrencyMeasurement {
        let fast = GroupSummary {
            label: "fast".to_string(),
            requests: 4,
            served: 4,
            error_count: 0,
            timeout_count: 0,
            qps: 2.0,
            latency: LatencySummary::from_samples_ms(&[fast_p50]),
            error_codes: Vec::new(),
            last_result_count: Some(10),
        };
        ConcurrencyMeasurement {
            clients,
            requests_per_client: 4,
            window_secs: 2.0,
            routes: MixedRoute::ALL
                .iter()
                .map(|route| GroupSummary {
                    label: route.as_str().to_string(),
                    ..fast.clone()
                })
                .collect(),
            fast,
            slow: slow.then(|| GroupSummary {
                label: "slow".to_string(),
                requests: 2,
                served: 2,
                error_count: 0,
                timeout_count: 0,
                qps: 1.0,
                latency: LatencySummary::from_samples_ms(&[500.0]),
                error_codes: Vec::new(),
                last_result_count: Some(256),
            }),
        }
    }

    fn report() -> ConcurrencyReport {
        ConcurrencyReport {
            tier: ScaleTier::Medium,
            seed: 7,
            requests_per_client: 4,
            measurements: vec![
                measurement(1, 2.0, false),
                measurement(8, 3.0, true),
                measurement(32, 6.0, true),
            ],
            model_revision: Some("model@rev:d16".to_string()),
            passed: true,
        }
    }

    #[test]
    fn the_head_of_line_ratio_compares_mixed_against_alone() {
        let ratio = head_of_line_ratio_p50(&report()).expect("both measured");
        assert!((ratio - 1.5).abs() < 1e-9, "3.0 mixed over 2.0 alone");
    }

    #[test]
    fn one_artifact_per_client_count_with_route_fast_and_slow_rows() {
        let head = GitHeadV1::parse("0123456789abcdef0123456789abcdef01234567").expect("a head");
        let host = HostV1 {
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            cpu_count: 4,
            mem_bytes: 1 << 30,
            hostname_hash: "sha256:host".to_string(),
        };
        let artifacts = artifacts(&report(), &head, &host).expect("observable");
        let counts: Vec<u32> = artifacts.iter().map(|(clients, _)| *clients).collect();
        assert_eq!(counts, CLIENT_COUNTS.to_vec());
        let (_, alone) = artifacts.first().expect("the single-client artifact");
        assert_eq!(alone.concurrency, 1);
        assert_eq!(
            alone.rows.len(),
            MixedRoute::ALL.len() + 1,
            "routes plus the fast aggregate"
        );
        let (_, mixed) = artifacts.get(1).expect("the eight-client artifact");
        assert_eq!(mixed.concurrency, 9, "eight fast clients plus the slow one");
        assert_eq!(mixed.rows.len(), MixedRoute::ALL.len() + 2);
        let value = mixed.to_json().expect("serializes");
        assert_eq!(value["rows"][0]["scenario_id"], "concurrency.c8.lexical");
        assert_eq!(value["rows"][0]["qps"], 2.0);
        assert_eq!(value["rows"][5]["scenario_id"], "concurrency.c8.slow");
        assert_eq!(value["detail"]["head_of_line_ratio_p50"], 1.5);
        assert_ne!(
            alone.provenance.config_digest, mixed.provenance.config_digest,
            "the client count is part of the configuration"
        );
        assert_eq!(
            alone.provenance.corpus_digest,
            mixed.provenance.corpus_digest
        );
    }
}
