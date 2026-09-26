#![forbid(unsafe_code)]
#![expect(
    clippy::match_same_arms,
    reason = "ast.rs LqFilter::serialize has per-variant explicit arms (Fork/Archived/Visibility) that look structurally identical but bind distinct mode types; merging would require value unification and obscure the variant→key map. Will revisit when PRE-CONTRACT-EXT lands the typed ErrorCode + filter carrier."
)]

//! PRE-NORM — Canonical DSL parser, normalizer, and CBOR/SHA-256 hasher
//! for the LQ family.
//!
//! The implemented parser, normalizer, hasher and limits are the executable
//! authority. Product-level DSL decisions live in
//! `docs/adr/JUN-02-001-search-dsl-authority-and-runtime-contract.md`.
//!
//! Surface guarantees:
//!
//! - Wire shapes use hand-rolled `impl serde::Serialize`/`Deserialize`
//!   per D18 (no proc-macro derives, semgrep-enforced).
//! - All public types carry [`LqSpan`] source anchors so
//!   diagnostics can render under the source string.

pub mod ast;
pub mod errors;
pub mod hasher;
pub mod limits;
pub mod normalizer;
pub mod parser;
pub mod regex_guard;
pub mod tokenizer;

pub use ast::{
    LQ_VERSION_TAG, LqCase, LqCountBound, LqDirective, LqExpr, LqFileScope, LqFilter, LqLeaf,
    LqMetaVar, LqNormalizedQuery, LqOptions, LqPatternType, LqPredicateArg, LqSelect,
    LqStructuralBlock, LqStructuralConstraint, LqStructuralConstraintOperand, LqStructuralExpr,
    LqStructuralHoleMultiplicity, LqStructuralHoleRef, LqStructuralNode, LqType, LqVisibility,
    LqYesNoOnly,
};
pub use errors::{LqParseError, LqParseErrorCode, LqSpan};
