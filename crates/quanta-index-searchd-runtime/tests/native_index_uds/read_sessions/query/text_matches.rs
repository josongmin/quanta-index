//! Native text matches end to end: SDK, UDS, read sessions and the reader.
//!
//! Every golden below is the output of `rg --json --no-config --sort path`
//! (ripgrep 15.1.0) over a directory holding exactly `CORPUS`, reduced to
//! `(path, absolute_offset + submatch.start, absolute_offset + submatch.end)`.
//! The flags per golden are `-F -s`, `-F -i` or `-s`, as named. Each row
//! was then checked by hand against the byte layout noted on `CORPUS`.

use super::*;
use quanta_index_contract::index::ViewRef;
use quanta_index_contract::native_protocol::{
    NativeReadSessionRequest, NativeTextMatchCase, NativeTextMatchPattern, NativeTextMatchQuery,
    NativeTextMatchesRequest,
};
use quanta_index_contract::{ContinuationTokenV2, NativeSourceIdentity};
use quanta_index_sdk::{ConnectOptions, NativeTextMatchesResponse, QuantaIndex};

/// The fixed fixture and its byte layout.
///
/// `a.rs`: line 1 is bytes 0..19 (`needle` at 0, 7 and the adjacent 13),
/// line 2 starts at 20 (`NEEDLE` at 24, `needle` at 31).
/// `crlf.txt`: line 1 is `needle\r` at 0..7, line 2 `xneedle\r` at 8..16.
/// `utf8.md`: `é` and `É` are two bytes each; line 1 is 0..20 (`café` at 0,
/// `CAFÉ` at 6, `café` at 12), line 2 starts at 21 (`néedle`, then `needle`
/// at 29).
const CORPUS: [(&str, &str); 3] = [
    ("a.rs", "needle needleneedle\nfoo NEEDLE needle\n"),
    ("crlf.txt", "needle\r\nxneedle\r\n"),
    (
        "utf8.md",
        "caf\u{e9} CAF\u{c9} caf\u{e9}ine\nn\u{e9}edle needle\n",
    ),
];

type Golden = &'static [(&'static str, u64, u64)];

/// `rg -F -s needle`.
const NEEDLE_SENSITIVE: Golden = &[
    ("a.rs", 0, 6),
    ("a.rs", 7, 13),
    ("a.rs", 13, 19),
    ("a.rs", 31, 37),
    ("crlf.txt", 0, 6),
    ("crlf.txt", 9, 15),
    ("utf8.md", 29, 35),
];
/// `rg -F -i needle`.
const NEEDLE_INSENSITIVE: Golden = &[
    ("a.rs", 0, 6),
    ("a.rs", 7, 13),
    ("a.rs", 13, 19),
    ("a.rs", 24, 30),
    ("a.rs", 31, 37),
    ("crlf.txt", 0, 6),
    ("crlf.txt", 9, 15),
    ("utf8.md", 29, 35),
];
/// `rg -s 'needle.$'`: the `\r` before `\n` belongs to the line.
const CRLF_LINE_END: Golden = &[("crlf.txt", 0, 7), ("crlf.txt", 9, 16)];
/// `rg -F -s café`.
const CAFE_SENSITIVE: Golden = &[("utf8.md", 0, 5), ("utf8.md", 12, 17)];
/// `rg -F -i café`: simple case folding pairs `É` with `é`.
const CAFE_INSENSITIVE: Golden = &[("utf8.md", 0, 5), ("utf8.md", 6, 11), ("utf8.md", 12, 17)];

fn query(pattern: NativeTextMatchPattern, case: NativeTextMatchCase) -> NativeTextMatchQuery {
    NativeTextMatchQuery {
        pattern,
        case,
        constraints: QueryConstraintSetV1::unconstrained(),
    }
}

fn literal(text: &str, case: NativeTextMatchCase) -> NativeTextMatchQuery {
    query(NativeTextMatchPattern::Literal(text.into()), case)
}

/// Publish `CORPUS` as owners 1, 2 and 3 in one Source advance.
fn publish_corpus<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
) -> TestResult<(ViewRef, Vec<NativeSourceIdentity>)> {
    let configured = create_text_workspace(client)?;
    let ingress = client.begin_input(operation(2), &budget())?;
    let mut mutations = Vec::new();
    let mut sources = Vec::new();
    for (stable_id, (path, text)) in (1_u8..).zip(CORPUS) {
        let source = stage_text_source_in_ingress(client, ingress, path, text, None)?;
        let owner = OwnerKey {
            producer: ingress.operation.producer,
            stable_id: StableOwnerId([stable_id; 16]),
        };
        mutations.push(OwnerMutation {
            owner,
            expected: None,
            next: Some(source),
        });
        sources.push(NativeSourceIdentity { owner, source });
    }
    let request = prepare_text_mutations_in_ingress(client, configured, ingress, mutations)?;
    Ok((published(&client.submit(request, &budget())?)?, sources))
}

fn sdk(socket: &std::path::Path) -> TestResult<QuantaIndex> {
    Ok(QuantaIndex::connect(ConnectOptions::from_native_socket(
        socket.to_path_buf(),
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
    ))?)
}

/// The `(path, start, end)` rows of one page; every row is unscoped and
/// carries the published Source identity of its path.
fn rows(
    matches: &[quanta_index_contract::native_protocol::NativeTextMatch],
    sources: &[(&str, NativeSourceIdentity)],
) -> TestResult<Vec<(String, u64, u64)>> {
    let mut rows = Vec::new();
    for row in matches {
        if row.repository.is_some() {
            return Err(format!("unscoped corpus returned a repository: {row:?}").into());
        }
        let expected = sources
            .iter()
            .find(|(path, _)| *path == row.path)
            .map(|(_, source)| *source)
            .ok_or_else(|| format!("unknown path {}", row.path))?;
        if row.native_source != expected {
            return Err(format!("{} carries another Source identity", row.path).into());
        }
        rows.push((row.path.clone(), row.range.start, row.range.end));
    }
    Ok(rows)
}

fn golden(rows: Golden) -> Vec<(String, u64, u64)> {
    rows.iter()
        .map(|(path, start, end)| ((*path).to_owned(), *start, *end))
        .collect()
}

/// Read every page of `query` through the SDK; each page must read `view`.
fn all_pages(
    client: &QuantaIndex,
    query: &NativeTextMatchQuery,
    limit: u32,
    view: ViewRef,
) -> TestResult<Vec<NativeTextMatchesResponse>> {
    let mut pages = Vec::new();
    let mut after: Option<ContinuationTokenV2> = None;
    loop {
        let page = client
            .lexical()
            .matches(query.clone(), limit, after.as_ref())?;
        assert_eq!(page.view, view);
        after.clone_from(&page.next_cursor);
        pages.push(page);
        if after.is_none() {
            return Ok(pages);
        }
    }
}

fn named<'a>(
    paths: impl IntoIterator<Item = &'a str>,
    sources: &[NativeSourceIdentity],
) -> Vec<(&'a str, NativeSourceIdentity)> {
    paths.into_iter().zip(sources.iter().copied()).collect()
}

#[test]
fn text_matches_equal_ripgrep_in_both_case_modes_and_across_pages() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult {
        let raw = NativeUdsClient::new(socket.clone());
        let (view, sources) = publish_corpus(&raw)?;
        let sources = named(CORPUS.map(|(path, _)| path), &sources);
        let client = sdk(&socket)?;
        let cases = [
            (
                literal("needle", NativeTextMatchCase::Sensitive),
                NEEDLE_SENSITIVE,
            ),
            (
                literal("needle", NativeTextMatchCase::Insensitive),
                NEEDLE_INSENSITIVE,
            ),
            (
                query(
                    NativeTextMatchPattern::Regex("needle.$".into()),
                    NativeTextMatchCase::Sensitive,
                ),
                CRLF_LINE_END,
            ),
            (
                literal("caf\u{e9}", NativeTextMatchCase::Sensitive),
                CAFE_SENSITIVE,
            ),
            (
                literal("caf\u{e9}", NativeTextMatchCase::Insensitive),
                CAFE_INSENSITIVE,
            ),
        ];
        for (query, expected) in &cases {
            let pages = all_pages(&client, query, 1024, view)?;
            assert_eq!(pages.len(), 1, "{query:?}");
            let page = pages.first().ok_or("no page")?;
            assert_eq!(
                rows(&page.matches, &sources)?,
                golden(expected),
                "{query:?}"
            );
            // The stateless request over the same view answers identically.
            let stateless = raw.search_text_matches(
                &NativeTextMatchesRequest {
                    selection: ViewSelection::Latest {
                        workspace: workspace(),
                        epoch: 1,
                    },
                    instance: native_text_spec().instance,
                    query: query.clone(),
                    after: None,
                    limit: 1024,
                },
                &budget(),
            )?;
            assert_eq!(stateless.view, view);
            assert_eq!(stateless.matches, page.matches, "{query:?}");
        }

        // Two rows per page: the first boundary falls between the adjacent
        // matches at 7..13 and 13..19, and the cursor resumes exactly there.
        let needle = literal("needle", NativeTextMatchCase::Sensitive);
        let pages = all_pages(&client, &needle, 2, view)?;
        assert_eq!(
            pages
                .iter()
                .map(|page| page.matches.len())
                .collect::<Vec<_>>(),
            [2, 2, 2, 1]
        );
        let mut joined = Vec::new();
        for page in &pages {
            joined.extend(rows(&page.matches, &sources)?);
        }
        assert_eq!(joined, golden(NEEDLE_SENSITIVE));
        // Exactly `limit` remaining rows still end with an exhausted page.
        let pages = all_pages(&client, &needle, 7, view)?;
        assert_eq!(pages.len(), 1);

        // `searchctl matches` pages through the same golden.
        let socket_text = socket.to_str().ok_or("socket path is not UTF-8")?;
        let key = workspace();
        let mut workspace_hex = String::new();
        for byte in key
            .tenant
            .0
            .iter()
            .chain(&key.collection.0)
            .chain(&key.worktree.0)
            .chain(&key.slot.0)
        {
            write!(workspace_hex, "{byte:02x}")?;
        }
        let mut cli_rows = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut args = vec![
                "matches",
                "--socket",
                socket_text,
                "--workspace",
                &workspace_hex,
                "--epoch",
                "1",
                "--literal",
                "needle",
                "--limit",
                "3",
                "--output",
                "json",
            ];
            if let Some(token) = &after {
                args.extend(["--after", token.as_str()]);
            }
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = quanta_index_searchctl::run(args, &mut stdout, &mut stderr);
            if exit != std::process::ExitCode::SUCCESS {
                return Err(String::from_utf8(stderr)?.into());
            }
            let page: serde_json::Value = serde_json::from_slice(&stdout)?;
            let field =
                |value: &serde_json::Value, pointer: &str| -> TestResult<serde_json::Value> {
                    Ok(value
                        .pointer(pointer)
                        .cloned()
                        .ok_or_else(|| format!("CLI page lacks {pointer}"))?)
                };
            for row in field(&page, "/matches")?
                .as_array()
                .ok_or("CLI matches are not a list")?
            {
                cli_rows.push((
                    field(row, "/path")?
                        .as_str()
                        .ok_or("CLI row path is not text")?
                        .to_owned(),
                    field(row, "/range/start")?
                        .as_u64()
                        .ok_or("CLI row start is not a number")?,
                    field(row, "/range/end")?
                        .as_u64()
                        .ok_or("CLI row end is not a number")?,
                ));
            }
            after = field(&page, "/next_cursor")?.as_str().map(str::to_owned);
            if after.is_none() {
                break;
            }
        }
        assert_eq!(cli_rows, golden(NEEDLE_SENSITIVE));
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}

#[test]
fn held_match_cursor_survives_publication_and_rebound_cursors_conflict() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    // The raw session is never renewed: its lease must outlast the test
    // under any machine load, so only its release ends it.
    let service = read_session_service(root.path(), &socket, Duration::from_secs(300))?;
    let retention = service.retention_observation();
    let running = NativeServiceGuard::start(service);
    let primary = (|| -> TestResult {
        let raw = NativeUdsClient::new(socket.clone());
        let (first_view, sources) = publish_corpus(&raw)?;
        let named_sources = named(CORPUS.map(|(path, _)| path), &sources);
        let needle = literal("needle", NativeTextMatchCase::Sensitive);
        let instance = native_text_spec().instance;

        // A raw page on an explicit session yields a cursor on `a.rs`.
        let session =
            raw.open_read_session(ViewSelection::Exact(first_view), [61; 32], &budget())?;
        let raw_first = raw.search_text_matches_in_read_session(
            &session,
            &NativeTextMatchesRequest {
                selection: ViewSelection::Exact(first_view),
                instance,
                query: needle.clone(),
                after: None,
                limit: 2,
            },
            &budget(),
        )?;
        let cursor = raw_first.next.ok_or("first raw page has no cursor")?;
        assert_eq!((cursor.path.as_str(), cursor.start), ("a.rs", 7));

        let client = sdk(&socket)?;
        let first = client.lexical().matches(needle.clone(), 2, None)?;
        assert_eq!(first.view, first_view);
        assert_eq!(
            rows(&first.matches, &named_sources)?,
            golden(NEEDLE_SENSITIVE.get(..2).ok_or("golden prefix")?)
        );

        // Replace `a.rs` with another owner and text in the next view.
        let [a_source, ..] = sources.as_slice() else {
            return Err("corpus has no a.rs source".into());
        };
        let ingress = raw.begin_input(operation(3), &budget())?;
        let replacement = stage_text_source_in_ingress(&raw, ingress, "a.rs", "needle\n", None)?;
        let replaced = OwnerKey {
            producer: ingress.operation.producer,
            stable_id: StableOwnerId([4; 16]),
        };
        let request = prepare_text_mutations_in_ingress(
            &raw,
            first_view,
            ingress,
            [
                OwnerMutation {
                    owner: a_source.owner,
                    expected: Some(a_source.source),
                    next: None,
                },
                OwnerMutation {
                    owner: replaced,
                    expected: None,
                    next: Some(replacement),
                },
            ],
        )?;
        let second_view = published(&raw.submit(request, &budget())?)?;
        assert_ne!(second_view, first_view);

        // The held session keeps the first view: the continuation finishes
        // the original golden, not the replaced file.
        let mut after = first.next_cursor;
        let mut rest = Vec::new();
        while let Some(token) = after {
            let page = client.lexical().matches(needle.clone(), 2, Some(&token))?;
            assert_eq!(page.view, first_view);
            rest.extend(rows(&page.matches, &named_sources)?);
            after = page.next_cursor;
        }
        assert_eq!(
            rest,
            golden(NEEDLE_SENSITIVE.get(2..).ok_or("golden suffix")?)
        );

        // A new sequence reads the newest view.
        let fresh = client.lexical().matches(needle.clone(), 1024, None)?;
        assert_eq!(fresh.view, second_view);
        let second_sources = [
            (
                "a.rs",
                NativeSourceIdentity {
                    owner: replaced,
                    source: replacement,
                },
            ),
            ("crlf.txt", *sources.get(1).ok_or("crlf source")?),
            ("utf8.md", *sources.get(2).ok_or("utf8 source")?),
        ];
        assert_eq!(
            rows(&fresh.matches, &second_sources)?,
            golden(&[
                ("a.rs", 0, 6),
                ("crlf.txt", 0, 6),
                ("crlf.txt", 9, 15),
                ("utf8.md", 29, 35)
            ])
        );

        // A cursor rebound to the new view names a file whose owner changed.
        let mut rebound = cursor;
        rebound.view = second_view;
        let conflict = raw.search_text_matches(
            &NativeTextMatchesRequest {
                selection: ViewSelection::Exact(second_view),
                instance,
                query: needle.clone(),
                after: Some(rebound),
                limit: 2,
            },
            &budget(),
        );
        assert!(
            matches!(&conflict, Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::Conflict),
            "{conflict:?}"
        );

        // Edit in place: `crlf.txt` keeps its owner but gets another Source.
        let [_, crlf_source, ..] = sources.as_slice() else {
            return Err("corpus has no crlf.txt source".into());
        };
        let ingress = raw.begin_input(operation(4), &budget())?;
        let edited =
            stage_text_source_in_ingress(&raw, ingress, "crlf.txt", "xx needle\r\n", None)?;
        let request = prepare_text_mutations_in_ingress(
            &raw,
            second_view,
            ingress,
            [OwnerMutation {
                owner: crlf_source.owner,
                expected: Some(crlf_source.source),
                next: Some(edited),
            }],
        )?;
        let third_view = published(&raw.submit(request, &budget())?)?;
        // A first-view cursor on `crlf.txt`, the fifth golden row.
        let held_page = raw.search_text_matches_in_read_session(
            &session,
            &NativeTextMatchesRequest {
                selection: ViewSelection::Exact(first_view),
                instance,
                query: needle.clone(),
                after: None,
                limit: 5,
            },
            &budget(),
        )?;
        let mut same_owner = held_page.next.ok_or("held page has no cursor")?;
        assert_eq!(
            (same_owner.path.as_str(), same_owner.start),
            ("crlf.txt", 0)
        );
        // Rebound to the edited view, the file keeps its owner but its
        // Source changed, so continuing would mix offsets of two texts.
        same_owner.view = third_view;
        let edited_conflict = raw.search_text_matches(
            &NativeTextMatchesRequest {
                selection: ViewSelection::Exact(third_view),
                instance,
                query: needle.clone(),
                after: Some(same_owner),
                limit: 2,
            },
            &budget(),
        );
        assert!(
            matches!(&edited_conflict, Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::Conflict),
            "{edited_conflict:?}"
        );

        // The daemon refuses a session request that selects another view.
        let foreign = raw.request(
            NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextMatches {
                session: session.session,
                request: NativeTextMatchesRequest {
                    selection: ViewSelection::Exact(second_view),
                    instance,
                    query: needle.clone(),
                    after: None,
                    limit: 2,
                },
            }),
            &budget(),
        );
        assert!(
            matches!(&foreign, Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::InvalidInput),
            "{foreign:?}"
        );

        // Without any lease, retention reclaims the first view, so only the
        // held sessions kept the continuation readable. A read of the newest
        // view retires the client's idle first-view lease before it returns;
        // the raw session is released explicitly.
        let newest = client.lexical().matches(needle.clone(), 2, None)?;
        assert_eq!(newest.view, third_view);
        let (released, _) = raw.release_read_session(&session, &budget())?;
        assert_eq!(released.state, NativeReadSessionState::Released);
        // Both releases are committed: the next retention rounds reconsider
        // the first view.
        retention.wait_released_after(retention.mark())?;
        let reclaimed = raw.search_text_matches(
            &NativeTextMatchesRequest {
                selection: ViewSelection::Exact(first_view),
                instance,
                query: needle,
                after: None,
                limit: 2,
            },
            &budget(),
        );
        assert!(
            matches!(&reclaimed, Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable),
            "retention did not reclaim the first view: {reclaimed:?}"
        );
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}

#[test]
fn exhausted_budget_and_cancellation_refuse_the_whole_match_page() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let published_view = (|| -> TestResult<ViewRef> {
        let raw = NativeUdsClient::new(socket.clone());
        let (view, _) = publish_corpus(&raw)?;
        let session = raw.open_read_session(ViewSelection::Exact(view), [62; 32], &budget())?;
        let request = NativeTextMatchesRequest {
            selection: ViewSelection::Exact(view),
            instance: native_text_spec().instance,
            query: literal("needle", NativeTextMatchCase::Sensitive),
            after: None,
            limit: 2,
        };
        // The same request answers under an open budget.
        let answered = raw.search_text_matches_in_read_session(&session, &request, &budget())?;
        assert_eq!(answered.matches.len(), 2);
        let cancelled = budget();
        cancelled.cancel_handle().cancel();
        let refused = raw.search_text_matches_in_read_session(&session, &request, &cancelled);
        assert!(refused.is_err(), "{refused:?}");
        let _released = raw.release_read_session(&session, &budget())?;
        Ok(view)
    })();
    let stopped = running.stop();
    let view = published_view?;
    stopped?;

    // One read cannot reach the selected member, let alone a Source body.
    let reopened = NativeServiceGuard::start(read_session_service_with_retention_and_work_limits(
        root.path(),
        &socket,
        Duration::from_secs(30),
        None,
        WorkLimits {
            reads: 1,
            ..work_limits()
        },
    )?);
    let refused = NativeUdsClient::new(socket).search_text_matches(
        &NativeTextMatchesRequest {
            selection: ViewSelection::Exact(view),
            instance: native_text_spec().instance,
            query: literal("needle", NativeTextMatchCase::Sensitive),
            after: None,
            limit: 2,
        },
        &budget(),
    );
    let stopped = reopened.stop();
    assert!(
        matches!(&refused, Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::ResourceLimited),
        "{refused:?}"
    );
    stopped
}
