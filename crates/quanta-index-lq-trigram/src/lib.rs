#![forbid(unsafe_code)]

//! LEX-02 — Byte-trigram inverted index for substring and regex prefilter.
//!
//! This crate is a pure data-structure foundation: it builds a per-generation
//! byte-trigram posting map, serializes the index to canonical CBOR, and
//! answers two query shapes that the chunk content index cannot answer
//! without a full scan:
//!
//! 1. raw-string substring leaves (`'…'` per the LQ DSL §3.3) via
//!    [`query::query_raw_substring`].
//! 2. regex prefilter over mandatory literals (extracted by a sibling
//!    ticket, e.g. LEX-04) via [`regex_prefilter::regex_prefilter`].
//!
//! ## Why **byte** trigrams (not code-point trigrams)
//!
//! The indexed substrate is bytes (raw strings preserve bytes; LEX-00
//! normalization does not apply to `'…'` leaves). UTF-8
//! self-synchronization guarantees that any byte-trigram that crosses a
//! code-point boundary is still a meaningful discriminator on the indexed
//! corpus, even though it has no "character" interpretation. False positives
//! produced by trigram-boundary collisions are caught by the
//! `memchr::memmem::find` verify step on the candidate documents — the
//! trigram shard is a candidate authority, the verify pass is truth.
//!
//! ## Build patterns
//!
//! [`TrigramIndexBuilder`] supports three patterns, all of which finalise
//! through the same [`TrigramIndexBuilder::finish`] surface:
//!
//! 1. **Scratch / append** — [`TrigramIndexBuilder::new`] then repeated
//!    [`TrigramIndexBuilder::add_doc`]. NOT replay-safe; callers driving
//!    a full snapshot rebuild use this pattern.
//! 2. **Upsert-driven** — [`TrigramIndexBuilder::new`] then repeated
//!    [`TrigramIndexBuilder::upsert_doc`] / [`TrigramIndexBuilder::remove_doc`].
//!    Replay-safe: re-issuing `upsert_doc(doc_id, ...)` after a crash
//!    REPLACES the prior content. Subscriber loops that consume
//!    `UpsertChunk` / `DeleteChunk` events from the producer use this
//!    pattern.
//! 3. **Cross-generation incremental** —
//!    [`TrigramIndexBuilder::from_prior`] with the previous generation's
//!    [`TrigramIndex`] and a bumped generation id, then optional
//!    [`TrigramIndexBuilder::upsert_doc`] / [`TrigramIndexBuilder::remove_doc`]
//!    deltas. Lets gen N+1 inherit from gen N rather than rebuilding from
//!    scratch.
//!
//! See `docs/ssot/producer-handoff.md` §3 for the producer/search-plane
//! delta-handling contract these patterns satisfy.
//!
//! ## Guarantees
//!
//! - Wire shapes use hand-rolled `impl serde::Serialize` / `Deserialize`
//!   per D18 (no proc-macro derives, semgrep-enforced).
//! - Every failure returns a [`TrigramError`] carrying a closed
//!   [`TrigramErrorCode`]; no silent failure / no silent fallback.
//! - The same upsert/remove sequence produces a byte-identical CBOR
//!   encoding of the index. `upsert_doc` is idempotent and replay-safe.
//! - Caps surface [`TrigramErrorCode::PlanLimitExceeded`] carrying a
//!   [`LimitDimension`] tag; no silent degradation.

pub mod builder;
pub mod errors;
pub mod index;
pub mod query;
pub mod regex_prefilter;
pub mod types;

pub use builder::TrigramIndexBuilder;
pub use errors::{LimitDimension, TrigramError, TrigramErrorCode};
pub use index::TrigramIndex;
pub use query::{DocResolver, query_raw_substring};
pub use regex_prefilter::regex_prefilter;
pub use types::{
    DocId, MAX_CANDIDATE_PRE_VERIFY, MAX_TRIGRAMS_PER_QUERY, TRIGRAM_LEN, Trigram, trigrams_of,
};
