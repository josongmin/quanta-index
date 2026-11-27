//! Existing canonical generation paths only; no Arrow schema or engine URI.

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::domains::generation::GenerationStorageKeyV1;
use std::path::{Path, PathBuf};

pub const SEALED_MANIFEST_FILE_NAME: &str = "semantic-sealed-manifest.cbor";

pub const DATASET_DIR_NAME: &str = "dataset";
pub const DATASET_STAGING_DIR_NAME: &str = "dataset.staging";
pub const BUILD_CONTRACT_FILE_NAME: &str = "semantic-build-contract.cbor";
pub const MANIFEST_FILE_NAME: &str = "semantic-manifest.cbor";
pub const MARKER_READY_FILE_NAME: &str = "MARKER_READY";
pub const MARKER_SEALED_FILE_NAME: &str = "MARKER_SEALED";
pub const SCRUB_RECEIPT_FILE_NAME: &str = "semantic-scrub-receipt.cbor";
pub const QUARANTINE_RECEIPT_FILE_NAME: &str = "semantic-quarantine.cbor";

pub fn generation_dir(
    semantic_root: &Path,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(repo, revision)
        .generation_dir(semantic_root, generation)
}

pub fn dataset_dir(generation_dir: &Path) -> PathBuf {
    generation_dir.join(DATASET_DIR_NAME)
}

pub fn build_contract_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(BUILD_CONTRACT_FILE_NAME)
}

pub fn manifest_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(MANIFEST_FILE_NAME)
}

pub fn ready_marker_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(MARKER_READY_FILE_NAME)
}

pub fn sealed_marker_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(MARKER_SEALED_FILE_NAME)
}

pub fn scrub_receipt_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(SCRUB_RECEIPT_FILE_NAME)
}

pub fn quarantine_receipt_path(generation_dir: &Path) -> PathBuf {
    generation_dir.join(QUARANTINE_RECEIPT_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use std::path::Component;

    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};

    #[test]
    fn generation_dir_contains_untrusted_identifiers_under_semantic_root() {
        let root = std::path::Path::new("/state/indexes/semantic");
        let path = super::generation_dir(
            root,
            &RepoId::new("../../outside").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("/absolute/revision")
                .expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(7),
        );
        let relative = path
            .strip_prefix(root)
            .expect("path must remain under root");
        assert_eq!(relative.components().count(), 2);
        assert!(
            relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
        );
    }
}
