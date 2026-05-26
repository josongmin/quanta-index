#![forbid(unsafe_code)]

//! PRE-CONF — Conformance corpus runner for the LQ family.
//!
//! Wires together: a TOML corpus loader, a pure `run_corpus` function
//! over [`LqQueryNormalizer`] + [`ConformanceExecutor`] traits, and a
//! `JUnit` XML emitter. Mocks live under [`mocks`] for self-tests; the
//! real normalizer arrives via PRE-NORM.
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
    Corpus, CorpusRow, ExpectedShape, Gate, RowClassification, RuntimeSyntax, load_corpus,
};
pub use errors::{ConformanceError, CorpusLoadError};
pub use report::render_junit;
pub use runner::{
    CandidateShape, ConformanceExecutor, LqQueryNormalizer, Report, ReportSummary, RowOutcome,
    Verdict, run_corpus,
};
