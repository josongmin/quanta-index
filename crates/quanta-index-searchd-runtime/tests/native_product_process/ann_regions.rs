//! Product UDS acceptance for a sealed managed ANN region across restart.

use super::*;
use quanta_index_contract::index::MapRoot;
use quanta_index_contract::native_protocol::{NativeSemanticResult, NativeSemanticTraversal};
use quanta_index_sdk::native_uds::MAX_NATIVE_STAGE_SOURCES_INPUTS;
use std::collections::BTreeSet;

const SEALED_OWNER_COUNT: u8 = 64;

fn check_sealed_result(
    result: &NativeSemanticResult,
    selected: quanta_index_contract::index::ViewRef,
    instance: IndexInstanceId,
    expected_source_owners: MapRoot,
    expected_owners: &BTreeSet<OwnerKey>,
    requested_top_k: usize,
    owner_limit: usize,
) -> TestResult {
    require_equal(&result.view, &selected, "sealed ANN selected view")?;
    require_equal(&result.instance, &instance, "sealed ANN instance")?;
    require_equal(
        &result.coverage.instance,
        &instance,
        "sealed ANN coverage instance",
    )?;
    require_equal(
        &result.coverage.status,
        &CoverageStatus::Complete,
        "sealed ANN coverage",
    )?;
    require_equal(
        &result.coverage.desired.root,
        &expected_source_owners,
        "sealed ANN selected source root",
    )?;
    require_equal(
        &result.coverage.covered,
        &Some(result.coverage.desired),
        "sealed ANN covered input binding",
    )?;
    require(
        result.coverage.searchable.is_some(),
        "sealed ANN searchable root absent",
    )?;
    require(
        matches!(
            result.traversal,
            NativeSemanticTraversal::Approximate { .. }
        ),
        format!("64 covered owners did not select a sealed ANN region: {result:?}"),
    )?;
    require(!result.exhaustive, "sealed ANN claimed exact exhaustion")?;
    require(
        requested_top_k == 1 || requested_top_k == usize::from(SEALED_OWNER_COUNT),
        "sealed ANN fixture requested an unexpected page size",
    )?;
    let expected_hits = requested_top_k.min(expected_owners.len()).min(owner_limit);
    require_equal(&result.hits.len(), &expected_hits, "sealed ANN hit count")?;
    if requested_top_k > owner_limit {
        require_equal(
            &result.traversal,
            &NativeSemanticTraversal::Approximate { capped: true },
            "sealed ANN owner limit traversal",
        )?;
        require(
            usize::try_from(result.scanned_owners)? <= owner_limit,
            "sealed ANN scanned beyond configured owner limit",
        )?;
    }
    let mut observed = BTreeSet::new();
    let mut previous_owner = None;
    for hit in &result.hits {
        require(
            expected_owners.contains(&hit.owner),
            format!("sealed ANN returned a foreign owner: {:?}", hit.owner),
        )?;
        require(
            observed.insert(hit.owner),
            "sealed ANN repeated a hit owner",
        )?;
        if let Some(previous) = previous_owner {
            require(previous < hit.owner, "sealed ANN hit owner order differs")?;
        }
        previous_owner = Some(hit.owner);
        require_equal(
            &hit.vector_id.as_slice(),
            &b"whole-file".as_slice(),
            "sealed ANN vector ID",
        )?;
        require_equal(&hit.score.get(), &1.0, "sealed ANN cosine")?;
    }
    Ok(())
}

#[test]
fn sealed_region_oracle_rejects_wrong_coverage_binding() -> TestResult {
    use quanta_index_contract::{
        FiniteScore,
        index::{CoverageBinding, InputBinding},
        native_protocol::NativeSemanticHit,
    };

    let selected = quanta_index_contract::index::ViewRef {
        workspace: workspace(),
        epoch: 1,
        sequence: 2,
        digest: Digest([1; 32]),
    };
    let instance = IndexInstanceId([3; 16]);
    let source_owners = MapRoot(Some(ObjectRef(Digest([4; 32]))));
    let desired = InputBinding {
        root: source_owners,
        schema: SchemaId([5; 16]),
        policy: Digest([6; 32]),
        context: Digest([7; 32]),
    };
    let owners: BTreeSet<_> = (1_u8..=SEALED_OWNER_COUNT)
        .map(|id| OwnerKey {
            producer: operation(1).producer,
            stable_id: StableOwnerId([id; 16]),
        })
        .collect();
    let owner_limit = 8_usize;
    let valid = NativeSemanticResult {
        view: selected,
        instance,
        coverage: CoverageBinding {
            instance,
            desired,
            covered: Some(desired),
            searchable: Some(ObjectRef(Digest([8; 32]))),
            status: CoverageStatus::Complete,
            owners: MapRoot(Some(ObjectRef(Digest([9; 32])))),
        },
        traversal: NativeSemanticTraversal::Approximate { capped: false },
        exhaustive: false,
        scanned_owners: u32::from(SEALED_OWNER_COUNT),
        skipped_stale: 0,
        hits: vec![NativeSemanticHit {
            owner: *owners.first().ok_or("sealed fixture owner missing")?,
            vector_id: b"whole-file".to_vec(),
            score: FiniteScore::new(1.0)?,
            chunk: None,
        }],
    };
    check_sealed_result(
        &valid,
        selected,
        instance,
        source_owners,
        &owners,
        1,
        owner_limit,
    )?;
    let mut accepted = Vec::new();
    for name in [
        "wrong source root",
        "missing covered",
        "missing searchable",
        "wrong coverage instance",
    ] {
        let mut malformed = valid.clone();
        match name {
            "wrong source root" => {
                malformed.coverage.desired.root = MapRoot(None);
                malformed.coverage.covered = Some(malformed.coverage.desired);
            }
            "missing covered" => malformed.coverage.covered = None,
            "missing searchable" => malformed.coverage.searchable = None,
            "wrong coverage instance" => malformed.coverage.instance = IndexInstanceId([10; 16]),
            _ => return Err("unknown sealed fixture case".into()),
        }
        if check_sealed_result(
            &malformed,
            selected,
            instance,
            source_owners,
            &owners,
            1,
            owner_limit,
        )
        .is_ok()
        {
            accepted.push(name);
        }
    }
    let first = valid
        .hits
        .first()
        .ok_or("sealed fixture hit missing")?
        .clone();
    let mut bounded = valid.clone();
    bounded.hits = owners
        .iter()
        .copied()
        .take(owner_limit)
        .map(|owner| NativeSemanticHit {
            owner,
            ..first.clone()
        })
        .collect();
    bounded.traversal = NativeSemanticTraversal::Approximate { capped: true };
    bounded.scanned_owners = u32::try_from(owner_limit)?;
    check_sealed_result(
        &bounded,
        selected,
        instance,
        source_owners,
        &owners,
        64,
        owner_limit,
    )?;
    for name in [
        "short page",
        "duplicate owner",
        "wrong order",
        "wrong tail score",
        "uncapped bounded page",
        "overscanned bounded page",
    ] {
        let mut malformed = bounded.clone();
        match name {
            "short page" => malformed.hits.truncate(1),
            "duplicate owner" => {
                *malformed
                    .hits
                    .last_mut()
                    .ok_or("sealed fixture tail missing")? = first.clone();
            }
            "wrong order" => malformed.hits.swap(0, 1),
            "wrong tail score" => {
                malformed
                    .hits
                    .last_mut()
                    .ok_or("sealed fixture tail missing")?
                    .score = FiniteScore::new(0.0)?;
            }
            "uncapped bounded page" => {
                malformed.traversal = NativeSemanticTraversal::Approximate { capped: false };
            }
            "overscanned bounded page" => {
                malformed.scanned_owners = u32::try_from(owner_limit + 1)?;
            }
            _ => return Err("unknown sealed full-page case".into()),
        }
        if check_sealed_result(
            &malformed,
            selected,
            instance,
            source_owners,
            &owners,
            64,
            owner_limit,
        )
        .is_ok()
        {
            accepted.push(name);
        }
    }
    require(
        accepted.is_empty(),
        format!("sealed ANN oracle accepted malformed response: {accepted:?}"),
    )
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
    let owner_limit = usize::try_from(
        configuration
            .pointer("/semantic/read/max_owners")
            .and_then(serde_json::Value::as_u64)
            .ok_or("sealed ANN fixture owner limit absent")?,
    )?;
    require_equal(
        &owner_limit,
        &8_usize,
        "sealed ANN fixed fixture owner limit",
    )?;
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
    let expected_owners: BTreeSet<_> = owner_sources.iter().map(|(owner, _)| *owner).collect();
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
                progress.verify_latest_publication(latest.identity)?;
                require(
                    latest.identity.sequence >= completed_sequence,
                    "completed ANN receipts precede the required publication sequence",
                )?;
                let result = client.search_semantic(
                    ViewSelection::Exact(latest.identity),
                    semantic.instance,
                    query.clone(),
                    &budget().capped_at(deadline),
                )?;
                if result.coverage.status == CoverageStatus::Complete {
                    break Ok((latest.identity, latest.source.owners, result));
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
    let (materialized, source_owners, before) = match readiness {
        Ok(ready) => ready,
        Err(error) => {
            return Err(super::ann_quality::quality_failure(
                "sealed region readiness",
                error.as_ref(),
                daemon,
            ));
        }
    };
    check_sealed_result(
        &before,
        materialized,
        semantic.instance,
        source_owners,
        &expected_owners,
        1,
        owner_limit,
    )?;
    let mut bounded_query = query.clone();
    bounded_query.top_k = u32::from(SEALED_OWNER_COUNT);
    let before_bounded = client.search_semantic(
        ViewSelection::Exact(materialized),
        semantic.instance,
        bounded_query.clone(),
        &budget(),
    )?;
    check_sealed_result(
        &before_bounded,
        materialized,
        semantic.instance,
        source_owners,
        &expected_owners,
        usize::from(SEALED_OWNER_COUNT),
        owner_limit,
    )?;

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
    require_equal(
        &selected.source.owners,
        &source_owners,
        "reopened sealed ANN source owner root",
    )?;
    let after = restored.search_semantic(
        ViewSelection::Exact(materialized),
        semantic.instance,
        query,
        &budget(),
    )?;
    check_sealed_result(
        &after,
        materialized,
        semantic.instance,
        selected.source.owners,
        &expected_owners,
        1,
        owner_limit,
    )?;
    require_equal(
        &after,
        &before,
        "reopened sealed ANN result changed for the same selected view and query",
    )?;
    let after_bounded = restored.search_semantic(
        ViewSelection::Exact(materialized),
        semantic.instance,
        bounded_query,
        &budget(),
    )?;
    check_sealed_result(
        &after_bounded,
        materialized,
        semantic.instance,
        selected.source.owners,
        &expected_owners,
        usize::from(SEALED_OWNER_COUNT),
        owner_limit,
    )?;
    require_equal(
        &after_bounded,
        &before_bounded,
        "reopened sealed ANN bounded page changed for the same selected view and query",
    )?;
    reopened.stop()?;
    stub.finish()
}
