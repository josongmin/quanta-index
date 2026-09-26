//! The old tests use compact operation constructors to author units. Convert
//! those fixture inputs into complete source files before invoking production
//! ingestion. No raw index state is subsequently relabeled as covered.
//!
//! Each file's bytes are the authored chunk literals and symbol names separated
//! by newlines. Byte spans are rebased; existing line metadata remains the test's
//! explicit ranking/filter input, not a claim about a source parser.

use std::collections::BTreeMap;

use quanta_index_contract::channel::LexicalChannelOp;
use quanta_index_contract::lex::{LanguageCode, SymbolRecord};
use quanta_index_contract::{
    ChunkRecord, ManifestGeneration, RepoId, RevisionId, SearchCorpusIngestBatch, SourceFileKey,
    source_event_payload_sha256,
};
use quanta_index_core::CoreError;

use super::source_fixture;

struct File {
    language: LanguageCode,
    raw: String,
    chunks: Vec<ChunkRecord>,
    symbols: Vec<SymbolRecord>,
}

fn file<'a>(
    files: &'a mut BTreeMap<SourceFileKey, File>,
    key: SourceFileKey,
    language: &LanguageCode,
) -> Result<&'a mut File, CoreError> {
    let file = files.entry(key).or_insert_with(|| File {
        language: language.clone(),
        raw: String::new(),
        chunks: Vec::new(),
        symbols: Vec::new(),
    });
    if file.language != *language {
        return Err(CoreError::InvalidContract(
            "fixture units disagree on a file's language".into(),
        ));
    }
    Ok(file)
}

fn append(raw: &mut String, text: &str) -> Result<(u32, u32), CoreError> {
    let start =
        u32::try_from(raw.len()).map_err(|error| CoreError::InvalidContract(error.to_string()))?;
    raw.push_str(text);
    let end =
        u32::try_from(raw.len()).map_err(|error| CoreError::InvalidContract(error.to_string()))?;
    raw.push('\n');
    Ok((start, end))
}

pub(super) fn batch(
    repo: &RepoId,
    revision: &RevisionId,
    generation: ManifestGeneration,
    ops: &[LexicalChannelOp],
) -> Result<SearchCorpusIngestBatch, CoreError> {
    let mut files = BTreeMap::new();
    let mut bundle_payload = None;
    for op in ops {
        if op.repo_id() != repo || op.revision_id() != revision || op.generation() != generation {
            return Err(CoreError::InvalidContract(
                "fixture op belongs to another target".into(),
            ));
        }
        match op {
            LexicalChannelOp::UpsertChunk(op) => {
                let mut chunk: ChunkRecord = ciborium::from_reader(op.payload.as_slice())
                    .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
                chunk.chunk_id.clone_from(&op.chunk_id);
                let key = source_fixture::file_key(
                    chunk.source_repo_id.as_ref().unwrap_or(repo),
                    chunk.repo_relative_path.as_str(),
                );
                let file = file(&mut files, key, &chunk.language)?;
                (chunk.start_byte, chunk.end_byte) = append(&mut file.raw, &chunk.text)?;
                file.chunks.push(chunk);
            }
            LexicalChannelOp::UpsertSymbol(op) => {
                let mut symbol: SymbolRecord = ciborium::from_reader(op.payload.as_slice())
                    .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
                symbol.symbol_id.clone_from(&op.symbol_id);
                let key = source_fixture::file_key(repo, symbol.repo_relative_path.as_str());
                let file = file(&mut files, key, &symbol.language)?;
                (
                    symbol.definition_span.byte_start,
                    symbol.definition_span.byte_end,
                ) = append(&mut file.raw, &symbol.local_name)?;
                file.symbols.push(symbol);
            }
            LexicalChannelOp::FullBundle(op) => {
                if bundle_payload.replace(op.payload.clone()).is_some() {
                    return Err(CoreError::InvalidContract(
                        "duplicate fixture bundle".into(),
                    ));
                }
            }
            LexicalChannelOp::ClearLexicalSurface(_)
            | LexicalChannelOp::ReplaceLexicalScope(_)
            | LexicalChannelOp::TombstoneLexicalScope(_)
            | LexicalChannelOp::Seal(_)
            | LexicalChannelOp::UpsertCommit(_)
            | LexicalChannelOp::UpsertRef(_)
            | LexicalChannelOp::UpsertParseTree(_)
            | LexicalChannelOp::ReplaceStructuralScope(_) => {
                return Err(CoreError::InvalidContract(
                    "unsupported fixture operation".into(),
                ));
            }
        }
    }
    let scopes = files
        .into_iter()
        .map(|(key, file)| {
            source_fixture::complete_file(
                key,
                revision,
                file.language,
                file.raw.as_bytes(),
                file.chunks,
                file.symbols,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut batch = source_fixture::sealed_batch(repo, revision, generation, scopes)?;
    batch.bundle_payload = bundle_payload;
    batch.source_event.payload_sha256 = source_event_payload_sha256(&batch)
        .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
    Ok(batch)
}
