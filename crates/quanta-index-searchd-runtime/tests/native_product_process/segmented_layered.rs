//! Real UDS publication, exact pinned reads, and restart for layered Text.

use super::*;
use quanta_index_contract::index::{IngressRef, ViewRef};

const REPOSITORY: &str = "layered-source";

fn layered_text_spec() -> InstanceSpec {
    let mut spec = text_spec();
    spec.policy = TextRecipe::segmented_layered(Digest([31; 32])).output_policy();
    spec.format = StorageFormatId([77; 16]);
    spec
}

fn owner(id: u8) -> OwnerKey {
    OwnerKey {
        producer: operation(2).producer,
        stable_id: StableOwnerId([id; 16]),
    }
}

fn dense_owner(id: u16) -> OwnerKey {
    let mut stable_id = [0; 16];
    stable_id[..2].copy_from_slice(&id.to_be_bytes());
    OwnerKey {
        producer: operation(2).producer,
        stable_id: StableOwnerId(stable_id),
    }
}

fn dense_path(id: u16) -> String {
    format!("densepath{id:04}.rs")
}

fn dense_body(id: u16) -> String {
    format!("denseword{id:04} fixture")
}

fn source(path: &str, bytes: &[u8]) -> SourceInput {
    SourceInput {
        path: path.into(),
        bytes: bytes.into(),
        text: SourceTextAdmission::Utf8 {
            policy: Digest([31; 32]),
        },
        language: None,
    }
}

fn stage_sources(
    client: &NativeUdsClient,
    ingress: IngressRef,
    inputs: Vec<SourceInput>,
) -> TestResult<Vec<ObjectRef>> {
    let NativeIndexResponse::StagedSources(staged) = client.request(
        NativeIndexRequest::StageRepositorySources {
            ingress,
            repository: quanta_index_contract::RepoId::try_from(REPOSITORY)?,
            inputs,
        },
        &budget(),
    )?
    else {
        return Err("layered source batch was not staged".into());
    };
    Ok(staged)
}

fn layered_config(root: &Path, socket: &Path) -> TestResult<serde_json::Value> {
    layered_text_config(config(root, socket))
}

fn layered_text_config(mut configuration: serde_json::Value) -> TestResult<serde_json::Value> {
    configuration["text"]["layout"] = serde_json::json!("segmented-layered-v2");
    set_fixture_json(
        &mut configuration,
        "/text/descriptor/format",
        serde_json::to_value(layered_text_spec().format)?,
    )?;
    Ok(configuration)
}

fn layered_session_config(root: &Path, socket: &Path) -> TestResult<serde_json::Value> {
    let mut configuration = layered_text_config(config_with_reclaim(root, socket)?)?;
    configuration["read_sessions"] = serde_json::json!({
        "global_limit": 1,
        "per_uid_limit": 1,
        "terminal_ttl_millis": 300_000_u64,
        "sweep_limit": 1
    });
    Ok(configuration)
}

fn configure_layered(client: &NativeUdsClient) -> TestResult<ViewRef> {
    let NativeIndexResponse::Created(genesis) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("layered workspace creation failed".into());
    };
    let ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        ingress,
        IndexProfile {
            instances: vec![literal_spec(), file_spec(), layered_text_spec()],
        },
        &budget(),
    )?;
    published(&client.submit(
        UpdateRequest {
            operation: operation(1),
            expected_view: genesis,
            kind: UpdateKind::Reconfigure { profile },
        },
        &budget(),
    )?)
}

fn advance(
    client: &NativeUdsClient,
    ingress: IngressRef,
    prior: ViewRef,
    mutations: impl IntoIterator<Item = OwnerMutation>,
) -> TestResult<ViewRef> {
    let request = prepare_advance(client, ingress, prior, mutations)?;
    submit_and_settle(client, &request)
}

fn prepare_advance(
    client: &NativeUdsClient,
    ingress: IngressRef,
    prior: ViewRef,
    mutations: impl IntoIterator<Item = OwnerMutation>,
) -> TestResult<UpdateRequest> {
    let operation = ingress.operation;
    let before = client.read_view(ViewSelection::Exact(prior), &budget())?;
    let owners = client.stage_mutations(ingress, mutations, &budget())?;
    let event_sequence = operation
        .sequence
        .checked_sub(1)
        .ok_or("source event underflow")?;
    let mut transition = SourceTransition {
        expected_source: before.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation.producer,
        stream: operation.stream,
        stream_epoch: operation.stream_epoch,
        predecessor: event_sequence.checked_sub(1).unwrap_or(0),
        sequence: event_sequence,
        payload: transition.owner_digest()?,
    });
    Ok(UpdateRequest {
        operation,
        expected_view: prior,
        kind: UpdateKind::SourceAdvance(transition),
    })
}

fn verify_paths(
    client: &NativeUdsClient,
    view: ViewRef,
    expected: &[(&str, OwnerKey, ObjectRef)],
) -> TestResult {
    let page = client.query_repository_paths(
        ViewSelection::Exact(view),
        file_spec().instance,
        String::new(),
        None,
        16,
        &budget(),
    )?;
    require_equal(&page.view, &view, "layered path view")?;
    require(!page.has_more, "layered path result truncated")?;
    require_equal(&page.entries.len(), &expected.len(), "layered path count")?;
    let repository = quanta_index_contract::RepoId::try_from(REPOSITORY)?;
    for (entry, (path, owner, source)) in page.entries.iter().zip(expected) {
        require_equal(
            &entry.repository.as_ref(),
            &Some(&repository),
            "layered path repository",
        )?;
        require_equal(&entry.file.path.as_str(), path, "layered path")?;
        require_equal(&entry.file.owner, owner, "layered path owner")?;
        require_equal(&entry.file.source, source, "layered path source")?;
    }
    Ok(())
}

fn verify_text(
    client: &NativeUdsClient,
    view: ViewRef,
    query: &str,
    expected: Option<(&str, OwnerKey, ObjectRef)>,
) -> TestResult {
    let selection = ViewSelection::Exact(view);
    let instance = layered_text_spec().instance;
    let request = TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: query.into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 8,
        cursor: None,
    };
    let result = if query.starts_with("select:path ") {
        let result = client.search_text_paths(selection, instance, request, &budget())?;
        let repository = quanta_index_contract::RepoId::try_from(REPOSITORY)?;
        require(
            result.ranked.repositories.len() == usize::from(expected.is_some())
                && result
                    .ranked
                    .repositories
                    .iter()
                    .all(|id| *id == repository),
            "layered path repository",
        )?;
        result.ranked.ranking
    } else {
        client.search_text(selection, instance, request, &budget())?
    };
    require_equal(&result.view, &view, "layered text view")?;
    require_equal(
        &result.instance,
        &layered_text_spec().instance,
        "layered text instance",
    )?;
    require(result.exhaustive, "layered text result was partial")?;
    require_equal(
        &result.matched,
        &(if expected.is_some() { 1 } else { 0 }),
        "layered text match count",
    )?;
    match expected {
        Some((path, owner, source)) => {
            let [hit] = result.hits.as_slice() else {
                return Err(format!("layered query {query:?} expected one hit").into());
            };
            require_equal(&hit.path.as_str(), &path, "layered text path")?;
            require_equal(&hit.owner, &owner, "layered text owner")?;
            require_equal(&hit.source, &source, "layered text source")
        }
        None => require(result.hits.is_empty(), "layered query returned stale hit"),
    }
}

fn layered_artifact(client: &NativeUdsClient, view: ViewRef, query: &str) -> TestResult<ObjectRef> {
    let result = query_layered_term(client, view, query)?;
    require_equal(&result.view, &view, "dense artifact view")?;
    require_equal(&result.matched, &1, "dense artifact match")?;
    Ok(result.artifact)
}

fn query_layered_term(
    client: &NativeUdsClient,
    view: ViewRef,
    query: &str,
) -> Result<NativeTextResult, NativeUdsError> {
    client.search_text(
        ViewSelection::Exact(view),
        layered_text_spec().instance,
        TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: query.into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        },
        &budget(),
    )
}

fn layered_root_parts(
    reader: &quanta_index_storage::FileObjectReader,
    artifact: ObjectRef,
    work: &mut WorkBudget,
) -> TestResult<Vec<ObjectRef>> {
    let object = reader.get(artifact, work)?;
    let payload = object.payload.as_slice();
    require(
        payload.starts_with(b"QTXROOT4"),
        "dense artifact is not a layered root",
    )?;
    let mut offset = 8 + 32;
    let mut prefix_children = 0;
    // Two optional TextCut refs (Source owners, paths), then the optional
    // path posting root. Source frontiers are not part of a layered root.
    for _ in 0..3 {
        let tag = *payload
            .get(offset)
            .ok_or("dense root optional tag absent")?;
        offset += 1;
        match tag {
            0 => {}
            1 => {
                offset += 32;
                prefix_children += 1;
            }
            _ => return Err("dense root optional tag invalid".into()),
        }
    }
    offset += 16; // Path document and indexed token counts.
    let count: [u8; 2] = payload
        .get(offset..offset + 2)
        .ok_or("dense root row count absent")?
        .try_into()?;
    let rows = usize::from(u16::from_be_bytes(count));
    require_equal(
        &object.registration.children.len(),
        &(prefix_children + rows),
        "dense root child count",
    )?;
    require_equal(
        &payload.len(),
        &(offset + 2 + rows * 77),
        "dense root row table length",
    )?;
    let partitions = object
        .registration
        .children
        .as_slice()
        .get(prefix_children..)
        .ok_or("dense root partition children absent")?;
    Ok(partitions.to_vec())
}

struct PostingFaultFixture {
    daemon: Daemon,
    client: NativeUdsClient,
    view: ViewRef,
    page: std::path::PathBuf,
    source: ObjectRef,
}

fn prepare_posting_fixture_append(
    client: &NativeUdsClient,
    view: ViewRef,
    id: u16,
) -> TestResult<(UpdateRequest, ObjectRef)> {
    let ingress = client.begin_input(operation(u64::from(id) + 2), &budget())?;
    let staged = stage_sources(
        client,
        ingress,
        vec![source(&dense_path(id), dense_body(id).as_bytes())],
    )?;
    let [reference] = staged.as_slice() else {
        return Err("posting fixture expected one staged Source".into());
    };
    let request = prepare_advance(
        client,
        ingress,
        view,
        [OwnerMutation {
            owner: dense_owner(id),
            expected: None,
            next: Some(*reference),
        }],
    )?;
    Ok((request, *reference))
}

fn posting_fault_fixture(temp: &Path, layers: u16) -> TestResult<PostingFaultFixture> {
    let root = temp.join("state");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let socket = temp.join("native.sock");
    let config_path = temp.join("native-config.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&layered_config(&root, &socket)?)?,
    )?;
    let daemon = Daemon::start(&config_path, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let mut view = configure_layered(&client)?;
    let last = layers
        .checked_sub(1)
        .ok_or("posting fixture needs a layer")?;
    for id in 0..last {
        let (request, _) = prepare_posting_fixture_append(&client, view, id)?;
        view = published(&client.submit(request, &budget())?)?;
    }
    let (request, source) = prepare_posting_fixture_append(&client, view, last)?;
    view = published(&client.submit(request, &budget())?)?;
    let term = format!("denseword{last:04}");
    verify_text(
        &client,
        view,
        &term,
        Some((&dense_path(last), dense_owner(last), source)),
    )?;
    let artifact = layered_artifact(&client, view, &term)?;
    daemon.stop()?;

    let page = {
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 1_000_000,
                writes: 0,
                bytes: 536_870_912,
                mutations: 0,
            },
            budget(),
        );
        let reader = super::large_restart::open_test_object_reader(&root, &mut work)?;
        let parts = layered_root_parts(&reader, artifact, &mut work)?;
        let [partition] = parts.as_slice() else {
            return Err("posting fixture expected one Text partition".into());
        };
        let segment = if layers == 1 {
            *partition
        } else {
            let manifest = reader.get(*partition, &mut work)?;
            require(
                manifest.payload.starts_with(b"QTXPART2"),
                "expected layered partition",
            )?;
            let children = manifest.registration.children.as_slice();
            require_equal(&children.len(), &usize::from(layers), "live layer count")?;
            *children
                .last()
                .ok_or("posting fixture needs a last layer")?
        };
        let segment = reader.get(segment, &mut work)?;
        require(
            segment.payload.starts_with(b"QTXSEGM1"),
            "expected selected Text segment",
        )?;
        let [page] = segment.registration.children.as_slice() else {
            return Err("posting fixture expected one posting page in the selected segment".into());
        };
        let object = reader.get(*page, &mut work)?;
        require(
            object.payload.starts_with(b"QTXPOST1")
                && object
                    .payload
                    .windows(term.len())
                    .any(|bytes| bytes == term.as_bytes()),
            "selected posting page must carry the fixed term",
        )?;
        native_object_file(&root, *page)?
    };

    let daemon = Daemon::start(&config_path, &socket)?;
    Ok(PostingFaultFixture {
        daemon,
        client,
        view,
        page,
        source,
    })
}

fn write_damaged_posting_page(path: &Path, original: &[u8]) -> TestResult {
    let mut damaged = original.to_vec();
    *damaged.last_mut().ok_or("posting page object is empty")? ^= 1;
    fs::write(path, damaged)?;
    Ok(())
}

fn require_corrupt_posting_read(
    result: Result<NativeTextResult, NativeUdsError>,
    context: &str,
) -> TestResult {
    require(
        matches!(&result, Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::CorruptObject),
        format!("{context}: expected CorruptObject, got {result:?}"),
    )
}

#[test]
fn selected_native_text_posting_page_faults_are_typed() -> TestResult {
    #[derive(Clone, Copy, Debug)]
    enum Fault {
        Modified,
        Missing,
        Symlink,
    }

    for fault in [Fault::Modified, Fault::Missing, Fault::Symlink] {
        let temp = quanta_index_searchd_harness::private_tempdir()?;
        let PostingFaultFixture {
            daemon,
            client,
            view,
            page,
            source,
        } = posting_fault_fixture(temp.path(), 1)?;
        let intact = temp.path().join("intact-posting-object");
        fs::rename(&page, &intact)?;
        match fault {
            Fault::Modified => {
                let original = fs::read(&intact)?;
                let copied = fs::copy(&intact, &page)?;
                require(
                    copied == u64::try_from(original.len())?,
                    "posting page fixture copy was incomplete",
                )?;
                write_damaged_posting_page(&page, &original)?;
            }
            Fault::Missing => {}
            Fault::Symlink => std::os::unix::fs::symlink(&intact, &page)?,
        }
        let rejected = query_layered_term(&client, view, "denseword0000");
        if !matches!(fault, Fault::Missing) {
            fs::remove_file(&page)?;
        }
        fs::rename(&intact, &page)?;
        require_corrupt_posting_read(rejected, &format!("selected posting page {fault:?}"))?;
        verify_text(
            &client,
            view,
            "denseword0000",
            Some((&dense_path(0), dense_owner(0), source)),
        )?;
        daemon.stop()?;
    }
    Ok(())
}

#[test]
fn layered_text_delta_refuses_a_corrupt_selected_base_posting_page() -> TestResult {
    // Appending to 15 one-owner layers forces the writer to replay the last
    // prior layer's postings when it merges the final two live layers.
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let PostingFaultFixture {
        daemon,
        client,
        view,
        page,
        source,
    } = posting_fault_fixture(temp.path(), 15)?;
    let (request, next_source) = prepare_posting_fixture_append(&client, view, 15)?;
    let key = request.operation;
    let latest = ViewSelection::Latest {
        workspace: workspace(),
        epoch: 1,
    };
    let before = client.read_view(latest, &budget())?;
    require_equal(&before.identity, &view, "base Text view before the append")?;

    let original = fs::read(&page)?;
    write_damaged_posting_page(&page, &original)?;
    let damaged_read = query_layered_term(&client, view, "denseword0014");
    let rejected = client.submit(request, &budget());
    let first_status = client.status(key, &budget());
    let retry_with_damage = client.resume(key, &budget());
    let retry_status = client.status(key, &budget());
    let head_with_damage = client.read_view(latest, &budget());
    fs::write(&page, original)?;

    require_corrupt_posting_read(damaged_read, "selected base posting page")?;
    // Native keeps a corrupt-object attempt recoverable under its original
    // key. Retrying that request while the same page is damaged also fails to publish.
    require_equal(
        &rejected?,
        &SubmitOutcome::Unknown(key),
        "corrupt Text base append",
    )?;
    require_equal(
        &retry_with_damage?,
        &SubmitOutcome::Unknown(key),
        "retry with corrupt Text base posting page",
    )?;
    let first_status = first_status?;
    let retry_status = retry_status?;
    require(
        matches!((&first_status, &retry_status),
            (OperationStatus::Pending(first), OperationStatus::Pending(retried))
                if first.operation == key && retried.operation == key && retried.fence > first.fence),
        format!(
            "expected the same pending key under a fresh fence, got {first_status:?} and {retry_status:?}"
        ),
    )?;
    let after_failure = head_with_damage?;
    require_equal(
        &after_failure.identity,
        &view,
        "head after failed Text append",
    )?;
    require_equal(
        &after_failure.source,
        &before.source,
        "Source cut after failed Text append",
    )?;
    let previous_path = dense_path(14);
    let previous = Some((previous_path.as_str(), dense_owner(14), source));
    verify_text(&client, view, "denseword0014", previous)?;
    verify_text(&client, view, "denseword0015", None)?;

    let recovered = published(&client.resume(key, &budget())?)?;
    require(
        recovered != view,
        "repaired append did not publish a new view",
    )?;
    require_equal(
        &client.status(key, &budget())?,
        &OperationStatus::Published(recovered),
        "recovered append status",
    )?;
    let after_repair = client.read_view(latest, &budget())?;
    require_equal(
        &after_repair.identity,
        &recovered,
        "head after repairing selected posting page",
    )?;
    verify_text(&client, recovered, "denseword0014", previous)?;
    verify_text(
        &client,
        recovered,
        "denseword0015",
        Some((&dense_path(15), dense_owner(15), next_source)),
    )?;
    daemon.stop()
}

#[test]
fn layered_text_full_patch_pinned_views_and_restart() -> TestResult {
    const OLD_PATH: &str = "oldroute/OLDROUTE.rs";
    const NEW_PATH: &str = "Renamed.rs";
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let root = temp.path().join("state");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-config.json");
    let configuration = layered_session_config(&root, &socket)?;
    fs::write(&config_path, serde_json::to_vec(&configuration)?)?;
    let daemon = Daemon::start(&config_path, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let configured = configure_layered(&client)?;
    verify_text(&client, configured, "emptyfixture", None)?;

    let ingress = client.begin_input(operation(2), &budget())?;
    let initial = stage_sources(
        &client,
        ingress,
        vec![
            source("a.rs", b"alpha shared phrase"),
            source("b.rs", b"beta deleted phrase"),
            source(OLD_PATH, b"gamma rename phrase"),
        ],
    )?;
    let [alpha, beta, gamma] = initial.as_slice() else {
        return Err("layered initial source count differs".into());
    };
    let old = advance(
        &client,
        ingress,
        configured,
        [
            OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(*alpha),
            },
            OwnerMutation {
                owner: owner(2),
                expected: None,
                next: Some(*beta),
            },
            OwnerMutation {
                owner: owner(3),
                expected: None,
                next: Some(*gamma),
            },
        ],
    )?;

    let ingress = client.begin_input(operation(3), &budget())?;
    let changed = stage_sources(
        &client,
        ingress,
        vec![
            source("a.rs", b"delta new phrase"),
            source(NEW_PATH, b"gamma rename phrase"),
        ],
    )?;
    let [delta, renamed] = changed.as_slice() else {
        return Err("layered changed source count differs".into());
    };
    let new = advance(
        &client,
        ingress,
        old,
        [
            OwnerMutation {
                owner: owner(1),
                expected: Some(*alpha),
                next: Some(*delta),
            },
            OwnerMutation {
                owner: owner(2),
                expected: Some(*beta),
                next: None,
            },
            OwnerMutation {
                owner: owner(3),
                expected: Some(*gamma),
                next: Some(*renamed),
            },
        ],
    )?;
    require(old != new, "layered Patch did not advance the view")?;
    let ingress = client.begin_input(operation(4), &budget())?;
    let after_noop = advance(
        &client,
        ingress,
        new,
        [OwnerMutation {
            owner: owner(1),
            expected: Some(*delta),
            next: Some(*delta),
        }],
    )?;
    let ingress = client.begin_input(operation(5), &budget())?;
    let after_delete = advance(
        &client,
        ingress,
        after_noop,
        [OwnerMutation {
            owner: owner(3),
            expected: Some(*renamed),
            next: None,
        }],
    )?;
    let deleted_artifact = layered_artifact(&client, after_delete, "delta")?;
    let after_repack = published(&client.submit(
        UpdateRequest {
            operation: operation(6),
            expected_view: after_delete,
            kind: UpdateKind::CompactSourceLocal {
                instance: layered_text_spec().instance,
                expected_artifact: deleted_artifact,
                owner: owner(1),
            },
        },
        &budget(),
    )?)?;
    let deletion_view = client.read_view(ViewSelection::Exact(after_delete), &budget())?;
    let repacked_view = client.read_view(ViewSelection::Exact(after_repack), &budget())?;
    require_equal(
        &deletion_view.source,
        &repacked_view.source,
        "deleted layer repack source cut",
    )?;
    require(
        layered_artifact(&client, after_repack, "delta")? != deleted_artifact,
        "deleted layer repack did not rewrite the sparse segment",
    )?;

    let verify = |reader: &NativeUdsClient| -> TestResult {
        let session = reader.open_read_session(ViewSelection::Exact(old), [0x64; 32], &budget())?;
        let term = TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "phrase".into(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 1,
            cursor: None,
        };
        let paged = (|| -> TestResult {
            let mut after = None;
            for (index, &(id, reference)) in
                [(1, *alpha), (2, *beta), (3, *gamma)].iter().enumerate()
            {
                let page = reader.search_text_page_in_read_session(
                    &session,
                    layered_text_spec().instance,
                    term.clone(),
                    after,
                    &budget(),
                )?;
                let [hit] = page.hits.as_slice() else {
                    return Err("layered common term lost its top-1 result".into());
                };
                require_equal(&page.matched, &3, "layered common term matched")?;
                require_equal(
                    &page.remaining,
                    &(3 - index as u64),
                    "layered common term remaining",
                )?;
                require_equal(&hit.owner, &owner(id), "layered common term tie order")?;
                require_equal(&hit.source, &reference, "layered common term pinned source")?;
                require(
                    (hit.score.get() - (0.5_f64 / 3.5).ln_1p()).abs() < 1e-14,
                    "layered common term BM25",
                )?;
                after = page.after;
                require(
                    after.is_some() == (index != 2),
                    "layered common term continuation",
                )?;
            }
            Ok(())
        })();
        let released = reader.release_read_session(&session, &budget());
        paged?;
        let (released, _) = released?;
        require(
            matches!(
                released.state,
                quanta_index_contract::native_protocol::NativeReadSessionState::Released
            ),
            "layered common term session did not release",
        )?;
        verify_paths(
            reader,
            old,
            &[
                ("a.rs", owner(1), *alpha),
                ("b.rs", owner(2), *beta),
                (OLD_PATH, owner(3), *gamma),
            ],
        )?;
        verify_paths(
            reader,
            new,
            &[(NEW_PATH, owner(3), *renamed), ("a.rs", owner(1), *delta)],
        )?;
        for (query, expected) in [
            ("alpha", Some(("a.rs", owner(1), *alpha))),
            ("\"alpha shared\"", Some(("a.rs", owner(1), *alpha))),
            ("NOT beta AND NOT gamma", Some(("a.rs", owner(1), *alpha))),
            ("beta", Some(("b.rs", owner(2), *beta))),
            ("gamma", Some((OLD_PATH, owner(3), *gamma))),
            ("\"gamma rename\"", Some((OLD_PATH, owner(3), *gamma))),
            (
                "select:path oldroute case:no",
                Some((OLD_PATH, owner(3), *gamma)),
            ),
            (
                "select:path OLDROUTE case:yes",
                Some((OLD_PATH, owner(3), *gamma)),
            ),
            ("select:path Oldroute case:yes", None),
            ("select:path renamed", None),
            ("delta", None),
        ] {
            verify_text(reader, old, query, expected)?;
        }
        for (query, expected) in [
            ("alpha", None),
            ("\"alpha shared\"", None),
            ("beta", None),
            ("select:path oldroute", None),
            ("delta", Some(("a.rs", owner(1), *delta))),
            ("\"delta new\"", Some(("a.rs", owner(1), *delta))),
            ("NOT gamma", Some(("a.rs", owner(1), *delta))),
            ("gamma", Some((NEW_PATH, owner(3), *renamed))),
            ("\"gamma rename\"", Some((NEW_PATH, owner(3), *renamed))),
            (
                "select:path renamed case:no",
                Some((NEW_PATH, owner(3), *renamed)),
            ),
            ("select:path renamed case:yes", None),
            (
                "select:path Renamed case:yes",
                Some((NEW_PATH, owner(3), *renamed)),
            ),
        ] {
            verify_text(reader, new, query, expected)?;
            verify_text(reader, after_noop, query, expected)?;
        }
        for view in [after_delete, after_repack] {
            verify_text(
                reader,
                view,
                "\"delta new\"",
                Some(("a.rs", owner(1), *delta)),
            )?;
            verify_text(reader, view, "gamma", None)?;
            verify_text(reader, view, "select:path renamed", None)?;
        }
        Ok(())
    };
    verify(&client)?;
    daemon.stop()?;
    let reopened = Daemon::start(&config_path, &socket)?;
    verify(&NativeUdsClient::new(socket))?;
    reopened.stop()
}

#[test]
fn layered_text_last_pin_releases_old_segment_and_posting_files() -> TestResult {
    use quanta_index_contract::native_protocol::NativeReadSettlementState;

    fn at<T, E: std::fmt::Display>(stage: &str, result: Result<T, E>) -> TestResult<T> {
        result.map_err(|error| format!("layered last-pin {stage}: {error}").into())
    }

    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let root = temp.path().join("state");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-config.json");
    let mut configuration = layered_session_config(&root, &socket)?;
    release_bounded_maintenance(&mut configuration)?;
    let options = configuration
        .as_object_mut()
        .ok_or("invalid layered retention config")?;
    let _previous = options.insert(
        "retention".into(),
        serde_json::json!({
            "scopes": [{ "workspace": workspace(), "epoch": 1 }],
            "sequence_window": 2,
            "page_limit": 1,
            "interval_ms": 20,
            "round_timeout_ms": 2_000,
            "head_probe_every": 2,
            "work_limits": {
                "reads": 1_000_000,
                "writes": 1_000_000,
                "bytes": 536_870_912,
                "mutations": 1_000_000
            }
        }),
    );
    fs::write(&config_path, serde_json::to_vec(&configuration)?)?;
    let daemon = at("start daemon", Daemon::start(&config_path, &socket))?;
    let client = NativeUdsClient::new(socket.clone());
    let configured = at("configure", configure_layered(&client))?;

    let ingress = at("begin first", client.begin_input(operation(2), &budget()))?;
    let sources = at(
        "stage first",
        stage_sources(&client, ingress, vec![source("first.rs", b"firstonly")]),
    )?;
    let [first] = sources.as_slice() else {
        return Err("first layered source count differs".into());
    };
    let first = *first;
    let first_view = at(
        "publish first",
        advance(
            &client,
            ingress,
            configured,
            [OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(first),
            }],
        ),
    )?;
    let ingress = at("begin second", client.begin_input(operation(3), &budget()))?;
    let sources = at(
        "stage second",
        stage_sources(&client, ingress, vec![source("second.rs", b"secondonly")]),
    )?;
    let [second] = sources.as_slice() else {
        return Err("second layered source count differs".into());
    };
    let second = *second;
    let old = at(
        "publish second",
        advance(
            &client,
            ingress,
            first_view,
            [OwnerMutation {
                owner: owner(2),
                expected: None,
                next: Some(second),
            }],
        ),
    )?;

    // Two separate advances produce one manifest with two immutable segments.
    // Track the exact old root, partition, segments and posting pages.
    let artifact = at(
        "read old artifact",
        layered_artifact(&client, old, "firstonly"),
    )?;
    at("stop daemon for offline object inspection", daemon.stop())?;
    let mut read_work = WorkBudget::with_request(
        WorkLimits {
            reads: 1_000_000,
            writes: 0,
            bytes: 536_870_912,
            mutations: 0,
        },
        budget(),
    );
    let reader = at(
        "open old objects",
        large_restart::open_test_object_reader(&root, &mut read_work),
    )?;
    let parts = at(
        "read old root",
        layered_root_parts(&reader, artifact, &mut read_work),
    )?;
    let [partition] = parts.as_slice() else {
        return Err("old layered root must have one partition".into());
    };
    let manifest = at("read old manifest", reader.get(*partition, &mut read_work))?;
    require(
        manifest.payload.starts_with(b"QTXPART2"),
        "old layered partition is not a manifest",
    )?;
    let [first_segment, second_segment] = manifest.registration.children.as_slice() else {
        return Err("old layered manifest must have two segments".into());
    };
    let first_object = at(
        "read first segment",
        reader.get(*first_segment, &mut read_work),
    )?;
    let second_object = at(
        "read second segment",
        reader.get(*second_segment, &mut read_work),
    )?;
    let [first_page] = first_object.registration.children.as_slice() else {
        return Err("first segment must have one posting page".into());
    };
    let [second_page] = second_object.registration.children.as_slice() else {
        return Err("second segment must have one posting page".into());
    };
    let tracked = [
        artifact,
        *partition,
        *first_segment,
        *second_segment,
        *first_page,
        *second_page,
    ];
    let files = tracked
        .into_iter()
        .map(|reference| native_object_file(&root, reference))
        .collect::<TestResult<Vec<_>>>()?;
    require(
        files.iter().all(|path| path.is_file()),
        "old layered object graph is missing before retirement",
    )?;
    drop(reader);

    let daemon = at("reopen daemon", Daemon::start(&config_path, &socket))?;
    let client = NativeUdsClient::new(socket.clone());

    let sdk = at(
        "connect SDK",
        quanta_index_sdk::QuantaIndex::connect_query_only(
            quanta_index_sdk::ConnectOptions::from_native_socket(
                &socket,
                ViewSelection::Exact(old),
            ),
        ),
    )?;
    let sdk_reader = at("select SDK reader", sdk.reader().native())?;
    let mut held = at("open old session", sdk_reader.open([93; 32], &budget()))?;
    let _profile = at("load old profile", held.load_profile(&budget()))?;
    let query = TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "firstonly".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 2,
        cursor: None,
    };
    let pinned_result = at(
        "read old session",
        held.text(&layered_text_spec(), query.clone(), &budget()),
    )?;
    require_equal(&pinned_result.matched, &1, "old pinned Text match")?;

    let ingress = at(
        "begin replacement",
        client.begin_input(operation(4), &budget()),
    )?;
    let sources = at(
        "stage replacement",
        stage_sources(&client, ingress, vec![source("current.rs", b"currentonly")]),
    )?;
    let [replacement] = sources.as_slice() else {
        return Err("replacement layered source count differs".into());
    };
    let replacement = *replacement;
    let successor = at(
        "publish replacement",
        advance(
            &client,
            ingress,
            old,
            [
                OwnerMutation {
                    owner: owner(1),
                    expected: Some(first),
                    next: None,
                },
                OwnerMutation {
                    owner: owner(2),
                    expected: Some(second),
                    next: None,
                },
                OwnerMutation {
                    owner: owner(3),
                    expected: None,
                    next: Some(replacement),
                },
            ],
        ),
    )?;
    let ingress = at("begin later", client.begin_input(operation(5), &budget()))?;
    let sources = at(
        "stage later",
        stage_sources(&client, ingress, vec![source("later.rs", b"lateronly")]),
    )?;
    let [later] = sources.as_slice() else {
        return Err("later layered source count differs".into());
    };
    let later = *later;
    let current = at(
        "publish later",
        advance(
            &client,
            ingress,
            successor,
            [OwnerMutation {
                owner: owner(4),
                expected: None,
                next: Some(later),
            }],
        ),
    )?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(12))
        .ok_or("layered retention deadline overflow")?;
    loop {
        match client.read_view(ViewSelection::Exact(old), &budget()) {
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable =>
            {
                break;
            }
            Ok(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(_) => return Err("layered retention did not advance past old view".into()),
            Err(error) => return Err(error.into()),
        }
    }
    require_equal(
        &at(
            "read held old after floor",
            held.text(&layered_text_spec(), query, &budget()),
        )?,
        &pinned_result,
        "held old Text after Exact admission retirement",
    )?;
    require(
        files.iter().all(|path| path.is_file()),
        "GC deleted a held layered root, partition, segment or posting page",
    )?;
    require(
        !at("release old session", held.release(&budget()))?,
        "old layered view retained another pin",
    )?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(12))
        .ok_or("layered settlement deadline overflow")?;
    loop {
        if at("settle old session", held.settlement(&budget()))?.state
            == NativeReadSettlementState::SettledAtSnapshot
        {
            break;
        }
        require(Instant::now() < deadline, "old layered view did not settle")?;
        thread::sleep(Duration::from_millis(20));
    }
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(20))
        .ok_or("layered reclaim deadline overflow")?;
    while files.iter().any(|path| path.exists()) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    for path in &files {
        require(
            !path.exists(),
            format!(
                "retired layered object was not reclaimed: {}",
                path.display()
            ),
        )?;
    }
    verify_text(
        &client,
        current,
        "currentonly",
        Some(("current.rs", owner(3), replacement)),
    )?;
    daemon.stop()?;
    let reopened = Daemon::start(&config_path, &socket)?;
    verify_text(
        &NativeUdsClient::new(socket),
        current,
        "lateronly",
        Some(("later.rs", owner(4), later)),
    )?;
    reopened.stop()
}

#[test]
fn layered_text_sixteen_live_layers_compact_without_losing_owners() -> TestResult {
    const OWNERS: usize = 16;
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let root = temp.path().join("state");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-config.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&layered_config(&root, &socket)?)?,
    )?;
    let daemon = Daemon::start(&config_path, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let configured = configure_layered(&client)?;

    let paths = (0..OWNERS)
        .map(|index| format!("owner{index:02}.rs"))
        .collect::<Vec<_>>();
    let originals = (0..OWNERS)
        .map(|index| format!("oldowner{index:02} baseline"))
        .collect::<Vec<_>>();
    let ingress = client.begin_input(operation(2), &budget())?;
    let initial = stage_sources(
        &client,
        ingress,
        paths
            .iter()
            .zip(&originals)
            .map(|(path, body)| source(path, body.as_bytes()))
            .collect(),
    )?;
    require_equal(&initial.len(), &OWNERS, "layered initial owner count")?;
    let full = advance(
        &client,
        ingress,
        configured,
        initial
            .iter()
            .enumerate()
            .map(|(index, reference)| {
                Ok(OwnerMutation {
                    owner: owner(u8::try_from(index + 1)?),
                    expected: None,
                    next: Some(*reference),
                })
            })
            .collect::<TestResult<Vec<_>>>()?,
    )?;

    let mut latest = initial.clone();
    let mut view = full;
    // One small FULL segment holds all 16 owners. Replacing a different
    // owner on each of 15 separate advances leaves one live base document
    // and 15 live delta layers in the same partition. The sixteenth layer
    // enters build_v2's deterministic local compaction trigger.
    for index in 0..OWNERS - 1 {
        let sequence = u64::try_from(index + 3)?;
        let body = format!("newowner{index:02} revised");
        let ingress = client.begin_input(operation(sequence), &budget())?;
        let staged = stage_sources(
            &client,
            ingress,
            vec![source(&paths[index], body.as_bytes())],
        )?;
        let [next] = staged.as_slice() else {
            return Err("layered replacement source count differs".into());
        };
        let prior = view;
        view = advance(
            &client,
            ingress,
            prior,
            [OwnerMutation {
                owner: owner(u8::try_from(index + 1)?),
                expected: Some(latest[index]),
                next: Some(*next),
            }],
        )?;
        require(
            view != prior,
            "layered replacement did not advance the view",
        )?;
        latest[index] = *next;
    }

    let expected_paths = paths
        .iter()
        .enumerate()
        .map(|(index, path)| {
            Ok((
                path.as_str(),
                owner(u8::try_from(index + 1)?),
                latest[index],
            ))
        })
        .collect::<TestResult<Vec<_>>>()?;
    let before_repack = client.read_view(ViewSelection::Exact(view), &budget())?;
    let prior_artifact = layered_artifact(&client, view, "newowner00")?;
    let repacked = published(&client.submit(
        UpdateRequest {
            operation: operation(18),
            expected_view: view,
            kind: UpdateKind::CompactSourceLocal {
                instance: layered_text_spec().instance,
                expected_artifact: prior_artifact,
                owner: owner(1),
            },
        },
        &budget(),
    )?)?;
    require(repacked != view, "layered repack did not advance the view")?;
    let after_repack = client.read_view(ViewSelection::Exact(repacked), &budget())?;
    require_equal(
        &after_repack.source,
        &before_repack.source,
        "repack source cut",
    )?;
    require_equal(
        &after_repack.profile,
        &before_repack.profile,
        "repack profile",
    )?;
    let repacked_artifact = layered_artifact(&client, repacked, "newowner00")?;
    require(
        repacked_artifact != prior_artifact,
        "layered repack reused its prior artifact",
    )?;
    let rank = |client: &NativeUdsClient, view| {
        client.search_text(
            ViewSelection::Exact(view),
            layered_text_spec().instance,
            TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "revised".into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: 16,
                cursor: None,
            },
            &budget(),
        )
    };
    let before = rank(&client, view)?;
    let after = rank(&client, repacked)?;
    require_equal(&before.matched, &15, "repack shared term match count")?;
    require_equal(
        &after.matched,
        &before.matched,
        "repack shared term match count",
    )?;
    require_equal(&after.hits, &before.hits, "repack score and rank parity")?;
    verify_paths(&client, view, &expected_paths)?;
    verify_paths(&client, repacked, &expected_paths)?;
    for index in 0..OWNERS {
        let id = u8::try_from(index + 1)?;
        let path = paths[index].as_str();
        let term = if index == OWNERS - 1 {
            format!("oldowner{index:02}")
        } else {
            format!("newowner{index:02}")
        };
        let phrase = if index == OWNERS - 1 {
            format!("\"oldowner{index:02} baseline\"")
        } else {
            format!("\"newowner{index:02} revised\"")
        };
        verify_text(&client, view, &term, Some((path, owner(id), latest[index])))?;
        verify_text(
            &client,
            repacked,
            &term,
            Some((path, owner(id), latest[index])),
        )?;
        verify_text(
            &client,
            view,
            &phrase,
            Some((path, owner(id), latest[index])),
        )?;
        verify_text(
            &client,
            repacked,
            &phrase,
            Some((path, owner(id), latest[index])),
        )?;
        if index < OWNERS - 1 {
            verify_text(&client, view, &format!("oldowner{index:02}"), None)?;
            verify_text(&client, repacked, &format!("oldowner{index:02}"), None)?;
        }
    }
    // Exact old roots must retain their original postings after the new
    // compacted view is published; this also catches a full-reset shortcut.
    verify_text(
        &client,
        full,
        "\"oldowner00 baseline\"",
        Some((paths[0].as_str(), owner(1), initial[0])),
    )?;
    verify_text(
        &client,
        full,
        "\"oldowner14 baseline\"",
        Some((paths[14].as_str(), owner(15), initial[14])),
    )?;
    let stale = client.submit(
        UpdateRequest {
            operation: operation(19),
            expected_view: repacked,
            kind: UpdateKind::CompactSourceLocal {
                instance: layered_text_spec().instance,
                expected_artifact: prior_artifact,
                owner: owner(1),
            },
        },
        &budget(),
    )?;
    require(
        matches!(
            &stale,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict))
        ),
        format!("stale repack artifact was accepted: {stale:?}"),
    )?;
    // The source frontier remains at event 16: compaction and its refused
    // stale replay consume operation IDs but do not advance source events.
    let ingress = client.begin_input(operation(20), &budget())?;
    let replacement = stage_sources(
        &client,
        ingress,
        vec![source(&paths[15], b"newowner15 revised")],
    )?;
    let [replacement] = replacement.as_slice() else {
        return Err("layered post-compaction replacement count differs".into());
    };
    let mut request = prepare_advance(
        &client,
        ingress,
        repacked,
        [
            OwnerMutation {
                owner: owner(1),
                expected: Some(latest[0]),
                next: None,
            },
            OwnerMutation {
                owner: owner(16),
                expected: Some(latest[15]),
                next: Some(*replacement),
            },
        ],
    )?;
    let UpdateKind::SourceAdvance(ref mut transition) = request.kind else {
        return Err("layered post-compaction request is not a source advance".into());
    };
    let event = transition
        .events
        .first_mut()
        .ok_or("missing source event")?;
    event.predecessor = 16;
    event.sequence = 17;
    let removed = published(&client.submit(request, &budget())?)?;

    let ingress = client.begin_input(operation(21), &budget())?;
    let revived = stage_sources(
        &client,
        ingress,
        vec![source(&paths[0], b"revivedowner00 returned")],
    )?;
    let [revived] = revived.as_slice() else {
        return Err("layered post-compaction re-add count differs".into());
    };
    let mut request = prepare_advance(
        &client,
        ingress,
        removed,
        [OwnerMutation {
            owner: owner(1),
            expected: None,
            next: Some(*revived),
        }],
    )?;
    let UpdateKind::SourceAdvance(ref mut transition) = request.kind else {
        return Err("layered re-add request is not a source advance".into());
    };
    let event = transition
        .events
        .first_mut()
        .ok_or("missing source event")?;
    event.predecessor = 17;
    event.sequence = 18;
    let revived_view = published(&client.submit(request, &budget())?)?;
    let mut final_paths = expected_paths.clone();
    final_paths[0].2 = *revived;
    final_paths[15].2 = *replacement;
    // The source set still has 16 two-token documents and `revised` occurs
    // once per matching document. Owner 16 has taken owner 1's place.
    let tied_score = before.hits.first().ok_or("missing repack rank")?.score;
    let expected_rank: Vec<_> = final_paths[1..]
        .iter()
        .map(|&(path, owner, _)| (owner, path, tied_score))
        .collect();
    let verify_final = |reader: &NativeUdsClient| -> TestResult {
        verify_paths(reader, revived_view, &final_paths)?;
        let ranked = rank(reader, revived_view)?;
        require(ranked.exhaustive, "post-compaction rank was truncated")?;
        require_equal(&ranked.matched, &15, "post-compaction shared term count")?;
        let observed_rank: Vec<_> = ranked
            .hits
            .iter()
            .map(|hit| (hit.owner, hit.path.as_str(), hit.score))
            .collect();
        require_equal(
            &observed_rank,
            &expected_rank,
            "post-compaction shared term rank and score",
        )?;
        for (query, expected) in [
            ("newowner00", None),
            ("oldowner15", None),
            (
                "revivedowner00",
                Some((paths[0].as_str(), owner(1), *revived)),
            ),
            (
                "newowner15",
                Some((paths[15].as_str(), owner(16), *replacement)),
            ),
        ] {
            verify_text(reader, revived_view, query, expected)?;
        }
        Ok(())
    };
    verify_final(&client)?;
    // A writer that still holds the pre-delete source cannot erase the
    // re-added owner, even when the new posting lives beside compacted layers.
    let delete_revived =
        |reader: &NativeUdsClient, operation, expected, predecessor| -> TestResult<_> {
            let ingress = reader.begin_input(operation, &budget())?;
            let mut request = prepare_advance(
                reader,
                ingress,
                revived_view,
                [OwnerMutation {
                    owner: owner(1),
                    expected: Some(expected),
                    next: None,
                }],
            )?;
            let UpdateKind::SourceAdvance(ref mut transition) = request.kind else {
                return Err("layered CAS request is not a source advance".into());
            };
            let event = transition
                .events
                .first_mut()
                .ok_or("missing source event")?;
            event.predecessor = predecessor;
            event.sequence = predecessor + 1;
            Ok(reader.submit(request, &budget())?)
        };
    let stale_source = delete_revived(&client, operation(22), latest[0], 18)?;
    require(
        matches!(
            &stale_source,
            SubmitOutcome::Terminal(OperationStatus::Refused(Refusal::Conflict))
        ),
        format!("stale source CAS erased the re-added owner: {stale_source:?}"),
    )?;
    require_equal(
        &client
            .read_view(
                ViewSelection::Latest {
                    workspace: workspace(),
                    epoch: 1,
                },
                &budget(),
            )?
            .identity,
        &revived_view,
        "stale source CAS changed the head",
    )?;
    daemon.stop()?;
    let reopened = Daemon::start(&config_path, &socket)?;
    let reader = NativeUdsClient::new(socket);
    verify_text(
        &reader,
        full,
        "\"oldowner00 baseline\"",
        Some((paths[0].as_str(), owner(1), initial[0])),
    )?;
    verify_text(
        &reader,
        view,
        "\"newowner00 revised\"",
        Some((paths[0].as_str(), owner(1), latest[0])),
    )?;
    verify_text(
        &reader,
        repacked,
        "\"newowner00 revised\"",
        Some((paths[0].as_str(), owner(1), latest[0])),
    )?;
    require_equal(
        &rank(&reader, repacked)?.hits,
        &after.hits,
        "cold old exact rank after source advances",
    )?;
    verify_text(&reader, removed, "newowner00", None)?;
    verify_final(&reader)?;
    // The refused event still binds its original digest. A fresh stream
    // supplies the current-reference control against the same view.
    let control = OperationKey {
        stream: StreamId([8; 16]),
        ..operation(2)
    };
    let deleted = published(&delete_revived(&reader, control, *revived, 0)?)?;
    verify_text(&reader, deleted, "revivedowner00", None)?;
    verify_text(
        &reader,
        revived_view,
        "revivedowner00",
        Some((paths[0].as_str(), owner(1), *revived)),
    )?;
    reopened.stop()
}

#[test]
fn layered_text_dense_layers_split_one_partition_and_survive_restart() -> TestResult {
    const BASE_OWNERS: usize = 66;
    const DELTA_OWNERS: usize = 65;
    const DELTA_LAYERS: usize = 15;
    // The initial 66 tiny documents fit one 128-document segment. Each later
    // batch adds 65 owners strictly inside its root fence. The owner IDs are
    // interleaved across that fence, so every new layer overlaps the earlier
    // physical ranges. At 16 live layers, even the smallest adjacent pair
    // contains 130 documents and cannot use the 128-document pair compactor.
    require(BASE_OWNERS <= 128, "dense FULL exceeds one segment")?;
    require(
        DELTA_OWNERS * 2 > 128 && BASE_OWNERS + DELTA_OWNERS > 128,
        "dense adjacent layers fit one segment",
    )?;
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let root = temp.path().join("state");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-config.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&layered_config(&root, &socket)?)?,
    )?;
    let daemon = Daemon::start(&config_path, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let configured = configure_layered(&client)?;

    let ingress = client.begin_input(operation(2), &budget())?;
    let base_ids = (0..BASE_OWNERS)
        .map(|index| u16::try_from(index * 16))
        .collect::<Result<Vec<_>, _>>()?;
    let base = stage_sources(
        &client,
        ingress,
        base_ids
            .iter()
            .map(|&id| source(&dense_path(id), dense_body(id).as_bytes()))
            .collect(),
    )?;
    require_equal(&base.len(), &BASE_OWNERS, "dense FULL source count")?;
    let full = advance(
        &client,
        ingress,
        configured,
        base_ids
            .iter()
            .zip(&base)
            .map(|(&id, &reference)| OwnerMutation {
                owner: dense_owner(id),
                expected: None,
                next: Some(reference),
            }),
    )?;
    let mut view = full;
    let mut final_batch = Vec::new();
    for layer in 1..=DELTA_LAYERS {
        let sequence = u64::try_from(layer + 2)?;
        let layer_id = u16::try_from(layer)?;
        let ids = (0..DELTA_OWNERS)
            .map(|index| u16::try_from(index * 16 + layer))
            .collect::<Result<Vec<_>, _>>()?;
        require(
            ids.iter().all(|id| *id > 0 && *id < 1040),
            "dense delta owner escaped original fence",
        )?;
        let ingress = client.begin_input(operation(sequence), &budget())?;
        let staged = stage_sources(
            &client,
            ingress,
            ids.iter()
                .map(|&id| source(&dense_path(id), dense_body(id).as_bytes()))
                .collect(),
        )?;
        require_equal(&staged.len(), &DELTA_OWNERS, "dense delta source count")?;
        let prior = view;
        view = advance(
            &client,
            ingress,
            prior,
            ids.iter()
                .zip(&staged)
                .map(|(&id, &reference)| OwnerMutation {
                    owner: dense_owner(id),
                    expected: None,
                    next: Some(reference),
                }),
        )?;
        require(view != prior, "dense delta did not advance the view")?;
        if layer_id == 15 {
            final_batch = ids.into_iter().zip(staged).collect();
        }
    }

    let verify = |reader: &NativeUdsClient| -> TestResult {
        let shared_term = reader.search_text(
            ViewSelection::Exact(view),
            layered_text_spec().instance,
            TextQueryRequest {
                syntax: TextQuerySyntax::Native,
                query_text: "fixture".into(),
                constraints: QueryConstraintSetV1::unconstrained(),
                generation: None,
                generation_selector: None,
                top_k: 8,
                cursor: None,
            },
            &budget(),
        )?;
        require(
            shared_term.exhaustive && shared_term.matched == 1_041,
            "dense shared term omitted or duplicated a live owner",
        )?;
        for (&id, &reference) in [0, 16, 1040]
            .iter()
            .zip([base[0], base[1], base[65]].iter())
        {
            let path = dense_path(id);
            let owner = dense_owner(id);
            let word = format!("denseword{id:04}");
            verify_text(reader, full, &word, Some((&path, owner, reference)))?;
            verify_text(
                reader,
                full,
                &format!("\"{word} fixture\""),
                Some((&path, owner, reference)),
            )?;
            verify_text(reader, view, &word, Some((&path, owner, reference)))?;
        }
        for &(id, reference) in &[final_batch[0], final_batch[32], final_batch[64]] {
            let path = dense_path(id);
            let owner = dense_owner(id);
            let word = format!("denseword{id:04}");
            verify_text(reader, full, &word, None)?;
            verify_text(reader, view, &word, Some((&path, owner, reference)))?;
            verify_text(
                reader,
                view,
                &format!("\"{word} fixture\""),
                Some((&path, owner, reference)),
            )?;
            verify_text(
                reader,
                view,
                &format!("select:path densepath{id:04}"),
                Some((&path, owner, reference)),
            )?;
        }
        Ok(())
    };
    verify(&client)?;
    let full_artifact = layered_artifact(&client, full, "denseword0000")?;
    let split_artifact = layered_artifact(&client, view, "denseword0000")?;
    // Change one owner in the left partition. The right partition's physical
    // segment references must remain identical across this SourceAdvance.
    let ingress = client.begin_input(operation(18), &budget())?;
    let replacement = stage_sources(
        &client,
        ingress,
        vec![source(&dense_path(0), b"replacementword0000 fixture")],
    )?;
    let [replacement] = replacement.as_slice() else {
        return Err("single-scope replacement source count differs".into());
    };
    let single_scope = advance(
        &client,
        ingress,
        view,
        [OwnerMutation {
            owner: dense_owner(0),
            expected: Some(base[0]),
            next: Some(*replacement),
        }],
    )?;
    verify_text(&client, single_scope, "denseword0000", None)?;
    verify_text(
        &client,
        single_scope,
        "replacementword0000",
        Some((&dense_path(0), dense_owner(0), *replacement)),
    )?;
    verify_text(
        &client,
        single_scope,
        "denseword1040",
        Some((&dense_path(1040), dense_owner(1040), base[65])),
    )?;
    let single_scope_artifact = layered_artifact(&client, single_scope, "denseword0016")?;
    daemon.stop()?;
    {
        let mut work = WorkBudget::with_request(
            WorkLimits {
                reads: 1_000_000,
                writes: 0,
                bytes: 536_870_912,
                mutations: 0,
            },
            budget(),
        );
        let reader = super::large_restart::open_test_object_reader(&root, &mut work)?;
        let full_parts = layered_root_parts(&reader, full_artifact, &mut work)?;
        require_equal(&full_parts.len(), &1, "dense initial root partitions")?;
        let split_parts = layered_root_parts(&reader, split_artifact, &mut work)?;
        let [left, right] = split_parts.as_slice() else {
            return Err("dense split did not publish two partitions".into());
        };
        let left = reader.get(*left, &mut work)?;
        let right = reader.get(*right, &mut work)?;
        require(
            left.payload.starts_with(b"QTXPART2") && right.payload.starts_with(b"QTXPART2"),
            "dense split did not publish two layer manifests",
        )?;
        require(
            left.registration
                .children
                .as_slice()
                .iter()
                .any(|child| right.registration.children.as_slice().contains(child)),
            "dense split did not reuse an immutable layer across its partitions",
        )?;
        let single_scope_parts = layered_root_parts(&reader, single_scope_artifact, &mut work)?;
        require(
            !single_scope_parts.contains(&split_parts[0]),
            "single-scope delta reused its changed partition",
        )?;
        require_equal(
            &single_scope_parts.last(),
            &Some(&split_parts[1]),
            "single-scope delta changed an unaffected physical partition ID",
        )?;
        let preserved = reader.get(
            *single_scope_parts
                .last()
                .ok_or("single-scope delta has no right partition")?,
            &mut work,
        )?;
        require_equal(
            &preserved.registration.children.as_slice(),
            &right.registration.children.as_slice(),
            "single-scope delta changed unaffected physical segment IDs",
        )?;
    }
    let reopened = Daemon::start(&config_path, &socket)?;
    let cold = NativeUdsClient::new(socket);
    verify(&cold)?;
    verify_text(&cold, single_scope, "denseword0000", None)?;
    verify_text(
        &cold,
        single_scope,
        "replacementword0000",
        Some((&dense_path(0), dense_owner(0), *replacement)),
    )?;
    verify_text(
        &cold,
        single_scope,
        "denseword1040",
        Some((&dense_path(1040), dense_owner(1040), base[65])),
    )?;
    reopened.stop()
}

#[test]
fn layered_text_excluded_sources_keep_paths_without_body_partitions() -> TestResult {
    use quanta_index_contract::{
        ExactRepoRelativePathV1, native_protocol::NativeTextPreviewEvidence,
    };

    const BINARY_PATH: &str = "src/binarystable.dat";
    const UTF8_PATH: &str = "src/utf8stable.dat";
    let temp = quanta_index_searchd_harness::private_tempdir()?;
    let root = temp.path().join("state");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let socket = temp.path().join("native.sock");
    let config_path = temp.path().join("native-config.json");
    fs::write(
        &config_path,
        serde_json::to_vec(&layered_config(&root, &socket)?)?,
    )?;
    let daemon = Daemon::start(&config_path, &socket)?;
    let client = NativeUdsClient::new(socket.clone());
    let configured = configure_layered(&client)?;

    let ingress = client.begin_input(operation(2), &budget())?;
    let initial = stage_sources(
        &client,
        ingress,
        vec![
            SourceInput {
                path: BINARY_PATH.into(),
                bytes: vec![0xff, 0],
                text: SourceTextAdmission::Excluded,
                language: None,
            },
            SourceInput {
                path: UTF8_PATH.into(),
                bytes: b"hiddenbodytoken".to_vec(),
                text: SourceTextAdmission::Excluded,
                language: None,
            },
        ],
    )?;
    let [binary, utf8] = initial.as_slice() else {
        return Err("layered excluded source count differs".into());
    };
    let g1 = advance(
        &client,
        ingress,
        configured,
        [
            OwnerMutation {
                owner: owner(1),
                expected: None,
                next: Some(*binary),
            },
            OwnerMutation {
                owner: owner(2),
                expected: None,
                next: Some(*utf8),
            },
        ],
    )?;

    let ingress = client.begin_input(operation(3), &budget())?;
    let next = stage_sources(
        &client,
        ingress,
        vec![source(BINARY_PATH, b"visiblebodytoken")],
    )?;
    let [visible] = next.as_slice() else {
        return Err("layered admitted replacement count differs".into());
    };
    let g2 = advance(
        &client,
        ingress,
        g1,
        [OwnerMutation {
            owner: owner(1),
            expected: Some(*binary),
            next: Some(*visible),
        }],
    )?;
    let ingress = client.begin_input(operation(4), &budget())?;
    let g3 = advance(
        &client,
        ingress,
        g2,
        [OwnerMutation {
            owner: owner(1),
            expected: Some(*visible),
            next: Some(*visible),
        }],
    )?;
    let ingress = client.begin_input(operation(5), &budget())?;
    let g4 = advance(
        &client,
        ingress,
        g3,
        [
            OwnerMutation {
                owner: owner(1),
                expected: Some(*visible),
                next: None,
            },
            OwnerMutation {
                owner: owner(2),
                expected: Some(*utf8),
                next: None,
            },
        ],
    )?;

    let verify = |reader: &NativeUdsClient| -> TestResult {
        let check_preview = |view: ViewRef, source: ObjectRef| -> TestResult {
            let response = reader.search_text_path_preview(
                ViewSelection::Exact(view),
                layered_text_spec().instance,
                TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "select:path binarystable".into(),
                    constraints: QueryConstraintSetV1::from_exact_repo_relative_path(
                        ExactRepoRelativePathV1::new(BINARY_PATH)?,
                    ),
                    generation: None,
                    generation_selector: None,
                    top_k: 2,
                    cursor: None,
                },
                &budget(),
            )?;
            require_equal(
                &response.ranked.ranking.matched,
                &1,
                "layered excluded preview match",
            )?;
            let [hit] = response.ranked.ranking.hits.as_slice() else {
                return Err("layered excluded preview hit absent".into());
            };
            require_equal(&hit.source, &source, "layered excluded preview source")?;
            let Some([preview]) = response.previews.as_deref() else {
                return Err("layered excluded path preview absent".into());
            };
            require_equal(
                &preview.evidence,
                &NativeTextPreviewEvidence::Path,
                "layered excluded preview evidence",
            )
        };
        verify_paths(
            reader,
            g1,
            &[
                (BINARY_PATH, owner(1), *binary),
                (UTF8_PATH, owner(2), *utf8),
            ],
        )?;
        for view in [g2, g3] {
            verify_paths(
                reader,
                view,
                &[
                    (BINARY_PATH, owner(1), *visible),
                    (UTF8_PATH, owner(2), *utf8),
                ],
            )?;
        }
        for (view, current) in [(g1, *binary), (g2, *visible), (g3, *visible)] {
            verify_text(
                reader,
                view,
                "select:path binarystable",
                Some((BINARY_PATH, owner(1), current)),
            )?;
            verify_text(
                reader,
                view,
                "select:path utf8stable",
                Some((UTF8_PATH, owner(2), *utf8)),
            )?;
            check_preview(view, current)?;
        }
        verify_text(reader, g1, "hiddenbodytoken", None)?;
        verify_text(reader, g1, "visiblebodytoken", None)?;
        verify_text(
            reader,
            g2,
            "visiblebodytoken",
            Some((BINARY_PATH, owner(1), *visible)),
        )?;
        verify_text(
            reader,
            g3,
            "visiblebodytoken",
            Some((BINARY_PATH, owner(1), *visible)),
        )?;
        verify_text(reader, g2, "hiddenbodytoken", None)?;
        verify_paths(reader, g4, &[])?;
        verify_text(reader, g4, "select:path binarystable", None)?;
        verify_text(reader, g4, "select:path utf8stable", None)?;
        verify_text(reader, g4, "visiblebodytoken", None)?;
        Ok(())
    };
    verify(&client)?;
    daemon.stop()?;
    let reopened = Daemon::start(&config_path, &socket)?;
    verify(&NativeUdsClient::new(socket))?;
    reopened.stop()
}
