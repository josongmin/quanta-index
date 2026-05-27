//! Traits the runner uses to delegate normalization + execution.
//!
//! Keeps the runner parametric so PRE-NORM can wire in the real
//! `parse_and_normalize` pipeline later, while tests substitute mocks.

use crate::corpus::ExpectedShape;
use crate::errors::ConformanceError;

/// LQ query normalizer surface consumed by the runner.
///
/// PRE-NORM lands the real impl; this crate ships a `MockNormalizer`
/// for self-tests. The runner depends only on this trait so swapping
/// the implementation is a wiring change at the binary edge.
pub trait LqQueryNormalizer {
    /// Normalized query value the executor consumes.
    type Normalized;
    /// Typed parse / normalize failure. Mapped to a
    /// [`crate::errors::ConformanceError`] by the runner.
    type Error: core::error::Error;

    /// Parse and normalize a raw query string.
    fn parse_and_normalize(&self, input: &str) -> Result<Self::Normalized, Self::Error>;

    /// Canonical 32-byte hash of the normalized form. Byte-stable
    /// across invocations for equal `q` inputs.
    fn canonical_hash(&self, q: &Self::Normalized) -> [u8; 32];

    /// Map an implementation-specific normalizer error onto the
    /// closed-set `ConformanceError` placeholder. The runner needs
    /// this to compare against `ExpectedShape::Error`.
    fn classify(&self, err: &Self::Error) -> ConformanceError;
}

/// Result candidate count categories that the executor must report.
///
/// The runner compares this against `ExpectedShape` rather than
/// materializing the candidate stream — keeping the trait small.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum CandidateShape {
    /// Zero candidates returned.
    Empty,
    /// Exactly one candidate.
    Single,
    /// `n` candidates where `n >= 2`.
    Multi { count: u32 },
    /// Paginated response with the indicated page size.
    Paginated { page_size: u32 },
}

/// Executor surface — what the runner calls after normalization.
///
/// The mock `MockExecutor` produces canned shapes; a wave-3 real
/// engine will implement this trait against the actual lexical
/// executor stack.
pub trait ConformanceExecutor<N> {
    /// Typed executor failure.
    type Error: core::error::Error;

    /// Run the executor against a normalized query and the row's
    /// declared `ExpectedShape`. Implementations may inspect
    /// `expected` only for hint purposes (e.g. page size).
    fn execute(
        &self,
        normalized: &N,
        expected: &ExpectedShape,
    ) -> Result<CandidateShape, Self::Error>;

    /// Map executor failure onto the closed-set placeholder.
    fn classify(&self, err: &Self::Error) -> ConformanceError;
}
