//! Source-bound lexical adapter fixtures shared by integration targets.

use std::error::Error;

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    ChunkRecord, RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch,
    SearchCorpusReplaceScope, SourceFileCoverage, SourceFileKey, SourceFileRevision,
    SourcePublicationEvent, SymbolCoverage, source_event_payload_sha256,
    source_file_unit_set_sha256,
};
use sha2::{Digest as _, Sha256};

pub(crate) fn text_scope(
    repo: &RepoId,
    revision: &RevisionId,
    path: &str,
    language: LanguageCode,
    body: &str,
    chunks: Vec<ChunkRecord>,
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    for chunk in &chunks {
        if chunk.repo_relative_path.as_str() != path || chunk.language != language {
            return Err(format!(
                "fixture chunk {} has wrong file identity",
                chunk.chunk_id.as_str()
            )
            .into());
        }
        let start = usize::try_from(chunk.start_byte)?;
        let end = usize::try_from(chunk.end_byte)?;
        if body.as_bytes().get(start..end) != Some(chunk.text.as_bytes()) {
            return Err(format!(
                "fixture chunk {} differs from source",
                chunk.chunk_id.as_str()
            )
            .into());
        }
    }
    let unit_set_sha256 = source_file_unit_set_sha256(&chunks, &[])?;
    Ok(SearchCorpusReplaceScope {
        source_bytes: body.as_bytes().to_vec(),
        coverage: SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: repo.clone(),
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: revision.clone(),
                source_sha256: Sha256::digest(body.as_bytes()).into(),
            },
            language,
            producer_policy_sha256: Sha256::digest(b"lexical-test-source-fixture-v1").into(),
            unit_set_sha256,
            text_admitted: !chunks.is_empty(),
            symbols: SymbolCoverage::NotRequested,
        },
        chunks,
        symbols: Vec::new(),
    })
}

pub(crate) fn empty_event() -> SourcePublicationEvent {
    SourcePublicationEvent {
        stream_id: String::new(),
        event_id: String::new(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    }
}

/// Recompute source publication after a test mutates a valid fixture batch.
/// These tests call the adapter below the IPC batch-digest verifier.
pub(crate) fn finish_batch(batch: &mut SearchCorpusIngestBatch) -> Result<(), Box<dyn Error>> {
    let event_id = |generation| {
        format!(
            "lexical-fixture-{}-{}-g{}",
            batch.repo_id.as_str(),
            batch.revision_id.as_str(),
            generation
        )
    };
    batch.source_event = SourcePublicationEvent {
        stream_id: "lexical-test-source-v1".to_string(),
        event_id: event_id(batch.generation.get()),
        expected_base_event_id: batch.base_generation.map(|base| event_id(base.get())),
        payload_sha256: [0; 32],
    };
    batch.source_event.payload_sha256 = source_event_payload_sha256(batch)?;
    let mut digest = Sha256::new();
    digest.update(b"lexical-test-direct-adapter-batch:v1");
    digest.update(batch.generation.get().to_le_bytes());
    digest.update(batch.source_event.payload_sha256);
    batch.batch_digest = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .concat();
    Ok(())
}
