//! The ingest ports: search-corpus batches, repo-metadata overlays, the writer sweep and the metrics scrape.


use crate::adapter::legacy_ops_for_batch;
use crate::generation_dir::{ensure_unsealed, is_writer_lock_entry, read_lexical_delta_base};
use crate::index_store::{lexical_sealed_identity_path, persist_lexical_sealed_identity};
use crate::overlay_codec::OverlayFamily;
use crate::overlay_codec::{
    encode_file_contributor_batch, encode_file_ownership_batch, encode_repo_commit_recency_batch,
    encode_repo_description_batch, encode_repo_meta_batch, encode_repo_topic_batch,
};
use crate::sealed_generation::seal_generation;
use crate::{
    GENERATION_IMMUTABLE_CODE, GenKey, LexicalAdapter, op_mutates_index, op_writes_generation,
};
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::{
    FileContributorIngestBatch, FileOwnershipIngestBatch, GenerationSnapshot, ManifestGeneration,
    RepoCommitRecencyIngestBatch, RepoDescriptionIngestBatch, RepoId, RepoMetaIngestBatch,
    RepoTopicIngestBatch, RevisionId, SearchCorpusIngestBatch, SearchPlaneTrackKind,
};
use quanta_index_core::domains::generation::unique_inode_tree_bytes;
use quanta_index_core::{
    CoreError, FileContributorIngestPort, FileOwnershipIngestPort, GenerationIdentityValidatePort,
    LexicalIndexBuildPort, MetricPointV1, MetricSourcePort, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMetaIngestPort, RepoTopicIngestPort, SearchCorpusBatchBuildPort,
    TrackDiskUsagePort, WriterIdleSweepPort, count_from_usize,
};

/// The writer envelope and the regex match cache as scrape points,
/// `lexical_…` (QI-BB-015).
impl WriterIdleSweepPort for LexicalAdapter {
    fn sweep_idle_writers(&self) -> Result<u64, CoreError> {
        self.release_idle_writers()
    }
}

/// Every generation directory under the lexical track root, measured by
/// the same walker a reclaim and an open use (QI-BB-015).
impl TrackDiskUsagePort for LexicalAdapter {
    /// Bytes the lexical state root occupies on disk, by unique inode (a
    /// delta's hard-linked base segments count once), measured while
    /// ingest, seals and reclaims keep running.
    fn track_disk_bytes(&self) -> Result<u64, CoreError> {
        if !self.state_root.exists() {
            return Ok(0);
        }
        unique_inode_tree_bytes(
            std::slice::from_ref(&self.state_root),
            &is_writer_lock_entry,
        )
        .map_err(|err| {
            CoreError::Storage(format!(
                "lexical: measure state root {}: {err}",
                self.state_root.display()
            ))
        })
    }
}

impl MetricSourcePort for LexicalAdapter {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let writers = self.writer_cache_stats()?;
        let regex = self.regex_match_cache_stats()?;
        let text_authority = self.text_authority_update_stats()?;
        let seals = self.seal_commitment_stats()?;
        Ok(vec![
            MetricPointV1::counter("lexical_seals_total", seals.seals),
            MetricPointV1::counter("lexical_seal_files_hashed_total", seals.files_hashed),
            MetricPointV1::counter("lexical_seal_bytes_hashed_total", seals.bytes_hashed),
            MetricPointV1::counter("lexical_seal_files_inherited_total", seals.files_inherited),
            MetricPointV1::counter("lexical_seal_bytes_inherited_total", seals.bytes_inherited),
            MetricPointV1::counter(
                "lexical_text_authority_rebuilds_total",
                text_authority.rebuilds,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_incremental_updates_total",
                text_authority.incremental_updates,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_docs_derived_total",
                text_authority.docs_derived,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_docs_retired_total",
                text_authority.docs_retired,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_shards_written_total",
                text_authority.shards_written,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_shards_inherited_total",
                text_authority.shards_inherited,
            ),
            MetricPointV1::gauge_count(
                "lexical_writers_open",
                count_from_usize(writers.open_writers),
            ),
            MetricPointV1::gauge_count(
                "lexical_writers_max",
                count_from_usize(writers.max_writers),
            ),
            MetricPointV1::gauge_count(
                "lexical_writers_allocated_heap_bytes",
                writers.allocated_heap_bytes,
            ),
            MetricPointV1::counter("lexical_writer_lru_releases_total", writers.lru_releases),
            MetricPointV1::counter("lexical_writer_idle_releases_total", writers.idle_releases),
            MetricPointV1::counter("lexical_writer_seal_releases_total", writers.seal_releases),
            MetricPointV1::counter("lexical_regex_cache_hits_total", regex.hits),
            MetricPointV1::counter("lexical_regex_cache_misses_total", regex.misses),
            MetricPointV1::gauge_count(
                "lexical_regex_cache_entries",
                count_from_usize(regex.entries),
            ),
            MetricPointV1::gauge_count("lexical_regex_cache_resident_bytes", regex.resident_bytes),
            MetricPointV1::counter("lexical_regex_cache_evictions_total", regex.evictions),
            MetricPointV1::counter(
                "lexical_regex_cache_refused_cardinality_total",
                regex.refused_cardinality,
            ),
            MetricPointV1::counter(
                "lexical_regex_cache_refused_bytes_total",
                regex.refused_bytes,
            ),
            MetricPointV1::counter("lexical_regex_match_sets_built_total", regex.sets_built),
            MetricPointV1::counter(
                "lexical_regex_match_set_members_built_total",
                regex.members_built,
            ),
            MetricPointV1::counter(
                "lexical_regex_match_set_bytes_built_total",
                regex.bytes_built,
            ),
        ])
    }
}

impl SearchCorpusBatchBuildPort for LexicalAdapter {
    fn build_batch(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        let candidate = GenerationSnapshot {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: batch.generation,
            manifest_digest: batch.manifest_digest.clone(),
        };
        let generation_dir = self.index_path(&GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        });
        if lexical_sealed_identity_path(&generation_dir).exists() {
            if !batch.seal {
                return Err(CoreError::Typed {
                    code: GENERATION_IMMUTABLE_CODE.to_string(),
                    message: format!(
                        "lexical: generation {} is already sealed; refusing non-seal mutation",
                        batch.generation.get()
                    ),
                });
            }
            self.validate_generation_identity(&candidate)?;
            return Ok(());
        }
        let ops = legacy_ops_for_batch(batch, batch.seal)?;
        self.build(&batch.repo_id, &batch.revision_id, batch.generation, &ops)?;
        if batch.seal {
            // Finalize and retire the writer before measuring: a cached
            // writer would commit again on eviction and rewrite `meta.json`
            // behind the manifest. Then manifest first, identity last: the
            // identity's presence is the promotion point and implies a
            // durable manifest.
            let key = GenKey {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation: batch.generation,
            };
            self.finalize_index_for_seal(&key)?;
            let base_dir = read_lexical_delta_base(&generation_dir)?.map(|base| {
                self.index_path(&GenKey {
                    repo_id: key.repo_id.clone(),
                    revision_id: key.revision_id.clone(),
                    generation: base,
                })
            });
            let measured = seal_generation(
                &generation_dir,
                &self.fields,
                &candidate,
                base_dir.as_deref(),
            )?;
            self.record_seal_measurement(measured)?;
            persist_lexical_sealed_identity(&generation_dir, &candidate)?;
        }
        // Every batch is a chance to give an abandoned generation's heap back
        // to the envelope (QI-BB-016).
        let _released = self.release_idle_writers()?;
        Ok(())
    }
}

impl RepoCommitRecencyIngestPort for LexicalAdapter {
    fn publish_batch(
        &self,
        batch: &RepoCommitRecencyIngestBatch,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let bytes = encode_repo_commit_recency_batch(batch)?;
        self.publish_overlay(
            &key,
            OverlayFamily::CommitRecency,
            &bytes,
            batch.batch_digest.clone(),
            batch.entries.len(),
        )
    }
}

impl RepoMetaIngestPort for LexicalAdapter {
    fn publish_batch(
        &self,
        batch: &RepoMetaIngestBatch,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let bytes = encode_repo_meta_batch(batch)?;
        self.publish_overlay(
            &key,
            OverlayFamily::Meta,
            &bytes,
            batch.batch_digest.clone(),
            batch.entries.len(),
        )
    }
}

impl RepoTopicIngestPort for LexicalAdapter {
    fn publish_batch(
        &self,
        batch: &RepoTopicIngestBatch,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let bytes = encode_repo_topic_batch(batch)?;
        self.publish_overlay(
            &key,
            OverlayFamily::Topic,
            &bytes,
            batch.batch_digest.clone(),
            batch.entries.len(),
        )
    }
}

impl RepoDescriptionIngestPort for LexicalAdapter {
    fn publish_batch(
        &self,
        batch: &RepoDescriptionIngestBatch,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let bytes = encode_repo_description_batch(batch)?;
        self.publish_overlay(
            &key,
            OverlayFamily::Description,
            &bytes,
            batch.batch_digest.clone(),
            batch.entries.len(),
        )
    }
}

impl FileOwnershipIngestPort for LexicalAdapter {
    fn publish_batch(
        &self,
        batch: &FileOwnershipIngestBatch,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let bytes = encode_file_ownership_batch(batch)?;
        self.publish_overlay(
            &key,
            OverlayFamily::FileOwnership,
            &bytes,
            batch.batch_digest.clone(),
            batch.entries.len(),
        )
    }
}

impl FileContributorIngestPort for LexicalAdapter {
    fn publish_batch(
        &self,
        batch: &FileContributorIngestBatch,
    ) -> Result<quanta_index_contract::BatchPublishReceipt, CoreError> {
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let bytes = encode_file_contributor_batch(batch)?;
        self.publish_overlay(
            &key,
            OverlayFamily::Contributor,
            &bytes,
            batch.batch_digest.clone(),
            batch.entries.len(),
        )
    }
}

impl LexicalIndexBuildPort for LexicalAdapter {
    fn build(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        ops: &[LexicalChannelOp],
    ) -> Result<(), CoreError> {
        if ops.is_empty() {
            return Ok(());
        }
        // All ops in a single `build` invocation must share the (repo, rev, gen)
        // triple. The dispatcher feeds us one op per call today; defending the
        // invariant here keeps the adapter safe if that changes.
        for op in ops {
            if op.repo_id() != repo || op.revision_id() != revision || op.generation() != generation
            {
                return Err(CoreError::InvalidContract(
                    "lexical: op (repo, revision, generation) mismatch with batch key".to_string(),
                ));
            }
        }
        let key = GenKey {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
        };
        let mutates_index = ops.iter().any(op_mutates_index);
        if ops.iter().any(op_writes_generation) {
            ensure_unsealed(&self.index_path(&key), generation, "an index or overlay op")?;
        }
        self.prepare_generation_for_ops(&key, ops)?;
        if !mutates_index {
            // Overlay-only ops never touch the index, so they must not open
            // a writer: a writer left in the cache would commit on eviction
            // and rewrite `meta.json` under a sealed manifest.
            for op in ops {
                let _committed = self.apply_snapshot_op(&key, op)?;
            }
            return Ok(());
        }
        let handle = self.writer_handle(&key)?;
        self.commit_ops_under_lock(&handle, &key, ops)
    }
}
