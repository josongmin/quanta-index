use quanta_index_contract::{
    RepoId,
    native_protocol::{NativeIndexErrorCode, NativeRepositoryPathCursor},
};

use super::*;

fn stage_repository(
    client: &NativeUdsClient,
    ingress: IngressRef,
    repository: &RepoId,
    bytes: &[u8],
) -> TestResult<ObjectRef> {
    let response = client.request(
        NativeIndexRequest::StageRepositorySources {
            ingress,
            repository: repository.clone(),
            inputs: vec![SourceInput {
                path: "src/shared.rs".into(),
                bytes: bytes.to_vec(),
                text: SourceTextAdmission::Excluded,
                language: None,
            }],
        },
        &budget(),
    )?;
    let NativeIndexResponse::StagedSources(records) = response else {
        return Err(format!("unexpected repository staging response: {response:?}").into());
    };
    let [record] = records.as_slice() else {
        return Err("repository staging returned unexpected record count".into());
    };
    Ok(*record)
}

fn check_row(
    page: &quanta_index_contract::native_protocol::NativeRepositoryPathPage,
    expected_repo: Option<&RepoId>,
    expected_source: ObjectRef,
) -> TestResult {
    let [entry] = page.entries.as_slice() else {
        return Err("repository page must contain one entry".into());
    };
    assert_eq!(entry.file.path, "src/shared.rs");
    assert_eq!(entry.repository.as_ref(), expected_repo);
    assert_eq!(entry.file.source, expected_source);
    Ok(())
}

fn expect_bound_cursor_refusal(
    client: &NativeUdsClient,
    selection: ViewSelection,
    prefix: &str,
    cursor: NativeRepositoryPathCursor,
) -> TestResult {
    let result = client.query_repository_paths(
        selection,
        file_spec().instance,
        prefix.into(),
        Some(cursor),
        1,
        &budget(),
    );
    assert!(matches!(result, Err(NativeUdsError::Protocol(_))));
    Ok(())
}

#[test]
fn native_uds_repository_paths_page_same_path_and_bind_cursor() -> TestResult {
    let root = private_root()?;
    let socket = root.path().join("native.sock");
    let service = bind_service(root.path(), &socket, work_limits(), descriptors())?;
    let running = NativeServiceGuard::start(service);
    let client = NativeUdsClient::new(socket);

    let genesis_reply = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?;
    let NativeIndexResponse::Created(genesis) = genesis_reply else {
        return Err(format!("unexpected genesis response: {genesis_reply:?}").into());
    };
    let profile_ingress = client.begin_input(operation(1), &budget())?;
    let profile = client.stage_profile(
        profile_ingress,
        IndexProfile {
            instances: vec![literal_spec(), file_spec()],
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

    let ingress = client.begin_input(operation(2), &budget())?;
    let legacy = client.stage_source(
        ingress,
        "src/shared.rs".into(),
        b"legacy body",
        SourceTextAdmission::Excluded,
        &budget(),
    )?;
    let repo_a = RepoId::try_from("repo-a")?;
    let repo_b = RepoId::try_from("repo-b")?;
    let source_a = stage_repository(&client, ingress, &repo_a, b"repo a body")?;
    let source_b = stage_repository(&client, ingress, &repo_b, b"repo b body")?;
    assert_ne!(legacy, source_a);
    assert_ne!(source_a, source_b);

    let prior = client.read_view(ViewSelection::Exact(configured), &budget())?;
    let owners = client.stage_mutations(
        ingress,
        [(1_u8, legacy), (2, source_a), (3, source_b)]
            .into_iter()
            .map(|(id, record)| OwnerMutation {
                owner: OwnerKey {
                    producer: operation(2).producer,
                    stable_id: StableOwnerId([id; 16]),
                },
                expected: None,
                next: Some(record),
            }),
        &budget(),
    )?;
    let mut transition = SourceTransition {
        expected_source: prior.source,
        owners,
        events: Vec::new(),
    };
    transition.events.push(SourceEvent {
        producer: operation(2).producer,
        stream: operation(2).stream,
        stream_epoch: 1,
        predecessor: 0,
        sequence: 1,
        payload: transition.owner_digest()?,
    });
    let published_view = published(&client.submit(
        UpdateRequest {
            operation: operation(2),
            expected_view: configured,
            kind: UpdateKind::SourceAdvance(transition),
        },
        &budget(),
    )?)?;

    let scalar = client.query_paths(
        ViewSelection::Exact(published_view),
        file_spec().instance,
        "src/",
        None,
        1,
        &budget(),
    )?;
    let [entry] = scalar.entries.as_slice() else {
        return Err("scalar path page must contain the unscoped entry".into());
    };
    assert_eq!(scalar.view, published_view);
    assert_eq!(entry.path, "src/shared.rs");
    assert_eq!(entry.source, legacy);
    assert!(!scalar.has_more);

    let expected = [
        (None, legacy),
        (Some(&repo_a), source_a),
        (Some(&repo_b), source_b),
    ];
    let mut after = None;
    let mut first_cursor = None;
    for (position, (repository, source)) in expected.into_iter().enumerate() {
        let page = client.query_repository_paths(
            ViewSelection::Exact(published_view),
            file_spec().instance,
            "src/".into(),
            after,
            1,
            &budget(),
        )?;
        assert_eq!(page.view, published_view);
        check_row(&page, repository, source)?;
        assert_eq!(page.has_more, position < 2);
        if position == 0 {
            first_cursor = page.next_after.clone();
        }
        after = page.next_after;
    }
    assert!(after.is_none());
    let cursor = first_cursor.ok_or("first repository page has no cursor")?;
    expect_bound_cursor_refusal(
        &client,
        ViewSelection::Exact(configured),
        "src/",
        cursor.clone(),
    )?;
    expect_bound_cursor_refusal(
        &client,
        ViewSelection::Exact(published_view),
        "src/shared",
        cursor.clone(),
    )?;

    let server_refusal = client.request(
        NativeIndexRequest::QueryRepositoryPaths {
            selection: ViewSelection::Exact(published_view),
            instance: file_spec().instance,
            prefix: "src/shared".into(),
            after: Some(cursor),
            limit: 1,
        },
        &budget(),
    );
    assert!(matches!(server_refusal, Err(NativeUdsError::Refused(error))
        if error.code == NativeIndexErrorCode::InvalidInput));
    running.stop()?;
    Ok(())
}
