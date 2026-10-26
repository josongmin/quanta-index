//! A small product-daemon UDS witness for query trace ancestry.

use super::*;
use std::collections::BTreeMap;
use std::io::Read as _;

use quanta_index_contract::FiniteScore;
use quanta_index_contract::native_protocol::{
    NativeOverlayDocument, NativeOverlayEffortContext, NativeOverlayReply, NativeOverlayRequest,
    NativeOverlaySession, NativeOverlayTextSource, NativeSemanticResult,
};

const TRACE_READ_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Debug, Eq, PartialEq)]
struct QueryResults {
    durable_fast: NativeTextResult,
    durable_deep: NativeSemanticResult,
    overlay_fast_owner: OwnerKey,
    overlay_fast_path: String,
    overlay_fast_score: FiniteScore,
    overlay_deep_owner: OwnerKey,
    overlay_deep_revision: u64,
    overlay_deep_score: FiniteScore,
}

fn query_text() -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "needle".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 1,
        cursor: None,
    }
}

fn query_semantic() -> SemanticQueryRequest {
    SemanticQueryRequest {
        query_text: "needle".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        lexical_scope: None,
        top_k: 1,
    }
}

fn selected_materialized(
    client: &NativeUdsClient,
    source_view: quanta_index_contract::index::ViewRef,
    semantic: &InstanceSpec,
    expected_owner: OwnerKey,
) -> TestResult<quanta_index_contract::index::ViewRef> {
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(15))
        .ok_or("small UDS query materialization deadline overflow")?;
    loop {
        let latest = client.read_view(
            ViewSelection::Latest {
                workspace: workspace(),
                epoch: 1,
            },
            &budget(),
        )?;
        if latest.identity.sequence > source_view.sequence {
            let result = client.search_semantic(
                ViewSelection::Exact(latest.identity),
                semantic.instance,
                query_semantic(),
                &budget(),
            )?;
            if result.coverage.status == CoverageStatus::Complete
                && result.hits.len() == 1
                && result
                    .hits
                    .first()
                    .is_some_and(|hit| hit.owner == expected_owner)
            {
                return Ok(latest.identity);
            }
        }
        if Instant::now() >= deadline {
            return Err("small UDS query semantic materialization did not complete".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn stage_one_owner(
    client: &NativeUdsClient,
    semantic: &InstanceSpec,
) -> TestResult<(quanta_index_contract::index::ViewRef, OwnerKey)> {
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("small UDS query workspace creation failed".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![text_spec(), semantic.clone()],
        },
        &budget(),
    )?;
    let configured = published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)?;
    let key = operation(2);
    let ingress = client.begin_input(key, &budget())?;
    let sources = client.stage_sources(
        ingress,
        vec![SourceInput {
            path: "semantic.rs".into(),
            bytes: b"needle needle".to_vec(),
            text: SourceTextAdmission::Utf8 {
                policy: Digest([31; 32]),
            },
        }],
        &budget(),
    )?;
    let [source] = sources.as_slice() else {
        return Err("small UDS query source staging failed".into());
    };
    let before = client.read_view(ViewSelection::Exact(configured), &budget())?;
    let owner = OwnerKey {
        producer: key.producer,
        stable_id: StableOwnerId([1; 16]),
    };
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner,
            expected: None,
            next: Some(*source),
        }],
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: before.source,
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
    let source_view = published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    let selected = selected_materialized(client, source_view, semantic, owner)?;
    Ok((selected, owner))
}

fn run_queries(
    client: &NativeUdsClient,
    selected: quanta_index_contract::index::ViewRef,
    semantic: &InstanceSpec,
    owner: OwnerKey,
) -> TestResult<QueryResults> {
    let mut durable_fast = None;
    let mut durable_deep = None;
    let done = client
        .query_effort_stream(
            NativeEffortStreamQuery {
                selection: ViewSelection::Exact(selected),
                text_instance: text_spec().instance,
                semantic_instance: semantic.instance,
                text_query: query_text(),
                semantic_query: query_semantic(),
                preference: NativeEffortPreference::Deep,
                overlay: None,
            },
            &budget(),
            &mut |stage| {
                match stage {
                    NativeEffortStreamStage::Fast(value) if durable_fast.is_none() => {
                        durable_fast = Some(value.clone());
                    }
                    NativeEffortStreamStage::Deep(value)
                        if durable_fast.is_some() && durable_deep.is_none() =>
                    {
                        durable_deep = Some(value.clone());
                    }
                    NativeEffortStreamStage::Fast(_)
                    | NativeEffortStreamStage::Deep(_)
                    | NativeEffortStreamStage::OverlayFast(_)
                    | NativeEffortStreamStage::OverlayDeep(_) => {
                        return Err(NativeUdsError::Protocol(
                            "small UDS durable effort stage order differs".into(),
                        ));
                    }
                }
                Ok(())
            },
        )
        .map_err(|error| format!("durable effort stream: {error}"))?;
    require_equal(&done.view, &selected, "durable effort selected view")?;
    require(done.deep_delivered, "durable effort Deep was not delivered")?;
    let durable_fast = durable_fast.ok_or("durable effort Fast missing")?;
    let durable_deep = durable_deep.ok_or("durable effort Deep missing")?;
    require_equal(&durable_fast.matched, &1, "durable Fast matched owners")?;
    require_equal(&durable_fast.hits.len(), &1, "durable Fast hits")?;
    let fast_hit = durable_fast
        .hits
        .first()
        .ok_or("durable Fast hit missing")?;
    require_equal(&fast_hit.owner, &owner, "durable Fast owner")?;
    require_equal(&fast_hit.path.as_str(), &"semantic.rs", "durable Fast path")?;
    require_equal(
        &durable_deep.coverage.status,
        &CoverageStatus::Complete,
        "durable Deep coverage",
    )?;
    require_equal(&durable_deep.hits.len(), &1, "durable Deep hits")?;
    let durable_deep_hit = durable_deep
        .hits
        .first()
        .ok_or("durable Deep hit missing")?;
    require_equal(&durable_deep_hit.owner, &owner, "durable Deep owner")?;
    require_equal(
        &durable_deep_hit.score.get(),
        &1.0,
        "Fixed8 durable Deep cosine",
    )?;

    let publisher = client
        .begin_interaction_publisher(Duration::from_secs(30), &budget())
        .map_err(|error| format!("overlay publisher begin: {error}"))?;
    let session = NativeOverlaySession {
        workspace: workspace(),
        publisher,
        epoch: 1,
    };
    require_equal(
        &client
            .overlay(NativeOverlayRequest::Begin { session }, &budget())
            .map_err(|error| format!("overlay session begin: {error}"))?,
        &NativeOverlayReply::Begun(session),
        "overlay session begin",
    )?;
    let document = NativeOverlayDocument { session, owner };
    require_equal(
        &client
            .update_overlay_at_path(
                document,
                None,
                1,
                Some("needle".into()),
                Some("overlay.rs".into()),
                &budget(),
            )
            .map_err(|error| format!("overlay document update: {error}"))?,
        &NativeOverlayReply::Updated {
            document,
            revision: 1,
        },
        "overlay document update",
    )?;
    let mut overlay_fast = None;
    let mut overlay_deep = None;
    let overlay_done = client
        .query_effort_stream(
            NativeEffortStreamQuery {
                selection: ViewSelection::Exact(selected),
                text_instance: text_spec().instance,
                semantic_instance: semantic.instance,
                text_query: query_text(),
                semantic_query: query_semantic(),
                preference: NativeEffortPreference::Deep,
                overlay: Some(NativeOverlayEffortContext {
                    session,
                    expected_corpus: None,
                }),
            },
            &budget(),
            &mut |stage| {
                match stage {
                    NativeEffortStreamStage::OverlayFast(value) if overlay_fast.is_none() => {
                        overlay_fast = Some(value.clone());
                    }
                    NativeEffortStreamStage::OverlayDeep(value)
                        if overlay_fast.is_some() && overlay_deep.is_none() =>
                    {
                        overlay_deep = Some(value.clone());
                    }
                    NativeEffortStreamStage::Fast(_)
                    | NativeEffortStreamStage::Deep(_)
                    | NativeEffortStreamStage::OverlayFast(_)
                    | NativeEffortStreamStage::OverlayDeep(_) => {
                        return Err(NativeUdsError::Protocol(
                            "small UDS overlay effort stage order differs".into(),
                        ));
                    }
                }
                Ok(())
            },
        )
        .map_err(|error| format!("overlay effort stream: {error}"))?;
    require_equal(
        &overlay_done.view,
        &selected,
        "overlay effort selected view",
    )?;
    require(
        overlay_done.deep_delivered,
        "overlay effort Deep was not delivered",
    )?;
    let overlay_fast = overlay_fast.ok_or("overlay effort Fast missing")?;
    let overlay_deep = overlay_deep.ok_or("overlay effort Deep missing")?;
    require_equal(&overlay_fast.matched, &1, "overlay Fast matched owners")?;
    require_equal(&overlay_fast.hits.len(), &1, "overlay Fast hits")?;
    let overlay_hit = overlay_fast
        .hits
        .first()
        .ok_or("overlay Fast hit missing")?;
    require_equal(&overlay_hit.owner, &owner, "overlay Fast owner")?;
    require_equal(
        &overlay_hit.path.as_str(),
        &"overlay.rs",
        "overlay Fast path",
    )?;
    require_equal(
        &overlay_hit.source,
        &NativeOverlayTextSource::Overlay { revision: 1 },
        "overlay Fast source",
    )?;
    require_equal(&overlay_deep.hits.len(), &1, "overlay Deep hits")?;
    let deep_hit = overlay_deep
        .hits
        .first()
        .ok_or("overlay Deep hit missing")?;
    require_equal(&deep_hit.owner, &owner, "overlay Deep owner")?;
    require_equal(&deep_hit.score.get(), &1.0, "Fixed8 overlay Deep cosine")?;
    let revision = deep_hit
        .overlay
        .as_ref()
        .ok_or("overlay Deep did not rank the unsaved document")?
        .revision;
    require_equal(&revision, &1, "overlay Deep revision")?;
    require_equal(
        &overlay_done.corpus.as_ref(),
        &Some(&overlay_fast.corpus),
        "overlay effort corpus",
    )?;
    require_equal(
        &client.overlay(NativeOverlayRequest::Close { session }, &budget())?,
        &NativeOverlayReply::Closed(session),
        "overlay session close",
    )?;
    Ok(QueryResults {
        durable_fast,
        durable_deep,
        overlay_fast_owner: overlay_hit.owner,
        overlay_fast_path: overlay_hit.path.clone(),
        overlay_fast_score: overlay_hit.score,
        overlay_deep_owner: deep_hit.owner,
        overlay_deep_revision: revision,
        overlay_deep_score: deep_hit.score,
    })
}

#[derive(Clone)]
struct Span {
    stage: String,
    parent: Option<u64>,
    root_kind: Option<String>,
}

fn ancestor_is(spans: &BTreeMap<u64, Span>, child: u64, ancestor: u64) -> bool {
    let mut current = spans.get(&child).and_then(|span| span.parent);
    for _ in 0..spans.len() {
        if current == Some(ancestor) {
            return true;
        }
        current = current.and_then(|id| spans.get(&id).and_then(|span| span.parent));
        if current.is_none() {
            return false;
        }
    }
    false
}

fn required_capture_id(row: &serde_json::Value) -> TestResult<&str> {
    let id = row
        .get("capture_id")
        .and_then(serde_json::Value::as_str)
        .ok_or("small UDS query capture id missing")?;
    require(
        id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "small UDS query capture id is not 32 hexadecimal digits",
    )?;
    Ok(id)
}

fn trace_witness(path: &Path) -> TestResult {
    let mut bytes = Vec::new();
    let _read = fs::File::open(path)?
        .take(TRACE_READ_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    require(
        u64::try_from(bytes.len())? <= TRACE_READ_LIMIT,
        "small UDS query trace exceeded read bound",
    )?;
    let mut header = None;
    let mut footer = None;
    let mut event_count = 0_u64;
    let mut spans = BTreeMap::<u64, Span>::new();
    let mut ended = BTreeMap::<u64, String>::new();
    for raw in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let row: serde_json::Value = serde_json::from_slice(raw)?;
        match row.get("record_type").and_then(serde_json::Value::as_str) {
            Some("header") if header.is_none() && event_count == 0 => {
                let _capture_id = required_capture_id(&row)?;
                header = Some(row);
            }
            Some("event") if header.is_some() && footer.is_none() => {
                let expected_capture = required_capture_id(
                    header
                        .as_ref()
                        .ok_or("small UDS query trace header missing")?,
                )?;
                require_equal(
                    &required_capture_id(&row)?,
                    &expected_capture,
                    "small UDS query event capture identity",
                )?;
                event_count = event_count
                    .checked_add(1)
                    .ok_or("small UDS query event count overflow")?;
                require_equal(
                    &row.get("output_seq").and_then(serde_json::Value::as_u64),
                    &Some(event_count),
                    "small UDS query trace output sequence",
                )?;
                let kind = row
                    .get("kind")
                    .ok_or("small UDS query event kind missing")?;
                match kind.get("type").and_then(serde_json::Value::as_str) {
                    Some("span_begin") => {
                        let id = kind
                            .get("span_id")
                            .and_then(serde_json::Value::as_u64)
                            .ok_or("small UDS query span id missing")?;
                        let stage = kind
                            .get("stage")
                            .and_then(serde_json::Value::as_str)
                            .ok_or("small UDS query stage missing")?;
                        require(
                            spans
                                .insert(
                                    id,
                                    Span {
                                        stage: stage.into(),
                                        parent: kind
                                            .get("parent_span_id")
                                            .and_then(serde_json::Value::as_u64),
                                        root_kind: kind
                                            .get("root_kind")
                                            .and_then(serde_json::Value::as_str)
                                            .map(str::to_owned),
                                    },
                                )
                                .is_none(),
                            "small UDS query duplicate span",
                        )?;
                    }
                    Some("span_end") => {
                        let id = kind
                            .get("span_id")
                            .and_then(serde_json::Value::as_u64)
                            .ok_or("small UDS query terminal span id missing")?;
                        let outcome = kind
                            .get("outcome")
                            .and_then(serde_json::Value::as_str)
                            .ok_or("small UDS query terminal outcome missing")?;
                        require(
                            ended.insert(id, outcome.into()).is_none(),
                            "small UDS query duplicate span terminal",
                        )?;
                    }
                    Some("health") => {
                        return Err("small UDS query trace reported health loss".into());
                    }
                    Some(_) => {}
                    None => return Err("small UDS query event type missing".into()),
                }
            }
            Some("footer") if header.is_some() && footer.is_none() => footer = Some(row),
            _ => return Err("small UDS query trace record order differs".into()),
        }
    }
    let header = header.ok_or("small UDS query trace header missing")?;
    let footer = footer.ok_or("small UDS query trace footer missing")?;
    require_equal(
        &header
            .get("schema_version")
            .and_then(serde_json::Value::as_u64),
        &Some(4),
        "small UDS query trace schema",
    )?;
    require_equal(
        &required_capture_id(&footer)?,
        &required_capture_id(&header)?,
        "small UDS query footer capture identity",
    )?;
    require_equal(
        &footer.get("status").and_then(serde_json::Value::as_str),
        &Some("complete"),
        "small UDS query capture completion",
    )?;
    for field in [
        "producer_leases",
        "events_dropped",
        "events_unwritten",
        "open_spans",
    ] {
        require_equal(
            &footer.get(field).and_then(serde_json::Value::as_u64),
            &Some(0),
            &format!("small UDS query {field}"),
        )?;
    }
    for field in ["events_written", "last_output_seq"] {
        require_equal(
            &footer.get(field).and_then(serde_json::Value::as_u64),
            &Some(event_count),
            &format!("small UDS query {field}"),
        )?;
    }
    require_equal(&spans.len(), &ended.len(), "small UDS query closed spans")?;
    for id in spans.keys() {
        require(
            ended.contains_key(id),
            "small UDS query span terminal missing",
        )?;
    }
    let stages = |name: &str| -> Vec<u64> {
        spans
            .iter()
            .filter_map(|(id, span)| (span.stage == name).then_some(*id))
            .collect()
    };
    let snapshots = stages("query_overlay_snapshot");
    let fast = stages("query_fast");
    let deep = stages("query_deep");
    let lexical = stages("query_lexical");
    let embedding = stages("query_embedding");
    let overlay_embedding = stages("query_overlay_embedding");
    require_equal(&snapshots.len(), &1, "overlay snapshot span count")?;
    require_equal(&fast.len(), &2, "Fast span count")?;
    require_equal(&deep.len(), &2, "Deep span count")?;
    require_equal(&lexical.len(), &2, "lexical span count")?;
    require_equal(&embedding.len(), &2, "query provider span count")?;
    require_equal(&overlay_embedding.len(), &1, "overlay provider span count")?;
    for id in snapshots
        .iter()
        .chain(fast.iter())
        .chain(deep.iter())
        .chain(lexical.iter())
        .chain(embedding.iter())
        .chain(overlay_embedding.iter())
    {
        require_equal(
            &ended.get(id).map(String::as_str),
            &Some("ok"),
            "small UDS query stage terminal",
        )?;
    }
    let snapshot = *snapshots.first().ok_or("overlay snapshot span missing")?;
    let overlay_parent = spans.get(&snapshot).and_then(|span| span.parent);
    let overlay_fast = fast
        .iter()
        .copied()
        .find(|id| spans.get(id).and_then(|span| span.parent) == overlay_parent)
        .ok_or("overlay Fast does not share Snapshot request")?;
    let overlay_deep = deep
        .iter()
        .copied()
        .find(|id| spans.get(id).and_then(|span| span.parent) == overlay_parent)
        .ok_or("overlay Deep does not share Snapshot request")?;
    require(
        overlay_parent
            .and_then(|id| spans.get(&id))
            .is_some_and(|span| {
                span.stage == "request" && span.root_kind.as_deref() == Some("request")
            }),
        "overlay query stages lack transport request root",
    )?;
    let durable_fast = *fast
        .iter()
        .find(|id| **id != overlay_fast)
        .ok_or("durable Fast span missing")?;
    let durable_deep = *deep
        .iter()
        .find(|id| **id != overlay_deep)
        .ok_or("durable Deep span missing")?;
    let durable_parent = spans.get(&durable_fast).and_then(|span| span.parent);
    require(
        durable_parent != overlay_parent
            && durable_parent == spans.get(&durable_deep).and_then(|span| span.parent)
            && durable_parent
                .and_then(|id| spans.get(&id))
                .is_some_and(|span| {
                    span.stage == "request" && span.root_kind.as_deref() == Some("request")
                }),
        "durable query does not retain its separate transport request root",
    )?;
    for parent in [durable_fast, overlay_fast] {
        require(
            lexical.iter().any(|id| ancestor_is(&spans, *id, parent)),
            "Fast did not parent its lexical span",
        )?;
    }
    for parent in [durable_deep, overlay_deep] {
        require(
            embedding.iter().any(|id| ancestor_is(&spans, *id, parent)),
            "Deep did not parent its query embedding span",
        )?;
    }
    require(
        overlay_embedding
            .iter()
            .all(|id| ancestor_is(&spans, *id, overlay_deep)),
        "overlay provider escaped Overlay Deep",
    )?;
    Ok(())
}

#[test]
fn product_uds_effort_record_keeps_results_and_query_ancestry() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = temp.path().join("state");
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-query-trace.json");
    let trace_path = temp.path().join("native-query-trace.qtrace");
    let stub = EmbeddingStub::start()?;
    let base_url = stub.base_url();
    let mut config = config_with_openai_semantic(&root, &socket)?;
    configure_semantic_endpoint(&mut config, &base_url)?;
    configure_owned_semantic(&mut config)?;
    configure_effort(&mut config)?;
    let previous = config
        .as_object_mut()
        .ok_or("small UDS query configuration is not an object")?
        .insert(
            "overlay".into(),
            serde_json::json!({
                "text_instance": vec![71_u8; 16],
                "sessions": 2,
                "documents": 4,
                "document_bytes": 4096,
                "total_bytes": 16384
            }),
        );
    require(
        previous.is_none(),
        "small UDS query overlay config already exists",
    )?;
    fs::write(&config_path, serde_json::to_vec(&config)?)?;
    let semantic = openai_semantic_spec(&base_url)?;
    let off = Daemon::start_with_api_key(&config_path, &socket, Some("test-key"))?;
    let client = NativeUdsClient::new(socket.clone());
    let (selected, owner) = stage_one_owner(&client, &semantic)
        .map_err(|error| format!("small UDS source/materialization: {error}"))?;
    let off_results = run_queries(&client, selected, &semantic, owner)
        .map_err(|error| format!("small UDS Off queries: {error}"))?;
    off.stop()?;

    let record = Daemon::start_with_api_key_and_trace(
        &config_path,
        &socket,
        Some("test-key"),
        Some(&trace_path),
    )?;
    let client = NativeUdsClient::new(socket);
    let recorded_results = run_queries(&client, selected, &semantic, owner)
        .map_err(|error| format!("small UDS Record queries: {error}"))?;
    require_equal(
        &recorded_results,
        &off_results,
        "Off/Record UDS query results",
    )?;
    // The SDK admits valid Native syntax, then the server must refuse its
    // unsupported generation selector before a stage without logging query content.
    let mut unsupported = query_text();
    unsupported.query_text = "private-query-must-not-be-logged".into();
    unsupported.generation_selector = Some(quanta_index_contract::GenerationSelector::Active {
        repo_id: quanta_index_contract::RepoId::new("unsupported-query-repo")?,
        revision_id: quanta_index_contract::RevisionId::new("unsupported-query-revision")?,
    });
    let refusal = client.query_effort_stream(
        NativeEffortStreamQuery {
            selection: ViewSelection::Exact(selected),
            text_instance: text_spec().instance,
            semantic_instance: semantic.instance,
            text_query: unsupported,
            semantic_query: query_semantic(),
            preference: NativeEffortPreference::Deep,
            overlay: None,
        },
        &budget(),
        &mut |_stage| {
            Err(NativeUdsError::Protocol(
                "unsupported generation selector query delivered a stage".into(),
            ))
        },
    );
    require(
        matches!(
            refusal,
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::InvalidInput
        ),
        "unsupported generation selector query did not retain its original InvalidInput refusal",
    )?;
    let diagnostics_reader = record.stderr_reader()?;
    record.stop()?;
    trace_witness(&trace_path)?;
    let mut diagnostics = String::new();
    let _read = diagnostics_reader
        .take(STDERR_OBSERVATION_BYTES + 1)
        .read_to_string(&mut diagnostics)?;
    require(
        u64::try_from(diagnostics.len())? <= STDERR_OBSERVATION_BYTES,
        "small UDS diagnostic read exceeded its bound",
    )?;
    require(
        !diagnostics.contains("private-query-must-not-be-logged"),
        "query diagnostics exposed request content",
    )?;
    let mut failures = 0;
    let mut postterminal_cancellations = 0;
    for line in diagnostics
        .lines()
        .filter(|line| line.contains("\"event\":\"native_effort_error_observed\""))
    {
        let event: serde_json::Value = serde_json::from_str(line)?;
        require(
            event
                .get("request_id")
                .and_then(serde_json::Value::as_u64)
                .is_some()
                && event
                    .get("connection_id")
                    .and_then(serde_json::Value::as_u64)
                    .is_some()
                && event.as_object().map(serde_json::Map::len) == Some(10)
                && event
                    .get("sample_limit")
                    .and_then(serde_json::Value::as_u64)
                    == Some(8)
                && event
                    .get("sample_ordinal")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|ordinal| (1..=8).contains(&ordinal))
                && event
                    .get("first_local_denial")
                    .and_then(serde_json::Value::as_str)
                    == Some("None"),
            format!("actual UDS query error changed fixed diagnostic metadata: {event}"),
        )?;
        if event
            .get("terminal_emitted")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            // The SDK closes after EffortDone; a later server close/checkpoint
            // may observe peer cancellation. It is not the explicit refusal.
            require(
                event.get("outcome").and_then(serde_json::Value::as_str) == Some("Err(Cancelled)")
                    && event
                        .get("fast_delivered")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
                    && event
                        .get("deep_delivered")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true),
                format!("unexpected postterminal query observation: {event}"),
            )?;
            postterminal_cancellations += 1;
            continue;
        }
        require(
            event.get("outcome").and_then(serde_json::Value::as_str) == Some("Err(InvalidInput)")
                && event
                    .get("fast_delivered")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
                && event
                    .get("deep_delivered")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false)
                && event
                    .get("terminal_emitted")
                    .and_then(serde_json::Value::as_bool)
                    == Some(false),
            format!("actual UDS query refusal changed fixed diagnostic metadata: {event}"),
        )?;
        failures += 1;
    }
    require_equal(&failures, &1, "actual UDS query failure diagnostic count")?;
    require(
        postterminal_cancellations <= 2,
        "postterminal query cancellation diagnostics exceeded their prefix cap",
    )?;
    let inputs = stub.observed_inputs()?;
    require(
        inputs.iter().any(|input| input == "needle needle")
            && inputs
                .iter()
                .filter(|input| input.as_str() == "needle")
                .count()
                >= 4,
        format!("small UDS query did not exercise source and query provider inputs: {inputs:?}"),
    )?;
    stub.finish()?;
    Ok(())
}
