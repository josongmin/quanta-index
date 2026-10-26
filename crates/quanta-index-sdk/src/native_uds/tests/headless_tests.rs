use super::*;
use quanta_index_contract::{
    FileId, RepoMapEdgeKind, RepoMapNodeRef,
    index::{Digest, IndexInstanceId, OwnerKey, StableOwnerId, ViewRef, ViewSelection},
    native_protocol::{
        GraphEdge, GraphNode, NativeGraphDirection, NativeGraphQuery, NativeGraphResult,
        NativeGraphRow, NativeInteractionPublisherId, NativeOverlayCorpusStamp,
        NativeOverlayDocument, NativeOverlayReply, NativeOverlayRequest, NativeOverlayRevision,
        NativeOverlaySaveIntent, NativeOverlaySaveReceipt, NativeOverlaySession,
        NativeOverlaySnapshot, NativeOverlayTextHit, NativeOverlayTextQuery,
        NativeOverlayTextResult, NativeOverlayTextSource,
    },
};

fn graph_node(path: &str) -> RepoMapNodeRef {
    RepoMapNodeRef::File(FileId::new(path))
}

fn graph_view() -> ViewRef {
    ViewRef {
        workspace: key(1).workspace,
        epoch: 1,
        sequence: 7,
        digest: Digest([31; 32]),
    }
}

fn graph_query() -> NativeGraphQuery {
    NativeGraphQuery {
        source: graph_node("src/a.rs"),
        direction: NativeGraphDirection::Outgoing,
        max_supporters: 2,
        max_edges: 4,
    }
}

fn graph_result() -> NativeGraphResult {
    NativeGraphResult {
        view: graph_view(),
        instance: IndexInstanceId([32; 16]),
        source: Some(GraphNode {
            identity: graph_node("src/a.rs"),
            payload_digest: Digest([33; 32]),
        }),
        source_supporters: 1,
        edges: vec![NativeGraphRow {
            edge: GraphEdge {
                kind: RepoMapEdgeKind::DependsOn,
                source: graph_node("src/a.rs"),
                target: graph_node("src/b.rs"),
            },
            supporters: 1,
        }],
        exhaustive: true,
    }
}

fn graph_reply(result: NativeGraphResult) -> Result<NativeGraphResult, NativeUdsError> {
    graph_reply_for(graph_query(), result)
}

fn graph_reply_for(
    query: NativeGraphQuery,
    result: NativeGraphResult,
) -> Result<NativeGraphResult, NativeUdsError> {
    scripted_reply(NativeIndexResponse::Graph(result), |client, budget| {
        client.query_graph(
            ViewSelection::Exact(graph_view()),
            IndexInstanceId([32; 16]),
            query,
            budget,
        )
    })
}

#[test]
fn graph_sdk_binds_incoming_edges_to_requested_target() -> Result<(), Box<dyn std::error::Error>> {
    let query = NativeGraphQuery {
        source: graph_node("src/b.rs"),
        direction: NativeGraphDirection::Incoming,
        ..graph_query()
    };
    let valid = NativeGraphResult {
        source: Some(GraphNode {
            identity: graph_node("src/b.rs"),
            payload_digest: Digest([34; 32]),
        }),
        ..graph_result()
    };
    if graph_reply_for(query.clone(), valid.clone())? != valid {
        return Err("graph SDK refused a correctly oriented incoming edge".into());
    }
    let reversed = NativeGraphResult {
        edges: vec![NativeGraphRow {
            edge: GraphEdge {
                kind: RepoMapEdgeKind::DependsOn,
                source: graph_node("src/b.rs"),
                target: graph_node("src/a.rs"),
            },
            supporters: 1,
        }],
        ..valid.clone()
    };
    if !matches!(
        graph_reply_for(query.clone(), reversed),
        Err(NativeUdsError::Protocol(_))
    ) {
        return Err("incoming graph accepted an outgoing edge from the anchor".into());
    }
    let outgoing = NativeGraphQuery {
        direction: NativeGraphDirection::Outgoing,
        ..query
    };
    if !matches!(
        graph_reply_for(outgoing, valid),
        Err(NativeUdsError::Protocol(_))
    ) {
        return Err("outgoing graph accepted an incoming edge to the anchor".into());
    }
    Ok(())
}

#[test]
fn graph_sdk_accepts_exact_selected_reply_and_empty_graph() -> Result<(), Box<dyn std::error::Error>>
{
    let valid = graph_result();
    if graph_reply(valid.clone())? != valid {
        return Err("graph SDK changed a selected result".into());
    }
    let empty = NativeGraphResult {
        source: None,
        source_supporters: 0,
        edges: Vec::new(),
        ..valid
    };
    if graph_reply(empty.clone())? != empty {
        return Err("graph SDK refused a valid empty result".into());
    }
    Ok(())
}

#[test]
fn graph_sdk_rejects_foreign_identity_and_impossible_support_counts()
-> Result<(), Box<dyn std::error::Error>> {
    let valid = graph_result();
    let first = valid
        .edges
        .first()
        .ok_or("graph fixture has no edge")?
        .clone();
    let cases = vec![
        NativeGraphResult {
            view: ViewRef {
                sequence: 8,
                ..valid.view
            },
            ..valid.clone()
        },
        NativeGraphResult {
            instance: IndexInstanceId([34; 16]),
            ..valid.clone()
        },
        NativeGraphResult {
            source: Some(GraphNode {
                identity: graph_node("src/foreign.rs"),
                payload_digest: Digest([33; 32]),
            }),
            ..valid.clone()
        },
        NativeGraphResult {
            source_supporters: 0,
            ..valid.clone()
        },
        NativeGraphResult {
            source_supporters: 3,
            ..valid.clone()
        },
        NativeGraphResult {
            source: None,
            ..valid.clone()
        },
        NativeGraphResult {
            edges: vec![NativeGraphRow {
                supporters: 0,
                ..first.clone()
            }],
            ..valid.clone()
        },
        NativeGraphResult {
            edges: vec![NativeGraphRow {
                supporters: 3,
                ..first.clone()
            }],
            ..valid.clone()
        },
        NativeGraphResult {
            edges: vec![NativeGraphRow {
                edge: GraphEdge {
                    source: graph_node("src/foreign.rs"),
                    ..first.edge
                },
                supporters: 1,
            }],
            ..valid.clone()
        },
        NativeGraphResult {
            exhaustive: false,
            ..valid
        },
    ];
    for case in cases {
        if !matches!(graph_reply(case), Err(NativeUdsError::Protocol(_))) {
            return Err("graph SDK accepted a forged selected result".into());
        }
    }
    Ok(())
}

#[test]
fn graph_sdk_rejects_repeated_edge_after_an_unrelated_row() -> Result<(), Box<dyn std::error::Error>>
{
    let mut result = graph_result();
    let first = result
        .edges
        .first()
        .ok_or("graph fixture has no edge")?
        .clone();
    let second = NativeGraphRow {
        edge: GraphEdge {
            target: graph_node("src/c.rs"),
            ..first.edge.clone()
        },
        supporters: 1,
    };
    result.edges = vec![first.clone(), second, first];
    if !matches!(graph_reply(result), Err(NativeUdsError::Protocol(_))) {
        return Err("graph SDK accepted a nonadjacent duplicate edge".into());
    }
    Ok(())
}

fn overlay_document() -> NativeOverlayDocument {
    let operation = key(33);
    NativeOverlayDocument {
        session: NativeOverlaySession {
            workspace: operation.workspace,
            publisher: NativeInteractionPublisherId([35; 32]),
            epoch: operation.epoch,
        },
        owner: OwnerKey {
            producer: operation.producer,
            stable_id: StableOwnerId([36; 16]),
        },
    }
}

fn overlay_reply(
    request: NativeOverlayRequest,
    reply: NativeOverlayReply,
) -> Result<NativeOverlayReply, NativeUdsError> {
    scripted_reply(NativeIndexResponse::Overlay(reply), |client, budget| {
        client.overlay(request, budget)
    })
}

fn bind_save_reply(
    intent: NativeOverlaySaveIntent,
    reply: NativeOverlayReply,
) -> Result<NativeOverlayReply, NativeUdsError> {
    scripted_reply(NativeIndexResponse::Overlay(reply), |client, budget| {
        client.bind_overlay_save(intent, budget)
    })
}

fn complete_save_reply(
    receipt: &NativeOverlaySaveReceipt,
    reply: NativeOverlayReply,
) -> Result<NativeOverlayReply, NativeUdsError> {
    scripted_reply(NativeIndexResponse::Overlay(reply), |client, budget| {
        client.complete_overlay_save(receipt, budget)
    })
}

#[test]
fn overlay_sdk_accepts_exact_update_snapshot_and_save_ack() -> Result<(), Box<dyn std::error::Error>>
{
    let document = overlay_document();
    let update = NativeOverlayRequest::Update {
        document,
        expected_revision: Some(1),
        revision: 2,
        text: Some("unsaved text".into()),
        path: Some("src/unsaved.rs".into()),
    };
    let updated = NativeOverlayReply::Updated {
        document,
        revision: 2,
    };
    if overlay_reply(update, updated.clone())? != updated {
        return Err("overlay SDK changed an exact update ACK".into());
    }
    let snapshot = NativeOverlayReply::Snapshot {
        document,
        overlay: Some(NativeOverlaySnapshot {
            revision: 2,
            text: None,
            path: None,
        }),
    };
    if overlay_reply(
        NativeOverlayRequest::Snapshot { document },
        snapshot.clone(),
    )? != snapshot
    {
        return Err("overlay SDK changed an exact deletion mask".into());
    }
    let with_path = NativeOverlayReply::Snapshot {
        document,
        overlay: Some(NativeOverlaySnapshot {
            revision: 2,
            text: Some("unsaved text".into()),
            path: Some("src/unsaved.rs".into()),
        }),
    };
    if overlay_reply(
        NativeOverlayRequest::Snapshot { document },
        with_path.clone(),
    )? != with_path
    {
        return Err("overlay SDK changed an exact path snapshot".into());
    }

    let request = source_submit_request();
    let selected = ViewRef {
        sequence: 8,
        ..request.expected_view
    };
    let operation = request.operation;
    let intent = NativeOverlaySaveIntent {
        document,
        revision: 2,
        expected_source: None,
        request,
    };
    let bound = NativeOverlayReply::SaveBound {
        document,
        revision: 2,
        operation,
    };
    if bind_save_reply(intent, bound.clone())? != bound {
        return Err("overlay SDK changed an exact save binding".into());
    }
    let receipt = NativeOverlaySaveReceipt {
        operation,
        document,
        revision: 2,
        source: None,
        selected,
    };
    let saved = NativeOverlayReply::Saved {
        document,
        revision: 2,
        selected,
    };
    if complete_save_reply(&receipt, saved.clone())? != saved {
        return Err("overlay SDK changed an exact save handoff".into());
    }
    Ok(())
}

#[test]
fn overlay_sdk_sends_path_on_canonical_update_wire() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("native.sock");
    let listener = UnixListener::bind(&socket)?;
    let document = overlay_document();
    let expected = NativeOverlayRequest::Update {
        document,
        expected_revision: Some(1),
        revision: 2,
        text: Some("unsaved".into()),
        path: Some("src/unsaved.rs".into()),
    };
    let peer = std::thread::spawn(
        move || -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
            let (mut stream, _) = listener.accept()?;
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream)?;
            let exact = request.payload == NativeIndexRequest::Overlay(expected);
            let reply = NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload: NativeIndexResponse::Overlay(NativeOverlayReply::Updated {
                    document,
                    revision: 2,
                }),
            };
            stream.write_all(&quanta_index_ipc::encode_response(&reply)?)?;
            Ok(exact)
        },
    );
    let client = NativeUdsClient::new(socket);
    let budget = RequestBudgetV1::for_duration(std::time::Duration::from_secs(2));
    let updated = client.update_overlay_at_path(
        document,
        Some(1),
        2,
        Some("unsaved".into()),
        Some("src/unsaved.rs".into()),
        &budget,
    )?;
    if updated
        != (NativeOverlayReply::Updated {
            document,
            revision: 2,
        })
    {
        return Err("overlay SDK changed the path update ACK".into());
    }
    let exact = peer
        .join()
        .map_err(|_error| std::io::Error::other("scripted overlay peer panicked"))?
        .map_err(|error| std::io::Error::other(format!("scripted overlay peer: {error}")))?;
    if !exact {
        return Err("overlay SDK omitted or changed the path on the wire".into());
    }
    Ok(())
}

#[test]
fn overlay_sdk_rejects_foreign_document_and_revision() -> Result<(), Box<dyn std::error::Error>> {
    let document = overlay_document();
    let foreign = NativeOverlayDocument {
        owner: OwnerKey {
            stable_id: StableOwnerId([37; 16]),
            ..document.owner
        },
        ..document
    };
    let update = NativeOverlayRequest::Update {
        document,
        expected_revision: Some(1),
        revision: 2,
        text: Some("unsaved text".into()),
        path: None,
    };
    for reply in [
        NativeOverlayReply::Updated {
            document: foreign,
            revision: 2,
        },
        NativeOverlayReply::Updated {
            document,
            revision: 3,
        },
    ] {
        if !matches!(
            overlay_reply(update.clone(), reply),
            Err(NativeUdsError::Protocol(_))
        ) {
            return Err("overlay SDK accepted a foreign update ACK".into());
        }
    }
    for reply in [
        NativeOverlayReply::Snapshot {
            document: foreign,
            overlay: None,
        },
        NativeOverlayReply::Snapshot {
            document,
            overlay: Some(NativeOverlaySnapshot {
                revision: 0,
                text: Some("invalid".into()),
                path: None,
            }),
        },
        NativeOverlayReply::Snapshot {
            document,
            overlay: Some(NativeOverlaySnapshot {
                revision: 2,
                text: Some("valid".into()),
                path: Some("../outside.rs".into()),
            }),
        },
        NativeOverlayReply::Snapshot {
            document,
            overlay: Some(NativeOverlaySnapshot {
                revision: 2,
                text: None,
                path: Some("src/deleted.rs".into()),
            }),
        },
    ] {
        if !matches!(
            overlay_reply(NativeOverlayRequest::Snapshot { document }, reply),
            Err(NativeUdsError::Protocol(_))
        ) {
            return Err("overlay SDK accepted a foreign snapshot".into());
        }
    }
    Ok(())
}

#[test]
fn overlay_sdk_rejects_foreign_save_operation_selection_and_revision()
-> Result<(), Box<dyn std::error::Error>> {
    let document = overlay_document();
    let request = source_submit_request();
    let operation = request.operation;
    let base_view = request.expected_view;
    let intent = NativeOverlaySaveIntent {
        document,
        revision: 2,
        expected_source: None,
        request,
    };
    for reply in [
        NativeOverlayReply::SaveBound {
            document,
            revision: 2,
            operation: key(34),
        },
        NativeOverlayReply::SaveBound {
            document,
            revision: 3,
            operation,
        },
        NativeOverlayReply::SaveBound {
            document: NativeOverlayDocument {
                owner: OwnerKey {
                    stable_id: StableOwnerId([37; 16]),
                    ..document.owner
                },
                ..document
            },
            revision: 2,
            operation,
        },
    ] {
        if !matches!(
            bind_save_reply(intent.clone(), reply),
            Err(NativeUdsError::Protocol(_))
        ) {
            return Err("overlay SDK accepted a foreign save bind ACK".into());
        }
    }
    let selected = ViewRef {
        sequence: 8,
        ..base_view
    };
    let receipt = NativeOverlaySaveReceipt {
        operation,
        document,
        revision: 2,
        source: None,
        selected,
    };
    for reply in [
        NativeOverlayReply::Saved {
            document,
            revision: 3,
            selected,
        },
        NativeOverlayReply::Saved {
            document,
            revision: 2,
            selected: ViewRef {
                sequence: 9,
                ..selected
            },
        },
    ] {
        if !matches!(
            complete_save_reply(&receipt, reply),
            Err(NativeUdsError::Protocol(_))
        ) {
            return Err("overlay SDK accepted a foreign save handoff ACK".into());
        }
    }
    Ok(())
}

fn overlay_text_result() -> Result<NativeOverlayTextResult, Box<dyn std::error::Error>> {
    let durable = text_result();
    let fixed_durable = durable.hits.first().ok_or("fixed durable hit missing")?;
    let owner = fixed_durable.owner;
    let durable_source = fixed_durable.source;
    let second_owner = OwnerKey {
        stable_id: StableOwnerId([12; 16]),
        ..owner
    };
    Ok(NativeOverlayTextResult {
        view: durable.view,
        instance: durable.instance,
        artifact: durable.artifact,
        matched: 2,
        exhaustive: true,
        corpus: NativeOverlayCorpusStamp {
            session: overlay_document().session,
            identity: Digest([50; 32]),
            revisions: vec![NativeOverlayRevision {
                owner,
                revision: 3,
                deleted: false,
                retired: false,
            }],
        },
        hits: vec![
            NativeOverlayTextHit {
                owner,
                source: NativeOverlayTextSource::Overlay { revision: 3 },
                path: "src/unsaved.rs".into(),
                score: FiniteScore::new(1.25).map_err(|_error| "invalid fixed score")?,
            },
            NativeOverlayTextHit {
                owner: second_owner,
                source: NativeOverlayTextSource::Durable(durable_source),
                path: "src/durable.rs".into(),
                score: FiniteScore::new(1.0).map_err(|_error| "invalid fixed score")?,
            },
        ],
    })
}

fn fixed_durable_source() -> Result<ObjectRef, Box<dyn std::error::Error>> {
    Ok(text_result()
        .hits
        .first()
        .ok_or("fixed durable hit missing")?
        .source)
}

fn fixture_revision_mut(
    result: &mut NativeOverlayTextResult,
) -> Result<&mut NativeOverlayRevision, Box<dyn std::error::Error>> {
    result
        .corpus
        .revisions
        .first_mut()
        .ok_or_else(|| "fixed overlay revision missing".into())
}

fn fixture_hit_mut(
    result: &mut NativeOverlayTextResult,
    ordinal: usize,
) -> Result<&mut NativeOverlayTextHit, Box<dyn std::error::Error>> {
    result
        .hits
        .get_mut(ordinal)
        .ok_or_else(|| "fixed overlay hit missing".into())
}

fn overlay_text_reply(
    reply: NativeOverlayTextResult,
    expected_corpus: Option<Digest>,
) -> Result<NativeOverlayTextResult, NativeUdsError> {
    let request = NativeOverlayTextQuery {
        selection: ViewSelection::Exact(text_result().view),
        instance: text_result().instance,
        session: overlay_document().session,
        query: text_query(2),
        expected_corpus,
    };
    scripted_reply(
        NativeIndexResponse::OverlayTextRanked(reply),
        |client, budget| client.search_overlay_text(request, budget),
    )
}

#[test]
fn overlay_text_sdk_accepts_exact_origin_revision_and_stamp()
-> Result<(), Box<dyn std::error::Error>> {
    let valid = overlay_text_result()?;
    if overlay_text_reply(valid.clone(), Some(valid.corpus.identity))? != valid {
        return Err("overlay SDK changed an exact selected corpus".into());
    }
    let mut retired = valid;
    let revision = fixture_revision_mut(&mut retired)?;
    revision.retired = true;
    revision.deleted = true;
    fixture_hit_mut(&mut retired, 0)?.source =
        NativeOverlayTextSource::Durable(fixed_durable_source()?);
    if overlay_text_reply(retired.clone(), None)? != retired {
        return Err("overlay SDK rejected a retired durable origin".into());
    }
    Ok(())
}

#[test]
fn overlay_text_sdk_rejects_foreign_corpus_origin_rank_and_cardinality()
-> Result<(), Box<dyn std::error::Error>> {
    let valid = overlay_text_result()?;
    let first_owner = valid.hits.first().ok_or("fixed first hit missing")?.owner;
    let mut malformed = Vec::new();

    let mut wrong_view = valid.clone();
    wrong_view.view.sequence = wrong_view
        .view
        .sequence
        .checked_add(1)
        .ok_or("fixed view sequence overflow")?;
    malformed.push(wrong_view);
    let mut wrong_instance = valid.clone();
    wrong_instance.instance = IndexInstanceId([99; 16]);
    malformed.push(wrong_instance);
    let mut wrong_session = valid.clone();
    wrong_session.corpus.session.epoch = wrong_session
        .corpus
        .session
        .epoch
        .checked_add(1)
        .ok_or("fixed session epoch overflow")?;
    malformed.push(wrong_session);
    let mut wrong_stamp = valid.clone();
    wrong_stamp.corpus.identity = Digest([51; 32]);
    malformed.push(wrong_stamp);
    let mut incomplete = valid.clone();
    incomplete.exhaustive = false;
    malformed.push(incomplete);
    let mut wrong_total = valid.clone();
    wrong_total.matched = 1;
    malformed.push(wrong_total);
    let mut missing_hit = valid.clone();
    let _removed = missing_hit.hits.pop();
    malformed.push(missing_hit);
    let mut zero_revision = valid.clone();
    fixture_revision_mut(&mut zero_revision)?.revision = 0;
    fixture_hit_mut(&mut zero_revision, 0)?.source =
        NativeOverlayTextSource::Overlay { revision: 0 };
    malformed.push(zero_revision);
    let mut duplicate_revision = valid.clone();
    let repeated = *duplicate_revision
        .corpus
        .revisions
        .first()
        .ok_or("fixed revision missing")?;
    duplicate_revision.corpus.revisions.push(repeated);
    malformed.push(duplicate_revision);
    let mut unsorted_revision = valid.clone();
    unsorted_revision.corpus.revisions.insert(
        0,
        NativeOverlayRevision {
            owner: valid.hits.get(1).ok_or("fixed second hit missing")?.owner,
            revision: 1,
            deleted: true,
            retired: false,
        },
    );
    malformed.push(unsorted_revision);
    let mut active_durable = valid.clone();
    fixture_hit_mut(&mut active_durable, 0)?.source =
        NativeOverlayTextSource::Durable(fixed_durable_source()?);
    malformed.push(active_durable);
    let mut absent_overlay = valid.clone();
    fixture_hit_mut(&mut absent_overlay, 1)?.source =
        NativeOverlayTextSource::Overlay { revision: 1 };
    malformed.push(absent_overlay);
    let mut wrong_revision = valid.clone();
    fixture_hit_mut(&mut wrong_revision, 0)?.source =
        NativeOverlayTextSource::Overlay { revision: 4 };
    malformed.push(wrong_revision);
    let mut deleted_overlay = valid.clone();
    fixture_revision_mut(&mut deleted_overlay)?.deleted = true;
    malformed.push(deleted_overlay);
    let mut retired_overlay = valid.clone();
    fixture_revision_mut(&mut retired_overlay)?.retired = true;
    malformed.push(retired_overlay);
    let mut retired_with_body = valid.clone();
    fixture_revision_mut(&mut retired_with_body)?.retired = true;
    fixture_hit_mut(&mut retired_with_body, 0)?.source =
        NativeOverlayTextSource::Durable(fixed_durable_source()?);
    malformed.push(retired_with_body);
    let mut duplicate_owner = valid.clone();
    fixture_hit_mut(&mut duplicate_owner, 1)?.owner = first_owner;
    malformed.push(duplicate_owner);
    let mut invalid_path = valid.clone();
    fixture_hit_mut(&mut invalid_path, 0)?.path = "../outside.rs".into();
    malformed.push(invalid_path);
    let mut wrong_rank = valid.clone();
    wrong_rank.hits.swap(0, 1);
    malformed.push(wrong_rank);

    for reply in malformed {
        if !matches!(
            overlay_text_reply(reply, Some(valid.corpus.identity)),
            Err(NativeUdsError::Protocol(_))
        ) {
            return Err("overlay SDK accepted malformed selected search evidence".into());
        }
    }
    Ok(())
}

#[test]
fn text_count_bound_preserves_exact_reply_cardinality() -> Result<(), Box<dyn std::error::Error>> {
    let mut result = text_result();
    result.matched = 2;
    let mut query = text_query(2);
    query.query_text = "needle count:1".into();
    let actual = scripted_reply(
        NativeIndexResponse::TextRanked(result.clone()),
        |client, budget| {
            client.search_text(
                ViewSelection::Exact(result.view),
                result.instance,
                query.clone(),
                budget,
            )
        },
    )?;
    if actual != result {
        return Err("count:1 lost the fixed complete match total".into());
    }
    let mut missing = result.clone();
    missing.hits.clear();
    if !matches!(
        scripted_reply(
            NativeIndexResponse::TextRanked(missing),
            |client, budget| client.search_text(
                ViewSelection::Exact(result.view),
                result.instance,
                query,
                budget
            )
        ),
        Err(NativeUdsError::Protocol(_))
    ) {
        return Err("count:1 accepted an incomplete ranked prefix".into());
    }
    Ok(())
}

#[test]
fn text_repeated_count_uses_last_bound_for_reply_cardinality()
-> Result<(), Box<dyn std::error::Error>> {
    let mut two_hits = text_result();
    two_hits.matched = 2;
    let mut second = two_hits
        .hits
        .first()
        .ok_or("fixed text fixture has no first hit")?
        .clone();
    second.owner.stable_id = StableOwnerId([12; 16]);
    second.path = "src/second.rs".into();
    second.score = FiniteScore::new(1.0).map_err(|_error| "invalid fixed score")?;
    two_hits.hits.push(second);
    let mut one_hit = two_hits.clone();
    one_hit.hits.truncate(1);

    for (source, accepted, refused) in [
        ("needle count:1 count:2", &two_hits, &one_hit),
        ("needle count:2 count:1", &one_hit, &two_hits),
    ] {
        let mut query = text_query(2);
        query.query_text = source.into();
        let observed = scripted_reply(
            NativeIndexResponse::TextRanked((*accepted).clone()),
            |client, budget| {
                client.search_text(
                    ViewSelection::Exact(two_hits.view),
                    two_hits.instance,
                    query.clone(),
                    budget,
                )
            },
        )?;
        if observed != *accepted {
            return Err(format!("{source} changed the fixed last-count result").into());
        }
        if !matches!(
            scripted_reply(
                NativeIndexResponse::TextRanked((*refused).clone()),
                |client, budget| client.search_text(
                    ViewSelection::Exact(two_hits.view),
                    two_hits.instance,
                    query,
                    budget,
                )
            ),
            Err(NativeUdsError::Protocol(_))
        ) {
            return Err(format!("{source} accepted a reply with the wrong row count").into());
        }
    }
    Ok(())
}

#[test]
fn native_text_refuses_unbounded_zero_and_forbidden_regex_before_connect()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let client = NativeUdsClient::new(directory.path().join("absent.sock"));
    let result = text_result();
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(2));
    for source in ["needle count:all", "needle count:0", r"/(?<=foo)/"] {
        let mut query = text_query(2);
        query.query_text = source.into();
        if !matches!(
            client.search_text(
                ViewSelection::Exact(result.view),
                result.instance,
                query,
                &budget,
            ),
            Err(NativeUdsError::Protocol(_))
        ) {
            return Err(format!("{source} was not refused before socket connection").into());
        }
    }
    let mut legal_regex = text_query(2);
    legal_regex.query_text = r"/\p{L}/".into();
    if !matches!(
        client.search_text(
            ViewSelection::Exact(result.view),
            result.instance,
            legal_regex,
            &budget,
        ),
        Err(NativeUdsError::Transport(_))
    ) {
        return Err("legal regex was refused before the missing socket was contacted".into());
    }
    Ok(())
}
