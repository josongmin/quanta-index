//! End-to-end certified `RepoMap` rank publication and query over Native UDS.

use super::*;
use quanta_index_contract::{
    CompiledRepoMapProjectionEntryV1, RepoMapDocType, RepoMapFocusSubjectDto, RepoRelativePath,
    SymbolId,
    native_protocol::{
        GraphProjectionRow, NativeRepoMapHit, NativeRepoMapQuery, NativeRepoMapResult,
        native_repomap_rank_policy,
    },
};
use quanta_index_core::index::{
    AttemptContext,
    ports::{IndexFuture, IndexProjectionBuildPort},
};
use quanta_index_repomap::native::graph::GraphCandidate;
use quanta_index_sdk::native_uds::NativeRepoMapAdvanceV1;
use std::sync::{Arc, Mutex};

struct ObservedGraph {
    inner: NativeGraph,
    first_error: FirstGraphFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct GraphFailure {
    stage: &'static str,
    error: IndexError,
}

type FirstGraphFailure = Arc<Mutex<Option<GraphFailure>>>;

impl ObservedGraph {
    fn capture(&self, stage: &'static str, error: &IndexError) {
        if let Ok(mut first) = self.first_error.lock()
            && first.is_none()
        {
            *first = Some(GraphFailure {
                stage,
                error: error.clone(),
            });
        }
    }
}

impl IndexProjectionBuildPort for ObservedGraph {
    type Candidate = GraphCandidate;

    fn resource_demand(&self) -> ResourceNeed {
        self.inner.resource_demand()
    }

    fn validate_instance(&self, instance: &InstanceSpec) -> Result<(), IndexError> {
        let result = self.inner.validate_instance(instance);
        if let Err(error) = &result {
            self.capture("validate_instance", error);
        }
        result
    }

    fn prepare_projection<'a>(
        &'a self,
        instance: &'a InstanceSpec,
        prior: Option<quanta_index_contract::index::MemberBinding>,
        projection: ProjectionBinding,
        affected: Option<quanta_index_contract::index::OwnerMutationSet>,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, Self::Candidate> {
        Box::pin(async move {
            let result = self
                .inner
                .prepare_projection(instance, prior, projection, affected, context)
                .await;
            if let Err(error) = &result {
                self.capture("prepare_projection", error);
            }
            result
        })
    }

    fn verify_projection<'a>(
        &'a self,
        candidate: Self::Candidate,
        context: &'a mut AttemptContext<'_>,
    ) -> IndexFuture<'a, quanta_index_contract::index::MemberBinding> {
        Box::pin(async move {
            let result = self.inner.verify_projection(candidate, context).await;
            if let Err(error) = &result {
                self.capture("verify_projection", error);
            }
            result
        })
    }
}

fn observed_service(
    root: &Path,
    socket: &Path,
    first_error: FirstGraphFailure,
) -> TestResult<NativeIndexService> {
    service_with_graph_builder(
        root,
        socket,
        false,
        ObservedGraph {
            inner: NativeGraph,
            first_error,
        },
    )
}

fn observed_error(first_error: &FirstGraphFailure) -> String {
    match first_error.lock() {
        Ok(first) => first.as_ref().map_or_else(
            || "no builder error captured".to_owned(),
            |failure| format!("{}: {}", failure.stage, failure.error),
        ),
        Err(_poisoned) => "builder observer lock poisoned".to_owned(),
    }
}

fn symbol(identity: &str) -> RepoMapNodeRef {
    RepoMapNodeRef::Symbol(SymbolId::new(identity))
}

fn rank_spec() -> InstanceSpec {
    InstanceSpec {
        policy: native_repomap_rank_policy(),
        ..spec()
    }
}

fn rank_configure(
    client: &NativeUdsClient,
    view: ViewRef,
    first_error: &FirstGraphFailure,
) -> TestResult<ViewRef> {
    let key = operation(2);
    let ingress = client.begin_input(key, &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![rank_spec()],
        },
        &budget(),
    )?;
    published(&client.submit(
        UpdateRequest {
            operation: key,
            expected_view: view,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)
    .map_err(|cause| {
        format!(
            "rank profile activation: {cause}; builder: {}",
            observed_error(first_error)
        )
        .into()
    })
}

fn rank_rows(changed: bool) -> Vec<CompiledRepoMapProjectionEntryV1> {
    vec![
        CompiledRepoMapProjectionEntryV1 {
            subject: node("src/other.rs"),
            doc_type: RepoMapDocType::File,
            owner_path: RepoRelativePath::new("src/other.rs"),
            search_text: String::new(),
            final_score_millis: 600,
        },
        CompiledRepoMapProjectionEntryV1 {
            subject: symbol("symbol-a"),
            doc_type: RepoMapDocType::Symbol,
            owner_path: RepoRelativePath::new("src/owner.rs"),
            search_text: "needle alpha".into(),
            final_score_millis: 800,
        },
        CompiledRepoMapProjectionEntryV1 {
            subject: symbol("symbol-b"),
            doc_type: RepoMapDocType::Symbol,
            owner_path: RepoRelativePath::new(if changed {
                "src/moved.rs"
            } else {
                "src/owner.rs"
            }),
            search_text: "needle beta".into(),
            final_score_millis: 700,
        },
    ]
}

fn stage_rank_owner(
    client: &NativeUdsClient,
    ingress: IngressRef,
    contribution_owner: OwnerKey,
    changed: bool,
    register_children: bool,
) -> TestResult<ObjectRef> {
    let mut projection = Vec::new();
    let mut children = Vec::new();
    let mut nodes = Vec::new();
    for (index, row) in rank_rows(changed).into_iter().enumerate() {
        let row_ref =
            client.stage_projection_output(ingress, &serde_json::to_vec(&row)?, &budget())?;
        let payload_ordinal = index
            .checked_add(1)
            .ok_or("rank fixture node ordinal overflow")?;
        nodes.push(GraphNode {
            identity: row.subject.clone(),
            payload_digest: Digest([u8::try_from(payload_ordinal)?; 32]),
        });
        projection.push(GraphProjectionRow {
            subject: row.subject,
            row: row_ref,
        });
        children.push(row_ref);
    }
    nodes.sort_by(|left, right| left.identity.cmp(&right.identity));
    projection.sort_by(|left, right| left.subject.cmp(&right.subject));
    children.sort_unstable();
    children.dedup();
    let output = GraphOwnerOutput {
        nodes,
        edges: Vec::new(),
        projection,
        facts: None,
    };
    let output_ref = client
        .stage_projection_output_with_children(
            ingress,
            &serde_json::to_vec(&output)?,
            if register_children { &children } else { &[] },
            &budget(),
        )
        .map_err(|cause| -> Box<dyn Error> { Box::new(cause) })?;
    Ok(client.stage_projection_owner(
        ingress,
        ProjectionOwnerContribution {
            projection: PROJECTION,
            producer: PRODUCER,
            owner: contribution_owner,
            source_owner: None,
            source: None,
            dependency_stamp: Digest([1; 32]),
            output: Some(output_ref),
        },
        &budget(),
    )?)
}

fn rank_advance(
    client: &NativeUdsClient,
    view: ViewRef,
    prior: Option<&ProjectionBinding>,
    operation_sequence: u64,
    event_sequence: u64,
    mutation: OwnerMutation,
    ingress: IngressRef,
) -> TestResult<SubmitOutcome> {
    if ingress.operation != operation(operation_sequence) {
        return Err("rank ingress differs from publication operation".into());
    }
    let source = client
        .read_view(ViewSelection::Exact(view), &budget())?
        .source;
    let request = client.prepare_ranked_repomap_advance_v1(
        ingress,
        NativeRepoMapAdvanceV1 {
            expected_view: view,
            target_source: source,
            projection: PROJECTION,
            schema: SCHEMA,
            prior: prior.copied(),
            dependency_context: Digest([50; 32]),
            event_predecessor: event_sequence
                .checked_sub(1)
                .ok_or("rank event sequence has no predecessor")?,
        },
        [Ok(mutation)],
        &budget(),
    )?;
    client
        .submit_borrowed(&request, &budget())
        .map_err(|cause| -> Box<dyn Error> { Box::new(cause) })
}

fn rank_query(
    client: &NativeUdsClient,
    view: ViewRef,
    focus: Vec<RepoMapFocusSubjectDto>,
) -> TestResult<NativeRepoMapResult> {
    client
        .query_repomap(
            ViewSelection::Exact(view),
            INSTANCE,
            NativeRepoMapQuery {
                query_text: "needle".into(),
                focus_subjects: focus,
                top_k: 8,
                token_budget: 256,
            },
            &budget(),
        )
        .map_err(|cause| -> Box<dyn Error> { Box::new(cause) })
}

fn focus(identity: &str) -> RepoMapFocusSubjectDto {
    RepoMapFocusSubjectDto {
        subject_identity: identity.into(),
        subject_doc_type: RepoMapDocType::Symbol,
    }
}

fn require_subjects(
    result: &NativeRepoMapResult,
    view: ViewRef,
    expected: &[RepoMapNodeRef],
) -> TestResult {
    let actual: Vec<_> = result.hits.iter().map(|hit| hit.subject.clone()).collect();
    if result.view != view
        || result.instance != INSTANCE
        || result.policy != native_repomap_rank_policy()
        || !result.exhaustive
        || actual != expected
    {
        return Err(format!("ranked view or subjects differ: {result:?}").into());
    }
    Ok(())
}

#[test]
fn compiled_bundle_publishes_ranked_graph_and_reopens() -> TestResult {
    use quanta_index_contract::{
        ChunkId, ManifestGeneration, RepoId, RepoMapChunkExactness, RepoMapChunkNode,
        RepoMapContainsEdge, RepoMapEdge, RepoMapExactnessSummary, RepoMapFileNode,
        RepoMapGraphCoverage, RepoMapGraphCoverageClass, RepoMapItemIndexAvailability, RepoMapNode,
        RepoMapOwnsChunkEdge, RepoMapRedactionState, RepoMapSourceBundle, RepoMapSymbolNode,
        RevisionId,
        lex::{LanguageCode, SymbolKindCode},
    };

    let path = RepoRelativePath::new("src/live_ingress.rs");
    let file_id = FileId::new("file://src/live_ingress.rs");
    let symbol_id = SymbolId::new("symbol://live-owner");
    let chunk_id = ChunkId::new("chunk://live-owner");
    let bundle = RepoMapSourceBundle::new(
        RepoId::new("repo")?,
        RevisionId::new("revision")?,
        ManifestGeneration::new(1),
        "c".repeat(64),
        "repomap-snapshot-live",
        1,
        "d".repeat(64),
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Available,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        },
        RepoMapExactnessSummary::Exact,
        RepoMapRedactionState::Unredacted,
    )
    .with_node(RepoMapNode::File(RepoMapFileNode {
        file_id: file_id.clone(),
        repo_relative_path: path.clone(),
        line_count: 40,
    }))
    .with_node(RepoMapNode::Symbol(RepoMapSymbolNode {
        symbol_id: symbol_id.clone(),
        owner_path: path.clone(),
        local_name: "LiveOwner".into(),
        qualified_name: "src::live_ingress::LiveOwner".into(),
        symbol_kind: SymbolKindCode::new("struct")?,
    }))
    .with_node(RepoMapNode::Chunk(RepoMapChunkNode {
        chunk_id: chunk_id.clone(),
        owner_path: path.clone(),
        language: LanguageCode::new("rust")?,
        start_byte: 0,
        end_byte: 64,
        start_line: 1,
        end_line: 8,
        token_count: 24,
        preview_text: "live owner repomap query anchor".into(),
        exactness: RepoMapChunkExactness::Exact,
    }))
    .with_edge(RepoMapEdge::Contains(RepoMapContainsEdge {
        container: RepoMapNodeRef::File(file_id),
        contained: RepoMapNodeRef::Symbol(symbol_id.clone()),
    }))
    .with_edge(RepoMapEdge::OwnsChunk(RepoMapOwnsChunkEdge {
        owner: RepoMapNodeRef::Symbol(symbol_id.clone()),
        chunk: RepoMapNodeRef::Chunk(chunk_id),
    }));
    let compiled = quanta_index_repomap::RepoMapGraphCompiler::with_default_budget()
        .compile_native_owner(&bundle)?;
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let first_error = Arc::new(Mutex::new(None));
    let service = RunningService::start(observed_service(
        root.path(),
        &socket,
        Arc::clone(&first_error),
    )?);
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(base) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("expected Native workspace view".into());
    };
    let key = operation(1);
    let ingress = client.begin_input(key, &budget())?;
    let mut leaves = Vec::new();
    for row in compiled.rank_rows() {
        leaves.push(GraphProjectionRow {
            subject: row.subject.clone(),
            row: client.stage_projection_output(ingress, &serde_json::to_vec(row)?, &budget())?,
        });
    }
    let (output, commitments) = compiled.bind_rank_leaves(&leaves)?;
    let mut children: Vec<_> = leaves.into_iter().map(|leaf| leaf.row).collect();
    children.sort_unstable();
    let graph = client.stage_projection_output_with_children(
        ingress,
        &serde_json::to_vec(&output)?,
        &children,
        &budget(),
    )?;
    let owner = owner(1);
    let contribution = client.stage_projection_owner(
        ingress,
        ProjectionOwnerContribution {
            projection: PROJECTION,
            producer: PRODUCER,
            owner,
            source_owner: None,
            source: None,
            dependency_stamp: Digest(commitments.producer_authority),
            output: Some(graph),
        },
        &budget(),
    )?;
    let initial = published(&rank_advance(
        &client,
        base,
        None,
        1,
        1,
        OwnerMutation {
            owner,
            expected: None,
            next: Some(contribution),
        },
        ingress,
    )?)?;
    let active = rank_configure(&client, initial, &first_error)?;
    let query = || NativeRepoMapQuery {
        query_text: "live owner".into(),
        focus_subjects: vec![focus("symbol://live-owner")],
        top_k: 1,
        token_budget: 64,
    };
    let before =
        client.query_repomap(ViewSelection::Exact(active), INSTANCE, query(), &budget())?;
    if before.hits
        != [NativeRepoMapHit {
            subject: RepoMapNodeRef::Symbol(symbol_id),
            owner_path: path,
            final_score_millis: 419,
            rank: 1,
        }]
        || before.dropped_entries_count != 1
        || before.drop_reason_codes != ["top_k_exhausted"]
        || !before.degraded_reason_codes.is_empty()
        || !before.exhaustive
    {
        return Err(format!("unexpected compiled bundle ranks: {before:?}").into());
    }
    service.stop()?;
    let reopened = RunningService::start(observed_service(root.path(), &socket, first_error)?);
    let client = NativeUdsClient::new(socket);
    let NativeIndexResponse::Status(status) =
        client.request(NativeIndexRequest::Status(key), &budget())?
    else {
        return Err("expected original RepoMap operation status after reopen".into());
    };
    if status.payload != OperationStatus::Published(initial)
        || published(&client.resume(key, &budget())?)? != initial
        || client
            .read_view(
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget(),
            )?
            .identity
            != active
        || client.query_repomap(ViewSelection::Exact(active), INSTANCE, query(), &budget())?
            != before
    {
        return Err("retained RepoMap operation or rank result changed after reopen".into());
    }
    reopened.stop()
}

#[test]
fn certified_rank_focus_replacement_old_view_and_reopen() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let first_error = Arc::new(Mutex::new(None));
    let running = RunningService::start(observed_service(
        root.path(),
        &socket,
        Arc::clone(&first_error),
    )?);
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("rank workspace creation did not return a view".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let first = stage_rank_owner(&client, ingress, owner(1), false, true)?;
    let first_view = published(&rank_advance(
        &client,
        genesis,
        None,
        1,
        1,
        OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(first),
        },
        ingress,
    )?)?;
    let first_view = rank_configure(&client, first_view, &first_error)?;

    require_subjects(
        &rank_query(&client, first_view, vec![focus("symbol-a")])?,
        first_view,
        &[symbol("symbol-a"), symbol("symbol-b")],
    )?;
    let all = rank_query(&client, first_view, Vec::new())?;
    if all.hits.len() != 3
        || !all
            .hits
            .iter()
            .any(|hit| hit.subject == node("src/other.rs"))
    {
        return Err("unfocused rank query omitted a certified candidate".into());
    }
    if !matches!(rank_query(&client, first_view, vec![focus("missing-symbol")]),
        Err(error) if matches!(error.downcast_ref::<NativeUdsError>(),
            Some(NativeUdsError::Refused(wire))
                if wire.code == NativeIndexErrorCode::FocusSubjectNotFound))
    {
        return Err("missing focus did not refuse with its typed code".into());
    }

    let binding =
        client.read_projection_binding(ViewSelection::Exact(first_view), PROJECTION, &budget())?;
    if binding.view != first_view {
        return Err("rank projection CAS selected another view".into());
    }
    let ingress = client.begin_input(operation(3), &budget())?;
    let replacement = stage_rank_owner(&client, ingress, owner(1), true, true)?;
    let second_view = published(&rank_advance(
        &client,
        first_view,
        Some(
            binding
                .binding
                .as_ref()
                .ok_or("rank projection binding absent")?,
        ),
        3,
        2,
        OwnerMutation {
            owner: owner(1),
            expected: Some(first),
            next: Some(replacement),
        },
        ingress,
    )?)?;
    require_subjects(
        &rank_query(&client, second_view, vec![focus("symbol-a")])?,
        second_view,
        &[symbol("symbol-a")],
    )?;
    require_subjects(
        &rank_query(&client, first_view, vec![focus("symbol-a")])?,
        first_view,
        &[symbol("symbol-a"), symbol("symbol-b")],
    )?;

    let binding =
        client.read_projection_binding(ViewSelection::Exact(second_view), PROJECTION, &budget())?;
    if binding.view != second_view {
        return Err("rank deletion CAS selected another view".into());
    }
    let ingress = client.begin_input(operation(4), &budget())?;
    let empty_view = published(&rank_advance(
        &client,
        second_view,
        Some(
            binding
                .binding
                .as_ref()
                .ok_or("rank deletion projection binding absent")?,
        ),
        4,
        3,
        OwnerMutation {
            owner: owner(1),
            expected: Some(replacement),
            next: None,
        },
        ingress,
    )?)?;
    require_subjects(
        &rank_query(&client, empty_view, Vec::new())?,
        empty_view,
        &[],
    )?;
    if !matches!(rank_query(&client, empty_view, vec![focus("symbol-a")]),
        Err(error) if matches!(error.downcast_ref::<NativeUdsError>(),
            Some(NativeUdsError::Refused(wire))
                if wire.code == NativeIndexErrorCode::FocusSubjectNotFound))
    {
        return Err("deleted rank focus did not refuse with its typed code".into());
    }

    running.stop()?;
    let reopened = RunningService::start(service(root.path(), &socket)?);
    let client = NativeUdsClient::new(socket);
    require_subjects(
        &rank_query(&client, first_view, vec![focus("symbol-a")])?,
        first_view,
        &[symbol("symbol-a"), symbol("symbol-b")],
    )?;
    require_subjects(
        &rank_query(&client, second_view, vec![focus("symbol-a")])?,
        second_view,
        &[symbol("symbol-a")],
    )?;
    require_subjects(
        &rank_query(&client, empty_view, Vec::new())?,
        empty_view,
        &[],
    )?;
    reopened.stop()?;
    Ok(())
}

#[test]
fn certified_rank_shared_support_retracts_reappears_and_reopens() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let first_error = Arc::new(Mutex::new(None));
    let running = RunningService::start(observed_service(
        root.path(),
        &socket,
        Arc::clone(&first_error),
    )?);
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("shared rank workspace creation did not return a view".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let first = stage_rank_owner(&client, ingress, owner(1), false, true)?;
    let initial = published(&rank_advance(
        &client,
        genesis,
        None,
        1,
        1,
        OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(first),
        },
        ingress,
    )?)?;
    let initial = rank_configure(&client, initial, &first_error)?;
    require_shared_rank_rows(&client, initial, true)?;

    let binding =
        client.read_projection_binding(ViewSelection::Exact(initial), PROJECTION, &budget())?;
    let ingress = client.begin_input(operation(3), &budget())?;
    let second = stage_rank_owner(&client, ingress, owner(2), false, true)?;
    let mut selected = published(&rank_advance(
        &client,
        initial,
        Some(
            binding
                .binding
                .as_ref()
                .ok_or("shared rank binding absent")?,
        ),
        3,
        2,
        OwnerMutation {
            owner: owner(2),
            expected: None,
            next: Some(second),
        },
        ingress,
    )?)?;
    require_shared_rank_rows(&client, selected, true)?;
    let mut history = vec![(initial, true), (selected, true)];

    // Both owners certify identical facts. Removing either duplicate must not
    // alter rank, scores or diagnostics; only the final removal erases rows.
    for (operation_sequence, event_sequence, contribution_owner, expected, recreate, present) in [
        (4, 3, owner(1), Some(first), false, true),
        (5, 4, owner(2), Some(second), false, false),
        (6, 5, owner(1), None, true, true),
    ] {
        let binding = client.read_projection_binding(
            ViewSelection::Exact(selected),
            PROJECTION,
            &budget(),
        )?;
        let ingress = client.begin_input(operation(operation_sequence), &budget())?;
        let next = if recreate {
            Some(stage_rank_owner(
                &client,
                ingress,
                contribution_owner,
                false,
                true,
            )?)
        } else {
            None
        };
        selected = published(&rank_advance(
            &client,
            selected,
            Some(
                binding
                    .binding
                    .as_ref()
                    .ok_or("shared rank binding absent")?,
            ),
            operation_sequence,
            event_sequence,
            OwnerMutation {
                owner: contribution_owner,
                expected,
                next,
            },
            ingress,
        )?)?;
        require_shared_rank_rows(&client, selected, present)?;
        // Query every predecessor after each mutation, including the empty
        // predecessor after recreation, before dropping the live service.
        for &(view, had_rows) in &history {
            require_shared_rank_rows(&client, view, had_rows)?;
        }
        history.push((selected, present));
    }
    running.stop()?;
    let reopened = RunningService::start(service(root.path(), &socket)?);
    let client = NativeUdsClient::new(socket);
    for (view, present) in history {
        require_shared_rank_rows(&client, view, present)?;
    }
    reopened.stop()?;
    Ok(())
}

fn require_shared_rank_rows(client: &NativeUdsClient, view: ViewRef, present: bool) -> TestResult {
    // Fixed contract gold, independent of the producer row encoder and ranker.
    let hits = if present {
        vec![
            NativeRepoMapHit {
                subject: symbol("symbol-a"),
                owner_path: RepoRelativePath::new("src/owner.rs"),
                final_score_millis: 800,
                rank: 1,
            },
            NativeRepoMapHit {
                subject: symbol("symbol-b"),
                owner_path: RepoRelativePath::new("src/owner.rs"),
                final_score_millis: 700,
                rank: 2,
            },
            NativeRepoMapHit {
                subject: node("src/other.rs"),
                owner_path: RepoRelativePath::new("src/other.rs"),
                final_score_millis: 600,
                rank: 3,
            },
        ]
    } else {
        Vec::new()
    };
    let expected = NativeRepoMapResult {
        view,
        instance: INSTANCE,
        policy: native_repomap_rank_policy(),
        hits,
        dropped_entries_count: 0,
        drop_reason_codes: Vec::new(),
        degraded_reason_codes: if present {
            Vec::new()
        } else {
            vec!["query_terms_unmatched".into()]
        },
        exhaustive: true,
    };
    let actual = rank_query(client, view, Vec::new())?;
    if actual != expected {
        return Err(
            format!("shared rank facts differ: expected {expected:?}, got {actual:?}").into(),
        );
    }
    Ok(())
}

#[test]
fn rank_output_missing_children_keeps_unknown_operation_and_prior_view() -> TestResult {
    let root = tempfile::tempdir()?;
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket = root.path().join("native.sock");
    let first_error = Arc::new(Mutex::new(None));
    let running = RunningService::start(observed_service(
        root.path(),
        &socket,
        Arc::clone(&first_error),
    )?);
    let client = NativeUdsClient::new(socket);
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("rank workspace creation did not return a view".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let first = stage_rank_owner(&client, ingress, owner(1), false, true)?;
    let first_view = published(&rank_advance(
        &client,
        genesis,
        None,
        1,
        1,
        OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(first),
        },
        ingress,
    )?)?;
    let first_view = rank_configure(&client, first_view, &first_error)?;
    let binding =
        client.read_projection_binding(ViewSelection::Exact(first_view), PROJECTION, &budget())?;
    if binding.view != first_view {
        return Err("rank projection CAS selected another view".into());
    }
    let ingress = client.begin_input(operation(3), &budget())?;
    let malformed = stage_rank_owner(&client, ingress, owner(1), true, false)?;
    let result = rank_advance(
        &client,
        first_view,
        Some(
            binding
                .binding
                .as_ref()
                .ok_or("rank projection binding absent")?,
        ),
        3,
        2,
        OwnerMutation {
            owner: owner(1),
            expected: Some(first),
            next: Some(malformed),
        },
        ingress,
    );
    if !matches!(&result,
        Ok(SubmitOutcome::Unknown(key)) if *key == operation(3))
    {
        return Err(format!(
            "malformed rank output did not keep its exact admitted operation unknown: {result:?}; builder: {}",
            observed_error(&first_error)
        )
        .into());
    }
    let observed = first_error
        .lock()
        .map_err(|_poisoned| "rank builder observer lock poisoned")?;
    if !matches!(observed.as_ref(),
        Some(GraphFailure {
            stage: "prepare_projection",
            error: IndexError::CorruptObject(cause),
        }) if cause == "graph output children mismatch")
    {
        return Err(
            format!("malformed rank output had another builder outcome: {observed:?}").into(),
        );
    }
    drop(observed);
    let status = client.status(operation(3), &budget())?;
    if !matches!(&status, OperationStatus::Pending(attempt) if attempt.operation == operation(3)) {
        return Err(
            format!("malformed rank operation lost its pending custody: {status:?}").into(),
        );
    }
    let latest = client.read_view(
        ViewSelection::Latest {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    if latest.identity != first_view {
        return Err("failed rank replacement advanced the published view".into());
    }
    require_subjects(
        &rank_query(&client, first_view, vec![focus("symbol-a")])?,
        first_view,
        &[symbol("symbol-a"), symbol("symbol-b")],
    )?;
    running.stop()?;
    Ok(())
}
