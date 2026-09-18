//! The per-epoch history text index (QI-BB-023 follow-up #1).
//!
//! Implements [`quanta_index_core::HistoryTextIndexPort`]: for every
//! auxiliary epoch of a history generation, one immutable directory
//! holding a commit-message index and a diff-hunk index, analyzed by the
//! crate's one normalizer, sharing unchanged segments with the previous
//! epoch by hard link, and stamped with the normalizer version so an
//! index built under another contract is rebuilt, never served.
//!
//! Module map (dependencies point downward only):
//!
//! - `adapter` — the port implementation. Depends on `publish`,
//!   `searcher`, `manifest`, `layout`.
//! - `publish` — building one epoch under staging and renaming it into
//!   place. Depends on `manifest`, `schema`, `layout`.
//! - `searcher` — an opened epoch; runs `query` under `collector`.
//! - `collector` — bounded relevance selection after a cursor under the
//!   search plane's row predicate. Depends on `schema`.
//! - `query` — lowering a text expression onto one kind's index.
//!   Depends on `schema`.
//! - `manifest` — the per-epoch commitment. Depends on `layout`.
//! - `schema`, `layout`, `bm25` — leaves.

mod adapter;
mod bm25;
mod collector;
mod layout;
mod manifest;
mod publish;
mod query;
mod schema;
mod searcher;

pub use adapter::HistoryTextIndexAdapter;
pub use bm25::{HISTORY_BM25_B, HISTORY_BM25_K1};

#[cfg(test)]
mod tests;
