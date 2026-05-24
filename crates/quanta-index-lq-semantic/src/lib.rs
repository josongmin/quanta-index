#![forbid(unsafe_code)]

//! SEM-01 — Semantic vector adapter (data-structure layer).
//!
//! This crate ships the pure-Rust building blocks for the SEM-01 semantic
//! vector adapter:
//!
//! 1. typed embeddings with dim and finiteness guards
//!    ([`types::Embedding`]),
//! 2. a closed [`types::DistanceMetric`] enum pinned to cosine at MVP
//!    (L2 and Dot are reserved name-wise but unsupported),
//! 3. a cosine similarity kernel that fails closed on every documented
//!    degradation ([`cosine::cosine_similarity`]),
//! 4. a per-generation [`index::SemanticIndex`] with manual-serde CBOR
//!    persistence,
//! 5. a deterministic exact-NN top-k executor
//!    ([`query::query_cosine_topk`]) with a 100k corpus cutoff above
//!    which we fail closed via [`errors::SemanticErrorCode::SemAnnNondeterministic`]
//!    (HNSW with pinned-seed is deferred per SEM-01 spec §4.4 / risk
//!    R-ANN-DET).
//!
//! Disk-backed Lance integration wires in at the lexical-adapter
//! integration ticket; this crate stays vendor-neutral and pure-data.
//!
//! ## Guarantees
//!
//! - Wire shapes use hand-rolled `impl serde::Serialize` / `Deserialize`
//!   per D18 (no proc-macro derives, semgrep-enforced).
//! - Every failure returns a [`errors::SemanticError`] carrying a closed
//!   [`errors::SemanticErrorCode`]; no silent failure / no silent
//!   fallback / no `panic!` / no `unwrap` / no `expect`.
//! - Same insertion sequence produces a byte-identical CBOR encoding of
//!   the index.
//! - Same `(query, generation)` produces a byte-identical top-k result.
//! - Caps surface [`errors::SemanticErrorCode::PlanLimitExceeded`]
//!   carrying a [`errors::LimitDimension`] tag.

pub mod cosine;
pub mod errors;
pub mod index;
pub mod query;
pub mod types;

pub use cosine::cosine_similarity;
pub use errors::{LimitDimension, SemanticError, SemanticErrorCode};
pub use index::{SemanticIndex, SemanticIndexBuilder};
pub use query::{AnnResult, query_cosine_topk};
pub use types::{DistanceMetric, DocId, EXACT_NN_CUTOFF, Embedding, MAX_EMBEDDING_DIM, MAX_TOP_K};
