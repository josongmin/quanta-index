//! Server-side monotonic timings attached to the query response that owns them.
//! These observations never drive ranking, admission, or deadline decisions.

use std::time::{Duration, Instant};

use quanta_index_contract::{QueryStageKindV1, QueryStageTimingV1};

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).map_or(u64::MAX, |value| value)
}

fn count(value: usize) -> u64 {
    u64::try_from(value).map_or(u64::MAX, |value| value)
}

pub(super) fn elapsed(
    stage: QueryStageKindV1,
    started: Instant,
    calls: u32,
    returned_candidates: Option<usize>,
) -> QueryStageTimingV1 {
    measured(stage, started.elapsed(), calls, returned_candidates)
}

pub(super) fn measured(
    stage: QueryStageKindV1,
    duration: Duration,
    calls: u32,
    returned_candidates: Option<usize>,
) -> QueryStageTimingV1 {
    QueryStageTimingV1 {
        stage,
        elapsed_ns: nanos(duration),
        calls,
        returned_candidates: returned_candidates.map(count),
    }
}
