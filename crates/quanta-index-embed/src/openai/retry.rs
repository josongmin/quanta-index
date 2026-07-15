//! HTTP status classification and full-jitter exponential backoff for the
//! `OpenAI` embedding transport.
//!
//! Pure retry-timing/status policy: it decides whether a response is a success,
//! an auth failure, a retryable transient, or fatal, and how long to wait before
//! the next attempt. It touches no HTTP client and no embedding data — the
//! randomness here is confined to retry timing and never affects output.

use std::time::Duration;

/// Base delay for full-jitter exponential backoff.
const RETRY_BASE_DELAY: Duration = Duration::from_millis(250);

pub(super) enum StatusClass {
    Success,
    Auth,
    Retryable,
    Fatal,
}

pub(super) fn classify_status(status: u16) -> StatusClass {
    match status {
        200..=299 => StatusClass::Success,
        401 | 403 => StatusClass::Auth,
        408 | 429 | 500..=599 => StatusClass::Retryable,
        _ => StatusClass::Fatal,
    }
}

/// Full-jitter exponential backoff: a uniform random delay in
/// `[0, RETRY_BASE_DELAY * 2^attempt]`.
///
/// Jitter prevents concurrent retriers from waking together after a 429. It
/// affects retry timing only and never embedding output.
pub(super) fn backoff_delay(attempt: u32) -> Duration {
    let bound = RETRY_BASE_DELAY.saturating_mul(2_u32.saturating_pow(attempt));
    let bound_nanos = duration_nanos_saturating(bound);
    if bound_nanos == 0 {
        return Duration::ZERO;
    }
    let jittered = next_jitter_u64().checked_rem(bound_nanos).unwrap_or(0);
    Duration::from_nanos(jittered)
}

fn duration_nanos_saturating(duration: Duration) -> u64 {
    let Ok(nanos) = u64::try_from(duration.as_nanos()) else {
        return u64::MAX;
    };
    nanos
}

/// Per-thread xorshift64 PRNG for backoff jitter only; not cryptographic and used
/// for nothing but retry timing.
///
/// State is thread-local (not a shared atomic): under concurrent dispatch a shared
/// load/store generator lets two workers read the same state and return identical
/// jitter, re-correlating the very 429 retries the jitter exists to spread. Each
/// thread seeds once from the wall clock XOR a process-global counter, so threads
/// seeded within the same nanosecond still diverge and the generators are
/// independent — no per-call atomic on the hot path.
fn next_jitter_u64() -> u64 {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, Ordering};

    thread_local! {
        static STATE: Cell<u64> = const { Cell::new(0) };
    }
    static SEED_COUNTER: AtomicU64 = AtomicU64::new(0);

    STATE.with(|cell| {
        let mut state = cell.get();
        if state == 0 {
            let nanos = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                Ok(elapsed) => duration_nanos_saturating(elapsed),
                Err(before_epoch) => duration_nanos_saturating(before_epoch.duration()),
            };
            // A distinct per-thread offset so two threads seeded in the same
            // nanosecond still start from different states.
            let unique = SEED_COUNTER
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_mul(0x9E37_79B9_7F4A_7C15);
            // Force non-zero so the generator never latches at zero.
            state = (nanos ^ unique) | 1;
        }
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        cell.set(state);
        state
    })
}
