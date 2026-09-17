//! Physical generation inspection and the sealed-generation build plan:
//! which tracks a batch must build, discard, or keep before it can seal.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::{
    BatchPublishReceipt, GenerationSnapshot, ManifestGeneration, SearchCorpusIngestBatch,
    SearchPlaneTrackKind,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, IncompleteGenerationDiscardOutcomeV1,
    IncompleteGenerationDiscardPort, SemanticIngestHeaderV1, SemanticStreamTallyV1,
};

use crate::ingest_dispatcher::errors::ERR_SEARCH_CORPUS_GENERATION_CONFLICT;

/// What one physical reclaim pass did, per track and generation.
#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct SearchCorpusPhysicalReclaimReceiptV1 {
    /// Bytes given back per reclaimed generation.
    pub(crate) reclaimed: BTreeMap<(SearchPlaneTrackKind, ManifestGeneration), u64>,
    /// Generations left on disk because a resident handle still had holders.
    pub(crate) deferred_pinned: BTreeSet<(SearchPlaneTrackKind, ManifestGeneration, usize)>,
}

pub(super) fn generation_pair_from_batch_v1(
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
pub(super) enum PhysicalGenerationStateV1 {
    Absent,
    InProgress,
    Exact,
}

#[derive(Clone, Debug)]
pub(super) struct SealedGenerationBuildPlanV1 {
    pub(super) lexical: GenerationSnapshot,
    pub(super) semantic: GenerationSnapshot,
    pub(super) lexical_state: PhysicalGenerationStateV1,
    pub(super) semantic_state: PhysicalGenerationStateV1,
}

impl SealedGenerationBuildPlanV1 {
    pub(super) fn finalize_only(lexical: GenerationSnapshot, semantic: GenerationSnapshot) -> Self {
        Self {
            lexical,
            semantic,
            lexical_state: PhysicalGenerationStateV1::Exact,
            semantic_state: PhysicalGenerationStateV1::Exact,
        }
    }

    pub(super) const fn is_finalize_only(&self) -> bool {
        matches!(self.lexical_state, PhysicalGenerationStateV1::Exact)
            && matches!(self.semantic_state, PhysicalGenerationStateV1::Exact)
    }

    pub(super) const fn build_lexical(&self) -> bool {
        !matches!(self.lexical_state, PhysicalGenerationStateV1::Exact)
    }

    pub(super) const fn build_semantic(&self) -> bool {
        !matches!(self.semantic_state, PhysicalGenerationStateV1::Exact)
    }

    pub(super) fn discard_incomplete_v1(
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

pub(super) fn inspect_physical_generation_v1(
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

pub(super) fn ensure_generation_is_mutable_v1(
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

pub(super) fn validate_physical_generation_v1(
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

pub(super) fn batch_publish_receipt_v1(batch: &SearchCorpusIngestBatch) -> BatchPublishReceipt {
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

/// The semantic receipt must acknowledge exactly the derived batch.
///
/// Its identity comes from the header, and it must count as many replace
/// scopes as the source issued across every window (`issued`, counted on
/// the source's side).
pub(super) fn validate_semantic_publish_receipt_v1(
    header: &SemanticIngestHeaderV1,
    issued: SemanticStreamTallyV1,
    receipt: &BatchPublishReceipt,
) -> Result<(), CoreError> {
    let expected_replace = u32::try_from(issued.replace_scopes).map_err(|err| {
        CoreError::InvalidContract(format!(
            "direct search-corpus materialize: semantic replace scope count overflow: {err}"
        ))
    })?;
    let expected_tombstone =
        u32::try_from(header.mutations.tombstone_scopes.len()).map_err(|err| {
            CoreError::InvalidContract(format!(
                "direct search-corpus materialize: semantic tombstone scope count overflow: {err}"
            ))
        })?;
    let expected_clear = u32::try_from(header.mutations.clear_surfaces.len()).map_err(|err| {
        CoreError::InvalidContract(format!(
            "direct search-corpus materialize: semantic clear surface count overflow: {err}"
        ))
    })?;
    if receipt.generation != header.pin.manifest_generation
        || receipt.manifest_digest.as_deref() != Some(header.batch.manifest_digest.as_str())
        || receipt.batch_digest != header.batch.batch_digest
        || receipt.accepted_replace_scopes != expected_replace
        || receipt.accepted_tombstone_scopes != expected_tombstone
        || receipt.accepted_clear_surfaces != expected_clear
        || receipt.sealed != header.batch.seal
    {
        return Err(CoreError::InvalidContract(format!(
            "direct search-corpus materialize: semantic receipt does not exactly acknowledge the derived batch: receipt={receipt:?}"
        )));
    }
    Ok(())
}
