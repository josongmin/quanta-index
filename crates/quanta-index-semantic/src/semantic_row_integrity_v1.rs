//! Canonical content commitment for the serve-time semantic table.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use arrow_array::{
    Array, BooleanArray, FixedSizeListArray, Float32Array, RecordBatch, StringArray, UInt32Array,
    UInt64Array,
};
use futures::TryStreamExt as _;
use lancedb::query::{ExecutableQuery as _, QueryBase as _};
use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};

use crate::errors::lancedb_err;
use crate::layout::{
    COLUMN_AUTHORITY_DIGEST, COLUMN_CAPABILITY_STATUS, COLUMN_CARD_SCHEMA_VERSION,
    COLUMN_CORPUS_KIND, COLUMN_EMBEDDING_ID, COLUMN_EMBEDDING_INPUT_DIGEST, COLUMN_END_LINE,
    COLUMN_GENERATED, COLUMN_LANGUAGE, COLUMN_OWNER_ID, COLUMN_OWNER_KIND, COLUMN_PACKAGE,
    COLUMN_PARENT_OWNER_ID, COLUMN_RECORD_ID, COLUMN_RENDER_POLICY_DIGEST,
    COLUMN_REPO_RELATIVE_PATH, COLUMN_SNIPPET, COLUMN_SOURCE_DOC_ID, COLUMN_SOURCE_ROLE,
    COLUMN_START_LINE, COLUMN_SYMBOL_KIND, COLUMN_VECTOR, COLUMN_VECTOR_DIGEST, COLUMN_VISIBILITY,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SemanticRowCommitmentV1 {
    pub(crate) root_digest: String,
    pub(crate) row_count: u64,
    pub(crate) coverage: SemanticRowCoverageV1,
    /// Canonical row identities and payload fingerprints in record-ID order.
    /// Moved from the existing root scan; no second dataset read is needed.
    pub(crate) row_fingerprints: Vec<SemanticRowFingerprintV1>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SemanticRowFingerprintV1 {
    pub(crate) record_id: String,
    pub(crate) leaf_digest: [u8; 32],
    /// Lance physical row identity, used only for delta index custody.
    pub(crate) native_row_id: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SemanticRowCoverageV1 {
    pub(crate) present_corpora: Vec<String>,
    pub(crate) card_schema_versions: Vec<u32>,
    pub(crate) render_policy_digests: Vec<String>,
}

#[derive(Eq, PartialEq)]
struct CanonicalSemanticRowV1 {
    embedding_id: String,
    record_id: String,
    leaf_digest: [u8; 32],
    native_row_id: u64,
}

fn column_as<'a, T: Array + 'static>(
    batch: &'a RecordBatch,
    name: &str,
    arrow_type: &str,
) -> Result<&'a T, CoreError> {
    batch
        .column_by_name(name)
        .and_then(|column| column.as_any().downcast_ref::<T>())
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "semantic row root: column `{name}` missing or not {arrow_type}"
            ))
        })
}

fn required_str<'a>(column: &'a StringArray, row: usize, name: &str) -> Result<&'a str, CoreError> {
    if column.is_null(row) {
        return Err(CoreError::Storage(format!(
            "semantic row root: required column `{name}` contains null"
        )));
    }
    Ok(column.value(row))
}

fn optional_str(column: &StringArray, row: usize) -> Option<&str> {
    (!column.is_null(row)).then(|| column.value(row))
}

fn hash_bytes_v1(hasher: &mut Sha256, bytes: &[u8]) -> Result<(), CoreError> {
    let len = u64::try_from(bytes.len()).map_err(|error| {
        CoreError::Storage(format!("semantic row root: field length overflow: {error}"))
    })?;
    hasher.update(len.to_le_bytes());
    hasher.update(bytes);
    Ok(())
}

fn hash_optional_v1(hasher: &mut Sha256, value: Option<&str>) -> Result<(), CoreError> {
    match value {
        Some(value) => {
            hasher.update([1]);
            hash_bytes_v1(hasher, value.as_bytes())?;
        }
        None => hasher.update([0]),
    }
    Ok(())
}

fn encode_sha256_v1(digest: impl IntoIterator<Item = u8>) -> String {
    let mut encoded = String::with_capacity("sha256:".len().saturating_add(64));
    encoded.push_str("sha256:");
    for byte in digest {
        let _written = write!(&mut encoded, "{byte:02x}");
    }
    encoded
}

/// Non-nullable string columns every canonical semantic row commits to.
const REQUIRED_NAMES: [&str; 13] = [
    COLUMN_REPO_RELATIVE_PATH,
    COLUMN_OWNER_ID,
    COLUMN_OWNER_KIND,
    COLUMN_CORPUS_KIND,
    COLUMN_SOURCE_DOC_ID,
    COLUMN_LANGUAGE,
    COLUMN_SOURCE_ROLE,
    COLUMN_CAPABILITY_STATUS,
    COLUMN_AUTHORITY_DIGEST,
    COLUMN_RENDER_POLICY_DIGEST,
    COLUMN_EMBEDDING_INPUT_DIGEST,
    COLUMN_VECTOR_DIGEST,
    COLUMN_SNIPPET,
];

/// Nullable string columns committed as present-or-absent.
const OPTIONAL_NAMES: [&str; 4] = [
    COLUMN_PARENT_OWNER_ID,
    COLUMN_PACKAGE,
    COLUMN_SYMBOL_KIND,
    COLUMN_VISIBILITY,
];

/// Commit the rows and derive manifest coverage in the same table scan.
pub(crate) async fn semantic_row_commitment_v1(
    table: &lancedb::Table,
) -> Result<SemanticRowCommitmentV1, CoreError> {
    let counted = table
        .count_rows(None)
        .await
        .map_err(|error| lancedb_err("count semantic rows for root", error))?;
    let capacity = counted;
    let mut rows = Vec::with_capacity(capacity);
    let mut present_corpora = BTreeSet::new();
    let mut card_schema_versions = BTreeSet::new();
    let mut render_policy_digests = BTreeSet::new();
    let mut stream = table
        .query()
        .with_row_id()
        .execute()
        .await
        .map_err(|error| lancedb_err("query semantic rows for root", error))?;
    while let Some(batch) = stream
        .try_next()
        .await
        .map_err(|error| lancedb_err("stream semantic rows for root", error))?
    {
        if rows
            .len()
            .checked_add(batch.num_rows())
            .is_none_or(|next| next > capacity)
        {
            return Err(CoreError::Storage(
                "semantic row root: stream exceeded counted row bound".to_string(),
            ));
        }
        let native_row_ids = column_as::<UInt64Array>(&batch, "_rowid", "UInt64")?;
        let embedding_ids = column_as::<StringArray>(&batch, COLUMN_EMBEDDING_ID, "Utf8")?;
        let record_ids = column_as::<StringArray>(&batch, COLUMN_RECORD_ID, "Utf8")?;
        let required = REQUIRED_NAMES
            .map(|name| column_as::<StringArray>(&batch, name, "Utf8"))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let optional = OPTIONAL_NAMES
            .map(|name| column_as::<StringArray>(&batch, name, "Utf8"))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let generated = column_as::<BooleanArray>(&batch, COLUMN_GENERATED, "Boolean")?;
        let card_schema = column_as::<UInt32Array>(&batch, COLUMN_CARD_SCHEMA_VERSION, "UInt32")?;
        let corpus_kinds = column_as::<StringArray>(&batch, COLUMN_CORPUS_KIND, "Utf8")?;
        let render_policies =
            column_as::<StringArray>(&batch, COLUMN_RENDER_POLICY_DIGEST, "Utf8")?;
        let starts = column_as::<UInt32Array>(&batch, COLUMN_START_LINE, "UInt32")?;
        let ends = column_as::<UInt32Array>(&batch, COLUMN_END_LINE, "UInt32")?;
        let vectors = column_as::<FixedSizeListArray>(&batch, COLUMN_VECTOR, "FixedSizeList")?;
        for row in 0..batch.num_rows() {
            if native_row_ids.is_null(row)
                || generated.is_null(row)
                || card_schema.is_null(row)
                || starts.is_null(row)
                || ends.is_null(row)
                || vectors.is_null(row)
            {
                return Err(CoreError::Storage(
                    "semantic row root: required scalar/vector column contains null".to_string(),
                ));
            }
            let vector = vectors.value(row);
            let vector = vector
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| {
                    CoreError::Storage("semantic row root: vector child is not Float32".to_string())
                })?;
            let embedding_id = required_str(embedding_ids, row, COLUMN_EMBEDDING_ID)?;
            let record_id = required_str(record_ids, row, COLUMN_RECORD_ID)?;
            let _corpus = present_corpora
                .insert(required_str(corpus_kinds, row, COLUMN_CORPUS_KIND)?.to_owned());
            let _schema = card_schema_versions.insert(card_schema.value(row));
            let _render = render_policy_digests.insert(
                required_str(render_policies, row, COLUMN_RENDER_POLICY_DIGEST)?.to_owned(),
            );
            let mut leaf = Sha256::new();
            leaf.update(b"quanta-index-semantic-row-leaf-v1\0");
            hash_bytes_v1(&mut leaf, record_id.as_bytes())?;
            hash_bytes_v1(&mut leaf, embedding_id.as_bytes())?;
            for (name, column) in REQUIRED_NAMES.iter().zip(required.iter()) {
                hash_optional_v1(&mut leaf, Some(required_str(column, row, name)?))?;
            }
            for column in &optional {
                hash_optional_v1(&mut leaf, optional_str(column, row))?;
            }
            leaf.update([u8::from(generated.value(row))]);
            leaf.update(card_schema.value(row).to_le_bytes());
            leaf.update(starts.value(row).to_le_bytes());
            leaf.update(ends.value(row).to_le_bytes());
            let vector_len = u64::try_from(vector.len()).map_err(|error| {
                CoreError::Storage(format!(
                    "semantic row root: vector length overflow: {error}"
                ))
            })?;
            leaf.update(vector_len.to_le_bytes());
            for value in vector {
                let value = value.ok_or_else(|| {
                    CoreError::Storage("semantic row root: vector contains null".to_string())
                })?;
                if !value.is_finite() {
                    return Err(CoreError::Storage(
                        "semantic row root: vector contains non-finite value".to_string(),
                    ));
                }
                leaf.update(value.to_bits().to_le_bytes());
            }
            rows.push(CanonicalSemanticRowV1 {
                embedding_id: embedding_id.to_owned(),
                record_id: record_id.to_owned(),
                leaf_digest: leaf.finalize().into(),
                native_row_id: native_row_ids.value(row),
            });
        }
    }
    if rows.len() != capacity {
        return Err(CoreError::Storage(format!(
            "semantic row root: streamed {} rows but counted {capacity}",
            rows.len()
        )));
    }
    // Candidate lookup and exact scoring use embedding_id alone inside one
    // generation. Check uniqueness before the canonical root ordering; this
    // also catches a collision carried forward from a delta base.
    let mut seen_native_row_ids = BTreeSet::new();
    for row in &rows {
        if !seen_native_row_ids.insert(row.native_row_id) {
            return Err(CoreError::Storage(
                "semantic row root: duplicate Lance native row ID in generation".to_string(),
            ));
        }
    }
    rows.sort_unstable_by(|left, right| left.embedding_id.cmp(&right.embedding_id));
    if rows
        .iter()
        .zip(rows.iter().skip(1))
        .any(|(left, right)| left.embedding_id == right.embedding_id)
    {
        return Err(CoreError::Storage(
            "semantic row root: duplicate embedding_id in generation".to_string(),
        ));
    }
    rows.sort_unstable_by(|left, right| {
        left.record_id
            .cmp(&right.record_id)
            .then_with(|| left.embedding_id.cmp(&right.embedding_id))
    });
    if rows
        .iter()
        .zip(rows.iter().skip(1))
        .any(|(left, right)| left.record_id == right.record_id)
    {
        return Err(CoreError::Storage(
            "semantic row root: duplicate record_id in generation".to_string(),
        ));
    }
    let mut root = Sha256::new();
    root.update(b"quanta-index-semantic-row-root-v1\0");
    root.update(
        u64::try_from(rows.len())
            .map_err(|error| {
                CoreError::Storage(format!("semantic row root: row count overflow: {error}"))
            })?
            .to_le_bytes(),
    );
    let mut row_fingerprints = Vec::with_capacity(rows.len());
    for row in rows {
        root.update(row.leaf_digest);
        row_fingerprints.push(SemanticRowFingerprintV1 {
            record_id: row.record_id,
            leaf_digest: row.leaf_digest,
            native_row_id: row.native_row_id,
        });
    }
    Ok(SemanticRowCommitmentV1 {
        row_fingerprints,
        root_digest: encode_sha256_v1(root.finalize()),
        row_count: u64::try_from(capacity).map_err(|error| {
            CoreError::Storage(format!("semantic row root: row count overflow: {error}"))
        })?,
        coverage: SemanticRowCoverageV1 {
            present_corpora: present_corpora.into_iter().collect(),
            card_schema_versions: card_schema_versions.into_iter().collect(),
            render_policy_digests: render_policy_digests.into_iter().collect(),
        },
    })
}
