#![forbid(unsafe_code)]

//! LEX-03 — Phrase position index for adjacency / phrase queries.
//!
//! Per-generation immutable shard mapping `(term, doc) -> Vec<Position>`
//! against a post-normalize token stream from the upstream text
//! normalizer. Positions drive two query paths:
//!
//! 1. Exact phrase (`PhraseQuery`, slop = 0) — `query_phrase` accepts an
//!    ordered slice of normalized terms and emits `PhraseMatch` rows for
//!    every doc whose token stream contains the consecutive run.
//! 2. Bounded-window adjacency — `query_adjacency` accepts two terms and
//!    an [`AdjacencyConfig`] (default window = 8 tokens per `dsl.md §5.3`).
//!
//! ## Invariants (locked)
//!
//! - **Stopword filter is OFF — forever.** Sourcegraph parity
//!   (`feature-scope.md §1.1.1`) plus RFC § Non-Negotiable Invariants §2
//!   forbid hidden semantic widening. A phrase like `"the quick brown fox"`
//!   must hit literally. Lifting this lock requires a DSL minor bump and a
//!   normalizer-surface opt-in.
//! - **Positions are post-normalize.** The index consumes
//!   normalizer-emitted tokens; it does not own tokenization, case folding,
//!   or stemming. Mismatched normalizer versions surface
//!   [`PositionsErrorCode::NormalizerVersionMismatch`] at open time.
//! - **Cross-chunk phrase = empty result, not an error.** Chunk id = doc id;
//!   positions are per-doc. A phrase that legitimately spans a chunk boundary
//!   returns `PhraseMatches { matches: vec![] }`, never a typed failure.
//! - **Adjacency default window = 8 tokens.** Configurable via
//!   [`AdjacencyConfig`]; the ceiling is [`MAX_WINDOW_TOKENS`].
//!
//! ## Wire shape
//!
//! D18 — every serialized shape uses hand-rolled
//! `impl serde::Serialize`/`Deserialize`. Proc-macro derives are banned
//! workspace-wide (semgrep `rust-no-serde-derive`).
//!
//! ## Scope
//!
//! Pure data-structure crate: builder + index + query + CBOR persist. The
//! lexical-adapter integration (Tantivy / disk layout / atomic
//! `MARKER_OK`) lands in a follow-up ticket; this crate has no I/O or
//! sibling-shard concerns.

pub mod adjacency_query;
pub mod builder;
pub mod errors;
pub mod index;
pub mod phrase_query;
pub mod types;
pub mod varint;

pub use adjacency_query::query_adjacency;
pub use builder::PositionsBuilder;
pub use errors::{LimitDimension, PositionsError, PositionsErrorCode};
pub use index::{PositionsIndex, TermPostings, TermPostingsEntry};
pub use phrase_query::{PhraseMatch, PhraseMatches, query_phrase};
pub use types::{
    AdjacencyConfig, DEFAULT_WINDOW_TOKENS, DocId, MAX_ADJACENCY_SCAN_DEPTH, MAX_DOCS_PER_TERM,
    MAX_PHRASE_LEN, MAX_POSITIONS_PER_CELL, MAX_WINDOW_TOKENS, NormalizerVersion, Position,
};
