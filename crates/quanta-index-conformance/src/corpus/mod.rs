//! Corpus model + loader.
//!
//! A `Corpus` is the in-memory representation of a TOML golden file
//! describing N rows from `usecase.md`. The loader is fail-closed:
//! unknown fields, missing fields, and bad gate / `gating_ticket`
//! combinations all surface as typed `CorpusLoadError` variants.

pub mod loader;
mod model;

pub use loader::load_corpus;
pub use model::{Corpus, CorpusRow, ExpectedShape, Gate};
