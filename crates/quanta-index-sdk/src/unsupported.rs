//! Local SDK refusal metadata. This is not a Native request or wire version.

use quanta_index_contract::{SearchPlaneQueryIpcRequest, native_protocol::NativeIndexErrorCode};

/// Version of the local unsupported-query diagnostic contract, independent of
/// package, Native wire and persisted state versions.
pub const UNSUPPORTED_QUERY_DIAGNOSTIC_VERSION: u32 = 1;

/// A Legacy query contract unavailable on a Native connection. The corresponding
/// Native reader may expose a different API with different authority and results.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LegacyQueryCapabilityV1 {
    ResolveActiveGeneration,
    ResolveLexicalGeneration,
    Text,
    Symbol,
    Semantic,
    SemanticWorkBoundedV1,
    Hybrid,
    HybridFileV1,
    GoTestTraceFileV1,
    HybridSeed,
    History,
    RuntimeMetadata,
    Structural,
    RepoMapQuery,
    Explain,
    ClusterMembershipRead,
}

impl LegacyQueryCapabilityV1 {
    #[must_use]
    pub const fn code(self) -> NativeIndexErrorCode {
        NativeIndexErrorCode::CapabilityUnavailable
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ResolveActiveGeneration => "legacy.resolve_active_generation",
            Self::ResolveLexicalGeneration => "legacy.resolve_lexical_generation",
            Self::Text => "legacy.text",
            Self::Symbol => "legacy.symbol",
            Self::Semantic => "legacy.semantic",
            Self::SemanticWorkBoundedV1 => "legacy.semantic_work_bounded",
            Self::Hybrid => "legacy.hybrid",
            Self::HybridFileV1 => "legacy.hybrid_file",
            Self::GoTestTraceFileV1 => "legacy.go_test_trace_file",
            Self::HybridSeed => "legacy.hybrid_seed",
            Self::History => "legacy.history",
            Self::RuntimeMetadata => "legacy.runtime_metadata",
            Self::Structural => "legacy.structural",
            Self::RepoMapQuery => "legacy.repomap",
            Self::Explain => "legacy.explain",
            Self::ClusterMembershipRead => "legacy.cluster_membership",
        }
    }

    pub(crate) fn from_request(request: &SearchPlaneQueryIpcRequest) -> Self {
        // Exhaustive over the canonical request. New routes require an explicit
        // refusal identity; no string inference or conversion to Native DTOs.
        match request {
            SearchPlaneQueryIpcRequest::ResolveActiveGeneration(_) => Self::ResolveActiveGeneration,
            SearchPlaneQueryIpcRequest::ResolveLexicalGeneration(_) => {
                Self::ResolveLexicalGeneration
            }
            SearchPlaneQueryIpcRequest::Text(_) => Self::Text,
            SearchPlaneQueryIpcRequest::Symbol(_) => Self::Symbol,
            SearchPlaneQueryIpcRequest::Semantic(_) => Self::Semantic,
            SearchPlaneQueryIpcRequest::SemanticWorkBoundedV1(_) => Self::SemanticWorkBoundedV1,
            SearchPlaneQueryIpcRequest::Hybrid(_) => Self::Hybrid,
            SearchPlaneQueryIpcRequest::HybridFileV1(_) => Self::HybridFileV1,
            SearchPlaneQueryIpcRequest::GoTestTraceFileV1(_) => Self::GoTestTraceFileV1,
            SearchPlaneQueryIpcRequest::HybridSeed(_) => Self::HybridSeed,
            SearchPlaneQueryIpcRequest::History(_) => Self::History,
            SearchPlaneQueryIpcRequest::RuntimeMetadata(_) => Self::RuntimeMetadata,
            SearchPlaneQueryIpcRequest::Structural(_) => Self::Structural,
            SearchPlaneQueryIpcRequest::RepoMapQuery(_) => Self::RepoMapQuery,
            SearchPlaneQueryIpcRequest::Explain(_) => Self::Explain,
            SearchPlaneQueryIpcRequest::ClusterMembershipRead(_) => Self::ClusterMembershipRead,
        }
    }
}
