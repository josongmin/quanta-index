//! Auxiliary authority mutations as deltas and durable rows (QI-BB-020).
//!
//! Every accepted history, runtime-metadata and structural batch is first
//! turned into a *delta* against the current in-memory state: the
//! transition validates the batch (unknown parents, refs to unknown
//! commits, parse trees for missing chunks, stale catalog epochs) and
//! names exactly the records that change. The delta is then encoded as
//! catalog rows and made durable as one transaction, and only after that
//! applied to the ledger. A reader therefore never sees a state that is
//! not durable, a receipt never precedes durability, and what is encoded
//! and written is proportional to the batch, never to the generation.
//!
//! The same row encoding restores a ledger from the catalog at boot, and
//! encodes a whole state for the one-shot migration of the pre-catalog
//! snapshot files.
//!
//! Every delta is stamped with the epoch its snapshot will have
//! (QI-BB-020 W2): the transition takes the next epoch of the generation
//! and domain, the rows carry it as the `Epoch` row of that generation
//! and domain in the same transaction, and the ledger applies the delta
//! at exactly that epoch. A generation restored without an `Epoch` row
//! is at [`AuxEpochV1::GENESIS`]: its rows predate epoch stamping, and
//! the first stamped mutation starts its sequence at one.

use std::collections::BTreeSet;

use imbl::OrdMap;
use quanta_index_contract::{
    AuxEpochV1, ChunkId, ChunkRecord, DirtyIngestBatch, DirtyMutation, HistoryIngestBatch,
    HistoryRefMutation, ManifestGeneration, RepoId, RevisionId, RuntimeCatalogIngestBatch,
    SearchCorpusIngestBatch, SearchPlaneTrackKind, SearchScopeSurface, StructuralIngestBatch,
    lex::{CommitRecord, CommitSha, DiffHunkRecord, ParseTreeRecord},
};
use quanta_index_core::{
    AuxiliaryDomainV1, AuxiliaryGenerationKeyV1, AuxiliaryMutationBatchV1, AuxiliaryRowFamilyV1,
    AuxiliaryRowKeyV1, AuxiliaryRowMutationV1, AuxiliaryRowV1, AuxiliaryTrackRowV1, CoreError,
};
use quanta_index_ipc::{decode_cbor_payload, encode_cbor_payload};
use serde::{Deserialize, Serialize};

use crate::readiness::{
    AuxDomainState, ChangedDocState, DirtyDocState, DocFacetState, HistoryAuthorityState,
    HistoryDiffKey, HistoryStateMeta, Ledger, RuntimeMetadataState, RuntimeStateMeta,
    StructuralAuthorityState, StructuralStateMeta, TrackAuthorityState,
    enforce_runtime_catalog_batch_order, validate_runtime_catalog_doc_ids,
    verify_parse_tree_against_chunk_map,
};

/// Separator between the commit sha and the path in a diff-hunk row key.
const DIFF_KEY_SEPARATOR: u8 = 0x1f;
/// Bytes of one commit sha, as the contract fixes it.
const COMMIT_SHA_LEN: usize = CommitSha::ZERO.as_bytes().len();

fn generation_key(
    repo_id: &RepoId,
    revision_id: &RevisionId,
    generation: ManifestGeneration,
) -> AuxiliaryGenerationKeyV1 {
    AuxiliaryGenerationKeyV1 {
        repo_id: repo_id.clone(),
        revision_id: revision_id.clone(),
        generation,
    }
}

fn row_key(
    domain: AuxiliaryDomainV1,
    generation: &AuxiliaryGenerationKeyV1,
    family: AuxiliaryRowFamilyV1,
    row_key: Vec<u8>,
) -> AuxiliaryRowKeyV1 {
    AuxiliaryRowKeyV1 {
        domain,
        generation: generation.clone(),
        family,
        row_key,
    }
}

fn encode<T: Serialize>(label: &str, value: &T) -> Result<Vec<u8>, CoreError> {
    encode_cbor_payload(value).map_err(|err| {
        CoreError::Storage(format!("auxiliary authority: encode {label} row: {err}"))
    })
}

fn decode<T: for<'de> Deserialize<'de>>(label: &str, bytes: &[u8]) -> Result<T, CoreError> {
    decode_cbor_payload(bytes).map_err(|err| {
        CoreError::Storage(format!("auxiliary authority: decode {label} row: {err}"))
    })
}

fn upsert<T: Serialize>(
    domain: AuxiliaryDomainV1,
    generation: &AuxiliaryGenerationKeyV1,
    family: AuxiliaryRowFamilyV1,
    key: Vec<u8>,
    value: &T,
) -> Result<AuxiliaryRowMutationV1, CoreError> {
    Ok(AuxiliaryRowMutationV1::Upsert(AuxiliaryRowV1 {
        key: row_key(domain, generation, family, key),
        value: encode(family.as_code_str(), value)?,
    }))
}

fn delete(
    domain: AuxiliaryDomainV1,
    generation: &AuxiliaryGenerationKeyV1,
    family: AuxiliaryRowFamilyV1,
    key: Vec<u8>,
) -> AuxiliaryRowMutationV1 {
    AuxiliaryRowMutationV1::Delete(row_key(domain, generation, family, key))
}

fn clear(
    domain: AuxiliaryDomainV1,
    generation: &AuxiliaryGenerationKeyV1,
    family: AuxiliaryRowFamilyV1,
) -> AuxiliaryRowMutationV1 {
    AuxiliaryRowMutationV1::ClearFamily {
        domain,
        generation: generation.clone(),
        family,
    }
}

/// The one `Epoch` row of a generation and domain: the read epoch of the
/// snapshot the rows in the same transaction produce.
fn epoch_row(
    domain: AuxiliaryDomainV1,
    generation: &AuxiliaryGenerationKeyV1,
    epoch: AuxEpochV1,
) -> Result<AuxiliaryRowMutationV1, CoreError> {
    upsert(
        domain,
        generation,
        AuxiliaryRowFamilyV1::Epoch,
        Vec::new(),
        &epoch,
    )
}

fn diff_key_bytes(key: &HistoryDiffKey) -> Vec<u8> {
    let mut bytes = key.commit_sha().as_bytes().to_vec();
    bytes.push(DIFF_KEY_SEPARATOR);
    bytes.extend_from_slice(key.file_path().as_bytes());
    bytes
}

fn diff_key_from_bytes(bytes: &[u8]) -> Result<HistoryDiffKey, CoreError> {
    let (sha, rest) = bytes.split_at_checked(COMMIT_SHA_LEN).ok_or_else(|| {
        CoreError::Storage("auxiliary authority: diff-hunk row key is too short".to_string())
    })?;
    let sha = <[u8; COMMIT_SHA_LEN]>::try_from(sha).map_err(|_wrong_length| {
        CoreError::Storage("auxiliary authority: diff-hunk row key sha is malformed".to_string())
    })?;
    let path = rest.strip_prefix(&[DIFF_KEY_SEPARATOR]).ok_or_else(|| {
        CoreError::Storage("auxiliary authority: diff-hunk row key lacks its separator".to_string())
    })?;
    let path = std::str::from_utf8(path).map_err(|err| {
        CoreError::Storage(format!(
            "auxiliary authority: diff-hunk row key path is not UTF-8: {err}"
        ))
    })?;
    Ok(HistoryDiffKey::new(CommitSha::from_bytes(sha), path))
}

fn commit_sha_from_bytes(bytes: &[u8]) -> Result<CommitSha, CoreError> {
    <[u8; COMMIT_SHA_LEN]>::try_from(bytes)
        .map(CommitSha::from_bytes)
        .map_err(|_wrong_length| {
            CoreError::Storage(format!(
                "auxiliary authority: commit row key is {} bytes, expected {}",
                bytes.len(),
                COMMIT_SHA_LEN
            ))
        })
}

fn utf8_key<'a>(label: &str, bytes: &'a [u8]) -> Result<&'a str, CoreError> {
    std::str::from_utf8(bytes).map_err(|err| {
        CoreError::Storage(format!(
            "auxiliary authority: {label} row key is not UTF-8: {err}"
        ))
    })
}

// ---------------------------------------------------------------------------
// Deltas
// ---------------------------------------------------------------------------

/// One ref or tag change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RefChange {
    Upsert(Box<str>, CommitSha),
    Delete(Box<str>),
}

/// What one history batch changes, validated against the state it will
/// apply to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HistoryDelta {
    pub(crate) generation: AuxiliaryGenerationKeyV1,
    /// The epoch the snapshot after this delta has.
    pub(crate) epoch: AuxEpochV1,
    pub(crate) commits: Vec<CommitRecord>,
    pub(crate) refs: Vec<RefChange>,
    pub(crate) tags: Vec<RefChange>,
    pub(crate) diff_hunks: Vec<(HistoryDiffKey, DiffHunkRecord)>,
    /// The materialization flags after the batch.
    pub(crate) meta: HistoryStateMeta,
}

/// What one dirty-overlay batch changes.
///
/// The generation's meta is carried unchanged so the generation exists in
/// the catalog even when the batch nets to zero dirty docs: a published
/// empty overlay is materialized, an absent one is not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeDirtyDelta {
    pub(crate) generation: AuxiliaryGenerationKeyV1,
    /// The epoch the snapshot after this delta has.
    pub(crate) epoch: AuxEpochV1,
    pub(crate) upserts: Vec<(ChunkId, DirtyDocState)>,
    pub(crate) deletes: Vec<ChunkId>,
    pub(crate) meta: RuntimeStateMeta,
}

/// What one runtime catalog batch changes: the whole catalog of the
/// generation, replaced, plus its meta.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeCatalogDelta {
    pub(crate) generation: AuxiliaryGenerationKeyV1,
    /// The epoch the snapshot after this delta has.
    pub(crate) epoch: AuxEpochV1,
    pub(crate) meta: RuntimeStateMeta,
    pub(crate) changed_docs: OrdMap<ChunkId, ChangedDocState>,
    pub(crate) doc_facets: OrdMap<ChunkId, DocFacetState>,
    pub(crate) snapshots: OrdMap<Box<str>, BTreeSet<ChunkId>>,
    pub(crate) affected_docs: OrdMap<Box<str>, BTreeSet<ChunkId>>,
    pub(crate) invalidated_by_docs: OrdMap<Box<str>, BTreeSet<ChunkId>>,
}

/// What one structural batch changes: parse trees removed and written,
/// the seal request after the batch, and the structural track's state
/// after the batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StructuralTreesDelta {
    pub(crate) generation: AuxiliaryGenerationKeyV1,
    /// The epoch the snapshot after this delta has.
    pub(crate) epoch: AuxEpochV1,
    pub(crate) removed: BTreeSet<ChunkId>,
    pub(crate) upserts: Vec<(ChunkId, ParseTreeRecord)>,
    pub(crate) seal_requested: bool,
    pub(crate) track: TrackAuthorityState,
    /// Whether the track sealed in this batch (the seal was requested and
    /// trees exist), which the caller reports on its receipt.
    pub(crate) sealed_track: bool,
}

/// What one search-corpus batch changes in the structural chunk universe.
///
/// The generation's meta is carried unchanged so the generation exists in
/// the catalog even for a batch without chunks: a published empty chunk
/// universe is materialized, an absent one is not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StructuralChunksDelta {
    pub(crate) generation: AuxiliaryGenerationKeyV1,
    /// The epoch the snapshot after this delta has.
    pub(crate) epoch: AuxEpochV1,
    /// Every chunk goes before the upserts (a `Chunk` surface clear).
    pub(crate) clear: bool,
    pub(crate) removed: BTreeSet<ChunkId>,
    pub(crate) upserts: Vec<ChunkRecord>,
    pub(crate) meta: StructuralStateMeta,
}

fn history_typed(code: &str, message: String) -> CoreError {
    CoreError::Typed {
        code: code.to_string(),
        message,
    }
}

/// Validate `batch` against `current` and name what it changes; the
/// snapshot after it is stamped `epoch`.
pub(crate) fn history_transition(
    current: Option<&HistoryAuthorityState>,
    epoch: AuxEpochV1,
    batch: &HistoryIngestBatch,
) -> Result<HistoryDelta, CoreError> {
    let generation = generation_key(&batch.repo_id, &batch.revision_id, batch.generation);
    let mut meta = current.map(HistoryAuthorityState::meta).unwrap_or_default();
    // Commits known after each step: the current state plus the batch so far.
    let mut known: BTreeSet<CommitSha> = BTreeSet::new();
    let is_known = |known: &BTreeSet<CommitSha>, sha: &CommitSha| {
        known.contains(sha) || current.is_some_and(|state| state.commits().contains_key(sha))
    };
    let mut commits = Vec::with_capacity(batch.commits.len());
    for record in &batch.commits {
        meta.commits_materialized = true;
        for parent in &record.parents {
            if !is_known(&known, parent) {
                return Err(history_typed(
                    "HISTORY_COMMIT_PARENT_UNKNOWN",
                    format!(
                        "history ingest: parent {} missing before child {}",
                        parent, record.sha
                    ),
                ));
            }
        }
        let _new = known.insert(record.sha);
        commits.push(record.clone());
    }
    let ref_changes = |mutations: &[HistoryRefMutation],
                       label: &str,
                       materialized: &mut bool|
     -> Result<Vec<RefChange>, CoreError> {
        let mut changes = Vec::with_capacity(mutations.len());
        for mutation in mutations {
            *materialized = true;
            match mutation {
                HistoryRefMutation::Upsert(payload) => {
                    if !is_known(&known, &payload.sha) {
                        return Err(history_typed(
                            "HISTORY_REF_NOT_FOUND",
                            format!(
                                "history ingest: {label} `{}` points to unknown commit {}",
                                payload.name, payload.sha
                            ),
                        ));
                    }
                    changes.push(RefChange::Upsert(payload.name.clone(), payload.sha));
                }
                HistoryRefMutation::Delete(payload) => {
                    changes.push(RefChange::Delete(payload.name.clone()));
                }
            }
        }
        Ok(changes)
    };
    let refs = ref_changes(&batch.refs, "ref", &mut meta.refs_materialized)?;
    let tags = ref_changes(&batch.tags, "tag", &mut meta.tags_materialized)?;
    let mut diff_hunks = Vec::with_capacity(batch.diff_hunks.len());
    for hunk in &batch.diff_hunks {
        meta.diff_hunks_materialized = true;
        if !is_known(&known, &hunk.commit_sha) {
            return Err(history_typed(
                "HISTORY_REF_NOT_FOUND",
                format!(
                    "history ingest: diff hunk for unknown commit {}",
                    hunk.commit_sha
                ),
            ));
        }
        diff_hunks.push((
            HistoryDiffKey::new(hunk.commit_sha, hunk.file_path.as_ref()),
            hunk.record.clone(),
        ));
    }
    Ok(HistoryDelta {
        generation,
        epoch,
        commits,
        refs,
        tags,
        diff_hunks,
        meta,
    })
}

/// Name what one dirty-overlay batch changes; nothing to validate. The
/// snapshot after it is stamped `epoch`.
pub(crate) fn runtime_dirty_transition(
    current: Option<&RuntimeMetadataState>,
    epoch: AuxEpochV1,
    batch: &DirtyIngestBatch,
) -> RuntimeDirtyDelta {
    let generation = generation_key(&batch.repo_id, &batch.revision_id, batch.generation);
    let mut upserts = Vec::new();
    let mut deletes = Vec::new();
    for entry in &batch.entries {
        match entry {
            DirtyMutation::Upsert(record) => upserts.push((
                record.doc_id.clone(),
                DirtyDocState::new(record.applied_at_ms, record.payload_hash),
            )),
            DirtyMutation::Delete(payload) => deletes.push(payload.doc_id.clone()),
        }
    }
    RuntimeDirtyDelta {
        generation,
        epoch,
        upserts,
        deletes,
        meta: current.map(RuntimeMetadataState::meta).unwrap_or_default(),
    }
}

/// Validate a runtime catalog batch against the chunk universe and the
/// current catalog epoch, and name the catalog it replaces the current one
/// with.
///
/// The snapshot after it is stamped `epoch` (the read epoch, not the
/// producer's overlay epoch the batch carries).
pub(crate) fn runtime_catalog_transition(
    structural: Option<&StructuralAuthorityState>,
    current: Option<&RuntimeMetadataState>,
    epoch: AuxEpochV1,
    batch: &RuntimeCatalogIngestBatch,
) -> Result<RuntimeCatalogDelta, CoreError> {
    let chunk_universe = structural
        .map(|state| state.chunks().keys().cloned().collect::<BTreeSet<_>>())
        .ok_or_else(|| CoreError::Typed {
            code: crate::readiness::ERR_RUNTIME_CATALOG_CHUNK_UNIVERSE_UNAVAILABLE.to_string(),
            message: "runtime catalog ingest: lexical chunk authority is not materialized for the pinned generation".to_string(),
        })?;
    validate_runtime_catalog_doc_ids(batch, &chunk_universe)?;
    if let Some(current) = current {
        enforce_runtime_catalog_batch_order(current, batch)?;
    }
    let owned = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::to_owned)
            .map(String::into_boxed_str)
    };
    Ok(RuntimeCatalogDelta {
        generation: generation_key(&batch.repo_id, &batch.revision_id, batch.generation),
        epoch,
        meta: RuntimeStateMeta {
            catalog_overlay_epoch_ms: Some(batch.overlay_epoch_ms),
            catalog_batch_digest: Some(batch.batch_digest.clone().into_boxed_str()),
            producer_head_applied_at_ms: Some(batch.producer_head_applied_at_ms),
            generation_materialized_at_ms: Some(batch.generation_materialized_at_ms),
            catalog_materialized: true,
        },
        changed_docs: batch
            .changed_entries
            .iter()
            .map(|record| {
                (
                    record.doc_id.clone(),
                    ChangedDocState::new(record.applied_at_ms, record.payload_hash),
                )
            })
            .collect(),
        doc_facets: batch
            .facet_entries
            .iter()
            .map(|record| {
                (
                    record.doc_id.clone(),
                    DocFacetState::new(
                        owned(&record.owner),
                        owned(&record.service),
                        owned(&record.layer),
                        owned(&record.surface),
                    ),
                )
            })
            .collect(),
        snapshots: batch
            .snapshot_entries
            .iter()
            .map(|record| {
                (
                    record.name.clone().into_boxed_str(),
                    record
                        .doc_ids
                        .iter()
                        .cloned()
                        .collect::<BTreeSet<ChunkId>>(),
                )
            })
            .collect(),
        affected_docs: batch
            .affected_entries
            .iter()
            .map(|record| {
                (
                    record.key.clone().into_boxed_str(),
                    record
                        .doc_ids
                        .iter()
                        .cloned()
                        .collect::<BTreeSet<ChunkId>>(),
                )
            })
            .collect(),
        invalidated_by_docs: batch
            .invalidated_by_entries
            .iter()
            .map(|record| {
                (
                    record.key.clone().into_boxed_str(),
                    record
                        .doc_ids
                        .iter()
                        .cloned()
                        .collect::<BTreeSet<ChunkId>>(),
                )
            })
            .collect(),
    })
}

/// Validate a structural batch against the chunk universe and name the
/// parse trees it removes and writes, the seal request after it, and the
/// structural track's state after it.
///
/// The snapshot after it is stamped `epoch`.
pub(crate) fn structural_transition(
    current: Option<&StructuralAuthorityState>,
    track: Option<&TrackAuthorityState>,
    epoch: AuxEpochV1,
    batch: &StructuralIngestBatch,
) -> Result<StructuralTreesDelta, CoreError> {
    let generation = generation_key(&batch.repo_id, &batch.revision_id, batch.generation);
    let empty = StructuralAuthorityState::default();
    let state = current.unwrap_or(&empty);
    let chunk_ids_at = |path: &str| -> BTreeSet<ChunkId> {
        state
            .chunks()
            .iter()
            .filter(|(_chunk_id, chunk)| chunk.repo_relative_path.as_str() == path)
            .map(|(chunk_id, _chunk)| chunk_id.clone())
            .collect()
    };
    let mut removed = BTreeSet::new();
    let mut upserts = Vec::new();
    for scope in &batch.replace_scopes {
        let path = scope.scope.repo_relative_path.as_str();
        for tree in &scope.trees {
            verify_parse_tree_against_chunk_map(
                state.chunks(),
                &tree.chunk_id,
                &tree.record,
                Some(path),
            )?;
        }
        removed.extend(chunk_ids_at(path));
        for tree in &scope.trees {
            upserts.push((tree.chunk_id.clone(), tree.record.clone()));
        }
    }
    for scope in &batch.tombstone_scopes {
        removed.extend(chunk_ids_at(scope.scope.repo_relative_path.as_str()));
    }
    // Trees after the batch: current minus removed plus upserts.
    let upserted: BTreeSet<&ChunkId> = upserts.iter().map(|(chunk_id, _tree)| chunk_id).collect();
    let has_trees_after = !upserted.is_empty()
        || state
            .parse_trees()
            .keys()
            .any(|chunk_id| !removed.contains(chunk_id));
    let seal_requested = state.seal_requested() || (batch.seal && has_trees_after);
    let mut track = track.cloned().unwrap_or_default();
    track.record_materialized(batch.generation, Some(batch.manifest_digest.as_str()));
    let sealed_track = batch.seal && has_trees_after;
    if sealed_track {
        track.record_seal(batch.generation, Some(batch.manifest_digest.as_str()));
    }
    Ok(StructuralTreesDelta {
        generation,
        epoch,
        removed,
        upserts,
        seal_requested,
        track,
        sealed_track,
    })
}

/// Name what one search-corpus batch changes in the structural chunk
/// universe; nothing to validate here, the batch was validated at entry.
/// The snapshot after it is stamped `epoch`.
pub(crate) fn structural_chunks_transition(
    current: Option<&StructuralAuthorityState>,
    epoch: AuxEpochV1,
    batch: &SearchCorpusIngestBatch,
) -> StructuralChunksDelta {
    let generation = generation_key(&batch.repo_id, &batch.revision_id, batch.generation);
    let clear = batch.clear_surfaces.contains(&SearchScopeSurface::Chunk);
    let mut removed = BTreeSet::new();
    if !clear && let Some(state) = current {
        let paths: BTreeSet<&str> = batch
            .replace_scopes
            .iter()
            .map(|scope| scope.scope.repo_relative_path.as_str())
            .chain(
                batch
                    .tombstone_scopes
                    .iter()
                    .map(|scope| scope.scope.repo_relative_path.as_str()),
            )
            .collect();
        removed.extend(
            state
                .chunks()
                .iter()
                .filter(|(_chunk_id, chunk)| paths.contains(chunk.repo_relative_path.as_str()))
                .map(|(chunk_id, _chunk)| chunk_id.clone()),
        );
    }
    let upserts = batch
        .replace_scopes
        .iter()
        .flat_map(|scope| scope.chunks.iter().cloned())
        .collect();
    StructuralChunksDelta {
        generation,
        epoch,
        clear,
        removed,
        upserts,
        meta: StructuralStateMeta {
            seal_requested: current.is_some_and(StructuralAuthorityState::seal_requested),
        },
    }
}

// ---------------------------------------------------------------------------
// Rows for deltas
// ---------------------------------------------------------------------------

/// The catalog rows one history delta amounts to.
pub(crate) fn history_delta_rows(
    delta: &HistoryDelta,
) -> Result<AuxiliaryMutationBatchV1, CoreError> {
    const DOMAIN: AuxiliaryDomainV1 = AuxiliaryDomainV1::History;
    let generation = &delta.generation;
    let mut rows = Vec::new();
    for record in &delta.commits {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::Commit,
            record.sha.as_bytes().to_vec(),
            record,
        )?);
    }
    for (family, changes) in [
        (AuxiliaryRowFamilyV1::Ref, &delta.refs),
        (AuxiliaryRowFamilyV1::Tag, &delta.tags),
    ] {
        for change in changes {
            rows.push(match change {
                RefChange::Upsert(name, sha) => {
                    upsert(DOMAIN, generation, family, name.as_bytes().to_vec(), sha)?
                }
                RefChange::Delete(name) => {
                    delete(DOMAIN, generation, family, name.as_bytes().to_vec())
                }
            });
        }
    }
    for (key, record) in &delta.diff_hunks {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::DiffHunk,
            diff_key_bytes(key),
            record,
        )?);
    }
    rows.push(upsert(
        DOMAIN,
        generation,
        AuxiliaryRowFamilyV1::StateMeta,
        Vec::new(),
        &delta.meta,
    )?);
    rows.push(epoch_row(DOMAIN, generation, delta.epoch)?);
    Ok(AuxiliaryMutationBatchV1 {
        rows,
        tracks: Vec::new(),
    })
}

/// The catalog rows one dirty-overlay delta amounts to.
pub(crate) fn runtime_dirty_delta_rows(
    delta: &RuntimeDirtyDelta,
) -> Result<AuxiliaryMutationBatchV1, CoreError> {
    const DOMAIN: AuxiliaryDomainV1 = AuxiliaryDomainV1::Runtime;
    let generation = &delta.generation;
    let mut rows = Vec::new();
    for (chunk_id, state) in &delta.upserts {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::DirtyDoc,
            chunk_id.as_str().as_bytes().to_vec(),
            state,
        )?);
    }
    for chunk_id in &delta.deletes {
        rows.push(delete(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::DirtyDoc,
            chunk_id.as_str().as_bytes().to_vec(),
        ));
    }
    rows.push(upsert(
        DOMAIN,
        generation,
        AuxiliaryRowFamilyV1::StateMeta,
        Vec::new(),
        &delta.meta,
    )?);
    rows.push(epoch_row(DOMAIN, generation, delta.epoch)?);
    Ok(AuxiliaryMutationBatchV1 {
        rows,
        tracks: Vec::new(),
    })
}

fn chunk_set_rows(
    generation: &AuxiliaryGenerationKeyV1,
    family: AuxiliaryRowFamilyV1,
    sets: &OrdMap<Box<str>, BTreeSet<ChunkId>>,
    rows: &mut Vec<AuxiliaryRowMutationV1>,
) -> Result<(), CoreError> {
    rows.push(clear(AuxiliaryDomainV1::Runtime, generation, family));
    for (name, chunk_ids) in sets {
        rows.push(upsert(
            AuxiliaryDomainV1::Runtime,
            generation,
            family,
            name.as_bytes().to_vec(),
            chunk_ids,
        )?);
    }
    Ok(())
}

/// The catalog rows one runtime catalog delta amounts to: every catalog
/// family cleared and rewritten, and the meta row.
pub(crate) fn runtime_catalog_delta_rows(
    delta: &RuntimeCatalogDelta,
) -> Result<AuxiliaryMutationBatchV1, CoreError> {
    const DOMAIN: AuxiliaryDomainV1 = AuxiliaryDomainV1::Runtime;
    let generation = &delta.generation;
    let mut rows = Vec::new();
    rows.push(clear(DOMAIN, generation, AuxiliaryRowFamilyV1::ChangedDoc));
    for (chunk_id, state) in &delta.changed_docs {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::ChangedDoc,
            chunk_id.as_str().as_bytes().to_vec(),
            state,
        )?);
    }
    rows.push(clear(DOMAIN, generation, AuxiliaryRowFamilyV1::DocFacet));
    for (chunk_id, state) in &delta.doc_facets {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::DocFacet,
            chunk_id.as_str().as_bytes().to_vec(),
            state,
        )?);
    }
    chunk_set_rows(
        generation,
        AuxiliaryRowFamilyV1::Snapshot,
        &delta.snapshots,
        &mut rows,
    )?;
    chunk_set_rows(
        generation,
        AuxiliaryRowFamilyV1::AffectedDocs,
        &delta.affected_docs,
        &mut rows,
    )?;
    chunk_set_rows(
        generation,
        AuxiliaryRowFamilyV1::InvalidatedByDocs,
        &delta.invalidated_by_docs,
        &mut rows,
    )?;
    rows.push(upsert(
        DOMAIN,
        generation,
        AuxiliaryRowFamilyV1::StateMeta,
        Vec::new(),
        &delta.meta,
    )?);
    rows.push(epoch_row(DOMAIN, generation, delta.epoch)?);
    Ok(AuxiliaryMutationBatchV1 {
        rows,
        tracks: Vec::new(),
    })
}

fn track_row(
    generation: &AuxiliaryGenerationKeyV1,
    track: &TrackAuthorityState,
) -> Result<AuxiliaryTrackRowV1, CoreError> {
    Ok(AuxiliaryTrackRowV1 {
        repo_id: generation.repo_id.clone(),
        revision_id: generation.revision_id.clone(),
        track: SearchPlaneTrackKind::Structural,
        value: encode("structural track", track)?,
    })
}

/// The catalog rows one structural delta amounts to: parse-tree deletes
/// and upserts, the meta row, and the structural track row.
pub(crate) fn structural_delta_rows(
    delta: &StructuralTreesDelta,
) -> Result<AuxiliaryMutationBatchV1, CoreError> {
    const DOMAIN: AuxiliaryDomainV1 = AuxiliaryDomainV1::Structural;
    let generation = &delta.generation;
    let mut rows = Vec::new();
    for chunk_id in &delta.removed {
        rows.push(delete(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::ParseTree,
            chunk_id.as_str().as_bytes().to_vec(),
        ));
    }
    for (chunk_id, record) in &delta.upserts {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::ParseTree,
            chunk_id.as_str().as_bytes().to_vec(),
            record,
        )?);
    }
    rows.push(upsert(
        DOMAIN,
        generation,
        AuxiliaryRowFamilyV1::StateMeta,
        Vec::new(),
        &StructuralStateMeta {
            seal_requested: delta.seal_requested,
        },
    )?);
    rows.push(epoch_row(DOMAIN, generation, delta.epoch)?);
    Ok(AuxiliaryMutationBatchV1 {
        rows,
        tracks: vec![track_row(generation, &delta.track)?],
    })
}

/// The catalog rows one chunk-universe delta amounts to.
pub(crate) fn structural_chunks_delta_rows(
    delta: &StructuralChunksDelta,
) -> Result<AuxiliaryMutationBatchV1, CoreError> {
    const DOMAIN: AuxiliaryDomainV1 = AuxiliaryDomainV1::Structural;
    let generation = &delta.generation;
    let mut rows = Vec::new();
    if delta.clear {
        rows.push(clear(DOMAIN, generation, AuxiliaryRowFamilyV1::Chunk));
    }
    for chunk_id in &delta.removed {
        rows.push(delete(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::Chunk,
            chunk_id.as_str().as_bytes().to_vec(),
        ));
    }
    for chunk in &delta.upserts {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::Chunk,
            chunk.chunk_id.as_str().as_bytes().to_vec(),
            chunk,
        )?);
    }
    rows.push(upsert(
        DOMAIN,
        generation,
        AuxiliaryRowFamilyV1::StateMeta,
        Vec::new(),
        &delta.meta,
    )?);
    rows.push(epoch_row(DOMAIN, generation, delta.epoch)?);
    Ok(AuxiliaryMutationBatchV1 {
        rows,
        tracks: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Rows for whole states (migration) and restore
// ---------------------------------------------------------------------------

/// Every row one history state amounts to, stamped `epoch`.
pub(crate) fn history_state_rows(
    generation: &AuxiliaryGenerationKeyV1,
    epoch: AuxEpochV1,
    state: &HistoryAuthorityState,
) -> Result<Vec<AuxiliaryRowMutationV1>, CoreError> {
    const DOMAIN: AuxiliaryDomainV1 = AuxiliaryDomainV1::History;
    let mut rows = Vec::new();
    for (sha, record) in state.commits() {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::Commit,
            sha.as_bytes().to_vec(),
            record,
        )?);
    }
    for (family, map) in [
        (AuxiliaryRowFamilyV1::Ref, state.refs()),
        (AuxiliaryRowFamilyV1::Tag, state.tags()),
    ] {
        for (name, sha) in map {
            rows.push(upsert(
                DOMAIN,
                generation,
                family,
                name.as_bytes().to_vec(),
                sha,
            )?);
        }
    }
    for (key, record) in state.diff_hunks() {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::DiffHunk,
            diff_key_bytes(key),
            record,
        )?);
    }
    rows.push(upsert(
        DOMAIN,
        generation,
        AuxiliaryRowFamilyV1::StateMeta,
        Vec::new(),
        &state.meta(),
    )?);
    rows.push(epoch_row(DOMAIN, generation, epoch)?);
    Ok(rows)
}

/// Every row one runtime state amounts to, stamped `epoch`.
pub(crate) fn runtime_state_rows(
    generation: &AuxiliaryGenerationKeyV1,
    epoch: AuxEpochV1,
    state: &RuntimeMetadataState,
) -> Result<Vec<AuxiliaryRowMutationV1>, CoreError> {
    const DOMAIN: AuxiliaryDomainV1 = AuxiliaryDomainV1::Runtime;
    let mut rows = Vec::new();
    for (chunk_id, doc) in state.dirty_docs() {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::DirtyDoc,
            chunk_id.as_str().as_bytes().to_vec(),
            doc,
        )?);
    }
    for (chunk_id, doc) in state.changed_docs() {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::ChangedDoc,
            chunk_id.as_str().as_bytes().to_vec(),
            doc,
        )?);
    }
    for (chunk_id, facet) in state.doc_facets() {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::DocFacet,
            chunk_id.as_str().as_bytes().to_vec(),
            facet,
        )?);
    }
    for (family, sets) in [
        (AuxiliaryRowFamilyV1::Snapshot, state.snapshots()),
        (AuxiliaryRowFamilyV1::AffectedDocs, state.affected_docs()),
        (
            AuxiliaryRowFamilyV1::InvalidatedByDocs,
            state.invalidated_by_docs(),
        ),
    ] {
        for (name, chunk_ids) in sets {
            rows.push(upsert(
                DOMAIN,
                generation,
                family,
                name.as_bytes().to_vec(),
                chunk_ids,
            )?);
        }
    }
    rows.push(upsert(
        DOMAIN,
        generation,
        AuxiliaryRowFamilyV1::StateMeta,
        Vec::new(),
        &state.meta(),
    )?);
    rows.push(epoch_row(DOMAIN, generation, epoch)?);
    Ok(rows)
}

/// Every row one structural state amounts to, stamped `epoch`.
pub(crate) fn structural_state_rows(
    generation: &AuxiliaryGenerationKeyV1,
    epoch: AuxEpochV1,
    state: &StructuralAuthorityState,
) -> Result<Vec<AuxiliaryRowMutationV1>, CoreError> {
    const DOMAIN: AuxiliaryDomainV1 = AuxiliaryDomainV1::Structural;
    let mut rows = Vec::new();
    for (chunk_id, chunk) in state.chunks() {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::Chunk,
            chunk_id.as_str().as_bytes().to_vec(),
            chunk,
        )?);
    }
    for (chunk_id, tree) in state.parse_trees() {
        rows.push(upsert(
            DOMAIN,
            generation,
            AuxiliaryRowFamilyV1::ParseTree,
            chunk_id.as_str().as_bytes().to_vec(),
            tree,
        )?);
    }
    rows.push(upsert(
        DOMAIN,
        generation,
        AuxiliaryRowFamilyV1::StateMeta,
        Vec::new(),
        &StructuralStateMeta {
            seal_requested: state.seal_requested(),
        },
    )?);
    rows.push(epoch_row(DOMAIN, generation, epoch)?);
    Ok(rows)
}

/// The structural track row for one pair.
pub(crate) fn structural_track_row(
    repo_id: &RepoId,
    revision_id: &RevisionId,
    track: &TrackAuthorityState,
) -> Result<AuxiliaryTrackRowV1, CoreError> {
    Ok(AuxiliaryTrackRowV1 {
        repo_id: repo_id.clone(),
        revision_id: revision_id.clone(),
        track: SearchPlaneTrackKind::Structural,
        value: encode("structural track", track)?,
    })
}

fn mismatched_family(domain: AuxiliaryDomainV1, family: AuxiliaryRowFamilyV1) -> CoreError {
    CoreError::Storage(format!(
        "auxiliary authority: row family {family} does not belong to domain {domain}"
    ))
}

/// Restore the `Epoch` row of one generation and domain into the ledger.
fn restore_epoch_row<S: AuxDomainState>(
    ledger: &mut Ledger,
    generation: &AuxiliaryGenerationKeyV1,
    value: &[u8],
) -> Result<(), CoreError> {
    let epoch: AuxEpochV1 = decode("epoch", value)?;
    ledger.aux_restore_epoch::<S>(
        &generation.repo_id,
        &generation.revision_id,
        generation.generation,
        epoch,
    );
    Ok(())
}

/// Restore one stored row into the ledger.
///
/// Rows rebuild the current snapshot in place; the `Epoch` row names the
/// epoch that snapshot has. A generation whose rows carry no `Epoch` row
/// stays at [`AuxEpochV1::GENESIS`].
pub(crate) fn restore_row_into(ledger: &mut Ledger, row: &AuxiliaryRowV1) -> Result<(), CoreError> {
    let key = &row.key;
    let generation = &key.generation;
    match key.domain {
        AuxiliaryDomainV1::History => {
            if key.family == AuxiliaryRowFamilyV1::Epoch {
                return restore_epoch_row::<HistoryAuthorityState>(ledger, generation, &row.value);
            }
            let state = ledger.aux_restore_mut::<HistoryAuthorityState>(
                &generation.repo_id,
                &generation.revision_id,
                generation.generation,
            );
            match key.family {
                AuxiliaryRowFamilyV1::Commit => {
                    let sha = commit_sha_from_bytes(&key.row_key)?;
                    let record: CommitRecord = decode("commit", &row.value)?;
                    state.restore_commit(sha, record);
                }
                AuxiliaryRowFamilyV1::Ref => {
                    let name = utf8_key("ref", &key.row_key)?;
                    let sha: CommitSha = decode("ref", &row.value)?;
                    state.restore_ref(name, sha);
                }
                AuxiliaryRowFamilyV1::Tag => {
                    let name = utf8_key("tag", &key.row_key)?;
                    let sha: CommitSha = decode("tag", &row.value)?;
                    state.restore_tag(name, sha);
                }
                AuxiliaryRowFamilyV1::DiffHunk => {
                    let diff_key = diff_key_from_bytes(&key.row_key)?;
                    let record: DiffHunkRecord = decode("diff-hunk", &row.value)?;
                    state.restore_diff_hunk(diff_key, record);
                }
                AuxiliaryRowFamilyV1::StateMeta => {
                    let meta: HistoryStateMeta = decode("history state-meta", &row.value)?;
                    state.restore_meta(meta);
                }
                AuxiliaryRowFamilyV1::DirtyDoc
                | AuxiliaryRowFamilyV1::ChangedDoc
                | AuxiliaryRowFamilyV1::DocFacet
                | AuxiliaryRowFamilyV1::Snapshot
                | AuxiliaryRowFamilyV1::AffectedDocs
                | AuxiliaryRowFamilyV1::InvalidatedByDocs
                | AuxiliaryRowFamilyV1::Chunk
                | AuxiliaryRowFamilyV1::ParseTree
                | AuxiliaryRowFamilyV1::Epoch => {
                    return Err(mismatched_family(key.domain, key.family));
                }
            }
        }
        AuxiliaryDomainV1::Runtime => {
            if key.family == AuxiliaryRowFamilyV1::Epoch {
                return restore_epoch_row::<RuntimeMetadataState>(ledger, generation, &row.value);
            }
            let state = ledger.aux_restore_mut::<RuntimeMetadataState>(
                &generation.repo_id,
                &generation.revision_id,
                generation.generation,
            );
            match key.family {
                AuxiliaryRowFamilyV1::DirtyDoc => {
                    let chunk_id = ChunkId::new(utf8_key("dirty-doc", &key.row_key)?);
                    state.restore_dirty_doc(chunk_id, decode("dirty-doc", &row.value)?);
                }
                AuxiliaryRowFamilyV1::ChangedDoc => {
                    let chunk_id = ChunkId::new(utf8_key("changed-doc", &key.row_key)?);
                    state.restore_changed_doc(chunk_id, decode("changed-doc", &row.value)?);
                }
                AuxiliaryRowFamilyV1::DocFacet => {
                    let chunk_id = ChunkId::new(utf8_key("doc-facet", &key.row_key)?);
                    state.restore_doc_facet(chunk_id, decode("doc-facet", &row.value)?);
                }
                AuxiliaryRowFamilyV1::Snapshot => {
                    let name = utf8_key("snapshot", &key.row_key)?;
                    state.restore_snapshot(name, decode("snapshot", &row.value)?);
                }
                AuxiliaryRowFamilyV1::AffectedDocs => {
                    let name = utf8_key("affected-docs", &key.row_key)?;
                    state.restore_affected_docs(name, decode("affected-docs", &row.value)?);
                }
                AuxiliaryRowFamilyV1::InvalidatedByDocs => {
                    let name = utf8_key("invalidated-by-docs", &key.row_key)?;
                    state.restore_invalidated_by_docs(
                        name,
                        decode("invalidated-by-docs", &row.value)?,
                    );
                }
                AuxiliaryRowFamilyV1::StateMeta => {
                    let meta: RuntimeStateMeta = decode("runtime state-meta", &row.value)?;
                    state.restore_meta(meta);
                }
                AuxiliaryRowFamilyV1::Commit
                | AuxiliaryRowFamilyV1::Ref
                | AuxiliaryRowFamilyV1::Tag
                | AuxiliaryRowFamilyV1::DiffHunk
                | AuxiliaryRowFamilyV1::Chunk
                | AuxiliaryRowFamilyV1::ParseTree
                | AuxiliaryRowFamilyV1::Epoch => {
                    return Err(mismatched_family(key.domain, key.family));
                }
            }
        }
        AuxiliaryDomainV1::Structural => {
            if key.family == AuxiliaryRowFamilyV1::Epoch {
                return restore_epoch_row::<StructuralAuthorityState>(
                    ledger, generation, &row.value,
                );
            }
            let state = ledger.aux_restore_mut::<StructuralAuthorityState>(
                &generation.repo_id,
                &generation.revision_id,
                generation.generation,
            );
            match key.family {
                AuxiliaryRowFamilyV1::Chunk => {
                    let chunk_id = ChunkId::new(utf8_key("chunk", &key.row_key)?);
                    state.restore_chunk(chunk_id, decode("chunk", &row.value)?);
                }
                AuxiliaryRowFamilyV1::ParseTree => {
                    let chunk_id = ChunkId::new(utf8_key("parse-tree", &key.row_key)?);
                    state.restore_parse_tree(chunk_id, decode("parse-tree", &row.value)?);
                }
                AuxiliaryRowFamilyV1::StateMeta => {
                    let meta: StructuralStateMeta = decode("structural state-meta", &row.value)?;
                    state.restore_meta(meta);
                }
                AuxiliaryRowFamilyV1::Commit
                | AuxiliaryRowFamilyV1::Ref
                | AuxiliaryRowFamilyV1::Tag
                | AuxiliaryRowFamilyV1::DiffHunk
                | AuxiliaryRowFamilyV1::DirtyDoc
                | AuxiliaryRowFamilyV1::ChangedDoc
                | AuxiliaryRowFamilyV1::DocFacet
                | AuxiliaryRowFamilyV1::Snapshot
                | AuxiliaryRowFamilyV1::AffectedDocs
                | AuxiliaryRowFamilyV1::InvalidatedByDocs
                | AuxiliaryRowFamilyV1::Epoch => {
                    return Err(mismatched_family(key.domain, key.family));
                }
            }
        }
    }
    Ok(())
}

/// Restore one stored track row into the ledger.
pub(crate) fn restore_track_row_into(
    ledger: &mut Ledger,
    row: &AuxiliaryTrackRowV1,
) -> Result<(), CoreError> {
    let state: TrackAuthorityState = decode("track", &row.value)?;
    ledger.restore_track_state(&row.repo_id, &row.revision_id, row.track, state);
    Ok(())
}

/// Test doubles shared by this crate's tests.
#[cfg(test)]
pub(crate) mod testing {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Mutex;

    use quanta_index_contract::{RepoId, RevisionId, SearchPlaneTrackKind};
    use quanta_index_core::CoreError;

    /// An in-memory auxiliary row catalog with the port's transaction
    /// semantics.
    ///
    /// A batch applies whole or not at all, rows are keyed exactly as the
    /// engine keys them, and a switch makes the next `apply` fail before
    /// anything is written, so a test can prove that a mutation whose rows
    /// never became durable is never visible.
    #[derive(Default)]
    pub(crate) struct MemoryAuxiliaryCatalog {
        rows: Mutex<BTreeMap<quanta_index_core::AuxiliaryRowKeyV1, Vec<u8>>>,
        tracks: Mutex<BTreeMap<(RepoId, RevisionId, SearchPlaneTrackKind), Vec<u8>>>,
        fail_next_apply: std::sync::atomic::AtomicBool,
        applies: std::sync::atomic::AtomicUsize,
        rows_written: std::sync::atomic::AtomicU64,
    }

    impl MemoryAuxiliaryCatalog {
        pub(crate) fn fail_next_apply(&self) {
            self.fail_next_apply
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }

        pub(crate) fn applies(&self) -> usize {
            self.applies.load(std::sync::atomic::Ordering::SeqCst)
        }

        pub(crate) fn rows_written(&self) -> u64 {
            self.rows_written.load(std::sync::atomic::Ordering::SeqCst)
        }

        pub(crate) fn row_count(&self) -> usize {
            let Ok(rows) = self.rows.lock() else {
                return usize::MAX;
            };
            rows.len()
        }

        pub(crate) fn generations(&self) -> BTreeSet<u64> {
            let Ok(rows) = self.rows.lock() else {
                return BTreeSet::from([u64::MAX]);
            };
            rows.keys()
                .map(|key| key.generation.generation.get())
                .collect()
        }
    }

    impl quanta_index_core::AuxiliaryAuthorityCatalogPort for MemoryAuxiliaryCatalog {
        fn apply(
            &self,
            batch: &quanta_index_core::AuxiliaryMutationBatchV1,
        ) -> Result<quanta_index_core::AuxiliaryMutationReceiptV1, CoreError> {
            if self
                .fail_next_apply
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(CoreError::Storage(
                    "memory auxiliary catalog: injected apply failure".to_string(),
                ));
            }
            let _prior = self
                .applies
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut rows = self
                .rows
                .lock()
                .map_err(|err| CoreError::Storage(format!("memory catalog poisoned: {err}")))?;
            let mut written = 0_u64;
            let mut deleted = 0_u64;
            for mutation in &batch.rows {
                match mutation {
                    quanta_index_core::AuxiliaryRowMutationV1::Upsert(row) => {
                        let _previous = rows.insert(row.key.clone(), row.value.clone());
                        written = written.saturating_add(1);
                    }
                    quanta_index_core::AuxiliaryRowMutationV1::Delete(key) => {
                        if rows.remove(key).is_some() {
                            deleted = deleted.saturating_add(1);
                        }
                    }
                    quanta_index_core::AuxiliaryRowMutationV1::ClearFamily {
                        domain,
                        generation,
                        family,
                    } => {
                        let before = rows.len();
                        rows.retain(|key, _value| {
                            !(key.domain == *domain
                                && key.generation == *generation
                                && key.family == *family)
                        });
                        deleted = deleted.saturating_add(
                            u64::try_from(before.saturating_sub(rows.len())).map_err(|err| {
                                CoreError::Storage(format!("count overflow: {err}"))
                            })?,
                        );
                    }
                    quanta_index_core::AuxiliaryRowMutationV1::ForgetGeneration(generation) => {
                        let before = rows.len();
                        rows.retain(|key, _value| key.generation != *generation);
                        deleted = deleted.saturating_add(
                            u64::try_from(before.saturating_sub(rows.len())).map_err(|err| {
                                CoreError::Storage(format!("count overflow: {err}"))
                            })?,
                        );
                    }
                }
            }
            drop(rows);
            let mut tracks = self
                .tracks
                .lock()
                .map_err(|err| CoreError::Storage(format!("memory catalog poisoned: {err}")))?;
            for track in &batch.tracks {
                let _previous = tracks.insert(
                    (
                        track.repo_id.clone(),
                        track.revision_id.clone(),
                        track.track,
                    ),
                    track.value.clone(),
                );
                written = written.saturating_add(1);
            }
            drop(tracks);
            let _prior = self
                .rows_written
                .fetch_add(written, std::sync::atomic::Ordering::SeqCst);
            Ok(quanta_index_core::AuxiliaryMutationReceiptV1 {
                rows_written: written,
                rows_deleted: deleted,
            })
        }

        fn for_each_row(
            &self,
            visit: &mut dyn FnMut(quanta_index_core::AuxiliaryRowV1) -> Result<(), CoreError>,
        ) -> Result<(), CoreError> {
            let snapshot: Vec<quanta_index_core::AuxiliaryRowV1> = {
                let rows = self
                    .rows
                    .lock()
                    .map_err(|err| CoreError::Storage(format!("memory catalog poisoned: {err}")))?;
                rows.iter()
                    .map(|(key, value)| quanta_index_core::AuxiliaryRowV1 {
                        key: key.clone(),
                        value: value.clone(),
                    })
                    .collect()
            };
            for row in snapshot {
                visit(row)?;
            }
            Ok(())
        }

        fn track_rows(&self) -> Result<Vec<quanta_index_core::AuxiliaryTrackRowV1>, CoreError> {
            let tracks = self
                .tracks
                .lock()
                .map_err(|err| CoreError::Storage(format!("memory catalog poisoned: {err}")))?;
            Ok(tracks
                .iter()
                .map(|((repo_id, revision_id, track), value)| {
                    quanta_index_core::AuxiliaryTrackRowV1 {
                        repo_id: repo_id.clone(),
                        revision_id: revision_id.clone(),
                        track: *track,
                        value: value.clone(),
                    }
                })
                .collect())
        }
    }
}
