//! The ingest ports: search-corpus batches, repo-metadata overlays, the writer sweep and the metrics scrape.

use crate::adapter::{declared_delta_base_generation, legacy_ops_for_batch};
use crate::adapter_open::LexicalMutationTimings;
use crate::channel_payloads::{decode_replace_scope_payload, decode_tombstone_scope_payload};
use crate::generation_dir::{ensure_unsealed, is_writer_lock_entry, read_lexical_delta_base};
use crate::index_store::sealed_identity_entry_present;
use crate::overlay_codec::OverlayFamily;
use crate::overlay_codec::{
    decode_repo_metadata_payload, encode_file_contributor_batch, encode_file_ownership_batch,
    encode_repo_commit_recency_batch, encode_repo_description_batch, encode_repo_meta_batch,
    encode_repo_topic_batch,
};
use crate::sealed_generation::coverage::{
    CoveragePlan, CoverageReadPhase, CoverageWriteBase, SOURCE_FILE_COVERAGE_FILE_NAME,
    plan_file_coverage, read_staged_coverage, write_staged_coverage_observed,
};
use crate::sealed_generation::seal_generation;
use crate::stage_timing::elapsed_stage_ns;
use crate::{GenKey, LexicalAdapter, op_mutates_index, op_writes_generation};
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::{
    BatchIngestMode, FileContributorIngestBatch, FileOwnershipIngestBatch, GenerationSnapshot,
    LexicalBuildStageDurationsV1, ManifestGeneration, RepoCommitRecencyIngestBatch,
    RepoDescriptionIngestBatch, RepoId, RepoMetaIngestBatch, RepoTopicIngestBatch, RevisionId,
    SearchCorpusIngestBatch, SearchPlaneTrackKind, validate_lexical_file_mutations_v1,
};
use std::time::Instant;

use quanta_index_core::{
    CoreError, FileContributorIngestPort, FileOwnershipIngestPort, GenerationIdentityValidatePort,
    LexicalIndexBuildPort, MetricPointV1, MetricSourcePort, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMetaIngestPort, RepoTopicIngestPort, RequestBudgetV1,
    SearchCorpusBatchBuildPort, SearchCorpusPreflightPhaseV1, TrackDiskUsagePort,
    WriterIdleSweepPort, count_from_usize,
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
    /// delta's hard-linked base segments count once), observed while writers
    /// continue. Exact admission and retention use the separate snapshot port.
    fn track_disk_bytes(&self, budget: &RequestBudgetV1) -> Result<u64, CoreError> {
        budget.checkpoint("lexical-disk-usage:entry")?;
        quanta_index_scratch_sqlite::physical::measure_live_inode_tree_bytes(
            &self.state_root,
            &is_writer_lock_entry,
            self.retention_measurement_policy.max_main_file_bytes(),
            budget,
            budget.diagnostic_context(),
        )
    }
}

fn append_merge_observation(
    points: &mut Vec<MetricPointV1>,
    observation: crate::LexicalMaintenanceObservation,
) {
    let file = observation.file_merge;
    for (suffix, value) in [
        ("input_runs", file.input_runs),
        ("output_runs", file.output_runs),
        ("transferred_segments", file.transferred_segments),
        (
            "transferred_segment_attributed_bytes",
            file.transferred_segment_attributed_bytes,
        ),
        (
            "retired_run_posting_attributed_bytes",
            file.retired_run_posting_attributed_bytes,
        ),
        (
            "created_run_posting_attributed_bytes",
            file.created_run_posting_attributed_bytes,
        ),
    ] {
        points.push(MetricPointV1::gauge_count(
            format!("lexical_maintenance_last_sealed_file_merge_{suffix}"),
            value,
        ));
    }
    let native = observation.native_merge;
    for (suffix, value) in [
        ("input_runs", native.input_runs),
        ("input_segments", native.input_segments),
        (
            "input_attributed_index_bytes",
            native.input_index_encoded_bytes,
        ),
        ("output_available", u64::from(native.output.is_some())),
    ] {
        points.push(MetricPointV1::gauge_count(
            format!("lexical_maintenance_last_sealed_native_merge_{suffix}"),
            value,
        ));
    }
    if let Some(output) = native.output {
        for (suffix, value) in [
            ("output_segments", output.segments),
            ("output_attributed_index_bytes", output.index_encoded_bytes),
        ] {
            points.push(MetricPointV1::gauge_count(
                format!("lexical_maintenance_last_sealed_native_merge_{suffix}"),
                value,
            ));
        }
    }
}

impl MetricSourcePort for LexicalAdapter {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let writers = self.writer_cache_stats()?;
        let regex = self.regex_match_cache_stats()?;
        let text_authority = self.text_authority_update_stats()?;
        let write_cost = self.publication_write_stats()?;
        let maintenance = self.maintenance_stats()?;
        let seals = self.seal_commitment_stats()?;
        let phase = self.coverage_read_by_phase_stats()?;
        // Total and phase counters must describe the same instant while
        // publication can record another read concurrently with this scrape.
        let coverage = phase.total;
        let mut points = vec![
            MetricPointV1::counter(
                "lexical_completed_builds_with_pool_write_cost_total",
                write_cost.completed_builds,
            ),
            MetricPointV1::counter(
                "lexical_completed_build_pool_new_objects_total",
                write_cost.new_objects,
            ),
            MetricPointV1::counter(
                "lexical_completed_build_pool_new_encoded_bytes_total",
                write_cost.new_encoded_bytes,
            ),
            MetricPointV1::counter(
                "lexical_completed_build_pool_reused_objects_total",
                write_cost.reused_objects,
            ),
            MetricPointV1::counter(
                "lexical_completed_build_pool_reused_encoded_bytes_total",
                write_cost.reused_encoded_bytes,
            ),
            MetricPointV1::counter("lexical_seals_total", seals.seals),
            MetricPointV1::counter("lexical_seal_files_hashed_total", seals.files_hashed),
            MetricPointV1::counter("lexical_seal_bytes_hashed_total", seals.bytes_hashed),
            MetricPointV1::counter("lexical_seal_files_inherited_total", seals.files_inherited),
            MetricPointV1::counter("lexical_seal_bytes_inherited_total", seals.bytes_inherited),
            MetricPointV1::counter(
                "lexical_seal_file_admission_sources_verified_total",
                seals.file_admission_sources_verified,
            ),
            MetricPointV1::counter(
                "lexical_seal_file_admission_source_bytes_verified_total",
                seals.file_admission_source_bytes_verified,
            ),
            MetricPointV1::counter("lexical_coverage_decodes_total", coverage.decodes),
            MetricPointV1::counter(
                "lexical_coverage_root_bytes_read_total",
                coverage.root_bytes,
            ),
            MetricPointV1::counter("lexical_coverage_pages_read_total", coverage.pages),
            MetricPointV1::counter(
                "lexical_coverage_page_bytes_read_total",
                coverage.page_bytes,
            ),
            MetricPointV1::counter("lexical_coverage_rows_decoded_total", coverage.rows),
            MetricPointV1::counter(
                "lexical_coverage_before_intent_root_bytes_read_total",
                phase.before_intent.root_bytes,
            ),
            MetricPointV1::counter(
                "lexical_coverage_before_intent_pages_read_total",
                phase.before_intent.pages,
            ),
            MetricPointV1::counter(
                "lexical_coverage_before_intent_page_bytes_read_total",
                phase.before_intent.page_bytes,
            ),
            MetricPointV1::counter(
                "lexical_coverage_before_intent_rows_decoded_total",
                phase.before_intent.rows,
            ),
            MetricPointV1::counter(
                "lexical_coverage_under_lock_root_bytes_read_total",
                phase.under_operation_lock.root_bytes,
            ),
            MetricPointV1::counter(
                "lexical_coverage_under_lock_pages_read_total",
                phase.under_operation_lock.pages,
            ),
            MetricPointV1::counter(
                "lexical_coverage_under_lock_page_bytes_read_total",
                phase.under_operation_lock.page_bytes,
            ),
            MetricPointV1::counter(
                "lexical_coverage_under_lock_rows_decoded_total",
                phase.under_operation_lock.rows,
            ),
            MetricPointV1::counter(
                "lexical_coverage_build_root_bytes_read_total",
                phase.build.root_bytes,
            ),
            MetricPointV1::counter("lexical_coverage_build_pages_read_total", phase.build.pages),
            MetricPointV1::counter(
                "lexical_coverage_build_page_bytes_read_total",
                phase.build.page_bytes,
            ),
            MetricPointV1::counter(
                "lexical_coverage_build_rows_decoded_total",
                phase.build.rows,
            ),
            MetricPointV1::counter(
                "lexical_coverage_open_root_bytes_read_total",
                phase.open.root_bytes,
            ),
            MetricPointV1::counter("lexical_coverage_open_pages_read_total", phase.open.pages),
            MetricPointV1::counter(
                "lexical_coverage_open_page_bytes_read_total",
                phase.open.page_bytes,
            ),
            MetricPointV1::counter("lexical_coverage_open_rows_decoded_total", phase.open.rows),
            MetricPointV1::gauge_count(
                "lexical_coverage_max_decode_heap_admission_bytes",
                coverage.max_decode_heap_admission_bytes,
            ),
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
            MetricPointV1::counter(
                "lexical_text_authority_posting_pages_written_total",
                text_authority.posting_pages_written,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_oversized_posting_pages_total",
                text_authority.oversized_posting_pages,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_oversized_posting_bytes_total",
                text_authority.oversized_posting_bytes,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_compaction_rederived_docs_total",
                text_authority.compaction_rederived_docs,
            ),
            MetricPointV1::counter(
                "lexical_text_authority_compaction_rederived_text_bytes_total",
                text_authority.compaction_rederived_text_bytes,
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
        ];
        points.push(MetricPointV1::counter(
            "lexical_maintenance_observed_sealed_publications_total",
            maintenance.sealed_publications,
        ));
        points.push(MetricPointV1::gauge_count(
            "lexical_maintenance_last_sealed_observation_available",
            u64::from(maintenance.last_observation.is_some()),
        ));
        if let Some(observation) = maintenance.last_observation {
            append_merge_observation(&mut points, observation);
            for (prefix, before, after, debt) in [
                (
                    "file",
                    observation.file_before,
                    observation.file_after,
                    observation.file_debt_entries,
                ),
                (
                    "native",
                    observation.native_before,
                    observation.native_after,
                    observation.native_debt_entries,
                ),
            ] {
                for (suffix, value) in [
                    ("base_runs", before.runs()),
                    ("base_segments", before.segments()),
                    ("base_attributed_index_bytes", before.active_encoded_bytes()),
                    ("target_runs", after.runs()),
                    ("target_segments", after.segments()),
                    (
                        "target_attributed_index_bytes",
                        after.active_encoded_bytes(),
                    ),
                    ("target_debt_entries", debt),
                ] {
                    points.push(MetricPointV1::gauge_count(
                        format!("lexical_maintenance_last_sealed_{prefix}_{suffix}"),
                        value,
                    ));
                }
            }
        }
        points.push(MetricPointV1::gauge_count(
            "lexical_pool_quota_enabled",
            u64::from(self.pool_write_admission.is_some()),
        ));
        if let Some(admission) = &self.pool_write_admission {
            // One ledger snapshot keeps the occupancy partitions consistent.
            // A damaged ledger makes the scrape fail, rather than publishing
            // plausible zero occupancy or headroom.
            let usage = admission.snapshot()?;
            let available = usage
                .ceiling_bytes
                .checked_sub(usage.total_charged_bytes)
                .ok_or_else(|| {
                    CoreError::Storage("lexical pool quota exceeds its ceiling".into())
                })?;
            points.extend([
                MetricPointV1::gauge_count("lexical_pool_quota_file_limit", usage.file_limit),
                MetricPointV1::gauge_count("lexical_pool_quota_charged_files", usage.charged_files),
                MetricPointV1::gauge_count("lexical_pool_quota_ceiling_bytes", usage.ceiling_bytes),
                MetricPointV1::gauge_count(
                    "lexical_pool_quota_canonical_bytes",
                    usage.canonical_bytes,
                ),
                MetricPointV1::gauge_count(
                    "lexical_pool_quota_in_flight_bytes",
                    usage.in_flight_bytes,
                ),
                MetricPointV1::gauge_count("lexical_pool_quota_orphan_bytes", usage.orphan_bytes),
                MetricPointV1::gauge_count(
                    "lexical_pool_quota_charged_bytes",
                    usage.total_charged_bytes,
                ),
                MetricPointV1::gauge_count("lexical_pool_quota_available_bytes", available),
            ]);
        }
        Ok(points)
    }
}

impl SearchCorpusBatchBuildPort for LexicalAdapter {
    fn prepare_hard_recovery_scan_with_owner(
        &self,
        batch: &SearchCorpusIngestBatch,
        owner: &mut quanta_index_core::PublicationValidationOwner,
        budget: &RequestBudgetV1,
        diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<(), CoreError> {
        owner.bind_batch(batch)?;
        if self.maintenance_policy.is_none() || batch.base_generation.is_none() {
            return Ok(());
        }
        let scan_budget = budget.child();
        let key = GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
        };
        let _mutation = self.generation_build_guards(&key, batch.base_generation)?;
        let _lifecycle = self.directory_lifecycle_read_guard()?;
        let base_directory = batch.base_generation.map(|generation| {
            self.index_path(&GenKey {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                generation,
            })
        });
        let reservation = owner.lexical_reservation().ok_or_else(|| {
            CoreError::InvalidContract(
                "hard scan requires catalog-owned document reservations".into(),
            )
        })?;
        let mut publication = crate::lexical_publication::PublicationContext::begin(
            &self.state_root,
            batch,
            reservation,
            base_directory.as_deref(),
            None,
            diagnostic,
            crate::lexical_publication::PublicationAdmission {
                maintenance_policy: self.maintenance_policy,
                pool_write: self.pool_write_admission.clone(),
                object_lifetime: self.object_lifetime.clone(),
                control_output: if owner.has_publication_control_permit() {
                    owner.output_admission().owned_control_admission()?
                } else {
                    None
                },
                work_output: owner.output_admission().owned_work_admission()?,
            },
        )?;
        let ops = legacy_ops_for_batch(batch, batch.seal)?;
        let source_directory = base_directory.as_deref().ok_or_else(|| {
            CoreError::InvalidContract("hard scan delta base directory is absent".into())
        })?;
        let mutation = crate::file_authority::plan_ops(source_directory, &ops)?;
        let native_plan = crate::text_authority_plan::plan_text_authority_delta(
            &publication.store,
            publication.native_base.as_ref(),
            &ops,
            &publication.reservation.native_text,
        )?;
        publication.prepare_or_load_hard_scan(
            &mutation,
            &native_plan,
            Some(&scan_budget),
            crate::lexical_hard_scan::HardScanStage::Prepare,
        )
    }

    fn preflight_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
        phase: SearchCorpusPreflightPhaseV1,
        diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<(), CoreError> {
        self.preflight_batch_with_owner(
            batch,
            phase,
            &mut quanta_index_core::PublicationValidationOwner::default(),
            diagnostic,
        )
    }

    fn preflight_batch_with_owner(
        &self,
        batch: &SearchCorpusIngestBatch,
        phase: SearchCorpusPreflightPhaseV1,
        owner: &mut quanta_index_core::PublicationValidationOwner,
        diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<(), CoreError> {
        crate::causal_profile::observe_result(
            diagnostic,
            quanta_index_core::DiagnosticStageV1::BuildPreflight,
            |diagnostic| {
                owner.bind_batch(batch)?;
                let read_phase = match phase {
                    SearchCorpusPreflightPhaseV1::BeforeIntent => CoverageReadPhase::BeforeIntent,
                    SearchCorpusPreflightPhaseV1::UnderOperationLock => {
                        CoverageReadPhase::UnderOperationLock
                    }
                };
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
                if sealed_identity_entry_present(&directory)? {
                    // A full replacement can reach the existing identity-fenced repair
                    // planner for damaged content. Proving that content here would
                    // refuse before the materializer can reclaim and rebuild it. This
                    // is mutation admission only: the sealed identity must still match,
                    // and neither build nor open accepts an unproved generation.
                    let (_directory, _observed) =
                        self.sealed_generation_dir_for(&identity, "batch preflight")?;
                    let proved = match crate::sealed_generation::walk_sealed_generation_reusing_proofs(
                &directory,
                &identity,
                None,
                owner
                    .proof_state::<crate::sealed_generation::publication_proof::PublicationProofs>(
                        SearchPlaneTrackKind::Lexical,
                    )?,
                None,
                diagnostic,
                &self.open_existing_store()?,
            ) {
                Ok(proved) => proved,
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    ..
                }) if batch.mode == BatchIngestMode::ReplaceGeneration => return Ok(()),
                Err(error) => return Err(error),
            };
                    self.record_coverage_read(read_phase, proved.coverage_read_stats)?;
                    if proved.source_publication.as_ref() != Some(&batch.source_event) {
                        return Err(CoreError::InvalidContract(
                            "lexical: sealed target belongs to another source event".into(),
                        ));
                    }
                    return Ok(());
                }
                let _planned = self.plan_batch_coverage(
                    batch, &identity, &directory, read_phase, owner, diagnostic,
                )?;
                self.preflight_file_authority_batch(batch, owner)?;
                Ok(())
            },
        )
    }

    fn build_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
        diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<Option<LexicalBuildStageDurationsV1>, CoreError> {
        self.build_batch_with_owner(
            batch,
            &mut quanta_index_core::PublicationValidationOwner::default(),
            diagnostic,
        )
    }

    fn build_batch_with_owner(
        &self,
        batch: &SearchCorpusIngestBatch,
        owner: &mut quanta_index_core::PublicationValidationOwner,
        diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<Option<LexicalBuildStageDurationsV1>, CoreError> {
        crate::causal_profile::observe_result_with_sync(
            diagnostic,
            quanta_index_core::DiagnosticStageV1::LexicalBuild,
            |diagnostic, sync_totals| {
                owner.bind_batch(batch)?;
                let preparation_started = Instant::now();
                // Storage-free admission precedes even the sealed replay shortcut.
                // The same owner validates the public materializer and raw channel.
                batch.validate_surface_mutations_v1().map_err(|error| {
                    CoreError::InvalidContract(format!("lexical: file mutation admission: {error}"))
                })?;
                batch.validate_v1().map_err(|error| {
                    CoreError::InvalidContract(format!("lexical: batch admission: {error}"))
                })?;
                if owner.lexical_reservation().is_none() {
                    return Err(CoreError::InvalidContract(
                        "lexical build requires catalog-owned document ID reservations".into(),
                    ));
                }
                if let Some(payload) = &batch.bundle_payload {
                    let _metadata = decode_repo_metadata_payload(payload)?;
                }
                let key = GenKey {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    generation: batch.generation,
                };
                let _mutation = self.generation_build_guards(&key, batch.base_generation)?;
                let _lifecycle = self.directory_lifecycle_read_guard()?;
                let candidate = GenerationSnapshot {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    track: SearchPlaneTrackKind::Lexical,
                    manifest_generation: batch.generation,
                    manifest_digest: batch.manifest_digest.clone(),
                };
                let generation_dir = self.index_path(&key);
                if sealed_identity_entry_present(&generation_dir)? {
                    if !batch.seal {
                        return Err(CoreError::Typed {
                            code:
                                quanta_index_contract::SearchPlaneErrorCodeV2::GenerationImmutable,
                            message: format!(
                                "lexical: generation {} is already sealed; refusing non-seal mutation",
                                batch.generation.get()
                            ),
                        });
                    }
                    self.validate_generation_identity_with_owner(&candidate, owner)?;
                    let verified = crate::sealed_generation::walk_sealed_generation_reusing_proofs(
                &generation_dir,
                &candidate,
                None,
                owner
                    .proof_state::<crate::sealed_generation::publication_proof::PublicationProofs>(
                        SearchPlaneTrackKind::Lexical,
                    )?,
                None,
                diagnostic,
                &self.open_existing_store()?,
            )?;
                    self.record_coverage_read(
                        CoverageReadPhase::Build,
                        verified.coverage_read_stats,
                    )?;
                    if verified.source_publication.as_ref() != Some(&batch.source_event) {
                        return Err(CoreError::InvalidContract(
                            "lexical: sealed generation belongs to a different source event".into(),
                        ));
                    }
                    self.settle_published_adoptions(&candidate)?;
                    return Ok(None);
                }
                let ops = legacy_ops_for_batch(batch, batch.seal)?;
                let preflight_started = Instant::now();
                self.preflight_file_authority_batch(batch, owner)?;
                let prep_file_authority_preflight_ns = elapsed_stage_ns(preflight_started)?;
                let coverage = self.plan_batch_coverage(
                    batch,
                    &candidate,
                    &generation_dir,
                    CoverageReadPhase::Build,
                    owner,
                    diagnostic,
                )?;
                let reservation = owner.lexical_reservation().ok_or_else(|| {
                    CoreError::InvalidContract(
                        "lexical build requires catalog-owned document ID reservations".into(),
                    )
                })?;
                let base_directory = batch.base_generation.map(|generation| {
                    self.index_path(&GenKey {
                        repo_id: batch.repo_id.clone(),
                        revision_id: batch.revision_id.clone(),
                        generation,
                    })
                });
                let mut publication = crate::lexical_publication::PublicationContext::begin(
                    &self.state_root,
                    batch,
                    reservation,
                    base_directory.as_deref(),
                    sync_totals.clone(),
                    diagnostic,
                    crate::lexical_publication::PublicationAdmission {
                        maintenance_policy: self.maintenance_policy,
                        pool_write: self.pool_write_admission.clone(),
                        object_lifetime: self.object_lifetime.clone(),
                        control_output: if owner.has_publication_control_permit() {
                            owner.output_admission().owned_control_admission()?
                        } else {
                            None
                        },
                        work_output: owner.output_admission().owned_work_admission()?,
                    },
                )?;
                let native_plan = crate::text_authority_plan::plan_text_authority_delta(
                    &publication.store,
                    publication.native_base.as_ref(),
                    &ops,
                    &publication.reservation.native_text,
                )?;
                if publication.maintenance_policy.is_some() {
                    let source_directory = base_directory.as_deref().unwrap_or(&generation_dir);
                    let mutation = crate::file_authority::plan_ops(source_directory, &ops)?;
                    publication.prepare_or_load_hard_scan(
                        &mutation,
                        &native_plan,
                        None,
                        crate::lexical_hard_scan::HardScanStage::UsePrepared,
                    )?;
                }
                let (clear_all, retired, prepare_file) = match &native_plan.write {
                    crate::TextAuthorityWrite::None => (false, &[][..], true),
                    crate::TextAuthorityWrite::Delta {
                        clear_all, retired, ..
                    } => (*clear_all, retired.as_slice(), false),
                };
                if !clear_all {
                    publication.compaction_worker =
                        crate::lexical_compaction::CompactionWorker::start(
                            &publication.store,
                            publication.file_base.as_ref().filter(|_base| prepare_file),
                            publication.native_base.as_ref(),
                            retired,
                            diagnostic,
                            owner.prepare_request_budget(),
                            publication.maintenance_policy.as_ref(),
                        )?;
                }
                publication.native_plan = Some(native_plan);
                // The prepared coverage marks this target as bound before any index
                // mutation. It is query-invisible until the index and artifact seal.
                let coverage_write_started = Instant::now();
                let coverage_root = write_staged_coverage_observed(
                    &generation_dir,
                    &candidate,
                    &batch.source_event,
                    &coverage,
                    sync_totals.as_deref(),
                )?;
                let prep_coverage_write_ns = elapsed_stage_ns(coverage_write_started)?;
                self.prepare_generation_from_base_observed(
                    &key,
                    batch.base_generation,
                    sync_totals.as_deref(),
                )?;
                let preparation_ns = elapsed_stage_ns(preparation_started)?;
                let mutation = crate::causal_profile::observe_result(
                    diagnostic,
                    quanta_index_core::DiagnosticStageV1::BuildPrepare,
                    |diagnostic| {
                        self.build_ops(
            &batch.repo_id,
            &batch.revision_id,
            batch.generation,
            &ops,
            true,
            Some(
                &mut owner
                    .proof_state::<crate::sealed_generation::publication_proof::PublicationProofs>(
                        SearchPlaneTrackKind::Lexical,
                    )?
                    .file_plan,
            ),
                            Some(&mut publication),
                            diagnostic,
                        )
                    },
                )?;
                if let Some(policy) = &publication.maintenance_policy {
                    publication.admit_maintenance_target(policy)?;
                }
                let mut stages = LexicalBuildStageDurationsV1 {
                    preparation_ns,
                    prep_file_authority_preflight_ns: Some(prep_file_authority_preflight_ns),
                    prep_coverage_write_ns: Some(prep_coverage_write_ns),
                    writer_mutation_ns: mutation.writer_mutation_ns,
                    text_authority_ns: mutation.text_authority_ns,
                    text_authority_collect_ns: mutation.text_authority_collect_ns,
                    text_authority_shard_build_ns: mutation.text_authority_shard_build_ns,
                    text_authority_publish_ns: mutation.text_authority_publish_ns,
                    file_authority_ns: mutation.file_authority_ns,
                    file_authority_source_write_ns: mutation.file_authority_source_write_ns,
                    ..LexicalBuildStageDurationsV1::default()
                };
                let completed_write_cost = if batch.seal {
                    let (writer_timings, commitment_ns, file_admission_ns, seal_ns, write_cost) =
                        crate::causal_profile::observe_result(
                            diagnostic,
                            quanta_index_core::DiagnosticStageV1::Finalize,
                            |finalize_diagnostic| {
                                let seal_started = Instant::now();
                                // Retire the writer before measuring. The sealed identity is
                                // the promotion point after the manifest and its inputs.
                                let writer_timings = self.finalize_index_for_seal_observed(
                                    &key,
                                    sync_totals.as_deref(),
                                )?;
                                let base_dir =
                                    read_lexical_delta_base(&generation_dir)?.map(|base| {
                                        self.index_path(&GenKey {
                                            repo_id: key.repo_id.clone(),
                                            revision_id: key.revision_id.clone(),
                                            generation: base,
                                        })
                                    });
                                let commitment_started = Instant::now();
                                let sealed = crate::causal_profile::observe_result(
                                    finalize_diagnostic,
                                    quanta_index_core::DiagnosticStageV1::LexicalSeal,
                                    |seal_diagnostic| {
                                        seal_generation(
                                            &generation_dir,
                                            &self.fields,
                                            &candidate,
                                            base_dir.as_deref(),
                                            &coverage_root,
                                            &publication,
                                            sync_totals.as_deref(),
                                            seal_diagnostic,
                                        )
                                    },
                                )?;
                                let commitment_ns = elapsed_stage_ns(commitment_started)?;
                                let file_admission_ns = sealed.file_admission_ns;
                                self.record_seal_measurement(sealed.measurement)?;
                                crate::index_store::persist_lexical_sealed_identity_observed(
                                    &generation_dir,
                                    &candidate,
                                    sync_totals.as_deref(),
                                )?;
                                // The sealed identity is already durable. Later attempt
                                // retirement can fail and be repaired by replay without
                                // rebuilding, so record the seal at this boundary.
                                self.record_maintenance_observation(
                                    publication.maintenance_observation(
                                        sealed.file_merge,
                                        sealed.native_merge,
                                    )?,
                                )?;
                                let write_cost = publication.finish()?;
                                Ok::<_, CoreError>((
                                    writer_timings,
                                    commitment_ns,
                                    file_admission_ns,
                                    elapsed_stage_ns(seal_started)?,
                                    write_cost,
                                ))
                            },
                        )?;
                    stages.seal_ns = Some(seal_ns);
                    stages.seal_writer_commit_ns = Some(writer_timings.commit_ns);
                    stages.seal_merge_wait_ns = Some(writer_timings.merge_wait_ns);
                    stages.seal_commitment_ns = Some(commitment_ns);
                    stages.seal_file_admission_ns = Some(file_admission_ns);
                    write_cost
                } else {
                    publication.write_cost_snapshot()?
                };
                // Every batch is a chance to give an abandoned generation's heap back
                // to the envelope (QI-BB-016).
                let _released = self.release_idle_writers_under_lifecycle()?;
                self.record_publication_write_cost(completed_write_cost)?;
                Ok(Some(stages))
            },
        )
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
        let key = GenKey {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            generation,
        };
        let base = declared_delta_base_generation(ops)?;
        let _mutation = self.generation_build_guards(&key, base)?;
        let _lifecycle = self.directory_lifecycle_read_guard()?;
        self.build_ops(repo, revision, generation, ops, false, None, None, None)
            .map(|_timings| ())
    }
}

#[cfg(test)]
mod tests;

impl LexicalAdapter {
    fn plan_batch_coverage(
        &self,
        batch: &SearchCorpusIngestBatch,
        candidate: &GenerationSnapshot,
        generation_dir: &std::path::Path,
        read_phase: CoverageReadPhase,
        owner: &mut quanta_index_core::PublicationValidationOwner,
        diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<CoveragePlan, CoreError> {
        crate::causal_profile::timed_work("lexical_plan_batch_coverage", || {
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
                    let mut cache = self.coverage_decode_cache.lock().map_err(|error| {
                        CoreError::Storage(format!(
                            "lexical: coverage decode cache poisoned: {error}"
                        ))
                    })?;
                    let mut decoded = std::mem::take(&mut *cache);
                    drop(cache);
                    let verified =
                        crate::sealed_generation::walk_sealed_generation_reusing_proofs(
                            &base_dir,
                            &identity,
                            Some(&mut decoded),
                            owner.proof_state::<crate::sealed_generation::publication_proof::PublicationProofs>(SearchPlaneTrackKind::Lexical)?,
                            None,
                            diagnostic,
                         &self.open_existing_store()?)?;
                    *self.coverage_decode_cache.lock().map_err(|error| {
                        CoreError::Storage(format!(
                            "lexical: coverage decode cache poisoned: {error}"
                        ))
                    })? = decoded;
                    self.record_coverage_read(read_phase, verified.coverage_read_stats)?;
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
                    self.validate_inherited_candidate_ownership(&verified.reader, batch)?;
                    let coverage = verified.coverage.ok_or_else(|| CoreError::Typed { code: quanta_index_contract::SearchPlaneErrorCodeV2::SymbolCoverageUnavailable, message: "lexical: coverage delta requires a base with admitted file coverage; rebuild the generation".into() })?;
                    let root = verified.manifest.source_coverage.ok_or_else(|| {
                        CoreError::Storage(
                            "lexical: verified coverage has no root commitment".into(),
                        )
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
            let coverage = plan_file_coverage(
                &base_coverage,
                base,
                batch.replace_scopes.iter().map(|scope| &scope.coverage),
                batch.tombstone_scopes.iter().map(|scope| &scope.file),
                &batch.clear_surfaces,
            )?;
            let staged = read_staged_coverage(generation_dir, candidate)?;
            if let Some(staged) = staged.as_ref() {
                self.record_coverage_read(read_phase, staged.read_stats)?;
            }
            match staged {
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
                        "lexical: cannot bind coverage over pre-existing unbound index content"
                            .into(),
                    ));
                }
                Some(_) | None => {}
            }
            Ok(coverage)
        })
    }

    fn validate_inherited_candidate_ownership(
        &self,
        reader: &tantivy::IndexReader,
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
    fn preflight_file_authority_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
        owner: &mut quanta_index_core::PublicationValidationOwner,
    ) -> Result<(), CoreError> {
        let source_generation = batch.base_generation.unwrap_or(batch.generation);
        let source_dir = self.index_path(&GenKey {
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: source_generation,
        });
        let ops = legacy_ops_for_batch(batch, false)?;
        owner
            .proof_state::<crate::sealed_generation::publication_proof::PublicationProofs>(
                SearchPlaneTrackKind::Lexical,
            )?
            .file_plan
            .prepare(&source_dir, &ops)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "The publication owner receives explicit source identity and independent plan, lease, and diagnostic custody."
    )]
    fn build_ops(
        &self,
        repo: &RepoId,
        revision: &RevisionId,
        generation: ManifestGeneration,
        ops: &[LexicalChannelOp],
        source_batch: bool,
        file_plan: Option<&mut crate::file_authority::FileAuthorityPlanCache>,
        publication: Option<&mut crate::lexical_publication::PublicationContext>,
        diagnostic: Option<&quanta_index_core::DiagnosticContextV1>,
    ) -> Result<LexicalMutationTimings, CoreError> {
        if ops.is_empty() && !source_batch {
            return Ok(LexicalMutationTimings::default());
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
        if mutates_index && publication.is_none() {
            return Err(CoreError::InvalidContract(
                "lexical mutations require a source-corpus publication and durable document IDs"
                    .into(),
            ));
        }
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
                let _committed = self.apply_snapshot_op(&key, op, None)?;
            }
            return Ok(LexicalMutationTimings::default());
        }
        let handle = self.writer_handle_observed(
            &key,
            publication
                .as_ref()
                .and_then(|context| context.store.sync_totals()),
        )?;
        self.commit_ops_under_lock(
            &handle,
            &key,
            ops,
            file_plan,
            publication.ok_or_else(|| {
                CoreError::InvalidContract("lexical publication context missing".into())
            })?,
            diagnostic,
        )
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
