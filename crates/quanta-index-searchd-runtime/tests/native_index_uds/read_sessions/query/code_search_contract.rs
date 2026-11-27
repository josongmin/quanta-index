//! Default product code search through the public Native SDK and daemon.
//!
//! Every expected file set below was derived by hand from `CORPUS` and
//! cross-checked with `rg -l` (`-i` for the folded default, `-s` for
//! `case:yes`, `-F` for literals) over the same bytes; none comes from the
//! Native implementation. The Native result unit is one repository/path
//! group, so a file with several matches is counted once.

use super::*;
use quanta_index_contract::SearchPlaneErrorCodeV2;
use quanta_index_contract::native_protocol::{NativeTextPathPageV1, NativeTextPathResultV1};
use quanta_index_sdk::native_uds::native_code_search_request;

const CORPUS: [(&str, &str); 13] = [
    ("src/lib.rs", "todo!()\nfn main() {}\n"),
    ("src/alpha.rs", "sphinx of quartz\n"),
    ("src/beta.rs", "sphinx riddles\n"),
    (
        "src/context.rs",
        "struct WithContext;\nimpl WithContext {\n    fn new() -> Self { Self }\n}\n",
    ),
    ("src/lower.rs", "let context = load();\n"),
    (
        "src/paths.rs",
        "use foo::bar;\nfn run() { foo::bar::go(); }\n",
    ),
    ("src/spaced.rs", "let greeting = \"hello brave world\";\n"),
    ("src/gapped.rs", "hello world, be brave\n"),
    ("pages/p0.rs", "pagemark 0\n"),
    ("pages/p1.rs", "pagemark 1\n"),
    ("pages/p2.rs", "pagemark 2\n"),
    ("pages/p3.rs", "pagemark 3\n"),
    ("pages/p4.rs", "pagemark 4\n"),
];

/// `select:path` groups need authenticated repository-scoped Sources.
const REPOSITORY: &str = "corp-code-search";

const PAGES: [&str; 5] = [
    "pages/p0.rs",
    "pages/p1.rs",
    "pages/p2.rs",
    "pages/p3.rs",
    "pages/p4.rs",
];

struct Corpus {
    view: quanta_index_contract::index::ViewRef,
    sources: Vec<(&'static str, ObjectRef)>,
}

/// Publish the whole fixture as one Source advance, one owner per file.
fn publish_corpus<P: AsRef<std::path::Path>>(client: &NativeUdsClient<P>) -> TestResult<Corpus> {
    let configured = create_text_workspace(client)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let mut sources = Vec::new();
    let mut mutations = Vec::new();
    for (stable_id, (path, text)) in (1_u8..).zip(CORPUS) {
        let NativeIndexResponse::StagedSources(staged) = client.request(
            NativeIndexRequest::StageRepositorySources {
                ingress,
                repository: quanta_index_contract::RepoId::try_from(REPOSITORY)?,
                inputs: vec![SourceInput {
                    path: path.into(),
                    bytes: text.as_bytes().to_vec(),
                    text: SourceTextAdmission::Utf8 {
                        policy: Digest([31; 32]),
                    },
                    language: Some(quanta_index_contract::lex::LanguageCode::new("rust")?),
                }],
            },
            &budget(),
        )?
        else {
            return Err("code-search fixture input was not staged".into());
        };
        let [source] = staged.as_slice() else {
            return Err("one fixture input must stage one Source".into());
        };
        let source = *source;
        mutations.push(OwnerMutation {
            owner: OwnerKey {
                producer: ingress.operation.producer,
                stable_id: StableOwnerId([stable_id; 16]),
            },
            expected: None,
            next: Some(source),
        });
        sources.push((path, source));
    }
    let request = prepare_text_mutations_in_ingress(client, configured, ingress, mutations)?;
    let view = published(&client.submit(request, &budget())?)?;
    Ok(Corpus { view, sources })
}

fn with_corpus(
    check: impl FnOnce(&NativeUdsClient<std::path::PathBuf>, &Corpus) -> TestResult,
) -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket.clone());
        let corpus = publish_corpus(&client)?;
        check(&client, &corpus)
    })();
    let stopped = running.stop();
    primary?;
    stopped
}

#[expect(
    clippy::result_large_err,
    reason = "callers assert the public SDK's own typed error variants"
)]
fn code_search<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    corpus: &Corpus,
    query: &str,
    top_k: u32,
) -> Result<NativeTextPathResultV1, NativeUdsError> {
    client.search_text_paths(
        ViewSelection::Exact(corpus.view),
        native_text_spec().instance,
        native_code_search_request(query, top_k),
        &budget(),
    )
}

/// Paths of one exhaustive file result, refusing duplicate groups.
fn files(query: &str, result: &NativeTextPathResultV1) -> TestResult<Vec<String>> {
    let ranking = &result.ranked.ranking;
    if !ranking.exhaustive || ranking.hits.len() != result.ranked.repositories.len() {
        return Err(format!("{query}: incomplete file result {result:?}").into());
    }
    let mut paths = ranking
        .hits
        .iter()
        .map(|hit| hit.path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    let distinct = paths.len();
    paths.dedup();
    if paths.len() != distinct {
        return Err(format!("{query}: duplicate file rows {paths:?}").into());
    }
    Ok(paths)
}

fn assert_files<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    corpus: &Corpus,
    query: &str,
    expected: &[&str],
) -> TestResult {
    let result = code_search(client, corpus, query, 32)?;
    assert_eq!(result.ranked.ranking.view, corpus.view, "{query}");
    assert_eq!(files(query, &result)?, expected, "{query}");
    assert_eq!(
        result.ranked.ranking.matched,
        u64::try_from(expected.len())?,
        "{query}: matched counts distinct files"
    );
    Ok(())
}

fn page_paths(page: &NativeTextPathPageV1) -> Vec<String> {
    page.ranked
        .ranking
        .hits
        .iter()
        .map(|hit| hit.path.clone())
        .collect()
}

#[test]
fn code_search_reproduces_the_legacy_frontdoor_file_cursor_and_regex_contract() -> TestResult {
    with_corpus(|client, corpus| {
        // Terms in different lines of one Source are one file result.
        let joined = code_search(client, corpus, "todo main", 10)?;
        assert_eq!(files("todo main", &joined)?, ["src/lib.rs"]);
        let [hit] = joined.ranked.ranking.hits.as_slice() else {
            return Err("todo main must return one file".into());
        };
        let lib_source = corpus
            .sources
            .iter()
            .find(|(path, _)| *path == "src/lib.rs")
            .map(|(_, source)| *source)
            .ok_or("fixture lost src/lib.rs")?;
        assert_eq!(
            hit.source, lib_source,
            "file identity is the published Source"
        );
        assert_eq!(
            joined
                .ranked
                .repositories
                .iter()
                .map(quanta_index_contract::RepoId::as_str)
                .collect::<Vec<_>>(),
            [REPOSITORY]
        );

        assert_files(client, corpus, "sphinx", &["src/alpha.rs", "src/beta.rs"])?;

        // Cursor pages: one file per page, bound to the retained view.
        let session =
            client.open_read_session(ViewSelection::Exact(corpus.view), [61; 32], &budget())?;
        let instance = native_text_spec().instance;
        let first = client.search_text_path_page_in_read_session(
            &session,
            instance,
            native_code_search_request("sphinx", 1),
            None,
            &budget(),
        )?;
        assert_eq!(first.ranked.ranking.view, corpus.view);
        assert_eq!((first.ranked.ranking.matched, first.remaining), (2, 2));
        assert_eq!(first.ranked.ranking.hits.len(), 1);
        let cursor = first
            .after
            .clone()
            .ok_or("first file page lacks continuation")?;
        let second = client.search_text_path_page_in_read_session(
            &session,
            instance,
            native_code_search_request("sphinx", 1),
            Some(cursor),
            &budget(),
        )?;
        assert_eq!((second.ranked.ranking.matched, second.remaining), (2, 1));
        assert_eq!(second.ranked.ranking.hits.len(), 1);
        assert!(second.after.is_none(), "the second file page is the last");
        let mut both = page_paths(&first);
        both.extend(page_paths(&second));
        both.sort();
        assert_eq!(both, ["src/alpha.rs", "src/beta.rs"]);
        let (released, _) = client.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);

        for (query, expected) in [
            ("regex:/sphinx.*quartz/", "src/alpha.rs"),
            ("content:regex:/sphinx.*riddles/", "src/beta.rs"),
            ("sphinx path:regex:/beta[.]rs/", "src/beta.rs"),
            ("sphinx regex:/quartz/", "src/alpha.rs"),
        ] {
            assert_files(client, corpus, query, &[expected])?;
        }
        // The Legacy path-only query, and terms that match only a path.
        for (query, expected) in [
            ("path:regex:/beta[.]rs/", &["src/beta.rs"][..]),
            // A bare regex matches a body or a path; no body has `beta.rs`.
            ("regex:/beta[.]rs/", &["src/beta.rs"]),
            ("case:yes regex:/BETA[.]rs/", &[]),
            ("regex:/BETA[.]rs/", &["src/beta.rs"]),
            ("path:alpha", &["src/alpha.rs"]),
            ("path:PAGES", &PAGES),
            ("case:yes path:PAGES", &[]),
            // No Source body contains `alpha` or `pages`; their paths do.
            ("alpha", &["src/alpha.rs"]),
            ("pages", &PAGES),
            ("case:yes pages", &PAGES),
            ("alpha sphinx", &["src/alpha.rs"]),
        ] {
            assert_files(client, corpus, query, expected)?;
        }
        Ok(())
    })
}

#[test]
fn code_search_literals_case_scopes_and_conjunction_match_independent_goldens() -> TestResult {
    with_corpus(|client, corpus| {
        for (query, expected) in [
            // Case-folded literal substrings; several matches count once.
            ("Context", &["src/context.rs", "src/lower.rs"][..]),
            ("CONTEXT", &["src/context.rs", "src/lower.rs"]),
            ("case:no Context", &["src/context.rs", "src/lower.rs"]),
            ("case:yes Context", &["src/context.rs"]),
            // `context` is in the body of lower.rs and the path of context.rs.
            ("case:yes context", &["src/context.rs", "src/lower.rs"]),
            ("WithContext", &["src/context.rs"]),
            ("foo::bar", &["src/paths.rs"]),
            // A quoted literal is contiguous; bare words only co-occur.
            ("\"hello brave world\"", &["src/spaced.rs"]),
            ("hello brave world", &["src/gapped.rs", "src/spaced.rs"]),
            ("sphinx quartz", &["src/alpha.rs"]),
            ("quartz riddles", &[]),
            // Body regex follows the same case policy.
            (r"regex:/fn\s+main/", &["src/lib.rs"]),
            ("regex:/WITHCONTEXT/", &["src/context.rs"]),
            ("case:yes regex:/WITHCONTEXT/", &[]),
            // Path scopes filter the Source path.
            ("sphinx path:alpha", &["src/alpha.rs"]),
            ("pagemark path:PAGES", &PAGES),
            ("case:yes pagemark path:PAGES", &[]),
            ("pagemark", &PAGES),
        ] {
            assert_files(client, corpus, query, expected)?;
        }

        // top-k limits returned rows; matched stays the full file count.
        let limited = code_search(client, corpus, "pagemark", 2)?;
        assert_eq!(limited.ranked.ranking.matched, 5);
        assert_eq!(limited.ranked.ranking.hits.len(), 2);

        // Page to the last page with no duplicate or omitted file.
        let session =
            client.open_read_session(ViewSelection::Exact(corpus.view), [62; 32], &budget())?;
        let instance = native_text_spec().instance;
        let mut after = None;
        let mut seen = Vec::new();
        let mut shape = Vec::new();
        loop {
            let page = client.search_text_path_page_in_read_session(
                &session,
                instance,
                native_code_search_request("pagemark", 2),
                after.clone(),
                &budget(),
            )?;
            shape.push((
                page.ranked.ranking.hits.len(),
                page.ranked.ranking.matched,
                page.remaining,
            ));
            seen.extend(page_paths(&page));
            after = page.after.clone();
            if after.is_none() || shape.len() > PAGES.len() {
                break;
            }
            // A cursor is bound to its query digest.
            let foreign = client.search_text_path_page_in_read_session(
                &session,
                instance,
                native_code_search_request("pagemark path:pages", 2),
                after.clone(),
                &budget(),
            );
            assert!(
                matches!(foreign, Err(NativeUdsError::Protocol(_))),
                "{foreign:?}"
            );
        }
        assert_eq!(shape, [(2, 5, 5), (2, 5, 3), (1, 5, 1)]);
        let total = seen.len();
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), total, "a file repeated across pages");
        assert_eq!(seen, PAGES);
        let (released, _) = client.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        Ok(())
    })
}

#[test]
fn code_search_unsupported_options_and_scopes_are_typed_errors() -> TestResult {
    with_corpus(|client, corpus| {
        let unsupported_combo = SearchPlaneErrorCodeV2::Lexical(
            quanta_index_contract::lex::LexicalErrorCode::UnsupportedCombo,
        );
        for (query, reason) in [
            ("typo:load_jsom", "typo:"),
            ("components:\"update available\"", "components:"),
            ("lang:rust sphinx", "native syntax"),
            ("symbol:Type", "symbol route"),
            ("sphinx case:maybe", "yes or no"),
        ] {
            let refusal = code_search(client, corpus, query, 10);
            assert!(
                matches!(&refusal, Err(NativeUdsError::QueryRejected { code, message })
                    if *code == unsupported_combo && message.contains(reason)),
                "{query}: {refusal:?}"
            );
        }

        // Five bare terms need five body and five path substrings; Native
        // admits four of each, so the lowering refuses before any I/O.
        let fanout = code_search(client, corpus, "a b c d e", 10);
        assert!(
            matches!(&fanout, Err(NativeUdsError::QueryRejected { code, message })
                if *code == SearchPlaneErrorCodeV2::Lexical(
                    quanta_index_contract::lex::LexicalErrorCode::LimitExceededFanout
                ) && message.contains("at most 4")),
            "{fanout:?}"
        );
        let fanout_bypass = client.request(
            NativeIndexRequest::SearchText {
                selection: ViewSelection::Exact(corpus.view),
                instance: native_text_spec().instance,
                query: native_code_search_request("a b c d e", 10),
            },
            &budget(),
        );
        assert!(
            matches!(&fanout_bypass, Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::InvalidInput
                    && error.message.contains("at most 4")),
            "{fanout_bypass:?}"
        );

        // The daemon refuses the same grammar when the SDK guard is bypassed.
        let bypass = client.request(
            NativeIndexRequest::SearchText {
                selection: ViewSelection::Exact(corpus.view),
                instance: native_text_spec().instance,
                query: native_code_search_request("typo:load_jsom", 10),
            },
            &budget(),
        );
        assert!(
            matches!(&bypass, Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::InvalidInput
                    && error.message.contains("code_search: ")),
            "{bypass:?}"
        );

        // The file unit has no owner-ranked projection.
        let owner_route = client.search_text(
            ViewSelection::Exact(corpus.view),
            native_text_spec().instance,
            native_code_search_request("sphinx", 10),
            &budget(),
        );
        assert!(
            matches!(owner_route, Err(NativeUdsError::Protocol(_))),
            "{owner_route:?}"
        );

        // Native bounds Source-body substring leaves per query; the shared
        // lowering refuses a fifth one before the daemon is asked.
        let wide = code_search(client, corpus, "sphinx of quartz riddles todo", 10);
        assert!(
            matches!(&wide, Err(NativeUdsError::QueryRejected { code, .. })
            if *code == SearchPlaneErrorCodeV2::Lexical(
                quanta_index_contract::lex::LexicalErrorCode::LimitExceededFanout
            )),
            "{wide:?}"
        );

        // The language constraint lowers to per-file `lang:` filters over the
        // Sources' verified language metadata; every fixture Source is Rust.
        for (language, expected) in [
            ("rust", &["src/alpha.rs", "src/beta.rs"][..]),
            ("markdown", &[]),
        ] {
            let mut scoped = native_code_search_request("sphinx", 10);
            scoped.constraints = scoped
                .constraints
                .with_languages([quanta_index_contract::lex::LanguageCode::new(language)?]);
            let result = client.search_text_paths(
                ViewSelection::Exact(corpus.view),
                native_text_spec().instance,
                scoped,
                &budget(),
            )?;
            assert_eq!(files(language, &result)?, expected, "{language}");
        }
        Ok(())
    })
}
