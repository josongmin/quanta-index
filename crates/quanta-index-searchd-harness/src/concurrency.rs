//! Concurrency rail (QI-BB-010 #4): 1 / 8 / 32 clients, a slow one, mixed routes.
//!
//! The rail drives 1 / 8 / 32 concurrent clients over the query socket, a
//! slow client mixed in, and mixed lexical / semantic / hybrid / symbol routes with a
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

use anyhow::{Context, Result as AnyResult};
use quanta_index_contract::{
    GenerationPin, HybridQueryRequest, QueryConstraintSetV1, SearchPlaneErrorCodeV2,
    SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SemanticQueryRequest, SymbolQueryRequest,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::{ClientIoPolicy, IpcError, send_request};
use serde_json::{Value, json};

use crate::artifact::{
    BenchArtifactV1, BenchMode, BenchProvenanceV1, BenchRowV1, BenchSyntax, GitHeadV1, HostV1,
    LatencySummary, PhaseDurationsV1, ResourceUsageV1, ResultShape, RouteFamily, config_digest,
    corpus_digest, model_revision_of, saturating_u64,
};
use crate::harness::{E2eRuntime, E2eTextChunkSpec};
use crate::scale::{ScaleTier, generate_corpus};

/// The artifact dimension this rail writes.
pub const DIMENSION: &str = "concurrency";

/// The client counts the finding names.
pub const CLIENT_COUNTS: [u32; 3] = [1, 8, 32];

/// Minimum answered latency samples in every authoritative artifact row.
pub const MINIMUM_ROW_SAMPLES: u32 = 16;

/// Five mixed routes must each reach the row floor at one fast client.
pub const DEFAULT_REQUESTS_PER_CLIENT: u32 = 80;

/// Bootstrap and readiness IPC have a separate deadline in debug builds.
const FIXTURE_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Hard resource bound, including additional fast requests during slow sampling.
const MAX_SAMPLES_PER_WORKER: u32 = 100_000;

/// New requests stop at this shared deadline; an in-flight request can take
/// at most `REQUEST_TIMEOUT` longer. Exhaustion is an error, never a partial rail.
const MEASUREMENT_TIMEOUT: Duration = Duration::from_secs(600);

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
    Symbol,
    /// The lexical route with `count:all`, the exact-count worst case.
    LexicalCount,
}

impl MixedRoute {
    pub const ALL: [Self; 5] = [
        Self::Lexical,
        Self::Semantic,
        Self::Hybrid,
        Self::Symbol,
        Self::LexicalCount,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
            Self::Symbol => "symbol",
            Self::LexicalCount => "lexical_count",
        }
    }

    const fn route_family(self) -> RouteFamily {
        match self {
            Self::Lexical | Self::LexicalCount => RouteFamily::Lexical,
            Self::Semantic => RouteFamily::Semantic,
            Self::Hybrid => RouteFamily::Hybrid,
            Self::Symbol => RouteFamily::Symbol,
        }
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
            &format!("{FAST_QUERY_TOKEN} count:all"),
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
        MixedRoute::Symbol => SearchPlaneQueryIpcRequest::Symbol(SymbolQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: FAST_QUERY_TOKEN.to_string(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: Some(pin.clone()),
            generation_selector: None,
            top_k: FAST_TOP_K,
            cursor: None,
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
    TypedError { code: SearchPlaneErrorCodeV2 },
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
        SearchPlaneQueryIpcResponse::Symbol(page) => page.results.len(),
        SearchPlaneQueryIpcResponse::Semantic(page) => page.results.len(),
        SearchPlaneQueryIpcResponse::Hybrid(page) => page.results.len(),
        SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
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

fn classify_response(
    request_id: u64,
    route: Option<MixedRoute>,
    expected_generation: &GenerationPin,
    response: &SearchPlaneQueryIpcResponseEnvelope,
) -> AnyResult<RequestOutcome> {
    if response.request_id != request_id {
        return Err(anyhow::anyhow!(
            "concurrency: response request_id {} differs from request {request_id}",
            response.request_id
        ));
    }
    if let SearchPlaneQueryIpcResponse::Error(error) = &response.payload {
        return Ok(RequestOutcome::TypedError { code: error.code });
    }
    let matches_route = matches!(
        (route, &response.payload),
        (
            None | Some(MixedRoute::Lexical | MixedRoute::LexicalCount),
            SearchPlaneQueryIpcResponse::Text(_)
        ) | (
            Some(MixedRoute::Symbol),
            SearchPlaneQueryIpcResponse::Symbol(_)
        ) | (
            Some(MixedRoute::Semantic),
            SearchPlaneQueryIpcResponse::Semantic(_)
        ) | (
            Some(MixedRoute::Hybrid),
            SearchPlaneQueryIpcResponse::Hybrid(_)
        )
    );
    if !matches_route {
        return Err(anyhow::anyhow!(
            "concurrency: response kind differs from requested route {route:?}"
        ));
    }
    let generation = match &response.payload {
        SearchPlaneQueryIpcResponse::Text(page) => &page.generation,
        SearchPlaneQueryIpcResponse::Symbol(page) => &page.generation,
        SearchPlaneQueryIpcResponse::Semantic(page) => &page.generation,
        SearchPlaneQueryIpcResponse::Hybrid(page) => &page.generation,
        SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(_)
        | SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(_)
        | SearchPlaneQueryIpcResponse::HybridSeed(_)
        | SearchPlaneQueryIpcResponse::History(_)
        | SearchPlaneQueryIpcResponse::Structural(_)
        | SearchPlaneQueryIpcResponse::RepoMapQuery(_)
        | SearchPlaneQueryIpcResponse::Explain(_)
        | SearchPlaneQueryIpcResponse::ClusterMembershipRead(_)
        | SearchPlaneQueryIpcResponse::RuntimeMetadata(_)
        | SearchPlaneQueryIpcResponse::Error(_) => {
            return Err(anyhow::anyhow!(
                "concurrency: unexpected response without query generation"
            ));
        }
    };
    if generation != expected_generation {
        return Err(anyhow::anyhow!(
            "concurrency: response generation differs from pinned fixture generation"
        ));
    }
    Ok(RequestOutcome::Served {
        result_count: result_count_of(&response.payload)
            .ok_or_else(|| anyhow::anyhow!("concurrency: a served answer of an unexpected kind"))?,
    })
}

/// Issue one request and classify its outcome; a transport failure that is
/// neither an answer nor a timeout is the caller's error.
fn timed_request(
    socket: &Path,
    expected_generation: &GenerationPin,
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
        Ok(response) => classify_response(request_id, route, expected_generation, &response)
            .with_context(|| format!("concurrency request: ID {request_id}, route {route:?}"))?,
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
    pub error_codes: Vec<SearchPlaneErrorCodeV2>,
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
    let mut error_codes: Vec<SearchPlaneErrorCodeV2> = samples
        .iter()
        .filter_map(|sample| match &sample.outcome {
            RequestOutcome::TypedError { code } => Some(*code),
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

fn validate_requests_per_client(requests: u32) -> AnyResult<()> {
    let routes = u32::try_from(MixedRoute::ALL.len())?;
    let minimum = MINIMUM_ROW_SAMPLES
        .checked_mul(routes)
        .ok_or_else(|| anyhow::anyhow!("concurrency: request floor overflow"))?;
    if requests > MAX_SAMPLES_PER_WORKER {
        return Err(anyhow::anyhow!(
            "concurrency: requests_per_client exceeds bounded sample budget {MAX_SAMPLES_PER_WORKER}"
        ));
    }
    if requests < minimum {
        return Err(anyhow::anyhow!(
            "concurrency: requests_per_client must be at least {minimum} ({MINIMUM_ROW_SAMPLES} samples per mixed route)"
        ));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct SamplingBudget {
    started: Instant,
    timeout: Duration,
    max_samples: u32,
}

impl SamplingBudget {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            timeout: MEASUREMENT_TIMEOUT,
            max_samples: MAX_SAMPLES_PER_WORKER,
        }
    }

    fn check(self, index: u64) -> AnyResult<()> {
        if index >= u64::from(self.max_samples) {
            return Err(anyhow::anyhow!(
                "concurrency: sampling exceeded bounded sample budget {}",
                self.max_samples
            ));
        }
        if self.started.elapsed() >= self.timeout {
            return Err(anyhow::anyhow!(
                "concurrency: sampling exceeded measurement deadline"
            ));
        }
        Ok(())
    }
}

/// Release fast workers if the slow worker fails or unwinds before its floor.
/// The error is still propagated when the worker is joined.
struct ReleaseFastWorkers<'a>(&'a AtomicBool);

impl Drop for ReleaseFastWorkers<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn collect_slow_samples(
    stop: &AtomicBool,
    slow_ready: &AtomicBool,
    budget: SamplingBudget,
    mut request: impl FnMut(u64) -> AnyResult<RequestSample>,
) -> AnyResult<Vec<RequestSample>> {
    let _release = ReleaseFastWorkers(slow_ready);
    let mut samples = Vec::new();
    let mut index = 0_u64;
    while !stop.load(Ordering::Acquire) || index < u64::from(MINIMUM_ROW_SAMPLES) {
        budget.check(index)?;
        samples.push(request(index)?);
        index = index
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("concurrency: slow request index overflow"))?;
        if index >= u64::from(MINIMUM_ROW_SAMPLES) {
            slow_ready.store(true, Ordering::Release);
        }
    }
    Ok(samples)
}

fn collect_fast_samples(
    requests_per_client: u32,
    slow_ready: &AtomicBool,
    budget: SamplingBudget,
    mut request: impl FnMut(u64, MixedRoute) -> AnyResult<RequestSample>,
) -> AnyResult<Vec<RequestSample>> {
    let mut samples = Vec::with_capacity(usize::try_from(requests_per_client)?);
    let mut index = 0_u64;
    while index < u64::from(requests_per_client) || !slow_ready.load(Ordering::Acquire) {
        budget.check(index)?;
        let route = MixedRoute::ALL
            .get(
                usize::try_from(index)?
                    .checked_rem(MixedRoute::ALL.len())
                    .ok_or_else(|| anyhow::anyhow!("concurrency: the route set is empty"))?,
            )
            .copied()
            .ok_or_else(|| anyhow::anyhow!("concurrency: the route set is empty"))?;
        samples.push(request(index, route)?);
        index = index
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("concurrency: fast request index overflow"))?;
    }
    Ok(samples)
}

/// Allocate positive, disjoint fast-worker IDs under the sample ceiling.
fn fast_request_id(client: u32, index: u64) -> AnyResult<u64> {
    u64::from(client)
        .checked_mul(1_000_000)
        .and_then(|base| base.checked_add(index))
        .and_then(|id| id.checked_add(1))
        .ok_or_else(|| anyhow::anyhow!("concurrency: fast request identity overflow"))
}

/// Run one client count against the served generation.
///
/// Fast workers remain active until the slow worker reaches the sample floor.
/// The extra slow samples remain part of the contention window.
fn measure_clients(
    socket: &Path,
    pin: &GenerationPin,
    clients: u32,
    requests_per_client: u32,
) -> AnyResult<ConcurrencyMeasurement> {
    let stop = Arc::new(AtomicBool::new(false));
    let slow_ready = Arc::new(AtomicBool::new(clients == 1));
    let window_started = Instant::now();
    let budget = SamplingBudget::new();
    let mut fast_handles = Vec::with_capacity(usize::try_from(clients)?);
    for client in 0..clients {
        let socket = socket.to_path_buf();
        let pin = pin.clone();
        let slow_ready = Arc::clone(&slow_ready);
        fast_handles.push(thread::spawn(move || -> AnyResult<Vec<RequestSample>> {
            collect_fast_samples(requests_per_client, &slow_ready, budget, |index, route| {
                let request_id = fast_request_id(client, index)?;
                timed_request(
                    &socket,
                    &pin,
                    request_id,
                    fast_request(route, &pin),
                    Some(route),
                )
            })
        }));
    }
    let slow_handle = (clients > 1).then(|| {
        let socket = socket.to_path_buf();
        let pin = pin.clone();
        let stop = Arc::clone(&stop);
        let slow_ready = Arc::clone(&slow_ready);
        thread::spawn(move || -> AnyResult<Vec<RequestSample>> {
            collect_slow_samples(&stop, &slow_ready, budget, |index| {
                timed_request(
                    &socket,
                    &pin,
                    u64::MAX.saturating_sub(index),
                    slow_request(&pin),
                    None,
                )
            })
        })
    });
    let mut fast_samples = Vec::new();
    let mut fast_error = None;
    for handle in fast_handles {
        let result = handle
            .join()
            .map_err(|panic| anyhow::anyhow!("concurrency: a fast client panicked: {panic:?}"))
            .and_then(std::convert::identity);
        match result {
            Ok(samples) => fast_samples.extend(samples),
            Err(error) => {
                if fast_error.is_none() {
                    fast_error = Some(error);
                }
            }
        }
    }
    stop.store(true, Ordering::Release);
    let slow_samples = match slow_handle {
        Some(handle) => Some(handle.join().map_err(|panic| {
            anyhow::anyhow!("concurrency: the slow client panicked: {panic:?}")
        })??),
        None => None,
    };
    if let Some(error) = fast_error {
        return Err(error);
    }
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

/// Keep each bulk record identical to the public single-file ingest fixture.
fn corpus_ingest_chunks(corpus: &[(String, String)]) -> Vec<[E2eTextChunkSpec<'_>; 1]> {
    corpus
        .iter()
        .map(|(_, content)| {
            [E2eTextChunkSpec {
                content,
                start_line: 1,
                end_line: 2,
                source_repo_id: None,
            }]
        })
        .collect()
}

/// Seed the medium corpus, serve it, and measure every client count.
pub fn run_concurrency_report(seed: u64, requests_per_client: u32) -> AnyResult<ConcurrencyReport> {
    validate_requests_per_client(requests_per_client)
        .context("concurrency configuration: request budget")?;
    let tier = ScaleTier::Medium;
    let corpus = generate_corpus(tier, seed);
    let mut rt = E2eRuntime::boot_with_client_request_timeout(FIXTURE_REQUEST_TIMEOUT)
        .context("concurrency bootstrap: runtime boot")?;
    let model_revision = model_revision_of(rt.embedder_profile());
    // Preserve the one-chunk-per-file fixture while publishing one ingest
    // wave, avoiding a semantic dataset append and transaction per file.
    let chunks = corpus_ingest_chunks(&corpus);
    let files: Vec<(&str, &[E2eTextChunkSpec<'_>])> = corpus
        .iter()
        .zip(&chunks)
        .map(|((path, _), chunks)| (path.as_str(), chunks.as_slice()))
        .collect();
    let ids = rt
        .ingest_text_files_one_batch(&files)
        .with_context(|| format!("concurrency bootstrap: ingest {} corpus files", files.len()))?;
    if ids.len() != corpus.len() {
        return Err(anyhow::anyhow!(
            "concurrency bootstrap: ingest returned {} chunk IDs for {} files",
            ids.len(),
            corpus.len()
        ));
    }
    rt.ingest_symbol(
        CONCURRENCY_REPO,
        "repo0/src/file_0.rs",
        "concurrency-scale-needle",
        FAST_QUERY_TOKEN,
    )
    .context("concurrency bootstrap: ingest symbol needle")?;
    let sealed = rt
        .seal()
        .context("concurrency bootstrap: seal generation")?;
    rt.activate_last_sealed_generation()
        .context("concurrency bootstrap: activate generation")?;
    let pin = GenerationPin::new(rt.repo(), rt.revision(), sealed);
    // One served request of every route before timing, so readiness and
    // the cold open are not inside any window and a route the fixture
    // cannot serve is a rail error, not a row of typed errors. The first
    // probe waits for the activated generation to serve.
    let primed = rt.query_text(TextQuerySyntax::Native, FAST_QUERY_TOKEN, FAST_TOP_K);
    if let Some(error) = primed.typed_error {
        return Err(anyhow::anyhow!(
            "concurrency prime: the fixture does not serve: {} {}",
            error.code,
            error.message
        ));
    }
    let probes = MixedRoute::ALL
        .iter()
        .map(|route| (route.as_str(), fast_request(*route, &pin)))
        .chain(std::iter::once(("slow", slow_request(&pin))));
    for (label, request) in probes {
        if let SearchPlaneQueryIpcResponse::Error(error) = rt
            .query_once(|_| request)
            .with_context(|| format!("concurrency prime: {label} route"))?
        {
            return Err(anyhow::anyhow!(
                "concurrency prime: {label} refused before timing: {} {}",
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
        measurements.push(
            measure_clients(&socket, &pin, clients, requests_per_client)
                .with_context(|| format!("concurrency measurement: {clients} fast clients"))?,
        );
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
        "error_codes": group.error_codes.iter().map(|code| code.as_wire_str()).collect::<Vec<_>>(),
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
        "minimum_row_samples": MINIMUM_ROW_SAMPLES,
        "maximum_samples_per_worker": MAX_SAMPLES_PER_WORKER,
        "measurement_timeout_secs": MEASUREMENT_TIMEOUT.as_secs(),
        "bootstrap_request_timeout_secs": FIXTURE_REQUEST_TIMEOUT.as_secs(),
        "prime_request_timeout_secs": FIXTURE_REQUEST_TIMEOUT.as_secs(),
        "sampling_policy": "fast clients run at least requests_per_client and remain active until the slow client reaches minimum_row_samples; slow client runs through the fast window",
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
        typed_error_code: group
            .error_codes
            .first()
            .map(|code| code.as_wire_str().to_owned()),
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
                                (
                                    "lexical_count_query",
                                    "scale_needle_token count:all".to_string(),
                                ),
                                ("slow_top_k", SLOW_TOP_K.to_string()),
                                ("minimum_row_samples", MINIMUM_ROW_SAMPLES.to_string()),
                                ("sampling_policy", "fast-until-slow-floor".to_string()),
                                (
                                    "request_identity_policy",
                                    "positive-disjoint-worker-ranges-v1".to_string(),
                                ),
                                ("bootstrap_ingest", "single-corpus-batch-v1".to_string()),
                                (
                                    "bootstrap_request_timeout_secs",
                                    FIXTURE_REQUEST_TIMEOUT.as_secs().to_string(),
                                ),
                                (
                                    "prime_request_timeout_secs",
                                    FIXTURE_REQUEST_TIMEOUT.as_secs().to_string(),
                                ),
                                (
                                    "maximum_samples_per_worker",
                                    MAX_SAMPLES_PER_WORKER.to_string(),
                                ),
                                (
                                    "measurement_timeout_secs",
                                    MEASUREMENT_TIMEOUT.as_secs().to_string(),
                                ),
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
    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
    };

    fn sample(route: Option<MixedRoute>, wall_ms: f64, outcome: RequestOutcome) -> RequestSample {
        RequestSample {
            route,
            wall_ms,
            outcome,
        }
    }

    #[test]
    fn bulk_bootstrap_preserves_public_single_file_chunk_contract() {
        let corpus = generate_corpus(ScaleTier::Medium, 7);
        let chunks = corpus_ingest_chunks(&corpus);
        assert_eq!(corpus.len(), 256, "the medium fixture has 4 * 64 files");
        assert_eq!(chunks.len(), corpus.len());
        for ((_, content), [chunk]) in corpus.iter().zip(&chunks) {
            // E2eRuntime::ingest_text_with_candidate_id's public fixture:
            // one complete content record, lines 1..2, no source-repo override.
            assert_eq!(chunk.content.as_bytes(), content.as_bytes());
            assert_eq!((chunk.start_line, chunk.end_line), (1, 2));
            assert!(chunk.source_repo_id.is_none());
        }
    }

    #[test]
    fn invalid_report_configuration_fails_with_stage_before_bootstrap() {
        let error = run_concurrency_report(7, 0).expect_err("underfilled budget must fail");
        let message = format!("{error:#}");
        assert!(message.contains("concurrency configuration: request budget"));
        assert!(message.contains("must be at least 80"));
    }

    #[test]
    fn fixture_deadline_does_not_relax_measured_query_deadline() {
        assert_eq!(FIXTURE_REQUEST_TIMEOUT, Duration::from_secs(300));
        assert_eq!(REQUEST_TIMEOUT, Duration::from_secs(30));
    }

    #[test]
    fn fast_request_ids_obey_public_nonzero_contract_without_collisions() {
        let slow_first = u64::MAX;
        let slow_last = u64::MAX - u64::from(MAX_SAMPLES_PER_WORKER - 1);
        assert!(std::num::NonZeroU64::new(slow_last).is_some());
        assert!(slow_first > slow_last);
        let mut previous_end = 0;
        for client in 0..32 {
            let first = fast_request_id(client, 0).expect("bounded client");
            let last = fast_request_id(client, u64::from(MAX_SAMPLES_PER_WORKER - 1))
                .expect("bounded last sample");
            assert!(std::num::NonZeroU64::new(first).is_some());
            assert!(std::num::NonZeroU64::new(last).is_some());
            assert!(
                first > previous_end,
                "worker request ranges must be disjoint"
            );
            assert!(last < slow_last, "fast and slow ID ranges must be disjoint");
            previous_end = last;
        }
        assert!(fast_request_id(1, u64::MAX).is_err());
        assert_eq!(fast_request_id(0, 0).expect("first request"), 1);
    }

    #[test]
    fn mixed_route_requests_use_public_query_contracts() {
        let pin = GenerationPin::new(
            RepoId::new("fixture").expect("repo"),
            RevisionId::new("fixture").expect("revision"),
            ManifestGeneration::new(1),
        );
        let SearchPlaneQueryIpcRequest::Text(request) =
            fast_request(MixedRoute::LexicalCount, &pin)
        else {
            panic!("exact count must use the public text route");
        };
        // The public DSL count bound accepts an integer or `all`;
        // the prior `yes` was refused at the real prime frontdoor.
        assert_eq!(request.query_text, "scale_needle_token count:all");
        assert_eq!(request.generation, Some(pin.clone()));
        assert_eq!(request.syntax, TextQuerySyntax::Native);
        for route in [
            MixedRoute::Lexical,
            MixedRoute::Semantic,
            MixedRoute::Hybrid,
            MixedRoute::Symbol,
        ] {
            let request = fast_request(route, &pin);
            match (route, request) {
                (MixedRoute::Lexical, SearchPlaneQueryIpcRequest::Text(request)) => {
                    assert_eq!(request.query_text, "scale_needle_token");
                    assert_eq!(request.syntax, TextQuerySyntax::Native);
                    assert_eq!(request.generation, Some(pin.clone()));
                }
                (MixedRoute::Semantic, SearchPlaneQueryIpcRequest::Semantic(request)) => {
                    assert_eq!(request.query_text, "scale_needle_token");
                    assert_eq!(request.generation, Some(pin.clone()));
                }
                (MixedRoute::Hybrid, SearchPlaneQueryIpcRequest::Hybrid(request)) => {
                    assert_eq!(request.text_query.query_text, "scale_needle_token");
                    assert_eq!(request.semantic_query_text, "scale_needle_token");
                    assert_eq!(request.generation, Some(pin.clone()));
                }
                (MixedRoute::Symbol, SearchPlaneQueryIpcRequest::Symbol(request)) => {
                    assert_eq!(request.query_text, "scale_needle_token");
                    assert_eq!(request.syntax, TextQuerySyntax::Native);
                    assert_eq!(request.generation, Some(pin.clone()));
                }
                _ => panic!("route must use its public request variant"),
            }
        }
    }

    #[test]
    fn mixed_routes_keep_their_artifact_route_family() {
        assert_eq!(MixedRoute::Lexical.route_family(), RouteFamily::Lexical);
        assert_eq!(
            MixedRoute::LexicalCount.route_family(),
            RouteFamily::Lexical
        );
        assert_eq!(MixedRoute::Semantic.route_family(), RouteFamily::Semantic);
        assert_eq!(MixedRoute::Hybrid.route_family(), RouteFamily::Hybrid);
        assert_eq!(MixedRoute::Symbol.route_family(), RouteFamily::Symbol);
    }

    #[test]
    fn authoritative_request_budget_rejects_every_underfilled_route() {
        for requests in [0, 1, 16, DEFAULT_REQUESTS_PER_CLIENT - 1] {
            assert!(validate_requests_per_client(requests).is_err());
        }
        validate_requests_per_client(DEFAULT_REQUESTS_PER_CLIENT).expect("default meets the floor");
        let samples = collect_fast_samples(
            DEFAULT_REQUESTS_PER_CLIENT,
            &AtomicBool::new(true),
            SamplingBudget::new(),
            |_, route| {
                Ok(sample(
                    Some(route),
                    1.0,
                    RequestOutcome::Served { result_count: 1 },
                ))
            },
        )
        .expect("requests served");
        for route in MixedRoute::ALL {
            let count = samples
                .iter()
                .filter(|sample| sample.route == Some(route))
                .count();
            assert_eq!(
                count, 16,
                "the public gate requires 16 samples for every route"
            );
        }
    }

    #[test]
    fn fast_workers_remain_active_until_slow_floor_and_then_stop() {
        let ready = AtomicBool::new(false);
        let samples = collect_fast_samples(
            DEFAULT_REQUESTS_PER_CLIENT,
            &ready,
            SamplingBudget::new(),
            |index, route| {
                if index == 84 {
                    ready.store(true, Ordering::Release);
                }
                Ok(sample(
                    Some(route),
                    1.0,
                    RequestOutcome::Served { result_count: 1 },
                ))
            },
        )
        .expect("requests served");
        assert_eq!(samples.len(), 85, "five more requests during slow sampling");
    }

    #[test]
    fn slow_sampling_reaches_floor_before_release_and_stops_with_fast_window() {
        let stop = AtomicBool::new(false);
        let ready = AtomicBool::new(false);
        let samples = collect_slow_samples(&stop, &ready, SamplingBudget::new(), |index| {
            assert_eq!(ready.load(Ordering::Acquire), index >= 16);
            if index == 19 {
                stop.store(true, Ordering::Release);
            }
            Ok(sample(
                None,
                1.0,
                RequestOutcome::Served { result_count: 1 },
            ))
        })
        .expect("requests served");
        assert_eq!(samples.len(), 20);
        assert!(ready.load(Ordering::Acquire));
    }

    #[test]
    fn slow_sampling_cannot_be_empty_when_fast_workers_stop_early() {
        let ready = AtomicBool::new(false);
        let samples = collect_slow_samples(
            &AtomicBool::new(true),
            &ready,
            SamplingBudget::new(),
            |_| {
                Ok(sample(
                    None,
                    1.0,
                    RequestOutcome::Served { result_count: 1 },
                ))
            },
        )
        .expect("floor served");
        assert_eq!(samples.len(), 16);
        assert!(ready.load(Ordering::Acquire));
    }

    #[test]
    fn slow_failure_releases_fast_workers_and_preserves_failure() {
        let ready = AtomicBool::new(false);
        let result = collect_slow_samples(
            &AtomicBool::new(false),
            &ready,
            SamplingBudget::new(),
            |_| Err(anyhow::anyhow!("transport refused")),
        );
        assert!(result.is_err());
        assert!(ready.load(Ordering::Acquire));
    }

    #[test]
    fn slow_panic_releases_fast_workers_and_remains_a_panic() {
        let ready = AtomicBool::new(false);
        let result = std::panic::catch_unwind(|| {
            collect_slow_samples(
                &AtomicBool::new(false),
                &ready,
                SamplingBudget::new(),
                |_| panic!("slow worker panicked"),
            )
        });
        assert!(result.is_err());
        assert!(ready.load(Ordering::Acquire));
    }

    #[test]
    fn sampling_budget_exhaustion_is_an_error_and_releases_fast_workers() {
        let ready = AtomicBool::new(false);
        let budget = SamplingBudget {
            started: Instant::now(),
            timeout: MEASUREMENT_TIMEOUT,
            max_samples: 2,
        };
        let result =
            collect_fast_samples(DEFAULT_REQUESTS_PER_CLIENT, &ready, budget, |_, route| {
                Ok(sample(
                    Some(route),
                    1.0,
                    RequestOutcome::Served { result_count: 1 },
                ))
            });
        assert!(
            result
                .expect_err("never return a partial fast rail")
                .to_string()
                .contains("sample budget")
        );
        let result = collect_slow_samples(&AtomicBool::new(false), &ready, budget, |_| {
            Ok(sample(
                None,
                1.0,
                RequestOutcome::Served { result_count: 1 },
            ))
        });
        assert!(
            result
                .expect_err("never return a partial slow rail")
                .to_string()
                .contains("sample budget")
        );
        assert!(ready.load(Ordering::Acquire));
        assert!(validate_requests_per_client(MAX_SAMPLES_PER_WORKER + 1).is_err());
    }

    #[test]
    fn elapsed_measurement_deadline_prevents_another_request() {
        let budget = SamplingBudget {
            started: Instant::now(),
            timeout: Duration::ZERO,
            max_samples: MAX_SAMPLES_PER_WORKER,
        };
        let result = collect_fast_samples(
            DEFAULT_REQUESTS_PER_CLIENT,
            &AtomicBool::new(true),
            budget,
            |_, _| panic!("must not issue a request after the deadline"),
        );
        assert!(
            result
                .expect_err("elapsed deadline must fail")
                .to_string()
                .contains("deadline")
        );
        let ready = AtomicBool::new(false);
        let result = collect_slow_samples(&AtomicBool::new(false), &ready, budget, |_| {
            panic!("must not issue a slow request after the deadline")
        });
        assert!(result.is_err());
        assert!(ready.load(Ordering::Acquire));
    }

    #[test]
    fn answered_queries_must_match_request_id_and_requested_route() {
        let pin = GenerationPin::new(
            RepoId::new("repo-concurrency").expect("fixture repo"),
            RevisionId::new("rev-concurrency").expect("fixture revision"),
            ManifestGeneration::new(1),
        );
        let mut response = SearchPlaneQueryIpcResponseEnvelope {
            request_id: 7,
            payload: SearchPlaneQueryIpcResponse::Symbol(
                quanta_index_contract::SymbolQueryResponse {
                    generation: pin.clone(),
                    results: Vec::new(),
                    window: quanta_index_contract::QueryResultWindowV2::exact_probe(0),
                    next_cursor: None,
                },
            ),
        };
        assert_eq!(
            classify_response(7, Some(MixedRoute::Symbol), &pin, &response)
                .expect("matching symbol answer"),
            RequestOutcome::Served { result_count: 0 }
        );
        for route in [
            None,
            Some(MixedRoute::Lexical),
            Some(MixedRoute::LexicalCount),
            Some(MixedRoute::Semantic),
            Some(MixedRoute::Hybrid),
        ] {
            assert!(
                classify_response(7, route, &pin, &response).is_err(),
                "symbol answer must not satisfy {route:?}"
            );
        }
        let wrong_generation = GenerationPin::new(
            RepoId::new("repo-concurrency").expect("fixture repo"),
            RevisionId::new("wrong-revision").expect("other revision"),
            ManifestGeneration::new(1),
        );
        assert!(
            classify_response(7, Some(MixedRoute::Symbol), &wrong_generation, &response)
                .expect_err("wrong source generation")
                .to_string()
                .contains("generation")
        );
        response.request_id = 8;
        assert!(
            classify_response(7, Some(MixedRoute::Symbol), &pin, &response)
                .expect_err("wrong request identity")
                .to_string()
                .contains("request_id")
        );
    }

    #[test]
    fn authority_responses_are_not_counted_as_served_queries() {
        let repo_id = RepoId::new("repo-concurrency").expect("canonical fixture repo");
        let revision_id = RevisionId::new("rev-concurrency").expect("canonical fixture revision");
        let generation = ManifestGeneration::new(1);
        let pin = GenerationPin::new(repo_id.clone(), revision_id.clone(), generation);
        let active = GenerationSnapshot {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: generation,
            manifest_digest: "fixture-digest".to_string(),
        };
        let resolution = quanta_index_contract::ActiveGenerationResolutionV1 {
            track: SearchPlaneTrackKind::Lexical,
            head: quanta_index_contract::SearchCorpusActiveHeadV1 {
                generation: quanta_index_contract::SearchCorpusGenerationIdentityV1 {
                    lexical: active,
                    semantic: GenerationSnapshot {
                        repo_id,
                        revision_id,
                        track: SearchPlaneTrackKind::Semantic,
                        manifest_generation: generation,
                        manifest_digest: "fixture-digest".to_string(),
                    },
                    semantic_content: quanta_index_contract::SemanticContentRootsV1 {
                        row_root_digest: format!("sha256:{}", "a".repeat(64)),
                        membership_root_digest: format!("sha256:{}", "b".repeat(64)),
                    },
                },
                activation_token: quanta_index_contract::SearchCorpusActivationTokenV1::new(
                    [7; quanta_index_contract::ACTIVATION_ROOT_INCARNATION_BYTES_V1],
                    std::num::NonZeroU64::MIN,
                )
                .expect("fixture root incarnation is nonzero"),
            },
        };
        assert_eq!(
            result_count_of(&SearchPlaneQueryIpcResponse::ActiveGenerationSnapshot(
                resolution
            )),
            None
        );
        assert_eq!(
            result_count_of(&SearchPlaneQueryIpcResponse::ResolvedLexicalGeneration(pin)),
            None
        );
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
                    code: SearchPlaneErrorCodeV2::ServerOverloaded,
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
        assert_eq!(
            summary.error_codes,
            vec![SearchPlaneErrorCodeV2::ServerOverloaded]
        );
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
        assert_eq!(value["rows"][5]["scenario_id"], "concurrency.c8.fast");
        assert_eq!(value["rows"][6]["scenario_id"], "concurrency.c8.slow");
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
