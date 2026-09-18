//! Process-local embedding telemetry: monotonic counters, exact maxima and
//! a bounded window of recent request samples (QI-BB-009).
//!
//! Counters and maxima accumulate for the process lifetime and cost a
//! fixed number of words. Request samples are kept in a ring of
//! [`REQUEST_SAMPLE_CAPACITY`] so a long-lived process observing millions
//! of outbound requests holds the most recent few hundred, and reports how
//! many older ones the ring let go.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use quanta_index_core::{CoreError, MetricPointV1, MetricSourcePort};

/// Most recent request samples the snapshot carries.
pub const REQUEST_SAMPLE_CAPACITY: usize = 256;

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
    /// Most texts one request carried, over every request observed.
    pub max_request_texts: u64,
    /// Highest estimated token count one request carried, over every
    /// request observed.
    pub max_estimated_tokens: u64,
    /// Request samples the bounded window let go, oldest first.
    pub request_samples_dropped: u64,
    /// The most recent request samples, oldest first, at most
    /// [`REQUEST_SAMPLE_CAPACITY`].
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
    max_request_texts: AtomicU64,
    max_estimated_tokens: AtomicU64,
    request_samples_dropped: AtomicU64,
    request_samples: Mutex<VecDeque<OpenAiRequestSample>>,
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
    let texts_submitted = usize_to_u64(texts_submitted);
    let estimated_tokens = usize_to_u64(estimated_tokens);
    let _previous: u64 = stats.http_request_count.fetch_add(1, Ordering::Relaxed);
    let _previous: u64 = stats
        .max_request_texts
        .fetch_max(texts_submitted, Ordering::Relaxed);
    let _previous: u64 = stats
        .max_estimated_tokens
        .fetch_max(estimated_tokens, Ordering::Relaxed);
    if let Ok(mut guard) = stats.request_samples.lock() {
        if guard.len() >= REQUEST_SAMPLE_CAPACITY {
            let _oldest: Option<OpenAiRequestSample> = guard.pop_front();
            let _previous: u64 = stats
                .request_samples_dropped
                .fetch_add(1, Ordering::Relaxed);
        }
        guard.push_back(OpenAiRequestSample {
            texts_submitted,
            estimated_tokens,
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
        Ok(guard) => guard.iter().cloned().collect(),
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
        max_request_texts: stats.max_request_texts.load(Ordering::Relaxed),
        max_estimated_tokens: stats.max_estimated_tokens.load(Ordering::Relaxed),
        request_samples_dropped: stats.request_samples_dropped.load(Ordering::Relaxed),
        request_samples,
    }
}

/// The provider telemetry as scrape points, `embed_provider_…`
/// (QI-BB-015): outbound requests, retries, retryable (HTTP) failures,
/// transport failures, and the samples the diagnostic window let go.
///
/// The counters are process-global, so one source registered by the
/// composition root reports every provider instance in the process.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OpenAiEmbedTelemetrySource;

impl MetricSourcePort for OpenAiEmbedTelemetrySource {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let snapshot = snapshot_openai_embed_stats();
        Ok(vec![
            MetricPointV1::counter(
                "embed_provider_http_requests_total",
                snapshot.http_request_count,
            ),
            MetricPointV1::counter("embed_provider_retries_total", snapshot.retry_count),
            MetricPointV1::counter(
                "embed_provider_http_failures_total",
                snapshot.retryable_status_count,
            ),
            MetricPointV1::counter(
                "embed_provider_transport_failures_total",
                snapshot.transport_error_count,
            ),
            MetricPointV1::counter(
                "embed_provider_texts_observed_total",
                snapshot.total_texts_observed,
            ),
            MetricPointV1::counter(
                "embed_provider_request_samples_dropped_total",
                snapshot.request_samples_dropped,
            ),
        ])
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
    stats.max_request_texts.store(0, Ordering::Relaxed);
    stats.max_estimated_tokens.store(0, Ordering::Relaxed);
    stats.request_samples_dropped.store(0, Ordering::Relaxed);
    if let Ok(mut guard) = stats.request_samples.lock() {
        guard.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::MetricValueV1;

    fn counter(points: &[MetricPointV1], name: &str) -> u64 {
        points
            .iter()
            .find(|point| point.name == name)
            .and_then(|point| match point.value {
                MetricValueV1::Counter(value) => Some(value),
                MetricValueV1::Gauge(_) => None,
            })
            .unwrap_or_else(|| panic!("counter `{name}` is scraped"))
    }

    /// Every failure the retry loop records reaches the scrape source
    /// (QI-BB-009 #5).
    ///
    /// The counters are process-global and other tests record into them
    /// concurrently, so the assertions are lower bounds on the deltas.
    #[test]
    fn the_telemetry_source_reports_retries_and_failures() {
        let before = OpenAiEmbedTelemetrySource.scrape().expect("scrape");
        record_http_request(3, 9);
        record_retry();
        record_retryable_status();
        record_transport_error();
        let after = OpenAiEmbedTelemetrySource.scrape().expect("scrape");
        for name in [
            "embed_provider_http_requests_total",
            "embed_provider_retries_total",
            "embed_provider_http_failures_total",
            "embed_provider_transport_failures_total",
        ] {
            assert!(
                counter(&after, name) >= counter(&before, name).saturating_add(1),
                "{name} moved by at least the recorded event"
            );
        }
        assert!(
            after
                .iter()
                .all(|point| quanta_index_contract::is_metric_name_v1(&point.name)),
            "every name is wire-valid"
        );
    }

    /// The sample window never grows past its capacity, the maxima stay
    /// exact past it, and the snapshot says how many samples it let go.
    ///
    /// The counters are process-global and other tests record into them
    /// concurrently, so the assertions are differences and bounds, not
    /// absolute values.
    #[test]
    fn request_samples_are_a_bounded_window_with_exact_maxima() {
        let before = snapshot_openai_embed_stats();
        let extra = 50_usize;
        let sentinel_tokens = 7_000_003_usize;
        for round in 0..REQUEST_SAMPLE_CAPACITY + extra {
            record_http_request(round % 9 + 1, sentinel_tokens);
        }
        let after = snapshot_openai_embed_stats();
        assert_eq!(after.request_samples.len(), REQUEST_SAMPLE_CAPACITY);
        assert!(
            after.request_samples_dropped - before.request_samples_dropped
                >= u64::try_from(extra).expect("fits"),
            "at least the overflow must be reported as dropped"
        );
        assert!(
            after.http_request_count - before.http_request_count
                >= u64::try_from(REQUEST_SAMPLE_CAPACITY + extra).expect("fits")
        );
        assert!(after.max_estimated_tokens >= u64::try_from(sentinel_tokens).expect("fits"));
        assert!(after.max_request_texts >= 9);
        assert!(
            after
                .request_samples
                .iter()
                .all(|sample| sample.texts_submitted >= 1),
            "every retained sample is a recorded request"
        );
    }
}
