//! Canonical source fixtures for adapter tests. The caller authors the raw
//! bytes and the complete synthetic unit inventory; this is not parser proof.

use quanta_index_contract::lex::{LanguageCode, SymbolRecord};
use quanta_index_contract::{
    BatchIngestMode, ChunkRecord, ManifestGeneration, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SourceFileCoverage, SourceFileKey,
    SourceFileRevision, SourcePublicationEvent, SymbolCoverage, source_event_payload_sha256,
    source_file_unit_set_sha256,
};
use quanta_index_core::CoreError;
use sha2::{Digest as _, Sha256};

pub(super) fn complete_file(
    file: SourceFileKey,
    revision: &RevisionId,
    language: LanguageCode,
    raw_source: &[u8],
    chunks: Vec<ChunkRecord>,
    symbols: Vec<SymbolRecord>,
) -> Result<SearchCorpusReplaceScope, CoreError> {
    for chunk in &chunks {
        let start = usize::try_from(chunk.start_byte)
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        let end = usize::try_from(chunk.end_byte)
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        if raw_source.get(start..end) != Some(chunk.text.as_bytes()) {
            return Err(CoreError::InvalidContract(format!(
                "fixture chunk {} does not equal its raw source slice",
                chunk.chunk_id.as_str(),
            )));
        }
    }
    for symbol in &symbols {
        let start = usize::try_from(symbol.definition_span.byte_start)
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        let end = usize::try_from(symbol.definition_span.byte_end)
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        if raw_source.get(start..end) != Some(symbol.local_name.as_bytes()) {
            return Err(CoreError::InvalidContract(format!(
                "fixture symbol {} does not equal its authored name slice",
                symbol.symbol_id.as_str(),
            )));
        }
    }
    let coverage = SourceFileCoverage {
        source: SourceFileRevision {
            file,
            revision_id: revision.clone(),
            source_sha256: Sha256::digest(raw_source).into(),
        },
        language,
        producer_policy_sha256: Sha256::digest(b"lexical-test:hand-authored-units:v1").into(),
        unit_set_sha256: source_file_unit_set_sha256(&chunks, &symbols)
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?,
        text_admitted: !chunks.is_empty(),
        symbols: SymbolCoverage::Complete {
            symbol_count: u64::try_from(symbols.len())
                .map_err(|error| CoreError::InvalidContract(error.to_string()))?,
        },
    };
    Ok(SearchCorpusReplaceScope {
        coverage,
        chunks,
        symbols,
    })
}

pub(super) fn file_key(repo: &RepoId, path: &str) -> SourceFileKey {
    SourceFileKey {
        source_repo_id: repo.clone(),
        repo_relative_path: RepoRelativePath::new(path),
    }
}

pub(super) fn sealed_batch(
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    replace_scopes: Vec<SearchCorpusReplaceScope>,
) -> Result<SearchCorpusIngestBatch, CoreError> {
    let mut batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "lexical-fixture".into(),
            event_id: format!("event-{}", generation.get()),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        generation,
        base_generation: None,
        manifest_digest: format!("fixture-manifest-{}", generation.get()),
        // These tests enter the adapter, below IPC transport-digest validation.
        // The native source-event digest below still binds the complete payload.
        batch_digest: "0".repeat(64),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes,
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: Vec::new(),
        semantic_tombstone_scopes: Vec::new(),
        seal: true,
    };
    batch.source_event.payload_sha256 = source_event_payload_sha256(&batch)
        .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
    Ok(batch)
}
