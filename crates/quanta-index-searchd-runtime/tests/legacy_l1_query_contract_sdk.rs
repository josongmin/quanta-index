//! CS-ENG-01 public query contract against a pinned external Legacy daemon.
//! The fixture owns a fresh root and the exact executable through shutdown.

use std::collections::BTreeSet;
use std::error::Error;

use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    CandidateCountV1, CapabilityStatusV1, ChunkId, ChunkRecord, EngineTouched,
    ExactRepoRelativePathV1, GenerationPin, HybridQueryRequest, HybridSeedQueryRequest,
    ManifestGeneration, OwnerDocKind, QueryConstraintSetV1, RawFallbackReasonV1, RepoId,
    RepoRelativePath, RevisionId, SearchPlaneErrorCodeV2, SemanticCorpusKindV1,
    SemanticQueryRequest, SemanticQueryResponse, SemanticSourceRecordV1, SemanticSourceScopeKeyV1,
    SourceFileCoverage, SourceFileKey, SourceFileRevision, SourcePublicationEvent, SourceRoleV1,
    SymbolCoverage, SymbolId, SymbolQueryRequest, TextQueryRequest, TextQuerySyntax,
    source_file_unit_set_sha256,
};
use quanta_index_sdk::{QuantaIndex, SdkError, SearchCorpusBatch};
use quanta_index_searchd_harness::legacy_reference::{LegacySearchdBinary, SearchdBinaryProcess};
use quanta_index_searchd_harness::private_tempdir;

type TestResult = Result<(), Box<dyn Error>>;
const LITERAL_PATH: &str = "src/a*)' \"literal file.rs";

fn text(pin: &GenerationPin, query: &str, syntax: TextQuerySyntax) -> TextQueryRequest {
    TextQueryRequest {
        syntax,
        query_text: query.into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: Some(pin.clone()),
        generation_selector: None,
        top_k: 10,
        cursor: None,
    }
}

fn symbol(request: TextQueryRequest) -> SymbolQueryRequest {
    SymbolQueryRequest {
        syntax: request.syntax,
        query_text: request.query_text,
        constraints: request.constraints,
        generation: request.generation,
        generation_selector: request.generation_selector,
        top_k: request.top_k,
        cursor: request.cursor,
    }
}

fn remote<T: std::fmt::Debug>(
    result: Result<T, SdkError>,
    expected: SearchPlaneErrorCodeV2,
) -> TestResult {
    match result {
        Err(SdkError::Remote { code, .. }) if code == expected => Ok(()),
        other => Err(format!("expected remote {expected:?}, observed {other:?}").into()),
    }
}

fn remote_for_text<T: std::fmt::Debug>(
    result: Result<T, SdkError>,
    expected: SearchPlaneErrorCodeV2,
    route: &str,
    request: &TextQueryRequest,
) -> TestResult {
    remote(result, expected).map_err(|error| {
        format!(
            "{route} {:?} query {:?} with {:?}: {error}",
            request.syntax, request.query_text, request.constraints
        )
        .into()
    })
}

fn require_semantic_lanes(
    response: &SemanticQueryResponse,
    expected: &[(&str, bool, bool)],
) -> TestResult {
    let observed = response
        .window
        .coverage()
        .lanes()
        .iter()
        .map(|lane| (lane.lane(), lane.executed(), lane.contributed()))
        .collect::<Vec<_>>();
    if observed != expected {
        return Err(format!("semantic window lanes {observed:?} != {expected:?}").into());
    }
    Ok(())
}

fn corpus(pin: &GenerationPin) -> Result<SearchCorpusBatch, Box<dyn Error>> {
    let mut batch = SearchCorpusBatch::replace_generation(
        pin.repo_id.clone(),
        pin.revision_id.clone(),
        pin.manifest_generation,
        "l1-query-contract",
    )
    .source_event(SourcePublicationEvent {
        stream_id: "l1-query-contract".into(),
        event_id: "initial".into(),
        expected_base_event_id: None,
        payload_sha256: [0; 32],
    });
    for (index, path) in ["src/item0.rs", "src/item1.rs", "src/item2.rs", LITERAL_PATH]
        .into_iter()
        .enumerate()
    {
        let chunk_id = format!("chunk-{index}");
        let language = LanguageCode::new("rust").map_err(str::to_string)?;
        let chunks = vec![ChunkRecord {
            chunk_id: ChunkId::new(chunk_id.clone()),
            repo_relative_path: RepoRelativePath::new(path),
            language: language.clone(),
            start_byte: 0,
            end_byte: 14,
            start_line: 1,
            end_line: 1,
            text: "fn needle() {}".into(),
            structural: None,
            parent_chunk_id: None,
            source_repo_id: None,
        }];
        let semantic_scope = SemanticSourceScopeKeyV1 {
            corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
            owner_kind: OwnerDocKind::Chunk,
            owner_id: chunk_id.clone(),
            source_file: SourceFileKey {
                source_repo_id: pin.repo_id.clone(),
                repo_relative_path: RepoRelativePath::new(path),
            },
        };
        let semantic_source = SemanticSourceRecordV1 {
            record_id: chunk_id.clone(),
            corpus_kind: SemanticCorpusKindV1::RawCodeFallback,
            owner_kind: OwnerDocKind::Chunk,
            owner_id: chunk_id.clone(),
            source_doc_id: chunk_id.clone(),
            parent_owner_id: Some(chunk_id),
            source_file: semantic_scope.source_file.clone(),
            language: Some(language.as_str().to_string()),
            package: None,
            symbol_kind: None,
            visibility: None,
            source_role: SourceRoleV1::RawFallbackText,
            generated: false,
            capability_status: CapabilityStatusV1::Degraded,
            raw_fallback_reason: Some(RawFallbackReasonV1::IntentNotRecoverableFromStructure),
            authority_digest: "l1-query-contract:direct-text-source:v1".into(),
            render_policy_digest: "l1-query-contract:direct-text-source:v1".into(),
            card_schema_version: 0,
            text: chunks
                .first()
                .ok_or("semantic chunk fixture is empty")?
                .text
                .to_string(),
        };
        let symbols = vec![SymbolRecord {
            symbol_id: SymbolId::new(format!("symbol-{index}")),
            repo_relative_path: RepoRelativePath::new(path),
            language: language.clone(),
            symbol_kind: SymbolKindCode::new("function").map_err(str::to_string)?,
            symbol_kind_family: None,
            local_name: "needle".into(),
            qualified_name: "needle".into(),
            signature: None,
            visibility: None,
            definition_span: SymbolSpan {
                path: path.into(),
                byte_start: 0,
                byte_end: 14,
                line_start: 1,
                line_end: 1,
            },
            container_qualified_name: None,
            relationship: SymbolRelationship::Def,
        }];
        batch = batch.replace_scope(
            SourceFileCoverage {
                source: SourceFileRevision {
                    file: SourceFileKey {
                        source_repo_id: pin.repo_id.clone(),
                        repo_relative_path: RepoRelativePath::new(path),
                    },
                    revision_id: pin.revision_id.clone(),
                    // Fixed SHA-256 of the literal source above, independent of query execution.
                    source_sha256: [
                        21, 83, 55, 63, 254, 189, 205, 78, 236, 17, 93, 109, 224, 252, 133, 48, 83,
                        142, 184, 81, 141, 228, 183, 111, 109, 93, 181, 104, 192, 232, 245, 85,
                    ],
                },
                language,
                producer_policy_sha256: [0x11; 32],
                symbol_name_source_policy:
                    quanta_index_contract::SymbolNameSourcePolicyV1::Unspecified,
                unit_set_sha256: source_file_unit_set_sha256(&chunks, &symbols)?,
                text_admitted: true,
                symbols: SymbolCoverage::Complete { symbol_count: 1 },
            },
            b"fn needle() {}".to_vec(),
            chunks,
            symbols,
        );
        batch = batch.replace_semantic_scope(
            semantic_scope,
            format!("l1-query-contract:semantic-source:scope:{index}"),
            vec![semantic_source],
            Vec::new(),
        );
    }
    Ok(batch)
}

#[test]
fn l1_real_daemon_sdk_domain_primitives_and_symbol_windows() -> TestResult {
    let parent = private_tempdir()?;
    let state_root = parent.path().join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let command = binary.benchmark_command(&state_root, 8)?;
    let process = SearchdBinaryProcess::start_with_pinned_command(&state_root, command)?;
    let outcome = (|| -> TestResult {
        let client = process.connect()?;
        run_l1_contract(&client)
    })();
    let stopped = process.stop();
    outcome.and(stopped)?;
    binary.verify()
}

fn verify_constraint_only_exact_path(client: &QuantaIndex, pin: &GenerationPin) -> TestResult {
    let literal_constraint = QueryConstraintSetV1::from_exact_repo_relative_path(
        ExactRepoRelativePathV1::new(LITERAL_PATH)?,
    );
    let mut empty = text(pin, "", TextQuerySyntax::Native);
    empty.constraints = literal_constraint;
    let selected_chunk = client.lexical().query_request(empty.clone())?;
    if selected_chunk.results.len() != 1 || selected_chunk.results[0].candidate_id != "chunk-3" {
        return Err(
            format!("empty typed exact path lost literal Chunk: {selected_chunk:?}").into(),
        );
    }
    let selected_symbol = client.symbol().query_request(symbol(empty))?;
    if selected_symbol.results.len() != 1 || selected_symbol.results[0].candidate_id != "symbol-3" {
        return Err(
            format!("empty typed exact path lost literal Symbol: {selected_symbol:?}").into(),
        );
    }
    let unscoped_empty = text(pin, "", TextQuerySyntax::Native);
    remote_for_text(
        client.lexical().query_request(unscoped_empty.clone()),
        SearchPlaneErrorCodeV2::InvalidRequest,
        "Lexical",
        &unscoped_empty,
    )?;
    remote_for_text(
        client
            .symbol()
            .query_request(symbol(unscoped_empty.clone())),
        SearchPlaneErrorCodeV2::InvalidRequest,
        "Symbol",
        &unscoped_empty,
    )?;
    Ok(())
}

fn run_l1_contract(client: &QuantaIndex) -> TestResult {
    let pin = GenerationPin::new(
        RepoId::new("l1-public")?,
        RevisionId::new("v1")?,
        ManifestGeneration::new(1),
    );
    if client
        .generations()
        .active_head(pin.repo_id.clone(), pin.revision_id.clone())?
        .is_some()
    {
        return Err("L1 process proof requires an unpublished fixture namespace".into());
    }
    let _published = client
        .search_corpus()
        .publish_and_activate(&corpus(&pin)?, None)?;
    verify_constraint_only_exact_path(client, &pin)?;
    let expected_ids = BTreeSet::from([
        "symbol-0".to_string(),
        "symbol-1".to_string(),
        "symbol-2".to_string(),
        "symbol-3".to_string(),
    ]);
    for (syntax, repo_content, file_content) in [
        (
            TextQuerySyntax::Native,
            r#"repo.has.content("!!!")"#,
            r#"file.has.content("!!!")"#,
        ),
        (
            TextQuerySyntax::Sourcegraph,
            r#"repo:has.content("!!!")"#,
            r#"file:has.content("!!!")"#,
        ),
    ] {
        let manual_scope = client.lexical().query_request(text(
            &pin,
            "index:no file:^src/item0.rs$ needle",
            syntax,
        ))?;
        if manual_scope.results.len() != 1
            || manual_scope
                .results
                .first()
                .is_none_or(|row| row.candidate_id != "chunk-0")
        {
            return Err(format!("manual scope lost its source row: {manual_scope:?}").into());
        }
        let file_type = client
            .lexical()
            .query_request(text(&pin, "type:file needle", syntax))?;
        let file_ids = file_type
            .results
            .iter()
            .map(|row| row.candidate_id.as_str())
            .collect::<BTreeSet<_>>();
        if file_type.results.len() != 4
            || file_ids != BTreeSet::from(["chunk-0", "chunk-1", "chunk-2", "chunk-3"])
        {
            return Err(format!("type:file lost source chunks: {file_type:?}").into());
        }

        // Every selected path must survive top_k=1, regardless of the
        // unconstrained rank leader. The fourth path also exercises quote and
        // DSL metacharacters as an opaque typed value.
        for (path, chunk_id, symbol_id) in [
            ("src/item0.rs", "chunk-0", "symbol-0"),
            ("src/item1.rs", "chunk-1", "symbol-1"),
            ("src/item2.rs", "chunk-2", "symbol-2"),
            (LITERAL_PATH, "chunk-3", "symbol-3"),
        ] {
            let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
                ExactRepoRelativePathV1::new(path)?,
            );
            for prefix in ["", "index:no "] {
                let mut request = text(&pin, &format!("{prefix}needle"), syntax);
                request.constraints = constraints.clone();
                request.top_k = 1;
                let response = client.lexical().query_request(request)?;
                if response.results.len() != 1 || response.results[0].candidate_id != chunk_id {
                    return Err(format!(
                        "typed exact path {path:?} with {prefix:?} missed {chunk_id}: {response:?}"
                    )
                    .into());
                }
            }
            let mut request = text(&pin, "needle", syntax);
            request.constraints = constraints;
            request.top_k = 1;
            let response = client.symbol().query_request(symbol(request))?;
            if response.results.len() != 1 || response.results[0].candidate_id != symbol_id {
                return Err(format!(
                    "typed Symbol exact path {path:?} missed {symbol_id}: {response:?}"
                )
                .into());
            }
        }
        for manual in ["", "index:no "] {
            for sensitive in ["", "case:yes "] {
                for projection in ["select:file", "select:path", "select:content", "type:file"] {
                    remote(
                        client.symbol().query_request(symbol(text(
                            &pin,
                            &format!("{manual}{sensitive}{projection} needle"),
                            syntax,
                        ))),
                        SearchPlaneErrorCodeV2::InvalidRequest,
                    )?;
                }
            }
            let mut request = symbol(text(&pin, &format!("{manual}count:1 needle"), syntax));
            let mut seen = BTreeSet::new();
            for page in 0..4 {
                let response = client.symbol().query_request(request.clone())?;
                if response.results.len() != 1
                    || response.window.candidate_count() != CandidateCountV1::Exact(4 - page)
                    || response.window.has_more() != Some(page < 3)
                {
                    return Err(format!("invalid bounded Symbol window: {response:?}").into());
                }
                for row in response.results {
                    if !seen.insert(row.candidate_id) {
                        return Err("duplicate Symbol page row".into());
                    }
                }
                if page == 0 {
                    let mut wrong = request.clone();
                    wrong.query_text.push_str(" case:yes");
                    wrong.cursor = response.next_cursor.clone();
                    remote(
                        client.symbol().query_request(wrong),
                        SearchPlaneErrorCodeV2::CursorContextMismatch,
                    )?;
                }
                if (page < 3) != response.next_cursor.is_some() {
                    return Err("incorrect Symbol continuation".into());
                }
                request.cursor = response.next_cursor;
            }
            if seen != expected_ids {
                return Err(format!("Symbol page membership: {seen:?}").into());
            }

            let rust = QueryConstraintSetV1::from_languages([
                LanguageCode::new("rust").map_err(str::to_string)?
            ]);
            for constraints in [QueryConstraintSetV1::unconstrained(), rust.clone()] {
                for (primitive, expected) in [
                    ("...", SearchPlaneErrorCodeV2::LexTextQueryNoTokens),
                    ("\"!!!\"", SearchPlaneErrorCodeV2::LexTextQueryNoTokens),
                    ("content:!!!", SearchPlaneErrorCodeV2::LexTextQueryNoTokens),
                    (repo_content, SearchPlaneErrorCodeV2::LexTextQueryNoTokens),
                    (file_content, SearchPlaneErrorCodeV2::LexTextQueryNoTokens),
                    (
                        "patterntype:regexp [",
                        SearchPlaneErrorCodeV2::LexRegexDialectParseError,
                    ),
                ] {
                    let mut request =
                        text(&pin, &format!("{manual}lang:python {primitive}"), syntax);
                    request.constraints = constraints.clone();
                    let response = client.lexical().query_request(request.clone());
                    remote_for_text(response, expected, "Lexical", &request)?;
                }
                let mut request = text(&pin, &format!("{manual}lang:python ..."), syntax);
                request.constraints = constraints;
                remote_for_text(
                    client.symbol().query_request(symbol(request.clone())),
                    SearchPlaneErrorCodeV2::LexTextQueryNoTokens,
                    "Symbol",
                    &request,
                )?;
            }
            let control = client.search().hybrid_request(HybridQueryRequest {
                text_query: text(&pin, &format!("{manual}needle"), syntax),
                semantic_query_text: "needle".into(),
                generation: Some(pin.clone()),
                generation_selector: None,
                top_k: 10,
            })?;
            let hybrid_candidate = control
                .results
                .into_iter()
                .next()
                .ok_or("positive hybrid query returned no row")?;
            for (constraints, primitive) in [QueryConstraintSetV1::unconstrained(), rust.clone()]
                .iter()
                .flat_map(|constraints| {
                    ["...", "\"!!!\"", "content:!!!"].map(|primitive| (constraints, primitive))
                })
            {
                let mut request = text(&pin, &format!("{manual}lang:python {primitive}"), syntax);
                request.constraints = constraints.clone();
                let expected = SearchPlaneErrorCodeV2::LexTextQueryNoTokens;
                remote_for_text(
                    client.search().hybrid_request(HybridQueryRequest {
                        text_query: request.clone(),
                        semantic_query_text: "needle".into(),
                        generation: Some(pin.clone()),
                        generation_selector: None,
                        top_k: 10,
                    }),
                    expected,
                    "Hybrid",
                    &request,
                )?;
                remote_for_text(
                    client.search().hybrid_seed_request(HybridSeedQueryRequest {
                        text_query: request.clone(),
                        semantic_query_text: "needle".into(),
                        generation: Some(pin.clone()),
                        generation_selector: None,
                        top_k: 10,
                        dense_corpora: Vec::new(),
                    }),
                    expected,
                    "Hybrid seed",
                    &request,
                )?;
                remote_for_text(
                    client.semantic().query_request(SemanticQueryRequest {
                        query_text: "needle".into(),
                        constraints: constraints.clone(),
                        generation: Some(pin.clone()),
                        generation_selector: None,
                        lexical_scope: Some(request.clone()),
                        top_k: 10,
                    }),
                    expected,
                    "Semantic lexical scope",
                    &request,
                )?;
                remote_for_text(
                    client.search().explain_hybrid_under_queries(
                        pin.clone(),
                        hybrid_candidate.clone(),
                        request.clone(),
                        "needle",
                    ),
                    expected,
                    "Hybrid explanation",
                    &request,
                )?;
            }
            let mut request = text(&pin, &format!("{manual}lang:python needle"), syntax);
            request.constraints = rust.clone();
            let response = client.lexical().query_request(request.clone())?;
            if !response.results.is_empty()
                || !response.explanation.engines_executed.is_empty()
                || response.window.candidate_count() != CandidateCountV1::Exact(0)
            {
                return Err(
                    format!("valid contradiction lost logical-empty truth: {response:?}").into(),
                );
            }
            let semantic = client.semantic().query_request(SemanticQueryRequest {
                query_text: "needle".into(),
                constraints: rust.clone(),
                generation: Some(pin.clone()),
                generation_selector: None,
                lexical_scope: Some(request.clone()),
                top_k: 10,
            })?;
            if !semantic.results.is_empty()
                || !semantic.explanation.engines_executed.is_empty()
                || semantic.window.empty_provenance()
                    != Some(quanta_index_contract::EmptyProvenanceV2::LogicalEmpty)
            {
                return Err(format!(
                    "contradictory semantic scope searched instead of proving empty: {semantic:?}"
                )
                .into());
            }
            require_semantic_lanes(
                &semantic,
                &[
                    ("semantic.lexical", false, false),
                    ("semantic.dense", false, false),
                ],
            )?;
            let zero_hit_scope = text(&pin, &format!("{manual}zzzznomatch"), syntax);
            let semantic = client.semantic().query_request(SemanticQueryRequest {
                query_text: "needle".into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                lexical_scope: Some(zero_hit_scope),
                top_k: 10,
            })?;
            if !semantic.results.is_empty()
                || semantic.explanation.engines_executed != vec![EngineTouched::Lexical]
                || semantic.window.empty_provenance()
                    != Some(quanta_index_contract::EmptyProvenanceV2::ZeroHitExecuted)
            {
                return Err(format!(
                    "zero-hit semantic scope lost lexical-only execution truth: {semantic:?}"
                )
                .into());
            }
            require_semantic_lanes(
                &semantic,
                &[
                    ("semantic.lexical", true, false),
                    ("semantic.dense", false, false),
                ],
            )?;
            let live_scope = text(&pin, &format!("{manual}needle"), syntax);
            let lexical = client.lexical().query_request(live_scope.clone())?;
            let allowed = lexical
                .results
                .iter()
                .map(|candidate| candidate.candidate_id.as_str())
                .collect::<BTreeSet<_>>();
            let semantic = client.semantic().query_request(SemanticQueryRequest {
                query_text: "needle".into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: Some(pin.clone()),
                generation_selector: None,
                lexical_scope: Some(live_scope),
                top_k: 10,
            })?;
            if allowed.is_empty()
                || semantic.results.is_empty()
                || semantic.explanation.engines_executed
                    != vec![EngineTouched::Lexical, EngineTouched::Semantic]
                || semantic
                    .results
                    .iter()
                    .any(|candidate| !allowed.contains(candidate.candidate_id.as_str()))
            {
                return Err(format!(
                    "live semantic scope escaped lexical allowlist: lexical={lexical:?} semantic={semantic:?}"
                )
                .into());
            }
            require_semantic_lanes(
                &semantic,
                &[
                    ("semantic.lexical", true, true),
                    ("semantic.dense", true, true),
                ],
            )?;
            if semantic
                .window
                .coverage()
                .lanes()
                .first()
                .map(quanta_index_contract::LaneTraceV1::candidates)
                != Some(CandidateCountV1::Exact(u64::try_from(allowed.len())?))
            {
                return Err(format!("live lexical scope count is false: {semantic:?}").into());
            }
            remote(
                client.semantic().query_request(SemanticQueryRequest {
                    query_text: "!!!".into(),
                    constraints: rust,
                    generation: Some(pin.clone()),
                    generation_selector: None,
                    lexical_scope: Some(request),
                    top_k: 10,
                }),
                SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::EmptyQuery,
                ),
            )?;
        }
    }
    Ok(())
}
