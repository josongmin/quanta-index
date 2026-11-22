//! Selected native text search with one live, revision-fenced overlay corpus.

use super::*;
use quanta_index_contract::{
    index::ObjectRef,
    native_protocol::{
        NativeOverlayTextQuery, NativeOverlayTextResult, NativeOverlayTextSource,
        NativeTextPreviewEvidence,
    },
};

const SOURCE_PATH: &str = "src/a.rs";
const DURABLE_TEXT: &str = "typed durable source";

fn other_owner() -> OwnerKey {
    OwnerKey {
        stable_id: StableOwnerId([8; 16]),
        ..owner()
    }
}

fn query_overlay(
    client: &NativeUdsClient,
    selected: ViewRef,
    session: NativeOverlaySession,
    word: &str,
    expected_corpus: Option<Digest>,
) -> Result<NativeOverlayTextResult, Box<NativeUdsError>> {
    client
        .search_overlay_text(
            NativeOverlayTextQuery {
                selection: ViewSelection::Exact(selected),
                instance: native_text_spec().instance,
                session,
                query: text_request(word),
                expected_corpus,
            },
            &budget(),
        )
        .map_err(Box::new)
}

fn require_overlay_conflict(
    result: Result<NativeOverlayTextResult, Box<NativeUdsError>>,
) -> TestResult {
    require(
        matches!(result, Err(error) if matches!(*error, NativeUdsError::Refused(ref refusal) if refusal.code == NativeIndexErrorCode::Conflict)),
        "overlay query accepted a stale or closed corpus",
    )
}

fn stage_fixture_source(
    client: &NativeUdsClient,
    base: ViewRef,
    sequence: u64,
    expected: Option<ObjectRef>,
    text: &str,
) -> TestResult<(ObjectRef, UpdateRequest)> {
    let predecessor = sequence
        .checked_sub(2)
        .ok_or("fixture source sequence starts at 2")?;
    let event_sequence = sequence
        .checked_sub(1)
        .ok_or("fixture source event underflow")?;
    let key = operation(sequence);
    let ingress = client.begin_input(key, &budget())?;
    let source = client.stage_source(
        ingress,
        SOURCE_PATH.into(),
        text.as_bytes(),
        SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        &budget(),
    )?;
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: owner(),
            expected,
            next: Some(source),
        }],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: client
            .read_view(ViewSelection::Exact(base), &budget())?
            .source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: key.stream_epoch,
        predecessor,
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    Ok((
        source,
        UpdateRequest {
            operation: key,
            expected_view: base,
            kind: UpdateKind::SourceAdvance(transition),
        },
    ))
}

fn publish_fixture(client: &NativeUdsClient) -> TestResult<(ViewRef, ObjectRef, UpdateRequest)> {
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
    let (source, request) = stage_fixture_source(client, configured, 2, None, DURABLE_TEXT)?;
    let selected = published(
        client,
        request.operation,
        &client.submit_borrowed(&request, &budget())?,
    )?;
    Ok((selected, source, request))
}

#[test]
fn overlay_text_query_masks_selected_rows_and_fences_exact_corpus() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = RunningService::start(service(root.path(), &socket, true)?);
    let client = NativeUdsClient::new(socket);
    let (selected, source, _) = publish_fixture(&client)?;
    let durable = client.search_text(
        ViewSelection::Exact(selected),
        native_text_spec().instance,
        text_request("typed"),
        &budget(),
    )?;
    require(
        durable.view == selected
            && durable.exhaustive
            && durable.matched == 1
            && durable.hits.len() == 1
            && durable
                .hits
                .first()
                .is_some_and(|hit| hit.owner == owner() && hit.source == source),
        "selected durable text fixture did not contain the fixed source",
    )?;

    let publisher = client.begin_interaction_publisher(Duration::from_secs(30), &budget())?;
    let session = NativeOverlaySession {
        workspace: workspace(),
        publisher,
        epoch: 1,
    };
    let old_document = NativeOverlayDocument {
        session,
        owner: owner(),
    };
    let new_document = NativeOverlayDocument {
        session,
        owner: other_owner(),
    };
    require(
        client.overlay(NativeOverlayRequest::Begin { session }, &budget())?
            == NativeOverlayReply::Begun(session),
        "overlay query session did not begin",
    )?;
    let baseline = query_overlay(&client, selected, session, "typed", None)?;
    require(
        baseline.view == selected
            && baseline.instance == native_text_spec().instance
            && baseline.exhaustive
            && baseline.matched == 1
            && baseline.hits.len() == 1
            && baseline.hits.first().is_some_and(|hit| {
                hit.owner == owner()
                    && hit.source == NativeOverlayTextSource::Durable(source)
                    && hit.path == "src/a.rs"
            })
            && baseline.corpus.session == session
            && baseline.corpus.revisions.is_empty(),
        "empty overlay corpus changed the selected durable row",
    )?;
    let empty_regex = query_overlay(&client, selected, session, "/typed/", None)?;
    require(
        empty_regex.matched == 1
            && empty_regex.hits.len() == 1
            && empty_regex
                .hits
                .first()
                .is_some_and(|hit| hit.source == NativeOverlayTextSource::Durable(source)),
        "empty overlay corpus lost the selected regex match",
    )?;
    let empty_file = query_overlay(&client, selected, session, "typed file:a", None)?;
    require(
        empty_file.matched == 1
            && empty_file.hits.len() == 1
            && empty_file
                .hits
                .first()
                .is_some_and(|hit| hit.path == "src/a.rs"),
        "empty overlay corpus lost the selected file filter",
    )?;

    for (document, text, path) in [
        (old_document, "draft replacement", "src/a.rs"),
        (new_document, "draft novel", "src/new.rs"),
    ] {
        require(
            client.update_overlay_at_path(
                document,
                None,
                1,
                Some(text.into()),
                Some(path.into()),
                &budget(),
            )? == NativeOverlayReply::Updated {
                document,
                revision: 1,
            },
            "overlay query revision was not accepted",
        )?;
    }
    let masked = query_overlay(&client, selected, session, "typed", None)?;
    require(
        masked.view == selected
            && masked.exhaustive
            && masked.matched == 0
            && masked.hits.is_empty(),
        "overlay replacement failed to mask the selected durable term",
    )?;
    let drafts = query_overlay(&client, selected, session, "draft", None)?;
    require(
        drafts.view == selected
            && drafts.exhaustive
            && drafts.matched == 2
            && drafts.hits.len() == 2
            && drafts.corpus.session == session
            && drafts.corpus.revisions.len() == 2
            && drafts
                .corpus
                .revisions
                .iter()
                .all(|row| row.revision == 1 && !row.deleted && !row.retired)
            && drafts.hits.iter().any(|hit| {
                hit.owner == owner()
                    && hit.source == NativeOverlayTextSource::Overlay { revision: 1 }
                    && hit.path == "src/a.rs"
            })
            && drafts.hits.iter().any(|hit| {
                hit.owner == other_owner()
                    && hit.source == NativeOverlayTextSource::Overlay { revision: 1 }
                    && hit.path == "src/new.rs"
            }),
        "overlay query did not return both exact unsaved revisions",
    )?;
    let regex = query_overlay(&client, selected, session, "/draft/", None)?;
    require(
        regex.matched == 2
            && regex.hits.len() == 2
            && regex
                .hits
                .iter()
                .all(|hit| matches!(hit.source, NativeOverlayTextSource::Overlay { revision: 1 })),
        "content regex did not evaluate both unsaved bodies",
    )?;
    let masked_regex = query_overlay(&client, selected, session, "/durable/", None)?;
    require(
        masked_regex.matched == 0 && masked_regex.hits.is_empty(),
        "content regex read the masked durable body",
    )?;
    let union = query_overlay(&client, selected, session, "draft OR NOT typed", None)?;
    require(
        union.matched == 2
            && union.hits.len() == 2
            && union
                .hits
                .iter()
                .all(|hit| matches!(hit.source, NativeOverlayTextSource::Overlay { revision: 1 })),
        "OR NOT used the durable universe instead of the overlay corpus",
    )?;
    let negative = query_overlay(&client, selected, session, "NOT draft", None)?;
    require(
        negative.matched == 0 && negative.hits.is_empty(),
        "NOT included an unsaved document containing the excluded term",
    )?;
    for source in ["draft path:new", "draft file:new"] {
        let filtered = query_overlay(&client, selected, session, source, None)?;
        require(
            filtered.matched == 1
                && filtered.hits.len() == 1
                && filtered.hits.first().is_some_and(|hit| {
                    hit.owner == other_owner()
                        && hit.path == "src/new.rs"
                        && hit.source == NativeOverlayTextSource::Overlay { revision: 1 }
                }),
            "file or path filter ignored the unsaved owner path",
        )?;
    }
    let stamp = drafts.corpus.identity;
    require(
        query_overlay(&client, selected, session, "draft", Some(stamp))?.corpus == drafts.corpus,
        "unchanged corpus identity was not replayable",
    )?;
    require(
        client.overlay(
            NativeOverlayRequest::Snapshot {
                document: new_document,
            },
            &budget(),
        )? == NativeOverlayReply::Snapshot {
            document: new_document,
            overlay: Some(
                quanta_index_contract::native_protocol::NativeOverlaySnapshot {
                    revision: 1,
                    text: Some("draft novel".into()),
                    path: Some("src/new.rs".into()),
                },
            ),
        },
        "overlay query changed the retained unsaved document",
    )?;

    require(
        client.overlay(
            NativeOverlayRequest::Update {
                document: new_document,
                expected_revision: Some(1),
                revision: 2,
                text: Some("fresh novel".into()),
                path: Some("src/new.rs".into()),
            },
            &budget(),
        )? == NativeOverlayReply::Updated {
            document: new_document,
            revision: 2,
        },
        "new document did not advance its overlay revision",
    )?;
    require_overlay_conflict(query_overlay(
        &client,
        selected,
        session,
        "draft",
        Some(stamp),
    ))?;
    let fresh = query_overlay(&client, selected, session, "fresh", None)?;
    require(
        fresh.matched == 1
            && fresh.hits.len() == 1
            && fresh.hits.first().is_some_and(|hit| {
                hit.owner == other_owner()
                    && hit.source == NativeOverlayTextSource::Overlay { revision: 2 }
            }),
        "query used a stale overlay revision",
    )?;
    require(
        client.overlay(
            NativeOverlayRequest::Update {
                document: old_document,
                expected_revision: Some(1),
                revision: 2,
                text: None,
                path: None,
            },
            &budget(),
        )? == NativeOverlayReply::Updated {
            document: old_document,
            revision: 2,
        },
        "overlay deletion was not accepted",
    )?;
    require(
        client.overlay(
            NativeOverlayRequest::Snapshot {
                document: old_document,
            },
            &budget(),
        )? == NativeOverlayReply::Snapshot {
            document: old_document,
            overlay: Some(
                quanta_index_contract::native_protocol::NativeOverlaySnapshot {
                    revision: 2,
                    text: None,
                    path: None,
                },
            ),
        },
        "tombstone snapshot attempted a zero-byte allocation or lost its mask",
    )?;
    let deleted = query_overlay(&client, selected, session, "typed", None)?;
    require(
        deleted.matched == 0
            && deleted.hits.is_empty()
            && deleted.corpus.revisions.iter().any(|row| {
                row.owner == owner() && row.revision == 2 && row.deleted && !row.retired
            }),
        "overlay deletion failed to mask the selected durable row",
    )?;
    let still_durable = client.search_text(
        ViewSelection::Exact(selected),
        native_text_spec().instance,
        text_request("typed"),
        &budget(),
    )?;
    require(
        still_durable.view == selected
            && still_durable.matched == 1
            && still_durable.hits.len() == 1
            && still_durable
                .hits
                .first()
                .is_some_and(|hit| hit.source == source),
        "overlay query or deletion changed the immutable durable view",
    )?;

    require(
        client.overlay(NativeOverlayRequest::Close { session }, &budget())?
            == NativeOverlayReply::Closed(session),
        "overlay query session did not close",
    )?;
    require_overlay_conflict(query_overlay(&client, selected, session, "typed", None))?;
    let next_session = NativeOverlaySession {
        epoch: 2,
        ..session
    };
    require(
        client.overlay(
            NativeOverlayRequest::Begin {
                session: next_session,
            },
            &budget(),
        )? == NativeOverlayReply::Begun(next_session),
        "second overlay query session did not begin",
    )?;
    client.end_interaction_publisher(publisher, 1, &budget())?;
    require_overlay_conflict(query_overlay(
        &client,
        selected,
        next_session,
        "typed",
        None,
    ))?;
    running.stop()
}

#[test]
fn interrupted_overlay_save_preserves_sealed_view_bytes_across_reopen() -> TestResult {
    const CANDIDATE: &str = "interrupted replacement";
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = RunningService::start(service(root.path(), &socket, true)?);
    let client = NativeUdsClient::new(socket.clone());
    let (selected, source, original) = publish_fixture(&client)?;
    let exact = ViewSelection::Exact(selected);
    let before = client.read_view(exact, &budget())?;
    let text_instance = native_text_spec().instance;
    let durable =
        client.search_text_preview(exact, text_instance, text_request("typed"), &budget())?;
    let [preview] = durable.previews.as_slice() else {
        return Err("the sealed view did not return exactly one Source preview".into());
    };
    let NativeTextPreviewEvidence::Excerpt(excerpt) = &preview.evidence else {
        return Err("the sealed Source preview has no physical excerpt".into());
    };
    require(
        durable.ranking.view == selected
            && durable.ranking.exhaustive
            && durable.ranking.matched == 1
            && preview.source == source
            && excerpt.text == DURABLE_TEXT,
        "published view was missing the original Source excerpt",
    )?;
    let object_root = root.path().join("native-objects");
    let source_path = native_object_files::object_file(&object_root, source)?;
    let artifact_path = native_object_files::object_file(&object_root, durable.ranking.artifact)?;
    let source_bytes = std::fs::read(&source_path)?;
    let artifact_bytes = std::fs::read(&artifact_path)?;

    let changed = UpdateRequest {
        expected_view: selected,
        ..original.clone()
    };
    require(
        matches!(client.submit(changed, &budget()), Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::DigestConflict),
        "changed replay reused the sealed publication key",
    )?;
    require(
        client.submit_borrowed(&original, &budget())?
            == SubmitOutcome::Terminal(OperationStatus::Published(selected)),
        "the exact original request lost its sealed receipt",
    )?;

    let publisher = client.begin_interaction_publisher(Duration::from_secs(30), &budget())?;
    let session = NativeOverlaySession {
        workspace: workspace(),
        publisher,
        epoch: 1,
    };
    let document = NativeOverlayDocument {
        session,
        owner: owner(),
    };
    require(
        client.overlay(NativeOverlayRequest::Begin { session }, &budget())?
            == NativeOverlayReply::Begun(session),
        "replacement overlay session did not begin",
    )?;
    require(
        client.overlay(
            NativeOverlayRequest::Update {
                document,
                expected_revision: None,
                revision: 1,
                text: Some(CANDIDATE.into()),
                path: Some(SOURCE_PATH.into()),
            },
            &budget(),
        )? == NativeOverlayReply::Updated {
            document,
            revision: 1,
        },
        "replacement overlay edit was not accepted",
    )?;
    let (candidate, request) = stage_fixture_source(&client, selected, 3, Some(source), CANDIDATE)?;
    let candidate_key = request.operation;
    let candidate_path = native_object_files::object_file(&object_root, candidate)?;
    require(
        candidate != source && !native_object_files::absent_nofollow(&candidate_path)?,
        "replacement Source was not physically staged",
    )?;
    require(
        client.bind_overlay_save(
            NativeOverlaySaveIntent {
                document,
                revision: 1,
                expected_source: Some(source),
                request,
            },
            &budget(),
        )? == NativeOverlayReply::SaveBound {
            document,
            revision: 1,
            operation: candidate_key,
        },
        "replacement Source was not bound to the live overlay revision",
    )?;
    // The save was staged and bound, but no SourceAdvance was submitted.
    running.stop()?;

    let reopened = RunningService::start(service(root.path(), &socket, true)?);
    let latest = ViewSelection::Latest {
        workspace: workspace(),
        epoch: 1,
    };
    require(
        client.read_view(latest, &budget())? == before
            && client.read_view(exact, &budget())? == before,
        "cold reopen changed the latest or exact sealed view",
    )?;
    require(
        client.status(candidate_key, &budget())? == OperationStatus::Unknown
            && client.status(original.operation, &budget())?
                == OperationStatus::Published(selected),
        "cold reopen changed the original or unsubmitted publication status",
    )?;
    require(
        std::fs::read(source_path)? == source_bytes
            && std::fs::read(artifact_path)? == artifact_bytes,
        "the sealed Source record or Text artifact bytes changed",
    )?;
    require(
        client.search_text_preview(exact, text_instance, text_request("typed"), &budget())?
            == durable,
        "cold reopen changed the exact query or the original Source excerpt",
    )?;
    let invisible = client.search_text(
        latest,
        text_instance,
        text_request("interrupted"),
        &budget(),
    )?;
    require(
        invisible.view == selected
            && invisible.exhaustive
            && invisible.matched == 0
            && invisible.hits.is_empty(),
        "the staged overlay became visible without Source publication",
    )?;
    reopened.stop()
}

#[test]
fn selected_text_root_same_length_damage_is_refused_through_uds() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = RunningService::start(service(root.path(), &socket, true)?);
    let client = NativeUdsClient::new(socket);
    let (selected, source, _) = publish_fixture(&client)?;
    let query = || {
        client.search_text(
            ViewSelection::Exact(selected),
            native_text_spec().instance,
            text_request("typed"),
            &budget(),
        )
    };
    let before = query()?;
    require(
        before.exhaustive
            && before.matched == 1
            && matches!(before.hits.as_slice(), [hit] if hit.source == source),
        "the intact Text root did not return the published Source",
    )?;
    let object_root = root.path().join("native-objects");
    let path = native_object_files::object_file(&object_root, before.artifact)?;
    let original = std::fs::read(&path)?;
    let mut damaged = original.clone();
    *damaged.last_mut().ok_or("empty Text root object")? ^= 1;
    std::fs::write(&path, damaged)?;
    let rejected = query();
    std::fs::write(&path, original)?;
    require(
        matches!(rejected, Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::CorruptObject),
        "the exact Text read did not return CorruptObject for its damaged root",
    )?;
    require(
        query()? == before,
        "restoring the root did not restore the same exact result",
    )?;
    running.stop()
}
