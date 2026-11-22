//! One canonical Source fixture for SDK Frontdoor CodeSearch and its Legacy process replay.

use std::error::Error;

use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    ChunkId, ChunkRecord, GenerationPin, RepoId, RepoRelativePath, RevisionId, SourceFileKey,
    SourcePublicationEvent, SymbolCoverage, SymbolId,
};
use quanta_index_sdk::{RepoMetaBatch, SearchCorpusBatch};
use quanta_index_searchd_harness::{
    fixture_source_scope_v1, semantic_source_scopes_for_chunk_records,
};

fn chunk(id: &str, path: &str, text: &str, end_byte: u32) -> Result<ChunkRecord, Box<dyn Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")?,
        start_byte: 0,
        end_byte,
        start_line: 1,
        end_line: 1,
        text: text.to_owned().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

pub(crate) fn symbol_record() -> Result<SymbolRecord, Box<dyn Error>> {
    Ok(SymbolRecord {
        symbol_id: SymbolId::new("sym-sdk"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: LanguageCode::new("rust")?,
        symbol_kind: SymbolKindCode::new("function")?,
        symbol_kind_family: Some(SymbolKindFamily::Callable),
        local_name: "MySdkSymbol".into(),
        qualified_name: "crate::MySdkSymbol".to_string().into_boxed_str(),
        signature: None,
        visibility: None,
        definition_span: SymbolSpan {
            path: "src/lib.rs".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: 12,
            line_start: 1,
            line_end: 1,
        },
        container_qualified_name: Some("crate".to_string().into_boxed_str()),
        relationship: SymbolRelationship::Def,
    })
}

pub(crate) fn source_batch(pin: &GenerationPin) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let mut batch = SearchCorpusBatch::replace_generation(
        pin.repo_id.clone(),
        pin.revision_id.clone(),
        pin.manifest_generation,
        "manifest:lexical",
    )
    .source_event(SourcePublicationEvent {
        stream_id: "fixture:sdk-frontdoor".into(),
        event_id: "fixture:sdk-lexical-v1".into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    });
    for (path, chunks, symbols) in [
        (
            "src/lib.rs",
            vec![
                chunk("chunk-dirty", "src/lib.rs", "todo!()", 12)?,
                chunk("chunk-tree", "src/lib.rs", "fn main() {}", 12)?,
            ],
            vec![symbol_record()?],
        ),
        (
            "src/alpha.rs",
            vec![chunk("alpha", "src/alpha.rs", "sphinx of quartz", 16)?],
            Vec::new(),
        ),
        (
            "src/beta.rs",
            vec![chunk("beta", "src/beta.rs", "sphinx riddles", 14)?],
            Vec::new(),
        ),
    ] {
        let no_symbols = symbols.is_empty();
        let mut scope = fixture_source_scope_v1(
            SourceFileKey {
                source_repo_id: pin.repo_id.clone(),
                repo_relative_path: RepoRelativePath::new(path),
            },
            pin.revision_id.clone(),
            chunks,
            symbols,
        )?;
        if no_symbols {
            scope.coverage.symbols = SymbolCoverage::Complete { symbol_count: 0 };
        }
        batch = batch.replace_scope(
            scope.coverage,
            scope.source_bytes,
            scope.chunks,
            scope.symbols,
        );
    }
    let chunks = batch
        .replace_scopes()
        .iter()
        .flat_map(|scope| scope.chunks.iter().cloned())
        .collect::<Vec<_>>();
    for semantic in semantic_source_scopes_for_chunk_records(&pin.repo_id, &chunks) {
        batch = batch.replace_semantic_scope(
            semantic.scope,
            semantic.scope_digest,
            semantic.sources,
            semantic.cluster_memberships,
        );
    }
    Ok(batch)
}

fn matrix_chunk(
    id: &str,
    path: &str,
    text: &str,
    end_byte: u32,
    source_repo_id: Option<&str>,
) -> Result<ChunkRecord, Box<dyn Error>> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: RepoRelativePath::new(path),
        language: LanguageCode::new("rust")?,
        start_byte: 0,
        end_byte,
        start_line: 1,
        end_line: 1,
        text: text.to_owned().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: source_repo_id.map(RepoId::new).transpose()?,
    })
}

fn replace_matrix_file(
    batch: SearchCorpusBatch,
    source_repo_id: RepoId,
    revision_id: RevisionId,
    path: &str,
    chunk: ChunkRecord,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let mut scope = fixture_source_scope_v1(
        SourceFileKey {
            source_repo_id,
            repo_relative_path: RepoRelativePath::new(path),
        },
        revision_id,
        vec![chunk],
        Vec::new(),
    )?;
    scope.coverage.symbols = SymbolCoverage::Complete { symbol_count: 0 };
    Ok(batch.replace_scope(
        scope.coverage,
        scope.source_bytes,
        scope.chunks,
        scope.symbols,
    ))
}

/// Exact Source rows used by the SDK Frontdoor matrix and binary DSL proof.
pub(crate) fn matrix_source_batch(
    pin: &GenerationPin,
) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let mut batch = source_batch(pin)?;
    for (source_repo, path, id, text, end_byte, source_repo_id) in [
        (
            pin.repo_id.clone(),
            "src/file_contains.rs",
            "chunk-file-contains",
            "foo oo_ba file_contains_needle",
            29,
            None,
        ),
        (
            RepoId::new("corp-a")?,
            "src/recency_a.rs",
            "chunk-recency-a",
            "shared_oracle_needle corp-a branch",
            33,
            Some("corp-a"),
        ),
        (
            RepoId::new("corp-a")?,
            "src/recency_gate.rs",
            "chunk-recency-a-gate",
            "shared_oracle_needle gate-a only",
            32,
            Some("corp-a"),
        ),
        (
            RepoId::new("corp-b")?,
            "src/recency_b.rs",
            "chunk-recency-b",
            "shared_oracle_needle corp-b branch",
            33,
            Some("corp-b"),
        ),
    ] {
        batch = replace_matrix_file(
            batch,
            source_repo,
            pin.revision_id.clone(),
            path,
            matrix_chunk(id, path, text, end_byte, source_repo_id)?,
        )?;
    }
    Ok(batch)
}

pub(crate) fn matrix_repo_meta_batch(pin: &GenerationPin) -> Result<RepoMetaBatch, Box<dyn Error>> {
    Ok(RepoMetaBatch::new(
        pin.repo_id.clone(),
        pin.revision_id.clone(),
        pin.manifest_generation,
    )
    .entry(RepoId::new("corp-a")?, "license", "apache-2.0")
    .entry(RepoId::new("corp-b")?, "license", "gpl-3.0"))
}
