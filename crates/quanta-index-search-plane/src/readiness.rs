use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use quanta_index_contract::ChunkRecord;
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{
    CommitRecord, CommitSha, DiffHunkRecord, ParseTreeRecord, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    ChunkId, DirtyIngestBatch, DirtyMutation, GenerationPin, GenerationSnapshot,
    HistoryIngestBatch, HistoryRefMutation, ManifestGeneration, RepoId, RevisionId,
    RuntimeCatalogIngestBatch, SearchCorpusIngestBatch, SearchPlaneRollbackGenerationAck,
    SearchPlaneRollbackGenerationRequest, SearchPlaneTrackKind, SearchScopeSurface,
    StructuralIngestBatch,
};
use quanta_index_core::CoreError;
use quanta_index_ipc::{decode_cbor_payload, encode_cbor_payload};

type SharedLedger = Arc<RwLock<Ledger>>;
type SharedActivationCatalog = Arc<ActivationCatalog>;

pub(crate) const ERR_SEMANTIC_GENERATION_NOT_MATERIALIZED: &str =
    "SEMANTIC_GENERATION_NOT_MATERIALIZED";
pub(crate) const ERR_SEMANTIC_GENERATION_NOT_SEALED: &str = "SEMANTIC_GENERATION_NOT_SEALED";
pub(crate) const ERR_SEMANTIC_MANIFEST_DIGEST_MISMATCH: &str = "SEMANTIC_MANIFEST_DIGEST_MISMATCH";
pub(crate) const ERR_SEARCH_TRACK_GENERATION_NOT_SEALED: &str =
    "SEARCH_TRACK_GENERATION_NOT_SEALED";
pub(crate) const ERR_SEARCH_TRACK_MANIFEST_DIGEST_MISMATCH: &str =
    "SEARCH_TRACK_MANIFEST_DIGEST_MISMATCH";
pub(crate) const ERR_ROLLBACK_CAS_CONFLICT: &str = "ROLLBACK_CAS_CONFLICT";
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
        if let Some(digest) = manifest_digest {
            let _previous = self.sealed_search_track_identities.insert(
                Self::track_generation_key(repo_id, revision_id, track, generation),
                digest.to_string(),
            );
        }
        if track == SearchPlaneTrackKind::Semantic
            && let Some(digest) = manifest_digest
        {
            self.record_semantic_generation_sealed(repo_id, revision_id, generation, digest);
        }
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

    pub(crate) fn validate_historically_sealed_track_identity(
        &self,
        candidate: &GenerationSnapshot,
        plane: &str,
    ) -> Result<(), CoreError> {
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

#[derive(Debug)]
pub struct AuxiliaryAuthorityStore {
    history: PathBuf,
    runtime: PathBuf,
    structural: PathBuf,
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

impl AuxiliaryAuthorityStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let root = root.as_ref();
        let history_dir = root.join("history");
        let runtime_dir = root.join("runtime");
        let structural_dir = root.join("structural");
        for dir in [&history_dir, &runtime_dir, &structural_dir] {
            fs::create_dir_all(dir).map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane authority store: create {}: {err}",
                    dir.display()
                ))
            })?;
        }
        Ok(Self {
            history: history_dir.join("state.cbor"),
            runtime: runtime_dir.join("state.cbor"),
            structural: structural_dir.join("state.cbor"),
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
        Ok(())
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
        fs::write(path, bytes).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane authority store: write {label} {}: {err}",
                path.display()
            ))
        })
    }

    fn read_cbor<T: for<'de> Deserialize<'de>>(
        &self,
        path: &Path,
        label: &str,
    ) -> Result<Option<T>, CoreError> {
        let bytes = match fs::read(path) {
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
    entries: RwLock<BTreeMap<ActivationKey, ActiveGenerationRecord>>,
    // A rename may succeed while the parent-directory fsync fails.  At that
    // point the durable head is ambiguous until a fresh process reopens the
    // canonical root, so this process must not serve its old in-memory head.
    durability_uncertain_v1: AtomicBool,
}

impl ActivationCatalog {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let root = root.as_ref();
        fs::create_dir_all(root).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane activation catalog: create root {}: {err}",
                root.display()
            ))
        })?;
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
            if !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().is_none_or(|value| value != "json") {
                continue;
            }
            if !is_search_corpus_root_path(&path) {
                return Err(CoreError::Storage(format!(
                    "search-plane activation catalog: legacy per-track root is unsupported: {}",
                    path.display()
                )));
            }
            let bytes = fs::read(&path).map_err(|err| {
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
            composite_roots.push(composite);
        }
        // Every persisted root is one canonical lexical+semantic authority.
        for composite in composite_roots {
            insert_search_corpus_generation_records(&mut entries, &composite);
        }
        Ok(Self {
            activations_dir: root.to_path_buf(),
            entries: RwLock::new(entries),
            durability_uncertain_v1: AtomicBool::new(false),
        })
    }

    pub fn shared(root: impl AsRef<Path>) -> Result<SharedActivationCatalog, CoreError> {
        Ok(Arc::new(Self::open(root)?))
    }

    /// Durably promote one fully prepared lexical plus semantic corpus root.
    ///
    /// The persistent root is replaced and its parent directory is synced
    /// before either in-memory track entry changes. A failed durable write
    /// therefore leaves the query-visible head unchanged.
    pub fn activate_prepared_search_corpus_generation_v1(
        &self,
        prepared: &PreparedSearchCorpusGenerationV1,
    ) -> Result<SearchCorpusGenerationActivationV1, CoreError> {
        let candidate = prepared.candidate();
        let mut entries = self.entries.write().map_err(|err| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
        })?;
        // Hold the same lock that serializes root replacement while checking
        // the fence. A waiter released after a failed parent fsync must see
        // the fence before it can read or mutate the previous in-memory head.
        self.ensure_durability_certain_v1()?;
        let current = active_search_corpus_generation_v1(
            &entries,
            candidate.repo_id(),
            candidate.revision_id(),
        )?;
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
        match atomic_write_activation_file_v1(&path, &bytes)? {
            AtomicActivationWriteOutcomeV1::Durable => {}
            AtomicActivationWriteOutcomeV1::RenamedButParentSyncFailed(error) => {
                self.mark_durability_uncertain_v1();
                return Err(error);
            }
        }
        insert_search_corpus_generation_records(&mut entries, candidate);

        Ok(SearchCorpusGenerationActivationV1 {
            active: candidate.clone(),
            previous_active: current,
        })
    }

    /// Roll back the complete query-visible lexical plus semantic corpus.
    ///
    /// The wire contract retains `track=Semantic` as the rollback capability
    /// selector, but it never creates or persists semantic-only authority.
    /// Both tracks are reconstructed from the same target identity and the
    /// composite root is atomically replaced before either in-memory record
    /// changes.
    pub fn rollback(
        &self,
        request: &SearchPlaneRollbackGenerationRequest,
    ) -> Result<SearchPlaneRollbackGenerationAck, CoreError> {
        if request.track != SearchPlaneTrackKind::Semantic {
            return Err(CoreError::InvalidContract(
                "rollback-generation: only semantic track supports explicit rollback".to_string(),
            ));
        }
        if request.expected_active_manifest_digest.trim().is_empty()
            || request.target_manifest_digest.trim().is_empty()
        {
            return Err(CoreError::InvalidContract(
                "rollback-generation: active and target manifest digests must not be empty"
                    .to_string(),
            ));
        }
        if request.target_generation.get() >= request.expected_active_generation.get() {
            return Err(CoreError::InvalidContract(
                "rollback-generation: target generation must be lower than expected active generation"
                    .to_string(),
            ));
        }

        let mut entries = self.entries.write().map_err(|err| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
        })?;
        self.ensure_durability_certain_v1()?;
        let current =
            active_search_corpus_generation_v1(&entries, &request.repo_id, &request.revision_id)?
                .ok_or_else(|| {
                CoreError::NotReady(format!(
                    "rollback-generation: no active composite generation for repo={} revision={}",
                    request.repo_id.as_str(),
                    request.revision_id.as_str()
                ))
            })?;
        if current.manifest_generation() != request.expected_active_generation
            || current.manifest_digest() != request.expected_active_manifest_digest
        {
            return Err(CoreError::Typed {
                code: ERR_ROLLBACK_CAS_CONFLICT.to_string(),
                message: format!(
                    "rollback-generation: active state changed for repo={} revision={}: expected generation={} digest={}, observed generation={} digest={}",
                    request.repo_id.as_str(),
                    request.revision_id.as_str(),
                    request.expected_active_generation.get(),
                    request.expected_active_manifest_digest,
                    current.manifest_generation().get(),
                    current.manifest_digest(),
                ),
            });
        }

        let target = search_corpus_generation_from_rollback_request(request)?;
        let persisted = PersistedSearchCorpusGenerationRootV1::from_generation(&target);
        let path = self.activations_dir.join(search_corpus_root_file_name(
            &request.repo_id,
            &request.revision_id,
        ));
        let bytes = serde_json::to_vec_pretty(&persisted).map_err(|err| {
            CoreError::Storage(format!(
                "search-plane activation catalog: encode composite rollback {}: {err}",
                path.display()
            ))
        })?;
        match atomic_write_activation_file_v1(&path, &bytes)? {
            AtomicActivationWriteOutcomeV1::Durable => {}
            AtomicActivationWriteOutcomeV1::RenamedButParentSyncFailed(error) => {
                self.mark_durability_uncertain_v1();
                return Err(error);
            }
        }
        insert_search_corpus_generation_records(&mut entries, &target);

        Ok(SearchPlaneRollbackGenerationAck {
            repo_id: request.repo_id.clone(),
            revision_id: request.revision_id.clone(),
            track: request.track,
            previous_generation: current.manifest_generation(),
            previous_manifest_digest: current.manifest_digest().to_string(),
            manifest_generation: request.target_generation,
            manifest_digest: request.target_manifest_digest.clone(),
        })
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

fn search_corpus_generation_from_rollback_request(
    request: &SearchPlaneRollbackGenerationRequest,
) -> Result<SearchCorpusGenerationV1, CoreError> {
    let lexical = GenerationSnapshot {
        repo_id: request.repo_id.clone(),
        revision_id: request.revision_id.clone(),
        track: SearchPlaneTrackKind::Lexical,
        manifest_generation: request.target_generation,
        manifest_digest: request.target_manifest_digest.clone(),
    };
    let semantic = GenerationSnapshot {
        repo_id: request.repo_id.clone(),
        revision_id: request.revision_id.clone(),
        track: SearchPlaneTrackKind::Semantic,
        manifest_generation: request.target_generation,
        manifest_digest: request.target_manifest_digest.clone(),
    };
    SearchCorpusGenerationV1::new(lexical, semantic)
}

fn validate_prepared_search_corpus_expectation(
    prepared: &PreparedSearchCorpusGenerationV1,
    current: Option<&SearchCorpusGenerationV1>,
) -> Result<(), CoreError> {
    if prepared.expected_active() == current {
        return Ok(());
    }
    let describe = |identity: Option<&SearchCorpusGenerationV1>| match identity {
        Some(identity) => format!(
            "generation={} digest={}",
            identity.manifest_generation().get(),
            identity.manifest_digest(),
        ),
        None => "absent".to_string(),
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
        "{}--{}--corpus.json",
        encode_component(repo_id.as_str()),
        encode_component(revision_id.as_str()),
    )
}

fn is_search_corpus_root_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("--corpus.json"))
}

enum AtomicActivationWriteOutcomeV1 {
    Durable,
    RenamedButParentSyncFailed(CoreError),
}

fn atomic_write_activation_file_v1(
    path: &Path,
    bytes: &[u8],
) -> Result<AtomicActivationWriteOutcomeV1, CoreError> {
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let parent = path.parent().ok_or_else(|| {
        CoreError::Storage(format!(
            "search-plane activation catalog: activation file has no parent: {}",
            path.display()
        ))
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CoreError::Storage(format!(
                "search-plane activation catalog: activation file has no UTF-8 file name: {}",
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
                "search-plane activation catalog: create activation temporary {}: {err}",
                temporary.display()
            ))
        })?;
    if let Err(err) = file.write_all(bytes) {
        return Err(cleanup_activation_temporary_v1(
            &temporary,
            CoreError::Storage(format!(
                "search-plane activation catalog: write activation temporary {}: {err}",
                temporary.display()
            )),
        ));
    }
    if let Err(err) = file.sync_all() {
        return Err(cleanup_activation_temporary_v1(
            &temporary,
            CoreError::Storage(format!(
                "search-plane activation catalog: fsync activation temporary {}: {err}",
                temporary.display()
            )),
        ));
    }
    drop(file);
    if let Err(err) = fs::rename(&temporary, path) {
        return Err(cleanup_activation_temporary_v1(
            &temporary,
            CoreError::Storage(format!(
                "search-plane activation catalog: rename activation temporary {} to {}: {err}",
                temporary.display(),
                path.display()
            )),
        ));
    }
    match File::open(parent).and_then(|directory| directory.sync_all()) {
        Ok(()) => Ok(AtomicActivationWriteOutcomeV1::Durable),
        Err(err) => Ok(AtomicActivationWriteOutcomeV1::RenamedButParentSyncFailed(
            CoreError::Storage(format!(
                "search-plane activation catalog: fsync activation parent {}: {err}",
                parent.display()
            )),
        )),
    }
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

fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.' {
            encoded.push(char::from(byte));
            continue;
        }
        encoded.push('%');
        encoded.push(hex_char(byte >> 4));
        encoded.push(hex_char(byte & 0x0F));
    }
    encoded
}

fn hex_char(nibble: u8) -> char {
    const HEX_DIGITS: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F',
    ];
    HEX_DIGITS
        .get(usize::from(nibble))
        .copied()
        .map_or('0', std::convert::identity)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use quanta_index_contract::channel::LexicalChannelOp;
    use quanta_index_contract::lex::{
        LanguageCode, ParseNode, ParseTreeRecord, compute_parse_tree_source_hash,
    };
    use quanta_index_contract::{
        BatchIngestMode, ChunkId, ChunkRecord, GenerationSnapshot, ManifestGeneration,
        ReplaceLexicalScope, ReplaceStructuralScope, RepoId, RepoRelativePath, RevisionId,
        RuntimeCatalogIngestBatch, RuntimeChangedRecord, RuntimeDocFacetRecord,
        RuntimeEdgeAuthorityRecord, RuntimeSnapshotRecord, SearchCorpusReplaceScope,
        SearchPlaneRollbackGenerationRequest, SearchPlaneTrackKind, SearchScopeKey,
        SearchScopeSurface, StructuralReplaceScope, StructuralTreeRecord, UpsertParseTree,
    };
    use quanta_index_core::CoreError;

    use super::{
        ActivationCatalog, AuxiliaryAuthorityStore, Ledger, PreparedSearchCorpusGenerationV1,
        SearchCorpusGenerationV1,
    };

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

        let rollback = catalog.rollback(&SearchPlaneRollbackGenerationRequest {
            repo_id: RepoId::new("repo-corpus"),
            revision_id: RevisionId::new("rev-corpus"),
            track: SearchPlaneTrackKind::Semantic,
            expected_active_generation: ManifestGeneration::new(17),
            expected_active_manifest_digest: "digest-17".to_string(),
            target_generation: ManifestGeneration::new(16),
            target_manifest_digest: "digest-16".to_string(),
        })?;
        assert_eq!(rollback.previous_generation, ManifestGeneration::new(17));
        assert_eq!(rollback.manifest_generation, ManifestGeneration::new(16));
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
        assert!(
            std::fs::read_dir(dir.path())?
                .filter_map(Result::ok)
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
    fn auxiliary_authority_store_roundtrips_history_runtime_and_structural_state() -> TestResult {
        let dir = tempdir()?;
        let store = AuxiliaryAuthorityStore::open(dir.path())?;
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
