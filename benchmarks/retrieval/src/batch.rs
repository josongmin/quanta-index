//! `SearchCorpusBatch` assembly from chunker output (RB-02, RBR-04).
//!
//! One `replace_generation` batch: a lexical `File` scope per file carrying
//! **both** the file's chunk records and its source-bound symbol records in
//! a single combined replacement (RBR-04: no second replacement for the
//! same path), plus one `RawCodeFallback` semantic scope per chunk. The
//! scope digest binds the symbol payload and producer identity, so a
//! symbol-only change still changes the digest.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_contract::lex::{LanguageCode, SymbolRecord};
use quanta_index_contract::{
    CapabilityStatusV1, ChunkId, ChunkRecord, ManifestGeneration, OwnerDocKind,
    RawFallbackReasonV1, RepoId, RepoRelativePath, RevisionId,
    SearchPlaneSearchCorpusActivationCasAck, SearchScopeKey, SearchScopeSurface,
    SemanticCorpusKindV1, SemanticSourceRecordV1, SemanticSourceReplaceScopeV1,
    SemanticSourceScopeKeyV1, SourceRoleV1,
};
use quanta_index_sdk::{BatchReceipt, SearchCorpusBatch};

use crate::chunking::Chunk;
use crate::symbols::{SYMBOL_PRODUCER_GRAMMARS, SYMBOL_PRODUCER_IDENTITY};
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
    let code = match path.rsplit('.').next().unwrap_or("") {
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
        other => {
            return Err(BenchError::Config(format!(
                "no language mapping for admitted file (exclude it from the manifest): {path} (.{other})"
            )));
        }
    };
    LanguageCode::new(code).map_err(|err| BenchError::Config(format!("bad language code: {err}")))
}

/// Canonical scope digest over ordered chunk and symbol descriptors. The
/// symbol producer identity participates, and any symbol name/span change
/// changes the digest (RBR-04).
fn scope_digest(chunks: &[Chunk], symbols: &[SymbolRecord]) -> String {
    let mut raw = Vec::new();
    raw.extend_from_slice(SYMBOL_PRODUCER_IDENTITY.as_bytes());
    raw.push(0);
    // Grammar identity participates: bumping a pinned tree-sitter crate
    // changes every scope digest and invalidates frozen evidence.
    raw.extend_from_slice(SYMBOL_PRODUCER_GRAMMARS.as_bytes());
    raw.push(0);
    // Order canonicalization: descriptors hash in sorted id order so the
    // digest does not depend on caller slice order (audit finding).
    let mut chunks: Vec<&Chunk> = chunks.iter().collect();
    chunks.sort_by(|left, right| left.chunk_id.cmp(&right.chunk_id));
    let mut symbols: Vec<&SymbolRecord> = symbols.iter().collect();
    symbols.sort_by(|left, right| left.symbol_id.as_str().cmp(right.symbol_id.as_str()));
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
    for symbol in symbols {
        for part in [
            symbol.symbol_id.as_str(),
            symbol.symbol_kind.as_str(),
            symbol.local_name.as_ref(),
            symbol.qualified_name.as_ref(),
            symbol.container_qualified_name.as_deref().unwrap_or(""),
            &symbol.definition_span.byte_start.to_string(),
            &symbol.definition_span.byte_end.to_string(),
            &symbol.definition_span.line_start.to_string(),
            &symbol.definition_span.line_end.to_string(),
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

/// Assemble the publishable batch.
///
/// Every file with content is published in exactly one combined replacement
/// carrying its chunks **and** its symbols (RBR-04); a file with neither
/// chunks nor symbols is reported in `skipped_empty`, and a symbol-only file
/// still gets its scope.
pub fn assemble_batch(
    identity: &BatchIdentity,
    chunks: &BTreeMap<String, Vec<Chunk>>,
    symbols: &BTreeMap<String, Vec<SymbolRecord>>,
) -> BenchResult<(SearchCorpusBatch, BatchAssemblyReport)> {
    let mut batch = SearchCorpusBatch::replace_generation(
        identity.repo_id.clone(),
        identity.revision_id.clone(),
        identity.generation,
        identity.manifest_digest.clone(),
    );
    let mut report = BatchAssemblyReport::default();
    let mut paths: BTreeSet<&String> = chunks.keys().collect();
    paths.extend(symbols.keys());
    for path in paths {
        let file_chunks: &[Chunk] = chunks.get(path).map_or(&[], Vec::as_slice);
        let file_symbols: &[SymbolRecord] = symbols.get(path).map_or(&[], Vec::as_slice);
        if file_chunks.is_empty() && file_symbols.is_empty() {
            report.skipped_empty.push(path.clone());
            continue;
        }
        // Chunk ids and symbol ids live in distinct typed namespaces; the
        // wire string comparison is the explicit collision guard (RBR-04).
        for symbol in file_symbols {
            if file_chunks
                .iter()
                .any(|chunk| chunk.chunk_id == symbol.symbol_id.as_str())
            {
                return Err(BenchError::Protocol(format!(
                    "symbol id collides with a chunk id in {path}: {}",
                    symbol.symbol_id.as_str()
                )));
            }
        }
        let digest = scope_digest(file_chunks, file_symbols);
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
            file_symbols.to_vec(),
        );
        report.scopes = report
            .scopes
            .checked_add(1)
            .ok_or_else(|| BenchError::Protocol("lexical scope count overflow".to_string()))?;
        report.chunks = report
            .chunks
            .checked_add(file_chunks.len())
            .ok_or_else(|| BenchError::Protocol("chunk count overflow".to_string()))?;
        report.symbols = report
            .symbols
            .checked_add(file_symbols.len())
            .ok_or_else(|| BenchError::Protocol("symbol count overflow".to_string()))?;
        if file_chunks.is_empty() {
            report.symbol_only_scopes.push(path.clone());
        }
        for chunk in file_chunks {
            let scope = semantic_scope(chunk)?;
            let scope_digest = scope.scope_digest.clone();
            // Retrieval benchmark chunks do not author ClusterCard membership
            // evidence. Keep that authority explicitly empty instead of
            // synthesizing memberships from semantic source proximity.
            batch =
                batch.replace_semantic_scope(scope.scope, scope_digest, scope.sources, Vec::new());
            report.semantic_scopes = report
                .semantic_scopes
                .checked_add(1)
                .ok_or_else(|| BenchError::Protocol("semantic scope count overflow".to_string()))?;
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
    pub symbols: usize,
    pub semantic_scopes: usize,
    pub skipped_empty: Vec<String>,
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
    use crate::chunking::Chunk;
    use crate::symbols::extract_symbols;

    fn identity() -> BatchIdentity {
        BatchIdentity::new("bench-repo", "bench-rev", 3, "manifest:test".to_string())
            .expect("identity")
    }

    fn chunk(path: &str, text: &str) -> Chunk {
        Chunk {
            chunk_id: format!("chunk-{path}-0"),
            path: path.to_string(),
            start_byte: 0,
            end_byte: u32::try_from(text.len()).expect("fixture length"),
            start_line: 1,
            end_line: 1,
            text: text.to_string(),
            strategy: "whole_file".to_string(),
            version: "v1".to_string(),
            config: "{}".to_string(),
            fallback: false,
        }
    }

    const RUST_SOURCE: &str = "pub fn first() {}\npub fn second() {}\n";

    #[test]
    fn combined_replacement_carries_chunks_and_symbols_together() {
        let identity = identity();
        let chunks = BTreeMap::from([(
            "src/lib.rs".to_string(),
            vec![chunk("src/lib.rs", RUST_SOURCE)],
        )]);
        let symbols = BTreeMap::from([(
            "src/lib.rs".to_string(),
            extract_symbols("src/lib.rs", RUST_SOURCE).expect("symbols"),
        )]);
        let (batch, report) = assemble_batch(&identity, &chunks, &symbols).expect("batch");
        assert_eq!(report.scopes, 1);
        assert_eq!(report.chunks, 1);
        assert_eq!(report.symbols, 2);
        assert!(report.symbol_only_scopes.is_empty());
        let scopes = batch.replace_scopes();
        assert_eq!(scopes.len(), 1, "one combined replacement per file");
        let scope = scopes.first().expect("combined replacement");
        assert_eq!(scope.chunks.len(), 1);
        assert_eq!(scope.symbols.len(), 2);
    }

    #[test]
    fn symbol_only_change_moves_the_scope_digest() {
        let identity = identity();
        let chunks = BTreeMap::from([(
            "src/lib.rs".to_string(),
            vec![chunk("src/lib.rs", RUST_SOURCE)],
        )]);
        let symbols = BTreeMap::from([(
            "src/lib.rs".to_string(),
            extract_symbols("src/lib.rs", RUST_SOURCE).expect("symbols"),
        )]);
        let edited_source = "pub fn first() {}\npub fn renamed() {}\n";
        let edited_symbols = BTreeMap::from([(
            "src/lib.rs".to_string(),
            extract_symbols("src/lib.rs", edited_source).expect("symbols"),
        )]);
        let (base, _) = assemble_batch(&identity, &chunks, &symbols).expect("batch");
        let (edited, _) = assemble_batch(&identity, &chunks, &edited_symbols).expect("batch");
        // Identical chunk bytes, changed symbol payload: different digest.
        let base_digest = &base
            .replace_scopes()
            .first()
            .expect("base scope")
            .scope_digest;
        let edited_digest = &edited
            .replace_scopes()
            .first()
            .expect("edited scope")
            .scope_digest;
        assert_ne!(base_digest, edited_digest);
        // Order-independence: same inputs rebuild the same digest.
        let (again, _) = assemble_batch(&identity, &chunks, &symbols).expect("batch");
        let again_digest = &again
            .replace_scopes()
            .first()
            .expect("rebuilt scope")
            .scope_digest;
        assert_eq!(base_digest, again_digest);
    }

    #[test]
    fn symbol_only_files_publish_their_scope_and_empty_files_skip() {
        let identity = identity();
        let header_source = "export function boot() {}\n";
        let symbols = BTreeMap::from([(
            "src/boot.ts".to_string(),
            extract_symbols("src/boot.ts", header_source).expect("symbols"),
        )]);
        let empty_chunks = BTreeMap::from([("docs/empty.md".to_string(), Vec::<Chunk>::new())]);
        let (batch, report) = assemble_batch(&identity, &empty_chunks, &symbols).expect("batch");
        assert_eq!(report.scopes, 1);
        assert_eq!(report.chunks, 0);
        assert_eq!(report.symbols, 1);
        assert_eq!(report.symbol_only_scopes, vec!["src/boot.ts".to_string()]);
        assert_eq!(report.skipped_empty, vec!["docs/empty.md".to_string()]);
        let scope = batch.replace_scopes().first().expect("symbol-only scope");
        assert_eq!(scope.scope.repo_relative_path.as_str(), "src/boot.ts");
        assert!(scope.chunks.is_empty());
        assert_eq!(scope.symbols.len(), 1);
    }

    #[test]
    fn scope_digest_is_order_independent_across_shuffled_inputs() {
        // Audit finding: the digest must canonicalize order itself; the
        // previous "order-independence" check rebuilt identical slices.
        let identity = identity();
        let source_a = "pub fn one() {}\nstruct Alpha;\n";
        let source_b = "export function two() {}\n";
        let mut chunks = BTreeMap::new();
        let _previous = chunks.insert(
            "src/a.rs".to_string(),
            vec![chunk("src/a.rs", source_a), {
                let mut second = chunk("src/a.rs", source_a);
                second.chunk_id = "chunk-src/a.rs-1".to_string();
                second.start_byte = 0;
                second
            }],
        );
        let mut symbols = BTreeMap::new();
        let _previous = symbols.insert(
            "src/a.rs".to_string(),
            extract_symbols("src/a.rs", source_a).expect("symbols"),
        );
        let _previous = symbols.insert(
            "ui/b.ts".to_string(),
            extract_symbols("ui/b.ts", source_b).expect("symbols"),
        );
        let (base, _) = assemble_batch(&identity, &chunks, &symbols).expect("batch");
        // Shuffle both maps' slices: reversed per-file orders must digest
        // identically because scope_digest sorts by id.
        let mut shuffled_chunks = BTreeMap::new();
        for (path, mut file_chunks) in chunks {
            file_chunks.reverse();
            let _previous = shuffled_chunks.insert(path, file_chunks);
        }
        let mut shuffled_symbols = BTreeMap::new();
        for (path, mut file_symbols) in symbols {
            file_symbols.reverse();
            let _previous = shuffled_symbols.insert(path, file_symbols);
        }
        let (shuffled, _) =
            assemble_batch(&identity, &shuffled_chunks, &shuffled_symbols).expect("batch");
        let base_digest = &base
            .replace_scopes()
            .first()
            .expect("base scope")
            .scope_digest;
        let shuffled_digest = &shuffled
            .replace_scopes()
            .first()
            .expect("shuffled scope")
            .scope_digest;
        assert_eq!(base_digest, shuffled_digest);
    }

    #[test]
    fn symbol_id_colliding_with_a_chunk_id_refuses() {
        let identity = identity();
        let chunks = BTreeMap::from([(
            "src/lib.rs".to_string(),
            vec![chunk("src/lib.rs", RUST_SOURCE)],
        )]);
        let mut records = extract_symbols("src/lib.rs", RUST_SOURCE).expect("symbols");
        // Forge a collision: symbol id equal to the chunk id string.
        records.first_mut().expect("at least one symbol").symbol_id =
            quanta_index_contract::SymbolId::new("chunk-src/lib.rs-0");
        let symbols = BTreeMap::from([("src/lib.rs".to_string(), records)]);
        let error = assemble_batch(&identity, &chunks, &symbols).unwrap_err();
        assert!(error.to_string().contains("collides with a chunk id"));
    }
}
