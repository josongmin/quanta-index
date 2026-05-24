#![forbid(unsafe_code)]

//! SEM-02 — Hybrid lex+sem result fusion.
//!
//! Wave-7 entry point per
//! `docs/plans/may-24-lexical-indexing-sorucegraph/tickets/SEM-02.md`.
//!
//! This crate is the pure data-structure layer for hybrid fusion. It is
//! **generic over lexical/semantic candidate shapes** via the
//! [`LexCandidate`] / [`SemCandidate`] types: any caller that maps its
//! engine outputs to these shapes can drive [`HybridExecutor::execute`]
//! without dragging in either the trigram, semantic, or any other engine
//! crate. This satisfies the spec's "does NOT depend on
//! `quanta-index-lq-trigram`, `quanta-index-lq-semantic`, etc." invariant.
//!
//! ## Shipped
//!
//! - `errors`: closed `HybridErrorCode` (6 variants) + `HybridError`
//! - `types`: `DocId` / `RepoId` / `ManifestGeneration` newtypes +
//!   `CandidateRef`
//! - `strategy`: `FusionStrategy::{Rrf, Weighted}` + `HybridWeights` +
//!   `RRF_DEFAULT_K`
//! - `contribution`: `LexCandidate` / `SemCandidate` / `HybridContribution`
//! - `rrf`: [`fuse_rrf`] — Reciprocal Rank Fusion (scale-free, default)
//! - `weighted`: [`fuse_weighted`] — linear score blend (opt-in)
//! - `merge`: [`order_by_merge_tuple`] — SEM-02 §4.5 8-component sort
//! - `executor`: [`HybridExecutor`] — strategy → fuse → merge → truncate
//!
//! ## Guarantees
//!
//! - Wire shapes use hand-rolled `impl serde::Serialize` / `Deserialize`
//!   per D18 (no proc-macro derives, semgrep-enforced).
//! - Every failure returns a [`HybridError`] carrying a closed
//!   [`HybridErrorCode`]; no silent failure / no silent fallback / no
//!   `panic!` / no `unwrap` / no `expect`.
//! - Same `(lex, sem, strategy, top_k)` produces a byte-identical fused
//!   sequence (the merge tuple is total per SEM-02 §4.5).
//! - `top_k` ceiling is pinned at [`MAX_TOP_K`] = `10_000`.
//! - RRF is the default strategy per SEM-02 §4.4 / ADR-019 candidate;
//!   `WeightedScore` is shipped as opt-in.

pub mod contribution;
pub mod errors;
pub mod executor;
pub mod merge;
pub mod rrf;
pub mod strategy;
pub mod types;
pub mod weighted;

pub use contribution::{HybridContribution, LexCandidate, SemCandidate};
pub use errors::{HybridError, HybridErrorCode};
pub use executor::{HybridExecutor, MAX_TOP_K};
pub use merge::order_by_merge_tuple;
pub use rrf::fuse_rrf;
pub use strategy::{FusionStrategy, HybridWeights, RRF_DEFAULT_K};
pub use types::{CandidateRef, DocId, ManifestGeneration, RepoId};
pub use weighted::fuse_weighted;
