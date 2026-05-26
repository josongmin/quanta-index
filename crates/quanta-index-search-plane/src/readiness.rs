use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use quanta_index_contract::lex::{CommitRecord, CommitSha, DiffHunkRecord, ParseTreeRecord};
use quanta_index_contract::{
    ChannelSeq, ChunkId, GenerationPin, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneActivateGenerationRequest, SearchPlaneTrackKind,
};
use quanta_index_contract::{ChunkRecord, LexicalChannelOp};
use quanta_index_core::CoreError;

type SharedLedger = Arc<RwLock<Ledger>>;
type SharedActivationCatalog = Arc<ActivationCatalog>;

/// Per-track readiness state.
#[derive(Debug, Default)]
pub struct TrackLedger {
    sealed: Option<ManifestGeneration>,
    last_seen: ChannelSeq,
}

impl TrackLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn sealed(&self) -> Option<ManifestGeneration> {
        self.sealed
    }

    #[must_use]
    pub fn last_seen(&self) -> ChannelSeq {
        self.last_seen
    }

    /// Monotonic seal update. Lower generations do not rewind readiness.
    pub fn record_seal(&mut self, generation: ManifestGeneration) {
        let next = match self.sealed {
            Some(current) if current.get() >= generation.get() => current,
            _ => generation,
        };
        self.sealed = Some(next);
    }

    pub fn set_last_seen(&mut self, seq: ChannelSeq) {
        self.last_seen = seq;
    }
}

/// Shared in-memory readiness ledger for query/readiness gating and channel replay.
#[derive(Debug, Default)]
pub struct Ledger {
    lexical: TrackLedger,
    semantic: TrackLedger,
    search_tracks: BTreeMap<TrackAuthorityKey, ManifestGeneration>,
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

    pub fn lexical_mut(&mut self) -> &mut TrackLedger {
        &mut self.lexical
    }

    pub fn semantic_mut(&mut self) -> &mut TrackLedger {
        &mut self.semantic
    }

    pub fn lexical_seal(&mut self, generation: ManifestGeneration) {
        self.lexical.record_seal(generation);
    }

    pub fn semantic_seal(&mut self, generation: ManifestGeneration) {
        self.semantic.record_seal(generation);
    }

    pub fn record_track_seal(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        track: SearchPlaneTrackKind,
        generation: ManifestGeneration,
    ) {
        let key = TrackAuthorityKey {
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            track,
        };
        let entry = self.search_tracks.entry(key).or_insert(generation);
        if entry.get() < generation.get() {
            *entry = generation;
        }
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
        self.search_tracks.get(&key).copied()
    }

    #[must_use]
    pub fn lexical_last_seen(&self) -> ChannelSeq {
        self.lexical.last_seen()
    }

    #[must_use]
    pub fn semantic_last_seen(&self) -> ChannelSeq {
        self.semantic.last_seen()
    }

    pub fn set_lexical_last_seen(&mut self, seq: ChannelSeq) {
        self.lexical.set_last_seen(seq);
    }

    pub fn set_semantic_last_seen(&mut self, seq: ChannelSeq) {
        self.semantic.set_last_seen(seq);
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
            LexicalChannelOp::UpsertCommit(payload) => {
                let record: CommitRecord = decode_record(&payload.payload, "commit")?;
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
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
                let _removed = self
                    .history_state_mut(&payload.repo_id, &payload.revision_id, payload.generation)
                    .refs
                    .remove(payload.name.as_ref());
            }
            LexicalChannelOp::UpsertTag(payload) => {
                let sha = CommitSha::from_bytes(payload.sha);
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
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
                let _removed = self
                    .history_state_mut(&payload.repo_id, &payload.revision_id, payload.generation)
                    .tags
                    .remove(payload.name.as_ref());
            }
            LexicalChannelOp::UpsertDiffHunk(payload) => {
                let record: DiffHunkRecord = decode_record(&payload.payload, "diff_hunk")?;
                let commit_sha = CommitSha::from_bytes(payload.commit_sha);
                let state = self.history_state_mut(
                    &payload.repo_id,
                    &payload.revision_id,
                    payload.generation,
                );
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
                let _previous = self
                    .structural_state_mut(
                        &payload.repo_id,
                        &payload.revision_id,
                        payload.generation,
                    )
                    .parse_trees
                    .insert(payload.chunk_id.clone(), record);
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
    ciborium::from_reader(payload).map_err(|err| {
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
        quanta_index_contract::LexicalReplaceScope,
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
        quanta_index_contract::LexicalTombstoneScope,
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

#[derive(Clone, Debug, Default)]
pub struct HistoryAuthorityState {
    commits: BTreeMap<CommitSha, CommitRecord>,
    refs: BTreeMap<Box<str>, CommitSha>,
    tags: BTreeMap<Box<str>, CommitSha>,
    diff_hunks: BTreeMap<HistoryDiffKey, DiffHunkRecord>,
}

impl HistoryAuthorityState {
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

#[derive(Clone, Debug, Default)]
pub struct RuntimeMetadataState {
    dirty_docs: BTreeMap<ChunkId, DirtyDocState>,
}

impl RuntimeMetadataState {
    #[must_use]
    pub fn dirty_docs(&self) -> &BTreeMap<ChunkId, DirtyDocState> {
        &self.dirty_docs
    }
}

#[derive(Clone, Debug, Default)]
pub struct StructuralAuthorityState {
    chunks: BTreeMap<ChunkId, ChunkRecord>,
    parse_trees: BTreeMap<ChunkId, ParseTreeRecord>,
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
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ActivationKey {
    repo_id: RepoId,
    revision_id: RevisionId,
    track: SearchPlaneTrackKind,
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
            let bytes = fs::read(&path).map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: read activation {}: {err}",
                    path.display()
                ))
            })?;
            let request = serde_json::from_slice::<SearchPlaneActivateGenerationRequest>(&bytes)
                .map_err(|err| {
                    CoreError::Storage(format!(
                        "search-plane activation catalog: decode activation {}: {err}",
                        path.display()
                    ))
                })?;
            for track in &request.tracks {
                insert_activation_record(&mut entries, &request, *track);
            }
        }
        Ok(Self {
            activations_dir: root.to_path_buf(),
            entries: RwLock::new(entries),
        })
    }

    pub fn shared(root: impl AsRef<Path>) -> Result<SharedActivationCatalog, CoreError> {
        Ok(Arc::new(Self::open(root)?))
    }

    pub fn activate(
        &self,
        request: &SearchPlaneActivateGenerationRequest,
    ) -> Result<(), CoreError> {
        if request.manifest_digest.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "activate-generation: manifest_digest must not be empty".to_string(),
            ));
        }
        if request.tracks.is_empty() {
            return Err(CoreError::InvalidContract(
                "activate-generation: tracks must not be empty".to_string(),
            ));
        }
        let mut entries = self.entries.write().map_err(|err| {
            CoreError::Storage(format!("search-plane activation catalog poisoned: {err}"))
        })?;
        for track in &request.tracks {
            let persisted = SearchPlaneActivateGenerationRequest {
                repo_id: request.repo_id.clone(),
                revision_id: request.revision_id.clone(),
                manifest_generation: request.manifest_generation,
                manifest_digest: request.manifest_digest.clone(),
                tracks: vec![*track],
            };
            let path = self.activations_dir.join(activation_file_name(
                &request.repo_id,
                &request.revision_id,
                *track,
            ));
            let bytes = serde_json::to_vec_pretty(&persisted).map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: encode activation {}: {err}",
                    path.display()
                ))
            })?;
            fs::write(&path, bytes).map_err(|err| {
                CoreError::Storage(format!(
                    "search-plane activation catalog: write activation {}: {err}",
                    path.display()
                ))
            })?;
            insert_activation_record(&mut entries, request, *track);
        }
        Ok(())
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
}

fn insert_activation_record(
    entries: &mut BTreeMap<ActivationKey, ActiveGenerationRecord>,
    request: &SearchPlaneActivateGenerationRequest,
    track: SearchPlaneTrackKind,
) {
    let key = ActivationKey {
        repo_id: request.repo_id.clone(),
        revision_id: request.revision_id.clone(),
        track,
    };
    let _prior = entries.insert(
        key,
        ActiveGenerationRecord {
            repo_id: request.repo_id.clone(),
            revision_id: request.revision_id.clone(),
            manifest_generation: request.manifest_generation,
            manifest_digest: request.manifest_digest.clone(),
            track,
        },
    );
}

fn activation_file_name(
    repo_id: &RepoId,
    revision_id: &RevisionId,
    track: SearchPlaneTrackKind,
) -> String {
    format!(
        "{}--{}--{}.json",
        encode_component(repo_id.as_str()),
        encode_component(revision_id.as_str()),
        track.as_code_str(),
    )
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

    use quanta_index_contract::{
        ChannelSeq, ManifestGeneration, RepoId, RevisionId, SearchPlaneActivateGenerationRequest,
        SearchPlaneTrackKind,
    };

    use super::{ActivationCatalog, Ledger};

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

    #[test]
    fn last_seen_cursors_are_track_local() {
        let mut ledger = Ledger::default();
        ledger.set_lexical_last_seen(ChannelSeq::new(11));
        ledger.set_semantic_last_seen(ChannelSeq::new(19));

        assert_eq!(ledger.lexical_last_seen(), ChannelSeq::new(11));
        assert_eq!(ledger.semantic_last_seen(), ChannelSeq::new(19));
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test asserts persistence/resolution via assert_eq! macros"
    )]
    fn activation_catalog_persists_and_resolves_track_locally() -> TestResult {
        let dir = tempdir()?;
        let catalog = ActivationCatalog::open(dir.path())?;
        let request = SearchPlaneActivateGenerationRequest {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_generation: ManifestGeneration::new(17),
            manifest_digest: "digest-17".to_string(),
            tracks: vec![SearchPlaneTrackKind::Lexical],
        };
        catalog.activate(&request)?;

        let pin = catalog.resolve(
            &RepoId::new("repo"),
            &RevisionId::new("rev"),
            SearchPlaneTrackKind::Lexical,
        )?;
        assert_eq!(pin.manifest_generation, ManifestGeneration::new(17));

        let reopened = ActivationCatalog::open(dir.path())?;
        let reopened_pin = reopened.resolve(
            &RepoId::new("repo"),
            &RevisionId::new("rev"),
            SearchPlaneTrackKind::Lexical,
        )?;
        assert_eq!(
            reopened_pin.manifest_generation,
            ManifestGeneration::new(17)
        );
        Ok(())
    }
}
