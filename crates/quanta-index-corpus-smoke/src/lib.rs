#![forbid(unsafe_code)]

//! Corpus parser / runner smoke test for the LQ family.
//!
//! Wires together: a TOML corpus loader, a pure `run_corpus` function
//! over [`LqQueryNormalizer`] + [`ConformanceExecutor`] traits, and a
//! `JUnit` XML emitter. Mocks live under [`mocks`] so the crate can
//! self-test the corpus parser + runner shape end-to-end without any
//! real producer or search-plane wiring.
//!
//! This crate is intentionally narrow: it is a smoke test of the
//! corpus parser + runner skeleton, not a cross-plane conformance
//! gate. The real conformance gate arrives via PRE-NORM (separately).
//!
//! Surface guarantees:
//!
//! - Wire shapes use hand-rolled serde per D18 (no proc-macro derives).
//! - Every loader / runner failure path is a typed [`CorpusLoadError`]
//!   or [`ConformanceError`] — no `panic!`, `unwrap`, `expect`, or
//!   silent fallback.
//! - [`Verdict::Pending`] is the explicit gate signal; the runner never
//!   silently skips an active row.

pub mod corpus;
pub mod errors;
pub mod mocks;
pub mod report;
pub mod runner;

pub use corpus::{
    Corpus, CorpusRow, ExpectedShape, ExpectedStructuralBinding, Gate, RowClassification,
    RuntimeRoute, RuntimeSyntax, load_corpus,
};
pub use errors::{ConformanceError, CorpusLoadError};
pub use report::render_junit;
pub use runner::{
    CandidateShape, ConformanceExecutor, LqQueryNormalizer, Report, ReportSummary, RowOutcome,
    Verdict, run_corpus,
};
