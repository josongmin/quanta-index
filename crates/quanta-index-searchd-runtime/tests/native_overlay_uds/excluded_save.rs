//! A core text member does not make an excluded owner searchable.

use super::*;

#[test]
fn excluded_saved_source_cannot_retire_overlay_even_with_core_text_profile() -> TestResult {
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
        "overlay begin identity mismatch",
    )?;
    require(
        client.overlay(
            NativeOverlayRequest::Update {
                document,
                expected_revision: None,
                revision: 1,
                text: Some(SAVED_SOURCE_TEXT.into()),
                path: Some("src/excluded.rs".into()),
            },
            &budget(),
        )? == NativeOverlayReply::Updated {
            document,
            revision: 1,
        },
        "excluded source overlay update was not accepted",
    )?;

    let key = operation(2);
    let ingress = client.begin_input(key, &budget())?;
    let source = client.stage_source(
        ingress,
        "src/excluded.rs".into(),
        SAVED_SOURCE_TEXT.as_bytes(),
        SourceTextAdmission::Excluded,
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
    let request = UpdateRequest {
        operation: key,
        expected_view: configured,
        kind: UpdateKind::SourceAdvance(transition),
    };
    require(
        matches!(
            client.bind_overlay_save(NativeOverlaySaveIntent {
                document,
                revision: 1,
                expected_source: None,
                request: request.clone(),
            }, &budget())?,
            NativeOverlayReply::SaveBound {
                operation,
                revision: 1,
                ..
            } if operation == key
        ),
        "excluded source save intent was not bound",
    )?;
    let selected = published(&client, key, &client.submit(request, &budget())?)?;
    let query = client.search_text(
        ViewSelection::Exact(selected),
        native_text_spec().instance,
        text_request("typed"),
        &budget(),
    )?;
    require(
        query.view == selected
            && query.coverage == NativeTextCoverage::CoreRequired
            && query.exhaustive
            && query.matched == 0
            && query.hits.is_empty(),
        "excluded source unexpectedly became searchable",
    )?;
    expect_snapshot_unavailable(client.complete_overlay_save(
        &NativeOverlaySaveReceipt {
            operation: key,
            document,
            revision: 1,
            source: Some(source),
            selected,
        },
        &budget(),
    ))?;
    let NativeOverlayReply::Snapshot {
        overlay: Some(snapshot),
        ..
    } = client.overlay(NativeOverlayRequest::Snapshot { document }, &budget())?
    else {
        return Err("excluded source retired unsaved overlay".into());
    };
    require(
        snapshot.revision == 1 && snapshot.text.as_deref() == Some(SAVED_SOURCE_TEXT),
        "excluded source changed unsaved overlay",
    )?;
    require(
        client.overlay(NativeOverlayRequest::Masks { session }, &budget())?
            == NativeOverlayReply::Masks {
                session,
                masks: vec![NativeOverlayMask {
                    owner: owner(),
                    revision: 1,
                    deleted: false,
                }],
            },
        "excluded source retired overlay mask",
    )?;
    running.stop()
}
