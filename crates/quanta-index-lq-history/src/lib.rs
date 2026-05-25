#![forbid(unsafe_code)]

//! LEX-07 — History + diff search engine + generation governance.
//!
//! This crate lands the Wave-4 history sublayer per
//! `docs/plans/may-24-lexical-indexing-sorucegraph/tickets/LEX-07.md`. It
//! holds the commit-DAG cache, history filter primitives
//! (`parent:`/`merge:`/`tag:`/`revisions:`/`since.time:`), write-packet
//! trace, and manifest-atomicity ledger.
//!
//! ## Hard locks
//!
//! - `merge:` matches the merge-result-only diff (the merge commit's own
//!   diff), never the merged-in side. `merge:only` is alias-of-yes per
//!   Q-LEX07-5.
//! - `revisions:<range>` is capped at `HISTORY_REVISIONS_MAX_DEFAULT`
//!   (`10_000`); over-cap surfaces
//!   [`errors::HistoryErrorCode::PlanLimitExceeded`] with dimension
//!   [`errors::LimitDimension::RevisionsCount`].
//! - `since.time:` is anchored to the write-packet trace's
//!   [`types::AppliedAtMs`]. Wall-clock `now()` is never consulted.
//! - `parent:` walk is depth-bounded by
//!   `HISTORY_PARENT_DEPTH_MAX_DEFAULT` (64); over-cap surfaces
//!   [`errors::HistoryErrorCode::PlanLimitExceeded`] with dimension
//!   [`errors::LimitDimension::ParentDepth`].
//!
//! ## Guarantees
//!
//! - Wire shapes use hand-rolled `impl serde::Serialize` / `Deserialize`
//!   per D18 (no proc-macro derives, semgrep-enforced).
//! - Every failure path returns a [`errors::HistoryError`] with a closed
//!   [`errors::HistoryErrorCode`]; no silent failure, no silent fallback.
//! - Same commit graph + same query → byte-identical commit set.
//!
//! ## Delta semantics (§3.6 incremental boundary)
//!
//! The [`commit_graph::CommitGraph`] mutators are the only delta surface
//! that downstream search-side state derives from. They give:
//!
//! - [`commit_graph::CommitGraph::upsert_commit`][] — idempotent insert
//!   or replace with strict parent validation. On overwrite, the prior
//!   node is returned so any downstream by-parent cache can be
//!   invalidated. Unknown parent in strict mode surfaces
//!   [`errors::HistoryErrorCode::HistoryCommitParentUnknown`].
//! - [`commit_graph::CommitGraph::remove_commit`][] — conservative
//!   refuse on commits with in-graph children
//!   ([`errors::HistoryErrorCode::HistoryCommitHasChildren`]). Used by
//!   search-side prior-graph reuse; the channel layer never emits a
//!   `DeleteCommit` op (AMB-PROD-3).
//! - [`commit_graph::CommitGraph::from_prior`][] — full-state carry-over
//!   with a fresh generation stamp. The AMB-PROD-3 fresh-generation
//!   force-push handler.
//! - [`commit_graph::CommitGraph::with_buffering`][] — enables
//!   parent-not-yet-present resolution for producer ordering tolerance
//!   (AMB-PROD-1). The pending queue surfaces via
//!   [`commit_graph::CommitGraph::pending`].
//! - [`manifest::ManifestLedger::remove_packet`][] — idempotent removal
//!   of an applied trace by hash for revert / rollback flows.

pub mod commit_graph;
pub mod errors;
pub mod manifest;
pub mod merge_filter;
pub mod parent_walk;
pub mod revisions;
pub mod since_time;
pub mod tag_filter;
pub mod types;
pub mod write_packet;

pub use commit_graph::{CommitGraph, CommitNode};
pub use errors::{HistoryError, HistoryErrorCode, LimitDimension};
pub use manifest::{ApplyOutcome, ManifestLedger};
pub use merge_filter::merge_commits;
pub use parent_walk::parents_within_depth;
pub use revisions::{RevisionRange, enumerate_revisions};
pub use since_time::{ParsedSince, parse_since_filter, since_time};
pub use tag_filter::tag_resolve;
pub use types::{
    AppliedAtMs, CommitSha, HISTORY_PARENT_DEPTH_MAX_DEFAULT, HISTORY_REVISIONS_MAX_DEFAULT,
    HISTORY_TAG_REGEX_NFA_MAX, ManifestGeneration, RepoRelativePath,
};
pub use write_packet::{TraceRecord, WRITE_PACKET_DOMAIN, WritePacket};
