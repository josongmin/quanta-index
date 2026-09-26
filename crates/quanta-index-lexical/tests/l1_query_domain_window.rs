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
        replace_scopes: (0..=matches)
            .rev()
            .map(|index| scope(index, matches, surface))
            .collect::<Result<_, _>>()?,
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
    let (_dir, searcher) = fixture()?;
    let budget = RequestBudgetV1::unbounded();
    for manual in [false, true] {
        for term in ["needle", "absent_term"] {
            let mut request = query(term, manual, false);
            request.options.count = Some(LqCountBound::All);
            for fetch in [0, u32::MAX] {
                let page = LexicalPageSpec::first(fetch);
                require_code(
                    searcher.search_constrained(
                        &request,
                        &QueryConstraintSetV1::unconstrained(),
                        &page,
                        &budget,
                    ),
                    SearchPlaneErrorCodeV2::QueryInternalFetchOutOfRange,
                    "invalid Text fetch",
                )?;
                require_code(
                    searcher.search_symbols_constrained(
                        &request,
                        &QueryConstraintSetV1::unconstrained(),
                        &page,
                        &budget,
                    ),
                    SearchPlaneErrorCodeV2::QueryInternalFetchOutOfRange,
                    "invalid Symbol fetch",
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
