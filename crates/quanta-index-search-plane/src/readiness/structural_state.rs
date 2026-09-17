//! Structural authority state: chunk universe and parse trees per generation.
//!
//! The record maps are persistent (structurally shared) so the ledger can
//! retain superseded epoch snapshots at the cost of the deltas alone
//! (QI-BB-020 W2); see `history_state`.

use std::collections::BTreeMap;
use std::fmt;

use imbl::OrdMap;
use quanta_index_contract::lex::{ParseTreeRecord, compute_parse_tree_source_hash};
use quanta_index_contract::{ChunkId, ChunkRecord};
use quanta_index_core::CoreError;
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

pub(super) fn verify_parse_tree_against_chunk(
    state: &StructuralAuthorityState,
    chunk_id: &ChunkId,
    record: &ParseTreeRecord,
    expected_scope_path: Option<&str>,
) -> Result<(), CoreError> {
    verify_parse_tree_against_chunk_map(&state.chunks, chunk_id, record, expected_scope_path)
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

#[derive(Clone, Debug, Default)]
pub struct StructuralAuthorityState {
    pub(super) chunks: OrdMap<ChunkId, ChunkRecord>,
    pub(super) parse_trees: OrdMap<ChunkId, ParseTreeRecord>,
    pub(super) seal_requested: bool,
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
