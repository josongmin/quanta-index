//! Canonical LQ-family wire types — additive scaffold for the
//! `PRE-CONTRACT-EXT` consolidation pass.
//!
//! This module lands the contract-side canonical shapes that the 16 `lq_*`
//! crates currently duplicate as private placeholders (per
//! [`docs/plans/may-24-lexical-indexing-sourcegraph/tickets/PRE-CONTRACT-EXT.md`](../../../../docs/plans/may-24-lexical-indexing-sourcegraph/tickets/PRE-CONTRACT-EXT.md)).
//! Wire shapes are pinned in
//! [`docs/ssot/producer-handoff.md`](../../../../docs/ssot/producer-handoff.md)
//! (sections 3.1.1, 3.2, 3.3, 3.4 — `CommitRecord`, `DirtyRecord`,
//! `ParseTreeRecord`, `SymbolRecord`).
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
