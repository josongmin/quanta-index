//! `SearchCorpusBatch` assembly from chunker output (RB-02).
//!
//! One `replace_generation` batch: a lexical `File` scope per file plus one
//! `RawCodeFallback` semantic scope per chunk (the plane embeds chunk text
//! with the daemon's configured provider). Symbols are declared out of scope
//! for the benchmark profiles and are never synthesized.

use std::collections::BTreeMap;

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    CapabilityStatusV1, ChunkId, ChunkRecord, ManifestGeneration, OwnerDocKind, RawFallbackReasonV1,
    RepoId, RepoRelativePath, RevisionId, SearchScopeKey, SearchScopeSurface,
    SearchPlaneSearchCorpusActivationCasAck, SemanticCorpusKindV1,
    SemanticSourceRecordV1, SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1, SourceRoleV1,
};
use quanta_index_sdk::SearchCorpusBatch;

use crate::chunking::Chunk;
use crate::{BenchError, BenchResult, sha256_hex};

pub const SEMANTIC_AUTHORITY_DIGEST: &str = "quanta-retrieval-bench:chunk-source:v1";
pub const SEMANTIC_RENDER_POLICY_DIGEST: &str = "quanta-retrieval-bench:chunk-source:v1";

/// Batch identity: repo/revision/generation triple plus the manifest digest
/// binding the admitted universe, strategy and configuration.
#[derive(Debug, Clone)]
pub struct BatchIdentity {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub manifest_digest: String,
}

impl BatchIdentity {
    pub fn new(
        repo_id: &str,
        revision_id: &str,
        generation: u64,
        manifest_digest: String,
    ) -> BenchResult<Self> {
        let repo_id = RepoId::new(repo_id).map_err(|err| {
            BenchError::Config(format!("invalid repo_id: {err}"))
        })?;
        let revision_id = RevisionId::new(revision_id).map_err(|err| {
            BenchError::Config(format!("invalid revision_id: {err}"))
        })?;
        Ok(Self {
            repo_id,
            revision_id,
            generation: ManifestGeneration::new(generation),
            manifest_digest,
        })
    }
}

fn language_for(path: &str) -> BenchResult<LanguageCode> {
    let code = match path.rsplit('.').next().unwrap_or("") {
        "rs" => "rust",
        "py" => "python",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescript",
        "go" => "go",
        "java" => "java",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "rb" => "ruby",
        "php" => "php",
        "swift" => "swift",
        "kt" => "kotlin",
        "scala" => "scala",
        "md" => "markdown",
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "sh" | "bash" => "shell",
        "css" => "css",
        "html" => "html",
        other => {
            return Err(BenchError::Config(format!(
                "no language mapping for admitted file (exclude it from the manifest): {path} (.{other})"
            )));
        }
    };
    LanguageCode::new(code).map_err(|err| BenchError::Config(format!("bad language code: {err}")))
}

/// Canonical scope digest over ordered chunk descriptors.
fn scope_digest(chunks: &[Chunk]) -> String {
    let mut raw = Vec::new();
    for chunk in chunks {
        for part in [
            chunk.chunk_id.as_str(),
            &chunk.start_byte.to_string(),
            &chunk.end_byte.to_string(),
            &chunk.start_line.to_string(),
            &chunk.end_line.to_string(),
            &sha256_hex(chunk.text.as_bytes()),
        ] {
            raw.extend_from_slice(part.as_bytes());
            raw.push(0);
        }
    }
    sha256_hex(&raw)
}

fn chunk_record(chunk: &Chunk) -> BenchResult<ChunkRecord> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(chunk.chunk_id.clone()),
        repo_relative_path: RepoRelativePath::new(chunk.path.clone()),
        language: language_for(&chunk.path)?,
        start_byte: chunk.start_byte,
        end_byte: chunk.end_byte,
        start_line: chunk.start_line,
        end_line: chunk.end_line,
        text: chunk.text.clone().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

fn semantic_scope(chunk: &Chunk) -> BenchResult<SemanticSourceReplaceScopeV1> {
    let language = language_for(&chunk.path)?;
    Ok(SemanticSourceReplaceScopeV1 {
        scope: SemanticSourceScopeKeyV1 {
            corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
            owner_kind: OwnerDocKind::Chunk,
            owner_id: chunk.chunk_id.clone(),
        },
        scope_digest: format!("retrieval-bench:semantic:{}", chunk.chunk_id),
        sources: vec![SemanticSourceRecordV1 {
            record_id: chunk.chunk_id.clone(),
            corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
            owner_kind: OwnerDocKind::Chunk,
            owner_id: chunk.chunk_id.clone(),
            source_doc_id: chunk.chunk_id.clone(),
            parent_owner_id: Some(chunk.path.clone()),
            repo_relative_path: RepoRelativePath::new(chunk.path.clone()),
            language: Some(language.as_str().to_string()),
            package: None,
            symbol_kind: None,
            visibility: None,
            source_role: SourceRoleV1::RawFallbackText,
            generated: false,
            capability_status: CapabilityStatusV1::Degraded,
            raw_fallback_reason: Some(RawFallbackReasonV1::IntentNotRecoverableFromStructure),
            authority_digest: SEMANTIC_AUTHORITY_DIGEST.to_string(),
            render_policy_digest: SEMANTIC_RENDER_POLICY_DIGEST.to_string(),
            card_schema_version: 0,
            text: chunk.text.clone(),
        }],
        cluster_memberships: Vec::new(),
    })
}

/// Assemble the publishable batch. Files with zero chunks are skipped (an
/// empty file holds nothing retrievable) and reported in `skipped_empty`.
pub fn assemble_batch(
    identity: &BatchIdentity,
    chunks: &BTreeMap<String, Vec<Chunk>>,
) -> BenchResult<(SearchCorpusBatch, BatchAssemblyReport)> {
    let mut batch = SearchCorpusBatch::replace_generation(
        identity.repo_id.clone(),
        identity.revision_id.clone(),
        identity.generation,
        identity.manifest_digest.clone(),
    );
    let mut report = BatchAssemblyReport::default();
    for (path, file_chunks) in chunks {
        if file_chunks.is_empty() {
            report.skipped_empty.push(path.clone());
            continue;
        }
        let digest = scope_digest(file_chunks);
        let mut records = Vec::with_capacity(file_chunks.len());
        for chunk in file_chunks {
            records.push(chunk_record(chunk)?);
        }
        batch = batch.replace_scope(
            SearchScopeKey {
                doc_surface: SearchScopeSurface::File,
                repo_relative_path: RepoRelativePath::new(path.clone()),
            },
            digest,
            records,
            Vec::new(),
        );
        report.scopes += 1;
        report.chunks += file_chunks.len();
        for chunk in file_chunks {
            let scope = semantic_scope(chunk)?;
            let scope_digest = scope.scope_digest.clone();
            batch = batch.replace_semantic_scope(scope.scope, scope_digest, scope.sources);
            report.semantic_scopes += 1;
        }
    }
    if report.scopes == 0 {
        return Err(BenchError::Protocol(
            "batch holds no lexical scopes; nothing admitted is retrievable".to_string(),
        ));
    }
    Ok((batch, report))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchAssemblyReport {
    pub scopes: usize,
    pub chunks: usize,
    pub semantic_scopes: usize,
    pub skipped_empty: Vec<String>,
}

/// Expected composite identity the activation ACK must match. The SDK
/// validates the ACK itself; the runner re-checks the lexical/revision
/// binding against the batch it built.
#[must_use]
pub fn expected_candidate_description(
    identity: &BatchIdentity,
    ack: &SearchPlaneSearchCorpusActivationCasAck,
) -> String {
    format!(
        "repo={} rev={} gen={} manifest={} active={:?}",
        identity.repo_id.as_str(),
        identity.revision_id.as_str(),
        identity.generation.get(),
        identity.manifest_digest,
        ack.active,
    )
}
