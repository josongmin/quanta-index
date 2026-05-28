//! Durable semantic generation layout (LDB-01 shape authority).
//!
//! One directory per `(repo, revision, generation)` under the adapter's
//! semantic state root (`{state_root}/indexes/semantic` at the composition
//! root). The manifest lives beside the dataset; readiness and seal are
//! explicit, generation-local marker files:
//!
//! ```text
//! {semantic_root}/{repo_id}/{revision_id}/g{generation}/
//!   dataset/
//!     rows.cbor          # columnar embedding rows (LDB-01 §4 data contract)
//!     graph.cbor         # persisted HNSW graph (present iff row_count > 0)
//!   semantic-manifest.cbor
//!   MARKER_READY         # rows materialized durably
//!   MARKER_SEALED        # generation finalized; openable for serving
//! ```

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::path::{Path, PathBuf};

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};

pub(crate) const DATASET_DIR_NAME: &str = "dataset";
pub(crate) const ROWS_FILE_NAME: &str = "rows.cbor";
pub(crate) const GRAPH_FILE_NAME: &str = "graph.cbor";
pub(crate) const MANIFEST_FILE_NAME: &str = "semantic-manifest.cbor";
pub(crate) const MARKER_READY_FILE_NAME: &str = "MARKER_READY";
pub(crate) const MARKER_SEALED_FILE_NAME: &str = "MARKER_SEALED";

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

pub(crate) fn rows_path(generation_dir: &Path) -> PathBuf {
    dataset_dir(generation_dir).join(ROWS_FILE_NAME)
}

pub(crate) fn graph_path(generation_dir: &Path) -> PathBuf {
    dataset_dir(generation_dir).join(GRAPH_FILE_NAME)
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
