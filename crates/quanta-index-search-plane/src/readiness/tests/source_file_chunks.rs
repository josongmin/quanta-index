//! Source-file ownership survives both durable-delta and channel-op consumers.

use std::time::Instant;

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::{
    AuxEpochV1, BatchIngestMode, ChunkId, ManifestGeneration, ReplaceLexicalScope, RepoId,
    RepoRelativePath, SearchCorpusIngestBatch, SearchCorpusTombstoneScope, SourceFileKey,
    SourcePublicationEvent, TombstoneLexicalScope,
};

use super::support::{
    TestResult, chunk_record, encode_cbor, file_replacement, generation, repo_id, revision_id,
};
use crate::auxiliary_authority;
use crate::readiness::{Ledger, StructuralAuthorityState};

fn file(repo: &str) -> SourceFileKey {
    SourceFileKey {
        source_repo_id: RepoId::new(repo).expect("fixture repo"),
        repo_relative_path: RepoRelativePath::new("src/shared.rs"),
    }
}

fn replacement(
    repo: &str,
    id: &str,
) -> Result<quanta_index_contract::SearchCorpusReplaceScope, Box<dyn std::error::Error>> {
    let mut chunk = chunk_record("src/shared.rs", "fn shared() {}")?;
    chunk.chunk_id = ChunkId::new(id);
    // Omitted chunk provenance must derive from file coverage, even when the
    // source repo differs from the containing materialization target.
    file_replacement(file(repo), vec![chunk])
}

#[test]
fn channel_file_replace_and_tombstone_preserve_other_source_repo() -> TestResult {
    let mut ledger = Ledger::default();
    for (repo, id) in [
        ("source-a", "a-old"),
        ("source-b", "b"),
        ("source-a", "a-new"),
    ] {
        ledger.apply_lexical_authority_op(
            &LexicalChannelOp::ReplaceLexicalScope(ReplaceLexicalScope {
                repo_id: repo_id(),
                revision_id: revision_id(),
                generation: generation(),
                payload: encode_cbor(&(
                    BatchIngestMode::ReplaceGeneration,
                    None::<ManifestGeneration>,
                    replacement(repo, id)?,
                ))?,
            }),
            Instant::now(),
        )?;
    }
    let state = ledger
        .structural_state(&repo_id(), &revision_id(), generation())
        .ok_or("missing state")?;
    if state.chunks().contains_key(&ChunkId::new("a-old"))
        || !state.chunks().contains_key(&ChunkId::new("a-new"))
        || state
            .chunks()
            .get(&ChunkId::new("b"))
            .and_then(|chunk| chunk.source_repo_id.as_ref())
            != Some(&file("source-b").source_repo_id)
    {
        return Err("file replacement lost source ownership or crossed repo boundary".into());
    }
    ledger.apply_lexical_authority_op(
        &LexicalChannelOp::TombstoneLexicalScope(TombstoneLexicalScope {
            repo_id: repo_id(),
            revision_id: revision_id(),
            generation: generation(),
            payload: encode_cbor(&(
                BatchIngestMode::ReplaceGeneration,
                None::<ManifestGeneration>,
                SearchCorpusTombstoneScope {
                    file: file("source-a"),
                },
            ))?,
        }),
        Instant::now(),
    )?;
    let state = ledger
        .structural_state(&repo_id(), &revision_id(), generation())
        .ok_or("missing state")?;
    if state.chunks().len() != 1 || !state.chunks().contains_key(&ChunkId::new("b")) {
        return Err("tombstone crossed source repo boundary".into());
    }
    Ok(())
}

#[test]
fn durable_file_delta_preserves_other_source_and_binds_omitted_provenance() -> TestResult {
    let mut state = StructuralAuthorityState::default();
    // The unscoped legacy fixture belongs only to the enclosing target repo.
    let mut legacy = chunk_record("src/shared.rs", "fn legacy() {}")?;
    legacy.chunk_id = ChunkId::new("legacy");
    state.restore_chunk(legacy.chunk_id.clone(), legacy);
    let mut batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "stream".into(),
            event_id: "event".into(),
            expected_base_event_id: None,
            payload_sha256: [1; 32],
        },
        repo_id: repo_id(),
        revision_id: revision_id(),
        generation: generation(),
        base_generation: None,
        manifest_digest: "manifest".into(),
        batch_digest: "batch".into(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![
            replacement("source-a", "a-old")?,
            replacement("source-b", "b")?,
        ],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: false,
    };
    let delta = auxiliary_authority::structural_chunks_transition(
        Some(&state),
        AuxEpochV1::GENESIS,
        &batch,
    );
    state.apply_chunks_delta(&delta);
    batch.replace_scopes = vec![replacement("source-a", "a-new")?];
    let delta = auxiliary_authority::structural_chunks_transition(
        Some(&state),
        AuxEpochV1::GENESIS,
        &batch,
    );
    if delta.removed.len() != 1 || !delta.removed.contains(&ChunkId::new("a-old")) {
        return Err("replace delta crossed source repo boundary".into());
    }
    state.apply_chunks_delta(&delta);
    batch.replace_scopes.clear();
    batch.tombstone_scopes = vec![SearchCorpusTombstoneScope {
        file: file("source-a"),
    }];
    let delta = auxiliary_authority::structural_chunks_transition(
        Some(&state),
        AuxEpochV1::GENESIS,
        &batch,
    );
    state.apply_chunks_delta(&delta);
    if state.chunks().len() != 2
        || !state.chunks().contains_key(&ChunkId::new("b"))
        || !state.chunks().contains_key(&ChunkId::new("legacy"))
    {
        return Err("tombstone delta crossed source repo boundary".into());
    }
    batch.tombstone_scopes = vec![SearchCorpusTombstoneScope {
        file: file(repo_id().as_str()),
    }];
    let delta = auxiliary_authority::structural_chunks_transition(
        Some(&state),
        AuxEpochV1::GENESIS,
        &batch,
    );
    state.apply_chunks_delta(&delta);
    if state.chunks().len() != 1 || !state.chunks().contains_key(&ChunkId::new("b")) {
        return Err("legacy unscoped chunk did not use containing repo identity".into());
    }
    Ok(())
}
