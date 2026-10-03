use super::*;

#[test]
fn rejects_missing_scope_syntax() {
    let parsed = ParsedCommand::parse([
        "semantic",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--query-text",
        "embedding query",
        "--top-k",
        "5",
        "--scope-query",
        "lang:rust",
    ]);
    assert!(parsed.is_err());
    let Err(error) = parsed else {
        return;
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("--scope-syntax"));
}

#[test]
fn parses_repomap_focus_subject() {
    let parsed = parse_focus_subject("subject-1:file");
    assert!(parsed.is_ok());
    let Ok(focus) = parsed else {
        return;
    };
    assert_eq!(focus.subject_identity, "subject-1");
    assert_eq!(focus.subject_doc_type, RepoMapDocType::File);
}

#[test]
fn parses_semantic_query_text() {
    let parsed = ParsedCommand::parse([
        "semantic",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--query-text",
        "1 0 2.5",
        "--top-k",
        "5",
    ]);
    assert!(parsed.is_ok());
    let Ok(parsed) = parsed else {
        return;
    };
    let CliRequest::Semantic(request) = parsed.request else {
        panic!("expected semantic payload");
    };
    assert_eq!(request.query_text, "1 0 2.5".to_string());
}

#[test]
fn parses_hybrid_semantic_query_text() {
    let parsed = ParsedCommand::parse([
        "hybrid-seed",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--lexical-query",
        "needle",
        "--lexical-syntax",
        "native",
        "--semantic-query",
        "1 0 2.5",
        "--top-k",
        "5",
    ]);
    assert!(parsed.is_ok());
    let Ok(parsed) = parsed else {
        return;
    };
    let CliRequest::HybridSeed(request) = parsed.request else {
        panic!("expected hybrid-seed payload");
    };
    assert_eq!(request.semantic_query_text, "1 0 2.5".to_string());
}

#[test]
fn rejects_legacy_semantic_query_vector_flag() {
    let parsed = ParsedCommand::parse([
        "semantic",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--query-vector",
        "1,2,3",
        "--top-k",
        "5",
    ]);
    assert!(parsed.is_err());
    let Err(error) = parsed else {
        return;
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("--query-vector"));
}

#[test]
fn rejects_legacy_semantic_query_handle_flag() {
    let parsed = ParsedCommand::parse([
        "semantic",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--query-vector-handle",
        "emb-123",
        "--top-k",
        "5",
    ]);
    assert!(parsed.is_err());
    let Err(error) = parsed else {
        return;
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("--query-vector-handle"));
}

#[test]
fn rejects_legacy_hybrid_semantic_handle_flag() {
    let parsed = ParsedCommand::parse([
        "hybrid-seed",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--lexical-query",
        "needle",
        "--lexical-syntax",
        "native",
        "--semantic-vector-handle",
        "emb-456",
        "--top-k",
        "5",
    ]);
    assert!(parsed.is_err());
    let Err(error) = parsed else {
        return;
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("--semantic-vector-handle"));
}

// QI-BB-018: the true-hybrid route is reachable from the operator
// surface, with the two lanes' queries and the fused top_k.
#[test]
fn parses_hybrid_subcommand_into_a_hybrid_request() {
    let parsed = ParsedCommand::parse([
        "hybrid",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--syntax",
        "sourcegraph",
        "--query-text",
        "needle",
        "--semantic-query-text",
        "where the needle is kept",
        "--top-k",
        "5",
    ]);
    assert!(parsed.is_ok(), "{parsed:?}");
    let Ok(parsed) = parsed else {
        return;
    };
    assert_eq!(parsed.kind, CommandKind::Hybrid);
    let CliRequest::Hybrid(request) = parsed.request else {
        panic!("expected hybrid payload");
    };
    assert_eq!(request.text_query.syntax, TextQuerySyntax::Sourcegraph);
    assert_eq!(request.text_query.query_text, "needle");
    assert_eq!(request.semantic_query_text, "where the needle is kept");
    assert_eq!(request.top_k, 5);
    assert_eq!(request.text_query.top_k, 5);
    assert_eq!(
        request.generation,
        Some(GenerationPin::new(
            RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
            RevisionId::new("rev").expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(7)
        ))
    );
    for (missing, flag) in [
        (
            vec![
                "--syntax",
                "native",
                "--query-text",
                "needle",
                "--top-k",
                "5",
            ],
            "--semantic-query-text",
        ),
        (
            vec![
                "--syntax",
                "native",
                "--semantic-query-text",
                "x",
                "--top-k",
                "5",
            ],
            "--query-text",
        ),
        (
            vec![
                "--query-text",
                "needle",
                "--semantic-query-text",
                "x",
                "--top-k",
                "5",
            ],
            "--syntax",
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
            "hybrid",
            "--repo-id",
            "repo",
            "--revision-id",
            "rev",
            "--manifest-generation",
            "7",
        ];
        args.extend(missing);
        let refused = ParsedCommand::parse(args);
        let Err(error) = refused else {
            panic!("hybrid without {flag} must be refused");
        };
        assert_eq!(error.exit_code, EXIT_USAGE);
        assert!(error.message.contains(flag), "{}", error.message);
    }
}

#[test]
fn lexical_defaults_to_code_search_without_changing_symbol_syntax() {
    let parsed = ParsedCommand::parse([
        "lexical",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--query-text",
        "writeContentType",
        "--top-k",
        "10",
    ]);
    let parsed = parsed.expect("lexical default request");
    let CliRequest::Lexical(request) = parsed.request else {
        panic!("expected lexical request");
    };
    assert_eq!(request.syntax, TextQuerySyntax::CodeSearch);
    assert_eq!(request.query_text, "writeContentType");

    let refused = ParsedCommand::parse([
        "symbol",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--query-text",
        "writeContentType",
        "--top-k",
        "10",
    ]);
    assert!(refused.is_err_and(|error| error.message.contains("missing --syntax")));
}

#[test]
fn parses_lexical_sourcegraph_query_request() {
    let parsed = ParsedCommand::parse([
        "lexical",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--syntax",
        "sourcegraph",
        "--query-text",
        "repo:repo lang:rust needle",
        "--top-k",
        "11",
    ]);
    assert!(parsed.is_ok());
    let Ok(parsed) = parsed else {
        return;
    };
    let CliRequest::Lexical(request) = parsed.request else {
        panic!("expected lexical text payload");
    };
    assert_eq!(request.syntax, TextQuerySyntax::Sourcegraph);
    assert_eq!(request.query_text.as_str(), "repo:repo lang:rust needle");
    assert_eq!(request.top_k, 11);
    assert_eq!(
        request.generation.map(|pin| pin.manifest_generation.get()),
        Some(7)
    );
}

#[test]
fn explicit_socket_override_builds_sdk_connect_options() {
    let parsed = ParsedCommand::parse([
        "--socket",
        "/tmp/quanta/query.sock",
        "lexical",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--syntax",
        "native",
        "--query-text",
        "needle",
        "--top-k",
        "3",
    ]);
    assert!(parsed.is_ok());
    let Ok(parsed) = parsed else {
        return;
    };
    assert_eq!(
        parsed.connect_options,
        ConnectOptions::default()
            .with_query_socket("/tmp/quanta/query.sock")
            .with_control_socket("/tmp/quanta/control.sock")
            .with_ingest_socket("/tmp/quanta/ingest.sock")
    );
}

#[test]
fn parses_symbol_query_request() {
    let parsed = ParsedCommand::parse([
        "symbol",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--syntax",
        "native",
        "--query-text",
        "MySymbol",
        "--top-k",
        "5",
    ]);
    assert!(parsed.is_ok());
    let Ok(parsed) = parsed else {
        return;
    };
    let CliRequest::Symbol(request) = parsed.request else {
        panic!("expected symbol payload");
    };
    assert_eq!(request.syntax, TextQuerySyntax::Native);
    assert_eq!(request.query_text.as_str(), "MySymbol");
    assert_eq!(request.top_k, 5);
    assert_eq!(
        request.generation.map(|pin| pin.manifest_generation.get()),
        Some(7)
    );
}

#[test]
fn rejects_symbol_missing_top_k() {
    let parsed = ParsedCommand::parse([
        "symbol",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "7",
        "--syntax",
        "native",
        "--query-text",
        "MySymbol",
    ]);
    assert!(parsed.is_err());
    let Err(error) = parsed else {
        return;
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("--top-k"));
}

#[test]
fn parses_runtime_metadata_query_request() {
    let parsed = ParsedCommand::parse([
        "runtime-metadata",
        "--repo-id",
        "repo",
        "--revision-id",
        "rev",
        "--manifest-generation",
        "9",
        "--syntax",
        "sourcegraph",
        "--query-text",
        "lang:rust runtime",
        "--top-k",
        "3",
    ]);
    assert!(parsed.is_ok());
    let Ok(parsed) = parsed else {
        return;
    };
    let CliRequest::RuntimeMetadata(request) = parsed.request else {
        panic!("expected runtime-metadata payload");
    };
    assert_eq!(request.text_query.syntax, TextQuerySyntax::Sourcegraph);
    assert_eq!(request.text_query.query_text.as_str(), "lang:rust runtime");
    assert_eq!(request.text_query.top_k, 3);
    assert_eq!(
        request
            .text_query
            .generation
            .map(|pin| pin.manifest_generation.get()),
        Some(9)
    );
}

#[test]
fn rejects_unknown_subcommand() {
    let parsed = ParsedCommand::parse(["nonsense"]);
    assert!(parsed.is_err());
    let Err(error) = parsed else {
        return;
    };
    assert_eq!(error.exit_code, EXIT_USAGE);
    assert!(error.message.contains("symbol"));
    assert!(error.message.contains("runtime-metadata"));
}
