//! Shared `CoreError` constructors for the semantic crate's storage paths.
//!
//! `build.rs` and `search.rs` both wrap lancedb/arrow/fs failures into
//! `CoreError::Storage` with an identical `semantic: …` prefix; keeping the
//! constructors here is the single source of truth for those message shapes.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::path::Path;

use quanta_index_core::CoreError;

pub(crate) fn fs_err(action: &str, path: &Path, err: &std::io::Error) -> CoreError {
    CoreError::Storage(format!("semantic: {action} {}: {err}", path.display()))
}

pub(crate) fn lancedb_err(action: &str, err: impl core::fmt::Display) -> CoreError {
    CoreError::Storage(format!("semantic: lancedb {action}: {err}"))
}

pub(crate) fn arrow_err(action: &str, err: impl core::fmt::Display) -> CoreError {
    CoreError::Storage(format!("semantic: arrow {action}: {err}"))
}
