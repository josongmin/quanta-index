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

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};

pub(crate) const DATASET_DIR_NAME: &str = "dataset";
pub(crate) const BUILD_CONTRACT_FILE_NAME: &str = "semantic-build-contract.cbor";
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
