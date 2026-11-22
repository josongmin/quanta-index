//! Structural fixtures and query oracles on the pinned Legacy process.

#![forbid(unsafe_code)]

use std::path::Path;
use std::time::Duration;

use anyhow::{Result as AnyResult, ensure};
use quanta_index_contract::lex::{
    LanguageCode, ParseNode, ParseRoleTag, ParseTreeRecord, SymbolKindCode, SymbolKindFamily,
    SymbolRecord, SymbolRelationship, SymbolSpan, compute_parse_tree_source_hash,
};
use quanta_index_contract::{
    BatchIngestMode, CapabilityStatusV1, ChunkId, ChunkRecord, DiagnosticAttemptFieldV1,
    OwnerDocKind, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope,
    SearchPlaneIngestIpcResponse, SearchPlaneIngestIpcResponseEnvelope, SearchScopeKey,
    SearchScopeSurface, SemanticCorpusKindV1, SemanticSourceRecordV1, SemanticSourceReplaceScopeV1,
    SemanticSourceScopeKeyV1, SourceRoleV1, StructuralIngestBatch, StructuralReplaceScope,
    StructuralTreeRecord, SymbolId,
};
use quanta_index_ipc::{ClientIoPolicy, send_request};
use quanta_index_sdk::{PublishedBatchEvidence, QuantaIndex};
use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, daemon_socket_paths};

use crate::fail_closed_wait::{RealTicker, wait_for};

fn scope_key(repo_id: quanta_index_contract::RepoId, path: &str) -> SearchScopeKey {
    SearchScopeKey {
        doc_surface: SearchScopeSurface::Chunk,
        source_file: quanta_index_contract::SourceFileKey {
            source_repo_id: repo_id,
            repo_relative_path: quanta_index_contract::RepoRelativePath::new(path),
        },
    }
}

fn semantic_source_scopes(
    batch_repo_id: &quanta_index_contract::RepoId,
    chunks: &[ChunkRecord],
) -> Vec<SemanticSourceReplaceScopeV1> {
    chunks
        .iter()
        .map(|chunk| {
            let owner_id = chunk.chunk_id.as_str().to_string();
            let source_file = quanta_index_contract::SourceFileKey {
                source_repo_id: chunk.searchable_repo_id(batch_repo_id).clone(),
                repo_relative_path: chunk.repo_relative_path.clone(),
            };
            SemanticSourceReplaceScopeV1 {
                scope: SemanticSourceScopeKeyV1 {
                    corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                    owner_kind: OwnerDocKind::Chunk,
                    owner_id: owner_id.clone(),
                    source_file: source_file.clone(),
                },
                scope_digest: format!("structural-hellgate:semantic:{owner_id}"),
                sources: vec![SemanticSourceRecordV1 {
                    record_id: owner_id.clone(),
                    corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
                    owner_kind: OwnerDocKind::Chunk,
                    owner_id: owner_id.clone(),
                    source_doc_id: owner_id.clone(),
                    parent_owner_id: Some(owner_id),
                    source_file,
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

fn pin() -> AnyResult<quanta_index_contract::GenerationPin> {
    Ok(quanta_index_contract::GenerationPin::new(
        quanta_index_contract::RepoId::new("repo-e2e")?,
        quanta_index_contract::RevisionId::new("rev-e2e")?,
        quanta_index_contract::ManifestGeneration::new(1),
    ))
}

fn chunk(id: &str, path: &str, text: &str, end_line: u32) -> AnyResult<ChunkRecord> {
    Ok(ChunkRecord {
        chunk_id: ChunkId::new(id),
        repo_relative_path: quanta_index_contract::RepoRelativePath::new(path),
        language: LanguageCode::new("rust")
            .map_err(|error| anyhow::anyhow!("fixture language: {error}"))?,
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 1,
        end_line,
        text: text.to_owned().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    })
}

struct StructuralFixture {
    path: &'static str,
    chunks: Vec<ChunkRecord>,
    symbols: Vec<SymbolRecord>,
    semantic_scopes: Vec<SemanticSourceReplaceScopeV1>,
    tree: ParseTreeRecord,
    manifest_digest: &'static str,
    scope_digest: &'static str,
}

fn publish_fixture(client: &QuantaIndex, root: &Path, fixture: StructuralFixture) -> AnyResult<()> {
    let pin = pin()?;
    let source_scope = quanta_index_searchd_harness::fixture_source_scope_v1(
        quanta_index_contract::SourceFileKey {
            source_repo_id: pin.repo_id.clone(),
            repo_relative_path: quanta_index_contract::RepoRelativePath::new(fixture.path),
        },
        pin.revision_id.clone(),
        fixture.chunks,
        fixture.symbols,
    )?;
    let tree_ids = source_scope
        .chunks
        .iter()
        .map(|chunk| StructuralTreeRecord {
            chunk_id: chunk.chunk_id.clone(),
            record: fixture.tree.clone(),
        })
        .collect();
    let mut source = quanta_index_sdk::SearchCorpusBatch::replace_generation(
        pin.repo_id.clone(),
        pin.revision_id.clone(),
        pin.manifest_generation,
        "lex-seal:1",
    )
    .source_event(quanta_index_contract::SourcePublicationEvent {
        stream_id: "e2e-harness-source-v1".into(),
        event_id: "e2e-source-event-1".into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    });
    for semantic in fixture.semantic_scopes {
        source = source.replace_semantic_scope(
            semantic.scope,
            semantic.scope_digest,
            semantic.sources,
            semantic.cluster_memberships,
        );
    }
    source = source.replace_scope(
        source_scope.coverage,
        source_scope.source_bytes,
        source_scope.chunks,
        source_scope.symbols,
    );
    let publisher = client.search_corpus();
    let outcome = publisher.publish_outcome(&source)?;
    ensure!(
        outcome.receipt.applied && outcome.receipt.durable_sequence > 0,
        "structural fixture Source was not durably published"
    );
    // The original fixture publishes an unsealed initial Delta with no base
    // after Source seal and before activation. The SDK builder's Delta
    // requires a base, so send that exact compatibility request on the UDS.
    let payload = quanta_index_searchd_harness::stamped_ingest_request(
        SearchPlaneIngestIpcRequest::PublishStructuralBatch(StructuralIngestBatch {
            repo_id: pin.repo_id.clone(),
            revision_id: pin.revision_id.clone(),
            generation: pin.manifest_generation,
            base_generation: None,
            manifest_digest: fixture.manifest_digest.into(),
            batch_digest: String::new(),
            mode: BatchIngestMode::Delta,
            replace_scopes: vec![StructuralReplaceScope {
                scope: scope_key(pin.repo_id.clone(), fixture.path),
                scope_digest: fixture.scope_digest.into(),
                trees: tree_ids,
            }],
            tombstone_scopes: Vec::new(),
            seal: false,
        }),
    )?;
    let [_, _, ingest_socket] = daemon_socket_paths(root);
    let request_id = 0xe031;
    let response: SearchPlaneIngestIpcResponseEnvelope = send_request(
        &ingest_socket,
        &SearchPlaneIngestIpcRequestEnvelope {
            request_id,
            diagnostic_attempt: DiagnosticAttemptFieldV1::Absent,
            payload,
        },
        ClientIoPolicy::default(),
    )?;
    ensure!(
        response.request_id == request_id,
        "structural publication response ID differs"
    );
    let SearchPlaneIngestIpcResponse::StructuralReceipt(receipt) = response.payload else {
        anyhow::bail!(
            "unexpected structural publication response: {:?}",
            response.payload
        );
    };
    ensure!(
        receipt.applied && receipt.durable_sequence > 0,
        "structural fixture tree was not durably published"
    );

    let activation = publisher.activate_published(&PublishedBatchEvidence::from(&outcome), None)?;
    ensure!(
        activation.active.generation.lexical.manifest_generation == pin.manifest_generation
            && client
                .generations()
                .active_head(pin.repo_id.clone(), pin.revision_id.clone())?
                == Some(activation.active),
        "structural fixture Source did not activate at the pinned generation"
    );
    Ok(())
}

fn publish_file_predicate_fixture(client: &QuantaIndex, root: &Path) -> AnyResult<()> {
    let path = "src/lib.rs";
    let content = "fn main() {}";
    let chunks = vec![chunk("e2e-1-src/lib.rs", path, content, 2)?];
    let semantic_scopes = quanta_index_searchd_harness::semantic_source_scopes_for_chunk_records(
        &pin()?.repo_id,
        &chunks,
    );
    publish_fixture(
        client,
        root,
        StructuralFixture {
            path,
            chunks,
            symbols: Vec::new(),
            semantic_scopes,
            tree: function_tree(content, "main")?,
            manifest_digest: "struct:src/lib.rs:1",
            scope_digest: "struct-scope:src/lib.rs:2",
        },
    )
}

fn publish_symbol_projection_fixture(client: &QuantaIndex, root: &Path) -> AnyResult<()> {
    let path = "src/sym-struct.rs";
    let content = "fn ParityTypeSymbol() {}";
    let symbol = SymbolRecord {
        symbol_id: SymbolId::new("sym-struct-needle"),
        repo_relative_path: quanta_index_contract::RepoRelativePath::new(path),
        language: LanguageCode::new("rust")
            .map_err(|error| anyhow::anyhow!("fixture language: {error}"))?,
        symbol_kind: SymbolKindCode::new("function")
            .map_err(|error| anyhow::anyhow!("fixture symbol kind: {error}"))?,
        symbol_kind_family: Some(SymbolKindFamily::Callable),
        local_name: "ParityTypeSymbol".to_string().into_boxed_str(),
        qualified_name: "crate::ParityTypeSymbol".to_string().into_boxed_str(),
        signature: None,
        visibility: None,
        definition_span: SymbolSpan {
            path: path.to_string().into_boxed_str(),
            byte_start: 0,
            byte_end: u32::try_from("ParityTypeSymbol".len())?,
            line_start: 1,
            line_end: 1,
        },
        container_qualified_name: Some("crate".to_string().into_boxed_str()),
        relationship: SymbolRelationship::Def,
    };
    let chunks = vec![
        chunk("sym-struct-a", path, content, 1)?,
        chunk("sym-struct-b", path, content, 1)?,
    ];
    let semantic_scopes = semantic_source_scopes(&pin()?.repo_id, &chunks);
    publish_fixture(
        client,
        root,
        StructuralFixture {
            path,
            chunks,
            symbols: vec![symbol],
            semantic_scopes,
            tree: function_tree(content, "ParityTypeSymbol")?,
            manifest_digest: "structural-symbol-hellgate-struct",
            scope_digest: "structural-symbol-hellgate-struct-scope",
        },
    )
}

fn structural_query(
    client: &quanta_index_sdk::QuantaIndex,
    query: &str,
) -> AnyResult<quanta_index_contract::SearchPlaneStructuralQueryResponse> {
    let response = client
        .structural()
        .query()
        .sourcegraph(query)
        .pinned(pin()?)
        .top_k(10)
        .execute()?;
    ensure!(
        response.generation == pin()?,
        "structural response pin changed"
    );
    Ok(response)
}

fn with_restarted_fixture(
    publish: fn(&QuantaIndex, &Path) -> AnyResult<()>,
    verify: fn(&QuantaIndex) -> AnyResult<()>,
) -> AnyResult<()> {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()
        .map_err(|error| anyhow::anyhow!("old-Q Structural input: {error}"))?;
    let first = binary
        .start(&state_root, 8)
        .map_err(|error| anyhow::anyhow!("start old-Q Structural process: {error}"))?;
    let outcome = (|| -> AnyResult<()> {
        let client = first
            .connect()
            .map_err(|error| anyhow::anyhow!("connect old-Q Structural process: {error}"))?;
        publish(&client, &state_root)
    })();
    let stopped = first.stop();
    outcome?;
    stopped.map_err(|error| anyhow::anyhow!("stop old-Q Structural process: {error}"))?;
    let restarted = binary
        .start(&state_root, 8)
        .map_err(|error| anyhow::anyhow!("restart old-Q Structural process: {error}"))?;
    let outcome = (|| -> AnyResult<()> {
        let client = restarted.connect().map_err(|error| {
            anyhow::anyhow!("connect restarted old-Q Structural process: {error}")
        })?;
        let _ready = wait_for(
            &RealTicker::new(),
            Duration::from_secs(20),
            Duration::from_millis(50),
            "pinned old-Q Structural backend ready after restart",
            || client.observability().process_readiness(),
            |report| report.ready,
            |_| true,
        )?;
        verify(&client)
    })();
    let stopped = restarted.stop();
    outcome?;
    stopped.map_err(|error| anyhow::anyhow!("stop restarted old-Q Structural process: {error}"))?;
    binary.verify().map_err(|error| anyhow::anyhow!("{error}"))
}

fn verify_file_predicate_boolean_scope(client: &quanta_index_sdk::QuantaIndex) -> AnyResult<()> {
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
        let result = structural_query(client, query)?;
        ensure!(
            result.results.len() == expected_count,
            "{query} must return {expected_count} structural candidates, got {:?}",
            result.results,
        );
    }

    let miss = structural_query(
        client,
        r#"patterntype:structural file:contains(path:src/miss.rs, main) AND "function_item { { identifier :[name] } }""#,
    )?;
    ensure!(
        miss.results.is_empty(),
        "path-miss mixed structural file:contains must return empty, got {:?}",
        miss.results,
    );
    let content_miss = structural_query(
        client,
        r#"patterntype:structural file:has.content(path:src/miss.rs, main) AND "function_item { { identifier :[name] } }""#,
    )?;
    ensure!(
        content_miss.results.is_empty(),
        "path-miss mixed structural file:has.content must return empty, got {:?}",
        content_miss.results,
    );
    Ok(())
}

fn verify_direct_phrase_and_regex_bodies(client: &quanta_index_sdk::QuantaIndex) -> AnyResult<()> {
    let direct_phrase = structural_query(
        client,
        r#"patterntype:structural "function_item { { identifier :[name] } }""#,
    )?;
    ensure!(
        direct_phrase.results.len() == 1,
        "quoted SG structural body must match exactly one structural candidate, got {:?}",
        direct_phrase.results,
    );
    let phrase_binding = direct_phrase
        .results
        .first()
        .and_then(|candidate| candidate.bindings.first())
        .ok_or_else(|| anyhow::anyhow!("direct SG structural phrase body must expose binding"))?;
    ensure!(
        phrase_binding.metavariable == "name"
            && phrase_binding.start_byte == 3
            && phrase_binding.end_byte == 7,
        "quoted SG structural body must bind identifier span, got {phrase_binding:?}",
    );

    let direct_regex = structural_query(client, r"patterntype:structural /^main$/")?;
    ensure!(
        direct_regex.results.len() == 1,
        "slash-delimited SG structural regex body must execute against one structural candidate, got {:?}",
        direct_regex.results,
    );
    let regex_binding = direct_regex
        .results
        .first()
        .and_then(|candidate| candidate.bindings.first())
        .ok_or_else(|| anyhow::anyhow!("direct SG structural regex body must expose binding"))?;
    ensure!(
        regex_binding.metavariable.starts_with("__sg_regex_")
            && regex_binding.start_byte == 3
            && regex_binding.end_byte == 7,
        "slash-delimited SG structural regex body must bind synthetic regex capture over `main`, got {regex_binding:?}",
    );

    let regex_miss = structural_query(client, r"patterntype:structural /^missing$/")?;
    ensure!(
        regex_miss.results.is_empty(),
        "non-matching SG structural regex body must miss instead of typed-failing, got {:?}",
        regex_miss.results,
    );
    Ok(())
}

fn verify_file_fixture(client: &quanta_index_sdk::QuantaIndex) -> AnyResult<()> {
    verify_file_predicate_boolean_scope(client)
        .map_err(|error| anyhow::anyhow!("file_predicates: {error:#}"))?;
    verify_direct_phrase_and_regex_bodies(client)
        .map_err(|error| anyhow::anyhow!("direct_bodies: {error:#}"))?;
    Ok(())
}

#[test]
fn sourcegraph_structural_file_queries_share_one_indexed_fixture() -> AnyResult<()> {
    with_restarted_fixture(publish_file_predicate_fixture, verify_file_fixture)
}

fn verify_symbol_projection(client: &quanta_index_sdk::QuantaIndex) -> AnyResult<()> {
    let and = structural_query(
        client,
        r#"patterntype:structural symbol:has.name(ParityTypeSymbol) AND "function_item { { identifier :[name] } }""#,
    )?;
    let and_ids = and
        .results
        .iter()
        .map(|row| row.candidate_id.as_str())
        .collect::<Vec<_>>();
    ensure!(
        and_ids == ["sym-struct-a", "sym-struct-b"],
        "symbol.has.name AND must union both ambiguous structural chunks deterministically, got {:?}",
        and.results,
    );
    let or = structural_query(
        client,
        r#"patterntype:structural symbol:has.name(ParityTypeSymbol) OR "trait_item""#,
    )?;
    ensure!(
        or.results
            .iter()
            .map(|row| row.candidate_id.as_str())
            .eq(and_ids.iter().copied()),
        "symbol.has.name OR must preserve the deterministic ambiguous union, got {:?}",
        or.results,
    );
    let and_not = structural_query(
        client,
        r#"patterntype:structural "identifier :[name]" AND NOT symbol:has.name(ParityTypeSymbol)"#,
    )?;
    ensure!(
        and_not.results.is_empty(),
        "symbol.has.name AND NOT must subtract the full ambiguous union, got {:?}",
        and_not.results,
    );
    let miss = structural_query(
        client,
        r#"patterntype:structural symbol:has.name(MissingSymbol) AND "function_item { { identifier :[name] } }""#,
    )?;
    ensure!(
        miss.results.is_empty(),
        "missing symbol.has.name must return empty, got {:?}",
        miss.results,
    );
    Ok(())
}

#[test]
fn sourcegraph_structural_symbol_projection_ambiguous_chunk_union_is_deterministic() -> AnyResult<()>
{
    with_restarted_fixture(publish_symbol_projection_fixture, verify_symbol_projection)
}
