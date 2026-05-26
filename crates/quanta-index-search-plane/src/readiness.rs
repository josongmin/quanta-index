use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use quanta_index_contract::ChunkRecord;
use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{
    CommitRecord, CommitSha, DiffHunkRecord, ParseTreeRecord, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    ChunkId, GenerationPin, ManifestGeneration, RepoId, RevisionId,
    SearchPlaneActivateGenerationRequest, SearchPlaneTrackKind,
};
use quanta_index_core::CoreError;

type SharedLedger = Arc<RwLock<Ledger>>;
type SharedActivationCatalog = Arc<ActivationCatalog>;

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

/// Shared in-memory readiness ledger for query/readiness gating and channel replay.
#[derive(Debug, Default)]
pub struct Ledger {
    lexical: TrackLedger,
    semantic: TrackLedger,
    search_tracks: BTreeMap<TrackAuthorityKey, TrackAuthorityState>,
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

    pub fn lexical_materialize(
        &mut self,
        generation: ManifestGeneration,
        manifest_digest: Option<&str>,
    ) {
        self.lexical
            .record_materialized(generation, manifest_digest);
    }

    pub fn semantic_mut(&mut self) -> &mut TrackLedger {
        &mut self.semantic
    }

    pub fn semantic_materialize(
        &mut self,
        generation: ManifestGeneration,
        manifest_digest: Option<&str>,
    ) {
        self.semantic
            .record_materialized(generation, manifest_digest);
    }

    pub fn lexical_seal(&mut self, generation: ManifestGeneration) {
        self.lexical.record_seal(generation, None);
    }

    pub fn lexical_seal_with_digest(
        &mut self,
        generation: ManifestGeneration,
        manifest_digest: &str,
    ) {
        self.lexical.record_seal(generation, Some(manifest_digest));
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

    pub fn request_structural_seal(
        &mut self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) {
        self.structural_state_mut(repo_id, revision_id, generation)
            .request_seal();
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
    let expected_hash = compute_parse_tree_source_hash(chunk.indexed_text.as_ref());
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

    use quanta_index_contract::lex::{
        LanguageCode, ParseNode, ParseTreeRecord, compute_parse_tree_source_hash,
    };
    use quanta_index_contract::{
        BatchIngestMode, ChunkId, ChunkRecord, LexicalChannelOp, LexicalReplaceScope,
        ManifestGeneration, ReplaceLexicalScope, ReplaceStructuralScope, RepoId, RepoRelativePath,
        RevisionId, SearchPlaneActivateGenerationRequest, SearchPlaneTrackKind, SearchScopeKey,
        SearchScopeSurface, StructuralReplaceScope, StructuralTreeRecord, UpsertParseTree,
    };
    use quanta_index_core::CoreError;

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

    fn chunk_record(
        path: &str,
        indexed_text: &str,
    ) -> Result<ChunkRecord, Box<dyn std::error::Error>> {
        Ok(ChunkRecord {
            chunk_id: ChunkId::new("chunk-1"),
            repo_relative_path: RepoRelativePath::new(path),
            language: rust_language()?,
            start_byte: 0,
            end_byte: u32::try_from(indexed_text.len())
                .map_err(|err| format!("indexed_text len overflow: {err}"))?,
            start_line: 1,
            end_line: 1,
            snippet: indexed_text.to_string().into_boxed_str(),
            indexed_text: indexed_text.to_string().into_boxed_str(),
            text_digest: "text:digest".to_string().into_boxed_str(),
            shape_digest: "shape:digest".to_string().into_boxed_str(),
            structural: None,
            parent_chunk_id: None,
        })
    }

    fn parse_tree_record(
        indexed_text: &str,
    ) -> Result<ParseTreeRecord, Box<dyn std::error::Error>> {
        Ok(ParseTreeRecord {
            wire_version: 1,
            lang: rust_language()?,
            root: ParseNode {
                kind: "function_item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end: u32::try_from(indexed_text.len())
                    .map_err(|err| format!("indexed_text len overflow: {err}"))?,
                children: Vec::new(),
            },
            source_hash: compute_parse_tree_source_hash(indexed_text),
            role_tag_schema_version: 0,
            role_tags: Vec::new(),
        })
    }

    fn encode_cbor<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut bytes = Vec::new();
        ciborium::into_writer(value, &mut bytes)?;
        Ok(bytes)
    }

    fn install_chunk(ledger: &mut Ledger, path: &str, indexed_text: &str) -> TestResult {
        let op = LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            payload: encode_cbor(&(
                BatchIngestMode::ReplaceGeneration,
                None::<ManifestGeneration>,
                LexicalReplaceScope {
                    scope: scope(path),
                    scope_digest: "scope:lex".to_string(),
                    chunks: vec![chunk_record(path, indexed_text)?],
                    symbols: Vec::new(),
                },
            ))?,
        });
        ledger.apply_lexical_authority_op(&op)?;
        Ok(())
    }

    fn install_parse_tree(ledger: &mut Ledger, indexed_text: &str) -> TestResult {
        let op = LexicalChannelOp::UpsertParseTree(UpsertParseTree {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            chunk_id: ChunkId::new("chunk-1"),
            payload: encode_cbor(&parse_tree_record(indexed_text)?)?,
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
