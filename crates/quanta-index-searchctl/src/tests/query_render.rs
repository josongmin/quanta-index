use super::*;

#[test]
fn pretty_renderer_supports_symbol_response() {
    use quanta_index_contract::{
        RepoRelativePath, SymbolQueryResponse,
        lex::{SymbolKindCode, SymbolKindFamily},
    };
    let Ok(symbol_kind) = SymbolKindCode::new("function") else {
        return;
    };
    let response = SearchPlaneQueryIpcResponseEnvelope {
        request_id: 1,
        payload: SearchPlaneQueryIpcResponse::Symbol(SymbolQueryResponse {
            generation: GenerationPin::new(
                RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(7),
            ),
            results: vec![SymbolCandidate {
                source_repo_id: RepoId::new("repo")
                    .expect("static fixture ID satisfies canonical policy"),
                source: None,
                preview: None,
                candidate_id: "sym-1".to_string(),
                repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                revision_id: RevisionId::new("rev")
                    .expect("static fixture ID satisfies canonical policy"),
                manifest_generation: ManifestGeneration::new(7),
                repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                start_line: 10,
                end_line: 12,
                score: 0.8,
                snippet: "fn my_symbol() {}".to_string(),
                symbol_kind,
                symbol_kind_family: Some(SymbolKindFamily::Callable),
            }],
            window: QueryResultWindowV2::exact_probe(1),
            next_cursor: None,
        }),
    };
    let mut stdout = Vec::new();
    let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
    assert!(rendered.is_ok());
    let text = String::from_utf8(stdout);
    assert!(text.is_ok());
    if let Ok(text) = text {
        assert!(text.contains("kind: symbol"));
        assert!(text.contains("symbol_kind=function"));
        assert!(text.contains("symbol_kind_family=Callable"));
    }
}

#[test]
fn pretty_renderer_supports_runtime_metadata_response() {
    use quanta_index_contract::RepoRelativePath;
    // One row returned of at least two: the continuation probe saw a
    // second match past the page.
    let probe_window = QueryResultWindowV2::pageable(
        1,
        quanta_index_contract::CandidateCountV1::AtLeast(2),
        true,
        Vec::new(),
    );
    assert!(probe_window.is_ok(), "{probe_window:?}");
    let Ok(probe_window) = probe_window else {
        return;
    };
    let response = SearchPlaneQueryIpcResponseEnvelope {
        request_id: 1,
        payload: SearchPlaneQueryIpcResponse::RuntimeMetadata(
            SearchPlaneRuntimeMetadataQueryResponse {
                generation: GenerationPin::new(
                    RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                    RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                    ManifestGeneration::new(7),
                ),
                results: vec![LexicalCandidate {
                    source_repo_id: RepoId::new("repo")
                        .expect("static fixture ID satisfies canonical policy"),
                    source: None,
                    preview: None,
                    candidate_id: "rt-1".to_string(),
                    repo_id: RepoId::new("repo")
                        .expect("static fixture ID satisfies canonical policy"),
                    revision_id: RevisionId::new("rev")
                        .expect("static fixture ID satisfies canonical policy"),
                    manifest_generation: ManifestGeneration::new(7),
                    repo_relative_path: RepoRelativePath::new("src/runtime.rs"),
                    start_line: 1,
                    end_line: 4,
                    score: 0.3,
                    snippet: "runtime body".to_string(),
                    snippet_hit_offset: None,
                    highlights: Vec::new(),
                }],
                window: probe_window,
                read_epoch: AuxEpochV1::new(4),
                universe_epoch: AuxEpochV1::new(9),
                examined: 2,
                next_cursor: Some(
                    ContinuationTokenV2::new("runtime-token".to_string()).expect("token"),
                ),
            },
        ),
    };
    let mut stdout = Vec::new();
    let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
    assert!(rendered.is_ok());
    let text = String::from_utf8(stdout);
    assert!(text.is_ok());
    if let Ok(text) = text {
        assert!(text.contains("kind: runtime-metadata"));
        assert!(text.contains("results: 1"));
        assert!(
            text.contains("epoch: 4"),
            "the read epoch is rendered: {text}"
        );
        assert!(
            text.contains("universe_epoch: 9"),
            "the universe epoch is rendered: {text}"
        );
        assert!(
            text.contains("order: candidate_id matched: >=2 examined: 2 has_more: true"),
            "the probe window is rendered: {text}"
        );
        assert!(
            text.contains("next_cursor: \"runtime-token\""),
            "the continuation is rendered: {text}"
        );
    }
}

#[test]
fn pretty_renderer_supports_sourcegraph_text_response() {
    let response = SearchPlaneQueryIpcResponseEnvelope {
        request_id: 1,
        payload: SearchPlaneQueryIpcResponse::Text(TextQueryResponse {
            explanation: quanta_index_contract::SearchExplanation::empty(),
            rank_unit: TextRankUnit::Chunk,
            generation: GenerationPin::new(
                RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(7),
            ),
            results: vec![LexicalCandidate {
                source_repo_id: RepoId::new("repo")
                    .expect("static fixture ID satisfies canonical policy"),
                source: None,
                preview: None,
                candidate_id: "cand-1".to_string(),
                repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                revision_id: RevisionId::new("rev")
                    .expect("static fixture ID satisfies canonical policy"),
                manifest_generation: ManifestGeneration::new(7),
                repo_relative_path: quanta_index_contract::RepoRelativePath::new("src/lib.rs"),
                start_line: 1,
                end_line: 3,
                score: 0.5,
                snippet: "fn sample() {}".to_string(),
                snippet_hit_offset: None,
                highlights: Vec::new(),
            }],
            window: QueryResultWindowV2::exact_probe(1),
            file_owner_rows: Some(vec![quanta_index_contract::FileOwnerProjectionRow {
                source_repo_id: RepoId::new("repo")
                    .expect("static fixture ID satisfies canonical policy"),
                candidate_id: "cand-1".to_string(),
                repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                revision_id: RevisionId::new("rev")
                    .expect("static fixture ID satisfies canonical policy"),
                manifest_generation: ManifestGeneration::new(7),
                repo_relative_path: quanta_index_contract::RepoRelativePath::new("src/lib.rs"),
                owners: vec!["@alice".to_string(), "@acme/platform".to_string()],
            }]),
            next_cursor: None,
        }),
    };
    let mut stdout = Vec::new();
    let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
    assert!(rendered.is_ok());
    let text = String::from_utf8(stdout);
    assert!(text.is_ok());
    if let Ok(text) = text {
        assert!(text.contains("kind: lexical"));
        assert!(text.contains("rank_unit: chunk"));
        assert!(text.contains("results: 1"));
        assert!(text.contains("file_owner_rows: 1"));
        assert!(text.contains("owners=@alice,@acme/platform"));
    }
}

#[test]
fn parses_history_query_request() {
    let parsed = ParsedCommand::parse([
        "history",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--syntax",
        "native",
        "--query-text",
        "feat: add x",
        "--top-k",
        "9",
        "--order",
        "relevance",
    ]);
    assert!(parsed.is_ok());
    let Ok(parsed) = parsed else {
        return;
    };
    let CliRequest::History(request) = parsed.request else {
        panic!("expected history payload");
    };
    assert_eq!(request.text_query.syntax, TextQuerySyntax::Native);
    assert_eq!(request.text_query.query_text.as_str(), "feat: add x");
    assert_eq!(request.text_query.top_k, 9);
    assert_eq!(request.order, HistoryOrderV1::Relevance);
    assert!(request.cursor.is_none());
    assert_eq!(
        request
            .text_query
            .generation
            .map(|pin| pin.manifest_generation.get()),
        Some(7)
    );
}

/// The history order is required and closed: no flag or an unknown
/// value is a usage error, never a default.
#[test]
fn history_requires_a_known_order() {
    let base = [
        "history",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--syntax",
        "native",
        "--query-text",
        "feat: add x",
        "--top-k",
        "9",
    ];
    let missing = ParsedCommand::parse(base);
    let Err(error) = missing else {
        panic!("history without --order must be a usage error");
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("--order"), "{}", error.message);

    let mut unknown: Vec<&str> = base.to_vec();
    unknown.extend(["--order", "newest"]);
    let Err(error) = ParsedCommand::parse(unknown) else {
        panic!("an unknown order must be a usage error");
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("newest"), "{}", error.message);

    let mut recency: Vec<&str> = base.to_vec();
    recency.extend(["--order", "recency"]);
    let parsed = ParsedCommand::parse(recency).expect("recency parses");
    let CliRequest::History(request) = parsed.request else {
        panic!("expected history payload");
    };
    assert_eq!(request.order, HistoryOrderV1::Recency);
}

#[test]
fn parses_structural_query_request() {
    let parsed = ParsedCommand::parse([
        "structural",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--syntax",
        "sourcegraph",
        "--query-text",
        "lang:rust fn $NAME(...) {...}",
        "--top-k",
        "4",
    ]);
    assert!(parsed.is_ok());
    let Ok(parsed) = parsed else {
        return;
    };
    let CliRequest::Structural(request) = parsed.request else {
        panic!("expected structural payload");
    };
    assert_eq!(request.text_query.syntax, TextQuerySyntax::Sourcegraph);
    assert_eq!(
        request.text_query.query_text.as_str(),
        "lang:rust fn $NAME(...) {...}"
    );
    assert_eq!(request.text_query.top_k, 4);
}

#[test]
fn rejects_history_missing_top_k() {
    let parsed = ParsedCommand::parse([
        "history",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--syntax",
        "native",
        "--query-text",
        "feat",
    ]);
    assert!(parsed.is_err());
    let Err(error) = parsed else {
        return;
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("--top-k"));
}

#[test]
fn rejects_structural_missing_query_text() {
    let parsed = ParsedCommand::parse([
        "structural",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--syntax",
        "native",
        "--top-k",
        "4",
    ]);
    assert!(parsed.is_err());
    let Err(error) = parsed else {
        return;
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("--query-text"));
}

#[test]
fn pretty_renderer_supports_history_response() {
    let response = SearchPlaneQueryIpcResponseEnvelope {
        request_id: 1,
        payload: SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
            generation: GenerationPin::new(
                RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(7),
            ),
            order: HistoryOrderV1::Recency,
            commits: vec![quanta_index_contract::CommitCandidate {
                sha: quanta_index_contract::lex::CommitSha::ZERO,
                parent_ids: Vec::new(),
                committed_at_unix_s: 1_700_000_000,
                author: "alice".to_string(),
                committer: "alice".to_string(),
                message: "fix: thing\nbody line".to_string(),
                is_merge: false,
                tags: vec!["v1.0".to_string()],
                score: None,
            }],
            diffs: Vec::new(),
            window: QueryResultWindowV2::pageable(
                1,
                quanta_index_contract::CandidateCountV1::AtLeast(3),
                true,
                Vec::new(),
            )
            .expect("a page of one out of three"),
            read_epoch: AuxEpochV1::new(12),
            examined: 9,
            next_cursor: Some(
                ContinuationTokenV2::new("history-recency-token".to_string()).expect("token"),
            ),
        }),
    };
    let mut stdout = Vec::new();
    let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
    assert!(rendered.is_ok());
    let text = String::from_utf8(stdout).expect("utf-8");
    assert!(
        text.contains("order: recency matched: >=3 examined: 9 has_more: true"),
        "{text}"
    );
    assert!(
        text.contains("next_cursor: \"history-recency-token\""),
        "{text}"
    );
    assert!(
        !text.contains("score="),
        "a recency page renders no score: {text}"
    );
    assert!(
        text.contains("epoch: 12"),
        "the read epoch is rendered: {text}"
    );
    assert!(text.contains("kind: history"));
    assert!(text.contains("commits: 1 diffs: 0"));
    assert!(text.contains("author=alice"));
}

/// A relevance page says so and prints each row's score and the
/// cursor's score.
#[test]
fn pretty_renderer_prints_relevance_scores() {
    let score = quanta_index_contract::HistoryScoreV1::try_new(1.5).expect("finite");
    let response = SearchPlaneQueryIpcResponseEnvelope {
        request_id: 1,
        payload: SearchPlaneQueryIpcResponse::History(SearchPlaneHistoryQueryResponse {
            generation: GenerationPin::new(
                RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(7),
            ),
            order: HistoryOrderV1::Relevance,
            commits: Vec::new(),
            diffs: vec![quanta_index_contract::DiffCandidate {
                repo_relative_path: "src/lib.rs".to_string(),
                hunk_header: "@@ -1 +1 @@".to_string(),
                side: quanta_index_contract::DiffHunkSide::After,
                line_start: 1,
                line_end: 2,
                snippet: "needle".to_string(),
                score: Some(score),
            }],
            window: QueryResultWindowV2::pageable(
                1,
                quanta_index_contract::CandidateCountV1::AtLeast(2),
                true,
                Vec::new(),
            )
            .expect("a page of one out of two"),
            read_epoch: AuxEpochV1::new(3),
            examined: 4,
            next_cursor: Some(
                ContinuationTokenV2::new("history-relevance-token".to_string()).expect("token"),
            ),
        }),
    };
    let mut stdout = Vec::new();
    let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
    assert!(rendered.is_ok());
    let text = String::from_utf8(stdout).expect("utf-8");
    assert!(
        text.contains("order: relevance matched: >=2 examined: 4 has_more: true"),
        "{text}"
    );
    assert!(
        text.contains("1. score=1.5 path=src/lib.rs"),
        "each row carries its score: {text}"
    );
    assert!(
        text.contains("next_cursor: \"history-relevance-token\""),
        "the opaque cursor is rendered: {text}"
    );
}
