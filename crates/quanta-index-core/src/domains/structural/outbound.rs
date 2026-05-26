//! LXE-09 structural outbound ports.
//!
//! The abstract parse-tree producer the structural service routes through,
//! plus typed readiness and domain error shapes. No vendor tokens
//! (tree-sitter, etc.) appear here; adapters keep those private.

use thiserror::Error;

use super::inbound::StructuralQueryRequest;
use super::types::StructuralMatchCandidate;

/// Readiness reported by the structural producer for a given generation.
///
/// Option B fail-closed semantics: any non-`Ready` value MUST be surfaced as a
/// typed [`StructuralError`] by the service — never silently degraded into
/// regex / text fallback or an empty success.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    #[error("structural: invalid request: {0}")]
    InvalidRequest(String),
    #[error("structural: producer execution failed: {0}")]
    ProducerExecution(String),
}

impl StructuralError {
    /// Stable wire code preserved for IPC consumers.
    ///
    /// These strings are part of the search-plane contract; do not rename
    /// without coordinating with `query_dispatcher.rs`.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::ParseTreeProducerUnavailable => "STR_PRODUCER_PARSE_TREE_UNAVAILABLE",
            Self::GenerationNotReady => "STR_GENERATION_NOT_READY",
            Self::ShardUnavailable => "STR_SHARD_UNAVAILABLE",
            Self::LangNotSupported(_) => "STR_LANG_NOT_SUPPORTED",
            Self::InvalidRequest(_) => "STR_INVALID_REQUEST",
            Self::ProducerExecution(_) => "STR_PRODUCER_EXECUTION_FAILED",
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
    ) -> Result<Vec<StructuralMatchCandidate>, StructuralError>;
}
