//! Product regressions for the retired RepoMap wire and retained Native reads.

use std::{collections::BTreeMap, os::unix::fs::MetadataExt as _, path::PathBuf};

use quanta_index_contract::{
    FileId, ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV2,
    RepoMapActiveHeadRequestV2, RepoMapEdgeKind, RepoMapExactnessSummary, RepoMapFileNode,
    RepoMapGraphCoverage, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapNode,
    RepoMapNodeRef, RepoMapPublishBundleRequestV2, RepoMapRedactionState, RepoMapSourceBundle,
    RepoRelativePath, RevisionId, SearchPlaneControlIpcRequestEnvelope,
    SearchPlaneIngestIpcRequestEnvelope, SearchPlaneQueryIpcRequestEnvelope,
    index::{
        ProducerCompleteness, ProjectionCertificate, ProjectionId, ProjectionOwnerContribution,
        ProjectionTransition, ViewRef,
    },
    native_protocol::{
        GraphEdge, GraphNode, GraphOwnerOutput, NativeGraphDirection, NativeGraphQuery,
        NativeGraphResult, NativeIndexRequestEnvelope,
    },
};
use quanta_index_searchd_harness::product_xfs::ProductXfsProfile;
use serde::ser::{SerializeStruct as _, SerializeStructVariant as _};

use super::*;

struct OldRankQuery<'a> {
    query_text: &'a str,
    focus_subjects: [(); 0],
    top_k: u32,
    token_budget: u32,
}

impl serde::Serialize for OldRankQuery<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut query = serializer.serialize_struct("NativeRepoMapQuery", 4)?;
        query.serialize_field("query_text", self.query_text)?;
        query.serialize_field("focus_subjects", &self.focus_subjects)?;
        query.serialize_field("top_k", &self.top_k)?;
        query.serialize_field("token_budget", &self.token_budget)?;
        query.end()
    }
}

struct OldRepoMapRequest<'a> {
    selection: &'a ViewSelection,
    instance: IndexInstanceId,
    query: OldRankQuery<'a>,
}

impl serde::Serialize for OldRepoMapRequest<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The pre-retirement NativeIndexRequest encoder used variant 37 and
        // these three fields in this order. Keep the encoder independent of
        // the current request enum so the removed tag cannot be reintroduced.
        let mut variant =
            serializer.serialize_struct_variant("NativeIndexRequest", 37, "QueryRepoMap", 3)?;
        variant.serialize_field("selection", self.selection)?;
        variant.serialize_field("instance", &self.instance)?;
        variant.serialize_field("query", &self.query)?;
        variant.end()
    }
}

struct OldEnvelope<'a> {
    request_id: u64,
    payload: OldRepoMapRequest<'a>,
}

impl serde::Serialize for OldEnvelope<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut envelope = serializer.serialize_struct("NativeIndexRequestEnvelope", 2)?;
        envelope.serialize_field("request_id", &self.request_id)?;
        envelope.serialize_field("payload", &self.payload)?;
        envelope.end()
    }
}

struct OldLegacyIngest<'a>(&'a quanta_index_contract::RepoMapPublishBundleRequestV2);

impl serde::Serialize for OldLegacyIngest<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Match the removed ingest encoder's newtype variant and payload.
        serializer.serialize_newtype_variant(
            "SearchPlaneIngestIpcRequest",
            14,
            "PublishRepoMapBundleV2",
            self.0,
        )
    }
}

struct OldLegacyIngestEnvelope<'a>(OldLegacyIngest<'a>);

impl serde::Serialize for OldLegacyIngestEnvelope<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut envelope = serializer.serialize_struct("SearchPlaneIngestIpcRequestEnvelope", 2)?;
        envelope.serialize_field("request_id", &12_u64)?;
        envelope.serialize_field("payload", &self.0)?;
        envelope.end()
    }
}

type StateSnapshot = BTreeMap<PathBuf, (u64, u64, Option<Vec<u8>>)>;

fn root_snapshot(root: &Path) -> TestResult<StateSnapshot> {
    fn visit(root: &Path, directory: &Path, files: &mut StateSnapshot) -> TestResult {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            let content = if metadata.is_dir() {
                None
            } else {
                require(
                    metadata.is_file(),
                    format!("unexpected state entry: {}", path.display()),
                )?;
                Some(fs::read(&path)?)
            };
            let previous = files.insert(
                path.strip_prefix(root)?.to_path_buf(),
                (metadata.dev(), metadata.ino(), content),
            );
            require(previous.is_none(), "duplicate state entry")?;
            if metadata.is_dir() {
                visit(root, &path, files)?;
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

fn product_binary() -> TestResult<PathBuf> {
    Ok(selected_test_daemon_binary(
        "retired wire product proof requires QUANTA_INDEX_SEARCHD_BIN",
        "product daemon path must be absolute",
    )?
    .path)
}

fn require_unknown_opcode<T: serde::de::DeserializeOwned>(
    frame: &[u8],
    tag: &str,
    reason: &str,
) -> TestResult {
    let error = match quanta_index_ipc::decode_request::<T, _>(&mut std::io::Cursor::new(frame)) {
        Ok(_) => return Err(format!("retired opcode {tag} decoded").into()),
        Err(error) => error,
    };
    require(
        matches!(&error, quanta_index_ipc::IpcError::Decode(message)
            if message.contains(reason) && message.contains(tag)),
        format!("retired opcode {tag} reached a different refusal: {error}"),
    )
}

fn require_native_peer_closes(socket: &Path, tag: &str, frame: &[u8]) -> TestResult {
    require_unknown_opcode::<NativeIndexRequestEnvelope>(frame, tag, "unknown field")?;
    let mut peer = UnixStream::connect(socket)?;
    peer.set_read_timeout(Some(Duration::from_secs(5)))?;
    peer.write_all(frame)?;
    peer.shutdown(Shutdown::Write)?;
    let mut reply = [0_u8; 1];
    let received = peer.read(&mut reply);
    require(
        matches!(&received, Ok(0))
            || matches!(&received, Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset),
        format!("retired {tag} received a reply or did not close: {received:?}"),
    )
}

#[test]
fn old_repomap_encoder_frame_closes_product_uds_without_dispatch_or_state_change() -> TestResult {
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let config_path = temp.path().join("native-config.json");
    let selected = selected_test_daemon_binary(
        "retired wire product proof requires QUANTA_INDEX_SEARCHD_BIN",
        "product daemon path must be absolute",
    )?;
    let socket = if selected.product {
        ProductXfsProfile::service_socket(temp.path(), "uds")?
    } else {
        temp.path().join("native.sock")
    };
    let profile = selected
        .product
        .then(ProductXfsProfile::from_env)
        .transpose()?;
    let product_root = profile
        .as_ref()
        .map(ProductXfsProfile::tempdir)
        .transpose()?;
    let root = product_root.as_ref().map_or_else(
        || temp.path().join("state"),
        |directory| directory.path().join("state"),
    );
    let graph = graph_spec();
    let mut settings = config(&root, &socket);
    settings["graph"] = serde_json::json!({
        "descriptor": {"kind": graph.kind.0, "schema": graph.schema.0, "format": graph.format.0},
        "read_need": settings["resources"]["execution"]["interactive"],
    });
    let sqlite_temp = if let Some(profile) = profile.as_ref() {
        profile.configure(&mut settings)?;
        Some(profile.prepare_root(&root, &socket)?)
    } else {
        None
    };
    fs::write(&config_path, serde_json::to_vec(&settings)?)?;
    let mut daemon = if let Some(sqlite_temp) = sqlite_temp.as_ref() {
        ProductXfsProfile::preflight(&selected.path, &config_path, sqlite_temp)?;
        Daemon::start_command_before(
            ProductXfsProfile::command(&selected.path, sqlite_temp),
            &config_path,
            &socket,
            None,
            None,
            None,
        )?
    } else {
        Daemon::start_binary(&selected.path, &config_path, &socket, None, None)?
    };
    let executable = read_view_daemon_witness::observe(daemon.0.id())?;
    let client = NativeUdsClient::new(socket.as_path());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("native workspace creation failed".into());
    };
    let (view, source_owner, source) = publish_retained_members(&client, genesis, graph)?;
    let unused_operation = operation(91);
    require_equal(
        &client.status(unused_operation, &budget())?,
        &OperationStatus::Unknown,
        "baseline operation",
    )?;
    let before = root_snapshot(&root)?;

    let selection = ViewSelection::Exact(view);
    let old = OldEnvelope {
        request_id: 91,
        payload: OldRepoMapRequest {
            selection: &selection,
            instance: IndexInstanceId([17; 16]),
            query: OldRankQuery {
                query_text: "old rank",
                focus_subjects: [],
                top_k: 5,
                token_budget: 1024,
            },
        },
    };
    require_native_peer_closes(
        &socket,
        "QueryRepoMap",
        &quanta_index_ipc::encode_request(&old)?,
    )?;
    for (tag, payload) in [
        (
            "ObserveRepoMapGraphInstallationV1",
            serde_json::to_value(selection)?,
        ),
        (
            "ObserveRepoMapEventFrontierV1",
            serde_json::json!({
                "view": view, "producer": unused_operation.producer, "stream": unused_operation.stream,
            }),
        ),
        (
            "ResolveProtectedRepoMapHead",
            serde_json::json!({
                "repo_id": "retired-wire-repo", "revision_id": "retired-wire-revision",
            }),
        ),
        (
            "ReadImportedRepoMapMember",
            serde_json::json!({"view": view, "instance": old.payload.instance}),
        ),
        (
            "ReadRepoMapPriorFileV1",
            serde_json::json!({
                "view": view, "instance": old.payload.instance, "owner": source_owner,
            }),
        ),
        (
            "QueryImportedRepoMap",
            serde_json::json!({
                "selection": selection, "instance": old.payload.instance,
                "query": {"source": graph_node("src/a.rs"), "direction": NativeGraphDirection::Outgoing,
                    "max_edges": 8, "after": null},
            }),
        ),
    ] {
        let envelope = serde_json::json!({"request_id": 91, "payload": {(tag): payload}});
        require_native_peer_closes(&socket, tag, &quanta_index_ipc::encode_request(&envelope)?)?;
    }
    require(
        daemon.0.try_wait()?.is_none(),
        "old frame stopped the product daemon",
    )?;
    require(
        root_snapshot(&root)? == before,
        "old frames changed persisted state root",
    )?;
    require_equal(
        &client.status(unused_operation, &budget())?,
        &OperationStatus::Unknown,
        "old frame operation",
    )?;
    require_equal(
        &client.read_view(selection, &budget())?.identity,
        &view,
        "maintained Source view",
    )?;
    for legacy in ["query.sock", "control.sock", "ingest.sock"] {
        require(
            !root.join("search-plane").join(legacy).exists(),
            format!("Native candidate exposed Legacy {legacy}"),
        )?;
    }
    require_retained_members(&client, view, source_owner, source)?;
    require(
        !root.join("repo-map").try_exists()?,
        "Native daemon created a dedicated RepoMap store",
    )?;
    daemon.stop()?;
    read_view_daemon_witness::write(
        [executable],
        "retired-repomap-wire-daemon.json",
        &nextest_test_event(
            module_path!(),
            "old_repomap_encoder_frame_closes_product_uds_without_dispatch_or_state_change",
        ),
        None,
    )
}

const GRAPH_PROJECTION: ProjectionId = ProjectionId([42; 16]);
const GRAPH_PRODUCER: ProducerId = ProducerId([41; 16]);

fn graph_spec() -> InstanceSpec {
    InstanceSpec {
        instance: IndexInstanceId([43; 16]),
        kind: IndexKind([47; 16]),
        group: PublicationGroupId([48; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::Projection {
            projection: GRAPH_PROJECTION,
            producer: GRAPH_PRODUCER,
        },
        schema: SchemaId([44; 16]),
        policy: Digest([45; 32]),
        format: StorageFormatId([49; 16]),
    }
}

fn graph_node(path: &str) -> RepoMapNodeRef {
    RepoMapNodeRef::File(FileId::new(path))
}

fn configure_profile(
    client: &NativeUdsClient<&Path>,
    view: ViewRef,
    key: OperationKey,
    instances: Vec<InstanceSpec>,
) -> TestResult<ViewRef> {
    let ingress = client.begin_input(key, &budget())?;
    let profile = client.stage_profile(ingress, IndexProfile { instances }, &budget())?;
    published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)
}

fn publish_retained_members(
    client: &NativeUdsClient<&Path>,
    genesis: ViewRef,
    graph: InstanceSpec,
) -> TestResult<(ViewRef, OwnerKey, ObjectRef)> {
    let configured = configure_profile(client, genesis, operation(1), vec![file_spec()])?;
    let key = operation(2);
    let ingress = client.begin_input(key, &budget())?;
    let source = client.stage_source(
        ingress,
        "src/a.rs".into(),
        b"fn retained() {}\n",
        SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        &budget(),
    )?;
    let owner = OwnerKey {
        producer: key.producer,
        stable_id: StableOwnerId([1; 16]),
    };
    let owners = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner,
            expected: None,
            next: Some(source),
        }],
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
    let source_view = published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;
    let graph_view = publish_graph(client, source_view, &graph)?;
    let view = configure_profile(client, graph_view, operation(3), vec![file_spec(), graph])?;
    Ok((view, owner, source))
}

fn publish_graph(
    client: &NativeUdsClient<&Path>,
    view: ViewRef,
    graph: &InstanceSpec,
) -> TestResult<ViewRef> {
    let key = OperationKey {
        producer: GRAPH_PRODUCER,
        stream: StreamId([46; 16]),
        ..operation(1)
    };
    let ingress = client.begin_input(key, &budget())?;
    let owner = OwnerKey {
        producer: key.producer,
        stable_id: StableOwnerId([1; 16]),
    };
    let output = GraphOwnerOutput {
        projection: Vec::new(),
        facts: None,
        nodes: vec![
            GraphNode {
                identity: graph_node("src/a.rs"),
                payload_digest: Digest([11; 32]),
            },
            GraphNode {
                identity: graph_node("src/b.rs"),
                payload_digest: Digest([12; 32]),
            },
        ],
        edges: vec![GraphEdge {
            kind: RepoMapEdgeKind::DependsOn,
            source: graph_node("src/a.rs"),
            target: graph_node("src/b.rs"),
        }],
    };
    let output =
        client.stage_projection_output(ingress, &serde_json::to_vec(&output)?, &budget())?;
    let contribution = client.stage_projection_owner(
        ingress,
        ProjectionOwnerContribution {
            projection: GRAPH_PROJECTION,
            producer: key.producer,
            owner,
            source_owner: None,
            source: None,
            dependency_stamp: Digest([1; 32]),
            output: Some(output),
        },
        &budget(),
    )?;
    let affected = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner,
            expected: None,
            next: Some(contribution),
        }],
        &budget(),
    )?;
    let target_source = client
        .read_view(ViewSelection::Exact(view), &budget())?
        .source;
    let certificate = client.stage_projection_certificate(
        ingress,
        ProjectionCertificate {
            projection: GRAPH_PROJECTION,
            producer: key.producer,
            prior: None,
            target_source,
            schema: graph.schema,
            producer_policy: graph.policy,
            dependency_context: Digest([50; 32]),
            affected,
        },
        &budget(),
    )?;
    let mut transition = ProjectionTransition {
        projection: GRAPH_PROJECTION,
        producer: key.producer,
        expected: None,
        target_source,
        schema: graph.schema,
        producer_policy: graph.policy,
        dependency_context: Digest([50; 32]),
        completeness: ProducerCompleteness::CompleteAffectedDomain { certificate },
        owners: affected,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: key.producer,
        stream: key.stream,
        stream_epoch: key.stream_epoch,
        predecessor: 0,
        sequence: 1,
        payload: transition.payload_digest()?,
    });
    published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
        },
        &budget(),
    )?)
}

fn graph_query() -> NativeGraphQuery {
    NativeGraphQuery {
        source: graph_node("src/a.rs"),
        direction: NativeGraphDirection::Outgoing,
        max_supporters: 8,
        max_edges: 8,
    }
}

fn require_retained_members(
    client: &NativeUdsClient<&Path>,
    view: ViewRef,
    owner: OwnerKey,
    source: ObjectRef,
) -> TestResult {
    let selection = ViewSelection::Exact(view);
    let paths = client.query_paths(selection, file_spec().instance, "src/", None, 8, &budget())?;
    require_equal(&paths.view, &view, "FileMap exact view")?;
    let [path] = paths.entries.as_slice() else {
        return Err(format!("expected one FileMap path, got {:?}", paths.entries).into());
    };
    require(
        path.path == "src/a.rs" && path.owner == owner && path.source == source && !paths.has_more,
        format!("wrong FileMap path: {path:?}"),
    )?;
    let paths = client.query_repository_paths(
        selection,
        file_spec().instance,
        "src/".into(),
        None,
        8,
        &budget(),
    )?;
    require_equal(&paths.view, &view, "repository path exact view")?;
    let [entry] = paths.entries.as_slice() else {
        return Err(format!("expected one repository path, got {:?}", paths.entries).into());
    };
    require(
        entry.repository.is_none() && entry.file == *path && !paths.has_more,
        format!("wrong repository path: {entry:?}"),
    )?;
    let instance = graph_spec().instance;
    let page = client.query_graph(selection, instance, graph_query(), &budget())?;
    require_graph_result(&page, view, instance)
}

fn require_graph_result(
    page: &NativeGraphResult,
    view: ViewRef,
    instance: IndexInstanceId,
) -> TestResult {
    require(
        page.view == view
            && page.instance == instance
            && page.exhaustive
            && page.source_supporters == 1
            && page
                .source
                .as_ref()
                .is_some_and(|source| source.payload_digest == Digest([11; 32])),
        format!("wrong Graph source: {page:?}"),
    )?;
    let [row] = page.edges.as_slice() else {
        return Err(format!("expected one Graph edge, got {:?}", page.edges).into());
    };
    require(
        row.edge.kind == RepoMapEdgeKind::DependsOn
            && row.edge.source == graph_node("src/a.rs")
            && row.edge.target == graph_node("src/b.rs")
            && row.supporters == 1,
        format!("wrong Graph edge: {row:?}"),
    )
}

#[test]
fn retired_legacy_repomap_query_control_and_ingest_frames_are_unknown() -> TestResult {
    let bundle = RepoMapSourceBundle::new(
        RepoId::new("retired-wire-repo")?,
        RevisionId::new("retired-wire-revision")?,
        ManifestGeneration::new(7),
        "a".repeat(64),
        "retired-wire-snapshot",
        3,
        "b".repeat(64),
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
    .with_node(RepoMapNode::File(RepoMapFileNode {
        file_id: FileId::new("file://src/lib.rs"),
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        line_count: 17,
    }));
    require_legacy_opcode::<SearchPlaneQueryIpcRequestEnvelope>(
        "RepoMapQuery",
        serde_json::json!({
            "repo_id": bundle.repo_id,
            "revision_id": bundle.revision_id,
            "manifest_generation": bundle.manifest_generation,
            "query_text": "old-query",
            "top_k": 5,
            "token_budget": 1024,
            "focus_subjects": [],
        }),
    )?;
    require_legacy_opcode::<SearchPlaneControlIpcRequestEnvelope>(
        "RepoMapActiveHeadV2",
        RepoMapActiveHeadRequestV2 {
            repo_id: bundle.repo_id.clone(),
            revision_id: bundle.revision_id.clone(),
        },
    )?;
    require_legacy_opcode::<SearchPlaneControlIpcRequestEnvelope>(
        "RepoMapActivateV2",
        RepoMapActivateGenerationRequestV2::for_bundle(&bundle)?,
    )?;
    let publish = RepoMapPublishBundleRequestV2::new(bundle)?;
    let payload = serde_json::to_value(&publish)?;
    require_equal(
        &serde_json::from_value::<RepoMapPublishBundleRequestV2>(payload)?,
        &publish,
        "old ingest payload round trip",
    )?;
    let old_ingest = OldLegacyIngestEnvelope(OldLegacyIngest(&publish));
    require_unknown_opcode::<SearchPlaneIngestIpcRequestEnvelope>(
        &quanta_index_ipc::encode_request(&old_ingest)?,
        "PublishRepoMapBundleV2",
        "unknown variant",
    )?;
    require_legacy_startup_and_cli_refusal()
}

fn require_legacy_opcode<T: serde::de::DeserializeOwned>(
    tag: &str,
    payload: impl serde::Serialize,
) -> TestResult {
    let envelope = serde_json::json!({
        "request_id": 12,
        "payload": {"kind": tag, "payload": payload},
    });
    require_unknown_opcode::<T>(
        &quanta_index_ipc::encode_request(&envelope)?,
        tag,
        "unknown variant",
    )
}

fn require_legacy_startup_and_cli_refusal() -> TestResult {
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let root = temp.path().join("legacy-state");
    let stderr_file = tempfile::NamedTempFile::new_in(temp.path())?;
    let mut daemon = Daemon(
        Command::new(product_binary()?)
            .arg("serve")
            .arg("--state-root")
            .arg(&root)
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr_file.reopen()?))
            .spawn()?,
        None,
    );
    let deadline = Instant::now() + NATIVE_STARTUP_LIMIT;
    let status = loop {
        if let Some(status) = daemon.0.try_wait()? {
            break status;
        }
        require(
            Instant::now() < deadline,
            "retired Legacy serve did not exit",
        )?;
        thread::sleep(Duration::from_millis(10));
    };
    require_equal(&status.code(), &Some(70), "retired Legacy serve exit")?;
    let stderr = String::from_utf8(stderr_prefix(stderr_file.path())?)?;
    require(
        stderr.contains("legacy serve unavailable in Native-only build"),
        format!("retired Legacy serve failed at a different boundary: {stderr}"),
    )?;
    require(!root.try_exists()?, "retired Legacy serve created state")?;

    let obsolete = temp.path().join("old-repomap.json");
    fs::write(&obsolete, b"retired RepoMap sentinel")?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = quanta_index_searchctl::run(
        [
            "legacy",
            "quarantine",
            "discard",
            "--repomap-file",
            obsolete.to_str().ok_or("fixture path is not UTF-8")?,
            "--reason",
            "retired store sentinel",
        ],
        &mut stdout,
        &mut stderr,
    );
    let stderr = String::from_utf8(stderr)?;
    require_equal(
        &exit,
        &std::process::ExitCode::from(2),
        "retired CLI flag exit",
    )?;
    require(
        stdout.is_empty() && stderr.contains("unknown quarantine discard flag `--repomap-file`"),
        format!("retired CLI flag failed at a different boundary: {stderr}"),
    )?;
    require_equal(
        &fs::read(obsolete)?.as_slice(),
        &b"retired RepoMap sentinel".as_slice(),
        "retired CLI path",
    )
}

#[test]
#[ignore = "requires the disposable, signed retirement installation and its live Q peer"]
fn signed_product_peer_refuses_retired_projection_and_instance_before_admission() -> TestResult {
    use quanta_index_contract::{
        index::{MapRoot, OwnerMutationSet, RetiredProjectionIdentityV1},
        native_protocol::{NativeFilePathEntry, NativeIndexResponseEnvelope},
    };
    use quanta_index_ipc::{KernelPeerCredentials, PeerCredentialsSource as _};

    require(
        cfg!(target_os = "linux"),
        "signed retirement fixture requires Linux",
    )?;
    // Provided by the installed test runner after verifying the deployment
    // approval and starting the selected ELF through the normal launcher.
    let path = std::env::var_os("QUANTA_INDEX_RETIRED_REPOMAP_FIXTURE")
        .ok_or("signed retirement fixture locator is required")?;
    let fixture: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
    let socket: PathBuf = serde_json::from_value(fixture["socket"].clone())?;
    let root: PathBuf = serde_json::from_value(fixture["state_root"].clone())?;
    let view: ViewRef = serde_json::from_value(fixture["view"].clone())?;
    let nonce: Digest = serde_json::from_value(fixture["root_nonce"].clone())?;
    let identity = RetiredProjectionIdentityV1 {
        projection: serde_json::from_value(fixture["retired_projection"].clone())?,
        instance: serde_json::from_value(fixture["retired_instance"].clone())?,
    };
    let retained_graph: IndexInstanceId =
        serde_json::from_value(fixture["retained_graph_instance"].clone())?;
    let retained_file: IndexInstanceId =
        serde_json::from_value(fixture["retained_file_instance"].clone())?;
    let file: NativeFilePathEntry = serde_json::from_value(fixture["retained_file_entry"].clone())?;
    let resume: OperationKey = serde_json::from_value(fixture["retired_resume_operation"].clone())?;
    let pid: u32 = serde_json::from_value(fixture["pid"].clone())?;
    let binary = std::env::var_os("QUANTA_INDEX_SEARCHD_BIN")
        .map(PathBuf::from)
        .ok_or("signed retirement test requires the selected Q ELF")?;
    require(
        socket.is_absolute() && root.is_absolute() && binary.is_absolute(),
        "signed retirement fixture paths must be absolute",
    )?;
    let root_metadata = fs::metadata(&root)?;
    let binary_metadata = fs::metadata(binary)?;
    let request = |request_id, payload| -> TestResult<NativeIndexResponse> {
        let mut peer = UnixStream::connect(&socket)?;
        peer.set_read_timeout(Some(Duration::from_secs(5)))?;
        require_equal(
            &KernelPeerCredentials.peer_credentials(&peer)?.pid,
            &Some(pid),
            "selected Q peer PID",
        )?;
        let process_image = fs::metadata(format!("/proc/{pid}/exe"))?;
        require(
            process_image.dev() == binary_metadata.dev()
                && process_image.ino() == binary_metadata.ino(),
            "selected Q peer did not execute the supplied ELF",
        )?;
        peer.write_all(&quanta_index_ipc::encode_request(
            &NativeIndexRequestEnvelope {
                request_id,
                payload,
            },
        )?)?;
        let reply: NativeIndexResponseEnvelope = quanta_index_ipc::decode_response(&mut peer)?;
        require_equal(
            &reply.request_id,
            &request_id,
            "selected Q reply request ID",
        )?;
        Ok(reply.payload)
    };

    let selection = ViewSelection::Latest {
        workspace: view.workspace,
        epoch: view.epoch,
    };
    let NativeIndexResponse::RootScopedView(before) =
        request(1, NativeIndexRequest::ObserveRootView(selection))?
    else {
        return Err("selected Q peer did not observe the fixture root".into());
    };
    require(
        before.view.identity == view
            && before.root_nonce == nonce
            && before.root_device == root_metadata.dev()
            && before.root_inode == root_metadata.ino(),
        "selected peer did not hold the expected root and exact current view",
    )?;
    let require_policy_refusal = |reply: NativeIndexResponse| {
        require(
            matches!(&reply, NativeIndexResponse::Error(error)
                if error.code == NativeIndexErrorCode::InvalidInput
                    && error.message.contains("retired projection or instance identity")),
            format!("retired identity failed at a different boundary: {reply:?}"),
        )
    };
    let exact = ViewSelection::Exact(view);
    let retained = request(
        5,
        NativeIndexRequest::QueryGraph {
            selection: exact,
            instance: retained_graph,
            query: graph_query(),
        },
    )?;
    let NativeIndexResponse::Graph(page) = retained else {
        return Err(
            format!("selected product did not serve the retained Graph: {retained:?}").into(),
        );
    };
    require_graph_result(&page, view, retained_graph)?;
    require_policy_refusal(request(
        6,
        NativeIndexRequest::QueryGraph {
            selection: exact,
            instance: identity.instance,
            query: graph_query(),
        },
    )?)?;
    require_policy_refusal(request(
        15,
        NativeIndexRequest::ReadProjection {
            selection: exact,
            projection: identity.projection,
        },
    )?)?;
    let paths = request(
        16,
        NativeIndexRequest::QueryPaths {
            selection: exact,
            instance: retained_file,
            prefix: file.path.clone(),
            after: None,
            limit: 8,
        },
    )?;
    require(
        matches!(&paths, NativeIndexResponse::PathPage(page)
            if page.view == view && !page.has_more && page.entries.as_slice() == std::slice::from_ref(&file)),
        format!("selected product did not serve the retained FileMap path: {paths:?}"),
    )?;
    let paths = request(
        17,
        NativeIndexRequest::QueryRepositoryPaths {
            selection: exact,
            instance: retained_file,
            prefix: file.path.clone(),
            after: None,
            limit: 8,
        },
    )?;
    require(
        matches!(&paths, NativeIndexResponse::RepositoryPathPage(page)
            if page.view == view && !page.has_more
                && matches!(page.entries.as_slice(), [entry] if entry.repository.is_none() && entry.file == file)),
        format!("selected product did not serve the retained repository path: {paths:?}"),
    )?;
    require(
        resume.workspace == view.workspace && resume.epoch == view.epoch,
        "retired resume operation is outside the selected workspace epoch",
    )?;
    let prior_status = request(7, NativeIndexRequest::Status(resume))?;
    require(
        matches!(&prior_status, NativeIndexResponse::Status(status)
            if status.operation == resume
                && matches!(&status.payload, OperationStatus::Pending(_) | OperationStatus::Published(_))),
        format!("signed fixture requires pending retired work or a published retired view; a terminal refusal replays unchanged: {prior_status:?}"),
    )?;
    require_policy_refusal(request(8, NativeIndexRequest::Resume(resume))?)?;
    require_equal(
        &request(9, NativeIndexRequest::Status(resume))?,
        &prior_status,
        "retired resume status",
    )?;

    let stage = OperationKey {
        workspace: view.workspace,
        epoch: view.epoch,
        ..operation(94)
    };
    let NativeIndexResponse::InputOpened(ingress) =
        request(10, NativeIndexRequest::BeginInput(stage))?
    else {
        return Err("selected peer did not open the staging fixture".into());
    };
    require_equal(&ingress.operation, &stage, "staging fixture operation")?;
    let objects = root.join("native-objects/objects");
    let staged = (|| {
        let inventory = root_snapshot(&objects)?;
        let mut retired_spec = file_spec();
        retired_spec.instance = identity.instance;
        require_policy_refusal(request(
            11,
            NativeIndexRequest::StageProfile {
                ingress,
                profile: IndexProfile {
                    instances: vec![retired_spec],
                },
            },
        )?)?;
        require_policy_refusal(request(
            12,
            NativeIndexRequest::StageProjectionOwner {
                ingress,
                contribution: ProjectionOwnerContribution {
                    projection: identity.projection,
                    producer: GRAPH_PRODUCER,
                    owner: OwnerKey {
                        producer: GRAPH_PRODUCER,
                        stable_id: StableOwnerId([1; 16]),
                    },
                    source_owner: None,
                    source: None,
                    dependency_stamp: Digest([1; 32]),
                    output: None,
                },
            },
        )?)?;
        require_policy_refusal(request(
            13,
            NativeIndexRequest::StageProjectionCertificate {
                ingress,
                certificate: ProjectionCertificate {
                    projection: identity.projection,
                    producer: GRAPH_PRODUCER,
                    prior: None,
                    target_source: before.view.source,
                    schema: graph_spec().schema,
                    producer_policy: graph_spec().policy,
                    dependency_context: Digest([50; 32]),
                    affected: OwnerMutationSet {
                        root: MapRoot(None),
                        count: 0,
                        digest: Digest([51; 32]),
                    },
                },
            },
        )?)?;
        require(
            root_snapshot(&objects)? == inventory,
            "retired staging changed the immutable object inventory",
        )
    })();
    let discarded = request(14, NativeIndexRequest::DiscardInput(ingress));
    staged?;
    require_equal(
        &discarded?,
        &NativeIndexResponse::InputDiscarded(ingress),
        "staging fixture cleanup",
    )?;

    let projection = UpdateKind::ProjectionAdvance(Box::new(ProjectionTransition {
        projection: identity.projection,
        producer: GRAPH_PRODUCER,
        expected: None,
        target_source: before.view.source,
        schema: graph_spec().schema,
        producer_policy: graph_spec().policy,
        dependency_context: Digest([50; 32]),
        completeness: ProducerCompleteness::Partial {
            missing: MapRoot(None),
        },
        owners: OwnerMutationSet {
            root: MapRoot(None),
            count: 0,
            digest: Digest([51; 32]),
        },
        events: Vec::new(),
    }));
    let instance = UpdateKind::CompactSourceLocal {
        instance: identity.instance,
        expected_artifact: before.view.profile,
        owner: OwnerKey {
            producer: GRAPH_PRODUCER,
            stable_id: StableOwnerId([1; 16]),
        },
    };
    for (id, update) in [(2, projection), (3, instance)] {
        let key = OperationKey {
            workspace: view.workspace,
            epoch: view.epoch,
            ..operation(90 + id)
        };
        let assert_unknown = |request_id| -> TestResult {
            let reply = request(request_id, NativeIndexRequest::Status(key))?;
            require(
                matches!(&reply, NativeIndexResponse::Status(status)
                    if status.operation == key && status.payload == OperationStatus::Unknown),
                format!("retired identity operation is present: {reply:?}"),
            )
        };
        assert_unknown(id * 10)?;
        let reply = request(
            id,
            NativeIndexRequest::Submit(UpdateRequest {
                operation: key,
                expected_view: view,
                kind: update,
            }),
        )?;
        require_policy_refusal(reply)?;
        assert_unknown(id * 10 + 1)?;
    }
    let NativeIndexResponse::RootScopedView(after) =
        request(4, NativeIndexRequest::ObserveRootView(selection))?
    else {
        return Err("selected Q peer lost its root after refusal".into());
    };
    require_equal(
        &after,
        &before,
        "root and current view after retired identity refusals",
    )?;
    require(
        !root.join("repo-map").try_exists()?,
        "signed product created a dedicated RepoMap store",
    )?;
    let executable = read_view_daemon_witness::observe(pid)?;
    read_view_daemon_witness::write(
        [executable],
        "retired-repomap-signed-peer.json",
        &nextest_test_event(
            module_path!(),
            "signed_product_peer_refuses_retired_projection_and_instance_before_admission",
        ),
        None,
    )
}
