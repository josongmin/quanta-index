//! Product UDS acceptance for a sealed managed ANN region across restart.

use super::*;
use quanta_index_contract::native_protocol::{NativeSemanticResult, NativeSemanticTraversal};
use quanta_index_sdk::native_uds::MAX_NATIVE_STAGE_SOURCES_INPUTS;

const SEALED_OWNER_COUNT: u8 = 64;

fn check_sealed_result(
    result: &NativeSemanticResult,
    selected: quanta_index_contract::index::ViewRef,
    instance: IndexInstanceId,
) -> TestResult {
    require_equal(&result.view, &selected, "sealed ANN selected view")?;
    require_equal(&result.instance, &instance, "sealed ANN instance")?;
    require_equal(
        &result.coverage.status,
        &CoverageStatus::Complete,
        "sealed ANN coverage",
    )?;
    require(
        matches!(
            result.traversal,
            NativeSemanticTraversal::Approximate { .. }
        ),
        format!("64 covered owners did not select a sealed ANN region: {result:?}"),
    )?;
    require(!result.exhaustive, "sealed ANN claimed exact exhaustion")?;
    require_equal(&result.hits.len(), &1, "sealed ANN hit count")?;
    let hit = result.hits.first().ok_or("sealed ANN hit missing")?;
    require(
        (1_u8..=SEALED_OWNER_COUNT).any(|id| {
            hit.owner
                == OwnerKey {
                    producer: operation(1).producer,
                    stable_id: StableOwnerId([id; 16]),
                }
        }),
        format!("sealed ANN returned a foreign owner: {:?}", hit.owner),
    )?;
    require_equal(
        &hit.vector_id,
        &b"whole-file".to_vec(),
        "sealed ANN vector ID",
    )?;
    require_equal(&hit.score.get(), &1.0, "sealed ANN cosine")
}

#[test]
fn product_daemon_seals_managed_ann_region_and_reopens_selected_view() -> TestResult {
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = temp.path().join("state");
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-semantic.json");
    let stub = EmbeddingStub::start()?;
    let base_url = stub.base_url();
    let mut configuration = config_with_openai_semantic(&root, &socket)?;
    configure_semantic_endpoint(&mut configuration, &base_url)?;
    configure_owned_semantic(&mut configuration)?;
    fs::write(&config_path, serde_json::to_vec(&configuration)?)?;
    let semantic = openai_semantic_spec(&base_url)?;
    let trace_directory =
        std::env::var_os("QIDX_NATIVE_ANN_TRACE_DIR").map(std::path::PathBuf::from);
    let first_trace = trace_directory
        .as_ref()
        .map(|directory| directory.join("ann-regions-initial.qtrace"));
    let daemon = Daemon::start_with_api_key_and_trace(
        &config_path,
        &socket,
        Some("test-key"),
        first_trace.as_deref(),
    )?;
    let client = NativeUdsClient::new(socket.clone());
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("sealed ANN workspace creation failed".into());
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
    let mut sources = (1_u8..=SEALED_OWNER_COUNT).map(|id| SourceInput {
        path: format!("src/{id:02}.rs"),
        bytes: format!("needle document {id:02}").into_bytes(),
        text: SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
    });
    let mut records = Vec::new();
    loop {
        let batch: Vec<_> = sources
            .by_ref()
            .take(MAX_NATIVE_STAGE_SOURCES_INPUTS)
            .collect();
        if batch.is_empty() {
            break;
        }
        records.extend(client.stage_sources(ingress, batch, &budget())?);
    }
    require_equal(
        &records.len(),
        &usize::from(SEALED_OWNER_COUNT),
        "sealed ANN staged source count",
    )?;
    let prior = client.read_view(ViewSelection::Exact(configured), &budget())?;
    let owner_sources: Vec<_> = (1_u8..=SEALED_OWNER_COUNT)
        .zip(records.iter().copied())
        .map(|(id, source)| {
            (
                OwnerKey {
                    producer: key.producer,
                    stable_id: StableOwnerId([id; 16]),
                },
                source,
            )
        })
        .collect();
    let owners = client.stage_mutations(
        ingress,
        owner_sources.iter().map(|(owner, record)| OwnerMutation {
            owner: *owner,
            expected: None,
            next: Some(*record),
        }),
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
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
    let query = SemanticQueryRequest {
        query_text: "needle".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        lexical_scope: None,
        top_k: 1,
    };
    // The product pump handles one owner per admitted attempt. This is only
    // the test watchdog; no production request or execution budget is raised.
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(60))
        .ok_or("sealed ANN test deadline overflow")?;
    // ReadView acquires/releases durable pins. Use the same read-only exact
    // work tracker as the 4K rail while waiting, then pin the final selected
    // view and independently check its sequence, coverage and sealed query.
    let completed_sequence = source_view
        .sequence
        .checked_add(u64::from(SEALED_OWNER_COUNT))
        .ok_or("sealed ANN completion sequence overflow")?;
    let mut progress = super::ann_quality::work_status::CompletionProgress::new(
        &owner_sources,
        source_view,
        &semantic,
    )?;
    let mut last_sequence = None;
    let readiness = (|| -> TestResult<_> {
        loop {
            if Instant::now() < deadline
                && progress.poll_next(|intent| {
                    let NativeIndexResponse::WorkStatus(status) = client.request(
                        NativeIndexRequest::WorkStatus(intent),
                        &budget().capped_at(deadline),
                    )?
                    else {
                        return Err("sealed ANN work-status response mismatch".into());
                    };
                    Ok(status)
                })?
            {
                let latest = client.read_view(
                    ViewSelection::Latest {
                        workspace: workspace(),
                        epoch: 1,
                    },
                    &budget().capped_at(deadline),
                )?;
                last_sequence = Some(latest.identity.sequence);
                require(
                    latest.identity.sequence >= completed_sequence,
                    "completed ANN receipts precede the required publication sequence",
                )?;
                let result = client.search_semantic(
                    ViewSelection::Exact(latest.identity),
                    semantic.instance,
                    query.clone(),
                    &budget(),
                )?;
                if result.coverage.status == CoverageStatus::Complete {
                    break Ok((latest.identity, result));
                }
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "product pump did not complete 64 ANN owners: source sequence {}, \
                 target sequence {completed_sequence}, last sequence {last_sequence:?}, \
                 observed completions {}/{}",
                    source_view.sequence,
                    progress.completed(),
                    progress.total(),
                )
                .into());
            }
            thread::sleep(Duration::from_millis(50));
        }
    })();
    let (materialized, before) = match readiness {
        Ok(ready) => ready,
        Err(error) => {
            return Err(super::ann_quality::quality_failure(
                "sealed region readiness",
                error.as_ref(),
                daemon,
            ));
        }
    };
    check_sealed_result(&before, materialized, semantic.instance)?;

    daemon.stop()?;
    let reopen_trace = trace_directory
        .as_ref()
        .map(|directory| directory.join("ann-regions-reopen.qtrace"));
    let reopened = Daemon::start_with_api_key_and_trace(
        &config_path,
        &socket,
        Some("test-key"),
        reopen_trace.as_deref(),
    )?;
    let restored = NativeUdsClient::new(socket);
    let selected = restored.read_view(ViewSelection::Exact(materialized), &budget())?;
    require_equal(
        &selected.identity,
        &materialized,
        "reopened sealed ANN view",
    )?;
    let after = restored.search_semantic(
        ViewSelection::Exact(materialized),
        semantic.instance,
        query,
        &budget(),
    )?;
    check_sealed_result(&after, materialized, semantic.instance)?;
    reopened.stop()?;
    stub.finish()
}
