#![forbid(unsafe_code)]
#![deny(unused_must_use)]
#![deny(clippy::let_underscore_must_use)]
#![deny(clippy::map_err_ignore)]

//! Stable core surface shared by the producer (`semantica-codegraph-v2`) and the
//! search-plane runtime.
//!
//! `quanta-index-contract` composes envelope/request/response shapes on top of
//! this crate. Downstream code that only needs the *core* identifiers,
//! generation pinning, and text-query syntax — i.e. does not need to encode or
//! decode envelopes — should depend on `quanta-index-contract-base` directly to
//! avoid pulling the full envelope compile surface.

#[macro_use]
pub mod macros;

pub mod ids;
pub mod query;
pub mod results;

pub use ids::{
    FileId, GenerationId, ManifestDigest, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
};
pub use query::{GenerationPin, GenerationSelector, TextQueryRequest, TextQuerySyntax};
pub use results::{
    DiffCandidate, DiffHunkSide, HighlightSpan, LexicalCandidate, StructuralBinding,
    StructuralCandidate,
};
