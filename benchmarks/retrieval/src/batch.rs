//! `SearchCorpusBatch` assembly from chunker output (RB-02, RBR-04).
//!
//! One `replace_generation` batch: a lexical `File` scope per file carrying
//! **both** the file's chunk records and its source-bound symbol records in
//! a single combined replacement (RBR-04: no second replacement for the
//! same path), plus one `RawCodeFallback` semantic scope per chunk. The
//! shared coverage commits the source bytes, symbol payload and producer
//! policy. Empty files are admitted with explicit zero-unit coverage.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::lex::LanguageCode;
use quanta_index_contract::{
    CapabilityStatusV1, ChunkId, ChunkRecord, ManifestGeneration, OwnerDocKind,
    RawFallbackReasonV1, RepoId, RepoRelativePath, RevisionId,
    SearchPlaneSearchCorpusActivationCasAck, SemanticCorpusKindV1, SemanticSourceRecordV1,
    SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1, SourceFileKey, SourceFileRevision,
    SourcePublicationEvent, SourceRoleV1,
};
use quanta_index_sdk::{BatchReceipt, SearchCorpusBatch};

use crate::chunking::Chunk;
use crate::corpus::SourceFile;
use crate::symbols::{SymbolCoveragePolicy, SymbolPreflight};
use crate::{BenchError, BenchResult, sha256_hex};
use sha2::{Digest, Sha256};

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
        let repo_id = RepoId::new(repo_id)
            .map_err(|err| BenchError::Config(format!("invalid repo_id: {err}")))?;
        let revision_id = RevisionId::new(revision_id)
            .map_err(|err| BenchError::Config(format!("invalid revision_id: {err}")))?;
        Ok(Self {
            repo_id,
            revision_id,
            generation: ManifestGeneration::new(generation),
            manifest_digest,
        })
    }
}

fn language_for(path: &str) -> BenchResult<LanguageCode> {
    let code = match path.rsplit_once('.').map_or("", |(_, extension)| extension) {
        "rs" => "rust",
        "py" => "python",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" | "tsx" => "typescript",
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
        // Unclassified admitted UTF-8 is a text surface. Strict symbol admission
        // has already refused it; AllowIncomplete preserves Unsupported facts.
        _ => "text",
    };
    LanguageCode::new(code).map_err(|err| BenchError::Config(format!("bad language code: {err}")))
}

fn chunk_record(chunk: &Chunk, source_repo: &RepoId) -> BenchResult<ChunkRecord> {
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
        source_repo_id: Some(source_repo.clone()),
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

/// Assemble one canonical replacement for every admitted source file.
///
/// Empty files remain members of the generation; coverage comes only from
/// preflight over these exact bytes, never from unit-vector presence.
pub fn assemble_batch(
    identity: &BatchIdentity,
    chunks: &BTreeMap<String, Vec<Chunk>>,
    files: &BTreeMap<String, SourceFile>,
    preflight: &SymbolPreflight,
    policy: SymbolCoveragePolicy,
    source_event: SourcePublicationEvent,
) -> BenchResult<(SearchCorpusBatch, BatchAssemblyReport)> {
    preflight.admit(policy)?;
    source_event
        .validate()
        .map_err(|error| BenchError::Config(error.to_string()))?;
    if files.is_empty() {
        return Err(BenchError::Protocol(
            "batch has no admitted source files".to_string(),
        ));
    }
    if !files.keys().eq(preflight.symbols().keys())
        || chunks.keys().any(|path| !files.contains_key(path))
    {
        return Err(BenchError::Protocol(
            "batch source universe differs from preflight/chunks".to_string(),
        ));
    }
    let mut batch = SearchCorpusBatch::replace_generation(
        identity.repo_id.clone(),
        identity.revision_id.clone(),
        identity.generation,
        identity.manifest_digest.clone(),
    )
    .source_event(source_event);
    let mut report = BatchAssemblyReport::default();
    let mut unit_ids = BTreeSet::new();
    for (path, file) in files {
        let file_chunks: &[Chunk] = chunks.get(path).map_or(&[], Vec::as_slice);
        let file_symbols = preflight
            .symbols()
            .get(path)
            .ok_or_else(|| BenchError::Protocol(format!("preflight lost admitted file: {path}")))?;
        let records = file_chunks
            .iter()
            .map(|chunk| chunk_record(chunk, &identity.repo_id))
            .collect::<BenchResult<Vec<_>>>()?;
        let source = SourceFileRevision {
            file: SourceFileKey {
                source_repo_id: identity.repo_id.clone(),
                repo_relative_path: RepoRelativePath::new(path.clone()),
            },
            revision_id: identity.revision_id.clone(),
            source_sha256: Sha256::digest(&file.bytes).into(),
        };
        let coverage = preflight.coverage_for(source, language_for(path)?, file, &records)?;
        for id in records
            .iter()
            .map(|chunk| chunk.chunk_id.as_str())
            .chain(file_symbols.iter().map(|symbol| symbol.symbol_id.as_str()))
        {
            if !unit_ids.insert(id.to_string()) {
                return Err(BenchError::Protocol(format!(
                    "duplicate published unit ID across source files: {id}"
                )));
            }
        }
        batch = batch.replace_scope(coverage, records, file_symbols.clone());
        report.scopes = report
            .scopes
            .checked_add(1)
            .ok_or_else(|| BenchError::Protocol("source scope count overflow".to_string()))?;
        report.chunks = report
            .chunks
            .checked_add(file_chunks.len())
            .ok_or_else(|| BenchError::Protocol("chunk count overflow".to_string()))?;
        report.symbols = report
            .symbols
            .checked_add(file_symbols.len())
            .ok_or_else(|| BenchError::Protocol("symbol count overflow".to_string()))?;
        if file_chunks.is_empty() {
            if file_symbols.is_empty() {
                report.empty_scopes.push(path.clone());
            } else {
                report.symbol_only_scopes.push(path.clone());
            }
        }
        for chunk in file_chunks {
            let scope = semantic_scope(chunk)?;
            batch = batch.replace_semantic_scope(
                scope.scope,
                scope.scope_digest,
                scope.sources,
                Vec::new(),
            );
            report.semantic_scopes = report
                .semantic_scopes
                .checked_add(1)
                .ok_or_else(|| BenchError::Protocol("semantic scope count overflow".to_string()))?;
        }
    }
    Ok((batch, report))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchAssemblyReport {
    pub scopes: usize,
    pub chunks: usize,
    pub symbols: usize,
    pub semantic_scopes: usize,
    pub empty_scopes: Vec<String>,
    pub symbol_only_scopes: Vec<String>,
}

/// Digest of the canonical sealed-receipt JSON: the capture's
/// `receipt_digest` binding. Fails rather than digesting a value the
/// canonical form refuses (floats would diverge from the evaluator).
pub fn receipt_digest(receipt: &BatchReceipt) -> BenchResult<String> {
    let value = serde_json::to_value(receipt).map_err(|err| BenchError::Json {
        path: "<batch-receipt>".to_string(),
        message: err.to_string(),
    })?;
    Ok(sha256_hex(
        crate::canonical::canonical_json(&value)?.as_bytes(),
    ))
}

/// Digest of the canonical activated-identity JSON (`ack.active`): the
/// capture's `activation_digest` binding.
pub fn activation_digest(ack: &SearchPlaneSearchCorpusActivationCasAck) -> BenchResult<String> {
    let value = serde_json::to_value(&ack.active).map_err(|err| BenchError::Json {
        path: "<activation-identity>".to_string(),
        message: err.to_string(),
    })?;
    Ok(sha256_hex(
        crate::canonical::canonical_json(&value)?.as_bytes(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbols::{SymbolPreflightOptions, preflight_corpus_symbols};
    use quanta_index_contract::SymbolCoverage;

    fn identity() -> BatchIdentity {
        BatchIdentity::new("bench-repo", "bench-rev", 3, "manifest:test".to_string())
            .expect("identity")
    }
    fn event() -> SourcePublicationEvent {
        SourcePublicationEvent {
            stream_id: "fixture-producer".to_string(),
            event_id: "source-event".to_string(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        }
    }
    fn files(rows: &[(&str, &str)]) -> BTreeMap<String, SourceFile> {
        rows.iter()
            .map(|(path, text)| {
                (
                    path.to_string(),
                    SourceFile {
                        path: path.to_string(),
                        bytes: text.as_bytes().to_vec(),
                        text: text.to_string(),
                        line_starts: crate::corpus::split_line_starts(text).0,
                        sha256: sha256_hex(text.as_bytes()),
                    },
                )
            })
            .collect()
    }
    fn chunk(path: &str, text: &str) -> Chunk {
        Chunk {
            chunk_id: format!("chunk-{path}-0"),
            path: path.to_string(),
            start_byte: 0,
            end_byte: u32::try_from(text.len()).expect("fixture length"),
            start_line: 1,
            end_line: u32::try_from(crate::corpus::split_line_starts(text).0.len()).expect("lines"),
            text: text.to_string(),
            strategy: "whole_file".to_string(),
            version: "v1".to_string(),
            config: "{}".to_string(),
            fallback: false,
        }
    }
    fn build(
        files: &BTreeMap<String, SourceFile>,
        chunks: &BTreeMap<String, Vec<Chunk>>,
        policy: SymbolCoveragePolicy,
    ) -> BenchResult<(SearchCorpusBatch, BatchAssemblyReport)> {
        let preflight = preflight_corpus_symbols(files, &SymbolPreflightOptions::default())?;
        assemble_batch(&identity(), chunks, files, &preflight, policy, event())
    }
    const RUST_SOURCE: &str = "pub fn first() {}\npub fn second() {}\n";

    #[test]
    fn incomplete_profile_preserves_plain_and_unclassified_text() {
        let source = files(&[
            ("notes.txt", "plain text marker"),
            ("LICENSE", "extensionless marker"),
            ("opaque.custom", "unclassified marker"),
            ("rs", "extensionless Rust-like filename"),
            ("json", "extensionless JSON-like filename"),
        ]);
        let chunks = source
            .iter()
            .map(|(path, file)| (path.clone(), vec![chunk(path, &file.text)]))
            .collect();
        assert!(build(&source, &chunks, SymbolCoveragePolicy::RequireComplete).is_err());
        let (batch, report) = build(&source, &chunks, SymbolCoveragePolicy::AllowIncomplete)
            .expect("explicit text admission must retain every unsupported source");
        assert_eq!((report.scopes, report.chunks, report.symbols), (5, 5, 0));
        for scope in batch.replace_scopes() {
            assert_eq!(scope.coverage.language.as_str(), "text");
            assert_eq!(scope.coverage.symbols, SymbolCoverage::Unsupported);
            assert!(scope.coverage.text_admitted);
            assert_eq!(scope.chunks.len(), 1);
            assert_eq!(
                scope.chunks.first().expect("one chunk").language.as_str(),
                "text"
            );
        }
        assert_eq!(batch.semantic_replace_scopes().len(), 5);
        for scope in batch.semantic_replace_scopes() {
            assert_eq!(scope.sources.len(), 1);
            assert_eq!(
                scope
                    .sources
                    .first()
                    .expect("one source")
                    .language
                    .as_deref(),
                Some("text")
            );
        }
    }

    #[test]
    fn combined_replacement_carries_chunks_symbols_and_source_coverage() {
        let files = files(&[("src/lib.rs", RUST_SOURCE)]);
        let chunks = BTreeMap::from([(
            "src/lib.rs".to_string(),
            vec![chunk("src/lib.rs", RUST_SOURCE)],
        )]);
        let (batch, report) =
            build(&files, &chunks, SymbolCoveragePolicy::RequireComplete).expect("batch");
        assert_eq!((report.scopes, report.chunks, report.symbols), (1, 1, 2));
        let scope = batch.replace_scopes().first().expect("combined scope");
        assert_eq!(
            scope.coverage.symbols,
            SymbolCoverage::Complete { symbol_count: 2 }
        );
        assert_eq!(
            scope.coverage.source.source_sha256,
            <[u8; 32]>::from(Sha256::digest(RUST_SOURCE.as_bytes()))
        );
        assert_eq!(
            scope.coverage.source.file.source_repo_id,
            identity().repo_id
        );
        assert_eq!((scope.chunks.len(), scope.symbols.len()), (1, 2));
        let _digest = batch
            .batch_digest()
            .expect("SDK finalizes source event before transport digest");
    }
    #[test]
    fn source_bound_symbol_only_change_moves_unit_commitment() {
        let first = files(&[("src/lib.rs", RUST_SOURCE)]);
        let second = files(&[("src/lib.rs", "pub fn first() {}\npub fn renamed() {}\n")]);
        let (base, _) = build(
            &first,
            &BTreeMap::new(),
            SymbolCoveragePolicy::RequireComplete,
        )
        .expect("base");
        let (edited, _) = build(
            &second,
            &BTreeMap::new(),
            SymbolCoveragePolicy::RequireComplete,
        )
        .expect("edited");
        assert_ne!(
            base.replace_scopes()
                .first()
                .expect("base")
                .coverage
                .unit_set_sha256,
            edited
                .replace_scopes()
                .first()
                .expect("edited")
                .coverage
                .unit_set_sha256
        );
        let (again, _) = build(
            &first,
            &BTreeMap::new(),
            SymbolCoveragePolicy::RequireComplete,
        )
        .expect("repeat");
        assert_eq!(
            base.batch_digest().expect("base digest"),
            again.batch_digest().expect("same source event")
        );
    }
    #[test]
    fn symbol_only_and_empty_files_both_publish_canonical_coverage() {
        let source = files(&[
            ("src/boot.ts", "export function boot() {}\n"),
            ("src/empty.ts", ""),
        ]);
        let (batch, report) = build(
            &source,
            &BTreeMap::new(),
            SymbolCoveragePolicy::RequireComplete,
        )
        .expect("batch");
        assert_eq!((report.scopes, report.chunks, report.symbols), (2, 0, 1));
        assert_eq!(report.symbol_only_scopes, ["src/boot.ts"]);
        assert_eq!(report.empty_scopes, ["src/empty.ts"]);
        let empty = batch
            .replace_scopes()
            .iter()
            .find(|scope| scope.coverage.source.file.repo_relative_path.as_str() == "src/empty.ts")
            .expect("empty admitted member");
        assert_eq!(
            empty.coverage.symbols,
            SymbolCoverage::Complete { symbol_count: 0 }
        );
        assert!(empty.chunks.is_empty() && empty.symbols.is_empty());
        assert!(empty.coverage.text_admitted);
        let symbol_only = batch
            .replace_scopes()
            .iter()
            .find(|scope| scope.coverage.source.file.repo_relative_path.as_str() == "src/boot.ts")
            .expect("symbol-only source");
        assert!(!symbol_only.coverage.text_admitted);
    }
    #[test]
    fn canonical_unit_commitment_is_independent_of_chunk_vector_order() {
        let source = files(&[("src/lib.rs", RUST_SOURCE)]);
        let first = chunk("src/lib.rs", RUST_SOURCE);
        let mut second = first.clone();
        second.chunk_id = "other-chunk".to_string();
        let a = BTreeMap::from([(
            "src/lib.rs".to_string(),
            vec![first.clone(), second.clone()],
        )]);
        let b = BTreeMap::from([("src/lib.rs".to_string(), vec![second, first])]);
        let (left, _) = build(&source, &a, SymbolCoveragePolicy::RequireComplete).expect("left");
        let (right, _) = build(&source, &b, SymbolCoveragePolicy::RequireComplete).expect("right");
        assert_eq!(
            left.replace_scopes()
                .first()
                .expect("left")
                .coverage
                .unit_set_sha256,
            right
                .replace_scopes()
                .first()
                .expect("right")
                .coverage
                .unit_set_sha256
        );
    }
    #[test]
    fn chunk_id_colliding_with_an_actual_symbol_id_refuses() {
        let source = files(&[("src/lib.rs", RUST_SOURCE)]);
        let preflight = preflight_corpus_symbols(&source, &SymbolPreflightOptions::default())
            .expect("preflight");
        let mut record = chunk("src/lib.rs", RUST_SOURCE);
        record.chunk_id = preflight
            .symbols()
            .get("src/lib.rs")
            .expect("symbols")
            .first()
            .expect("symbol")
            .symbol_id
            .as_str()
            .to_string();
        let chunks = BTreeMap::from([("src/lib.rs".to_string(), vec![record])]);
        let error = assemble_batch(
            &identity(),
            &chunks,
            &source,
            &preflight,
            SymbolCoveragePolicy::RequireComplete,
            event(),
        )
        .expect_err("duplicate unit identity");
        assert!(error.to_string().contains("duplicate source-file unit ID"));
    }
    #[test]
    fn incomplete_profile_keeps_text_and_typed_parse_failure() {
        let malformed = "export function broken( {\n";
        let source = files(&[("broken.ts", malformed)]);
        let chunks =
            BTreeMap::from([("broken.ts".to_string(), vec![chunk("broken.ts", malformed)])]);
        assert!(build(&source, &chunks, SymbolCoveragePolicy::RequireComplete).is_err());
        let (batch, _) = build(&source, &chunks, SymbolCoveragePolicy::AllowIncomplete)
            .expect("explicit text policy");
        let scope = batch
            .replace_scopes()
            .first()
            .expect("failed source remains admitted");
        assert_eq!(scope.coverage.symbols, SymbolCoverage::ParseFailed);
        assert!(scope.symbols.is_empty());
        assert_eq!(
            scope.chunks.first().expect("source text").text.as_ref(),
            malformed
        );
    }
    #[test]
    fn forged_chunk_bytes_or_foreign_source_paths_refuse() {
        let source = files(&[("src/lib.rs", RUST_SOURCE)]);
        let wrong_bytes = BTreeMap::from([(
            "src/lib.rs".to_string(),
            vec![chunk("src/lib.rs", "pub fn forged() {}")],
        )]);
        assert!(build(&source, &wrong_bytes, SymbolCoveragePolicy::RequireComplete).is_err());
        let foreign =
            BTreeMap::from([("other.rs".to_string(), vec![chunk("other.rs", RUST_SOURCE)])]);
        assert!(build(&source, &foreign, SymbolCoveragePolicy::RequireComplete).is_err());
    }

    #[test]
    fn duplicate_unit_ids_across_source_files_refuse() {
        let source = files(&[("a.ts", "function a() {}"), ("b.ts", "function b() {}")]);
        let preflight = preflight_corpus_symbols(&source, &SymbolPreflightOptions::default())
            .expect("preflight");
        let foreign_symbol_id = preflight
            .symbols()
            .get("b.ts")
            .expect("b symbols")
            .first()
            .expect("b definition")
            .symbol_id
            .as_str();
        for collision in ["same-chunk-id", foreign_symbol_id] {
            let mut a = chunk("a.ts", "function a() {}");
            let mut b = chunk("b.ts", "function b() {}");
            a.chunk_id = collision.to_string();
            if collision == "same-chunk-id" {
                b.chunk_id = collision.to_string();
            }
            let chunks =
                BTreeMap::from([("a.ts".to_string(), vec![a]), ("b.ts".to_string(), vec![b])]);
            let error = assemble_batch(
                &identity(),
                &chunks,
                &source,
                &preflight,
                SymbolCoveragePolicy::RequireComplete,
                event(),
            )
            .expect_err("global collision refuses");
            assert!(
                error
                    .to_string()
                    .contains("duplicate published unit ID across source files")
            );
        }
    }
}
