//! LXE-09 structural policy.
//!
//! Service knobs consulted before dispatching to the live structural producer
//! (`DirectStructuralMaterializer` / `TruthfulSubsetAuthorityMatcher`). The
//! binding cap is conservative; lexical `structural_block_leaf` remains a
//! typed refusal (`LexicalPlannerError::Unimplemented`).

/// Policy knobs for structural queries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StructuralPolicy {
    /// Upper bound on bindings emitted per matched node. Service truncates and
    /// returns the typed prefix if the producer over-reports.
    pub max_bindings_per_match: u32,
    /// Whether the service consults [`super::StructuralProducerPort::readiness`]
    /// before invoking `execute`. Tests may disable to exercise the raw
    /// producer path.
    pub default_readiness_check: bool,
}

impl StructuralPolicy {
    /// Default policy: 256 bindings per match, readiness check on.
    ///
    /// Conservative binding cap for the live subset matcher.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            max_bindings_per_match: 256,
            default_readiness_check: true,
        }
    }
}

impl Default for StructuralPolicy {
    fn default() -> Self {
        Self::defaults()
    }
}
