//! Durable semantic generation layout (LDB-01 shape authority).
//!
//! One directory per `(repo, revision, generation)` under the adapter's
//! semantic state root (`{state_root}/indexes/semantic` at the composition
//! root). The lancedb dataset lives in a `dataset/` subdir (its own files +
//! manifest are managed by lancedb itself); our scope-level manifest and
//! readiness/seal markers sit alongside it:
//!
//! ```text
//! {semantic_root}/{repo_id}/{revision_id}/g{generation}/
//!   dataset/                  # lancedb dataset root (managed by lancedb)
//!   semantic-build-contract.cbor
//!                            # pre-seal batch contract / base provenance
//!   semantic-manifest.cbor    # our scope-level metadata + integrity gate
//!   MARKER_READY              # rows materialized durably
//!   MARKER_SEALED             # generation finalized; openable for serving
//! ```

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::CoreError;

pub(crate) const DATASET_DIR_NAME: &str = "dataset";
pub(crate) const BUILD_CONTRACT_FILE_NAME: &str = "semantic-build-contract.cbor";
pub(crate) const MANIFEST_FILE_NAME: &str = "semantic-manifest.cbor";
pub(crate) const MARKER_READY_FILE_NAME: &str = "MARKER_READY";
pub(crate) const MARKER_SEALED_FILE_NAME: &str = "MARKER_SEALED";

/// Lancedb table name for the semantic dataset.
pub(crate) const TABLE_NAME: &str = "semantic";

// Column names for the semantic table. This is the single source of truth for
// the physical schema shared by the build path (schema + record batch) and the
// search path (column extraction) — neither reaches into the other for it.
pub(crate) const COLUMN_EMBEDDING_ID: &str = "embedding_id";
pub(crate) const COLUMN_REPO_RELATIVE_PATH: &str = "repo_relative_path";
pub(crate) const COLUMN_START_LINE: &str = "start_line";
pub(crate) const COLUMN_END_LINE: &str = "end_line";
pub(crate) const COLUMN_SNIPPET: &str = "snippet";
pub(crate) const COLUMN_VECTOR: &str = "vector";

/// Arrow schema for the lancedb `semantic` table at the given vector dimension.
pub(crate) fn semantic_schema(dimension: i32) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(COLUMN_EMBEDDING_ID, DataType::Utf8, false),
        Field::new(COLUMN_REPO_RELATIVE_PATH, DataType::Utf8, false),
        Field::new(COLUMN_START_LINE, DataType::UInt32, false),
        Field::new(COLUMN_END_LINE, DataType::UInt32, false),
        Field::new(COLUMN_SNIPPET, DataType::Utf8, false),
        Field::new(
            COLUMN_VECTOR,
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimension,
            ),
            false,
        ),
    ]))
}

pub(crate) fn dimension_to_i32(dimension: usize) -> Result<i32, CoreError> {
    i32::try_from(dimension).map_err(|err| {
        CoreError::Storage(format!(
            "semantic: dimension {dimension} does not fit in i32 (Arrow FixedSizeList list size): {err}"
        ))
    })
}

/// Filesystem path of the lancedb dataset, as the UTF-8 string lancedb's
/// `connect` expects.
pub(crate) fn dataset_uri(generation_dir: &Path) -> Result<String, CoreError> {
    dataset_dir_to_uri(&dataset_dir(generation_dir))
}

/// Convert an already-resolved dataset directory into the UTF-8 URI lancedb's
/// `connect` expects. The single place the not-UTF-8 failure is shaped.
pub(crate) fn dataset_dir_to_uri(dataset_dir: &Path) -> Result<String, CoreError> {
    dataset_dir.to_str().map(str::to_owned).ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: dataset path is not valid UTF-8: {}",
            dataset_dir.display()
        ))
    })
}

/// `{semantic_root}/{repo_id}/{revision_id}/g{generation}/`
pub(crate) fn generation_dir(
    semantic_root: &Path,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
) -> PathBuf {
    semantic_root
        .join(repo.as_str())
        .join(revision.as_str())
        .join(format!("g{}", generation.get()))
}

pub(crate) fn dataset_dir(generation_dir: &Path) -> PathBuf {
    generation_dir.join(DATASET_DIR_NAME)
}

pub(crate) fn build_contract_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(BUILD_CONTRACT_FILE_NAME)
}

pub(crate) fn manifest_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(MANIFEST_FILE_NAME)
}

pub(crate) fn ready_marker_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(MARKER_READY_FILE_NAME)
}

pub(crate) fn sealed_marker_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(MARKER_SEALED_FILE_NAME)
}
