//! Opt-in diagnostic timers for lexical durability calls.
//!
//! These timings cover the named `sync_all` call, including its userspace and
//! kernel wait. They do not establish physical device completion or crash
//! consistency. A profiling run must retain stderr and the matching source.

use std::io;
use std::time::Instant;

pub(crate) fn enabled() -> bool {
    std::env::var("QUANTA_INDEX_CAUSAL_PROFILE_V1")
        .ok()
        .as_deref()
        == Some("1")
}

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
