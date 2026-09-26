//! Server-side monotonic timings attached to the query response that owns them.
//! These observations never drive ranking, admission, or deadline decisions.

use std::time::{Duration, Instant};

use quanta_index_contract::{QueryStageKindV1, QueryStageTimingV1};

/// Startup-bound stage observation policy. Operational metrics and request
/// deadlines remain active under both policies; neither policy affects ranking.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum QueryStageObservationPolicy {
    #[default]
    Enabled,
    Disabled,
}

impl QueryStageObservationPolicy {
    /// Reject aliases and empty values rather than changing observation silently.
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        match value {
            "enabled" => Ok(Self::Enabled),
            "disabled" => Ok(Self::Disabled),
            _ => Err("query stage observation must be enabled or disabled"),
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }

    pub(super) fn start(self) -> Option<Instant> {
        match self {
            Self::Enabled => Some(Instant::now()),
            Self::Disabled => None,
        }
    }
}

/// Disabled observations allocate no stage storage and carry no invented zeros.
pub(super) struct StageTimings(Option<Vec<QueryStageTimingV1>>);

impl StageTimings {
    pub(super) fn new(policy: QueryStageObservationPolicy, capacity: usize) -> Self {
        Self(match policy {
            QueryStageObservationPolicy::Enabled => Some(Vec::with_capacity(capacity)),
            QueryStageObservationPolicy::Disabled => None,
        })
    }

    pub(super) fn push(&mut self, timing: Option<QueryStageTimingV1>) {
        if let (Some(stages), Some(timing)) = (&mut self.0, timing) {
            stages.push(timing);
        }
    }

    pub(super) fn finish(self) -> Option<Vec<QueryStageTimingV1>> {
        self.0
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).map_or(u64::MAX, |value| value)
}

fn count(value: usize) -> u64 {
    u64::try_from(value).map_or(u64::MAX, |value| value)
}

pub(super) fn elapsed(
    stage: QueryStageKindV1,
    started: Option<Instant>,
    calls: u32,
    returned_candidates: Option<usize>,
) -> Option<QueryStageTimingV1> {
    started.map(|started| measured(stage, started.elapsed(), calls, returned_candidates))
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

#[cfg(test)]
mod tests {
    use super::{QueryStageObservationPolicy, StageTimings, elapsed};
    use quanta_index_contract::QueryStageKindV1;

    #[test]
    fn lexical_observation_cbor_shape_fits_fixed_page_reserve() {
        use crate::query_dispatcher::response_budget::LEXICAL_STAGE_RESERVE_BYTES;
        use quanta_index_contract::QueryStageTimingV1;
        let stages = [
            QueryStageKindV1::LexicalPrepare,
            QueryStageKindV1::LexicalReadView,
            QueryStageKindV1::LexicalSearch,
            QueryStageKindV1::LexicalProject,
        ]
        .map(|stage| QueryStageTimingV1 {
            stage,
            elapsed_ns: u64::MAX,
            calls: u32::MAX,
            returned_candidates: Some(u64::MAX),
        });
        let bytes =
            quanta_index_ipc::cbor_payload_len(&Some(stages)).expect("bounded stage shape encodes");
        assert_eq!(LEXICAL_STAGE_RESERVE_BYTES, 1024);
        assert!(
            bytes < LEXICAL_STAGE_RESERVE_BYTES,
            "lexical stage bytes exceed fixed reserve: {bytes}"
        );
    }

    #[test]
    fn disabled_stage_observation_has_no_clock_storage_or_measurement() {
        let policy = QueryStageObservationPolicy::Disabled;
        assert_eq!(policy.start(), None);
        let mut stages = StageTimings::new(policy, 7);
        stages.push(elapsed(
            QueryStageKindV1::HybridPrepare,
            policy.start(),
            1,
            None,
        ));
        assert!(stages.finish().is_none());
        assert_eq!(policy.as_str(), "disabled");
    }

    #[test]
    fn enabled_stage_observation_retains_real_measurements() {
        let policy = QueryStageObservationPolicy::default();
        let started = policy.start();
        assert!(started.is_some());
        let mut stages = StageTimings::new(policy, 4);
        stages.push(elapsed(QueryStageKindV1::LexicalPrepare, started, 1, None));
        let stages = stages
            .finish()
            .expect("enabled observations allocate storage");
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].calls, 1);
        assert_eq!(policy.as_str(), "enabled");
    }
}
