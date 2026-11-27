//! The public SDK `.text()` builder on a Native connection.
//!
//! Every step drives the same builder calls as
//! `legacy_sdk_code_search_frontdoor.rs`. Native addresses a repository
//! revision through its sealed protected Source head instead of a manifest
//! generation, so the response generation names that head's view sequence.
//! Expected file sets are from `CORPUS`. The public syntax goldens were
//! cross-checked with `rg --json --no-config --sort path`, explicit case
//! flags, and an independent check of the fixture paths. The file id is the
//! fixed published hash.

use super::*;
use quanta_index_contract::index::{
    NativeProtectedSourceHeadV1, NativeProtectedSourceRoutesV1,
    native_protected_view_marker_bytes_v1,
};
use quanta_index_contract::lex::{LanguageCode, LexicalErrorCode};
use quanta_index_contract::native_protocol::{
    NativeImmutableObjectChildrenV1, NativeImmutableObjectPayloadV1,
};
use quanta_index_contract::{
    GenerationPin, ManifestGeneration, RepoId, RevisionId, SearchPlaneErrorCodeV2, TextRankUnit,
};
use quanta_index_sdk::{ClientQueryRpcKindV1, ConnectOptions, QuantaIndex, SdkError};
use quanta_index_searchd_runtime::native::NativeRetentionObservation;
use std::collections::BTreeSet;

const REPO: &str = "repo-e2e";
const REVISION: &str = "rev-e2e";
/// `file:` + SHA-256 of the code-search domain and framed `repo-e2e`/`src/lib.rs`.
const LIB_FILE_ID: &str = "file:b62f08ecdde0f0a5645834653f99ee579f13f7e4d8f2744aabb5fc4634ad2ed5";

/// The Legacy fixture's Sources plus language and query syntax witnesses.
const CORPUS: [(&str, &str, &str); 7] = [
    ("src/lib.rs", "rust", "todo!()\nfn main() {}\n"),
    ("src/alpha.rs", "rust", "sphinx of quartz\n"),
    ("src/beta.rs", "rust", "sphinx riddles\n"),
    ("docs/readme.md", "markdown", "quartz notes\n"),
    ("src/context.rs", "rust", "struct WithContext;\n"),
    ("src/lower.rs", "rust", "let context = load();\n"),
    ("src/paths.rs", "rust", "use foo::bar;\nfoo::bar::go();\n"),
];

fn source_scope() -> WorkspaceKey {
    WorkspaceKey {
        slot: SlotId([41; 16]),
        ..workspace()
    }
}

fn in_source_scope(sequence: u64) -> OperationKey {
    let mut key = operation(sequence);
    key.workspace = source_scope();
    key
}

/// Publish the corpus as one repository revision in its own workspace.
/// One published file: its path and the Native Source identity Q reports.
type PublishedFile = (&'static str, quanta_index_contract::NativeSourceIdentity);

fn publish_source_revision<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
) -> TestResult<(
    OperationKey,
    quanta_index_contract::index::ViewRef,
    Vec<PublishedFile>,
)> {
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: source_scope(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("Source revision workspace was not created".into());
    };
    let profile_key = in_source_scope(1);
    let profile_ingress = client.begin_input(profile_key, &budget())?;
    let profile = client.stage_profile(
        profile_ingress,
        IndexProfile {
            instances: vec![native_text_spec()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: profile_key,
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let source_key = in_source_scope(2);
    let ingress = client.begin_input(source_key, &budget())?;
    let mut mutations = Vec::new();
    let mut files = Vec::new();
    for (stable_id, (path, language, text)) in (1_u8..).zip(CORPUS) {
        let NativeIndexResponse::StagedSources(staged) = client.request(
            NativeIndexRequest::StageRepositorySources {
                ingress,
                repository: RepoId::new(REPO)?,
                inputs: vec![SourceInput {
                    path: path.into(),
                    bytes: text.as_bytes().to_vec(),
                    text: SourceTextAdmission::Utf8 {
                        policy: Digest([31; 32]),
                    },
                    language: Some(LanguageCode::new(language)?),
                }],
            },
            &budget(),
        )?
        else {
            return Err("Source revision input was not staged".into());
        };
        let [source] = staged.as_slice() else {
            return Err("one Source input must stage one Source".into());
        };
        let owner = OwnerKey {
            producer: ingress.operation.producer,
            stable_id: StableOwnerId([stable_id; 16]),
        };
        mutations.push(OwnerMutation {
            owner,
            expected: None,
            next: Some(*source),
        });
        files.push((
            path,
            quanta_index_contract::NativeSourceIdentity {
                owner,
                source: *source,
            },
        ));
    }
    let request = prepare_text_mutations_in_ingress(client, configured, ingress, mutations)?;
    Ok((
        source_key,
        published(&client.submit(request, &budget())?)?,
        files,
    ))
}

fn stage_object<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    ingress: IngressRef,
    bytes: Vec<u8>,
    children: Vec<ObjectRef>,
) -> TestResult<ObjectRef> {
    let NativeIndexResponse::Staged(reference) = client.request(
        NativeIndexRequest::StageImmutableObject {
            ingress,
            bytes: NativeImmutableObjectPayloadV1::new(bytes)?,
            children: NativeImmutableObjectChildrenV1::new(children)?,
        },
        &budget(),
    )?
    else {
        return Err("protected route object was not staged".into());
    };
    Ok(reference)
}

/// Seal `(REPO, REVISION)` as a protected Source head beside a primary view.
fn seal_protected_head(
    root: &std::path::Path,
    socket: &std::path::Path,
) -> TestResult<(quanta_index_contract::index::ViewRef, Vec<PublishedFile>)> {
    let running = NativeServiceGuard::start(read_session_service_with_retention(
        root,
        socket,
        Duration::from_secs(30),
        None,
    )?);
    let client = NativeUdsClient::new(socket);
    let primary = create_text_workspace(&client)?;
    let _primary = publish_text_owner(&client, primary, 2, 1, "primary.rs", "primary only")?;
    let (source_key, source_view, files) = publish_source_revision(&client)?;
    running.stop()?;

    let catalog = SqliteCatalog::open(root, Duration::from_secs(2))?;
    let mut work = WorkBudget::with_request(work_limits(), budget());
    let source_operation = catalog.operation(source_key)?;
    let publication = catalog.published_view_metadata(source_key, &mut work)?;
    let routes = NativeProtectedSourceRoutesV1 {
        wire_version: NativeProtectedSourceRoutesV1::WIRE_VERSION,
        native_origin: None,
        heads: vec![NativeProtectedSourceHeadV1 {
            repo_id: RepoId::new(REPO)?,
            revision_id: RevisionId::new(REVISION)?,
            source_operation,
            source_publication: publication.clone(),
            operation: source_operation,
            publication: publication.clone(),
        }],
        repomap_heads: Vec::new(),
    };
    routes.validate()?;
    drop(catalog);

    let running = NativeServiceGuard::start(read_session_service_with_retention(
        root,
        socket,
        Duration::from_secs(30),
        None,
    )?);
    let sealed = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let ingress = client.begin_input(operation(50), &budget())?;
        let _marker = stage_object(
            &client,
            ingress,
            native_protected_view_marker_bytes_v1(publication.identity)?,
            Vec::new(),
        )?;
        let routes_ref = stage_object(
            &client,
            ingress,
            serde_json::to_vec(&routes)?,
            routes.children()?,
        )?;
        let NativeIndexResponse::RootScopedView(_selected) = client.request(
            NativeIndexRequest::SealCandidateWithSourceRoutes {
                primary: operation(2),
                ingress,
                routes: routes_ref,
            },
            &budget(),
        )?
        else {
            return Err("protected Source routes were not sealed".into());
        };
        Ok(())
    })();
    let stopped = running.stop();
    sealed?;
    stopped?;
    Ok((source_view, files))
}

/// Replace the selected revision while keeping its Source publication live.
/// The old revision has no selected route after this seal, but its view stays
/// independently retained by the new route.
fn replace_protected_revision(
    root: &std::path::Path,
    socket: &std::path::Path,
    source_key: OperationKey,
    revision: &str,
) -> TestResult {
    let catalog = SqliteCatalog::open(root, Duration::from_secs(2))?;
    let mut work = WorkBudget::with_request(work_limits(), budget());
    let source_operation = catalog.operation(source_key)?;
    let publication = catalog.published_view_metadata(source_key, &mut work)?;
    drop(catalog);
    let routes = NativeProtectedSourceRoutesV1 {
        wire_version: NativeProtectedSourceRoutesV1::WIRE_VERSION,
        native_origin: None,
        heads: vec![NativeProtectedSourceHeadV1 {
            repo_id: RepoId::new(REPO)?,
            revision_id: RevisionId::new(revision)?,
            source_operation,
            source_publication: publication.clone(),
            operation: source_operation,
            publication: publication.clone(),
        }],
        repomap_heads: Vec::new(),
    };
    routes.validate()?;
    let client = NativeUdsClient::new(socket);
    let ingress = client.begin_input(operation(51), &budget())?;
    let _marker = stage_object(
        &client,
        ingress,
        native_protected_view_marker_bytes_v1(publication.identity)?,
        Vec::new(),
    )?;
    let routes_ref = stage_object(
        &client,
        ingress,
        serde_json::to_vec(&routes)?,
        routes.children()?,
    )?;
    let NativeIndexResponse::RootScopedView(_selected) = client.request(
        NativeIndexRequest::SealCandidateWithSourceRoutes {
            primary: operation(2),
            ingress,
            routes: routes_ref,
        },
        &budget(),
    )?
    else {
        return Err("replacement protected Source routes were not sealed".into());
    };
    Ok(())
}

fn paths(response: &quanta_index_contract::TextQueryResponse) -> Vec<&str> {
    response
        .results
        .iter()
        .map(|candidate| candidate.repo_relative_path.as_str())
        .collect()
}

/// Publish one more file of `REPO` into the revision's workspace.
fn publish_later_file<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    expected: quanta_index_contract::index::ViewRef,
    sequence: u64,
    stable_id: u8,
    path: &str,
    text: &str,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let ingress = client.begin_input(in_source_scope(sequence), &budget())?;
    let NativeIndexResponse::StagedSources(staged) = client.request(
        NativeIndexRequest::StageRepositorySources {
            ingress,
            repository: RepoId::new(REPO)?,
            inputs: vec![SourceInput {
                path: path.into(),
                bytes: text.as_bytes().to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
                language: Some(LanguageCode::new("rust")?),
            }],
        },
        &budget(),
    )?
    else {
        return Err("later Source input was not staged".into());
    };
    let [source] = staged.as_slice() else {
        return Err("one later input must stage one Source".into());
    };
    let request = prepare_text_mutations_in_ingress(
        client,
        expected,
        ingress,
        [OwnerMutation {
            owner: OwnerKey {
                producer: ingress.operation.producer,
                stable_id: StableOwnerId([stable_id; 16]),
            },
            expected: None,
            next: Some(*source),
        }],
    )?;
    published(&client.submit(request, &budget())?)
}

/// Release `pin`, the last lease on its view, and check that retention
/// then reclaims that view.
///
/// The wait observes retention rounds, never elapsed time: the release is
/// committed before the mark, so the second round completed after it has
/// reconsidered the released view. Its exact read is then the typed
/// `SnapshotUnavailable` refusal.
fn release_and_check_reclaimed<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    retention: &NativeRetentionObservation,
    pin: &quanta_index_contract::native_protocol::NativeReadSessionStatus,
) -> TestResult {
    let (released, _) = client.release_read_session(pin, &budget())?;
    assert_eq!(released.state, NativeReadSessionState::Released);
    retention.wait_released_after(retention.mark())?;
    let read = client.search_text(
        ViewSelection::Exact(pin.view),
        native_text_spec().instance,
        native_text_request("sphinx", 1),
        &budget(),
    );
    assert!(
        matches!(&read, Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::SnapshotUnavailable),
        "retention did not reclaim {:?}: {read:?}",
        pin.view
    );
    Ok(())
}

/// Retain only the newest view of each scope, so a later publication would
/// reclaim the head view unless a read session pins it.
fn newest_view_retention() -> NativeRetentionConfig {
    NativeRetentionConfig {
        scopes: vec![
            NativeRetentionScope {
                workspace: workspace(),
                epoch: 1,
            },
            NativeRetentionScope {
                workspace: source_scope(),
                epoch: 1,
            },
        ],
        sequence_window: 1,
        page_limit: 8,
        custody_page_limit: 8,
        interval: Duration::from_millis(20),
        round_timeout: Duration::from_secs(2),
        work_limits: work_limits(),
        head_probe_every: 2,
    }
}

#[test]
fn sdk_default_text_on_native_matches_the_legacy_code_search_frontdoor() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (head_view, files) = seal_protected_head(root.path(), &socket)?;
    let head_sequence = head_view.sequence;
    let retention = newest_view_retention();
    let running = NativeServiceGuard::start(read_session_service_with_retention(
        root.path(),
        &socket,
        Duration::from_secs(30),
        Some(retention),
    )?);
    let primary = (|| -> TestResult {
        let client = QuantaIndex::connect(ConnectOptions::from_native_socket(
            socket.clone(),
            ViewSelection::Latest {
                workspace: source_scope(),
                epoch: 1,
            },
        ))?;
        let pin = GenerationPin::new(
            RepoId::new(REPO)?,
            RevisionId::new(REVISION)?,
            ManifestGeneration::new(head_sequence),
        );
        let active = || (RepoId::new(REPO), RevisionId::new(REVISION));

        // Terms on different lines of one Source are one file result.
        let (repo, revision) = active();
        let response = client
            .lexical()
            .query()
            .text("todo main")
            .active(repo?, revision?)
            .top_k(10)
            .execute()
            .map_err(|error| format!("frontdoor step 1: {error:?}"))?;
        assert_eq!(response.generation, pin);
        assert_eq!(response.rank_unit, TextRankUnit::File);
        let [file] = response.results.as_slice() else {
            return Err(format!("todo main must return one file: {response:?}").into());
        };
        assert_eq!(file.repo_relative_path.as_str(), "src/lib.rs");
        assert_eq!(file.candidate_id, LIB_FILE_ID);
        assert_eq!(file.source_repo_id.as_str(), REPO);
        // Native reports the published Source record, not a byte SHA-256.
        assert_eq!(file.source, None);
        let lib_source = files
            .iter()
            .find(|(path, _)| *path == "src/lib.rs")
            .map(|(_, identity)| *identity)
            .ok_or("fixture lost src/lib.rs")?;
        assert_eq!(file.native_source, Some(lib_source));
        // The representative witness is one of the two terms; either way its
        // line is reported and the 21-byte file fits in one excerpt.
        let lines = file
            .lines
            .ok_or("Native CodeSearch row lost its match lines")?;
        assert!(lines.start_line == lines.end_line && (1..=2).contains(&lines.start_line));
        assert_eq!(
            file.excerpt
                .as_ref()
                .map(|excerpt| excerpt.snippet.as_str()),
            Some("todo!()\nfn main() {}\n")
        );
        // Native rows carry no Legacy preview metadata or Source SHA-256.
        assert_eq!(file.preview, None);

        let (repo, revision) = active();
        let two_files = client
            .lexical()
            .query()
            .text("sphinx")
            .active(repo?, revision?)
            .top_k(10)
            .execute()
            .map_err(|error| format!("frontdoor step 2: {error:?}"))?;
        assert_eq!(two_files.generation, pin);
        assert_eq!(
            paths(&two_files).into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from(["src/alpha.rs", "src/beta.rs"])
        );

        let (repo, revision) = active();
        let first_page = client
            .lexical()
            .query()
            .text("sphinx")
            .active(repo?, revision?)
            .top_k(1)
            .execute()
            .map_err(|error| format!("frontdoor step 3: {error:?}"))?;
        assert_eq!(first_page.rank_unit, TextRankUnit::File);
        assert_eq!(first_page.results.len(), 1);
        let cursor = first_page
            .next_cursor
            .clone()
            .ok_or("first file page lacks continuation")?;
        let second_page = client
            .lexical()
            .query()
            .text("sphinx")
            .pinned(first_page.generation.clone())
            .top_k(1)
            .after(cursor)
            .execute()
            .map_err(|error| format!("frontdoor step 4: {error:?}"))?;
        assert_eq!(second_page.rank_unit, TextRankUnit::File);
        assert_eq!(second_page.results.len(), 1);
        assert!(second_page.next_cursor.is_none());
        assert_ne!(paths(&first_page), paths(&second_page));
        let mut both = paths(&first_page);
        both.extend(paths(&second_page));
        both.sort_unstable();
        assert_eq!(both, ["src/alpha.rs", "src/beta.rs"]);

        for (query, expected) in [
            ("regex:/sphinx.*quartz/", "src/alpha.rs"),
            ("content:regex:/sphinx.*riddles/", "src/beta.rs"),
            ("path:regex:/beta[.]rs/", "src/beta.rs"),
            ("sphinx regex:/quartz/", "src/alpha.rs"),
            // No Source body contains `alpha`, `readme` or `beta.rs`; their
            // paths do.
            ("path:alpha", "src/alpha.rs"),
            ("alpha", "src/alpha.rs"),
            ("readme", "docs/readme.md"),
            ("regex:/beta[.]rs/", "src/beta.rs"),
            ("regex:/read.e/", "docs/readme.md"),
        ] {
            let (repo, revision) = active();
            let response = client
                .lexical()
                .query()
                .text(query)
                .active(repo?, revision?)
                .top_k(10)
                .execute()
                .map_err(|error| format!("frontdoor step 5: {error:?}"))?;
            assert_eq!(paths(&response), [expected], "{query}");
        }

        // Match positions, derived by hand from `CORPUS` and the Legacy
        // excerpt rule: up to 120 bytes each side, inside the Source. A path
        // match shows the path itself and reports no lines.
        let lines = |start_line, end_line| {
            Some(quanta_index_contract::LexicalLineRange {
                start_line,
                end_line,
            })
        };
        for (query, expected_path, expected_lines, snippet, offset, len) in [
            // `main` is bytes 11..15 of "todo!()\nfn main() {}\n", line 2.
            (
                "main",
                "src/lib.rs",
                lines(2, 2),
                "todo!()\nfn main() {}\n",
                11,
                4,
            ),
            (
                "content:main",
                "src/lib.rs",
                lines(2, 2),
                "todo!()\nfn main() {}\n",
                11,
                4,
            ),
            // `notes` is bytes 7..12 of "quartz notes\n", line 1.
            (
                "notes",
                "docs/readme.md",
                lines(1, 1),
                "quartz notes\n",
                7,
                5,
            ),
            // `alpha` is bytes 4..9 of the path "src/alpha.rs".
            ("path:alpha", "src/alpha.rs", None, "src/alpha.rs", 4, 5),
            ("alpha", "src/alpha.rs", None, "src/alpha.rs", 4, 5),
            // `beta.rs` is bytes 4..11 of the path "src/beta.rs".
            (
                "path:regex:/beta[.]rs/",
                "src/beta.rs",
                None,
                "src/beta.rs",
                4,
                7,
            ),
            (
                "regex:/beta[.]rs/",
                "src/beta.rs",
                None,
                "src/beta.rs",
                4,
                7,
            ),
        ] {
            let (repo, revision) = active();
            let response = client
                .lexical()
                .query()
                .text(query)
                .active(repo?, revision?)
                .top_k(10)
                .execute()
                .map_err(|error| format!("frontdoor step 5b: {error:?}"))?;
            let [row] = response.results.as_slice() else {
                return Err(format!("{query}: one file expected: {response:?}").into());
            };
            assert_eq!(row.repo_relative_path.as_str(), expected_path, "{query}");
            assert_eq!(row.lines, expected_lines, "{query}");
            assert_eq!(
                row.excerpt,
                Some(quanta_index_contract::LexicalMatchExcerpt {
                    snippet: snippet.to_string(),
                    snippet_hit_offset: Some(offset),
                    highlights: vec![quanta_index_contract::HighlightSpan { start: offset, len }],
                }),
                "{query}"
            );
        }

        // Scores, derived by hand from the Legacy CodeSearch scorer: an exact
        // boundary span scores 100, a path span adds 35 in the basename, a
        // folded literal with its exact spelling in the witness adds 5, and
        // a query with a regex term earns no proximity. `beta.rs` in
        // "src/beta.rs" is a basename boundary span: 135. In
        // `sphinx path:regex:/beta/`, `sphinx` is a body boundary span of
        // "sphinx riddles\n" with its exact spelling (105), and `beta` a
        // basename boundary span (135): 240.
        for (query, expected_path, score) in [
            ("regex:/beta[.]rs/", "src/beta.rs", 135.0),
            ("path:regex:/beta[.]rs/", "src/beta.rs", 135.0),
            ("sphinx path:regex:/beta/", "src/beta.rs", 240.0),
        ] {
            let (repo, revision) = active();
            let response = client
                .lexical()
                .query()
                .text(query)
                .active(repo?, revision?)
                .top_k(10)
                .execute()
                .map_err(|error| format!("frontdoor step 5c: {error:?}"))?;
            let ranked = response
                .results
                .iter()
                .map(|row| (row.repo_relative_path.as_str(), row.score))
                .collect::<Vec<_>>();
            assert_eq!(ranked, [(expected_path, score)], "{query}");
        }

        // The language constraint selects verified Source languages, any-of.
        let rust = LanguageCode::new("rust")?;
        let markdown = LanguageCode::new("markdown")?;
        for (languages, expected) in [
            (Vec::new(), &["docs/readme.md", "src/alpha.rs"][..]),
            (vec![rust.clone()], &["src/alpha.rs"]),
            (vec![markdown.clone()], &["docs/readme.md"]),
            (vec![rust, markdown], &["docs/readme.md", "src/alpha.rs"]),
        ] {
            let (repo, revision) = active();
            let response = client
                .lexical()
                .query()
                .text("quartz")
                .language_any_of(languages.clone())
                .active(repo?, revision?)
                .top_k(10)
                .execute()
                .map_err(|error| format!("frontdoor step 6: {error:?}"))?;
            let mut found = paths(&response);
            found.sort_unstable();
            assert_eq!(found, expected, "{languages:?}");
        }

        // The CLI's repo + revision path reproduces the same goldens.
        let socket_text = socket.to_str().ok_or("socket path is not UTF-8")?;
        let run_cli = |extra: &[&str]| {
            let mut args = vec![
                "code-search",
                "--socket",
                socket_text,
                "--repo",
                REPO,
                "--revision",
                REVISION,
                "--output",
                "json",
            ];
            args.extend_from_slice(extra);
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = quanta_index_searchctl::run(args, &mut stdout, &mut stderr);
            (exit, stdout, stderr)
        };
        let cli = |extra: &[&str]| -> TestResult<quanta_index_contract::TextQueryResponse> {
            let (exit, stdout, stderr) = run_cli(extra);
            if exit != std::process::ExitCode::SUCCESS {
                return Err(format!(
                    "code-search {extra:?} exited {exit:?}: {}",
                    String::from_utf8_lossy(&stderr)
                )
                .into());
            }
            Ok(serde_json::from_slice(&stdout)?)
        };

        // File goldens from the fixed corpus, including real zero-hit pages.
        for (query, expected) in [
            (
                "content:SPHINX case:no",
                &["src/alpha.rs", "src/beta.rs"][..],
            ),
            ("content:SPHINX case:yes", &[]),
            (r#"content:"sphinx of quartz""#, &["src/alpha.rs"]),
            (r#"content:"sphinx quartz""#, &[]),
            ("Context", &["src/context.rs", "src/lower.rs"]),
            ("case:yes Context", &["src/context.rs"]),
            ("WithContext", &["src/context.rs"]),
            ("foo::bar", &["src/paths.rs"]),
            ("quartz riddles", &[]),
            ("content:regex:/sphinx.*riddles/", &["src/beta.rs"]),
            ("path:regex:/beta[.]rs/", &["src/beta.rs"]),
        ] {
            let (repo, revision) = active();
            let sdk = client
                .lexical()
                .query()
                .text(query)
                .active(repo?, revision?)
                .top_k(10)
                .execute()?;
            for response in [sdk, cli(&["--query-text", query, "--top-k", "10"])?] {
                let mut found = paths(&response);
                found.sort_unstable();
                assert_eq!(found, expected, "{query}");
                assert_eq!(response.generation, pin, "{query}");
                assert!(response.window.outcome().is_exhausted(), "{query}");
                assert!(response.next_cursor.is_none(), "{query}");
            }
        }

        for (query, expected) in [
            ("typo:load_jsom", LexicalErrorCode::UnsupportedCombo),
            ("a b c d e", LexicalErrorCode::LimitExceededFanout),
        ] {
            let (repo, revision) = active();
            let refused = client
                .lexical()
                .query()
                .text(query)
                .active(repo?, revision?)
                .top_k(10)
                .execute();
            let expected = SearchPlaneErrorCodeV2::Lexical(expected);
            assert!(
                matches!(&refused, Err(SdkError::Remote { code, .. }) if *code == expected),
                "{query}: {refused:?}"
            );
            let (exit, stdout, stderr) = run_cli(&["--query-text", query, "--top-k", "10"]);
            assert_eq!(exit, std::process::ExitCode::from(3), "{query}");
            assert!(stdout.is_empty(), "{query}: CLI emitted a result page");
            let message = String::from_utf8(stderr)?;
            assert!(
                message.contains(expected.as_wire_str()),
                "{query}: {message}"
            );
        }

        let joined = cli(&["--query-text", "todo main", "--top-k", "10"])?;
        assert_eq!(joined.generation, pin);
        assert_eq!(paths(&joined), ["src/lib.rs"]);
        let narrowed = cli(&[
            "--query-text",
            "quartz",
            "--top-k",
            "10",
            "--language",
            "markdown",
        ])?;
        assert_eq!(paths(&narrowed), ["docs/readme.md"]);
        let first = cli(&["--query-text", "sphinx", "--top-k", "1"])?;
        let token = first
            .next_cursor
            .as_ref()
            .ok_or("CLI first page lacks continuation")?
            .as_str()
            .to_owned();
        let generation = first.generation.manifest_generation.get().to_string();
        let second = cli(&[
            "--query-text",
            "sphinx",
            "--top-k",
            "1",
            "--generation",
            &generation,
            "--after",
            &token,
        ])?;
        assert!(second.next_cursor.is_none());
        let mut both = paths(&first);
        both.extend(paths(&second));
        both.sort_unstable();
        assert_eq!(both, ["src/alpha.rs", "src/beta.rs"]);

        // An explicit member overrides discovery and must exist in the profile.
        let configured = QuantaIndex::connect(
            ConnectOptions::from_native_socket(
                socket.clone(),
                ViewSelection::Latest {
                    workspace: source_scope(),
                    epoch: 1,
                },
            )
            .with_native_text_instance(native_text_spec().instance),
        )?;
        let (repo, revision) = active();
        let explicit = configured
            .lexical()
            .query()
            .text("sphinx")
            .active(repo?, revision?)
            .top_k(10)
            .execute()
            .map_err(|error| format!("frontdoor step 7: {error:?}"))?;
        assert_eq!(paths(&explicit), paths(&two_files));
        let absent = QuantaIndex::connect(
            ConnectOptions::from_native_socket(
                socket.clone(),
                ViewSelection::Latest {
                    workspace: source_scope(),
                    epoch: 1,
                },
            )
            .with_native_text_instance(IndexInstanceId([99; 16])),
        )?;
        let (repo, revision) = active();
        let refused = absent
            .lexical()
            .query()
            .text("sphinx")
            .active(repo?, revision?)
            .top_k(10)
            .execute();
        assert!(
            matches!(&refused, Err(SdkError::Native(error))
                if matches!(**error, quanta_index_sdk::NativeQueryError::InstanceMismatch)),
            "{refused:?}"
        );

        // A stale generation and an unknown revision refuse typed.
        let stale = client
            .lexical()
            .query()
            .text("sphinx")
            .pinned(GenerationPin::new(
                RepoId::new(REPO)?,
                RevisionId::new(REVISION)?,
                ManifestGeneration::new(head_sequence.checked_add(7).ok_or("sequence")?),
            ))
            .top_k(1)
            .execute();
        assert!(
            matches!(
                &stale,
                Err(SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::GenerationMismatch,
                    ..
                })
            ),
            "{stale:?}"
        );
        let unknown = client
            .lexical()
            .query()
            .text("sphinx")
            .active(RepoId::new(REPO)?, RevisionId::new("rev-absent")?)
            .top_k(1)
            .execute();
        assert!(
            matches!(&unknown, Err(SdkError::Native(error))
                if matches!(&**error, quanta_index_sdk::NativeQueryError::Transport(
                    NativeUdsError::Refused(refusal)
                ) if refusal.code == NativeIndexErrorCode::SnapshotUnavailable)),
            "{unknown:?}"
        );
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}

// Cargo only exports binary paths for the package under test. When no
// QUANTA_INDEX_SEARCHCTL_BIN is supplied, a child of this test executable
// calls the same public CLI entry as searchctl's main function.
#[test]
fn code_search_cli_child_entry() -> TestResult {
    let Some(output_path) = std::env::var_os("QUANTA_INDEX_Q01_CLI_CHILD_OUTPUT") else {
        return Ok(());
    };
    let args: Vec<String> =
        serde_json::from_str(&std::env::var("QUANTA_INDEX_Q01_CLI_CHILD_ARGS")?)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = quanta_index_searchctl::run(args, &mut stdout, &mut stderr);
    if exit != std::process::ExitCode::SUCCESS {
        return Err(format!(
            "CLI child exited {exit:?}: {}",
            String::from_utf8_lossy(&stderr)
        )
        .into());
    }
    std::fs::write(output_path, stdout)?;
    Ok(())
}

#[test]
fn code_search_cli_processes_reuse_released_capacity_and_continue_a_page() -> TestResult {
    use quanta_index_core::index::ports::CatalogExactHistoryPort as _;

    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (view, _files) = seal_protected_head(root.path(), &socket)?;
    // Drive the floor and retirement explicitly. The autonomous retention
    // pump also retires terminal publication request objects, which the
    // fixture still needs when it seals the replacement BOOT route.
    let running = NativeServiceGuard::start(read_session_service_with_retention(
        root.path(),
        &socket,
        Duration::from_secs(30),
        None,
    )?);
    let checked = (|| -> TestResult {
        assert_eq!(view_pin_count(root.path(), view.sequence)?, 0);
        let executable = std::env::current_exe()?;
        let searchctl_binary = std::env::var_os("QUANTA_INDEX_SEARCHCTL_BIN");
        let socket_text = socket.to_str().ok_or("fixture socket is not UTF-8")?;
        let started = Instant::now();
        let mut page_number = 0_u32;
        let mut cli =
            |revision: &str,
             query: &str,
             top_k: u32,
             continuation: Option<(&str, u64)>|
             -> TestResult<quanta_index_contract::TextQueryResponse> {
                page_number += 1;
                let output_path = root.path().join(format!("cli-page-{page_number}.json"));
                let top_k = top_k.to_string();
                let mut args = vec![
                    "code-search",
                    "--socket",
                    socket_text,
                    "--repo",
                    REPO,
                    "--revision",
                    revision,
                    "--output",
                    "json",
                    "--query-text",
                    query,
                    "--top-k",
                    &top_k,
                ];
                let generation_text;
                if let Some((cursor, generation)) = continuation {
                    generation_text = generation.to_string();
                    args.extend(["--after", cursor, "--generation", &generation_text]);
                }
                let completed = match &searchctl_binary {
                Some(binary) => std::process::Command::new(binary).args(&args).output()?,
                None => std::process::Command::new(&executable)
                    .arg("--exact")
                    .arg("read_sessions::query::code_search_frontdoor::code_search_cli_child_entry")
                    .env("QUANTA_INDEX_Q01_CLI_CHILD_OUTPUT", &output_path)
                    .env("QUANTA_INDEX_Q01_CLI_CHILD_ARGS", serde_json::to_string(&args)?)
                    .output()?,
            };
                if !completed.status.success() {
                    return Err(format!(
                        "CLI process {page_number} failed: stdout={} stderr={}",
                        String::from_utf8_lossy(&completed.stdout),
                        String::from_utf8_lossy(&completed.stderr)
                    )
                    .into());
                }
                let bytes = match searchctl_binary.as_ref() {
                    Some(_) => completed.stdout,
                    None => std::fs::read(output_path)?,
                };
                let page = serde_json::from_slice(&bytes)?;
                Ok(page)
            };

        // A released session must not consume a live-session slot for the
        // full five-minute replay TTL. All children run under the same UID.
        for ordinal in 0..17 {
            let page = cli(REVISION, "content:SPHINX case:no", 10, None)?;
            let mut found = paths(&page);
            found.sort_unstable();
            assert_eq!(found, ["src/alpha.rs", "src/beta.rs"], "{ordinal}");
            assert_eq!(page.generation.manifest_generation.get(), view.sequence);
            assert!(page.next_cursor.is_none(), "{ordinal}");
            assert_eq!(
                view_pin_count(root.path(), view.sequence)?,
                0,
                "CLI process {ordinal} retained a read pin after exit"
            );
        }
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "replay TTL expired before the 17th CLI process"
        );

        let first = cli(REVISION, "sphinx", 1, None)?;
        assert_eq!(paths(&first).len(), 1);
        let cursor = first
            .next_cursor
            .as_ref()
            .ok_or("first CLI process did not issue a cursor")?
            .as_str()
            .to_owned();
        let producer = NativeUdsClient::new(&socket);
        let pin = producer.open_read_session(ViewSelection::Exact(view), [64; 32], &budget())?;
        let next_view = publish_later_file(
            &producer,
            view,
            3,
            9,
            "src/gamma.rs",
            "sphinx from a later Source publication\n",
        )
        .map_err(|error| format!("publish next revision: {error}"))?;
        replace_protected_revision(root.path(), &socket, in_source_scope(3), "rev-next")
            .map_err(|error| format!("seal replacement revision: {error}"))?;
        let old_route = producer.resolve_protected_source_head_v1(
            RepoId::new(REPO)?,
            RevisionId::new(REVISION)?,
            &budget(),
        );
        assert!(
            matches!(&old_route, Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable),
            "old protected route unexpectedly survived replacement: {old_route:?}"
        );
        let new_route = producer.resolve_protected_source_head_v1(
            RepoId::new(REPO)?,
            RevisionId::new("rev-next")?,
            &budget(),
        )?;
        assert_eq!(new_route.view.identity, next_view);
        let retained = producer.resolve_protected_source_view_v1(
            RepoId::new(REPO)?,
            RevisionId::new(REVISION)?,
            view,
            &budget(),
        )?;
        assert_eq!(retained.view.identity, view);
        for (revision, unselected_view) in [(REVISION, next_view), ("rev-next", view)] {
            let refused = producer.resolve_protected_source_view_v1(
                RepoId::new(REPO)?,
                RevisionId::new(revision)?,
                unselected_view,
                &budget(),
            );
            assert!(
                matches!(&refused, Err(NativeUdsError::Refused(error))
                    if error.code == NativeIndexErrorCode::SnapshotUnavailable),
                "unselected revision/view pair was accepted: {refused:?}"
            );
        }
        let foreign_repository = producer.resolve_protected_source_view_v1(
            RepoId::new("another-repository")?,
            RevisionId::new(REVISION)?,
            view,
            &budget(),
        );
        assert!(
            matches!(&foreign_repository, Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable),
            "foreign repository received the protected view: {foreign_repository:?}"
        );
        let foreign_revision = cli(
            "rev-next",
            "sphinx",
            1,
            Some((&cursor, first.generation.manifest_generation.get())),
        );
        assert!(
            foreign_revision.as_ref().is_err_and(|error| error
                .to_string()
                .contains("QUERY_CURSOR_GENERATION_MISMATCH")),
            "foreign revision accepted the old cursor: {foreign_revision:?}"
        );
        let foreign_generation = cli(REVISION, "sphinx", 1, Some((&cursor, next_view.sequence)));
        assert!(
            foreign_generation.as_ref().is_err_and(|error| error
                .to_string()
                .contains("QUERY_CURSOR_GENERATION_MISMATCH")),
            "foreign generation accepted the old cursor: {foreign_generation:?}"
        );
        let second = cli(
            REVISION,
            "sphinx",
            1,
            Some((&cursor, first.generation.manifest_generation.get())),
        )?;
        assert!(second.next_cursor.is_none());
        let mut found = paths(&first);
        found.extend(paths(&second));
        found.sort_unstable();
        assert_eq!(found, ["src/alpha.rs", "src/beta.rs"]);
        assert_eq!(view_pin_count(root.path(), view.sequence)?, 1);
        // Closing exact admission does not invalidate the old held pin, but
        // it must stop a new CLI process from taking another one.
        let catalog = SqliteCatalog::open(root.path(), Duration::from_secs(2))?;
        let mut work = WorkBudget::with_request(work_limits(), budget());
        let floor = catalog.advance_exact_history_floor(
            source_scope(),
            view.epoch,
            next_view.sequence,
            &mut work,
        )?;
        assert_eq!(floor, next_view.sequence);
        let below_floor = cli(REVISION, "sphinx", 1, Some((&cursor, view.sequence)));
        assert!(
            below_floor
                .as_ref()
                .is_err_and(|error| error.to_string().contains("code: SnapshotUnavailable")),
            "CLI opened a new session below the exact floor: {below_floor:?}"
        );
        assert_eq!(view_pin_count(root.path(), view.sequence)?, 1);
        let (released, _) = producer.release_read_session(&pin, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        let retired =
            catalog.retire_exact_history_page(source_scope(), view.epoch, None, 8, &mut work)?;
        assert!(retired.retired > 0 && !retired.has_more, "{retired:?}");
        drop(catalog);
        let reclaimed = cli(
            REVISION,
            "sphinx",
            1,
            Some((&cursor, first.generation.manifest_generation.get())),
        );
        assert!(
            reclaimed
                .as_ref()
                .is_err_and(|error| error.to_string().contains("code: SnapshotUnavailable")),
            "reclaimed view remained accessible through the old cursor: {reclaimed:?}"
        );
        let catalog = rusqlite::Connection::open_with_flags(
            root.path().join("catalog/catalog-v1.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let retired_bindings: i64 = catalog.query_row(
            "SELECT count(*) FROM native_protected_source_view_v1 WHERE repo=?1 AND revision=?2",
            [REPO, REVISION],
            |row| row.get(0),
        )?;
        assert_eq!(retired_bindings, 0);
        Ok(())
    })();
    let stopped = running.stop();
    checked?;
    stopped
}

#[test]
fn sdk_text_cursor_keeps_its_view_after_a_later_publication_and_retention() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let (head_view, _files) = seal_protected_head(root.path(), &socket)?;
    let service = read_session_service_with_retention(
        root.path(),
        &socket,
        Duration::from_secs(30),
        Some(newest_view_retention()),
    )?;
    let retention = service.retention_observation();
    let running = NativeServiceGuard::start(service);
    let primary = (|| -> TestResult {
        let client = QuantaIndex::connect(ConnectOptions::from_native_socket(
            socket.clone(),
            ViewSelection::Latest {
                workspace: source_scope(),
                epoch: 1,
            },
        ))?;
        let active = || (RepoId::new(REPO), RevisionId::new(REVISION));
        // A cursor keeps reading its first view after a later publication.
        let (repo, revision) = active();
        let (first_page, first_trace) = client
            .lexical()
            .query()
            .text("sphinx")
            .active(repo?, revision?)
            .top_k(1)
            .execute_observed()
            .map_err(|error| format!("cursor first page: {error:?}"))?;
        use ClientQueryRpcKindV1::{
            NativeOpenReadSession, NativeReadSessionProfile, NativeResolveProtectedSourceHead,
            NativeTextPathPage,
        };
        let trace_keys = |trace: &quanta_index_sdk::ClientLexicalQueryObservationV1| {
            for rpc in &trace.rpcs {
                assert!(rpc.ipc.read_io <= rpc.ipc.decode_call);
                assert!(rpc.ipc.decode_call <= rpc.ipc.total);
                assert!(rpc.ipc.write <= rpc.ipc.total);
            }
            trace
                .rpcs
                .iter()
                .map(|rpc| (rpc.kind, rpc.request_id))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            trace_keys(&first_trace),
            [
                (NativeResolveProtectedSourceHead, 1),
                (NativeOpenReadSession, 2),
                (NativeReadSessionProfile, 3),
                (NativeTextPathPage, 4),
            ]
        );
        let cursor = first_page
            .next_cursor
            .clone()
            .ok_or("first file page lacks continuation")?;
        // Two later publications of the same repository move the workspace
        // past the head view. Retention keeps one view per scope, so once it
        // has reclaimed the unpinned middle view, its floor is past the head.
        let producer = NativeUdsClient::new(socket.clone());
        let later_view =
            publish_later_file(&producer, head_view, 3, 9, "src/gamma.rs", "sphinx again\n")?;
        // A session pins the middle view while it is the newest, so its
        // release, not elapsed time, hands that view to retention.
        let pin =
            producer.open_read_session(ViewSelection::Exact(later_view), [63; 32], &budget())?;
        let newest_view =
            publish_later_file(&producer, later_view, 4, 10, "src/zeta.rs", "zeta only\n")?;
        assert_ne!(newest_view, later_view);
        release_and_check_reclaimed(&producer, &retention, &pin)?;
        let foreign = client
            .lexical()
            .query()
            .text("sphinx")
            .pinned(GenerationPin::new(
                RepoId::new(REPO)?,
                RevisionId::new(REVISION)?,
                ManifestGeneration::new(later_view.sequence),
            ))
            .top_k(1)
            .after(cursor.clone())
            .execute();
        assert!(
            matches!(
                &foreign,
                Err(SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::QueryCursorGenerationMismatch,
                    ..
                })
            ),
            "{foreign:?}"
        );
        let foreign_revision = client
            .lexical()
            .query()
            .text("sphinx")
            .active(RepoId::new(REPO)?, RevisionId::new("another-revision")?)
            .top_k(1)
            .after(cursor.clone())
            .execute_observed();
        assert!(
            matches!(
                &foreign_revision,
                Err(SdkError::Remote {
                    code: SearchPlaneErrorCodeV2::QueryCursorGenerationMismatch,
                    ..
                })
            ),
            "{foreign_revision:?}"
        );
        let (second_page, second_trace) = client
            .lexical()
            .query()
            .text("sphinx")
            .pinned(first_page.generation.clone())
            .top_k(1)
            .after(cursor.clone())
            .execute_observed()
            .map_err(|error| format!("cursor second page: {error:?}"))?;
        assert_eq!(trace_keys(&second_trace), [(NativeTextPathPage, 5)]);
        let plain_second = client
            .lexical()
            .query()
            .text("sphinx")
            .pinned(first_page.generation.clone())
            .top_k(1)
            .after(cursor)
            .execute()?;
        assert_eq!(second_page, plain_second);
        assert!(second_page.next_cursor.is_none());
        let mut both = paths(&first_page);
        both.extend(paths(&second_page));
        both.sort_unstable();
        assert_eq!(both, ["src/alpha.rs", "src/beta.rs"]);
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}
