#![forbid(unsafe_code)]

//! LEX-05 — Symbol index for definitions and local references.
//!
//! This crate is the per-language symbol authority that sits next to the
//! lexical content and path authorities. It owns:
//!
//! 1. The closed [`SymbolKind`] enum (v1 subset of the 20-value spec list:
//!    `Function`, `Method`, `Class`, `Struct`, `Enum`, `Trait`,
//!    `Interface`, `Variable`, `Constant`, `Module`, `Macro`,
//!    `TypeAlias`).
//! 2. The per-language [`SymbolExtractor`] trait and lang-keyed
//!    [`ExtractorRegistry`].
//! 3. The serializable [`SymbolIndex`] with builder + by-name / by-kind /
//!    by-doc lookups.
//!
//! ## Reference/def boundary
//!
//! Per the LEX-05 spec §3.5, this crate emits **local lexical positions
//! only** for both definitions and references. Cross-file resolution
//! (which definition does this reference point to?) is **out of scope**
//! and belongs to SEM-01.
//!
//! ## Tree-sitter deferral
//!
//! The spec sheet calls for tree-sitter + per-language `tags.scm`. The C
//! transitive build of `tree-sitter` blew past the 60 s cold-build budget
//! the spec sheet's caveat allows, so this landing ships the stable trait
//! surface plus a [`MockExtractor`] test implementation. Real per-language
//! extractors land in the lexical-adapter integration wave with the
//! tree-sitter grammar pins in workspace `Cargo.toml`. See spec §12.
//!
//! ## Guarantees
//!
//! - Wire shapes use hand-rolled `impl serde::Serialize` / `Deserialize`
//!   per D18 (no proc-macro derives, semgrep-enforced).
//! - Unsupported-language lookup fails closed with
//!   `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED{lang_id}` per spec §3.4.
//! - CBOR encoding of [`SymbolIndex`] is byte-identical across runs with
//!   the same insertion sequence.

pub mod errors;
pub mod extractor;
pub mod index;
pub mod registry;
pub mod symbol_kind;
pub mod types;

pub use errors::{SymbolError, SymbolErrorCode};
pub use extractor::{MockExtractor, SymbolExtractor};
pub use index::{SymbolIndex, SymbolIndexBuilder};
pub use registry::ExtractorRegistry;
pub use symbol_kind::SymbolKind;
pub use types::{ByteSpan, DocId, LangId, Symbol};
