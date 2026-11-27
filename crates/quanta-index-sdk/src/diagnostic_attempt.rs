//! Diagnostic attempt contracts shared by Native and Legacy transports.

use quanta_index_contract::{DiagnosticAttemptOutcomeV1, DiagnosticAttemptTokenV1};

/// Owned attempt scope returned by an explicitly injected diagnostic source.
/// Dropping without `finish` represents an interrupted attempt.
pub trait DiagnosticAttemptGuardV1: std::fmt::Debug + Send {
    fn token(&self) -> DiagnosticAttemptTokenV1;
    fn finish(self: Box<Self>, outcome: DiagnosticAttemptOutcomeV1);
}

/// Creates a fresh correlation token at each actual transport send boundary.
/// Implementations own trace IDs and clocks; SDK callers do not synthesize IDs.
pub trait DiagnosticAttemptSourceV1: std::fmt::Debug + Send + Sync {
    fn begin_attempt(&self) -> Option<Box<dyn DiagnosticAttemptGuardV1>>;
}
