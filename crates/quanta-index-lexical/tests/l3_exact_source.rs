//! Explicit name/source contracts through the real sealed adapter. Fixed IDs
//! and cardinalities come from the fixture, never a full-search baseline.
#![forbid(unsafe_code)]
#![expect(
    clippy::expect_used,
    clippy::float_cmp,
    clippy::indexing_slicing,
    clippy::panic_in_result_fn,
    clippy::string_slice,
    clippy::unchecked_time_subtraction,
    reason = "fixed source fixtures assert exact ranks, scores, spans, and expired deadlines"
)]

use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LexicalCursor, LqCase, LqCountBound,
    LqExpr, LqFilter, LqLeaf, LqOptions, LqPatternType, LqPredicateArg, LqQuery, LqSelect, LqSpan,
    LqYesNoOnly, ManifestGeneration, QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SourceFileCoverage, SourceFileKey,
    SourceFileRevision, SourcePublicationEvent, SymbolCoverage, SymbolId,
    source_event_payload_sha256, source_file_unit_set_sha256,
};
use quanta_index_core::{
    CoreError, LexicalIndexOpenPort, LexicalPageSpec, LexicalSearcher, RequestBudgetV1,
    SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;
use std::error::Error;

type TestResult = Result<(), Box<dyn Error>>;
type ScopeExpectation<'a> = (&'a str, Option<&'a str>, &'a [&'a str], &'a [&'a str]);

fn scope(
    owner: &str,
    file: &str,
    definitions: &[(&str, &str, &str, Option<&str>)],
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust").map_err(str::to_string)?;
    let text = format!("content_only Café One::Café Café::Inner {owner} {file}");
    let path = RepoRelativePath::new(file);
    let chunks = vec![ChunkRecord {
        chunk_id: ChunkId::new(format!("chunk-{owner}-{file}")),
        repo_relative_path: path.clone(),
        language: language.clone(),
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 1,
        end_line: 1,
        text: text.clone().into_boxed_str(),
        structural: None,
        parent_chunk_id: None,
        source_repo_id: Some(RepoId::new(owner)?),
    }];
    let symbols = definitions
        .iter()
        .map(|(id, local, qualified, container)| {
            Ok(SymbolRecord {
                symbol_id: SymbolId::new(*id),
                repo_relative_path: path.clone(),
                language: language.clone(),
                symbol_kind: SymbolKindCode::new("function").map_err(str::to_string)?,
                symbol_kind_family: None,
                local_name: (*local).into(),
                qualified_name: (*qualified).into(),
                signature: Some("fn()".into()),
                visibility: None,
                definition_span: SymbolSpan {
                    path: file.into(),
                    byte_start: 0,
                    byte_end: u32::try_from(text.len())?,
                    line_start: 1,
                    line_end: 1,
                },
                container_qualified_name: container.map(Into::into),
                relationship: SymbolRelationship::Def,
            })
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    Ok(SearchCorpusReplaceScope {
        source_bytes: text.as_bytes().to_vec(),
        coverage: SourceFileCoverage {
            source: SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new(owner)?,
                    repo_relative_path: path,
                },
                revision_id: RevisionId::new(format!("rev-{owner}"))?,
                source_sha256: Sha256::digest(text.as_bytes()).into(),
            },
            language,
            producer_policy_sha256: [3; 32],
            symbol_name_source_policy: quanta_index_contract::SymbolNameSourcePolicyV1::Unspecified,
            unit_set_sha256: source_file_unit_set_sha256(&chunks, &symbols)?,
            text_admitted: true,
            symbols: SymbolCoverage::Complete {
                symbol_count: u64::try_from(symbols.len())?,
            },
        },
        chunks,
        symbols,
    })
}

// Fixed SHA-256 goldens for the independent source byte fixtures above.
// Distinct source/path pairs deliberately have different content identities.
fn expected_source_hash(owner: &str, path: &str) -> Result<[u8; 32], Box<dyn Error>> {
    Ok(match (owner, path) {
        ("source-a", "same.rs") => [
            0xe9, 0xb5, 0x26, 0x51, 0x5c, 0x80, 0x9d, 0x93, 0x78, 0xb0, 0x7d, 0xeb, 0x8e, 0xea,
            0xbf, 0x8c, 0x74, 0xad, 0xaa, 0x36, 0x12, 0x07, 0x50, 0xb7, 0xa7, 0x81, 0x14, 0x2c,
            0x90, 0x97, 0xec, 0x87,
        ],
        ("source-a", "other.rs") => [
            0x05, 0xaf, 0x1a, 0x12, 0xfa, 0x02, 0x9b, 0x63, 0xf5, 0x70, 0x2a, 0x51, 0x52, 0x2f,
            0xc0, 0x00, 0x07, 0x61, 0x4f, 0x6b, 0x72, 0x0f, 0xd6, 0xfc, 0xf2, 0xce, 0x20, 0x88,
            0x2e, 0xec, 0x4f, 0xe7,
        ],
        ("source-b", "same.rs") => [
            0x9a, 0x1b, 0xa4, 0xf9, 0x59, 0xd1, 0x66, 0xe7, 0x61, 0xb9, 0xb1, 0xd5, 0xbf, 0x8b,
            0xec, 0x83, 0x7e, 0xc0, 0x2c, 0x7a, 0x81, 0x0b, 0xf1, 0x82, 0x14, 0x3f, 0xc4, 0x66,
            0x4b, 0xaf, 0xf8, 0x44,
        ],
        ("zero-symbols", "same.rs") => [
            0xb6, 0xa9, 0xae, 0xec, 0xdf, 0xe8, 0x2d, 0x46, 0x35, 0x52, 0xf6, 0xdd, 0x19, 0xb2,
            0xe4, 0x87, 0x1c, 0x25, 0xd1, 0xa2, 0xe9, 0xe4, 0x20, 0xb6, 0x80, 0x7f, 0xe3, 0xbc,
            0x9a, 0xb7, 0x07, 0x7a,
        ],
        _ => return Err("unknown source hash fixture".into()),
    })
}

fn fixture() -> Result<(tempfile::TempDir, Box<dyn LexicalSearcher>), Box<dyn Error>> {
    fixture_with_scopes(vec![
        scope(
            "source-b",
            "same.rs",
            &[("b", "Café", "Two::Café", Some("Two"))],
        )?,
        scope(
            "source-a",
            "same.rs",
            &[
                ("a1", "Café", "One::Café", Some("One")),
                ("a2", "Café", "One::Café", Some("One")),
                ("nested", "Inner", "Café::Inner", Some("Café")),
            ],
        )?,
        scope("zero-symbols", "same.rs", &[])?,
    ])
}

fn fixture_with_scopes(
    replace_scopes: Vec<SearchCorpusReplaceScope>,
) -> Result<(tempfile::TempDir, Box<dyn LexicalSearcher>), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let adapter = LexicalAdapter::with_state_root(dir.path().to_path_buf());
    let repo = RepoId::new("containing-snapshot")?;
    let revision = RevisionId::new("snapshot-rev")?;
    let mut batch = SearchCorpusIngestBatch {
        source_event: SourcePublicationEvent {
            stream_id: "l3".into(),
            event_id: "one".into(),
            expected_base_event_id: None,
            payload_sha256: [0; 32],
        },
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        generation: ManifestGeneration::new(1),
        base_generation: None,
        manifest_digest: "l3-manifest".into(),
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
    let searcher = adapter.open(
        &repo,
        &revision,
        ManifestGeneration::new(1),
        &RequestBudgetV1::unbounded(),
    )?;
    Ok((dir, searcher))
}

fn query(name: &str, value: &str, manual: bool, sensitive: bool) -> LqQuery {
    let mut options = LqOptions::defaults();
    if manual {
        options.index_mode = Some(LqYesNoOnly::No);
    }
    if sensitive {
        options.case = Some(LqCase::Sensitive);
    }
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Predicate {
            name: name.into(),
            args: vec![LqPredicateArg::Keyword(value.into())],
        }),
        filters: Vec::new(),
        options,
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn code_query(terms: &[&str], sensitive: bool) -> LqQuery {
    let mut options = LqOptions::defaults();
    options.pattern_type = LqPatternType::CodeSearch;
    if sensitive {
        options.case = Some(LqCase::Sensitive);
    }
    let leaves: Vec<_> = terms
        .iter()
        .map(|term| LqExpr::Leaf(LqLeaf::RawString((*term).into())))
        .collect();
    let expr = if leaves.len() == 1 {
        leaves.into_iter().next().expect("one leaf")
    } else {
        LqExpr::All(leaves)
    };
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr,
        filters: vec![LqFilter::Select {
            dim: LqSelect::File,
        }],
        options,
        directives: Vec::new(),
        source_span: LqSpan::eof(0),
    }
}

fn code_scope(
    file: &str,
    body: &str,
    split: usize,
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let mut scope = scope("source-a", file, &[])?;
    let body_bytes = body.as_bytes();
    let parts = [
        (&body[..split], 0_u32),
        (&body[split..], u32::try_from(split)?),
    ];
    scope.chunks = parts
        .into_iter()
        .enumerate()
        .map(|(index, (text, start))| {
            Ok(ChunkRecord {
                chunk_id: ChunkId::new(format!("chunk-{file}-{index}")),
                repo_relative_path: RepoRelativePath::new(file),
                language: LanguageCode::new("rust")?,
                start_byte: start,
                end_byte: start
                    .checked_add(u32::try_from(text.len())?)
                    .ok_or("chunk end overflow")?,
                start_line: 1,
                end_line: 1,
                text: text.into(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: Some(RepoId::new("source-a")?),
            })
        })
        .collect::<Result<_, Box<dyn Error>>>()?;
    scope.source_bytes = body_bytes.to_vec();
    scope.coverage.source.source_sha256 = Sha256::digest(body_bytes).into();
    scope.coverage.unit_set_sha256 = source_file_unit_set_sha256(&scope.chunks, &[])?;
    Ok(scope)
}

fn repartition_code_scope(
    scope: &mut SearchCorpusReplaceScope,
    spans: &[(usize, usize)],
) -> Result<(), Box<dyn Error>> {
    let path = scope.coverage.source.file.repo_relative_path.clone();
    let owner = scope.coverage.source.file.source_repo_id.clone();
    scope.chunks = spans
        .iter()
        .enumerate()
        .map(|(index, &(start, end))| {
            let text = std::str::from_utf8(
                scope
                    .source_bytes
                    .get(start..end)
                    .ok_or("span outside source")?,
            )?;
            Ok(ChunkRecord {
                chunk_id: ChunkId::new(format!("{}-{index}", path.as_str())),
                repo_relative_path: path.clone(),
                language: scope.coverage.language.clone(),
                start_byte: u32::try_from(start)?,
                end_byte: u32::try_from(end)?,
                start_line: 1,
                end_line: 1,
                text: text.into(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: Some(owner.clone()),
            })
        })
        .collect::<Result<_, Box<dyn Error>>>()?;
    scope.coverage.text_admitted = !spans.is_empty();
    scope.coverage.unit_set_sha256 = source_file_unit_set_sha256(&scope.chunks, &[])?;
    Ok(())
}

#[test]
fn explicit_typo_search_uses_source_tokens_and_valid_spans() -> TestResult {
    let (_dir, searcher) = fixture_with_scopes(vec![
        code_scope("exact.rs", "load_jsom", 4)?,
        code_scope("typo.rs", "load_json", 4)?,
        code_scope("negative.rs", "load_jsxx", 4)?,
        code_scope("load_json.rs", "other stuff", 5)?,
    ])?;
    let mut request = code_query(&["load_jsom"], false);
    request.expr = LqExpr::Leaf(LqLeaf::Predicate {
        name: "code_search.identifier_typo".into(),
        args: vec![LqPredicateArg::RawString("load_jsom".into())],
    });
    let rows = searcher
        .search_constrained(
            &request,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(
        rows.iter()
            .map(|row| row.repo_relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["exact.rs", "typo.rs"]
    );
    assert!(rows[0].score > rows[1].score);
    assert_eq!(
        rows[1]
            .preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .map(|span| (span.start, span.end)),
        Some((0, 9))
    );
    let exact = code_query(&["load_jsom"], false);
    let exact_rows = searcher
        .search_constrained(
            &exact,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(exact_rows.len(), 1);
    assert_eq!(exact_rows[0].repo_relative_path.as_str(), "exact.rs");
    request.options.case = Some(LqCase::Sensitive);
    request.expr = LqExpr::Leaf(LqLeaf::Predicate {
        name: "code_search.identifier_typo".into(),
        args: vec![LqPredicateArg::RawString("LOAD_JSOM".into())],
    });
    let sensitive_rows = searcher
        .search_constrained(
            &request,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert!(sensitive_rows.is_empty());
    Ok(())
}

#[test]
fn symbol_components_match_one_ordered_name_and_page_distinct_files() -> TestResult {
    let (_dir, searcher) = fixture_with_scopes(vec![
        scope(
            "source-a",
            "exact.rs",
            &[("exact", "UpdateAvailableNo", "A::UpdateAvailableNo", None)],
        )?,
        scope(
            "source-a",
            "longer.rs",
            &[(
                "longer",
                "UpdateAvailableNoCurrentVersion",
                "B::UpdateAvailableNoCurrentVersion",
                None,
            )],
        )?,
        scope(
            "source-a",
            "reversed.rs",
            &[(
                "reversed",
                "UpdateNoAvailable",
                "C::UpdateNoAvailable",
                None,
            )],
        )?,
        scope(
            "source-a",
            "split.rs",
            &[
                ("update", "Update", "D::Update", None),
                ("available", "AvailableNo", "D::AvailableNo", None),
            ],
        )?,
    ])?;
    let mut query = code_query(&["update available no"], false);
    query.expr = LqExpr::Leaf(LqLeaf::Predicate {
        name: "code_search.symbol_components".into(),
        args: vec![LqPredicateArg::RawString("update available no".into())],
    });
    let budget = RequestBudgetV1::unbounded();
    let first = searcher.search_constrained(
        &query,
        &QueryConstraintSetV1::default(),
        &LexicalPageSpec::first(10),
        &budget,
    )?;
    assert_eq!(first.exact_total, Some(2));
    assert_eq!(
        first
            .candidates
            .iter()
            .map(|row| row.repo_relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["exact.rs", "longer.rs"]
    );
    assert!(first.candidates[0].score > first.candidates[1].score);

    let one = searcher.search_constrained(
        &query,
        &QueryConstraintSetV1::default(),
        &LexicalPageSpec::first(1),
        &budget,
    )?;
    assert_eq!(one.candidates.len(), 1);
    assert_eq!(
        one.candidates[0].candidate_id,
        first.candidates[0].candidate_id
    );
    let next = searcher.search_constrained(
        &query,
        &QueryConstraintSetV1::default(),
        &LexicalPageSpec {
            fetch: 1,
            after: Some(LexicalCursor::at(
                ManifestGeneration::new(1),
                one.candidates[0].order_key(),
            )),
        },
        &budget,
    )?;
    assert_eq!(next.candidates.len(), 1);
    assert_eq!(
        next.candidates[0].candidate_id,
        first.candidates[1].candidate_id
    );
    let mut sensitive = query;
    sensitive.options.case = Some(LqCase::Sensitive);
    assert!(matches!(
        searcher.search_constrained(
            &sensitive,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &budget,
        ),
        Err(CoreError::Typed { .. })
    ));
    Ok(())
}

#[test]
fn symbol_components_refuse_incomplete_in_scope_source() -> TestResult {
    let complete = scope(
        "source-a",
        "complete.rs",
        &[("clean", "cleanUp", "C::cleanUp", None)],
    )?;
    let mut incomplete = scope("source-a", "CLEAN-UP.rs", &[])?;
    incomplete.coverage.symbols = SymbolCoverage::ParseFailed;
    let (_dir, searcher) = fixture_with_scopes(vec![complete, incomplete])?;
    let mut query = code_query(&["clean up"], false);
    query.expr = LqExpr::Leaf(LqLeaf::Predicate {
        name: "code_search.symbol_components".into(),
        args: vec![LqPredicateArg::RawString("clean up".into())],
    });
    let error = searcher
        .search_constrained(
            &query,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )
        .expect_err("incomplete symbol authority must not look like complete search");
    assert!(matches!(
        error,
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SymbolCoverageIncomplete,
            ..
        }
    ));
    let constraints = QueryConstraintSetV1 {
        language_any_of: BTreeSet::new(),
        repo_relative_path_exact: Some(
            quanta_index_contract::ExactRepoRelativePathV1::new("complete.rs")
                .map_err(str::to_string)?,
        ),
    };
    let page = searcher.search_constrained(
        &query,
        &constraints,
        &LexicalPageSpec::first(10),
        &RequestBudgetV1::unbounded(),
    )?;
    assert_eq!(page.exact_total, Some(1));
    assert_eq!(
        page.candidates[0].repo_relative_path.as_str(),
        "complete.rs"
    );

    let complete = scope(
        "source-a",
        "complete.rs",
        &[("clean", "cleanUp", "C::cleanUp", None)],
    )?;
    let mut no_literal_component = scope("source-a", "unparsed.rs", &[])?;
    no_literal_component.coverage.symbols = SymbolCoverage::ParseFailed;
    let (_dir, searcher) = fixture_with_scopes(vec![complete, no_literal_component])?;
    let error = searcher
        .search_constrained(
            &query,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )
        .expect_err("source-byte absence does not establish a complete symbol census");
    assert!(matches!(
        error,
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::SymbolCoverageIncomplete,
            ..
        }
    ));
    Ok(())
}

#[test]
fn default_bare_identifier_uses_osa1_only_after_empty_literal_search() -> TestResult {
    let (_dir, searcher) = fixture_with_scopes(vec![
        code_scope("near.rs", "load_json", 4)?,
        code_scope("far.rs", "load_jsxx", 4)?,
        code_scope("load_json.rs", "other stuff", 5)?,
    ])?;
    let request = code_query(&["load_jsom"], false);
    let rows = searcher
        .search_constrained(
            &request,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].repo_relative_path.as_str(), "near.rs");
    assert_eq!(
        rows[0]
            .preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .map(|span| (span.start, span.end)),
        Some((0, 9))
    );

    let mut content_only = request.clone();
    content_only.expr = LqExpr::Leaf(LqLeaf::Predicate {
        name: "code_search.content".into(),
        args: vec![LqPredicateArg::RawString("load_jsom".into())],
    });
    let mut path_only = request.clone();
    path_only.expr = LqExpr::Leaf(LqLeaf::Predicate {
        name: "code_search.path".into(),
        args: vec![LqPredicateArg::RawString("load_jsom".into())],
    });
    for exact_request in [
        content_only,
        path_only,
        code_query(&["load_jsom"], true),
        code_query(&["load_jsom", "other"], false),
        code_query(&["zzzxxxx"], false),
    ] {
        assert!(
            searcher
                .search_constrained(
                    &exact_request,
                    &QueryConstraintSetV1::default(),
                    &LexicalPageSpec::first(10),
                    &RequestBudgetV1::unbounded(),
                )?
                .candidates
                .is_empty()
        );
    }

    let (_dir, exact_searcher) = fixture_with_scopes(vec![
        code_scope("exact.rs", "load_jsom", 4)?,
        code_scope("near.rs", "load_json", 4)?,
    ])?;
    let first = exact_searcher.search_constrained(
        &request,
        &QueryConstraintSetV1::default(),
        &LexicalPageSpec::first(1),
        &RequestBudgetV1::unbounded(),
    )?;
    assert_eq!(first.exact_total, Some(1));
    assert_eq!(first.candidates.len(), 1);
    assert_eq!(first.candidates[0].repo_relative_path.as_str(), "exact.rs");
    let after = LexicalCursor::at(ManifestGeneration::new(1), first.candidates[0].order_key());
    assert!(
        exact_searcher
            .search_constrained(
                &request,
                &QueryConstraintSetV1::default(),
                &LexicalPageSpec {
                    fetch: 1,
                    after: Some(after),
                },
                &RequestBudgetV1::unbounded(),
            )?
            .candidates
            .is_empty()
    );
    Ok(())
}

#[test]
fn code_search_matches_file_across_chunk_boundaries_and_maps_unicode_source_span() -> TestResult {
    let (_dir, searcher) = fixture_with_scopes(vec![
        code_scope("cross.rs", "alphaBeta İ", 5)?,
        code_scope("first.rs", "alpha only", 5)?,
        code_scope("second.rs", "Beta only", 4)?,
    ])?;
    let request = code_query(&["alpha", "Beta"], true);
    let rows = searcher
        .search_constrained(
            &request,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(
        rows.iter()
            .map(|row| row.repo_relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["cross.rs"]
    );
    assert!(rows[0].candidate_id.starts_with("file:"));
    assert_eq!((rows[0].start_line, rows[0].end_line), (1, 1));

    let boundary = code_query(&["haBe"], true);
    let rows = searcher
        .search_constrained(
            &boundary,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(
        rows.iter()
            .map(|row| row.repo_relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["cross.rs"]
    );
    assert_eq!(
        rows[0]
            .preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .map(|span| (span.start, span.end)),
        Some((3, 7))
    );

    let folded = code_query(&["i\u{307}"], false);
    let rows = searcher
        .search_constrained(
            &folded,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(
        rows.iter()
            .map(|row| row.repo_relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["cross.rs"]
    );
    assert_eq!(
        rows[0]
            .preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .map(|span| (span.start, span.end)),
        Some((10, 12))
    );
    let mut unsupported = code_query(&["alpha"], true);
    unsupported.options.count = Some(LqCountBound::All);
    assert!(matches!(
        searcher.search_constrained(
            &unsupported,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        ),
        Err(CoreError::Typed { .. })
    ));
    let expired =
        RequestBudgetV1::until(std::time::Instant::now() - std::time::Duration::from_millis(1));
    assert!(matches!(
        searcher.search_constrained(
            &request,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &expired,
        ),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::RequestDeadlineExceeded,
            ..
        })
    ));
    Ok(())
}

#[test]
fn code_search_preview_flags_only_actual_nfc_source_difference() -> TestResult {
    let (_dir, searcher) = fixture_with_scopes(vec![
        code_scope("identity.rs", "needle", 6)?,
        code_scope("decomposed.rs", "e\u{301}", 3)?,
    ])?;
    for (term, expected_path, expected_difference) in [
        ("needle", "identity.rs", false),
        ("é", "decomposed.rs", true),
    ] {
        let rows = searcher
            .search_constrained(
                &code_query(&[term], true),
                &QueryConstraintSetV1::unconstrained(),
                &LexicalPageSpec::first(10),
                &RequestBudgetV1::unbounded(),
            )?
            .candidates;
        let row = rows.first().ok_or("source match missing")?;
        if rows.len() != 1 || row.repo_relative_path.as_str() != expected_path {
            return Err(format!("unexpected source match for {term}: {rows:?}").into());
        }
        let preview = row.preview.as_ref().ok_or("source preview missing")?;
        if preview.normalization_equivalent != expected_difference {
            return Err(format!("wrong NFC preview flag for {term}: {preview:?}").into());
        }
        row.validate_source_metadata().map_err(str::to_owned)?;
    }
    Ok(())
}

#[test]
fn code_search_regex_case_flag_text_inside_class_is_literal() -> TestResult {
    let (_dir, searcher) = fixture_with_scopes(vec![code_scope(
        "src/class.rs",
        "the ( token is present",
        7,
    )?])?;
    let mut query = code_query(&["unused"], false);
    query.expr = LqExpr::Leaf(LqLeaf::Predicate {
        name: "code_search.content_regex".into(),
        args: vec![LqPredicateArg::RawString("[(?i]".into())],
    });
    for sensitive in [false, true] {
        query.options.case = sensitive.then_some(LqCase::Sensitive);
        let rows = searcher
            .search_constrained(
                &query,
                &QueryConstraintSetV1::default(),
                &LexicalPageSpec::first(10),
                &RequestBudgetV1::unbounded(),
            )?
            .candidates;
        assert_eq!(rows.len(), 1, "case_sensitive={sensitive}");
        assert_eq!(rows[0].repo_relative_path.as_str(), "src/class.rs");
    }
    // The case flag remains forbidden even when another flag comes first.
    for (pattern, sensitive) in [("(?m-i:FOO)", false), ("(?i:foo)", true)] {
        query.options.case = sensitive.then_some(LqCase::Sensitive);
        query.expr = LqExpr::Leaf(LqLeaf::Predicate {
            name: "code_search.content_regex".into(),
            args: vec![LqPredicateArg::RawString(pattern.into())],
        });
        assert!(matches!(
            searcher.search_constrained(
                &query,
                &QueryConstraintSetV1::default(),
                &LexicalPageSpec::first(10),
                &RequestBudgetV1::unbounded(),
            ),
            Err(CoreError::Typed {
                code:
                    quanta_index_contract::SearchPlaneErrorCodeV2::LexPlannerUnsupportedFilterCombo,
                ..
            })
        ));
    }
    Ok(())
}

#[test]
fn code_search_regex_uses_full_file_authority_and_bounded_source_focus() -> TestResult {
    let (_dir, searcher) = fixture_with_scopes(vec![
        code_scope("src/target.rs", "sphinx middle quartz", 7)?,
        code_scope("src/other.rs", "sphinx alone", 7)?,
        code_scope("misc/target.rs", "nothing relevant", 7)?,
        code_scope("unicode.rs", "Cafe\u{301} needle", 4)?,
    ])?;
    let run = |query: &LqQuery| {
        searcher.search_constrained(
            query,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )
    };
    let mut content = code_query(&["unused"], false);
    content.expr = LqExpr::Leaf(LqLeaf::Regex("sphinx.*quartz".into()));
    let hits = run(&content)?.candidates;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].repo_relative_path.as_str(), "src/target.rs");
    let focus = hits[0]
        .preview
        .as_ref()
        .and_then(|preview| preview.original_focus)
        .ok_or("missing regex focus")?;
    assert_eq!((focus.start, focus.end), (0, 20));
    let mut scoped = code_query(&["unused"], false);
    scoped.expr = LqExpr::All(vec![
        LqExpr::Leaf(LqLeaf::RawString("sphinx".into())),
        LqExpr::Leaf(LqLeaf::Predicate {
            name: "code_search.path_regex".into(),
            args: vec![LqPredicateArg::RawString(r"src/target\.rs".into())],
        }),
    ]);
    let hits = run(&scoped)?.candidates;
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].repo_relative_path.as_str(), "src/target.rs");
    let mut content_scoped = code_query(&["unused"], false);
    content_scoped.expr = LqExpr::Leaf(LqLeaf::Predicate {
        name: "code_search.content_regex".into(),
        args: vec![LqPredicateArg::RawString("SPHINX.*QUARTZ".into())],
    });
    assert_eq!(run(&content_scoped)?.candidates.len(), 1);
    content_scoped.options.case = Some(LqCase::Sensitive);
    assert!(run(&content_scoped)?.candidates.is_empty());
    let mut unicode = code_query(&["unused"], false);
    unicode.expr = LqExpr::Leaf(LqLeaf::Regex("caf.".into()));
    let unicode_rows = run(&unicode)?.candidates;
    assert_eq!(unicode_rows.len(), 1);
    assert_eq!(unicode_rows[0].repo_relative_path.as_str(), "unicode.rs");
    assert_eq!(
        unicode_rows[0]
            .preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .map(|range| (range.start, range.end)),
        Some((0, 6))
    );
    let mut zero_width = code_query(&["unused"], false);
    zero_width.expr = LqExpr::Leaf(LqLeaf::Regex(".*".into()));
    assert!(matches!(run(&zero_width), Err(CoreError::Typed { .. })));
    let mut forbidden = code_query(&["unused"], false);
    forbidden.expr = LqExpr::Leaf(LqLeaf::Regex("(?<=sphinx)quartz".into()));
    assert!(matches!(run(&forbidden), Err(CoreError::Typed { .. })));
    let expired =
        RequestBudgetV1::until(std::time::Instant::now() - std::time::Duration::from_millis(1));
    assert!(matches!(
        searcher.search_constrained(
            &content,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &expired,
        ),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::RequestDeadlineExceeded,
            ..
        })
    ));
    Ok(())
}

#[test]
fn code_search_applies_language_before_short_and_regex_candidates() -> TestResult {
    let mut go = code_scope("src/go.rs", "xa", 1)?;
    go.coverage.language = LanguageCode::new("go")?;
    for chunk in &mut go.chunks {
        chunk.language = go.coverage.language.clone();
    }
    go.coverage.unit_set_sha256 = source_file_unit_set_sha256(&go.chunks, &[])?;
    let rust = code_scope("src/rust.rs", "xa", 1)?;
    let (_dir, searcher) = fixture_with_scopes(vec![go, rust])?;
    let mut constraints = QueryConstraintSetV1::unconstrained();
    let _inserted = constraints.language_any_of.insert(LanguageCode::new("go")?);
    for regex in [false, true] {
        let mut request = code_query(&["x"], true);
        if regex {
            request.expr = LqExpr::Leaf(LqLeaf::Regex("x".into()));
        }
        let rows = searcher
            .search_constrained(
                &request,
                &constraints,
                &LexicalPageSpec::first(10),
                &RequestBudgetV1::unbounded(),
            )?
            .candidates;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].repo_relative_path.as_str(), "src/go.rs");
    }
    Ok(())
}

#[test]
fn code_search_ranks_unicode_exact_identifier_above_combining_mark_prefix() -> TestResult {
    let (_dir, searcher) = fixture_with_scopes(vec![
        code_scope("a-prefix.rs", "म\u{094d} ", "म".len())?,
        code_scope("z-exact.rs", "म ", "म".len())?,
    ])?;
    let rows = searcher
        .search_constrained(
            &code_query(&["म"], true),
            &QueryConstraintSetV1::unconstrained(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )?
        .candidates;
    assert_eq!(
        rows.iter()
            .map(|row| row.repo_relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["z-exact.rs", "a-prefix.rs"]
    );
    assert!(rows[0].score > rows[1].score);
    Ok(())
}

#[test]
fn code_search_regex_match_cap_is_a_typed_refusal() -> TestResult {
    let source = "a".repeat(4_100);
    let (_dir, searcher) = fixture_with_scopes(vec![code_scope("dense.rs", &source, 2_050)?])?;
    let mut query = code_query(&["unused"], true);
    query.expr = LqExpr::Leaf(LqLeaf::Regex("a".into()));
    assert!(matches!(
        searcher.search_constrained(
            &query,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        ),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
            ..
        })
    ));
    query.expr = LqExpr::All(
        (0..5)
            .map(|_| LqExpr::Leaf(LqLeaf::Regex("a".into())))
            .collect(),
    );
    assert!(matches!(
        searcher.search_constrained(
            &query,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        ),
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::LexRegexPlanLimitExceeded,
            ..
        })
    ));
    Ok(())
}

#[test]
fn code_search_file_ranking_is_distinct_and_cursor_stable_with_fifteen_chunks() -> TestResult {
    let repeated = "alpha".repeat(15);
    let mut packed = code_scope("packed.rs", &repeated, 5)?;
    packed.chunks = (0..15)
        .map(|index| {
            let start = u32::try_from(index * 5)?;
            Ok(ChunkRecord {
                chunk_id: ChunkId::new(format!("packed-{index}")),
                repo_relative_path: RepoRelativePath::new("packed.rs"),
                language: LanguageCode::new("rust")?,
                start_byte: start,
                end_byte: start + 5,
                start_line: 1,
                end_line: 1,
                text: "alpha".into(),
                structural: None,
                parent_chunk_id: None,
                source_repo_id: Some(RepoId::new("source-a")?),
            })
        })
        .collect::<Result<_, Box<dyn Error>>>()?;
    packed.coverage.unit_set_sha256 = source_file_unit_set_sha256(&packed.chunks, &[])?;
    let mut scopes = vec![packed];
    for index in 0..9 {
        scopes.push(code_scope(&format!("f{index}.rs"), "alpha stable", 5)?);
    }
    let mut path_only = scope("source-a", "alpha-path.rs", &[])?;
    path_only.source_bytes.clear();
    path_only.chunks.clear();
    path_only.coverage.text_admitted = false;
    path_only.coverage.source.source_sha256 = Sha256::digest(b"").into();
    path_only.coverage.unit_set_sha256 = source_file_unit_set_sha256(&[], &[])?;
    scopes.push(path_only);
    let (_dir, searcher) = fixture_with_scopes(scopes)?;
    let mut request = code_query(&["alpha"], true);
    request.expr = LqExpr::Leaf(LqLeaf::Predicate {
        name: "code_search.content".into(),
        args: vec![LqPredicateArg::RawString("alpha".into())],
    });
    let budget = RequestBudgetV1::unbounded();
    let first = searcher.search_constrained(
        &request,
        &QueryConstraintSetV1::default(),
        &LexicalPageSpec::first(10),
        &budget,
    )?;
    assert_eq!(first.exact_total, Some(10));
    assert_eq!(first.candidates.len(), 10);
    let expected: BTreeSet<String> = std::iter::once("packed.rs".to_string())
        .chain((0..9).map(|index| format!("f{index}.rs")))
        .collect();
    let observed: BTreeSet<String> = first
        .candidates
        .iter()
        .map(|row| row.repo_relative_path.as_str().to_string())
        .collect();
    assert_eq!(observed, expected);
    let mut after = None;
    let mut paged = Vec::new();
    loop {
        let page = searcher.search_constrained(
            &request,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec { fetch: 3, after },
            &budget,
        )?;
        let Some(last) = page.candidates.last() else {
            break;
        };
        after = Some(LexicalCursor::at(
            ManifestGeneration::new(1),
            last.order_key(),
        ));
        paged.extend(page.candidates.into_iter().map(|row| row.candidate_id));
    }
    assert_eq!(
        paged,
        first
            .candidates
            .iter()
            .map(|row| row.candidate_id.clone())
            .collect::<Vec<_>>()
    );
    let mut path_request = code_query(&["alpha-path.rs"], true);
    path_request.expr = LqExpr::Leaf(LqLeaf::Predicate {
        name: "code_search.path".into(),
        args: vec![LqPredicateArg::RawString("alpha-path.rs".into())],
    });
    let path = searcher.search_constrained(
        &path_request,
        &QueryConstraintSetV1::default(),
        &LexicalPageSpec::first(10),
        &budget,
    )?;
    assert_eq!(path.candidates.len(), 1);
    assert_eq!(
        path.candidates[0].repo_relative_path.as_str(),
        "alpha-path.rs"
    );
    assert_eq!(
        path.candidates[0]
            .preview
            .as_ref()
            .map(|preview| preview.kind),
        Some(quanta_index_contract::PreviewKind::Path)
    );
    Ok(())
}

#[test]
fn code_search_fixed_scores_and_chunk_overlap_invariance() -> TestResult {
    let mut plain = code_scope("plain.rs", "alpha beta alpha", 5)?;
    repartition_code_scope(&mut plain, &[(0, 16)])?;
    let mut overlap = code_scope("overlap.rs", "alpha beta alpha", 5)?;
    repartition_code_scope(&mut overlap, &[(0, 10), (6, 16)])?;
    let mut path_only = scope("source-a", "alpha.rs", &[])?;
    path_only.source_bytes.clear();
    path_only.coverage.source.source_sha256 = Sha256::digest(b"").into();
    repartition_code_scope(&mut path_only, &[])?;
    let (_dir, searcher) = fixture_with_scopes(vec![
        code_scope("single.rs", "alpha", 1)?,
        code_scope("many.rs", "alpha alpha alpha alpha alpha", 1)?,
        code_scope("adjacent.rs", "alpha beta", 5)?,
        code_scope("distant.rs", &format!("alpha {} beta", "x".repeat(40)), 5)?,
        code_scope("case_upper.rs", "Alpha", 1)?,
        code_scope("case_lower.rs", "alpha", 1)?,
        plain,
        overlap,
        path_only,
    ])?;
    let scores =
        |query: LqQuery| -> Result<std::collections::BTreeMap<String, f32>, Box<dyn Error>> {
            Ok(searcher
                .search_constrained(
                    &query,
                    &QueryConstraintSetV1::default(),
                    &LexicalPageSpec::first(20),
                    &RequestBudgetV1::unbounded(),
                )?
                .candidates
                .into_iter()
                .map(|row| (row.repo_relative_path.as_str().to_string(), row.score))
                .collect())
        };
    let one = scores(code_query(&["alpha"], true))?;
    assert_eq!(one["single.rs"], 100.0);
    assert_eq!(one["many.rs"], 106.0); // Five occurrences, three extra count.
    let two = scores(code_query(&["alpha", "beta"], true))?;
    assert_eq!(two["adjacent.rs"], 231.0); // 100 + 100 + (32 - 1 byte gap).
    assert_eq!(two["distant.rs"], 200.0); // Gap beyond 32 bytes.
    assert_eq!(two["plain.rs"], 233.0); // Two alpha occurrences add 2.
    assert_eq!(two["overlap.rs"], two["plain.rs"]);
    let folded = scores(code_query(&["Alpha"], false))?;
    assert_eq!(folded["case_upper.rs"], 105.0);
    assert_eq!(folded["case_lower.rs"], 100.0);
    let mut path_query = code_query(&["alpha"], true);
    path_query.expr = LqExpr::Leaf(LqLeaf::Predicate {
        name: "code_search.path".into(),
        args: vec![LqPredicateArg::RawString("alpha".into())],
    });
    assert_eq!(scores(path_query)?["alpha.rs"], 135.0);
    Ok(())
}

#[test]
fn code_search_matches_independent_source_byte_scan_oracle() -> TestResult {
    let bodies = [
        ("oracle-a.rs", "preNEEDLEpost", 6),
        ("oracle-b.rs", "not here", 3),
        ("oracle-c.rs", "NEEDLE", 3),
        ("oracle-d.rs", "left---right", 7),
        ("unicode.rs", "İNEEDLE", 2),
    ];
    let scopes = bodies
        .iter()
        .map(|(path, body, split)| code_scope(path, body, *split))
        .collect::<Result<Vec<_>, _>>()?;
    let (_dir, searcher) = fixture_with_scopes(scopes)?;
    let run = |query: &LqQuery| {
        searcher.search_constrained(
            query,
            &QueryConstraintSetV1::default(),
            &LexicalPageSpec::first(10),
            &RequestBudgetV1::unbounded(),
        )
    };
    let hits = run(&code_query(&["NEEDLE"], true))?.candidates;
    let expected: BTreeSet<_> = bodies
        .iter()
        .filter(|(_, body, _)| {
            body.as_bytes()
                .windows(b"NEEDLE".len())
                .any(|part| part == b"NEEDLE")
        })
        .map(|(path, _, _)| *path)
        .collect();
    let actual: BTreeSet<_> = hits
        .iter()
        .map(|row| row.repo_relative_path.as_str())
        .collect();
    assert_eq!(actual, expected);
    for hit in &hits {
        let (_, raw, _) = bodies
            .iter()
            .find(|(path, _, _)| *path == hit.repo_relative_path.as_str())
            .ok_or("unknown oracle file")?;
        let focus = hit
            .preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .ok_or("missing source focus")?;
        let expected_start = raw
            .as_bytes()
            .windows(b"NEEDLE".len())
            .position(|part| part == b"NEEDLE")
            .ok_or("oracle lost literal")?;
        assert_eq!(
            (focus.start, focus.end),
            (
                u64::try_from(expected_start)?,
                u64::try_from(expected_start + 6)?
            )
        );
        assert_eq!(
            raw.as_bytes()
                .get(usize::try_from(focus.start)?..usize::try_from(focus.end)?),
            Some(b"NEEDLE".as_slice())
        );
    }
    let multi = run(&code_query(&["left", "right"], true))?.candidates;
    assert_eq!(
        multi
            .iter()
            .map(|row| row.repo_relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["oracle-d.rs"]
    );
    let mut path_query = code_query(&["oracle-a"], true);
    path_query.expr = LqExpr::Leaf(LqLeaf::Predicate {
        name: "code_search.path".into(),
        args: vec![LqPredicateArg::RawString("oracle-a".into())],
    });
    let path = run(&path_query)?.candidates;
    assert_eq!(
        path.iter()
            .map(|row| row.repo_relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["oracle-a.rs"]
    );
    let unicode = run(&code_query(&["i\u{307}needle"], false))?.candidates;
    assert_eq!(unicode.len(), 1);
    assert_eq!(unicode[0].repo_relative_path.as_str(), "unicode.rs");
    assert_eq!(
        unicode[0]
            .preview
            .as_ref()
            .and_then(|preview| preview.original_focus)
            .map(|span| (span.start, span.end)),
        Some((0, 8))
    );
    Ok(())
}

#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed oracle assertions in a fallible fixture"
)]
fn check_exact_symbol_policy(manual: bool) -> TestResult {
    let (_dir, searcher) = fixture()?;
    let budget = RequestBudgetV1::unbounded();
    {
        for (name, value, sensitive, expected) in [
            (
                "symbol.local_name.exact",
                "CAFE\u{301}",
                false,
                vec!["a1", "a2", "b"],
            ),
            ("symbol.local_name.exact", "café", true, vec![]),
            (
                "symbol.local_name.exact",
                "Café",
                true,
                vec!["a1", "a2", "b"],
            ),
            (
                "symbol.qualified_name.exact",
                "One::Café",
                false,
                vec!["a1", "a2"],
            ),
            ("symbol.qualified_name.exact", "Café", false, vec![]),
            ("symbol.local_name.exact", "One", false, vec![]),
            (
                "symbol.qualified_name.exact",
                "Café::Inner",
                false,
                vec!["nested"],
            ),
            ("symbol.local_name.exact", "absent", false, vec![]),
        ] {
            let request = query(name, value, manual, sensitive);
            let rows = searcher.search_symbols(&request, 20, &budget)?;
            let ids: BTreeSet<_> = rows.iter().map(|row| row.candidate_id.as_str()).collect();
            assert_eq!(
                ids,
                expected.iter().copied().collect(),
                "{name} {value} manual={manual}"
            );
            for row in rows {
                assert_eq!(row.repo_id.as_str(), "containing-snapshot");
                let source = row.source.ok_or("missing source revision")?;
                assert_eq!(row.source_repo_id, source.file.source_repo_id);
                assert_eq!(
                    source.revision_id.as_str(),
                    format!("rev-{}", row.source_repo_id.as_str())
                );
                assert_eq!(row.repo_relative_path, source.file.repo_relative_path);
                assert_eq!(
                    source.source_sha256,
                    expected_source_hash(
                        row.source_repo_id.as_str(),
                        row.repo_relative_path.as_str()
                    )?
                );
            }
        }
    }
    Ok(())
}

#[test]
fn l3_indexed_exact_symbol_policy_preserves_overloads_and_source_owner() -> TestResult {
    check_exact_symbol_policy(false)
}

#[test]
fn l3_manual_exact_symbol_policy_preserves_overloads_and_source_owner() -> TestResult {
    check_exact_symbol_policy(true)
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed oracle assertions in a fallible fixture"
)]
fn l3_exact_symbol_case_distinguishes_definition_from_folded_names() -> TestResult {
    let (_dir, searcher) = fixture_with_scopes(vec![
        scope(
            "source-a",
            "render.go",
            &[("definition", "writeContentType", "writeContentType", None)],
        )?,
        scope(
            "source-a",
            "json.go",
            &[("other", "WriteContentType", "WriteContentType", None)],
        )?,
    ])?;
    for manual in [false, true] {
        for (sensitive, expected) in [
            (false, BTreeSet::from(["definition", "other"])),
            (true, BTreeSet::from(["definition"])),
        ] {
            let rows = searcher.search_symbols(
                &query(
                    "symbol.local_name.exact",
                    "writeContentType",
                    manual,
                    sensitive,
                ),
                10,
                &RequestBudgetV1::unbounded(),
            )?;
            let actual: BTreeSet<_> = rows.iter().map(|row| row.candidate_id.as_str()).collect();
            assert_eq!(actual, expected, "manual={manual} sensitive={sensitive}");
        }
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed oracle assertions in a fallible fixture"
)]
fn l3_exact_symbol_case_and_file_anchor_compose_before_top_k() -> TestResult {
    use quanta_index_contract::ExactRepoRelativePathV1;
    let (_dir, searcher) = fixture_with_scopes(vec![
        scope(
            "source-a",
            "render/render.go",
            &[("target", "writeContentType", "writeContentType", None)],
        )?,
        scope(
            "source-a",
            "alternate/helper.go",
            &[("same-name", "writeContentType", "writeContentType", None)],
        )?,
        scope(
            "source-a",
            "render/case.go",
            &[("wrong-case", "WriteContentType", "WriteContentType", None)],
        )?,
    ])?;
    for manual in [false, true] {
        for (path, expected) in [
            ("render/render.go", Some("target")),
            ("alternate/helper.go", Some("same-name")),
            ("render/case.go", None),
        ] {
            let constraints = QueryConstraintSetV1::from_exact_repo_relative_path(
                ExactRepoRelativePathV1::new(path).map_err(str::to_string)?,
            );
            let page = searcher.search_symbols_constrained(
                &query("symbol.local_name.exact", "writeContentType", manual, true),
                &constraints,
                &LexicalPageSpec::first(1),
                &RequestBudgetV1::unbounded(),
            )?;
            assert_eq!(
                page.candidates
                    .iter()
                    .map(|row| row.candidate_id.as_str())
                    .collect::<Vec<_>>(),
                expected.into_iter().collect::<Vec<_>>(),
                "manual={manual} path={path}"
            );
        }
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed oracle assertions in a fallible fixture"
)]
fn l3_boolean_exact_names_use_symbol_fields_on_indexed_and_manual_routes() -> TestResult {
    let (_dir, searcher) = fixture()?;
    let local = query("symbol.local_name.exact", "Café", false, false).expr;
    let qualified = query("symbol.qualified_name.exact", "One::Café", false, false).expr;
    let nested = query("symbol.qualified_name.exact", "Café::Inner", false, false).expr;
    for manual in [false, true] {
        for (expr, expected) in [
            (
                LqExpr::All(vec![local.clone(), qualified.clone()]),
                vec!["a1", "a2"],
            ),
            (
                LqExpr::Any(vec![qualified.clone(), nested.clone()]),
                vec!["a1", "a2", "nested"],
            ),
            (LqExpr::Not(Box::new(local.clone())), vec!["nested"]),
        ] {
            let mut request = query("symbol.local_name.exact", "unused", manual, false);
            request.expr = expr;
            let rows = searcher.search_symbols(&request, 20, &RequestBudgetV1::unbounded())?;
            let actual: BTreeSet<_> = rows.iter().map(|row| row.candidate_id.as_str()).collect();
            assert_eq!(
                actual,
                expected.into_iter().collect(),
                "manual={manual} expr={:?}",
                request.expr
            );
        }
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed oracle assertions in a fallible fixture"
)]
fn l3_broad_symbol_and_content_policies_remain_independent() -> TestResult {
    let (_dir, searcher) = fixture()?;
    for manual in [false, true] {
        let broad = searcher.search_symbols(
            &query("symbol.has.name", "Café", manual, false),
            20,
            &RequestBudgetV1::unbounded(),
        )?;
        let ids: BTreeSet<_> = broad.iter().map(|row| row.candidate_id.as_str()).collect();
        assert_eq!(ids, BTreeSet::from(["a1", "a2", "b", "nested"]));
        let mut content = query("symbol.local_name.exact", "absent", manual, false);
        content.expr = LqExpr::Leaf(LqLeaf::Keyword("content_only".into()));
        let rows = searcher.search(&content, 20, &RequestBudgetV1::unbounded())?;
        let ids: BTreeSet<_> = rows.iter().map(|row| row.candidate_id.as_str()).collect();
        assert_eq!(
            ids,
            BTreeSet::from([
                "chunk-source-a-same.rs",
                "chunk-source-b-same.rs",
                "chunk-zero-symbols-same.rs"
            ])
        );
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed oracle assertions in a fallible fixture"
)]
fn l3_source_file_and_repo_projections_page_in_fixed_source_order() -> TestResult {
    for reverse in [false, true] {
        let mut scopes = vec![
            scope("source-b", "same.rs", &[])?,
            scope("source-a", "same.rs", &[])?,
            scope("source-a", "other.rs", &[])?,
            scope("zero-symbols", "same.rs", &[])?,
        ];
        if reverse {
            scopes.reverse();
        }
        let (_dir, searcher) = fixture_with_scopes(scopes)?;
        for manual in [false, true] {
            for projection in [
                None,
                Some(LqSelect::File),
                Some(LqSelect::Path),
                Some(LqSelect::Repo),
            ] {
                let mut request = query("symbol.local_name.exact", "unused", manual, false);
                request.expr = LqExpr::Leaf(LqLeaf::Keyword("content_only".into()));
                request.options.count = Some(LqCountBound::All);
                if let Some(dim) = projection {
                    request.filters.push(LqFilter::Select { dim });
                }
                let expected = if projection == Some(LqSelect::Repo) {
                    vec![
                        ("source-a", "other.rs"),
                        ("source-b", "same.rs"),
                        ("zero-symbols", "same.rs"),
                    ]
                } else {
                    vec![
                        ("source-a", "other.rs"),
                        ("source-a", "same.rs"),
                        ("source-b", "same.rs"),
                        ("zero-symbols", "same.rs"),
                    ]
                };
                for fetch in [1, 2, 5] {
                    let mut after = None;
                    let mut actual = Vec::new();
                    // One extra empty page proves terminal exhaustion even at an exact cut.
                    for _ in 0..=expected.len() {
                        let page = searcher.search_constrained(
                            &request,
                            &QueryConstraintSetV1::unconstrained(),
                            &LexicalPageSpec {
                                fetch,
                                after: after.clone(),
                            },
                            &RequestBudgetV1::unbounded(),
                        )?;
                        assert_eq!(
                            page.exact_total,
                            Some(u64::try_from(expected.len() - actual.len())?)
                        );
                        assert!(page.candidates.len() <= usize::try_from(fetch)?);
                        let Some(last) = page.candidates.last() else {
                            break;
                        };
                        after = Some(LexicalCursor::at(
                            ManifestGeneration::new(1),
                            last.order_key(),
                        ));
                        for row in page.candidates {
                            assert_eq!(row.repo_id.as_str(), "containing-snapshot");
                            assert_eq!(row.revision_id.as_str(), "snapshot-rev");
                            let source = row.source.as_ref().ok_or("missing source revision")?;
                            assert_eq!(source.file.source_repo_id, row.source_repo_id);
                            assert_eq!(source.file.repo_relative_path, row.repo_relative_path);
                            assert_eq!(
                                source.source_sha256,
                                expected_source_hash(
                                    row.source_repo_id.as_str(),
                                    row.repo_relative_path.as_str()
                                )?
                            );
                            actual.push((
                                row.source_repo_id.as_str().to_owned(),
                                row.repo_relative_path.as_str().to_owned(),
                            ));
                        }
                        assert!(actual.len() <= expected.len(), "duplicate page rows");
                    }
                    let actual: Vec<_> = actual
                        .iter()
                        .map(|(repo, path)| (repo.as_str(), path.as_str()))
                        .collect();
                    assert_eq!(
                        actual, expected,
                        "projection={projection:?} manual={manual} reverse={reverse} fetch={fetch}"
                    );
                }
            }
        }
    }
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed oracle assertions in a fallible fixture"
)]
fn l3_repo_and_path_scope_precede_source_projection_and_paging() -> TestResult {
    use quanta_index_contract::ExactRepoRelativePathV1;
    let (_dir, searcher) = fixture_with_scopes(vec![
        scope("source-b", "same.rs", &[])?,
        scope("source-a", "same.rs", &[])?,
        scope("source-a", "other.rs", &[])?,
        scope("zero-symbols", "same.rs", &[])?,
    ])?;
    let cases: [ScopeExpectation<'_>; 7] = [
        ("source-a", None, &["other.rs", "same.rs"], &["other.rs"]),
        ("source-a", Some("same.rs"), &["same.rs"], &["same.rs"]),
        ("source-a", Some("missing.rs"), &[], &[]),
        ("source-b", None, &["same.rs"], &["same.rs"]),
        ("zero-symbols", None, &["same.rs"], &["same.rs"]),
        ("containing-snapshot", None, &[], &[]),
        ("absent-source", None, &[], &[]),
    ];
    for manual in [false, true] {
        for (source, path, expected_files, expected_repos) in cases {
            for projection in [None, Some(LqSelect::File), Some(LqSelect::Repo)] {
                let mut request = query("symbol.local_name.exact", "unused", manual, false);
                request.expr = LqExpr::Leaf(LqLeaf::Keyword("content_only".into()));
                request.options.count = Some(LqCountBound::All);
                request.filters.push(LqFilter::Repo {
                    pattern: source.into(),
                    revs: Vec::new(),
                });
                if let Some(dim) = projection {
                    request.filters.push(LqFilter::Select { dim });
                }
                let constraints = QueryConstraintSetV1 {
                    language_any_of: BTreeSet::new(),
                    repo_relative_path_exact: path
                        .map(ExactRepoRelativePathV1::new)
                        .transpose()
                        .map_err(str::to_string)?,
                };
                let expected = if projection == Some(LqSelect::Repo) {
                    expected_repos
                } else {
                    expected_files
                };
                let mut after = None;
                let mut actual = Vec::new();
                for _ in 0..=expected.len() {
                    let page = searcher.search_constrained(
                        &request,
                        &constraints,
                        &LexicalPageSpec {
                            fetch: 1,
                            after: after.clone(),
                        },
                        &RequestBudgetV1::unbounded(),
                    )?;
                    assert_eq!(
                        page.exact_total,
                        Some(u64::try_from(expected.len() - actual.len())?),
                        "source={source} path={path:?} manual={manual} projection={projection:?}"
                    );
                    assert!(page.candidates.len() <= 1);
                    let Some(row) = page.candidates.first() else {
                        break;
                    };
                    assert_eq!(row.source_repo_id.as_str(), source);
                    assert_eq!(row.repo_id.as_str(), "containing-snapshot");
                    after = Some(LexicalCursor::at(
                        ManifestGeneration::new(1),
                        row.order_key(),
                    ));
                    actual.push(row.repo_relative_path.as_str().to_owned());
                    assert!(actual.len() <= expected.len());
                }
                assert_eq!(
                    actual, expected,
                    "source={source} path={path:?} manual={manual} projection={projection:?}"
                );
            }
        }
    }
    Ok(())
}
