//! L1 endpoint/domain regressions and independent pagination controls.
//!
//! The fixture owns the expected IDs and cardinality. A full production search
//! is never used to discover the expected result set. Counts are checked against
//! that fixture universe independently from the capped candidate rows.

#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt::Debug;

use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolKindFamily, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LexicalCursor, LqCase, LqCountBound,
    LqExpr, LqFilter, LqLeaf, LqOptions, LqPredicateArg, LqQuery, LqSelect, LqSpan, LqType,
    LqYesNoOnly, ManifestGeneration, QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SearchPlaneErrorCodeV2, SearchScopeSurface,
    SourceFileCoverage, SourceFileKey, SourceFileRevision, SourcePublicationEvent, SymbolCoverage,
    SymbolId, source_event_payload_sha256, source_file_unit_set_sha256,
};
use quanta_index_core::{
    CoreError, LexicalIndexOpenPort, LexicalPageSpec, LexicalSearcher, RequestBudgetV1,
    SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;
use sha2::{Digest as _, Sha256};

type TestResult = Result<(), Box<dyn Error>>;
const MATCHES: u32 = 5;

fn generation() -> ManifestGeneration {
    ManifestGeneration::new(1)
}

fn scope(
    index: u32,
    matches: u32,
    surface: SearchScopeSurface,
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let path = format!("src/file_{index:02}.rs");
    let language = LanguageCode::new("rust").map_err(str::to_string)?;
    let name = if index < matches {
        "needle"
    } else {
        "unrelated"
    };
    let mut chunks = vec![ChunkRecord {
        chunk_id: ChunkId::new(format!("chunk-{index:02}")),
        repo_relative_path: RepoRelativePath::new(path.clone()),
        language: language.clone(),
        start_byte: 0,
        end_byte: u32::try_from(name.len())?,
        start_line: 1,
        end_line: 1,
        text: name.into(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: None,
    }];
    let mut symbols = vec![SymbolRecord {
        symbol_id: SymbolId::new(format!("symbol-{index:02}")),
        repo_relative_path: RepoRelativePath::new(path.clone()),
        language: language.clone(),
        symbol_kind: SymbolKindCode::new("function").map_err(str::to_string)?,
        symbol_kind_family: Some(SymbolKindFamily::Callable),
        local_name: name.into(),
        qualified_name: format!("crate::{name}").into(),
        signature: None,
        visibility: None,
        definition_span: SymbolSpan {
            path: path.clone().into(),
            byte_start: 0,
            byte_end: u32::try_from(name.len())?,
            line_start: 1,
            line_end: 1,
        },
        container_qualified_name: None,
        relationship: SymbolRelationship::Def,
    }];
    match surface {
        SearchScopeSurface::Chunk => symbols.clear(),
        SearchScopeSurface::Symbol => chunks.clear(),
        SearchScopeSurface::File => {}
        SearchScopeSurface::Module => {
            return Err("L1 fixture models canonical files, not module scopes".into());
        }
    }
    Ok(SearchCorpusReplaceScope {
        coverage: SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("l1-domain-window")?,
                    repo_relative_path: RepoRelativePath::new(path),
                },
                revision_id: RevisionId::new("revision-one")?,
                source_sha256: Sha256::digest(name.as_bytes()).into(),
            },
            language,
            producer_policy_sha256: [3; 32],
            unit_set_sha256: source_file_unit_set_sha256(&chunks, &symbols)?,
            text_admitted: surface != SearchScopeSurface::Symbol,
            symbols: SymbolCoverage::Complete {
                symbol_count: u64::try_from(symbols.len())?,
            },
        },
        chunks,
        symbols,
    })
}

fn fixture() -> Result<(tempfile::TempDir, Box<dyn LexicalSearcher>), Box<dyn Error>> {
    fixture_with_matches(MATCHES)
}

fn fixture_with_matches(
    matches: u32,
) -> Result<(tempfile::TempDir, Box<dyn LexicalSearcher>), Box<dyn Error>> {
    fixture_with_surface(matches, SearchScopeSurface::File)
}

fn fixture_with_surface(
    matches: u32,
    surface: SearchScopeSurface,
) -> Result<(tempfile::TempDir, Box<dyn LexicalSearcher>), Box<dyn Error>> {
    fixture_with_scopes(
        (0..=matches)
            .rev()
            .map(|index| scope(index, matches, surface))
            .collect::<Result<_, _>>()?,
    )
}

fn fixture_with_scopes(
    replace_scopes: Vec<SearchCorpusReplaceScope>,
) -> Result<(tempfile::TempDir, Box<dyn LexicalSearcher>), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let repo = RepoId::new("l1-domain-window")?;
    let revision = RevisionId::new("revision-one")?;
    let mut batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "l1-fixture".into(),
            event_id: "first".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        generation: generation(),
        base_generation: None,
        manifest_digest: "l1-manifest-one".into(),
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
    batch.source_event.payload_sha256 = source_event_payload_sha256(&batch)?;
    adapter.build_batch(&batch)?;
    let searcher = adapter.open(&repo, &revision, generation())?;
    Ok((dir, searcher))
}

fn query(term: &str, manual: bool, sensitive: bool) -> LqQuery {
    let mut options = LqOptions::defaults();
    if manual {
        options.index_mode = Some(LqYesNoOnly::No);
    }
    if sensitive {
        options.case = Some(LqCase::Sensitive);
    }
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Keyword(term.into())),
        filters: Vec::new(),
        options,
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

#[test]
fn l1_manual_scope_admission_preserves_supported_regex_grammar() -> TestResult {
    let (_dir, searcher) = fixture()?;
    for pattern in [r"^src/file_00\.rs$", r"\bsrc/file_00\.rs\b"] {
        let mut request = query("needle", true, false);
        request.filters.push(LqFilter::File {
            pattern: pattern.into(),
            scope: quanta_index_contract::LqFileScope::PathOnly,
        });
        let rows = searcher.search(&request, 10, &RequestBudgetV1::unbounded())?;
        if rows.len() != 1
            || rows
                .first()
                .is_none_or(|row| row.repo_relative_path.as_str() != "src/file_00.rs")
        {
            return Err(format!("manual scope {pattern:?} lost its fixture row: {rows:?}").into());
        }
    }
    for manual in [false, true] {
        let mut request = query("needle", manual, false);
        request.filters.push(LqFilter::File {
            pattern: "[".into(),
            scope: quanta_index_contract::LqFileScope::PathOnly,
        });
        require_code(
            searcher.search(&request, 10, &RequestBudgetV1::unbounded()),
            SearchPlaneErrorCodeV2::InvalidRequest,
            "malformed scope regex",
        )?;
    }
    Ok(())
}

fn require_code<T: Debug>(
    outcome: Result<T, CoreError>,
    expected: SearchPlaneErrorCodeV2,
    context: &str,
) -> TestResult {
    match outcome {
        Err(error) => {
            let (code, message) = error.into_search_plane_wire();
            if code == expected {
                Ok(())
            } else {
                Err(format!("{context}: expected {expected}, got {code}: {message}").into())
            }
        }
        Ok(rows) => Err(format!("{context}: invalid request succeeded: {rows:?}").into()),
    }
}

fn require_symbol_refusal(
    searcher: &dyn LexicalSearcher,
    query: &LqQuery,
    expected: SearchPlaneErrorCodeV2,
) -> TestResult {
    let budget = RequestBudgetV1::unbounded();
    require_code(
        searcher.search_symbols(query, 8, &budget),
        expected,
        "symbol",
    )?;
    require_code(
        searcher.search_symbols_constrained(
            query,
            &QueryConstraintSetV1::unconstrained(),
            &LexicalPageSpec::first(8),
            &budget,
        ),
        expected,
        "constrained symbol",
    )?;
    require_code(
        searcher.search_symbols_all(query, &budget),
        expected,
        "all symbols",
    )
}

#[test]
fn symbol_endpoint_rejects_text_projections_with_and_without_hits() -> TestResult {
    for surface in [
        SearchScopeSurface::Chunk,
        SearchScopeSurface::Symbol,
        SearchScopeSurface::File,
    ] {
        let (_dir, searcher) = fixture_with_surface(MATCHES, surface)?;
        for manual in [false, true] {
            for sensitive in [false, true] {
                for term in ["needle", "absent_term"] {
                    for select in [
                        LqSelect::File,
                        LqSelect::FileOwners,
                        LqSelect::Path,
                        LqSelect::Content,
                        LqSelect::ContentMatch,
                        LqSelect::Repo,
                    ] {
                        let mut request = query(term, manual, sensitive);
                        request.filters.push(LqFilter::Select { dim: select });
                        require_symbol_refusal(
                            searcher.as_ref(),
                            &request,
                            SearchPlaneErrorCodeV2::InvalidRequest,
                        )?;
                        request.filters.push(LqFilter::Type {
                            kind: LqType::Symbol,
                        });
                        require_symbol_refusal(
                            searcher.as_ref(),
                            &request,
                            SearchPlaneErrorCodeV2::InvalidRequest,
                        )?;
                    }
                    for kind in [LqType::File, LqType::Path, LqType::Repo] {
                        let mut request = query(term, manual, sensitive);
                        request.filters.push(LqFilter::Type { kind });
                        require_symbol_refusal(
                            searcher.as_ref(),
                            &request,
                            SearchPlaneErrorCodeV2::InvalidRequest,
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

#[test]
fn projection_cannot_reroute_unsupported_symbol_regex_to_text() -> TestResult {
    let (_dir, searcher) = fixture()?;
    for manual in [false, true] {
        let mut request = query("needle", manual, false);
        request.expr = LqExpr::Leaf(LqLeaf::Regex("needle".into()));
        require_symbol_refusal(
            searcher.as_ref(),
            &request,
            SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
        )?;
        request.filters.push(LqFilter::Select {
            dim: LqSelect::File,
        });
        require_symbol_refusal(
            searcher.as_ref(),
            &request,
            SearchPlaneErrorCodeV2::InvalidRequest,
        )?;
    }
    Ok(())
}

#[test]
fn empty_repo_predicate_cannot_hide_invalid_count() -> TestResult {
    let (_dir, searcher) = fixture()?;
    let budget = RequestBudgetV1::unbounded();
    for manual in [false, true] {
        for predicate_term in ["needle", "absent_term"] {
            let mut request = query("needle", manual, false);
            request.expr = LqExpr::All(vec![
                request.expr,
                LqExpr::Leaf(LqLeaf::Predicate {
                    name: "repo.has.content".into(),
                    args: vec![LqPredicateArg::Keyword(predicate_term.into())],
                }),
            ]);
            request.options.count = Some(LqCountBound::Bounded(0));
            require_symbol_refusal(
                searcher.as_ref(),
                &request,
                SearchPlaneErrorCodeV2::LexFilterInvalidCount,
            )?;
            require_code(
                searcher.search(&request, 8, &budget),
                SearchPlaneErrorCodeV2::LexFilterInvalidCount,
                "text count:0 with repo predicate",
            )?;
        }
    }
    Ok(())
}

#[test]
fn invalid_fetch_cannot_mint_zero_exact_count_before_execution() -> TestResult {
    fn require_internal_fetch_refusal<T: Debug>(outcome: Result<T, CoreError>) -> TestResult {
        match outcome {
            Err(CoreError::InvalidContract(message))
                if message.starts_with(quanta_index_contract::INTERNAL_FETCH_OUT_OF_RANGE_CODE) =>
            {
                Ok(())
            }
            other => {
                Err(format!("expected the internal-fetch contract refusal, got {other:?}").into())
            }
        }
    }

    let (_dir, searcher) = fixture()?;
    let budget = RequestBudgetV1::unbounded();
    for manual in [false, true] {
        for term in ["needle", "absent_term"] {
            let mut request = query(term, manual, false);
            request.options.count = Some(LqCountBound::All);
            for fetch in [0, u32::MAX] {
                let page = LexicalPageSpec::first(fetch);
                require_internal_fetch_refusal(searcher.search_constrained(
                    &request,
                    &QueryConstraintSetV1::unconstrained(),
                    &page,
                    &budget,
                ))?;
                require_internal_fetch_refusal(searcher.search_symbols_constrained(
                    &request,
                    &QueryConstraintSetV1::unconstrained(),
                    &page,
                    &budget,
                ))?;
            }
        }
    }
    Ok(())
}

#[test]
fn invalid_predicate_arguments_keep_registry_code_before_empty_results() -> TestResult {
    let (_dir, searcher) = fixture()?;
    for manual in [false, true] {
        for gate_term in ["needle", "absent_term"] {
            let mut request = query("needle", manual, false);
            request.expr = LqExpr::All(vec![
                LqExpr::Leaf(LqLeaf::Predicate {
                    name: "repo.has.content".into(),
                    args: vec![LqPredicateArg::Keyword(gate_term.into())],
                }),
                LqExpr::Leaf(LqLeaf::Predicate {
                    name: "file.contains".into(),
                    args: vec![
                        LqPredicateArg::Phrase("needle".into()),
                        LqPredicateArg::Phrase("extra scalar".into()),
                    ],
                }),
            ]);
            require_code(
                searcher.search(&request, 8, &RequestBudgetV1::unbounded()),
                SearchPlaneErrorCodeV2::LexPredicateUnimplemented,
                "invalid predicate arity before repo gate",
            )?;
        }
    }
    Ok(())
}

#[test]
fn invalid_regex_keeps_dialect_code_before_empty_results() -> TestResult {
    let (_dir, searcher) = fixture()?;
    for manual in [false, true] {
        for gate_term in ["needle", "absent_term"] {
            for (pattern, expected) in [
                ("[", SearchPlaneErrorCodeV2::LexRegexDialectParseError),
                (
                    "(?<=x)y",
                    SearchPlaneErrorCodeV2::LexRegexDialectUnsupported,
                ),
            ] {
                let mut request = query("needle", manual, false);
                request.expr = LqExpr::All(vec![
                    LqExpr::Leaf(LqLeaf::Predicate {
                        name: "repo.has.content".into(),
                        args: vec![LqPredicateArg::Keyword(gate_term.into())],
                    }),
                    LqExpr::Leaf(LqLeaf::Regex(pattern.into())),
                ]);
                require_code(
                    searcher.search(&request, 8, &RequestBudgetV1::unbounded()),
                    expected,
                    "regex dialect refusal before repo gate",
                )?;
                require_code(
                    searcher.search_all(&request, &RequestBudgetV1::unbounded()),
                    expected,
                    "regex dialect refusal in exact-all path",
                )?;
            }
        }
    }
    Ok(())
}

#[test]
fn predicate_result_empty_cannot_hide_tokenless_phrase() -> TestResult {
    let (_dir, searcher) = fixture()?;
    let budget = RequestBudgetV1::unbounded();
    for manual in [false, true] {
        for predicate_term in ["needle", "absent_term"] {
            let mut request = query("needle", manual, false);
            request.expr = LqExpr::All(vec![
                LqExpr::Leaf(LqLeaf::Predicate {
                    name: "repo.has.content".into(),
                    args: vec![LqPredicateArg::Keyword(predicate_term.into())],
                }),
                LqExpr::Leaf(LqLeaf::Phrase("!!!".into())),
            ]);
            require_code(
                searcher.search(&request, 8, &budget),
                SearchPlaneErrorCodeV2::LexTextQueryNoTokens,
                "tokenless phrase following repo predicate",
            )?;
            require_code(
                searcher.search_all(&request, &budget),
                SearchPlaneErrorCodeV2::LexTextQueryNoTokens,
                "tokenless phrase in exact-all path",
            )?;
        }
    }
    Ok(())
}

#[test]
fn predicate_result_empty_cannot_hide_keyword_or_content_tokens() -> TestResult {
    let (_dir, searcher) = fixture()?;
    let mut failures = Vec::new();
    for manual in [false, true] {
        for content_filter in [false, true] {
            for gate_term in ["needle", "absent_term"] {
                let mut request = query("needle", manual, false);
                let gate = LqExpr::Leaf(LqLeaf::Predicate {
                    name: "repo.has.content".into(),
                    args: vec![LqPredicateArg::Keyword(gate_term.into())],
                });
                let invalid = LqLeaf::Keyword("!!!".into());
                if content_filter {
                    request.expr = gate;
                    request.filters.push(LqFilter::Content { leaf: invalid });
                } else {
                    request.expr = LqExpr::All(vec![gate, LqExpr::Leaf(invalid)]);
                }
                if let Err(error) = require_code(
                    searcher.search(&request, 8, &RequestBudgetV1::unbounded()),
                    SearchPlaneErrorCodeV2::LexTextQueryNoTokens,
                    &format!("manual={manual}, content_filter={content_filter}, gate={gate_term}"),
                ) {
                    failures.push(error.to_string());
                }
                if let Err(error) = require_code(
                    searcher.search_all(&request, &RequestBudgetV1::unbounded()),
                    SearchPlaneErrorCodeV2::LexTextQueryNoTokens,
                    &format!(
                        "exact-all manual={manual}, content_filter={content_filter}, gate={gate_term}"
                    ),
                ) {
                    failures.push(error.to_string());
                }
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n").into())
    }
}

#[test]
fn l1_primitive_admission_is_independent_of_boolean_order_and_leaf_location() -> TestResult {
    let (_dir, searcher) = fixture()?;
    for manual in [false, true] {
        for location in 0..5 {
            for leaf in [
                LqLeaf::Keyword("!!!".into()),
                LqLeaf::Phrase("!!!".into()),
                LqLeaf::Predicate {
                    name: "repo.has.content".into(),
                    args: vec![LqPredicateArg::Keyword("!!!".into())],
                },
                LqLeaf::Predicate {
                    name: "file.has.content".into(),
                    args: vec![LqPredicateArg::Phrase("!!!".into())],
                },
                LqLeaf::Predicate {
                    name: "repo.has.file".into(),
                    args: vec![LqPredicateArg::Filter {
                        name: "content".into(),
                        value: "!!!".into(),
                    }],
                },
            ] {
                let mut request = query("needle", manual, false);
                let gate = LqExpr::Leaf(LqLeaf::Predicate {
                    name: "repo.has.content".into(),
                    args: vec![LqPredicateArg::Keyword("absent_term".into())],
                });
                let invalid = LqExpr::Leaf(leaf.clone());
                request.expr = match location {
                    0 => LqExpr::All(vec![gate, invalid]),
                    1 => LqExpr::All(vec![invalid, gate]),
                    2 => LqExpr::Any(vec![gate, invalid]),
                    3 => LqExpr::All(vec![gate, LqExpr::Not(Box::new(invalid))]),
                    _ => {
                        request.filters.push(LqFilter::Content { leaf });
                        gate
                    }
                };
                let budget = RequestBudgetV1::unbounded();
                require_code(
                    searcher.search(&request, 8, &budget),
                    SearchPlaneErrorCodeV2::LexTextQueryNoTokens,
                    "every primitive is admitted",
                )?;
                require_code(
                    searcher.search_all(&request, &budget),
                    SearchPlaneErrorCodeV2::LexTextQueryNoTokens,
                    "exact-all primitive admission",
                )?;
                require_code(
                    searcher.explain_candidate(
                        &request,
                        &QueryConstraintSetV1::unconstrained(),
                        "chunk-00",
                        &budget,
                    ),
                    SearchPlaneErrorCodeV2::LexTextQueryNoTokens,
                    "explain primitive admission",
                )?;
            }
        }
    }
    Ok(())
}

#[test]
fn l1_opener_primitive_admission_never_opens_a_generation() -> TestResult {
    use quanta_index_contract::LqPatternType;
    use quanta_index_core::{LexicalEndpoint, LexicalPolicy};
    let dir = tempfile::tempdir()?;
    let absent_root = dir.path().join("must-remain-absent");
    let adapter = LexicalAdapter::with_state_root(absent_root.clone());
    let budget = RequestBudgetV1::unbounded();
    for manual in [false, true] {
        for leaf in [
            LqLeaf::Keyword("[".into()),
            LqLeaf::RawString("[".into()),
            LqLeaf::Regex("[".into()),
        ] {
            for in_filter in [false, true] {
                let mut request = query("needle", manual, false);
                request.options.pattern_type = LqPatternType::Regexp;
                if in_filter {
                    request
                        .filters
                        .push(LqFilter::Content { leaf: leaf.clone() });
                } else {
                    request.expr = LqExpr::Leaf(leaf.clone());
                }
                let plan = LexicalPolicy::plan_query(
                    &request,
                    &QueryConstraintSetV1::unconstrained(),
                    LexicalEndpoint::Text,
                )?;
                require_code(
                    adapter.preflight_query_primitives(&plan, &budget),
                    SearchPlaneErrorCodeV2::LexRegexDialectParseError,
                    "effective regex interpretation before open",
                )?;
            }
        }
        for leaf in [
            LqLeaf::Keyword("needle".into()),
            LqLeaf::Phrase("needle value".into()),
            LqLeaf::RawString("!!!".into()),
        ] {
            let mut request = query("needle", manual, false);
            request.expr = LqExpr::Leaf(leaf);
            // Producer availability is not a pure-input failure.
            request.filters.push(LqFilter::Fork {
                mode: LqYesNoOnly::No,
            });
            let plan = LexicalPolicy::plan_query(
                &request,
                &QueryConstraintSetV1::unconstrained(),
                LexicalEndpoint::Text,
            )?;
            adapter.preflight_query_primitives(&plan, &budget)?;
        }
    }
    if absent_root.exists() {
        return Err("pure admission created adapter state".into());
    }
    Ok(())
}

#[test]
fn l1_primitive_admission_uses_adapter_regex_policy_and_literal_limits() -> TestResult {
    use quanta_index_core::{LexicalEndpoint, LexicalPolicy};
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root_and_policies(
        dir.path().join("not-opened"),
        quanta_index_lexical::regex::RegexPolicy {
            require_literal: true,
            ..quanta_index_lexical::regex::RegexPolicy::defaults()
        },
        quanta_index_core::LexicalExecutionBudgetV1::DEFAULT,
        quanta_index_core::RegexMatchCachePolicy::DEFAULT,
        quanta_index_core::LexicalWriterPolicy::DEFAULT,
    );
    for (leaf, expected) in [
        (
            LqLeaf::Regex(".*".into()),
            SearchPlaneErrorCodeV2::LexRegexDialectUnsupported,
        ),
        (
            LqLeaf::Keyword("a".repeat(100_000)),
            SearchPlaneErrorCodeV2::LexTextQueryTokenTooLong,
        ),
    ] {
        let mut request = query("needle", false, false);
        request.expr = LqExpr::Leaf(leaf);
        let plan = LexicalPolicy::plan_query(
            &request,
            &QueryConstraintSetV1::unconstrained(),
            LexicalEndpoint::Text,
        )?;
        require_code(
            adapter.preflight_query_primitives(&plan, &RequestBudgetV1::unbounded()),
            expected,
            "adapter policy and canonical literal limits",
        )?;
    }
    Ok(())
}

#[test]
fn symbol_as_text_explanation_keeps_domain_when_a_predicate_is_empty() -> TestResult {
    let (_dir, searcher) = fixture()?;
    let budget = RequestBudgetV1::unbounded();
    for manual in [false, true] {
        let mut request = query("needle", manual, false);
        request.filters.push(LqFilter::Type {
            kind: LqType::Symbol,
        });
        request.expr = LqExpr::All(vec![
            request.expr,
            LqExpr::Leaf(LqLeaf::Predicate {
                name: "repo.has.content".into(),
                args: vec![LqPredicateArg::Keyword("absent_term".into())],
            }),
        ]);
        let explanation = searcher.explain_candidate(
            &request,
            &QueryConstraintSetV1::unconstrained(),
            "symbol-00",
            &budget,
        )?;
        if !matches!(
            explanation,
            quanta_index_core::LexicalCandidateExplanationV1::NotMatched { .. }
        ) {
            return Err(
                format!("empty Symbol plan lost candidate presence: {explanation:?}").into(),
            );
        }
    }
    Ok(())
}

#[test]
fn generic_text_symbol_selection_preserves_fixture_membership() -> TestResult {
    let (_dir, searcher) = fixture()?;
    let budget = RequestBudgetV1::unbounded();
    let expected: Vec<String> = (0..MATCHES).map(|id| format!("symbol-{id:02}")).collect();
    for manual in [false, true] {
        for filter in [
            LqFilter::Type {
                kind: LqType::Symbol,
            },
            LqFilter::Select {
                dim: LqSelect::Symbol,
            },
        ] {
            let mut request = query("needle", manual, false);
            request.filters.push(filter);
            let ids: Vec<_> = searcher
                .search(&request, 8, &budget)?
                .into_iter()
                .map(|row| row.candidate_id)
                .collect();
            if ids != expected {
                return Err(format!("generic symbol selection: {ids:?} != {expected:?}").into());
            }
        }
    }
    Ok(())
}

#[test]
fn symbol_count_cap_preserves_rows_when_explicitly_walking_boundaries() -> TestResult {
    for matches in [0, 1, 3, 5, 6] {
        let (_dir, searcher) = fixture_with_matches(matches)?;
        let budget = RequestBudgetV1::unbounded();
        let expected: Vec<String> = (0..matches).map(|id| format!("symbol-{id:02}")).collect();
        for manual in [false, true] {
            for count in [
                None,
                Some(LqCountBound::Bounded(1)),
                Some(LqCountBound::Bounded(3)),
                Some(LqCountBound::Bounded(5)),
                Some(LqCountBound::All),
            ] {
                for fetch in [1, 3, 5, 8] {
                    let mut request = query("needle", manual, false);
                    request.options.count = count;
                    let mut after = None;
                    let mut seen = Vec::new();
                    // This control deliberately does not infer exhaustion from a short
                    // page: the existing Vec-only port cannot provide that evidence.
                    for _ in 0..=matches {
                        let page = searcher.search_symbols_constrained(
                            &request,
                            &QueryConstraintSetV1::unconstrained(),
                            &LexicalPageSpec {
                                fetch,
                                after: after.clone(),
                            },
                            &budget,
                        )?;
                        let remaining = u64::from(matches)
                            .checked_sub(u64::try_from(seen.len())?)
                            .ok_or("walk exceeded fixture cardinality")?;
                        if count.is_some() && page.exact_total != Some(remaining) {
                            return Err(format!(
                                "symbol count facts: {:?} != {remaining}",
                                page.exact_total
                            )
                            .into());
                        }
                        let rows = page.candidates;
                        let cap = match count {
                            Some(LqCountBound::Bounded(bound)) => fetch.min(bound),
                            Some(LqCountBound::All) | None => fetch,
                        };
                        if rows.len() > usize::try_from(cap)? {
                            return Err("symbol page exceeded its requested cap".into());
                        }
                        let Some(last) = rows.last() else { break };
                        after = Some(LexicalCursor::at(generation(), last.order_key()));
                        seen.extend(rows.into_iter().map(|row| row.candidate_id));
                    }
                    if seen != expected {
                        return Err(format!(
                        "manual={manual}, count={count:?}, fetch={fetch}: {seen:?} != {expected:?}"
                    )
                    .into());
                    }
                }
            }
        }
    }
    Ok(())
}

#[test]
fn exact_all_ports_refuse_bounded_count_and_preserve_full_set_controls() -> TestResult {
    let (_dir, searcher) = fixture()?;
    let budget = RequestBudgetV1::unbounded();
    for manual in [false, true] {
        for count in [None, Some(LqCountBound::All)] {
            let mut request = query("needle", manual, false);
            request.options.count = count;
            let symbols: Vec<_> = searcher
                .search_symbols_all(&request, &budget)?
                .into_iter()
                .map(|row| row.candidate_id)
                .collect();
            let text: Vec<_> = searcher
                .search_all(&request, &budget)?
                .into_iter()
                .map(|row| row.candidate_id)
                .collect();
            let expected_symbols: Vec<_> =
                (0..MATCHES).map(|id| format!("symbol-{id:02}")).collect();
            let expected_text: Vec<_> = (0..MATCHES).map(|id| format!("chunk-{id:02}")).collect();
            if symbols != expected_symbols || text != expected_text {
                return Err(format!("exact-all membership drift: {symbols:?}, {text:?}").into());
            }
        }
        for bound in [1, 3, 5] {
            for term in ["needle", "absent_term"] {
                let mut request = query(term, manual, false);
                request.options.count = Some(LqCountBound::Bounded(bound));
                require_code(
                    searcher.search_symbols_all(&request, &budget),
                    SearchPlaneErrorCodeV2::LexFilterInvalidCount,
                    "symbol exact-all cap",
                )?;
                require_code(
                    searcher.search_all(&request, &budget),
                    SearchPlaneErrorCodeV2::LexFilterInvalidCount,
                    "text exact-all cap",
                )?;
            }
        }
    }
    Ok(())
}

#[test]
fn l1_audit_exact_all_symbol_name_uses_the_normal_query_rewrite() -> TestResult {
    let (_dir, searcher) = fixture()?;
    let expected: Vec<_> = (0..MATCHES)
        .map(|index| format!("symbol-{index:02}"))
        .collect();
    let mut failures = Vec::new();
    for manual in [false, true] {
        for count in [None, Some(LqCountBound::All)] {
            for explicit_symbol in [false, true] {
                let mut request = query("needle", manual, false);
                request.expr = LqExpr::Leaf(LqLeaf::Predicate {
                    name: "symbol.has.name".into(),
                    args: vec![LqPredicateArg::Keyword("needle".into())],
                });
                request.options.count = count;
                if explicit_symbol {
                    request.filters.push(LqFilter::Type {
                        kind: LqType::Symbol,
                    });
                }
                let ordinary =
                    searcher.search(&request, MATCHES + 1, &RequestBudgetV1::unbounded())?;
                let ordinary_ids: Vec<_> = ordinary.iter().map(|row| &row.candidate_id).collect();
                if ordinary_ids != expected.iter().collect::<Vec<_>>() {
                    return Err(format!(
                        "ordinary Symbol control: {ordinary_ids:?} != {expected:?}"
                    )
                    .into());
                }
                match searcher.search_all(&request, &RequestBudgetV1::unbounded()) {
                    Ok(rows) if rows.iter().map(|row| &row.candidate_id).collect::<Vec<_>>() == expected.iter().collect::<Vec<_>>() => {}
                    actual => failures.push(format!("manual={manual} count={count:?} explicit_symbol={explicit_symbol}: {actual:?}")),
                }
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n").into())
    }
}

#[test]
fn l1_audit_exact_all_repo_projection_retains_each_source() -> TestResult {
    let mut scopes = Vec::new();
    for (index, owner) in [(2, "source-b"), (1, "source-a"), (0, "source-a")] {
        let mut replacement = scope(index, MATCHES, SearchScopeSurface::File)?;
        let source = RepoId::new(owner)?;
        replacement.coverage.source.file.source_repo_id = source.clone();
        for chunk in &mut replacement.chunks {
            chunk.source_repo_id = Some(source.clone());
        }
        replacement.coverage.unit_set_sha256 =
            source_file_unit_set_sha256(&replacement.chunks, &replacement.symbols)?;
        scopes.push(replacement);
    }
    let (_dir, searcher) = fixture_with_scopes(scopes)?;
    let mut failures = Vec::new();
    for manual in [false, true] {
        for count in [None, Some(LqCountBound::All)] {
            for repo_filter in [
                LqFilter::Select {
                    dim: LqSelect::Repo,
                },
                LqFilter::Type { kind: LqType::Repo },
            ] {
                for term in ["needle", "absent_term"] {
                    let mut request = query(term, manual, false);
                    request.options.count = count;
                    request.filters.push(repo_filter.clone());
                    let expected = if term == "needle" {
                        vec![("source-a", "chunk-00"), ("source-b", "chunk-02")]
                    } else {
                        Vec::new()
                    };
                    let ordinary = searcher.search(&request, 4, &RequestBudgetV1::unbounded())?;
                    let ordinary_ids: Vec<_> = ordinary
                        .iter()
                        .map(|row| (row.source_repo_id.as_str(), row.candidate_id.as_str()))
                        .collect();
                    if ordinary_ids != expected {
                        return Err(format!(
                            "ordinary repo control: {ordinary_ids:?} != {expected:?}"
                        )
                        .into());
                    }
                    let all = searcher.search_all(&request, &RequestBudgetV1::unbounded())?;
                    let actual: Vec<_> = all
                        .iter()
                        .map(|row| (row.source_repo_id.as_str(), row.candidate_id.as_str()))
                        .collect();
                    if actual != expected {
                        failures.push(format!("manual={manual} count={count:?} filter={repo_filter:?} term={term}: expected {expected:?}, got {actual:?}"));
                    }
                }
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n").into())
    }
}
