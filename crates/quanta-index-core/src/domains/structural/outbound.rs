//! LXE-09 structural outbound ports.
//!
//! The abstract parse-tree producer the structural service routes through,
//! plus typed readiness and domain error shapes. No vendor tokens
//! (tree-sitter, etc.) appear here; adapters keep those private.

use crate::{REQUEST_CANCELLED_CODE, REQUEST_DEADLINE_EXCEEDED_CODE, RequestBudgetV1};
use thiserror::Error;

use super::inbound::StructuralQueryRequest;
use super::types::StructuralMatchCandidate;

/// Readiness reported by the structural producer for a given generation.
///
/// Option B fail-closed semantics: any non-`Ready` value MUST be surfaced as a
/// typed [`StructuralError`] by the service — never silently degraded into
/// regex / text fallback or an empty success.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StructuralReadiness {
    /// Parse-tree producer data exists for the requested generation.
    Ready,
    /// Parse-tree producer ops have not been wired or no parse-tree generation
    /// is materialized. Stable code: `STR_PRODUCER_PARSE_TREE_UNAVAILABLE`.
    ParseTreeProducerUnavailable,
    /// Producer has parse-tree ops but the requested generation has not been
    /// materialized yet. Stable code: `STR_GENERATION_NOT_READY`.
    GenerationNotReady,
    /// The shard backing the requested generation is unavailable. Stable code:
    /// `STR_SHARD_UNAVAILABLE`.
    ShardUnavailable,
    /// The producer rejected the request shape before execution. The payload
    /// must carry a typed reason that can surface as
    /// [`StructuralError::InvalidRequest`].
    InvalidRequest(Box<str>),
    /// The producer hit an internal failure before execution and therefore
    /// must fail closed rather than advertising readiness. The payload must
    /// carry a typed reason that can surface as
    /// [`StructuralError::ProducerExecution`].
    ProducerExecution(Box<str>),
}

/// Typed structural-domain failure.
///
/// Mapped from [`StructuralReadiness`] and from producer execution errors.
/// Every variant carries a stable string code suitable for IPC propagation;
/// see [`StructuralError::code`].
#[derive(Debug, Error)]
pub enum StructuralError {
    #[error("structural: parse-tree producer ops are unavailable; runtime remains fail-closed")]
    ParseTreeProducerUnavailable,
    #[error("structural: requested generation is not yet materialized")]
    GenerationNotReady,
    #[error("structural: shard for requested generation is unavailable")]
    ShardUnavailable,
    #[error("structural: language `{0}` is not supported on the current adapter set")]
    LangNotSupported(String),
    #[error("structural: typed hole kind is not supported on the current adapter set: {0}")]
    HoleKindUnsupported(String),
    #[error("structural: invalid request: {0}")]
    InvalidRequest(String),
    /// A structural regex filter exceeded the shared regex executor's resource budget.
    #[error("structural: regex plan limit exceeded: {0}")]
    RegexPlanLimitExceeded(String),
    #[error("structural: request cancelled: {0}")]
    RequestCancelled(String),
    #[error("structural: request deadline exceeded: {0}")]
    RequestDeadlineExceeded(String),
    #[error("structural: producer execution failed: {0}")]
    ProducerExecution(String),
    /// The request pinned a structural authority epoch the producer no
    /// longer retains (QI-BB-020 W2). Stable code: `AUX_EPOCH_EXPIRED`.
    #[error("structural: {0}")]
    AuxEpochExpired(String),
    /// The request pinned a structural authority epoch newer than the
    /// producer's current one. Stable code: `AUX_EPOCH_UNKNOWN`.
    #[error("structural: {0}")]
    AuxEpochUnknown(String),
}

impl StructuralError {
    /// Stable wire code preserved for IPC consumers.
    ///
    /// These strings are part of the search-plane contract; do not rename
    /// without coordinating with `query_dispatcher.rs`.
    #[must_use]
    pub fn code(&self) -> quanta_index_contract::SearchPlaneErrorCodeV2 {
        use quanta_index_contract::{SearchPlaneErrorCodeV2, lex::LexicalErrorCode};
        match self {
            Self::ParseTreeProducerUnavailable => {
                SearchPlaneErrorCodeV2::Lexical(LexicalErrorCode::StrProducerParseTreeUnavailable)
            }
            Self::GenerationNotReady => SearchPlaneErrorCodeV2::StrGenerationNotReady,
            Self::ShardUnavailable => SearchPlaneErrorCodeV2::StrShardUnavailable,
            Self::LangNotSupported(_) => {
                SearchPlaneErrorCodeV2::Lexical(LexicalErrorCode::StrLangNotSupported)
            }
            Self::HoleKindUnsupported(_) => {
                SearchPlaneErrorCodeV2::Lexical(LexicalErrorCode::StrHoleKindUnsupported)
            }
            Self::InvalidRequest(_) => SearchPlaneErrorCodeV2::StrInvalidRequest,
            Self::RegexPlanLimitExceeded(_) => SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
            Self::RequestCancelled(_) => REQUEST_CANCELLED_CODE,
            Self::RequestDeadlineExceeded(_) => REQUEST_DEADLINE_EXCEEDED_CODE,
            Self::ProducerExecution(_) => SearchPlaneErrorCodeV2::StrProducerExecutionFailed,
            Self::AuxEpochExpired(_) => crate::domains::auxiliary::AUX_EPOCH_EXPIRED_CODE,
            Self::AuxEpochUnknown(_) => crate::domains::auxiliary::AUX_EPOCH_UNKNOWN_CODE,
        }
    }
}

/// Abstract structural producer port.
///
/// Adapters that own parse-tree producer ops (none in-repo yet — LXE-09
/// step 2) implement this trait; the structural service depends only on this
/// `dyn`-compatible port.
///
/// Implementations MUST NOT synthesize matches from source text or regex when
/// parse-tree ops are absent; report
/// [`StructuralReadiness::ParseTreeProducerUnavailable`] from
/// [`Self::readiness`] instead.
pub trait StructuralProducerPort: Send + Sync {
    /// Report readiness for the generation referenced by `request`.
    fn readiness(&self, request: &StructuralQueryRequest) -> StructuralReadiness;

    /// Execute the structural query against the producer's parse-tree data.
    ///
    /// Callers (the structural service) MUST gate this call on
    /// [`Self::readiness`] returning [`StructuralReadiness::Ready`].
    /// Implementations should still defensively return
    /// [`StructuralError::ProducerExecution`] if the underlying parse-tree
    /// data races out from under them.
    fn execute(
        &self,
        request: &StructuralQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<Vec<StructuralMatchCandidate>, StructuralError>;
}
