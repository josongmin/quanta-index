//! Shared fallible conversion of lexical stage durations.

use std::time::Instant;

use quanta_index_core::CoreError;

pub(crate) fn elapsed_stage_ns(started: Instant) -> Result<u64, CoreError> {
    u64::try_from(started.elapsed().as_nanos()).map_err(|error| {
        CoreError::Storage(format!("lexical stage nanoseconds exceed u64: {error}"))
    })
}
