//! Physical generation inspection and the sealed-generation build plan:
//! which tracks a batch must build, discard, or keep before it can seal.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::{
    BatchIngestMode, BatchPublishReceipt, GenerationSnapshot, ManifestGeneration,
    SearchCorpusIngestBatch, SearchPlaneTrackKind,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, IncompleteGenerationDiscardOutcomeV1,
    IncompleteGenerationDiscardPort, SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort,
    SemanticIngestHeaderV1, SemanticStreamTallyV1,
};

use crate::ingest_dispatcher::errors::{
    ERR_SEARCH_CORPUS_GENERATION_CONFLICT, ERR_SEARCH_CORPUS_GENERATION_REPAIR_REQUIRED,
};
use crate::{SnapshotKey, SnapshotRegistries, SnapshotRetireOutcome};

/// What one physical reclaim pass did, per track and generation.
#[derive(Debug, Default, Eq, PartialEq)]
pub(super) struct SearchCorpusPhysicalReclaimReceiptV1 {
    /// Bytes given back per reclaimed generation.
    pub(crate) reclaimed: BTreeMap<(SearchPlaneTrackKind, ManifestGeneration), u64>,
    /// Generations left on disk because a resident handle still had holders.
    pub(crate) deferred_pinned: BTreeSet<(SearchPlaneTrackKind, ManifestGeneration, usize)>,
    /// Generations whose idempotency records were forgotten because the
    /// generation is no longer a whole sealed pair on disk (QI-BB-032
    /// retention), with how many records each forget dropped.
    pub(crate) forgotten_records: BTreeMap<ManifestGeneration, u64>,
    /// The steps this pass could not complete (QI-BB-020).
    pub(crate) deferred: BTreeSet<DeferredGcStep>,
}

/// One step of a physical reclaim pass the storage failed after the seal it
/// follows was durable (QI-BB-020).
///
/// The seal stands. Every step's input is still on disk or in the
/// catalog, and the pass derives its work from the disk and the catalog,
/// never from an earlier pass, so the next pass of the pair finds it again.
/// A refusal is never deferred ([`crate::post_durable`]).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum DeferredGcStep {
    /// A track's sealed generations could not be listed.
    ListSealed(SearchPlaneTrackKind),
    /// A retired generation could not be reclaimed; it is still on disk.
    Reclaim(SearchPlaneTrackKind, ManifestGeneration),
    /// The generations the pair holds idempotency records for could not be
    /// listed.
    ListRecords,
    /// The records of a generation that is no longer whole could not be
    /// forgotten.
    ForgetRecords(ManifestGeneration),
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

/// What one track physically holds for a generation, as the identity
/// validator reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum PhysicalGenerationStateV1 {
    Absent,
    InProgress,
    Exact,
    /// Present under a sealed identity, but not the exact generation the
    /// batch names: a damaged sidecar or manifest, or a different digest.
    /// `code` is the validator's typed refusal, kept so the repair refusal
    /// can name it.
    Corrupt {
        code: String,
    },
}

/// Which tracks a sealing batch must repair, discard, build or merely
/// finalize, decided before any mutation (QI-BB-029).
#[derive(Clone, Debug)]
pub(super) struct SealedGenerationBuildPlanV1 {
    pub(super) lexical: GenerationSnapshot,
    pub(super) semantic: GenerationSnapshot,
    pub(super) lexical_state: PhysicalGenerationStateV1,
    pub(super) semantic_state: PhysicalGenerationStateV1,
}

impl SealedGenerationBuildPlanV1 {
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

    /// The tracks that are sealed but not exact, with the validator's code.
    fn corrupt_tracks(&self) -> Vec<(&GenerationSnapshot, &str)> {
        let mut tracks = Vec::new();
        if let PhysicalGenerationStateV1::Corrupt { code } = &self.lexical_state {
            tracks.push((&self.lexical, code.as_str()));
        }
        if let PhysicalGenerationStateV1::Corrupt { code } = &self.semantic_state {
            tracks.push((&self.semantic, code.as_str()));
        }
        tracks
    }

    /// Typed repair of a sealed-but-corrupt track (QI-BB-029 보완 #4).
    ///
    /// A track that is sealed under the batch's identity but fails exact
    /// validation cannot be served, activated or built on, and until now
    /// could only be removed by hand. A `ReplaceGeneration` seal batch
    /// carries the generation's whole content, so it can rebuild the track:
    /// the damaged directory is reclaimed through the track's identity-
    /// verified reclaim port (which refuses a directory whose sealed
    /// identity is not the batch's) and the track is built again from the
    /// batch. A `Delta` batch cannot rebuild a track it only patches, so it
    /// is refused typed with the repair to perform.
    ///
    /// The reclaim port's protocol holds here as in physical GC: the
    /// damaged generation is fenced out of the snapshot registry first, and
    /// a handle a reader still holds refuses the repair typed instead of
    /// deleting files under it.
    pub(super) fn repair_corrupt_v1(
        &self,
        mode: BatchIngestMode,
        snapshots: &SnapshotRegistries,
        lexical_reclaim: &dyn SealedGenerationReclaimPort,
        semantic_reclaim: &dyn SealedGenerationReclaimPort,
    ) -> Result<(), CoreError> {
        for (track, code) in self.corrupt_tracks() {
            if mode != BatchIngestMode::ReplaceGeneration {
                return Err(CoreError::Typed {
                    code: ERR_SEARCH_CORPUS_GENERATION_REPAIR_REQUIRED.to_string(),
                    message: format!(
                        "direct search-corpus materialize: {:?} track of repo={} revision={} generation={} is sealed but fails exact validation ({code}); a Delta batch cannot rebuild it — publish a ReplaceGeneration seal batch for generation {} to rebuild the damaged track",
                        track.track,
                        track.repo_id.as_str(),
                        track.revision_id.as_str(),
                        track.manifest_generation.get(),
                        track.manifest_generation.get(),
                    ),
                });
            }
            let key = SnapshotKey::new(
                &track.repo_id,
                &track.revision_id,
                track.manifest_generation,
            );
            let (port, fence) = match track.track {
                SearchPlaneTrackKind::Lexical => (lexical_reclaim, snapshots.lexical.retire(&key)?),
                SearchPlaneTrackKind::Semantic => {
                    (semantic_reclaim, snapshots.semantic.retire(&key)?)
                }
                SearchPlaneTrackKind::Structural => {
                    return Err(CoreError::InvalidContract(
                        "direct search-corpus materialize: structural is not a search-corpus track"
                            .to_string(),
                    ));
                }
            };
            if let SnapshotRetireOutcome::StillReferenced { holders } = fence {
                return Err(CoreError::Typed {
                    code: ERR_SEARCH_CORPUS_GENERATION_CONFLICT.to_string(),
                    message: format!(
                        "direct search-corpus materialize: {:?} track of generation {} fails exact validation ({code}) but {holders} reader(s) still hold its handle; the rebuild is refused rather than deleting under them — retry once they release",
                        track.track,
                        track.manifest_generation.get(),
                    ),
                });
            }
            match port.reclaim_sealed_generation(track) {
                Ok(SealedGenerationReclaimOutcomeV1::Reclaimed { .. }) => {}
                Ok(SealedGenerationReclaimOutcomeV1::Absent) => {
                    return Err(CoreError::Typed {
                        code: ERR_SEARCH_CORPUS_GENERATION_CONFLICT.to_string(),
                        message: format!(
                            "direct search-corpus materialize: {:?} track of generation {} failed validation ({code}) but vanished before it could be reclaimed for rebuild",
                            track.track,
                            track.manifest_generation.get(),
                        ),
                    });
                }
                Err(refused) => {
                    return Err(CoreError::Typed {
                        code: ERR_SEARCH_CORPUS_GENERATION_REPAIR_REQUIRED.to_string(),
                        message: format!(
                            "direct search-corpus materialize: {:?} track of repo={} revision={} generation={} is sealed but fails exact validation ({code}) and its sealed identity cannot be reclaimed for rebuild ({refused}); discard it through the quarantine surface, then publish a ReplaceGeneration seal batch for generation {}",
                            track.track,
                            track.repo_id.as_str(),
                            track.revision_id.as_str(),
                            track.manifest_generation.get(),
                            track.manifest_generation.get(),
                        ),
                    });
                }
            }
        }
        Ok(())
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

/// Classify what a track holds for `candidate`.
///
/// A typed refusal other than "incomplete" means the directory is sealed
/// but not the exact generation: it is reported as `Corrupt` so the plan
/// can decide the repair, rather than failing here. Anything untyped (an
/// I/O error reading the track) is not a physical state and propagates.
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
        Err(CoreError::Typed { code, .. }) => Ok(PhysicalGenerationStateV1::Corrupt { code }),
        Err(source) => Err(CoreError::Storage(format!(
            "direct search-corpus materialize: {label} generation could not be inspected for repo={} revision={} generation={}: {source}",
            candidate.repo_id.as_str(),
            candidate.revision_id.as_str(),
            candidate.manifest_generation.get(),
        ))),
    }
}

/// A non-seal batch may only mutate a generation that is absent or in
/// progress on the track.
///
/// A sealed exact generation is immutable; a sealed but damaged one is
/// refused with the repair to perform, since only a `ReplaceGeneration`
/// seal batch can rebuild it.
pub(super) fn ensure_generation_is_mutable_v1(
    validator: &dyn GenerationIdentityValidatePort,
    candidate: &GenerationSnapshot,
    label: &str,
) -> Result<(), CoreError> {
    match inspect_physical_generation_v1(validator, candidate, label)? {
        PhysicalGenerationStateV1::Absent | PhysicalGenerationStateV1::InProgress => Ok(()),
        PhysicalGenerationStateV1::Exact => Err(CoreError::Typed {
            code: "GENERATION_IMMUTABLE".to_string(),
            message: format!(
                "direct search-corpus materialize: {label} generation is already sealed; refusing non-seal mutation for repo={} revision={} generation={}",
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
                candidate.manifest_generation.get(),
            ),
        }),
        PhysicalGenerationStateV1::Corrupt { code } => Err(CoreError::Typed {
            code: ERR_SEARCH_CORPUS_GENERATION_REPAIR_REQUIRED.to_string(),
            message: format!(
                "direct search-corpus materialize: {label} generation is sealed but fails exact validation ({code}) for repo={} revision={} generation={}; a non-seal batch cannot rebuild it — publish a ReplaceGeneration seal batch for generation {} to rebuild the damaged track",
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
                candidate.manifest_generation.get(),
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

/// A delta base must be the exact sealed generation on the track.
///
/// A base that is sealed but damaged is refused with the repair to
/// perform, since a delta cannot rebuild the base it patches (QI-BB-029
/// 보완 #4).
pub(super) fn validate_delta_base_v1(
    validator: &dyn GenerationIdentityValidatePort,
    base: &GenerationSnapshot,
    label: &str,
) -> Result<(), CoreError> {
    match inspect_physical_generation_v1(validator, base, label)? {
        PhysicalGenerationStateV1::Exact => Ok(()),
        PhysicalGenerationStateV1::Corrupt { code } => Err(CoreError::Typed {
            code: ERR_SEARCH_CORPUS_GENERATION_REPAIR_REQUIRED.to_string(),
            message: format!(
                "direct search-corpus materialize: {label} generation {} for repo={} revision={} is sealed but fails exact validation ({code}); a delta cannot build on a damaged base — publish a ReplaceGeneration seal batch for generation {} to rebuild it, then retry the delta",
                base.manifest_generation.get(),
                base.repo_id.as_str(),
                base.revision_id.as_str(),
                base.manifest_generation.get(),
            ),
        }),
        state @ (PhysicalGenerationStateV1::Absent | PhysicalGenerationStateV1::InProgress) => {
            Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_GENERATION_CONFLICT.to_string(),
                message: format!(
                    "direct search-corpus materialize: {label} generation {} for repo={} revision={} is {state:?} on disk although the ledger recorded it sealed; refusing before any mutation",
                    base.manifest_generation.get(),
                    base.repo_id.as_str(),
                    base.revision_id.as_str(),
                ),
            })
        }
    }
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
