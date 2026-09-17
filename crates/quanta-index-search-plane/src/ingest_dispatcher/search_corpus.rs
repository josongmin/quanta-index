//! `DirectSearchCorpusMaterializer`: the lexical + semantic search-corpus
//! ingest path, from resource preflight through sealed-generation finalize
//! and retired-generation reclaim.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, RwLock};

use quanta_index_contract::{
    BatchPublishReceipt, GenerationSnapshot, ManifestGeneration, SearchCorpusIngestBatch,
    SearchPlaneTrackKind,
};
use quanta_index_core::{
    AuxiliaryAuthorityCatalogPort, CoreError, GenerationIdentityValidatePort,
    IdempotencyCatalogPort, IncompleteGenerationDiscardPort, IngestBatchFootprint,
    IngestResourcePolicy, MetricPointV1, MetricSourcePort, SealedGenerationReclaimOutcomeV1,
    SealedGenerationReclaimPort, SearchCorpusBatchBuildPort, SearchCorpusIngestPort,
    SemanticIngestPort, TextEmbeddingProvider, count_from_usize,
};

use crate::auxiliary_authority::{structural_chunks_delta_rows, structural_chunks_transition};
use crate::ingest_dispatcher::auxiliary::AuxiliaryMutationCoordinator;
use crate::ingest_dispatcher::errors::{
    ERR_SEARCH_CORPUS_BATCH_SHAPE, ERR_SEARCH_CORPUS_DELTA_BASE_NOT_SEALED,
    ERR_SEARCH_CORPUS_GENERATION_CONFLICT,
};
use crate::ingest_dispatcher::generation_plan::{
    PhysicalGenerationStateV1, SealedGenerationBuildPlanV1, SearchCorpusPhysicalReclaimReceiptV1,
    batch_publish_receipt_v1, ensure_generation_is_mutable_v1, generation_pair_from_batch_v1,
    inspect_physical_generation_v1, validate_physical_generation_v1,
    validate_semantic_publish_receipt_v1,
};
use crate::ingest_dispatcher::ports::SearchCorpusAuthorityWritePort;
use crate::readiness::{
    SEARCH_CORPUS_LOCK_STRIPES_V1, SearchCorpusHistoryRetentionReceiptV1,
    search_corpus_lock_stripe_v1,
};
use crate::semantic_derive::{
    DEFAULT_SEMANTIC_DERIVATION_MODE_V1, SemanticDerivationModeV1,
    derive_semantic_batch_with_mode_v1, semantic_derivation_mode_from_env_v1,
};
use crate::{
    Ledger, SealedSearchCorpusAuthorityStateV1, SnapshotKey, SnapshotRegistries,
    SnapshotRetireOutcome,
};

/// Direct search-corpus batch materializer that commits lexical and derived
/// semantic generations, then durably admits the complete sealed identity into
/// rollback history.
pub struct DirectSearchCorpusMaterializer {
    builder: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
    semantic_ingest: Arc<dyn SemanticIngestPort + Send + Sync>,
    semantic_embedder: Arc<dyn TextEmbeddingProvider + Send + Sync>,
    authority: Arc<dyn SearchCorpusAuthorityWritePort + Send + Sync>,
    lexical_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    semantic_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    lexical_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>,
    semantic_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>,
    /// Physical GC of retired sealed generations (QI-BB-003): one reclaim
    /// port per track plus the registries that must release their handles
    /// before any bytes go.
    lexical_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    semantic_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    snapshots: SnapshotRegistries,
    idempotency: Arc<dyn IdempotencyCatalogPort + Send + Sync>,
    /// The envelope every batch is measured against before anything is
    /// held (QI-BB-021), and what the measured batches added up to.
    resource_policy: IngestResourcePolicy,
    resource_stats: Mutex<IngestResourceStats>,
    auxiliary_catalog: Arc<dyn AuxiliaryAuthorityCatalogPort + Send + Sync>,
    auxiliary_coordinator: Arc<AuxiliaryMutationCoordinator>,
    operation_locks: [Mutex<()>; SEARCH_CORPUS_LOCK_STRIPES_V1],
    semantic_derivation_mode: SemanticDerivationModeV1,
}

/// What the resource envelope admitted and refused, and the widest batch
/// it admitted (QI-BB-021).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IngestResourceStats {
    /// Batches that fit the envelope.
    pub admitted: u64,
    /// Batches refused typed for exceeding it.
    pub refused: u64,
    /// Most embedded records one admitted batch carried.
    pub peak_embedded_records: usize,
    /// Most bytes of text one admitted batch asked to embed.
    pub peak_text_bytes: u64,
    /// Most bytes of vectors one admitted batch expanded into.
    pub peak_vector_bytes: u64,
}

impl IngestResourceStats {
    fn record_admitted(&mut self, footprint: &IngestBatchFootprint) {
        self.admitted = self.admitted.saturating_add(1);
        self.peak_embedded_records = self.peak_embedded_records.max(footprint.embedded_records);
        self.peak_text_bytes = self.peak_text_bytes.max(footprint.text_bytes);
        self.peak_vector_bytes = self.peak_vector_bytes.max(footprint.vector_bytes);
    }

    fn record_refused(&mut self) {
        self.refused = self.refused.saturating_add(1);
    }
}

/// Composition-owned ports required to materialize one search-corpus generation.
pub struct SearchCorpusMaterializerParts {
    pub builder: Arc<dyn SearchCorpusBatchBuildPort + Send + Sync>,
    pub ledger: Arc<RwLock<Ledger>>,
    pub semantic_ingest: Arc<dyn SemanticIngestPort + Send + Sync>,
    pub semantic_embedder: Arc<dyn TextEmbeddingProvider + Send + Sync>,
    pub authority: Arc<dyn SearchCorpusAuthorityWritePort + Send + Sync>,
    pub lexical_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    pub semantic_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    pub lexical_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>,
    pub semantic_incomplete_discard: Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>,
    pub lexical_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    pub semantic_reclaim: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    pub snapshots: SnapshotRegistries,
    /// Idempotency records are forgotten with the generation they describe
    /// (QI-BB-032 retention).
    pub idempotency: Arc<dyn IdempotencyCatalogPort + Send + Sync>,
    /// The resource envelope one batch may ask the plane to hold (QI-BB-021).
    pub resource_policy: IngestResourcePolicy,
    /// The structural chunk universe of every generation is durable in the
    /// auxiliary catalog before the generation is finalized, and auxiliary
    /// generations are forgotten with retention (QI-BB-020).
    pub auxiliary_catalog: Arc<dyn AuxiliaryAuthorityCatalogPort + Send + Sync>,
    pub auxiliary_coordinator: Arc<AuxiliaryMutationCoordinator>,
}

impl DirectSearchCorpusMaterializer {
    #[must_use]
    pub fn new_with_search_owned_semantics(parts: SearchCorpusMaterializerParts) -> Self {
        Self::new_with_search_owned_semantics_with_mode(parts, DEFAULT_SEMANTIC_DERIVATION_MODE_V1)
    }

    pub fn new_with_search_owned_semantics_from_env(
        parts: SearchCorpusMaterializerParts,
    ) -> Result<Self, CoreError> {
        let mode = semantic_derivation_mode_from_env_v1()?;
        Ok(Self::new_with_search_owned_semantics_with_mode(parts, mode))
    }

    fn new_with_search_owned_semantics_with_mode(
        parts: SearchCorpusMaterializerParts,
        semantic_derivation_mode: SemanticDerivationModeV1,
    ) -> Self {
        let SearchCorpusMaterializerParts {
            builder,
            ledger,
            semantic_ingest,
            semantic_embedder,
            authority,
            lexical_generation_validator,
            semantic_generation_validator,
            lexical_incomplete_discard,
            semantic_incomplete_discard,
            lexical_reclaim,
            semantic_reclaim,
            snapshots,
            idempotency,
            resource_policy,
            auxiliary_catalog,
            auxiliary_coordinator,
        } = parts;
        Self {
            builder,
            ledger,
            semantic_ingest,
            semantic_embedder,
            authority,
            lexical_generation_validator,
            semantic_generation_validator,
            lexical_incomplete_discard,
            semantic_incomplete_discard,
            lexical_reclaim,
            semantic_reclaim,
            snapshots,
            idempotency,
            resource_policy,
            resource_stats: Mutex::new(IngestResourceStats::default()),
            auxiliary_catalog,
            auxiliary_coordinator,
            operation_locks: std::array::from_fn(|_index| Mutex::new(())),
            semantic_derivation_mode,
        }
    }

    /// What the resource envelope has admitted and refused so far.
    pub fn resource_stats(&self) -> Result<IngestResourceStats, CoreError> {
        self.resource_stats
            .lock()
            .map(|stats| *stats)
            .map_err(|err| {
                CoreError::Storage(format!(
                    "direct search-corpus materialize: resource stats poisoned: {err}"
                ))
            })
    }
}

/// The resource envelope's tallies as scrape points, `ingest_…` (QI-BB-015).
impl MetricSourcePort for DirectSearchCorpusMaterializer {
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        let stats = self.resource_stats()?;
        Ok(vec![
            MetricPointV1::counter("ingest_batches_admitted_total", stats.admitted),
            MetricPointV1::counter("ingest_batches_refused_total", stats.refused),
            MetricPointV1::gauge_count(
                "ingest_peak_embedded_records",
                count_from_usize(stats.peak_embedded_records),
            ),
            MetricPointV1::gauge_count("ingest_peak_text_bytes", stats.peak_text_bytes),
            MetricPointV1::gauge_count("ingest_peak_vector_bytes", stats.peak_vector_bytes),
        ])
    }
}

impl DirectSearchCorpusMaterializer {
    /// Measure `batch` against the envelope before anything is held
    /// (QI-BB-021); a batch that does not fit is refused typed here, with
    /// zero bytes changed.
    fn admit_resource_envelope(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
        let outcome = self
            .resource_policy
            .admit_search_corpus_batch(batch, self.semantic_embedder.dimension());
        {
            let mut stats = self.resource_stats.lock().map_err(|err| {
                CoreError::Storage(format!(
                    "direct search-corpus materialize: resource stats poisoned: {err}"
                ))
            })?;
            match &outcome {
                Ok(footprint) => stats.record_admitted(footprint),
                Err(_refused) => stats.record_refused(),
            }
        }
        outcome.map(|_footprint| ())
    }
}

impl SearchCorpusIngestPort for DirectSearchCorpusMaterializer {
    fn publish_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        batch.validate_v1().map_err(|err| CoreError::Typed {
            code: ERR_SEARCH_CORPUS_BATCH_SHAPE.to_string(),
            message: format!("direct search-corpus materialize: {err}"),
        })?;
        batch.validate_surface_mutations_v1().map_err(|err| {
            CoreError::InvalidContract(format!("direct search-corpus materialize: {err}"))
        })?;
        self.admit_resource_envelope(batch)?;
        let stripe = search_corpus_lock_stripe_v1(&batch.repo_id, &batch.revision_id);
        let operation_lock = self.operation_locks.get(stripe).ok_or_else(|| {
            CoreError::Storage(format!(
                "direct search-corpus materialize: computed operation-lock stripe {stripe} outside configured range"
            ))
        })?;
        let _operation_guard = operation_lock.lock().map_err(|err| {
            CoreError::Storage(format!(
                "direct search-corpus materialize: operation-lock stripe {stripe} poisoned: {err}"
            ))
        })?;

        if let Some(base_generation) = batch.base_generation {
            self.preflight_delta_base_v1(batch, base_generation)?;
        }

        if !batch.seal {
            let (lexical, semantic) = generation_pair_from_batch_v1(batch);
            ensure_generation_is_mutable_v1(
                self.lexical_generation_validator.as_ref(),
                &lexical,
                "lexical",
            )?;
            ensure_generation_is_mutable_v1(
                self.semantic_generation_validator.as_ref(),
                &semantic,
                "semantic",
            )?;
        }

        let sealed_plan = if batch.seal {
            Some(self.preflight_sealed_generation_v1(batch)?)
        } else {
            None
        };
        if sealed_plan
            .as_ref()
            .is_some_and(SealedGenerationBuildPlanV1::is_finalize_only)
        {
            self.finalize_sealed_generation_v1(batch)?;
            return Ok(batch_publish_receipt_v1(batch));
        }
        if let Some(plan) = sealed_plan.as_ref() {
            plan.discard_incomplete_v1(
                self.lexical_incomplete_discard.as_ref(),
                self.semantic_incomplete_discard.as_ref(),
            )?;
        }

        let build_lexical = sealed_plan
            .as_ref()
            .is_none_or(SealedGenerationBuildPlanV1::build_lexical);
        let build_semantic = sealed_plan
            .as_ref()
            .is_none_or(SealedGenerationBuildPlanV1::build_semantic);
        let derived_semantic_batch = if build_semantic {
            Some(derive_semantic_batch_with_mode_v1(
                batch,
                self.semantic_embedder.as_ref(),
                self.semantic_derivation_mode,
            )?)
        } else {
            None
        };
        if build_lexical {
            self.builder.build_batch(batch)?;
        }
        // Search-owned semantic derivation is mandatory follow-on work from
        // every accepted search-corpus batch. Failure is surfaced to the caller;
        // there is no lexical-only downgrade path.
        if let Some(derived_semantic_batch) = derived_semantic_batch {
            let semantic_receipt = self
                .semantic_ingest
                .publish_batch(&derived_semantic_batch)?;
            validate_semantic_publish_receipt_v1(&derived_semantic_batch, &semantic_receipt)?;
        }
        if batch.seal {
            let (lexical, semantic) = generation_pair_from_batch_v1(batch);
            validate_physical_generation_v1(
                self.lexical_generation_validator.as_ref(),
                &lexical,
                "lexical post-build",
            )?;
            validate_physical_generation_v1(
                self.semantic_generation_validator.as_ref(),
                &semantic,
                "semantic post-build",
            )?;
            self.finalize_sealed_generation_v1(batch)?;
        } else {
            self.finalize_generation_v1(batch, None)?;
        }
        Ok(batch_publish_receipt_v1(batch))
    }
}

impl DirectSearchCorpusMaterializer {
    /// A delta may only build on a base that both tracks hold as the exact
    /// sealed identity the ledger recorded (QI-BB-029).
    ///
    /// Runs before any adapter mutates, so an absent, unsealed or mismatched
    /// base refuses the batch with zero bytes changed instead of letting the
    /// lexical track clone and seal on it and the semantic track refuse it
    /// afterwards. The digest comes from the ledger's sealed-track record;
    /// a base the ledger never sealed is refused outright rather than
    /// trusted because a directory happens to exist.
    fn preflight_delta_base_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
        base_generation: ManifestGeneration,
    ) -> Result<(), CoreError> {
        let tracks = [
            (
                SearchPlaneTrackKind::Lexical,
                &self.lexical_generation_validator,
                "lexical",
            ),
            (
                SearchPlaneTrackKind::Semantic,
                &self.semantic_generation_validator,
                "semantic",
            ),
        ];
        for (track, validator, label) in tracks {
            // Read the ledger only long enough to copy the digest out; the
            // physical validation below reads files and must not hold it.
            let recorded = self
                .ledger
                .read()
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "direct search-corpus materialize: ledger poisoned while preflighting delta base: {err}"
                    ))
                })?
                .sealed_track_identity_digest(
                    &batch.repo_id,
                    &batch.revision_id,
                    track,
                    base_generation,
                );
            let Some(digest) = recorded else {
                return Err(CoreError::Typed {
                    code: ERR_SEARCH_CORPUS_DELTA_BASE_NOT_SEALED.to_string(),
                    message: format!(
                        "direct search-corpus materialize: delta base generation {} is not a sealed {label} track for repo={} revision={}; refusing before any mutation",
                        base_generation.get(),
                        batch.repo_id.as_str(),
                        batch.revision_id.as_str(),
                    ),
                });
            };
            let base = GenerationSnapshot {
                repo_id: batch.repo_id.clone(),
                revision_id: batch.revision_id.clone(),
                track,
                manifest_generation: base_generation,
                manifest_digest: digest,
            };
            validate_physical_generation_v1(
                validator.as_ref(),
                &base,
                &format!("{label} delta base"),
            )?;
        }
        Ok(())
    }

    /// Computes the convergent per-track recovery plan before any mutation.
    fn preflight_sealed_generation_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<SealedGenerationBuildPlanV1, CoreError> {
        let authority = self.authority.inspect_sealed_search_corpus(
            &batch.repo_id,
            &batch.revision_id,
            batch.generation,
            batch.manifest_digest.as_str(),
        )?;
        let (lexical, semantic) = generation_pair_from_batch_v1(batch);
        let lexical_state = inspect_physical_generation_v1(
            self.lexical_generation_validator.as_ref(),
            &lexical,
            "lexical preflight",
        )?;
        let semantic_state = inspect_physical_generation_v1(
            self.semantic_generation_validator.as_ref(),
            &semantic,
            "semantic preflight",
        )?;

        match (authority, lexical_state, semantic_state) {
            (
                SealedSearchCorpusAuthorityStateV1::Exact,
                PhysicalGenerationStateV1::Exact,
                PhysicalGenerationStateV1::Exact,
            ) => Ok(SealedGenerationBuildPlanV1::finalize_only(
                lexical, semantic,
            )),
            (SealedSearchCorpusAuthorityStateV1::Absent, lexical_state, semantic_state) => {
                Ok(SealedGenerationBuildPlanV1 {
                    lexical,
                    semantic,
                    lexical_state,
                    semantic_state,
                })
            }
            (authority, lexical_state, semantic_state) => Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_GENERATION_CONFLICT.to_string(),
                message: format!(
                    "direct search-corpus materialize: non-atomic sealed-generation state for repo={} revision={} generation={}: authority={authority:?} lexical={lexical_state:?} semantic={semantic_state:?}",
                    batch.repo_id.as_str(),
                    batch.revision_id.as_str(),
                    batch.generation.get(),
                ),
            }),
        }
    }

    fn finalize_sealed_generation_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<(), CoreError> {
        // Fence rollback before entering the durable retention owner. A
        // delete may succeed while the following directory fsync fails; in
        // that state the old same-process ledger is not authoritative. Only a
        // reconciled retained-set receipt clears this pair-local fence.
        self.ledger
            .write()
            .map_err(|err| {
                CoreError::Storage(format!(
                    "direct search-corpus materialize: ledger poisoned while fencing retention mutation: {err}"
                ))
            })?
            .fence_search_corpus_history_v1(&batch.repo_id, &batch.revision_id);
        let retention = self.authority.record_sealed_search_corpus(
            &batch.repo_id,
            &batch.revision_id,
            batch.generation,
            batch.manifest_digest.as_str(),
        );
        let retention = match retention {
            Ok(receipt) => receipt,
            Err(error @ CoreError::Storage(_)) => return Err(error),
            Err(error) => {
                // Typed/contract rejections are pre-mutation outcomes of this
                // port and therefore do not create durability ambiguity.
                self.ledger
                    .write()
                    .map_err(|err| {
                        CoreError::Storage(format!(
                            "direct search-corpus materialize: ledger poisoned while clearing rejected retention mutation fence: {err}; original error: {error}"
                        ))
                    })?
                    .clear_search_corpus_history_fence_v1(
                        &batch.repo_id,
                        &batch.revision_id,
                    );
                return Err(error);
            }
        };
        self.finalize_generation_v1(batch, Some(&retention))
    }

    pub(super) fn finalize_generation_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
        retention: Option<&SearchCorpusHistoryRetentionReceiptV1>,
    ) -> Result<(), CoreError> {
        const WHAT: &str = "direct search-corpus materialize";
        // The chunk universe is durable before the generation is visible
        // (QI-BB-020): validated against the ledger, written to the catalog,
        // then applied under the write lock with the track bookkeeping.
        let _serial = self.auxiliary_coordinator.lock()?;
        let chunks = {
            let guard = self.ledger.read().map_err(|err| {
                CoreError::Storage(format!(
                    "{WHAT}: ledger poisoned while finalizing generation: {err}"
                ))
            })?;
            let epoch = guard.structural_next_epoch(
                &batch.repo_id,
                &batch.revision_id,
                batch.generation,
            )?;
            structural_chunks_transition(
                guard.structural_state(&batch.repo_id, &batch.revision_id, batch.generation),
                epoch,
                batch,
            )
        };
        let _durable = self
            .auxiliary_catalog
            .apply(&structural_chunks_delta_rows(&chunks)?)?;
        let reaped_auxiliary = {
            let mut guard = self.ledger.write().map_err(|err| {
                CoreError::Storage(format!(
                    "{WHAT}: ledger poisoned while finalizing generation: {err}"
                ))
            })?;
            if let Some(retention) = retention {
                guard.apply_search_corpus_history_retention_receipt_v1(
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    retention,
                )?;
            } else if batch.seal {
                return Err(CoreError::InvalidContract(
                    "direct search-corpus materialize: sealed generation requires durable retention receipt"
                        .to_string(),
                ));
            }
            guard.apply_structural_chunks_delta(&chunks, std::time::Instant::now())?;
            guard.materialize_track(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Lexical,
                batch.generation,
                Some(batch.manifest_digest.as_str()),
            );
            if batch.seal {
                guard.seal_track_with_digest(
                    &batch.repo_id,
                    &batch.revision_id,
                    SearchPlaneTrackKind::Lexical,
                    batch.generation,
                    batch.manifest_digest.as_str(),
                );
                guard.record_historically_sealed_search_corpus(
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    batch.manifest_digest.as_str(),
                );
            }
            // Auxiliary generations older than the one just sealed that the
            // retention receipt does not retain go with it; a newer
            // generation still being staged is never touched.
            let reaped: Vec<ManifestGeneration> = retention.map_or_else(Vec::new, |retention| {
                guard
                    .auxiliary_generations_older_than(
                        &batch.repo_id,
                        &batch.revision_id,
                        batch.generation,
                    )
                    .into_iter()
                    .filter(|generation| !retention.retains(*generation))
                    .collect()
            });
            for generation in &reaped {
                guard.forget_auxiliary_generation(&batch.repo_id, &batch.revision_id, *generation);
            }
            reaped
        };
        for generation in reaped_auxiliary {
            let _removed = self.auxiliary_catalog.forget_generation(
                &batch.repo_id,
                &batch.revision_id,
                generation,
            )?;
        }
        if let Some(retention) = retention {
            let _receipt = self.reclaim_retired_generations_v1(batch, retention)?;
        }
        Ok(())
    }

    /// Physical GC for the pair the batch just sealed (QI-BB-003).
    ///
    /// Runs only after the durable authority has been reaped and the ledger
    /// reconciled from the receipt, so no query can resolve or pin a retired
    /// generation any more. For each track, every sealed generation on disk
    /// that the receipt does not retain and that is older than the one being
    /// sealed is an orphan of this or an earlier retention pass; it is fenced
    /// out of the snapshot registry first, and reclaimed only if nothing
    /// still holds its handle. A pinned generation is deferred, not deleted
    /// under a reader; the next pass will find it again. Sweeping from the
    /// filesystem rather than from the receipt's reaped set is what makes a
    /// crash between reap and reclaim recoverable.
    pub(super) fn reclaim_retired_generations_v1(
        &self,
        batch: &SearchCorpusIngestBatch,
        retention: &SearchCorpusHistoryRetentionReceiptV1,
    ) -> Result<SearchCorpusPhysicalReclaimReceiptV1, CoreError> {
        let mut receipt = SearchCorpusPhysicalReclaimReceiptV1::default();
        let tracks: [(
            &Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
            SearchPlaneTrackKind,
        ); 2] = [
            (&self.lexical_reclaim, SearchPlaneTrackKind::Lexical),
            (&self.semantic_reclaim, SearchPlaneTrackKind::Semantic),
        ];
        for (port, track) in tracks {
            for retired in port.sealed_generations_for_pair(&batch.repo_id, &batch.revision_id)? {
                let generation = retired.manifest_generation;
                if retention.retains(generation) || generation >= batch.generation {
                    continue;
                }
                let key = SnapshotKey::new(&batch.repo_id, &batch.revision_id, generation);
                let fence = match track {
                    SearchPlaneTrackKind::Lexical => self.snapshots.lexical.retire(&key)?,
                    SearchPlaneTrackKind::Semantic => self.snapshots.semantic.retire(&key)?,
                    SearchPlaneTrackKind::Structural => {
                        return Err(CoreError::InvalidContract(
                            "search-corpus physical reclaim: structural is not a search-corpus track"
                                .to_string(),
                        ));
                    }
                };
                if let SnapshotRetireOutcome::StillReferenced { holders } = fence {
                    let _deferred = receipt.deferred_pinned.insert((track, generation, holders));
                    continue;
                }
                match port.reclaim_sealed_generation(&retired)? {
                    SealedGenerationReclaimOutcomeV1::Absent => {}
                    SealedGenerationReclaimOutcomeV1::Reclaimed { bytes } => {
                        let _prior = receipt.reclaimed.insert((track, generation), bytes);
                    }
                }
            }
        }
        // A generation reclaimed on both tracks describes nothing a replay
        // could still converge on; its idempotency records go with it.
        let mut forgotten = BTreeSet::new();
        for (track, generation) in receipt.reclaimed.keys() {
            let other = match track {
                SearchPlaneTrackKind::Lexical => SearchPlaneTrackKind::Semantic,
                SearchPlaneTrackKind::Semantic => SearchPlaneTrackKind::Lexical,
                SearchPlaneTrackKind::Structural => continue,
            };
            if receipt.reclaimed.contains_key(&(other, *generation)) {
                let _new = forgotten.insert(*generation);
            }
        }
        for generation in forgotten {
            let _records = self.idempotency.forget_generation(
                &batch.repo_id,
                &batch.revision_id,
                generation,
            )?;
        }
        Ok(receipt)
    }
}
