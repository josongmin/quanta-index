//! LXE-09 structural policy.
//!
//! Placeholder knobs the service consults before dispatching to the producer.
//! Kept minimal; LXE-10+ will expand these as real producer adapters land.

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
    /// The value is conservative; once real producers land it will be re-tuned.
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
