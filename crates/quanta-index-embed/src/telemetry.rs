use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// One observed outbound embeddings request attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenAiRequestSample {
    pub texts_submitted: u64,
    pub estimated_tokens: u64,
}

/// Process-local embedding telemetry snapshot.
///
/// This is intentionally scoped to local diagnostic / A-B capture flows that run
/// the daemon in-process. It is NOT a per-request API surface and does not claim
/// cross-process completeness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenAiEmbedStatsSnapshot {
    pub total_texts_observed: u64,
    pub cache_hits: u64,
    pub distinct_miss_texts: u64,
    pub http_request_count: u64,
    pub retry_count: u64,
    pub retryable_status_count: u64,
    pub transport_error_count: u64,
    pub request_samples: Vec<OpenAiRequestSample>,
}

#[derive(Default)]
struct OpenAiEmbedStats {
    total_texts_observed: AtomicU64,
    cache_hits: AtomicU64,
    distinct_miss_texts: AtomicU64,
    http_request_count: AtomicU64,
    retry_count: AtomicU64,
    retryable_status_count: AtomicU64,
    transport_error_count: AtomicU64,
    request_samples: Mutex<Vec<OpenAiRequestSample>>,
}

fn stats() -> &'static OpenAiEmbedStats {
    static STATS: OnceLock<OpenAiEmbedStats> = OnceLock::new();
    STATS.get_or_init(OpenAiEmbedStats::default)
}

fn usize_to_u64(value: usize) -> u64 {
    let Ok(value) = u64::try_from(value) else {
        return u64::MAX;
    };
    value
}

pub(crate) fn record_cache_observation(
    total_texts_observed: usize,
    cache_hits: usize,
    distinct_miss_texts: usize,
) {
    let stats = stats();
    let _previous: u64 = stats
        .total_texts_observed
        .fetch_add(usize_to_u64(total_texts_observed), Ordering::Relaxed);
    let _previous: u64 = stats
        .cache_hits
        .fetch_add(usize_to_u64(cache_hits), Ordering::Relaxed);
    let _previous: u64 = stats
        .distinct_miss_texts
        .fetch_add(usize_to_u64(distinct_miss_texts), Ordering::Relaxed);
}

pub(crate) fn record_http_request(texts_submitted: usize, estimated_tokens: usize) {
    let stats = stats();
    let _previous: u64 = stats.http_request_count.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut guard) = stats.request_samples.lock() {
        guard.push(OpenAiRequestSample {
            texts_submitted: usize_to_u64(texts_submitted),
            estimated_tokens: usize_to_u64(estimated_tokens),
        });
    }
}

pub(crate) fn record_retry() {
    let _previous: u64 = stats().retry_count.fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_retryable_status() {
    let _previous: u64 = stats()
        .retryable_status_count
        .fetch_add(1, Ordering::Relaxed);
}

pub(crate) fn record_transport_error() {
    let _previous: u64 = stats()
        .transport_error_count
        .fetch_add(1, Ordering::Relaxed);
}

#[must_use]
pub fn snapshot_openai_embed_stats() -> OpenAiEmbedStatsSnapshot {
    let stats = stats();
    let request_samples = match stats.request_samples.lock() {
        Ok(guard) => guard.clone(),
        // A poisoned diagnostics lock cannot invalidate the atomic counters.
        // Return an explicitly empty sample list instead of propagating stale data.
        Err(_poisoned) => Vec::new(),
    };
    OpenAiEmbedStatsSnapshot {
        total_texts_observed: stats.total_texts_observed.load(Ordering::Relaxed),
        cache_hits: stats.cache_hits.load(Ordering::Relaxed),
        distinct_miss_texts: stats.distinct_miss_texts.load(Ordering::Relaxed),
        http_request_count: stats.http_request_count.load(Ordering::Relaxed),
        retry_count: stats.retry_count.load(Ordering::Relaxed),
        retryable_status_count: stats.retryable_status_count.load(Ordering::Relaxed),
        transport_error_count: stats.transport_error_count.load(Ordering::Relaxed),
        request_samples,
    }
}

pub fn reset_openai_embed_stats() {
    let stats = stats();
    stats.total_texts_observed.store(0, Ordering::Relaxed);
    stats.cache_hits.store(0, Ordering::Relaxed);
    stats.distinct_miss_texts.store(0, Ordering::Relaxed);
    stats.http_request_count.store(0, Ordering::Relaxed);
    stats.retry_count.store(0, Ordering::Relaxed);
    stats.retryable_status_count.store(0, Ordering::Relaxed);
    stats.transport_error_count.store(0, Ordering::Relaxed);
    if let Ok(mut guard) = stats.request_samples.lock() {
        guard.clear();
    }
}
