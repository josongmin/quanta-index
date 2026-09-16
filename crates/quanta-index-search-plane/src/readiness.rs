use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

use quanta_index_contract::ChunkRecord;
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{
    CommitRecord, CommitSha, DiffHunkRecord, ParseTreeRecord, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    ChunkId, DirtyIngestBatch, DirtyMutation, GenerationPin, GenerationSnapshot,
    HistoryIngestBatch, HistoryRefMutation, ManifestGeneration, RepoId, RevisionId,
    RuntimeCatalogIngestBatch, SearchCorpusIngestBatch,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneSearchCorpusRollbackCasAck,
    SearchPlaneTrackKind, SearchScopeSurface, StructuralIngestBatch,
};
use quanta_index_core::CoreError;
use quanta_index_ipc::{decode_cbor_payload, encode_cbor_payload};

use crate::search_corpus_lifecycle::{
    ActiveSearchCorpusPinReadPort, SearchCorpusPairMutationCoordinator,
    SearchCorpusPairMutationGuard,
};
pub use crate::search_corpus_retention::SearchCorpusHistoryRetentionPolicyV1;
use crate::search_corpus_retention::{
    ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED, SearchCorpusHistoryRetentionItemV1,
};

type SharedLedger = Arc<RwLock<Ledger>>;
pub(crate) const ERR_SEMANTIC_GENERATION_NOT_MATERIALIZED: &str =
    "SEMANTIC_GENERATION_NOT_MATERIALIZED";
pub(crate) const ERR_SEMANTIC_GENERATION_NOT_SEALED: &str = "SEMANTIC_GENERATION_NOT_SEALED";
pub(crate) const ERR_SEMANTIC_MANIFEST_DIGEST_MISMATCH: &str = "SEMANTIC_MANIFEST_DIGEST_MISMATCH";
pub(crate) const ERR_SEARCH_TRACK_GENERATION_NOT_SEALED: &str =
    "SEARCH_TRACK_GENERATION_NOT_SEALED";
pub(crate) const ERR_SEARCH_TRACK_MANIFEST_DIGEST_MISMATCH: &str =
    "SEARCH_TRACK_MANIFEST_DIGEST_MISMATCH";
pub(crate) const ERR_ROLLBACK_CAS_CONFLICT: &str = "ROLLBACK_CAS_CONFLICT";
pub(crate) const ERR_SEARCH_CORPUS_AUTHORITY_CONFLICT: &str = "SEARCH_CORPUS_AUTHORITY_CONFLICT";
pub(crate) const ERR_COMPOSITE_ACTIVATION_CAS_CONFLICT: &str = "COMPOSITE_ACTIVATION_CAS_CONFLICT";
pub(crate) const ERR_RUNTIME_CATALOG_STALE_BATCH: &str = "RUNTIME_CATALOG_STALE_BATCH";
pub(crate) const ERR_RUNTIME_CATALOG_CONFLICTING_BATCH: &str = "RUNTIME_CATALOG_CONFLICTING_BATCH";
pub(crate) const ERR_RUNTIME_CATALOG_UNKNOWN_DOC_ID: &str = "RUNTIME_CATALOG_UNKNOWN_DOC_ID";
pub(crate) const ERR_RUNTIME_CATALOG_CHUNK_UNIVERSE_UNAVAILABLE: &str =
    "RUNTIME_CATALOG_CHUNK_UNIVERSE_UNAVAILABLE";

macro_rules! impl_struct_serde {
    ($ty:ident { $($field:ident : $field_ty:ty),+ $(,)? }) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                let mut state = serializer.serialize_struct(stringify!($ty), FIELDS.len())?;
                $(state.serialize_field(stringify!($field), &self.$field)?;)+
                state.end()
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct StructVisitor;

                impl<'de> Visitor<'de> for StructVisitor {
                    type Value = $ty;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!("struct ", stringify!($ty)))
                    }

                    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
                    where
                        A: MapAccess<'de>,
                    {
                        const FIELDS: &[&str] = &[$(stringify!($field)),+];
                        $(let mut $field: Option<$field_ty> = None;)+
                        while let Some(key) = map.next_key::<String>()? {
                            match key.as_str() {
                                $(
                                    stringify!($field) => {
                                        if $field.is_some() {
                                            return Err(de::Error::duplicate_field(stringify!($field)));
                                        }
                                        $field = Some(map.next_value()?);
                                    }
                                )+
                                _ => return Err(de::Error::unknown_field(key.as_str(), FIELDS)),
                            }
                        }
                        Ok($ty {
                            $(
                                $field: $field.ok_or_else(|| de::Error::missing_field(stringify!($field)))?,
                            )+
                        })
                    }
                }

                const FIELDS: &[&str] = &[$(stringify!($field)),+];
                deserializer.deserialize_struct(stringify!($ty), FIELDS, StructVisitor)
            }
        }
    };
}

/// Per-track readiness state.
#[derive(Debug, Default)]
pub struct TrackLedger {
    materialized: Option<ManifestGeneration>,
    sealed: Option<ManifestGeneration>,
    manifest_digest: Option<String>,
}

impl TrackLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn materialized(&self) -> Option<ManifestGeneration> {
        self.materialized
    }

    #[must_use]
    pub fn sealed(&self) -> Option<ManifestGeneration> {
        self.sealed
    }

    #[must_use]
    pub fn manifest_digest(&self) -> Option<&str> {
        self.manifest_digest.as_deref()
    }

    /// Monotonic materialization update. Lower generations do not rewind
    /// readiness truth.
    pub fn record_materialized(
        &mut self,
        generation: ManifestGeneration,
        manifest_digest: Option<&str>,
    ) {
        let next = match self.materialized {
            Some(current) if current.get() > generation.get() => current,
            _ => generation,
        };
        self.materialized = Some(next);
        if self
            .materialized
            .is_some_and(|current| current.get() == generation.get())
            && let Some(digest) = manifest_digest
        {
            self.manifest_digest = Some(digest.to_string());
        }
    }

    /// Monotonic seal update. Lower generations do not rewind readiness.
    pub fn record_seal(&mut self, generation: ManifestGeneration, manifest_digest: Option<&str>) {
        self.record_materialized(generation, manifest_digest);
        let next = match self.sealed {
            Some(current) if current.get() >= generation.get() => current,
            _ => generation,
        };
        self.sealed = Some(next);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TrackAuthorityState {
    materialized: Option<ManifestGeneration>,
    sealed: Option<ManifestGeneration>,
    manifest_digest: Option<String>,
}

impl TrackAuthorityState {
    fn record_materialized(
        &mut self,
        generation: ManifestGeneration,
        manifest_digest: Option<&str>,
    ) {
        let next = match self.materialized {
            Some(current) if current.get() > generation.get() => current,
            _ => generation,
        };
        self.materialized = Some(next);
        if self
            .materialized
            .is_some_and(|current| current.get() == generation.get())
            && let Some(digest) = manifest_digest
        {
            self.manifest_digest = Some(digest.to_string());
        }
    }

    fn record_seal(&mut self, generation: ManifestGeneration, manifest_digest: Option<&str>) {
        self.record_materialized(generation, manifest_digest);
        let next = match self.sealed {
            Some(current) if current.get() >= generation.get() => current,
            _ => generation,
        };
        self.sealed = Some(next);
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SemanticGenerationState {
    manifest_digest: String,
    materialized: bool,
    sealed: bool,
}

impl SemanticGenerationState {
    #[must_use]
    pub(crate) fn manifest_digest(&self) -> &str {
        self.manifest_digest.as_str()
    }

    #[must_use]
    pub(crate) const fn materialized(&self) -> bool {
        self.materialized
    }

    #[must_use]
    pub(crate) const fn sealed(&self) -> bool {
        self.sealed
    }

    fn record_materialized(&mut self, manifest_digest: &str) {
        self.materialized = true;
        self.manifest_digest = manifest_digest.to_string();
    }

    fn record_sealed(&mut self, manifest_digest: &str) {
        self.record_materialized(manifest_digest);
        self.sealed = true;
    }
}

/// Shared in-memory readiness ledger for query/readiness gating and direct authority recovery.
#[derive(Debug, Default)]
pub struct Ledger {
    lexical: TrackLedger,
    semantic: TrackLedger,
    search_tracks: BTreeMap<TrackAuthorityKey, TrackAuthorityState>,
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
    history: BTreeMap<AuthorityKey, HistoryAuthorityState>,
    runtime_metadata: BTreeMap<AuthorityKey, RuntimeMetadataState>,
    structural: BTreeMap<AuthorityKey, StructuralAuthorityState>,
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
                .is_some_and(StructuralAuthorityState::seal_requested)
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
    pub(crate) fn sealed_track_identity_digest(
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

    fn history_state_mut(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> &mut HistoryAuthorityState {
        let key = Self::authority_key(repo_id, revision_id, generation);
        self.history.entry(key).or_default()
    }

    fn runtime_state_mut(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> &mut RuntimeMetadataState {
        let key = Self::authority_key(repo_id, revision_id, generation);
        self.runtime_metadata.entry(key).or_default()
    }

    fn structural_state_mut(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> &mut StructuralAuthorityState {
        let key = Self::authority_key(repo_id, revision_id, generation);
        self.structural.entry(key).or_default()
    }

    #[must_use]
    pub fn history_state(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<&HistoryAuthorityState> {
        let key = Self::authority_key(repo_id, revision_id, generation);
        self.history.get(&key)
    }

    #[must_use]
    pub fn runtime_state(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<&RuntimeMetadataState> {
        let key = Self::authority_key(repo_id, revision_id, generation);
        self.runtime_metadata.get(&key)
    }

    #[must_use]
    pub fn structural_state(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Option<&StructuralAuthorityState> {
        let key = Self::authority_key(repo_id, revision_id, generation);
        self.structural.get(&key)
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

    pub fn apply_search_corpus_batch(&mut self, batch: &SearchCorpusIngestBatch) {
        let state = self.structural_state_mut(&batch.repo_id, &batch.revision_id, batch.generation);
        if batch.clear_surfaces.contains(&SearchScopeSurface::Chunk) {
            state.chunks.clear();
        }
        for scope in &batch.replace_scopes {
            state.chunks.retain(|_chunk_id, chunk| {
                chunk.repo_relative_path != scope.scope.repo_relative_path
            });
            for chunk in &scope.chunks {
                let _previous = state.chunks.insert(chunk.chunk_id.clone(), chunk.clone());
            }
        }
        for scope in &batch.tombstone_scopes {
            state.chunks.retain(|_chunk_id, chunk| {
                chunk.repo_relative_path != scope.scope.repo_relative_path
            });
        }
    }

    pub fn apply_history_batch(&mut self, batch: &HistoryIngestBatch) -> Result<(), CoreError> {
        let state = self.history_state_mut(&batch.repo_id, &batch.revision_id, batch.generation);
        for record in &batch.commits {
            state.note_commits_materialized();
            for parent in &record.parents {
                if !state.commits.contains_key(parent) {
                    return Err(CoreError::Typed {
                        code: "HISTORY_COMMIT_PARENT_UNKNOWN".to_string(),
                        message: format!(
                            "history ingest: parent {} missing before child {}",
                            parent, record.sha
                        ),
                    });
                }
            }
            let _previous = state.commits.insert(record.sha, record.clone());
        }
        for mutation in &batch.refs {
            state.note_refs_materialized();
            match mutation {
                HistoryRefMutation::Upsert(payload) => {
                    if !state.commits.contains_key(&payload.sha) {
                        return Err(CoreError::Typed {
                            code: "HISTORY_REF_NOT_FOUND".to_string(),
                            message: format!(
                                "history ingest: ref `{}` points to unknown commit {}",
                                payload.name, payload.sha
                            ),
                        });
                    }
                    let _previous = state.refs.insert(payload.name.clone(), payload.sha);
                }
                HistoryRefMutation::Delete(payload) => {
                    let _removed = state.refs.remove(payload.name.as_ref());
                }
            }
        }
        for mutation in &batch.tags {
            state.note_tags_materialized();
            match mutation {
                HistoryRefMutation::Upsert(payload) => {
                    if !state.commits.contains_key(&payload.sha) {
                        return Err(CoreError::Typed {
                            code: "HISTORY_REF_NOT_FOUND".to_string(),
                            message: format!(
                                "history ingest: tag `{}` points to unknown commit {}",
                                payload.name, payload.sha
                            ),
                        });
                    }
                    let _previous = state.tags.insert(payload.name.clone(), payload.sha);
                }
                HistoryRefMutation::Delete(payload) => {
                    let _removed = state.tags.remove(payload.name.as_ref());
                }
            }
        }
        for hunk in &batch.diff_hunks {
            state.note_diff_hunks_materialized();
            if !state.commits.contains_key(&hunk.commit_sha) {
                return Err(CoreError::Typed {
                    code: "HISTORY_REF_NOT_FOUND".to_string(),
                    message: format!(
                        "history ingest: diff hunk for unknown commit {}",
                        hunk.commit_sha
                    ),
                });
            }
            let _previous = state.diff_hunks.insert(
                HistoryDiffKey {
                    commit_sha: hunk.commit_sha,
                    file_path: hunk.file_path.clone(),
                },
                hunk.record.clone(),
            );
        }
        Ok(())
    }

    pub fn apply_runtime_batch(&mut self, batch: &DirtyIngestBatch) {
        let state = self.runtime_state_mut(&batch.repo_id, &batch.revision_id, batch.generation);
        for entry in &batch.entries {
            match entry {
                DirtyMutation::Upsert(record) => {
                    let _previous = state.dirty_docs.insert(
                        record.doc_id.clone(),
                        DirtyDocState {
                            applied_at_ms: record.applied_at_ms,
                            payload_hash: record.payload_hash,
                        },
                    );
                }
                DirtyMutation::Delete(payload) => {
                    let _removed = state.dirty_docs.remove(&payload.doc_id);
                }
            }
        }
    }

    pub fn apply_runtime_catalog_batch(
        &mut self,
        batch: &RuntimeCatalogIngestBatch,
    ) -> Result<(), CoreError> {
        let chunk_universe = self
            .structural_state(&batch.repo_id, &batch.revision_id, batch.generation)
            .map(|state| state.chunks.keys().cloned().collect::<BTreeSet<_>>())
            .ok_or_else(|| {
                runtime_catalog_typed(
                    ERR_RUNTIME_CATALOG_CHUNK_UNIVERSE_UNAVAILABLE,
                    "runtime catalog ingest: lexical chunk authority is not materialized for the pinned generation",
                )
            })?;
        validate_runtime_catalog_doc_ids(batch, &chunk_universe)?;

        let state = self.runtime_state_mut(&batch.repo_id, &batch.revision_id, batch.generation);
        enforce_runtime_catalog_batch_order(state, batch)?;
        state.catalog_overlay_epoch_ms = Some(batch.overlay_epoch_ms);
        state.catalog_batch_digest = Some(batch.batch_digest.clone().into_boxed_str());
        state.producer_head_applied_at_ms = Some(batch.producer_head_applied_at_ms);
        state.generation_materialized_at_ms = Some(batch.generation_materialized_at_ms);
        state.catalog_materialized = true;
        state.changed_docs = batch
            .changed_entries
            .iter()
            .map(|record| {
                (
                    record.doc_id.clone(),
                    ChangedDocState {
                        applied_at_ms: record.applied_at_ms,
                        payload_hash: record.payload_hash,
                    },
                )
            })
            .collect();
        state.doc_facets = batch
            .facet_entries
            .iter()
            .map(|record| {
                (
                    record.doc_id.clone(),
                    DocFacetState {
                        owner: record
                            .owner
                            .as_deref()
                            .map(str::to_owned)
                            .map(String::into_boxed_str),
                        service: record
                            .service
                            .as_deref()
                            .map(str::to_owned)
                            .map(String::into_boxed_str),
                        layer: record
                            .layer
                            .as_deref()
                            .map(str::to_owned)
                            .map(String::into_boxed_str),
                        surface: record
                            .surface
                            .as_deref()
                            .map(str::to_owned)
                            .map(String::into_boxed_str),
                    },
                )
            })
            .collect();
        state.snapshots = batch
            .snapshot_entries
            .iter()
            .map(|record| {
                (
                    record.name.clone().into_boxed_str(),
                    record.doc_ids.iter().cloned().collect(),
                )
            })
            .collect();
        state.affected_docs = batch
            .affected_entries
            .iter()
            .map(|record| {
                (
                    record.key.clone().into_boxed_str(),
                    record.doc_ids.iter().cloned().collect(),
                )
            })
            .collect();
        state.invalidated_by_docs = batch
            .invalidated_by_entries
            .iter()
            .map(|record| {
                (
                    record.key.clone().into_boxed_str(),
                    record.doc_ids.iter().cloned().collect(),
                )
            })
            .collect();
        Ok(())
    }

    pub fn apply_structural_batch(
        &mut self,
        batch: &StructuralIngestBatch,
    ) -> Result<(), CoreError> {
        let state = self.structural_state_mut(&batch.repo_id, &batch.revision_id, batch.generation);
        for scope in &batch.replace_scopes {
            let allowed_chunk_ids: std::collections::BTreeSet<ChunkId> = state
                .chunks
                .iter()
                .filter(|(_chunk_id, chunk)| {
                    chunk.repo_relative_path == scope.scope.repo_relative_path
                })
                .map(|(chunk_id, _chunk)| chunk_id.clone())
                .collect();
            for tree in &scope.trees {
                verify_parse_tree_against_chunk(
                    state,
                    &tree.chunk_id,
                    &tree.record,
                    Some(scope.scope.repo_relative_path.as_str()),
                )?;
            }
            state
                .parse_trees
                .retain(|chunk_id, _tree| !allowed_chunk_ids.contains(chunk_id));
            for tree in &scope.trees {
                let _previous = state
                    .parse_trees
                    .insert(tree.chunk_id.clone(), tree.record.clone());
            }
        }
        for scope in &batch.tombstone_scopes {
            let allowed_chunk_ids: std::collections::BTreeSet<ChunkId> = state
                .chunks
                .iter()
                .filter(|(_chunk_id, chunk)| {
                    chunk.repo_relative_path == scope.scope.repo_relative_path
                })
                .map(|(chunk_id, _chunk)| chunk_id.clone())
                .collect();
            state
                .parse_trees
                .retain(|chunk_id, _tree| !allowed_chunk_ids.contains(chunk_id));
        }
        Ok(())
    }

    pub fn apply_lexical_authority_op(&mut self, op: &LexicalChannelOp) -> Result<(), CoreError> {
        match op {
            LexicalChannelOp::UpsertChunk(payload) => {
                let _previous = self
                    .structural_state_mut(
                        &payload.repo_id,
                        &payload.revision_id,
                        payload.generation,
                    )
                    .chunks
                    .insert(
                        payload.chunk_id.clone(),
                        decode_record(&payload.payload, "chunk")?,
                    );
            }
            LexicalChannelOp::DeleteChunk(payload) => {
                let _removed = self
                    .structural_state_mut(
                        &payload.repo_id,
                        &payload.revision_id,
                        payload.generation,
                    )
                    .chunks
                    .remove(&payload.chunk_id);
            }
            LexicalChannelOp::ReplaceLexicalScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_lexical_replace_scope(&payload.payload)?;
                let state = self.structural_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.chunks.retain(|_chunk_id, chunk| {
                    chunk.repo_relative_path != scope.scope.repo_relative_path
                });
                for chunk in scope.chunks {
                    let _previous = state.chunks.insert(chunk.chunk_id.clone(), chunk);
                }
            }
            LexicalChannelOp::TombstoneLexicalScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_lexical_tombstone_scope(&payload.payload)?;
                self.structural_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                )
                .chunks
                .retain(|_chunk_id, chunk| {
                    chunk.repo_relative_path != scope.scope.repo_relative_path
                });
            }
            LexicalChannelOp::ClearLexicalSurface(payload) => {
                if payload.surface == SearchScopeSurface::Chunk {
                    self.structural_state_mut(
                        &payload.repo_id,
                        &payload.revision_id,
                        payload.generation,
                    )
                    .chunks
                    .clear();
                }
            }
            LexicalChannelOp::UpsertCommit(payload) => {
                let record: CommitRecord = decode_record(&payload.payload, "commit")?;
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.note_commits_materialized();
                for parent in &record.parents {
                    if !state.commits.contains_key(parent) {
                        return Err(CoreError::Typed {
                            code: "HISTORY_COMMIT_PARENT_UNKNOWN".to_string(),
                            message: format!(
                                "history ingest: parent {} missing before child {}",
                                parent, record.sha
                            ),
                        });
                    }
                }
                let _previous = state.commits.insert(record.sha, record);
            }
            LexicalChannelOp::UpsertRef(payload) => {
                let sha = CommitSha::from_bytes(payload.sha);
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.note_refs_materialized();
                if !state.commits.contains_key(&sha) {
                    return Err(CoreError::Typed {
                        code: "HISTORY_REF_NOT_FOUND".to_string(),
                        message: format!(
                            "history ingest: ref `{}` points to unknown commit {}",
                            payload.name, sha
                        ),
                    });
                }
                let _previous = state.refs.insert(payload.name.clone(), sha);
            }
            LexicalChannelOp::DeleteRef(payload) => {
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.note_refs_materialized();
                let _removed = state.refs.remove(payload.name.as_ref());
            }
            LexicalChannelOp::UpsertTag(payload) => {
                let sha = CommitSha::from_bytes(payload.sha);
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.note_tags_materialized();
                if !state.commits.contains_key(&sha) {
                    return Err(CoreError::Typed {
                        code: "HISTORY_REF_NOT_FOUND".to_string(),
                        message: format!(
                            "history ingest: tag `{}` points to unknown commit {}",
                            payload.name, sha
                        ),
                    });
                }
                let _previous = state.tags.insert(payload.name.clone(), sha);
            }
            LexicalChannelOp::DeleteTag(payload) => {
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.note_tags_materialized();
                let _removed = state.tags.remove(payload.name.as_ref());
            }
            LexicalChannelOp::UpsertDiffHunk(payload) => {
                let record: DiffHunkRecord = decode_record(&payload.payload, "diff_hunk")?;
                let commit_sha = CommitSha::from_bytes(payload.commit_sha);
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                state.note_diff_hunks_materialized();
                if !state.commits.contains_key(&commit_sha) {
                    return Err(CoreError::Typed {
                        code: "HISTORY_REF_NOT_FOUND".to_string(),
                        message: format!(
                            "history ingest: diff hunk for unknown commit {commit_sha}"
                        ),
                    });
                }
                let _previous = state.diff_hunks.insert(
                    HistoryDiffKey {
                        commit_sha,
                        file_path: payload.file_path.clone(),
                    },
                    record,
                );
            }
            LexicalChannelOp::UpsertDirty(payload) => {
                let _previous = self
                    .runtime_state_mut(&payload.repo_id, &payload.revision_id, payload.generation)
                    .dirty_docs
                    .insert(
                        payload.doc_id.clone(),
                        DirtyDocState {
                            applied_at_ms: payload.applied_at_ms,
                            payload_hash: payload.payload_hash,
                        },
                    );
            }
            LexicalChannelOp::EvictDirty(payload) => {
                let _removed = self
                    .runtime_state_mut(&payload.repo_id, &payload.revision_id, payload.generation)
                    .dirty_docs
                    .remove(&payload.doc_id);
            }
            LexicalChannelOp::UpsertParseTree(payload) => {
                let record: ParseTreeRecord = decode_record(&payload.payload, "parse_tree")?;
                let state = self.structural_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                verify_parse_tree_against_chunk(state, &payload.chunk_id, &record, None)?;
                let _previous = state.parse_trees.insert(payload.chunk_id.clone(), record);
            }
            LexicalChannelOp::DeleteParseTree(payload) => {
                let _removed = self
                    .structural_state_mut(
                        &payload.repo_id,
                        &payload.revision_id,
                        payload.generation,
                    )
                    .parse_trees
                    .remove(&payload.chunk_id);
            }
            LexicalChannelOp::ReplaceStructuralScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_structural_replace_scope(&payload.payload)?;
                let state = self.structural_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                let allowed_chunk_ids: std::collections::BTreeSet<ChunkId> = state
                    .chunks
                    .iter()
                    .filter(|(_chunk_id, chunk)| {
                        chunk.repo_relative_path == scope.scope.repo_relative_path
                    })
                    .map(|(chunk_id, _chunk)| chunk_id.clone())
                    .collect();
                for tree in &scope.trees {
                    verify_parse_tree_against_chunk(
                        state,
                        &tree.chunk_id,
                        &tree.record,
                        Some(scope.scope.repo_relative_path.as_str()),
                    )?;
                }
                state
                    .parse_trees
                    .retain(|chunk_id, _tree| !allowed_chunk_ids.contains(chunk_id));
                for tree in scope.trees {
                    let _previous = state.parse_trees.insert(tree.chunk_id.clone(), tree.record);
                }
            }
            LexicalChannelOp::TombstoneStructuralScope(payload) => {
                let (_mode, _base_generation, scope) =
                    decode_structural_tombstone_scope(&payload.payload)?;
                let state = self.structural_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
                let allowed_chunk_ids: std::collections::BTreeSet<ChunkId> = state
                    .chunks
                    .iter()
                    .filter(|(_chunk_id, chunk)| {
                        chunk.repo_relative_path == scope.scope.repo_relative_path
                    })
                    .map(|(chunk_id, _chunk)| chunk_id.clone())
                    .collect();
                state
                    .parse_trees
                    .retain(|chunk_id, _tree| !allowed_chunk_ids.contains(chunk_id));
            }
            LexicalChannelOp::FullBundle(_)
            | LexicalChannelOp::UpsertSymbol(_)
            | LexicalChannelOp::DeleteSymbol(_)
            | LexicalChannelOp::Seal(_) => {}
        }
        Ok(())
    }
}

fn decode_record<T>(payload: &[u8], label: &str) -> Result<T, CoreError>
where
    T: for<'de> serde::Deserialize<'de>,
{
    decode_cbor_payload(payload).map_err(|err| {
        CoreError::InvalidContract(format!(
            "search-plane authority ledger: decode {label}: {err}"
        ))
    })
}

fn decode_lexical_replace_scope(
    payload: &[u8],
) -> Result<
    (
        quanta_index_contract::BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::SearchCorpusReplaceScope,
    ),
    CoreError,
> {
    decode_record(payload, "lexical_replace_scope")
}

fn decode_lexical_tombstone_scope(
    payload: &[u8],
) -> Result<
    (
        quanta_index_contract::BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::SearchCorpusTombstoneScope,
    ),
    CoreError,
> {
    decode_record(payload, "lexical_tombstone_scope")
}

fn decode_structural_replace_scope(
    payload: &[u8],
) -> Result<
    (
        quanta_index_contract::BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::StructuralReplaceScope,
    ),
    CoreError,
> {
    decode_record(payload, "structural_replace_scope")
}

fn decode_structural_tombstone_scope(
    payload: &[u8],
) -> Result<
    (
        quanta_index_contract::BatchIngestMode,
        Option<ManifestGeneration>,
        quanta_index_contract::StructuralTombstoneScope,
    ),
    CoreError,
> {
    decode_record(payload, "structural_tombstone_scope")
}

fn structural_parse_tree_decode_fail(reason: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: "STR_PARSE_TREE_DECODE_FAIL".to_string(),
        message: reason.into(),
    }
}

fn verify_parse_tree_against_chunk(
    state: &StructuralAuthorityState,
    chunk_id: &ChunkId,
    record: &ParseTreeRecord,
    expected_scope_path: Option<&str>,
) -> Result<(), CoreError> {
    let chunk = state.chunks.get(chunk_id).ok_or_else(|| {
        structural_parse_tree_decode_fail(format!(
            "STR_PARSE_TREE_DECODE_FAIL{{reason=source_chunk_missing, chunk_id=\"{}\"}}",
            chunk_id.as_str()
        ))
    })?;
    if let Some(expected_path) = expected_scope_path
        && chunk.repo_relative_path.as_str() != expected_path
    {
        return Err(structural_parse_tree_decode_fail(format!(
            "STR_PARSE_TREE_DECODE_FAIL{{reason=scope_chunk_path_mismatch, chunk_id=\"{}\", expected_scope=\"{}\", observed_path=\"{}\"}}",
            chunk_id.as_str(),
            expected_path,
            chunk.repo_relative_path.as_str(),
        )));
    }
    let expected_hash = compute_parse_tree_source_hash(chunk.text.as_ref());
    if record.source_hash != expected_hash {
        return Err(structural_parse_tree_decode_fail(format!(
            "STR_PARSE_TREE_DECODE_FAIL{{reason=source_hash_mismatch, chunk_id=\"{}\"}}",
            chunk_id.as_str()
        )));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct AuthorityKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TrackAuthorityKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    track: SearchPlaneTrackKind,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct TrackGenerationKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    track: SearchPlaneTrackKind,
    generation: ManifestGeneration,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "history authority tracks four independently materialized shard families"
)]
#[derive(Clone, Debug, Default)]
pub struct HistoryAuthorityState {
    commits: BTreeMap<CommitSha, CommitRecord>,
    refs: BTreeMap<Box<str>, CommitSha>,
    tags: BTreeMap<Box<str>, CommitSha>,
    diff_hunks: BTreeMap<HistoryDiffKey, DiffHunkRecord>,
    commits_materialized: bool,
    refs_materialized: bool,
    tags_materialized: bool,
    diff_hunks_materialized: bool,
}

impl HistoryAuthorityState {
    fn note_commits_materialized(&mut self) {
        self.commits_materialized = true;
    }

    fn note_refs_materialized(&mut self) {
        self.refs_materialized = true;
    }

    fn note_tags_materialized(&mut self) {
        self.tags_materialized = true;
    }

    fn note_diff_hunks_materialized(&mut self) {
        self.diff_hunks_materialized = true;
    }

    #[must_use]
    pub fn commits(&self) -> &BTreeMap<CommitSha, CommitRecord> {
        &self.commits
    }

    #[must_use]
    pub fn refs(&self) -> &BTreeMap<Box<str>, CommitSha> {
        &self.refs
    }

    #[must_use]
    pub fn tags(&self) -> &BTreeMap<Box<str>, CommitSha> {
        &self.tags
    }

    #[must_use]
    pub fn diff_hunks(&self) -> &BTreeMap<HistoryDiffKey, DiffHunkRecord> {
        &self.diff_hunks
    }

    #[must_use]
    pub const fn commits_materialized(&self) -> bool {
        self.commits_materialized
    }

    #[must_use]
    pub const fn refs_materialized(&self) -> bool {
        self.refs_materialized
    }

    #[must_use]
    pub const fn tags_materialized(&self) -> bool {
        self.tags_materialized
    }

    #[must_use]
    pub const fn diff_hunks_materialized(&self) -> bool {
        self.diff_hunks_materialized
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HistoryDiffKey {
    commit_sha: CommitSha,
    file_path: Box<str>,
}

impl HistoryDiffKey {
    #[must_use]
    pub fn commit_sha(&self) -> CommitSha {
        self.commit_sha
    }

    #[must_use]
    pub fn file_path(&self) -> &str {
        self.file_path.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyDocState {
    applied_at_ms: u64,
    payload_hash: [u8; 32],
}

impl DirtyDocState {
    #[must_use]
    pub const fn applied_at_ms(&self) -> u64 {
        self.applied_at_ms
    }

    #[must_use]
    pub const fn payload_hash(&self) -> &[u8; 32] {
        &self.payload_hash
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChangedDocState {
    applied_at_ms: u64,
    payload_hash: [u8; 32],
}

impl ChangedDocState {
    #[must_use]
    pub const fn applied_at_ms(&self) -> u64 {
        self.applied_at_ms
    }

    #[must_use]
    pub const fn payload_hash(&self) -> &[u8; 32] {
        &self.payload_hash
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DocFacetState {
    owner: Option<Box<str>>,
    service: Option<Box<str>>,
    layer: Option<Box<str>>,
    surface: Option<Box<str>>,
}

impl DocFacetState {
    #[must_use]
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }

    #[must_use]
    pub fn service(&self) -> Option<&str> {
        self.service.as_deref()
    }

    #[must_use]
    pub fn layer(&self) -> Option<&str> {
        self.layer.as_deref()
    }

    #[must_use]
    pub fn surface(&self) -> Option<&str> {
        self.surface.as_deref()
    }
}

#[derive(Clone, Debug, Default)]
pub struct RuntimeMetadataState {
    dirty_docs: BTreeMap<ChunkId, DirtyDocState>,
    changed_docs: BTreeMap<ChunkId, ChangedDocState>,
    doc_facets: BTreeMap<ChunkId, DocFacetState>,
    snapshots: BTreeMap<Box<str>, BTreeSet<ChunkId>>,
    affected_docs: BTreeMap<Box<str>, BTreeSet<ChunkId>>,
    invalidated_by_docs: BTreeMap<Box<str>, BTreeSet<ChunkId>>,
    catalog_overlay_epoch_ms: Option<u64>,
    catalog_batch_digest: Option<Box<str>>,
    producer_head_applied_at_ms: Option<u64>,
    generation_materialized_at_ms: Option<u64>,
    catalog_materialized: bool,
}

impl RuntimeMetadataState {
    #[must_use]
    pub fn dirty_docs(&self) -> &BTreeMap<ChunkId, DirtyDocState> {
        &self.dirty_docs
    }

    #[must_use]
    pub fn changed_docs(&self) -> &BTreeMap<ChunkId, ChangedDocState> {
        &self.changed_docs
    }

    #[must_use]
    pub fn doc_facets(&self) -> &BTreeMap<ChunkId, DocFacetState> {
        &self.doc_facets
    }

    #[must_use]
    pub fn snapshots(&self) -> &BTreeMap<Box<str>, BTreeSet<ChunkId>> {
        &self.snapshots
    }

    #[must_use]
    pub fn affected_docs(&self) -> &BTreeMap<Box<str>, BTreeSet<ChunkId>> {
        &self.affected_docs
    }

    #[must_use]
    pub fn invalidated_by_docs(&self) -> &BTreeMap<Box<str>, BTreeSet<ChunkId>> {
        &self.invalidated_by_docs
    }

    #[must_use]
    pub const fn catalog_overlay_epoch_ms(&self) -> Option<u64> {
        self.catalog_overlay_epoch_ms
    }

    #[must_use]
    pub fn catalog_batch_digest(&self) -> Option<&str> {
        self.catalog_batch_digest.as_deref()
    }

    #[must_use]
    pub const fn producer_head_applied_at_ms(&self) -> Option<u64> {
        self.producer_head_applied_at_ms
    }

    #[must_use]
    pub const fn generation_materialized_at_ms(&self) -> Option<u64> {
        self.generation_materialized_at_ms
    }

    #[must_use]
    pub const fn catalog_materialized(&self) -> bool {
        self.catalog_materialized
    }
}

fn runtime_catalog_typed(code: &str, message: impl Into<String>) -> CoreError {
    CoreError::Typed {
        code: code.to_string(),
        message: message.into(),
    }
}

fn enforce_runtime_catalog_batch_order(
    state: &RuntimeMetadataState,
    batch: &RuntimeCatalogIngestBatch,
) -> Result<(), CoreError> {
    let Some(current_epoch) = state.catalog_overlay_epoch_ms() else {
        return Ok(());
    };
    if batch.overlay_epoch_ms < current_epoch {
        return Err(runtime_catalog_typed(
            ERR_RUNTIME_CATALOG_STALE_BATCH,
            format!(
                "runtime catalog ingest: stale overlay epoch {} is older than materialized epoch {}",
                batch.overlay_epoch_ms, current_epoch
            ),
        ));
    }
    if batch.overlay_epoch_ms == current_epoch
        && state.catalog_batch_digest() != Some(batch.batch_digest.as_str())
    {
        return Err(runtime_catalog_typed(
            ERR_RUNTIME_CATALOG_CONFLICTING_BATCH,
            format!(
                "runtime catalog ingest: conflicting batch digest `{}` for overlay epoch {}",
                batch.batch_digest, batch.overlay_epoch_ms
            ),
        ));
    }
    Ok(())
}

fn validate_runtime_catalog_doc_ids(
    batch: &RuntimeCatalogIngestBatch,
    chunk_universe: &BTreeSet<ChunkId>,
) -> Result<(), CoreError> {
    for doc_id in batch
        .changed_entries
        .iter()
        .map(|record| &record.doc_id)
        .chain(batch.facet_entries.iter().map(|record| &record.doc_id))
        .chain(
            batch
                .snapshot_entries
                .iter()
                .flat_map(|record| record.doc_ids.iter()),
        )
        .chain(
            batch
                .affected_entries
                .iter()
                .flat_map(|record| record.doc_ids.iter()),
        )
        .chain(
            batch
                .invalidated_by_entries
                .iter()
                .flat_map(|record| record.doc_ids.iter()),
        )
    {
        if !chunk_universe.contains(doc_id) {
            return Err(runtime_catalog_typed(
                ERR_RUNTIME_CATALOG_UNKNOWN_DOC_ID,
                format!(
                    "runtime catalog ingest: doc_id `{}` is not present in the pinned lexical chunk universe",
                    doc_id.as_str()
                ),
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
pub struct StructuralAuthorityState {
    chunks: BTreeMap<ChunkId, ChunkRecord>,
    parse_trees: BTreeMap<ChunkId, ParseTreeRecord>,
    seal_requested: bool,
}

impl StructuralAuthorityState {
    #[must_use]
    pub fn chunks(&self) -> &BTreeMap<ChunkId, ChunkRecord> {
        &self.chunks
    }

    #[must_use]
    pub fn parse_trees(&self) -> &BTreeMap<ChunkId, ParseTreeRecord> {
        &self.parse_trees
    }

    #[must_use]
    pub fn seal_requested(&self) -> bool {
        self.seal_requested
    }

    pub fn request_seal(&mut self) {
        self.seal_requested = true;
    }
}

#[derive(Clone, Debug, Default)]
struct HistoryAuthoritySnapshot {
    entries: BTreeMap<AuthorityKey, HistoryAuthorityState>,
}

#[derive(Clone, Debug, Default)]
struct RuntimeAuthoritySnapshot {
    entries: BTreeMap<AuthorityKey, RuntimeMetadataState>,
}

#[derive(Clone, Debug, Default)]
struct StructuralAuthoritySnapshot {
    entries: BTreeMap<AuthorityKey, StructuralAuthorityState>,
    tracks: BTreeMap<TrackAuthorityKey, TrackAuthorityState>,
}

const SEARCH_CORPUS_AUTHORITY_SCHEMA_V1: u32 = 1;
pub(crate) const SEARCH_CORPUS_LOCK_STRIPES_V1: usize = 64;

#[derive(Clone, Debug)]
struct SearchCorpusAuthorityRecordV1 {
    schema_version: u32,
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    manifest_digest: String,
}

#[derive(Debug)]
struct SearchCorpusAuthorityFileV1 {
    path: PathBuf,
    record: SearchCorpusAuthorityRecordV1,
    encoded_len: u64,
}

#[derive(Debug, Default)]
struct SearchCorpusAuthorityRootSnapshotV1 {
    pair_directories: BTreeSet<PathBuf>,
    pairs: BTreeMap<PathBuf, Vec<SearchCorpusAuthorityFileV1>>,
    total_bytes: u64,
}

#[derive(Debug)]
struct EnforcedSearchCorpusHistoryRetentionV1 {
    retained: Vec<SearchCorpusAuthorityFileV1>,
    receipt: SearchCorpusHistoryRetentionReceiptV1,
}

/// Durable result of enforcing one repo/revision history window.
///
/// Consumers must apply this receipt to the in-memory ledger before exposing a
/// newly sealed generation. This keeps same-process rollback authority aligned
/// with the durable retained window.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusHistoryRetentionReceiptV1 {
    repo_id: RepoId,
    revision_id: RevisionId,
    retained_generations: BTreeSet<ManifestGeneration>,
    reaped_generations: BTreeSet<ManifestGeneration>,
    store_reconciled_v1: bool,
}

impl SearchCorpusHistoryRetentionReceiptV1 {
    #[cfg(test)]
    pub(crate) fn retaining_generations_v1(
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generations: impl IntoIterator<Item = ManifestGeneration>,
    ) -> Self {
        Self {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            retained_generations: generations.into_iter().collect(),
            reaped_generations: BTreeSet::new(),
            store_reconciled_v1: true,
        }
    }

    #[must_use]
    pub fn repo_id(&self) -> &RepoId {
        &self.repo_id
    }

    #[must_use]
    pub fn revision_id(&self) -> &RevisionId {
        &self.revision_id
    }

    #[must_use]
    pub fn retains(&self, generation: ManifestGeneration) -> bool {
        self.retained_generations.contains(&generation)
    }

    #[must_use]
    pub fn reaped_generations(&self) -> &BTreeSet<ManifestGeneration> {
        &self.reaped_generations
    }
}

impl SearchCorpusAuthorityRecordV1 {
    fn new(
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Self {
        Self {
            schema_version: SEARCH_CORPUS_AUTHORITY_SCHEMA_V1,
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            generation,
            manifest_digest: manifest_digest.to_string(),
        }
    }

    fn validate_identity(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        path: &Path,
    ) -> Result<(), CoreError> {
        if self.schema_version != SEARCH_CORPUS_AUTHORITY_SCHEMA_V1 {
            return Err(CoreError::Storage(format!(
                "search-corpus authority: unsupported schema version {} in {}",
                self.schema_version,
                path.display()
            )));
        }
        if &self.repo_id != repo_id
            || &self.revision_id != revision_id
            || self.generation != generation
        {
            return Err(CoreError::Storage(format!(
                "search-corpus authority: filename/payload identity mismatch in {}",
                path.display()
            )));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct AuxiliaryAuthorityStore {
    history: PathBuf,
    runtime: PathBuf,
    structural: PathBuf,
    search_corpus_dir: PathBuf,
    search_corpus_staging_dir: PathBuf,
    // State-root count/byte admission must be atomic across pair stripes.
    // Production also holds the process-level state-root lease, while this
    // lock closes same-process races between different repo/revision pairs.
    search_corpus_root_lock: Mutex<()>,
    lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
    active_pins: Arc<dyn ActiveSearchCorpusPinReadPort>,
    search_corpus_history_retention: SearchCorpusHistoryRetentionPolicyV1,
    parent_sync: Arc<dyn ParentDirectorySyncPort>,
}

#[cfg(test)]
#[derive(Debug)]
struct NoActiveSearchCorpusPinsV1;

#[cfg(test)]
impl ActiveSearchCorpusPinReadPort for NoActiveSearchCorpusPinsV1 {
    fn active_search_corpus_under_guard_v1(
        &self,
        _guard: &SearchCorpusPairMutationGuard<'_>,
        _repo_id: &RepoId,
        _revision_id: &RevisionId,
    ) -> Result<Option<SearchCorpusGenerationV1>, CoreError> {
        Ok(None)
    }

    fn all_active_search_corpora_for_bootstrap_v1(
        &self,
    ) -> Result<Vec<SearchCorpusGenerationV1>, CoreError> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SealedSearchCorpusAuthorityStateV1 {
    Absent,
    Exact,
}

impl_struct_serde!(TrackAuthorityState {
    materialized: Option<ManifestGeneration>,
    sealed: Option<ManifestGeneration>,
    manifest_digest: Option<String>,
});

impl_struct_serde!(SemanticGenerationState {
    manifest_digest: String,
    materialized: bool,
    sealed: bool,
});

impl_struct_serde!(AuthorityKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
});

impl_struct_serde!(TrackAuthorityKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    track: SearchPlaneTrackKind,
});

impl_struct_serde!(HistoryAuthorityState {
    commits: BTreeMap<CommitSha, CommitRecord>,
    refs: BTreeMap<Box<str>, CommitSha>,
    tags: BTreeMap<Box<str>, CommitSha>,
    diff_hunks: BTreeMap<HistoryDiffKey, DiffHunkRecord>,
    commits_materialized: bool,
    refs_materialized: bool,
    tags_materialized: bool,
    diff_hunks_materialized: bool,
});

impl_struct_serde!(HistoryDiffKey {
    commit_sha: CommitSha,
    file_path: Box<str>,
});

impl_struct_serde!(DirtyDocState {
    applied_at_ms: u64,
    payload_hash: [u8; 32],
});

impl_struct_serde!(ChangedDocState {
    applied_at_ms: u64,
    payload_hash: [u8; 32],
});

impl_struct_serde!(DocFacetState {
    owner: Option<Box<str>>,
    service: Option<Box<str>>,
    layer: Option<Box<str>>,
    surface: Option<Box<str>>,
});

impl_struct_serde!(RuntimeMetadataState {
    dirty_docs: BTreeMap<ChunkId, DirtyDocState>,
    changed_docs: BTreeMap<ChunkId, ChangedDocState>,
    doc_facets: BTreeMap<ChunkId, DocFacetState>,
    snapshots: BTreeMap<Box<str>, BTreeSet<ChunkId>>,
    affected_docs: BTreeMap<Box<str>, BTreeSet<ChunkId>>,
    invalidated_by_docs: BTreeMap<Box<str>, BTreeSet<ChunkId>>,
    catalog_overlay_epoch_ms: Option<u64>,
    catalog_batch_digest: Option<Box<str>>,
    producer_head_applied_at_ms: Option<u64>,
    generation_materialized_at_ms: Option<u64>,
    catalog_materialized: bool,
});

impl_struct_serde!(StructuralAuthorityState {
    chunks: BTreeMap<ChunkId, ChunkRecord>,
    parse_trees: BTreeMap<ChunkId, ParseTreeRecord>,
    seal_requested: bool,
});

impl_struct_serde!(HistoryAuthoritySnapshot {
    entries: BTreeMap<AuthorityKey, HistoryAuthorityState>,
});

impl_struct_serde!(RuntimeAuthoritySnapshot {
    entries: BTreeMap<AuthorityKey, RuntimeMetadataState>,
});

impl_struct_serde!(StructuralAuthoritySnapshot {
    entries: BTreeMap<AuthorityKey, StructuralAuthorityState>,
    tracks: BTreeMap<TrackAuthorityKey, TrackAuthorityState>,
});

impl_struct_serde!(SearchCorpusAuthorityRecordV1 {
    schema_version: u32,
    repo_id: RepoId,
    revision_id: RevisionId,
    generation: ManifestGeneration,
    manifest_digest: String,
});

impl AuxiliaryAuthorityStore {
    #[cfg(test)]
    pub fn open(
        root: impl AsRef<Path>,
        search_corpus_history_retention: SearchCorpusHistoryRetentionPolicyV1,
    ) -> Result<Self, CoreError> {
        let coordinator = SearchCorpusPairMutationCoordinator::shared();
        Self::open_with_parent_sync(
            root,
            search_corpus_history_retention,
            coordinator,
            Arc::new(NoActiveSearchCorpusPinsV1),
            Arc::new(FsParentDirectorySyncPort),
        )
    }

    pub(crate) fn open_with_lifecycle_v1(
        root: impl AsRef<Path>,
        search_corpus_history_retention: SearchCorpusHistoryRetentionPolicyV1,
        lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
        active_pins: Arc<dyn ActiveSearchCorpusPinReadPort>,
    ) -> Result<Self, CoreError> {
        Self::open_with_parent_sync(
            root,
            search_corpus_history_retention,
            lifecycle_coordinator,
            active_pins,
            Arc::new(FsParentDirectorySyncPort),
        )
    }

    fn open_with_parent_sync(
        root: impl AsRef<Path>,
        search_corpus_history_retention: SearchCorpusHistoryRetentionPolicyV1,
        lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
        active_pins: Arc<dyn ActiveSearchCorpusPinReadPort>,
        parent_sync: Arc<dyn ParentDirectorySyncPort>,
    ) -> Result<Self, CoreError> {
        let root = root.as_ref();
        ensure_durable_directory_v1(root, "search-plane authority store", parent_sync.as_ref())?;
        let history_dir = root.join("history");
        let runtime_dir = root.join("runtime");
        let structural_dir = root.join("structural");
        let search_corpus_dir = root.join("search-corpus");
        let search_corpus_staging_dir = search_corpus_dir.join(".staging");
        for dir in [
            &history_dir,
            &runtime_dir,
            &structural_dir,
            &search_corpus_dir,
            &search_corpus_staging_dir,
        ] {
            ensure_durable_directory_v1(dir, "search-plane authority store", parent_sync.as_ref())?;
        }
        let store = Self {
            history: history_dir.join("state.cbor"),
            runtime: runtime_dir.join("state.cbor"),
            structural: structural_dir.join("state.cbor"),
            search_corpus_dir,
            search_corpus_staging_dir,
            search_corpus_root_lock: Mutex::new(()),
            lifecycle_coordinator,
            active_pins,
            search_corpus_history_retention,
            parent_sync,
        };
        store.reconcile_search_corpus_startup_v1()?;
        Ok(store)
    }

    fn reconcile_search_corpus_startup_v1(&self) -> Result<(), CoreError> {
        let _root_guard = self.search_corpus_root_lock.lock().map_err(|error| {
            CoreError::Storage(format!(
                "search-corpus authority: startup reconciliation lock poisoned: {error}"
            ))
        })?;
        self.reconcile_search_corpus_staging_v1()?;
        let active_pair_names = self.active_search_corpus_pair_names_v1()?;
        let empty_pairs =
            self.collect_abandoned_search_corpus_pair_directories_v1(&active_pair_names)?;
        self.remove_abandoned_search_corpus_pair_directories_v1(&empty_pairs)
    }

    fn reconcile_search_corpus_staging_v1(&self) -> Result<(), CoreError> {
        let mut removed_staging = false;
        for entry in fs::read_dir(&self.search_corpus_staging_dir).map_err(|error| {
            CoreError::Storage(format!(
                "search-corpus authority: list staging {}: {error}",
                self.search_corpus_staging_dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: read staging entry in {}: {error}",
                    self.search_corpus_staging_dir.display()
                ))
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: inspect staging entry {}: {error}",
                    path.display()
                ))
            })?;
            let owned = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_owned_search_corpus_staging_name_v1);
            if !file_type.is_file() || !owned {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: foreign staging entry {}",
                    path.display()
                )));
            }
            fs::remove_file(&path).map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: remove abandoned staging file {}: {error}",
                    path.display()
                ))
            })?;
            removed_staging = true;
        }
        if removed_staging {
            self.parent_sync
                .sync_parent(&self.search_corpus_staging_dir)
                .map_err(|error| {
                    CoreError::Storage(format!(
                        "search-corpus authority: fsync staging after reconciliation {}: {error}",
                        self.search_corpus_staging_dir.display()
                    ))
                })?;
        }
        Ok(())
    }

    fn active_search_corpus_pair_names_v1(&self) -> Result<BTreeSet<String>, CoreError> {
        let active_pair_names = self
            .active_pins
            .all_active_search_corpora_for_bootstrap_v1()?
            .into_iter()
            .map(|active| search_corpus_pair_digest(active.repo_id(), active.revision_id()))
            .collect();
        Ok(active_pair_names)
    }

    fn collect_abandoned_search_corpus_pair_directories_v1(
        &self,
        active_pair_names: &BTreeSet<String>,
    ) -> Result<Vec<PathBuf>, CoreError> {
        let mut empty_pairs = Vec::new();
        for entry in fs::read_dir(&self.search_corpus_dir).map_err(|error| {
            CoreError::Storage(format!(
                "search-corpus authority: list root for reconciliation {}: {error}",
                self.search_corpus_dir.display()
            ))
        })? {
            let entry = entry.map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: read root entry during reconciliation: {error}"
                ))
            })?;
            let path = entry.path();
            if path == self.search_corpus_staging_dir {
                continue;
            }
            let file_type = entry.file_type().map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: inspect root entry {}: {error}",
                    path.display()
                ))
            })?;
            let pair_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    CoreError::Storage(format!(
                        "search-corpus authority: non-UTF8 root entry {}",
                        path.display()
                    ))
                })?;
            if !file_type.is_dir()
                || pair_name.len() != 64
                || !pair_name.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: foreign root entry {}",
                    path.display()
                )));
            }
            reconcile_legacy_atomic_temporaries_v1(
                &path,
                ".cbor",
                "search-corpus authority",
                self.parent_sync.as_ref(),
            )?;
            if fs::read_dir(&path)
                .map_err(|error| {
                    CoreError::Storage(format!(
                        "search-corpus authority: list pair {} during reconciliation: {error}",
                        path.display()
                    ))
                })?
                .next()
                .is_none()
            {
                if active_pair_names.contains(pair_name) {
                    return Err(CoreError::Storage(format!(
                        "search-corpus authority: active pair directory is empty: {}",
                        path.display()
                    )));
                }
                empty_pairs.push(path);
            }
        }
        Ok(empty_pairs)
    }

    fn remove_abandoned_search_corpus_pair_directories_v1(
        &self,
        empty_pairs: &[PathBuf],
    ) -> Result<(), CoreError> {
        for pair in empty_pairs {
            fs::remove_dir(pair).map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: remove abandoned empty pair {}: {error}",
                    pair.display()
                ))
            })?;
        }
        if !empty_pairs.is_empty() {
            self.parent_sync
                .sync_parent(&self.search_corpus_dir)
                .map_err(|error| {
                    CoreError::Storage(format!(
                        "search-corpus authority: fsync root after empty-pair reconciliation {}: {error}",
                        self.search_corpus_dir.display()
                    ))
                })?;
        }
        Ok(())
    }

    /// Persist one complete sealed corpus as one immutable generation record.
    ///
    /// Admission performs one authoritative state-root scan, O(P + G), where
    /// `P` is the number of repo/revision pairs and `G` is the number of
    /// retained generation records. The resulting snapshot is reused for
    /// pair-local GC; no second pair scan occurs. Cross-pair deletion is
    /// intentionally forbidden because this owner has no product-active pin
    /// authority for choosing a safe victim.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the pair guard must remain held through active-pin validation, durable record admission, and retention GC"
    )]
    pub fn record_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError> {
        let pair_guard = self.lifecycle_coordinator.lock_pair(repo_id, revision_id)?;
        let active = self.active_pins.active_search_corpus_under_guard_v1(
            &pair_guard,
            repo_id,
            revision_id,
        )?;
        let _root_guard = self.search_corpus_root_lock.lock().map_err(|err| {
            CoreError::Storage(format!(
                "search-corpus authority: state-root retention lock poisoned: {err}"
            ))
        })?;
        let pair_dir = self.search_corpus_pair_dir(repo_id, revision_id);
        let mut root_snapshot = self.load_search_corpus_root_snapshot_v1()?;
        let _pair_directory_was_present = root_snapshot.pair_directories.remove(&pair_dir);
        let pair_records = root_snapshot.pairs.remove(&pair_dir).unwrap_or_default();
        for authority in &pair_records {
            if authority.record.repo_id != *repo_id || authority.record.revision_id != *revision_id
            {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: pair directory contains foreign identity in {}",
                    authority.path.display()
                )));
            }
        }
        let path = self.search_corpus_authority_path(repo_id, revision_id, generation);
        let existing_digest = pair_records
            .iter()
            .find(|authority| authority.record.generation == generation)
            .map(|authority| authority.record.manifest_digest.clone());
        if let Some(existing_digest) = existing_digest {
            if existing_digest == manifest_digest {
                return self.reconcile_existing_search_corpus_record_v1(
                    repo_id,
                    revision_id,
                    generation,
                    &path,
                    &pair_dir,
                    active.as_ref(),
                    &root_snapshot,
                    pair_records,
                );
            }
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_AUTHORITY_CONFLICT.to_string(),
                message: format!(
                    "search-corpus authority: conflicting digest for repo={} revision={} generation={}: expected={}, observed={}",
                    repo_id.as_str(),
                    revision_id.as_str(),
                    generation.get(),
                    manifest_digest,
                    existing_digest,
                ),
            });
        }
        self.persist_new_search_corpus_record_v1(
            repo_id,
            revision_id,
            generation,
            manifest_digest,
            &path,
            &pair_dir,
            active.as_ref(),
            &root_snapshot,
            pair_records,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "exact retry reconciliation consumes one complete immutable pair/root snapshot and its active pin"
    )]
    fn reconcile_existing_search_corpus_record_v1(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        path: &Path,
        pair_dir: &Path,
        active: Option<&SearchCorpusGenerationV1>,
        root_without_pair: &SearchCorpusAuthorityRootSnapshotV1,
        pair_records: Vec<SearchCorpusAuthorityFileV1>,
    ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError> {
        ensure_durable_directory_v1(
            pair_dir,
            "search-corpus authority",
            self.parent_sync.as_ref(),
        )?;
        sync_existing_file_parent_v1(path, "search-corpus authority", self.parent_sync.as_ref())?;
        self.sync_search_corpus_staging_after_exact_retry_v1()?;
        let plan =
            self.plan_search_corpus_pair_records_v1(&pair_records, Some(generation), active)?;
        self.validate_search_corpus_state_root_projection_v1(
            root_without_pair,
            &pair_records,
            &plan,
        )?;
        let enforced = self.enforce_search_corpus_history_retention_snapshot_v1(
            repo_id,
            revision_id,
            pair_dir,
            pair_records,
            &plan,
        )?;
        if enforced
            .retained
            .iter()
            .any(|authority| authority.record.generation == generation)
        {
            return Ok(enforced.receipt);
        }
        Err(CoreError::Typed {
            code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
            message: format!(
                "search-corpus history retention: generation {} has been reaped for repo={} revision={}",
                generation.get(),
                repo_id.as_str(),
                revision_id.as_str(),
            ),
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the immutable record admission joins one exact pair identity, path pair, active pin, root snapshot, and pair snapshot"
    )]
    fn persist_new_search_corpus_record_v1(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
        path: &Path,
        pair_dir: &Path,
        active: Option<&SearchCorpusGenerationV1>,
        root_without_pair: &SearchCorpusAuthorityRootSnapshotV1,
        mut pair_records: Vec<SearchCorpusAuthorityFileV1>,
    ) -> Result<SearchCorpusHistoryRetentionReceiptV1, CoreError> {
        let record =
            SearchCorpusAuthorityRecordV1::new(repo_id, revision_id, generation, manifest_digest);
        let bytes = encode_cbor_payload(&record).map_err(|err| {
            CoreError::Storage(format!(
                "search-corpus authority: encode {}: {err}",
                path.display()
            ))
        })?;
        let encoded_len = u64::try_from(bytes.len()).map_err(|_error| {
            CoreError::Storage(
                "search-corpus history retention: candidate record length exceeds u64".to_string(),
            )
        })?;
        if encoded_len > self.search_corpus_history_retention.max_bytes() {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
                message: format!(
                    "search-corpus history retention: generation {} record exceeds max_bytes={} for repo={} revision={}",
                    generation.get(),
                    self.search_corpus_history_retention.max_bytes(),
                    repo_id.as_str(),
                    revision_id.as_str(),
                ),
            });
        }
        pair_records.push(SearchCorpusAuthorityFileV1 {
            path: path.to_path_buf(),
            record,
            encoded_len,
        });
        pair_records.sort_by(|left, right| {
            right
                .record
                .generation
                .get()
                .cmp(&left.record.generation.get())
        });
        let plan =
            self.plan_search_corpus_pair_records_v1(&pair_records, Some(generation), active)?;
        self.validate_search_corpus_state_root_projection_v1(
            root_without_pair,
            &pair_records,
            &plan,
        )?;
        ensure_durable_directory_v1(
            pair_dir,
            "search-corpus authority",
            self.parent_sync.as_ref(),
        )?;
        match atomic_replace_file_from_staging_v1(
            path,
            &bytes,
            &self.search_corpus_staging_dir,
            "search-corpus authority",
            self.parent_sync.as_ref(),
        )? {
            AtomicFileWriteOutcomeV1::Durable => {
                let enforced = self.enforce_search_corpus_history_retention_snapshot_v1(
                    repo_id,
                    revision_id,
                    pair_dir,
                    pair_records,
                    &plan,
                )?;
                Ok(enforced.receipt)
            }
            AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(error) => Err(error),
        }
    }

    fn sync_search_corpus_staging_after_exact_retry_v1(&self) -> Result<(), CoreError> {
        self.parent_sync
            .sync_parent(&self.search_corpus_staging_dir)
            .map_err(|error| {
                CoreError::Storage(format!(
                    "search-corpus authority: exact retry failed to fsync staging parent {}: {error}",
                    self.search_corpus_staging_dir.display(),
                ))
            })
    }

    pub fn inspect_sealed_search_corpus(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError> {
        let path = self.search_corpus_authority_path(repo_id, revision_id, generation);
        let Some(record) =
            self.read_cbor::<SearchCorpusAuthorityRecordV1>(&path, "search corpus")?
        else {
            return Ok(SealedSearchCorpusAuthorityStateV1::Absent);
        };
        record.validate_identity(repo_id, revision_id, generation, &path)?;
        if record.manifest_digest != manifest_digest {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_AUTHORITY_CONFLICT.to_string(),
                message: format!(
                    "search-corpus authority: conflicting digest for repo={} revision={} generation={}",
                    repo_id.as_str(),
                    revision_id.as_str(),
                    generation.get(),
                ),
            });
        }
        Ok(SealedSearchCorpusAuthorityStateV1::Exact)
    }

    fn load_search_corpus_root_snapshot_v1(
        &self,
    ) -> Result<SearchCorpusAuthorityRootSnapshotV1, CoreError> {
        let mut snapshot = SearchCorpusAuthorityRootSnapshotV1::default();
        for pair_entry in fs::read_dir(&self.search_corpus_dir).map_err(|err| {
            CoreError::Storage(format!(
                "search-corpus authority: list {}: {err}",
                self.search_corpus_dir.display()
            ))
        })? {
            let pair_entry = pair_entry.map_err(|err| {
                CoreError::Storage(format!(
                    "search-corpus authority: read directory entry in {}: {err}",
                    self.search_corpus_dir.display()
                ))
            })?;
            let pair_path = pair_entry.path();
            if pair_path == self.search_corpus_staging_dir {
                continue;
            }
            if !pair_entry
                .file_type()
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "search-corpus authority: inspect {}: {err}",
                        pair_path.display()
                    ))
                })?
                .is_dir()
            {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: foreign root entry {}",
                    pair_path.display()
                )));
            }
            if !pair_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
            {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: invalid pair directory name {}",
                    pair_path.display()
                )));
            }
            let _inserted = snapshot.pair_directories.insert(pair_path.clone());
            let records = self.load_search_corpus_pair_records_v1(&pair_path)?;
            if records.is_empty() {
                continue;
            }
            for record in &records {
                snapshot.total_bytes = snapshot
                    .total_bytes
                    .checked_add(record.encoded_len)
                    .ok_or_else(|| {
                        CoreError::Storage(
                            "search-corpus history retention: state-root byte total overflow"
                                .to_string(),
                        )
                    })?;
            }
            let _previous = snapshot.pairs.insert(pair_path, records);
        }
        Ok(snapshot)
    }

    fn load_search_corpus_pair_records_v1(
        &self,
        pair_dir: &Path,
    ) -> Result<Vec<SearchCorpusAuthorityFileV1>, CoreError> {
        let mut records = Vec::new();
        for entry in fs::read_dir(pair_dir).map_err(|err| {
            CoreError::Storage(format!(
                "search-corpus authority: list {}: {err}",
                pair_dir.display()
            ))
        })? {
            let entry = entry.map_err(|err| {
                CoreError::Storage(format!(
                    "search-corpus authority: read directory entry in {}: {err}",
                    pair_dir.display()
                ))
            })?;
            let path = entry.path();
            let file_type = entry.file_type().map_err(|err| {
                CoreError::Storage(format!(
                    "search-corpus authority: inspect {}: {err}",
                    path.display()
                ))
            })?;
            if !file_type.is_file() || path.extension().is_none_or(|extension| extension != "cbor")
            {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: foreign history entry {}",
                    path.display()
                )));
            }
            let bytes = read_regular_file_nofollow_v1(&path).map_err(|err| {
                CoreError::Storage(format!(
                    "search-corpus authority: read {}: {err}",
                    path.display()
                ))
            })?;
            let record = decode_cbor_payload::<SearchCorpusAuthorityRecordV1>(bytes.as_slice())
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "search-corpus authority: decode {}: {err}",
                        path.display()
                    ))
                })?;
            record.validate_identity(
                &record.repo_id,
                &record.revision_id,
                record.generation,
                &path,
            )?;
            let expected_path = self.search_corpus_authority_path(
                &record.repo_id,
                &record.revision_id,
                record.generation,
            );
            if expected_path != path {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: filename/payload identity mismatch in {}",
                    path.display()
                )));
            }
            let encoded_len = u64::try_from(bytes.len()).map_err(|_error| {
                CoreError::Storage(format!(
                    "search-corpus authority: record length exceeds u64 in {}",
                    path.display()
                ))
            })?;
            records.push(SearchCorpusAuthorityFileV1 {
                path,
                record,
                encoded_len,
            });
        }
        records.sort_by(|left, right| {
            right
                .record
                .generation
                .get()
                .cmp(&left.record.generation.get())
        });
        Ok(records)
    }

    fn plan_search_corpus_pair_records_v1(
        &self,
        records: &[SearchCorpusAuthorityFileV1],
        required_generation: Option<ManifestGeneration>,
        active: Option<&SearchCorpusGenerationV1>,
    ) -> Result<crate::search_corpus_retention::SearchCorpusHistoryRetentionPlanV1, CoreError> {
        if let Some(active) = active {
            let exact = records.iter().any(|record| {
                record.record.generation == active.manifest_generation()
                    && record.record.manifest_digest.as_str() == active.manifest_digest()
            });
            if !exact {
                return Err(CoreError::Storage(format!(
                    "search-corpus history retention: active generation is absent from durable history for repo={} revision={} generation={} digest={}",
                    active.repo_id().as_str(),
                    active.revision_id().as_str(),
                    active.manifest_generation().get(),
                    active.manifest_digest(),
                )));
            }
        }
        self.search_corpus_history_retention.plan(
            records
                .iter()
                .map(|record| SearchCorpusHistoryRetentionItemV1 {
                    generation: record.record.generation.get(),
                    encoded_len: record.encoded_len,
                    candidate: required_generation == Some(record.record.generation),
                    active: active.is_some_and(|active| {
                        active.manifest_generation() == record.record.generation
                            && active.manifest_digest() == record.record.manifest_digest.as_str()
                    }),
                })
                .collect(),
        )
    }

    fn validate_search_corpus_state_root_projection_v1(
        &self,
        root_without_pair: &SearchCorpusAuthorityRootSnapshotV1,
        pair_records: &[SearchCorpusAuthorityFileV1],
        plan: &crate::search_corpus_retention::SearchCorpusHistoryRetentionPlanV1,
    ) -> Result<(), CoreError> {
        let existing_pair_bytes = pair_records
            .iter()
            .filter(|record| record.path.exists())
            .try_fold(0_u64, |total, record| {
                total.checked_add(record.encoded_len).ok_or_else(|| {
                    CoreError::Storage(
                        "search-corpus history retention: pair byte total overflow".to_string(),
                    )
                })
            })?;
        let retained_pair_bytes = pair_records
            .iter()
            .filter(|record| plan.retains(record.record.generation.get()))
            .try_fold(0_u64, |total, record| {
                total.checked_add(record.encoded_len).ok_or_else(|| {
                    CoreError::Storage(
                        "search-corpus history retention: retained byte total overflow".to_string(),
                    )
                })
            })?;
        let projected_pairs = root_without_pair
            .pair_directories
            .len()
            .checked_add(1)
            .ok_or_else(|| {
                CoreError::Storage(
                    "search-corpus history retention: revision-pair count overflow".to_string(),
                )
            })?;
        let projected_total_bytes = root_without_pair
            .total_bytes
            .checked_sub(existing_pair_bytes)
            .and_then(|remaining| remaining.checked_add(retained_pair_bytes))
            .ok_or_else(|| {
                CoreError::Storage(
                    "search-corpus history retention: projected state-root byte total overflow"
                        .to_string(),
                )
            })?;
        if projected_pairs > self.search_corpus_history_retention.max_revision_pairs()
            || projected_total_bytes > self.search_corpus_history_retention.max_total_bytes()
        {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
                message: format!(
                    "search-corpus history retention: state-root admission requires revision_pairs={projected_pairs} total_bytes={projected_total_bytes}, limits are max_revision_pairs={} max_total_bytes={}; cross-pair deletion is unavailable without product-active pin authority",
                    self.search_corpus_history_retention.max_revision_pairs(),
                    self.search_corpus_history_retention.max_total_bytes(),
                ),
            });
        }
        Ok(())
    }

    fn enforce_search_corpus_history_retention_snapshot_v1(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        pair_dir: &Path,
        records: Vec<SearchCorpusAuthorityFileV1>,
        plan: &crate::search_corpus_retention::SearchCorpusHistoryRetentionPlanV1,
    ) -> Result<EnforcedSearchCorpusHistoryRetentionV1, CoreError> {
        for record in &records {
            if &record.record.repo_id != repo_id || &record.record.revision_id != revision_id {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: pair directory contains foreign identity in {}",
                    record.path.display()
                )));
            }
        }

        let (retained, reaped): (Vec<_>, Vec<_>) = records
            .into_iter()
            .partition(|record| plan.retains(record.record.generation.get()));
        let retained_generations = retained
            .iter()
            .map(|record| record.record.generation)
            .collect();
        let reaped_generations = reaped
            .iter()
            .map(|record| record.record.generation)
            .collect();
        for record in &reaped {
            fs::remove_file(&record.path).map_err(|err| {
                CoreError::Storage(format!(
                    "search-corpus history retention: remove reaped authority {}: {err}",
                    record.path.display()
                ))
            })?;
        }
        if !reaped.is_empty() {
            self.parent_sync.sync_parent(pair_dir).map_err(|err| {
                CoreError::Storage(format!(
                    "search-corpus history retention: fsync pair directory {} after GC: {err}",
                    pair_dir.display()
                ))
            })?;
        }
        Ok(EnforcedSearchCorpusHistoryRetentionV1 {
            retained,
            receipt: SearchCorpusHistoryRetentionReceiptV1 {
                repo_id: repo_id.clone(),
                revision_id: revision_id.clone(),
                retained_generations,
                reaped_generations,
                store_reconciled_v1: true,
            },
        })
    }

    pub fn persist_from_ledger(&self, ledger: &Ledger) -> Result<(), CoreError> {
        let history = HistoryAuthoritySnapshot {
            entries: ledger.history.clone(),
        };
        self.write_cbor(&self.history, &history, "history")?;

        let runtime = RuntimeAuthoritySnapshot {
            entries: ledger.runtime_metadata.clone(),
        };
        self.write_cbor(&self.runtime, &runtime, "runtime metadata")?;

        let structural = StructuralAuthoritySnapshot {
            entries: ledger.structural.clone(),
            tracks: ledger
                .search_tracks
                .iter()
                .filter(|(key, _state)| key.track == SearchPlaneTrackKind::Structural)
                .map(|(key, state)| (key.clone(), state.clone()))
                .collect(),
        };
        self.write_cbor(&self.structural, &structural, "structural")?;
        Ok(())
    }

    pub fn restore_into(&self, ledger: &mut Ledger) -> Result<(), CoreError> {
        if let Some(history) =
            self.read_cbor::<HistoryAuthoritySnapshot>(&self.history, "history")?
        {
            ledger.history = history.entries;
        }
        if let Some(runtime) =
            self.read_cbor::<RuntimeAuthoritySnapshot>(&self.runtime, "runtime metadata")?
        {
            ledger.runtime_metadata = runtime.entries;
        }
        if let Some(structural) =
            self.read_cbor::<StructuralAuthoritySnapshot>(&self.structural, "structural")?
        {
            ledger.structural = structural.entries;
            ledger
                .search_tracks
                .retain(|key, _state| key.track != SearchPlaneTrackKind::Structural);
            ledger.search_tracks.extend(structural.tracks);
        }
        self.restore_search_corpus_history_into(ledger)?;
        Ok(())
    }

    fn restore_search_corpus_history_into(&self, ledger: &mut Ledger) -> Result<(), CoreError> {
        let _root_guard = self.search_corpus_root_lock.lock().map_err(|err| {
            CoreError::Storage(format!(
                "search-corpus authority: restore state-root retention lock poisoned: {err}"
            ))
        })?;
        let snapshot = self.load_search_corpus_root_snapshot_v1()?;
        let active_corpora = self
            .active_pins
            .all_active_search_corpora_for_bootstrap_v1()?;
        for active in &active_corpora {
            let exact = snapshot.pairs.values().flatten().any(|record| {
                &record.record.repo_id == active.repo_id()
                    && &record.record.revision_id == active.revision_id()
                    && record.record.generation == active.manifest_generation()
                    && record.record.manifest_digest.as_str() == active.manifest_digest()
            });
            if !exact {
                return Err(CoreError::Storage(format!(
                    "search-corpus authority: active composite root has no exact durable history record for repo={} revision={} generation={}",
                    active.repo_id().as_str(),
                    active.revision_id().as_str(),
                    active.manifest_generation().get(),
                )));
            }
        }
        if snapshot.pair_directories.len()
            > self.search_corpus_history_retention.max_revision_pairs()
        {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
                message: format!(
                    "search-corpus history retention: restore observed {} revision pairs, exceeding max_revision_pairs={}; cross-pair deletion is unavailable without product-active pin authority",
                    snapshot.pair_directories.len(),
                    self.search_corpus_history_retention.max_revision_pairs(),
                ),
            });
        }
        let mut planned = Vec::with_capacity(snapshot.pairs.len());
        let mut projected_total_bytes = 0_u64;
        for (pair_path, observed) in snapshot.pairs {
            let first = observed.first().ok_or_else(|| {
                CoreError::Storage(format!(
                    "search-corpus history retention: non-empty root snapshot lost pair records for {}",
                    pair_path.display()
                ))
            })?;
            let repo_id = first.record.repo_id.clone();
            let revision_id = first.record.revision_id.clone();
            let active = active_corpora.iter().find(|active| {
                active.repo_id() == &repo_id && active.revision_id() == &revision_id
            });
            let plan = self.plan_search_corpus_pair_records_v1(&observed, None, active)?;
            for record in observed
                .iter()
                .filter(|record| plan.retains(record.record.generation.get()))
            {
                projected_total_bytes = projected_total_bytes
                    .checked_add(record.encoded_len)
                    .ok_or_else(|| {
                        CoreError::Storage(
                            "search-corpus history retention: restore byte total overflow"
                                .to_string(),
                        )
                    })?;
            }
            planned.push((pair_path, observed, repo_id, revision_id, plan));
        }
        if projected_total_bytes > self.search_corpus_history_retention.max_total_bytes() {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
                message: format!(
                    "search-corpus history retention: restore requires total_bytes={projected_total_bytes}, exceeding max_total_bytes={}; cross-pair deletion is unavailable without product-active pin authority",
                    self.search_corpus_history_retention.max_total_bytes(),
                ),
            });
        }
        for (pair_path, observed, repo_id, revision_id, plan) in planned {
            let enforced = self.enforce_search_corpus_history_retention_snapshot_v1(
                &repo_id,
                &revision_id,
                &pair_path,
                observed,
                &plan,
            )?;
            for authority in enforced.retained {
                let record = authority.record;
                ledger.record_historically_sealed_search_corpus(
                    &record.repo_id,
                    &record.revision_id,
                    record.generation,
                    &record.manifest_digest,
                );
            }
        }
        Ok(())
    }

    fn search_corpus_pair_dir(&self, repo_id: &RepoId, revision_id: &RevisionId) -> PathBuf {
        self.search_corpus_dir
            .join(search_corpus_pair_digest(repo_id, revision_id))
    }

    fn search_corpus_authority_path(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> PathBuf {
        self.search_corpus_pair_dir(repo_id, revision_id)
            .join(format!("g{}.cbor", generation.get()))
    }

    fn write_cbor<T: Serialize>(
        &self,
        path: &Path,
        value: &T,
        label: &str,
    ) -> Result<(), CoreError> {
        let bytes = encode_cbor_payload(value).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane authority store: encode {label} {}: {err}",
                path.display()
            ))
        })?;
        match atomic_replace_file_v1(
            path,
            &bytes,
            &format!("search-plane authority store {label}"),
            self.parent_sync.as_ref(),
        )? {
            AtomicFileWriteOutcomeV1::Durable => Ok(()),
            AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(error) => Err(error),
        }
    }

    fn read_cbor<T: for<'de> Deserialize<'de>>(
        &self,
        path: &Path,
        label: &str,
    ) -> Result<Option<T>, CoreError> {
        let bytes = match read_regular_file_nofollow_v1(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(CoreError::Storage(format!(
                    "search-plane authority store: read {label} {}: {err}",
                    path.display()
                )));
            }
        };
        decode_cbor_payload(bytes.as_slice())
            .map(Some)
            .map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane authority store: decode {label} {}: {err}",
                    path.display()
                ))
            })
    }
}

#[cfg(unix)]
fn read_regular_file_nofollow_v1(path: &Path) -> std::io::Result<Vec<u8>> {
    use rustix::fs::{Mode, OFlags, open};

    let descriptor = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;
    let mut file = File::from(descriptor);
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other(format!(
            "authority path is not a regular file: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    let _bytes_read = file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(not(unix))]
fn read_regular_file_nofollow_v1(path: &Path) -> std::io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(std::io::Error::other(format!(
            "authority path is not a regular non-symlink file: {}",
            path.display()
        )));
    }
    fs::read(path)
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActivationKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    track: SearchPlaneTrackKind,
}

/// One immutable, query-visible lexical plus semantic generation identity.
///
/// Construction validates that both tracks name the exact same source
/// generation.  The fields intentionally stay private: callers cannot create
/// a lexical-only or mixed-generation corpus identity by struct literal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusGenerationV1 {
    lexical: GenerationSnapshot,
    semantic: GenerationSnapshot,
}

impl SearchCorpusGenerationV1 {
    pub fn new(
        lexical: GenerationSnapshot,
        semantic: GenerationSnapshot,
    ) -> Result<Self, CoreError> {
        if lexical.track != SearchPlaneTrackKind::Lexical
            || semantic.track != SearchPlaneTrackKind::Semantic
        {
            return Err(CoreError::InvalidContract(
                "search-corpus generation: expected exactly lexical and semantic tracks"
                    .to_string(),
            ));
        }
        if lexical.repo_id != semantic.repo_id
            || lexical.revision_id != semantic.revision_id
            || lexical.manifest_generation != semantic.manifest_generation
            || lexical.manifest_digest != semantic.manifest_digest
        {
            return Err(CoreError::InvalidContract(
                "search-corpus generation: lexical and semantic identities must match exactly"
                    .to_string(),
            ));
        }
        if lexical.manifest_digest.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "search-corpus generation: manifest_digest must not be empty".to_string(),
            ));
        }
        Ok(Self { lexical, semantic })
    }

    #[must_use]
    pub fn lexical(&self) -> &GenerationSnapshot {
        &self.lexical
    }

    #[must_use]
    pub fn semantic(&self) -> &GenerationSnapshot {
        &self.semantic
    }

    #[must_use]
    pub fn repo_id(&self) -> &RepoId {
        &self.lexical.repo_id
    }

    #[must_use]
    pub fn revision_id(&self) -> &RevisionId {
        &self.lexical.revision_id
    }

    #[must_use]
    pub fn manifest_generation(&self) -> ManifestGeneration {
        self.lexical.manifest_generation
    }

    #[must_use]
    pub fn manifest_digest(&self) -> &str {
        self.lexical.manifest_digest.as_str()
    }
}

/// A validated compare-and-swap promotion for one complete search corpus.
///
/// The only constructor requires a full lexical plus semantic identity.  This
/// makes lexical-only activation unrepresentable on the canonical path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedSearchCorpusGenerationV1 {
    candidate: SearchCorpusGenerationV1,
    expected_active: Option<SearchCorpusGenerationV1>,
}

impl PreparedSearchCorpusGenerationV1 {
    pub fn new(
        candidate: SearchCorpusGenerationV1,
        expected_active: Option<SearchCorpusGenerationV1>,
    ) -> Result<Self, CoreError> {
        if let Some(expected) = &expected_active
            && (expected.repo_id() != candidate.repo_id()
                || expected.revision_id() != candidate.revision_id())
        {
            return Err(CoreError::InvalidContract(
                "search-corpus activation: expected active identity must match candidate repo and revision"
                    .to_string(),
            ));
        }
        Ok(Self {
            candidate,
            expected_active,
        })
    }

    #[must_use]
    pub const fn candidate(&self) -> &SearchCorpusGenerationV1 {
        &self.candidate
    }

    #[must_use]
    pub const fn expected_active(&self) -> Option<&SearchCorpusGenerationV1> {
        self.expected_active.as_ref()
    }
}

/// Exact receipt of a composite activation.  `previous_active` is retained
/// for a future typed rollback contract; it is absent for first activation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchCorpusGenerationActivationV1 {
    pub active: SearchCorpusGenerationV1,
    pub previous_active: Option<SearchCorpusGenerationV1>,
}

/// Private on-disk activation-root record.
///
/// This deliberately does not reuse any public control DTO: persisted catalog
/// state is a complete lexical+semantic root, not an ingress request.
#[derive(Debug)]
struct PersistedSearchCorpusGenerationRootV1 {
    lexical: GenerationSnapshot,
    semantic: GenerationSnapshot,
}

const PERSISTED_SEARCH_CORPUS_GENERATION_ROOT_V1_FIELDS: &[&str] = &["lexical", "semantic"];

impl PersistedSearchCorpusGenerationRootV1 {
    fn from_generation(generation: &SearchCorpusGenerationV1) -> Self {
        Self {
            lexical: generation.lexical().clone(),
            semantic: generation.semantic().clone(),
        }
    }

    fn into_generation(self) -> Result<SearchCorpusGenerationV1, CoreError> {
        SearchCorpusGenerationV1::new(self.lexical, self.semantic).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane activation catalog: invalid composite root: {err:?}"
            ))
        })
    }
}

impl Serialize for PersistedSearchCorpusGenerationRootV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("PersistedSearchCorpusGenerationRootV1", 2)?;
        state.serialize_field("lexical", &self.lexical)?;
        state.serialize_field("semantic", &self.semantic)?;
        state.end()
    }
}

struct PersistedSearchCorpusGenerationRootV1Visitor;

impl<'de> Visitor<'de> for PersistedSearchCorpusGenerationRootV1Visitor {
    type Value = PersistedSearchCorpusGenerationRootV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a persisted search-corpus generation root map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut lexical = None;
        let mut semantic = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "lexical" => {
                    if lexical.is_some() {
                        return Err(de::Error::duplicate_field("lexical"));
                    }
                    lexical = Some(map.next_value()?);
                }
                "semantic" => {
                    if semantic.is_some() {
                        return Err(de::Error::duplicate_field("semantic"));
                    }
                    semantic = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        PERSISTED_SEARCH_CORPUS_GENERATION_ROOT_V1_FIELDS,
                    ));
                }
            }
        }
        Ok(PersistedSearchCorpusGenerationRootV1 {
            lexical: lexical.ok_or_else(|| de::Error::missing_field("lexical"))?,
            semantic: semantic.ok_or_else(|| de::Error::missing_field("semantic"))?,
        })
    }
}

impl<'de> Deserialize<'de> for PersistedSearchCorpusGenerationRootV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PersistedSearchCorpusGenerationRootV1",
            PERSISTED_SEARCH_CORPUS_GENERATION_ROOT_V1_FIELDS,
            PersistedSearchCorpusGenerationRootV1Visitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveGenerationRecord {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
    pub track: SearchPlaneTrackKind,
}

#[derive(Debug)]
pub struct ActivationCatalog {
    activations_dir: PathBuf,
    staging_dir: PathBuf,
    entries: RwLock<BTreeMap<ActivationKey, ActiveGenerationRecord>>,
    lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
    // A rename may succeed while the parent-directory fsync fails.  At that
    // point the durable head is ambiguous until a fresh process reopens the
    // canonical root, so this process must not serve its old in-memory head.
    durability_uncertain_v1: AtomicBool,
    parent_sync: Arc<dyn ParentDirectorySyncPort>,
}

impl ActivationCatalog {
    #[cfg(test)]
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        Self::open_with_parent_sync(
            root,
            SearchCorpusPairMutationCoordinator::shared(),
            Arc::new(FsParentDirectorySyncPort),
        )
    }

    pub(crate) fn open_with_lifecycle_v1(
        root: impl AsRef<Path>,
        lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
    ) -> Result<Self, CoreError> {
        Self::open_with_parent_sync(
            root,
            lifecycle_coordinator,
            Arc::new(FsParentDirectorySyncPort),
        )
    }

    fn open_with_parent_sync(
        root: impl AsRef<Path>,
        lifecycle_coordinator: Arc<SearchCorpusPairMutationCoordinator>,
        parent_sync: Arc<dyn ParentDirectorySyncPort>,
    ) -> Result<Self, CoreError> {
        let root = root.as_ref();
        ensure_durable_directory_v1(
            root,
            "search-plane activation catalog",
            parent_sync.as_ref(),
        )?;
        let staging_dir = root.join(".staging");
        ensure_durable_directory_v1(
            &staging_dir,
            "search-plane activation catalog staging",
            parent_sync.as_ref(),
        )?;
        reconcile_owned_staging_directory_v1(
            &staging_dir,
            "search-plane activation catalog",
            parent_sync.as_ref(),
        )?;
        reconcile_legacy_atomic_temporaries_v1(
            root,
            "--corpus.json",
            "search-plane activation catalog",
            parent_sync.as_ref(),
        )?;
        let mut entries = BTreeMap::new();
        let mut composite_roots = Vec::new();
        let dir_entries = fs::read_dir(root).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane activation catalog: list root {}: {err}",
                root.display()
            ))
        })?;
        for entry in dir_entries {
            let entry = entry.map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: read dir entry in {}: {err}",
                    root.display()
                ))
            })?;
            let file_type = entry.file_type().map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: inspect dir entry in {}: {err}",
                    root.display()
                ))
            })?;
            let path = entry.path();
            if path == staging_dir {
                continue;
            }
            if !file_type.is_file() {
                if is_search_corpus_root_path(&path) {
                    return Err(CoreError::Storage(format!(
                        "search-plane activation catalog: activation root is not a regular non-symlink file: {}",
                        path.display()
                    )));
                }
                continue;
            }
            if path.extension().is_none_or(|value| value != "json") {
                continue;
            }
            if !is_search_corpus_root_path(&path) {
                return Err(CoreError::Storage(format!(
                    "search-plane activation catalog: legacy per-track root is unsupported: {}",
                    path.display()
                )));
            }
            let bytes = read_regular_file_nofollow_v1(&path).map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: read activation {}: {err}",
                    path.display()
                ))
            })?;
            let persisted = serde_json::from_slice::<PersistedSearchCorpusGenerationRootV1>(&bytes)
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "search-plane activation catalog: decode activation {}: {err}",
                        path.display()
                    ))
                })?;
            let composite = persisted.into_generation()?;
            validate_loaded_search_corpus_root_v1(&path, &composite)?;
            composite_roots.push(composite);
        }
        // Every persisted root is one canonical lexical+semantic authority.
        for composite in composite_roots {
            insert_search_corpus_generation_records(&mut entries, &composite);
        }
        Ok(Self {
            activations_dir: root.to_path_buf(),
            staging_dir,
            entries: RwLock::new(entries),
            lifecycle_coordinator,
            durability_uncertain_v1: AtomicBool::new(false),
            parent_sync,
        })
    }

    pub(crate) fn lifecycle_coordinator(&self) -> Arc<SearchCorpusPairMutationCoordinator> {
        Arc::clone(&self.lifecycle_coordinator)
    }

    /// Durably promote one fully prepared lexical plus semantic corpus root.
    ///
    /// The persistent root is replaced and its parent directory is synced
    /// before either in-memory track entry changes. A failed durable write
    /// therefore leaves the query-visible head unchanged.
    pub(crate) fn activate_prepared_under_guard_v1(
        &self,
        guard: &SearchCorpusPairMutationGuard<'_>,
        prepared: &PreparedSearchCorpusGenerationV1,
    ) -> Result<SearchCorpusGenerationActivationV1, CoreError> {
        let candidate = prepared.candidate();
        guard.require_pair_v1(
            self.lifecycle_coordinator.as_ref(),
            candidate.repo_id(),
            candidate.revision_id(),
        )?;
        self.ensure_durability_certain_v1()?;
        let current = {
            let entries = self.entries.read().map_err(|err| {
                CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
            })?;
            active_search_corpus_generation_v1(
                &entries,
                candidate.repo_id(),
                candidate.revision_id(),
            )?
        };
        validate_prepared_search_corpus_expectation(prepared, current.as_ref())?;

        if let Some(current) = &current
            && candidate.manifest_generation().get() <= current.manifest_generation().get()
        {
            return Err(CoreError::InvalidContract(format!(
                "search-corpus activation: candidate generation must advance active generation for repo={} revision={}: candidate={} active={}",
                candidate.repo_id().as_str(),
                candidate.revision_id().as_str(),
                candidate.manifest_generation().get(),
                current.manifest_generation().get(),
            )));
        }

        let persisted = PersistedSearchCorpusGenerationRootV1::from_generation(candidate);
        let path = self.activations_dir.join(search_corpus_root_file_name(
            candidate.repo_id(),
            candidate.revision_id(),
        ));
        let bytes = serde_json::to_vec_pretty(&persisted).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane activation catalog: encode composite activation {}: {err}",
                path.display()
            ))
        })?;
        match atomic_replace_file_from_staging_v1(
            &path,
            &bytes,
            &self.staging_dir,
            "search-plane activation catalog",
            self.parent_sync.as_ref(),
        )? {
            AtomicFileWriteOutcomeV1::Durable => {}
            AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(error) => {
                self.mark_durability_uncertain_v1();
                return Err(error);
            }
        }
        let memory_commit = (|| {
            self.ensure_durability_certain_v1()?;
            let mut entries = self.entries.write().map_err(|err| {
                CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
            })?;
            let observed = active_search_corpus_generation_v1(
                &entries,
                candidate.repo_id(),
                candidate.revision_id(),
            )?;
            if observed != current {
                return Err(CoreError::Storage(
                    "search-corpus activation: mutation lock failed to preserve the checked head"
                        .to_string(),
                ));
            }
            insert_search_corpus_generation_records(&mut entries, candidate);
            drop(entries);
            Ok(())
        })();
        if let Err(error) = memory_commit {
            self.mark_durability_uncertain_v1();
            return Err(error);
        }

        Ok(SearchCorpusGenerationActivationV1 {
            active: candidate.clone(),
            previous_active: current,
        })
    }

    #[cfg(test)]
    pub fn activate_prepared_search_corpus_generation_v1(
        &self,
        prepared: &PreparedSearchCorpusGenerationV1,
    ) -> Result<SearchCorpusGenerationActivationV1, CoreError> {
        let candidate = prepared.candidate();
        let guard = self
            .lifecycle_coordinator
            .lock_pair(candidate.repo_id(), candidate.revision_id())?;
        self.activate_prepared_under_guard_v1(&guard, prepared)
    }

    /// Roll back the complete query-visible lexical plus semantic corpus.
    ///
    /// The request carries both complete identities, so no single-track
    /// selector can create or persist semantic-only authority.
    pub(crate) fn rollback_under_guard_v1(
        &self,
        guard: &SearchCorpusPairMutationGuard<'_>,
        request: &SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    ) -> Result<SearchPlaneSearchCorpusRollbackCasAck, CoreError> {
        request.validate_v1().map_err(|error| {
            CoreError::InvalidContract(format!(
                "search-corpus rollback: invalid request: {}",
                error.code_v1()
            ))
        })?;
        let expected_active =
            search_corpus_generation_from_validated_rollback_identity(&request.expected_active)?;
        let target = search_corpus_generation_from_validated_rollback_identity(&request.target)?;

        guard.require_pair_v1(
            self.lifecycle_coordinator.as_ref(),
            expected_active.repo_id(),
            expected_active.revision_id(),
        )?;
        self.ensure_durability_certain_v1()?;
        let current = {
            let entries = self.entries.read().map_err(|err| {
                CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
            })?;
            active_search_corpus_generation_v1(
                &entries,
                expected_active.repo_id(),
                expected_active.revision_id(),
            )?
            .ok_or_else(|| {
                CoreError::NotReady(format!(
                    "search-corpus rollback: no active composite generation for repo={} revision={}",
                    expected_active.repo_id().as_str(),
                    expected_active.revision_id().as_str()
                ))
            })?
        };
        if current != expected_active {
            return Err(CoreError::Typed {
                code: ERR_ROLLBACK_CAS_CONFLICT.to_string(),
                message: format!(
                    "search-corpus rollback: active composite root changed for repo={} revision={}: expected generation={} digest={}, observed generation={} digest={}",
                    expected_active.repo_id().as_str(),
                    expected_active.revision_id().as_str(),
                    expected_active.manifest_generation().get(),
                    expected_active.manifest_digest(),
                    current.manifest_generation().get(),
                    current.manifest_digest(),
                ),
            });
        }

        let persisted = PersistedSearchCorpusGenerationRootV1::from_generation(&target);
        let path = self.activations_dir.join(search_corpus_root_file_name(
            target.repo_id(),
            target.revision_id(),
        ));
        let bytes = serde_json::to_vec_pretty(&persisted).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane activation catalog: encode composite rollback {}: {err}",
                path.display()
            ))
        })?;
        match atomic_replace_file_from_staging_v1(
            &path,
            &bytes,
            &self.staging_dir,
            "search-plane activation catalog",
            self.parent_sync.as_ref(),
        )? {
            AtomicFileWriteOutcomeV1::Durable => {}
            AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(error) => {
                self.mark_durability_uncertain_v1();
                return Err(error);
            }
        }
        let memory_commit = (|| {
            self.ensure_durability_certain_v1()?;
            let mut entries = self.entries.write().map_err(|err| {
                CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
            })?;
            let observed = active_search_corpus_generation_v1(
                &entries,
                expected_active.repo_id(),
                expected_active.revision_id(),
            )?;
            if observed.as_ref() != Some(&current) {
                return Err(CoreError::Storage(
                    "search-corpus rollback: mutation lock failed to preserve the checked head"
                        .to_string(),
                ));
            }
            insert_search_corpus_generation_records(&mut entries, &target);
            drop(entries);
            Ok(())
        })();
        if let Err(error) = memory_commit {
            self.mark_durability_uncertain_v1();
            return Err(error);
        }

        Ok(SearchPlaneSearchCorpusRollbackCasAck {
            active: search_corpus_generation_into_contract(&target),
            previous_sealed_active: search_corpus_generation_into_contract(&current),
        })
    }

    #[cfg(test)]
    pub(crate) fn rollback(
        &self,
        request: &SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    ) -> Result<SearchPlaneSearchCorpusRollbackCasAck, CoreError> {
        let expected_active =
            search_corpus_generation_from_validated_rollback_identity(&request.expected_active)?;
        let guard = self
            .lifecycle_coordinator
            .lock_pair(expected_active.repo_id(), expected_active.revision_id())?;
        self.rollback_under_guard_v1(&guard, request)
    }

    pub fn resolve(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
    ) -> Result<GenerationPin, CoreError> {
        let record = self.resolve_record(repo_id, revision_id, track)?;
        Ok(GenerationPin::new(
            record.repo_id,
            record.revision_id,
            record.manifest_generation,
        ))
    }

    /// QI-ACT-01: return the full [`ActiveGenerationRecord`] for one
    /// `(repo, revision, track)` triple — same lookup as [`Self::resolve`]
    /// but preserves `manifest_digest` (load-bearing for the admin
    /// surface). Fail-closed on missing entry per CLAUDE.md safety rule.
    pub fn resolve_record(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
    ) -> Result<ActiveGenerationRecord, CoreError> {
        let key = ActivationKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
        };
        let entries = self.entries.read().map_err(|err| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
        })?;
        self.ensure_durability_certain_v1()?;
        entries.get(&key).cloned().ok_or_else(|| {
            CoreError::NotReady(format!(
                "activate-generation: no active {track:?} generation for repo={} revision={}",
                repo_id.as_str(),
                revision_id.as_str()
            ))
        })
    }

    /// QI-ACT-01: return every active generation record for one
    /// `(repo, revision)` pair, ordered by track declaration order (lexical
    /// before semantic). Empty Vec when nothing is activated yet — empty
    /// is a legitimate state, distinct from `NotReady` for a specific track.
    pub fn entries_for(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<ActiveGenerationRecord>, CoreError> {
        let mut records: Vec<ActiveGenerationRecord> = {
            let entries = self.entries.read().map_err(|err| {
                CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
            })?;
            self.ensure_durability_certain_v1()?;
            entries
                .iter()
                .filter(|(key, _)| key.repo_id == *repo_id && key.revision_id == *revision_id)
                .map(|(_, record)| record.clone())
                .collect()
        };
        // ActivationKey BTreeMap iteration is ordered by (repo, revision,
        // track); the filter preserves that, so `records` is already in
        // track-declaration order.
        records.sort_by(|a, b| a.track.cmp(&b.track));
        Ok(records)
    }

    fn ensure_durability_certain_v1(&self) -> Result<(), CoreError> {
        if self.durability_uncertain_v1.load(Ordering::Acquire) {
            return Err(CoreError::NotReady(
                "search-plane activation catalog: activation durability is uncertain; reopen the catalog before serving or mutating generations".to_string(),
            ));
        }
        Ok(())
    }

    fn mark_durability_uncertain_v1(&self) {
        self.durability_uncertain_v1.store(true, Ordering::Release);
    }
}

impl ActiveSearchCorpusPinReadPort for ActivationCatalog {
    fn active_search_corpus_under_guard_v1(
        &self,
        guard: &SearchCorpusPairMutationGuard<'_>,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Option<SearchCorpusGenerationV1>, CoreError> {
        guard.require_pair_v1(self.lifecycle_coordinator.as_ref(), repo_id, revision_id)?;
        self.ensure_durability_certain_v1()?;
        let entries = self.entries.read().map_err(|error| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {error}"))
        })?;
        active_search_corpus_generation_v1(&entries, repo_id, revision_id)
    }

    fn all_active_search_corpora_for_bootstrap_v1(
        &self,
    ) -> Result<Vec<SearchCorpusGenerationV1>, CoreError> {
        self.ensure_durability_certain_v1()?;
        let entries = self.entries.read().map_err(|error| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {error}"))
        })?;
        let mut active = Vec::new();
        for key in entries
            .keys()
            .filter(|key| key.track == SearchPlaneTrackKind::Lexical)
        {
            if let Some(generation) =
                active_search_corpus_generation_v1(&entries, &key.repo_id, &key.revision_id)?
            {
                active.push(generation);
            }
        }
        drop(entries);
        Ok(active)
    }
}

fn active_record_snapshot(record: &ActiveGenerationRecord) -> GenerationSnapshot {
    GenerationSnapshot {
        repo_id: record.repo_id.clone(),
        revision_id: record.revision_id.clone(),
        track: record.track,
        manifest_generation: record.manifest_generation,
        manifest_digest: record.manifest_digest.clone(),
    }
}

fn active_search_corpus_generation_v1(
    entries: &BTreeMap<ActivationKey, ActiveGenerationRecord>,
    repo_id: &RepoId,
    revision_id: &RevisionId,
) -> Result<Option<SearchCorpusGenerationV1>, CoreError> {
    let lexical = entries.get(&ActivationKey {
        repo_id: repo_id.clone(),
        revision_id: revision_id.clone(),
        track: SearchPlaneTrackKind::Lexical,
    });
    let semantic = entries.get(&ActivationKey {
        repo_id: repo_id.clone(),
        revision_id: revision_id.clone(),
        track: SearchPlaneTrackKind::Semantic,
    });
    match (lexical, semantic) {
        (None, None) => Ok(None),
        (Some(lexical), Some(semantic)) => Ok(Some(SearchCorpusGenerationV1::new(
            active_record_snapshot(lexical),
            active_record_snapshot(semantic),
        )?)),
        _ => Err(CoreError::NotReady(format!(
            "search-corpus activation: incomplete active composite root for repo={} revision={}",
            repo_id.as_str(),
            revision_id.as_str(),
        ))),
    }
}

fn search_corpus_generation_from_validated_rollback_identity(
    identity: &quanta_index_contract::SearchCorpusGenerationIdentityV1,
) -> Result<SearchCorpusGenerationV1, CoreError> {
    SearchCorpusGenerationV1::new(identity.lexical.clone(), identity.semantic.clone())
}

fn search_corpus_generation_into_contract(
    identity: &SearchCorpusGenerationV1,
) -> quanta_index_contract::SearchCorpusGenerationIdentityV1 {
    quanta_index_contract::SearchCorpusGenerationIdentityV1 {
        lexical: identity.lexical().clone(),
        semantic: identity.semantic().clone(),
    }
}

fn validate_prepared_search_corpus_expectation(
    prepared: &PreparedSearchCorpusGenerationV1,
    current: Option<&SearchCorpusGenerationV1>,
) -> Result<(), CoreError> {
    if prepared.expected_active() == current {
        return Ok(());
    }
    let describe = |identity: Option<&SearchCorpusGenerationV1>| {
        identity.map_or_else(
            || "absent".to_string(),
            |identity| {
                format!(
                    "generation={} digest={}",
                    identity.manifest_generation().get(),
                    identity.manifest_digest(),
                )
            },
        )
    };
    Err(CoreError::Typed {
        code: ERR_COMPOSITE_ACTIVATION_CAS_CONFLICT.to_string(),
        message: format!(
            "search-corpus activation: active composite root changed for repo={} revision={}: expected={}, observed={}",
            prepared.candidate().repo_id().as_str(),
            prepared.candidate().revision_id().as_str(),
            describe(prepared.expected_active()),
            describe(current),
        ),
    })
}

fn insert_search_corpus_generation_records(
    entries: &mut BTreeMap<ActivationKey, ActiveGenerationRecord>,
    generation: &SearchCorpusGenerationV1,
) {
    for snapshot in [generation.lexical(), generation.semantic()] {
        let key = ActivationKey {
            repo_id: snapshot.repo_id.clone(),
            revision_id: snapshot.revision_id.clone(),
            track: snapshot.track,
        };
        let _prior = entries.insert(
            key,
            ActiveGenerationRecord {
                repo_id: snapshot.repo_id.clone(),
                revision_id: snapshot.revision_id.clone(),
                manifest_generation: snapshot.manifest_generation,
                manifest_digest: snapshot.manifest_digest.clone(),
                track: snapshot.track,
            },
        );
    }
}

fn search_corpus_root_file_name(repo_id: &RepoId, revision_id: &RevisionId) -> String {
    format!(
        "{}--corpus.json",
        search_corpus_pair_digest(repo_id, revision_id)
    )
}

fn is_search_corpus_root_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("--corpus.json"))
}

fn validate_loaded_search_corpus_root_v1(
    path: &Path,
    generation: &SearchCorpusGenerationV1,
) -> Result<(), CoreError> {
    let expected_name =
        search_corpus_root_file_name(generation.repo_id(), generation.revision_id());
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err(CoreError::Storage(format!(
            "search-plane activation catalog: filename/payload identity mismatch: expected {expected_name}, observed {}",
            path.display()
        )));
    }
    Ok(())
}

trait ParentDirectorySyncPort: fmt::Debug + Send + Sync {
    fn sync_parent(&self, parent: &Path) -> std::io::Result<()>;
}

fn ensure_durable_directory_v1(
    path: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<(), CoreError> {
    let mut boundary = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "{owner}: durable directory has no parent: {}",
            path.display()
        ))
    })?;
    while !boundary.is_dir() {
        boundary = boundary.parent().ok_or_else(|| {
            CoreError::Storage(format!(
                "{owner}: durable directory has no existing ancestor: {}",
                path.display()
            ))
        })?;
    }
    ensure_durable_directory_from_boundary_v1(path, boundary, owner, parent_sync)
}

fn ensure_durable_directory_from_boundary_v1(
    path: &Path,
    boundary: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<(), CoreError> {
    if path == boundary {
        return Ok(());
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(CoreError::Storage(format!(
                    "{owner}: durable directory path is a symlink: {}",
                    path.display()
                )));
            }
            if !metadata.is_dir() {
                return Err(CoreError::Storage(format!(
                    "{owner}: durable directory path is not a directory: {}",
                    path.display()
                )));
            }
            let parent = path.parent().ok_or_else(|| {
                CoreError::Storage(format!(
                    "{owner}: durable directory has no parent: {}",
                    path.display()
                ))
            })?;
            ensure_durable_directory_from_boundary_v1(parent, boundary, owner, parent_sync)?;
            return parent_sync.sync_parent(parent).map_err(|err| {
                CoreError::Storage(format!(
                    "{owner}: revalidate durable-directory parent {}: {err}",
                    parent.display()
                ))
            });
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(CoreError::Storage(format!(
                "{owner}: inspect durable directory {}: {err}",
                path.display()
            )));
        }
    }
    let parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "{owner}: durable directory has no parent: {}",
            path.display()
        ))
    })?;
    ensure_durable_directory_from_boundary_v1(parent, boundary, owner, parent_sync)?;
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {}
        Err(err) => {
            return Err(CoreError::Storage(format!(
                "{owner}: create durable directory {}: {err}",
                path.display()
            )));
        }
    }
    parent_sync.sync_parent(parent).map_err(|err| {
        CoreError::Storage(format!(
            "{owner}: fsync durable-directory parent {}: {err}",
            parent.display()
        ))
    })
}

fn sync_existing_file_parent_v1(
    path: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<(), CoreError> {
    let parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "{owner}: durable file has no parent: {}",
            path.display()
        ))
    })?;
    parent_sync.sync_parent(parent).map_err(|err| {
        CoreError::Storage(format!(
            "{owner}: revalidate parent durability {}: {err}",
            parent.display()
        ))
    })
}

#[derive(Debug)]
struct FsParentDirectorySyncPort;

impl ParentDirectorySyncPort for FsParentDirectorySyncPort {
    fn sync_parent(&self, parent: &Path) -> std::io::Result<()> {
        File::open(parent)?.sync_all()
    }
}

enum AtomicFileWriteOutcomeV1 {
    Durable,
    RenamedButParentSyncFailed(CoreError),
}

fn atomic_replace_file_v1(
    path: &Path,
    bytes: &[u8],
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<AtomicFileWriteOutcomeV1, CoreError> {
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "{owner}: durable file has no parent: {}",
            path.display()
        ))
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "{owner}: durable file has no UTF-8 file name: {}",
                path.display()
            ))
        })?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{file_name}.tmp-{}-{sequence}",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|err| {
            CoreError::Storage(format!(
                "{owner}: create temporary {}: {err}",
                temporary.display()
            ))
        })?;
    if let Err(err) = file.write_all(bytes) {
        return Err(cleanup_activation_temporary_v1(
            &temporary,
            CoreError::Storage(format!(
                "{owner}: write temporary {}: {err}",
                temporary.display()
            )),
        ));
    }
    if let Err(err) = file.sync_all() {
        return Err(cleanup_activation_temporary_v1(
            &temporary,
            CoreError::Storage(format!(
                "{owner}: fsync temporary {}: {err}",
                temporary.display()
            )),
        ));
    }
    drop(file);
    if let Err(err) = fs::rename(&temporary, path) {
        return Err(cleanup_activation_temporary_v1(
            &temporary,
            CoreError::Storage(format!(
                "{owner}: rename temporary {} to {}: {err}",
                temporary.display(),
                path.display()
            )),
        ));
    }
    match parent_sync.sync_parent(parent) {
        Ok(()) => Ok(AtomicFileWriteOutcomeV1::Durable),
        Err(err) => Ok(AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(
            CoreError::Storage(format!("{owner}: fsync parent {}: {err}", parent.display())),
        )),
    }
}

fn atomic_replace_file_from_staging_v1(
    path: &Path,
    bytes: &[u8],
    staging_dir: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<AtomicFileWriteOutcomeV1, CoreError> {
    static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let target_parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "{owner}: durable file has no parent: {}",
            path.display()
        ))
    })?;
    let target_parent_name = target_parent
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "{owner}: target parent has no UTF-8 name: {}",
                target_parent.display()
            ))
        })?;
    let target_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "{owner}: durable file has no UTF-8 name: {}",
                path.display()
            ))
        })?;
    let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = staging_dir.join(format!(
        "scv1-{target_parent_name}-{target_name}.tmp-{}-{sequence}",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: create staging file {}: {error}",
                temporary.display()
            ))
        })?;
    if let Err(error) = file.write_all(bytes) {
        return Err(cleanup_staging_temporary_v1(
            &temporary,
            staging_dir,
            parent_sync,
            CoreError::Storage(format!(
                "{owner}: write staging file {}: {error}",
                temporary.display()
            )),
        ));
    }
    if let Err(error) = file.sync_all() {
        return Err(cleanup_staging_temporary_v1(
            &temporary,
            staging_dir,
            parent_sync,
            CoreError::Storage(format!(
                "{owner}: fsync staging file {}: {error}",
                temporary.display()
            )),
        ));
    }
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        return Err(cleanup_staging_temporary_v1(
            &temporary,
            staging_dir,
            parent_sync,
            CoreError::Storage(format!(
                "{owner}: rename staging file {} to {}: {error}",
                temporary.display(),
                path.display()
            )),
        ));
    }
    if let Err(error) = parent_sync.sync_parent(target_parent) {
        return Ok(AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(
            CoreError::Storage(format!(
                "{owner}: fsync target parent {}: {error}",
                target_parent.display()
            )),
        ));
    }
    match parent_sync.sync_parent(staging_dir) {
        Ok(()) => Ok(AtomicFileWriteOutcomeV1::Durable),
        Err(error) => Ok(AtomicFileWriteOutcomeV1::RenamedButParentSyncFailed(
            CoreError::Storage(format!(
                "{owner}: fsync staging parent {}: {error}",
                staging_dir.display()
            )),
        )),
    }
}

fn cleanup_staging_temporary_v1(
    temporary: &Path,
    staging_dir: &Path,
    parent_sync: &dyn ParentDirectorySyncPort,
    primary: CoreError,
) -> CoreError {
    match fs::remove_file(temporary) {
        Ok(()) => match parent_sync.sync_parent(staging_dir) {
            Ok(()) => primary,
            Err(error) => CoreError::Storage(format!(
                "{primary:?}; additionally failed to fsync staging cleanup {}: {error}",
                staging_dir.display()
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => primary,
        Err(error) => CoreError::Storage(format!(
            "{primary:?}; additionally failed to remove staging file {}: {error}",
            temporary.display()
        )),
    }
}

fn is_owned_search_corpus_staging_name_v1(name: &str) -> bool {
    let Some((target, suffix)) = name.split_once(".tmp-") else {
        return false;
    };
    if !target.starts_with("scv1-")
        || !target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return false;
    }
    let mut suffix = suffix.split('-');
    suffix
        .next()
        .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()))
        && suffix.next().is_some_and(|sequence| {
            !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
        })
        && suffix.next().is_none()
}

fn reconcile_owned_staging_directory_v1(
    staging_dir: &Path,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<(), CoreError> {
    let mut removed = false;
    for entry in fs::read_dir(staging_dir).map_err(|error| {
        CoreError::Storage(format!(
            "{owner}: list staging directory {}: {error}",
            staging_dir.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: read staging directory entry in {}: {error}",
                staging_dir.display()
            ))
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: inspect staging entry {}: {error}",
                path.display()
            ))
        })?;
        let owned_staging_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_owned_search_corpus_staging_name_v1);
        if !file_type.is_file() || !owned_staging_name {
            return Err(CoreError::Storage(format!(
                "{owner}: foreign staging entry {}",
                path.display()
            )));
        }
        fs::remove_file(&path).map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: remove abandoned staging file {}: {error}",
                path.display()
            ))
        })?;
        removed = true;
    }
    if removed {
        parent_sync.sync_parent(staging_dir).map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: fsync staging directory after reconciliation {}: {error}",
                staging_dir.display()
            ))
        })?;
    }
    Ok(())
}

fn reconcile_legacy_atomic_temporaries_v1(
    directory: &Path,
    target_suffix: &str,
    owner: &str,
    parent_sync: &dyn ParentDirectorySyncPort,
) -> Result<(), CoreError> {
    let mut removed = false;
    for entry in fs::read_dir(directory).map_err(|error| {
        CoreError::Storage(format!(
            "{owner}: list legacy temporary directory {}: {error}",
            directory.display()
        ))
    })? {
        let entry = entry.map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: read legacy temporary entry in {}: {error}",
                directory.display()
            ))
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !is_legacy_atomic_temporary_name_v1(name, target_suffix) {
            continue;
        }
        let file_type = entry.file_type().map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: inspect legacy temporary {}: {error}",
                path.display()
            ))
        })?;
        if !file_type.is_file() {
            return Err(CoreError::Storage(format!(
                "{owner}: legacy temporary is not a regular file: {}",
                path.display()
            )));
        }
        fs::remove_file(&path).map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: remove abandoned legacy temporary {}: {error}",
                path.display()
            ))
        })?;
        removed = true;
    }
    if removed {
        parent_sync.sync_parent(directory).map_err(|error| {
            CoreError::Storage(format!(
                "{owner}: fsync directory after legacy temporary reconciliation {}: {error}",
                directory.display()
            ))
        })?;
    }
    Ok(())
}

fn is_legacy_atomic_temporary_name_v1(name: &str, target_suffix: &str) -> bool {
    let Some((target, suffix)) = name.split_once(".tmp-") else {
        return false;
    };
    if !target.starts_with('.') || !target.ends_with(target_suffix) {
        return false;
    }
    let mut suffix = suffix.split('-');
    suffix
        .next()
        .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|byte| byte.is_ascii_digit()))
        && suffix.next().is_some_and(|sequence| {
            !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
        })
        && suffix.next().is_none()
}

fn cleanup_activation_temporary_v1(temporary: &Path, primary: CoreError) -> CoreError {
    match fs::remove_file(temporary) {
        Ok(()) => primary,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => primary,
        Err(cleanup) => CoreError::Storage(format!(
            "search-plane activation catalog: {primary:?}; additionally failed to remove temporary {}: {cleanup}",
            temporary.display()
        )),
    }
}

fn search_corpus_pair_digest(repo_id: &RepoId, revision_id: &RevisionId) -> String {
    let mut hasher = Sha256::new();
    for value in [repo_id.as_str(), revision_id.as_str()] {
        hasher.update(value.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(value.as_bytes());
    }
    let digest = hasher.finalize();
    format!("{digest:X}")
}

pub(crate) fn search_corpus_lock_stripe_v1(repo_id: &RepoId, revision_id: &RevisionId) -> usize {
    // Stable FNV-1a is used only to distribute lock contention. Collisions are
    // conservative serialization, never an authority or identity decision.
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in repo_id
        .as_str()
        .bytes()
        .chain(std::iter::once(0xff))
        .chain(revision_id.as_str().bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    usize::from(hash.to_le_bytes()[0] & 0x3f)
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::panic_in_result_fn,
        reason = "Result-returning durability and CAS tests use assertions as test-failure reporting"
    )]
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    use tempfile::tempdir;

    use quanta_index_contract::channel::LexicalChannelOp;
    use quanta_index_contract::lex::{
        LanguageCode, ParseNode, ParseTreeRecord, compute_parse_tree_source_hash,
    };
    use quanta_index_contract::{
        BatchIngestMode, ChunkId, ChunkRecord, GenerationSnapshot, ManifestGeneration,
        ReplaceLexicalScope, ReplaceStructuralScope, RepoId, RepoRelativePath, RevisionId,
        RuntimeCatalogIngestBatch, RuntimeChangedRecord, RuntimeDocFacetRecord,
        RuntimeEdgeAuthorityRecord, RuntimeSnapshotRecord, SearchCorpusGenerationIdentityV1,
        SearchCorpusReplaceScope, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
        SearchPlaneTrackKind, SearchScopeKey, SearchScopeSurface, StructuralReplaceScope,
        StructuralTreeRecord, UpsertParseTree,
    };
    use quanta_index_core::CoreError;

    use super::{
        ActivationCatalog, AuxiliaryAuthorityStore, Ledger, PreparedSearchCorpusGenerationV1,
        SearchCorpusGenerationV1, SearchCorpusHistoryRetentionPolicyV1,
    };
    use crate::SearchCorpusLifecycleOwner;
    use crate::search_corpus_lifecycle::SearchCorpusPairMutationCoordinator;

    fn search_corpus_retention(
        max_generations: usize,
    ) -> Result<SearchCorpusHistoryRetentionPolicyV1, CoreError> {
        SearchCorpusHistoryRetentionPolicyV1::new(
            max_generations,
            1024 * 1024,
            64,
            64 * 1024 * 1024,
        )
    }

    fn search_corpus_authority_record_len(
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) -> Result<u64, Box<dyn std::error::Error>> {
        let record = super::SearchCorpusAuthorityRecordV1::new(
            repo_id,
            revision_id,
            generation,
            manifest_digest,
        );
        Ok(u64::try_from(
            quanta_index_ipc::encode_cbor_payload(&record)?.len(),
        )?)
    }

    #[derive(Debug)]
    struct AlwaysFailParentSync;

    impl super::ParentDirectorySyncPort for AlwaysFailParentSync {
        fn sync_parent(&self, _parent: &Path) -> std::io::Result<()> {
            Err(std::io::Error::other("injected parent sync failure"))
        }
    }

    #[derive(Debug)]
    struct FailAtParentSync {
        calls: AtomicUsize,
        fail_at: usize,
    }

    impl super::ParentDirectorySyncPort for FailAtParentSync {
        fn sync_parent(&self, parent: &Path) -> std::io::Result<()> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call == self.fail_at {
                return Err(std::io::Error::other(format!(
                    "injected parent sync failure at call {call}"
                )));
            }
            std::fs::File::open(parent)?.sync_all()
        }
    }

    #[derive(Debug)]
    struct FailNthSyncForParent {
        target_parent: PathBuf,
        matching_calls: AtomicUsize,
        fail_at_matching_call: usize,
    }

    impl super::ParentDirectorySyncPort for FailNthSyncForParent {
        fn sync_parent(&self, parent: &Path) -> std::io::Result<()> {
            if parent == self.target_parent {
                let matching_call = self.matching_calls.fetch_add(1, Ordering::SeqCst);
                if matching_call == self.fail_at_matching_call {
                    return Err(std::io::Error::other(format!(
                        "injected parent sync failure for {} at matching call {matching_call}",
                        parent.display()
                    )));
                }
            }
            std::fs::File::open(parent)?.sync_all()
        }
    }

    #[derive(Debug)]
    struct ToggleParentSyncFailure {
        fail: AtomicBool,
    }

    impl super::ParentDirectorySyncPort for ToggleParentSyncFailure {
        fn sync_parent(&self, parent: &Path) -> std::io::Result<()> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(std::io::Error::other(format!(
                    "injected parent sync failure for {}",
                    parent.display()
                )));
            }
            std::fs::File::open(parent)?.sync_all()
        }
    }

    #[test]
    fn seals_are_monotonic_per_track() {
        let mut ledger = Ledger::default();
        ledger.lexical_seal(ManifestGeneration::new(7));
        ledger.lexical_seal(ManifestGeneration::new(3));
        ledger.semantic_seal(ManifestGeneration::new(2));
        ledger.semantic_seal(ManifestGeneration::new(5));

        assert_eq!(ledger.lexical_sealed(), Some(ManifestGeneration::new(7)));
        assert_eq!(ledger.semantic_sealed(), Some(ManifestGeneration::new(5)));
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn repo_id() -> RepoId {
        RepoId::new("repo")
    }

    fn revision_id() -> RevisionId {
        RevisionId::new("rev")
    }

    fn generation() -> ManifestGeneration {
        ManifestGeneration::new(17)
    }

    fn rust_language() -> Result<LanguageCode, Box<dyn std::error::Error>> {
        LanguageCode::new("rust")
            .map_err(|err| format!("invalid hard-coded language: {err}").into())
    }

    fn scope(path: &str) -> SearchScopeKey {
        SearchScopeKey {
            doc_surface: SearchScopeSurface::Chunk,
            repo_relative_path: RepoRelativePath::new(path),
        }
    }

    fn chunk_record(path: &str, text: &str) -> Result<ChunkRecord, Box<dyn std::error::Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new("chunk-1"),
            repo_relative_path: RepoRelativePath::new(path),
            language: rust_language()?,
            start_byte: 0,
            end_byte: u32::try_from(text.len())
                .map_err(|err| format!("text len overflow: {err}"))?,
            start_line: 1,
            end_line: 1,
            text: text.to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        })
    }

    fn parse_tree_record(text: &str) -> Result<ParseTreeRecord, Box<dyn std::error::Error>> {
        Ok(ParseTreeRecord {
            wire_version: 1,
            lang: rust_language()?,
            root: ParseNode {
                kind: "function_item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: u32::try_from(text.len())
                    .map_err(|err| format!("text len overflow: {err}"))?,
                children: Vec::new(),
            },
            source_hash: compute_parse_tree_source_hash(text),
            role_tag_schema_version: 0,
            role_tags: Vec::new(),
        })
    }

    fn encode_cbor<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        quanta_index_ipc::encode_cbor_payload(value)
            .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })
    }

    fn install_chunk(ledger: &mut Ledger, path: &str, text: &str) -> TestResult {
        install_chunk_with_id(ledger, "chunk-1", path, text)
    }

    fn install_chunk_with_id(
        ledger: &mut Ledger,
        chunk_id: &str,
        path: &str,
        text: &str,
    ) -> TestResult {
        let op = LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            payload: encode_cbor(&(
                BatchIngestMode::ReplaceGeneration,
                None::<ManifestGeneration>,
                SearchCorpusReplaceScope {
                    scope: scope(path),
                    scope_digest: "scope:lex".to_string(),
                    chunks: vec![{
                        let mut record = chunk_record(path, text)?;
                        record.chunk_id = ChunkId::new(chunk_id);
                        record
                    }],
                    symbols: Vec::new(),
                },
            ))?,
        });
        ledger.apply_lexical_authority_op(&op)?;
        Ok(())
    }

    fn install_parse_tree(ledger: &mut Ledger, text: &str) -> TestResult {
        let op = LexicalChannelOp::UpsertParseTree(UpsertParseTree {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            chunk_id: ChunkId::new("chunk-1"),
            payload: encode_cbor(&parse_tree_record(text)?)?,
        });
        ledger.apply_lexical_authority_op(&op)?;
        Ok(())
    }

    fn expect_decode_fail(err: CoreError, needle: &str) -> TestResult {
        match err {
            CoreError::Typed { code, message } => {
                if code != "STR_PARSE_TREE_DECODE_FAIL" {
                    return Err(format!("expected STR_PARSE_TREE_DECODE_FAIL, got {code}").into());
                }
                if !message.contains(needle) {
                    return Err(
                        format!("expected message to contain `{needle}`, got `{message}`").into(),
                    );
                }
                Ok(())
            }
            other @ (CoreError::InvalidContract(_)
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => {
                Err(format!("expected typed decode fail, got {other:?}").into())
            }
        }
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts persistence/resolution via assert_eq! macros"
    )]
    fn activation_catalog_persists_composite_root_and_rolls_back_both_tracks_v1() -> TestResult {
        let dir = tempdir()?;
        let catalog = ActivationCatalog::open(dir.path())?;
        let active = corpus_generation(17, "digest-17")?;
        let prepared = PreparedSearchCorpusGenerationV1::new(active, None)?;
        let activation = catalog.activate_prepared_search_corpus_generation_v1(&prepared)?;
        assert_eq!(
            activation.active.manifest_generation(),
            ManifestGeneration::new(17)
        );

        let pin = catalog.resolve(
            &RepoId::new("repo-corpus"),
            &RevisionId::new("rev-corpus"),
            SearchPlaneTrackKind::Lexical,
        )?;
        assert_eq!(pin.manifest_generation, ManifestGeneration::new(17));

        let reopened = ActivationCatalog::open(dir.path())?;
        let reopened_pin = reopened.resolve(
            &RepoId::new("repo-corpus"),
            &RevisionId::new("rev-corpus"),
            SearchPlaneTrackKind::Lexical,
        )?;
        assert_eq!(
            reopened_pin.manifest_generation,
            ManifestGeneration::new(17)
        );

        let semantic_before = reopened.resolve_record(
            &RepoId::new("repo-corpus"),
            &RevisionId::new("rev-corpus"),
            SearchPlaneTrackKind::Semantic,
        )?;
        assert_eq!(
            semantic_before.manifest_generation,
            ManifestGeneration::new(17)
        );

        let rollback = catalog.rollback(&SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: super::search_corpus_generation_into_contract(&corpus_generation(
                17,
                "digest-17",
            )?),
            target: super::search_corpus_generation_into_contract(&corpus_generation(
                16,
                "digest-16",
            )?),
        })?;
        assert_eq!(
            rollback.previous_sealed_active.lexical.manifest_generation,
            ManifestGeneration::new(17)
        );
        assert_eq!(
            rollback.active.lexical.manifest_generation,
            ManifestGeneration::new(16)
        );
        let reopened = ActivationCatalog::open(dir.path())?;
        let lexical_after = reopened.resolve_record(
            &RepoId::new("repo-corpus"),
            &RevisionId::new("rev-corpus"),
            SearchPlaneTrackKind::Lexical,
        )?;
        let semantic_after = reopened.resolve_record(
            &RepoId::new("repo-corpus"),
            &RevisionId::new("rev-corpus"),
            SearchPlaneTrackKind::Semantic,
        )?;
        assert_eq!(
            lexical_after.manifest_generation,
            ManifestGeneration::new(16)
        );
        assert_eq!(
            semantic_after.manifest_generation,
            ManifestGeneration::new(16)
        );
        assert_eq!(lexical_after.manifest_digest, "digest-16");
        assert_eq!(semantic_after.manifest_digest, "digest-16");
        Ok(())
    }

    fn corpus_snapshot(
        track: SearchPlaneTrackKind,
        generation: u64,
        digest: &str,
    ) -> GenerationSnapshot {
        GenerationSnapshot {
            repo_id: RepoId::new("repo-corpus"),
            revision_id: RevisionId::new("rev-corpus"),
            track,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: digest.to_string(),
        }
    }

    fn corpus_generation(
        generation: u64,
        digest: &str,
    ) -> Result<SearchCorpusGenerationV1, CoreError> {
        SearchCorpusGenerationV1::new(
            corpus_snapshot(SearchPlaneTrackKind::Lexical, generation, digest),
            corpus_snapshot(SearchPlaneTrackKind::Semantic, generation, digest),
        )
    }

    fn corpus_identity(generation: &SearchCorpusGenerationV1) -> SearchCorpusGenerationIdentityV1 {
        SearchCorpusGenerationIdentityV1 {
            lexical: generation.lexical().clone(),
            semantic: generation.semantic().clone(),
        }
    }

    fn assert_active_composite_v1(
        catalog: &ActivationCatalog,
        generation: &SearchCorpusGenerationV1,
    ) -> TestResult {
        for track in [
            SearchPlaneTrackKind::Lexical,
            SearchPlaneTrackKind::Semantic,
        ] {
            let observed =
                catalog.resolve_record(generation.repo_id(), generation.revision_id(), track)?;
            assert_eq!(
                observed.manifest_generation,
                generation.manifest_generation()
            );
            assert_eq!(observed.manifest_digest, generation.manifest_digest());
        }
        Ok(())
    }

    fn search_corpus_history_file_names_v1(
        store: &AuxiliaryAuthorityStore,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
        let pair_dir = store.search_corpus_pair_dir(repo_id, revision_id);
        if !pair_dir.exists() {
            return Ok(BTreeSet::new());
        }
        std::fs::read_dir(pair_dir)?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(Into::into)
    }

    #[test]
    fn prepared_search_corpus_generation_rejects_single_track_and_mixed_identity() -> TestResult {
        let single_track = SearchCorpusGenerationV1::new(
            corpus_snapshot(SearchPlaneTrackKind::Lexical, 17, "digest-17"),
            corpus_snapshot(SearchPlaneTrackKind::Lexical, 17, "digest-17"),
        );
        let Err(CoreError::InvalidContract(single_track_message)) = single_track else {
            return Err("lexical-only corpus generation unexpectedly constructed".into());
        };
        assert!(single_track_message.contains("lexical and semantic tracks"));

        let mixed_identity = SearchCorpusGenerationV1::new(
            corpus_snapshot(SearchPlaneTrackKind::Lexical, 17, "digest-17"),
            corpus_snapshot(SearchPlaneTrackKind::Semantic, 18, "digest-18"),
        );
        let Err(CoreError::InvalidContract(mixed_identity_message)) = mixed_identity else {
            return Err("mixed corpus generation unexpectedly constructed".into());
        };
        assert!(mixed_identity_message.contains("must match exactly"));

        let foreign_expected = SearchCorpusGenerationV1::new(
            GenerationSnapshot {
                repo_id: RepoId::new("repo-other"),
                revision_id: RevisionId::new("rev-corpus"),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(17),
                manifest_digest: "digest-17".to_string(),
            },
            GenerationSnapshot {
                repo_id: RepoId::new("repo-other"),
                revision_id: RevisionId::new("rev-corpus"),
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation: ManifestGeneration::new(17),
                manifest_digest: "digest-17".to_string(),
            },
        )?;
        let mismatched_expected = PreparedSearchCorpusGenerationV1::new(
            corpus_generation(17, "digest-17")?,
            Some(foreign_expected),
        );
        let Err(CoreError::InvalidContract(expected_message)) = mismatched_expected else {
            return Err("foreign expected active unexpectedly constructed".into());
        };
        assert!(expected_message.contains("candidate repo and revision"));
        Ok(())
    }

    #[test]
    fn prepared_search_corpus_activation_is_durable_before_reopen_and_rejects_stale_head()
    -> TestResult {
        let dir = tempdir()?;
        let catalog = ActivationCatalog::open(dir.path())?;
        let first = corpus_generation(17, "digest-17")?;
        let first_prepared = PreparedSearchCorpusGenerationV1::new(first.clone(), None)?;
        let first_receipt =
            catalog.activate_prepared_search_corpus_generation_v1(&first_prepared)?;
        assert_eq!(first_receipt.active, first);
        assert_eq!(first_receipt.previous_active, None);

        let root = dir.path().join(super::search_corpus_root_file_name(
            first.repo_id(),
            first.revision_id(),
        ));
        assert!(
            root.is_file(),
            "composite root must exist before memory receipt"
        );
        let directory_entries = std::fs::read_dir(dir.path())?.collect::<Result<Vec<_>, _>>()?;
        assert!(
            directory_entries
                .iter()
                .all(|entry| !entry.file_name().to_string_lossy().contains(".tmp-")),
            "durable activation must not leave temporary roots behind"
        );

        let second = corpus_generation(18, "digest-18")?;
        let promoted_prepared =
            PreparedSearchCorpusGenerationV1::new(second.clone(), Some(first.clone()))?;
        let promoted = catalog.activate_prepared_search_corpus_generation_v1(&promoted_prepared)?;
        assert_eq!(promoted.active, second);
        assert_eq!(promoted.previous_active, Some(first.clone()));

        let stale_prepared = PreparedSearchCorpusGenerationV1::new(
            corpus_generation(19, "digest-19")?,
            Some(first),
        )?;
        let stale = catalog.activate_prepared_search_corpus_generation_v1(&stale_prepared);
        let Err(CoreError::Typed { code, .. }) = stale else {
            return Err("stale composite expectation unexpectedly succeeded".into());
        };
        assert_eq!(code, super::ERR_COMPOSITE_ACTIVATION_CAS_CONFLICT);

        let reopened = ActivationCatalog::open(dir.path())?;
        let lexical = reopened.resolve_record(
            &RepoId::new("repo-corpus"),
            &RevisionId::new("rev-corpus"),
            SearchPlaneTrackKind::Lexical,
        )?;
        let semantic = reopened.resolve_record(
            &RepoId::new("repo-corpus"),
            &RevisionId::new("rev-corpus"),
            SearchPlaneTrackKind::Semantic,
        )?;
        assert_eq!(lexical.manifest_generation, ManifestGeneration::new(18));
        assert_eq!(lexical.manifest_generation, semantic.manifest_generation);
        assert_eq!(lexical.manifest_digest, semantic.manifest_digest);
        Ok(())
    }

    #[test]
    fn activation_catalog_concurrent_cas_promotions_select_one_composite_winner() -> TestResult {
        let dir = tempdir()?;
        let catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let active = corpus_generation(17, "digest-17")?;
        let initial_active = active.clone();
        let _initial_activation = catalog.activate_prepared_search_corpus_generation_v1(
            &PreparedSearchCorpusGenerationV1::new(active.clone(), None)?,
        )?;

        let first_candidate = corpus_generation(18, "digest-18")?;
        let second_candidate = corpus_generation(19, "digest-19")?;
        let first_prepared =
            PreparedSearchCorpusGenerationV1::new(first_candidate.clone(), Some(active.clone()))?;
        let second_prepared =
            PreparedSearchCorpusGenerationV1::new(second_candidate.clone(), Some(active))?;

        // Both contenders are fully prepared before either can enter the
        // catalog. The barrier releases their CAS calls together; winner
        // selection is intentionally unspecified, but split composite heads
        // and multiple successes are not.
        let start = Arc::new(Barrier::new(3));
        let first_catalog = Arc::clone(&catalog);
        let first_start = Arc::clone(&start);
        let first = thread::spawn(move || {
            let _barrier_receipt = first_start.wait();
            first_catalog.activate_prepared_search_corpus_generation_v1(&first_prepared)
        });
        let second_catalog = Arc::clone(&catalog);
        let second_start = Arc::clone(&start);
        let second = thread::spawn(move || {
            let _barrier_receipt = second_start.wait();
            second_catalog.activate_prepared_search_corpus_generation_v1(&second_prepared)
        });
        let _barrier_receipt = start.wait();

        let first_result = first
            .join()
            .map_err(|_join_error| "first activation contender panicked")?;
        let second_result = second
            .join()
            .map_err(|_join_error| "second activation contender panicked")?;

        let mut winner = None;
        for result in [first_result, second_result] {
            match result {
                Ok(receipt) => {
                    assert_eq!(receipt.previous_active, Some(initial_active.clone()));
                    if winner.replace(receipt.active).is_some() {
                        return Err("concurrent activation CAS admitted multiple winners".into());
                    }
                }
                Err(CoreError::Typed { code, .. }) => {
                    assert_eq!(code, super::ERR_COMPOSITE_ACTIVATION_CAS_CONFLICT);
                }
                Err(error) => {
                    return Err(format!(
                        "concurrent activation returned unexpected error: {error}"
                    )
                    .into());
                }
            }
        }
        let winner = winner.ok_or("concurrent activation CAS produced no winner")?;
        assert!(winner == first_candidate || winner == second_candidate);

        let lexical = catalog.resolve_record(
            winner.repo_id(),
            winner.revision_id(),
            SearchPlaneTrackKind::Lexical,
        )?;
        let semantic = catalog.resolve_record(
            winner.repo_id(),
            winner.revision_id(),
            SearchPlaneTrackKind::Semantic,
        )?;
        assert_eq!(lexical.manifest_generation, winner.manifest_generation());
        assert_eq!(semantic.manifest_generation, winner.manifest_generation());
        assert_eq!(lexical.manifest_digest, winner.manifest_digest());
        assert_eq!(semantic.manifest_digest, winner.manifest_digest());
        Ok(())
    }

    #[test]
    fn activation_catalog_fails_closed_after_durability_becomes_uncertain() -> TestResult {
        let dir = tempdir()?;
        let catalog = ActivationCatalog::open(dir.path())?;
        let first = corpus_generation(17, "digest-17")?;
        let activation = catalog.activate_prepared_search_corpus_generation_v1(
            &PreparedSearchCorpusGenerationV1::new(first.clone(), None)?,
        )?;
        assert_eq!(activation.active, first);

        // This is the post-rename / parent-fsync-failure state. The next
        // process reconstructs from the durable root; this one must never
        // serve its potentially stale in-memory records in the meantime.
        catalog.mark_durability_uncertain_v1();

        let resolve = catalog.resolve_record(
            first.repo_id(),
            first.revision_id(),
            SearchPlaneTrackKind::Lexical,
        );
        let Err(CoreError::NotReady(resolve_message)) = resolve else {
            return Err("durability-uncertain catalog unexpectedly served a read".into());
        };
        assert!(resolve_message.contains("reopen the catalog"));

        let second = corpus_generation(18, "digest-18")?;
        let mutate = catalog.activate_prepared_search_corpus_generation_v1(
            &PreparedSearchCorpusGenerationV1::new(second, Some(first))?,
        );
        let Err(CoreError::NotReady(mutate_message)) = mutate else {
            return Err("durability-uncertain catalog unexpectedly accepted a mutation".into());
        };
        assert!(mutate_message.contains("reopen the catalog"));
        Ok(())
    }

    #[test]
    fn activation_catalog_fences_real_post_rename_parent_sync_failure() -> TestResult {
        let dir = tempdir()?;
        let catalog = ActivationCatalog::open_with_parent_sync(
            dir.path(),
            SearchCorpusPairMutationCoordinator::shared(),
            Arc::new(FailAtParentSync {
                calls: AtomicUsize::new(0),
                // Existing-root durability revalidation is call zero and the
                // staging-directory creation fence is call one; the
                // activation-file target-parent sync after rename is call two.
                fail_at: 2,
            }),
        )?;
        let candidate = corpus_generation(17, "digest-17")?;
        let result = catalog.activate_prepared_search_corpus_generation_v1(
            &PreparedSearchCorpusGenerationV1::new(candidate.clone(), None)?,
        );
        let Err(CoreError::Storage(message)) = result else {
            return Err("injected parent sync failure unexpectedly activated".into());
        };
        assert!(message.contains("injected parent sync failure"));

        let persisted = dir.path().join(super::search_corpus_root_file_name(
            candidate.repo_id(),
            candidate.revision_id(),
        ));
        assert!(
            persisted.is_file(),
            "rename must precede injected sync failure"
        );
        let resolve = catalog.resolve_record(
            candidate.repo_id(),
            candidate.revision_id(),
            SearchPlaneTrackKind::Lexical,
        );
        assert!(matches!(resolve, Err(CoreError::NotReady(_))));

        let reopened = ActivationCatalog::open(dir.path())?;
        let active = reopened.resolve_record(
            candidate.repo_id(),
            candidate.revision_id(),
            SearchPlaneTrackKind::Lexical,
        )?;
        assert_eq!(active.manifest_generation, candidate.manifest_generation());
        Ok(())
    }

    #[test]
    fn activation_staging_write_failure_preserves_complete_active_pointer_v1() -> TestResult {
        let dir = tempdir()?;
        let catalog = ActivationCatalog::open(dir.path())?;
        let active = corpus_generation(17, "digest-17")?;
        let _activation = catalog.activate_prepared_search_corpus_generation_v1(
            &PreparedSearchCorpusGenerationV1::new(active.clone(), None)?,
        )?;
        let staging = dir.path().join(".staging");
        std::fs::remove_dir(&staging)?;
        std::fs::write(&staging, b"injected non-directory staging path")?;

        let candidate = corpus_generation(18, "digest-18")?;
        let rejected = catalog.activate_prepared_search_corpus_generation_v1(
            &PreparedSearchCorpusGenerationV1::new(candidate, Some(active.clone()))?,
        );
        let Err(CoreError::Storage(message)) = rejected else {
            return Err("activation staging write failure unexpectedly advanced the head".into());
        };
        assert!(message.contains("create staging file"));
        assert_active_composite_v1(&catalog, &active)?;

        std::fs::remove_file(&staging)?;
        std::fs::create_dir(&staging)?;
        let reopened = ActivationCatalog::open(dir.path())?;
        assert_active_composite_v1(&reopened, &active)
    }

    #[test]
    fn rollback_staging_write_failure_preserves_complete_active_pointer_v1() -> TestResult {
        let dir = tempdir()?;
        let catalog = ActivationCatalog::open(dir.path())?;
        let rollback_target = corpus_generation(17, "digest-17")?;
        let _first = catalog.activate_prepared_search_corpus_generation_v1(
            &PreparedSearchCorpusGenerationV1::new(rollback_target.clone(), None)?,
        )?;
        let active = corpus_generation(18, "digest-18")?;
        let _second = catalog.activate_prepared_search_corpus_generation_v1(
            &PreparedSearchCorpusGenerationV1::new(active.clone(), Some(rollback_target.clone()))?,
        )?;
        let staging = dir.path().join(".staging");
        std::fs::remove_dir(&staging)?;
        std::fs::write(&staging, b"injected non-directory staging path")?;

        let rejected = catalog.rollback(&SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: corpus_identity(&active),
            target: corpus_identity(&rollback_target),
        });
        let Err(CoreError::Storage(message)) = rejected else {
            return Err("rollback staging write failure unexpectedly moved the head".into());
        };
        assert!(message.contains("create staging file"));
        assert_active_composite_v1(&catalog, &active)?;

        std::fs::remove_file(&staging)?;
        std::fs::create_dir(&staging)?;
        let reopened = ActivationCatalog::open(dir.path())?;
        assert_active_composite_v1(&reopened, &active)
    }

    #[test]
    fn rollback_parent_sync_failure_fences_serving_and_rehydrates_one_composite_v1() -> TestResult {
        let dir = tempdir()?;
        let sync = Arc::new(ToggleParentSyncFailure {
            fail: AtomicBool::new(false),
        });
        let catalog = ActivationCatalog::open_with_parent_sync(
            dir.path(),
            SearchCorpusPairMutationCoordinator::shared(),
            sync.clone(),
        )?;
        let rollback_target = corpus_generation(17, "digest-17")?;
        let _first = catalog.activate_prepared_search_corpus_generation_v1(
            &PreparedSearchCorpusGenerationV1::new(rollback_target.clone(), None)?,
        )?;
        let active = corpus_generation(18, "digest-18")?;
        let _second = catalog.activate_prepared_search_corpus_generation_v1(
            &PreparedSearchCorpusGenerationV1::new(active.clone(), Some(rollback_target.clone()))?,
        )?;

        sync.fail.store(true, Ordering::SeqCst);
        let rejected = catalog.rollback(&SearchPlaneRollbackSearchCorpusGenerationCasRequest {
            expected_active: corpus_identity(&active),
            target: corpus_identity(&rollback_target),
        });
        assert!(matches!(rejected, Err(CoreError::Storage(_))));
        for track in [
            SearchPlaneTrackKind::Lexical,
            SearchPlaneTrackKind::Semantic,
        ] {
            assert!(matches!(
                catalog.resolve_record(active.repo_id(), active.revision_id(), track),
                Err(CoreError::NotReady(_))
            ));
        }

        sync.fail.store(false, Ordering::SeqCst);
        let reopened = ActivationCatalog::open(dir.path())?;
        assert_active_composite_v1(&reopened, &rollback_target)
    }

    #[test]
    fn injected_parent_sync_covers_fresh_catalog_and_authority_directories() -> TestResult {
        let dir = tempdir()?;
        let catalog_root = dir.path().join("fresh-catalog");
        let catalog = ActivationCatalog::open_with_parent_sync(
            &catalog_root,
            SearchCorpusPairMutationCoordinator::shared(),
            Arc::new(AlwaysFailParentSync),
        );
        let Err(CoreError::Storage(catalog_error)) = catalog else {
            return Err("fresh catalog bootstrap bypassed injected parent sync".into());
        };
        assert!(catalog_error.contains("injected parent sync failure"));

        let authority_root = dir.path().join("fresh-authority");
        let authority = AuxiliaryAuthorityStore::open_with_parent_sync(
            &authority_root,
            search_corpus_retention(2)?,
            SearchCorpusPairMutationCoordinator::shared(),
            Arc::new(super::NoActiveSearchCorpusPinsV1),
            Arc::new(AlwaysFailParentSync),
        );
        let Err(CoreError::Storage(authority_error)) = authority else {
            return Err("fresh authority bootstrap bypassed injected parent sync".into());
        };
        assert!(authority_error.contains("injected parent sync failure"));
        Ok(())
    }

    #[test]
    fn activation_catalog_rejects_legacy_per_track_root_before_decode() -> TestResult {
        let dir = tempdir()?;
        let legacy = dir.path().join("repo-corpus--rev-corpus--Lexical.json");
        std::fs::write(&legacy, b"{not-a-composite-root")?;
        let result = ActivationCatalog::open(dir.path());
        let Err(CoreError::Storage(message)) = result else {
            return Err("legacy per-track root unexpectedly opened".into());
        };
        assert!(message.contains("legacy per-track root is unsupported"));
        assert!(message.contains("Lexical.json"));
        Ok(())
    }

    #[test]
    fn activation_catalog_rejects_filename_payload_identity_mismatch() -> TestResult {
        let dir = tempdir()?;
        let generation = corpus_generation(17, "digest-17")?;
        let persisted = super::PersistedSearchCorpusGenerationRootV1::from_generation(&generation);
        let alias = dir.path().join("alias--alias--corpus.json");
        std::fs::write(&alias, serde_json::to_vec_pretty(&persisted)?)?;
        let result = ActivationCatalog::open(dir.path());
        let Err(CoreError::Storage(message)) = result else {
            return Err("filename/payload mismatch unexpectedly opened".into());
        };
        assert!(message.contains("filename/payload identity mismatch"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn activation_catalog_refuses_symlink_composite_root_v1() -> TestResult {
        use std::os::unix::fs::symlink;

        let dir = tempdir()?;
        let attacker_dir = tempdir()?;
        let generation = corpus_generation(17, "digest-17")?;
        let persisted = super::PersistedSearchCorpusGenerationRootV1::from_generation(&generation);
        let attacker_target = attacker_dir.path().join("attacker-controlled.json");
        std::fs::write(&attacker_target, serde_json::to_vec_pretty(&persisted)?)?;
        let activation_path = dir.path().join(super::search_corpus_root_file_name(
            generation.repo_id(),
            generation.revision_id(),
        ));
        symlink(&attacker_target, &activation_path)?;

        let result = ActivationCatalog::open(dir.path());
        let Err(CoreError::Storage(message)) = result else {
            return Err("activation catalog silently ignored a symlink composite root".into());
        };
        assert!(message.contains("not a regular non-symlink file"));
        Ok(())
    }

    #[test]
    fn sealed_search_corpus_history_reaps_max_plus_one_and_preserves_predecessor() -> TestResult {
        let dir = tempdir()?;
        let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
        let repo = RepoId::new("repo-corpus");
        let revision = RevisionId::new("rev-corpus");
        let mut live_ledger = Ledger::new();
        for generation in 17..=19 {
            let manifest_generation = ManifestGeneration::new(generation);
            let digest = format!("digest-{generation}");
            let retention = store.record_sealed_search_corpus(
                &repo,
                &revision,
                manifest_generation,
                digest.as_str(),
            )?;
            live_ledger.apply_search_corpus_history_retention_receipt_v1(
                &repo,
                &revision,
                manifest_generation,
                &retention,
            )?;
            live_ledger.record_historically_sealed_search_corpus(
                &repo,
                &revision,
                manifest_generation,
                digest.as_str(),
            );
        }
        let conflict = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(18),
            "conflicting-digest",
        );
        let Err(CoreError::Typed { code, .. }) = conflict else {
            return Err("conflicting historical digest unexpectedly overwrote authority".into());
        };
        assert_eq!(code, super::ERR_SEARCH_CORPUS_AUTHORITY_CONFLICT);
        let root_snapshot = store.load_search_corpus_root_snapshot_v1()?;
        assert_eq!(
            root_snapshot.pair_directories.len(),
            1,
            "one repo/revision history must occupy one bounded pair directory independently of the owned staging surface"
        );
        let pair_dir = store.search_corpus_pair_dir(&repo, &revision);
        assert_eq!(
            std::fs::read_dir(pair_dir)?.count(),
            2,
            "count policy must keep only the newest generation and predecessor"
        );
        assert!(
            !store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(17))
                .exists()
        );
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(18))
                .is_file()
        );
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(19))
                .is_file()
        );
        let reaped_same_process = live_ledger.validate_historically_sealed_track_identity(
            &GenerationSnapshot {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(17),
                manifest_digest: "digest-17".to_string(),
            },
            "test",
        );
        assert!(matches!(reaped_same_process, Err(CoreError::Typed { .. })));

        let reopened = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
        let mut ledger = Ledger::new();
        reopened.restore_into(&mut ledger)?;
        for track in [
            SearchPlaneTrackKind::Lexical,
            SearchPlaneTrackKind::Semantic,
        ] {
            ledger.validate_historically_sealed_track_identity(
                &GenerationSnapshot {
                    repo_id: repo.clone(),
                    revision_id: revision.clone(),
                    track,
                    manifest_generation: ManifestGeneration::new(18),
                    manifest_digest: "digest-18".to_string(),
                },
                "test",
            )?;
        }
        Ok(())
    }

    #[test]
    fn unreconciled_retention_receipt_fails_before_ledger_pruning() -> TestResult {
        let repo = RepoId::new("repo-incomplete-receipt");
        let revision = RevisionId::new("rev-incomplete-receipt");
        let mut ledger = Ledger::new();
        ledger.record_historically_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(17),
            "digest-17",
        );
        let receipt = super::SearchCorpusHistoryRetentionReceiptV1 {
            repo_id: repo.clone(),
            revision_id: revision.clone(),
            retained_generations: BTreeSet::from([ManifestGeneration::new(18)]),
            reaped_generations: BTreeSet::new(),
            store_reconciled_v1: false,
        };
        assert!(
            ledger
                .apply_search_corpus_history_retention_receipt_v1(
                    &repo,
                    &revision,
                    ManifestGeneration::new(18),
                    &receipt,
                )
                .is_err()
        );
        ledger.validate_historically_sealed_track_identity(
            &GenerationSnapshot {
                repo_id: repo,
                revision_id: revision,
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(17),
                manifest_digest: "digest-17".to_string(),
            },
            "test",
        )?;
        Ok(())
    }

    #[test]
    fn sealed_search_corpus_history_enforces_byte_cap_without_losing_predecessor() -> TestResult {
        let dir = tempdir()?;
        let repo = RepoId::new("repo-byte-cap");
        let revision = RevisionId::new("rev-byte-cap");
        let first_len = search_corpus_authority_record_len(
            &repo,
            &revision,
            ManifestGeneration::new(18),
            "digest-18",
        )?;
        let second_len = search_corpus_authority_record_len(
            &repo,
            &revision,
            ManifestGeneration::new(19),
            "digest-19",
        )?;
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(
            4,
            first_len + second_len,
            64,
            64 * 1024 * 1024,
        )?;
        let store = AuxiliaryAuthorityStore::open(dir.path(), policy)?;
        for generation in 18..=20 {
            let _retention_receipt = store.record_sealed_search_corpus(
                &repo,
                &revision,
                ManifestGeneration::new(generation),
                format!("digest-{generation}").as_str(),
            )?;
        }
        let pair_dir = store.search_corpus_pair_dir(&repo, &revision);
        assert_eq!(std::fs::read_dir(pair_dir)?.count(), 2);
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(19))
                .is_file()
        );
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(20))
                .is_file()
        );
        Ok(())
    }

    #[test]
    fn sealed_search_corpus_history_rejects_write_before_predecessor_window_overflows() -> TestResult
    {
        let dir = tempdir()?;
        let repo = RepoId::new("repo-byte-exhausted");
        let revision = RevisionId::new("rev-byte-exhausted");
        let first_len = search_corpus_authority_record_len(
            &repo,
            &revision,
            ManifestGeneration::new(18),
            "digest-18",
        )?;
        let second_len = search_corpus_authority_record_len(
            &repo,
            &revision,
            ManifestGeneration::new(19),
            "digest-19",
        )?;
        let pair_bytes = first_len
            .checked_add(second_len)
            .and_then(|sum| sum.checked_sub(1))
            .ok_or("test byte cap underflow")?;
        let policy =
            SearchCorpusHistoryRetentionPolicyV1::new(4, pair_bytes, 64, 64 * 1024 * 1024)?;
        let store = AuxiliaryAuthorityStore::open(dir.path(), policy)?;
        let _initial_retention_receipt = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(18),
            "digest-18",
        )?;
        let rejected = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(19),
            "digest-19",
        );
        let Err(CoreError::Typed { code, .. }) = rejected else {
            return Err("byte-exhausted predecessor window unexpectedly admitted".into());
        };
        assert_eq!(code, super::ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED);
        assert_eq!(
            std::fs::read_dir(store.search_corpus_pair_dir(&repo, &revision))?.count(),
            1,
            "preflight must reject before a durable max+1 write"
        );
        Ok(())
    }

    #[test]
    fn state_root_revision_pair_cap_rejects_growth_without_cross_pair_deletion() -> TestResult {
        let dir = tempdir()?;
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 1, 4 * 1024 * 1024)?;
        let store = AuxiliaryAuthorityStore::open(dir.path(), policy)?;
        let first_repo = RepoId::new("repo-global-first");
        let first_revision = RevisionId::new("rev-global-first");
        let _first_pair_retention_receipt = store.record_sealed_search_corpus(
            &first_repo,
            &first_revision,
            ManifestGeneration::new(1),
            "digest-first",
        )?;

        let second_repo = RepoId::new("repo-global-second");
        let second_revision = RevisionId::new("rev-global-second");
        let rejected = store.record_sealed_search_corpus(
            &second_repo,
            &second_revision,
            ManifestGeneration::new(1),
            "digest-second",
        );
        let Err(CoreError::Typed { code, .. }) = rejected else {
            return Err("state-root revision-pair overflow unexpectedly admitted".into());
        };
        assert_eq!(code, super::ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED);
        assert!(
            store
                .search_corpus_authority_path(
                    &first_repo,
                    &first_revision,
                    ManifestGeneration::new(1),
                )
                .is_file(),
            "global admission must not delete another pair without active-pin authority"
        );
        assert!(
            !store
                .search_corpus_pair_dir(&second_repo, &second_revision)
                .exists(),
            "rejected admission must not leave an empty durable pair directory"
        );
        Ok(())
    }

    #[test]
    fn state_root_total_byte_cap_rejects_growth_before_write() -> TestResult {
        let dir = tempdir()?;
        let first_repo = RepoId::new("repo-byte-root-first");
        let first_revision = RevisionId::new("rev-byte-root-first");
        let second_repo = RepoId::new("repo-byte-root-second");
        let second_revision = RevisionId::new("rev-byte-root-second");
        let first_len = search_corpus_authority_record_len(
            &first_repo,
            &first_revision,
            ManifestGeneration::new(1),
            "digest-first",
        )?;
        let second_len = search_corpus_authority_record_len(
            &second_repo,
            &second_revision,
            ManifestGeneration::new(1),
            "digest-second",
        )?;
        let pair_limit = first_len.max(second_len);
        let store = AuxiliaryAuthorityStore::open(
            dir.path(),
            SearchCorpusHistoryRetentionPolicyV1::new(2, pair_limit, 8, pair_limit)?,
        )?;
        let _first_pair_retention_receipt = store.record_sealed_search_corpus(
            &first_repo,
            &first_revision,
            ManifestGeneration::new(1),
            "digest-first",
        )?;
        let rejected = store.record_sealed_search_corpus(
            &second_repo,
            &second_revision,
            ManifestGeneration::new(1),
            "digest-second",
        );
        let Err(CoreError::Typed { code, .. }) = rejected else {
            return Err("state-root byte overflow unexpectedly admitted".into());
        };
        assert_eq!(code, super::ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED);
        assert!(
            !store
                .search_corpus_pair_dir(&second_repo, &second_revision)
                .exists()
        );
        Ok(())
    }

    #[test]
    fn restore_refuses_cross_pair_gc_when_state_root_pair_cap_shrinks() -> TestResult {
        let dir = tempdir()?;
        let writer = AuxiliaryAuthorityStore::open(
            dir.path(),
            SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 2, 4 * 1024 * 1024)?,
        )?;
        for ordinal in 1..=2 {
            let _retention_receipt = writer.record_sealed_search_corpus(
                &RepoId::new(format!("repo-shrink-{ordinal}")),
                &RevisionId::new(format!("rev-shrink-{ordinal}")),
                ManifestGeneration::new(1),
                format!("digest-{ordinal}").as_str(),
            )?;
        }
        drop(writer);

        let reopened = AuxiliaryAuthorityStore::open(
            dir.path(),
            SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 1, 4 * 1024 * 1024)?,
        )?;
        let mut ledger = Ledger::new();
        let Err(CoreError::Typed { code, .. }) = reopened.restore_into(&mut ledger) else {
            return Err("restore guessed a cross-pair GC victim".into());
        };
        assert_eq!(code, super::ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED);
        assert_eq!(
            reopened.load_search_corpus_root_snapshot_v1()?.pairs.len(),
            2,
            "failed restore must preserve every pair when active-pin authority is unavailable"
        );
        Ok(())
    }

    #[test]
    fn restart_repairs_one_over_limit_before_restoring_history() -> TestResult {
        let dir = tempdir()?;
        let repo = RepoId::new("repo-restart-gc");
        let revision = RevisionId::new("rev-restart-gc");
        let writer = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(3)?)?;
        for generation in 17..=19 {
            let _retention_receipt = writer.record_sealed_search_corpus(
                &repo,
                &revision,
                ManifestGeneration::new(generation),
                format!("digest-{generation}").as_str(),
            )?;
        }
        drop(writer);

        let reopened = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
        let mut ledger = Ledger::new();
        reopened.restore_into(&mut ledger)?;
        assert_eq!(
            std::fs::read_dir(reopened.search_corpus_pair_dir(&repo, &revision))?.count(),
            2
        );
        for generation in [18, 19] {
            ledger.validate_historically_sealed_track_identity(
                &GenerationSnapshot {
                    repo_id: repo.clone(),
                    revision_id: revision.clone(),
                    track: SearchPlaneTrackKind::Lexical,
                    manifest_generation: ManifestGeneration::new(generation),
                    manifest_digest: format!("digest-{generation}"),
                },
                "test",
            )?;
        }
        Ok(())
    }

    #[test]
    fn restore_fails_closed_on_foreign_pair_entry() -> TestResult {
        let dir = tempdir()?;
        let repo = RepoId::new("repo-foreign");
        let revision = RevisionId::new("rev-foreign");
        let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
        let _retention_receipt = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(17),
            "digest-17",
        )?;
        std::fs::write(
            store
                .search_corpus_pair_dir(&repo, &revision)
                .join("foreign.tmp"),
            b"foreign",
        )?;
        let mut ledger = Ledger::new();
        let Err(CoreError::Storage(message)) = store.restore_into(&mut ledger) else {
            return Err("foreign history entry unexpectedly ignored".into());
        };
        assert!(message.contains("foreign history entry"));
        Ok(())
    }

    #[test]
    fn startup_reconciles_owned_staging_and_empty_pair_v1() -> TestResult {
        let dir = tempdir()?;
        let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
        let staging = store.search_corpus_staging_dir.clone();
        let empty_pair = store.search_corpus_dir.join("a".repeat(64));
        std::fs::write(staging.join("scv1-pair-g17.cbor.tmp-1-1"), b"abandoned")?;
        std::fs::create_dir(&empty_pair)?;
        drop(store);

        let reopened = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
        assert!(
            std::fs::read_dir(&reopened.search_corpus_staging_dir)?
                .next()
                .is_none()
        );
        assert!(!empty_pair.exists());
        Ok(())
    }

    #[test]
    fn retention_missing_active_history_preserves_activation_and_empty_history_v1() -> TestResult {
        let dir = tempdir()?;
        let owner = SearchCorpusLifecycleOwner::open(dir.path(), search_corpus_retention(2)?)?;
        let catalog = owner.activation_catalog();
        let store = owner.authority_store();
        let active = corpus_generation(1, "digest-active-1")?;
        let coordinator = owner.coordinator();
        {
            let guard = coordinator.lock_pair(active.repo_id(), active.revision_id())?;
            let _activation = catalog.activate_prepared_under_guard_v1(
                &guard,
                &PreparedSearchCorpusGenerationV1::new(active.clone(), None)?,
            )?;
        }
        let history_before =
            search_corpus_history_file_names_v1(&store, active.repo_id(), active.revision_id())?;
        assert!(history_before.is_empty());

        let rejected = store.record_sealed_search_corpus(
            active.repo_id(),
            active.revision_id(),
            ManifestGeneration::new(2),
            "digest-candidate-2",
        );
        let Err(CoreError::Storage(message)) = rejected else {
            return Err("retention admitted a candidate without durable active history".into());
        };
        assert!(message.contains("active generation is absent from durable history"));
        assert_active_composite_v1(&catalog, &active)?;
        assert_eq!(
            search_corpus_history_file_names_v1(&store, active.repo_id(), active.revision_id(),)?,
            history_before
        );
        Ok(())
    }

    #[test]
    fn retention_active_digest_mismatch_preserves_activation_and_history_v1() -> TestResult {
        let dir = tempdir()?;
        let owner = SearchCorpusLifecycleOwner::open(dir.path(), search_corpus_retention(2)?)?;
        let catalog = owner.activation_catalog();
        let store = owner.authority_store();
        let active = corpus_generation(1, "digest-active-1")?;
        let _history = store.record_sealed_search_corpus(
            active.repo_id(),
            active.revision_id(),
            active.manifest_generation(),
            "digest-history-1",
        )?;
        let coordinator = owner.coordinator();
        {
            let guard = coordinator.lock_pair(active.repo_id(), active.revision_id())?;
            let _activation = catalog.activate_prepared_under_guard_v1(
                &guard,
                &PreparedSearchCorpusGenerationV1::new(active.clone(), None)?,
            )?;
        }
        let history_before =
            search_corpus_history_file_names_v1(&store, active.repo_id(), active.revision_id())?;

        let rejected = store.record_sealed_search_corpus(
            active.repo_id(),
            active.revision_id(),
            ManifestGeneration::new(2),
            "digest-candidate-2",
        );
        let Err(CoreError::Storage(message)) = rejected else {
            return Err("retention admitted an active digest absent from durable history".into());
        };
        assert!(message.contains("active generation is absent from durable history"));
        assert_active_composite_v1(&catalog, &active)?;
        assert_eq!(
            search_corpus_history_file_names_v1(&store, active.repo_id(), active.revision_id(),)?,
            history_before
        );
        assert_eq!(
            store.inspect_sealed_search_corpus(
                active.repo_id(),
                active.revision_id(),
                active.manifest_generation(),
                "digest-history-1",
            )?,
            super::SealedSearchCorpusAuthorityStateV1::Exact
        );
        Ok(())
    }

    #[test]
    fn retention_required_set_exhaustion_preserves_activation_and_history_v1() -> TestResult {
        let dir = tempdir()?;
        let repo = RepoId::new("repo-required-set-exhausted");
        let revision = RevisionId::new("rev-required-set-exhausted");
        let active_digest = "digest-active-1";
        let candidate_digest = "digest-candidate-2";
        let active_len = search_corpus_authority_record_len(
            &repo,
            &revision,
            ManifestGeneration::new(1),
            active_digest,
        )?;
        let candidate_len = search_corpus_authority_record_len(
            &repo,
            &revision,
            ManifestGeneration::new(2),
            candidate_digest,
        )?;
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(
            2,
            active_len.max(candidate_len),
            8,
            8 * 1024 * 1024,
        )?;
        let owner = SearchCorpusLifecycleOwner::open(dir.path(), policy)?;
        let catalog = owner.activation_catalog();
        let store = owner.authority_store();
        let active = SearchCorpusGenerationV1::new(
            GenerationSnapshot {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(1),
                manifest_digest: active_digest.to_string(),
            },
            GenerationSnapshot {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation: ManifestGeneration::new(1),
                manifest_digest: active_digest.to_string(),
            },
        )?;
        let _history = store.record_sealed_search_corpus(
            &repo,
            &revision,
            active.manifest_generation(),
            active_digest,
        )?;
        let coordinator = owner.coordinator();
        {
            let guard = coordinator.lock_pair(&repo, &revision)?;
            let _activation = catalog.activate_prepared_under_guard_v1(
                &guard,
                &PreparedSearchCorpusGenerationV1::new(active.clone(), None)?,
            )?;
        }
        let history_before = search_corpus_history_file_names_v1(&store, &repo, &revision)?;

        let rejected = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(2),
            candidate_digest,
        );
        let Err(CoreError::Typed { code, .. }) = rejected else {
            return Err("required active/candidate set unexpectedly fit below byte cap".into());
        };
        assert_eq!(code, super::ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED);
        assert_active_composite_v1(&catalog, &active)?;
        assert_eq!(
            search_corpus_history_file_names_v1(&store, &repo, &revision)?,
            history_before
        );
        assert!(
            !store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(2),)
                .exists(),
            "retention preflight must reject before candidate write"
        );
        Ok(())
    }

    #[test]
    fn retention_preserves_rolled_back_active_generation_before_next_activation_v1() -> TestResult {
        let dir = tempdir()?;
        let owner = SearchCorpusLifecycleOwner::open(dir.path(), search_corpus_retention(2)?)?;
        let store = owner.authority_store();
        let catalog = owner.activation_catalog();
        let repo = RepoId::new("repo-corpus");
        let revision = RevisionId::new("rev-corpus");
        for raw_generation in 1..=2 {
            let _receipt = store.record_sealed_search_corpus(
                &repo,
                &revision,
                ManifestGeneration::new(raw_generation),
                &format!("digest-{raw_generation}"),
            )?;
        }
        let generation_one = corpus_generation(1, "digest-1")?;
        let generation_two = corpus_generation(2, "digest-2")?;
        let coordinator = owner.coordinator();
        {
            let guard = coordinator.lock_pair(&repo, &revision)?;
            let _activation = catalog.activate_prepared_under_guard_v1(
                &guard,
                &PreparedSearchCorpusGenerationV1::new(generation_two.clone(), None)?,
            )?;
        }
        {
            let guard = coordinator.lock_pair(&repo, &revision)?;
            let _rollback = catalog.rollback_under_guard_v1(
                &guard,
                &SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                    expected_active: corpus_identity(&generation_two),
                    target: corpus_identity(&generation_one),
                },
            )?;
        }

        let _receipt = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(3),
            "digest-3",
        )?;
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(1))
                .is_file()
        );
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(3))
                .is_file()
        );
        assert!(
            !store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(2))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn startup_rejects_foreign_staging_entry_v1() -> TestResult {
        let dir = tempdir()?;
        let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
        std::fs::write(
            store.search_corpus_staging_dir.join("foreign.tmp"),
            b"foreign",
        )?;
        drop(store);

        let Err(CoreError::Storage(message)) =
            AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)
        else {
            return Err("foreign staging entry unexpectedly reconciled".into());
        };
        assert!(message.contains("foreign staging entry"));
        Ok(())
    }

    #[test]
    fn startup_rejects_non_hex_pair_directory_v1() -> TestResult {
        let dir = tempdir()?;
        let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
        let foreign_pair = store.search_corpus_dir.join("G".repeat(64));
        std::fs::create_dir(&foreign_pair)?;
        drop(store);

        let Err(CoreError::Storage(message)) =
            AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)
        else {
            return Err("non-hex pair directory unexpectedly reconciled".into());
        };
        assert!(message.contains("foreign root entry"));
        assert!(foreign_pair.exists());
        Ok(())
    }

    #[test]
    fn concurrent_history_writes_serialize_gc_and_remain_bounded() -> TestResult {
        let dir = tempdir()?;
        let store = Arc::new(AuxiliaryAuthorityStore::open(
            dir.path(),
            search_corpus_retention(3)?,
        )?);
        let repo = RepoId::new("repo-concurrent-gc");
        let revision = RevisionId::new("rev-concurrent-gc");
        let barrier = Arc::new(Barrier::new(8));
        let mut workers = Vec::new();
        for generation in 1..=8 {
            let worker_store = store.clone();
            let worker_repo = repo.clone();
            let worker_revision = revision.clone();
            let worker_barrier = barrier.clone();
            workers.push(thread::spawn(move || {
                let _barrier_receipt = worker_barrier.wait();
                worker_store.record_sealed_search_corpus(
                    &worker_repo,
                    &worker_revision,
                    ManifestGeneration::new(generation),
                    format!("digest-{generation}").as_str(),
                )
            }));
        }
        for worker in workers {
            match worker
                .join()
                .map_err(|_panic_payload| "history GC worker panicked")?
            {
                Ok(_receipt) => {}
                Err(CoreError::Typed { code, .. })
                    if code == super::ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED => {}
                Err(error) => return Err(format!("unexpected concurrent GC error: {error}").into()),
            }
        }
        assert_eq!(
            std::fs::read_dir(store.search_corpus_pair_dir(&repo, &revision))?.count(),
            3
        );
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(8))
                .is_file()
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn search_corpus_authority_refuses_symlink_records() -> TestResult {
        use std::os::unix::fs::symlink;

        let dir = tempdir()?;
        let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
        let repo = RepoId::new("repo-symlink");
        let revision = RevisionId::new("rev-symlink");
        let generation = ManifestGeneration::new(17);
        let record_path = store.search_corpus_authority_path(&repo, &revision, generation);
        let parent = record_path.parent().ok_or("authority path has no parent")?;
        std::fs::create_dir_all(parent)?;
        let attacker_target = dir.path().join("attacker-controlled.cbor");
        std::fs::write(&attacker_target, b"not-authority")?;
        symlink(&attacker_target, &record_path)?;

        let observed =
            store.inspect_sealed_search_corpus(&repo, &revision, generation, "digest-17");
        let Err(CoreError::Storage(message)) = observed else {
            return Err("authority store followed a symlink record".into());
        };
        assert!(message.contains("read search corpus"));
        Ok(())
    }

    #[test]
    fn durable_pair_names_are_bounded_and_length_delimited() {
        let first = super::search_corpus_pair_digest(
            &RepoId::new("repo--with--delimiter"),
            &RevisionId::new("rev"),
        );
        let second = super::search_corpus_pair_digest(
            &RepoId::new("repo"),
            &RevisionId::new("with--delimiter--rev"),
        );
        assert_ne!(first, second);
        assert_eq!(first.len(), 64);

        let long = super::search_corpus_root_file_name(
            &RepoId::new("r".repeat(8_192)),
            &RevisionId::new("v".repeat(8_192)),
        );
        assert_eq!(long.len(), 64 + "--corpus.json".len());
    }

    #[test]
    fn sealed_search_corpus_retry_revalidates_parent_durability() -> TestResult {
        let dir = tempdir()?;
        drop(AuxiliaryAuthorityStore::open(
            dir.path(),
            search_corpus_retention(2)?,
        )?);
        let sync = Arc::new(FailAtParentSync {
            calls: AtomicUsize::new(0),
            // Revalidating the authority root plus five owned directories
            // consumes calls 0..=5. Fail pair-directory durability at call six
            // so the retry must revalidate that existing directory.
            fail_at: 6,
        });
        let store = AuxiliaryAuthorityStore::open_with_parent_sync(
            dir.path(),
            search_corpus_retention(2)?,
            SearchCorpusPairMutationCoordinator::shared(),
            Arc::new(super::NoActiveSearchCorpusPinsV1),
            sync.clone(),
        )?;
        let repo = RepoId::new("repo-retry");
        let revision = RevisionId::new("rev-retry");
        let first = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(17),
            "digest-17",
        );
        assert!(matches!(first, Err(CoreError::Storage(_))));

        let _retry_retention_receipt = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(17),
            "digest-17",
        )?;
        assert!(
            sync.calls.load(Ordering::SeqCst) >= 8,
            "retry must re-fsync the pair-directory parent and immutable record parent"
        );
        Ok(())
    }

    #[test]
    fn sealed_search_corpus_retry_repairs_post_rename_parent_sync_failure() -> TestResult {
        let dir = tempdir()?;
        drop(AuxiliaryAuthorityStore::open(
            dir.path(),
            search_corpus_retention(2)?,
        )?);
        let sync = Arc::new(FailAtParentSync {
            calls: AtomicUsize::new(0),
            // Revalidating the authority root plus five owned directories
            // consumes calls 0..=5, pair-directory durability is call six,
            // and target-parent sync after the immutable rename is call seven.
            fail_at: 7,
        });
        let store = AuxiliaryAuthorityStore::open_with_parent_sync(
            dir.path(),
            search_corpus_retention(2)?,
            SearchCorpusPairMutationCoordinator::shared(),
            Arc::new(super::NoActiveSearchCorpusPinsV1),
            sync.clone(),
        )?;
        let repo = RepoId::new("repo-post-rename");
        let revision = RevisionId::new("rev-post-rename");
        let generation = ManifestGeneration::new(17);
        let first =
            store.record_sealed_search_corpus(&repo, &revision, generation, "digest-post-rename");
        assert!(matches!(first, Err(CoreError::Storage(_))));
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, generation)
                .is_file(),
            "rename must precede the injected parent sync failure"
        );

        let _retry_retention_receipt = store.record_sealed_search_corpus(
            &repo,
            &revision,
            generation,
            "digest-post-rename",
        )?;
        assert!(sync.calls.load(Ordering::SeqCst) >= 4);
        assert_eq!(
            store.inspect_sealed_search_corpus(
                &repo,
                &revision,
                generation,
                "digest-post-rename",
            )?,
            super::SealedSearchCorpusAuthorityStateV1::Exact
        );
        Ok(())
    }

    #[test]
    fn sealed_search_corpus_retry_repairs_staging_parent_sync_failure_v1() -> TestResult {
        let dir = tempdir()?;
        drop(AuxiliaryAuthorityStore::open(
            dir.path(),
            search_corpus_retention(2)?,
        )?);
        let staging_dir = dir.path().join("search-corpus/.staging");
        let sync = Arc::new(FailNthSyncForParent {
            target_parent: staging_dir,
            matching_calls: AtomicUsize::new(0),
            fail_at_matching_call: 0,
        });
        let store = AuxiliaryAuthorityStore::open_with_parent_sync(
            dir.path(),
            search_corpus_retention(2)?,
            SearchCorpusPairMutationCoordinator::shared(),
            Arc::new(super::NoActiveSearchCorpusPinsV1),
            sync.clone(),
        )?;
        let repo = RepoId::new("repo-staging-retry");
        let revision = RevisionId::new("rev-staging-retry");
        let generation = ManifestGeneration::new(17);

        let first =
            store.record_sealed_search_corpus(&repo, &revision, generation, "digest-staging-retry");
        assert!(matches!(first, Err(CoreError::Storage(_))));
        assert!(
            store
                .search_corpus_authority_path(&repo, &revision, generation)
                .is_file(),
            "target rename must precede the injected staging-parent sync failure"
        );

        let _reconciled = store.record_sealed_search_corpus(
            &repo,
            &revision,
            generation,
            "digest-staging-retry",
        )?;
        assert_eq!(
            sync.matching_calls.load(Ordering::SeqCst),
            2,
            "exact retry must re-fsync the source staging directory"
        );
        Ok(())
    }

    #[test]
    fn post_delete_fsync_failure_fences_rollback_and_retry_reconciles_authoritative_set()
    -> TestResult {
        let dir = tempdir()?;
        let repo = RepoId::new("repo-post-delete");
        let revision = RevisionId::new("rev-post-delete");
        let writer = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(4)?)?;
        let mut ledger = Ledger::new();
        for generation in 1..=4 {
            let generation = ManifestGeneration::new(generation);
            let digest = format!("digest-{}", generation.get());
            let _retention_receipt =
                writer.record_sealed_search_corpus(&repo, &revision, generation, &digest)?;
            ledger.record_historically_sealed_search_corpus(&repo, &revision, generation, &digest);
        }
        let pair_dir = writer.search_corpus_pair_dir(&repo, &revision);
        drop(writer);

        let sync = Arc::new(FailNthSyncForParent {
            target_parent: pair_dir,
            matching_calls: AtomicUsize::new(0),
            // The first pair sync revalidates the existing generation record;
            // the second is the post-delete durability barrier.
            fail_at_matching_call: 1,
        });
        let store = AuxiliaryAuthorityStore::open_with_parent_sync(
            dir.path(),
            search_corpus_retention(2)?,
            SearchCorpusPairMutationCoordinator::shared(),
            Arc::new(super::NoActiveSearchCorpusPinsV1),
            sync.clone(),
        )?;
        ledger.fence_search_corpus_history_v1(&repo, &revision);
        let first = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(4),
            "digest-4",
        );
        assert!(matches!(first, Err(CoreError::Storage(_))));
        assert_eq!(
            sync.matching_calls.load(Ordering::SeqCst),
            2,
            "failure must occur at the post-delete pair-directory durability barrier"
        );
        assert!(
            !store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(1))
                .exists()
        );
        assert!(
            !store
                .search_corpus_authority_path(&repo, &revision, ManifestGeneration::new(2))
                .exists()
        );
        let fenced = ledger.validate_historically_sealed_track_identity(
            &GenerationSnapshot {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(3),
                manifest_digest: "digest-3".to_string(),
            },
            "post-delete retry",
        );
        assert!(matches!(fenced, Err(CoreError::NotReady(_))));

        let reconciled = store.record_sealed_search_corpus(
            &repo,
            &revision,
            ManifestGeneration::new(4),
            "digest-4",
        )?;
        assert!(
            reconciled.reaped_generations().is_empty(),
            "retry observes files already deleted before the failed fsync"
        );
        ledger.apply_search_corpus_history_retention_receipt_v1(
            &repo,
            &revision,
            ManifestGeneration::new(4),
            &reconciled,
        )?;
        let stale = ledger.validate_historically_sealed_track_identity(
            &GenerationSnapshot {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(1),
                manifest_digest: "digest-1".to_string(),
            },
            "post-delete retry",
        );
        assert!(matches!(stale, Err(CoreError::Typed { .. })));
        ledger.validate_historically_sealed_track_identity(
            &GenerationSnapshot {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(3),
                manifest_digest: "digest-3".to_string(),
            },
            "post-delete retry",
        )?;

        drop(store);
        let reopened = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
        let mut restored = Ledger::new();
        reopened.restore_into(&mut restored)?;
        restored.validate_historically_sealed_track_identity(
            &GenerationSnapshot {
                repo_id: repo.clone(),
                revision_id: revision.clone(),
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation: ManifestGeneration::new(3),
                manifest_digest: "digest-3".to_string(),
            },
            "post-delete restart",
        )?;
        let reaped_after_restart = restored.validate_historically_sealed_track_identity(
            &GenerationSnapshot {
                repo_id: repo,
                revision_id: revision,
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation: ManifestGeneration::new(2),
                manifest_digest: "digest-2".to_string(),
            },
            "post-delete restart",
        );
        assert!(matches!(reaped_after_restart, Err(CoreError::Typed { .. })));
        Ok(())
    }

    #[test]
    fn auxiliary_authority_store_roundtrips_history_runtime_and_structural_state() -> TestResult {
        let dir = tempdir()?;
        let store = AuxiliaryAuthorityStore::open(dir.path(), search_corpus_retention(2)?)?;
        let mut ledger = Ledger::default();

        ledger
            .history_state_mut(&repo_id(), &revision_id(), generation())
            .note_commits_materialized();
        let _previous = ledger
            .runtime_state_mut(&repo_id(), &revision_id(), generation())
            .dirty_docs
            .insert(
                ChunkId::new("dirty-1"),
                super::DirtyDocState {
                    applied_at_ms: 42,
                    payload_hash: [7_u8; 32],
                },
            );
        install_chunk(&mut ledger, "src/lib.rs", "fn main() {}")?;
        ledger.request_structural_seal(&repo_id(), &revision_id(), generation());
        ledger.record_track_materialized(
            &repo_id(),
            &revision_id(),
            SearchPlaneTrackKind::Structural,
            generation(),
            Some("digest-17"),
        );
        ledger.record_track_seal_with_digest(
            &repo_id(),
            &revision_id(),
            SearchPlaneTrackKind::Structural,
            generation(),
            "digest-17",
        );

        store.persist_from_ledger(&ledger)?;

        let mut restored = Ledger::default();
        store.restore_into(&mut restored)?;

        if !restored
            .history_state(&repo_id(), &revision_id(), generation())
            .is_some_and(super::HistoryAuthorityState::commits_materialized)
        {
            return Err("history authority did not restore commit materialization".into());
        }
        if restored
            .runtime_state(&repo_id(), &revision_id(), generation())
            .and_then(|state| state.dirty_docs().get(&ChunkId::new("dirty-1")))
            .map(super::DirtyDocState::applied_at_ms)
            != Some(42)
        {
            return Err("runtime authority did not restore dirty-doc payload".into());
        }
        install_chunk_with_id(
            &mut ledger,
            "changed-1",
            "src/changed.rs",
            "fn changed() {}",
        )?;
        install_chunk_with_id(&mut ledger, "facet-1", "src/facet.rs", "fn facet() {}")?;
        install_chunk_with_id(&mut ledger, "snap-1", "src/snap.rs", "fn snap() {}")?;
        install_chunk_with_id(
            &mut ledger,
            "affected-1",
            "src/affected.rs",
            "fn affected() {}",
        )?;
        install_chunk_with_id(
            &mut ledger,
            "invalidated-1",
            "src/invalidated.rs",
            "fn invalidated() {}",
        )?;
        ledger.apply_runtime_catalog_batch(&RuntimeCatalogIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            overlay_epoch_ms: 99,
            batch_digest: "catalog-roundtrip".to_string(),
            producer_head_applied_at_ms: 100,
            generation_materialized_at_ms: 20,
            changed_entries: vec![RuntimeChangedRecord {
                doc_id: ChunkId::new("changed-1"),
                applied_at_ms: 25,
                payload_hash: [0xbb; 32],
            }],
            facet_entries: vec![RuntimeDocFacetRecord {
                doc_id: ChunkId::new("facet-1"),
                owner: Some("team-a".to_string()),
                service: None,
                layer: None,
                surface: None,
            }],
            snapshot_entries: vec![RuntimeSnapshotRecord {
                name: "active".to_string(),
                doc_ids: vec![ChunkId::new("snap-1")],
            }],
            affected_entries: vec![RuntimeEdgeAuthorityRecord {
                key: "rebuild=lexical".to_string(),
                doc_ids: vec![ChunkId::new("affected-1")],
            }],
            invalidated_by_entries: vec![RuntimeEdgeAuthorityRecord {
                key: "rebuild=lexical".to_string(),
                doc_ids: vec![ChunkId::new("invalidated-1")],
            }],
        })?;
        store.persist_from_ledger(&ledger)?;
        let mut restored_catalog = Ledger::default();
        store.restore_into(&mut restored_catalog)?;
        let runtime = restored_catalog
            .runtime_state(&repo_id(), &revision_id(), generation())
            .ok_or("runtime catalog state missing after restore")?;
        if !runtime.catalog_materialized() {
            return Err("runtime catalog materialization flag did not restore".into());
        }
        if runtime.catalog_overlay_epoch_ms() != Some(99) {
            return Err("runtime catalog overlay epoch did not restore".into());
        }
        if runtime.catalog_batch_digest() != Some("catalog-roundtrip") {
            return Err("runtime catalog batch digest did not restore".into());
        }
        if runtime.generation_materialized_at_ms() != Some(20) {
            return Err("runtime catalog generation timestamp did not restore".into());
        }
        if runtime
            .changed_docs()
            .get(&ChunkId::new("changed-1"))
            .map(super::ChangedDocState::applied_at_ms)
            != Some(25)
        {
            return Err("runtime catalog changed-doc payload did not restore".into());
        }
        if runtime
            .doc_facets()
            .get(&ChunkId::new("facet-1"))
            .and_then(super::DocFacetState::owner)
            != Some("team-a")
        {
            return Err("runtime catalog facet payload did not restore".into());
        }
        if !runtime
            .snapshots()
            .get("active")
            .is_some_and(|docs| docs.contains(&ChunkId::new("snap-1")))
        {
            return Err("runtime catalog snapshot membership did not restore".into());
        }
        if !runtime
            .affected_docs()
            .get("rebuild=lexical")
            .is_some_and(|docs| docs.contains(&ChunkId::new("affected-1")))
        {
            return Err("runtime catalog affected edge payload did not restore".into());
        }
        if !runtime
            .invalidated_by_docs()
            .get("rebuild=lexical")
            .is_some_and(|docs| docs.contains(&ChunkId::new("invalidated-1")))
        {
            return Err("runtime catalog invalidated_by edge payload did not restore".into());
        }
        if restored
            .structural_state(&repo_id(), &revision_id(), generation())
            .map(|state| state.chunks().len())
            != Some(1)
        {
            return Err("structural authority did not restore chunk inventory".into());
        }
        if restored.track_sealed(&repo_id(), &revision_id(), SearchPlaneTrackKind::Structural)
            != Some(generation())
        {
            return Err("structural track seal did not restore".into());
        }
        Ok(())
    }

    #[test]
    fn runtime_catalog_batch_replaces_previous_snapshot_state() -> TestResult {
        let mut ledger = Ledger::default();
        install_chunk_with_id(
            &mut ledger,
            "changed-1",
            "src/changed.rs",
            "fn changed() {}",
        )?;
        install_chunk_with_id(&mut ledger, "facet-1", "src/facet.rs", "fn facet() {}")?;
        install_chunk_with_id(&mut ledger, "snap-1", "src/snap.rs", "fn snap() {}")?;
        install_chunk_with_id(
            &mut ledger,
            "affected-1",
            "src/affected.rs",
            "fn affected() {}",
        )?;
        install_chunk_with_id(
            &mut ledger,
            "invalidated-1",
            "src/invalidated.rs",
            "fn invalidated() {}",
        )?;
        install_chunk_with_id(
            &mut ledger,
            "changed-2",
            "src/changed2.rs",
            "fn changed2() {}",
        )?;

        ledger.apply_runtime_catalog_batch(&RuntimeCatalogIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            overlay_epoch_ms: 10,
            batch_digest: "catalog-v1".to_string(),
            producer_head_applied_at_ms: 100,
            generation_materialized_at_ms: 20,
            changed_entries: vec![RuntimeChangedRecord {
                doc_id: ChunkId::new("changed-1"),
                applied_at_ms: 25,
                payload_hash: [0xbb; 32],
            }],
            facet_entries: vec![RuntimeDocFacetRecord {
                doc_id: ChunkId::new("facet-1"),
                owner: Some("team-a".to_string()),
                service: None,
                layer: None,
                surface: None,
            }],
            snapshot_entries: vec![RuntimeSnapshotRecord {
                name: "active".to_string(),
                doc_ids: vec![ChunkId::new("snap-1")],
            }],
            affected_entries: vec![RuntimeEdgeAuthorityRecord {
                key: "rebuild=lexical".to_string(),
                doc_ids: vec![ChunkId::new("affected-1")],
            }],
            invalidated_by_entries: vec![RuntimeEdgeAuthorityRecord {
                key: "rebuild=lexical".to_string(),
                doc_ids: vec![ChunkId::new("invalidated-1")],
            }],
        })?;

        ledger.apply_runtime_catalog_batch(&RuntimeCatalogIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            overlay_epoch_ms: 11,
            batch_digest: "catalog-v2".to_string(),
            producer_head_applied_at_ms: 101,
            generation_materialized_at_ms: 21,
            changed_entries: vec![RuntimeChangedRecord {
                doc_id: ChunkId::new("changed-2"),
                applied_at_ms: 30,
                payload_hash: [0xcc; 32],
            }],
            facet_entries: Vec::new(),
            snapshot_entries: Vec::new(),
            affected_entries: Vec::new(),
            invalidated_by_entries: Vec::new(),
        })?;

        let runtime = ledger
            .runtime_state(&repo_id(), &revision_id(), generation())
            .ok_or("runtime catalog state missing")?;
        if runtime
            .changed_docs()
            .contains_key(&ChunkId::new("changed-1"))
        {
            return Err("runtime catalog retained old changed-doc entry after replacement".into());
        }
        if !runtime
            .changed_docs()
            .contains_key(&ChunkId::new("changed-2"))
        {
            return Err("runtime catalog did not materialize new changed-doc entry".into());
        }
        if !runtime.doc_facets().is_empty()
            || !runtime.snapshots().is_empty()
            || !runtime.affected_docs().is_empty()
            || !runtime.invalidated_by_docs().is_empty()
        {
            return Err("runtime catalog replacement failed to drop removed keyspaces".into());
        }
        Ok(())
    }

    #[test]
    fn runtime_catalog_batch_rejects_older_epoch_replay() -> TestResult {
        let mut ledger = Ledger::default();
        install_chunk_with_id(
            &mut ledger,
            "changed-1",
            "src/changed.rs",
            "fn changed() {}",
        )?;
        ledger.apply_runtime_catalog_batch(&RuntimeCatalogIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            overlay_epoch_ms: 10,
            batch_digest: "catalog-v1".to_string(),
            producer_head_applied_at_ms: 100,
            generation_materialized_at_ms: 20,
            changed_entries: vec![RuntimeChangedRecord {
                doc_id: ChunkId::new("changed-1"),
                applied_at_ms: 25,
                payload_hash: [0xbb; 32],
            }],
            facet_entries: Vec::new(),
            snapshot_entries: Vec::new(),
            affected_entries: Vec::new(),
            invalidated_by_entries: Vec::new(),
        })?;

        let err = ledger
            .apply_runtime_catalog_batch(&RuntimeCatalogIngestBatch {
                repo_id: repo_id(),
                revision_id: revision_id(),
                generation: generation(),
                overlay_epoch_ms: 9,
                batch_digest: "catalog-stale".to_string(),
                producer_head_applied_at_ms: 101,
                generation_materialized_at_ms: 21,
                changed_entries: Vec::new(),
                facet_entries: Vec::new(),
                snapshot_entries: Vec::new(),
                affected_entries: Vec::new(),
                invalidated_by_entries: Vec::new(),
            })
            .err()
            .ok_or("expected stale runtime catalog replay to fail")?;
        match err {
            CoreError::Typed { code, .. } if code == super::ERR_RUNTIME_CATALOG_STALE_BATCH => {}
            other @ (CoreError::Typed { .. }
            | CoreError::InvalidContract(_)
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => {
                return Err(format!("expected stale batch typed error, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn runtime_catalog_batch_rejects_conflicting_same_epoch_replay() -> TestResult {
        let mut ledger = Ledger::default();
        install_chunk_with_id(
            &mut ledger,
            "changed-1",
            "src/changed.rs",
            "fn changed() {}",
        )?;
        ledger.apply_runtime_catalog_batch(&RuntimeCatalogIngestBatch {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            overlay_epoch_ms: 10,
            batch_digest: "catalog-v1".to_string(),
            producer_head_applied_at_ms: 100,
            generation_materialized_at_ms: 20,
            changed_entries: vec![RuntimeChangedRecord {
                doc_id: ChunkId::new("changed-1"),
                applied_at_ms: 25,
                payload_hash: [0xbb; 32],
            }],
            facet_entries: Vec::new(),
            snapshot_entries: Vec::new(),
            affected_entries: Vec::new(),
            invalidated_by_entries: Vec::new(),
        })?;

        let err = ledger
            .apply_runtime_catalog_batch(&RuntimeCatalogIngestBatch {
                repo_id: repo_id(),
                revision_id: revision_id(),
                generation: generation(),
                overlay_epoch_ms: 10,
                batch_digest: "catalog-v2".to_string(),
                producer_head_applied_at_ms: 100,
                generation_materialized_at_ms: 20,
                changed_entries: Vec::new(),
                facet_entries: Vec::new(),
                snapshot_entries: Vec::new(),
                affected_entries: Vec::new(),
                invalidated_by_entries: Vec::new(),
            })
            .err()
            .ok_or("expected conflicting runtime catalog replay to fail")?;
        match err {
            CoreError::Typed { code, .. }
                if code == super::ERR_RUNTIME_CATALOG_CONFLICTING_BATCH => {}
            other @ (CoreError::Typed { .. }
            | CoreError::InvalidContract(_)
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => {
                return Err(
                    format!("expected conflicting batch typed error, got {other:?}").into(),
                );
            }
        }
        Ok(())
    }

    #[test]
    fn runtime_catalog_batch_rejects_unknown_doc_id() -> TestResult {
        let mut ledger = Ledger::default();
        install_chunk(&mut ledger, "src/lib.rs", "fn main() {}")?;
        let err = ledger
            .apply_runtime_catalog_batch(&RuntimeCatalogIngestBatch {
                repo_id: repo_id(),
                revision_id: revision_id(),
                generation: generation(),
                overlay_epoch_ms: 10,
                batch_digest: "catalog-v1".to_string(),
                producer_head_applied_at_ms: 100,
                generation_materialized_at_ms: 20,
                changed_entries: vec![RuntimeChangedRecord {
                    doc_id: ChunkId::new("missing-doc"),
                    applied_at_ms: 25,
                    payload_hash: [0xbb; 32],
                }],
                facet_entries: Vec::new(),
                snapshot_entries: Vec::new(),
                affected_entries: Vec::new(),
                invalidated_by_entries: Vec::new(),
            })
            .err()
            .ok_or("expected unknown runtime catalog doc id to fail")?;
        match err {
            CoreError::Typed { code, .. } if code == super::ERR_RUNTIME_CATALOG_UNKNOWN_DOC_ID => {}
            other @ (CoreError::Typed { .. }
            | CoreError::InvalidContract(_)
            | CoreError::NotReady(_)
            | CoreError::NotImplemented(_)
            | CoreError::NotFound(_)
            | CoreError::Storage(_)) => {
                return Err(format!("expected unknown doc typed error, got {other:?}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn semantic_generation_state_records_exact_digest_and_seal() -> TestResult {
        let mut ledger = Ledger::default();
        ledger.record_track_materialized(
            &repo_id(),
            &revision_id(),
            SearchPlaneTrackKind::Semantic,
            generation(),
            Some("digest-sem-17"),
        );
        ledger.record_track_seal_with_digest(
            &repo_id(),
            &revision_id(),
            SearchPlaneTrackKind::Semantic,
            generation(),
            "digest-sem-17",
        );
        let state = ledger
            .semantic_generation_state(&repo_id(), &revision_id(), generation())
            .ok_or("missing semantic generation state")?;
        if !state.materialized() {
            return Err("semantic generation state did not record materialized".into());
        }
        if !state.sealed() {
            return Err("semantic generation state did not record sealed".into());
        }
        if state.manifest_digest() != "digest-sem-17" {
            return Err("semantic generation state lost manifest digest".into());
        }
        Ok(())
    }

    #[test]
    fn structural_upsert_parse_tree_rejects_unknown_chunk() -> TestResult {
        let mut ledger = Ledger::default();
        let op = LexicalChannelOp::UpsertParseTree(UpsertParseTree {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            chunk_id: ChunkId::new("chunk-1"),
            payload: encode_cbor(&parse_tree_record("fn main() {}")?)?,
        });
        let err = ledger
            .apply_lexical_authority_op(&op)
            .err()
            .ok_or_else(|| "expected unknown-chunk parse tree apply to fail".to_string())?;
        expect_decode_fail(err, "reason=source_chunk_missing")
    }

    #[test]
    fn structural_upsert_parse_tree_rejects_source_hash_mismatch() -> TestResult {
        let mut ledger = Ledger::default();
        install_chunk(&mut ledger, "src/lib.rs", "fn main() {}")?;
        let op = LexicalChannelOp::UpsertParseTree(UpsertParseTree {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            chunk_id: ChunkId::new("chunk-1"),
            payload: encode_cbor(&parse_tree_record("fn other() {}")?)?,
        });
        let err = ledger
            .apply_lexical_authority_op(&op)
            .err()
            .ok_or_else(|| "expected source_hash mismatch to fail".to_string())?;
        expect_decode_fail(err, "reason=source_hash_mismatch")
    }

    #[test]
    fn structural_replace_scope_rejects_chunk_outside_scope_path() -> TestResult {
        let mut ledger = Ledger::default();
        install_chunk(&mut ledger, "src/lib.rs", "fn main() {}")?;
        let op = LexicalChannelOp::ReplaceStructuralScope(ReplaceStructuralScope {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            payload: encode_cbor(&(
                BatchIngestMode::ReplaceGeneration,
                None::<ManifestGeneration>,
                StructuralReplaceScope {
                    scope: scope("src/other.rs"),
                    scope_digest: "scope:str".to_string(),
                    trees: vec![StructuralTreeRecord {
                        chunk_id: ChunkId::new("chunk-1"),
                        record: parse_tree_record("fn main() {}")?,
                    }],
                },
            ))?,
        });
        let err = ledger
            .apply_lexical_authority_op(&op)
            .err()
            .ok_or_else(|| "expected structural scope mismatch to fail".to_string())?;
        expect_decode_fail(err, "reason=scope_chunk_path_mismatch")
    }

    #[test]
    fn structural_replace_scope_failure_preserves_existing_parse_tree_set() -> TestResult {
        let mut ledger = Ledger::default();
        install_chunk(&mut ledger, "src/lib.rs", "fn main() {}")?;
        install_parse_tree(&mut ledger, "fn main() {}")?;
        let prior = ledger
            .structural_state(&repo_id(), &revision_id(), generation())
            .ok_or_else(|| "expected structural state after initial tree install".to_string())?
            .parse_trees()
            .get(&ChunkId::new("chunk-1"))
            .cloned()
            .ok_or_else(|| "expected installed parse tree".to_string())?;

        let op = LexicalChannelOp::ReplaceStructuralScope(ReplaceStructuralScope {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            payload: encode_cbor(&(
                BatchIngestMode::ReplaceGeneration,
                None::<ManifestGeneration>,
                StructuralReplaceScope {
                    scope: scope("src/lib.rs"),
                    scope_digest: "scope:str".to_string(),
                    trees: vec![StructuralTreeRecord {
                        chunk_id: ChunkId::new("chunk-1"),
                        record: parse_tree_record("fn other() {}")?,
                    }],
                },
            ))?,
        });
        let err = ledger
            .apply_lexical_authority_op(&op)
            .err()
            .ok_or_else(|| "expected structural replace with bad tree to fail".to_string())?;
        expect_decode_fail(err, "reason=source_hash_mismatch")?;

        let after = ledger
            .structural_state(&repo_id(), &revision_id(), generation())
            .ok_or_else(|| "expected structural state after failed replace".to_string())?
            .parse_trees()
            .get(&ChunkId::new("chunk-1"))
            .cloned()
            .ok_or_else(|| {
                "expected prior parse tree to remain after failed replace".to_string()
            })?;
        if after != prior {
            return Err(format!(
                "expected prior parse tree to survive failed replace, got after={after:?} prior={prior:?}"
            )
            .into());
        }
        Ok(())
    }
}
