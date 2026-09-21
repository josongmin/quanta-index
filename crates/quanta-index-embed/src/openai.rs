use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use quanta_index_contract::EmbeddingNormalization;
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_core::{
    CoreError, EMBED_CHECKPOINT, MAX_EMBEDDING_DIMENSION, RequestBudgetV1, TextEmbeddingProvider,
};
use serde::{Deserialize, Serialize};

use crate::telemetry;

const EMBEDDINGS_PATH: &str = "/v1/embeddings";
const DEFAULT_BASE_URL: &str = "https://api.openai.com";
/// Default max inputs per `/v1/embeddings` request. Public so daemon env knobs
/// can default to the same value without a second source of truth.
pub const DEFAULT_MAX_BATCH: usize = 256;
/// Conservative per-request packing budget used by the local batching policy.
///
/// This is a request-shaping heuristic, not a claim about any upstream provider
/// hard limit. A single oversized text is still sent alone instead of failing
/// closed on an estimate.
pub const DEFAULT_MAX_ESTIMATED_TOKENS_PER_REQUEST: usize = 4096;
/// Default bounded retry count for transient failures.
pub const DEFAULT_MAX_RETRIES: u32 = 3;
/// Default per-request HTTP timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
/// Default number of `/v1/embeddings` requests dispatched concurrently from one
/// `embed_batch` call.
///
/// A small pool overlaps network round-trips while staying under typical provider
/// rate limits. Set to 1 to force fully sequential dispatch.
pub const DEFAULT_CONCURRENCY: usize = 4;
/// Most `/v1/embeddings` requests one `embed_batch` call may hold in flight
/// (QI-BB-021).
///
/// Every in-flight request holds its serialized inputs and, on return, its
/// decoded vectors; the ceiling keeps that transient bounded by a number an
/// operator can reason about instead of by whatever the env var said. The
/// texts of one `embed_batch` call are one stream window's (the search
/// plane embeds a batch window by window), so the vectors resident in this
/// crate at once are one window plus one round of in-flight requests; the
/// default window is sized to exactly one round at the default tuning
/// (`DEFAULT_CONCURRENCY × DEFAULT_MAX_BATCH` texts), which the tests pin.
pub const MAX_CONCURRENCY: usize = 64;
const ERROR_BODY_PREVIEW_CHARS: usize = 200;
/// How often an attempt in flight re-reads its request budget (QI-BB-002):
/// a cancelled or expired budget is answered within one interval.
const BUDGET_POLL_INTERVAL: Duration = Duration::from_millis(25);

mod batching;
mod retry;
use batching::{RequestBatch, partition_request_batches};
use retry::{StatusClass, backoff_delay, classify_status};

/// Outbound transport for a single embeddings request.
///
/// Returns the HTTP status code and response body, or a typed transport error.
/// Abstracted so the provider's batching / retry / parse logic is unit-testable
/// without network.
pub trait EmbeddingTransport: Send + Sync {
    /// Post one embeddings request, giving up after `timeout`.
    ///
    /// The provider passes the smaller of its configured timeout and what
    /// is left of the request budget (QI-BB-002), so no attempt outlives
    /// the request it serves.
    fn post_embeddings(
        &self,
        url: &str,
        api_key: &str,
        body: &str,
        timeout: Duration,
    ) -> Result<HttpResponse, CoreError>;

    /// The HTTP timeout this transport's client was built with, when it has one.
    /// Defaults to `None` for transports without a network client (e.g. test
    /// stubs); the real reqwest transport returns its configured value so the
    /// `with_reqwest(config)` -> client timeout wire is observable end to end.
    fn configured_timeout(&self) -> Option<Duration> {
        None
    }
}

/// A raw HTTP response (status + body) returned by an [`EmbeddingTransport`].
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Construction parameters for the `OpenAI` embedding provider.
///
/// `api_key` is held only inside the provider and is never logged. Tuning (batch
/// / retries / timeout) defaults to the constants and is overridable so the
/// daemon can wire it from env knobs.
#[derive(Clone)]
pub struct OpenAiProviderConfig {
    pub api_key: String,
    pub model: String,
    /// The immutable revision of `model` the operator is pinning (QI-BB-028).
    /// `OpenAI` does not expose one on the wire, so the operator names it
    /// and rotates it when the served model changes; every cache namespace,
    /// sealed generation and query gate keys on it.
    pub model_revision: String,
    pub dimension: usize,
    pub base_url: String,
    pub max_batch: usize,
    pub max_estimated_tokens_per_request: usize,
    pub max_retries: u32,
    pub timeout: Duration,
    pub concurrency: usize,
}

impl OpenAiProviderConfig {
    /// Config for `model` at `model_revision` and `dimension` against the
    /// public `OpenAI` endpoint, with default tuning.
    #[must_use]
    pub fn new(api_key: String, model: String, model_revision: String, dimension: usize) -> Self {
        Self {
            api_key,
            model,
            model_revision,
            dimension,
            base_url: DEFAULT_BASE_URL.to_string(),
            max_batch: DEFAULT_MAX_BATCH,
            max_estimated_tokens_per_request: DEFAULT_MAX_ESTIMATED_TOKENS_PER_REQUEST,
            max_retries: DEFAULT_MAX_RETRIES,
            timeout: DEFAULT_TIMEOUT,
            concurrency: DEFAULT_CONCURRENCY,
        }
    }

    /// Override the API base URL (used by tests to target a stub server).
    #[must_use]
    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    /// Max inputs per `/v1/embeddings` request; zero is refused at
    /// construction.
    #[must_use]
    pub const fn with_max_batch(mut self, max_batch: usize) -> Self {
        self.max_batch = max_batch;
        self
    }

    /// Conservative estimated-token budget per `/v1/embeddings` request;
    /// zero is refused at construction.
    #[must_use]
    pub const fn with_max_estimated_tokens_per_request(
        mut self,
        max_estimated_tokens_per_request: usize,
    ) -> Self {
        self.max_estimated_tokens_per_request = max_estimated_tokens_per_request;
        self
    }

    /// Bounded retry count for transient (429 / 5xx / transport) failures.
    #[must_use]
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Per-request HTTP timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Number of embedding requests dispatched concurrently per
    /// `embed_batch` (1 = fully sequential); zero and anything past
    /// [`MAX_CONCURRENCY`] are refused at construction.
    #[must_use]
    pub const fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency;
        self
    }
}

/// `OpenAI`-backed [`TextEmbeddingProvider`].
///
/// Embeds via batched `/v1/embeddings` calls behind a blocking transport, failing
/// closed on auth / transport / shape errors and retrying transient (429 / 5xx /
/// network) failures with bounded exponential backoff.
pub struct OpenAiEmbeddingProvider {
    /// Shared with the thread each attempt runs on, so an attempt a
    /// cancelled budget abandons can finish on its own.
    transport: Arc<dyn EmbeddingTransport>,
    api_key: Arc<str>,
    model: String,
    model_id: String,
    model_revision: String,
    dimension: usize,
    base_url: String,
    max_batch: usize,
    max_estimated_tokens_per_request: usize,
    max_retries: u32,
    timeout: Duration,
    concurrency: usize,
}

impl OpenAiEmbeddingProvider {
    /// Build a provider over an explicit transport (the real one or a test stub).
    pub fn new(
        config: OpenAiProviderConfig,
        transport: Box<dyn EmbeddingTransport>,
    ) -> Result<Self, CoreError> {
        if config.api_key.trim().is_empty() {
            return Err(typed(LexicalErrorCode::SemProviderAuth, "openai: API key is empty"));
        }
        if config.model.trim().is_empty() {
            return Err(invalid("openai: model is empty"));
        }
        if config.model_revision.trim().is_empty()
            || !config
                .model_revision
                .bytes()
                .all(|byte| byte.is_ascii_graphic())
        {
            return Err(invalid(
                "openai: model revision must be a non-empty printable ASCII token (QI-BB-028)",
            ));
        }
        if config.dimension == 0 || config.dimension > MAX_EMBEDDING_DIMENSION {
            return Err(invalid(&format!(
                "openai: dimension {} is outside 1..={MAX_EMBEDDING_DIMENSION}",
                config.dimension
            )));
        }
        if config.max_batch == 0 {
            return Err(invalid("openai: max_batch must be at least 1"));
        }
        if config.max_estimated_tokens_per_request == 0 {
            return Err(invalid("openai: max_estimated_tokens_per_request must be at least 1"));
        }
        if config.concurrency == 0 || config.concurrency > MAX_CONCURRENCY {
            return Err(invalid(&format!(
                "openai: concurrency {} is outside 1..={MAX_CONCURRENCY}",
                config.concurrency
            )));
        }
        if config.timeout.is_zero() {
            return Err(invalid("openai: timeout must be non-zero"));
        }
        let model_id = format!("openai:{}", config.model);
        Ok(Self {
            transport: Arc::from(transport),
            api_key: Arc::from(config.api_key),
            model: config.model,
            model_id,
            model_revision: config.model_revision,
            dimension: config.dimension,
            base_url: config.base_url,
            max_batch: config.max_batch,
            max_estimated_tokens_per_request: config.max_estimated_tokens_per_request,
            max_retries: config.max_retries,
            timeout: config.timeout,
            concurrency: config.concurrency,
        })
    }

    /// Build a provider over the real in-process blocking reqwest transport,
    /// honoring the config's timeout / batch / retry tuning.
    pub fn with_reqwest(config: OpenAiProviderConfig) -> Result<Self, CoreError> {
        let transport = ReqwestBlockingTransport::new(config.timeout)?;
        Self::new(config, Box::new(transport))
    }

    /// The HTTP timeout the provider's transport was built with, if it exposes
    /// one. Lets tests observe that `with_reqwest` threaded `config.timeout` all
    /// the way into the transport's client.
    #[cfg(test)]
    fn transport_configured_timeout(&self) -> Option<Duration> {
        self.transport.configured_timeout()
    }

    fn embed_one_batch(
        &self,
        texts: &[&str],
        estimated_tokens: usize,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<Vec<f32>>, CoreError> {
        let request = EmbeddingsRequest {
            model: &self.model,
            input: texts,
            dimensions: self.dimension,
        };
        let body = serde_json::to_string(&request)
            .map_err(|err| invalid(&format!("openai: request encode failed: {err}")))?;
        let url = format!("{}{EMBEDDINGS_PATH}", self.base_url);
        let raw = self.post_with_retry(&url, &body, texts.len(), estimated_tokens, budget)?;
        let parsed: EmbeddingsResponse = serde_json::from_str(&raw)
            .map_err(|err| CoreError::Storage(format!("openai: response decode failed: {err}")))?;
        let mut data = parsed.data;
        data.sort_by(|left, right| left.index.cmp(&right.index));
        if data.len() != texts.len() {
            return Err(CoreError::Storage(format!(
                "openai: returned {} embeddings for {} inputs",
                data.len(),
                texts.len()
            )));
        }
        for (expected, item) in data.iter().enumerate() {
            if item.index != expected {
                return Err(CoreError::Storage(
                    "openai: non-contiguous embedding indices in response".to_string(),
                ));
            }
            if item.embedding.len() != self.dimension {
                return Err(CoreError::Storage(format!(
                    "openai: embedding dim {} != configured dim {}",
                    item.embedding.len(),
                    self.dimension
                )));
            }
        }
        Ok(data.into_iter().map(|item| item.embedding).collect())
    }

    /// Every attempt runs under the request budget (QI-BB-002).
    ///
    /// The budget is checked before each attempt and each backoff, the
    /// attempt's HTTP timeout is capped by what is left of the budget, and
    /// an attempt in flight is abandoned — answered typed at the
    /// `semantic:embed` checkpoint — the moment the budget is cancelled or
    /// expires. The abandoned attempt finishes on its own thread within
    /// its capped timeout; nothing waits for it.
    fn post_with_retry(
        &self,
        url: &str,
        body: &str,
        texts_in_request: usize,
        estimated_tokens: usize,
        budget: &RequestBudgetV1,
    ) -> Result<String, CoreError> {
        let mut last_error: Option<CoreError> = None;
        for attempt in 0..=self.max_retries {
            budget.checkpoint(EMBED_CHECKPOINT)?;
            telemetry::record_http_request(texts_in_request, estimated_tokens);
            match self.post_under_budget(url, body, budget) {
                Ok(response) => match classify_status(response.status) {
                    StatusClass::Success => return Ok(response.body),
                    StatusClass::Auth => {
                        return Err(typed(
                            LexicalErrorCode::SemProviderAuth,
                            &format!("openai: auth rejected (status {})", response.status),
                        ));
                    }
                    StatusClass::Fatal => {
                        return Err(typed(
                            LexicalErrorCode::SemProviderTransport,
                            &format!(
                                "openai: non-retryable status {} body {}",
                                response.status,
                                preview(&response.body)
                            ),
                        ));
                    }
                    StatusClass::Retryable => {
                        telemetry::record_retryable_status();
                        last_error = Some(typed(
                            LexicalErrorCode::SemProviderTransport,
                            &format!("openai: retryable status {}", response.status),
                        ));
                    }
                },
                Err(err) => {
                    telemetry::record_transport_error();
                    last_error = Some(err);
                }
            }
            if attempt < self.max_retries {
                telemetry::record_retry();
                sleep_within_budget(backoff_delay(attempt), budget)?;
            }
        }
        Err(last_error.unwrap_or_else(|| {
            typed(
                LexicalErrorCode::SemProviderTransport,
                "openai: transport exhausted retries without a response",
            )
        }))
    }

    /// One attempt, on its own thread, watched against the budget.
    ///
    /// The attempt's timeout is the smaller of the configured one and the
    /// budget's remainder, so the thread never outlives the request by more
    /// than that. The caller polls the budget while it waits; an
    /// interruption returns at once and the attempt's eventual result is
    /// dropped with the channel.
    fn post_under_budget(
        &self,
        url: &str,
        body: &str,
        budget: &RequestBudgetV1,
    ) -> Result<HttpResponse, CoreError> {
        let timeout = self.timeout.min(budget.remaining());
        if timeout.is_zero() {
            // The deadline passed between the checkpoint and here.
            return Err(budget.interrupted_at(EMBED_CHECKPOINT).unwrap_or_else(|| {
                typed(
                    LexicalErrorCode::SemProviderTransport,
                    "openai: no time left in the request budget for an attempt",
                )
            }));
        }
        let transport = Arc::clone(&self.transport);
        let api_key = Arc::clone(&self.api_key);
        let url = url.to_string();
        let body = body.to_string();
        let (done, outcome) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("openai-embed-attempt".to_string())
            .spawn(move || {
                let result = transport.post_embeddings(&url, &api_key, &body, timeout);
                // The requester may have left: a failed send means exactly
                // that, and the result is dropped with the receiver.
                let _abandoned = done.send(result);
            });
        if let Err(error) = spawned {
            return Err(typed(
                LexicalErrorCode::SemProviderTransport,
                &format!("openai: could not start the embedding attempt: {error}"),
            ));
        }
        loop {
            match outcome.recv_timeout(BUDGET_POLL_INTERVAL) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => budget.checkpoint(EMBED_CHECKPOINT)?,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(typed(
                        LexicalErrorCode::SemProviderTransport,
                        "openai: embedding attempt thread ended without a result",
                    ));
                }
            }
        }
    }

    /// Dispatch `batches` over a bounded pool of scoped worker threads, each
    /// pulling the next batch index from a shared cursor and running its own
    /// retry/backoff. The transport is `Send + Sync`, so `&self` is shared
    /// directly with no clone. Results are reassembled by batch index, so the
    /// returned vectors keep input order regardless of completion order. An error
    /// fails the whole call (returning the lowest-index error among batches that
    /// ran), and a worker stops pulling new work once any batch has failed (bounds
    /// wasted requests on failure).
    fn embed_batches_concurrently(
        &self,
        texts: &[&str],
        batches: &[RequestBatch],
        budget: &RequestBudgetV1,
    ) -> Result<Vec<Vec<f32>>, CoreError> {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let cursor = AtomicUsize::new(0);
        let failed = AtomicBool::new(false);
        let worker_count = self.concurrency.min(batches.len()).max(1);
        let worker_results: Result<Vec<Vec<IndexedBatchResult>>, CoreError> =
            std::thread::scope(|scope| {
                let handles: Vec<_> = (0..worker_count)
                    .map(|_| {
                        scope.spawn(|| {
                            let mut local: Vec<IndexedBatchResult> = Vec::new();
                            loop {
                                if failed.load(Ordering::Relaxed) {
                                    break;
                                }
                                let index = cursor.fetch_add(1, Ordering::Relaxed);
                                let Some(batch) = batches.get(index) else {
                                    break;
                                };
                                let result =
                                    request_batch_texts(texts, *batch).and_then(|batch_texts| {
                                        self.embed_one_batch(
                                            batch_texts,
                                            batch.estimated_tokens,
                                            budget,
                                        )
                                    });
                                let is_err = result.is_err();
                                local.push((index, result));
                                if is_err {
                                    failed.store(true, Ordering::Relaxed);
                                    break;
                                }
                            }
                            local
                        })
                    })
                    .collect();
                // A worker panic must not silently drop its batches (that would
                // shorten the output) — surface it as a typed transport error.
                handles
                    .into_iter()
                    .map(|handle| {
                        handle.join().map_err(|_panicked| {
                            typed(
                                LexicalErrorCode::SemProviderTransport,
                                "openai: embedding worker thread panicked",
                            )
                        })
                    })
                    .collect()
            });

        // Reassemble in batch-index order so output matches input order.
        let mut indexed: Vec<IndexedBatchResult> = worker_results?.into_iter().flatten().collect();
        indexed.sort_by_key(|(index, _)| *index);
        flatten_batch_results(indexed.into_iter().map(|(_, result)| result), texts.len())
    }
}

/// Concatenate per-batch embedding results into one flat `vector-per-text` list.
///
/// Preserves iteration order and propagates the first error — the single place
/// the output-order + fail-closed invariant lives for both the sequential and
/// concurrent embed paths.
fn flatten_batch_results(
    ordered: impl IntoIterator<Item = Result<Vec<Vec<f32>>, CoreError>>,
    text_count: usize,
) -> Result<Vec<Vec<f32>>, CoreError> {
    let mut out: Vec<Vec<f32>> = Vec::with_capacity(text_count);
    for result in ordered {
        out.extend(result?);
    }
    Ok(out)
}

/// Sleep `delay` in budget-poll slices, stopping typed the moment the
/// budget is cancelled or expires.
fn sleep_within_budget(delay: Duration, budget: &RequestBudgetV1) -> Result<(), CoreError> {
    let mut left = delay;
    while !left.is_zero() {
        budget.checkpoint(EMBED_CHECKPOINT)?;
        let slice = left.min(BUDGET_POLL_INTERVAL).min(budget.remaining());
        if slice.is_zero() {
            return budget.checkpoint(EMBED_CHECKPOINT);
        }
        std::thread::sleep(slice);
        left = left.saturating_sub(slice);
    }
    Ok(())
}

fn request_batch_texts<'a>(
    texts: &'a [&'a str],
    batch: RequestBatch,
) -> Result<&'a [&'a str], CoreError> {
    texts.get(batch.start..batch.end).ok_or_else(|| {
        CoreError::Storage(format!(
            "openai: invalid request batch range {}..{} for {} texts",
            batch.start,
            batch.end,
            texts.len()
        ))
    })
}

impl TextEmbeddingProvider for OpenAiEmbeddingProvider {
    /// The corpus path: the same attempts as the budgeted path, under a
    /// budget that never interrupts.
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        self.embed_batch_within(texts, &RequestBudgetV1::unbounded())
    }

    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<Vec<Vec<f32>>, CoreError> {
        budget.checkpoint(EMBED_CHECKPOINT)?;
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let batches =
            partition_request_batches(texts, self.max_batch, self.max_estimated_tokens_per_request);
        // Common per-call case (one batch) or concurrency disabled: stay sequential
        // with zero thread/coordination overhead.
        if batches.len() <= 1 || self.concurrency <= 1 {
            return flatten_batch_results(
                batches.iter().map(|batch| {
                    request_batch_texts(texts, *batch).and_then(|batch_texts| {
                        self.embed_one_batch(batch_texts, batch.estimated_tokens, budget)
                    })
                }),
                texts.len(),
            );
        }
        self.embed_batches_concurrently(texts, &batches, budget)
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn model_revision(&self) -> &str {
        &self.model_revision
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn normalization(&self) -> EmbeddingNormalization {
        // Raw provider output; the composition root's L2Unit wrapper is what
        // makes the served vectors unit-normalized (QI-BB-031).
        EmbeddingNormalization::None
    }
}

/// The real blocking transport: a `reqwest::blocking::Client` over rustls.
pub struct ReqwestBlockingTransport {
    client: reqwest::blocking::Client,
    /// The timeout the inner client was built with. `reqwest::Client` does not
    /// expose its configured timeout, so we retain it to make the
    /// config -> transport timeout wire observable in tests.
    configured_timeout: Duration,
}

impl ReqwestBlockingTransport {
    pub fn new(timeout: Duration) -> Result<Self, CoreError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|err| {
                typed(
                    LexicalErrorCode::SemProviderTransport,
                    &format!("openai: http client build failed: {err}"),
                )
            })?;
        Ok(Self {
            client,
            configured_timeout: timeout,
        })
    }
}

impl EmbeddingTransport for ReqwestBlockingTransport {
    fn configured_timeout(&self) -> Option<Duration> {
        Some(self.configured_timeout)
    }

    fn post_embeddings(
        &self,
        url: &str,
        api_key: &str,
        body: &str,
        timeout: Duration,
    ) -> Result<HttpResponse, CoreError> {
        let response = self
            .client
            .post(url)
            .bearer_auth(api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .timeout(timeout)
            .send()
            .map_err(|err| {
                typed(
                    LexicalErrorCode::SemProviderTransport,
                    &format!("openai: request send failed: {err}"),
                )
            })?;
        let status = response.status().as_u16();
        let text = response.text().map_err(|err| {
            typed(
                LexicalErrorCode::SemProviderTransport,
                &format!("openai: response body read failed: {err}"),
            )
        })?;
        Ok(HttpResponse { status, body: text })
    }
}

struct EmbeddingsRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
    dimensions: usize,
}

impl Serialize for EmbeddingsRequest<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;

        let mut state = serializer.serialize_struct("EmbeddingsRequest", 3)?;
        state.serialize_field("model", self.model)?;
        state.serialize_field("input", self.input)?;
        state.serialize_field("dimensions", &self.dimensions)?;
        state.end()
    }
}

struct EmbeddingsResponse {
    data: Vec<EmbeddingItem>,
}

struct EmbeddingItem {
    embedding: Vec<f32>,
    index: usize,
}

// Implements the provider response object decoder without a serde proc macro.
// Unknown fields are ignored because OpenAI adds response metadata over time;
// declared fields remain required and duplicates fail closed.
macro_rules! impl_openai_response_deserialize {
    ($ty:ident { $($field:ident : $field_ty:ty => $field_index:literal),+ $(,)? }) => {
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                struct ResponseVisitor;

                impl<'de> serde::de::Visitor<'de> for ResponseVisitor {
                    type Value = $ty;

                    fn expecting(
                        &self,
                        formatter: &mut core::fmt::Formatter<'_>,
                    ) -> core::fmt::Result {
                        formatter.write_str(concat!("struct ", stringify!($ty)))
                    }

                    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
                    where
                        A: serde::de::SeqAccess<'de>,
                    {
                        $(
                            let $field: $field_ty = sequence.next_element()?.ok_or_else(|| {
                                serde::de::Error::invalid_length($field_index, &self)
                            })?;
                        )+
                        Ok($ty { $($field),+ })
                    }

                    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
                    where
                        A: serde::de::MapAccess<'de>,
                    {
                        $(let mut $field: Option<$field_ty> = None;)+
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $(
                                    stringify!($field) => {
                                        if $field.is_some() {
                                            return Err(serde::de::Error::duplicate_field(
                                                stringify!($field),
                                            ));
                                        }
                                        $field = Some(map.next_value()?);
                                    }
                                )+
                                _ => {
                                    let _: serde::de::IgnoredAny = map.next_value()?;
                                }
                            }
                        }
                        Ok($ty {
                            $(
                                $field: $field.ok_or_else(|| {
                                    serde::de::Error::missing_field(stringify!($field))
                                })?,
                            )+
                        })
                    }
                }

                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                deserializer.deserialize_struct(stringify!($ty), FIELDS, ResponseVisitor)
            }
        }
    };
}

impl_openai_response_deserialize!(EmbeddingsResponse {
    data: Vec<EmbeddingItem> => 0,
});
impl_openai_response_deserialize!(EmbeddingItem {
    embedding: Vec<f32> => 0,
    index: usize => 1,
});

/// A batch's embedding result paired with its batch index, used to reassemble
/// concurrent worker output back into input order.
type IndexedBatchResult = (usize, Result<Vec<Vec<f32>>, CoreError>);

fn preview(body: &str) -> String {
    body.chars().take(ERROR_BODY_PREVIEW_CHARS).collect()
}

fn typed(code: LexicalErrorCode, message: &str) -> CoreError {
    CoreError::Typed {
        code: code.into(),
        message: message.to_string(),
    }
}

fn invalid(message: &str) -> CoreError {
    CoreError::InvalidContract(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Replays a fixed sequence of canned transport results and counts calls, so
    /// batch / retry knobs can be proven by the number of HTTP requests issued.
    struct ScriptedTransport {
        responses: Mutex<Vec<Result<HttpResponse, CoreError>>>,
        calls: Arc<AtomicUsize>,
    }

    impl ScriptedTransport {
        fn new(responses: Vec<Result<HttpResponse, CoreError>>) -> Self {
            Self {
                responses: Mutex::new(responses),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        /// Shared call counter (clone out before the transport is boxed into the
        /// provider, then read after `embed_batch`).
        fn calls_handle(&self) -> Arc<AtomicUsize> {
            Arc::clone(&self.calls)
        }
    }

    impl EmbeddingTransport for ScriptedTransport {
        fn post_embeddings(
            &self,
            _url: &str,
            _api_key: &str,
            _body: &str,
            _timeout: Duration,
        ) -> Result<HttpResponse, CoreError> {
            let _prior = self.calls.fetch_add(1, Ordering::SeqCst);
            let mut responses = self.responses.lock().expect("test mutex");
            if responses.is_empty() {
                return Err(typed(
                    LexicalErrorCode::SemProviderTransport,
                    "scripted transport exhausted",
                ));
            }
            responses.remove(0)
        }
    }

    fn ok_body(vectors: &[(usize, Vec<f32>)]) -> HttpResponse {
        let items: Vec<String> = vectors
            .iter()
            .map(|(index, vector)| {
                let nums: Vec<String> = vector
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect();
                format!("{{\"index\":{index},\"embedding\":[{}]}}", nums.join(","))
            })
            .collect();
        HttpResponse {
            status: 200,
            body: format!("{{\"data\":[{}]}}", items.join(",")),
        }
    }

    fn cfg(dimension: usize) -> OpenAiProviderConfig {
        OpenAiProviderConfig::new(
            "test-key".to_string(),
            "text-embedding-3-small".to_string(),
            "2024-01".to_string(),
            dimension,
        )
    }

    fn provider_with(
        config: OpenAiProviderConfig,
        transport: impl EmbeddingTransport + 'static,
    ) -> OpenAiEmbeddingProvider {
        OpenAiEmbeddingProvider::new(config, Box::new(transport))
            .expect("provider builds with a non-empty key/model/dim")
    }

    #[test]
    fn openai_request_wire_shape_is_exact() {
        let input = ["alpha", "beta"];
        let request = EmbeddingsRequest {
            model: "text-embedding-test",
            input: &input,
            dimensions: 2,
        };

        let body = serde_json::to_string(&request);

        assert!(matches!(
            body.as_deref(),
            Ok(r#"{"model":"text-embedding-test","input":["alpha","beta"],"dimensions":2}"#)
        ));
    }

    #[test]
    fn openai_response_ignores_unknown_fields() {
        let body = r#"{
            "data": [{"embedding": [0.25, 0.75], "index": 0, "object": "embedding"}],
            "model": "text-embedding-test",
            "usage": {"prompt_tokens": 2}
        }"#;

        let response = serde_json::from_str::<EmbeddingsResponse>(body);

        assert!(matches!(
            response.as_ref(),
            Ok(EmbeddingsResponse { data })
                if matches!(
                    data.as_slice(),
                    [item] if item.embedding.as_slice() == [0.25, 0.75] && item.index == 0
                )
        ));
    }

    #[test]
    fn openai_response_rejects_missing_required_fields() {
        let missing_data = serde_json::from_str::<EmbeddingsResponse>(r#"{"object":"list"}"#)
            .map_err(|error| error.to_string());
        let missing_embedding =
            serde_json::from_str::<EmbeddingsResponse>(r#"{"data":[{"index":0}]}"#)
                .map_err(|error| error.to_string());
        let missing_index =
            serde_json::from_str::<EmbeddingsResponse>(r#"{"data":[{"embedding":[0.25]}]}"#)
                .map_err(|error| error.to_string());

        assert!(matches!(missing_data, Err(ref error) if error.contains("missing field `data`")));
        assert!(
            matches!(missing_embedding, Err(ref error) if error.contains("missing field `embedding`"))
        );
        assert!(matches!(missing_index, Err(ref error) if error.contains("missing field `index`")));
    }

    #[test]
    fn embed_batch_returns_vectors_in_input_order() {
        // Response intentionally out of order; provider must reorder by index.
        let transport =
            ScriptedTransport::new(vec![Ok(ok_body(&[(1, vec![0.0, 1.0]), (0, vec![1.0, 0.0])]))]);
        let provider = provider_with(cfg(2), transport);
        let vectors = provider.embed_batch(&["a", "b"]).expect("embed ok");
        assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        assert_eq!(provider.model_id(), "openai:text-embedding-3-small");
        assert_eq!(provider.dimension(), 2);
    }

    /// Records peak concurrency and the inputs of every request, and
    /// answers each request from its OWN body.
    ///
    /// Each input text `"t<N>"` produces a deterministic, order-checkable vector.
    struct ConcurrencyProbeTransport {
        in_flight: Arc<AtomicUsize>,
        peak_in_flight: Arc<AtomicUsize>,
        /// Inputs each request carried, in completion order.
        request_sizes: Arc<Mutex<Vec<usize>>>,
        delay: Duration,
    }

    impl EmbeddingTransport for ConcurrencyProbeTransport {
        fn post_embeddings(
            &self,
            _url: &str,
            _api_key: &str,
            body: &str,
            _timeout: Duration,
        ) -> Result<HttpResponse, CoreError> {
            let current = self
                .in_flight
                .fetch_add(1, Ordering::SeqCst)
                .saturating_add(1);
            let _prev_peak = self.peak_in_flight.fetch_max(current, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            let parsed: serde_json::Value =
                serde_json::from_str(body).expect("probe: request body is valid json");
            let inputs = parsed
                .get("input")
                .and_then(serde_json::Value::as_array)
                .expect("probe: body has an input array");
            self.request_sizes
                .lock()
                .expect("probe: request sizes mutex")
                .push(inputs.len());
            let items: Vec<(usize, Vec<f32>)> = inputs
                .iter()
                .enumerate()
                .map(|(position, value)| {
                    let text = value.as_str().expect("probe: input is a string");
                    let n: f32 = text
                        .trim_start_matches('t')
                        .parse()
                        .expect("probe: input is t<N>");
                    (position, vec![n])
                })
                .collect();
            let _prior = self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(ok_body(&items))
        }
    }

    /// Always returns the same HTTP status, deterministically, regardless of how
    /// many concurrent calls land before the failure flag stops the rest.
    struct AlwaysStatusTransport {
        status: u16,
    }

    impl EmbeddingTransport for AlwaysStatusTransport {
        fn post_embeddings(
            &self,
            _url: &str,
            _api_key: &str,
            _body: &str,
            _timeout: Duration,
        ) -> Result<HttpResponse, CoreError> {
            Ok(HttpResponse {
                status: self.status,
                body: "{\"error\":\"forced\"}".to_string(),
            })
        }
    }

    #[test]
    fn embed_batch_dispatches_batches_concurrently_and_preserves_order() {
        // max_batch=1 -> one batch per text (10 batches); concurrency=4 -> workers
        // overlap. The probe's per-request delay makes the overlap observable.
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let transport = ConcurrencyProbeTransport {
            in_flight: Arc::clone(&in_flight),
            peak_in_flight: Arc::clone(&peak),
            request_sizes: Arc::new(Mutex::new(Vec::new())),
            delay: Duration::from_millis(25),
        };
        let provider = OpenAiEmbeddingProvider::new(
            cfg(1).with_max_batch(1).with_concurrency(4),
            Box::new(transport),
        )
        .expect("provider builds");
        let texts: Vec<String> = (0..10).map(|n| format!("t{n}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let out = provider.embed_batch(&refs).expect("concurrent embed ok");
        // Output order MUST match input order despite concurrent completion.
        let expected: Vec<Vec<f32>> = (0_u8..10).map(|n| vec![f32::from(n)]).collect();
        assert_eq!(out, expected, "concurrent dispatch must preserve input order");
        // Concurrency actually happened: peak in-flight > 1 (bounded by 4).
        let observed_peak = peak.load(Ordering::SeqCst);
        assert!(observed_peak >= 2, "batches must overlap; peak in-flight was {observed_peak}");
        assert!(
            observed_peak <= 4,
            "concurrency must stay bounded by the knob; peak was {observed_peak}"
        );
    }

    // CASE-COVERS (QI-BB-021 follow-up #2): the search plane embeds a batch
    // one stream window at a time, so one `embed_batch` call carries at most
    // one window of texts. The default window is exactly one round of
    // in-flight requests at the default tuning, and the window byte bound is
    // that round at the widest admitted dimension, so no window under-fills
    // a round and no round exceeds a window; a window of texts is dispatched
    // as `window / max_batch` requests of at most `max_batch` inputs with at
    // most `concurrency` in flight, and the vectors come back in input order.
    #[test]
    fn a_stream_window_of_texts_is_one_round_of_bounded_requests() {
        use quanta_index_core::{
            SEMANTIC_STREAM_WINDOW_SCOPES, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES,
        };
        let round_texts = DEFAULT_CONCURRENCY
            .checked_mul(DEFAULT_MAX_BATCH)
            .expect("round texts fit usize");
        assert_eq!(
            round_texts, SEMANTIC_STREAM_WINDOW_SCOPES,
            "the default window of one-record owner scopes is one round of requests"
        );
        let round_bytes = u64::try_from(round_texts)
            .expect("round texts fit u64")
            .checked_mul(u64::try_from(MAX_EMBEDDING_DIMENSION).expect("dimension fits u64"))
            .and_then(|components| components.checked_mul(4))
            .expect("round bytes fit u64");
        assert_eq!(
            round_bytes, SEMANTIC_STREAM_WINDOW_VECTOR_BYTES,
            "the default window byte bound is one round at the widest dimension"
        );

        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let request_sizes = Arc::new(Mutex::new(Vec::new()));
        let transport = ConcurrencyProbeTransport {
            in_flight: Arc::clone(&in_flight),
            peak_in_flight: Arc::clone(&peak),
            request_sizes: Arc::clone(&request_sizes),
            delay: Duration::ZERO,
        };
        let provider =
            OpenAiEmbeddingProvider::new(cfg(1), Box::new(transport)).expect("provider builds");
        let texts: Vec<String> = (0..SEMANTIC_STREAM_WINDOW_SCOPES)
            .map(|n| format!("t{n}"))
            .collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let out = provider.embed_batch(&refs).expect("a window embeds");
        let expected: Vec<Vec<f32>> = (0..SEMANTIC_STREAM_WINDOW_SCOPES)
            .map(|n| {
                vec![
                    u16::try_from(n)
                        .map(f32::from)
                        .expect("window index fits f32 exactly"),
                ]
            })
            .collect();
        assert_eq!(out, expected, "a window's vectors keep input order");
        let sizes = request_sizes
            .lock()
            .expect("probe: request sizes mutex")
            .clone();
        assert_eq!(sizes.len(), DEFAULT_CONCURRENCY, "a window is window / max_batch requests");
        assert!(
            sizes.iter().all(|size| *size == DEFAULT_MAX_BATCH),
            "every request of a window carries max_batch inputs: {sizes:?}"
        );
        assert!(
            peak.load(Ordering::SeqCst) <= DEFAULT_CONCURRENCY,
            "in-flight requests never exceed the concurrency knob"
        );
        assert_eq!(in_flight.load(Ordering::SeqCst), 0, "nothing is left in flight");
    }

    #[test]
    fn concurrent_embed_batch_propagates_a_batch_error() {
        // A non-retryable status on any batch must fail the whole call (no partial
        // success) even under concurrency.
        let provider = OpenAiEmbeddingProvider::new(
            cfg(1).with_max_batch(1).with_concurrency(4),
            Box::new(AlwaysStatusTransport { status: 400 }),
        )
        .expect("provider builds");
        let texts: Vec<String> = (0..6).map(|n| format!("t{n}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        match provider.embed_batch(&refs) {
            Err(CoreError::Typed { code, .. }) => {
                assert_eq!(
                    code,
                    quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                        LexicalErrorCode::SemProviderTransport
                    )
                );
            }
            other => panic!("a fatal batch must fail the whole concurrent call, got {other:?}"),
        }
    }

    /// Succeeds for every input except `"t<fail_at>"`, which gets a fatal status —
    /// so most concurrent batches succeed while exactly one fails.
    struct PartialFailureTransport {
        fail_at: usize,
    }

    impl EmbeddingTransport for PartialFailureTransport {
        fn post_embeddings(
            &self,
            _url: &str,
            _api_key: &str,
            body: &str,
            _timeout: Duration,
        ) -> Result<HttpResponse, CoreError> {
            let parsed: serde_json::Value =
                serde_json::from_str(body).expect("partial: body is valid json");
            let inputs = parsed
                .get("input")
                .and_then(serde_json::Value::as_array)
                .expect("partial: body has an input array");
            let mut items: Vec<(usize, Vec<f32>)> = Vec::new();
            for (position, value) in inputs.iter().enumerate() {
                let text = value.as_str().expect("partial: input is a string");
                let n: u8 = text
                    .trim_start_matches('t')
                    .parse()
                    .expect("partial: input is t<N>");
                if usize::from(n) == self.fail_at {
                    return Ok(HttpResponse {
                        status: 400,
                        body: "{\"error\":\"forced\"}".to_string(),
                    });
                }
                items.push((position, vec![f32::from(n)]));
            }
            Ok(ok_body(&items))
        }
    }

    #[test]
    fn concurrent_embed_batch_fails_closed_when_one_of_many_batches_errors() {
        // Partial-failure race: with max_batch=1 over 8 inputs (8 batches) and
        // concurrency=4, batch "t5" fails while the other seven succeed. The whole
        // call must still fail closed — a partial success must never leak through as
        // Ok with a short or hole-punched vector list.
        let provider = OpenAiEmbeddingProvider::new(
            cfg(1).with_max_batch(1).with_concurrency(4),
            Box::new(PartialFailureTransport { fail_at: 5 }),
        )
        .expect("provider builds");
        let texts: Vec<String> = (0..8).map(|n| format!("t{n}")).collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        match provider.embed_batch(&refs) {
            Err(CoreError::Typed { code, .. }) => {
                assert_eq!(
                    code,
                    quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                        LexicalErrorCode::SemProviderTransport
                    ),
                    "one failing batch among successes must fail the whole call"
                );
            }
            other => panic!("partial failure must fail closed, got {other:?}"),
        }
    }

    #[test]
    fn empty_input_makes_no_calls() {
        let transport = ScriptedTransport::new(Vec::new());
        let calls = transport.calls_handle();
        let provider = provider_with(cfg(2), transport);
        let vectors = provider.embed_batch(&[]).expect("empty ok");
        assert!(vectors.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "empty input must make no HTTP calls");
    }

    #[test]
    fn auth_failure_fails_closed_without_retry() {
        // max_retries=3, but 401 must NOT retry -> exactly one transport call.
        let transport = ScriptedTransport::new(vec![Ok(HttpResponse {
            status: 401,
            body: "{\"error\":\"bad key\"}".to_string(),
        })]);
        let calls = transport.calls_handle();
        let provider = provider_with(cfg(2).with_max_retries(3), transport);
        match provider.embed_batch(&["a"]) {
            Err(CoreError::Typed { code, .. }) => {
                assert_eq!(
                    code,
                    quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                        LexicalErrorCode::SemProviderAuth
                    )
                );
            }
            other => panic!("auth must fail closed with SEM_PROVIDER_AUTH, got {other:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "auth must not retry");
    }

    #[test]
    fn retryable_then_success_recovers() {
        let transport = ScriptedTransport::new(vec![
            Ok(HttpResponse {
                status: 429,
                body: "rate limited".to_string(),
            }),
            Ok(ok_body(&[(0, vec![0.5, 0.5])])),
        ]);
        let calls = transport.calls_handle();
        let provider = provider_with(cfg(2).with_max_retries(2), transport);
        let vectors = provider
            .embed_batch(&["a"])
            .expect("recovers after one 429");
        assert_eq!(vectors, vec![vec![0.5, 0.5]]);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one 429 then success = two calls");
    }

    #[test]
    fn exhausted_retries_fail_closed_as_transport() {
        // KNOB PROOF: max_retries=1 -> exactly 2 transport attempts (1 + 1 retry).
        let transport = ScriptedTransport::new(vec![
            Ok(HttpResponse {
                status: 503,
                body: "x".to_string(),
            }),
            Ok(HttpResponse {
                status: 503,
                body: "x".to_string(),
            }),
        ]);
        let calls = transport.calls_handle();
        let provider = provider_with(cfg(2).with_max_retries(1), transport);
        match provider.embed_batch(&["a"]) {
            Err(CoreError::Typed { code, .. }) => {
                assert_eq!(
                    code,
                    quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                        LexicalErrorCode::SemProviderTransport
                    )
                );
            }
            other => panic!("exhausted retries must be SEM_PROVIDER_TRANSPORT, got {other:?}"),
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "max_retries=1 must attempt exactly 1 + 1 retry"
        );
    }

    #[test]
    fn max_retries_zero_disables_retry() {
        // KNOB PROOF: max_retries=0 -> exactly one attempt, no retry.
        let transport = ScriptedTransport::new(vec![Ok(HttpResponse {
            status: 503,
            body: "x".to_string(),
        })]);
        let calls = transport.calls_handle();
        let provider = provider_with(cfg(2).with_max_retries(0), transport);
        assert!(provider.embed_batch(&["a"]).is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "max_retries=0 must not retry");
    }

    #[test]
    fn dimension_mismatch_in_response_fails_closed() {
        let transport = ScriptedTransport::new(vec![Ok(ok_body(&[(0, vec![1.0, 2.0, 3.0])]))]);
        let provider = provider_with(cfg(2), transport); // configured dim 2, response dim 3
        match provider.embed_batch(&["a"]) {
            Err(CoreError::Storage(message)) => assert!(message.contains("embedding dim")),
            other => panic!("dim mismatch must fail closed, got {other:?}"),
        }
    }

    #[test]
    fn max_batch_knob_controls_request_splitting() {
        // KNOB PROOF: max_batch=1 over 2 inputs -> exactly 2 HTTP requests; the
        // same inputs with the default (256) batch -> 1 request.
        //
        // The scripted transport answers in script order, not per input, so
        // the batches must be issued by one worker for the vectors to line up
        // with the inputs; with the default concurrency two workers race for
        // the two responses and the assertion flips at random.
        let split_transport = ScriptedTransport::new(vec![
            Ok(ok_body(&[(0, vec![1.0, 0.0])])),
            Ok(ok_body(&[(0, vec![0.0, 1.0])])),
        ]);
        let split_calls = split_transport.calls_handle();
        let split = provider_with(cfg(2).with_max_batch(1).with_concurrency(1), split_transport);
        let vectors = split.embed_batch(&["a", "b"]).expect("batched ok");
        assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        assert_eq!(split_calls.load(Ordering::SeqCst), 2, "max_batch=1 -> 2 requests");

        let one_transport =
            ScriptedTransport::new(vec![Ok(ok_body(&[(0, vec![1.0, 0.0]), (1, vec![0.0, 1.0])]))]);
        let one_calls = one_transport.calls_handle();
        let one = provider_with(cfg(2), one_transport); // default max_batch (256)
        let _vectors = one.embed_batch(&["a", "b"]).expect("single batch ok");
        assert_eq!(one_calls.load(Ordering::SeqCst), 1, "default batch -> 1 request");
    }

    #[test]
    fn estimated_token_budget_splits_large_requests_even_when_count_limit_allows_one() {
        let transport = ScriptedTransport::new(vec![
            Ok(ok_body(&[(0, vec![1.0, 0.0])])),
            Ok(ok_body(&[(0, vec![0.0, 1.0])])),
        ]);
        let calls = transport.calls_handle();
        let provider = provider_with(cfg(2).with_max_estimated_tokens_per_request(3), transport);
        let _vectors = provider
            .embed_batch(&["abcdef", "ghijkl"])
            .expect("token-budget split ok");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "two long texts must split once the estimated-token budget is exceeded"
        );
    }

    #[test]
    fn oversized_single_text_is_sent_as_a_singleton_batch() {
        let transport = ScriptedTransport::new(vec![Ok(ok_body(&[(0, vec![1.0, 0.0])]))]);
        let calls = transport.calls_handle();
        let provider = provider_with(cfg(2).with_max_estimated_tokens_per_request(1), transport);
        let _vectors = provider
            .embed_batch(&["this text is larger than the request budget estimate"])
            .expect("oversized singleton still runs");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "an oversized single text must run alone, not fail closed on the estimate"
        );
    }

    #[test]
    fn config_defaults_and_overrides_thread_into_provider() {
        // Defaults from OpenAiProviderConfig::new are what the provider uses, and
        // builder overrides replace them.
        let defaults = cfg(8);
        assert_eq!(defaults.max_batch, DEFAULT_MAX_BATCH);
        assert_eq!(
            defaults.max_estimated_tokens_per_request,
            DEFAULT_MAX_ESTIMATED_TOKENS_PER_REQUEST
        );
        assert_eq!(defaults.max_retries, DEFAULT_MAX_RETRIES);
        assert_eq!(defaults.timeout, DEFAULT_TIMEOUT);
        let tuned = cfg(8)
            .with_max_batch(3)
            .with_max_estimated_tokens_per_request(9)
            .with_max_retries(7)
            .with_timeout(Duration::from_secs(5));
        assert_eq!(tuned.max_batch, 3);
        assert_eq!(tuned.max_estimated_tokens_per_request, 9);
        assert_eq!(tuned.max_retries, 7);
        assert_eq!(tuned.timeout, Duration::from_secs(5));
    }

    /// A zero batch, token budget or concurrency, or a concurrency or
    /// dimension past the ceiling, is a configuration defect refused at
    /// construction — never silently clamped into a working value.
    #[test]
    fn out_of_range_tuning_is_refused_at_construction() {
        fn stub() -> Box<dyn EmbeddingTransport> {
            Box::new(ScriptedTransport::new(Vec::new()))
        }
        assert!(OpenAiEmbeddingProvider::new(cfg(8).with_max_batch(0), stub()).is_err());
        assert!(
            OpenAiEmbeddingProvider::new(cfg(8).with_max_estimated_tokens_per_request(0), stub())
                .is_err()
        );
        assert!(OpenAiEmbeddingProvider::new(cfg(8).with_concurrency(0), stub()).is_err());
        assert!(
            OpenAiEmbeddingProvider::new(cfg(8).with_concurrency(MAX_CONCURRENCY + 1), stub())
                .is_err()
        );
        assert!(
            OpenAiEmbeddingProvider::new(cfg(8).with_concurrency(MAX_CONCURRENCY), stub()).is_ok()
        );
        assert!(OpenAiEmbeddingProvider::new(cfg(0), stub()).is_err());
        assert!(OpenAiEmbeddingProvider::new(cfg(MAX_EMBEDDING_DIMENSION + 1), stub()).is_err());
        assert!(OpenAiEmbeddingProvider::new(cfg(MAX_EMBEDDING_DIMENSION), stub()).is_ok());
    }

    #[test]
    fn reqwest_transport_retains_its_configured_timeout() {
        // The transport builds its reqwest client with exactly the timeout it is
        // handed and exposes it (reqwest hides its own). A non-default custom value
        // guards against a vacuous pass against DEFAULT_TIMEOUT.
        let custom = Duration::from_millis(4321);
        assert_ne!(custom, DEFAULT_TIMEOUT);
        let transport =
            ReqwestBlockingTransport::new(custom).expect("transport builds with a valid timeout");
        assert_eq!(
            transport.configured_timeout(),
            Some(custom),
            "transport must build its client with exactly the timeout it is handed"
        );
    }

    #[test]
    fn with_reqwest_threads_config_timeout_into_transport() {
        // Closes the end-to-end wire: `with_reqwest(config)` must build the real
        // transport with `config.timeout`, not a hardcoded/default. If the single
        // `ReqwestBlockingTransport::new(config.timeout)` line dropped config.timeout
        // (e.g. used DEFAULT_TIMEOUT), this fails. Custom value differs from default.
        let custom = Duration::from_millis(7654);
        assert_ne!(custom, DEFAULT_TIMEOUT);
        let provider = OpenAiEmbeddingProvider::with_reqwest(cfg(8).with_timeout(custom))
            .expect("provider builds over the real transport");
        assert_eq!(
            provider.transport_configured_timeout(),
            Some(custom),
            "with_reqwest must thread config.timeout all the way into the transport"
        );
    }

    fn dot(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
    }

    // Real-API proof of genuine SEMANTIC relatedness (synonym closer than an
    // unrelated word) — impossible with the FNV-1a token-hash embedder. Gated:
    // only runs with `--ignored` and a real OPENAI_API_KEY.
    #[test]
    #[ignore = "hits the real OpenAI API; run with OPENAI_API_KEY set and --ignored"]
    fn openai_real_semantic_relatedness_synonym_beats_unrelated_v1() {
        let api_key = match std::env::var("OPENAI_API_KEY") {
            Ok(key) if !key.trim().is_empty() => key,
            _ => return,
        };
        let provider = OpenAiEmbeddingProvider::with_reqwest(OpenAiProviderConfig::new(
            api_key,
            "text-embedding-3-small".to_string(),
            "live".to_string(),
            1536,
        ))
        .expect("provider builds with a real key");
        let vectors = provider
            .embed_batch(&["car", "automobile", "banana"])
            .expect("real embeddings");
        assert_eq!(vectors.len(), 3);
        let car = vectors.first().expect("car vector");
        let automobile = vectors.get(1).expect("automobile vector");
        let banana = vectors.get(2).expect("banana vector");
        let synonym_sim = dot(car, automobile);
        let unrelated_sim = dot(car, banana);
        assert!(
            synonym_sim > unrelated_sim,
            "car~automobile ({synonym_sim}) must exceed car~banana ({unrelated_sim}) — real semantic relatedness"
        );
    }

    #[test]
    fn empty_api_key_is_rejected_at_construction() {
        let result = OpenAiEmbeddingProvider::new(
            OpenAiProviderConfig::new(String::new(), "m".to_string(), "r1".to_string(), 2),
            Box::new(ScriptedTransport::new(Vec::new())),
        );
        match result {
            Err(CoreError::Typed { code, .. }) => {
                assert_eq!(
                    code,
                    quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                        LexicalErrorCode::SemProviderAuth
                    )
                );
            }
            Err(other) => panic!("empty key wrong error variant: {other:?}"),
            Ok(_) => panic!("empty key must be rejected at construction"),
        }
    }

    /// A transport whose attempt parks until the test releases it, records
    /// the timeout it was handed, and counts its calls — the fault injection
    /// for the budget proofs (QI-BB-002).
    struct ParkedTransport {
        entered: std::sync::mpsc::Sender<Duration>,
        release: Arc<std::sync::Barrier>,
        calls: Arc<AtomicUsize>,
    }

    impl EmbeddingTransport for ParkedTransport {
        fn post_embeddings(
            &self,
            _url: &str,
            _api_key: &str,
            _body: &str,
            timeout: Duration,
        ) -> Result<HttpResponse, CoreError> {
            let _prior = self.calls.fetch_add(1, Ordering::SeqCst);
            let _told = self.entered.send(timeout);
            let _released = self.release.wait();
            Ok(ok_body(&[(0, vec![0.5, 0.5])]))
        }
    }

    fn parked_provider(
        retries: u32,
        timeout: Duration,
    ) -> (
        OpenAiEmbeddingProvider,
        std::sync::mpsc::Receiver<Duration>,
        Arc<std::sync::Barrier>,
        Arc<AtomicUsize>,
    ) {
        let (entered, entered_rx) = std::sync::mpsc::channel();
        let release = Arc::new(std::sync::Barrier::new(2));
        let calls = Arc::new(AtomicUsize::new(0));
        let transport = ParkedTransport {
            entered,
            release: Arc::clone(&release),
            calls: Arc::clone(&calls),
        };
        let provider =
            provider_with(cfg(2).with_max_retries(retries).with_timeout(timeout), transport);
        (provider, entered_rx, release, calls)
    }

    /// A cancelled budget abandons the attempt in flight.
    ///
    /// The call returns `REQUEST_CANCELLED` at the `semantic:embed`
    /// checkpoint while the transport is still parked, and the attempt
    /// finishes on its own afterwards without a second call.
    #[test]
    fn a_cancelled_budget_abandons_the_attempt_in_flight() {
        let (provider, entered, release, calls) = parked_provider(3, Duration::from_secs(60));
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let cancel = budget.cancel_handle();
        let canceller = std::thread::spawn(move || {
            let _timeout = entered
                .recv_timeout(Duration::from_secs(10))
                .expect("the attempt started");
            cancel.cancel();
        });
        let outcome = provider.embed_batch_within(&["a"], &budget);
        match outcome {
            Err(CoreError::Typed { code, message }) => {
                assert_eq!(code, quanta_index_core::REQUEST_CANCELLED_CODE);
                assert!(message.contains("checkpoint `semantic:embed`"), "{message}");
            }
            other => panic!("a cancelled attempt answers typed, got {other:?}"),
        }
        canceller.join().expect("canceller thread");
        // The attempt is still parked: the provider did not wait for it.
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let _released = release.wait();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "no retry after a cancellation");
    }

    /// Every attempt's HTTP timeout is capped by what the budget has left.
    #[test]
    fn an_attempts_timeout_is_capped_by_the_budget_remainder() {
        let (provider, entered, release, _calls) = parked_provider(0, Duration::from_secs(60));
        let budget = RequestBudgetV1::for_duration(Duration::from_millis(400));
        let releaser = std::thread::spawn(move || {
            let told = entered
                .recv_timeout(Duration::from_secs(10))
                .expect("the attempt started");
            let _released = release.wait();
            told
        });
        let vectors = provider
            .embed_batch_within(&["a"], &budget)
            .expect("released attempt succeeds");
        assert_eq!(vectors, vec![vec![0.5, 0.5]]);
        let told = releaser.join().expect("releaser thread");
        assert!(
            told <= Duration::from_millis(400),
            "the attempt was told the budget remainder, not the configured minute: {told:?}"
        );
        assert!(!told.is_zero());
    }

    /// An expired budget between attempts stops the retry loop typed
    /// instead of sleeping through the backoff and calling again.
    #[test]
    fn a_deadline_during_backoff_stops_the_retry_loop_typed() {
        let transport = ScriptedTransport::new(vec![
            Ok(HttpResponse {
                status: 503,
                body: "unavailable".to_string(),
            }),
            Ok(ok_body(&[(0, vec![0.5, 0.5])])),
        ]);
        let calls = transport.calls_handle();
        let provider = provider_with(cfg(2).with_max_retries(3), transport);
        // The budget expires during the first backoff (≤ 250 ms).
        let budget = RequestBudgetV1::for_duration(Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        match provider.embed_batch_within(&["a"], &budget) {
            Err(CoreError::Typed { code, message }) => {
                assert_eq!(code, quanta_index_core::REQUEST_DEADLINE_EXCEEDED_CODE);
                assert!(message.contains("checkpoint `semantic:embed`"), "{message}");
            }
            other => panic!("an expired budget answers typed, got {other:?}"),
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "an already-expired budget makes no attempt at all"
        );
    }
}
