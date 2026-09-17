//! How ingest batches, catalog deltas, and lexical channel ops land on the
//! ledger's auxiliary authority states.

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{CommitRecord, CommitSha, DiffHunkRecord, ParseTreeRecord};
use quanta_index_contract::{
    ChunkId, DirtyIngestBatch, HistoryIngestBatch, ManifestGeneration, RuntimeCatalogIngestBatch,
    SearchCorpusIngestBatch, SearchPlaneTrackKind, SearchScopeSurface, StructuralIngestBatch,
};
use quanta_index_core::CoreError;
use quanta_index_ipc::decode_cbor_payload;

use crate::auxiliary_authority;
use crate::readiness::history_state::HistoryDiffKey;
use crate::readiness::ledger::Ledger;
use crate::readiness::runtime_state::DirtyDocState;
use crate::readiness::structural_state::verify_parse_tree_against_chunk;

impl Ledger {
    /// Apply a search-corpus batch's chunk universe in memory, without
    /// the durable step (tests and in-memory fixtures; production goes
    /// through the transition, the catalog and then the delta).
    pub fn apply_search_corpus_batch(&mut self, batch: &SearchCorpusIngestBatch) {
        let delta = auxiliary_authority::structural_chunks_transition(
            self.structural_state(&batch.repo_id, &batch.revision_id, batch.generation),
            batch,
        );
        self.apply_structural_chunks_delta(&delta);
    }

    /// Validate and apply a history batch in memory, without the durable
    /// step.
    pub fn apply_history_batch(&mut self, batch: &HistoryIngestBatch) -> Result<(), CoreError> {
        let delta = auxiliary_authority::history_transition(
            self.history_state(&batch.repo_id, &batch.revision_id, batch.generation),
            batch,
        )?;
        self.apply_history_delta(&delta);
        Ok(())
    }

    /// Apply a dirty-overlay batch in memory, without the durable step.
    pub fn apply_runtime_batch(&mut self, batch: &DirtyIngestBatch) {
        let delta = auxiliary_authority::runtime_dirty_transition(
            self.runtime_state(&batch.repo_id, &batch.revision_id, batch.generation),
            batch,
        );
        self.apply_runtime_dirty_delta(&delta);
    }

    /// Validate and apply a runtime catalog batch in memory, without the
    /// durable step.
    pub fn apply_runtime_catalog_batch(
        &mut self,
        batch: &RuntimeCatalogIngestBatch,
    ) -> Result<(), CoreError> {
        let delta = auxiliary_authority::runtime_catalog_transition(
            self.structural_state(&batch.repo_id, &batch.revision_id, batch.generation),
            self.runtime_state(&batch.repo_id, &batch.revision_id, batch.generation),
            batch,
        )?;
        self.apply_runtime_catalog_delta(&delta);
        Ok(())
    }

    /// Validate and apply a structural batch's parse trees in memory,
    /// without the durable step and without the track bookkeeping.
    pub fn apply_structural_batch(
        &mut self,
        batch: &StructuralIngestBatch,
    ) -> Result<(), CoreError> {
        let delta = auxiliary_authority::structural_transition(
            self.structural_state(&batch.repo_id, &batch.revision_id, batch.generation),
            self.track_state(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Structural,
            ),
            batch,
        )?;
        self.apply_structural_trees_delta(&delta);
        Ok(())
    }

    /// Apply a validated history delta: the durable rows it was encoded
    /// from are already committed.
    pub(crate) fn apply_history_delta(&mut self, delta: &auxiliary_authority::HistoryDelta) {
        let state = self.history_state_mut(
            &delta.generation.repo_id,
            &delta.generation.revision_id,
            delta.generation.generation,
        );
        for record in &delta.commits {
            let _previous = state.commits.insert(record.sha, record.clone());
        }
        for (changes, map) in [
            (&delta.refs, &mut state.refs),
            (&delta.tags, &mut state.tags),
        ] {
            for change in changes {
                match change {
                    auxiliary_authority::RefChange::Upsert(name, sha) => {
                        let _previous = map.insert(name.clone(), *sha);
                    }
                    auxiliary_authority::RefChange::Delete(name) => {
                        let _removed = map.remove(name.as_ref());
                    }
                }
            }
        }
        for (key, record) in &delta.diff_hunks {
            let _previous = state.diff_hunks.insert(key.clone(), record.clone());
        }
        state.restore_meta(delta.meta);
    }

    /// Apply a dirty-overlay delta.
    pub(crate) fn apply_runtime_dirty_delta(
        &mut self,
        delta: &auxiliary_authority::RuntimeDirtyDelta,
    ) {
        let state = self.runtime_state_mut(
            &delta.generation.repo_id,
            &delta.generation.revision_id,
            delta.generation.generation,
        );
        for (chunk_id, doc) in &delta.upserts {
            let _previous = state.dirty_docs.insert(chunk_id.clone(), doc.clone());
        }
        for chunk_id in &delta.deletes {
            let _removed = state.dirty_docs.remove(chunk_id);
        }
    }

    /// Apply a validated runtime catalog delta: the generation's catalog is
    /// replaced whole.
    pub(crate) fn apply_runtime_catalog_delta(
        &mut self,
        delta: &auxiliary_authority::RuntimeCatalogDelta,
    ) {
        let state = self.runtime_state_mut(
            &delta.generation.repo_id,
            &delta.generation.revision_id,
            delta.generation.generation,
        );
        state.restore_meta(delta.meta.clone());
        state.changed_docs.clone_from(&delta.changed_docs);
        state.doc_facets.clone_from(&delta.doc_facets);
        state.snapshots.clone_from(&delta.snapshots);
        state.affected_docs.clone_from(&delta.affected_docs);
        state
            .invalidated_by_docs
            .clone_from(&delta.invalidated_by_docs);
    }

    /// Apply a validated structural delta: parse trees, the seal request
    /// and the structural track's state.
    pub(crate) fn apply_structural_trees_delta(
        &mut self,
        delta: &auxiliary_authority::StructuralTreesDelta,
    ) {
        let state = self.structural_state_mut(
            &delta.generation.repo_id,
            &delta.generation.revision_id,
            delta.generation.generation,
        );
        state
            .parse_trees
            .retain(|chunk_id, _tree| !delta.removed.contains(chunk_id));
        for (chunk_id, record) in &delta.upserts {
            let _previous = state.parse_trees.insert(chunk_id.clone(), record.clone());
        }
        state.seal_requested = delta.seal_requested;
        self.restore_track_state(
            &delta.generation.repo_id,
            &delta.generation.revision_id,
            SearchPlaneTrackKind::Structural,
            delta.track.clone(),
        );
    }

    /// Apply a chunk-universe delta.
    pub(crate) fn apply_structural_chunks_delta(
        &mut self,
        delta: &auxiliary_authority::StructuralChunksDelta,
    ) {
        let state = self.structural_state_mut(
            &delta.generation.repo_id,
            &delta.generation.revision_id,
            delta.generation.generation,
        );
        if delta.clear {
            state.chunks.clear();
        }
        state
            .chunks
            .retain(|chunk_id, _chunk| !delta.removed.contains(chunk_id));
        for chunk in &delta.upserts {
            let _previous = state.chunks.insert(chunk.chunk_id.clone(), chunk.clone());
        }
    }

    pub fn apply_lexical_authority_op(&mut self, op: &LexicalChannelOp) -> Result<(), CoreError> {
        match op {
            LexicalChannelOp::UpsertChunk(payload) => {
                let _previous = self
                    .structural_state_mut(
                        &payload.repo_id,
                        &payload.revision_id,
                        payload.generation,
                    )
                    .chunks
                    .insert(
                        payload.chunk_id.clone(),
                        decode_record(&payload.payload, "chunk")?,
                    );
            }
            LexicalChannelOp::DeleteChunk(payload) => {
                let _removed = self
                    .structural_state_mut(
                        &payload.repo_id,
                        &payload.revision_id,
                        payload.generation,
                    )
                    .chunks
                    .remove(&payload.chunk_id);
            }
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_lexical_replace_scope(&payload.payload)?;
                let state = self.structural_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.chunks.retain(|_chunk_id, chunk| {
                    chunk.repo_relative_path != scope.scope.repo_relative_path
                });
                for chunk in scope.chunks {
                    let _previous = state.chunks.insert(chunk.chunk_id.clone(), chunk);
                }
            }
            LexicalChannelOp::TombstoneLexicalScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_lexical_tombstone_scope(&payload.payload)?;
                self.structural_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                )
                .chunks
                .retain(|_chunk_id, chunk| {
                    chunk.repo_relative_path != scope.scope.repo_relative_path
                });
            }
            LexicalChannelOp::ClearLexicalSurface(payload) => {
                if payload.surface == SearchScopeSurface::Chunk {
                    self.structural_state_mut(
                        &payload.repo_id,
                        &payload.revision_id,
                        payload.generation,
                    )
                    .chunks
                    .clear();
                }
            }
            LexicalChannelOp::UpsertCommit(payload) => {
                let record: CommitRecord = decode_record(&payload.payload, "commit")?;
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.note_commits_materialized();
                for parent in &record.parents {
                    if !state.commits.contains_key(parent) {
                        return Err(CoreError::Typed {
                            code: "HISTORY_COMMIT_PARENT_UNKNOWN".to_string(),
                            message: format!(
                                "history ingest: parent {} missing before child {}",
                                parent, record.sha
                            ),
                        });
                    }
                }
                let _previous = state.commits.insert(record.sha, record);
            }
            LexicalChannelOp::UpsertRef(payload) => {
                let sha = CommitSha::from_bytes(payload.sha);
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.note_refs_materialized();
                if !state.commits.contains_key(&sha) {
                    return Err(CoreError::Typed {
                        code: "HISTORY_REF_NOT_FOUND".to_string(),
                        message: format!(
                            "history ingest: ref `{}` points to unknown commit {}",
                            payload.name, sha
                        ),
                    });
                }
                let _previous = state.refs.insert(payload.name.clone(), sha);
            }
            LexicalChannelOp::DeleteRef(payload) => {
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.note_refs_materialized();
                let _removed = state.refs.remove(payload.name.as_ref());
            }
            LexicalChannelOp::UpsertTag(payload) => {
                let sha = CommitSha::from_bytes(payload.sha);
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.note_tags_materialized();
                if !state.commits.contains_key(&sha) {
                    return Err(CoreError::Typed {
                        code: "HISTORY_REF_NOT_FOUND".to_string(),
                        message: format!(
                            "history ingest: tag `{}` points to unknown commit {}",
                            payload.name, sha
                        ),
                    });
                }
                let _previous = state.tags.insert(payload.name.clone(), sha);
            }
            LexicalChannelOp::DeleteTag(payload) => {
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.note_tags_materialized();
                let _removed = state.tags.remove(payload.name.as_ref());
            }
            LexicalChannelOp::UpsertDiffHunk(payload) => {
                let record: DiffHunkRecord = decode_record(&payload.payload, "diff_hunk")?;
                let commit_sha = CommitSha::from_bytes(payload.commit_sha);
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.note_diff_hunks_materialized();
                if !state.commits.contains_key(&commit_sha) {
                    return Err(CoreError::Typed {
                        code: "HISTORY_REF_NOT_FOUND".to_string(),
                        message: format!(
                            "history ingest: diff hunk for unknown commit {commit_sha}"
                        ),
                    });
                }
                let _previous = state.diff_hunks.insert(
                    HistoryDiffKey {
                        commit_sha,
                        file_path: payload.file_path.clone(),
                    },
                    record,
                );
            }
            LexicalChannelOp::UpsertDirty(payload) => {
                let _previous = self
                    .runtime_state_mut(&payload.repo_id, &payload.revision_id, payload.generation)
                    .dirty_docs
                    .insert(
                        payload.doc_id.clone(),
                        DirtyDocState {
                            applied_at_ms: payload.applied_at_ms,
                            payload_hash: payload.payload_hash,
                        },
                    );
            }
            LexicalChannelOp::EvictDirty(payload) => {
                let _removed = self
                    .runtime_state_mut(&payload.repo_id, &payload.revision_id, payload.generation)
                    .dirty_docs
                    .remove(&payload.doc_id);
            }
            LexicalChannelOp::UpsertParseTree(payload) => {
                let record: ParseTreeRecord = decode_record(&payload.payload, "parse_tree")?;
                let state = self.structural_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                verify_parse_tree_against_chunk(state, &payload.chunk_id, &record, None)?;
                let _previous = state.parse_trees.insert(payload.chunk_id.clone(), record);
            }
            LexicalChannelOp::DeleteParseTree(payload) => {
                let _removed = self
                    .structural_state_mut(
                        &payload.repo_id,
                        &payload.revision_id,
                        payload.generation,
                    )
                    .parse_trees
                    .remove(&payload.chunk_id);
            }
            LexicalChannelOp::ReplaceStructuralScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_structural_replace_scope(&payload.payload)?;
                let state = self.structural_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                let allowed_chunk_ids: std::collections::BTreeSet<ChunkId> = state
                    .chunks
                    .iter()
                    .filter(|(_chunk_id, chunk)| {
                        chunk.repo_relative_path == scope.scope.repo_relative_path
                    })
                    .map(|(chunk_id, _chunk)| chunk_id.clone())
                    .collect();
                for tree in &scope.trees {
                    verify_parse_tree_against_chunk(
                        state,
                        &tree.chunk_id,
                        &tree.record,
                        Some(scope.scope.repo_relative_path.as_str()),
                    )?;
                }
                state
                    .parse_trees
                    .retain(|chunk_id, _tree| !allowed_chunk_ids.contains(chunk_id));
                for tree in scope.trees {
                    let _previous = state.parse_trees.insert(tree.chunk_id.clone(), tree.record);
                }
            }
            LexicalChannelOp::TombstoneStructuralScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_structural_tombstone_scope(&payload.payload)?;
                let state = self.structural_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                let allowed_chunk_ids: std::collections::BTreeSet<ChunkId> = state
                    .chunks
                    .iter()
                    .filter(|(_chunk_id, chunk)| {
                        chunk.repo_relative_path == scope.scope.repo_relative_path
                    })
                    .map(|(chunk_id, _chunk)| chunk_id.clone())
                    .collect();
                state
                    .parse_trees
                    .retain(|chunk_id, _tree| !allowed_chunk_ids.contains(chunk_id));
            }
            LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::DeleteSymbol(_)
            | LexicalChannelOp::Seal(_) => {}
        }
        Ok(())
    }
}

fn decode_record<T>(payload: &[u8], label: &str) -> Result<T, CoreError>
where
    T: for<'de> serde::Deserialize<'de>,
{
    decode_cbor_payload(payload).map_err(|err| {
        CoreError::InvalidContract(format!(
            "search-plane authority ledger: decode {label}: {err}"
        ))
    })
}

fn decode_lexical_replace_scope(
    payload: &[u8],
) -> Result<
    (
        quanta_index_contract::BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::SearchCorpusReplaceScope,
    ),
    CoreError,
> {
    decode_record(payload, "lexical_replace_scope")
}

fn decode_lexical_tombstone_scope(
    payload: &[u8],
) -> Result<
    (
        quanta_index_contract::BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::SearchCorpusTombstoneScope,
    ),
    CoreError,
> {
    decode_record(payload, "lexical_tombstone_scope")
}

fn decode_structural_replace_scope(
    payload: &[u8],
) -> Result<
    (
        quanta_index_contract::BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::StructuralReplaceScope,
    ),
    CoreError,
> {
    decode_record(payload, "structural_replace_scope")
}

fn decode_structural_tombstone_scope(
    payload: &[u8],
) -> Result<
    (
        quanta_index_contract::BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::StructuralTombstoneScope,
    ),
    CoreError,
> {
    decode_record(payload, "structural_tombstone_scope")
}
