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
//!
//! ## Build patterns
//!
//! Both [`index::SemanticIndexBuilder`] and [`hnsw::HnswIndexBuilder`]
//! support three build patterns, mirroring the lexical-adapter delta API
//! (see also `docs/ssot/producer-handoff.md` §3.5.5 — cross-generation
//! delta):
//!
//! 1. **Scratch / append**: construct via `new(...)`, call
//!    `add_embedding(doc_id, embedding)` for each document, then
//!    `finish()`. The append surface is intentionally NOT replay-safe;
//!    re-issuing `add_embedding` for the same `doc_id` returns
//!    [`errors::SemanticErrorCode::IndexCorrupted`]. Use this pattern
//!    only when driving a fresh build from an authoritative snapshot.
//!
//! 2. **Upsert-driven** (replay-safe): construct via `new(...)`, call
//!    `upsert_embedding(doc_id, embedding)` for each `(doc_id, vector)`
//!    pair. Re-issuing the same upsert is a no-op at the wire level;
//!    a different vector for an existing `doc_id` REPLACES the prior
//!    entry. `remove_embedding(doc_id)` is the matching `DeleteChunk`
//!    handler. Channel subscribers that may replay events after a crash
//!    MUST use this pattern.
//!
//! 3. **Cross-generation incremental**: construct via
//!    `from_prior(&prior, new_generation)` with the previous generation's
//!    finished index. Apply any `upsert_embedding` / `remove_embedding`
//!    deltas, then `finish()`. Carries forward every `(doc_id, vector)`
//!    pair from the prior in ascending [`types::DocId`] order so gen N+1
//!    inherits gen N's full corpus plus new deltas, rather than
//!    rebuilding from scratch. The HNSW variant rebuilds the layered
//!    graph deterministically per `(seed, prior.docs)`; note that graph
//!    topology is path-dependent and the rebuilt graph may differ from
//!    `prior`'s exact neighbour lists.

pub mod cosine;
pub mod errors;
pub mod handle;
pub mod hnsw;
pub mod index;
pub mod query;
pub mod since_time;
pub mod types;

pub use cosine::cosine_similarity;
pub use errors::{LimitDimension, SemanticError, SemanticErrorCode};
pub use handle::{SemanticHandleResolver, SemanticIndexHandleResolver};
pub use hnsw::{
    HnswIndex, HnswIndexBuilder, HnswNode, HnswParams, MAX_EF as HNSW_MAX_EF,
    MAX_LEVEL as HNSW_MAX_LEVEL, MAX_M as HNSW_MAX_M, MIN_M as HNSW_MIN_M, query_hnsw,
};
pub use index::{SemanticIndex, SemanticIndexBuilder};
pub use query::{AnnResult, QueryOpts, query_cosine_topk, query_cosine_topk_with};
pub use since_time::{AppliedAtMs, ParsedSince, parse_since_filter};
pub use types::{DistanceMetric, DocId, EXACT_NN_CUTOFF, Embedding, MAX_EMBEDDING_DIM, MAX_TOP_K};
