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

use std::sync::{Arc, Mutex, RwLock};

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
    SnapshotRegistries,
};
use quanta_index_contract::{
    BatchPublishReceipt, DirtyIngestBatch, DirtyMutation, GenerationSnapshot, HistoryIngestBatch,
    HistoryRefMutation, ManifestGeneration, RepoId, RepoMapMutationAck, RevisionId,
    RuntimeCatalogIngestBatch, SearchCorpusIngestBatch, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse, SearchPlaneIpcError, SearchPlaneTrackKind, SemanticIngestBatch,
    StructuralIngestBatch,
};
use quanta_index_core::{
    CoreError, FileContributorIngestPort, FileOwnershipIngestPort, GenerationIdentityValidatePort,
    IncompleteGenerationDiscardOutcomeV1, IncompleteGenerationDiscardPort,
    RepoCommitRecencyIngestPort, RepoDescriptionIngestPort, RepoMapBundleIngestPort,
    RepoMetaIngestPort, RepoTopicIngestPort, SearchCorpusBatchBuildPort, SearchCorpusIngestPort,
    SemanticBatchBuildPort, SemanticIngestPort, TextEmbeddingProvider,
};

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";
const ERR_SEARCH_CORPUS_GENERATION_CONFLICT: &str = "SEARCH_CORPUS_GENERATION_CONFLICT";

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
    operation_locks: [Mutex<()>; SEARCH_CORPUS_LOCK_STRIPES_V1],
    semantic_derivation_mode: SemanticDerivationModeV1,
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
            operation_locks: std::array::from_fn(|_index| Mutex::new(())),
            semantic_derivation_mode,
        }
    }
}

impl SearchCorpusIngestPort for DirectSearchCorpusMaterializer {
    fn publish_batch(
        &self,
        batch: &SearchCorpusIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        batch.validate_surface_mutations_v1().map_err(|err| {
            CoreError::InvalidContract(format!("direct search-corpus materialize: {err}"))
        })?;
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
        {
            let mut guard = self.ledger.write().map_err(|err| {
                CoreError::Storage(format!(
                    "direct search-corpus materialize: ledger poisoned while finalizing generation: {err}"
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
            guard.apply_search_corpus_batch(batch);
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
        }
        Ok(())
    }
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
    let mut receipt =
        BatchPublishReceipt::empty_for(batch.generation, batch.manifest_digest.clone());
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
        || receipt.manifest_digest != batch.manifest_digest
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
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.manifest_digest.clone());
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

/// Direct history materializer. History is auxiliary and non-activation
/// blocking, but direct ledger updates keep query truth aligned with accepted
/// ingest batches.
pub struct DirectHistoryMaterializer {
    authority_store: Arc<AuxiliaryAuthorityStore>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectHistoryMaterializer {
    #[must_use]
    pub fn new(authority_store: Arc<AuxiliaryAuthorityStore>, ledger: Arc<RwLock<Ledger>>) -> Self {
        Self {
            authority_store,
            ledger,
        }
    }
}

impl HistoryIngestPort for DirectHistoryMaterializer {
    fn publish_batch(&self, batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        let mut receipt = BatchPublishReceipt::empty_for(batch.generation, String::new());
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct history materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.apply_history_batch(batch)?;
        for _record in &batch.commits {
            receipt.accept_replace_scope();
        }
        for mutation in &batch.refs {
            match mutation {
                HistoryRefMutation::Upsert(_) => receipt.accept_replace_scope(),
                HistoryRefMutation::Delete(_) => receipt.accept_tombstone_scope(),
            }
        }
        for mutation in &batch.tags {
            match mutation {
                HistoryRefMutation::Upsert(_) => receipt.accept_replace_scope(),
                HistoryRefMutation::Delete(_) => receipt.accept_tombstone_scope(),
            }
        }
        for _record in &batch.diff_hunks {
            receipt.accept_replace_scope();
        }
        self.authority_store.persist_from_ledger(&guard)?;
        drop(guard);
        Ok(receipt)
    }
}

/// Direct dirty-overlay materializer. Dirty state remains auxiliary and
/// non-activation-blocking.
pub struct DirectRuntimeMetadataMaterializer {
    authority_store: Arc<AuxiliaryAuthorityStore>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectRuntimeMetadataMaterializer {
    #[must_use]
    pub fn new(authority_store: Arc<AuxiliaryAuthorityStore>, ledger: Arc<RwLock<Ledger>>) -> Self {
        Self {
            authority_store,
            ledger,
        }
    }
}

fn dirty_publish_receipt_v1(batch: &DirtyIngestBatch) -> BatchPublishReceipt {
    let mut receipt = BatchPublishReceipt::empty_for(batch.generation, batch.batch_digest.clone());
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
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!("direct dirty materialize: ledger poisoned: {err}"))
        })?;
        guard.apply_runtime_batch(batch);
        self.authority_store.persist_from_ledger(&guard)?;
        drop(guard);
        Ok(dirty_publish_receipt_v1(batch))
    }

    fn publish_catalog_batch(
        &self,
        batch: &RuntimeCatalogIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.batch_digest.clone());
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct runtime catalog materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.apply_runtime_catalog_batch(batch)?;
        for _record in &batch.changed_entries {
            receipt.accept_replace_scope();
        }
        for _record in &batch.facet_entries {
            receipt.accept_replace_scope();
        }
        for _record in &batch.snapshot_entries {
            receipt.accept_replace_scope();
        }
        for _record in &batch.affected_entries {
            receipt.accept_replace_scope();
        }
        for _record in &batch.invalidated_by_entries {
            receipt.accept_replace_scope();
        }
        self.authority_store.persist_from_ledger(&guard)?;
        drop(guard);
        Ok(receipt)
    }
}

/// Direct structural materializer. Structural readiness is first-class and no
/// longer inferred from lexical seal replay; the mirrored lexical channel path
/// is kept only for restart-time authority rebuild.
pub struct DirectStructuralMaterializer {
    authority_store: Arc<AuxiliaryAuthorityStore>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectStructuralMaterializer {
    #[must_use]
    pub fn new(authority_store: Arc<AuxiliaryAuthorityStore>, ledger: Arc<RwLock<Ledger>>) -> Self {
        Self {
            authority_store,
            ledger,
        }
    }
}

impl StructuralIngestPort for DirectStructuralMaterializer {
    fn publish_batch(
        &self,
        batch: &StructuralIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct structural materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.apply_structural_batch(batch)?;
        guard.record_track_materialized(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Structural,
            batch.generation,
            Some(batch.manifest_digest.as_str()),
        );
        let has_parse_trees = guard
            .structural_state(&batch.repo_id, &batch.revision_id, batch.generation)
            .is_some_and(|state| !state.parse_trees().is_empty());
        if batch.seal && has_parse_trees {
            guard.request_structural_seal(&batch.repo_id, &batch.revision_id, batch.generation);
            guard.record_track_seal_with_digest(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Structural,
                batch.generation,
                batch.manifest_digest.as_str(),
            );
        }
        self.authority_store.persist_from_ledger(&guard)?;
        drop(guard);
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, batch.manifest_digest.clone());
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
        }
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

    #[must_use]
    pub fn dispatch(&self, request: SearchPlaneIngestIpcRequest) -> SearchPlaneIngestIpcResponse {
        match request {
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch) => {
                match self.publish_generation_scoped(
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    || self.lexical.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) => {
                match self.history.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::HistoryReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(batch) => {
                match self.publish_generation_scoped(
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    || self.repo_commit_recency.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(batch) => {
                match self.publish_generation_scoped(
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    || self.repo_topic.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoTopicReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(batch) => {
                match self.publish_generation_scoped(
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    || self.repo_description.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(batch) => {
                match self.publish_generation_scoped(
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    || self.file_ownership.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::FileOwnershipReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(batch) => {
                match self.publish_generation_scoped(
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    || self.file_contributor.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(batch) => {
                match self.publish_generation_scoped(
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    || self.repo_meta.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoMetaReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch) => {
                match self.runtime.publish_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::DirtyReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(batch) => {
                match self.runtime.publish_catalog_batch(&batch) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(batch) => {
                match self.structural.publish_batch(&batch) {
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
        assert_eq!(receipt.manifest_digest, batch.batch_digest);
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
                "mismatched-semantic-digest".to_string(),
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
        if !receipt.sealed || receipt.manifest_digest != batch.manifest_digest {
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

    #[expect(
        clippy::suspicious_operation_groupings,
        reason = "the receipt deliberately echoes the batch digest in its manifest_digest field; that field conflation is the behavior under test, not a mis-typed comparison"
    )]
    #[test]
    fn direct_dirty_materializer_echoes_batch_digest_in_receipt() -> TestRes {
        let dir = tempfile::tempdir()?;
        let authority_store = Arc::new(AuxiliaryAuthorityStore::open(
            dir.path(),
            crate::search_corpus_retention::SearchCorpusHistoryRetentionPolicyV1::new(
                2,
                1024 * 1024,
                2,
                4 * 1024 * 1024,
            )?,
        )?);
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let materializer =
            DirectRuntimeMetadataMaterializer::new(authority_store, Arc::clone(&ledger));
        let batch = fixture_dirty_batch();
        let receipt = materializer.publish_batch(&batch)?;
        if receipt.manifest_digest != batch.batch_digest
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
        fn model_version(&self) -> Option<&str> {
            None
        }
        fn dimension(&self) -> usize {
            self.dimension
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
        fn model_version(&self) -> Option<&str> {
            None
        }
        fn dimension(&self) -> usize {
            self.dimension
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
