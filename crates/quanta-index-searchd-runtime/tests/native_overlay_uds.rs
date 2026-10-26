//! End-to-end source-CAS save handoff over the public native UDS client.

#[path = "native_overlay_uds/excluded_save.rs"]
mod excluded_save;
#[path = "common/native_physical_profile.rs"]
mod native_physical_profile;
#[path = "native_overlay_uds/overlay_search.rs"]
mod overlay_search;

use std::{
    error::Error,
    os::unix::fs::PermissionsExt,
    sync::Arc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use quanta_index_contract::{
    QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax,
    index::{
        CollectionId, Digest, GroupActivation, IndexInstanceId, IndexKind, IndexProfile,
        InputDomain, InstanceSpec, OperationKey, OperationStatus, OwnerKey, OwnerMutation,
        ProducerId, PublicationGroupId, SchemaId, SlotId, SourceEvent, SourceTextAdmission,
        SourceTransition, StableOwnerId, StorageFormatId, StreamId, SubmitOutcome, TenantId,
        UpdateKind, UpdateRequest, ViewRef, ViewSelection, WorkspaceKey, WorktreeId,
    },
    native_protocol::{
        NativeIndexErrorCode, NativeIndexRequest, NativeIndexResponse, NativeOverlayDocument,
        NativeOverlayMask, NativeOverlayReply, NativeOverlayRequest, NativeOverlaySaveIntent,
        NativeOverlaySaveReceipt, NativeOverlaySession, NativeStubBinding, NativeStubByteEdit,
        NativeStubCoordinateUnit, NativeStubEdit, NativeStubItem, NativeStubPosition,
        NativeStubRange, NativeStubReply, NativeStubRequest, NativeTextCoverage,
    },
};
use quanta_index_core::{
    RequestBudgetV1,
    index::{WorkLimits, effort::ResourceNeed},
};
use quanta_index_kernel::{AdapterDescriptor, AdapterRegistry, QueryAdapterRegistry};
use quanta_index_lexical::native::{
    Bm25Policy, NativeTextIndex, NativeTextReader, PlannedTextRead, TextReadResult, TextRecipe,
};
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};
use quanta_index_searchd_runtime::native::{
    NativeIndexService, NativeInteractionRecorder, NativeResourcePlan, NativeServiceCapabilities,
    NativeServiceResources,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

// Independent immutable AST facts, published through the same projection path
// as a real parser. This fixture does not claim parser/language support.
const SAVED_SOURCE_TEXT: &str = "typed 🦀\r\nsource";

fn fixture_grammar_policy() -> quanta_index_searchd_runtime::native::StubGrammarPolicy {
    quanta_index_searchd_runtime::native::StubGrammarPolicy {
        language: "fixture".into(),
        grammar_engine: "fixed-independent-fixture".into(),
        grammar_version: "1".into(),
        grammar_hash: None,
        grammar_policy: Digest([80; 32]),
    }
}

fn stub_items() -> Vec<NativeStubItem> {
    // Reverse display order to verify certification uses stable IDs.
    vec![
        NativeStubItem {
            stable_id: Digest([2; 32]),
            ast_digest: Digest([12; 32]),
            start: 12,
            end: 18,
            label: "second".into(),
        },
        NativeStubItem {
            stable_id: Digest([1; 32]),
            ast_digest: Digest([11; 32]),
            start: 0,
            end: 10,
            label: "first".into(),
        },
    ]
}

fn budget() -> RequestBudgetV1 {
    RequestBudgetV1::for_duration(Duration::from_secs(30))
}

fn workspace() -> WorkspaceKey {
    WorkspaceKey {
        tenant: TenantId([1; 16]),
        collection: CollectionId([2; 16]),
        worktree: WorktreeId([3; 16]),
        slot: SlotId([4; 16]),
    }
}

fn operation(sequence: u64) -> OperationKey {
    OperationKey {
        workspace: workspace(),
        epoch: 1,
        producer: ProducerId([5; 16]),
        stream: StreamId([6; 16]),
        stream_epoch: 1,
        sequence,
    }
}

fn owner() -> OwnerKey {
    OwnerKey {
        producer: operation(1).producer,
        stable_id: StableOwnerId([7; 16]),
    }
}

fn native_text_spec() -> InstanceSpec {
    let recipe = TextRecipe::new(Digest([31; 32]));
    InstanceSpec {
        instance: IndexInstanceId([71; 16]),
        kind: IndexKind([72; 16]),
        group: PublicationGroupId([73; 16]),
        activation: GroupActivation::CoreRequired,
        input_domain: InputDomain::SourceLocal,
        schema: SchemaId([74; 16]),
        policy: recipe.output_policy(),
        format: StorageFormatId([75; 16]),
    }
}

fn text_request(text: &str) -> TextQueryRequest {
    TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: text.into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 4,
        cursor: None,
    }
}

fn resources() -> NativeServiceResources {
    let need = ResourceNeed {
        cpu_units: 1,
        io_bytes: 64 * 1024 * 1024,
        memory_bytes: 64 * 1024 * 1024,
        provider_units: 0,
    };
    NativeServiceResources {
        physical: native_physical_profile::profile(),
        scratch_reserved_bytes: 16 * 1024 * 1024 * 1024,
        read_ticket_lease: Duration::from_secs(30),
        execution: NativeResourcePlan {
            capacity: ResourceNeed {
                cpu_units: 4,
                io_bytes: 256 * 1024 * 1024,
                memory_bytes: 5 * 64 * 1024 * 1024,
                provider_units: 1,
            },
            control: need,
            interactive: need,
            core: need,
            enrichment: ResourceNeed {
                provider_units: 1,
                ..need
            },
        },
    }
}

fn service(
    root: &std::path::Path,
    socket: &std::path::Path,
    with_text_profile: bool,
) -> TestResult<NativeIndexService> {
    let recipe = TextRecipe::new(Digest([31; 32]));
    let spec = native_text_spec();
    let descriptor = AdapterDescriptor {
        kind: spec.kind,
        schema: spec.schema,
        format: spec.format,
    };
    let mut builders = AdapterRegistry::new();
    let mut text = QueryAdapterRegistry::<PlannedTextRead, TextReadResult>::new();
    if with_text_profile {
        builders.register(descriptor, NativeTextIndex::new(recipe), |delta| {
            Ok(delta.clone())
        })?;
        text.register(
            descriptor,
            resources().execution.interactive,
            NativeTextReader::new(
                recipe,
                Bm25Policy { k1: 1.2, b: 0.75 },
                resources().execution.interactive,
            )?,
        )?;
    }
    let service = NativeIndexService::bind_with_capabilities(
        root,
        socket,
        WorkLimits {
            reads: 1_000_000,
            writes: 1_000_000,
            bytes: 512 * 1024 * 1024,
            mutations: 1_000_000,
        },
        NativeServiceCapabilities {
            builders,
            literal: QueryAdapterRegistry::new(),
            path: QueryAdapterRegistry::new(),
            text,
            semantic: QueryAdapterRegistry::new(),
            pump: None,
            provider_attempt_pool: None,
        },
        resources(),
    )?;
    service.configure_interaction_activity(Arc::new(NativeInteractionRecorder::new()))?;
    service.configure_overlay(2, 4, 1024, 4096, native_text_spec().instance)?;
    if with_text_profile {
        service.configure_published_stub(vec![fixture_grammar_policy()])?;
    }
    Ok(service)
}

struct RunningService {
    shutdown: quanta_index_ipc::ShutdownHandle,
    thread: Option<JoinHandle<anyhow::Result<()>>>,
}

impl RunningService {
    fn start(service: NativeIndexService) -> Self {
        let shutdown = service.shutdown_handle();
        let thread = std::thread::spawn(move || service.run(Duration::from_millis(5)));
        Self {
            shutdown,
            thread: Some(thread),
        }
    }

    fn stop(mut self) -> TestResult {
        self.shutdown.trigger();
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|panic_payload| {
                let message = panic_payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic_payload.downcast_ref::<&'static str>().copied())
                    .unwrap_or("non-string panic payload");
                std::io::Error::other(format!("overlay service panicked: {message}"))
            })??;
        }
        Ok(())
    }
}

impl Drop for RunningService {
    fn drop(&mut self) {
        self.shutdown.trigger();
        if let Some(thread) = self.thread.take() {
            drop(thread.join());
        }
    }
}

fn require(condition: bool, message: &'static str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn expect_conflict(result: Result<NativeOverlayReply, NativeUdsError>) -> TestResult {
    require(
        matches!(result, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Conflict),
        "overlay request should be refused with Conflict",
    )
}

fn expect_snapshot_unavailable(result: Result<NativeOverlayReply, NativeUdsError>) -> TestResult {
    require(
        matches!(result, Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::SnapshotUnavailable),
        "overlay request should be refused with SnapshotUnavailable",
    )
}

fn expect_stub_conflict(result: Result<NativeStubReply, NativeUdsError>) -> TestResult {
    match result {
        Err(NativeUdsError::Refused(error)) if error.code == NativeIndexErrorCode::Conflict => {
            Ok(())
        }
        other => Err(format!(
            "stale Native Stub request should be refused with Conflict; got {other:?}"
        )
        .into()),
    }
}

fn published(
    client: &NativeUdsClient,
    key: OperationKey,
    first: &SubmitOutcome,
) -> TestResult<ViewRef> {
    if let SubmitOutcome::Terminal(OperationStatus::Published(view)) = first {
        return Ok(*view);
    }
    let until = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or("source advance fixture deadline overflow")?;
    while Instant::now() < until {
        match client.status(key, &budget())? {
            OperationStatus::Published(view) => return Ok(view),
            OperationStatus::Refused(reason) => {
                return Err(format!("source advance refused: {reason:?}").into());
            }
            OperationStatus::Pending(_) | OperationStatus::Unknown => {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    Err("source advance did not publish within fixture deadline".into())
}

fn publish_stub_certificate(
    client: &NativeUdsClient,
    view: ViewRef,
    source: quanta_index_contract::index::ObjectRef,
) -> TestResult<ViewRef> {
    use quanta_index_contract::{
        index::{
            MapRoot, ProducerCompleteness, ProjectionCertificate, ProjectionOwnerContribution,
            ProjectionTransition,
        },
        native_protocol::{
            NATIVE_STUB_AST_PRODUCER, NATIVE_STUB_AST_PROJECTION, NATIVE_STUB_AST_SCHEMA,
            NativeStubCertificateHeader, NativeStubCertificateItem,
            encode_native_stub_certificate_into, native_stub_certificate_encoded_len,
            native_stub_certificate_policy,
        },
    };
    let key = OperationKey {
        producer: NATIVE_STUB_AST_PRODUCER,
        ..operation(1)
    };
    let ingress = client.begin_input(key, &budget())?;
    let policy = fixture_grammar_policy();
    let projection_owner = OwnerKey {
        producer: NATIVE_STUB_AST_PRODUCER,
        stable_id: owner().stable_id,
    };
    let prior = client.lookup_projection_owners(
        ViewSelection::Exact(view),
        NATIVE_STUB_AST_PROJECTION,
        vec![projection_owner],
        &budget(),
    )?;
    require(
        prior.view == view
            && prior.binding.is_none()
            && prior
                .rows
                .as_slice()
                .first()
                .is_some_and(|row| row.contribution.is_none()),
        "initial projection lookup invented a prior contribution",
    )?;
    let header = NativeStubCertificateHeader {
        projection: NATIVE_STUB_AST_PROJECTION,
        producer: NATIVE_STUB_AST_PRODUCER,
        schema: NATIVE_STUB_AST_SCHEMA,
        producer_policy: native_stub_certificate_policy(),
        owner: projection_owner,
        source_owner: owner(),
        source,
        source_bytes: u32::try_from(SAVED_SOURCE_TEXT.len())?,
        language: &policy.language,
        grammar_engine: &policy.grammar_engine,
        grammar_version: &policy.grammar_version,
        grammar_hash: policy.grammar_hash,
        grammar_policy: policy.grammar_policy,
    };
    let mut facts = stub_items();
    facts.sort_unstable_by_key(|item| item.stable_id);
    let items = facts
        .iter()
        .map(|item| NativeStubCertificateItem {
            stable_id: item.stable_id,
            ast_digest: item.ast_digest,
            start: item.start,
            end: item.end,
            label: &item.label,
        })
        .collect::<Vec<_>>();
    let mut bytes = vec![0; native_stub_certificate_encoded_len(&header, &items)?];
    encode_native_stub_certificate_into(&header, &items, &mut bytes)?;
    let output =
        client.stage_projection_output_with_children(ingress, &bytes, &[source], &budget())?;
    let envelope = client.stage_projection_owner(
        ingress,
        ProjectionOwnerContribution {
            projection: header.projection,
            producer: header.producer,
            owner: projection_owner,
            source_owner: Some(owner()),
            source: Some(source),
            dependency_stamp: policy.grammar_policy,
            output: Some(output),
        },
        &budget(),
    )?;
    let affected = client.stage_mutations(
        ingress,
        [OwnerMutation {
            owner: projection_owner,
            expected: None,
            next: Some(envelope),
        }],
        &budget(),
    )?;
    let source_cut = client
        .read_view(ViewSelection::Exact(view), &budget())?
        .source;
    let mut transition = ProjectionTransition {
        projection: header.projection,
        producer: header.producer,
        expected: None,
        target_source: source_cut,
        schema: header.schema,
        producer_policy: header.producer_policy,
        dependency_context: policy.grammar_policy,
        completeness: ProducerCompleteness::Partial {
            missing: MapRoot(None),
        },
        owners: affected,
        events: Vec::new(),
    };
    let certificate = client.stage_projection_certificate(
        ingress,
        ProjectionCertificate {
            projection: header.projection,
            producer: header.producer,
            prior: None,
            target_source: source_cut,
            schema: header.schema,
            producer_policy: header.producer_policy,
            dependency_context: policy.grammar_policy,
            affected,
        },
        &budget(),
    )?;
    transition.completeness = ProducerCompleteness::CompleteAffectedDomain { certificate };
    transition.events.push(SourceEvent {
        producer: header.producer,
        stream: key.stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.payload_digest()?,
    });
    let published_view = published(
        client,
        key,
        &client.submit(
            UpdateRequest {
                operation: key,
                expected_view: view,
                kind: UpdateKind::ProjectionAdvance(Box::new(transition)),
            },
            &budget(),
        )?,
    )?;
    let selected = client.lookup_projection_owners(
        ViewSelection::Exact(published_view),
        NATIVE_STUB_AST_PROJECTION,
        vec![projection_owner],
        &budget(),
    )?;
    require(
        selected.view == published_view
            && selected.binding.is_some()
            && selected
                .rows
                .as_slice()
                .first()
                .is_some_and(|row| row.contribution == Some(envelope)),
        "published projection lookup did not return the exact contribution CAS stamp",
    )?;
    let historical = client.lookup_projection_owners(
        ViewSelection::Exact(view),
        NATIVE_STUB_AST_PROJECTION,
        vec![projection_owner],
        &budget(),
    )?;
    require(
        historical.binding.is_none()
            && historical
                .rows
                .as_slice()
                .first()
                .is_some_and(|row| row.contribution.is_none()),
        "projection lookup crossed its selected history view",
    )?;
    let source_namespace = client.lookup_projection_owners(
        ViewSelection::Exact(published_view),
        NATIVE_STUB_AST_PROJECTION,
        vec![owner()],
        &budget(),
    )?;
    require(
        source_namespace
            .rows
            .as_slice()
            .first()
            .is_some_and(|row| row.contribution.is_none()),
        "projection lookup returned the source record from another owner namespace",
    )?;
    Ok(published_view)
}

#[test]
fn overlay_save_requires_original_source_cas_and_live_publisher() -> TestResult {
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
                path: Some("src/a.rs".into()),
            },
            &budget(),
        )? == NativeOverlayReply::Updated {
            document,
            revision: 1,
        },
        "overlay update identity mismatch",
    )?;
    let NativeOverlayReply::Snapshot {
        overlay: Some(snapshot),
        ..
    } = client.overlay(NativeOverlayRequest::Snapshot { document }, &budget())?
    else {
        return Err("overlay snapshot missing".into());
    };
    require(
        snapshot.revision == 1
            && snapshot.text.as_deref() == Some(SAVED_SOURCE_TEXT)
            && snapshot.path.as_deref() == Some("src/a.rs"),
        "overlay snapshot text mismatch",
    )?;
    require(
        matches!(
            client.update_overlay_at_path(
                document,
                Some(1),
                2,
                Some("invalid path".into()),
                Some("../outside.rs".into()),
                &budget(),
            ),
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::InvalidInput
        ),
        "overlay accepted a non-canonical unsaved source path",
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
        "overlay mask metadata mismatch",
    )?;

    let key = operation(2);
    let ingress = client.begin_input(key, &budget())?;
    let source = client.stage_source(
        ingress,
        "src/a.rs".into(),
        SAVED_SOURCE_TEXT.as_bytes(),
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
    let request = UpdateRequest {
        operation: key,
        expected_view: configured,
        kind: UpdateKind::SourceAdvance(transition),
    };
    let intent = NativeOverlaySaveIntent {
        document,
        revision: 1,
        expected_source: None,
        request: request.clone(),
    };
    let mut forged = intent.clone();
    forged.expected_source = Some(source);
    expect_conflict(client.bind_overlay_save(forged, &budget()))?;
    let mut forged_document = intent.clone();
    forged_document.document.owner.stable_id = StableOwnerId([8; 16]);
    expect_conflict(client.bind_overlay_save(forged_document, &budget()))?;
    let mut forged_revision = intent.clone();
    forged_revision.revision = 2;
    expect_conflict(client.bind_overlay_save(forged_revision, &budget()))?;
    require(
        matches!(client.bind_overlay_save(intent.clone(), &budget()).map_err(|cause| format!("initial save bind: {cause:?}"))?, NativeOverlayReply::SaveBound { operation, revision: 1, .. } if operation == key),
        "source save intent was not bound",
    )?;
    let mut forged_request = intent.clone();
    forged_request.request.expected_view = genesis;
    expect_conflict(client.bind_overlay_save(forged_request, &budget()))?;
    let mut forged_operation = intent.clone();
    forged_operation.request.operation = operation(99);
    expect_conflict(client.bind_overlay_save(forged_operation, &budget()))?;
    for _retry in 0..2 {
        require(
            client.bind_overlay_save(intent.clone(), &budget())?
                == NativeOverlayReply::SaveBound {
                    document,
                    revision: 1,
                    operation: key,
                },
            "same original save intent did not survive an ACK-loss retry",
        )?;
    }
    let prepublish = NativeOverlaySaveReceipt {
        operation: key,
        document,
        revision: 1,
        source: Some(source),
        selected: configured,
    };
    expect_conflict(client.complete_overlay_save(&prepublish, &budget()))?;
    let selected = published(&client, key, &client.submit(request, &budget())?)?;
    require(
        client.bind_overlay_save(intent.clone(), &budget())?
            == NativeOverlayReply::SaveBound {
                document,
                revision: 1,
                operation: key,
            },
        "published original save intent did not survive an ACK-loss retry",
    )?;
    expect_conflict(client.complete_overlay_save(&prepublish, &budget()))?;
    let forged_op = NativeOverlaySaveReceipt {
        operation: operation(99),
        selected,
        ..prepublish
    };
    expect_conflict(client.complete_overlay_save(&forged_op, &budget()))?;
    let receipt = NativeOverlaySaveReceipt {
        selected,
        ..prepublish
    };
    let forged_source = NativeOverlaySaveReceipt {
        source: Some(quanta_index_contract::index::ObjectRef(Digest([99; 32]))),
        ..receipt
    };
    expect_conflict(client.complete_overlay_save(&forged_source, &budget()))?;
    let forged_document = NativeOverlaySaveReceipt {
        document: NativeOverlayDocument {
            owner: OwnerKey {
                stable_id: StableOwnerId([8; 16]),
                ..owner()
            },
            ..document
        },
        ..receipt
    };
    expect_conflict(client.complete_overlay_save(&forged_document, &budget()))?;
    let forged_revision = NativeOverlaySaveReceipt {
        revision: 2,
        ..receipt
    };
    expect_conflict(client.complete_overlay_save(&forged_revision, &budget()))?;
    let NativeOverlayReply::Snapshot {
        overlay: Some(still_present),
        ..
    } = client.overlay(NativeOverlayRequest::Snapshot { document }, &budget())?
    else {
        return Err("failed handoff retired overlay text".into());
    };
    require(
        still_present.text.as_deref() == Some(SAVED_SOURCE_TEXT),
        "failed handoff changed overlay text",
    )?;
    require(
        client
            .complete_overlay_save(&receipt, &budget())
            .map_err(|cause| format!("published source save handoff: {cause:?}"))?
            == NativeOverlayReply::Saved {
                document,
                revision: 1,
                selected,
            },
        "certified save handoff was not accepted",
    )?;
    require(
        client.complete_overlay_save(&receipt, &budget())?
            == NativeOverlayReply::Saved {
                document,
                revision: 1,
                selected,
            },
        "same original save receipt did not survive an ACK-loss retry",
    )?;
    for forged in [
        prepublish,
        forged_op,
        forged_source,
        forged_document,
        forged_revision,
    ] {
        expect_conflict(client.complete_overlay_save(&forged, &budget()))?;
    }
    // Source save and AST publication are distinct Catalog views. Missing
    // evidence refuses, then the later projection authenticates the same source.
    expect_stub_conflict(client.stub(
        NativeStubRequest::Certify {
            binding: NativeStubBinding {
                document,
                revision: 1,
                selected,
                source,
                language: "fixture".into(),
            },
            items: stub_items(),
        },
        SAVED_SOURCE_TEXT,
        &budget(),
    ))?;
    let selected = publish_stub_certificate(&client, selected, source)
        .map_err(|cause| format!("AST projection publish: {cause}"))?;
    require(
        selected.sequence > receipt.selected.sequence,
        "AST certificate did not publish after the immutable save receipt",
    )?;
    require(
        client.complete_overlay_save(&receipt, &budget())?
            == NativeOverlayReply::Saved {
                document,
                revision: 1,
                selected: receipt.selected,
            },
        "later AST view rewrote the original save receipt",
    )?;
    let indexed = client.search_text(
        ViewSelection::Exact(selected),
        native_text_spec().instance,
        text_request("typed"),
        &budget(),
    )?;
    require(
        indexed.view == selected
            && indexed.coverage == NativeTextCoverage::CoreRequired
            && indexed.exhaustive
            && indexed.matched == 1
            && indexed.hits.len() == 1
            && indexed
                .hits
                .first()
                .is_some_and(|hit| hit.owner == owner() && hit.source == source),
        "selected native text member did not contain saved source",
    )?;
    let stub_binding = NativeStubBinding {
        document,
        revision: 1,
        selected,
        source,
        language: "fixture".into(),
    };
    let mut certified_items = stub_items();
    certified_items.reverse();
    require(
        client
            .stub(
                NativeStubRequest::Certify {
                    binding: stub_binding.clone(),
                    items: stub_items(),
                },
                SAVED_SOURCE_TEXT,
                &budget(),
            )
            .map_err(|cause| format!("later AST view Stub certify: {cause:?}"))?
            == NativeStubReply::Certified {
                binding: stub_binding.clone(),
                items: certified_items,
                action_supported: true,
            },
        "Native Stub did not certify the fixed source in stable-ID order",
    )?;
    let crab_edit = NativeStubEdit {
        target_id: Digest([1; 32]),
        expected_ast: Digest([11; 32]),
        range: NativeStubRange {
            start: NativeStubPosition { line: 0, column: 6 },
            end: NativeStubPosition { line: 0, column: 8 },
        },
        replacement: "crab".into(),
    };
    require(
        client
            .stub(
                NativeStubRequest::Preflight {
                    binding: stub_binding.clone(),
                    items: stub_items(),
                    selected_id: Digest([1; 32]),
                    edits: vec![crab_edit.clone()],
                    unit: NativeStubCoordinateUnit::Utf16Units,
                },
                SAVED_SOURCE_TEXT,
                &budget(),
            )
            .map_err(|cause| format!("later AST view Stub preflight: {cause:?}"))?
            == NativeStubReply::Prepared {
                binding: stub_binding.clone(),
                selected_id: Digest([1; 32]),
                edits: vec![NativeStubByteEdit {
                    target_id: Digest([1; 32]),
                    start: 6,
                    end: 10,
                    replacement: b"crab".to_vec(),
                }],
            },
        "Native Stub UTF-16 preflight did not return the fixed byte edit",
    )?;
    let mut stale_ast = crab_edit;
    stale_ast.expected_ast = Digest([99; 32]);
    let mut stale_ast_items = stub_items();
    let changed_item = stale_ast_items
        .iter_mut()
        .find(|item| item.stable_id == Digest([1; 32]))
        .ok_or("fixed Stub item missing")?;
    changed_item.ast_digest = Digest([99; 32]);
    expect_stub_conflict(client.stub(
        NativeStubRequest::Preflight {
            binding: stub_binding.clone(),
            items: stale_ast_items,
            selected_id: Digest([1; 32]),
            edits: vec![stale_ast],
            unit: NativeStubCoordinateUnit::Utf16Units,
        },
        SAVED_SOURCE_TEXT,
        &budget(),
    ))?;
    for stale_binding in [
        NativeStubBinding {
            revision: 2,
            ..stub_binding.clone()
        },
        NativeStubBinding {
            source: quanta_index_contract::index::ObjectRef(Digest([99; 32])),
            ..stub_binding.clone()
        },
        NativeStubBinding {
            selected: ViewRef {
                sequence: selected
                    .sequence
                    .checked_add(1)
                    .ok_or("view sequence overflow")?,
                ..selected
            },
            ..stub_binding.clone()
        },
        NativeStubBinding {
            document: NativeOverlayDocument {
                session: NativeOverlaySession {
                    epoch: session
                        .epoch
                        .checked_add(1)
                        .ok_or("session epoch overflow")?,
                    ..session
                },
                ..document
            },
            ..stub_binding.clone()
        },
    ] {
        expect_stub_conflict(client.stub(
            NativeStubRequest::Certify {
                binding: stale_binding,
                items: stub_items(),
            },
            SAVED_SOURCE_TEXT,
            &budget(),
        ))?;
    }
    require(
        client.overlay(NativeOverlayRequest::Snapshot { document }, &budget())?
            == NativeOverlayReply::Snapshot {
                document,
                overlay: None,
            },
        "saved overlay remained visible",
    )?;
    require(
        client.overlay(NativeOverlayRequest::Masks { session }, &budget())?
            == NativeOverlayReply::Masks {
                session,
                masks: Vec::new(),
            },
        "saved mask remained visible",
    )?;
    expect_conflict(client.overlay(
        NativeOverlayRequest::Update {
            document,
            expected_revision: None,
            revision: 1,
            text: Some("stale replay".into()),
            path: None,
        },
        &budget(),
    ))?;
    require(
        client.overlay(NativeOverlayRequest::Snapshot { document }, &budget())?
            == NativeOverlayReply::Snapshot {
                document,
                overlay: None,
            },
        "rejected replay revived saved text",
    )?;
    require(
        client.overlay(
            NativeOverlayRequest::Update {
                document,
                expected_revision: Some(1),
                revision: 2,
                text: Some("next local revision".into()),
                path: None,
            },
            &budget(),
        )? == NativeOverlayReply::Updated {
            document,
            revision: 2,
        },
        "post-save monotonic revision was rejected",
    )?;
    expect_conflict(client.bind_overlay_save(intent.clone(), &budget()))?;
    expect_conflict(client.complete_overlay_save(&receipt, &budget()))?;
    require(
        client.overlay(NativeOverlayRequest::Masks { session }, &budget())?
            == NativeOverlayReply::Masks {
                session,
                masks: vec![NativeOverlayMask {
                    owner: owner(),
                    revision: 2,
                    deleted: false,
                }],
            },
        "post-save revision mask mismatch",
    )?;
    expect_stub_conflict(client.stub(
        NativeStubRequest::Certify {
            binding: stub_binding,
            items: stub_items(),
        },
        SAVED_SOURCE_TEXT,
        &budget(),
    ))?;
    require(
        client.overlay(NativeOverlayRequest::Close { session }, &budget())?
            == NativeOverlayReply::Closed(session),
        "overlay close identity mismatch",
    )?;
    expect_conflict(client.bind_overlay_save(intent, &budget()))?;
    expect_conflict(client.complete_overlay_save(&receipt, &budget()))?;
    expect_conflict(client.overlay(NativeOverlayRequest::Masks { session }, &budget()))?;
    expect_conflict(client.overlay(NativeOverlayRequest::Begin { session }, &budget()))?;
    let fenced_session = NativeOverlaySession {
        epoch: 2,
        ..session
    };
    let fenced_document = NativeOverlayDocument {
        session: fenced_session,
        ..document
    };
    require(
        client.overlay(
            NativeOverlayRequest::Begin {
                session: fenced_session,
            },
            &budget(),
        )? == NativeOverlayReply::Begun(fenced_session),
        "second overlay session did not begin",
    )?;
    require(
        client.overlay(
            NativeOverlayRequest::Update {
                document: fenced_document,
                expected_revision: None,
                revision: 1,
                text: Some("unsaved".into()),
                path: None,
            },
            &budget(),
        )? == NativeOverlayReply::Updated {
            document: fenced_document,
            revision: 1,
        },
        "second overlay update was not accepted",
    )?;
    require(
        client.overlay(
            NativeOverlayRequest::Masks {
                session: fenced_session,
            },
            &budget(),
        )? == NativeOverlayReply::Masks {
            session: fenced_session,
            masks: vec![NativeOverlayMask {
                owner: owner(),
                revision: 1,
                deleted: false,
            }],
        },
        "second session mask metadata mismatch",
    )?;
    require(
        client.overlay(
            NativeOverlayRequest::Update {
                document: fenced_document,
                expected_revision: Some(1),
                revision: 2,
                text: None,
                path: None,
            },
            &budget(),
        )? == NativeOverlayReply::Updated {
            document: fenced_document,
            revision: 2,
        },
        "overlay tombstone update was not accepted",
    )?;
    require(
        client.overlay(
            NativeOverlayRequest::Masks {
                session: fenced_session,
            },
            &budget(),
        )? == NativeOverlayReply::Masks {
            session: fenced_session,
            masks: vec![NativeOverlayMask {
                owner: owner(),
                revision: 2,
                deleted: true,
            }],
        },
        "overlay tombstone mask metadata mismatch",
    )?;
    let delete_key = operation(3);
    let delete_ingress = client.begin_input(delete_key, &budget())?;
    let delete_mutations = client.stage_mutations(
        delete_ingress,
        [OwnerMutation {
            owner: owner(),
            expected: Some(source),
            next: None,
        }],
        &budget(),
    )?;
    let mut delete_transition = SourceTransition {
        expected_source: client
            .read_view(ViewSelection::Exact(selected), &budget())?
            .source,
        owners: delete_mutations,
        events: Vec::new(),
    };
    delete_transition.events.push(SourceEvent {
        producer: delete_key.producer,
        stream: delete_key.stream,
        stream_epoch: delete_key.stream_epoch,
        predecessor: 1,
        sequence: 2,
        payload: delete_transition.owner_digest()?,
    });
    let delete_request = UpdateRequest {
        operation: delete_key,
        expected_view: selected,
        kind: UpdateKind::SourceAdvance(delete_transition),
    };
    let delete_intent = NativeOverlaySaveIntent {
        document: fenced_document,
        revision: 2,
        expected_source: Some(source),
        request: delete_request.clone(),
    };
    require(
        matches!(client.bind_overlay_save(delete_intent.clone(), &budget()).map_err(|cause| format!("deletion save bind: {cause:?}"))?, NativeOverlayReply::SaveBound { operation, revision: 2, .. } if operation == delete_key),
        "source deletion save intent was not bound",
    )?;
    require(
        client.bind_overlay_save(delete_intent.clone(), &budget())?
            == NativeOverlayReply::SaveBound {
                document: fenced_document,
                revision: 2,
                operation: delete_key,
            },
        "same original deletion intent did not survive an ACK-loss retry",
    )?;
    let deleted_view = published(
        &client,
        delete_key,
        &client.submit(delete_request, &budget())?,
    )?;
    require(
        client.bind_overlay_save(delete_intent.clone(), &budget())?
            == NativeOverlayReply::SaveBound {
                document: fenced_document,
                revision: 2,
                operation: delete_key,
            },
        "published original deletion intent did not survive an ACK-loss retry",
    )?;
    let delete_receipt = NativeOverlaySaveReceipt {
        operation: delete_key,
        document: fenced_document,
        revision: 2,
        source: None,
        selected: deleted_view,
    };
    require(
        client.complete_overlay_save(&delete_receipt, &budget())?
            == NativeOverlayReply::Saved {
                document: fenced_document,
                revision: 2,
                selected: deleted_view,
            },
        "certified source deletion handoff was not accepted",
    )?;
    require(
        client.complete_overlay_save(&delete_receipt, &budget())?
            == NativeOverlayReply::Saved {
                document: fenced_document,
                revision: 2,
                selected: deleted_view,
            },
        "same original deletion receipt did not survive an ACK-loss retry",
    )?;
    let deleted_search = client.search_text(
        ViewSelection::Exact(deleted_view),
        native_text_spec().instance,
        text_request("typed"),
        &budget(),
    )?;
    require(
        deleted_search.view == deleted_view
            && deleted_search.coverage == NativeTextCoverage::CoreRequired
            && deleted_search.exhaustive
            && deleted_search.matched == 0
            && deleted_search.hits.is_empty(),
        "selected native text member retained deleted source",
    )?;
    require(
        client.overlay(
            NativeOverlayRequest::Masks {
                session: fenced_session,
            },
            &budget(),
        )? == NativeOverlayReply::Masks {
            session: fenced_session,
            masks: Vec::new(),
        },
        "saved tombstone mask remained visible",
    )?;
    expect_conflict(client.overlay(
        NativeOverlayRequest::Update {
            document: fenced_document,
            expected_revision: None,
            revision: 1,
            text: Some("stale after deletion".into()),
            path: None,
        },
        &budget(),
    ))?;
    require(
        client.overlay(
            NativeOverlayRequest::Update {
                document: fenced_document,
                expected_revision: Some(2),
                revision: 3,
                text: Some("post deletion edit".into()),
                path: None,
            },
            &budget(),
        )? == NativeOverlayReply::Updated {
            document: fenced_document,
            revision: 3,
        },
        "post-deletion monotonic revision was rejected",
    )?;
    expect_conflict(client.bind_overlay_save(delete_intent.clone(), &budget()))?;
    expect_conflict(client.complete_overlay_save(&delete_receipt, &budget()))?;
    client.end_interaction_publisher(publisher, 1, &budget())?;
    expect_conflict(client.bind_overlay_save(delete_intent, &budget()))?;
    expect_conflict(client.complete_overlay_save(&delete_receipt, &budget()))?;
    expect_conflict(client.overlay(
        NativeOverlayRequest::Snapshot {
            document: fenced_document,
        },
        &budget(),
    ))?;
    expect_conflict(client.overlay(
        NativeOverlayRequest::Masks {
            session: fenced_session,
        },
        &budget(),
    ))?;
    running.stop()
}

#[test]
fn overlay_save_keeps_unsaved_text_when_selected_view_has_no_text_member() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let running = RunningService::start(service(root.path(), &socket, false)?);
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
                text: Some("visible only in overlay".into()),
                path: None,
            },
            &budget(),
        )? == NativeOverlayReply::Updated {
            document,
            revision: 1,
        },
        "overlay update identity mismatch",
    )?;
    let key = operation(1);
    let ingress = client.begin_input(key, &budget())?;
    let source = client.stage_source(
        ingress,
        "src/a.rs".into(),
        b"visible only in overlay",
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
            .read_view(ViewSelection::Exact(genesis), &budget())?
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
        expected_view: genesis,
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
        "source save intent was not bound",
    )?;
    let selected = published(&client, key, &client.submit(request, &budget())?)?;
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
        return Err("missing text member retired unsaved overlay".into());
    };
    require(
        snapshot.revision == 1 && snapshot.text.as_deref() == Some("visible only in overlay"),
        "missing text member changed unsaved overlay",
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
        "missing text member retired overlay mask",
    )?;
    running.stop()
}
