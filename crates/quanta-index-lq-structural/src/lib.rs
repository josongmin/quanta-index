#![forbid(unsafe_code)]

//! STR-01 — Structural pattern engine.
//!
//! This crate is the per-language structural-search authority. It owns:
//!
//! 1. The closed [`LangId`] ship-set (`Rust`, `Python`, `TypeScript`,
//!    `JavaScript`, `Go`) — a subset of LEX-05's symbol matrix.
//! 2. The pattern IR [`StructuralPattern`] / [`PatternNode`] with
//!    metavariable (`:[name]`) capture grammar.
//! 3. The parse-tree-authority [`StructuralAuthorityMatcher`] trait plus
//!    truthful lowering from contract-layer structural blocks.
//! 4. The serializable [`StructuralBinding`] metavariable carrier (closes
//!    contract gap GAP-03 from the use-case catalog).
//! 5. The bounded-input gates: 256-node + 16-depth + 32-metavar caps that
//!    surface as typed `PlanLimitExceeded` failures.
//!
//! ## Disjointness lock
//!
//! Per RFC § Engine Decomposition § Structural engine and STR-01 §4.6, the
//! structural match envelope ([`StructuralAuthorityCandidate`]) is
//! **disjoint** from the lexical envelope. Metavariable bindings have no
//! `LexicalCandidate` representation; mixing the two erases the bindings.
//! Cross-family ranking is out of scope for this wave.
//!
//! ## Tree-sitter deferral (per LEX-05 precedent / STR-01 §4.8)
//!
//! The STR-01 spec sheet selects tree-sitter native walk + unified pattern
//! IR + per-grammar lowering adapter (Comby and stack-machine-on-RE2
//! rejected per §4.8). We have not pulled `tree-sitter` in this landing
//! because the C-library transitive build dragged cold-build time past
//! the 60 s budget the spec sheet's caveat allows. The parse-tree-authority
//! surface, pattern IR, and metavariable binder are stable so a wider
//! per-language walker can land without re-shaping callers. Until then,
//! [`TruthfulSubsetAuthorityMatcher`] provides the bounded parse-tree
//! authority implementation for the current live subset. See `matcher`
//! module doc.
//!
//! ## Guarantees
//!
//! - Wire shapes use hand-rolled `impl serde::Serialize` / `Deserialize`
//!   per D18 (no proc-macro derives, semgrep-enforced).
//! - Unsupported-language lookup fails closed with
//!   `STR_LANG_NOT_SUPPORTED{lang}` per STR-01 §8.
//! - Pattern caps fail closed with `PLAN_LIMIT_EXCEEDED{dimension}` —
//!   never silent truncation, per RFC § Non-Negotiable Invariants §10.
//! - CBOR encoding of [`StructuralBinding`] /
//!   [`StructuralAuthorityCandidate`] is byte-identical across runs for the
//!   same input (`BTreeMap` canonical key order).

pub mod binding;
pub mod errors;
pub mod matcher;
pub mod pattern;
pub mod types;

pub use binding::{StructuralAuthorityCandidate, StructuralBinding};
pub use errors::{LimitDimension, StructuralError, StructuralErrorCode};
pub use matcher::{
    StructuralAuthorityMatcher, StructuralAuthorityPatternError, StructuralAuthorityPatternKind,
    StructuralAuthorityPatternRef, StructuralAuthorityView, TruthfulSubsetAuthorityMatcher,
    compile_authoritative_pattern,
};
pub use pattern::{PatternNode, StructuralPattern};
pub use types::{
    ByteSpan, LangId, MAX_DEPTH, MAX_METAVARS_PER_PATTERN, MAX_STRUCTURAL_NODES, MetaVar,
};
