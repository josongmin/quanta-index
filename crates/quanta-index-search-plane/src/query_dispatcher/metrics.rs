//! The closed query-route metric taxonomy: route names, error-metric
//! classification, and metric value shaping.

use quanta_index_core::{CoreError, REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE};

/// The closed set of query routes, for the per-route metric names
/// (QI-BB-015).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QueryRoute {
    Lexical,
    Symbol,
    Semantic,
    Hybrid,
    HybridSeed,
    History,
    Structural,
    RepoMap,
    Explain,
    RuntimeMetadata,
    ClusterMembershipRead,
}

impl QueryRoute {
    const fn name(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::Symbol => "symbol",
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
            Self::HybridSeed => "hybrid_seed",
            Self::History => "history",
            Self::Structural => "structural",
            Self::RepoMap => "repo_map",
            Self::Explain => "explain",
            Self::RuntimeMetadata => "runtime_metadata",
            Self::ClusterMembershipRead => "cluster_membership_read",
        }
    }

    /// `lq_route_<route>_<suffix>`: eleven routes times three suffixes, and
    /// nothing from a request.
    pub(super) fn metric_name(self, suffix: &str) -> String {
        format!("lq_route_{}_{suffix}", self.name())
    }
}

/// Milliseconds as a metric value, exact for any duration a request can
/// take.
pub(super) fn elapsed_millis_metric(elapsed: std::time::Duration) -> f64 {
    u32::try_from(elapsed.as_millis()).map_or(f64::MAX, f64::from)
}

pub(super) fn classify_error_metric_name(err: &CoreError) -> &'static str {
    match err {
        CoreError::Typed { code, .. }
            if code == REQUEST_DEADLINE_EXCEEDED_CODE || code == REQUEST_CANCELLED_CODE =>
        {
            "lq_typed_error_interrupted_total"
        }
        CoreError::Typed { code, .. }
            if code.contains("PARSE")
                || code.contains("TRANSLATE_FAIL")
                || code.contains("INVALID_VECTOR")
                || code.contains("HOLE_KIND_UNSUPPORTED") =>
        {
            "lq_typed_error_parse_total"
        }
        CoreError::Typed { code, .. } if code.contains("DIRTY_ONLY_UNSUPPORTED") => {
            "lq_typed_error_invalid_request_total"
        }
        CoreError::Typed { code, .. }
            if code.contains("UNAVAILABLE")
                || code.contains("NOT_IMPLEMENTED")
                || code.contains("NOT_FOUND") =>
        {
            "lq_typed_error_unavailable_total"
        }
        CoreError::Typed { code, .. }
            if code.contains("PLAN_LIMIT")
                || code.contains("BUDGET_EXCEEDED")
                || code.contains("QUERY_TIMEOUT")
                || code.contains("COUNT_INVALID") =>
        {
            "lq_typed_error_plan_limit_total"
        }
        CoreError::NotReady(_) => "lq_typed_error_not_ready_total",
        CoreError::Typed { code, .. } if code.contains("NOT_READY") => {
            "lq_typed_error_not_ready_total"
        }
        CoreError::Storage(_) => "lq_typed_error_internal_total",
        CoreError::InvalidContract(_) => "lq_typed_error_invalid_request_total",
        CoreError::NotImplemented(_) | CoreError::NotFound(_) => "lq_typed_error_unavailable_total",
        CoreError::Typed { .. } => "lq_typed_error_other_total",
    }
}

pub(super) fn metric_count_value(count: usize) -> f64 {
    u32::try_from(count).map_or_else(|_| f64::from(u32::MAX), f64::from)
}
