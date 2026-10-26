//! Selected native text search with one live, revision-fenced overlay corpus.

use super::*;
use quanta_index_contract::native_protocol::{
    NativeOverlayTextQuery, NativeOverlayTextResult, NativeOverlayTextSource,
};

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

#[test]
fn overlay_text_query_masks_selected_rows_and_fences_exact_corpus() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = RunningService::start(service(root.path(), &socket, true)?);
    let client = NativeUdsClient::new(socket);

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
        &client,
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
    let key = operation(2);
    let ingress = client.begin_input(key, &budget())?;
    let source = client.stage_source(
        ingress,
        "src/a.rs".into(),
        b"typed durable source",
        SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        &budget(),
    )?;
    let mutations = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: owner(),
            expected: None,
            next: Some(source),
        }],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: client
            .read_view(ViewSelection::Exact(configured), &budget())?
            .source,
        owners: mutations,
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
        &client,
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
