//! Search-plane ingest orchestration (QI-RT-01).
//!
//! Producer sends a typed [`SearchPlaneIngestIpcRequest`] over UDS
//! `ingest.sock`. This dispatcher routes the typed batch to owner materializer
//! ports. The concrete runtime may choose to mirror accepted batches into
//! legacy channel persistence, but channel row-op fanout is no longer the
//! public ingest truth.
//!
//! Composition root in `quanta-index-searchd` is the only place that names
//! concrete adapter types (materializers, channel mirrors, repo-map ingest);
//! this module holds only [`Arc<dyn ...Port>`] (CLAUDE.md DIP rule).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, RwLock};

use crate::auxiliary_authority::{
    history_delta_rows, history_transition, runtime_catalog_delta_rows, runtime_catalog_transition,
    runtime_dirty_delta_rows, runtime_dirty_transition, structural_chunks_delta_rows,
    structural_chunks_transition, structural_delta_rows, structural_transition,
};
use crate::readiness::{
    SEARCH_CORPUS_LOCK_STRIPES_V1, SearchCorpusHistoryRetentionReceiptV1,
    search_corpus_lock_stripe_v1,
};
use crate::semantic_derive::{
    DEFAULT_SEMANTIC_DERIVATION_MODE_V1, SemanticDerivationModeV1,
    derive_semantic_batch_with_mode_v1, semantic_derivation_mode_from_env_v1,
};
use crate::{
    AuxiliaryAuthorityStore, Ledger, SealedSearchCorpusAuthorityStateV1, SnapshotKey,
    SnapshotRegistries, SnapshotRetireOutcome,
};
use quanta_index_contract::{
    BatchPublishReceipt, DirtyIngestBatch, DirtyMutation, GenerationSnapshot, HistoryIngestBatch,
    HistoryRefMutation, ManifestGeneration, RepoId, RepoMapMutationAck, RevisionId,
    RuntimeCatalogIngestBatch, SearchCorpusIngestBatch, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneIpcError, SearchPlaneTrackKind, SemanticIngestBatch,
    StructuralIngestBatch,
};
use quanta_index_core::{
    AuxiliaryAuthorityCatalogPort, CoreError, FileContributorIngestPort, FileOwnershipIngestPort,
    GenerationIdentityValidatePort, IdempotencyBeginV1, IdempotencyCatalogPort, IdempotencyKeyV1,
    IncompleteGenerationDiscardOutcomeV1, IncompleteGenerationDiscardPort, IngestBatchFootprint,
    IngestOperationKindV1, IngestResourcePolicy, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMapBundleIngestPort, RepoMetaIngestPort, RepoTopicIngestPort,
    RequestBudgetV1, SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort,
    SearchCorpusBatchBuildPort, SearchCorpusIngestPort, SemanticBatchBuildPort, SemanticIngestPort,
    TextEmbeddingProvider,
};

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";
const ERR_SEARCH_CORPUS_GENERATION_CONFLICT: &str = "SEARCH_CORPUS_GENERATION_CONFLICT";
/// The batch's own shape is invalid; nothing was mutated (QI-BB-029).
const ERR_SEARCH_CORPUS_BATCH_SHAPE: &str = "SEARCH_CORPUS_BATCH_SHAPE_INVALID";
/// A delta names a base the ledger never sealed; nothing was mutated.
const ERR_SEARCH_CORPUS_DELTA_BASE_NOT_SEALED: &str = "SEARCH_CORPUS_DELTA_BASE_NOT_SEALED";

pub trait HistoryIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError>;
}

pub trait RuntimeMetadataIngestPort: Send + Sync {
    fn publish_batch(&self, batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError>;

    fn publish_catalog_batch(
        &self,
        batch: &RuntimeCatalogIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

pub trait StructuralIngestPort: Send + Sync {
    fn publish_batch(
        &self,
        batch: &StructuralIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError>;
}

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

/// Durable owner for complete lexical+semantic rollback history.
///
/// The port is intentionally composite. Per-track materializers cannot mint a
/// rollback target independently.
pub trait SearchCorpusAuthorityWritePort: Send + Sync {
    fn inspect_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError>;

    /// Persist and reconcile the complete retained set.
    ///
    /// Contract: typed/contract errors reject before durable mutation.
    /// `CoreError::Storage` may describe a post-mutation durability ambiguity,
    /// so callers must keep rollback fenced until a later receipt succeeds.
    fn record_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError>;
}

impl SearchCorpusAuthorityWritePort for AuxiliaryAuthorityStore {
    fn inspect_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError> {
        Self::inspect_sealed_search_corpus(self, repo_id, revision_id, generation, manifest_digest)
    }

    fn record_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError> {
        Self::record_sealed_search_corpus(self, repo_id, revision_id, generation, manifest_digest)
    }
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

    fn finalize_generation_v1(
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
            structural_chunks_transition(
                guard.structural_state(&batch.repo_id, &batch.revision_id, batch.generation),
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
            guard.apply_structural_chunks_delta(&chunks);
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
    fn reclaim_retired_generations_v1(
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

/// What one physical reclaim pass did, per track and generation.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct SearchCorpusPhysicalReclaimReceiptV1 {
    /// Bytes given back per reclaimed generation.
    pub(crate) reclaimed: BTreeMap<(SearchPlaneTrackKind, ManifestGeneration), u64>,
    /// Generations left on disk because a resident handle still had holders.
    pub(crate) deferred_pinned: BTreeSet<(SearchPlaneTrackKind, ManifestGeneration, usize)>,
}

fn generation_pair_from_batch_v1(
    batch: &SearchCorpusIngestBatch,
) -> (GenerationSnapshot, GenerationSnapshot) {
    let snapshot = |track| GenerationSnapshot {
        repo_id: batch.repo_id.clone(),
        revision_id: batch.revision_id.clone(),
        track,
        manifest_generation: batch.generation,
        manifest_digest: batch.manifest_digest.clone(),
    };
    (
        snapshot(SearchPlaneTrackKind::Lexical),
        snapshot(SearchPlaneTrackKind::Semantic),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PhysicalGenerationStateV1 {
    Absent,
    InProgress,
    Exact,
}

#[derive(Clone, Debug)]
struct SealedGenerationBuildPlanV1 {
    lexical: GenerationSnapshot,
    semantic: GenerationSnapshot,
    lexical_state: PhysicalGenerationStateV1,
    semantic_state: PhysicalGenerationStateV1,
}

impl SealedGenerationBuildPlanV1 {
    fn finalize_only(lexical: GenerationSnapshot, semantic: GenerationSnapshot) -> Self {
        Self {
            lexical,
            semantic,
            lexical_state: PhysicalGenerationStateV1::Exact,
            semantic_state: PhysicalGenerationStateV1::Exact,
        }
    }

    const fn is_finalize_only(&self) -> bool {
        matches!(self.lexical_state, PhysicalGenerationStateV1::Exact)
            && matches!(self.semantic_state, PhysicalGenerationStateV1::Exact)
    }

    const fn build_lexical(&self) -> bool {
        !matches!(self.lexical_state, PhysicalGenerationStateV1::Exact)
    }

    const fn build_semantic(&self) -> bool {
        !matches!(self.semantic_state, PhysicalGenerationStateV1::Exact)
    }

    fn discard_incomplete_v1(
        &self,
        lexical_discard: &dyn IncompleteGenerationDiscardPort,
        semantic_discard: &dyn IncompleteGenerationDiscardPort,
    ) -> Result<(), CoreError> {
        // Both tracks being in progress is the ordinary path after accepted
        // non-seal ingests. Discarding either side here would turn a normal
        // seal into an empty rebuild. A discard is only safe for the
        // asymmetric recovery case: the peer track is already sealed and the
        // incomplete track can only be stale crash residue.
        if matches!(self.lexical_state, PhysicalGenerationStateV1::InProgress)
            && matches!(self.semantic_state, PhysicalGenerationStateV1::Exact)
        {
            match lexical_discard.discard_incomplete_generation(&self.lexical)? {
                IncompleteGenerationDiscardOutcomeV1::Absent
                | IncompleteGenerationDiscardOutcomeV1::Discarded => {}
            }
        }
        if matches!(self.semantic_state, PhysicalGenerationStateV1::InProgress)
            && matches!(self.lexical_state, PhysicalGenerationStateV1::Exact)
        {
            match semantic_discard.discard_incomplete_generation(&self.semantic)? {
                IncompleteGenerationDiscardOutcomeV1::Absent
                | IncompleteGenerationDiscardOutcomeV1::Discarded => {}
            }
        }
        Ok(())
    }
}

fn inspect_physical_generation_v1(
    validator: &dyn GenerationIdentityValidatePort,
    candidate: &GenerationSnapshot,
    label: &str,
) -> Result<PhysicalGenerationStateV1, CoreError> {
    match validator.validate_generation_identity(candidate) {
        Ok(()) => Ok(PhysicalGenerationStateV1::Exact),
        Err(CoreError::NotFound(_)) => Ok(PhysicalGenerationStateV1::Absent),
        Err(CoreError::Typed { code, .. }) if code == "GENERATION_IDENTITY_INCOMPLETE" => {
            Ok(PhysicalGenerationStateV1::InProgress)
        }
        Err(source) => Err(CoreError::Typed {
            code: ERR_SEARCH_CORPUS_GENERATION_CONFLICT.to_string(),
            message: format!(
                "direct search-corpus materialize: {label} generation is present but invalid for repo={} revision={} generation={}: {source:?}",
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
                candidate.manifest_generation.get(),
            ),
        }),
    }
}

fn ensure_generation_is_mutable_v1(
    validator: &dyn GenerationIdentityValidatePort,
    candidate: &GenerationSnapshot,
    label: &str,
) -> Result<(), CoreError> {
    match validator.validate_generation_identity(candidate) {
        Err(CoreError::NotFound(_)) => Ok(()),
        Err(CoreError::Typed { code, .. }) if code == "GENERATION_IDENTITY_INCOMPLETE" => Ok(()),
        Ok(()) => Err(CoreError::Typed {
            code: "GENERATION_IMMUTABLE".to_string(),
            message: format!(
                "direct search-corpus materialize: {label} generation is already sealed; refusing non-seal mutation for repo={} revision={} generation={}",
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
                candidate.manifest_generation.get(),
            ),
        }),
        Err(source) => Err(CoreError::Typed {
            code: ERR_SEARCH_CORPUS_GENERATION_CONFLICT.to_string(),
            message: format!(
                "direct search-corpus materialize: {label} generation mutability is ambiguous for repo={} revision={} generation={}: {source:?}",
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
                candidate.manifest_generation.get(),
            ),
        }),
    }
}

fn validate_physical_generation_v1(
    validator: &dyn GenerationIdentityValidatePort,
    candidate: &GenerationSnapshot,
    label: &str,
) -> Result<(), CoreError> {
    validator
        .validate_generation_identity(candidate)
        .map_err(|source| CoreError::Typed {
            code: ERR_SEARCH_CORPUS_GENERATION_CONFLICT.to_string(),
            message: format!(
                "direct search-corpus materialize: {label} generation failed exact validation for repo={} revision={} generation={}: {source:?}",
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
                candidate.manifest_generation.get(),
            ),
        })
}

fn batch_publish_receipt_v1(batch: &SearchCorpusIngestBatch) -> BatchPublishReceipt {
    let mut receipt = BatchPublishReceipt::empty_for(
        batch.generation,
        Some(batch.manifest_digest.clone()),
        batch.batch_digest.clone(),
    );
    for _scope in &batch.replace_scopes {
        receipt.accept_replace_scope();
    }
    for _scope in &batch.tombstone_scopes {
        receipt.accept_tombstone_scope();
    }
    for _surface in &batch.clear_surfaces {
        receipt.accept_clear_surface();
    }
    if batch.seal {
        receipt.mark_sealed();
    }
    receipt
}

fn validate_semantic_publish_receipt_v1(
    batch: &SemanticIngestBatch,
    receipt: &BatchPublishReceipt,
) -> Result<(), CoreError> {
    let expected_replace = u32::try_from(batch.replace_scopes.len()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "direct search-corpus materialize: semantic replace scope count overflow: {err}"
        ))
    })?;
    let expected_tombstone = u32::try_from(batch.tombstone_scopes.len()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "direct search-corpus materialize: semantic tombstone scope count overflow: {err}"
        ))
    })?;
    let expected_clear = u32::try_from(batch.clear_surfaces.len()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "direct search-corpus materialize: semantic clear surface count overflow: {err}"
        ))
    })?;
    if receipt.generation != batch.generation
        || receipt.manifest_digest.as_deref() != Some(batch.manifest_digest.as_str())
        || receipt.batch_digest != batch.batch_digest
        || receipt.accepted_replace_scopes != expected_replace
        || receipt.accepted_tombstone_scopes != expected_tombstone
        || receipt.accepted_clear_surfaces != expected_clear
        || receipt.sealed != batch.seal
    {
        return Err(CoreError::InvalidContract(format!(
            "direct search-corpus materialize: semantic receipt does not exactly acknowledge the derived batch: receipt={receipt:?}"
        )));
    }
    Ok(())
}

/// Direct semantic batch materializer.
///
/// Writes the durable, generation-scoped semantic adapter first (rows on every
/// batch; graph + manifest + seal on `seal`), then updates the readiness
/// ledger. Durability lives entirely in the adapter's generation directories;
/// there is no journal write here. A failed durable write leaves no SEALED
/// marker and does not touch the ledger, so readiness cannot go falsely ready.
pub struct DirectSemanticMaterializer {
    builder: Arc<dyn SemanticBatchBuildPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectSemanticMaterializer {
    #[must_use]
    pub fn new(
        builder: Arc<dyn SemanticBatchBuildPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self { builder, ledger }
    }
}

impl SemanticIngestPort for DirectSemanticMaterializer {
    fn publish_batch(&self, batch: &SemanticIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        self.builder.build_batch(batch)?;
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct semantic materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.materialize_track(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Semantic,
            batch.generation,
            Some(batch.manifest_digest.as_str()),
        );
        if batch.seal {
            guard.seal_track_with_digest(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Semantic,
                batch.generation,
                batch.manifest_digest.as_str(),
            );
        }
        drop(guard);
        let mut receipt = BatchPublishReceipt::empty_for(
            batch.generation,
            Some(batch.manifest_digest.clone()),
            batch.batch_digest.clone(),
        );
        for _scope in &batch.replace_scopes {
            receipt.accept_replace_scope();
        }
        for _scope in &batch.tombstone_scopes {
            receipt.accept_tombstone_scope();
        }
        for _surface in &batch.clear_surfaces {
            receipt.accept_clear_surface();
        }
        if batch.seal {
            receipt.mark_sealed();
        }
        Ok(receipt)
    }
}

/// Serializes the validate → persist → apply protocol of every auxiliary
/// mutation (QI-BB-020).
///
/// A transition is validated against the ledger under its read lock,
/// made durable in the catalog, then applied under the write lock. Two
/// mutations interleaving between those steps could validate against a
/// state the other is about to change, so every auxiliary materializer —
/// and the search-corpus path that owns the chunk universe — takes this
/// lock for the whole protocol. Queries never take it: they clone a
/// snapshot under the ledger's read lock and scan outside it.
#[derive(Debug, Default)]
pub struct AuxiliaryMutationCoordinator {
    serial: Mutex<()>,
}

impl AuxiliaryMutationCoordinator {
    #[must_use]
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>, CoreError> {
        self.serial.lock().map_err(|err| {
            CoreError::Storage(format!(
                "auxiliary materialize: mutation coordinator poisoned: {err}"
            ))
        })
    }
}

/// The catalog and the ledger every auxiliary materializer writes through.
#[derive(Clone)]
pub struct AuxiliaryMaterializerParts {
    pub catalog: Arc<dyn AuxiliaryAuthorityCatalogPort + Send + Sync>,
    pub coordinator: Arc<AuxiliaryMutationCoordinator>,
    pub ledger: Arc<RwLock<Ledger>>,
}

impl AuxiliaryMaterializerParts {
    fn read_ledger(&self, what: &str) -> Result<std::sync::RwLockReadGuard<'_, Ledger>, CoreError> {
        self.ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("{what}: ledger poisoned: {err}")))
    }

    fn write_ledger(
        &self,
        what: &str,
    ) -> Result<std::sync::RwLockWriteGuard<'_, Ledger>, CoreError> {
        self.ledger
            .write()
            .map_err(|err| CoreError::Storage(format!("{what}: ledger poisoned: {err}")))
    }
}

/// Direct history materializer. History is auxiliary and non-activation
/// blocking; its rows are durable before its receipt and visible only
/// after (QI-BB-020).
pub struct DirectHistoryMaterializer {
    parts: AuxiliaryMaterializerParts,
}

impl DirectHistoryMaterializer {
    #[must_use]
    pub const fn new(parts: AuxiliaryMaterializerParts) -> Self {
        Self { parts }
    }
}

impl HistoryIngestPort for DirectHistoryMaterializer {
    fn publish_batch(&self, batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        const WHAT: &str = "direct history materialize";
        let _serial = self.parts.coordinator.lock()?;
        let delta = {
            let guard = self.parts.read_ledger(WHAT)?;
            history_transition(
                guard.history_state(&batch.repo_id, &batch.revision_id, batch.generation),
                batch,
            )?
        };
        let _durable = self.parts.catalog.apply(&history_delta_rows(&delta)?)?;
        {
            let mut guard = self.parts.write_ledger(WHAT)?;
            guard.apply_history_delta(&delta);
        }
        let mut receipt = BatchPublishReceipt::empty_for(
            batch.generation,
            batch.manifest_digest.clone(),
            batch.batch_digest.clone(),
        );
        for _record in &batch.commits {
            receipt.accept_replace_scope();
        }
        for mutation in batch.refs.iter().chain(batch.tags.iter()) {
            match mutation {
                HistoryRefMutation::Upsert(_) => receipt.accept_replace_scope(),
                HistoryRefMutation::Delete(_) => receipt.accept_tombstone_scope(),
            }
        }
        for _record in &batch.diff_hunks {
            receipt.accept_replace_scope();
        }
        Ok(receipt)
    }
}

/// Direct dirty-overlay and runtime catalog materializer. Runtime state
/// remains auxiliary and non-activation-blocking; its rows are durable
/// before its receipt and visible only after (QI-BB-020).
pub struct DirectRuntimeMetadataMaterializer {
    parts: AuxiliaryMaterializerParts,
}

impl DirectRuntimeMetadataMaterializer {
    #[must_use]
    pub const fn new(parts: AuxiliaryMaterializerParts) -> Self {
        Self { parts }
    }
}

fn dirty_publish_receipt_v1(batch: &DirtyIngestBatch) -> BatchPublishReceipt {
    let mut receipt =
        BatchPublishReceipt::empty_for(batch.generation, None, batch.batch_digest.clone());
    for entry in &batch.entries {
        match entry {
            DirtyMutation::Upsert(_) => receipt.accept_replace_scope(),
            DirtyMutation::Delete(_) => receipt.accept_tombstone_scope(),
        }
    }
    receipt
}

impl RuntimeMetadataIngestPort for DirectRuntimeMetadataMaterializer {
    fn publish_batch(&self, batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        const WHAT: &str = "direct dirty materialize";
        let _serial = self.parts.coordinator.lock()?;
        let delta = {
            let guard = self.parts.read_ledger(WHAT)?;
            runtime_dirty_transition(
                guard.runtime_state(&batch.repo_id, &batch.revision_id, batch.generation),
                batch,
            )
        };
        let _durable = self
            .parts
            .catalog
            .apply(&runtime_dirty_delta_rows(&delta)?)?;
        {
            let mut guard = self.parts.write_ledger(WHAT)?;
            guard.apply_runtime_dirty_delta(&delta);
        }
        Ok(dirty_publish_receipt_v1(batch))
    }

    fn publish_catalog_batch(
        &self,
        batch: &RuntimeCatalogIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        const WHAT: &str = "direct runtime catalog materialize";
        let _serial = self.parts.coordinator.lock()?;
        let delta = {
            let guard = self.parts.read_ledger(WHAT)?;
            runtime_catalog_transition(
                guard.structural_state(&batch.repo_id, &batch.revision_id, batch.generation),
                guard.runtime_state(&batch.repo_id, &batch.revision_id, batch.generation),
                batch,
            )?
        };
        let _durable = self
            .parts
            .catalog
            .apply(&runtime_catalog_delta_rows(&delta)?)?;
        {
            let mut guard = self.parts.write_ledger(WHAT)?;
            guard.apply_runtime_catalog_delta(&delta);
        }
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, None, batch.batch_digest.clone());
        let accepted = [
            batch.changed_entries.len(),
            batch.facet_entries.len(),
            batch.snapshot_entries.len(),
            batch.affected_entries.len(),
            batch.invalidated_by_entries.len(),
        ]
        .into_iter()
        .fold(0_usize, usize::saturating_add);
        for _record in 0..accepted {
            receipt.accept_replace_scope();
        }
        Ok(receipt)
    }
}

/// Direct structural materializer. Structural readiness is first-class;
/// parse trees and the structural track's state are durable before the
/// receipt and visible only after (QI-BB-020).
pub struct DirectStructuralMaterializer {
    parts: AuxiliaryMaterializerParts,
}

impl DirectStructuralMaterializer {
    #[must_use]
    pub const fn new(parts: AuxiliaryMaterializerParts) -> Self {
        Self { parts }
    }
}

impl StructuralIngestPort for DirectStructuralMaterializer {
    fn publish_batch(
        &self,
        batch: &StructuralIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        const WHAT: &str = "direct structural materialize";
        let _serial = self.parts.coordinator.lock()?;
        let delta = {
            let guard = self.parts.read_ledger(WHAT)?;
            structural_transition(
                guard.structural_state(&batch.repo_id, &batch.revision_id, batch.generation),
                guard.track_state(
                    &batch.repo_id,
                    &batch.revision_id,
                    SearchPlaneTrackKind::Structural,
                ),
                batch,
            )?
        };
        let _durable = self.parts.catalog.apply(&structural_delta_rows(&delta)?)?;
        {
            let mut guard = self.parts.write_ledger(WHAT)?;
            guard.apply_structural_trees_delta(&delta);
        }
        let mut receipt = BatchPublishReceipt::empty_for(
            batch.generation,
            Some(batch.manifest_digest.clone()),
            batch.batch_digest.clone(),
        );
        for _scope in &batch.replace_scopes {
            receipt.accept_replace_scope();
        }
        for _scope in &batch.tombstone_scopes {
            receipt.accept_tombstone_scope();
        }
        if batch.seal {
            receipt.mark_sealed();
        }
        Ok(receipt)
    }
}

// =============================================================================
// Top-level dispatcher
// =============================================================================

/// Routes typed ingest requests to the appropriate domain port. Mirrors the
/// shape of [`crate::SearchPlaneControlDispatcher`] / [`crate::SearchPlaneDispatcher`]
/// for the new ingest surface (QI-RT-01).
pub struct SearchPlaneIngestDispatcher {
    lexical: Arc<dyn SearchCorpusIngestPort + Send + Sync>,
    history: Arc<dyn HistoryIngestPort + Send + Sync>,
    repo_commit_recency: Arc<dyn RepoCommitRecencyIngestPort + Send + Sync>,
    repo_topic: Arc<dyn RepoTopicIngestPort + Send + Sync>,
    repo_description: Arc<dyn RepoDescriptionIngestPort + Send + Sync>,
    file_ownership: Arc<dyn FileOwnershipIngestPort + Send + Sync>,
    file_contributor: Arc<dyn FileContributorIngestPort + Send + Sync>,
    repo_meta: Arc<dyn RepoMetaIngestPort + Send + Sync>,
    runtime: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
    structural: Arc<dyn StructuralIngestPort + Send + Sync>,
    repomap: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    /// Resident opened generations shared with the query side. Every routed
    /// mutation that names a generation drops that generation's residency
    /// after it lands, so a handle opened before the mutation is never
    /// served after it (QI-BB-001; sealed-corpus overlay is W2's job).
    snapshots: SnapshotRegistries,
    /// Durable idempotency records (QI-BB-032): every receipt-bearing route
    /// goes through intent → apply → finalize, so a replay of the same body
    /// is answered from the record and a different body under the same key
    /// is refused before any mutation.
    idempotency: Arc<dyn IdempotencyCatalogPort + Send + Sync>,
}

impl SearchPlaneIngestDispatcher {
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "composition-root wiring of one Arc<dyn ...Port> per ingest authority; bundling into a struct is a separate refactor"
    )]
    pub fn new(
        lexical: Arc<dyn SearchCorpusIngestPort + Send + Sync>,
        history: Arc<dyn HistoryIngestPort + Send + Sync>,
        repo_commit_recency: Arc<dyn RepoCommitRecencyIngestPort + Send + Sync>,
        repo_topic: Arc<dyn RepoTopicIngestPort + Send + Sync>,
        repo_description: Arc<dyn RepoDescriptionIngestPort + Send + Sync>,
        file_ownership: Arc<dyn FileOwnershipIngestPort + Send + Sync>,
        file_contributor: Arc<dyn FileContributorIngestPort + Send + Sync>,
        repo_meta: Arc<dyn RepoMetaIngestPort + Send + Sync>,
        runtime: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
        structural: Arc<dyn StructuralIngestPort + Send + Sync>,
        repomap: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
        snapshots: SnapshotRegistries,
        idempotency: Arc<dyn IdempotencyCatalogPort + Send + Sync>,
    ) -> Self {
        Self {
            lexical,
            history,
            repo_commit_recency,
            repo_topic,
            repo_description,
            file_ownership,
            file_contributor,
            repo_meta,
            runtime,
            structural,
            repomap,
            snapshots,
            idempotency,
        }
    }

    /// Run one receipt-bearing publish under its idempotency record
    /// (QI-BB-032).
    ///
    /// The canonical body is hashed, the record is begun, and then: a
    /// finalized record with the same body answers with the recorded receipt
    /// marked `applied = false` and nothing runs; a record with a different
    /// body is refused typed by the catalog; otherwise `apply` runs and the
    /// receipt it produces is recorded, stamped with the catalog's durable
    /// sequence, and returned as `applied = true`. A record left in progress
    /// by a crash re-runs `apply`, which every route makes idempotent.
    fn publish_idempotent<B: serde::Serialize>(
        &self,
        kind: IngestOperationKindV1,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        batch_digest: &str,
        body: &B,
        apply: impl FnOnce() -> Result<BatchPublishReceipt, CoreError>,
    ) -> Result<BatchPublishReceipt, CoreError> {
        let key = IdempotencyKeyV1 {
            kind,
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            generation,
            batch_digest: batch_digest.to_string(),
        };
        let body_sha256 = canonical_body_sha256_v1(kind, body)?;
        match self.idempotency.begin(&key, &body_sha256)? {
            IdempotencyBeginV1::Replay {
                receipt,
                durable_sequence,
            } => return Ok(receipt.recorded_at(durable_sequence).replayed()),
            IdempotencyBeginV1::Fresh | IdempotencyBeginV1::Resume => {}
        }
        let receipt = apply()?;
        let durable_sequence = self.idempotency.finalize(&key, &body_sha256, &receipt)?;
        Ok(receipt.recorded_at(durable_sequence))
    }

    /// Publish one generation-scoped batch and, on success, drop both
    /// tracks' residency for that generation.
    ///
    /// The invalidation is unconditional on success rather than keyed to
    /// "did the adapter actually change bytes": the adapter is the only party
    /// that knows, and asking it would put a second cache-coherence contract
    /// on every ingest port. Dropping a handle costs one cold open on the
    /// next query; serving a stale one costs correctness.
    fn publish_generation_scoped<R>(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        publish: impl FnOnce() -> Result<R, CoreError>,
    ) -> Result<R, CoreError> {
        let receipt = publish()?;
        self.snapshots
            .invalidate(&SnapshotKey::new(repo_id, revision_id, generation))?;
        Ok(receipt)
    }

    /// Serve one ingest request (QI-BB-002).
    ///
    /// The budget is checked once, at entry: a batch whose producer hung up
    /// or whose deadline passed while it queued for the serial dispatch slot
    /// is refused before any track mutates, so the producer's retry starts
    /// from zero bytes changed. Once admitted, a publish runs to its durable
    /// end under the dispatcher's ownership — a batch applied halfway and
    /// then abandoned would leave the generation in a shape no retry can
    /// reason about.
    #[must_use]
    pub fn dispatch(
        &self,
        request: SearchPlaneIngestIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneIngestIpcResponse {
        if let Err(err) = budget.checkpoint("ingest:entry") {
            return SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err));
        }
        match request {
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::SearchCorpus,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || {
                        self.publish_generation_scoped(
                            &batch.repo_id,
                            &batch.revision_id,
                            batch.generation,
                            || self.lexical.publish_batch(&batch),
                        )
                    },
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::History,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.history.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::HistoryReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::RepoCommitRecency,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || {
                        self.publish_generation_scoped(
                            &batch.repo_id,
                            &batch.revision_id,
                            batch.generation,
                            || self.repo_commit_recency.publish_batch(&batch),
                        )
                    },
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::RepoTopic,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || {
                        self.publish_generation_scoped(
                            &batch.repo_id,
                            &batch.revision_id,
                            batch.generation,
                            || self.repo_topic.publish_batch(&batch),
                        )
                    },
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoTopicReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::RepoDescription,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || {
                        self.publish_generation_scoped(
                            &batch.repo_id,
                            &batch.revision_id,
                            batch.generation,
                            || self.repo_description.publish_batch(&batch),
                        )
                    },
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::FileOwnership,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || {
                        self.publish_generation_scoped(
                            &batch.repo_id,
                            &batch.revision_id,
                            batch.generation,
                            || self.file_ownership.publish_batch(&batch),
                        )
                    },
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::FileOwnershipReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::FileContributor,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || {
                        self.publish_generation_scoped(
                            &batch.repo_id,
                            &batch.revision_id,
                            batch.generation,
                            || self.file_contributor.publish_batch(&batch),
                        )
                    },
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::RepoMeta,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || {
                        self.publish_generation_scoped(
                            &batch.repo_id,
                            &batch.revision_id,
                            batch.generation,
                            || self.repo_meta.publish_batch(&batch),
                        )
                    },
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoMetaReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::Dirty,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.runtime.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::DirtyReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::RuntimeCatalog,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.runtime.publish_catalog_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::Structural,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.structural.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::StructuralReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoMapBundle(bundle) => {
                match self.repomap.ingest_bundle(&bundle) {
                    Ok(()) => SearchPlaneIngestIpcResponse::RepoMapReceipt(RepoMapMutationAck {
                        repo_id: bundle.repo_id,
                        revision_id: bundle.revision_id,
                        manifest_generation: bundle.manifest_generation,
                    }),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            } // QI-LXB-01 / QI-HIST-01 / QI-RT-02 / QI-STR-02: history /
              // dirty / structural batches now have first-class arms above.
              // No fallback arm needed.
        }
    }
}

/// SHA-256 over the batch's canonical CBOR encoding under a per-route
/// domain: the identity a replay must match byte for byte.
fn canonical_body_sha256_v1<B: serde::Serialize>(
    kind: IngestOperationKindV1,
    body: &B,
) -> Result<[u8; 32], CoreError> {
    use sha2::Digest as _;
    let encoded = quanta_index_ipc::encode_cbor_payload(body).map_err(|err| {
        CoreError::Storage(format!(
            "ingest idempotency: encode {kind} batch body: {err}"
        ))
    })?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"quanta-index:ingest-batch-body:v1\0");
    hasher.update(kind.as_code_str().as_bytes());
    hasher.update(b"\x1f");
    hasher.update(&encoded);
    Ok(hasher.finalize().into())
}

fn core_error_to_ipc(err: CoreError) -> SearchPlaneIpcError {
    let (code, message) = match err {
        CoreError::InvalidContract(msg) => (ERR_INVALID.to_string(), msg),
        CoreError::Typed { code, message } => (code, message),
        CoreError::NotReady(msg) => (ERR_NOT_READY.to_string(), msg),
        CoreError::NotImplemented(msg) => (ERR_NOT_IMPLEMENTED.to_string(), msg),
        CoreError::NotFound(msg) => (ERR_NOT_FOUND.to_string(), msg),
        CoreError::Storage(msg) => (ERR_INTERNAL.to_string(), msg),
    };
    // Ingest failures carry no query-intent repair metadata (J7Q-06 repair is
    // query-route specific); the wire field stays None.
    SearchPlaneIpcError {
        code,
        message,
        repair: None,
    }
}

// =============================================================================
// Tests — direct authority persistence
// =============================================================================

#[cfg(test)]
mod tests {
    #![expect(
        clippy::panic_in_result_fn,
        reason = "Result-returning ingest tests use assertions as test-failure reporting"
    )]
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, RwLock};

    use super::*;
    use crate::SEARCH_OWNED_SEMANTIC_DIMENSION;
    use crate::semantic_derive::{
        semantic_embedding_input_digest, semantic_embedding_input_text, semantic_vector_digest,
    };
    use quanta_index_contract::{
        BatchIngestMode, CapabilityStatusV1, ChunkId, ChunkRecord, EmbeddingDistanceMetric,
        EmbeddingId, EmbeddingModelContract, EmbeddingNormalization, EmbeddingRecord,
        ManifestGeneration, OwnerDocKind, RepoId, RepoRelativePath, RevisionId,
        SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchPlaneTrackKind, SearchScopeKey,
        SearchScopeSurface, SemanticCorpusKindV1, SemanticIngestBatch, SemanticReplaceScope,
        SourceRoleV1,
    };

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    /// One in-memory record: the body hash and, once finalized, the receipt
    /// and its sequence.
    type MemoryRecord = ([u8; 32], Option<(BatchPublishReceipt, u64)>);

    /// An in-memory idempotency catalog with the port's exact semantics, for
    /// tests that need the protocol without the storage engine.
    #[derive(Default)]
    pub(crate) struct MemoryIdempotencyCatalog {
        records: Mutex<BTreeMap<IdempotencyKeyV1, MemoryRecord>>,
        next_sequence: AtomicUsize,
    }

    impl MemoryIdempotencyCatalog {
        pub(crate) fn records(&self) -> usize {
            self.records.lock().map_or(0, |records| records.len())
        }
    }

    impl IdempotencyCatalogPort for MemoryIdempotencyCatalog {
        fn begin(
            &self,
            key: &IdempotencyKeyV1,
            body_sha256: &[u8; 32],
        ) -> Result<IdempotencyBeginV1, CoreError> {
            let mut records = self
                .records
                .lock()
                .map_err(|err| CoreError::Storage(format!("memory catalog poisoned: {err}")))?;
            let outcome = match records.get(key) {
                None => {
                    let _new = records.insert(key.clone(), (*body_sha256, None));
                    Ok(IdempotencyBeginV1::Fresh)
                }
                Some((stored, _)) if stored != body_sha256 => Err(CoreError::Typed {
                    code: quanta_index_core::BATCH_DIGEST_CONFLICT_CODE.to_string(),
                    message: format!(
                        "{} batch_digest={} body differs",
                        key.kind, key.batch_digest
                    ),
                }),
                Some((_, Some((receipt, durable_sequence)))) => Ok(IdempotencyBeginV1::Replay {
                    receipt: receipt.clone(),
                    durable_sequence: *durable_sequence,
                }),
                Some((_, None)) => Ok(IdempotencyBeginV1::Resume),
            };
            drop(records);
            outcome
        }

        fn finalize(
            &self,
            key: &IdempotencyKeyV1,
            body_sha256: &[u8; 32],
            receipt: &BatchPublishReceipt,
        ) -> Result<u64, CoreError> {
            let mut records = self
                .records
                .lock()
                .map_err(|err| CoreError::Storage(format!("memory catalog poisoned: {err}")))?;
            let Some(record) = records.get_mut(key) else {
                return Err(CoreError::InvalidContract(
                    "finalize before begin".to_string(),
                ));
            };
            if record.0 != *body_sha256 {
                return Err(CoreError::InvalidContract(
                    "finalize under another body".to_string(),
                ));
            }
            if record.1.is_some() {
                return Err(CoreError::InvalidContract("finalize twice".to_string()));
            }
            let sequence = u64::try_from(self.next_sequence.fetch_add(1, Ordering::SeqCst))
                .map_err(|err| CoreError::Storage(err.to_string()))?
                .saturating_add(1);
            record.1 = Some((receipt.clone(), sequence));
            drop(records);
            Ok(sequence)
        }

        fn forget_generation(
            &self,
            repo_id: &RepoId,
            revision_id: &RevisionId,
            generation: ManifestGeneration,
        ) -> Result<u64, CoreError> {
            let mut records = self
                .records
                .lock()
                .map_err(|err| CoreError::Storage(format!("memory catalog poisoned: {err}")))?;
            let before = records.len();
            records.retain(|key, _| {
                !(key.repo_id == *repo_id
                    && key.revision_id == *revision_id
                    && key.generation == generation)
            });
            u64::try_from(before.saturating_sub(records.len()))
                .map_err(|err| CoreError::Storage(err.to_string()))
        }
    }

    fn memory_catalog() -> Arc<MemoryIdempotencyCatalog> {
        Arc::new(MemoryIdempotencyCatalog::default())
    }

    use crate::auxiliary_authority::testing::MemoryAuxiliaryCatalog;

    fn memory_aux_catalog() -> Arc<MemoryAuxiliaryCatalog> {
        Arc::new(MemoryAuxiliaryCatalog::default())
    }

    fn aux_parts(
        ledger: Arc<RwLock<Ledger>>,
    ) -> (AuxiliaryMaterializerParts, Arc<MemoryAuxiliaryCatalog>) {
        let catalog = memory_aux_catalog();
        (
            AuxiliaryMaterializerParts {
                catalog: catalog.clone(),
                coordinator: AuxiliaryMutationCoordinator::shared(),
                ledger,
            },
            catalog,
        )
    }

    #[test]
    fn dirty_publish_receipt_binds_exact_auxiliary_batch_without_sealing_v1() {
        let batch = DirtyIngestBatch {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(9),
            overlay_epoch_ms: 7,
            batch_digest: "dirty-batch:exact".to_string(),
            entries: Vec::new(),
        };
        let receipt = dirty_publish_receipt_v1(&batch);
        assert_eq!(receipt.generation, batch.generation);
        assert_eq!(receipt.manifest_digest, None);
        assert_eq!(receipt.batch_digest, batch.batch_digest);
        assert_eq!(receipt.accepted_replace_scopes, 0);
        assert_eq!(receipt.accepted_tombstone_scopes, 0);
        assert!(!receipt.sealed);
    }

    macro_rules! search_corpus_materializer {
        (
            $builder:expr,
            $ledger:expr,
            $semantic_ingest:expr,
            $semantic_embedder:expr,
            $authority:expr,
            $lexical_generation_validator:expr,
            $semantic_generation_validator:expr,
            $lexical_incomplete_discard:expr,
            $semantic_incomplete_discard:expr $(,)?
        ) => {
            DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
                SearchCorpusMaterializerParts {
                    builder: $builder,
                    ledger: $ledger,
                    semantic_ingest: $semantic_ingest,
                    semantic_embedder: $semantic_embedder,
                    authority: $authority,
                    lexical_generation_validator: $lexical_generation_validator,
                    semantic_generation_validator: $semantic_generation_validator,
                    lexical_incomplete_discard: $lexical_incomplete_discard,
                    semantic_incomplete_discard: $semantic_incomplete_discard,
                    lexical_reclaim: no_storage_sealed_reclaim(),
                    semantic_reclaim: no_storage_sealed_reclaim(),
                    snapshots: SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT),
                    idempotency: memory_catalog(),
                    resource_policy: IngestResourcePolicy::DEFAULT,
                    auxiliary_catalog: memory_aux_catalog(),
                    auxiliary_coordinator: AuxiliaryMutationCoordinator::shared(),
                },
            )
        };
    }

    #[derive(Default)]
    struct RecordingSearchCorpusAuthority {
        identities: Mutex<Vec<(RepoId, RevisionId, ManifestGeneration, String)>>,
        exact: bool,
    }

    impl SearchCorpusAuthorityWritePort for RecordingSearchCorpusAuthority {
        fn inspect_sealed_search_corpus(
            &self,
            _repo_id: &RepoId,
            _revision_id: &RevisionId,
            _generation: ManifestGeneration,
            _manifest_digest: &str,
        ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError> {
            Ok(if self.exact {
                SealedSearchCorpusAuthorityStateV1::Exact
            } else {
                SealedSearchCorpusAuthorityStateV1::Absent
            })
        }

        fn record_sealed_search_corpus(
            &self,
            repo_id: &RepoId,
            revision_id: &RevisionId,
            generation: ManifestGeneration,
            manifest_digest: &str,
        ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError> {
            let retained_generations = {
                let mut identities = self.identities.lock().map_err(|err| {
                    CoreError::Storage(format!("recording search-corpus authority poisoned: {err}"))
                })?;
                identities.push((
                    repo_id.clone(),
                    revision_id.clone(),
                    generation,
                    manifest_digest.to_string(),
                ));
                identities
                    .iter()
                    .filter(|(observed_repo, observed_revision, _, _)| {
                        observed_repo == repo_id && observed_revision == revision_id
                    })
                    .map(|(_, _, observed_generation, _)| *observed_generation)
                    .collect::<Vec<_>>()
            };
            Ok(
                SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
                    repo_id,
                    revision_id,
                    retained_generations,
                ),
            )
        }
    }

    fn recording_search_corpus_authority() -> Arc<dyn SearchCorpusAuthorityWritePort + Send + Sync>
    {
        Arc::new(RecordingSearchCorpusAuthority::default())
    }

    struct FailingRetentionAuthority;

    impl SearchCorpusAuthorityWritePort for FailingRetentionAuthority {
        fn inspect_sealed_search_corpus(
            &self,
            _repo_id: &RepoId,
            _revision_id: &RevisionId,
            _generation: ManifestGeneration,
            _manifest_digest: &str,
        ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError> {
            Ok(SealedSearchCorpusAuthorityStateV1::Exact)
        }

        fn record_sealed_search_corpus(
            &self,
            _repo_id: &RepoId,
            _revision_id: &RevisionId,
            _generation: ManifestGeneration,
            _manifest_digest: &str,
        ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError> {
            Err(CoreError::Storage(
                "injected post-delete retention durability failure".to_string(),
            ))
        }
    }

    #[derive(Default)]
    struct BuildThenValidGeneration {
        validations: AtomicUsize,
    }

    impl GenerationIdentityValidatePort for BuildThenValidGeneration {
        fn validate_generation_identity(
            &self,
            candidate: &GenerationSnapshot,
        ) -> Result<(), CoreError> {
            if self.validations.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(CoreError::NotFound(format!(
                    "test generation not built yet: {:?}",
                    candidate.track
                )));
            }
            Ok(())
        }
    }

    fn build_then_valid_generation() -> Arc<dyn GenerationIdentityValidatePort + Send + Sync> {
        Arc::new(BuildThenValidGeneration::default())
    }

    #[derive(Default)]
    struct IncompleteThenValidGeneration {
        validations: AtomicUsize,
    }

    impl GenerationIdentityValidatePort for IncompleteThenValidGeneration {
        fn validate_generation_identity(
            &self,
            _candidate: &GenerationSnapshot,
        ) -> Result<(), CoreError> {
            if self.validations.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(CoreError::Typed {
                    code: "GENERATION_IDENTITY_INCOMPLETE".to_string(),
                    message: "injected incomplete generation".to_string(),
                });
            }
            Ok(())
        }
    }

    fn incomplete_then_valid_generation() -> Arc<dyn GenerationIdentityValidatePort + Send + Sync> {
        Arc::new(IncompleteThenValidGeneration::default())
    }

    struct AlwaysValidGeneration;

    impl GenerationIdentityValidatePort for AlwaysValidGeneration {
        fn validate_generation_identity(
            &self,
            _candidate: &GenerationSnapshot,
        ) -> Result<(), CoreError> {
            Ok(())
        }
    }

    fn always_valid_generation() -> Arc<dyn GenerationIdentityValidatePort + Send + Sync> {
        Arc::new(AlwaysValidGeneration)
    }

    struct TestIncompleteGenerationDiscard;

    impl IncompleteGenerationDiscardPort for TestIncompleteGenerationDiscard {
        fn discard_incomplete_generation(
            &self,
            _candidate: &GenerationSnapshot,
        ) -> Result<IncompleteGenerationDiscardOutcomeV1, CoreError> {
            Ok(IncompleteGenerationDiscardOutcomeV1::Discarded)
        }
    }

    fn test_incomplete_generation_discard() -> Arc<dyn IncompleteGenerationDiscardPort + Send + Sync>
    {
        Arc::new(TestIncompleteGenerationDiscard)
    }

    /// A reclaim port over no storage.
    ///
    /// Nothing is ever on disk, so the sweep finds nothing. The materializer
    /// tests here exercise the authority and ledger protocol; physical
    /// reclaim is proven against the real adapters in the daemon-level
    /// `e2e_physical_gc` test.
    struct NoStorageSealedReclaim;

    impl SealedGenerationReclaimPort for NoStorageSealedReclaim {
        fn reclaim_sealed_generation(
            &self,
            _retired: &GenerationSnapshot,
        ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError> {
            Ok(SealedGenerationReclaimOutcomeV1::Absent)
        }

        fn sealed_generations_for_pair(
            &self,
            _repo_id: &RepoId,
            _revision_id: &RevisionId,
        ) -> Result<Vec<GenerationSnapshot>, CoreError> {
            Ok(Vec::new())
        }
    }

    fn no_storage_sealed_reclaim() -> Arc<dyn SealedGenerationReclaimPort + Send + Sync> {
        Arc::new(NoStorageSealedReclaim)
    }

    /// A reclaim port over a scripted set of on-disk sealed generations; it
    /// records every reclaim so the protocol's decisions are observable.
    struct ScriptedSealedReclaim {
        track: SearchPlaneTrackKind,
        on_disk: Mutex<Vec<ManifestGeneration>>,
        reclaimed: Mutex<Vec<ManifestGeneration>>,
    }

    impl ScriptedSealedReclaim {
        fn new(track: SearchPlaneTrackKind, on_disk: &[u64]) -> Arc<Self> {
            Arc::new(Self {
                track,
                on_disk: Mutex::new(
                    on_disk
                        .iter()
                        .copied()
                        .map(ManifestGeneration::new)
                        .collect(),
                ),
                reclaimed: Mutex::new(Vec::new()),
            })
        }

        fn reclaimed(&self) -> Vec<u64> {
            match self.reclaimed.lock() {
                Ok(guard) => guard.iter().map(|generation| generation.get()).collect(),
                Err(poisoned) => poisoned
                    .into_inner()
                    .iter()
                    .map(|generation| generation.get())
                    .collect(),
            }
        }

        fn remaining(&self) -> Vec<u64> {
            match self.on_disk.lock() {
                Ok(guard) => guard.iter().map(|generation| generation.get()).collect(),
                Err(poisoned) => poisoned
                    .into_inner()
                    .iter()
                    .map(|generation| generation.get())
                    .collect(),
            }
        }
    }

    impl SealedGenerationReclaimPort for ScriptedSealedReclaim {
        fn reclaim_sealed_generation(
            &self,
            retired: &GenerationSnapshot,
        ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError> {
            let mut on_disk = self
                .on_disk
                .lock()
                .map_err(|err| CoreError::Storage(format!("scripted reclaim poisoned: {err}")))?;
            let Some(index) = on_disk
                .iter()
                .position(|generation| *generation == retired.manifest_generation)
            else {
                return Ok(SealedGenerationReclaimOutcomeV1::Absent);
            };
            let _removed = on_disk.remove(index);
            drop(on_disk);
            self.reclaimed
                .lock()
                .map_err(|err| CoreError::Storage(format!("scripted reclaim poisoned: {err}")))?
                .push(retired.manifest_generation);
            Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { bytes: 1 })
        }

        fn sealed_generations_for_pair(
            &self,
            repo_id: &RepoId,
            revision_id: &RevisionId,
        ) -> Result<Vec<GenerationSnapshot>, CoreError> {
            Ok(self
                .on_disk
                .lock()
                .map_err(|err| CoreError::Storage(format!("scripted reclaim poisoned: {err}")))?
                .iter()
                .map(|generation| GenerationSnapshot {
                    repo_id: repo_id.clone(),
                    revision_id: revision_id.clone(),
                    track: self.track,
                    manifest_generation: *generation,
                    manifest_digest: format!("digest:{}", generation.get()),
                })
                .collect())
        }
    }

    /// The smallest `LexicalSearcher` that can occupy a registry slot: it
    /// exists only so a test can hold a pin on a generation.
    struct PinnedLexicalHandle;

    impl quanta_index_core::LexicalSearcher for PinnedLexicalHandle {
        fn resident_bytes_estimate(&self) -> u64 {
            1
        }

        fn search_constrained(
            &self,
            _query: &quanta_index_contract::LqQuery,
            _constraints: &quanta_index_contract::QueryConstraintSetV1,
            _top_k: u32,
        ) -> Result<quanta_index_core::LexicalSearchPageV1, CoreError> {
            Err(CoreError::NotImplemented("pin-only handle".to_string()))
        }

        fn project_file_owners(
            &self,
            _candidates: &[quanta_index_contract::LexicalCandidate],
        ) -> Result<Vec<quanta_index_contract::FileOwnerProjectionRow>, CoreError> {
            Err(CoreError::NotImplemented("pin-only handle".to_string()))
        }

        fn search_symbols(
            &self,
            _query: &quanta_index_contract::LqQuery,
            _top_k: u32,
        ) -> Result<Vec<quanta_index_contract::SymbolCandidate>, CoreError> {
            Err(CoreError::NotImplemented("pin-only handle".to_string()))
        }

        fn search_all(
            &self,
            _query: &quanta_index_contract::LqQuery,
        ) -> Result<Vec<quanta_index_contract::LexicalCandidate>, CoreError> {
            Err(CoreError::NotImplemented("pin-only handle".to_string()))
        }

        fn candidate_presence(
            &self,
            _candidate_id: &str,
        ) -> Result<quanta_index_contract::CandidatePresenceV1, CoreError> {
            Err(CoreError::NotImplemented("pin-only handle".to_string()))
        }

        fn explain_candidate(
            &self,
            _query: &quanta_index_contract::LqQuery,
            _constraints: &quanta_index_contract::QueryConstraintSetV1,
            _candidate_id: &str,
        ) -> Result<quanta_index_core::LexicalCandidateExplanationV1, CoreError> {
            Err(CoreError::NotImplemented("pin-only handle".to_string()))
        }
    }

    /// One materializer over recording fakes, plus the fakes, so a test can
    /// prove that a refused batch touched nothing.
    struct ZeroMutationProbe {
        materializer: DirectSearchCorpusMaterializer,
        lexical_builder: Arc<FakeSearchCorpusBuilder>,
        semantic_builder: Arc<FakeSemanticBuilder>,
        authority: Arc<RecordingSearchCorpusAuthority>,
        ledger: Arc<RwLock<Ledger>>,
    }

    impl ZeroMutationProbe {
        fn new(validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>) -> Self {
            Self::with_resource_policy(validator, IngestResourcePolicy::DEFAULT)
        }

        fn with_resource_policy(
            validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
            resource_policy: IngestResourcePolicy,
        ) -> Self {
            let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
            let semantic_builder = Arc::new(FakeSemanticBuilder::default());
            let authority = Arc::new(RecordingSearchCorpusAuthority::default());
            let ledger = Arc::new(RwLock::new(Ledger::new()));
            let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
                Arc::new(DirectSemanticMaterializer::new(
                    semantic_builder.clone(),
                    Arc::new(RwLock::new(Ledger::new())),
                ));
            let materializer = DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
                SearchCorpusMaterializerParts {
                    builder: lexical_builder.clone(),
                    ledger: Arc::clone(&ledger),
                    semantic_ingest: semantic_materializer,
                    semantic_embedder: Arc::new(crate::HashingQueryTextEmbedder::new(
                        SEARCH_OWNED_SEMANTIC_DIMENSION,
                    )),
                    authority: authority.clone(),
                    lexical_generation_validator: Arc::clone(&validator),
                    semantic_generation_validator: validator,
                    lexical_incomplete_discard: test_incomplete_generation_discard(),
                    semantic_incomplete_discard: test_incomplete_generation_discard(),
                    lexical_reclaim: no_storage_sealed_reclaim(),
                    semantic_reclaim: no_storage_sealed_reclaim(),
                    snapshots: SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT),
                    idempotency: memory_catalog(),
                    resource_policy,
                    auxiliary_catalog: memory_aux_catalog(),
                    auxiliary_coordinator: AuxiliaryMutationCoordinator::shared(),
                },
            );
            Self {
                materializer,
                lexical_builder,
                semantic_builder,
                authority,
                ledger,
            }
        }

        fn assert_nothing_touched(&self, what: &str) -> TestRes {
            let lexical = self
                .lexical_builder
                .batches
                .lock()
                .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
                .len();
            let semantic = self.semantic_builder.take()?.len();
            let recorded = self
                .authority
                .identities
                .lock()
                .map_err(|err| format!("recording authority poisoned: {err}"))?
                .len();
            if lexical != 0 || semantic != 0 || recorded != 0 {
                return Err(format!(
                    "{what}: refused batch still mutated: lexical_builds={lexical} semantic_builds={semantic} authority_records={recorded}"
                )
                .into());
            }
            Ok(())
        }
    }

    /// A validator that reports the base as sealed under a different digest.
    struct MismatchedGeneration;

    impl GenerationIdentityValidatePort for MismatchedGeneration {
        fn validate_generation_identity(
            &self,
            candidate: &GenerationSnapshot,
        ) -> Result<(), CoreError> {
            Err(CoreError::Typed {
                code: "GENERATION_IDENTITY_DIGEST_MISMATCH".to_string(),
                message: format!(
                    "injected digest mismatch for {:?} generation {}",
                    candidate.track,
                    candidate.manifest_generation.get()
                ),
            })
        }
    }

    /// QI-BB-029: refused batches change nothing.
    ///
    /// A batch the contract refuses, a delta on a base the ledger never
    /// sealed, and a delta on a base whose physical identity disagrees with
    /// the ledger all refuse before either builder or the authority is
    /// touched.
    #[test]
    fn malformed_or_baseless_batches_change_zero_bytes() -> TestRes {
        let probe = ZeroMutationProbe::new(always_valid_generation());

        let mut malformed = fixture_search_corpus_batch()?;
        malformed.base_generation = Some(ManifestGeneration::new(3));
        match probe.materializer.publish_batch(&malformed) {
            Err(CoreError::Typed { code, .. }) if code == ERR_SEARCH_CORPUS_BATCH_SHAPE => {}
            other => return Err(format!("mode/base mismatch answered {other:?}").into()),
        }
        probe.assert_nothing_touched("mode/base mismatch")?;

        let mut empty_digest = fixture_search_corpus_batch()?;
        empty_digest.manifest_digest = String::new();
        match probe.materializer.publish_batch(&empty_digest) {
            Err(CoreError::Typed { code, .. }) if code == ERR_SEARCH_CORPUS_BATCH_SHAPE => {}
            other => return Err(format!("empty digest answered {other:?}").into()),
        }
        probe.assert_nothing_touched("empty digest")?;

        let mut unsealed_base = fixture_search_corpus_batch()?;
        unsealed_base.mode = BatchIngestMode::Delta;
        unsealed_base.base_generation = Some(ManifestGeneration::new(3));
        match probe.materializer.publish_batch(&unsealed_base) {
            Err(CoreError::Typed { code, .. })
                if code == ERR_SEARCH_CORPUS_DELTA_BASE_NOT_SEALED => {}
            other => return Err(format!("unsealed base answered {other:?}").into()),
        }
        probe.assert_nothing_touched("base never sealed")?;

        // The ledger knows the base, but the physical identity disagrees.
        let mismatched = ZeroMutationProbe::new(Arc::new(MismatchedGeneration));
        mismatched
            .ledger
            .write()
            .map_err(|err| format!("ledger poisoned: {err}"))?
            .record_historically_sealed_search_corpus(
                &unsealed_base.repo_id,
                &unsealed_base.revision_id,
                ManifestGeneration::new(3),
                "manifest:base",
            );
        match mismatched.materializer.publish_batch(&unsealed_base) {
            Err(CoreError::Typed { code, .. }) if code == ERR_SEARCH_CORPUS_GENERATION_CONFLICT => {
            }
            other => return Err(format!("mismatched base answered {other:?}").into()),
        }
        mismatched.assert_nothing_touched("base identity mismatch")
    }

    /// QI-BB-021: a batch outside the resource envelope is refused typed
    /// before any track is touched, and the same batch under an envelope it
    /// fits is admitted and measured.
    #[test]
    fn a_batch_outside_the_resource_envelope_changes_zero_bytes() -> TestRes {
        let batch = multi_scope_corpus_batch()?;
        let embedded_records: usize = batch.replace_scopes.iter().map(|s| s.chunks.len()).sum();
        let text_bytes: u64 = batch
            .replace_scopes
            .iter()
            .flat_map(|scope| scope.chunks.iter().map(|chunk| chunk.text.len()))
            .map(u64::try_from)
            .sum::<Result<u64, _>>()?;
        let vector_bytes = u64::try_from(embedded_records * SEARCH_OWNED_SEMANTIC_DIMENSION * 4)?;

        let tight = ZeroMutationProbe::with_resource_policy(
            always_valid_generation(),
            IngestResourcePolicy::new(usize::MAX, u64::MAX, vector_bytes - 1)?,
        );
        match tight.materializer.publish_batch(&batch) {
            Err(CoreError::Typed { code, .. })
                if code == quanta_index_core::INGEST_RESOURCE_BUDGET_EXCEEDED_CODE => {}
            other => return Err(format!("oversized batch answered {other:?}").into()),
        }
        tight.assert_nothing_touched("resource envelope")?;
        let stats = tight.materializer.resource_stats()?;
        if stats
            != (IngestResourceStats {
                refused: 1,
                ..IngestResourceStats::default()
            })
        {
            return Err(format!("refusal must be counted and nothing admitted: {stats:?}").into());
        }

        let fits = ZeroMutationProbe::with_resource_policy(
            always_valid_generation(),
            IngestResourcePolicy::new(embedded_records, text_bytes, vector_bytes)?,
        );
        let _receipt = fits.materializer.publish_batch(&batch)?;
        let stats = fits.materializer.resource_stats()?;
        let expected = IngestResourceStats {
            admitted: 1,
            refused: 0,
            peak_embedded_records: embedded_records,
            peak_text_bytes: text_bytes,
            peak_vector_bytes: vector_bytes,
        };
        if stats != expected {
            return Err(format!("admitted footprint drifted: {stats:?} != {expected:?}").into());
        }
        Ok(())
    }

    /// The physical reclaim protocol under pins and orphans.
    ///
    /// Every sealed directory the receipt does not retain is reclaimed —
    /// including one whose authority record was reaped by an earlier pass
    /// (the crash orphan) — except a generation a resident handle still
    /// pins, which is deferred and reclaimed on the next pass once the pin
    /// is gone.
    #[test]
    fn reclaim_sweeps_orphans_and_defers_pinned_generations() -> TestRes {
        let lexical_reclaim =
            ScriptedSealedReclaim::new(SearchPlaneTrackKind::Lexical, &[1, 2, 3, 4, 5]);
        let semantic_reclaim =
            ScriptedSealedReclaim::new(SearchPlaneTrackKind::Semantic, &[2, 3, 4, 5]);
        let snapshots = SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT);
        let lexical_ledger = Arc::new(RwLock::new(Ledger::new()));
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
            Arc::new(DirectSemanticMaterializer::new(
                Arc::new(FakeSemanticBuilder::default()),
                Arc::new(RwLock::new(Ledger::new())),
            ));
        let materializer = DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
            SearchCorpusMaterializerParts {
                builder: Arc::new(FakeSearchCorpusBuilder::default()),
                ledger: lexical_ledger,
                semantic_ingest: semantic_materializer,
                semantic_embedder: Arc::new(crate::HashingQueryTextEmbedder::new(
                    SEARCH_OWNED_SEMANTIC_DIMENSION,
                )),
                authority: Arc::new(RecordingSearchCorpusAuthority::default()),
                lexical_generation_validator: always_valid_generation(),
                semantic_generation_validator: always_valid_generation(),
                lexical_incomplete_discard: test_incomplete_generation_discard(),
                semantic_incomplete_discard: test_incomplete_generation_discard(),
                lexical_reclaim: lexical_reclaim.clone(),
                semantic_reclaim: semantic_reclaim.clone(),
                snapshots: snapshots.clone(),
                idempotency: memory_catalog(),
                resource_policy: IngestResourcePolicy::DEFAULT,
                auxiliary_catalog: memory_aux_catalog(),
                auxiliary_coordinator: AuxiliaryMutationCoordinator::shared(),
            },
        );
        let mut batch = fixture_search_corpus_batch()?;
        batch.generation = ManifestGeneration::new(5);
        let receipt = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
            &batch.repo_id,
            &batch.revision_id,
            [ManifestGeneration::new(4), ManifestGeneration::new(5)],
        );

        // A query still holds generation 2 on the lexical track.
        let pinned_key = SnapshotKey::new(
            &batch.repo_id,
            &batch.revision_id,
            ManifestGeneration::new(2),
        );
        let pin = snapshots
            .lexical
            .acquire(&pinned_key, || {
                let handle: Arc<dyn quanta_index_core::LexicalSearcher> =
                    Arc::new(PinnedLexicalHandle);
                Ok(crate::OpenedSnapshot {
                    handle,
                    resident_bytes: 1,
                })
            })?
            .handle;

        let first = materializer.reclaim_retired_generations_v1(&batch, &receipt)?;
        if lexical_reclaim.reclaimed() != [1, 3] || semantic_reclaim.reclaimed() != [2, 3] {
            return Err(format!(
                "first pass drifted: lexical={:?} semantic={:?}",
                lexical_reclaim.reclaimed(),
                semantic_reclaim.reclaimed()
            )
            .into());
        }
        if !first.deferred_pinned.contains(&(
            SearchPlaneTrackKind::Lexical,
            ManifestGeneration::new(2),
            1,
        )) {
            return Err(format!("pinned generation was not deferred: {first:?}").into());
        }
        if lexical_reclaim.remaining() != [2, 4, 5] {
            return Err(format!(
                "pinned generation 2 must survive the pass: {:?}",
                lexical_reclaim.remaining()
            )
            .into());
        }

        // Release the pin: the next pass reclaims the orphan it left behind.
        drop(pin);
        let second = materializer.reclaim_retired_generations_v1(&batch, &receipt)?;
        if !second.deferred_pinned.is_empty() || lexical_reclaim.remaining() != [4, 5] {
            return Err(format!(
                "second pass drifted: deferred={:?} remaining={:?}",
                second.deferred_pinned,
                lexical_reclaim.remaining()
            )
            .into());
        }
        Ok(())
    }

    #[derive(Default)]
    struct RecordingIncompleteGenerationDiscard {
        calls: AtomicUsize,
    }

    impl IncompleteGenerationDiscardPort for RecordingIncompleteGenerationDiscard {
        fn discard_incomplete_generation(
            &self,
            _candidate: &GenerationSnapshot,
        ) -> Result<IncompleteGenerationDiscardOutcomeV1, CoreError> {
            let _previous = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(IncompleteGenerationDiscardOutcomeV1::Discarded)
        }
    }

    struct MismatchedSemanticIngest;

    impl SemanticIngestPort for MismatchedSemanticIngest {
        fn publish_batch(
            &self,
            batch: &SemanticIngestBatch,
        ) -> Result<BatchPublishReceipt, CoreError> {
            let mut receipt = BatchPublishReceipt::empty_for(
                batch.generation,
                Some("mismatched-semantic-digest".to_string()),
                batch.batch_digest.clone(),
            );
            if batch.seal {
                receipt.mark_sealed();
            }
            Ok(receipt)
        }
    }

    #[derive(Default)]
    struct FakeSemanticBuilder {
        batches: Mutex<Vec<SemanticIngestBatch>>,
    }

    impl FakeSemanticBuilder {
        fn take(&self) -> Result<Vec<SemanticIngestBatch>, Box<dyn std::error::Error>> {
            let mut guard = self
                .batches
                .lock()
                .map_err(|err| format!("fake semantic builder poisoned: {err}"))?;
            Ok(std::mem::take(&mut *guard))
        }
    }

    impl SemanticBatchBuildPort for FakeSemanticBuilder {
        fn build_batch(&self, batch: &SemanticIngestBatch) -> Result<(), CoreError> {
            self.batches
                .lock()
                .map_err(|err| {
                    CoreError::Storage(format!("fake semantic builder poisoned: {err}"))
                })?
                .push(batch.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeSearchCorpusBuilder {
        batches: Mutex<Vec<SearchCorpusIngestBatch>>,
    }

    impl quanta_index_core::SearchCorpusBatchBuildPort for FakeSearchCorpusBuilder {
        fn build_batch(&self, batch: &SearchCorpusIngestBatch) -> Result<(), CoreError> {
            self.batches
                .lock()
                .map_err(|err| {
                    CoreError::Storage(format!("fake search-corpus builder poisoned: {err}"))
                })?
                .push(batch.clone());
            Ok(())
        }
    }

    fn fixture_scope() -> SearchScopeKey {
        SearchScopeKey {
            doc_surface: SearchScopeSurface::Chunk,
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
        }
    }

    fn fixture_model_contract() -> EmbeddingModelContract {
        EmbeddingModelContract {
            model_id: "test-model".to_string().into_boxed_str(),
            model_version: None,
            dimension: 3,
            normalization: EmbeddingNormalization::None,
            distance_metric: EmbeddingDistanceMetric::Cosine,
            policy_digest: "policy:abc".to_string().into_boxed_str(),
            view_policy_digest: None,
        }
    }

    fn fixture_embedding_record() -> Result<EmbeddingRecord, Box<dyn std::error::Error>> {
        Ok(EmbeddingRecord {
            embedding_id: EmbeddingId::new("emb-1"),
            record_id: "emb-1".to_string().into_boxed_str(),
            owner_kind: OwnerDocKind::Chunk,
            owner_id: "main".to_string().into_boxed_str(),
            corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
            parent_owner_id: None,
            source_doc_id: "chunk-1".to_string().into_boxed_str(),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: quanta_index_contract::lex::LanguageCode::new("rust")
                .map_err(str::to_string)?,
            package: None,
            symbol_kind: None,
            visibility: None,
            source_role: SourceRoleV1::RawFallbackText,
            generated: false,
            capability_status: CapabilityStatusV1::Degraded,
            authority_digest: "search-owned:legacy-chunk-text"
                .to_string()
                .into_boxed_str(),
            render_policy_digest: "search-owned:legacy-chunk-text"
                .to_string()
                .into_boxed_str(),
            card_schema_version: 0,
            start_byte: 0,
            end_byte: 12,
            start_line: 1,
            end_line: 1,
            snippet: "fn main() {}".to_string().into_boxed_str(),
            embedding_input_digest: "input:abc".to_string().into_boxed_str(),
            vector_digest: "vec:def".to_string().into_boxed_str(),
            view_kind: "raw_chunk".to_string().into_boxed_str(),
            vector: vec![0.1, 0.2, 0.3],
        })
    }

    fn fixture_semantic_batch() -> Result<SemanticIngestBatch, Box<dyn std::error::Error>> {
        Ok(SemanticIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(1),
            base_generation: None,
            manifest_digest: "manifest:sem".to_string(),
            batch_digest: "batch:sem".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            model_contract: fixture_model_contract(),
            required_corpora: vec![SemanticCorpusKindV1::RawCodeFallback],
            corpus_policy_digest: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SemanticReplaceScope {
                scope: fixture_scope(),
                scope_digest: "scope:sem".to_string(),
                embeddings: vec![fixture_embedding_record()?],
                cluster_memberships: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            seal: true,
        })
    }

    fn fixture_chunk_record() -> Result<ChunkRecord, Box<dyn std::error::Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new("chunk-1"),
            repo_relative_path: RepoRelativePath::new("src/main.rs"),
            language: quanta_index_contract::lex::LanguageCode::new("rust")
                .map_err(str::to_string)?,
            start_byte: 0,
            end_byte: 24,
            start_line: 1,
            end_line: 1,
            text: "typed semantic parser".to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        })
    }

    fn fixture_search_corpus_batch() -> Result<SearchCorpusIngestBatch, Box<dyn std::error::Error>>
    {
        Ok(SearchCorpusIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(7),
            base_generation: None,
            manifest_digest: "manifest:lex".to_string(),
            batch_digest: "batch:lex".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![SearchCorpusReplaceScope {
                scope: fixture_scope(),
                scope_digest: "scope:lex".to_string(),
                chunks: vec![fixture_chunk_record()?],
                symbols: Vec::new(),
            }],
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: true,
        })
    }

    fn fixture_dirty_batch() -> DirtyIngestBatch {
        DirtyIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(9),
            overlay_epoch_ms: 123,
            batch_digest: "batch:dirty".to_string(),
            entries: vec![DirtyMutation::Upsert(
                quanta_index_contract::lex::DirtyRecord {
                    wire_version: 1,
                    doc_id: ChunkId::new("chunk-1"),
                    applied_at_ms: 123,
                    payload_hash: [7; 32],
                },
            )],
        }
    }

    #[test]
    fn direct_semantic_materializer_builds_durably_and_marks_ledger() -> TestRes {
        let builder = Arc::new(FakeSemanticBuilder::default());
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let materializer = DirectSemanticMaterializer::new(builder.clone(), Arc::clone(&ledger));
        let batch = fixture_semantic_batch()?;
        let receipt = materializer.publish_batch(&batch)?;
        if !receipt.sealed
            || receipt.manifest_digest.as_deref() != Some(batch.manifest_digest.as_str())
        {
            return Err("unexpected semantic materialize receipt".into());
        }

        // The durable builder received the batch (durability lives in the adapter).
        let built = builder.take()?;
        if built.as_slice() != [batch.clone()] {
            return Err(format!("durable builder did not receive batch: {built:?}").into());
        }

        // Readiness reflects the durable seal, not a journal write.
        let guard = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        if guard.track_sealed(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Semantic,
        ) != Some(batch.generation)
        {
            return Err("publish did not record sealed generation".into());
        }
        if guard.track_manifest_digest(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Semantic,
        ) != Some(batch.manifest_digest.as_str())
        {
            return Err("publish did not preserve manifest digest".into());
        }
        drop(guard);
        Ok(())
    }

    fn fixture_commit(sha_byte: u8, parents: &[u8]) -> quanta_index_contract::lex::CommitRecord {
        quanta_index_contract::lex::CommitRecord {
            wire_version: 1,
            sha: quanta_index_contract::lex::CommitSha::from_bytes([sha_byte; 20]),
            parents: parents
                .iter()
                .map(|parent| quanta_index_contract::lex::CommitSha::from_bytes([*parent; 20]))
                .collect(),
            author_time_ms: 1,
            committer_time_ms: 2,
            applied_at_ms: 3,
            author: "a".into(),
            author_name: None,
            author_email: None,
            committer: "c".into(),
            committer_name: None,
            committer_email: None,
            message: format!("commit {sha_byte}").into_boxed_str(),
            is_merge: false,
            tags: Vec::new(),
        }
    }

    fn fixture_history_batch(
        generation: u64,
        commits: Vec<quanta_index_contract::lex::CommitRecord>,
    ) -> HistoryIngestBatch {
        HistoryIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(generation),
            manifest_digest: None,
            batch_digest: format!("batch:history:{generation}"),
            commits,
            refs: Vec::new(),
            tags: Vec::new(),
            diff_hunks: Vec::new(),
        }
    }

    /// QI-BB-020: a batch whose rows never became durable is never
    /// visible, and its receipt never issued; the next attempt applies.
    #[test]
    fn a_mutation_whose_rows_never_became_durable_is_never_visible() -> TestRes {
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let (parts, catalog) = aux_parts(Arc::clone(&ledger));
        let materializer = DirectHistoryMaterializer::new(parts);
        let batch = fixture_history_batch(9, vec![fixture_commit(1, &[])]);

        catalog.fail_next_apply();
        let refused = materializer
            .publish_batch(&batch)
            .expect_err("a catalog that cannot commit refuses the publish");
        if !matches!(refused, CoreError::Storage(_)) {
            return Err(format!("expected the catalog's failure, got {refused:?}").into());
        }
        {
            let guard = ledger
                .read()
                .map_err(|err| format!("ledger poisoned: {err}"))?;
            if guard
                .history_state(&batch.repo_id, &batch.revision_id, batch.generation)
                .is_some()
            {
                return Err("rows that never became durable must not be visible".into());
            }
        }
        if catalog.applies() != 0 {
            return Err("nothing was applied".into());
        }

        let receipt = materializer.publish_batch(&batch)?;
        if receipt.accepted_replace_scopes != 1 || catalog.applies() != 1 {
            return Err(format!("the retry must apply once: {receipt:?}").into());
        }
        let guard = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        let commits = guard
            .history_state(&batch.repo_id, &batch.revision_id, batch.generation)
            .map(|state| state.commits().len());
        drop(guard);
        if commits != Some(1) {
            return Err(format!("the durable batch must be visible, saw {commits:?}").into());
        }
        Ok(())
    }

    /// QI-BB-020: a batch that fails validation writes nothing and touches
    /// nothing — the catalog is never asked.
    #[test]
    fn a_batch_that_fails_validation_never_reaches_the_catalog() -> TestRes {
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let (parts, catalog) = aux_parts(Arc::clone(&ledger));
        let materializer = DirectHistoryMaterializer::new(parts);
        // A child whose parent is neither in the state nor earlier in the batch.
        let orphan = fixture_history_batch(9, vec![fixture_commit(2, &[1])]);
        match materializer.publish_batch(&orphan) {
            Err(CoreError::Typed { code, .. }) if code == "HISTORY_COMMIT_PARENT_UNKNOWN" => {}
            other => return Err(format!("orphan commit answered {other:?}").into()),
        }
        if catalog.applies() != 0 || catalog.row_count() != 0 {
            return Err("a refused batch must not reach the catalog".into());
        }
        // The parent earlier in the same batch is enough.
        let ordered =
            fixture_history_batch(9, vec![fixture_commit(1, &[]), fixture_commit(2, &[1])]);
        let _receipt = materializer.publish_batch(&ordered)?;
        if catalog.applies() != 1 {
            return Err("an ordered batch applies once".into());
        }
        Ok(())
    }

    /// QI-BB-020: a one-row mutation over a generation holding a thousand
    /// rows writes one row.
    #[test]
    fn a_one_row_dirty_mutation_writes_one_row() -> TestRes {
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let (parts, catalog) = aux_parts(Arc::clone(&ledger));
        let materializer = DirectRuntimeMetadataMaterializer::new(parts);
        let mut seed = fixture_dirty_batch();
        seed.entries = (0..1_000_u32)
            .map(|index| {
                DirtyMutation::Upsert(quanta_index_contract::lex::DirtyRecord {
                    wire_version: 1,
                    doc_id: ChunkId::new(format!("doc-{index}")),
                    applied_at_ms: 1,
                    payload_hash: [0; 32],
                })
            })
            .collect();
        let _seeded = materializer.publish_batch(&seed)?;
        let before = catalog.rows_written();
        // 1,000 doc rows and the generation's one meta row.
        if before != 1_001 {
            return Err(format!("seeding wrote {before} rows, expected 1001").into());
        }
        let mut one = fixture_dirty_batch();
        one.batch_digest = "batch:dirty:one".to_string();
        one.entries = vec![DirtyMutation::Upsert(
            quanta_index_contract::lex::DirtyRecord {
                wire_version: 1,
                doc_id: ChunkId::new("doc-500"),
                applied_at_ms: 2,
                payload_hash: [1; 32],
            },
        )];
        let _receipt = materializer.publish_batch(&one)?;
        // The one doc row plus the generation's meta row: two rows, not a
        // thousand.
        let written = catalog.rows_written().saturating_sub(before);
        if written != 2 {
            return Err(format!("a one-row mutation wrote {written} rows").into());
        }
        let guard = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        let applied = guard
            .runtime_state(&one.repo_id, &one.revision_id, one.generation)
            .and_then(|state| state.dirty_docs().get(&ChunkId::new("doc-500")))
            .map(crate::readiness::DirtyDocState::applied_at_ms);
        let resident = guard
            .runtime_state(&one.repo_id, &one.revision_id, one.generation)
            .map(|state| state.dirty_docs().len());
        drop(guard);
        if applied != Some(2) || resident != Some(1_000) {
            return Err(format!(
                "the row changed in place: applied={applied:?} resident={resident:?}"
            )
            .into());
        }
        Ok(())
    }

    /// QI-BB-020: a reader holding a snapshot neither blocks a mutation nor
    /// sees it; the ledger holds the new state while the snapshot holds
    /// the old one.
    #[test]
    fn a_reader_holding_a_snapshot_neither_blocks_nor_sees_a_mutation() -> TestRes {
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let (parts, _catalog) = aux_parts(Arc::clone(&ledger));
        let materializer = Arc::new(DirectHistoryMaterializer::new(parts));
        let first = fixture_history_batch(9, vec![fixture_commit(1, &[])]);
        let _receipt = materializer.publish_batch(&first)?;
        let snapshot = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?
            .history_snapshot(&first.repo_id, &first.revision_id, first.generation)
            .ok_or("the first batch is visible")?;
        // The reader has released the lock but still holds the snapshot;
        // a mutation on another thread must complete.
        let second = fixture_history_batch(9, vec![fixture_commit(2, &[1])]);
        let writer = {
            let materializer = Arc::clone(&materializer);
            std::thread::spawn(move || materializer.publish_batch(&second))
        };
        let _receipt = writer
            .join()
            .map_err(|panic| format!("writer panicked: {panic:?}"))??;
        if snapshot.commits().len() != 1 {
            return Err("the held snapshot must not change under the reader".into());
        }
        let after = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?
            .history_snapshot(&first.repo_id, &first.revision_id, first.generation)
            .map(|state| state.commits().len());
        if after != Some(2) {
            return Err(format!("the ledger must hold the mutation, saw {after:?}").into());
        }
        Ok(())
    }

    /// QI-BB-020: retention forgets unretained auxiliary generations.
    ///
    /// Sealing a generation under a retention receipt forgets the
    /// auxiliary generations older than it that the receipt does not
    /// retain — in memory and in the catalog — and leaves the retained and
    /// the just-sealed ones.
    #[test]
    fn retention_forgets_auxiliary_generations_the_receipt_does_not_retain() -> TestRes {
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let (aux, catalog) = aux_parts(Arc::clone(&ledger));
        let history = DirectHistoryMaterializer::new(aux.clone());
        for generation in [3, 4] {
            let _receipt = history.publish_batch(&fixture_history_batch(
                generation,
                vec![fixture_commit(1, &[])],
            ))?;
        }
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
            Arc::new(DirectSemanticMaterializer::new(
                Arc::new(FakeSemanticBuilder::default()),
                Arc::new(RwLock::new(Ledger::new())),
            ));
        let materializer = DirectSearchCorpusMaterializer::new_with_search_owned_semantics(
            SearchCorpusMaterializerParts {
                builder: Arc::new(FakeSearchCorpusBuilder::default()),
                ledger: Arc::clone(&ledger),
                semantic_ingest: semantic_materializer,
                semantic_embedder: Arc::new(crate::HashingQueryTextEmbedder::new(
                    SEARCH_OWNED_SEMANTIC_DIMENSION,
                )),
                authority: recording_search_corpus_authority(),
                lexical_generation_validator: always_valid_generation(),
                semantic_generation_validator: always_valid_generation(),
                lexical_incomplete_discard: test_incomplete_generation_discard(),
                semantic_incomplete_discard: test_incomplete_generation_discard(),
                lexical_reclaim: no_storage_sealed_reclaim(),
                semantic_reclaim: no_storage_sealed_reclaim(),
                snapshots: SnapshotRegistries::new(crate::SnapshotRegistryPolicy::DEFAULT),
                idempotency: memory_catalog(),
                resource_policy: IngestResourcePolicy::DEFAULT,
                auxiliary_catalog: catalog.clone(),
                auxiliary_coordinator: aux.coordinator,
            },
        );
        let mut batch = fixture_search_corpus_batch()?;
        batch.generation = ManifestGeneration::new(5);
        let receipt = SearchCorpusHistoryRetentionReceiptV1::retaining_generations_v1(
            &batch.repo_id,
            &batch.revision_id,
            [ManifestGeneration::new(4), ManifestGeneration::new(5)],
        );
        materializer.finalize_generation_v1(&batch, Some(&receipt))?;

        let guard = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        let forgotten = guard
            .history_state(
                &batch.repo_id,
                &batch.revision_id,
                ManifestGeneration::new(3),
            )
            .is_none();
        let retained = guard
            .history_state(
                &batch.repo_id,
                &batch.revision_id,
                ManifestGeneration::new(4),
            )
            .is_some();
        let sealed_chunks = guard
            .structural_state(
                &batch.repo_id,
                &batch.revision_id,
                ManifestGeneration::new(5),
            )
            .map(|state| state.chunks().len());
        drop(guard);
        if !forgotten || !retained || sealed_chunks != Some(1) {
            return Err(format!(
                "ledger drifted: forgotten={forgotten} retained={retained} sealed_chunks={sealed_chunks:?}"
            )
            .into());
        }
        let generations = catalog.generations();
        if generations != BTreeSet::from([4, 5]) {
            return Err(format!(
                "catalog must hold exactly the retained generations, holds {generations:?}"
            )
            .into());
        }
        Ok(())
    }

    /// An auxiliary receipt names its batch digest and no manifest
    /// (QI-BB-032): the two identities are distinct fields.
    #[test]
    fn direct_dirty_materializer_names_the_batch_digest_and_no_manifest() -> TestRes {
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let (parts, _catalog) = aux_parts(Arc::clone(&ledger));
        let materializer = DirectRuntimeMetadataMaterializer::new(parts);
        let batch = fixture_dirty_batch();
        let receipt = materializer.publish_batch(&batch)?;
        if receipt.manifest_digest.is_some()
            || receipt.batch_digest != batch.batch_digest
            || receipt.generation != batch.generation
            || receipt.accepted_replace_scopes != 1
            || receipt.accepted_tombstone_scopes != 0
            || receipt.sealed
        {
            return Err(format!(
                "unexpected dirty materialize receipt: batch={batch:?} receipt={receipt:?}"
            )
            .into());
        }
        Ok(())
    }

    #[test]
    fn search_corpus_materializer_derives_search_owned_semantic_batch() -> TestRes {
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let semantic_ledger = Arc::new(RwLock::new(Ledger::new()));
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
            DirectSemanticMaterializer::new(semantic_builder.clone(), Arc::clone(&semantic_ledger)),
        );
        let search_corpus_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let lexical_ledger = Arc::new(RwLock::new(Ledger::new()));
        let authority = Arc::new(RecordingSearchCorpusAuthority::default());
        let materializer = search_corpus_materializer!(
            search_corpus_builder,
            Arc::clone(&lexical_ledger),
            semantic_materializer,
            Arc::new(crate::HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
            authority.clone(),
            build_then_valid_generation(),
            build_then_valid_generation(),
            test_incomplete_generation_discard(),
            test_incomplete_generation_discard(),
        );
        let mut batch = fixture_search_corpus_batch()?;
        batch.clear_surfaces = vec![SearchScopeSurface::Symbol];
        let receipt = materializer.publish_batch(&batch)?;
        if !receipt.sealed || receipt.accepted_clear_surfaces != 1 {
            return Err("derived semantic search-corpus receipt must preserve seal".into());
        }
        let semantic_batches = semantic_builder.take()?;
        let derived = semantic_batches
            .first()
            .ok_or_else(|| "expected one derived semantic batch".to_string())?;
        if derived.generation != batch.generation || !derived.seal {
            return Err("derived semantic batch lost generation/seal truth".into());
        }
        if derived.clear_surfaces != [SearchScopeSurface::Symbol] {
            return Err("derived semantic batch lost clear-surface truth".into());
        }
        if derived.replace_scopes.len() != 1 {
            return Err(
                "derived semantic batch did not mirror search-corpus scope/chunk count".into(),
            );
        }
        let scope = derived
            .replace_scopes
            .first()
            .ok_or_else(|| "derived semantic batch missing replace scope".to_string())?;
        if scope.embeddings.len() != 1 {
            return Err("derived semantic batch did not mirror lexical scope/chunk count".into());
        }
        let embedding = scope
            .embeddings
            .first()
            .ok_or_else(|| "derived semantic batch missing embedding".to_string())?;
        if embedding.embedding_id.as_str() != "chunk-1" {
            return Err("derived semantic embedding_id must equal chunk_id".into());
        }
        if !embedding
            .embedding_input_digest
            .starts_with("search-owned-in:sha256:")
        {
            return Err(format!(
                "input digest must be content-hash based, got {}",
                embedding.embedding_input_digest
            )
            .into());
        }
        if !embedding
            .vector_digest
            .starts_with("search-owned-vec:sha256:")
        {
            return Err(format!(
                "vector digest must be vector-hash based, got {}",
                embedding.vector_digest
            )
            .into());
        }
        let recorded = authority
            .identities
            .lock()
            .map_err(|err| format!("recording authority poisoned: {err}"))?;
        if recorded.as_slice()
            != [(
                batch.repo_id.clone(),
                batch.revision_id.clone(),
                batch.generation,
                batch.manifest_digest,
            )]
        {
            return Err(
                format!("sealed composite authority was not recorded: {recorded:?}").into(),
            );
        }
        drop(recorded);
        Ok(())
    }

    #[test]
    fn sealed_exact_retry_repairs_authority_without_rebuilding_tracks() -> TestRes {
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
            Arc::new(DirectSemanticMaterializer::new(
                semantic_builder.clone(),
                Arc::new(RwLock::new(Ledger::new())),
            ));
        let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let authority = Arc::new(RecordingSearchCorpusAuthority {
            identities: Mutex::new(Vec::new()),
            exact: true,
        });
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let materializer = search_corpus_materializer!(
            lexical_builder.clone(),
            Arc::clone(&ledger),
            semantic_materializer,
            Arc::new(crate::HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
            authority.clone(),
            always_valid_generation(),
            always_valid_generation(),
            test_incomplete_generation_discard(),
            test_incomplete_generation_discard(),
        );
        let batch = fixture_search_corpus_batch()?;
        let receipt = materializer.publish_batch(&batch)?;
        assert!(receipt.sealed);
        assert!(
            lexical_builder
                .batches
                .lock()
                .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
                .is_empty()
        );
        assert!(semantic_builder.take()?.is_empty());
        assert_eq!(
            authority
                .identities
                .lock()
                .map_err(|err| format!("recording authority poisoned: {err}"))?
                .len(),
            1
        );
        let guard = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?;
        guard.validate_historically_sealed_track_identity(
            &generation_pair_from_batch_v1(&batch).0,
            "test exact retry",
        )?;
        drop(guard);
        Ok(())
    }

    #[test]
    fn durable_retention_error_fences_same_process_rollback_authority() -> TestRes {
        let batch = fixture_search_corpus_batch()?;
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        ledger
            .write()
            .map_err(|err| format!("ledger poisoned: {err}"))?
            .record_historically_sealed_search_corpus(
                &batch.repo_id,
                &batch.revision_id,
                batch.generation,
                &batch.manifest_digest,
            );
        let materializer = search_corpus_materializer!(
            Arc::new(FakeSearchCorpusBuilder::default()),
            Arc::clone(&ledger),
            Arc::new(DirectSemanticMaterializer::new(
                Arc::new(FakeSemanticBuilder::default()),
                Arc::new(RwLock::new(Ledger::new())),
            )),
            Arc::new(crate::HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
            Arc::new(FailingRetentionAuthority),
            always_valid_generation(),
            always_valid_generation(),
            test_incomplete_generation_discard(),
            test_incomplete_generation_discard(),
        );
        assert!(matches!(
            materializer.publish_batch(&batch),
            Err(CoreError::Storage(_))
        ));
        let rollback = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?
            .validate_historically_sealed_track_identity(
                &generation_pair_from_batch_v1(&batch).0,
                "retention failure",
            );
        assert!(matches!(rollback, Err(CoreError::NotReady(_))));
        Ok(())
    }

    #[test]
    fn non_seal_batch_cannot_mutate_an_already_sealed_generation() -> TestRes {
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
            Arc::new(DirectSemanticMaterializer::new(
                semantic_builder.clone(),
                Arc::new(RwLock::new(Ledger::new())),
            ));
        let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let materializer = search_corpus_materializer!(
            lexical_builder.clone(),
            Arc::new(RwLock::new(Ledger::new())),
            semantic_materializer,
            Arc::new(crate::HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
            recording_search_corpus_authority(),
            always_valid_generation(),
            always_valid_generation(),
            test_incomplete_generation_discard(),
            test_incomplete_generation_discard(),
        );
        let mut batch = fixture_search_corpus_batch()?;
        batch.seal = false;
        let result = materializer.publish_batch(&batch);
        let Err(CoreError::Typed { code, .. }) = result else {
            return Err("non-seal mutation of sealed generation unexpectedly succeeded".into());
        };
        assert_eq!(code, "GENERATION_IMMUTABLE");
        assert!(
            lexical_builder
                .batches
                .lock()
                .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
                .is_empty()
        );
        assert!(semantic_builder.take()?.is_empty());
        Ok(())
    }

    #[test]
    fn exact_lexical_missing_semantic_retry_builds_only_missing_track() -> TestRes {
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
            Arc::new(DirectSemanticMaterializer::new(
                semantic_builder.clone(),
                Arc::new(RwLock::new(Ledger::new())),
            ));
        let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let materializer = search_corpus_materializer!(
            lexical_builder.clone(),
            Arc::new(RwLock::new(Ledger::new())),
            semantic_materializer,
            Arc::new(crate::HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
            recording_search_corpus_authority(),
            always_valid_generation(),
            build_then_valid_generation(),
            test_incomplete_generation_discard(),
            test_incomplete_generation_discard(),
        );
        let receipt = materializer.publish_batch(&fixture_search_corpus_batch()?)?;
        assert!(receipt.sealed);
        assert!(
            lexical_builder
                .batches
                .lock()
                .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
                .is_empty()
        );
        assert_eq!(semantic_builder.take()?.len(), 1);
        Ok(())
    }

    #[test]
    fn incomplete_lexical_exact_semantic_retry_discards_and_rebuilds_only_lexical() -> TestRes {
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
            Arc::new(DirectSemanticMaterializer::new(
                semantic_builder.clone(),
                Arc::new(RwLock::new(Ledger::new())),
            ));
        let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let lexical_discard = Arc::new(RecordingIncompleteGenerationDiscard::default());
        let materializer = search_corpus_materializer!(
            lexical_builder.clone(),
            Arc::new(RwLock::new(Ledger::new())),
            semantic_materializer,
            Arc::new(crate::HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
            recording_search_corpus_authority(),
            incomplete_then_valid_generation(),
            always_valid_generation(),
            lexical_discard.clone(),
            test_incomplete_generation_discard(),
        );

        let receipt = materializer.publish_batch(&fixture_search_corpus_batch()?)?;
        assert!(receipt.sealed);
        assert_eq!(lexical_discard.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            lexical_builder
                .batches
                .lock()
                .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
                .len(),
            1
        );
        assert!(semantic_builder.take()?.is_empty());
        Ok(())
    }

    #[test]
    fn jointly_incomplete_tracks_keep_staged_data_for_normal_seal() -> TestRes {
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> =
            Arc::new(DirectSemanticMaterializer::new(
                semantic_builder.clone(),
                Arc::new(RwLock::new(Ledger::new())),
            ));
        let lexical_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let lexical_discard = Arc::new(RecordingIncompleteGenerationDiscard::default());
        let semantic_discard = Arc::new(RecordingIncompleteGenerationDiscard::default());
        let materializer = search_corpus_materializer!(
            lexical_builder.clone(),
            Arc::new(RwLock::new(Ledger::new())),
            semantic_materializer,
            Arc::new(crate::HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
            recording_search_corpus_authority(),
            incomplete_then_valid_generation(),
            incomplete_then_valid_generation(),
            lexical_discard.clone(),
            semantic_discard.clone(),
        );

        let receipt = materializer.publish_batch(&fixture_search_corpus_batch()?)?;

        assert!(receipt.sealed);
        assert_eq!(lexical_discard.calls.load(Ordering::SeqCst), 0);
        assert_eq!(semantic_discard.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            lexical_builder
                .batches
                .lock()
                .map_err(|err| format!("fake lexical builder poisoned: {err}"))?
                .len(),
            1
        );
        assert_eq!(semantic_builder.take()?.len(), 1);
        Ok(())
    }

    #[test]
    fn search_corpus_materializer_rejects_mismatched_semantic_receipt_before_authority_admission()
    -> TestRes {
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let authority = Arc::new(RecordingSearchCorpusAuthority::default());
        let materializer = search_corpus_materializer!(
            Arc::new(FakeSearchCorpusBuilder::default()),
            Arc::clone(&ledger),
            Arc::new(MismatchedSemanticIngest),
            Arc::new(crate::HashingQueryTextEmbedder::new(
                SEARCH_OWNED_SEMANTIC_DIMENSION,
            )),
            authority.clone(),
            build_then_valid_generation(),
            build_then_valid_generation(),
            test_incomplete_generation_discard(),
            test_incomplete_generation_discard(),
        );
        let batch = fixture_search_corpus_batch()?;
        let result = materializer.publish_batch(&batch);
        assert!(matches!(result, Err(CoreError::InvalidContract(_))));
        assert!(
            authority
                .identities
                .lock()
                .map_err(|err| format!("recording authority poisoned: {err}"))?
                .is_empty()
        );
        let historical = ledger
            .read()
            .map_err(|err| format!("ledger poisoned: {err}"))?
            .validate_historically_sealed_track_identity(
                &quanta_index_contract::GenerationSnapshot {
                    repo_id: batch.repo_id.clone(),
                    revision_id: batch.revision_id.clone(),
                    track: SearchPlaneTrackKind::Lexical,
                    manifest_generation: batch.generation,
                    manifest_digest: batch.manifest_digest,
                },
                "test",
            );
        assert!(matches!(historical, Err(CoreError::Typed { .. })));
        Ok(())
    }

    struct FixedFakeEmbedder {
        dimension: usize,
        vectors_per_call: usize,
        vector_len: usize,
    }

    impl TextEmbeddingProvider for FixedFakeEmbedder {
        fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
            Ok((0..self.vectors_per_call)
                .map(|_| vec![0.0_f32; self.vector_len])
                .collect())
        }
        fn model_id(&self) -> &'static str {
            "fake-embedder"
        }
        fn model_revision(&self) -> &'static str {
            "r1"
        }
        fn dimension(&self) -> usize {
            self.dimension
        }
        fn normalization(&self) -> EmbeddingNormalization {
            EmbeddingNormalization::L2Unit
        }
    }

    fn materializer_with_embedder(
        embedder: Arc<dyn TextEmbeddingProvider + Send + Sync>,
    ) -> DirectSearchCorpusMaterializer {
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let semantic_ledger = Arc::new(RwLock::new(Ledger::new()));
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
            DirectSemanticMaterializer::new(semantic_builder, Arc::clone(&semantic_ledger)),
        );
        let search_corpus_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let lexical_ledger = Arc::new(RwLock::new(Ledger::new()));
        search_corpus_materializer!(
            search_corpus_builder,
            lexical_ledger,
            semantic_materializer,
            embedder,
            recording_search_corpus_authority(),
            build_then_valid_generation(),
            build_then_valid_generation(),
            test_incomplete_generation_discard(),
            test_incomplete_generation_discard(),
        )
    }

    // CASE-COVERS: corpus derivation fails closed when the embedder returns the
    // wrong number of vectors — a misaligned batch must never reach the index.
    #[test]
    fn search_corpus_derivation_fails_closed_on_embedder_count_mismatch() -> TestRes {
        let materializer = materializer_with_embedder(Arc::new(FixedFakeEmbedder {
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
            vectors_per_call: 0,
            vector_len: SEARCH_OWNED_SEMANTIC_DIMENSION,
        }));
        let batch = fixture_search_corpus_batch()?;
        match materializer.publish_batch(&batch) {
            Err(CoreError::InvalidContract(message)) => {
                if !message.contains("vectors for") {
                    return Err(format!("unexpected count-mismatch message: {message}").into());
                }
            }
            other => return Err(format!("count mismatch must fail closed, got {other:?}").into()),
        }
        Ok(())
    }

    // CASE-COVERS: corpus derivation fails closed when a returned vector has the
    // wrong dimension — would corrupt the lancedb fixed-size-list schema.
    #[test]
    fn search_corpus_derivation_fails_closed_on_embedder_dim_mismatch() -> TestRes {
        let materializer = materializer_with_embedder(Arc::new(FixedFakeEmbedder {
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
            vectors_per_call: 1,
            vector_len: SEARCH_OWNED_SEMANTIC_DIMENSION + 1,
        }));
        let batch = fixture_search_corpus_batch()?;
        match materializer.publish_batch(&batch) {
            Err(CoreError::InvalidContract(message)) => {
                if !message.contains("returned dim") {
                    return Err(format!("unexpected dim-mismatch message: {message}").into());
                }
            }
            other => return Err(format!("dim mismatch must fail closed, got {other:?}").into()),
        }
        Ok(())
    }

    fn chunk_record_v(
        id: &str,
        path: &str,
        text: &str,
    ) -> Result<ChunkRecord, Box<dyn std::error::Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new(id),
            repo_relative_path: RepoRelativePath::new(path),
            language: quanta_index_contract::lex::LanguageCode::new("rust")
                .map_err(str::to_string)?,
            start_byte: 0,
            end_byte: 24,
            start_line: 1,
            end_line: 1,
            text: text.to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        })
    }

    fn scope_with_chunks(
        path: &str,
        digest: &str,
        chunks: Vec<ChunkRecord>,
    ) -> SearchCorpusReplaceScope {
        SearchCorpusReplaceScope {
            scope: SearchScopeKey {
                doc_surface: SearchScopeSurface::Chunk,
                repo_relative_path: RepoRelativePath::new(path),
            },
            scope_digest: digest.to_string(),
            chunks,
            symbols: Vec::new(),
        }
    }

    // A batch spanning 3 scopes with 2 / 1 / 2 chunks = 5 chunk texts in total.
    fn multi_scope_corpus_batch() -> Result<SearchCorpusIngestBatch, Box<dyn std::error::Error>> {
        Ok(SearchCorpusIngestBatch {
            repo_id: RepoId::new("r"),
            revision_id: RevisionId::new("rev"),
            generation: ManifestGeneration::new(7),
            base_generation: None,
            manifest_digest: "manifest:lex".to_string(),
            batch_digest: "batch:lex".to_string(),
            mode: BatchIngestMode::ReplaceGeneration,
            bundle_payload: None,
            clear_surfaces: Vec::new(),
            replace_scopes: vec![
                scope_with_chunks(
                    "a.rs",
                    "scope:a",
                    vec![
                        chunk_record_v("a-1", "a.rs", "alpha one")?,
                        chunk_record_v("a-2", "a.rs", "alpha two")?,
                    ],
                ),
                scope_with_chunks(
                    "b.rs",
                    "scope:b",
                    vec![chunk_record_v("b-1", "b.rs", "beta one")?],
                ),
                scope_with_chunks(
                    "c.rs",
                    "scope:c",
                    vec![
                        chunk_record_v("c-1", "c.rs", "gamma one")?,
                        chunk_record_v("c-2", "c.rs", "gamma two")?,
                    ],
                ),
            ],
            tombstone_scopes: Vec::new(),
            semantic_replace_scopes: Vec::new(),
            semantic_tombstone_scopes: Vec::new(),
            seal: true,
        })
    }

    // An embedder that counts embed_batch calls and returns one zero vector per
    // input text, so a test can assert how many provider round trips a batch costs.
    struct CountingEmbedder {
        dimension: usize,
        calls: Mutex<usize>,
    }

    impl TextEmbeddingProvider for CountingEmbedder {
        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
            {
                let mut calls = self.calls.lock().map_err(|err| {
                    CoreError::InvalidContract(format!("counting embedder lock poisoned: {err}"))
                })?;
                *calls = calls.checked_add(1).ok_or_else(|| {
                    CoreError::InvalidContract(
                        "counting embedder call counter overflow".to_string(),
                    )
                })?;
            }
            Ok(texts
                .iter()
                .map(|_| vec![0.0_f32; self.dimension])
                .collect())
        }
        fn model_id(&self) -> &'static str {
            "counting-embedder"
        }
        fn model_revision(&self) -> &'static str {
            "r1"
        }
        fn dimension(&self) -> usize {
            self.dimension
        }
        fn normalization(&self) -> EmbeddingNormalization {
            EmbeddingNormalization::L2Unit
        }
    }

    // CASE-COVERS: a multi-scope ingest batch is embedded in ONE provider call
    // (not one per scope/file), and the flat vectors are redistributed back to
    // each scope's chunks IN ORDER. Reverting the derivation to a per-scope embed
    // makes the call-count assertion fail; misaligning the redistribution makes
    // the chunk-id-order assertion fail.
    #[test]
    fn corpus_derivation_batches_all_scopes_into_one_embed_call() -> TestRes {
        let embedder = Arc::new(CountingEmbedder {
            dimension: SEARCH_OWNED_SEMANTIC_DIMENSION,
            calls: Mutex::new(0),
        });
        let semantic_builder = Arc::new(FakeSemanticBuilder::default());
        let semantic_ledger = Arc::new(RwLock::new(Ledger::new()));
        let semantic_materializer: Arc<dyn SemanticIngestPort + Send + Sync> = Arc::new(
            DirectSemanticMaterializer::new(semantic_builder.clone(), Arc::clone(&semantic_ledger)),
        );
        let search_corpus_builder = Arc::new(FakeSearchCorpusBuilder::default());
        let lexical_ledger = Arc::new(RwLock::new(Ledger::new()));
        let materializer = search_corpus_materializer!(
            search_corpus_builder,
            lexical_ledger,
            semantic_materializer,
            embedder.clone(),
            recording_search_corpus_authority(),
            build_then_valid_generation(),
            build_then_valid_generation(),
            test_incomplete_generation_discard(),
            test_incomplete_generation_discard(),
        );

        let batch = multi_scope_corpus_batch()?;
        let _receipt = materializer.publish_batch(&batch)?;

        // (1) The whole 3-scope / 5-chunk batch costs exactly ONE embed call.
        let calls = *embedder
            .calls
            .lock()
            .map_err(|err| format!("counting embedder lock poisoned: {err}"))?;
        if calls != 1 {
            return Err(format!(
                "expected ONE batched embed call for the whole batch, got {calls} (per-scope regression)"
            )
            .into());
        }

        // (2) Vectors redistributed back to scopes with chunk counts + order intact.
        let derived = semantic_builder.take()?;
        let derived_batch = derived
            .first()
            .ok_or_else(|| "expected one derived semantic batch".to_string())?;
        let per_scope_counts: Vec<usize> = derived_batch
            .replace_scopes
            .iter()
            .map(|scope| scope.embeddings.len())
            .collect();
        if per_scope_counts != vec![2, 1, 2] {
            return Err(format!(
                "scope->chunk redistribution wrong: {per_scope_counts:?}, expected [2, 1, 2]"
            )
            .into());
        }
        let ids: Vec<&str> = derived_batch
            .replace_scopes
            .iter()
            .flat_map(|scope| {
                scope
                    .embeddings
                    .iter()
                    .map(|record| record.embedding_id.as_str())
            })
            .collect();
        if ids != vec!["a-1", "a-2", "b-1", "c-1", "c-2"] {
            return Err(format!("chunk<->vector alignment lost across scopes: {ids:?}").into());
        }
        Ok(())
    }

    #[test]
    fn semantic_digests_change_when_input_or_vector_changes() -> TestRes {
        let model = fixture_model_contract();
        let chunk_a = fixture_chunk_record()?;
        let mut chunk_b = fixture_chunk_record()?;
        chunk_b.text = "typed semantic parser with different body"
            .to_string()
            .into_boxed_str();
        let input_a = semantic_embedding_input_digest(
            &model,
            "chunk.text",
            semantic_embedding_input_text(&chunk_a),
        );
        let input_b = semantic_embedding_input_digest(
            &model,
            "chunk.text",
            semantic_embedding_input_text(&chunk_b),
        );
        if input_a == input_b {
            return Err("input digest must change when embedding input text changes".into());
        }
        let vec_a = semantic_vector_digest(&model, &[0.1, 0.2, 0.3]);
        let vec_b = semantic_vector_digest(&model, &[0.1, 0.2, 0.4]);
        if vec_a == vec_b {
            return Err("vector digest must change when vector contents change".into());
        }
        if !input_a.starts_with("search-owned-in:sha256:") {
            return Err(format!("unexpected input digest format: {input_a}").into());
        }
        if !vec_a.starts_with("search-owned-vec:sha256:") {
            return Err(format!("unexpected vector digest format: {vec_a}").into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod idempotency_tests {
    //! QI-BB-032 — every receipt-bearing route runs under its idempotency
    //! record, proven at the dispatcher with one live route and the rest
    //! unreachable.

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use quanta_index_contract::lex::DirtyRecord;
    use quanta_index_contract::{
        BatchPublishReceipt, ChunkId, DirtyIngestBatch, DirtyMutation, FileContributorIngestBatch,
        FileOwnershipIngestBatch, HistoryIngestBatch, ManifestGeneration,
        RepoCommitRecencyIngestBatch, RepoDescriptionIngestBatch, RepoId, RepoMapSourceBundle,
        RepoMetaIngestBatch, RepoTopicIngestBatch, RevisionId, RuntimeCatalogIngestBatch,
        SearchCorpusIngestBatch, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse,
        StructuralIngestBatch,
    };
    use quanta_index_core::{
        BATCH_DIGEST_CONFLICT_CODE, CoreError, FileContributorIngestPort, FileOwnershipIngestPort,
        RepoCommitRecencyIngestPort, RepoDescriptionIngestPort, RepoMapBundleIngestPort,
        RepoMetaIngestPort, RepoTopicIngestPort, RequestBudgetV1, SearchCorpusIngestPort,
    };

    use super::tests::MemoryIdempotencyCatalog;
    use super::{
        HistoryIngestPort, RuntimeMetadataIngestPort, SearchPlaneIngestDispatcher,
        StructuralIngestPort, dirty_publish_receipt_v1,
    };
    use crate::{SnapshotRegistries, SnapshotRegistryPolicy};

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    /// A route the test never expects to reach.
    struct Unreachable;

    fn unreachable_route(route: &str) -> CoreError {
        CoreError::Storage(format!("test: {route} route must not be reached"))
    }

    impl SearchCorpusIngestPort for Unreachable {
        fn publish_batch(
            &self,
            _batch: &SearchCorpusIngestBatch,
        ) -> Result<BatchPublishReceipt, CoreError> {
            Err(unreachable_route("search corpus"))
        }
    }
    impl HistoryIngestPort for Unreachable {
        fn publish_batch(
            &self,
            _batch: &HistoryIngestBatch,
        ) -> Result<BatchPublishReceipt, CoreError> {
            Err(unreachable_route("history"))
        }
    }
    impl RepoCommitRecencyIngestPort for Unreachable {
        fn publish_batch(
            &self,
            _batch: &RepoCommitRecencyIngestBatch,
        ) -> Result<BatchPublishReceipt, CoreError> {
            Err(unreachable_route("repo commit recency"))
        }
    }
    impl RepoTopicIngestPort for Unreachable {
        fn publish_batch(
            &self,
            _batch: &RepoTopicIngestBatch,
        ) -> Result<BatchPublishReceipt, CoreError> {
            Err(unreachable_route("repo topic"))
        }
    }
    impl RepoDescriptionIngestPort for Unreachable {
        fn publish_batch(
            &self,
            _batch: &RepoDescriptionIngestBatch,
        ) -> Result<BatchPublishReceipt, CoreError> {
            Err(unreachable_route("repo description"))
        }
    }
    impl FileOwnershipIngestPort for Unreachable {
        fn publish_batch(
            &self,
            _batch: &FileOwnershipIngestBatch,
        ) -> Result<BatchPublishReceipt, CoreError> {
            Err(unreachable_route("file ownership"))
        }
    }
    impl FileContributorIngestPort for Unreachable {
        fn publish_batch(
            &self,
            _batch: &FileContributorIngestBatch,
        ) -> Result<BatchPublishReceipt, CoreError> {
            Err(unreachable_route("file contributor"))
        }
    }
    impl RepoMetaIngestPort for Unreachable {
        fn publish_batch(
            &self,
            _batch: &RepoMetaIngestBatch,
        ) -> Result<BatchPublishReceipt, CoreError> {
            Err(unreachable_route("repo meta"))
        }
    }
    impl StructuralIngestPort for Unreachable {
        fn publish_batch(
            &self,
            _batch: &StructuralIngestBatch,
        ) -> Result<BatchPublishReceipt, CoreError> {
            Err(unreachable_route("structural"))
        }
    }
    impl RepoMapBundleIngestPort for Unreachable {
        fn ingest_bundle(&self, _bundle: &RepoMapSourceBundle) -> Result<(), CoreError> {
            Err(unreachable_route("repo map bundle"))
        }
    }

    /// The one live route: counts applies and answers the dirty receipt.
    struct CountingRuntime {
        applies: AtomicUsize,
    }

    impl RuntimeMetadataIngestPort for CountingRuntime {
        fn publish_batch(
            &self,
            batch: &DirtyIngestBatch,
        ) -> Result<BatchPublishReceipt, CoreError> {
            let _prior = self.applies.fetch_add(1, Ordering::SeqCst);
            Ok(dirty_publish_receipt_v1(batch))
        }
        fn publish_catalog_batch(
            &self,
            _batch: &RuntimeCatalogIngestBatch,
        ) -> Result<BatchPublishReceipt, CoreError> {
            Err(unreachable_route("runtime catalog"))
        }
    }

    fn dispatcher(
        runtime: Arc<CountingRuntime>,
        catalog: Arc<MemoryIdempotencyCatalog>,
    ) -> SearchPlaneIngestDispatcher {
        let unreachable = Arc::new(Unreachable);
        SearchPlaneIngestDispatcher::new(
            unreachable.clone(),
            unreachable.clone(),
            unreachable.clone(),
            unreachable.clone(),
            unreachable.clone(),
            unreachable.clone(),
            unreachable.clone(),
            unreachable.clone(),
            runtime,
            unreachable.clone(),
            unreachable,
            SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
            catalog,
        )
    }

    fn dirty_batch(digest: &str, doc: &str) -> DirtyIngestBatch {
        DirtyIngestBatch {
            repo_id: RepoId::new("repo-idem"),
            revision_id: RevisionId::new("rev-idem"),
            generation: ManifestGeneration::new(4),
            overlay_epoch_ms: 11,
            batch_digest: digest.to_string(),
            entries: vec![DirtyMutation::Upsert(DirtyRecord {
                wire_version: 1,
                doc_id: ChunkId::new(doc),
                applied_at_ms: 11,
                payload_hash: [3; 32],
            })],
        }
    }

    fn typed_code_of(response: &SearchPlaneIngestIpcResponse) -> Option<String> {
        match response {
            SearchPlaneIngestIpcResponse::Error(error) => Some(error.code.clone()),
            SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::DirtyReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_) => None,
        }
    }

    fn receipt_of(response: SearchPlaneIngestIpcResponse) -> Result<BatchPublishReceipt, String> {
        match response {
            SearchPlaneIngestIpcResponse::DirtyReceipt(receipt) => Ok(receipt),
            SearchPlaneIngestIpcResponse::Error(error) => {
                Err(format!("{}: {}", error.code, error.message))
            }
            other @ (SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
            | SearchPlaneIngestIpcResponse::HistoryReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoTopicReceipt(_)
            | SearchPlaneIngestIpcResponse::FileOwnershipReceipt(_)
            | SearchPlaneIngestIpcResponse::FileContributorReceipt(_)
            | SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(_)
            | SearchPlaneIngestIpcResponse::StructuralReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMapReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoMetaReceipt(_)
            | SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(_)) => {
                Err(format!("unexpected response {other:?}"))
            }
        }
    }

    /// The same body twice applies once; a different body is refused.
    ///
    /// The second receipt is the first one's counts and sequence with
    /// `applied = false`, and the route was not reached again. A different
    /// body under the same key is refused typed and the route is not reached
    /// at all.
    #[test]
    fn a_replay_is_answered_from_the_record_and_a_conflict_is_refused() -> TestRes {
        let runtime = Arc::new(CountingRuntime {
            applies: AtomicUsize::new(0),
        });
        let catalog = Arc::new(MemoryIdempotencyCatalog::default());
        let dispatcher = dispatcher(Arc::clone(&runtime), Arc::clone(&catalog));
        let budget = RequestBudgetV1::unbounded();

        let first = receipt_of(dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(dirty_batch("d1", "src/a.rs")),
            &budget,
        ))?;
        if !first.applied || first.durable_sequence != 1 || first.batch_digest != "d1" {
            return Err(format!("first publish must apply at sequence 1: {first:?}").into());
        }
        let replay = receipt_of(dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(dirty_batch("d1", "src/a.rs")),
            &budget,
        ))?;
        if replay.applied || replay.durable_sequence != 1 {
            return Err(
                format!("replay must be the recorded apply, not a new one: {replay:?}").into(),
            );
        }
        if replay.accepted_replace_scopes != first.accepted_replace_scopes
            || replay.generation != first.generation
        {
            return Err("replay counts must be the original apply's".into());
        }
        if runtime.applies.load(Ordering::SeqCst) != 1 {
            return Err(format!(
                "the route ran {} times for one body",
                runtime.applies.load(Ordering::SeqCst)
            )
            .into());
        }

        let conflict = dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(dirty_batch("d1", "src/b.rs")),
            &budget,
        );
        match typed_code_of(&conflict) {
            Some(code) if code == BATCH_DIGEST_CONFLICT_CODE => {}
            other => {
                return Err(format!(
                    "a different body under the same digest must be refused typed, got {other:?}"
                )
                .into());
            }
        }
        if runtime.applies.load(Ordering::SeqCst) != 1 {
            return Err("a conflicting body must not reach the route".into());
        }
        if catalog.records() != 1 {
            return Err(
                format!("one key must hold one record, found {}", catalog.records()).into(),
            );
        }

        let second = receipt_of(dispatcher.dispatch(
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(dirty_batch("d2", "src/b.rs")),
            &budget,
        ))?;
        if !second.applied || second.durable_sequence != 2 {
            return Err(format!("a new key applies at the next sequence: {second:?}").into());
        }
        Ok(())
    }
}
