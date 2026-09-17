//! How ingest batches, catalog deltas, and lexical channel ops land on the
//! ledger's auxiliary authority states.
//!
//! Every application produces the next epoch of the generation and
//! domain it touches (QI-BB-020 W2): a delta carries the epoch its rows
//! were made durable under and is applied through
//! [`Ledger::aux_advance`], which refuses any drift from the sequence;
//! the in-memory batch paths and the channel ops (tests and fixtures)
//! take the next epoch themselves.

use std::collections::BTreeSet;
use std::time::Instant;

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{CommitRecord, CommitSha, DiffHunkRecord, ParseTreeRecord};
use quanta_index_contract::{
    ChunkId, DirtyIngestBatch, HistoryIngestBatch, ManifestGeneration, RepoId, RevisionId,
    RuntimeCatalogIngestBatch, SearchCorpusIngestBatch, SearchPlaneTrackKind, SearchScopeSurface,
    StructuralIngestBatch,
};
use quanta_index_core::CoreError;
use quanta_index_ipc::decode_cbor_payload;

use crate::auxiliary_authority;
use crate::readiness::history_state::{HistoryAuthorityState, HistoryDiffKey};
use crate::readiness::ledger::{AuxDomainState, Ledger};
use crate::readiness::runtime_state::{DirtyDocState, RuntimeMetadataState};
use crate::readiness::structural_state::{
    StructuralAuthorityState, verify_parse_tree_against_chunk,
};

impl Ledger {
    /// Apply a search-corpus batch's chunk universe in memory, without
    /// the durable step (tests and in-memory fixtures; production goes
    /// through the transition, the catalog and then the delta).
    pub fn apply_search_corpus_batch(
        &mut self,
        batch: &SearchCorpusIngestBatch,
        now: Instant,
    ) -> Result<(), CoreError> {
        let epoch =
            self.structural_next_epoch(&batch.repo_id, &batch.revision_id, batch.generation)?;
        let delta = auxiliary_authority::structural_chunks_transition(
            self.structural_state(&batch.repo_id, &batch.revision_id, batch.generation),
            epoch,
            batch,
        );
        self.apply_structural_chunks_delta(&delta, now)
    }

    /// Validate and apply a history batch in memory, without the durable
    /// step.
    pub fn apply_history_batch(
        &mut self,
        batch: &HistoryIngestBatch,
        now: Instant,
    ) -> Result<(), CoreError> {
        let epoch =
            self.history_next_epoch(&batch.repo_id, &batch.revision_id, batch.generation)?;
        let delta = auxiliary_authority::history_transition(
            self.history_state(&batch.repo_id, &batch.revision_id, batch.generation),
            epoch,
            batch,
        )?;
        self.apply_history_delta(&delta, now)
    }

    /// Apply a dirty-overlay batch in memory, without the durable step.
    pub fn apply_runtime_batch(
        &mut self,
        batch: &DirtyIngestBatch,
        now: Instant,
    ) -> Result<(), CoreError> {
        let epoch =
            self.runtime_next_epoch(&batch.repo_id, &batch.revision_id, batch.generation)?;
        let delta = auxiliary_authority::runtime_dirty_transition(
            self.runtime_state(&batch.repo_id, &batch.revision_id, batch.generation),
            epoch,
            batch,
        );
        self.apply_runtime_dirty_delta(&delta, now)
    }

    /// Validate and apply a runtime catalog batch in memory, without the
    /// durable step.
    pub fn apply_runtime_catalog_batch(
        &mut self,
        batch: &RuntimeCatalogIngestBatch,
        now: Instant,
    ) -> Result<(), CoreError> {
        let epoch =
            self.runtime_next_epoch(&batch.repo_id, &batch.revision_id, batch.generation)?;
        let delta = auxiliary_authority::runtime_catalog_transition(
            self.structural_state(&batch.repo_id, &batch.revision_id, batch.generation),
            self.runtime_state(&batch.repo_id, &batch.revision_id, batch.generation),
            epoch,
            batch,
        )?;
        self.apply_runtime_catalog_delta(&delta, now)
    }

    /// Validate and apply a structural batch's parse trees in memory,
    /// without the durable step and without the track bookkeeping.
    pub fn apply_structural_batch(
        &mut self,
        batch: &StructuralIngestBatch,
        now: Instant,
    ) -> Result<(), CoreError> {
        let epoch =
            self.structural_next_epoch(&batch.repo_id, &batch.revision_id, batch.generation)?;
        let delta = auxiliary_authority::structural_transition(
            self.structural_state(&batch.repo_id, &batch.revision_id, batch.generation),
            self.track_state(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Structural,
            ),
            epoch,
            batch,
        )?;
        self.apply_structural_trees_delta(&delta, now)
    }

    /// Apply a validated history delta at the epoch its durable rows were
    /// stamped with.
    pub(crate) fn apply_history_delta(
        &mut self,
        delta: &auxiliary_authority::HistoryDelta,
        now: Instant,
    ) -> Result<(), CoreError> {
        self.aux_advance::<HistoryAuthorityState>(
            &delta.generation.repo_id,
            &delta.generation.revision_id,
            delta.generation.generation,
            delta.epoch,
            now,
            |state| {
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
                Ok(())
            },
        )
    }

    /// Apply a dirty-overlay delta at the epoch its durable rows were
    /// stamped with.
    pub(crate) fn apply_runtime_dirty_delta(
        &mut self,
        delta: &auxiliary_authority::RuntimeDirtyDelta,
        now: Instant,
    ) -> Result<(), CoreError> {
        self.aux_advance::<RuntimeMetadataState>(
            &delta.generation.repo_id,
            &delta.generation.revision_id,
            delta.generation.generation,
            delta.epoch,
            now,
            |state| {
                for (chunk_id, doc) in &delta.upserts {
                    let _previous = state.dirty_docs.insert(chunk_id.clone(), doc.clone());
                }
                for chunk_id in &delta.deletes {
                    let _removed = state.dirty_docs.remove(chunk_id);
                }
                Ok(())
            },
        )
    }

    /// Apply a validated runtime catalog delta at the epoch its durable
    /// rows were stamped with: the generation's catalog is replaced whole.
    pub(crate) fn apply_runtime_catalog_delta(
        &mut self,
        delta: &auxiliary_authority::RuntimeCatalogDelta,
        now: Instant,
    ) -> Result<(), CoreError> {
        self.aux_advance::<RuntimeMetadataState>(
            &delta.generation.repo_id,
            &delta.generation.revision_id,
            delta.generation.generation,
            delta.epoch,
            now,
            |state| {
                state.restore_meta(delta.meta.clone());
                state.changed_docs = delta.changed_docs.clone();
                state.doc_facets = delta.doc_facets.clone();
                state.snapshots = delta.snapshots.clone();
                state.affected_docs = delta.affected_docs.clone();
                state.invalidated_by_docs = delta.invalidated_by_docs.clone();
                Ok(())
            },
        )
    }

    /// Apply a validated structural delta at the epoch its durable rows
    /// were stamped with: parse trees, the seal request and the
    /// structural track's state.
    pub(crate) fn apply_structural_trees_delta(
        &mut self,
        delta: &auxiliary_authority::StructuralTreesDelta,
        now: Instant,
    ) -> Result<(), CoreError> {
        self.aux_advance::<StructuralAuthorityState>(
            &delta.generation.repo_id,
            &delta.generation.revision_id,
            delta.generation.generation,
            delta.epoch,
            now,
            |state| {
                for chunk_id in &delta.removed {
                    let _removed = state.parse_trees.remove(chunk_id);
                }
                for (chunk_id, record) in &delta.upserts {
                    let _previous = state.parse_trees.insert(chunk_id.clone(), record.clone());
                }
                state.seal_requested = delta.seal_requested;
                Ok(())
            },
        )?;
        self.restore_track_state(
            &delta.generation.repo_id,
            &delta.generation.revision_id,
            SearchPlaneTrackKind::Structural,
            delta.track.clone(),
        );
        Ok(())
    }

    /// Apply a chunk-universe delta at the epoch its durable rows were
    /// stamped with.
    pub(crate) fn apply_structural_chunks_delta(
        &mut self,
        delta: &auxiliary_authority::StructuralChunksDelta,
        now: Instant,
    ) -> Result<(), CoreError> {
        self.aux_advance::<StructuralAuthorityState>(
            &delta.generation.repo_id,
            &delta.generation.revision_id,
            delta.generation.generation,
            delta.epoch,
            now,
            |state| {
                if delta.clear {
                    state.chunks.clear();
                }
                for chunk_id in &delta.removed {
                    let _removed = state.chunks.remove(chunk_id);
                }
                for chunk in &delta.upserts {
                    let _previous = state.chunks.insert(chunk.chunk_id.clone(), chunk.clone());
                }
                Ok(())
            },
        )
    }

    /// One in-memory mutation of one authority generation at its next
    /// epoch (the channel-op path; tests and fixtures).
    fn advance_next<S: AuxDomainState>(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        now: Instant,
        mutate: impl FnOnce(&mut S) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let epoch = self.aux_next_epoch::<S>(repo_id, revision_id, generation)?;
        self.aux_advance::<S>(repo_id, revision_id, generation, epoch, now, mutate)
    }

    /// Apply one lexical channel op to the auxiliary authorities in memory
    /// (tests and fixtures). Each op is one mutation and so one epoch; an
    /// op the authority refuses leaves it unchanged.
    pub fn apply_lexical_authority_op(
        &mut self,
        op: &LexicalChannelOp,
        now: Instant,
    ) -> Result<(), CoreError> {
        match op {
            LexicalChannelOp::UpsertChunk(payload) => {
                let chunk = decode_record(&payload.payload, "chunk")?;
                self.advance_next::<StructuralAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        let _previous = state.chunks.insert(payload.chunk_id.clone(), chunk);
                        Ok(())
                    },
                )
            }
            LexicalChannelOp::DeleteChunk(payload) => self
                .advance_next::<StructuralAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        let _removed = state.chunks.remove(&payload.chunk_id);
                        Ok(())
                    },
                ),
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_lexical_replace_scope(&payload.payload)?;
                self.advance_next::<StructuralAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        remove_chunks_at_path(state, scope.scope.repo_relative_path.as_str());
                        for chunk in scope.chunks {
                            let _previous = state.chunks.insert(chunk.chunk_id.clone(), chunk);
                        }
                        Ok(())
                    },
                )
            }
            LexicalChannelOp::TombstoneLexicalScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_lexical_tombstone_scope(&payload.payload)?;
                self.advance_next::<StructuralAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        remove_chunks_at_path(state, scope.scope.repo_relative_path.as_str());
                        Ok(())
                    },
                )
            }
            LexicalChannelOp::ClearLexicalSurface(payload) => {
                if payload.surface != SearchScopeSurface::Chunk {
                    return Ok(());
                }
                self.advance_next::<StructuralAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        state.chunks.clear();
                        Ok(())
                    },
                )
            }
            LexicalChannelOp::UpsertCommit(payload) => {
                let record: CommitRecord = decode_record(&payload.payload, "commit")?;
                self.advance_next::<HistoryAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
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
                        Ok(())
                    },
                )
            }
            LexicalChannelOp::UpsertRef(payload) => {
                let sha = CommitSha::from_bytes(payload.sha);
                self.advance_next::<HistoryAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        state.note_refs_materialized();
                        if !state.commits.contains_key(&sha) {
                            return Err(history_ref_not_found(format!(
                                "history ingest: ref `{}` points to unknown commit {}",
                                payload.name, sha
                            )));
                        }
                        let _previous = state.refs.insert(payload.name.clone(), sha);
                        Ok(())
                    },
                )
            }
            LexicalChannelOp::DeleteRef(payload) => self.advance_next::<HistoryAuthorityState>(
                &payload.repo_id,
                &payload.revision_id,
                payload.generation,
                now,
                |state| {
                    state.note_refs_materialized();
                    let _removed = state.refs.remove(payload.name.as_ref());
                    Ok(())
                },
            ),
            LexicalChannelOp::UpsertTag(payload) => {
                let sha = CommitSha::from_bytes(payload.sha);
                self.advance_next::<HistoryAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        state.note_tags_materialized();
                        if !state.commits.contains_key(&sha) {
                            return Err(history_ref_not_found(format!(
                                "history ingest: tag `{}` points to unknown commit {}",
                                payload.name, sha
                            )));
                        }
                        let _previous = state.tags.insert(payload.name.clone(), sha);
                        Ok(())
                    },
                )
            }
            LexicalChannelOp::DeleteTag(payload) => self.advance_next::<HistoryAuthorityState>(
                &payload.repo_id,
                &payload.revision_id,
                payload.generation,
                now,
                |state| {
                    state.note_tags_materialized();
                    let _removed = state.tags.remove(payload.name.as_ref());
                    Ok(())
                },
            ),
            LexicalChannelOp::UpsertDiffHunk(payload) => {
                let record: DiffHunkRecord = decode_record(&payload.payload, "diff_hunk")?;
                let commit_sha = CommitSha::from_bytes(payload.commit_sha);
                self.advance_next::<HistoryAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        state.note_diff_hunks_materialized();
                        if !state.commits.contains_key(&commit_sha) {
                            return Err(history_ref_not_found(format!(
                                "history ingest: diff hunk for unknown commit {commit_sha}"
                            )));
                        }
                        let _previous = state.diff_hunks.insert(
                            HistoryDiffKey {
                                commit_sha,
                                file_path: payload.file_path.clone(),
                            },
                            record,
                        );
                        Ok(())
                    },
                )
            }
            LexicalChannelOp::UpsertDirty(payload) => self.advance_next::<RuntimeMetadataState>(
                &payload.repo_id,
                &payload.revision_id,
                payload.generation,
                now,
                |state| {
                    let _previous = state.dirty_docs.insert(
                        payload.doc_id.clone(),
                        DirtyDocState {
                            applied_at_ms: payload.applied_at_ms,
                            payload_hash: payload.payload_hash,
                        },
                    );
                    Ok(())
                },
            ),
            LexicalChannelOp::EvictDirty(payload) => self.advance_next::<RuntimeMetadataState>(
                &payload.repo_id,
                &payload.revision_id,
                payload.generation,
                now,
                |state| {
                    let _removed = state.dirty_docs.remove(&payload.doc_id);
                    Ok(())
                },
            ),
            LexicalChannelOp::UpsertParseTree(payload) => {
                let record: ParseTreeRecord = decode_record(&payload.payload, "parse_tree")?;
                self.advance_next::<StructuralAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        verify_parse_tree_against_chunk(state, &payload.chunk_id, &record, None)?;
                        let _previous = state.parse_trees.insert(payload.chunk_id.clone(), record);
                        Ok(())
                    },
                )
            }
            LexicalChannelOp::DeleteParseTree(payload) => self
                .advance_next::<StructuralAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        let _removed = state.parse_trees.remove(&payload.chunk_id);
                        Ok(())
                    },
                ),
            LexicalChannelOp::ReplaceStructuralScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_structural_replace_scope(&payload.payload)?;
                self.advance_next::<StructuralAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        let path = scope.scope.repo_relative_path.as_str();
                        for tree in &scope.trees {
                            verify_parse_tree_against_chunk(
                                state,
                                &tree.chunk_id,
                                &tree.record,
                                Some(path),
                            )?;
                        }
                        remove_parse_trees_at_path(state, path);
                        for tree in scope.trees {
                            let _previous =
                                state.parse_trees.insert(tree.chunk_id.clone(), tree.record);
                        }
                        Ok(())
                    },
                )
            }
            LexicalChannelOp::TombstoneStructuralScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_structural_tombstone_scope(&payload.payload)?;
                self.advance_next::<StructuralAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        remove_parse_trees_at_path(state, scope.scope.repo_relative_path.as_str());
                        Ok(())
                    },
                )
            }
            LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::DeleteSymbol(_)
            | LexicalChannelOp::Seal(_) => Ok(()),
        }
    }
}

fn history_ref_not_found(message: String) -> CoreError {
    CoreError::Typed {
        code: "HISTORY_REF_NOT_FOUND".to_string(),
        message,
    }
}

/// The chunk ids of one path in the chunk universe.
fn chunk_ids_at_path(state: &StructuralAuthorityState, path: &str) -> BTreeSet<ChunkId> {
    state
        .chunks
        .iter()
        .filter(|(_chunk_id, chunk)| chunk.repo_relative_path.as_str() == path)
        .map(|(chunk_id, _chunk)| chunk_id.clone())
        .collect()
}

fn remove_chunks_at_path(state: &mut StructuralAuthorityState, path: &str) {
    for chunk_id in chunk_ids_at_path(state, path) {
        let _removed = state.chunks.remove(&chunk_id);
    }
}

fn remove_parse_trees_at_path(state: &mut StructuralAuthorityState, path: &str) {
    for chunk_id in chunk_ids_at_path(state, path) {
        let _removed = state.parse_trees.remove(&chunk_id);
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
