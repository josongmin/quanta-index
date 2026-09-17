//! The in-memory readiness ledger: per-track seal state, historically
//! sealed search-corpus identities, semantic generation state, and the
//! per-generation auxiliary authority snapshots.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
};
use quanta_index_core::CoreError;

use crate::readiness::errors::{
    ERR_SEARCH_TRACK_GENERATION_NOT_SEALED, ERR_SEARCH_TRACK_MANIFEST_DIGEST_MISMATCH,
    ERR_SEMANTIC_GENERATION_NOT_MATERIALIZED, ERR_SEMANTIC_GENERATION_NOT_SEALED,
    ERR_SEMANTIC_MANIFEST_DIGEST_MISMATCH,
};
use crate::readiness::history_state::HistoryAuthorityState;
use crate::readiness::keys::{AuthorityKey, TrackAuthorityKey, TrackGenerationKey};
use crate::readiness::retention_receipt::SearchCorpusHistoryRetentionReceiptV1;
use crate::readiness::runtime_state::RuntimeMetadataState;
use crate::readiness::structural_state::StructuralAuthorityState;
use crate::readiness::track_state::{SemanticGenerationState, TrackAuthorityState, TrackLedger};

pub(super) type SharedLedger = Arc<RwLock<Ledger>>;

/// Shared in-memory readiness ledger for query/readiness gating and direct authority recovery.
#[derive(Debug, Default)]
pub struct Ledger {
    lexical: TrackLedger,
    semantic: TrackLedger,
    pub(super) search_tracks: BTreeMap<TrackAuthorityKey, TrackAuthorityState>,
    // The current head is intentionally monotonic, but rollback must prove
    // that its lower target was once sealed with this exact digest rather
    // than comparing it to the current head.
    sealed_search_track_identities: BTreeMap<TrackGenerationKey, String>,
    // A durable authority mutation may have changed the retained set before
    // its parent-directory fsync failed. Until a reconciled authoritative
    // retained-set receipt is applied, every rollback for that pair must fail
    // closed instead of consulting stale same-process history.
    search_corpus_history_fenced_pairs: BTreeSet<(RepoId, RevisionId)>,
    semantic_generations: BTreeMap<AuthorityKey, SemanticGenerationState>,
    // The auxiliary authorities are held as immutable snapshots per
    // generation (QI-BB-020): a query clones the `Arc` under the read lock
    // and scans outside it, and a mutation copies the one generation it
    // changes only while a reader still holds the previous snapshot.
    pub(super) history: BTreeMap<AuthorityKey, Arc<HistoryAuthorityState>>,
    pub(super) runtime_metadata: BTreeMap<AuthorityKey, Arc<RuntimeMetadataState>>,
    pub(super) structural: BTreeMap<AuthorityKey, Arc<StructuralAuthorityState>>,
}

impl Ledger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn shared() -> SharedLedger {
        Arc::new(RwLock::new(Self::default()))
    }

    #[must_use]
    pub fn lexical(&self) -> &TrackLedger {
        &self.lexical
    }

    #[must_use]
    pub fn semantic(&self) -> &TrackLedger {
        &self.semantic
    }

    pub fn lexical_materialize(
        &mut self,
        generation: ManifestGeneration,
        manifest_digest: Option<&str>,
    ) {
        self.lexical
            .record_materialized(generation, manifest_digest);
    }

    pub fn lexical_seal(&mut self, generation: ManifestGeneration) {
        self.lexical.record_seal(generation, None);
    }

    pub fn semantic_seal(&mut self, generation: ManifestGeneration) {
        self.semantic.record_seal(generation, None);
    }

    pub fn semantic_seal_with_digest(
        &mut self,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) {
        self.semantic.record_seal(generation, Some(manifest_digest));
    }

    pub fn record_track_materialized(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
        generation: ManifestGeneration,
        manifest_digest: Option<&str>,
    ) {
        let key = TrackAuthorityKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
        };
        self.search_tracks
            .entry(key)
            .or_default()
            .record_materialized(generation, manifest_digest);
        if track == SearchPlaneTrackKind::Semantic
            && let Some(digest) = manifest_digest
        {
            self.record_semantic_generation_materialized(repo_id, revision_id, generation, digest);
        }
    }

    pub fn record_track_seal(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
        generation: ManifestGeneration,
    ) {
        self.record_track_seal_inner(repo_id, revision_id, track, generation, None);
    }

    pub fn record_track_seal_with_digest(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) {
        self.record_track_seal_inner(
            repo_id,
            revision_id,
            track,
            generation,
            Some(manifest_digest),
        );
    }

    fn record_track_seal_inner(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
        generation: ManifestGeneration,
        manifest_digest: Option<&str>,
    ) {
        if track == SearchPlaneTrackKind::Structural
            && !self
                .structural
                .get(&Self::authority_key(repo_id, revision_id, generation))
                .is_some_and(|state| state.seal_requested())
        {
            return;
        }
        let key = TrackAuthorityKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
        };
        self.search_tracks
            .entry(key)
            .or_default()
            .record_seal(generation, manifest_digest);
        if track == SearchPlaneTrackKind::Semantic
            && let Some(digest) = manifest_digest
        {
            self.record_semantic_generation_sealed(repo_id, revision_id, generation, digest);
        }
    }

    /// Admit one complete lexical+semantic generation into rollback history.
    ///
    /// Callers must durably persist the composite identity before invoking this
    /// method. Per-track seal mutations intentionally do not populate rollback
    /// history: a half-persisted corpus must never become a rollback target.
    pub(crate) fn record_historically_sealed_search_corpus(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) {
        for track in [
            SearchPlaneTrackKind::Lexical,
            SearchPlaneTrackKind::Semantic,
        ] {
            let _previous = self.sealed_search_track_identities.insert(
                Self::track_generation_key(repo_id, revision_id, track, generation),
                manifest_digest.to_string(),
            );
        }
    }

    pub(crate) fn apply_search_corpus_history_retention_receipt_v1(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        required_generation: ManifestGeneration,
        receipt: &SearchCorpusHistoryRetentionReceiptV1,
    ) -> Result<(), CoreError> {
        if receipt.repo_id() != repo_id || receipt.revision_id() != revision_id {
            return Err(CoreError::InvalidContract(format!(
                "search-corpus history retention: receipt identity mismatch for repo={} revision={}",
                repo_id.as_str(),
                revision_id.as_str(),
            )));
        }
        if !receipt.store_reconciled_v1 {
            return Err(CoreError::InvalidContract(
                "search-corpus history retention: receipt is not store-reconciled".to_string(),
            ));
        }
        if !receipt.retains(required_generation) {
            return Err(CoreError::InvalidContract(format!(
                "search-corpus history retention: receipt does not retain required generation {} for repo={} revision={}",
                required_generation.get(),
                repo_id.as_str(),
                revision_id.as_str(),
            )));
        }
        if receipt
            .reaped_generations()
            .iter()
            .any(|generation| receipt.retains(*generation))
        {
            return Err(CoreError::InvalidContract(
                "search-corpus history retention: receipt retained/reaped sets overlap".to_string(),
            ));
        }
        // `retained_generations` is the complete post-reconciliation durable
        // set, not a delta. This remains correct after a prior partial delete:
        // a retry can prune stale in-memory generations even when those files
        // were already absent from the retry's observed `reaped_generations`.
        self.sealed_search_track_identities.retain(|key, _digest| {
            key.repo_id != *repo_id
                || key.revision_id != *revision_id
                || receipt.retains(key.generation)
        });
        let _removed = self
            .search_corpus_history_fenced_pairs
            .remove(&(repo_id.clone(), revision_id.clone()));
        Ok(())
    }

    pub(crate) fn fence_search_corpus_history_v1(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) {
        let _inserted = self
            .search_corpus_history_fenced_pairs
            .insert((repo_id.clone(), revision_id.clone()));
    }

    pub(crate) fn clear_search_corpus_history_fence_v1(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) {
        let _removed = self
            .search_corpus_history_fenced_pairs
            .remove(&(repo_id.clone(), revision_id.clone()));
    }

    /// Materialize a track in a single call: updates the global lexical/semantic
    /// ledger (for those tracks) and the per-(repo,revision,track) authority map
    /// together, so a caller cannot update one and silently forget the other.
    pub fn materialize_track(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
        generation: ManifestGeneration,
        manifest_digest: Option<&str>,
    ) {
        if track == SearchPlaneTrackKind::Lexical {
            self.lexical
                .record_materialized(generation, manifest_digest);
        } else if track == SearchPlaneTrackKind::Semantic {
            self.semantic
                .record_materialized(generation, manifest_digest);
        }
        self.record_track_materialized(repo_id, revision_id, track, generation, manifest_digest);
    }

    /// Seal a track in a single call, mirroring [`Ledger::materialize_track`]:
    /// updates the global lexical/semantic ledger (for those tracks) and the
    /// per-(repo,revision,track) authority map together.
    pub fn seal_track_with_digest(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) {
        if track == SearchPlaneTrackKind::Lexical {
            self.lexical.record_seal(generation, Some(manifest_digest));
        } else if track == SearchPlaneTrackKind::Semantic {
            self.semantic.record_seal(generation, Some(manifest_digest));
        }
        self.record_track_seal_with_digest(
            repo_id,
            revision_id,
            track,
            generation,
            manifest_digest,
        );
    }

    #[must_use]
    pub fn lexical_sealed(&self) -> Option<ManifestGeneration> {
        self.lexical.sealed()
    }

    #[must_use]
    pub fn semantic_sealed(&self) -> Option<ManifestGeneration> {
        self.semantic.sealed()
    }

    #[must_use]
    pub fn track_sealed(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
    ) -> Option<ManifestGeneration> {
        let key = TrackAuthorityKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
        };
        self.search_tracks.get(&key).and_then(|state| state.sealed)
    }

    #[must_use]
    pub fn track_materialized(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
    ) -> Option<ManifestGeneration> {
        let key = TrackAuthorityKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
        };
        self.search_tracks
            .get(&key)
            .and_then(|state| state.materialized)
    }

    #[must_use]
    pub fn track_manifest_digest(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
    ) -> Option<&str> {
        let key = TrackAuthorityKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
        };
        self.search_tracks
            .get(&key)
            .and_then(|state| state.manifest_digest.as_deref())
    }

    /// The digest the ledger recorded when this track generation was sealed,
    /// if it ever was. Retention prunes the record when the generation is
    /// reaped, so a reaped base reads as never sealed here.
    #[must_use]
    pub fn sealed_track_identity_digest(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
        generation: ManifestGeneration,
    ) -> Option<String> {
        self.sealed_search_track_identities
            .get(&Self::track_generation_key(
                repo_id,
                revision_id,
                track,
                generation,
            ))
            .cloned()
    }

    pub(crate) fn validate_historically_sealed_track_identity(
        &self,
        candidate: &GenerationSnapshot,
        plane: &str,
    ) -> Result<(), CoreError> {
        if self
            .search_corpus_history_fenced_pairs
            .contains(&(candidate.repo_id.clone(), candidate.revision_id.clone()))
        {
            return Err(CoreError::NotReady(format!(
                "{plane}: rollback authority is fenced pending durable retention reconciliation for repo={} revision={}",
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
            )));
        }
        let key = Self::track_generation_key(
            &candidate.repo_id,
            &candidate.revision_id,
            candidate.track,
            candidate.manifest_generation,
        );
        let Some(observed_digest) = self.sealed_search_track_identities.get(&key) else {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_TRACK_GENERATION_NOT_SEALED.to_string(),
                message: format!(
                    "{plane}: rollback target is not a historically sealed track identity for repo={} revision={} track={:?} generation={}",
                    candidate.repo_id.as_str(),
                    candidate.revision_id.as_str(),
                    candidate.track,
                    candidate.manifest_generation.get(),
                ),
            });
        };
        if observed_digest != &candidate.manifest_digest {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_TRACK_MANIFEST_DIGEST_MISMATCH.to_string(),
                message: format!(
                    "{plane}: rollback target manifest digest mismatch for repo={} revision={} track={:?} generation={}: expected={}, observed={}",
                    candidate.repo_id.as_str(),
                    candidate.revision_id.as_str(),
                    candidate.track,
                    candidate.manifest_generation.get(),
                    candidate.manifest_digest,
                    observed_digest,
                ),
            });
        }
        Ok(())
    }

    pub(crate) fn record_semantic_generation_materialized(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) {
        self.semantic_generations
            .entry(Self::authority_key(repo_id, revision_id, generation))
            .or_default()
            .record_materialized(manifest_digest);
    }

    pub(crate) fn record_semantic_generation_sealed(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) {
        self.semantic_generations
            .entry(Self::authority_key(repo_id, revision_id, generation))
            .or_default()
            .record_sealed(manifest_digest);
    }

    #[must_use]
    pub(crate) fn semantic_generation_state(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<&SemanticGenerationState> {
        self.semantic_generations
            .get(&Self::authority_key(repo_id, revision_id, generation))
    }

    pub(crate) fn validate_semantic_generation(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        expected_manifest_digest: Option<&str>,
        require_sealed: bool,
        plane: &str,
    ) -> Result<(), CoreError> {
        let Some(state) = self.semantic_generation_state(repo_id, revision_id, generation) else {
            return Err(CoreError::Typed {
                code: ERR_SEMANTIC_GENERATION_NOT_MATERIALIZED.to_string(),
                message: format!(
                    "{plane}: semantic generation {} is not materialized for repo={} revision={}",
                    generation.get(),
                    repo_id.as_str(),
                    revision_id.as_str(),
                ),
            });
        };
        if !state.materialized() {
            return Err(CoreError::Typed {
                code: ERR_SEMANTIC_GENERATION_NOT_MATERIALIZED.to_string(),
                message: format!(
                    "{plane}: semantic generation {} is not materialized for repo={} revision={}",
                    generation.get(),
                    repo_id.as_str(),
                    revision_id.as_str(),
                ),
            });
        }
        if require_sealed && !state.sealed() {
            return Err(CoreError::Typed {
                code: ERR_SEMANTIC_GENERATION_NOT_SEALED.to_string(),
                message: format!(
                    "{plane}: semantic generation {} is not sealed for repo={} revision={}",
                    generation.get(),
                    repo_id.as_str(),
                    revision_id.as_str(),
                ),
            });
        }
        if let Some(expected_manifest_digest) = expected_manifest_digest
            && state.manifest_digest() != expected_manifest_digest
        {
            return Err(CoreError::Typed {
                code: ERR_SEMANTIC_MANIFEST_DIGEST_MISMATCH.to_string(),
                message: format!(
                    "{plane}: semantic manifest digest mismatch for repo={} revision={} generation={}: expected={}, observed={}",
                    repo_id.as_str(),
                    revision_id.as_str(),
                    generation.get(),
                    expected_manifest_digest,
                    state.manifest_digest(),
                ),
            });
        }
        Ok(())
    }

    fn authority_key(
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> AuthorityKey {
        AuthorityKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            generation,
        }
    }

    fn track_generation_key(
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
        generation: ManifestGeneration,
    ) -> TrackGenerationKey {
        TrackGenerationKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
            generation,
        }
    }

    pub(crate) fn history_state_mut(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> &mut HistoryAuthorityState {
        let key = Self::authority_key(repo_id, revision_id, generation);
        Arc::make_mut(self.history.entry(key).or_default())
    }

    pub(crate) fn runtime_state_mut(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> &mut RuntimeMetadataState {
        let key = Self::authority_key(repo_id, revision_id, generation);
        Arc::make_mut(self.runtime_metadata.entry(key).or_default())
    }

    pub(crate) fn structural_state_mut(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> &mut StructuralAuthorityState {
        let key = Self::authority_key(repo_id, revision_id, generation);
        Arc::make_mut(self.structural.entry(key).or_default())
    }

    #[must_use]
    pub fn history_state(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<&HistoryAuthorityState> {
        let key = Self::authority_key(repo_id, revision_id, generation);
        self.history.get(&key).map(Arc::as_ref)
    }

    #[must_use]
    pub fn runtime_state(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<&RuntimeMetadataState> {
        let key = Self::authority_key(repo_id, revision_id, generation);
        self.runtime_metadata.get(&key).map(Arc::as_ref)
    }

    #[must_use]
    pub fn structural_state(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<&StructuralAuthorityState> {
        let key = Self::authority_key(repo_id, revision_id, generation);
        self.structural.get(&key).map(Arc::as_ref)
    }

    /// The history authority of one generation as an immutable snapshot a
    /// caller may scan after releasing the ledger lock.
    #[must_use]
    pub fn history_snapshot(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<Arc<HistoryAuthorityState>> {
        let key = Self::authority_key(repo_id, revision_id, generation);
        self.history.get(&key).map(Arc::clone)
    }

    /// The runtime-metadata authority of one generation as an immutable
    /// snapshot a caller may scan after releasing the ledger lock.
    #[must_use]
    pub fn runtime_snapshot(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<Arc<RuntimeMetadataState>> {
        let key = Self::authority_key(repo_id, revision_id, generation);
        self.runtime_metadata.get(&key).map(Arc::clone)
    }

    /// The structural authority of one generation as an immutable snapshot
    /// a caller may scan after releasing the ledger lock.
    #[must_use]
    pub fn structural_snapshot(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<Arc<StructuralAuthorityState>> {
        let key = Self::authority_key(repo_id, revision_id, generation);
        self.structural.get(&key).map(Arc::clone)
    }

    /// The structural track's authority state for one pair, if any.
    #[must_use]
    pub(crate) fn track_state(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
    ) -> Option<&TrackAuthorityState> {
        self.search_tracks.get(&TrackAuthorityKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
        })
    }

    /// Install a track's authority state as restored from the catalog.
    pub(crate) fn restore_track_state(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
        state: TrackAuthorityState,
    ) {
        let _previous = self.search_tracks.insert(
            TrackAuthorityKey {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                track,
            },
            state,
        );
    }

    /// Forget every auxiliary authority of one generation (retention).
    pub(crate) fn forget_auxiliary_generation(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) {
        let key = Self::authority_key(repo_id, revision_id, generation);
        drop(self.history.remove(&key));
        drop(self.runtime_metadata.remove(&key));
        drop(self.structural.remove(&key));
    }

    /// Every auxiliary generation of one pair older than `newer_than`.
    #[must_use]
    pub(crate) fn auxiliary_generations_older_than(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        newer_than: ManifestGeneration,
    ) -> BTreeSet<ManifestGeneration> {
        self.history
            .keys()
            .chain(self.runtime_metadata.keys())
            .chain(self.structural.keys())
            .filter(|key| {
                key.repo_id == *repo_id
                    && key.revision_id == *revision_id
                    && key.generation.get() < newer_than.get()
            })
            .map(|key| key.generation)
            .collect()
    }

    pub fn request_structural_seal(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) {
        self.structural_state_mut(repo_id, revision_id, generation)
            .request_seal();
    }
}
