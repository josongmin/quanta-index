//! Durable semantic generation layout (LDB-01 shape authority).
//!
//! One directory per `(repo, revision, generation)` under the adapter's
//! semantic state root (`{state_root}/indexes/semantic` at the composition
//! root). The lancedb dataset lives in a `dataset/` subdir (its own files +
//! manifest are managed by lancedb itself); our scope-level manifest and
//! readiness/seal markers sit alongside it:
//!
//! ```text
//! {semantic_root}/generation-v1-{sha256(repo, revision)}/g{generation}/
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

use crate::manifest::format_capabilities_v1;
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{CoreError, domains::generation::GenerationStorageKeyV1};

pub(crate) const DATASET_DIR_NAME: &str = "dataset";
pub(crate) const BUILD_CONTRACT_FILE_NAME: &str = "semantic-build-contract.cbor";
pub(crate) const MANIFEST_FILE_NAME: &str = "semantic-manifest.cbor";
pub(crate) const MARKER_READY_FILE_NAME: &str = "MARKER_READY";
pub(crate) const MARKER_SEALED_FILE_NAME: &str = "MARKER_SEALED";

/// Lancedb table name for the semantic dataset.
pub(crate) const TABLE_NAME: &str = "semantic";
pub(crate) const CLUSTER_MEMBERSHIP_TABLE_NAME: &str = "cluster_membership";

pub(crate) const COLUMN_MEMBERSHIP_CLUSTER_RECORD_ID: &str = "cluster_record_id";
pub(crate) const COLUMN_MEMBERSHIP_AUTHORITY_DIGEST: &str = "authority_digest";
pub(crate) const COLUMN_MEMBERSHIP_OWNER_KIND: &str = "owner_kind";
pub(crate) const COLUMN_MEMBERSHIP_OWNER_ID: &str = "owner_id";
pub(crate) const COLUMN_MEMBERSHIP_MEMBER_SYMBOL_ID: &str = "member_symbol_id";
pub(crate) const COLUMN_MEMBERSHIP_ORDINAL: &str = "ordinal";
pub(crate) const COLUMN_MEMBERSHIP_MEMBER_COUNT: &str = "member_count";
pub(crate) const COLUMN_MEMBERSHIP_CONTENT_DIGEST: &str = "membership_content_digest";

// Column names for the semantic table. This is the single source of truth for
// the physical schema shared by the build path (schema + record batch) and the
// search path (column extraction) — neither reaches into the other for it.
pub(crate) const COLUMN_EMBEDDING_ID: &str = "embedding_id";
pub(crate) const COLUMN_RECORD_ID: &str = "record_id";
pub(crate) const COLUMN_REPO_RELATIVE_PATH: &str = "repo_relative_path";
pub(crate) const COLUMN_OWNER_ID: &str = "owner_id";
pub(crate) const COLUMN_OWNER_KIND: &str = "owner_kind";
pub(crate) const COLUMN_CORPUS_KIND: &str = "corpus_kind";
pub(crate) const COLUMN_PARENT_OWNER_ID: &str = "parent_owner_id";
pub(crate) const COLUMN_SOURCE_DOC_ID: &str = "source_doc_id";
pub(crate) const COLUMN_LANGUAGE: &str = "language";
pub(crate) const COLUMN_PACKAGE: &str = "package";
pub(crate) const COLUMN_SYMBOL_KIND: &str = "symbol_kind";
pub(crate) const COLUMN_VISIBILITY: &str = "visibility";
pub(crate) const COLUMN_SOURCE_ROLE: &str = "source_role";
pub(crate) const COLUMN_GENERATED: &str = "generated";
pub(crate) const COLUMN_CAPABILITY_STATUS: &str = "capability_status";
pub(crate) const COLUMN_AUTHORITY_DIGEST: &str = "authority_digest";
pub(crate) const COLUMN_RENDER_POLICY_DIGEST: &str = "render_policy_digest";
pub(crate) const COLUMN_CARD_SCHEMA_VERSION: &str = "card_schema_version";
pub(crate) const COLUMN_EMBEDDING_INPUT_DIGEST: &str = "embedding_input_digest";
pub(crate) const COLUMN_VECTOR_DIGEST: &str = "vector_digest";
pub(crate) const COLUMN_START_LINE: &str = "start_line";
pub(crate) const COLUMN_END_LINE: &str = "end_line";
pub(crate) const COLUMN_SNIPPET: &str = "snippet";
pub(crate) const COLUMN_VECTOR: &str = "vector";

/// Arrow schema for the legacy v3 lancedb `semantic` table at the given vector dimension.
pub(crate) fn semantic_schema_v3(dimension: i32) -> SchemaRef {
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

/// Arrow schema for the v4 lancedb `semantic` table at the given vector dimension.
pub(crate) fn semantic_schema(dimension: i32) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(COLUMN_EMBEDDING_ID, DataType::Utf8, false),
        Field::new(COLUMN_RECORD_ID, DataType::Utf8, false),
        Field::new(COLUMN_REPO_RELATIVE_PATH, DataType::Utf8, false),
        Field::new(COLUMN_OWNER_ID, DataType::Utf8, false),
        Field::new(COLUMN_OWNER_KIND, DataType::Utf8, false),
        Field::new(COLUMN_CORPUS_KIND, DataType::Utf8, false),
        Field::new(COLUMN_PARENT_OWNER_ID, DataType::Utf8, true),
        Field::new(COLUMN_SOURCE_DOC_ID, DataType::Utf8, false),
        Field::new(COLUMN_LANGUAGE, DataType::Utf8, false),
        Field::new(COLUMN_PACKAGE, DataType::Utf8, true),
        Field::new(COLUMN_SYMBOL_KIND, DataType::Utf8, true),
        Field::new(COLUMN_VISIBILITY, DataType::Utf8, true),
        Field::new(COLUMN_SOURCE_ROLE, DataType::Utf8, false),
        Field::new(COLUMN_GENERATED, DataType::Boolean, false),
        Field::new(COLUMN_CAPABILITY_STATUS, DataType::Utf8, false),
        Field::new(COLUMN_AUTHORITY_DIGEST, DataType::Utf8, false),
        Field::new(COLUMN_RENDER_POLICY_DIGEST, DataType::Utf8, false),
        Field::new(COLUMN_CARD_SCHEMA_VERSION, DataType::UInt32, false),
        Field::new(COLUMN_EMBEDDING_INPUT_DIGEST, DataType::Utf8, false),
        Field::new(COLUMN_VECTOR_DIGEST, DataType::Utf8, false),
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

pub(crate) fn cluster_membership_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(COLUMN_MEMBERSHIP_CLUSTER_RECORD_ID, DataType::Utf8, false),
        Field::new(COLUMN_MEMBERSHIP_AUTHORITY_DIGEST, DataType::Utf8, false),
        Field::new(COLUMN_MEMBERSHIP_OWNER_KIND, DataType::Utf8, false),
        Field::new(COLUMN_MEMBERSHIP_OWNER_ID, DataType::Utf8, false),
        Field::new(COLUMN_MEMBERSHIP_MEMBER_SYMBOL_ID, DataType::Utf8, false),
        Field::new(COLUMN_MEMBERSHIP_ORDINAL, DataType::UInt32, false),
        Field::new(COLUMN_MEMBERSHIP_MEMBER_COUNT, DataType::UInt32, false),
        Field::new(COLUMN_MEMBERSHIP_CONTENT_DIGEST, DataType::Utf8, false),
    ]))
}

/// The table schema a manifest format version was written with.
///
/// The physical layout changed once, at the corpus-metadata format; every
/// later format (membership, row root, file commitment, index seal) added
/// sidecars and manifest fields, not columns.
pub(crate) fn semantic_schema_for_manifest_version(
    format_version: u32,
    dimension: i32,
) -> Result<SchemaRef, CoreError> {
    let capabilities = format_capabilities_v1(format_version).ok_or_else(|| {
        CoreError::Storage(format!(
            "semantic: no physical schema registered for manifest format version {format_version}"
        ))
    })?;
    if capabilities.corpus_metadata() {
        Ok(semantic_schema(dimension))
    } else {
        Ok(semantic_schema_v3(dimension))
    }
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

/// `{semantic_root}/{bounded_generation_storage_key}/g{generation}/`
pub(crate) fn generation_dir(
    semantic_root: &Path,
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
) -> PathBuf {
    GenerationStorageKeyV1::for_repo_revision(repo, revision)
        .generation_dir(semantic_root, generation)
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

#[cfg(test)]
mod tests {
    use std::path::Component;

    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};

    #[test]
    fn generation_dir_contains_untrusted_identifiers_under_semantic_root() {
        let root = std::path::Path::new("/state/indexes/semantic");
        let path = super::generation_dir(
            root,
            &RepoId::new("../../outside"),
            &RevisionId::new("/absolute/revision"),
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
