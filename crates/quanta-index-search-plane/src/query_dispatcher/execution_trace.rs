//! Backend-invocation truth for the semantic/hybrid/seed/explain routes.
//!
//! Exactly one object observes whether a lane RAN: the route calls
//! `record_*_invocation` immediately before each backend-adapter call and
//! `record_*_contribution` when that lane's rows reach the response. Hit
//! counts, plan shape, and `force_empty` never imply execution on their
//! own — a skipped backend call records nothing, so `force_empty` lanes
//! report zero invocations while executed zero-hit lanes still count.
//!
//! [`LaneExecutionSummaryV1`] is the snapshot every consumer reads: the
//! explanation builders (`engines_executed`, `engines_touched`), the
//! `LaneTraceV1` entries, and (through the explanation) the fanout metric.
//! Provider embedding is deliberately NOT a semantic invocation: embedding
//! is provider work, tracked by provider diagnostics, not lane execution.

use std::cell::Cell;

use quanta_index_contract::EngineTouched;

/// How many times each backend engine was invoked and whether it
/// contributed rows to the response.
///
/// Built only by [`LaneExecutionRecorderV1::summary`]; routes never
/// construct it by hand, so every consumer reads the same observed truth.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct LaneExecutionSummaryV1 {
    pub(super) lexical_invocations: u64,
    pub(super) semantic_invocations: u64,
    pub(super) lexical_contributed: bool,
    pub(super) semantic_contributed: bool,
}

impl LaneExecutionSummaryV1 {
    /// Whether the lexical backend was invoked at least once.
    #[must_use]
    pub(super) const fn lexical_executed(&self) -> bool {
        self.lexical_invocations > 0
    }

    /// Whether the semantic backend was invoked at least once.
    #[must_use]
    pub(super) const fn semantic_executed(&self) -> bool {
        self.semantic_invocations > 0
    }

    /// Engines with at least one invocation, in lane order. This is the
    /// single derivation of `SearchExplanation.engines_executed`.
    pub(super) fn executed_engines(&self) -> Vec<EngineTouched> {
        let mut engines = Vec::new();
        if self.lexical_executed() {
            engines.push(EngineTouched::Lexical);
        }
        if self.semantic_executed() {
            engines.push(EngineTouched::Semantic);
        }
        engines
    }

    /// Engines that contributed rows, in lane order. This is the single
    /// derivation of `SearchExplanation.engines_touched`.
    pub(super) fn touched_engines(&self) -> Vec<EngineTouched> {
        let mut engines = Vec::new();
        if self.lexical_contributed {
            engines.push(EngineTouched::Lexical);
        }
        if self.semantic_contributed {
            engines.push(EngineTouched::Semantic);
        }
        engines
    }
}

/// Mutable per-request recorder feeding [`LaneExecutionSummaryV1`].
///
/// Routes hold one and record at the true backend call: `force_empty`,
/// preflight refusal, and early returns record nothing because they invoke
/// nothing.
///
/// Interior mutability (`&self` recording) is deliberate: the recorder is
/// stack-local to one request thread, and both the admission loop and the
/// fetch closures it drives must record through a shared borrow.
#[derive(Debug, Default)]
pub(super) struct LaneExecutionRecorderV1 {
    lexical_invocations: Cell<u64>,
    semantic_invocations: Cell<u64>,
    lexical_contributed: Cell<bool>,
    semantic_contributed: Cell<bool>,
}

impl LaneExecutionRecorderV1 {
    /// A recorder that has observed nothing yet.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Record one lexical backend invocation. Call immediately before the
    /// adapter call, never for a skipped one.
    pub(super) fn record_lexical_invocation(&self) {
        self.lexical_invocations
            .set(self.lexical_invocations.get().saturating_add(1));
    }

    /// Record one semantic backend invocation. Call immediately before the
    /// adapter call, never for a skipped one.
    pub(super) fn record_semantic_invocation(&self) {
        self.semantic_invocations
            .set(self.semantic_invocations.get().saturating_add(1));
    }

    /// Record that the lexical lane's rows reached the response.
    pub(super) fn record_lexical_contribution(&self) {
        self.lexical_contributed.set(true);
    }

    /// Record that the semantic lane's rows reached the response.
    pub(super) fn record_semantic_contribution(&self) {
        self.semantic_contributed.set(true);
    }

    /// Snapshot the observed truth for the explanation, lane traces, and
    /// (through them) the fanout metric.
    #[must_use]
    pub(super) fn summary(&self) -> LaneExecutionSummaryV1 {
        LaneExecutionSummaryV1 {
            lexical_invocations: self.lexical_invocations.get(),
            semantic_invocations: self.semantic_invocations.get(),
            lexical_contributed: self.lexical_contributed.get(),
            semantic_contributed: self.semantic_contributed.get(),
        }
    }
}
