//! Conformance runner core.
//!
//! Pure function `run_corpus(corpus, normalizer, executor) -> Report`.
//! IO stays at the binary edge. The runner short-circuits on
//! non-active gates with a typed [`Verdict::Pending`] and never
//! auto-skips an active row.

mod core;
pub mod normalizer_trait;

pub use core::{Report, ReportSummary, RowOutcome, Verdict, run_corpus};
pub use normalizer_trait::{CandidateShape, ConformanceExecutor, LqQueryNormalizer};
