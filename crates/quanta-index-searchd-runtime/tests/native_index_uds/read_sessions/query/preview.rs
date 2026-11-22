//! Published-source preview through the ordinary Native SDK and UDS.

use super::*;
use quanta_index_contract::native_protocol::{NativeTextExcerpt, NativeTextPreviewEvidence};
use quanta_index_contract::{HighlightSpan, PreviewByteRange, PreviewUnavailableReason};

#[test]
fn native_select_path_combines_path_and_body_matches_after_cold_reopen() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(socket.clone());
        let configured = create_text_workspace(&client)?;
        let repo_a = quanta_index_contract::RepoId::try_from("corp-a")?;
        let repo_b = quanta_index_contract::RepoId::try_from("corp-b")?;
        let publish = |expected, sequence, owner, path, content, repository| -> TestResult<_> {
            let ingress = client.begin_input(operation(sequence), &budget())?;
            let (request, source) = prepare_text_owner_in_repository(
                &client,
                expected,
                ingress,
                owner,
                path,
                content,
                Some(repository),
            )?;
            Ok((published(&client.submit(request, &budget())?)?, source))
        };
        let (path_view, path_source) = publish(
            configured,
            2,
            1,
            "src/needlepath.rs",
            "unrelated",
            repo_a.clone(),
        )?;
        // The owner order opposes the projected path order for these tied hits.
        let (body_view, body_source) =
            publish(path_view, 3, 3, "src/body.rs", "needlepath", repo_a.clone())?;
        let (tied_view, tied_source) =
            publish(body_view, 4, 2, "src/zzzz.rs", "needlepath", repo_a.clone())?;
        let (view, foreign_source) =
            publish(tied_view, 5, 4, "src/body.rs", "needlepath", repo_b.clone())?;
        let selection = ViewSelection::Exact(view);
        let instance = native_text_spec().instance;
        let query = native_text_request("select:path needlepath", 4);
        let grouped = client.search_text_paths(selection, instance, query.clone(), &budget())?;
        let preview =
            client.search_text_path_preview(selection, instance, query.clone(), &budget())?;
        let ordinary = &grouped.ranked.ranking;
        let preview_rows = preview.previews.as_deref().ok_or("missing path previews")?;
        assert!(grouped.previews.is_none());
        assert_eq!(preview.ranked, grouped.ranked);
        assert_eq!(ordinary.view, view);
        assert!(ordinary.exhaustive);
        assert_eq!(ordinary.matched, 4);
        assert_eq!(ordinary.hits.len(), 4);
        assert_eq!(preview_rows.len(), 4);

        let mut path_found = false;
        let mut body_paths = Vec::new();
        for ((hit, row), repository) in ordinary
            .hits
            .iter()
            .zip(preview_rows)
            .zip(&grouped.ranked.repositories)
        {
            assert_eq!((row.owner, row.source), (hit.owner, hit.source));
            match hit.path.as_str() {
                "src/needlepath.rs" => {
                    assert_eq!(hit.owner.stable_id, StableOwnerId([1; 16]));
                    assert_eq!(hit.source, path_source);
                    assert_eq!(row.evidence, NativeTextPreviewEvidence::Path);
                    assert_eq!(repository, &repo_a);
                    path_found = true;
                }
                path @ ("src/body.rs" | "src/zzzz.rs") => {
                    let (stable_id, source) = match (repository == &repo_a, path) {
                        (true, "src/body.rs") => (3, body_source),
                        (true, "src/zzzz.rs") => (2, tied_source),
                        (false, "src/body.rs") if repository == &repo_b => (4, foreign_source),
                        _ => return Err("unexpected repository path identity".into()),
                    };
                    assert_eq!(hit.owner.stable_id, StableOwnerId([stable_id; 16]));
                    assert_eq!(hit.source, source);
                    assert_eq!(
                        row.evidence,
                        NativeTextPreviewEvidence::Excerpt(NativeTextExcerpt {
                            text: "needlepath".into(),
                            original_focus: PreviewByteRange { start: 0, end: 10 },
                            original_context: PreviewByteRange { start: 0, end: 10 },
                            normalized_focus: PreviewByteRange { start: 0, end: 10 },
                            highlights: vec![HighlightSpan { start: 0, len: 10 }],
                        })
                    );
                    body_paths.push((repository, path, hit.score));
                }
                other => return Err(format!("unexpected select:path group: {other}").into()),
            }
        }
        assert!(path_found);
        let [
            (a, "src/body.rs", body_score),
            (same, "src/zzzz.rs", tied_score),
            (b, "src/body.rs", foreign_score),
        ] = body_paths.as_slice()
        else {
            return Err("expected body groups ordered by repository and path".into());
        };
        assert_eq!((*a, *same, *b), (&repo_a, &repo_a, &repo_b));
        assert_eq!(body_score, tied_score);
        assert_eq!(body_score, foreign_score);

        let limited_query = native_text_request("select:path needlepath", 1);
        let limited =
            client.search_text_path_preview(selection, instance, limited_query, &budget())?;
        assert!(limited.ranked.ranking.exhaustive);
        assert_eq!(limited.ranked.ranking.matched, 4);
        assert_eq!(limited.ranked.ranking.hits, ordinary.hits[..1]);
        assert_eq!(
            limited.ranked.repositories,
            grouped.ranked.repositories[..1]
        );
        assert_eq!(limited.previews.as_deref(), Some(&preview_rows[..1]));

        let plain = client.search_text(
            selection,
            instance,
            native_text_request("needlepath", 2),
            &budget(),
        )?;
        assert_eq!(plain.matched, 3);
        assert_eq!(
            plain
                .hits
                .iter()
                .map(|hit| hit.path.as_str())
                .collect::<Vec<_>>(),
            ["src/zzzz.rs", "src/body.rs"]
        );
        let boolean = client.search_text_path_preview(
            selection,
            instance,
            native_text_request("select:path unrelated AND NOT needlepath", 3),
            &budget(),
        )?;
        assert_eq!(boolean.ranked.ranking.view, view);
        assert_eq!(boolean.ranked.ranking.matched, 1);
        assert_eq!(boolean.ranked.repositories, [repo_a]);
        let ([hit], Some([evidence])) = (
            boolean.ranked.ranking.hits.as_slice(),
            boolean.previews.as_deref(),
        ) else {
            return Err("Boolean path projection must retain the body match".into());
        };
        assert_eq!(hit.path, "src/needlepath.rs");
        assert_eq!(hit.source, path_source);
        assert_eq!((evidence.owner, evidence.source), (hit.owner, hit.source));
        assert_eq!(
            evidence.evidence,
            NativeTextPreviewEvidence::Excerpt(NativeTextExcerpt {
                text: "unrelated".into(),
                original_focus: PreviewByteRange { start: 0, end: 9 },
                original_context: PreviewByteRange { start: 0, end: 9 },
                normalized_focus: PreviewByteRange { start: 0, end: 9 },
                highlights: vec![HighlightSpan { start: 0, len: 9 }],
            })
        );
        let both = client.search_text_paths(
            selection,
            instance,
            native_text_request("select:path unrelated AND needlepath", 3),
            &budget(),
        )?;
        assert_eq!(both.ranked.ranking.view, view);
        assert!(both.ranked.ranking.exhaustive);
        assert_eq!(both.ranked.ranking.matched, 0);
        assert!(both.ranked.ranking.hits.is_empty());

        let session = client.open_read_session(selection, [71; 32], &budget())?;
        assert_eq!(
            client.search_text_paths_in_read_session(
                &session,
                instance,
                query.clone(),
                &budget()
            )?,
            grouped
        );
        let page_query = native_text_request("select:path needlepath", 2);
        let mut after = None;
        let mut first_cursor = None;
        for (i, (hits, repositories)) in ordinary
            .hits
            .chunks_exact(2)
            .zip(grouped.ranked.repositories.chunks_exact(2))
            .enumerate()
        {
            let page = client.search_text_path_page_in_read_session(
                &session,
                instance,
                page_query.clone(),
                after,
                &budget(),
            )?;
            assert_eq!(page.ranked.ranking.matched, 4);
            assert_eq!(page.remaining, 4 - 2 * u64::try_from(i)?);
            assert_eq!(page.ranked.ranking.hits.as_slice(), hits);
            assert_eq!(page.ranked.repositories.as_slice(), repositories);
            after = page.after;
            if i == 0 {
                first_cursor = after.clone();
            }
        }
        assert!(after.is_none());
        let first_cursor = first_cursor.ok_or("first path page omitted continuation")?;
        let mut wrong_repository = first_cursor.clone();
        wrong_repository.repository = quanta_index_contract::RepoId::try_from("corp-c")?;
        for (binding, query, cursor) in [
            (
                "query",
                native_text_request("select:path unrelated", 2),
                first_cursor.clone(),
            ),
            ("repository", page_query, wrong_repository),
        ] {
            let refusal = client.request(
                NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPathPageV1 {
                    session: session.session,
                    instance,
                    query,
                    after: Some(cursor),
                }),
                &budget(),
            );
            assert!(
                matches!(&refusal, Err(NativeUdsError::Refused(error))
                    if error.code == NativeIndexErrorCode::InvalidInput),
                "path cursor with a mismatched {binding} returned {refusal:?}"
            );
        }
        let _released = client.release_read_session(&session, &budget())?;
        Ok((view, query, preview, limited, grouped, first_cursor))
    })();
    let stopped = running.stop();
    let (view, query, preview, limited, grouped, first_cursor) = primary?;
    stopped?;

    let reopened = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let replay = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let selection = ViewSelection::Exact(view);
        let instance = native_text_spec().instance;
        assert_eq!(
            client.search_text_path_preview(selection, instance, query.clone(), &budget())?,
            preview
        );
        assert_eq!(
            client.search_text_path_preview(
                selection,
                instance,
                native_text_request("select:path needlepath", 1),
                &budget(),
            )?,
            limited
        );
        assert_eq!(
            client.search_text_paths(selection, instance, query.clone(), &budget())?,
            grouped,
        );
        let session = client.open_read_session(selection, [72; 32], &budget())?;
        assert_eq!(
            client.search_text_paths_in_read_session(&session, instance, query, &budget())?,
            grouped
        );
        let second = client.search_text_path_page_in_read_session(
            &session,
            instance,
            native_text_request("select:path needlepath", 2),
            Some(first_cursor),
            &budget(),
        )?;
        assert_eq!(second.remaining, 2);
        assert_eq!(second.ranked.ranking.hits, grouped.ranked.ranking.hits[2..]);
        assert_eq!(second.ranked.repositories, grouped.ranked.repositories[2..]);
        assert!(second.after.is_none());
        let _released = client.release_read_session(&session, &budget())?;
        Ok(())
    })();
    let stopped = reopened.stop();
    replay?;
    stopped
}

#[test]
fn native_text_preview_keeps_rank_and_source_coordinates_across_reopen() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(socket.clone());
        let mut view = create_text_workspace(&client)?;
        for (sequence, id, path, body) in [
            (2, 1, "src/nfc.rs", "cafe\u{301} and café"),
            (3, 2, "src/alpha.rs", "alpha only let needle_xx = 1;"),
            (
                4,
                3,
                "src/beta.rs",
                r#"const VERSION: &str = "v1.2.3-rc.4";"#,
            ),
        ] {
            (view, _) = publish_text_owner(&client, view, sequence, id, path, body)?;
        }
        let instance = native_text_spec().instance;
        let selection = ViewSelection::Exact(view);
        let query = native_text_request("café", 3);
        let ordinary = client.search_text(selection, instance, query.clone(), &budget())?;
        let preview = client.search_text_preview(selection, instance, query.clone(), &budget())?;
        assert_eq!(preview.ranking, ordinary);
        assert_eq!(preview.ranking.view, view);
        assert!(preview.ranking.exhaustive);
        assert_eq!(preview.ranking.matched, 1);
        let [hit] = preview.ranking.hits.as_slice() else {
            return Err("fixed NFC query must return exactly one published source".into());
        };
        assert_eq!(hit.owner.stable_id, StableOwnerId([1; 16]));
        assert_eq!(hit.path, "src/nfc.rs");
        let [row] = preview.previews.as_slice() else {
            return Err("fixed NFC query must return exactly one preview".into());
        };
        assert_eq!((row.owner, row.source), (hit.owner, hit.source));
        let excerpt = NativeTextExcerpt {
            text: "cafe\u{301} and café".into(),
            original_focus: PreviewByteRange { start: 0, end: 6 },
            original_context: PreviewByteRange { start: 0, end: 16 },
            normalized_focus: PreviewByteRange { start: 0, end: 5 },
            highlights: vec![
                HighlightSpan { start: 0, len: 6 },
                HighlightSpan { start: 11, len: 5 },
            ],
        };
        assert_eq!(row.evidence, NativeTextPreviewEvidence::Excerpt(excerpt));

        let regex = native_text_request(r"/v\d+\.\d+\.\d+/", 3);
        let regex_hits = client.search_text(selection, instance, regex.clone(), &budget())?;
        let regex_preview = client.search_text_preview(selection, instance, regex, &budget())?;
        assert_eq!(regex_preview.ranking, regex_hits);
        assert_eq!(regex_hits.view, view);
        assert!(regex_hits.exhaustive);
        assert_eq!(regex_hits.matched, 1);
        let ([hit], [row]) = (
            regex_hits.hits.as_slice(),
            regex_preview.previews.as_slice(),
        ) else {
            return Err("whole-document regex must return one Source preview".into());
        };
        assert_eq!(hit.owner.stable_id, StableOwnerId([3; 16]));
        assert_eq!(hit.path, "src/beta.rs");
        assert_eq!((row.owner, row.source), (hit.owner, hit.source));
        let NativeTextPreviewEvidence::Excerpt(excerpt) = &row.evidence else {
            return Err("whole-document regex must return a Source excerpt".into());
        };
        assert_eq!(excerpt.text, r#"const VERSION: &str = "v1.2.3-rc.4";"#);
        let regex_miss = client.search_text(
            selection,
            instance,
            native_text_request(r"/needle_x[0-9]/", 3),
            &budget(),
        )?;
        assert!(regex_miss.exhaustive);
        assert_eq!(regex_miss.matched, 0);
        assert!(regex_miss.hits.is_empty());

        // Neither source contains both terms. A false AND must not mint evidence.
        let and_query = native_text_request("café AND alpha", 3);
        let and_ordinary = client.search_text(selection, instance, and_query.clone(), &budget())?;
        let and_preview = client.search_text_preview(selection, instance, and_query, &budget())?;
        assert_eq!(and_preview.ranking, and_ordinary);
        assert_eq!(and_preview.ranking.matched, 0);
        assert!(and_preview.ranking.hits.is_empty());
        assert!(and_preview.previews.is_empty());

        let not_query = native_text_request("café AND NOT café", 3);
        let not_ordinary = client.search_text(selection, instance, not_query.clone(), &budget())?;
        let not_preview = client.search_text_preview(selection, instance, not_query, &budget())?;
        assert_eq!(not_preview.ranking, not_ordinary);
        assert_eq!(not_preview.ranking.matched, 0);
        assert!(not_preview.previews.is_empty());

        // The beta source matches only through NOT and has no positive body witness.
        let or_not_query = native_text_request("café OR NOT alpha", 3);
        let or_not_ordinary =
            client.search_text(selection, instance, or_not_query.clone(), &budget())?;
        let or_not_preview =
            client.search_text_preview(selection, instance, or_not_query, &budget())?;
        assert_eq!(or_not_preview.ranking, or_not_ordinary);
        assert_eq!(or_not_preview.ranking.matched, 2);
        assert_eq!(or_not_preview.previews.len(), 2);
        for (hit, row) in or_not_preview
            .ranking
            .hits
            .iter()
            .zip(&or_not_preview.previews)
        {
            assert_eq!((row.owner, row.source), (hit.owner, hit.source));
            if hit.owner.stable_id == StableOwnerId([1; 16]) {
                assert_eq!(row.evidence, preview.previews[0].evidence);
            } else if hit.owner.stable_id == StableOwnerId([3; 16]) {
                assert_eq!(
                    row.evidence,
                    NativeTextPreviewEvidence::Unavailable(
                        PreviewUnavailableReason::NoPositiveWitness
                    )
                );
            } else {
                return Err(format!("unexpected OR/NOT owner: {:?}", hit.owner.stable_id).into());
            }
        }
        Ok((view, query, ordinary, preview))
    })();
    let stopped = running.stop();
    let (view, query, ordinary, preview) = primary?;
    stopped?;

    let reopened = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let replay = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let selection = ViewSelection::Exact(view);
        let instance = native_text_spec().instance;
        assert_eq!(
            client.search_text(selection, instance, query.clone(), &budget())?,
            ordinary
        );
        assert_eq!(
            client.search_text_preview(selection, instance, query, &budget())?,
            preview
        );
        Ok(())
    })();
    let stopped = reopened.stop();
    replay?;
    stopped
}

#[test]
fn native_text_preview_keeps_source_byte_boundary_and_refuses_oversized() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let configured = create_text_workspace(&client)?;
        let boundary_body = format!("café{}", " ".repeat(65_531));
        assert_eq!(boundary_body.len(), 65_536);
        let (boundary_view, boundary_source) = publish_text_owner(
            &client,
            configured,
            2,
            1,
            "src/nfc-boundary.rs",
            &boundary_body,
        )?;
        let boundary_selection = ViewSelection::Exact(boundary_view);
        let instance = native_text_spec().instance;
        let boundary_query = native_text_request("café", 1);
        let boundary_rank = client.search_text(
            boundary_selection,
            instance,
            boundary_query.clone(),
            &budget(),
        )?;
        let boundary_preview =
            client.search_text_preview(boundary_selection, instance, boundary_query, &budget())?;
        assert_eq!(boundary_preview.ranking, boundary_rank);
        assert_eq!(boundary_preview.ranking.view, boundary_view);
        assert!(boundary_preview.ranking.exhaustive);
        assert_eq!(boundary_preview.ranking.matched, 1);
        let [boundary_hit] = boundary_preview.ranking.hits.as_slice() else {
            return Err("NFC source at the byte cap must remain ranked".into());
        };
        assert_eq!(boundary_hit.owner.stable_id, StableOwnerId([1; 16]));
        assert_eq!(boundary_hit.source, boundary_source);
        assert_eq!(boundary_hit.path, "src/nfc-boundary.rs");
        let [boundary_row] = boundary_preview.previews.as_slice() else {
            return Err("NFC source at the byte cap must have one preview".into());
        };
        assert_eq!(
            (boundary_row.owner, boundary_row.source),
            (boundary_hit.owner, boundary_source)
        );
        assert_eq!(
            boundary_row.evidence,
            NativeTextPreviewEvidence::Excerpt(NativeTextExcerpt {
                text: format!("café{}", " ".repeat(235)),
                original_focus: PreviewByteRange { start: 0, end: 5 },
                original_context: PreviewByteRange { start: 0, end: 240 },
                normalized_focus: PreviewByteRange { start: 0, end: 5 },
                highlights: vec![HighlightSpan { start: 0, len: 5 }],
            })
        );

        let body = format!("needle{}", " ".repeat(65_531));
        assert_eq!(body.len(), 65_537);
        let (view, source) =
            publish_text_owner(&client, boundary_view, 3, 2, "src/long.rs", &body)?;
        let selection = ViewSelection::Exact(view);
        let query = native_text_request("needle", 1);
        let ordinary = client.search_text(selection, instance, query.clone(), &budget())?;
        let preview = client.search_text_preview(selection, instance, query, &budget())?;
        assert_eq!(preview.ranking, ordinary);
        assert_eq!(preview.ranking.view, view);
        assert!(preview.ranking.exhaustive);
        assert_eq!(preview.ranking.matched, 1);
        let [hit] = preview.ranking.hits.as_slice() else {
            return Err("oversized source must remain in ordinary Text ranking".into());
        };
        assert_eq!(hit.owner.stable_id, StableOwnerId([2; 16]));
        assert_eq!(hit.source, source);
        assert_eq!(hit.path, "src/long.rs");
        let [row] = preview.previews.as_slice() else {
            return Err("oversized source must have one preview decision".into());
        };
        assert_eq!((row.owner, row.source), (hit.owner, source));
        assert_eq!(
            row.evidence,
            NativeTextPreviewEvidence::Unavailable(PreviewUnavailableReason::WorkBudget)
        );
        Ok(())
    })();
    let stopped = running.stop();
    primary?;
    stopped
}

#[test]
fn native_text_preview_work_refusal_keeps_ranked_suffix_after_cold_reopen() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(socket.clone());
        let configured = create_text_workspace(&client)?;
        let body = format!("{}{}", "needle ".repeat(256), " ".repeat(63_744));
        assert_eq!(body.len(), 65_536);
        let (first_view, first_source) =
            publish_text_owner(&client, configured, 2, 1, "src/long-needle.rs", &body)?;
        let (view, second_source) =
            publish_text_owner(&client, first_view, 3, 2, "src/short-needle.rs", "needle")?;
        let instance = native_text_spec().instance;
        let ordinary = client.search_text(
            ViewSelection::Exact(view),
            instance,
            native_text_request("needle", 2),
            &budget(),
        )?;
        assert_eq!(ordinary.view, view);
        assert!(ordinary.exhaustive);
        assert_eq!(ordinary.matched, 2);
        let [first, second] = ordinary.hits.as_slice() else {
            return Err("fixed Work fixture must rank two durable sources".into());
        };
        assert_eq!(
            (first.owner.stable_id, first.source, first.path.as_str()),
            (StableOwnerId([1; 16]), first_source, "src/long-needle.rs")
        );
        assert_eq!(
            (second.owner.stable_id, second.source, second.path.as_str()),
            (StableOwnerId([2; 16]), second_source, "src/short-needle.rs")
        );
        Ok((view, ordinary))
    })();
    let stopped = running.stop();
    let (view, expected) = primary?;
    stopped?;

    // The first body fits the source cap but its preview-only source read and
    // tokenization exceed this original Work allowance. Ranking still fits.
    let limited = WorkLimits {
        bytes: 100 * 1024,
        ..work_limits()
    };
    let reopened = NativeServiceGuard::start(read_session_service_with_retention_and_work_limits(
        root.path(),
        &socket,
        Duration::from_secs(30),
        None,
        limited,
    )?);
    let replay = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let selection = ViewSelection::Exact(view);
        let instance = native_text_spec().instance;
        let query = native_text_request("needle", 2);
        let ordinary = client
            .search_text(selection, instance, query.clone(), &budget())
            .map_err(|error| format!("ordinary Text at 100 KiB Work: {error}"))?;
        assert_eq!(ordinary, expected);
        let preview = client
            .search_text_preview(selection, instance, query, &budget())
            .map_err(|error| format!("Text preview at 100 KiB Work: {error}"))?;
        assert_eq!(preview.ranking, ordinary);
        let [first_hit, second_hit] = preview.ranking.hits.as_slice() else {
            return Err("limited Work ranking lost a durable hit".into());
        };
        let [first_row, second_row] = preview.previews.as_slice() else {
            return Err("limited Work preview lost a ranked decision".into());
        };
        assert_eq!(
            (first_row.owner, first_row.source),
            (first_hit.owner, first_hit.source)
        );
        assert_eq!(
            (second_row.owner, second_row.source),
            (second_hit.owner, second_hit.source)
        );
        assert_eq!(second_hit.path, "src/short-needle.rs");
        assert_eq!(
            first_row.evidence,
            NativeTextPreviewEvidence::Unavailable(PreviewUnavailableReason::WorkBudget)
        );
        assert!(matches!(
            second_row.evidence,
            NativeTextPreviewEvidence::Excerpt(_)
        ));
        Ok(())
    })();
    let stopped = reopened.stop();
    replay?;
    stopped
}

fn publish_path_source_change<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    expected_view: quanta_index_contract::index::ViewRef,
    sequence: u64,
    stable_id: u8,
    expected_source: ObjectRef,
    next: Option<(&quanta_index_contract::RepoId, &str, &str)>,
) -> TestResult<(quanta_index_contract::index::ViewRef, Option<ObjectRef>)> {
    let ingress = client.begin_input(operation(sequence), &budget())?;
    let next_source = if let Some((repository, path, body)) = next {
        Some(stage_text_source_in_ingress(
            client,
            ingress,
            path,
            body,
            Some(repository.clone()),
        )?)
    } else {
        None
    };
    let request = prepare_text_owner_mutation_in_ingress(
        client,
        expected_view,
        ingress,
        stable_id,
        Some(expected_source),
        next_source,
    )?;
    let view = published(&client.submit(request, &budget())?)?;
    Ok((view, next_source))
}

#[test]
fn native_select_path_rename_and_delete_survive_published_cold_reopen() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let running = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let primary = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(socket.clone());
        let configured = create_text_workspace(&client)?;
        let repository = quanta_index_contract::RepoId::try_from("corp-path")?;
        let publish = |expected, sequence, stable_id, path| -> TestResult<_> {
            let ingress = client.begin_input(operation(sequence), &budget())?;
            let (request, source) = prepare_text_owner_in_repository(
                &client,
                expected,
                ingress,
                stable_id,
                path,
                "bodyneutral",
                Some(repository.clone()),
            )?;
            Ok((published(&client.submit(request, &budget())?)?, source))
        };
        let (old_view, old_source) = publish(configured, 2, 1, "src/oldneedle.rs")?;
        let (preimage, deleted_source) = publish(old_view, 3, 2, "src/deletetoken.rs")?;
        let instance = native_text_spec().instance;
        for (term, path, source, stable_id) in [
            ("oldneedle", "src/oldneedle.rs", old_source, 1_u8),
            ("deletetoken", "src/deletetoken.rs", deleted_source, 2_u8),
        ] {
            let result = client.search_text_path_preview(
                ViewSelection::Exact(preimage),
                instance,
                native_text_request(&format!("select:path {term}"), 2),
                &budget(),
            )?;
            assert_eq!(result.ranked.ranking.matched, 1);
            assert_eq!(
                result.ranked.repositories.as_slice(),
                std::slice::from_ref(&repository)
            );
            let [hit] = result.ranked.ranking.hits.as_slice() else {
                return Err("preimage path-only hit is missing".into());
            };
            assert_eq!(
                (hit.path.as_str(), hit.source, hit.owner.stable_id),
                (path, source, StableOwnerId([stable_id; 16]))
            );
            let Some([preview]) = result.previews.as_deref() else {
                return Err("preimage path-only evidence is missing".into());
            };
            assert_eq!(preview.evidence, NativeTextPreviewEvidence::Path);
        }

        let (renamed_view, new_source) = publish_path_source_change(
            &client,
            preimage,
            4,
            1,
            old_source,
            Some((&repository, "src/newneedle.rs", "bodyneutral")),
        )?;
        let new_source = new_source.ok_or("rename did not stage a source")?;
        assert_ne!(new_source, old_source);
        let renamed_before_delete = client.search_text_path_preview(
            ViewSelection::Exact(renamed_view),
            instance,
            native_text_request("select:path newneedle", 2),
            &budget(),
        )?;
        let ranking = &renamed_before_delete.ranked.ranking;
        assert_eq!(ranking.matched, 1);
        assert_eq!(ranking.hits[0].source, new_source);
        assert_eq!(ranking.hits[0].path, "src/newneedle.rs");
        assert_eq!(
            renamed_before_delete.ranked.repositories.as_slice(),
            std::slice::from_ref(&repository)
        );
        let Some([preview]) = renamed_before_delete.previews.as_deref() else {
            return Err("renamed path-only evidence is missing before delete".into());
        };
        assert_eq!(preview.evidence, NativeTextPreviewEvidence::Path);
        let old_after_rename = client.search_text_path_preview(
            ViewSelection::Exact(renamed_view),
            instance,
            native_text_request("select:path oldneedle", 2),
            &budget(),
        )?;
        assert_eq!(old_after_rename.ranked.ranking.matched, 0);
        assert!(old_after_rename.ranked.ranking.hits.is_empty());
        assert!(old_after_rename.ranked.repositories.is_empty());
        let (deleted_view, _) =
            publish_path_source_change(&client, renamed_view, 5, 2, deleted_source, None)?;
        let renamed = client.search_text_path_preview(
            ViewSelection::Exact(deleted_view),
            instance,
            native_text_request("select:path newneedle", 2),
            &budget(),
        )?;
        assert_eq!(renamed.ranked.ranking.matched, 1);
        assert_eq!(
            renamed.ranked.repositories.as_slice(),
            std::slice::from_ref(&repository)
        );
        let [hit] = renamed.ranked.ranking.hits.as_slice() else {
            return Err("renamed path-only hit is missing".into());
        };
        assert_eq!(
            (hit.path.as_str(), hit.source, hit.owner.stable_id),
            ("src/newneedle.rs", new_source, StableOwnerId([1; 16]))
        );
        let Some([preview]) = renamed.previews.as_deref() else {
            return Err("renamed path-only evidence is missing after delete".into());
        };
        assert_eq!(preview.evidence, NativeTextPreviewEvidence::Path);
        for term in ["oldneedle", "deletetoken"] {
            let stale = client.search_text_path_preview(
                ViewSelection::Exact(deleted_view),
                instance,
                native_text_request(&format!("select:path {term}"), 2),
                &budget(),
            )?;
            assert_eq!(stale.ranked.ranking.matched, 0);
            assert!(stale.ranked.ranking.hits.is_empty());
            assert!(stale.ranked.repositories.is_empty());
        }
        Ok((deleted_view, renamed))
    })();
    let stopped = running.stop();
    let (view, renamed) = primary?;
    stopped?;

    let reopened = NativeServiceGuard::start(read_session_service(
        root.path(),
        &socket,
        Duration::from_secs(30),
    )?);
    let replay = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        let instance = native_text_spec().instance;
        assert_eq!(
            client.search_text_path_preview(
                ViewSelection::Exact(view),
                instance,
                native_text_request("select:path newneedle", 2),
                &budget(),
            )?,
            renamed
        );
        for term in ["oldneedle", "deletetoken"] {
            let stale = client.search_text_path_preview(
                ViewSelection::Exact(view),
                instance,
                native_text_request(&format!("select:path {term}"), 2),
                &budget(),
            )?;
            assert_eq!(stale.ranked.ranking.matched, 0);
            assert!(stale.ranked.ranking.hits.is_empty());
            assert!(stale.ranked.repositories.is_empty());
        }
        Ok(())
    })();
    let stopped = reopened.stop();
    replay?;
    stopped
}
