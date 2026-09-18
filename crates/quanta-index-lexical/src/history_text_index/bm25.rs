//! The BM25 parameters the history text index scores under.
//!
//! The engine fixes them (`k1 = 1.2`, `b = 0.75`, the Lucene defaults) and
//! exposes no knob; they are named here so the contract a score is
//! computed under is written down once and the in-module oracle test
//! recomputes scores from these constants and the shared tokenizer,
//! which is what pins them to the engine's.
//!
//! The score of one document for one term is
//! `idf * (k1 + 1) * tf / (tf + k1 * (1 - b + b * dl / avgdl))` with
//! `idf = ln(1 + (N - df + 0.5) / (df + 0.5))`, where `N` counts every
//! document of the kind's index, `df` the documents containing the term,
//! `tf` the term's occurrences in the document, `dl` the document's
//! token count and `avgdl` the mean token count over `N`. A boolean
//! query sums the scores of its matching clauses; a phrase scores its
//! occurrence count under the sum of its terms' `idf`. A document
//! superseded by an upsert in the same segment still counts in `N` and
//! `df` until the engine merges the segment away, so a score is a
//! property of the epoch's index and is the same for every continuation
//! of that epoch.

/// BM25 term-frequency saturation.
pub const HISTORY_BM25_K1: f32 = 1.2;

/// BM25 document-length normalization.
pub const HISTORY_BM25_B: f32 = 0.75;
