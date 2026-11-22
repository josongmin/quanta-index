//! F15's source admission oracle through the published Native SDK route.
//! Both excluded bodies retain paths and raw bytes without becoming text.

use super::*;
use quanta_index_contract::native_protocol::NativeTextPreviewEvidence;
use quanta_index_core::index::{MemoryLifetime, SourceRecordOwner};
use sha2::{Digest as _, Sha256};

const BINARY_PATH: &str = "src/binarystable.dat";
const UTF8_PATH: &str = "src/utf8stable.dat";
const BINARY_BODY: &[u8] = b"\xff\x00";
const EXCLUDED_UTF8_BODY: &[u8] = b"hiddenbodytoken";
const ADMITTED_BODY: &[u8] = b"visiblebodytoken";
const BINARY_SHA256: &str = "ea5dbf9596d187e9500f23e9a680109475341cf4e81f7e043f7d97152c10772f";
const EXCLUDED_UTF8_SHA256: &str =
    "5fb108bb088f2307c6400aa0de25ea6cfde144aa86e6c6dcdfbffdd6a847ad81";

fn advance(
    client: &NativeUdsClient,
    expected: quanta_index_contract::index::ViewRef,
    ingress: IngressRef,
    mutations: Vec<OwnerMutation>,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let request = prepare_text_mutations_in_ingress(client, expected, ingress, mutations)?;
    published(&client.submit(request, &budget())?)
}

fn mutation(id: u8, expected: Option<ObjectRef>, next: Option<ObjectRef>) -> OwnerMutation {
    OwnerMutation {
        owner: owner(id),
        expected,
        next,
    }
}

fn stage(
    client: &NativeUdsClient,
    ingress: IngressRef,
    inputs: Vec<SourceInput>,
) -> TestResult<Vec<ObjectRef>> {
    let NativeIndexResponse::StagedSources(sources) = client.request(
        NativeIndexRequest::StageRepositorySources {
            ingress,
            repository: quanta_index_contract::RepoId::try_from("source-a")?,
            inputs,
        },
        &budget(),
    )?
    else {
        return Err("F15 repository source staging returned another response".into());
    };
    Ok(sources)
}

fn source(path: &str, bytes: &[u8], text: SourceTextAdmission) -> SourceInput {
    SourceInput {
        path: path.into(),
        bytes: bytes.to_vec(),
        text,
        language: None,
    }
}

fn assert_path(
    client: &NativeUdsClient,
    view: quanta_index_contract::index::ViewRef,
    term: &str,
    path: &str,
    source: ObjectRef,
    id: u8,
) -> TestResult {
    let reply = client.search_text_path_preview(
        ViewSelection::Exact(view),
        native_text_spec().instance,
        native_text_request(&format!("select:path {term}"), 2),
        &budget(),
    )?;
    assert_eq!(reply.ranked.ranking.view, view);
    assert!(reply.ranked.ranking.exhaustive);
    assert_eq!(reply.ranked.ranking.matched, 1);
    assert_eq!(
        reply.ranked.repositories,
        [quanta_index_contract::RepoId::try_from("source-a")?]
    );
    let [hit] = reply.ranked.ranking.hits.as_slice() else {
        return Err(format!("{term}: expected one path hit").into());
    };
    assert_eq!(
        (hit.path.as_str(), hit.source, hit.owner),
        (path, source, owner(id))
    );
    let Some([preview]) = reply.previews.as_deref() else {
        return Err(format!("{term}: expected one path preview").into());
    };
    assert_eq!((preview.owner, preview.source), (hit.owner, hit.source));
    assert_eq!(preview.evidence, NativeTextPreviewEvidence::Path);
    Ok(())
}

fn assert_absent(
    client: &NativeUdsClient,
    view: quanta_index_contract::index::ViewRef,
    query: &str,
) -> TestResult {
    let reply = client.search_text_path_preview(
        ViewSelection::Exact(view),
        native_text_spec().instance,
        native_text_request(query, 2),
        &budget(),
    )?;
    assert_eq!(reply.ranked.ranking.view, view);
    assert!(reply.ranked.ranking.exhaustive);
    assert_eq!(reply.ranked.ranking.matched, 0, "{query}");
    assert!(reply.ranked.ranking.hits.is_empty(), "{query}");
    assert!(reply.ranked.repositories.is_empty(), "{query}");
    assert!(
        reply.previews.as_deref().is_none_or(|rows| rows.is_empty()),
        "{query}"
    );
    Ok(())
}

fn assert_body(
    client: &NativeUdsClient,
    view: quanta_index_contract::index::ViewRef,
    term: &str,
    expected_source: Option<ObjectRef>,
) -> TestResult {
    let reply = client.search_text(
        ViewSelection::Exact(view),
        native_text_spec().instance,
        native_text_request(term, 2),
        &budget(),
    )?;
    assert_eq!(reply.view, view);
    assert!(reply.exhaustive);
    assert_eq!(
        reply.matched,
        u64::from(expected_source.is_some()),
        "{term}"
    );
    match (reply.hits.as_slice(), expected_source) {
        ([], None) => Ok(()),
        ([hit], Some(expected)) if hit.source == expected && hit.path == BINARY_PATH => Ok(()),
        _ => Err(format!("{term}: body result differs: {:?}", reply.hits).into()),
    }
}

fn assert_noop_source_successor(
    client: &NativeUdsClient,
    before: quanta_index_contract::index::ViewRef,
    after: quanta_index_contract::index::ViewRef,
) -> TestResult {
    let prior = client.read_view(ViewSelection::Exact(before), &budget())?;
    let successor = client.read_view(ViewSelection::Exact(after), &budget())?;
    assert_eq!(
        after.sequence,
        before
            .sequence
            .checked_add(1)
            .ok_or("view sequence overflow")?
    );
    assert_eq!(successor.source.owners, prior.source.owners);
    assert_eq!(successor.source.paths, prior.source.paths);
    assert_ne!(successor.source.frontiers, prior.source.frontiers);
    Ok(())
}

fn assert_deleted(
    client: &NativeUdsClient,
    view: quanta_index_contract::index::ViewRef,
) -> TestResult {
    for term in ["binarystable", "utf8stable"] {
        assert_absent(client, view, &format!("select:path {term}"))?;
    }
    for term in ["hiddenbodytoken", "visiblebodytoken"] {
        assert_body(client, view, term, None)?;
    }
    Ok(())
}

fn assert_excluded_source_bytes(
    root: &std::path::Path,
    source: ObjectRef,
    path: &str,
    expected: &[u8],
    expected_sha256: &str,
) -> TestResult {
    // The query's source ObjectRef is independently read from the immutable
    // store after the service releases its namespace. The fixed body is the
    // digest oracle, not a digest copied from the source record.
    let objects = FileObjectStore::open(root.join("native-objects"))?;
    let mut work = WorkBudget::with_request(work_limits(), budget());
    let stored = objects.get(source, &mut work)?;
    assert_eq!(stored.registration.reference, source);
    let record =
        SourceRecordOwner::decode_json(&stored.payload, &mut work, MemoryLifetime::Retained)?;
    assert_eq!(record.path, path);
    assert_eq!(record.text, SourceTextAdmission::Excluded);
    assert_eq!(
        record.source_repo_id,
        Some(quanta_index_contract::RepoId::try_from("source-a")?)
    );
    assert_eq!(record.byte_len, u64::try_from(expected.len())?);
    assert_eq!(stored.registration.children.as_slice(), [record.bytes]);
    let body = objects.get(record.bytes, &mut work)?;
    assert_eq!(body.registration.reference, record.bytes);
    assert!(body.registration.children.is_empty());
    assert_eq!(body.payload, expected);
    assert_eq!(
        format!("{:x}", Sha256::digest(&body.payload)),
        expected_sha256
    );
    Ok(())
}

#[test]
fn excluded_binary_and_utf8_source_stay_path_only_through_update_noop_delete_and_reopen()
-> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    // Keep G1 available after G4. The explicit read session still proves the
    // pinned read path; this fixture does not test asynchronous GC policy.
    let running = NativeServiceGuard::start(read_session_service_with_retention(
        root.path(),
        &socket,
        Duration::from_secs(300),
        None,
    )?);
    let result = (|| -> TestResult<_> {
        let client = NativeUdsClient::new(socket.clone());
        let configured = create_text_workspace(&client)?;

        let ingress = client.begin_input(operation(2), &budget())?;
        let staged = stage(
            &client,
            ingress,
            vec![
                source(BINARY_PATH, BINARY_BODY, SourceTextAdmission::Excluded),
                source(UTF8_PATH, EXCLUDED_UTF8_BODY, SourceTextAdmission::Excluded),
            ],
        )?;
        let [binary_source, utf8_source] = staged.as_slice() else {
            return Err("G1 did not stage both excluded sources".into());
        };
        let (binary_source, utf8_source) = (*binary_source, *utf8_source);
        let g1 = advance(
            &client,
            configured,
            ingress,
            vec![
                mutation(1, None, Some(binary_source)),
                mutation(2, None, Some(utf8_source)),
            ],
        )?;
        let held = client.open_read_session(ViewSelection::Exact(g1), [81; 32], &budget())?;
        assert_eq!(held.view, g1);
        assert_path(&client, g1, "binarystable", BINARY_PATH, binary_source, 1)?;
        assert_path(&client, g1, "utf8stable", UTF8_PATH, utf8_source, 2)?;
        assert_body(&client, g1, "hiddenbodytoken", None)?;

        let ingress = client.begin_input(operation(3), &budget())?;
        let staged = stage(
            &client,
            ingress,
            vec![source(
                BINARY_PATH,
                ADMITTED_BODY,
                SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
            )],
        )?;
        let [visible_source] = staged.as_slice() else {
            return Err("G2 did not stage its admitted replacement".into());
        };
        let visible_source = *visible_source;
        let g2 = advance(
            &client,
            g1,
            ingress,
            vec![mutation(1, Some(binary_source), Some(visible_source))],
        )?;
        assert_path(&client, g2, "binarystable", BINARY_PATH, visible_source, 1)?;
        assert_path(&client, g2, "utf8stable", UTF8_PATH, utf8_source, 2)?;
        assert_body(&client, g2, "visiblebodytoken", Some(visible_source))?;
        assert_body(&client, g2, "hiddenbodytoken", None)?;

        // Same-owner/same-source is the published Native no-op used by the
        // Kernel oracle; it advances the event without changing source bytes.
        let ingress = client.begin_input(operation(4), &budget())?;
        let g3 = advance(
            &client,
            g2,
            ingress,
            vec![mutation(1, Some(visible_source), Some(visible_source))],
        )?;
        assert_noop_source_successor(&client, g2, g3)?;
        assert_path(&client, g3, "binarystable", BINARY_PATH, visible_source, 1)?;
        assert_path(&client, g3, "utf8stable", UTF8_PATH, utf8_source, 2)?;
        assert_body(&client, g3, "visiblebodytoken", Some(visible_source))?;
        assert_body(&client, g3, "hiddenbodytoken", None)?;

        let ingress = client.begin_input(operation(5), &budget())?;
        let g4 = advance(
            &client,
            g3,
            ingress,
            vec![
                mutation(1, Some(visible_source), None),
                mutation(2, Some(utf8_source), None),
            ],
        )?;
        assert_deleted(&client, g4)?;
        for (term, path, source, id) in [
            ("binarystable", BINARY_PATH, binary_source, 1),
            ("utf8stable", UTF8_PATH, utf8_source, 2),
        ] {
            let reply = client.search_text_paths_in_read_session(
                &held,
                native_text_spec().instance,
                native_text_request(&format!("select:path {term}"), 2),
                &budget(),
            )?;
            assert_eq!(reply.ranked.ranking.view, g1);
            assert!(reply.ranked.ranking.exhaustive);
            assert_eq!(reply.ranked.ranking.matched, 1);
            assert_eq!(
                reply.ranked.repositories,
                [quanta_index_contract::RepoId::try_from("source-a")?]
            );
            let [hit] = reply.ranked.ranking.hits.as_slice() else {
                return Err(format!("held G1 did not return exactly one path for {term}").into());
            };
            assert_eq!(
                (hit.path.as_str(), hit.source, hit.owner),
                (path, source, owner(id))
            );
        }
        for term in ["hiddenbodytoken", "visiblebodytoken"] {
            let reply = client.search_text_in_read_session(
                &held,
                native_text_spec().instance,
                native_text_request(term, 2),
                &budget(),
            )?;
            assert_eq!(reply.view, g1);
            assert!(reply.exhaustive);
            assert_eq!(reply.matched, 0, "{term}");
            assert!(reply.hits.is_empty(), "{term}");
        }
        let _released = client.release_read_session(&held, &budget())?;
        Ok((g1, g2, g3, g4, binary_source, utf8_source, visible_source))
    })();
    let stopped = running.stop();
    let (g1, g2, g3, g4, binary_source, utf8_source, visible_source) = result?;
    stopped?;

    assert_excluded_source_bytes(
        root.path(),
        binary_source,
        BINARY_PATH,
        BINARY_BODY,
        BINARY_SHA256,
    )?;
    assert_excluded_source_bytes(
        root.path(),
        utf8_source,
        UTF8_PATH,
        EXCLUDED_UTF8_BODY,
        EXCLUDED_UTF8_SHA256,
    )?;

    let reopened = NativeServiceGuard::start(read_session_service_with_retention(
        root.path(),
        &socket,
        Duration::from_secs(300),
        None,
    )?);
    let replay = (|| -> TestResult {
        let client = NativeUdsClient::new(socket);
        assert_noop_source_successor(&client, g2, g3)?;
        assert_path(&client, g1, "binarystable", BINARY_PATH, binary_source, 1)?;
        assert_path(&client, g1, "utf8stable", UTF8_PATH, utf8_source, 2)?;
        assert_body(&client, g1, "hiddenbodytoken", None)?;
        for view in [g2, g3] {
            assert_path(
                &client,
                view,
                "binarystable",
                BINARY_PATH,
                visible_source,
                1,
            )?;
            assert_path(&client, view, "utf8stable", UTF8_PATH, utf8_source, 2)?;
            assert_body(&client, view, "visiblebodytoken", Some(visible_source))?;
            assert_body(&client, view, "hiddenbodytoken", None)?;
        }
        assert_deleted(&client, g4)?;
        Ok(())
    })();
    let stopped = reopened.stop();
    replay?;
    stopped
}
