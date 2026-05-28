//! Columnar embedding row shard (LDB-01 §4 data contract).
//!
//! Rows carry everything required to rebuild a `LexicalCandidate` at query time
//! without auxiliary replay: the embedding id, repo-relative path, line range,
//! snippet, and the vector payload (stored as little-endian `f32` bytes for a
//! compact, deterministic on-disk form). The generation directory itself is the
//! `(repo, revision, generation)` scope authority, so rows do not repeat it.

#![expect(
    clippy::redundant_pub_crate,
    reason = "module is intentionally crate-internal; pub(crate) is the deliberate visibility — clippy normalizes to redundant but workspace `unreachable_pub = deny` blocks the alternate `pub` form"
)]

use std::collections::BTreeMap;

use quanta_index_core::CoreError;

use crate::codec::{self, cbor_serde};
use crate::manifest::FORMAT_VERSION;

pub(crate) struct SemanticRow {
    pub(crate) embedding_id: String,
    pub(crate) repo_relative_path: String,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    pub(crate) snippet: String,
    pub(crate) vector_le_bytes: Vec<u8>,
}

cbor_serde!(SemanticRow {
    embedding_id: String,
    repo_relative_path: String,
    start_line: u32,
    end_line: u32,
    snippet: String,
    vector_le_bytes: Vec<u8>,
});

pub(crate) struct DatasetShard {
    pub(crate) format_version: u32,
    pub(crate) dimension: u32,
    pub(crate) rows: Vec<SemanticRow>,
}

cbor_serde!(DatasetShard {
    format_version: u32,
    dimension: u32,
    rows: Vec<SemanticRow>,
});

impl DatasetShard {
    pub(crate) fn encode(&self) -> Result<Vec<u8>, CoreError> {
        codec::encode(self, "semantic dataset shard")
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, CoreError> {
        codec::decode(bytes, "semantic dataset shard")
    }
}

/// Mutable working representation of a generation's rows, keyed by embedding id
/// (so replace upserts and tombstone deletes are O(log n) and deterministic).
pub(crate) struct DatasetWorkingSet {
    dimension: u32,
    rows: BTreeMap<String, SemanticRow>,
}

fn expected_vector_byte_len(dimension: u32) -> Result<usize, CoreError> {
    let dim = usize::try_from(dimension)
        .map_err(|err| CoreError::Storage(format!("semantic: dimension overflow: {err}")))?;
    dim.checked_mul(4)
        .ok_or_else(|| CoreError::Storage("semantic: dimension * 4 overflows usize".to_string()))
}

impl DatasetWorkingSet {
    pub(crate) fn empty() -> Self {
        Self {
            dimension: 0,
            rows: BTreeMap::new(),
        }
    }

    /// Rehydrate from a decoded shard, failing closed on shape inconsistency.
    pub(crate) fn from_shard(shard: DatasetShard) -> Result<Self, CoreError> {
        if shard.format_version != FORMAT_VERSION {
            return Err(CoreError::Storage(format!(
                "semantic: dataset shard format version {} unsupported (expected {FORMAT_VERSION})",
                shard.format_version
            )));
        }
        if shard.dimension == 0 && !shard.rows.is_empty() {
            return Err(CoreError::Storage(
                "semantic: dataset shard declares zero dimension but carries rows".to_string(),
            ));
        }
        let mut rows: BTreeMap<String, SemanticRow> = BTreeMap::new();
        if shard.dimension != 0 {
            let expected = expected_vector_byte_len(shard.dimension)?;
            for row in shard.rows {
                if row.vector_le_bytes.len() != expected {
                    return Err(CoreError::Storage(format!(
                        "semantic: row `{}` vector byte length {} != expected {expected}",
                        row.embedding_id,
                        row.vector_le_bytes.len()
                    )));
                }
                match rows.entry(row.embedding_id.clone()) {
                    std::collections::btree_map::Entry::Occupied(_) => {
                        return Err(CoreError::Storage(format!(
                            "semantic: duplicate embedding id `{}` in dataset shard",
                            row.embedding_id
                        )));
                    }
                    std::collections::btree_map::Entry::Vacant(slot) => {
                        let _row = slot.insert(row);
                    }
                }
            }
        }
        Ok(Self {
            dimension: shard.dimension,
            rows,
        })
    }

    pub(crate) fn dimension(&self) -> u32 {
        self.dimension
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub(crate) fn row_count(&self) -> Result<u64, CoreError> {
        u64::try_from(self.rows.len())
            .map_err(|err| CoreError::Storage(format!("semantic: row count overflow: {err}")))
    }

    /// Establish (or confirm) the working dimension; rejects a conflicting one.
    pub(crate) fn set_dimension(&mut self, dimension: u32) -> Result<(), CoreError> {
        if dimension == 0 {
            return Ok(());
        }
        if self.dimension == 0 {
            self.dimension = dimension;
            return Ok(());
        }
        if self.dimension != dimension {
            return Err(CoreError::InvalidContract(format!(
                "semantic: batch dimension {dimension} conflicts with generation dimension {}",
                self.dimension
            )));
        }
        Ok(())
    }

    pub(crate) fn remove_path(&mut self, repo_relative_path: &str) {
        self.rows
            .retain(|_id, row| row.repo_relative_path != repo_relative_path);
    }

    pub(crate) fn upsert(&mut self, row: SemanticRow) {
        let _prior = self.rows.insert(row.embedding_id.clone(), row);
    }

    pub(crate) fn to_shard(&self) -> DatasetShard {
        let mut rows: Vec<SemanticRow> = Vec::with_capacity(self.rows.len());
        for row in self.rows.values() {
            rows.push(SemanticRow {
                embedding_id: row.embedding_id.clone(),
                repo_relative_path: row.repo_relative_path.clone(),
                start_line: row.start_line,
                end_line: row.end_line,
                snippet: row.snippet.clone(),
                vector_le_bytes: row.vector_le_bytes.clone(),
            });
        }
        DatasetShard {
            format_version: FORMAT_VERSION,
            dimension: self.dimension,
            rows,
        }
    }

    /// Decode `(embedding_id, vector)` pairs in deterministic id order for graph
    /// construction at seal time.
    pub(crate) fn graph_rows(&self) -> Result<Vec<(String, Vec<f32>)>, CoreError> {
        let mut out: Vec<(String, Vec<f32>)> = Vec::with_capacity(self.rows.len());
        for (id, row) in &self.rows {
            let vector = codec::le_bytes_to_f32_vec(&row.vector_le_bytes)?;
            out.push((id.clone(), vector));
        }
        Ok(out)
    }
}
