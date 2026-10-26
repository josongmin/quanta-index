use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use quanta_index_contract::EmbeddingNormalization;
use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_core::{
    CoreError, EMBED_CHECKPOINT, MAX_EMBEDDING_DIMENSION, RequestBudgetV1, TextEmbeddingProvider,
    index::{MemoryLease, MemoryLifetime},
};
use serde::{
    Serialize,
    de::{DeserializeSeed, Error as _, IgnoredAny, MapAccess, SeqAccess, Visitor},
};

use crate::pool::ProviderAttemptPool;
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

    /// Native calls transfer the owned body and an explicit decoded-body
    /// ceiling. A transport must implement the bounded read itself; checking
    /// length after an unbounded read cannot enforce physical memory admission.
    /// Only unbounded standalone calls may use the borrowed fallback.
    fn post_embeddings_owned_bounded(
        &self,
        url: &str,
        api_key: &str,
        body: String,
        timeout: Duration,
        max_response_bytes: Option<u64>,
    ) -> Result<HttpResponse, CoreError> {
        if max_response_bytes.is_some() {
            return Err(CoreError::InvalidContract(
                "embedding transport does not implement bounded response reading".into(),
            ));
        }
        self.post_embeddings(url, api_key, &body, timeout)
    }

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

struct ChargedResponse {
    response: HttpResponse,
    memory: Option<MemoryLease>,
}

struct ChargedBody {
    body: String,
    _memory: Option<MemoryLease>,
}

#[derive(Default)]
struct ByteCounter(u64);

impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let length = u64::try_from(bytes.len())
            .map_err(|_error| std::io::Error::other("serialized provider request is too large"))?;
        self.0 = self
            .0
            .checked_add(length)
            .ok_or_else(|| std::io::Error::other("serialized provider request is too large"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Budget-aware backoff sleep for the retry loop (TOPT-04 / R2).
///
/// Production sleeps in [`BUDGET_POLL_INTERVAL`] slices so a cancelled or
/// expired budget interrupts the backoff typed; unit tests inject a
/// recorder/zero sleeper and prove the retry schedule with no wall-clock
/// wait. The trait is provider-local: it changes no public constructor and
/// no production delay distribution.
trait RetrySleeper: Send + Sync {
    fn sleep_backoff(&self, delay: Duration, budget: &RequestBudgetV1) -> Result<(), CoreError>;
}

/// The production sleeper: [`sleep_within_budget`] semantics under testable
/// ownership.
struct BudgetPollSleeper;

impl RetrySleeper for BudgetPollSleeper {
    fn sleep_backoff(&self, delay: Duration, budget: &RequestBudgetV1) -> Result<(), CoreError> {
        sleep_within_budget(delay, budget)
    }
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
    /// Decoded response-body ceiling. Native product composition supplies
    /// this explicitly; standalone injected transports may leave it unset.
    pub max_response_bytes: Option<u64>,
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
            max_response_bytes: None,
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

    /// Exact maximum decoded HTTP response bytes for one attempt.
    #[must_use]
    pub const fn with_max_response_bytes(mut self, bytes: u64) -> Self {
        self.max_response_bytes = Some(bytes);
        self
    }

    /// Logical admission envelope for one source or query text.
    ///
    /// `max_input_bytes` is the source/query owner bound. JSON can escape one
    /// input byte as six ASCII bytes (`\u0000`). The fixed request frame and
    /// model name are counted by the same serializer as the actual request.
    /// The peak includes both owned JSON/URL copies, one reserved response,
    /// and the parser's bounded vector/metadata/scratch charge. `escaping`
    /// is the simultaneous retained charge while response and parse state
    /// overlap. Allocator overcapacity and reqwest internal buffers are not
    /// measured by this logical envelope.
    pub fn single_input_memory_envelope(
        &self,
        max_input_bytes: u64,
    ) -> Result<(u64, u64), CoreError> {
        let response = self
            .max_response_bytes
            .filter(|bytes| *bytes != 0)
            .ok_or_else(|| {
                invalid("openai: single-input memory envelope needs a response byte ceiling")
            })?;
        if self.dimension == 0 || self.dimension > MAX_EMBEDDING_DIMENSION {
            return Err(invalid(
                "openai: single-input memory envelope has invalid dimension",
            ));
        }
        let empty = [""];
        let request = EmbeddingsRequest {
            model: &self.model,
            input: &empty,
            dimensions: self.dimension,
        };
        let mut frame = ByteCounter::default();
        serde_json::to_writer(&mut frame, &request)
            .map_err(|error| invalid(&format!("openai: request frame count failed: {error}")))?;
        let escaped_input = max_input_bytes
            .checked_mul(6)
            .ok_or_else(|| invalid("openai: escaped input bound overflows"))?;
        let body = frame
            .0
            .checked_add(escaped_input)
            .ok_or_else(|| invalid("openai: serialized request bound overflows"))?;
        let url = self
            .base_url
            .len()
            .checked_add(EMBEDDINGS_PATH.len())
            .ok_or_else(|| invalid("openai: URL bound overflows"))?;
        let url = u64::try_from(url)
            .map_err(|_error| invalid("openai: URL bound exceeds platform representation"))?;
        let output = quanta_index_core::TextEmbeddingBatch::output_bytes(1, self.dimension)?;
        let metadata = u64::try_from(std::mem::size_of::<EmbeddingItem>())
            .map_err(|_error| invalid("openai: metadata bound exceeds platform representation"))?;
        let first_peak = body
            .checked_mul(2)
            .and_then(|bytes| bytes.checked_add(url.checked_mul(2)?))
            .and_then(|bytes| bytes.checked_add(response))
            .and_then(|bytes| bytes.checked_add(output))
            .ok_or_else(|| invalid("openai: attempt memory peak overflows"))?;
        let escaping = response
            .checked_add(output)
            .ok_or_else(|| invalid("openai: retained output envelope overflows"))?;
        let parse_peak = body
            .checked_add(url)
            .and_then(|bytes| bytes.checked_add(escaping))
            .and_then(|bytes| bytes.checked_add(metadata))
            .and_then(|bytes| bytes.checked_add(response))
            .ok_or_else(|| invalid("openai: parse memory peak overflows"))?;
        Ok((first_peak.max(parse_peak), escaping))
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
    /// The supervisor-owned attempt pool (S21-09): every attempt thread
    /// spawns through it, so no per-request thread is ever detached.
    attempts: Arc<ProviderAttemptPool>,
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
    max_response_bytes: Option<u64>,
    /// Native composition requires one actual admitted physical lease for
    /// every HTTP attempt. Standalone callers keep the explicit off-native
    /// budget contract.
    required_execution_custody: bool,
    retry_sleeper: Arc<dyn RetrySleeper>,
}

impl OpenAiEmbeddingProvider {
    /// Build a provider over an explicit transport (the real one or a test stub).
    pub fn new(
        config: OpenAiProviderConfig,
        transport: Box<dyn EmbeddingTransport>,
    ) -> Result<Self, CoreError> {
        if config.api_key.trim().is_empty() {
            return Err(typed(
                LexicalErrorCode::SemProviderAuth,
                "openai: API key is empty",
            ));
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
            return Err(invalid(
                "openai: max_estimated_tokens_per_request must be at least 1",
            ));
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
        if config
            .max_response_bytes
            .is_some_and(|bytes| bytes == 0 || usize::try_from(bytes).is_err() || bytes == u64::MAX)
        {
            return Err(invalid(
                "openai: max response bytes must fit a nonzero platform buffer",
            ));
        }
        let model_id = format!("openai:{}", config.model);
        // Concurrency was validated above, so the pool cap cannot fail;
        // the composition root swaps in the supervised pool anyway.
        let attempts = ProviderAttemptPool::new(config.concurrency)
            .map_err(|err| invalid(&format!("openai: {err:?}")))?;
        Ok(Self {
            transport: Arc::from(transport),
            attempts: Arc::new(attempts),
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
            max_response_bytes: config.max_response_bytes,
            required_execution_custody: false,
            retry_sleeper: Arc::new(BudgetPollSleeper),
        })
    }

    /// Swap in the supervisor-owned attempt pool (S21-09). The pool the
    /// runtime passes here is the same one the supervised drain joins, so
    /// attempts stay owned from spawn to shutdown.
    #[must_use]
    pub fn with_attempt_pool(mut self, attempts: Arc<ProviderAttemptPool>) -> Self {
        self.attempts = attempts;
        self
    }

    /// Require a Native execution grant at the physical HTTP boundary.
    /// Product composition enables this before registering the provider so
    /// a missing or zero-provider reservation cannot run uncharged remote IO.
    #[must_use]
    pub fn with_required_execution_custody(mut self) -> Self {
        self.required_execution_custody = true;
        self
    }

    /// Attempts currently tracked by this provider's pool, for gauges
    /// and tests.
    #[must_use]
    pub fn live_attempts(&self) -> usize {
        self.attempts.live_attempts()
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

    /// Swap the backoff sleeper. Test-only: production always sleeps in
    /// budget-poll slices via [`BudgetPollSleeper`].
    #[cfg(test)]
    fn with_retry_sleeper(mut self, sleeper: Arc<dyn RetrySleeper>) -> Self {
        self.retry_sleeper = sleeper;
        self
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
        let mut count = ByteCounter::default();
        serde_json::to_writer(&mut count, &request)
            .map_err(|err| invalid(&format!("openai: request size failed: {err}")))?;
        let url_bytes = self
            .base_url
            .len()
            .checked_add(EMBEDDINGS_PATH.len())
            .ok_or_else(|| invalid("openai: request URL size overflows"))?;
        let url_bytes = u64::try_from(url_bytes).map_err(|_error| {
            invalid("openai: request URL size exceeds platform representation")
        })?;
        let request_bytes = count
            .0
            .checked_add(url_bytes)
            .ok_or_else(|| invalid("openai: request allocation size overflows"))?;
        let _body_memory = reserve_provider_memory(
            budget,
            request_bytes,
            MemoryLifetime::Temporary,
            self.required_execution_custody,
        )?;
        let capacity = usize::try_from(count.0)
            .map_err(|_error| invalid("openai: request body exceeds platform capacity"))?;
        let mut encoded = Vec::new();
        encoded.try_reserve_exact(capacity).map_err(|error| {
            CoreError::Storage(format!("openai: request reserve failed: {error}"))
        })?;
        serde_json::to_writer(&mut encoded, &request)
            .map_err(|err| invalid(&format!("openai: request encode failed: {err}")))?;
        if encoded.len() != capacity {
            return Err(invalid("openai: counted and encoded request sizes differ"));
        }
        let body = String::from_utf8(encoded)
            .map_err(|error| invalid(&format!("openai: request UTF-8 failed: {error}")))?;
        let url = format!("{}{EMBEDDINGS_PATH}", self.base_url);
        let raw = self.post_with_retry(&url, &body, texts.len(), estimated_tokens, budget)?;
        let per_item_metadata = std::mem::size_of::<EmbeddingItem>();
        let metadata_bytes = texts
            .len()
            .checked_mul(per_item_metadata)
            .ok_or_else(|| invalid("openai: decoded vector metadata footprint overflows"))?;
        let parser_scratch = usize::try_from(self.max_response_bytes.unwrap_or(0))
            .map_err(|_error| invalid("openai: parser scratch exceeds platform capacity"))?;
        let decoded_bytes = metadata_bytes
            .checked_add(parser_scratch)
            .ok_or_else(|| invalid("openai: decoded parser footprint overflows"))?;
        let decoded_bytes = u64::try_from(decoded_bytes).map_err(|_error| {
            invalid("openai: decoded parser footprint exceeds platform representation")
        })?;
        let _decoded_memory = reserve_provider_memory(
            budget,
            decoded_bytes,
            MemoryLifetime::Temporary,
            self.required_execution_custody,
        )?;
        let parsed = parse_embeddings_response(&raw.body, texts.len(), self.dimension)?;
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
    ) -> Result<ChargedBody, CoreError> {
        let mut last_error: Option<CoreError> = None;
        for attempt in 0..=self.max_retries {
            budget.checkpoint(EMBED_CHECKPOINT)?;
            telemetry::record_http_request(texts_in_request, estimated_tokens);
            match self.post_under_budget(url, body, budget) {
                Ok(charged) => match classify_status(charged.response.status) {
                    StatusClass::Success => {
                        return Ok(ChargedBody {
                            body: charged.response.body,
                            _memory: charged.memory,
                        });
                    }
                    StatusClass::Auth => {
                        return Err(typed(
                            LexicalErrorCode::SemProviderAuth,
                            &format!("openai: auth rejected (status {})", charged.response.status),
                        ));
                    }
                    StatusClass::Fatal => {
                        return Err(typed(
                            LexicalErrorCode::SemProviderTransport,
                            &format!(
                                "openai: non-retryable status {} body {}",
                                charged.response.status,
                                preview(&charged.response.body)
                            ),
                        ));
                    }
                    StatusClass::Retryable => {
                        telemetry::record_retryable_status();
                        last_error = Some(typed(
                            LexicalErrorCode::SemProviderTransport,
                            &format!("openai: retryable status {}", charged.response.status),
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
                self.retry_sleeper
                    .sleep_backoff(backoff_delay(attempt), budget)?;
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
    ) -> Result<ChargedResponse, CoreError> {
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
        // The request may return before a blocking HTTP transport does. This
        // finite lease retains its original Native admission on the physical
        // thread, without extending the caller's deadline or cancellation.
        let custody = budget.begin_provider_attempt_v1()?;
        if self.required_execution_custody && custody.is_none() {
            return Err(CoreError::InvalidContract(
                "openai: physical attempt lacks an admitted execution grant".into(),
            ));
        }
        if self.required_execution_custody && self.max_response_bytes.is_none() {
            return Err(invalid(
                "openai: native response byte ceiling is not configured",
            ));
        }
        let copy_bytes = body
            .len()
            .checked_add(url.len())
            .ok_or_else(|| invalid("openai: physical request copy size overflows"))?;
        let copy_bytes = u64::try_from(copy_bytes).map_err(|_error| {
            invalid("openai: physical request copy exceeds platform representation")
        })?;
        let request_copy_memory = reserve_provider_memory(
            budget,
            copy_bytes,
            MemoryLifetime::Temporary,
            self.required_execution_custody,
        )?;
        let response_memory = self
            .max_response_bytes
            .map(|maximum| {
                reserve_provider_memory(
                    budget,
                    maximum,
                    MemoryLifetime::Retained,
                    self.required_execution_custody,
                )
            })
            .transpose()?
            .flatten();
        let transport = Arc::clone(&self.transport);
        let api_key = Arc::clone(&self.api_key);
        let max_response_bytes = self.max_response_bytes;
        let url = url.to_string();
        let body = body.to_string();
        let (done, outcome) = mpsc::channel();
        // Tracked, never detached (S21-09): the pool bounds inflight
        // attempts, refuses past shutdown, and owns the thread until it
        // ends or the supervised drain escalates it by name.
        self.attempts
            .spawn_tracked_with_custody("openai-embed-attempt", custody, move || {
                let result = transport
                    .post_embeddings_owned_bounded(
                        &url,
                        &api_key,
                        body,
                        timeout,
                        max_response_bytes,
                    )
                    .and_then(|response| {
                        let response_len = u64::try_from(response.body.len()).map_err(|_error| {
                            CoreError::Typed {
                                code: quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
                                message: "openai: decoded response length exceeds platform representation".into(),
                            }
                        })?;
                        if max_response_bytes.is_some_and(|maximum| response_len > maximum) {
                            return Err(CoreError::Typed {
                                code:
                                    quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
                                message: "openai: decoded response exceeds configured byte ceiling"
                                    .into(),
                            });
                        }
                        Ok(ChargedResponse {
                            response,
                            memory: response_memory,
                        })
                    });
                // The HTTP call has ended. Release the physical request copy
                // before waking the parser, so its decoded-buffer admission
                // cannot overlap with the old copy's live charge.
                drop(url);
                drop(request_copy_memory);
                // The requester may have left: a failed send means exactly
                // that, and the result is dropped with the receiver.
                let _abandoned = done.send(result);
            })?;
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
    let mut out: Vec<Vec<f32>> = Vec::new();
    out.try_reserve_exact(text_count)
        .map_err(|error| invalid(&format!("openai: output reserve failed: {error}")))?;
    for result in ordered {
        let batch = result?;
        if out
            .len()
            .checked_add(batch.len())
            .is_none_or(|length| length > text_count)
        {
            return Err(invalid("openai: output batch exceeds input cardinality"));
        }
        out.extend(batch);
    }
    if out.len() != text_count {
        return Err(invalid("openai: output batch omits an input"));
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
    fn embed_batch(
        &self,
        texts: &[&str],
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        self.embed_batch_within(texts, &RequestBudgetV1::unbounded())
    }

    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<quanta_index_core::TextEmbeddingBatch, CoreError> {
        budget.checkpoint(EMBED_CHECKPOINT)?;
        if texts.is_empty() {
            return Ok(quanta_index_core::TextEmbeddingBatch::unscoped(Vec::new()));
        }
        let memory = reserve_provider_memory(
            budget,
            quanta_index_core::TextEmbeddingBatch::output_bytes(texts.len(), self.dimension)?,
            MemoryLifetime::Retained,
            self.required_execution_custody,
        )?;
        let batches =
            partition_request_batches(texts, self.max_batch, self.max_estimated_tokens_per_request);
        // Common per-call case (one batch) or concurrency disabled: stay sequential
        // with zero thread/coordination overhead.
        let vectors = if batches.len() <= 1 || self.concurrency <= 1 {
            flatten_batch_results(
                batches.iter().map(|batch| {
                    request_batch_texts(texts, *batch).and_then(|batch_texts| {
                        self.embed_one_batch(batch_texts, batch.estimated_tokens, budget)
                    })
                }),
                texts.len(),
            )?
        } else {
            self.embed_batches_concurrently(texts, &batches, budget)?
        };
        budget.checkpoint(EMBED_CHECKPOINT)?;
        Ok(quanta_index_core::TextEmbeddingBatch::from_parts(
            vectors, memory,
        ))
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
            .redirect(reqwest::redirect::Policy::none())
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

    fn post_owned(
        &self,
        url: &str,
        api_key: &str,
        body: String,
        timeout: Duration,
        max_response_bytes: Option<u64>,
    ) -> Result<HttpResponse, CoreError> {
        let response = self
            .client
            .post(url)
            .bearer_auth(api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .timeout(timeout)
            .send()
            .map_err(|err| {
                typed(
                    LexicalErrorCode::SemProviderTransport,
                    &format!("openai: request send failed: {err}"),
                )
            })?;
        let status = response.status().as_u16();
        let text = read_response_body(response, max_response_bytes)?;
        Ok(HttpResponse { status, body: text })
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
        self.post_owned(url, api_key, body.to_string(), timeout, None)
    }

    fn post_embeddings_owned_bounded(
        &self,
        url: &str,
        api_key: &str,
        body: String,
        timeout: Duration,
        max_response_bytes: Option<u64>,
    ) -> Result<HttpResponse, CoreError> {
        self.post_owned(url, api_key, body, timeout, max_response_bytes)
    }
}

fn read_response_body(
    mut response: impl Read,
    max_response_bytes: Option<u64>,
) -> Result<String, CoreError> {
    let mut bytes = Vec::new();
    match max_response_bytes {
        Some(maximum) => {
            if maximum == 0 {
                return Err(invalid("openai: response ceiling must be nonzero"));
            }
            let capacity = usize::try_from(maximum)
                .map_err(|_error| invalid("openai: response ceiling exceeds platform capacity"))?;
            bytes.try_reserve_exact(capacity).map_err(|error| {
                CoreError::Storage(format!("openai: response buffer reserve failed: {error}"))
            })?;
            let mut chunk = [0_u8; 8192];
            while bytes.len() < capacity {
                let remaining = capacity
                    .checked_sub(bytes.len())
                    .ok_or_else(|| invalid("openai: response buffer exceeded ceiling"))?;
                let requested = remaining.min(chunk.len());
                let read = loop {
                    let buffer = chunk.get_mut(..requested).ok_or_else(|| {
                        invalid("openai: requested response chunk exceeds buffer")
                    })?;
                    match response.read(buffer) {
                        Ok(read) => break read,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => {
                            return Err(typed(
                                LexicalErrorCode::SemProviderTransport,
                                &format!("openai: response body read failed: {error}"),
                            ));
                        }
                    }
                };
                if read == 0 {
                    break;
                }
                if read > requested {
                    return Err(invalid("openai: response reader exceeded supplied buffer"));
                }
                let next_len = bytes
                    .len()
                    .checked_add(read)
                    .ok_or_else(|| invalid("openai: response buffer length overflows"))?;
                if next_len > capacity || next_len > bytes.capacity() {
                    return Err(invalid(
                        "openai: response reader exceeded reserved capacity",
                    ));
                }
                let chunk_bytes = chunk
                    .get(..read)
                    .ok_or_else(|| invalid("openai: response reader exceeded supplied buffer"))?;
                bytes.extend_from_slice(chunk_bytes);
            }
            if bytes.len() == capacity {
                let mut overflow = [0_u8; 1];
                let read = response.read(&mut overflow).map_err(|error| {
                    typed(
                        LexicalErrorCode::SemProviderTransport,
                        &format!("openai: response overflow probe failed: {error}"),
                    )
                })?;
                if read != 0 {
                    return Err(CoreError::Typed {
                        code: quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
                        message: "openai: decoded response exceeds configured byte ceiling".into(),
                    });
                }
            }
        }
        None => {
            let _read_bytes = response.read_to_end(&mut bytes).map_err(|error| {
                typed(
                    LexicalErrorCode::SemProviderTransport,
                    &format!("openai: response body read failed: {error}"),
                )
            })?;
        }
    }
    String::from_utf8(bytes).map_err(|error| {
        typed(
            LexicalErrorCode::SemProviderTransport,
            &format!("openai: response is not UTF-8: {error}"),
        )
    })
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

#[derive(Clone, Copy)]
struct ResponseSeed {
    count: usize,
    dimension: usize,
}

impl<'de> DeserializeSeed<'de> for ResponseSeed {
    type Value = EmbeddingsResponse;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_struct("EmbeddingsResponse", &["data"], ResponseVisitor(self))
    }
}

struct ResponseVisitor(ResponseSeed);

impl<'de> Visitor<'de> for ResponseVisitor {
    type Value = EmbeddingsResponse;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("embedding response object")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let data = sequence
            .next_element_seed(DataSeed(self.0))?
            .ok_or_else(|| A::Error::missing_field("data"))?;
        if sequence.next_element::<IgnoredAny>()?.is_some() {
            return Err(A::Error::custom(
                "embedding response has extra positional fields",
            ));
        }
        Ok(EmbeddingsResponse { data })
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut data = None;
        while let Some(key) = map.next_key::<String>()? {
            if key == "data" {
                if data.is_some() {
                    return Err(A::Error::duplicate_field("data"));
                }
                data = Some(map.next_value_seed(DataSeed(self.0))?);
            } else {
                let _: IgnoredAny = map.next_value()?;
            }
        }
        Ok(EmbeddingsResponse {
            data: data.ok_or_else(|| A::Error::missing_field("data"))?,
        })
    }
}

#[derive(Clone, Copy)]
struct DataSeed(ResponseSeed);

impl<'de> DeserializeSeed<'de> for DataSeed {
    type Value = Vec<EmbeddingItem>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(DataVisitor(self.0))
    }
}

struct DataVisitor(ResponseSeed);

impl<'de> Visitor<'de> for DataVisitor {
    type Value = Vec<EmbeddingItem>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("bounded embedding data array")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut data = Vec::new();
        data.try_reserve_exact(self.0.count)
            .map_err(A::Error::custom)?;
        for index in 0..self.0.count {
            let item = sequence
                .next_element_seed(ItemSeed(self.0.dimension))?
                .ok_or_else(|| A::Error::invalid_length(index, &self))?;
            data.push(item);
        }
        if sequence.next_element::<IgnoredAny>()?.is_some() {
            return Err(A::Error::custom(
                "embedding data count exceeds request batch",
            ));
        }
        Ok(data)
    }
}

#[derive(Clone, Copy)]
struct ItemSeed(usize);

impl<'de> DeserializeSeed<'de> for ItemSeed {
    type Value = EmbeddingItem;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "EmbeddingItem",
            &["embedding", "index"],
            ItemVisitor(self.0),
        )
    }
}

struct ItemVisitor(usize);

impl<'de> Visitor<'de> for ItemVisitor {
    type Value = EmbeddingItem;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("bounded embedding item")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let embedding = sequence
            .next_element_seed(VectorSeed(self.0))?
            .ok_or_else(|| A::Error::missing_field("embedding"))?;
        let index = sequence
            .next_element()?
            .ok_or_else(|| A::Error::missing_field("index"))?;
        if sequence.next_element::<IgnoredAny>()?.is_some() {
            return Err(A::Error::custom(
                "embedding item has extra positional fields",
            ));
        }
        Ok(EmbeddingItem { embedding, index })
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut embedding = None;
        let mut index = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "embedding" => {
                    if embedding.is_some() {
                        return Err(A::Error::duplicate_field("embedding"));
                    }
                    embedding = Some(map.next_value_seed(VectorSeed(self.0))?);
                }
                "index" => {
                    if index.is_some() {
                        return Err(A::Error::duplicate_field("index"));
                    }
                    index = Some(map.next_value()?);
                }
                _ => {
                    let _: IgnoredAny = map.next_value()?;
                }
            }
        }
        Ok(EmbeddingItem {
            embedding: embedding.ok_or_else(|| A::Error::missing_field("embedding"))?,
            index: index.ok_or_else(|| A::Error::missing_field("index"))?,
        })
    }
}

#[derive(Clone, Copy)]
struct VectorSeed(usize);

impl<'de> DeserializeSeed<'de> for VectorSeed {
    type Value = Vec<f32>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(VectorVisitor(self.0))
    }
}

struct VectorVisitor(usize);

impl<'de> Visitor<'de> for VectorVisitor {
    type Value = Vec<f32>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("bounded embedding vector")
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut vector = Vec::new();
        vector.try_reserve_exact(self.0).map_err(A::Error::custom)?;
        for index in 0..self.0 {
            let value = sequence
                .next_element()?
                .ok_or_else(|| A::Error::invalid_length(index, &self))?;
            vector.push(value);
        }
        if sequence.next_element::<IgnoredAny>()?.is_some() {
            return Err(A::Error::custom(
                "embedding dimension exceeds configured dimension",
            ));
        }
        Ok(vector)
    }
}

fn parse_embeddings_response(
    body: &str,
    count: usize,
    dimension: usize,
) -> Result<EmbeddingsResponse, CoreError> {
    let mut deserializer = serde_json::Deserializer::from_str(body);
    let response = ResponseSeed { count, dimension }
        .deserialize(&mut deserializer)
        .map_err(|error| CoreError::Storage(format!("openai: response decode failed: {error}")))?;
    deserializer
        .end()
        .map_err(|error| CoreError::Storage(format!("openai: response trailing data: {error}")))?;
    Ok(response)
}

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

fn reserve_provider_memory(
    budget: &RequestBudgetV1,
    bytes: u64,
    lifetime: MemoryLifetime,
    required: bool,
) -> Result<Option<MemoryLease>, CoreError> {
    let admitted = budget.reserve_provider_memory_v1(bytes, lifetime)?;
    if required && admitted.is_none() {
        return Err(invalid(
            "openai: physical attempt lacks an admitted execution grant",
        ));
    }
    Ok(admitted)
}

#[cfg(test)]
mod tests {
    use super::retry::backoff_bound;
    use super::*;
    use quanta_index_core::{ProviderAttemptCustodyPortV1, ProviderAttemptLeaseV1};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};

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

        fn post_embeddings_owned_bounded(
            &self,
            url: &str,
            api_key: &str,
            body: String,
            timeout: Duration,
            max_response_bytes: Option<u64>,
        ) -> Result<HttpResponse, CoreError> {
            // Fixtures are fixed before dispatch. Production transports must
            // enforce this limit while reading the response stream.
            let response = self.post_embeddings(url, api_key, &body, timeout)?;
            let response_len = u64::try_from(response.body.len())
                .map_err(|_error| invalid("test response length exceeds u64"))?;
            if max_response_bytes.is_some_and(|maximum| response_len > maximum) {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
                    message: "scripted response exceeds configured byte ceiling".into(),
                });
            }
            Ok(response)
        }
    }

    /// A [`RetrySleeper`] that records every proposed backoff.
    ///
    /// It never sleeps: retry tests prove the schedule with zero wall-clock
    /// wait. The budget is still checkpointed so cancellation semantics stay honest.
    struct RecordingSleeper {
        delays: Mutex<Vec<Duration>>,
    }

    impl RecordingSleeper {
        fn new() -> Self {
            Self {
                delays: Mutex::new(Vec::new()),
            }
        }

        fn recorded(&self) -> Vec<Duration> {
            self.delays.lock().expect("test mutex").clone()
        }
    }

    impl RetrySleeper for RecordingSleeper {
        fn sleep_backoff(
            &self,
            delay: Duration,
            budget: &RequestBudgetV1,
        ) -> Result<(), CoreError> {
            self.delays.lock().expect("test mutex").push(delay);
            budget.checkpoint(EMBED_CHECKPOINT)
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
    fn required_execution_custody_refuses_unbound_remote_io() {
        let transport = ScriptedTransport::new(vec![Ok(ok_body(&[(0, vec![0.5, 0.5])]))]);
        let calls = transport.calls_handle();
        let provider = provider_with(cfg(2).with_max_response_bytes(256), transport)
            .with_required_execution_custody();
        let result = provider.embed_batch_within(
            &["a"],
            &RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        assert!(matches!(
            result,
            Err(CoreError::InvalidContract(message))
                if message.contains("lacks an admitted execution grant")
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.live_attempts(), 0);
    }

    #[test]
    fn bounded_transport_default_refuses_before_borrowed_io() {
        struct BorrowedOnlyTransport(Arc<AtomicUsize>);

        impl EmbeddingTransport for BorrowedOnlyTransport {
            fn post_embeddings(
                &self,
                _url: &str,
                _api_key: &str,
                _body: &str,
                _timeout: Duration,
            ) -> Result<HttpResponse, CoreError> {
                let _previous = self.0.fetch_add(1, Ordering::SeqCst);
                Ok(ok_body(&[(0, vec![0.5, 0.5])]))
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let transport = BorrowedOnlyTransport(Arc::clone(&calls));
        let outcome = transport.post_embeddings_owned_bounded(
            "https://example.invalid/v1/embeddings",
            "test-key",
            "{}".into(),
            Duration::from_secs(1),
            Some(256),
        );
        assert!(matches!(
            outcome,
            Err(CoreError::InvalidContract(message))
                if message.contains("does not implement bounded response reading")
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let standalone = transport.post_embeddings_owned_bounded(
            "https://example.invalid/v1/embeddings",
            "test-key",
            "{}".into(),
            Duration::from_secs(1),
            None,
        );
        assert!(matches!(standalone, Ok(HttpResponse { status: 200, .. })));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn bounded_response_reader_refuses_the_first_excess_byte() {
        let mut response = std::io::Cursor::new(vec![b'a'; 100]);
        let result = read_response_body(&mut response, Some(16));
        assert!(matches!(
            result,
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
                ..
            })
        ));
        assert_eq!(response.position(), 17);
    }

    #[test]
    fn bounded_response_reader_accepts_exact_limit() {
        let response = std::io::Cursor::new(b"bounded".to_vec());
        let result = read_response_body(response, Some(7));
        assert!(matches!(result, Ok(body) if body == "bounded"));
    }

    #[test]
    fn bounded_response_reader_handles_short_chunks_without_capacity_growth() {
        struct OneByteReader<'a>(&'a [u8]);

        impl Read for OneByteReader<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let count = self.0.len().min(buf.len()).min(1);
                let (head, tail) = self
                    .0
                    .split_at_checked(count)
                    .ok_or_else(|| std::io::Error::other("invalid short-reader source range"))?;
                buf.get_mut(..count)
                    .ok_or_else(|| std::io::Error::other("invalid short-reader output range"))?
                    .copy_from_slice(head);
                self.0 = tail;
                Ok(count)
            }
        }

        let short = read_response_body(OneByteReader(b"short"), Some(64));
        assert!(matches!(short, Ok(body) if body == "short"));
        let exact = read_response_body(OneByteReader(b"exact"), Some(5));
        assert!(matches!(exact, Ok(body) if body == "exact"));
    }

    #[test]
    fn bounded_response_reader_refuses_zero_ceiling_and_read_errors() {
        struct FailingReader;

        impl Read for FailingReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected read failure"))
            }
        }

        assert!(matches!(
            read_response_body(FailingReader, Some(0)),
            Err(CoreError::InvalidContract(message)) if message.contains("nonzero")
        ));
        assert!(matches!(
            read_response_body(FailingReader, Some(8)),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                    LexicalErrorCode::SemProviderTransport
                ),
                ..
            })
        ));
    }

    #[test]
    fn bounded_response_reader_refuses_overreporting_read_contract() {
        struct OverreportingReader;

        impl Read for OverreportingReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                Ok(buf.len().saturating_add(1))
            }
        }

        assert!(matches!(
            read_response_body(OverreportingReader, Some(8)),
            Err(CoreError::InvalidContract(message)) if message.contains("reader exceeded supplied buffer")
        ));
    }

    #[test]
    fn request_body_count_includes_json_escaping_before_memory_admission() {
        let request = EmbeddingsRequest {
            model: "model",
            input: &["quoted \" text", "line\nfeed", "\\"],
            dimensions: 2,
        };
        let mut count = ByteCounter::default();
        assert!(serde_json::to_writer(&mut count, &request).is_ok());
        let encoded = serde_json::to_vec(&request);
        assert!(matches!(
            encoded,
            Ok(body) if u64::try_from(body.len()).is_ok_and(|length| length == count.0)
        ));
    }

    #[test]
    fn single_input_memory_envelope_bounds_maximum_json_escape_and_refuses_overflow()
    -> Result<(), CoreError> {
        let config = cfg(2).with_max_response_bytes(128);
        let (peak, escaping) = config.single_input_memory_envelope(1)?;
        let request = EmbeddingsRequest {
            model: &config.model,
            input: &["\u{0000}"],
            dimensions: config.dimension,
        };
        let actual_body = serde_json::to_vec(&request)
            .map_err(|error| invalid(&format!("test request encode failed: {error}")))?;
        let body_bytes = u64::try_from(actual_body.len())
            .map_err(|_error| invalid("test request length cannot fit u64"))?;
        let url_bytes = u64::try_from(config.base_url.len() + EMBEDDINGS_PATH.len())
            .map_err(|_error| invalid("test URL length cannot fit u64"))?;
        let observed_attempt = body_bytes
            .checked_add(url_bytes)
            .and_then(|bytes| bytes.checked_mul(2))
            .and_then(|bytes| bytes.checked_add(128))
            .ok_or_else(|| invalid("test peak overflows"))?;
        if peak < observed_attempt || escaping <= 128 {
            return Err(invalid(
                "single-input envelope undercounts worst-case escape",
            ));
        }
        if config.single_input_memory_envelope(u64::MAX).is_ok()
            || cfg(2).single_input_memory_envelope(1).is_ok()
        {
            return Err(invalid(
                "single-input envelope accepted missing or overflowing bounds",
            ));
        }
        Ok(())
    }

    // S21-09: attempts spawn through the supervised pool, so a shut-down
    // pool refuses before any HTTP — zero transport calls, typed refusal.
    #[test]
    fn shutdown_pool_refuses_attempts_with_zero_transport_calls() {
        use std::sync::atomic::Ordering;

        use quanta_index_contract::SearchPlaneErrorCodeV2;

        use crate::pool::ProviderAttemptPool;

        let transport = ScriptedTransport::new(vec![Ok(HttpResponse {
            status: 200,
            body: "{\"data\":[{\"embedding\":[0.5,0.5]}]}".to_string(),
        })]);
        let calls = transport.calls_handle();
        let pool = Arc::new(ProviderAttemptPool::new(2).expect("valid pool"));
        pool.shutdown();
        let provider = provider_with(cfg(2), transport).with_attempt_pool(Arc::clone(&pool));
        let err = provider
            .embed_batch(&["alpha"])
            .expect_err("a shut-down pool must refuse");
        let CoreError::Typed { code, .. } = err else {
            panic!("pool refusal must be typed");
        };
        assert_eq!(code, SearchPlaneErrorCodeV2::RequestCancelled);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.live_attempts(), 0);
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

        let response = parse_embeddings_response(body, 1, 2);

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
        let missing_data = parse_embeddings_response(r#"{"object":"list"}"#, 1, 1)
            .map_err(|error| error.to_string());
        let missing_embedding = parse_embeddings_response(r#"{"data":[{"index":0}]}"#, 1, 1)
            .map_err(|error| error.to_string());
        let missing_index = parse_embeddings_response(r#"{"data":[{"embedding":[0.25]}]}"#, 1, 1)
            .map_err(|error| error.to_string());

        assert!(matches!(missing_data, Err(ref error) if error.contains("missing field `data`")));
        assert!(
            matches!(missing_embedding, Err(ref error) if error.contains("missing field `embedding`"))
        );
        assert!(matches!(missing_index, Err(ref error) if error.contains("missing field `index`")));
    }

    #[test]
    fn bounded_response_decoder_rejects_excess_items_and_coordinates() {
        let extra_item =
            r#"{"data":[{"embedding":[0.25,0.75],"index":0},{"embedding":[0.5,0.5],"index":1}]}"#;
        let extra_coordinate = r#"{"data":[{"embedding":[0.25,0.5,0.75],"index":0}]}"#;
        assert!(matches!(
            parse_embeddings_response(extra_item, 1, 2),
            Err(CoreError::Storage(message)) if message.contains("count exceeds")
        ));
        assert!(matches!(
            parse_embeddings_response(extra_coordinate, 1, 2),
            Err(CoreError::Storage(message)) if message.contains("dimension exceeds")
        ));
    }

    #[test]
    fn embed_batch_returns_vectors_in_input_order() {
        // Response intentionally out of order; provider must reorder by index.
        let transport = ScriptedTransport::new(vec![Ok(ok_body(&[
            (1, vec![0.0, 1.0]),
            (0, vec![1.0, 0.0]),
        ]))]);
        let provider = provider_with(cfg(2), transport);
        let vectors = provider.embed_batch(&["a", "b"]).expect("embed ok");
        assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        assert_eq!(provider.model_id(), "openai:text-embedding-3-small");
        assert_eq!(provider.dimension(), 2);
    }

    /// Records peak concurrency and the inputs of every request, and
    /// answers each request from its OWN body, with no wall-clock wait.
    ///
    /// Each input text `"t<N>"` produces a deterministic, order-checkable vector.
    struct ConcurrencyProbeTransport {
        in_flight: Arc<AtomicUsize>,
        peak_in_flight: Arc<AtomicUsize>,
        /// Inputs each request carried, in completion order.
        request_sizes: Arc<Mutex<Vec<usize>>>,
    }

    /// Answer a probe request body from its own `t<N>` inputs.
    fn answer_probe_body(body: &str, request_sizes: &Mutex<Vec<usize>>) -> Vec<(usize, Vec<f32>)> {
        let parsed: serde_json::Value =
            serde_json::from_str(body).expect("probe: request body is valid json");
        let inputs = parsed
            .get("input")
            .and_then(serde_json::Value::as_array)
            .expect("probe: body has an input array");
        request_sizes
            .lock()
            .expect("probe: request sizes mutex")
            .push(inputs.len());
        inputs
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
            .collect()
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
            let items = answer_probe_body(body, &self.request_sizes);
            let _prior = self.in_flight.fetch_sub(1, Ordering::SeqCst);
            Ok(ok_body(&items))
        }
    }

    /// Failure-containment bound for the first-wave rendezvous: reached only
    /// when the provider fails to overlap its cohort, in which case the
    /// transport errors typed and the test fails instead of hanging.
    const RENDEZVOUS_TIMEOUT: Duration = Duration::from_secs(30);

    /// A concurrency probe whose first `cohort` arrivals rendezvous before
    /// any of them is answered, so overlap is structural instead of
    /// sleep-timed (TOPT-04 / R3).
    ///
    /// Later arrivals proceed without waiting: the latch opens the moment the
    /// cohort is complete and never closes again. A latch timeout is a typed
    /// transport error, never a silent pass.
    struct RendezvousProbeTransport {
        in_flight: Arc<AtomicUsize>,
        peak_in_flight: Arc<AtomicUsize>,
        request_sizes: Arc<Mutex<Vec<usize>>>,
        cohort: usize,
        arrivals: Mutex<usize>,
        released: Mutex<bool>,
        release: Condvar,
    }

    impl RendezvousProbeTransport {
        fn wait_for_cohort(&self, arrival: usize) -> Result<(), CoreError> {
            let mut released = self.released.lock().expect("probe: release mutex");
            if *released {
                return Ok(());
            }
            if arrival >= self.cohort {
                *released = true;
                self.release.notify_all();
                return Ok(());
            }
            let (guard, waited) = self
                .release
                .wait_timeout_while(released, RENDEZVOUS_TIMEOUT, |open| !*open)
                .expect("probe: release mutex");
            released = guard;
            if *released {
                return Ok(());
            }
            drop(released);
            debug_assert!(waited.timed_out());
            Err(typed(
                LexicalErrorCode::SemProviderTransport,
                &format!(
                    "probe: rendezvous latch timed out with {arrival} of {} arrivals",
                    self.cohort
                ),
            ))
        }
    }

    impl EmbeddingTransport for RendezvousProbeTransport {
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
            let arrival = {
                let mut arrivals = self.arrivals.lock().expect("probe: arrivals mutex");
                *arrivals = arrivals.saturating_add(1);
                *arrivals
            };
            self.wait_for_cohort(arrival)?;
            let items = answer_probe_body(body, &self.request_sizes);
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
        // max_batch=1 -> one batch per text (10 batches); concurrency=4 -> 4
        // workers. The first wave of 4 rendezvous inside the transport, so a
        // peak of exactly 4 proves four-way overlap structurally, with no
        // sleep-timed observation window.
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let request_sizes = Arc::new(Mutex::new(Vec::new()));
        let transport = RendezvousProbeTransport {
            in_flight: Arc::clone(&in_flight),
            peak_in_flight: Arc::clone(&peak),
            request_sizes: Arc::clone(&request_sizes),
            cohort: 4,
            arrivals: Mutex::new(0),
            released: Mutex::new(false),
            release: Condvar::new(),
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
        assert_eq!(
            out, expected,
            "concurrent dispatch must preserve input order"
        );
        // All four first-wave workers were inside the transport together.
        let observed_peak = peak.load(Ordering::SeqCst);
        assert_eq!(
            observed_peak, 4,
            "the four-worker first wave must rendezvous; peak in-flight was {observed_peak}"
        );
        assert_eq!(
            in_flight.load(Ordering::SeqCst),
            0,
            "nothing is left in flight"
        );
        let sizes = request_sizes
            .lock()
            .expect("probe: request sizes mutex")
            .clone();
        assert_eq!(sizes.len(), 10, "ten texts at max_batch=1 -> ten requests");
        assert!(
            sizes.iter().all(|size| *size == 1),
            "every request carries exactly one text: {sizes:?}"
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
        assert_eq!(
            sizes.len(),
            DEFAULT_CONCURRENCY,
            "a window is window / max_batch requests"
        );
        assert!(
            sizes.iter().all(|size| *size == DEFAULT_MAX_BATCH),
            "every request of a window carries max_batch inputs: {sizes:?}"
        );
        assert!(
            peak.load(Ordering::SeqCst) <= DEFAULT_CONCURRENCY,
            "in-flight requests never exceed the concurrency knob"
        );
        assert_eq!(
            in_flight.load(Ordering::SeqCst),
            0,
            "nothing is left in flight"
        );
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
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "empty input must make no HTTP calls"
        );
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
        let sleeper = Arc::new(RecordingSleeper::new());
        let provider = provider_with(cfg(2).with_max_retries(2), transport)
            .with_retry_sleeper(sleeper.clone());
        let vectors = provider
            .embed_batch(&["a"])
            .expect("recovers after one 429");
        assert_eq!(vectors, vec![vec![0.5, 0.5]]);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "one 429 then success = two calls"
        );
        assert_eq!(
            sleeper.recorded().len(),
            1,
            "exactly one backoff between the two attempts"
        );
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
        let sleeper = Arc::new(RecordingSleeper::new());
        let provider = provider_with(cfg(2).with_max_retries(1), transport)
            .with_retry_sleeper(sleeper.clone());
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
        assert_eq!(
            sleeper.recorded().len(),
            1,
            "exactly one backoff between the two attempts"
        );
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "each index follows an exact length assert on the same delays vector"
    )]
    #[test]
    fn retryable_status_invokes_sleeper_once_per_retry_inside_attempt_bounds() {
        // Two 503s then success with max_retries=2: the sleeper sees exactly
        // the attempt-0 and attempt-1 delays, each inside its production
        // bound, with zero wall-clock wait.
        let transport = ScriptedTransport::new(vec![
            Ok(HttpResponse {
                status: 503,
                body: "x".to_string(),
            }),
            Ok(HttpResponse {
                status: 503,
                body: "x".to_string(),
            }),
            Ok(ok_body(&[(0, vec![0.5, 0.5])])),
        ]);
        let calls = transport.calls_handle();
        let sleeper = Arc::new(RecordingSleeper::new());
        let provider = provider_with(cfg(2).with_max_retries(2), transport)
            .with_retry_sleeper(sleeper.clone());
        let vectors = provider
            .embed_batch(&["a"])
            .expect("recovers after two 503s");
        assert_eq!(vectors, vec![vec![0.5, 0.5]]);
        assert_eq!(calls.load(Ordering::SeqCst), 3, "two retries = three calls");
        let delays = sleeper.recorded();
        assert_eq!(
            delays.len(),
            2,
            "one backoff per retry, none after the final success"
        );
        assert!(
            delays[0] < backoff_bound(0),
            "attempt-0 backoff {:?} must stay below {:?}",
            delays[0],
            backoff_bound(0)
        );
        assert!(
            delays[1] < backoff_bound(1),
            "attempt-1 backoff {:?} must stay below {:?}",
            delays[1],
            backoff_bound(1)
        );
    }

    #[test]
    fn non_retryable_status_never_sleeps() {
        // A fatal 400 fails the call with no retry and no backoff at all.
        let sleeper = Arc::new(RecordingSleeper::new());
        let provider = provider_with(
            cfg(2).with_max_retries(3),
            AlwaysStatusTransport { status: 400 },
        )
        .with_retry_sleeper(sleeper.clone());
        match provider.embed_batch(&["a"]) {
            Err(CoreError::Typed { code, .. }) => {
                assert_eq!(
                    code,
                    quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                        LexicalErrorCode::SemProviderTransport
                    )
                );
            }
            other => panic!("a fatal status must fail closed, got {other:?}"),
        }
        assert!(
            sleeper.recorded().is_empty(),
            "a non-retryable status must never invoke the sleeper"
        );
    }

    #[test]
    fn production_sleeper_answers_an_expired_budget_typed_without_sleeping() {
        // The budget holds nothing while the proposed delay is ten seconds:
        // the production sleeper checkpoints first and returns the existing
        // interruption class instead of sleeping through the backoff.
        let budget = RequestBudgetV1::for_duration(Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        match BudgetPollSleeper.sleep_backoff(Duration::from_secs(10), &budget) {
            Err(CoreError::Typed { code, message }) => {
                assert_eq!(code, quanta_index_core::REQUEST_DEADLINE_EXCEEDED_CODE);
                assert!(message.contains("checkpoint `semantic:embed`"), "{message}");
            }
            other => panic!("an expired budget answers typed, got {other:?}"),
        }
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
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "max_retries=0 must not retry"
        );
    }

    #[test]
    fn dimension_mismatch_in_response_fails_closed() {
        let transport = ScriptedTransport::new(vec![Ok(ok_body(&[(0, vec![1.0, 2.0, 3.0])]))]);
        let provider = provider_with(cfg(2), transport); // configured dim 2, response dim 3
        match provider.embed_batch(&["a"]) {
            Err(CoreError::Storage(message)) => assert!(message.contains("embedding dimension")),
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
        let split = provider_with(
            cfg(2).with_max_batch(1).with_concurrency(1),
            split_transport,
        );
        let vectors = split.embed_batch(&["a", "b"]).expect("batched ok");
        assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        assert_eq!(
            split_calls.load(Ordering::SeqCst),
            2,
            "max_batch=1 -> 2 requests"
        );

        let one_transport = ScriptedTransport::new(vec![Ok(ok_body(&[
            (0, vec![1.0, 0.0]),
            (1, vec![0.0, 1.0]),
        ]))]);
        let one_calls = one_transport.calls_handle();
        let one = provider_with(cfg(2), one_transport); // default max_batch (256)
        let _vectors = one.embed_batch(&["a", "b"]).expect("single batch ok");
        assert_eq!(
            one_calls.load(Ordering::SeqCst),
            1,
            "default batch -> 1 request"
        );
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
    fn reqwest_embedding_transport_does_not_redirect_payload_or_authorization()
    -> Result<(), CoreError> {
        let primary = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|error| CoreError::Storage(format!("primary bind failed: {error}")))?;
        primary.set_nonblocking(true).map_err(|error| {
            CoreError::Storage(format!("primary nonblocking setup failed: {error}"))
        })?;
        let primary_addr = primary
            .local_addr()
            .map_err(|error| CoreError::Storage(format!("primary address failed: {error}")))?;
        let redirect_target = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|error| CoreError::Storage(format!("redirect target bind failed: {error}")))?;
        redirect_target.set_nonblocking(true).map_err(|error| {
            CoreError::Storage(format!("redirect target nonblocking setup failed: {error}"))
        })?;
        let target_addr = redirect_target.local_addr().map_err(|error| {
            CoreError::Storage(format!("redirect target address failed: {error}"))
        })?;
        let transport = ReqwestBlockingTransport::new(Duration::from_secs(2))?;
        let server = std::thread::spawn(move || -> std::io::Result<bool> {
            let started = std::time::Instant::now();
            let (mut stream, _peer) = loop {
                match primary.accept() {
                    Ok(accepted) => break accepted,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if started.elapsed() >= Duration::from_secs(2) {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "primary server did not receive embedding POST",
                            ));
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(error),
                }
            };
            stream.set_read_timeout(Some(Duration::from_secs(2)))?;
            let mut request = [0_u8; 4096];
            let mut filled = 0_usize;
            loop {
                let observed = request
                    .get(..filled)
                    .ok_or_else(|| std::io::Error::other("invalid request buffer length"))?;
                if observed
                    .windows(b"secret-payload".len())
                    .any(|window| window == b"secret-payload")
                {
                    break;
                }
                if filled == request.len() {
                    return Err(std::io::Error::other(
                        "embedding request exceeded test buffer",
                    ));
                }
                let writable = request
                    .get_mut(filled..)
                    .ok_or_else(|| std::io::Error::other("invalid request buffer cursor"))?;
                let read = stream.read(writable)?;
                if read == 0 {
                    return Err(std::io::Error::other(
                        "embedding request ended before its body",
                    ));
                }
                filled = filled
                    .checked_add(read)
                    .filter(|next| *next <= request.len())
                    .ok_or_else(|| std::io::Error::other("request reader overreported bytes"))?;
            }
            let observed = request
                .get(..filled)
                .ok_or_else(|| std::io::Error::other("invalid final request buffer length"))?;
            let request_text = String::from_utf8_lossy(observed);
            let saw_embedding_post = request_text.starts_with("POST /v1/embeddings HTTP/1.1")
                && request_text
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-key");
            let redirect = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{target_addr}/collect\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(redirect.as_bytes())?;
            Ok(saw_embedding_post)
        });

        let response = transport.post_embeddings_owned_bounded(
            &format!("http://{primary_addr}/v1/embeddings"),
            "test-key",
            r#"{"input":["secret-payload"]}"#.into(),
            Duration::from_secs(2),
            Some(1024),
        );
        let saw_embedding_post = server
            .join()
            .map_err(|_panic| CoreError::Storage("primary server panicked".into()))?
            .map_err(|error| CoreError::Storage(format!("primary server failed: {error}")))?;
        if !saw_embedding_post || !matches!(response, Ok(HttpResponse { status: 307, .. })) {
            return Err(invalid(
                "reqwest did not return the original endpoint's redirect response",
            ));
        }
        match redirect_target.accept() {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Ok(_) => Err(invalid(
                "reqwest sent embedding body or authorization to redirect target",
            )),
            Err(error) => Err(CoreError::Storage(format!(
                "redirect target accept failed: {error}"
            ))),
        }
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
            _ => panic!("OPENAI_API_KEY is required when this ignored test is selected"),
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

        fn post_embeddings_owned_bounded(
            &self,
            url: &str,
            api_key: &str,
            body: String,
            timeout: Duration,
            max_response_bytes: Option<u64>,
        ) -> Result<HttpResponse, CoreError> {
            // The only response is a fixed in-memory fixture.
            let response = self.post_embeddings(url, api_key, &body, timeout)?;
            let response_len = u64::try_from(response.body.len())
                .map_err(|_error| invalid("test response length exceeds u64"))?;
            if max_response_bytes.is_some_and(|maximum| response_len > maximum) {
                return Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
                    message: "parked response exceeds configured byte ceiling".into(),
                });
            }
            Ok(response)
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
        let provider = provider_with(
            cfg(2)
                .with_max_retries(retries)
                .with_timeout(timeout)
                .with_max_response_bytes(256),
            transport,
        );
        (provider, entered_rx, release, calls)
    }

    #[derive(Debug)]
    struct AttemptCustodyProbe {
        live_attempts: Arc<AtomicUsize>,
        live_memory_bytes: Arc<AtomicU64>,
        max_memory_bytes: u64,
    }

    struct AttemptMemoryOwner(Arc<AtomicU64>);

    impl quanta_index_core::index::MemoryLeaseOwner for AttemptMemoryOwner {
        fn release(&self, bytes: u64) {
            let _previous = self.0.fetch_sub(bytes, Ordering::SeqCst);
        }
    }

    struct AttemptLeaseProbe(Arc<AtomicUsize>);

    impl ProviderAttemptLeaseV1 for AttemptLeaseProbe {}

    impl Drop for AttemptLeaseProbe {
        fn drop(&mut self) {
            let _previous = self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl ProviderAttemptCustodyPortV1 for AttemptCustodyProbe {
        fn begin_attempt_v1(&self) -> Result<Box<dyn ProviderAttemptLeaseV1>, CoreError> {
            let _previous = self.live_attempts.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(AttemptLeaseProbe(Arc::clone(&self.live_attempts))))
        }

        fn reserve_memory_v1(
            &self,
            bytes: u64,
            _lifetime: MemoryLifetime,
        ) -> Result<MemoryLease, CoreError> {
            if bytes == 0 {
                return Err(invalid("test: zero-byte memory reservation"));
            }
            let _previous_memory = self
                .live_memory_bytes
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                    current
                        .checked_add(bytes)
                        .filter(|next| *next <= self.max_memory_bytes)
                })
                .map_err(|_current| CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::ServerOverloaded,
                    message: "test: provider memory admission exhausted".into(),
                })?;
            Ok(MemoryLease::new(
                bytes,
                Arc::new(AttemptMemoryOwner(Arc::clone(&self.live_memory_bytes))),
            ))
        }
    }

    #[test]
    fn cancelled_openai_call_keeps_physical_attempt_lease_until_transport_returns() {
        let (provider, entered, release, _calls) = parked_provider(0, Duration::from_secs(60));
        let provider = provider.with_required_execution_custody();
        let live = Arc::new(AtomicUsize::new(0));
        let live_memory_bytes = Arc::new(AtomicU64::new(0));
        let authority: Arc<dyn ProviderAttemptCustodyPortV1> = Arc::new(AttemptCustodyProbe {
            live_attempts: Arc::clone(&live),
            live_memory_bytes: Arc::clone(&live_memory_bytes),
            max_memory_bytes: 1_048_576,
        });
        let mut budget = RequestBudgetV1::for_duration(Duration::from_secs(30));
        assert!(budget.bind_provider_attempt_custody_v1(&authority).is_ok());
        let cancel = budget.cancel_handle();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let canceller = std::thread::spawn(move || {
            let started = entered.recv_timeout(Duration::from_secs(10)).is_ok();
            if started {
                cancel.cancel();
            }
            let _sent = started_tx.send(started);
        });
        let result = provider.embed_batch_within(&["a"], &budget);
        let canceller_joined = canceller.join().is_ok();
        let started = started_rx
            .recv_timeout(Duration::from_secs(1))
            .is_ok_and(std::convert::identity);
        let live_while_parked = live.load(Ordering::SeqCst);
        let memory_while_parked = live_memory_bytes.load(Ordering::SeqCst);
        drop(authority);
        drop(budget);
        let live_without_caller = live.load(Ordering::SeqCst);
        let memory_without_caller = live_memory_bytes.load(Ordering::SeqCst);
        if started {
            let _released = release.wait();
        }
        let drained = provider.attempts.drain(Duration::from_secs(2));
        assert!(started);
        assert!(canceller_joined);
        assert!(matches!(
            result,
            Err(CoreError::Typed {
                code: quanta_index_core::REQUEST_CANCELLED_CODE,
                ..
            })
        ));
        assert_eq!(live_while_parked, 1);
        assert!(memory_while_parked > 0);
        assert_eq!(live_without_caller, 1);
        assert!(memory_without_caller > 0);
        assert!(drained.unfinished.is_empty());
        assert_eq!(live.load(Ordering::SeqCst), 0);
        assert_eq!(live_memory_bytes.load(Ordering::SeqCst), 0);
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
        assert_eq!(provider.attempts.live_attempts(), 1);
        let draining = provider.attempts.drain(Duration::ZERO);
        assert_eq!(draining.joined, 0);
        assert_eq!(draining.unfinished.len(), 1);
        assert_eq!(provider.attempts.live_attempts(), 1);
        let _released = release.wait();
        let completed = provider.attempts.drain(Duration::from_secs(2));
        assert_eq!(completed.joined, 1);
        assert!(completed.unfinished.is_empty());
        assert_eq!(provider.attempts.live_attempts(), 0);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "no retry after a cancellation"
        );
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
