#![forbid(unsafe_code)]

//! LEX-04 — RE2-class regex executor with NFA budget, dialect filter, and
//! trigram prefilter.
//!
//! This crate wraps the Rust `regex` and `regex_syntax` crates as the
//! authoritative RE2 lane for `LQ/Core-1.0` queries. The pipeline is:
//!
//! 1. parse the pattern with [`regex_syntax::parse`] (typed
//!    [`RegexErrorCode::ParseFail`] on syntax errors);
//! 2. walk the resulting HIR through [`dialect::dialect_filter`] to reject
//!    `lookbehind` / `lookahead` / `backreference` / inline mode-switch
//!    constructs at parse time with [`RegexErrorCode::ForbiddenSyntax`];
//! 3. estimate the upper-bound NFA state count with
//!    [`estimator::estimate_nfa_states`]; exceed the 100k cap →
//!    [`RegexErrorCode::PlanLimitExceeded`] tagged
//!    [`LimitDimension::NfaStates`];
//! 4. extract mandatory byte literals with
//!    [`literal_extract::extract_prefilter_literal_alternation`] for the trigram
//!    prefilter; pure-wildcard patterns surface
//!    [`RegexErrorCode::RegexPrefilterUnusable`] so the caller can route
//!    to a verify-only path explicitly (no silent fallback);
//! 5. compile and verify with [`regex::bytes::Regex`];
//!    [`RegexExecutor::execute_with_budget`] iterates candidates with a
//!    cooperative cancel checkpoint and surfaces
//!    [`RegexErrorCode::QueryTimeout`] when the budget elapses.
//!
//! ## Guarantees
//!
//! - Dependency identity: `Cargo.lock` resolves the workspace's `regex` and
//!   `regex-syntax` requirements; do not infer exact versions from this API.
//! - NFA budget: `100_000` states.
//! - D18: wire shapes use hand-rolled `impl serde::Serialize`.
//! - No silent failure / no silent fallback / no
//!   `panic!`/`unwrap`/`expect`/`todo!`.

pub mod dialect;
pub mod dialect_ast_walk;
pub mod errors;
pub mod estimator;
pub mod executor;
pub mod literal_extract;

pub use dialect::dialect_filter;
pub use dialect_ast_walk::ast_walk_filter;
pub use errors::{ForbiddenKind, LimitDimension, RegexError, RegexErrorCode};
pub use estimator::{MAX_NFA_STATES, estimate_nfa_states};
pub use executor::{RegexCompilationPlan, RegexExecutor};
pub use literal_extract::extract_prefilter_literal_alternation;
pub use quanta_index_lq_trigram::{DocId, DocResolver};
