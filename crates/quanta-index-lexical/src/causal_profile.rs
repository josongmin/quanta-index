//! Opt-in diagnostic timers for lexical durability calls.
//!
//! These timings cover the named `sync_all` call, including its userspace and
//! kernel wait. They do not establish physical device completion or crash
//! consistency. A profiling run must retain stderr and the matching source.

use std::io;
use std::time::Instant;

pub(crate) fn enabled() -> bool {
    std::env::var_os("QUANTA_INDEX_CAUSAL_PROFILE_V1")
        .is_some_and(|value| value == std::ffi::OsStr::new("1"))
}

#[expect(
    clippy::print_stderr,
    reason = "The opt-in causal capture contract consumes these diagnostic markers from stderr."
)]
pub(crate) fn timed_sync(
    label: &'static str,
    operation: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let started = enabled().then(Instant::now);
    let result = operation();
    if let Some(started) = started {
        eprintln!(
            "QI_CAUSAL_V1 kind=sync label={label} ok={} elapsed_ns={}",
            u8::from(result.is_ok()),
            started.elapsed().as_nanos()
        );
    }
    result
}

/// Trace one ingest/proof span without changing its result.
#[expect(
    clippy::print_stderr,
    reason = "Opt-in ingest diagnosis writes source-bound span markers."
)]
pub(crate) fn timed_work<T, E>(
    label: &'static str,
    operation: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let started = enabled().then(Instant::now);
    let result = operation();
    if let Some(started) = started {
        eprintln!(
            "QI_INGEST_TRACE_V1 label={label} ok={} elapsed_ns={}",
            u8::from(result.is_ok()),
            started.elapsed().as_nanos()
        );
    }
    result
}
