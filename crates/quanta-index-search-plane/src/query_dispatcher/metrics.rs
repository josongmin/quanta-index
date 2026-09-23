//! The closed query-route metric taxonomy: route names, error-metric
//! classification, and metric value shaping.

use quanta_index_contract::{SearchPlaneErrorCodeV2 as Code, lex::LexicalErrorCode as Lexical};
use quanta_index_core::CoreError;

/// The closed set of query routes, for the per-route metric names
/// (QI-BB-015).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QueryRoute {
    ActiveResolution,
    LexicalResolution,
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
            Self::ActiveResolution => "active_resolution",
            Self::LexicalResolution => "lexical_resolution",
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

    /// `lq_route_<route>_<suffix>`: eleven routes times a closed set of
    /// suffixes (latency, served, errors, deadline exceeded, cancelled,
    /// examined candidates), and nothing from a request.
    pub(super) fn metric_name(self, suffix: &str) -> String {
        format!("lq_route_{}_{suffix}", self.name())
    }
}

/// The per-route counter a budget interruption lands in, by its wire code
/// (QI-BB-002): a deadline and a peer cancellation are different events
/// and are never folded into one metric.
pub(super) fn interruption_route_suffix(code: Code) -> Option<&'static str> {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "only deadline and cancellation have a dedicated route counter; every other code, including future ones, has none"
    )]
    match code {
        Code::RequestDeadlineExceeded => Some("deadline_exceeded_total"),
        Code::RequestCancelled => Some("cancelled_total"),
        _ => None,
    }
}

/// The candidates a route materialized before it cut the page, as the
/// `lq_route_<route>_examined_candidates_total` counter reports them
/// (QI-BB-015).
///
/// The window's candidate count is a lower bound of what the route
/// observed, exact when the page was not cut.
pub(super) fn examined_candidates_metric(
    window: &quanta_index_contract::QueryResultWindowV2,
) -> f64 {
    quanta_index_core::count_as_f64(window.candidate_count().lower_bound())
}

/// Milliseconds as a metric value, exact for any duration a request can
/// take.
pub(super) fn elapsed_millis_metric(elapsed: std::time::Duration) -> f64 {
    u32::try_from(elapsed.as_millis()).map_or(f64::MAX, f64::from)
}

pub(super) fn classify_error_metric_name(err: &CoreError) -> &'static str {
    match err {
        CoreError::Typed { code, .. } => classify_typed_error_metric_name(*code),
        CoreError::NotReady(_) => "lq_typed_error_not_ready_total",
        CoreError::Storage(_) => "lq_typed_error_internal_total",
        CoreError::InvalidContract(_) => "lq_typed_error_invalid_request_total",
        CoreError::NotImplemented(_) | CoreError::NotFound(_) => "lq_typed_error_unavailable_total",
    }
}

fn classify_typed_error_metric_name(code: Code) -> &'static str {
    #[expect(
        clippy::wildcard_enum_match_arm,
        reason = "the `_other_total` bucket is the deliberate catch-all for every code without a dedicated counter, including future ones"
    )]
    match code {
        Code::RequestDeadlineExceeded => "lq_typed_error_deadline_exceeded_total",
        Code::RequestCancelled => "lq_typed_error_cancelled_total",
        Code::Lexical(
            Lexical::RegexParse
            | Lexical::ParseFail
            | Lexical::StrParseFail
            | Lexical::StrParseTreeDecodeFail
            | Lexical::BridgeTranslateFail
            | Lexical::SemInvalidVector
            | Lexical::StrHoleKindUnsupported,
        )
        | Code::LexRegexDialectParseError => "lq_typed_error_parse_total",
        Code::RuntimeDirtyOnlyUnsupported => "lq_typed_error_invalid_request_total",
        Code::Lexical(
            Lexical::StrProducerParseTreeUnavailable
            | Lexical::SemProviderUnavailable
            | Lexical::HistoryRefNotFound,
        )
        | Code::FileContributorUnavailable
        | Code::FileOwnershipUnavailable
        | Code::HistoryProducerUnavailable
        | Code::HistoryRepoCommitRecencyUnavailable
        | Code::HistoryRelevanceUnavailable
        | Code::HistoryShardUnavailable
        | Code::HistoryTextIndexNormalizerUnsupported
        | Code::LexFilterArchivedUnavailable
        | Code::LexFilterAuthorUnavailable
        | Code::LexFilterCommitterUnavailable
        | Code::LexFilterContextUnavailable
        | Code::LexFilterDirtyUnavailable
        | Code::LexFilterForkUnavailable
        | Code::LexFilterMessageUnavailable
        | Code::LexFilterRevUnavailable
        | Code::LexFilterRuntimeCatalogUnavailable
        | Code::LexFilterVisibilityUnavailable
        | Code::RepoDescriptionUnavailable
        | Code::RepoMetaUnavailable
        | Code::RepoTopicUnavailable
        | Code::NotFound
        | Code::NotImplemented
        | Code::RuntimeCatalogChunkUniverseUnavailable
        | Code::StrShardUnavailable => "lq_typed_error_unavailable_total",
        Code::Lexical(Lexical::PlanLimitExceeded | Lexical::QueryTimeout)
        | Code::IngestResourceBudgetExceeded
        | Code::LexicalExaminedBudgetExceeded
        | Code::LexPhrasePlanLimitExceeded
        | Code::LexRegexBudgetExceeded
        | Code::LexTrigramPlanLimitExceeded => "lq_typed_error_plan_limit_total",
        Code::HistoryGenerationNotReady
        | Code::HistoryTextIndexNotReady
        | Code::NotReady
        | Code::RuntimeCatalogNotReady
        | Code::RuntimeNotReady
        | Code::StrGenerationNotReady
        | Code::Lexical(Lexical::SemNotReady | Lexical::StateNotReady) => {
            "lq_typed_error_not_ready_total"
        }
        _ => "lq_typed_error_other_total",
    }
}

pub(super) fn metric_count_value(count: usize) -> f64 {
    u32::try_from(count).map_or_else(|_| f64::from(u32::MAX), f64::from)
}
