//! Component proof for exact protected Native Source heads through UDS import,
//! candidate seal, and daemon restart. Installed source authority is separate.

use super::*;
use quanta_index_contract::index::{
    NativeProtectedFrontierV1, NativeProtectedNativeOriginV1, NativeProtectedNativeSnapshotV1,
    NativeProtectedSourceAuthorityV1, NativeProtectedSourceHeadV1, NativeProtectedSourceRoutesV1,
    native_protected_view_marker_bytes_v1,
};
use quanta_index_contract::native_protocol::{
    NativeImmutableObjectChildrenV1, NativeImmutableObjectPayloadV1,
};
use quanta_index_contract::{RepoId, RevisionId};
use rusqlite::params;

#[derive(Debug)]
struct SdkPhaseError {
    phase: String,
    source: Box<dyn std::error::Error>,
}

impl std::fmt::Display for SdkPhaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "protected Source SDK phase {}: {}",
            self.phase, self.source
        )
    }
}

impl std::error::Error for SdkPhaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn sdk_phase<T, E>(phase: impl Into<String>, result: Result<T, E>) -> TestResult<T>
where
    E: Into<Box<dyn std::error::Error>>,
{
    result.map_err(|source| {
        Box::new(SdkPhaseError {
            phase: phase.into(),
            source: source.into(),
        }) as Box<dyn std::error::Error>
    })
}

fn sibling(slot: u8) -> WorkspaceKey {
    WorkspaceKey {
        slot: SlotId([slot; 16]),
        ..workspace()
    }
}

fn in_workspace(mut key: OperationKey, scope: WorkspaceKey) -> OperationKey {
    key.workspace = scope;
    key
}

fn publish_sibling<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    scope: WorkspaceKey,
    id: u8,
) -> TestResult<(OperationKey, ViewRef, UpdateRequest)> {
    let NativeIndexResponse::Created(genesis) = sdk_phase(
        "sibling create workspace",
        client.request(
            NativeIndexRequest::CreateWorkspace {
                workspace: scope,
                epoch: 1,
            },
            &budget(),
        ),
    )?
    else {
        return Err("protected Source genesis was not created".into());
    };
    let profile_key = in_workspace(operation(1), scope);
    let profile_ingress = sdk_phase(
        "sibling begin profile input",
        client.begin_input(profile_key, &budget()),
    )?;
    let profile = sdk_phase(
        "sibling stage profile",
        client.stage_profile(
            profile_ingress,
            IndexProfile {
                instances: vec![native_text_spec()],
            },
            &budget(),
        ),
    )?;
    let configured = published(&sdk_phase(
        "sibling submit profile",
        client.submit(
            UpdateRequest {
                operation: profile_key,
                expected_view: genesis,
                kind: UpdateKind::Reconfigure { profile },
            },
            &budget(),
        ),
    )?)?;
    let source_key = in_workspace(operation(2), scope);
    let source_ingress = sdk_phase(
        "sibling begin Source input",
        client.begin_input(source_key, &budget()),
    )?;
    let (request, _) = sdk_phase(
        "sibling prepare Source input",
        prepare_text_owner_in_ingress(
            client,
            configured,
            source_ingress,
            id,
            &format!("protected/{id}.rs"),
            &format!("protected source {id}"),
        ),
    )?;
    let view = published(&sdk_phase(
        "sibling submit Source",
        client.submit(request.clone(), &budget()),
    )?)?;
    Ok((source_key, view, request))
}

fn stage_object<P: AsRef<std::path::Path>>(
    client: &NativeUdsClient<P>,
    ingress: IngressRef,
    bytes: Vec<u8>,
    children: Vec<ObjectRef>,
) -> TestResult<ObjectRef> {
    let phase = format!(
        "stage protected immutable object (payload {} bytes, {} children)",
        bytes.len(),
        children.len()
    );
    let NativeIndexResponse::Staged(reference) = sdk_phase(
        phase,
        client.request(
            NativeIndexRequest::StageImmutableObject {
                ingress,
                bytes: NativeImmutableObjectPayloadV1::new(bytes)?,
                children: NativeImmutableObjectChildrenV1::new(children)?,
            },
            &budget(),
        ),
    )?
    else {
        return Err("protected immutable object was not staged".into());
    };
    Ok(reference)
}

fn actual_snapshot_from_sql(
    root: &std::path::Path,
    routes: &NativeProtectedSourceRoutesV1,
    route_ref: ObjectRef,
    nonce: Digest,
) -> TestResult<NativeProtectedNativeSnapshotV1> {
    // This fixture copies persisted DATA, including the private operation and
    // event-claim codecs. It never constructs an ACK, event, or frontier.
    let db = rusqlite::Connection::open_with_flags(
        root.join("catalog/catalog-v1.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut snapshot = NativeProtectedNativeSnapshotV1 {
        wire_version: NativeProtectedNativeSnapshotV1::WIRE_VERSION,
        source_root_nonce: nonce,
        source_routes: route_ref,
        operation_rows: Vec::new(),
        genesis_views: Vec::new(),
        frontiers: Vec::new(),
        source_authorities: Vec::new(),
        event_claims: Vec::new(),
        roots: vec![route_ref],
    };
    for head in &routes.heads {
        let workspace = serde_json::to_vec(&head.operation.key.workspace)?;
        let mut operations = db.prepare(
            "SELECT payload FROM native_operation_v1 WHERE workspace=?1 ORDER BY operation",
        )?;
        for payload in operations.query_map(params![&workspace], |row| row.get::<_, Vec<u8>>(0))? {
            snapshot.operation_rows.push(payload?);
        }
        let mut claims = db.prepare(
            "SELECT c.payload FROM native_event_claim_v1 c JOIN native_operation_v1 o ON c.operation=o.operation WHERE o.workspace=?1 ORDER BY c.identity",
        )?;
        for payload in claims.query_map(params![&workspace], |row| row.get::<_, Vec<u8>>(0))? {
            snapshot.event_claims.push(payload?);
        }
        let mut frontiers = db.prepare(
            "SELECT producer,stream,stream_epoch,sequence FROM native_frontier_v1 WHERE workspace=?1 ORDER BY producer,stream",
        )?;
        for row in frontiers.query_map(params![&workspace], |row| {
            Ok((
                row.get::<_, Vec<u8>>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })? {
            let (producer, stream, stream_epoch, sequence) = row?;
            snapshot.frontiers.push(NativeProtectedFrontierV1 {
                workspace: head.operation.key.workspace,
                producer: ProducerId(
                    producer
                        .try_into()
                        .map_err(|_| "protected frontier producer width differs")?,
                ),
                stream: StreamId(
                    stream
                        .try_into()
                        .map_err(|_| "protected frontier stream width differs")?,
                ),
                stream_epoch: u64::try_from(stream_epoch)?,
                sequence: u64::try_from(sequence)?,
            });
        }
        let mut authorities = db.prepare(
            "SELECT workspace_epoch,producer,source_epoch FROM native_source_authority_v1 WHERE workspace=?1 ORDER BY workspace_epoch,producer",
        )?;
        for row in authorities.query_map(params![&workspace], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })? {
            let (workspace_epoch, producer, source_epoch) = row?;
            snapshot
                .source_authorities
                .push(NativeProtectedSourceAuthorityV1 {
                    workspace: head.operation.key.workspace,
                    workspace_epoch: u64::try_from(workspace_epoch)?,
                    producer: ProducerId(
                        producer
                            .try_into()
                            .map_err(|_| "protected source authority producer width differs")?,
                    ),
                    source_epoch: u64::try_from(source_epoch)?,
                });
        }
        snapshot.roots.extend([
            head.source_operation.request,
            head.operation.request,
            head.source_publication.profile,
            head.publication.profile,
        ]);
        snapshot.roots.extend(
            [
                head.publication.source.owners.0,
                head.publication.source.paths.0,
                head.publication.source.frontiers.0,
                head.publication.projections.0,
                head.publication.members.0,
                head.publication.coverage.0,
            ]
            .into_iter()
            .flatten(),
        );
    }
    snapshot.roots.sort_unstable();
    snapshot.roots.dedup();
    snapshot.validate_shape_v1()?;
    Ok(snapshot)
}

#[test]
fn native_protected_two_source_heads_import_seal_and_reopen() -> TestResult {
    use quanta_index_core::index::ports::CatalogExactHistoryPort;
    use std::io::Write as _;
    use std::net::Shutdown;
    use std::os::unix::net::UnixStream;
    let old_root = private_root()?;
    let old_socket = old_root.path().join("native.sock");
    // Match the product candidate lifecycle: the original immutable requests
    // must keep their custody until BOOT adopts them. Normal retention starts
    // only after selection, as in the reopened service below.
    let old_running = NativeServiceGuard::start(read_session_service_with_retention(
        old_root.path(),
        &old_socket,
        Duration::from_secs(30),
        None,
    )?);
    let old_client = NativeUdsClient::new(&old_socket);
    let old_primary = sdk_phase(
        "old root create primary",
        create_text_workspace(&old_client),
    )?;
    let (old_primary, _) = sdk_phase(
        "old root publish primary",
        publish_text_owner(&old_client, old_primary, 2, 1, "primary.rs", "old primary"),
    )?;
    let first = sdk_phase(
        "old root publish sibling 31",
        publish_sibling(&old_client, sibling(31), 31),
    )?;
    let second = sdk_phase(
        "old root publish sibling 32",
        publish_sibling(&old_client, sibling(32), 32),
    )?;
    old_running.stop()?;

    let catalog = SqliteCatalog::open(old_root.path(), Duration::from_secs(2))?;
    let mut work = WorkBudget::with_request(work_limits(), budget());
    // Restore and seal must accept a legitimate persisted primary floor even
    // when candidate assembly does not run the background retention pump.
    assert_eq!(
        catalog.advance_exact_history_floor(
            old_primary.workspace,
            old_primary.epoch,
            old_primary.sequence,
            &mut work,
        )?,
        old_primary.sequence
    );
    let routes = NativeProtectedSourceRoutesV1 {
        wire_version: NativeProtectedSourceRoutesV1::WIRE_VERSION,
        native_origin: None,
        heads: [("repo-a", "rev-a", &first), ("repo-b", "rev-b", &second)]
            .into_iter()
            .map(|(repo, revision, (key, _, _))| -> TestResult<_> {
                let operation = catalog.operation(*key)?;
                let publication = catalog.published_view_metadata(*key, &mut work)?;
                Ok(NativeProtectedSourceHeadV1 {
                    repo_id: RepoId::new(repo)?,
                    revision_id: RevisionId::new(revision)?,
                    source_operation: operation,
                    source_publication: publication.clone(),
                    operation,
                    publication,
                })
            })
            .collect::<TestResult<Vec<_>>>()?,
        repomap_heads: Vec::new(),
    };
    routes.validate()?;
    let primary_request = catalog.operation(operation(2))?.request;
    drop(catalog);

    let old_running = NativeServiceGuard::start(read_session_service_with_retention(
        old_root.path(),
        &old_socket,
        Duration::from_secs(30),
        None,
    )?);
    let old_client = NativeUdsClient::new(&old_socket);
    let old_ingress = sdk_phase(
        "old root begin route input",
        old_client.begin_input(operation(50), &budget()),
    )?;
    for head in &routes.heads {
        let marker = native_protected_view_marker_bytes_v1(head.publication.identity)?;
        let reference = sdk_phase(
            format!("old root stage view marker {:?}", head.publication.identity),
            stage_object(&old_client, old_ingress, marker, Vec::new()),
        )?;
        assert_eq!(
            reference,
            quanta_index_contract::index::native_protected_view_marker_ref_v1(
                head.publication.identity
            )?
        );
    }
    let old_route_ref = sdk_phase(
        "old root stage routes",
        stage_object(
            &old_client,
            old_ingress,
            serde_json::to_vec(&routes)?,
            routes.children()?,
        ),
    )?;
    assert_eq!(old_route_ref, routes.object_ref()?);
    let seal = old_client.request(
        NativeIndexRequest::SealCandidateWithSourceRoutes {
            primary: operation(2),
            ingress: old_ingress,
            routes: old_route_ref,
        },
        &budget(),
    );
    let stopped = old_running.stop();
    let reply = seal.map_err(|error| {
        // The normal retention pump may retire terminal request custody.
        // Inspect the failed root before TempDir removes it, preserving the
        // actual refusal as the source of the phase error.
        let registered = rusqlite::Connection::open_with_flags(
            old_root.path().join("catalog/catalog-v1.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .and_then(|db| {
            db.query_row(
                "SELECT EXISTS(SELECT 1 FROM native_object_v1 WHERE object=?1)",
                params![primary_request.0.0.as_slice()],
                |row| row.get::<_, bool>(0),
            )
        });
        SdkPhaseError {
            phase: format!(
                "old root seal protected routes (primary request registered: {registered:?})",
            ),
            source: Box::new(error),
        }
    })?;
    stopped?;
    let NativeIndexResponse::RootScopedView(old_selected) = reply else {
        return Err("old protected route was not sealed".into());
    };
    let snapshot = actual_snapshot_from_sql(
        old_root.path(),
        &routes,
        old_route_ref,
        old_selected.root_nonce,
    )?;
    assert_eq!(snapshot.operation_rows.len(), 4);
    assert_eq!(snapshot.event_claims.len(), 2);
    assert_eq!(snapshot.frontiers.len(), 2);
    assert_eq!(snapshot.source_authorities.len(), 2);
    let old_objects = FileObjectStore::open(old_root.path().join("native-objects"))?;

    let new_root = private_root()?;
    let new_socket = new_root.path().join("native.sock");
    let new_running = NativeServiceGuard::start(read_session_service_with_retention(
        new_root.path(),
        &new_socket,
        Duration::from_secs(30),
        None,
    )?);
    let new_client = NativeUdsClient::new(new_socket.as_path());
    let new_primary = sdk_phase(
        "new root create primary",
        create_text_workspace(&new_client),
    )?;
    let (new_primary, _) = sdk_phase(
        "new root publish primary",
        publish_text_owner(
            &new_client,
            new_primary,
            2,
            8,
            "new-primary.rs",
            "new primary",
        ),
    )?;
    assert_ne!(
        old_selected.view.source,
        sdk_phase(
            "new root read primary",
            new_client.read_view(ViewSelection::Exact(new_primary), &budget())
        )?
        .source
    );
    let snapshot_ref = snapshot.object_ref_v1()?;
    let mut imported_routes = routes.clone();
    imported_routes.native_origin = Some(NativeProtectedNativeOriginV1 {
        source_root_nonce: old_selected.root_nonce,
        source_routes: old_route_ref,
        snapshot: snapshot_ref,
    });
    let imported_ref = imported_routes.object_ref()?;
    let stage_import_v1 = |phase: &'static str,
                           client: &NativeUdsClient<&std::path::Path>,
                           ingress|
     -> TestResult<()> {
        for root in &snapshot.roots {
            assert!(
                sdk_phase(
                    format!("{phase} stage old object closure {root:?}"),
                    native_restore::stage_old_history_closure(&old_objects, client, ingress, *root),
                )? > 0
            );
        }
        assert_eq!(
            sdk_phase(
                format!("{phase} stage snapshot"),
                stage_object(
                    client,
                    ingress,
                    serde_json::to_vec(&snapshot)?,
                    snapshot.roots.clone()
                )
            )?,
            snapshot_ref
        );
        assert_eq!(
            sdk_phase(
                format!("{phase} stage imported routes"),
                stage_object(
                    client,
                    ingress,
                    serde_json::to_vec(&imported_routes)?,
                    imported_routes.children()?
                )
            )?,
            imported_ref
        );
        Ok(())
    };
    let ingress = sdk_phase(
        "new root begin import input",
        new_client.begin_input(operation(50), &budget()),
    )?;
    stage_import_v1("new root", &new_client, ingress)?;
    // A physically absent, previously staged marker must refuse before the
    // provisional Native heads are written. Restoring the same immutable file
    // then retries the exact ingress and route below.
    let marker_ref = quanta_index_contract::index::native_protected_view_marker_ref_v1(first.1)?;
    let marker_path = crate::native_object_file(new_root.path(), marker_ref)?;
    let withheld_marker = new_root.path().join("withheld-protected-view-marker");
    std::fs::rename(&marker_path, &withheld_marker)?;
    assert!(matches!(
        new_client.request(
            NativeIndexRequest::SealCandidateWithSourceRoutes {
                primary: operation(2),
                ingress,
                routes: imported_ref,
            },
            &budget(),
        ),
        Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::CorruptObject
    ));
    let db = rusqlite::Connection::open_with_flags(
        quanta_index_catalog::catalog_dir(new_root.path())
            .join(quanta_index_catalog::CATALOG_FILE_NAME),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let first_workspace = serde_json::to_vec(&first.0.workspace)?;
    let second_workspace = serde_json::to_vec(&second.0.workspace)?;
    for table in ["native_workspace_v1", "native_view_v1"] {
        let count: i64 = db.query_row(
            &format!("SELECT count(*) FROM {table} WHERE workspace IN (?1,?2)"),
            params![&first_workspace, &second_workspace],
            |row| row.get(0),
        )?;
        assert_eq!(count, 0, "marker refusal imported {table} rows");
    }
    let stored_fence: i64 = db.query_row(
        "SELECT fence FROM native_ingress_v1 WHERE operation=?1",
        params![serde_json::to_vec(&ingress.operation)?],
        |row| row.get(0),
    )?;
    assert_eq!(stored_fence, i64::try_from(ingress.fence)?);
    drop(db);
    std::fs::rename(&withheld_marker, &marker_path)?;
    // Send the actual admitted UDS request, then discard only its reply.
    // The route resolver below independently observes whether Q committed it.
    let mut lost_ack = UnixStream::connect(&new_socket)?;
    lost_ack.set_write_timeout(Some(Duration::from_secs(2)))?;
    lost_ack.write_all(&quanta_index_ipc::encode_request(
        &quanta_index_contract::native_protocol::NativeIndexRequestEnvelope {
            request_id: 70_001,
            payload: NativeIndexRequest::SealCandidateWithSourceRoutes {
                primary: operation(2),
                ingress,
                routes: imported_ref,
            },
        },
    )?)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match new_client.request(
            NativeIndexRequest::ResolveProtectedSourceHead {
                repo_id: RepoId::new("repo-a")?,
                revision_id: RevisionId::new("rev-a")?,
            },
            &budget(),
        ) {
            Ok(NativeIndexResponse::RootScopedView(found)) if found.view.identity == first.1 => {
                break;
            }
            Err(NativeUdsError::Refused(error))
                if error.code == NativeIndexErrorCode::SnapshotUnavailable
                    && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(10));
            }
            other => {
                return Err(format!("lost-ACK Seal did not commit exact route: {other:?}").into());
            }
        }
    }
    // Keep the request peer live until Q has committed. Closing it earlier
    // would cancel the server's dispatch budget, testing cancellation instead
    // of a reply that the caller never adopted.
    lost_ack.shutdown(Shutdown::Both)?;
    drop(lost_ack);
    new_running.stop()?;
    let retry_running = NativeServiceGuard::start(read_session_service_with_retention(
        new_root.path(),
        &new_socket,
        Duration::from_secs(30),
        None,
    )?);
    let retry_client = NativeUdsClient::new(new_socket.as_path());
    let retry_ingress = sdk_phase(
        "retry root begin import input",
        retry_client.begin_input(operation(50), &budget()),
    )?;
    assert_ne!(retry_ingress.fence, ingress.fence);
    stage_import_v1("retry root", &retry_client, retry_ingress)?;
    let NativeIndexResponse::RootScopedView(selected) = sdk_phase(
        "retry root seal protected routes",
        retry_client.request(
            NativeIndexRequest::SealCandidateWithSourceRoutes {
                primary: operation(2),
                ingress: retry_ingress,
                routes: imported_ref,
            },
            &budget(),
        ),
    )?
    else {
        return Err("lost-ACK protected route retry was not sealed".into());
    };
    assert_eq!(selected.view.identity, new_primary);
    retry_running.stop()?;
    drop(old_objects);
    drop(old_root);

    let reopened = NativeServiceGuard::start(read_session_service(
        new_root.path(),
        &new_socket,
        Duration::from_secs(30),
    )?);
    let client = NativeUdsClient::new(&new_socket);
    for (repo, revision, owner, (_, expected, request)) in [
        ("repo-a", "rev-a", 31, &first),
        ("repo-b", "rev-b", 32, &second),
    ] {
        let NativeIndexResponse::RootScopedView(actual) = sdk_phase(
            format!("reopen resolve {repo}/{revision}"),
            client.request(
                NativeIndexRequest::ResolveProtectedSourceHead {
                    repo_id: RepoId::new(repo)?,
                    revision_id: RevisionId::new(revision)?,
                },
                &budget(),
            ),
        )?
        else {
            return Err("protected Source head was not resolved".into());
        };
        assert_eq!(actual.view.identity, *expected);
        assert_eq!(actual.root_nonce, selected.root_nonce);
        let session = sdk_phase(
            format!("reopen open read session {repo}/{revision}"),
            client.open_read_session(ViewSelection::Exact(*expected), [owner; 32], &budget()),
        )?;
        let page = sdk_phase(
            format!("reopen search Text page {repo}/{revision}"),
            client.search_text_page_in_read_session(
                &session,
                native_text_spec().instance,
                native_text_request("protected", 4),
                None,
                &budget(),
            ),
        )?;
        assert_eq!(page.view, *expected);
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].owner.stable_id, StableOwnerId([owner; 16]));
        let _released = sdk_phase(
            format!("reopen release read session {repo}/{revision}"),
            client.release_read_session(&session, &budget()),
        )?;
        assert_eq!(
            sdk_phase(
                format!("reopen repeat Source submit {repo}/{revision}"),
                client.submit(request.clone(), &budget())
            )?,
            SubmitOutcome::Terminal(OperationStatus::Published(*expected))
        );
    }
    assert!(matches!(
        client.request(
            NativeIndexRequest::ResolveProtectedSourceHead {
                repo_id: RepoId::new("repo-c")?,
                revision_id: RevisionId::new("rev-c")?,
            },
            &budget(),
        ),
        Err(NativeUdsError::Refused(error))
            if error.code == NativeIndexErrorCode::SnapshotUnavailable
    ));
    reopened.stop()?;
    Ok(())
}
