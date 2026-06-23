use std::time::Duration;

use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_core::{CoreError, TextEmbeddingProvider};
use serde::{Deserialize, Serialize};

const EMBEDDINGS_PATH: &str = "/v1/embeddings";
const DEFAULT_BASE_URL: &str = "https://api.openai.com";
/// Default max inputs per `/v1/embeddings` request. Public so daemon env knobs
/// can default to the same value without a second source of truth.
pub const DEFAULT_MAX_BATCH: usize = 256;
/// Default bounded retry count for transient failures.
pub const DEFAULT_MAX_RETRIES: u32 = 3;
/// Default per-request HTTP timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);
const RETRY_BASE_DELAY: Duration = Duration::from_millis(250);
const ERROR_BODY_PREVIEW_CHARS: usize = 200;

/// Outbound transport for a single embeddings request.
///
/// Returns the HTTP status code and response body, or a typed transport error.
/// Abstracted so the provider's batching / retry / parse logic is unit-testable
/// without network.
pub trait EmbeddingTransport: Send + Sync {
    fn post_embeddings(
        &self,
        url: &str,
        api_key: &str,
        body: &str,
    ) -> Result<HttpResponse, CoreError>;
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
    pub dimension: usize,
    pub base_url: String,
    pub max_batch: usize,
    pub max_retries: u32,
    pub timeout: Duration,
}

impl OpenAiProviderConfig {
    /// Config for `model` at `dimension` against the public `OpenAI` endpoint, with
    /// default tuning.
    #[must_use]
    pub fn new(api_key: String, model: String, dimension: usize) -> Self {
        Self {
            api_key,
            model,
            dimension,
            base_url: DEFAULT_BASE_URL.to_string(),
            max_batch: DEFAULT_MAX_BATCH,
            max_retries: DEFAULT_MAX_RETRIES,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Override the API base URL (used by tests to target a stub server).
    #[must_use]
    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    /// Max inputs per `/v1/embeddings` request (clamped to >= 1).
    #[must_use]
    pub fn with_max_batch(mut self, max_batch: usize) -> Self {
        self.max_batch = max_batch.max(1);
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
}

/// `OpenAI`-backed [`TextEmbeddingProvider`].
///
/// Embeds via batched `/v1/embeddings` calls behind a blocking transport, failing
/// closed on auth / transport / shape errors and retrying transient (429 / 5xx /
/// network) failures with bounded exponential backoff.
pub struct OpenAiEmbeddingProvider {
    transport: Box<dyn EmbeddingTransport>,
    api_key: String,
    model: String,
    model_id: String,
    dimension: usize,
    base_url: String,
    max_batch: usize,
    max_retries: u32,
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
        if config.dimension == 0 {
            return Err(invalid("openai: dimension must be non-zero"));
        }
        let model_id = format!("openai:{}", config.model);
        Ok(Self {
            transport,
            api_key: config.api_key,
            model: config.model,
            model_id,
            dimension: config.dimension,
            base_url: config.base_url,
            max_batch: config.max_batch.max(1),
            max_retries: config.max_retries,
        })
    }

    /// Build a provider over the real in-process blocking reqwest transport,
    /// honoring the config's timeout / batch / retry tuning.
    pub fn with_reqwest(config: OpenAiProviderConfig) -> Result<Self, CoreError> {
        let transport = ReqwestBlockingTransport::new(config.timeout)?;
        Self::new(config, Box::new(transport))
    }

    fn embed_one_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        let request = EmbeddingsRequest {
            model: &self.model,
            input: texts,
            dimensions: self.dimension,
        };
        let body = serde_json::to_string(&request)
            .map_err(|err| invalid(&format!("openai: request encode failed: {err}")))?;
        let url = format!("{}{EMBEDDINGS_PATH}", self.base_url);
        let raw = self.post_with_retry(&url, &body)?;
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

    fn post_with_retry(&self, url: &str, body: &str) -> Result<String, CoreError> {
        let mut last_error: Option<CoreError> = None;
        for attempt in 0..=self.max_retries {
            match self.transport.post_embeddings(url, &self.api_key, body) {
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
                        last_error = Some(typed(
                            LexicalErrorCode::SemProviderTransport,
                            &format!("openai: retryable status {}", response.status),
                        ));
                    }
                },
                Err(err) => last_error = Some(err),
            }
            if attempt < self.max_retries {
                std::thread::sleep(backoff_delay(attempt));
            }
        }
        Err(last_error.unwrap_or_else(|| {
            typed(
                LexicalErrorCode::SemProviderTransport,
                "openai: transport exhausted retries without a response",
            )
        }))
    }
}

impl TextEmbeddingProvider for OpenAiEmbeddingProvider {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(self.max_batch) {
            out.extend(self.embed_one_batch(chunk)?);
        }
        Ok(out)
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn model_version(&self) -> Option<&str> {
        None
    }

    fn dimension(&self) -> usize {
        self.dimension
    }
}

/// The real blocking transport: a `reqwest::blocking::Client` over rustls.
pub struct ReqwestBlockingTransport {
    client: reqwest::blocking::Client,
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
        Ok(Self { client })
    }
}

impl EmbeddingTransport for ReqwestBlockingTransport {
    fn post_embeddings(
        &self,
        url: &str,
        api_key: &str,
        body: &str,
    ) -> Result<HttpResponse, CoreError> {
        let response = self
            .client
            .post(url)
            .bearer_auth(api_key)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
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

#[derive(Serialize)]
struct EmbeddingsRequest<'a> {
    model: &'a str,
    input: &'a [&'a str],
    dimensions: usize,
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    embedding: Vec<f32>,
    index: usize,
}

enum StatusClass {
    Success,
    Auth,
    Retryable,
    Fatal,
}

fn classify_status(status: u16) -> StatusClass {
    match status {
        200..=299 => StatusClass::Success,
        401 | 403 => StatusClass::Auth,
        408 | 429 | 500..=599 => StatusClass::Retryable,
        _ => StatusClass::Fatal,
    }
}

/// Full-jitter exponential backoff: a uniform random delay in
/// `[0, RETRY_BASE_DELAY * 2^attempt]`. Jitter is essential once more than one
/// request (or process) can hit a 429 — a deterministic schedule makes all
/// retriers wake together and re-stampede. Randomness is confined to retry
/// timing and never affects embedding output.
fn backoff_delay(attempt: u32) -> Duration {
    let bound = RETRY_BASE_DELAY.saturating_mul(2_u32.saturating_pow(attempt));
    let bound_nanos = u64::try_from(bound.as_nanos()).unwrap_or(u64::MAX);
    if bound_nanos == 0 {
        return Duration::ZERO;
    }
    let jittered = next_jitter_u64().checked_rem(bound_nanos).unwrap_or(0);
    Duration::from_nanos(jittered)
}

/// Process-local xorshift64 PRNG for backoff jitter only. Seeded once from the
/// wall clock; not cryptographic and used for nothing but retry timing.
fn next_jitter_u64() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut state = STATE.load(Ordering::Relaxed);
    if state == 0 {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| u64::try_from(elapsed.as_nanos()).ok())
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        // Force non-zero so the generator never latches at zero.
        state = seed | 1;
    }
    state ^= state << 13;
    state ^= state >> 7;
    state ^= state << 17;
    STATE.store(state, Ordering::Relaxed);
    state
}

fn preview(body: &str) -> String {
    body.chars().take(ERROR_BODY_PREVIEW_CHARS).collect()
}

fn typed(code: LexicalErrorCode, message: &str) -> CoreError {
    CoreError::Typed {
        code: code.as_code_str().to_string(),
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
                let nums: Vec<String> = vector.iter().map(std::string::ToString::to_string).collect();
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
            dimension,
        )
    }

    fn provider_with(
        config: OpenAiProviderConfig,
        transport: ScriptedTransport,
    ) -> OpenAiEmbeddingProvider {
        OpenAiEmbeddingProvider::new(config, Box::new(transport))
            .expect("provider builds with a non-empty key/model/dim")
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

    #[test]
    fn empty_input_makes_no_calls() {
        let transport = ScriptedTransport::new(Vec::new());
        let calls = transport.calls_handle();
        let provider = provider_with(cfg(2), transport);
        let vectors = provider.embed_batch(&[]).expect("empty ok");
        assert!(vectors.is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0, "empty input must make no HTTP calls");
    }

    // P0-2: full-jitter backoff stays within the exponential bound every sample.
    #[test]
    fn backoff_jitter_stays_within_exponential_bound() {
        for attempt in 0_u32..5 {
            let bound = RETRY_BASE_DELAY.saturating_mul(2_u32.saturating_pow(attempt));
            for _sample in 0..64 {
                let delay = backoff_delay(attempt);
                assert!(
                    delay <= bound,
                    "attempt {attempt}: delay {delay:?} exceeds bound {bound:?}"
                );
            }
        }
        // Attempt 0's bound is exactly RETRY_BASE_DELAY.
        for _sample in 0..64 {
            assert!(backoff_delay(0) <= RETRY_BASE_DELAY);
        }
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
                assert_eq!(code, LexicalErrorCode::SemProviderAuth.as_code_str());
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
        let vectors = provider.embed_batch(&["a"]).expect("recovers after one 429");
        assert_eq!(vectors, vec![vec![0.5, 0.5]]);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "one 429 then success = two calls");
    }

    #[test]
    fn exhausted_retries_fail_closed_as_transport() {
        // KNOB PROOF: max_retries=1 -> exactly 2 transport attempts (1 + 1 retry).
        let transport = ScriptedTransport::new(vec![
            Ok(HttpResponse { status: 503, body: "x".to_string() }),
            Ok(HttpResponse { status: 503, body: "x".to_string() }),
        ]);
        let calls = transport.calls_handle();
        let provider = provider_with(cfg(2).with_max_retries(1), transport);
        match provider.embed_batch(&["a"]) {
            Err(CoreError::Typed { code, .. }) => {
                assert_eq!(code, LexicalErrorCode::SemProviderTransport.as_code_str());
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
        let split_transport = ScriptedTransport::new(vec![
            Ok(ok_body(&[(0, vec![1.0, 0.0])])),
            Ok(ok_body(&[(0, vec![0.0, 1.0])])),
        ]);
        let split_calls = split_transport.calls_handle();
        let split = provider_with(cfg(2).with_max_batch(1), split_transport);
        let vectors = split.embed_batch(&["a", "b"]).expect("batched ok");
        assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        assert_eq!(split_calls.load(Ordering::SeqCst), 2, "max_batch=1 -> 2 requests");

        let one_transport = ScriptedTransport::new(vec![Ok(ok_body(&[
            (0, vec![1.0, 0.0]),
            (1, vec![0.0, 1.0]),
        ]))]);
        let one_calls = one_transport.calls_handle();
        let one = provider_with(cfg(2), one_transport); // default max_batch (256)
        let _vectors = one.embed_batch(&["a", "b"]).expect("single batch ok");
        assert_eq!(one_calls.load(Ordering::SeqCst), 1, "default batch -> 1 request");
    }

    #[test]
    fn config_defaults_and_overrides_thread_into_provider() {
        // Defaults from OpenAiProviderConfig::new are what the provider uses, and
        // builder overrides replace them (clamped where applicable).
        let defaults = cfg(8);
        assert_eq!(defaults.max_batch, DEFAULT_MAX_BATCH);
        assert_eq!(defaults.max_retries, DEFAULT_MAX_RETRIES);
        assert_eq!(defaults.timeout, DEFAULT_TIMEOUT);
        let tuned = cfg(8)
            .with_max_batch(0) // clamped to >= 1
            .with_max_retries(7)
            .with_timeout(Duration::from_secs(5));
        assert_eq!(tuned.max_batch, 1);
        assert_eq!(tuned.max_retries, 7);
        assert_eq!(tuned.timeout, Duration::from_secs(5));
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
            OpenAiProviderConfig::new(String::new(), "m".to_string(), 2),
            Box::new(ScriptedTransport::new(Vec::new())),
        );
        match result {
            Err(CoreError::Typed { code, .. }) => {
                assert_eq!(code, LexicalErrorCode::SemProviderAuth.as_code_str());
            }
            Err(other) => panic!("empty key wrong error variant: {other:?}"),
            Ok(_) => panic!("empty key must be rejected at construction"),
        }
    }
}
