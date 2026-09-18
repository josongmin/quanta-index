//! The in-memory readiness ledger: per-track seal state, historically
//! sealed search-corpus identities, semantic generation state, and the
//! per-generation, epoch-named auxiliary authority snapshots.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use quanta_index_contract::{
    AuxEpochV1, GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
};
use quanta_index_core::{AuxiliaryDomainV1, CoreError};

use crate::readiness::aux_epoch::{AuxRead, AuxSnapshots};
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

/// One of the three auxiliary authority state families as the ledger
/// holds it: which domain names it and which map its registries live in.
///
/// Implemented by the three state types only; it is what lets one generic
/// read / advance / restore path serve every domain.
pub(crate) trait AuxDomainState: Clone + Default + Send + Sync + 'static {
    const DOMAIN: AuxiliaryDomainV1;
    fn registries(ledger: &Ledger) -> &BTreeMap<AuthorityKey, AuxSnapshots<Self>>;
    fn registries_mut(ledger: &mut Ledger) -> &mut BTreeMap<AuthorityKey, AuxSnapshots<Self>>;
}

impl AuxDomainState for HistoryAuthorityState {
    const DOMAIN: AuxiliaryDomainV1 = AuxiliaryDomainV1::History;

    fn registries(ledger: &Ledger) -> &BTreeMap<AuthorityKey, AuxSnapshots<Self>> {
        &ledger.history
    }

    fn registries_mut(ledger: &mut Ledger) -> &mut BTreeMap<AuthorityKey, AuxSnapshots<Self>> {
        &mut ledger.history
    }
}

impl AuxDomainState for RuntimeMetadataState {
    const DOMAIN: AuxiliaryDomainV1 = AuxiliaryDomainV1::Runtime;

    fn registries(ledger: &Ledger) -> &BTreeMap<AuthorityKey, AuxSnapshots<Self>> {
        &ledger.runtime_metadata
    }

    fn registries_mut(ledger: &mut Ledger) -> &mut BTreeMap<AuthorityKey, AuxSnapshots<Self>> {
        &mut ledger.runtime_metadata
    }
}

impl AuxDomainState for StructuralAuthorityState {
    const DOMAIN: AuxiliaryDomainV1 = AuxiliaryDomainV1::Structural;

    fn registries(ledger: &Ledger) -> &BTreeMap<AuthorityKey, AuxSnapshots<Self>> {
        &ledger.structural
    }

    fn registries_mut(ledger: &mut Ledger) -> &mut BTreeMap<AuthorityKey, AuxSnapshots<Self>> {
        &mut ledger.structural
    }
}

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
    // The auxiliary authorities are held as epoch-named immutable snapshots
    // per generation and domain (QI-BB-020 W2): a query takes the current
    // snapshot — or the retained one a continuation names — under the read
    // lock and scans outside it, and a mutation produces the next snapshot
    // from a structurally shared clone that costs only what it changes.
    pub(super) history: BTreeMap<AuthorityKey, AuxSnapshots<HistoryAuthorityState>>,
    pub(super) runtime_metadata: BTreeMap<AuthorityKey, AuxSnapshots<RuntimeMetadataState>>,
    pub(super) structural: BTreeMap<AuthorityKey, AuxSnapshots<StructuralAuthorityState>>,
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
                .is_some_and(|registry| registry.current().seal_requested())
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

    // -----------------------------------------------------------------
    // Auxiliary authorities: epoch-named snapshots per generation and
    // domain (QI-BB-020 W2). One generic path per operation; the domain
    // methods below are its named entry points.
    // -----------------------------------------------------------------

    fn aux_registry<S: AuxDomainState>(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<&AuxSnapshots<S>> {
        S::registries(self).get(&Self::authority_key(repo_id, revision_id, generation))
    }

    fn aux_registry_or_genesis<S: AuxDomainState>(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> &mut AuxSnapshots<S> {
        S::registries_mut(self)
            .entry(Self::authority_key(repo_id, revision_id, generation))
            .or_insert_with(|| AuxSnapshots::genesis(S::DOMAIN))
    }

    /// The current state of one authority generation, if it exists.
    pub(crate) fn aux_current<S: AuxDomainState>(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<&S> {
        self.aux_registry::<S>(repo_id, revision_id, generation)
            .map(AuxSnapshots::current)
    }

    /// The epoch the next durable mutation of one authority generation
    /// produces; a generation that does not exist yet starts the sequence.
    pub(crate) fn aux_next_epoch<S: AuxDomainState>(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<AuxEpochV1, CoreError> {
        self.aux_registry::<S>(repo_id, revision_id, generation)
            .map_or_else(
                || AuxSnapshots::<S>::genesis(S::DOMAIN).next_epoch(),
                AuxSnapshots::next_epoch,
            )
    }

    /// The snapshot of one authority generation a reader may scan after
    /// releasing the lock: the current one, or the one at `epoch` when a
    /// continuation names it.
    ///
    /// `Ok(None)` is a generation with no state at all; a named epoch the
    /// registry no longer holds — or never had — is refused typed, never
    /// served from another epoch.
    pub(crate) fn aux_read_at<S: AuxDomainState>(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        epoch: Option<AuxEpochV1>,
        now: Instant,
    ) -> Result<Option<AuxRead<S>>, CoreError> {
        let Some(registry) = self.aux_registry::<S>(repo_id, revision_id, generation) else {
            return Ok(None);
        };
        match epoch {
            Some(epoch) => Ok(Some(registry.read_at(epoch, now)?)),
            None => Ok(Some(registry.read_current())),
        }
    }

    /// Produce the next snapshot of one authority generation at `epoch`
    /// (the epoch the caller made durable with the rows) from `mutate`
    /// applied to a clone of the current state; see
    /// [`AuxSnapshots::advance`].
    ///
    /// A generation that does not exist yet comes into existence only if
    /// the mutation succeeds: a refused mutation leaves the ledger exactly
    /// as it was.
    pub(crate) fn aux_advance<S: AuxDomainState>(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        epoch: AuxEpochV1,
        now: Instant,
        mutate: impl FnOnce(&mut S) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        match S::registries_mut(self).entry(Self::authority_key(repo_id, revision_id, generation)) {
            Entry::Occupied(mut occupied) => occupied.get_mut().advance(epoch, now, mutate),
            Entry::Vacant(vacant) => {
                let mut registry = AuxSnapshots::genesis(S::DOMAIN);
                registry.advance(epoch, now, mutate)?;
                let _inserted = vacant.insert(registry);
                Ok(())
            }
        }
    }

    /// Mutate the current snapshot of one authority generation in place
    /// without advancing its epoch: for restoring durable rows at boot
    /// only, before the ledger is shared.
    pub(crate) fn aux_restore_mut<S: AuxDomainState>(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> &mut S {
        self.aux_registry_or_genesis::<S>(repo_id, revision_id, generation)
            .restore_state_mut()
    }

    /// Install the epoch the durable rows of one authority generation
    /// were stamped with (boot only).
    pub(crate) fn aux_restore_epoch<S: AuxDomainState>(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        epoch: AuxEpochV1,
    ) {
        self.aux_registry_or_genesis::<S>(repo_id, revision_id, generation)
            .restore_epoch(epoch);
    }

    #[must_use]
    pub fn history_state(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<&HistoryAuthorityState> {
        self.aux_current(repo_id, revision_id, generation)
    }

    #[must_use]
    pub fn runtime_state(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<&RuntimeMetadataState> {
        self.aux_current(repo_id, revision_id, generation)
    }

    #[must_use]
    pub fn structural_state(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<&StructuralAuthorityState> {
        self.aux_current(repo_id, revision_id, generation)
    }

    /// The epoch the next durable history mutation of one generation
    /// produces.
    pub(crate) fn history_next_epoch(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<AuxEpochV1, CoreError> {
        self.aux_next_epoch::<HistoryAuthorityState>(repo_id, revision_id, generation)
    }

    /// The epoch the next durable runtime-metadata mutation of one
    /// generation produces.
    pub(crate) fn runtime_next_epoch(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<AuxEpochV1, CoreError> {
        self.aux_next_epoch::<RuntimeMetadataState>(repo_id, revision_id, generation)
    }

    /// The epoch the next durable structural mutation of one generation
    /// produces.
    pub(crate) fn structural_next_epoch(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<AuxEpochV1, CoreError> {
        self.aux_next_epoch::<StructuralAuthorityState>(repo_id, revision_id, generation)
    }

    /// The history authority of one generation as an immutable snapshot a
    /// caller may scan after releasing the ledger lock: the current one,
    /// or the retained one at `epoch` when a continuation names it.
    pub fn history_read_at(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        epoch: Option<AuxEpochV1>,
        now: Instant,
    ) -> Result<Option<AuxRead<HistoryAuthorityState>>, CoreError> {
        self.aux_read_at(repo_id, revision_id, generation, epoch, now)
    }

    /// Every epoch of one history generation the ledger still holds a
    /// snapshot of (see [`AuxSnapshots::retained_epochs`]).
    ///
    /// `None` for a generation with no state. What is bound to an epoch
    /// outside this set — its text index — may be reclaimed.
    #[must_use]
    pub fn history_retained_epochs(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<Vec<AuxEpochV1>> {
        self.aux_registry::<HistoryAuthorityState>(repo_id, revision_id, generation)
            .map(AuxSnapshots::retained_epochs)
    }

    /// The runtime-metadata authority of one generation as an immutable
    /// snapshot a caller may scan after releasing the ledger lock; see
    /// [`Ledger::history_read_at`].
    pub fn runtime_read_at(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        epoch: Option<AuxEpochV1>,
        now: Instant,
    ) -> Result<Option<AuxRead<RuntimeMetadataState>>, CoreError> {
        self.aux_read_at(repo_id, revision_id, generation, epoch, now)
    }

    /// The structural authority of one generation as an immutable snapshot
    /// a caller may scan after releasing the ledger lock; see
    /// [`Ledger::history_read_at`].
    pub fn structural_read_at(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        epoch: Option<AuxEpochV1>,
        now: Instant,
    ) -> Result<Option<AuxRead<StructuralAuthorityState>>, CoreError> {
        self.aux_read_at(repo_id, revision_id, generation, epoch, now)
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

    /// Forget every auxiliary authority of one generation (retention),
    /// retained epochs included: a reaped generation has no continuation
    /// to serve.
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

    /// Request the structural seal of one generation as an in-memory
    /// mutation (tests and in-memory fixtures; production carries the
    /// request in the structural delta).
    pub fn request_structural_seal(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        now: Instant,
    ) -> Result<(), CoreError> {
        let epoch = self.structural_next_epoch(repo_id, revision_id, generation)?;
        self.aux_advance::<StructuralAuthorityState>(
            repo_id,
            revision_id,
            generation,
            epoch,
            now,
            |state| {
                state.request_seal();
                Ok(())
            },
        )
    }
}
