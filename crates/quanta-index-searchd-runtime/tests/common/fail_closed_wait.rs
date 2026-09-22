//! Fail-closed wait primitives (TOPT-06/TH-1/TH-2).
//!
//! Helpers return success only when their predicate holds: a spent wait
//! is a typed timeout carrying the elapsed time, the attempt count, the
//! last observed value or error, and the expected-predicate description —
//! never the last value as success, and never a retryable error relabeled
//! terminal merely because the deadline elapsed.

#![forbid(unsafe_code)]

use std::fmt;
use std::time::Duration;

/// Monotonic clock plus sleep, injected so unit seams advance a virtual
/// clock instead of sleeping on the wall clock.
pub(super) trait WaitTicker {
    /// Monotonic time since an arbitrary ticker epoch.
    fn now(&self) -> Duration;
    /// Sleep, or advance the virtual clock past it.
    fn sleep(&self, duration: Duration);
}

/// Wall-clock ticker for real waits.
pub(super) struct RealTicker {
    start: std::time::Instant,
}

impl RealTicker {
    pub(super) fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
        }
    }
}

impl Default for RealTicker {
    fn default() -> Self {
        Self::new()
    }
}

impl WaitTicker for RealTicker {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// A spent wait: how long it ran, how many attempts it made, what it
/// expected, and the last value or error it saw.
#[derive(Debug)]
pub(super) struct WaitTimeout {
    pub(super) elapsed: Duration,
    pub(super) attempts: u64,
    pub(super) expected: String,
    pub(super) last: Option<String>,
}

impl fmt::Display for WaitTimeout {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.last {
            Some(last) => write!(
                formatter,
                "timed out after {:?} and {} attempts waiting for {}; last observed: {last}",
                self.elapsed, self.attempts, self.expected
            ),
            None => write!(
                formatter,
                "timed out after {:?} and {} attempts waiting for {}; nothing was observed",
                self.elapsed, self.attempts, self.expected
            ),
        }
    }
}

impl std::error::Error for WaitTimeout {}

/// A wait that did not produce its value: the deadline elapsed with
/// evidence, or the operation refused terminally.
#[derive(Debug)]
pub(super) enum WaitError<E> {
    Timeout(WaitTimeout),
    Terminal(E),
}

impl<E: fmt::Display> fmt::Display for WaitError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout(timeout) => write!(formatter, "{timeout}"),
            Self::Terminal(error) => write!(formatter, "terminal error while waiting: {error}"),
        }
    }
}

impl<E: std::error::Error + fmt::Debug + 'static> std::error::Error for WaitError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Timeout(timeout) => Some(timeout),
            Self::Terminal(error) => Some(error),
        }
    }
}

/// A value observed while a terminal error was expected: immediate
/// failure evidence, never a reason to keep waiting.
#[derive(Debug)]
pub(super) struct UnexpectedSuccess<T>(pub(super) T);

impl<T: fmt::Debug> fmt::Display for UnexpectedSuccess<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "operation unexpectedly succeeded while waiting for a typed error: {:?}",
            render_capped(&self.0)
        )
    }
}

impl<T: fmt::Debug> std::error::Error for UnexpectedSuccess<T> {}

/// Debug rendering capped so a timeout message carries evidence, not a
/// multi-megabyte response dump.
fn render_capped(value: &dyn fmt::Debug) -> String {
    const CAP_CHARS: usize = 2048;
    let rendered = format!("{value:?}");
    if rendered.chars().count() <= CAP_CHARS {
        return rendered;
    }
    let prefix: String = rendered.chars().take(CAP_CHARS).collect();
    format!("{prefix}…[truncated]")
}

/// Poll `run` until `ready` holds: the first attempt runs immediately,
/// a non-ready value or a retryable error sleeps `poll_interval` and
/// retries, a terminal error returns at once, and a spent deadline
/// returns typed timeout evidence. Attempts count every `run` call.
pub(super) fn wait_for<T, E>(
    ticker: &dyn WaitTicker,
    deadline: Duration,
    poll_interval: Duration,
    expected: &str,
    mut run: impl FnMut() -> Result<T, E>,
    mut ready: impl FnMut(&T) -> bool,
    mut retryable: impl FnMut(&E) -> bool,
) -> Result<T, WaitError<E>>
where
    T: fmt::Debug,
    E: fmt::Debug,
{
    debug_assert!(
        poll_interval > Duration::ZERO,
        "a zero poll interval never advances a virtual clock"
    );
    let start = ticker.now();
    let mut attempts = 0_u64;
    let mut last: Option<String>;
    loop {
        attempts += 1;
        match run() {
            Ok(value) if ready(&value) => return Ok(value),
            Ok(value) => {
                last = Some(format!("value: {}", render_capped(&value)));
            }
            Err(error) if retryable(&error) => {
                last = Some(format!("error: {}", render_capped(&error)));
            }
            Err(error) => return Err(WaitError::Terminal(error)),
        }
        let elapsed = ticker.now().saturating_sub(start);
        if elapsed >= deadline {
            return Err(WaitError::Timeout(WaitTimeout {
                elapsed,
                attempts,
                expected: expected.to_string(),
                last,
            }));
        }
        ticker.sleep(poll_interval);
    }
}

/// Poll `run` until it refuses terminally: a retryable error sleeps and
/// retries, a success fails at once with the unexpected value, and a
/// spent deadline returns typed timeout evidence — never the retryable
/// error relabeled as the terminal one.
pub(super) fn wait_for_terminal_error<T, E>(
    ticker: &dyn WaitTicker,
    deadline: Duration,
    poll_interval: Duration,
    expected: &str,
    mut run: impl FnMut() -> Result<T, E>,
    mut retryable: impl FnMut(&E) -> bool,
) -> Result<E, WaitError<UnexpectedSuccess<T>>>
where
    T: fmt::Debug,
    E: fmt::Debug,
{
    let start = ticker.now();
    let mut attempts = 0_u64;
    let mut last: Option<String>;
    loop {
        attempts += 1;
        match run() {
            Ok(value) => return Err(WaitError::Terminal(UnexpectedSuccess(value))),
            Err(error) if retryable(&error) => {
                last = Some(format!("error: {}", render_capped(&error)));
            }
            Err(error) => return Ok(error),
        }
        let elapsed = ticker.now().saturating_sub(start);
        if elapsed >= deadline {
            return Err(WaitError::Timeout(WaitTimeout {
                elapsed,
                attempts,
                expected: expected.to_string(),
                last,
            }));
        }
        ticker.sleep(poll_interval);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use super::{UnexpectedSuccess, WaitError, WaitTicker, wait_for, wait_for_terminal_error};

    /// Virtual clock: `sleep` advances time without touching the wall
    /// clock, so every seam below runs in microseconds.
    #[derive(Default)]
    struct FakeTicker {
        now: Cell<Duration>,
        sleeps: Cell<u64>,
    }

    impl WaitTicker for FakeTicker {
        fn now(&self) -> Duration {
            self.now.get()
        }

        fn sleep(&self, duration: Duration) {
            self.now.set(self.now.get().saturating_add(duration));
            self.sleeps.set(self.sleeps.get().saturating_add(1));
        }
    }

    #[test]
    fn immediate_ready_returns_the_first_value_without_sleeping() {
        let ticker = FakeTicker::default();
        let attempts = Cell::new(0_u64);
        let value: u64 = wait_for(
            &ticker,
            Duration::from_secs(5),
            Duration::from_millis(10),
            "immediate value",
            || {
                attempts.set(attempts.get().saturating_add(1));
                Ok::<u64, String>(7)
            },
            |value| *value == 7,
            |_| false,
        )
        .expect("an immediately ready value returns");
        assert_eq!(value, 7);
        assert_eq!(attempts.get(), 1);
        assert_eq!(ticker.sleeps.get(), 0);
    }

    #[test]
    fn retry_then_ready_returns_the_value() {
        let ticker = FakeTicker::default();
        let calls = Cell::new(0_u64);
        let value: u64 = wait_for(
            &ticker,
            Duration::from_secs(5),
            Duration::from_millis(10),
            "eventual value",
            || {
                let call = calls.get().saturating_add(1);
                calls.set(call);
                Ok::<u64, String>(call)
            },
            |value| *value >= 3,
            |_| false,
        )
        .expect("a value that becomes ready returns");
        assert_eq!(value, 3);
        assert_eq!(ticker.sleeps.get(), 2);
    }

    #[test]
    fn never_ready_returns_typed_timeout_evidence() {
        let ticker = FakeTicker::default();
        let error = wait_for(
            &ticker,
            Duration::from_millis(25),
            Duration::from_millis(10),
            "the value that never comes",
            || Ok::<u64, String>(1),
            |_| false,
            |_| false,
        )
        .expect_err("a never-ready wait fails");
        let WaitError::Timeout(timeout) = error else {
            panic!("a never-ready wait times out, it never succeeds: {error:?}");
        };
        assert_eq!(timeout.attempts, 4);
        assert!(timeout.elapsed >= Duration::from_millis(25));
        assert_eq!(timeout.expected, "the value that never comes");
        let last = timeout.last.as_ref().expect("the last value is evidence");
        assert!(last.contains('1'), "the last value is kept: {last}");
        let rendered = timeout.to_string();
        assert!(
            rendered.contains("the value that never comes") && rendered.contains("4 attempts"),
            "the timeout renders its evidence: {rendered}"
        );
    }

    #[test]
    fn terminal_error_returns_at_once_without_sleeping() {
        let ticker = FakeTicker::default();
        let error = wait_for(
            &ticker,
            Duration::from_secs(5),
            Duration::from_millis(10),
            "no value",
            || Err::<u64, String>("REFUSED".to_string()),
            |_| true,
            |_| false,
        )
        .expect_err("a terminal error fails");
        assert!(
            matches!(&error, WaitError::Terminal(message) if message.as_str() == "REFUSED"),
            "the terminal error passes through unwrapped: {error:?}"
        );
        assert_eq!(ticker.sleeps.get(), 0);
    }

    #[test]
    fn retryable_error_timeout_preserves_the_last_error_without_relabeling_it() {
        let ticker = FakeTicker::default();
        let error = wait_for(
            &ticker,
            Duration::from_millis(25),
            Duration::from_millis(10),
            "readiness",
            || Err::<u64, String>("NOT_READY".to_string()),
            |_| true,
            |message| message == "NOT_READY",
        )
        .expect_err("a retryable error that never clears fails");
        let WaitError::Timeout(timeout) = error else {
            panic!("a spent deadline is a timeout, never a terminal error: {error:?}");
        };
        assert_eq!(timeout.attempts, 4);
        let last = timeout.last.expect("the last error is evidence");
        assert!(
            last.contains("NOT_READY"),
            "the last retryable error is kept: {last}"
        );
    }

    #[test]
    fn unexpected_success_is_terminal_evidence() {
        let ticker = FakeTicker::default();
        let error = wait_for_terminal_error(
            &ticker,
            Duration::from_secs(5),
            Duration::from_millis(10),
            "a typed refusal",
            || Ok::<u64, String>(7),
            |_| false,
        )
        .expect_err("a success while waiting for an error fails");
        assert!(
            matches!(&error, WaitError::Terminal(UnexpectedSuccess(7))),
            "the unexpected value is terminal evidence: {error:?}"
        );
        assert_eq!(ticker.sleeps.get(), 0);
    }

    #[test]
    fn terminal_error_wait_returns_the_refusal_and_times_out_typed() {
        let ticker = FakeTicker::default();
        let error = wait_for_terminal_error(
            &ticker,
            Duration::from_secs(5),
            Duration::from_millis(10),
            "a typed refusal",
            || Err::<u64, String>("REFUSED".to_string()),
            |message| message == "NOT_READY",
        )
        .expect("a terminal refusal returns");
        assert_eq!(error, "REFUSED");

        let ticker = FakeTicker::default();
        let error = wait_for_terminal_error(
            &ticker,
            Duration::from_millis(25),
            Duration::from_millis(10),
            "a typed refusal",
            || Err::<u64, String>("NOT_READY".to_string()),
            |message| message == "NOT_READY",
        )
        .expect_err("an eternally retryable wait fails");
        let WaitError::Timeout(timeout) = error else {
            panic!("a spent terminal wait times out: {error:?}");
        };
        assert_eq!(timeout.attempts, 4);
        assert_eq!(timeout.expected, "a typed refusal");
    }
}
