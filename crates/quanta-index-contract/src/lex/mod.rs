//! Canonical LQ-family wire types — additive scaffold for the
//! `PRE-CONTRACT-EXT` consolidation pass.
//!
//! This module lands the contract-side canonical shapes that the 16 `lq_*`
//! crates currently duplicate as private placeholders (per
//! [`docs/plans/may-24-lexical-indexing-sorucegraph/tickets/PRE-CONTRACT-EXT.md`](../../../../docs/plans/may-24-lexical-indexing-sorucegraph/tickets/PRE-CONTRACT-EXT.md)).
//! Wire shapes are pinned in
//! [`docs/ssot/producer-handoff.md`](../../../../docs/ssot/producer-handoff.md)
//! (sections 3.1.1, 3.2, 3.3, 3.4 — `CommitRecord`, `DirtyRecord`,
//! `ParseTreeRecord`, `SymbolRecord`).
//!
//! Downstream LQ-crate migration is a separate ticket; this scaffold only
//! lands the canonical types so they can be referenced.
//!
//! Per `CLAUDE.md` D18, every type below carries a manual `impl Serialize` /
//! `impl<'de> Deserialize<'de>`. No proc-macro derives.

pub mod dirty;
pub mod diff;
pub mod error_code;
pub mod explanation;
pub mod history;
pub mod lang;
pub mod parse_tree;
pub mod symbol;

pub use dirty::DirtyRecord;
pub use diff::DiffHunkRecord;
pub use error_code::LexicalErrorCode;
pub use explanation::{ExplanationRow, SearchExplanation};
pub use history::{CommitRecord, CommitSha, CommitShaParseError};
pub use lang::LangId;
pub use parse_tree::{ParseNode, ParseRoleTag, ParseTreeRecord};
pub use symbol::{SymbolKind, SymbolRecord, SymbolRelationship, SymbolSpan};
