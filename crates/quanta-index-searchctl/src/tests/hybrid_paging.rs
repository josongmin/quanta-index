use super::*;
use std::fs;

#[test]
fn pretty_renderer_supports_structural_response() {
    let response = SearchPlaneQueryIpcResponseEnvelope {
        request_id: 1,
        payload: SearchPlaneQueryIpcResponse::Structural(SearchPlaneStructuralQueryResponse {
            generation: GenerationPin::new(
                RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(7),
            ),
            results: vec![quanta_index_contract::StructuralCandidate {
                candidate_id: "struct-1".to_string(),
                bindings: vec![quanta_index_contract::StructuralBinding {
                    metavariable: "$NAME".to_string(),
                    start_byte: 10,
                    end_byte: 14,
                    start_line: 2,
                    end_line: 2,
                }],
            }],
            window: QueryResultWindowV2::exact_probe(1),
            read_epoch: AuxEpochV1::new(3),
            examined: 1,
            next_cursor: None,
        }),
    };
    let mut stdout = Vec::new();
    let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
    assert!(rendered.is_ok());
    let text = String::from_utf8(stdout);
    assert!(text.is_ok());
    if let Ok(text) = text {
        assert!(text.contains("kind: structural"));
        assert!(
            text.contains("epoch: 3"),
            "the read epoch is rendered: {text}"
        );
        assert!(text.contains("results: 1"));
        assert!(text.contains("candidate_id=struct-1"));
        assert!(text.contains("$NAME: bytes=10-14"));
        assert!(
            text.contains("order: candidate_id matched: 1 examined: 1 has_more: false"),
            "the exact window is rendered: {text}"
        );
        assert!(
            !text.contains("next_cursor:"),
            "a final page prints no continuation: {text}"
        );
    }
}

fn sample_candidate(id: &str, score: f32) -> LexicalCandidate {
    LexicalCandidate {
        source_repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
        source: None,
        preview: None,
        candidate_id: id.to_string(),
        repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
        manifest_generation: ManifestGeneration::new(7),
        repo_relative_path: quanta_index_contract::RepoRelativePath::new("src/lib.rs"),
        start_line: 1,
        end_line: 3,
        score,
        snippet: "fn sample() {}".to_string(),
        snippet_hit_offset: None,
        highlights: Vec::new(),
    }
}

/// A hybrid row both lanes saw: the lexical lane's row at lexical rank
/// 1 and dense rank 2, with the RRF score of those ranks.
fn sample_hybrid_candidate() -> HybridCandidateV1 {
    HybridCandidateV1 {
        candidate: sample_candidate("cand-1", 2.5),
        fused_score: 1.0 / 61.0 + 1.0 / 62.0,
        contributions: vec![
            quanta_index_contract::HybridLaneContributionV1 {
                lane: quanta_index_contract::HybridLaneV1::Lexical,
                rank: 1,
                raw_score: 2.5,
            },
            quanta_index_contract::HybridLaneContributionV1 {
                lane: quanta_index_contract::HybridLaneV1::Dense,
                rank: 2,
                raw_score: 0.75,
            },
        ],
    }
}

// QI-BB-022: one line per fused row names the RRF score and each lane's
// rank and raw score; the JSON path is the wire DTO itself.
#[test]
fn pretty_renderer_supports_hybrid_response_with_lane_provenance() {
    let response = SearchPlaneQueryIpcResponseEnvelope {
        request_id: 1,
        payload: SearchPlaneQueryIpcResponse::Hybrid(HybridQueryResponse {
            selected_active_head: None,
            generation: GenerationPin::new(
                RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
                RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
                ManifestGeneration::new(7),
            ),
            results: vec![
                sample_hybrid_candidate(),
                HybridCandidateV1 {
                    candidate: sample_candidate("cand-2", 0.5),
                    fused_score: 1.0 / 61.0,
                    contributions: vec![quanta_index_contract::HybridLaneContributionV1 {
                        lane: quanta_index_contract::HybridLaneV1::Dense,
                        rank: 1,
                        raw_score: 0.5,
                    }],
                },
            ],
            window: QueryResultWindowV2::exact_probe(2),
            explanation: SearchExplanation {
                planner_trace: Vec::new(),
                engines_touched: vec![EngineTouched::Lexical, EngineTouched::Semantic],
                engines_executed: vec![EngineTouched::Lexical, EngineTouched::Semantic],
                request_id: 0,
                stage_timings: None,
                early_stop_reason: None,
                contributions: Vec::new(),
                ranker_weights_hash: [0u8; 32],
                strategy: "rrf".to_string(),
                summary: "two lanes".to_string(),
            },
        }),
    };
    let mut stdout = Vec::new();
    let rendered = render_response(OutputMode::Pretty, &response, &mut stdout);
    assert!(rendered.is_ok());
    let text = String::from_utf8(stdout);
    assert!(text.is_ok());
    if let Ok(text) = text {
        assert!(text.contains("kind: hybrid"), "{text}");
        assert!(text.contains("results: 2"), "{text}");
        assert!(
                text.contains(&format!(
                    "1. candidate_id=cand-1 path=src/lib.rs lines=1-3 score=2.5 fused={} lanes=lexical#1(2.5) dense#2(0.75)",
                    1.0_f64 / 61.0 + 1.0 / 62.0
                )),
                "{text}"
            );
        assert!(
                text.contains(&format!(
                    "2. candidate_id=cand-2 path=src/lib.rs lines=1-3 score=0.5 fused={} lanes=dense#1(0.5)",
                    1.0_f64 / 61.0
                )),
                "{text}"
            );
        assert!(text.contains("strategy: rrf"), "{text}");
    }
    let mut json = Vec::new();
    let rendered = render_response(OutputMode::Json, &response, &mut json);
    assert!(rendered.is_ok());
    let decoded: Result<SearchPlaneQueryIpcResponseEnvelope, _> = serde_json::from_slice(&json);
    assert!(
        decoded.is_ok(),
        "the JSON output is the wire DTO: {decoded:?}"
    );
    if let Ok(decoded) = decoded {
        assert_eq!(decoded, response);
    }
}

// A hybrid row is explained from the JSON the hybrid route emitted for
// it, and only under its query.
#[test]
fn parses_explain_with_a_hybrid_candidate_json_under_its_query() {
    let dir = tempfile::tempdir();
    assert!(dir.is_ok());
    let Ok(dir) = dir else {
        return;
    };
    let path = dir.path().join("hybrid-candidate.json");
    let written = serde_json::to_vec_pretty(&sample_hybrid_candidate())
        .map_err(|err| err.to_string())
        .and_then(|bytes| fs::write(&path, bytes).map_err(|err| err.to_string()));
    assert!(written.is_ok(), "{written:?}");
    let path = path.to_string_lossy().into_owned();
    let parsed = ParsedCommand::parse([
        "explain",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--hybrid-candidate-json",
        path.as_str(),
        "--syntax",
        "native",
        "--query-text",
        "needle",
        "--semantic-query-text",
        "where the needle is kept",
        "--top-k",
        "10",
    ]);
    assert!(parsed.is_ok(), "{parsed:?}");
    let Ok(parsed) = parsed else {
        return;
    };
    let CliRequest::Explain {
        candidate,
        text_query,
        semantic_query_text,
        ..
    } = parsed.request
    else {
        panic!("expected explain payload");
    };
    assert_eq!(
        *candidate,
        ExplainCandidateV1::Hybrid(sample_hybrid_candidate())
    );
    let Some(text_query) = text_query else {
        panic!("a hybrid explain carries its text query");
    };
    assert_eq!(text_query.query_text, "needle");
    assert_eq!(
        text_query.top_k, 10,
        "the fused top_k sizes the re-run lanes"
    );
    assert_eq!(
        semantic_query_text.as_deref(),
        Some("where the needle is kept")
    );

    // Each of the three hybrid requirements is refused by name.
    for (dropped, flag) in [
        (
            vec!["--semantic-query-text", "x", "--top-k", "10"],
            "--query-text",
        ),
        (
            vec![
                "--syntax",
                "native",
                "--query-text",
                "needle",
                "--top-k",
                "10",
            ],
            "--semantic-query-text",
        ),
        (
            vec![
                "--syntax",
                "native",
                "--query-text",
                "needle",
                "--semantic-query-text",
                "x",
            ],
            "--top-k",
        ),
    ] {
        let mut args = vec![
            "explain",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
            "--hybrid-candidate-json",
            path.as_str(),
        ];
        args.extend(dropped);
        let refused = ParsedCommand::parse(args);
        let Err(error) = refused else {
            panic!("a hybrid explain without {flag} must be refused");
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains(flag), "{}", error.message);
    }

    let both = ParsedCommand::parse([
        "explain",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--candidate-json",
        path.as_str(),
        "--hybrid-candidate-json",
        path.as_str(),
    ]);
    assert!(both.is_err());
    if let Err(error) = both {
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(
            error.message.contains("mutually exclusive"),
            "{}",
            error.message
        );
    }
}
/// A keyset route continues from the cursor a previous page printed
/// as JSON (QI-BB-025 W4).
///
/// `--cursor-json` carries the opaque token unchanged. Binding to a
/// route is verified by the daemon, not by the CLI JSON parser.
#[test]
fn parses_keyset_routes_with_a_cursor_json_continuation() {
    let dir = tempfile::tempdir();
    assert!(dir.is_ok());
    let Ok(dir) = dir else {
        return;
    };
    let runtime_cursor = ContinuationTokenV2::new("runtime-token".to_string()).expect("token");
    let structural_cursor =
        ContinuationTokenV2::new("structural-token".to_string()).expect("token");
    let history_cursor = ContinuationTokenV2::new("history-token".to_string()).expect("token");
    let write = |name: &str, json: Result<Vec<u8>, serde_json::Error>| -> String {
        let path = dir.path().join(name);
        let written = json
            .map_err(|err| err.to_string())
            .and_then(|bytes| fs::write(&path, bytes).map_err(|err| err.to_string()));
        assert!(written.is_ok(), "{written:?}");
        path.to_string_lossy().into_owned()
    };
    let runtime_path = write(
        "runtime-cursor.json",
        serde_json::to_vec_pretty(&runtime_cursor),
    );
    let structural_path = write(
        "structural-cursor.json",
        serde_json::to_vec_pretty(&structural_cursor),
    );
    let history_path = write(
        "history-cursor.json",
        serde_json::to_vec_pretty(&history_cursor),
    );
    let malformed_path = dir.path().join("malformed.json");
    let written = fs::write(&malformed_path, b"{ not json");
    assert!(written.is_ok(), "{written:?}");
    let malformed_path = malformed_path.to_string_lossy().into_owned();

    let common = [
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "9",
        "--syntax",
        "native",
        "--query-text",
        "dirty:yes needle",
        "--top-k",
        "10",
    ];
    let with_cursor = |command: &str, path: &str| {
        let mut args = vec![command.to_string()];
        args.extend(common.iter().map(ToString::to_string));
        if command == "history" {
            // The history route names its order; a recency cursor
            // continues a recency walk.
            args.push("--order".to_string());
            args.push("recency".to_string());
        }
        args.push("--cursor-json".to_string());
        args.push(path.to_string());
        ParsedCommand::parse(args)
    };

    let parsed = with_cursor("runtime-metadata", &runtime_path);
    assert!(parsed.is_ok(), "{parsed:?}");
    if let Ok(parsed) = parsed {
        let CliRequest::RuntimeMetadata(request) = parsed.request else {
            panic!("expected runtime-metadata payload");
        };
        assert_eq!(request.cursor, Some(runtime_cursor));
    }
    let parsed = with_cursor("structural", &structural_path);
    assert!(parsed.is_ok(), "{parsed:?}");
    if let Ok(parsed) = parsed {
        let CliRequest::Structural(request) = parsed.request else {
            panic!("expected structural payload");
        };
        assert_eq!(request.cursor, Some(structural_cursor));
    }
    let parsed = with_cursor("history", &history_path);
    assert!(parsed.is_ok(), "{parsed:?}");
    if let Ok(parsed) = parsed {
        let CliRequest::History(request) = parsed.request else {
            panic!("expected history payload");
        };
        assert_eq!(request.cursor, Some(history_cursor));
    }

    // A fresh walk carries no cursor.
    let mut fresh = vec!["structural".to_string()];
    fresh.extend(common.iter().map(ToString::to_string));
    let parsed = ParsedCommand::parse(fresh);
    assert!(parsed.is_ok(), "{parsed:?}");
    if let Ok(parsed) = parsed {
        let CliRequest::Structural(request) = parsed.request else {
            panic!("expected structural payload");
        };
        assert_eq!(request.cursor, None);
    }

    // Route binding is server-side; CLI must not inspect an opaque token.
    assert!(with_cursor("structural", &runtime_path).is_ok());
    assert!(with_cursor("runtime-metadata", &structural_path).is_ok());
    // A malformed JSON document is still a local usage error.
    let (command, path) = ("history", malformed_path.as_str());
    let parsed = with_cursor(command, path);
    assert!(parsed.is_err(), "{command}: {parsed:?}");
    if let Err(error) = parsed {
        assert_eq!(error.exit_code, EXIT_USAGE, "{command}: {error:?}");
        assert!(
            error
                .message
                .contains(&format!("failed to decode {command} cursor json")),
            "{command}: {}",
            error.message
        );
    }
}
