//! How ingest batches, catalog deltas, and lexical channel ops land on the
//! ledger's auxiliary authority states.
//!
//! Every application produces the next epoch of the generation and
//! domain it touches (QI-BB-020 W2): a delta carries the epoch its rows
//! were made durable under and is applied through
//! [`Ledger::aux_advance`], which refuses any drift from the sequence;
//! the in-memory batch paths and the channel ops (tests and fixtures)
//! take the next epoch themselves.
//!
//! This module only sequences epochs and decodes op payloads. What a
//! delta or an op does to a state is the state type's own method
//! (`history_state`, `runtime_state`, `structural_state`), so the record
//! maps have exactly one writer each.

use std::time::Instant;

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{CommitRecord, CommitSha, ParseTreeRecord};
use quanta_index_contract::{
    DirtyIngestBatch, HistoryIngestBatch, ManifestGeneration, RepoId, RevisionId,
    RuntimeCatalogIngestBatch, SearchCorpusIngestBatch, SearchPlaneTrackKind, SearchScopeSurface,
    StructuralIngestBatch,
};
use quanta_index_core::CoreError;
use quanta_index_ipc::decode_cbor_payload;

use crate::auxiliary_authority;
use crate::readiness::history_state::HistoryAuthorityState;
use crate::readiness::ledger::{AuxDomainState, Ledger};
use crate::readiness::runtime_state::RuntimeMetadataState;
use crate::readiness::structural_state::StructuralAuthorityState;

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
                state.apply_delta(delta);
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
                state.apply_dirty_delta(delta);
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
                state.apply_catalog_delta(delta);
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
                state.apply_trees_delta(delta);
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
                state.apply_chunks_delta(delta);
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
                        state.restore_chunk(payload.chunk_id.clone(), chunk);
                        Ok(())
                    },
                )
            }
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_lexical_replace_scope(&payload.payload)?;
                self.advance_next::<StructuralAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        state.replace_scope_chunks(
                            scope.scope.repo_relative_path.as_str(),
                            scope.chunks,
                        );
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
                        state.tombstone_scope_chunks(scope.scope.repo_relative_path.as_str());
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
                        state.clear_chunks();
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
                    |state| state.upsert_commit(record),
                )
            }
            LexicalChannelOp::UpsertRef(payload) => {
                let sha = CommitSha::from_bytes(payload.sha);
                self.advance_next::<HistoryAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| state.upsert_ref(&payload.name, sha),
                )
            }
            LexicalChannelOp::UpsertParseTree(payload) => {
                let record: ParseTreeRecord = decode_record(&payload.payload, "parse_tree")?;
                self.advance_next::<StructuralAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| state.upsert_parse_tree(payload.chunk_id.clone(), record),
                )
            }
            LexicalChannelOp::ReplaceStructuralScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_structural_replace_scope(&payload.payload)?;
                self.advance_next::<StructuralAuthorityState>(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                    now,
                    |state| {
                        state.replace_scope_parse_trees(
                            scope.scope.repo_relative_path.as_str(),
                            scope
                                .trees
                                .into_iter()
                                .map(|tree| (tree.chunk_id, tree.record))
                                .collect(),
                        )
                    },
                )
            }
            LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::Seal(_) => Ok(()),
        }
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
