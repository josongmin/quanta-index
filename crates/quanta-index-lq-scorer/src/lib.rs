#![forbid(unsafe_code)]

//! LEX-01 — Per-generation IDF / BM25 scoring foundation for the LQ family.
//!
//! Status: standalone foundation crate. Integration into the lexical adapter
//! lands under a follow-up ticket; until then this crate is consumed via its
//! pure-Rust public surface (no `tantivy` / `rusqlite` / `lance` linkage).
//!
//! Surface guarantees:
//!
//! - Wire shapes use hand-rolled `impl serde::Serialize`/`Deserialize` per
//!   D18 (no proc-macro derives, semgrep-enforced).
//! - Every scoring failure returns [`ScorerError`] carrying a closed
//!   [`ScorerErrorCode`]; no silent failure / no silent fallback.
//! - Same `(generation, query, doc set)` produces byte-identical scores
//!   across runs and processes (proptest enforces).
//! - Final score envelope is `f32 ∈ [0.0, 1.0]` via the closed-form
//!   normalization documented on [`scorer::Bm25Scorer::score_doc`].
//! - Per-generation IDF tables persist via canonical CBOR through
//!   [`idf::IdfTable::serialize_cbor`].

pub mod bm25;
pub mod builder;
pub mod errors;
pub mod idf;
pub mod scorer;

pub use bm25::Bm25Params;
pub use builder::IdfBuilder;
pub use errors::{ScorerError, ScorerErrorCode};
pub use idf::IdfTable;
pub use scorer::{Bm25Scorer, IdfTokenSource};
