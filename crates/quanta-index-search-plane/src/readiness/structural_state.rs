//! Structural authority state: chunk universe and parse trees per generation.
//!
//! The record maps are persistent (structurally shared) so the ledger can
//! retain superseded epoch snapshots at the cost of the deltas alone
//! (QI-BB-020 W2); see `history_state`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use imbl::OrdMap;
use quanta_index_contract::AuxEpochV1;
use quanta_index_contract::lex::{ParseTreeRecord, compute_parse_tree_source_hash};
use quanta_index_contract::{ChunkId, ChunkRecord};
use quanta_index_core::{AuxiliaryGenerationKeyV1, CoreError};
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::readiness::keys::{AuthorityKey, TrackAuthorityKey};
use crate::readiness::serde_support::impl_struct_serde;
use crate::readiness::track_state::TrackAuthorityState;

pub(super) fn structural_parse_tree_decode_fail(reason: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: "STR_PARSE_TREE_DECODE_FAIL".to_string(),
        message: reason.into(),
    }
}

pub(crate) fn verify_parse_tree_against_chunk_map(
    chunks: &OrdMap<ChunkId, ChunkRecord>,
    chunk_id: &ChunkId,
    record: &ParseTreeRecord,
    expected_scope_path: Option<&str>,
) -> Result<(), CoreError> {
    let chunk = chunks.get(chunk_id).ok_or_else(|| {
        structural_parse_tree_decode_fail(format!(
            "STR_PARSE_TREE_DECODE_FAIL{{reason=source_chunk_missing, chunk_id=\"{}\"}}",
            chunk_id.as_str()
        ))
    })?;
    if let Some(expected_path) = expected_scope_path
        && chunk.repo_relative_path.as_str() != expected_path
    {
        return Err(structural_parse_tree_decode_fail(format!(
            "STR_PARSE_TREE_DECODE_FAIL{{reason=scope_chunk_path_mismatch, chunk_id=\"{}\", expected_scope=\"{}\", observed_path=\"{}\"}}",
            chunk_id.as_str(),
            expected_path,
            chunk.repo_relative_path.as_str(),
        )));
    }
    let expected_hash = compute_parse_tree_source_hash(chunk.text.as_ref());
    if record.source_hash != expected_hash {
        return Err(structural_parse_tree_decode_fail(format!(
            "STR_PARSE_TREE_DECODE_FAIL{{reason=source_hash_mismatch, chunk_id=\"{}\"}}",
            chunk_id.as_str()
        )));
    }
    Ok(())
}

/// One generation's chunk universe and parse trees.
///
/// The record maps are private: a parse tree is written only after it
/// was verified against the chunk it names, and scope-level replace or
/// tombstone removes by path through the methods here; nothing else
/// writes them.
#[derive(Clone, Debug, Default)]
pub struct StructuralAuthorityState {
    chunks: OrdMap<ChunkId, ChunkRecord>,
    parse_trees: OrdMap<ChunkId, ParseTreeRecord>,
    seal_requested: bool,
}

/// The part of a structural generation's state that is not a record.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct StructuralStateMeta {
    pub(crate) seal_requested: bool,
}

impl StructuralAuthorityState {
    pub(crate) const fn restore_meta(&mut self, meta: StructuralStateMeta) {
        self.seal_requested = meta.seal_requested;
    }

    pub(crate) fn restore_chunk(&mut self, chunk_id: ChunkId, chunk: ChunkRecord) {
        let _previous = self.chunks.insert(chunk_id, chunk);
    }

    pub(crate) fn restore_parse_tree(&mut self, chunk_id: ChunkId, tree: ParseTreeRecord) {
        let _previous = self.parse_trees.insert(chunk_id, tree);
    }

    /// Apply a validated parse-tree delta: removals, then upserts, then
    /// the seal request as the transition computed it.
    pub(crate) fn apply_trees_delta(&mut self, delta: &StructuralTreesDelta) {
        for chunk_id in &delta.removed {
            self.remove_parse_tree(chunk_id);
        }
        for (chunk_id, record) in &delta.upserts {
            self.restore_parse_tree(chunk_id.clone(), record.clone());
        }
        self.seal_requested = delta.seal_requested;
    }

    /// Apply a chunk-universe delta: clear, removals, then upserts.
    pub(crate) fn apply_chunks_delta(&mut self, delta: &StructuralChunksDelta) {
        if delta.clear {
            self.clear_chunks();
        }
        for chunk_id in &delta.removed {
            self.remove_chunk(chunk_id);
        }
        for chunk in &delta.upserts {
            self.restore_chunk(chunk.chunk_id.clone(), chunk.clone());
        }
    }

    /// Drop one chunk from the universe (absent is fine).
    pub(crate) fn remove_chunk(&mut self, chunk_id: &ChunkId) {
        let _removed = self.chunks.remove(chunk_id);
    }

    /// Empty the chunk universe.
    pub(crate) fn clear_chunks(&mut self) {
        self.chunks.clear();
    }

    /// Replace every chunk at `path` with `chunks`.
    pub(crate) fn replace_scope_chunks(&mut self, path: &str, chunks: Vec<ChunkRecord>) {
        self.tombstone_scope_chunks(path);
        for chunk in chunks {
            self.restore_chunk(chunk.chunk_id.clone(), chunk);
        }
    }

    /// Drop every chunk at `path`.
    pub(crate) fn tombstone_scope_chunks(&mut self, path: &str) {
        for chunk_id in self.chunk_ids_at_path(path) {
            self.remove_chunk(&chunk_id);
        }
    }

    /// Write one parse tree after verifying it against the chunk it
    /// names.
    pub(crate) fn upsert_parse_tree(
        &mut self,
        chunk_id: ChunkId,
        record: ParseTreeRecord,
    ) -> Result<(), CoreError> {
        verify_parse_tree_against_chunk_map(&self.chunks, &chunk_id, &record, None)?;
        self.restore_parse_tree(chunk_id, record);
        Ok(())
    }

    /// Drop one parse tree (absent is fine).
    pub(crate) fn remove_parse_tree(&mut self, chunk_id: &ChunkId) {
        let _removed = self.parse_trees.remove(chunk_id);
    }

    /// Replace every parse tree at `path` with `trees`, each verified
    /// against its chunk and the scope path before anything is removed.
    pub(crate) fn replace_scope_parse_trees(
        &mut self,
        path: &str,
        trees: Vec<(ChunkId, ParseTreeRecord)>,
    ) -> Result<(), CoreError> {
        for (chunk_id, record) in &trees {
            verify_parse_tree_against_chunk_map(&self.chunks, chunk_id, record, Some(path))?;
        }
        self.tombstone_scope_parse_trees(path);
        for (chunk_id, record) in trees {
            self.restore_parse_tree(chunk_id, record);
        }
        Ok(())
    }

    /// Drop every parse tree whose chunk sits at `path`.
    pub(crate) fn tombstone_scope_parse_trees(&mut self, path: &str) {
        for chunk_id in self.chunk_ids_at_path(path) {
            self.remove_parse_tree(&chunk_id);
        }
    }

    /// The chunk ids of one path in the chunk universe.
    fn chunk_ids_at_path(&self, path: &str) -> BTreeSet<ChunkId> {
        self.chunks
            .iter()
            .filter(|(_chunk_id, chunk)| chunk.repo_relative_path.as_str() == path)
            .map(|(chunk_id, _chunk)| chunk_id.clone())
            .collect()
    }

    #[must_use]
    pub fn chunks(&self) -> &OrdMap<ChunkId, ChunkRecord> {
        &self.chunks
    }

    #[must_use]
    pub fn parse_trees(&self) -> &OrdMap<ChunkId, ParseTreeRecord> {
        &self.parse_trees
    }

    #[must_use]
    pub fn seal_requested(&self) -> bool {
        self.seal_requested
    }

    pub fn request_seal(&mut self) {
        self.seal_requested = true;
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct StructuralAuthoritySnapshot {
    pub(super) entries: BTreeMap<AuthorityKey, StructuralAuthorityState>,
    pub(super) tracks: BTreeMap<TrackAuthorityKey, TrackAuthorityState>,
}
impl_struct_serde!(StructuralStateMeta {
    seal_requested: bool,
});

impl_struct_serde!(StructuralAuthorityState {
    chunks: OrdMap<ChunkId, ChunkRecord>,
    parse_trees: OrdMap<ChunkId, ParseTreeRecord>,
    seal_requested: bool,
});

impl_struct_serde!(StructuralAuthoritySnapshot {
    entries: BTreeMap<AuthorityKey, StructuralAuthorityState>,
    tracks: BTreeMap<TrackAuthorityKey, TrackAuthorityState>,
});

/// What one structural batch changes: parse trees removed and written,
/// the seal request after the batch, and the structural track's state
/// after the batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StructuralTreesDelta {
    pub(crate) generation: AuxiliaryGenerationKeyV1,
    /// The epoch the snapshot after this delta has.
    pub(crate) epoch: AuxEpochV1,
    pub(crate) removed: BTreeSet<ChunkId>,
    pub(crate) upserts: Vec<(ChunkId, ParseTreeRecord)>,
    pub(crate) seal_requested: bool,
    pub(crate) track: TrackAuthorityState,
    /// Whether the track sealed in this batch (the seal was requested and
    /// trees exist), which the caller reports on its receipt.
    pub(crate) sealed_track: bool,
}

/// What one search-corpus batch changes in the structural chunk universe.
///
/// The generation's meta is carried unchanged so the generation exists in
/// the catalog even for a batch without chunks: a published empty chunk
/// universe is materialized, an absent one is not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StructuralChunksDelta {
    pub(crate) generation: AuxiliaryGenerationKeyV1,
    /// The epoch the snapshot after this delta has.
    pub(crate) epoch: AuxEpochV1,
    /// Every chunk goes before the upserts (a `Chunk` surface clear).
    pub(crate) clear: bool,
    pub(crate) removed: BTreeSet<ChunkId>,
    pub(crate) upserts: Vec<ChunkRecord>,
    pub(crate) meta: StructuralStateMeta,
}
