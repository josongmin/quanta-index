//! Explicit name/source contracts through the real sealed adapter. Fixed IDs
//! and cardinalities come from the fixture, never a full-search baseline.
#![forbid(unsafe_code)]

use quanta_index_contract::lex::{
    LanguageCode, SymbolKindCode, SymbolRecord, SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{
    BatchIngestMode, ChunkId, ChunkRecord, LQ_VERSION_TAG, LexicalCursor, LqCase, LqCountBound,
    LqExpr, LqFilter, LqLeaf, LqOptions, LqPredicateArg, LqQuery, LqSelect, LqSpan, LqYesNoOnly,
    ManifestGeneration, QueryConstraintSetV1, RepoId, RepoRelativePath, RevisionId,
    SearchCorpusIngestBatch, SearchCorpusReplaceScope, SourceFileCoverage, SourceFileKey,
    SourceFileRevision, SourcePublicationEvent, SymbolCoverage, SymbolId,
    source_event_payload_sha256, source_file_unit_set_sha256,
};
use quanta_index_core::{
    LexicalIndexOpenPort, LexicalPageSpec, LexicalSearcher, RequestBudgetV1,
    SearchCorpusBatchBuildPort,
};
use quanta_index_lexical::LexicalAdapter;
use sha2::{Digest as _, Sha256};
use std::collections::BTreeSet;
use std::error::Error;

type TestResult = Result<(), Box<dyn Error>>;

fn scope(
    owner: &str,
    file: &str,
    definitions: &[(&str, &str, &str, Option<&str>)],
) -> Result<SearchCorpusReplaceScope, Box<dyn Error>> {
    let language = LanguageCode::new("rust").map_err(str::to_string)?;
    let text = "content_only Café One::Café Café::Inner";
    let path = RepoRelativePath::new(file);
    let chunks = vec![ChunkRecord {
        chunk_id: ChunkId::new(format!("chunk-{owner}-{file}")),
        repo_relative_path: path.clone(),
        language: language.clone(),
        start_byte: 0,
        end_byte: u32::try_from(text.len())?,
        start_line: 1,
        end_line: 1,
        text: text.into(),
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
    let searcher = adapter.open(&repo, &revision, ManifestGeneration::new(1))?;
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
fn l3_repo_and_path_scope_precede_source_projection_and_paging() -> TestResult {
    use quanta_index_contract::ExactRepoRelativePathV1;
    let (_dir, searcher) = fixture_with_scopes(vec![
        scope("source-b", "same.rs", &[])?,
        scope("source-a", "same.rs", &[])?,
        scope("source-a", "other.rs", &[])?,
        scope("zero-symbols", "same.rs", &[])?,
    ])?;
    let cases: [(&str, Option<&str>, &[&str], &[&str]); 7] = [
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
