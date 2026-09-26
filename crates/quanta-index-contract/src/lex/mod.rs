//! Canonical LQ-family wire types — additive scaffold for the
//! `PRE-CONTRACT-EXT` consolidation pass.
//!
//! This module owns canonical lexical wire shapes, including `CommitRecord`,
//! `DirtyRecord`, `ParseTreeRecord` and `SymbolRecord`. The types and their
//! serializers below define the exact fields. The SDK/ingress ownership rule
//! is `docs/adr/MAY-27-002-sdk-ingress-and-public-surface-boundary.md`.
//!
//! Per `CLAUDE.md` D18, every type below carries a manual `impl Serialize` /
//! `impl<'de> Deserialize<'de>`. No proc-macro derives.
//!
//! The ranker-explanation surface (`ExplanationRow`, `SearchExplanation`,
//! `SearchExplanationBuilder`, `PlannerTraceEntry`, `PlannerStage`,
//! `EngineTouched`, `EarlyStopReason`, `WeightsHashError`) lives in
//! [`crate::results`]; the names below are convenience
//! re-exports so callers that imported them from `lex::` keep compiling.
//! New code should prefer `crate::results::*` directly.

pub mod diff;
pub mod dirty;
pub mod error_code;
pub mod history;
pub mod parse_tree;
pub mod symbol;

pub use crate::results::{
    EarlyStopReason, EngineTouched, ExplanationRow, PlannerStage, PlannerTraceEntry,
    SearchExplanation, SearchExplanationBuilder, WeightsHashError,
};
pub use diff::DiffHunkRecord;
pub use dirty::DirtyRecord;
pub use error_code::LexicalErrorCode;
pub use history::{CommitRecord, CommitSha, CommitShaParseError};
pub use parse_tree::{ParseNode, ParseRoleTag, ParseTreeRecord, compute_parse_tree_source_hash};
pub use quanta_index_contract_base::LanguageCode;
pub use symbol::{SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship, SymbolSpan};
