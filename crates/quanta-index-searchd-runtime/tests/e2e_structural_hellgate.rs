#![forbid(unsafe_code)]

use anyhow::{Result as AnyResult, ensure};
use quanta_index_contract::lex::{
    LanguageCode, ParseNode, ParseRoleTag, ParseTreeRecord, SymbolKindCode, SymbolKindFamily,
    SymbolRecord, SymbolRelationship, SymbolSpan, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    BatchIngestMode, CapabilityStatusV1, ChunkId, ChunkRecord, OwnerDocKind,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchScopeKey, SearchScopeSurface,
    SemanticCorpusKindV1, SemanticSourceRecordV1, SemanticSourceReplaceScopeV1,
    SemanticSourceScopeKeyV1, SourceRoleV1, StructuralIngestBatch, StructuralReplaceScope,
    StructuralTreeRecord, SymbolId, TextQuerySyntax,
};
use crate::e2e_harness::E2eRuntime;

fn scope_key(path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        repo_relative_path: quanta_index_contract::RepoRelativePath::new(path),
    }
}

fn semantic_source_scopes(chunks: &[ChunkRecord]) -> Vec<SemanticSourceReplaceScopeV1> {
    chunks
        .iter()
        .map(|chunk| {
            let owner_id = chunk.chunk_id.as_str().to_string();
            SemanticSourceReplaceScopeV1 {
                scope: SemanticSourceScopeKeyV1 {
                    corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                    owner_kind: OwnerDocKind::Chunk,
                    owner_id: owner_id.clone(),
                },
                scope_digest: format!("structural-hellgate:semantic:{owner_id}"),
                sources: vec![SemanticSourceRecordV1 {
                    record_id: owner_id.clone(),
                    corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                    owner_kind: OwnerDocKind::Chunk,
                    owner_id: owner_id.clone(),
                    source_doc_id: owner_id.clone(),
                    parent_owner_id: Some(owner_id),
                    repo_relative_path: chunk.repo_relative_path.clone(),
                    language: Some(chunk.language.as_str().to_string()),
                    package: None,
                    symbol_kind: None,
                    visibility: None,
                    source_role: SourceRoleV1::RawFallbackText,
                    generated: false,
                    capability_status: CapabilityStatusV1::Degraded,
                    raw_fallback_reason: Some(
                        quanta_index_contract::RawFallbackReasonV1::IntentNotRecoverableFromStructure,
                    ),
                    authority_digest: "structural-hellgate:source:v1".to_string(),
                    render_policy_digest: "structural-hellgate:source:v1".to_string(),
                    card_schema_version: 0,
                    text: chunk.text.to_string(),
                }],
                cluster_memberships: Vec::new(),
            }
        })
        .collect()
}

fn function_tree(content: &str, identifier: &str) -> AnyResult<ParseTreeRecord> {
    let identifier_start = content.find(identifier).ok_or_else(|| {
        anyhow::anyhow!("identifier `{identifier}` not present in structural content")
    })?;
    let identifier_end = identifier_start.saturating_add(identifier.len());
    let byte_end = u32::try_from(content.len())
        .map_err(|err| anyhow::anyhow!("structural content overflow: {err}"))?;
    let identifier_start = u32::try_from(identifier_start)
        .map_err(|err| anyhow::anyhow!("identifier start overflow: {err}"))?;
    let identifier_end = u32::try_from(identifier_end)
        .map_err(|err| anyhow::anyhow!("identifier end overflow: {err}"))?;
    let block_start = byte_end.saturating_sub(2);
    Ok(ParseTreeRecord {
        wire_version: 1,
        lang: LanguageCode::new("rust")
            .map_err(|err| anyhow::anyhow!("invalid structural language code: {err}"))?,
        root: ParseNode {
            kind: "function_item".to_string().into_boxed_str(),
            byte_start: 0,
            byte_end,
            children: vec![
                ParseNode {
                    kind: "identifier".to_string().into_boxed_str(),
                    byte_start: identifier_start,
                    byte_end: identifier_end,
                    children: Vec::new(),
                },
                ParseNode {
                    kind: "block".to_string().into_boxed_str(),
                    byte_start: block_start,
                    byte_end,
                    children: Vec::new(),
                },
            ],
        },
        source_hash: compute_parse_tree_source_hash(content),
        role_tag_schema_version: 1,
        role_tags: vec![
            ParseRoleTag {
                role: "item".to_string().into_boxed_str(),
                byte_start: 0,
                byte_end,
            },
            ParseRoleTag {
                role: "expr".to_string().into_boxed_str(),
                byte_start: identifier_start,
                byte_end: identifier_end,
            },
            ParseRoleTag {
                role: "stmt".to_string().into_boxed_str(),
                byte_start: block_start,
                byte_end,
            },
        ],
    })
}

fn boot_structural_file_predicate_fixture() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    let content = "fn main() {}";
    rt.ingest_text("repo-structural-hellgate", "src/lib.rs", content)?;
    rt.ingest_structural_function_tree("src/lib.rs", content, "main")?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt.reopen())
}

fn boot_symbol_projection_fixture() -> AnyResult<E2eRuntime> {
    let mut rt = E2eRuntime::boot()?;
    let path = "src/sym-struct.rs";
    let content = "fn ParityTypeSymbol() {}";
    let chunk_a = ChunkRecord {
        chunk_id: ChunkId::new("sym-struct-a"),
        repo_relative_path: quanta_index_contract::RepoRelativePath::new(path),
        language: LanguageCode::new("rust")
            .map_err(|err| anyhow::anyhow!("invalid symbol fixture language code: {err}"))?,
        start_byte: 0,
        end_byte: u32::try_from(content.len())
            .map_err(|err| anyhow::anyhow!("symbol fixture content overflow: {err}"))?,
        start_line: 1,
        end_line: 1,
        text: content.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    };
    let chunk_b = ChunkRecord {
        chunk_id: ChunkId::new("sym-struct-b"),
        repo_relative_path: quanta_index_contract::RepoRelativePath::new(path),
        language: LanguageCode::new("rust")
            .map_err(|err| anyhow::anyhow!("invalid symbol fixture language code: {err}"))?,
        start_byte: 0,
        end_byte: u32::try_from(content.len())
            .map_err(|err| anyhow::anyhow!("symbol fixture content overflow: {err}"))?,
        start_line: 1,
        end_line: 1,
        text: content.to_string().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    };
    let symbol = SymbolRecord {
        symbol_id: SymbolId::new("sym-struct-needle"),
        repo_relative_path: quanta_index_contract::RepoRelativePath::new(path),
        language: LanguageCode::new("rust")
            .map_err(|err| anyhow::anyhow!("invalid symbol language code: {err}"))?,
        symbol_kind: SymbolKindCode::new("function")
            .map_err(|err| anyhow::anyhow!("invalid symbol kind: {err}"))?,
        symbol_kind_family: Some(SymbolKindFamily::Callable),
        local_name: "ParityTypeSymbol".to_string().into_boxed_str(),
        qualified_name: "crate::ParityTypeSymbol".to_string().into_boxed_str(),
        signature: None,
        visibility: None,
        definition_span: SymbolSpan {
            path: path.to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: u32::try_from("ParityTypeSymbol".len())
                .map_err(|err| anyhow::anyhow!("symbol span overflow: {err}"))?,
            line_start: 1,
            line_end: 1,
        },
        container_qualified_name: Some("crate".to_string().into_boxed_str()),
        relationship: SymbolRelationship::Def,
    };
    rt.publish_search_corpus_batch(SearchCorpusIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        base_generation: None,
        manifest_digest: "structural-symbol-hellgate-lex".to_string(),
        batch_digest: "structural-symbol-hellgate-lex-batch".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: vec![SearchCorpusReplaceScope {
            scope: scope_key(path),
            scope_digest: "structural-symbol-hellgate-scope".to_string(),
            chunks: vec![chunk_a.clone(), chunk_b.clone()],
            symbols: vec![symbol],
        }],
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: semantic_source_scopes(&[chunk_a.clone(), chunk_b.clone()]),
        semantic_tombstone_scopes: Vec::new(),
        seal: false,
    })?;
    let tree = function_tree(content, "ParityTypeSymbol")?;
    rt.publish_structural_batch(StructuralIngestBatch {
        repo_id: rt.repo(),
        revision_id: rt.revision(),
        generation: rt.current_generation(),
        base_generation: None,
        manifest_digest: "structural-symbol-hellgate-struct".to_string(),
        batch_digest: "structural-symbol-hellgate-struct-batch".to_string(),
        mode: BatchIngestMode::Delta,
        replace_scopes: vec![StructuralReplaceScope {
            scope: scope_key(path),
            scope_digest: "structural-symbol-hellgate-struct-scope".to_string(),
            trees: vec![
                StructuralTreeRecord {
                    chunk_id: chunk_a.chunk_id,
                    record: tree.clone(),
                },
                StructuralTreeRecord {
                    chunk_id: chunk_b.chunk_id,
                    record: tree,
                },
            ],
        }],
        tombstone_scopes: Vec::new(),
        seal: false,
    })?;
    let _generation = rt.seal()?;
    rt.activate_last_sealed_generation()?;
    Ok(rt.reopen())
}

#[test]
fn sourcegraph_structural_file_predicate_siblings_execute_in_boolean_scope() -> AnyResult<()> {
    let mut rt = boot_structural_file_predicate_fixture()?;
    for (query, expected_count) in [
        (
            r#"patterntype:structural file:contains(path:src/lib.rs, main) AND "function_item { { identifier :[name] } }""#,
            1_usize,
        ),
        (
            r#"patterntype:structural file:contains(path:src/lib.rs, main) OR "trait_item""#,
            1_usize,
        ),
        (
            r#"patterntype:structural "identifier :[name]" AND NOT file:contains(path:src/lib.rs, main)"#,
            0_usize,
        ),
        (
            r#"patterntype:structural file:has.content(path:src/lib.rs, main) AND "function_item { { identifier :[name] } }""#,
            1_usize,
        ),
        (
            r#"patterntype:structural file:has.content(path:src/lib.rs, main) OR "trait_item""#,
            1_usize,
        ),
        (
            r#"patterntype:structural "identifier :[name]" AND NOT file:has.content(path:src/lib.rs, main)"#,
            0_usize,
        ),
    ] {
        let result = rt.query_structural(TextQuerySyntax::Sourcegraph, query, 10);
        ensure!(
            result.typed_error.is_none(),
            "{query} must not typed-fail: {:?}",
            result.typed_error,
        );
        ensure!(
            result.candidate_ids.len() == expected_count,
            "{query} must return {expected_count} structural candidates, got {:?}",
            result.candidate_ids,
        );
    }

    let miss = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural file:contains(path:src/miss.rs, main) AND "function_item { { identifier :[name] } }""#,
        10,
    );
    ensure!(
        miss.typed_error.is_none() && miss.candidate_ids.is_empty(),
        "path-miss mixed structural file:contains must return empty, got {:?}",
        miss.candidate_ids,
    );

    let content_miss = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural file:has.content(path:src/miss.rs, main) AND "function_item { { identifier :[name] } }""#,
        10,
    );
    ensure!(
        content_miss.typed_error.is_none() && content_miss.candidate_ids.is_empty(),
        "path-miss mixed structural file:has.content must return empty, got {:?}",
        content_miss.candidate_ids,
    );
    Ok(())
}

#[test]
fn sourcegraph_structural_direct_phrase_and_regex_demote_into_structural_bodies() -> AnyResult<()> {
    let mut rt = boot_structural_file_predicate_fixture()?;

    let direct_phrase = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural "function_item { { identifier :[name] } }""#,
        10,
    );
    ensure!(
        direct_phrase.typed_error.is_none(),
        "direct SG structural phrase body must execute, got {:?}",
        direct_phrase.typed_error,
    );
    ensure!(
        direct_phrase.candidate_ids.len() == 1,
        "quoted SG structural body must match exactly one structural candidate, got {:?}",
        direct_phrase.candidate_ids,
    );
    let phrase_binding = direct_phrase
        .structural_results
        .first()
        .and_then(|candidate| candidate.bindings.first())
        .ok_or_else(|| anyhow::anyhow!("direct SG structural phrase body must expose binding"))?;
    ensure!(
        phrase_binding.metavariable == "name"
            && phrase_binding.start_byte == 3
            && phrase_binding.end_byte == 7,
        "quoted SG structural body must bind identifier span, got {phrase_binding:?}",
    );

    let direct_regex = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r"patterntype:structural /^main$/",
        10,
    );
    ensure!(
        direct_regex.typed_error.is_none(),
        "direct SG structural regex body must execute, got {:?}",
        direct_regex.typed_error,
    );
    ensure!(
        direct_regex.candidate_ids.len() == 1,
        "slash-delimited SG structural regex body must execute against one structural candidate, got {:?}",
        direct_regex.candidate_ids,
    );
    let regex_binding = direct_regex
        .structural_results
        .first()
        .and_then(|candidate| candidate.bindings.first())
        .ok_or_else(|| anyhow::anyhow!("direct SG structural regex body must expose binding"))?;
    ensure!(
        regex_binding.metavariable.starts_with("__sg_regex_")
            && regex_binding.start_byte == 3
            && regex_binding.end_byte == 7,
        "slash-delimited SG structural regex body must bind synthetic regex capture over `main`, got {regex_binding:?}",
    );

    let regex_miss = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r"patterntype:structural /^missing$/",
        10,
    );
    ensure!(
        regex_miss.typed_error.is_none() && regex_miss.candidate_ids.is_empty(),
        "non-matching SG structural regex body must miss instead of typed-failing, got {:?}",
        regex_miss.candidate_ids,
    );
    Ok(())
}

#[test]
fn sourcegraph_structural_symbol_projection_ambiguous_chunk_union_is_deterministic() -> AnyResult<()>
{
    let mut rt = boot_symbol_projection_fixture()?;

    let and = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural symbol:has.name(ParityTypeSymbol) AND "function_item { { identifier :[name] } }""#,
        10,
    );
    ensure!(
        and.typed_error.is_none(),
        "symbol.has.name AND must not typed-fail: {:?}",
        and.typed_error,
    );
    ensure!(
        and.candidate_ids == ["sym-struct-a", "sym-struct-b"],
        "symbol.has.name AND must union both ambiguous structural chunks deterministically, got {:?}",
        and.candidate_ids,
    );

    let or = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural symbol:has.name(ParityTypeSymbol) OR "trait_item""#,
        10,
    );
    ensure!(
        or.typed_error.is_none() && or.candidate_ids == and.candidate_ids,
        "symbol.has.name OR must preserve the deterministic ambiguous union, got {:?}",
        or.candidate_ids,
    );

    let and_not = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural "identifier :[name]" AND NOT symbol:has.name(ParityTypeSymbol)"#,
        10,
    );
    ensure!(
        and_not.typed_error.is_none() && and_not.candidate_ids.is_empty(),
        "symbol.has.name AND NOT must subtract the full ambiguous union, got {:?}",
        and_not.candidate_ids,
    );

    let miss = rt.query_structural(
        TextQuerySyntax::Sourcegraph,
        r#"patterntype:structural symbol:has.name(MissingSymbol) AND "function_item { { identifier :[name] } }""#,
        10,
    );
    ensure!(
        miss.typed_error.is_none() && miss.candidate_ids.is_empty(),
        "missing symbol.has.name must return empty, got {:?}",
        miss.candidate_ids,
    );
    Ok(())
}
