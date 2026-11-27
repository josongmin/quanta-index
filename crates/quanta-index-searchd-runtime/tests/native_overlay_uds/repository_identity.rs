//! Overlay identity is `(repository, path)` in a two-repository selected view.

use super::*;
use quanta_index_contract::{
    RepoId,
    index::{IngressRef, ObjectRef, SourceInput},
    native_protocol::{NativeOverlayTextQuery, NativeOverlayTextResult, NativeOverlayTextSource},
};

const SHARED: &str = "src/shared.rs";

pub(super) fn repository_owner(id: u8) -> OwnerKey {
    OwnerKey {
        producer: operation(1).producer,
        stable_id: StableOwnerId([id; 16]),
    }
}

fn stage_repository(
    client: &NativeUdsClient,
    ingress: IngressRef,
    repository: &RepoId,
    text: &str,
) -> TestResult<ObjectRef> {
    stage_repository_at(client, ingress, repository, SHARED, text)
}

fn stage_repository_at(
    client: &NativeUdsClient,
    ingress: IngressRef,
    repository: &RepoId,
    path: &str,
    text: &str,
) -> TestResult<ObjectRef> {
    stage_source(client, ingress, repository, (path, text), None)
}

/// Stage one `(path, text)` Source of `repository` with its language.
pub(super) fn stage_source(
    client: &NativeUdsClient,
    ingress: IngressRef,
    repository: &RepoId,
    (path, text): (&str, &str),
    language: Option<quanta_index_contract::lex::LanguageCode>,
) -> TestResult<ObjectRef> {
    let response = client.request(
        NativeIndexRequest::StageRepositorySources {
            ingress,
            repository: repository.clone(),
            inputs: vec![SourceInput {
                path: path.into(),
                bytes: text.as_bytes().to_vec(),
                text: SourceTextAdmission::Utf8 {
                    policy: Digest([31; 32]),
                },
                language,
            }],
        },
        &budget(),
    )?;
    let NativeIndexResponse::StagedSources(records) = response else {
        return Err(format!("unexpected repository staging response: {response:?}").into());
    };
    let [record] = records.as_slice() else {
        return Err("repository staging returned an unexpected record count".into());
    };
    Ok(*record)
}

/// Owner 10 is `repo-a:src/shared.rs` ("shared alpha") and owner 11 is
/// `repo-b:src/shared.rs` ("shared beta").
/// A created workspace whose selected view has the Text profile and no source.
pub(super) fn configure_empty_view(client: &NativeUdsClient) -> TestResult<ViewRef> {
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("workspace creation did not return a view".into());
    };
    let profile_ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        profile_ingress,
        IndexProfile {
            instances: vec![native_text_spec()],
        },
        &budget(),
    )?;
    let configured = published(
        client,
        operation(1),
        &client.submit(
            UpdateRequest {
                operation: operation(1),
                expected_view: genesis,
                kind: UpdateKind::Reconfigure { profile },
            },
            &budget(),
        )?,
    )?;
    Ok(configured)
}

fn publish_two_repositories(
    client: &NativeUdsClient,
) -> TestResult<(ViewRef, ObjectRef, ObjectRef)> {
    publish_two_repositories_at(client, SHARED)
}

fn publish_two_repositories_at(
    client: &NativeUdsClient,
    path: &str,
) -> TestResult<(ViewRef, ObjectRef, ObjectRef)> {
    let configured = configure_empty_view(client)?;
    let key = operation(2);
    let ingress = client.begin_input(key, &budget())?;
    let source_a = stage_repository_at(
        client,
        ingress,
        &RepoId::try_from("repo-a")?,
        path,
        "shared alpha",
    )?;
    let source_b = stage_repository_at(
        client,
        ingress,
        &RepoId::try_from("repo-b")?,
        path,
        "shared beta",
    )?;
    let owners = client.stage_mutations(
        ingress,
        [(10_u8, source_a), (11, source_b)]
            .into_iter()
            .map(|(id, record)| OwnerMutation {
                owner: repository_owner(id),
                expected: None,
                next: Some(record),
            }),
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: client
            .read_view(ViewSelection::Exact(configured), &budget())?
            .source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: key.stream_epoch,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let selected = published(
        client,
        key,
        &client.submit(
            UpdateRequest {
                operation: key,
                expected_view: configured,
                kind: UpdateKind::SourceAdvance(transition),
            },
            &budget(),
        )?,
    )?;
    Ok((selected, source_a, source_b))
}

fn query(
    client: &NativeUdsClient,
    selected: ViewRef,
    session: NativeOverlaySession,
    word: &str,
) -> Result<NativeOverlayTextResult, NativeUdsError> {
    client.search_overlay_text(
        NativeOverlayTextQuery {
            selection: ViewSelection::Exact(selected),
            instance: native_text_spec().instance,
            session,
            query: text_request(word),
            expected_corpus: None,
        },
        &budget(),
    )
}

fn query_latest(
    client: &NativeUdsClient,
    session: NativeOverlaySession,
    word: &str,
) -> Result<NativeOverlayTextResult, NativeUdsError> {
    client.search_overlay_text(
        NativeOverlayTextQuery {
            selection: ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            instance: native_text_spec().instance,
            session,
            query: text_request(word),
            expected_corpus: None,
        },
        &budget(),
    )
}

/// Exhaustive hits ordered by owner as `(owner id, origin, "repository:path")`.
fn hits(
    client: &NativeUdsClient,
    selected: ViewRef,
    session: NativeOverlaySession,
    word: &str,
) -> TestResult<Vec<(u8, NativeOverlayTextSource, String)>> {
    let result = query(client, selected, session, word)
        .map_err(|error| format!("overlay query {word:?}: {error:?}"))?;
    if !result.exhaustive || usize::try_from(result.matched)? != result.hits.len() {
        return Err("repository overlay gold expects one exhaustive page".into());
    }
    let mut rows = Vec::new();
    for hit in result.hits {
        let id = hit
            .owner
            .stable_id
            .0
            .first()
            .copied()
            .ok_or("owner id is missing")?;
        let repository = hit.repository.as_ref().map_or("", RepoId::as_str);
        rows.push((id, hit.source, format!("{repository}:{}", hit.path)));
    }
    rows.sort_by_key(|(id, _, _)| *id);
    Ok(rows)
}

fn row(
    id: u8,
    source: NativeOverlayTextSource,
    repository: &str,
) -> (u8, NativeOverlayTextSource, String) {
    (id, source, format!("{repository}:{SHARED}"))
}

pub(super) fn update(
    client: &NativeUdsClient,
    live: &LivePublisher<'_>,
    document: NativeOverlayDocument,
    expected_revision: Option<u64>,
    revision: u64,
    text: Option<&str>,
    path: Option<&str>,
    repository_witness: Option<OwnerKey>,
) -> TestResult {
    live.renew()?;
    let reply = client
        .update_overlay_at_path(
            document,
            expected_revision,
            revision,
            text.map(str::to_owned),
            path.map(str::to_owned),
            repository_witness,
            &budget(),
        )
        .map_err(|error| format!("overlay update {revision}: {error:?}"))?;
    require(
        reply == NativeOverlayReply::Updated { document, revision },
        "overlay revision was not accepted",
    )
}

fn refused(
    result: Result<NativeOverlayTextResult, NativeUdsError>,
    code: NativeIndexErrorCode,
) -> bool {
    matches!(result, Err(NativeUdsError::Refused(error)) if error.code == code)
}

#[test]
fn overlay_masks_and_conflicts_per_repository_and_path() -> TestResult {
    const UNSAVED_1: NativeOverlayTextSource = NativeOverlayTextSource::Overlay { revision: 1 };
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = RunningService::start(service(root.path(), &socket, true)?);
    let client = NativeUdsClient::new(socket);
    let (selected, _source_a, source_b) = publish_two_repositories(&client)
        .map_err(|error| format!("two-repository publication: {error}"))?;
    let live = LivePublisher::begin(&client, Duration::from_secs(30))?;
    let publisher = live.id();
    let session = NativeOverlaySession {
        workspace: workspace(),
        publisher,
        epoch: 1,
    };
    let document = |id| NativeOverlayDocument {
        session,
        owner: repository_owner(id),
    };
    require(
        client.overlay(NativeOverlayRequest::Begin { session }, &budget())?
            == NativeOverlayReply::Begun(session),
        "overlay session did not begin",
    )?;
    let shared = hits(&client, selected, session, "shared")?;
    require(
        shared
            .iter()
            .all(|(_, source, _)| matches!(source, NativeOverlayTextSource::Durable(_)))
            && shared
                .iter()
                .map(|(id, _, identity)| (*id, identity.as_str()))
                .eq([(10, "repo-a:src/shared.rs"), (11, "repo-b:src/shared.rs")]),
        "durable hits did not expose their two repositories",
    )?;

    // Unsaved modify of repo-a's file masks only repo-a's durable row.
    update(
        &client,
        &live,
        document(10),
        None,
        1,
        Some("draft alpha"),
        None,
        None,
    )?;
    require(
        hits(&client, selected, session, "shared")?
            == vec![row(
                11,
                NativeOverlayTextSource::Durable(source_b),
                "repo-b",
            )]
            && hits(&client, selected, session, "draft")? == vec![row(10, UNSAVED_1, "repo-a")],
        "repo-a modify masked or conflicted with repo-b's same path",
    )?;
    // The same relative path edited unsaved in repo-b is a distinct file.
    update(
        &client,
        &live,
        document(11),
        None,
        1,
        Some("draft beta"),
        None,
        None,
    )?;
    require(
        hits(&client, selected, session, "shared")?.is_empty()
            && hits(&client, selected, session, "draft")?
                == vec![row(10, UNSAVED_1, "repo-a"), row(11, UNSAVED_1, "repo-b")],
        "same relative path edited in two repositories was refused",
    )?;
    // Unsaved delete in repo-a, then an unsaved create at repo-a's path.
    update(&client, &live, document(10), Some(1), 2, None, None, None)?;
    update(
        &client,
        &live,
        document(12),
        None,
        1,
        Some("draft gamma"),
        Some(SHARED),
        Some(repository_owner(10)),
    )?;
    require(
        hits(&client, selected, session, "draft")?
            == vec![row(11, UNSAVED_1, "repo-b"), row(12, UNSAVED_1, "repo-a")],
        "repo-a create did not replace only repo-a's deleted file",
    )?;
    // A second unsaved repo-b file at repo-b's live overlay path conflicts.
    update(
        &client,
        &live,
        document(13),
        None,
        1,
        Some("draft delta"),
        Some(SHARED),
        Some(repository_owner(11)),
    )?;
    require(
        refused(
            query(&client, selected, session, "draft"),
            NativeIndexErrorCode::Conflict,
        ),
        "duplicate repository/path overlay rows were not refused",
    )?;
    live.renew()?;
    update(&client, &live, document(13), Some(1), 2, None, None, None)?;
    require(
        hits(&client, selected, session, "draft")?.len() == 2,
        "withdrawing the duplicate did not restore the corpus",
    )?;
    require(
        client.overlay(NativeOverlayRequest::Close { session }, &budget())?
            == NativeOverlayReply::Closed(session),
        "overlay session did not close",
    )?;
    // A witness outside the selected view cannot establish a repository. A
    // fresh session keeps this case inside the harness's four-slot limit.
    let foreign = NativeOverlaySession {
        epoch: 2,
        ..session
    };
    require(
        client.overlay(NativeOverlayRequest::Begin { session: foreign }, &budget())?
            == NativeOverlayReply::Begun(foreign),
        "second overlay session did not begin",
    )?;
    update(
        &client,
        &live,
        NativeOverlayDocument {
            session: foreign,
            owner: repository_owner(14),
        },
        None,
        1,
        Some("draft epsilon"),
        Some("src/other.rs"),
        Some(repository_owner(99)),
    )?;
    require(
        refused(
            query(&client, selected, foreign, "draft"),
            NativeIndexErrorCode::InvalidInput,
        ),
        "a repository witness outside the selected view was admitted",
    )?;
    live.renew()?;
    require(
        client.overlay(NativeOverlayRequest::Close { session: foreign }, &budget())?
            == NativeOverlayReply::Closed(foreign),
        "second overlay session did not close",
    )?;
    running.stop()
}

#[test]
fn overlay_latest_unicode_paths_mask_by_repository() -> TestResult {
    const NFC: &str = "src/café.rs";
    const NFD: &str = "src/cafe\u{301}.rs";
    require(
        NFC.as_bytes() == b"src/caf\xc3\xa9.rs"
            && NFD.as_bytes() == b"src/cafe\xcc\x81.rs"
            && NFC != NFD,
        "Unicode path fixture changed its raw bytes",
    )?;

    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = RunningService::start(service(root.path(), &socket, true)?);
    let client = NativeUdsClient::new(socket);
    let (selected, _source_a, source_b) = publish_two_repositories_at(&client, NFC)?;
    let live = LivePublisher::begin(&client, Duration::from_secs(30))?;
    let session = NativeOverlaySession {
        workspace: workspace(),
        publisher: live.id(),
        epoch: 1,
    };
    let document = |id| NativeOverlayDocument {
        session,
        owner: repository_owner(id),
    };
    require(
        client.overlay(NativeOverlayRequest::Begin { session }, &budget())?
            == NativeOverlayReply::Begun(session),
        "Unicode overlay session did not begin",
    )?;
    update(
        &client,
        &live,
        document(10),
        None,
        1,
        Some("unicode alpha"),
        None,
        None,
    )?;
    update(
        &client,
        &live,
        document(12),
        None,
        1,
        Some("unicode decomposed"),
        Some(NFD),
        Some(repository_owner(10)),
    )?;

    let check_latest = |expected_view| -> TestResult {
        live.renew()?;
        let durable = query_latest(&client, session, "shared")?;
        require(
            durable.view == expected_view
                && durable.exhaustive
                && durable.matched == 1
                && durable.hits.len() == 1
                && durable.hits[0].owner == repository_owner(11)
                && durable.hits[0].source == NativeOverlayTextSource::Durable(source_b)
                && durable.hits[0]
                    .repository
                    .as_ref()
                    .is_some_and(|repo| repo.as_str() == "repo-b")
                && durable.hits[0].path.as_bytes() == NFC.as_bytes(),
            "repo-a Unicode edit masked repo-b's same-path durable file",
        )?;
        let draft = query_latest(&client, session, "unicode")?;
        require(
            draft.view == expected_view
                && draft.exhaustive
                && draft.matched == 2
                && draft.hits.len() == 2
                && draft.hits.iter().any(|hit| {
                    hit.owner == repository_owner(10)
                        && hit.source == NativeOverlayTextSource::Overlay { revision: 1 }
                        && hit
                            .repository
                            .as_ref()
                            .is_some_and(|repo| repo.as_str() == "repo-a")
                        && hit.path.as_bytes() == NFC.as_bytes()
                })
                && draft.hits.iter().any(|hit| {
                    hit.owner == repository_owner(12)
                        && hit.source == NativeOverlayTextSource::Overlay { revision: 1 }
                        && hit
                            .repository
                            .as_ref()
                            .is_some_and(|repo| repo.as_str() == "repo-a")
                        && hit.path.as_bytes() == NFD.as_bytes()
                }),
            "Latest merged distinct NFC/NFD paths or changed their repository",
        )
    };
    check_latest(selected)?;

    update(
        &client,
        &live,
        document(13),
        None,
        1,
        Some("unicode duplicate"),
        Some(NFC),
        Some(repository_owner(10)),
    )?;
    require(
        refused(
            query_latest(&client, session, "unicode"),
            NativeIndexErrorCode::Conflict,
        ),
        "same-repository Unicode path collision was accepted",
    )?;
    update(&client, &live, document(13), Some(1), 2, None, None, None)?;

    let key = operation(3);
    let ingress = client.begin_input(key, &budget())?;
    let unrelated = stage_repository_at(
        &client,
        ingress,
        &RepoId::try_from("repo-b")?,
        "src/other.rs",
        "unrelated",
    )?;
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: repository_owner(14),
            expected: None,
            next: Some(unrelated),
        }],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: client
            .read_view(ViewSelection::Exact(selected), &budget())?
            .source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: key.stream_epoch,
        predecessor: 1,
        sequence: 2,
        payload: transition.owner_digest()?,
    });
    let advanced = published(
        &client,
        key,
        &client.submit(
            UpdateRequest {
                operation: key,
                expected_view: selected,
                kind: UpdateKind::SourceAdvance(transition),
            },
            &budget(),
        )?,
    )?;
    require(advanced != selected, "SourceAdvance did not move Latest")?;
    check_latest(advanced)?;
    require(
        client.overlay(NativeOverlayRequest::Close { session }, &budget())?
            == NativeOverlayReply::Closed(session),
        "Unicode overlay session did not close",
    )?;
    running.stop()
}

/// One unsubmitted SourceAdvance that saves `document` as `record`.
fn save_intent(
    client: &NativeUdsClient,
    selected: ViewRef,
    (sequence, event): (u64, u64),
    document: NativeOverlayDocument,
    expected_source: Option<ObjectRef>,
    stage: impl FnOnce(IngressRef) -> TestResult<ObjectRef>,
) -> TestResult<(NativeOverlaySaveIntent, ObjectRef)> {
    let key = operation(sequence);
    let ingress = client.begin_input(key, &budget())?;
    let record = stage(ingress)?;
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: document.owner,
            expected: expected_source,
            next: Some(record),
        }],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: client
            .read_view(ViewSelection::Exact(selected), &budget())?
            .source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: key.stream_epoch,
        predecessor: event.checked_sub(1).ok_or("save event starts at one")?,
        sequence: event,
        payload: transition.owner_digest()?,
    });
    Ok((
        NativeOverlaySaveIntent {
            document,
            revision: 1,
            expected_source,
            request: UpdateRequest {
                operation: key,
                expected_view: selected,
                kind: UpdateKind::SourceAdvance(transition),
            },
        },
        record,
    ))
}

/// Bind a save; a Conflict counts as a refusal of this intent only when the
/// publisher lease is still renewable right afterwards.
fn bound(
    client: &NativeUdsClient,
    live: &LivePublisher<'_>,
    (intent, _record): (NativeOverlaySaveIntent, ObjectRef),
) -> TestResult<bool> {
    let key = intent.operation();
    match client.bind_overlay_save(intent, &budget()) {
        Ok(NativeOverlayReply::SaveBound { operation, .. }) if operation == key => Ok(true),
        refused @ Err(_) => {
            live.refused(refused, NativeIndexErrorCode::Conflict)?;
            Ok(false)
        }
        other @ Ok(_) => Err(format!("unexpected save binding outcome: {other:?}").into()),
    }
}

/// A row that asserts a repository (a durable owner or a witness) saves only
/// into that repository; another repository is refused. A new file without a
/// witness asserts none, so the staged record's repository is authoritative.
#[test]
fn overlay_save_binds_only_the_overlay_identity_repository() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = RunningService::start(service(root.path(), &socket, true)?);
    let client = NativeUdsClient::new(socket);
    let (selected, source_a, source_b) = publish_two_repositories(&client)
        .map_err(|error| format!("two-repository publication: {error}"))?;
    let (repo_a, repo_b) = (RepoId::try_from("repo-a")?, RepoId::try_from("repo-b")?);
    let live = LivePublisher::begin(&client, Duration::from_secs(30))?;
    let publisher = live.id();
    let session = NativeOverlaySession {
        workspace: workspace(),
        publisher,
        epoch: 1,
    };
    let document = |id| NativeOverlayDocument {
        session,
        owner: repository_owner(id),
    };
    require(
        client.overlay(NativeOverlayRequest::Begin { session }, &budget())?
            == NativeOverlayReply::Begun(session),
        "overlay session did not begin",
    )?;
    // Durable repo-a file, a new repo-a file and a new file without witness.
    update(
        &client,
        &live,
        document(10),
        None,
        1,
        Some("saved alpha"),
        None,
        None,
    )?;
    update(
        &client,
        &live,
        document(11),
        None,
        1,
        Some("saved beta"),
        Some("src/renamed.rs"),
        None,
    )?;
    update(
        &client,
        &live,
        document(12),
        None,
        1,
        Some("saved new"),
        Some("src/new.rs"),
        Some(repository_owner(10)),
    )?;
    for (id, path) in [(13, "src/loose.rs"), (14, "src/plain.rs")] {
        update(
            &client,
            &live,
            document(id),
            None,
            1,
            Some("saved loose"),
            Some(path),
            None,
        )?;
    }

    let durable_elsewhere = save_intent(
        &client,
        selected,
        (3, 2),
        document(10),
        Some(source_a),
        |ingress| stage_repository(&client, ingress, &repo_b, "saved alpha"),
    )?;
    require(
        !bound(&client, &live, durable_elsewhere)?,
        "a durable repo-a file was saved into repo-b",
    )?;
    // `path=None` inherits the durable path. Identical bytes in the same
    // repository cannot silently move that overlay to another path.
    let durable_renamed = save_intent(
        &client,
        selected,
        (9, 2),
        document(10),
        Some(source_a),
        |ingress| stage_repository_at(&client, ingress, &repo_a, "src/other.rs", "saved alpha"),
    )?;
    require(
        !bound(&client, &live, durable_renamed)?,
        "a path-inheriting durable overlay was saved at another path",
    )?;
    let durable_home = save_intent(
        &client,
        selected,
        (4, 2),
        document(10),
        Some(source_a),
        |ingress| stage_repository(&client, ingress, &repo_a, "saved alpha"),
    )?;
    require(
        bound(&client, &live, durable_home)?,
        "a durable file was not saved into its own repository",
    )?;
    let explicit_rename = save_intent(
        &client,
        selected,
        (10, 2),
        document(11),
        Some(source_b),
        |ingress| stage_repository_at(&client, ingress, &repo_b, "src/renamed.rs", "saved beta"),
    )?;
    require(
        bound(&client, &live, explicit_rename)?,
        "an explicit overlay path rename was refused",
    )?;

    let new_elsewhere = save_intent(&client, selected, (5, 2), document(12), None, |ingress| {
        stage_repository_at(&client, ingress, &repo_b, "src/new.rs", "saved new")
    })?;
    require(
        !bound(&client, &live, new_elsewhere)?,
        "a new file was saved outside its witness repository",
    )?;
    let new_home = save_intent(&client, selected, (6, 2), document(12), None, |ingress| {
        stage_repository_at(&client, ingress, &repo_a, "src/new.rs", "saved new")
    })?;
    require(
        bound(&client, &live, new_home)?,
        "a new file was not saved into its witness repository",
    )?;

    // A witness-less new file asserts no repository: the producer-staged
    // record's authenticated repository, named or unscoped, is authoritative.
    let loose_scoped = save_intent(&client, selected, (7, 2), document(13), None, |ingress| {
        stage_repository_at(&client, ingress, &repo_b, "src/loose.rs", "saved loose")
    })?;
    require(
        bound(&client, &live, loose_scoped)?,
        "a witness-less new file could not save into a named repository",
    )?;
    let loose_unscoped = save_intent(&client, selected, (8, 2), document(14), None, |ingress| {
        Ok(client.stage_source(
            ingress,
            "src/plain.rs".into(),
            b"saved loose",
            SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
            &budget(),
        )?)
    })?;
    require(
        bound(&client, &live, loose_unscoped)?,
        "a witness-less new file could not save an unscoped record",
    )?;
    // A tombstone has no next Source path. It must bind and retire after the
    // source CAS removes the owner, even though the base owner had a path.
    update(&client, &live, document(10), Some(1), 2, None, None, None)?;
    let delete_key = operation(11);
    let ingress = client.begin_input(delete_key, &budget())?;
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: document(10).owner,
            expected: Some(source_a),
            next: None,
        }],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: client
            .read_view(ViewSelection::Exact(selected), &budget())?
            .source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: delete_key.producer,
        stream: delete_key.stream,
        stream_epoch: delete_key.stream_epoch,
        predecessor: 1,
        sequence: 2,
        payload: transition.owner_digest()?,
    });
    let delete_request = UpdateRequest {
        operation: delete_key,
        expected_view: selected,
        kind: UpdateKind::SourceAdvance(transition),
    };
    live.renew()?;
    require(
        client.bind_overlay_save(
            NativeOverlaySaveIntent {
                document: document(10),
                revision: 2,
                expected_source: Some(source_a),
                request: delete_request.clone(),
            },
            &budget(),
        )? == NativeOverlayReply::SaveBound {
            document: document(10),
            revision: 2,
            operation: delete_key,
        },
        "a tombstone save did not bind",
    )?;
    let deleted_view = published(
        &client,
        delete_key,
        &client.submit(delete_request, &budget())?,
    )?;
    live.renew()?;
    require(
        client.complete_overlay_save(
            &NativeOverlaySaveReceipt {
                operation: delete_key,
                document: document(10),
                revision: 2,
                source: None,
                selected: deleted_view,
            },
            &budget(),
        )? == NativeOverlayReply::Saved {
            document: document(10),
            revision: 2,
            selected: deleted_view,
        },
        "the published tombstone did not retire its overlay revision",
    )?;
    require(
        client.overlay(NativeOverlayRequest::Close { session }, &budget())?
            == NativeOverlayReply::Closed(session),
        "overlay session did not close",
    )?;
    running.stop()
}

/// An empty view has no durable owner to witness a repository. Its first
/// named-repository file is saved through the overlay without a witness, and
/// the published file is then found durably inside that repository.
#[test]
fn overlay_saves_the_first_named_repository_file_into_an_empty_view() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = RunningService::start(service(root.path(), &socket, true)?);
    let client = NativeUdsClient::new(socket);
    let configured = configure_empty_view(&client)?;
    let repository = RepoId::try_from("repo-c")?;
    let live = LivePublisher::begin(&client, Duration::from_secs(30))?;
    let session = NativeOverlaySession {
        workspace: workspace(),
        publisher: live.id(),
        epoch: 1,
    };
    let document = NativeOverlayDocument {
        session,
        owner: repository_owner(20),
    };
    require(
        client.overlay(NativeOverlayRequest::Begin { session }, &budget())?
            == NativeOverlayReply::Begun(session),
        "overlay session did not begin",
    )?;
    update(
        &client,
        &live,
        document,
        None,
        1,
        Some("first named"),
        Some("src/first.rs"),
        None,
    )?;
    let (intent, record) = save_intent(&client, configured, (2, 1), document, None, |ingress| {
        stage_repository_at(&client, ingress, &repository, "src/first.rs", "first named")
    })?;
    let request = intent.request.clone();
    require(
        bound(&client, &live, (intent, record))?,
        "the first named-repository file was refused in an empty view",
    )?;
    let selected = published(
        &client,
        request.operation,
        &client.submit(request, &budget())?,
    )?;
    live.renew()?;
    require(
        client.complete_overlay_save(
            &NativeOverlaySaveReceipt {
                operation: operation(2),
                document,
                revision: 1,
                source: Some(record),
                selected,
            },
            &budget(),
        )? == NativeOverlayReply::Saved {
            document,
            revision: 1,
            selected,
        },
        "the published first file did not retire its overlay revision",
    )?;
    require(
        hits(&client, selected, session, "first")?
            == vec![(
                20,
                NativeOverlayTextSource::Durable(record),
                "repo-c:src/first.rs".to_owned(),
            )],
        "the saved file was not found durably in its named repository",
    )?;
    require(
        client.overlay(NativeOverlayRequest::Close { session }, &budget())?
            == NativeOverlayReply::Closed(session),
        "overlay session did not close",
    )?;
    running.stop()
}
