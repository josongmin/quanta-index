//! The ingest ports: search-corpus batches, repo-metadata overlays, the writer sweep and the metrics scrape.

use crate::adapter::{declared_delta_base_generation, legacy_ops_for_batch};
use crate::channel_payloads::{decode_replace_scope_payload, decode_tombstone_scope_payload};
use crate::generation_dir::{ensure_unsealed, is_writer_lock_entry, read_lexical_delta_base};
use crate::index_store::{lexical_sealed_identity_path, persist_lexical_sealed_identity};
use crate::overlay_codec::OverlayFamily;
use crate::overlay_codec::{
    decode_repo_metadata_payload, encode_file_contributor_batch, encode_file_ownership_batch,
    encode_repo_commit_recency_batch, encode_repo_description_batch, encode_repo_meta_batch,
    encode_repo_topic_batch,
};
use crate::sealed_generation::coverage::{
    CoveragePlan, CoverageWriteBase, SOURCE_FILE_COVERAGE_FILE_NAME, plan_file_coverage,
    read_staged_coverage, write_staged_coverage,
};
use crate::sealed_generation::{DiscardingVisitor, seal_generation, walk_sealed_generation};
use crate::{GenKey, LexicalAdapter, op_mutates_index, op_writes_generation};
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::{
    BatchIngestMode, FileContributorIngestBatch, FileOwnershipIngestBatch, GenerationSnapshot,
    ManifestGeneration, RepoCommitRecencyIngestBatch, RepoDescriptionIngestBatch, RepoId,
    RepoMetaIngestBatch, RepoTopicIngestBatch, RevisionId, SearchCorpusIngestBatch,
    SearchPlaneTrackKind, validate_lexical_file_mutations_v1,
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
    fn preflight_batch(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        batch
            .validate_v1()
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        batch
            .validate_surface_mutations_v1()
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        if let Some(payload) = &batch.bundle_payload {
            let _metadata = decode_repo_metadata_payload(payload)?;
        }
        let identity = GenerationSnapshot {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: batch.generation,
            manifest_digest: batch.manifest_digest.clone(),
        };
        let directory = self.index_path(&GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        });
        if lexical_sealed_identity_path(&directory).exists() {
            // A full replacement can reach the existing identity-fenced repair
            // planner for damaged content. Proving that content here would
            // refuse before the materializer can reclaim and rebuild it. This
            // is mutation admission only: the sealed identity must still match,
            // and neither build nor open accepts an unproved generation.
            let (_directory, _observed) =
                self.sealed_generation_dir_for(&identity, "batch preflight")?;
            let proved = match walk_sealed_generation(&directory, &identity, &mut DiscardingVisitor)
            {
                Ok(proved) => proved,
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    ..
                }) if batch.mode == BatchIngestMode::ReplaceGeneration => return Ok(()),
                Err(error) => return Err(error),
            };
            if proved.source_publication.as_ref() != Some(&batch.source_event) {
                return Err(CoreError::InvalidContract(
                    "lexical: sealed target belongs to another source event".into(),
                ));
            }
            return Ok(());
        }
        let _planned = self.plan_batch_coverage(batch, &identity, &directory)?;
        Ok(())
    }

    fn build_batch(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        // Storage-free admission precedes even the sealed replay shortcut.
        // The same owner validates the public materializer and raw channel.
        batch.validate_surface_mutations_v1().map_err(|error| {
            CoreError::InvalidContract(format!("lexical: file mutation admission: {error}"))
        })?;
        batch.validate_v1().map_err(|error| {
            CoreError::InvalidContract(format!("lexical: batch admission: {error}"))
        })?;
        if let Some(payload) = &batch.bundle_payload {
            let _metadata = decode_repo_metadata_payload(payload)?;
        }
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
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationImmutable,
                    message: format!(
                        "lexical: generation {} is already sealed; refusing non-seal mutation",
                        batch.generation.get()
                    ),
                });
            }
            self.validate_generation_identity(&candidate)?;
            let verified =
                walk_sealed_generation(&generation_dir, &candidate, &mut DiscardingVisitor)?;
            if verified.source_publication.as_ref() != Some(&batch.source_event) {
                return Err(CoreError::InvalidContract(
                    "lexical: sealed generation belongs to a different source event".into(),
                ));
            }
            return Ok(());
        }
        let ops = legacy_ops_for_batch(batch, batch.seal)?;
        let coverage = self.plan_batch_coverage(batch, &candidate, &generation_dir)?;
        // The prepared coverage marks this target as bound before any index
        // mutation. It is query-invisible until the index and artifact seal.
        let coverage_root =
            write_staged_coverage(&generation_dir, &candidate, &batch.source_event, &coverage)?;
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        self.prepare_generation_from_base(&key, batch.base_generation)?;
        self.build_ops(
            &batch.repo_id,
            &batch.revision_id,
            batch.generation,
            &ops,
            true,
        )?;
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
                &coverage_root,
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
        self.build_ops(repo, revision, generation, ops, false)
    }
}

impl LexicalAdapter {
    fn plan_batch_coverage(
        &self,
        batch: &SearchCorpusIngestBatch,
        candidate: &GenerationSnapshot,
        generation_dir: &std::path::Path,
    ) -> Result<CoveragePlan, CoreError> {
        // Derive the complete next admitted universe from a proved base before
        // any target writes. A missing base capability cannot become empty.
        batch
            .source_event
            .validate()
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        let (base_coverage, base) = match batch.mode {
            BatchIngestMode::ReplaceGeneration => {
                (quanta_index_contract::FileCoverageSnapshot::new(), None)
            }
            BatchIngestMode::Delta => {
                let base = batch.base_generation.ok_or_else(|| {
                    CoreError::InvalidContract("lexical: coverage delta requires a base".into())
                })?;
                let base_dir = self.index_path(&GenKey {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: base,
                });
                let identity = crate::index_store::read_lexical_sealed_identity(&base_dir)?;
                if identity.repo_id != batch.repo_id
                    || identity.revision_id != batch.revision_id
                    || identity.manifest_generation != base
                {
                    return Err(CoreError::Storage(
                        "lexical: coverage base identity mismatch".into(),
                    ));
                }
                let verified =
                    walk_sealed_generation(&base_dir, &identity, &mut DiscardingVisitor)?;
                // A current source high-water alone cannot authorize cloning an
                // older physical snapshot: unchanged files would be resurrected
                // while the new event claims to extend the current lineage.
                // Prove the actual inherited snapshot is the declared parent.
                if verified.source_publication.as_ref().is_none_or(|event| {
                    event.stream_id != batch.source_event.stream_id
                        || Some(&event.event_id)
                            != batch.source_event.expected_base_event_id.as_ref()
                }) {
                    return Err(CoreError::Typed {
                        code: quanta_index_contract::SearchPlaneErrorCodeV2::DeltaBaseConflict,
                        message: "lexical: delta base source event differs from the declared stream parent; publish a full replacement to start another lineage".into(),
                    });
                }
                self.validate_inherited_candidate_ownership(&verified.index, batch)?;
                let coverage = verified.coverage.ok_or_else(|| CoreError::Typed { code: quanta_index_contract::SearchPlaneErrorCodeV2::SymbolCoverageUnavailable, message: "lexical: coverage delta requires a base with admitted file coverage; rebuild the generation".into() })?;
                let root = verified.manifest.source_coverage.ok_or_else(|| {
                    CoreError::Storage("lexical: verified coverage has no root commitment".into())
                })?;
                (
                    coverage,
                    Some(CoverageWriteBase {
                        directory: base_dir,
                        root,
                    }),
                )
            }
        };
        let replacements: Vec<_> = batch
            .replace_scopes
            .iter()
            .map(|scope| scope.coverage.clone())
            .collect();
        let tombstones: Vec<_> = batch
            .tombstone_scopes
            .iter()
            .map(|scope| scope.file.clone())
            .collect();
        let coverage = plan_file_coverage(
            &base_coverage,
            base,
            &replacements,
            &tombstones,
            &batch.clear_surfaces,
        )?;
        match read_staged_coverage(generation_dir, candidate)? {
            Some(staged)
                if staged.publication != batch.source_event
                    || staged.coverage != *coverage.snapshot() =>
            {
                return Err(CoreError::InvalidContract(
                    "lexical: target already belongs to a different source publication".into(),
                ));
            }
            None if crate::generation_dir::lexical_index_content_exists(generation_dir) => {
                return Err(CoreError::InvalidContract(
                    "lexical: cannot bind coverage over pre-existing unbound index content".into(),
                ));
            }
            Some(_) | None => {}
        }
        Ok(coverage)
    }

    fn validate_inherited_candidate_ownership(
        &self,
        index: &tantivy::Index,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<(), CoreError> {
        use tantivy::query::{BooleanQuery, Occur, Query, TermSetQuery};
        let terms: Vec<_> = batch
            .replace_scopes
            .iter()
            .flat_map(|scope| {
                scope
                    .chunks
                    .iter()
                    .map(|chunk| chunk.chunk_id.as_str())
                    .chain(scope.symbols.iter().map(|symbol| symbol.symbol_id.as_str()))
            })
            .map(|id| tantivy::Term::from_field_text(self.fields.candidate_id, id))
            .collect();
        if terms.is_empty() {
            return Ok(());
        }
        let retired: Vec<Box<dyn Query>> = batch
            .replace_scopes
            .iter()
            .map(|scope| &scope.coverage.source.file)
            .chain(batch.tombstone_scopes.iter().map(|scope| &scope.file))
            .map(|file| -> Box<dyn Query> {
                Box::new(crate::text_docs::source_file_query(&self.fields, file))
            })
            .collect();
        let conflict = BooleanQuery::new(vec![
            (Occur::Must, Box::new(TermSetQuery::new(terms))),
            (Occur::MustNot, Box::new(BooleanQuery::union(retired))),
        ]);
        let reader: tantivy::IndexReader = index
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::Manual)
            .try_into()
            .map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: inherited candidate ownership reader: {error}"
                ))
            })?;
        let count = reader
            .searcher()
            .search(&conflict, &tantivy::collector::Count)
            .map_err(|error| {
                CoreError::Storage(format!(
                    "lexical: inherited candidate ownership scan: {error}"
                ))
            })?;
        if count != 0 {
            return Err(CoreError::InvalidContract("lexical: replacement candidate IDs collide with inherited units outside the deleted source-file set".into()));
        }
        Ok(())
    }
}

impl LexicalAdapter {
    fn build_ops(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        ops: &[LexicalChannelOp],
        source_batch: bool,
    ) -> Result<(), CoreError> {
        if ops.is_empty() && !source_batch {
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
        validate_raw_file_mutations(ops)?;
        let key = GenKey {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
        };
        let mutates_index = source_batch || ops.iter().any(op_mutates_index);
        if ops.iter().any(op_writes_generation) {
            ensure_unsealed(&self.index_path(&key), generation, "an index or overlay op")?;
        }
        if !source_batch && mutates_index {
            let base = declared_delta_base_generation(ops)?;
            let mut checked = vec![self.index_path(&key)];
            if let Some(base) = base {
                checked.push(self.index_path(&GenKey {
                    repo_id: repo.clone(),
                    revision_id: revision.clone(),
                    generation: base,
                }));
            }
            for path in checked {
                match std::fs::symlink_metadata(path.join(SOURCE_FILE_COVERAGE_FILE_NAME)) {
                    Ok(_) => return Err(CoreError::InvalidContract("lexical: independent raw mutations cannot alter or inherit a coverage-bound generation".into())),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                    Err(error) => return Err(CoreError::Storage(format!("lexical: inspect coverage binding: {error}"))),
                }
            }
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

/// Decode the entire mutation list before any directory/writer is prepared.
/// Individual apply operations must never discover a later file-owner conflict
/// after an earlier operation has modified the index.
fn validate_raw_file_mutations(ops: &[LexicalChannelOp]) -> Result<(), CoreError> {
    let mut clear = Vec::new();
    let mut replace = Vec::new();
    let mut tombstone = Vec::new();
    let mut declared: Option<(BatchIngestMode, Option<ManifestGeneration>)> = None;
    for op in ops {
        match op {
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                let (mode, base, scope) = decode_replace_scope_payload(&payload.payload)?;
                validate_raw_base(&mut declared, mode, base, op.generation())?;
                replace.push(scope);
            }
            LexicalChannelOp::TombstoneLexicalScope(payload) => {
                let (mode, base, scope) = decode_tombstone_scope_payload(&payload.payload)?;
                validate_raw_base(&mut declared, mode, base, op.generation())?;
                tombstone.push(scope);
            }
            LexicalChannelOp::ClearLexicalSurface(payload) => {
                let mode = if payload.base_generation.is_some() {
                    BatchIngestMode::Delta
                } else {
                    BatchIngestMode::ReplaceGeneration
                };
                validate_raw_base(
                    &mut declared,
                    mode,
                    payload.base_generation,
                    op.generation(),
                )?;
                clear.push(payload.surface);
            }
            LexicalChannelOp::FullBundle(bundle) => {
                let _metadata = decode_repo_metadata_payload(&bundle.payload)?;
            }
            LexicalChannelOp::UpsertChunk(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::ReplaceStructuralScope(_)
            | LexicalChannelOp::Seal(_) => {}
        }
    }
    validate_lexical_file_mutations_v1(&clear, &replace, &tombstone).map_err(|error| {
        CoreError::InvalidContract(format!("lexical: file mutation admission: {error}"))
    })
}

fn validate_raw_base(
    declared: &mut Option<(BatchIngestMode, Option<ManifestGeneration>)>,
    mode: BatchIngestMode,
    base: Option<ManifestGeneration>,
    target: ManifestGeneration,
) -> Result<(), CoreError> {
    let valid = match (mode, base) {
        (BatchIngestMode::ReplaceGeneration, None) => true,
        (BatchIngestMode::Delta, Some(base)) => base < target,
        (BatchIngestMode::ReplaceGeneration, Some(_)) | (BatchIngestMode::Delta, None) => false,
    };
    if !valid
        || declared
            .as_ref()
            .is_some_and(|prior| *prior != (mode, base))
    {
        return Err(CoreError::InvalidContract(
            "lexical: raw file mutations disagree on a valid mode/base generation".into(),
        ));
    }
    *declared = Some((mode, base));
    Ok(())
}
