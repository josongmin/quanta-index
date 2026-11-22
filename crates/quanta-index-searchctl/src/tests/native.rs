use super::super::*;

fn workspace_hex() -> String {
    "01".repeat(64)
}

fn run_native(args: Vec<String>) -> (ExitCode, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let exit = run(args, &mut stdout, &mut stderr);
    let message = String::from_utf8(stderr).expect("CLI errors are UTF-8");
    (exit, message)
}

#[cfg(unix)]
fn scripted_native_peer(
    socket: &std::path::Path,
    calls: usize,
    mut respond: impl FnMut(
        quanta_index_contract::native_protocol::NativeIndexRequest,
    ) -> Result<
        quanta_index_contract::native_protocol::NativeIndexResponse,
        String,
    > + Send
    + 'static,
) -> Result<std::thread::JoinHandle<Result<(), String>>, Box<dyn std::error::Error>> {
    use quanta_index_contract::native_protocol::{
        NativeIndexRequestEnvelope, NativeIndexResponseEnvelope,
    };
    use std::{
        os::unix::net::UnixListener,
        time::{Duration, Instant},
    };
    let listener = UnixListener::bind(socket)?;
    listener.set_nonblocking(true)?;
    Ok(std::thread::spawn(move || {
        for _ in 0..calls {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => return Err(error.to_string()),
                }
            };
            stream
                .set_nonblocking(false)
                .map_err(|error| error.to_string())?;
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).map_err(|error| error.to_string())?;
            let payload = respond(request.payload)?;
            let encoded = quanta_index_ipc::encode_response(&NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload,
            })
            .map_err(|error| error.to_string())?;
            std::io::Write::write_all(&mut stream, &encoded).map_err(|error| error.to_string())?;
        }
        Ok(())
    }))
}

#[test]
fn native_read_view_requires_an_explicit_socket() {
    let (exit, stderr) = run_native(vec![
        "native".into(),
        "read-view".into(),
        "--workspace".into(),
        workspace_hex(),
        "--epoch".into(),
        "1".into(),
    ]);
    assert_eq!(exit, ExitCode::from(EXIT_USAGE));
    assert!(stderr.contains("--socket"), "{stderr}");
}

#[test]
fn native_read_view_rejects_a_truncated_workspace_identity() {
    let (exit, stderr) = run_native(vec![
        "native".into(),
        "read-view".into(),
        "--socket".into(),
        "/tmp/absent-native.sock".into(),
        "--workspace".into(),
        "01".repeat(63),
        "--epoch".into(),
        "1".into(),
    ]);
    assert_eq!(exit, ExitCode::from(EXIT_USAGE));
    assert!(stderr.contains("workspace"), "{stderr}");
}

#[test]
fn native_text_request_uses_the_explicit_native_socket() {
    let directory = tempfile::tempdir().expect("private missing socket fixture");
    let socket = directory.path().join("absent-native.sock");
    let (exit, stderr) = run_native(vec![
        "native".into(),
        "search-text".into(),
        "--socket".into(),
        socket.to_string_lossy().into_owned(),
        "--workspace".into(),
        workspace_hex(),
        "--epoch".into(),
        "1".into(),
        "--instance".into(),
        "02".repeat(16),
        "--query-text".into(),
        "needle".into(),
        "--top-k".into(),
        "2".into(),
    ]);
    assert_eq!(exit, ExitCode::from(EXIT_TRANSPORT));
    assert!(stderr.contains("native"), "{stderr}");
    assert!(
        stderr.contains(&socket.to_string_lossy().to_string()),
        "{stderr}"
    );
}

#[cfg(unix)]
#[test]
fn native_text_cli_preserves_projected_repository_and_ordinary_shape()
-> Result<(), Box<dyn std::error::Error>> {
    use quanta_index_contract::{
        FiniteScore, RepoId,
        index::{
            CollectionId, Digest, IndexInstanceId, ObjectRef, OwnerKey, ProducerId, SlotId,
            StableOwnerId, TenantId, ViewRef, ViewSelection, WorkspaceKey, WorktreeId,
        },
        native_protocol::{
            NativeIndexRequest, NativeIndexResponse, NativeTextCoverage, NativeTextHit,
            NativeTextPathRankingV1, NativeTextPathResultV1, NativeTextResult,
        },
    };
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("text-path.sock");
    let view = ViewRef {
        workspace: WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([1; 16]),
            worktree: WorktreeId([1; 16]),
            slot: SlotId([1; 16]),
        },
        epoch: 1,
        sequence: 2,
        digest: Digest([3; 32]),
    };
    let instance = IndexInstanceId([2; 16]);
    let ranking = NativeTextResult {
        view,
        instance,
        coverage: NativeTextCoverage::CoreRequired,
        artifact: ObjectRef(Digest([4; 32])),
        matched: 1,
        exhaustive: true,
        hits: vec![NativeTextHit {
            owner: OwnerKey {
                producer: ProducerId([5; 16]),
                stable_id: StableOwnerId([6; 16]),
            },
            source: ObjectRef(Digest([7; 32])),
            path: "src/a.rs".into(),
            score: FiniteScore::new(1.0)?,
        }],
    };
    let projected = NativeTextPathResultV1 {
        ranked: NativeTextPathRankingV1 {
            ranking: ranking.clone(),
            repositories: vec![RepoId::new("corp-a")?],
        },
        previews: None,
    };
    let expected_path = projected.clone();
    let ordinary = ranking.clone();
    let peer = scripted_native_peer(&socket, 3, move |request| {
        let NativeIndexRequest::SearchText {
            selection,
            instance: actual,
            query,
        } = request
        else {
            return Err("CLI did not use Native SearchText".into());
        };
        let expected_selection = ViewSelection::Latest {
            workspace: view.workspace,
            epoch: view.epoch,
        };
        if selection != expected_selection || actual != instance || query.top_k != 1 {
            return Err("CLI changed selected view, instance or limit".into());
        }
        match query.query_text.as_str() {
            "select:path needle" => {
                Ok(NativeIndexResponse::TextPathRankedV1(expected_path.clone()))
            }
            "needle" => Ok(NativeIndexResponse::TextRanked(ordinary.clone())),
            _ => Err("CLI changed text query".into()),
        }
    })?;
    let args = |query: &str, output: &str| {
        vec![
            "native".into(),
            "search-text".into(),
            "--socket".into(),
            socket.to_string_lossy().into_owned(),
            "--workspace".into(),
            workspace_hex(),
            "--epoch".into(),
            "1".into(),
            "--instance".into(),
            "02".repeat(16),
            "--query-text".into(),
            query.into(),
            "--top-k".into(),
            "1".into(),
            "--output".into(),
            output.into(),
        ]
    };
    let mut output = Vec::new();
    let mut errors = Vec::new();
    assert_eq!(
        run(args("select:path needle", "json"), &mut output, &mut errors),
        ExitCode::SUCCESS,
        "{}",
        String::from_utf8_lossy(&errors)
    );
    let parsed: NativeTextPathResultV1 = serde_json::from_slice(&output)?;
    assert_eq!(parsed, projected);
    output.clear();
    errors.clear();
    assert_eq!(
        run(
            args("select:path needle", "pretty"),
            &mut output,
            &mut errors
        ),
        ExitCode::SUCCESS,
        "{}",
        String::from_utf8_lossy(&errors)
    );
    let pretty = String::from_utf8(output)?;
    assert!(pretty.contains("corp-a src/a.rs"), "{pretty}");
    let mut output = Vec::new();
    errors.clear();
    assert_eq!(
        run(args("needle", "json"), &mut output, &mut errors),
        ExitCode::SUCCESS,
        "{}",
        String::from_utf8_lossy(&errors)
    );
    assert_eq!(
        serde_json::from_slice::<NativeTextResult>(&output)?,
        ranking
    );
    peer.join().map_err(|_| "scripted text peer panicked")??;
    Ok(())
}

#[cfg(unix)]
#[test]
fn native_repository_path_cli_pages_one_published_view_across_repositories()
-> Result<(), Box<dyn std::error::Error>> {
    use quanta_index_contract::{
        RepoId,
        index::{
            CollectionId, Digest, IndexInstanceId, ObjectRef, OwnerKey, ProducerId, SlotId,
            StableOwnerId, TenantId, ViewRef, ViewSelection, WorkspaceKey, WorktreeId,
        },
        native_protocol::{
            NativeFilePathEntry, NativeIndexRequest, NativeIndexRequestEnvelope,
            NativeIndexResponse, NativeIndexResponseEnvelope, NativeRepositoryPathCursor,
            NativeRepositoryPathEntry, NativeRepositoryPathPage,
        },
    };
    use std::{
        os::unix::net::UnixListener,
        time::{Duration, Instant},
    };

    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("repository-paths.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let view = ViewRef {
        workspace: WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([1; 16]),
            worktree: WorktreeId([1; 16]),
            slot: SlotId([1; 16]),
        },
        epoch: 1,
        sequence: 2,
        digest: Digest([3; 32]),
    };
    let instance = IndexInstanceId([2; 16]);
    let file = NativeFilePathEntry {
        owner: OwnerKey {
            producer: ProducerId([4; 16]),
            stable_id: StableOwnerId([5; 16]),
        },
        path: "src/a.rs".into(),
        source: ObjectRef(Digest([6; 32])),
        bytes: ObjectRef(Digest([7; 32])),
        byte_len: 8,
    };
    let cursor = NativeRepositoryPathCursor {
        view,
        instance,
        prefix: "src/".into(),
        path: file.path.clone(),
        repository: None,
    };
    let first = NativeRepositoryPathPage {
        view,
        entries: vec![NativeRepositoryPathEntry {
            repository: None,
            file: file.clone(),
        }],
        has_more: true,
        next_after: Some(cursor.clone()),
    };
    let second = NativeRepositoryPathPage {
        view,
        entries: vec![NativeRepositoryPathEntry {
            repository: Some(RepoId::new("corp-a")?),
            file,
        }],
        has_more: false,
        next_after: None,
    };
    let expected_cursor = cursor.clone();
    let peer = std::thread::spawn(move || -> Result<(), String> {
        for (number, page) in [first, second].into_iter().enumerate() {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => return Err(error.to_string()),
                }
            };
            stream
                .set_nonblocking(false)
                .map_err(|error| error.to_string())?;
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).map_err(|error| error.to_string())?;
            let NativeIndexRequest::QueryRepositoryPaths {
                selection,
                instance: actual_instance,
                prefix,
                after,
                limit,
            } = request.payload
            else {
                return Err("CLI did not use repository-aware Native request".into());
            };
            let expected_selection = if number == 0 {
                ViewSelection::Latest {
                    workspace: view.workspace,
                    epoch: view.epoch,
                }
            } else {
                ViewSelection::Exact(view)
            };
            if selection != expected_selection
                || actual_instance != instance
                || prefix != "src/"
                || limit != 1
                || after != (number == 1).then_some(expected_cursor.clone())
            {
                return Err("CLI changed the pinned repository path cursor".into());
            }
            let encoded = quanta_index_ipc::encode_response(&NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload: NativeIndexResponse::RepositoryPathPage(page),
            })
            .map_err(|error| error.to_string())?;
            stream
                .write_all(&encoded)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    });

    let mut args = vec![
        "native".into(),
        "query-repository-paths".into(),
        "--socket".into(),
        socket.to_string_lossy().into_owned(),
        "--workspace".into(),
        workspace_hex(),
        "--epoch".into(),
        "1".into(),
        "--instance".into(),
        "02".repeat(16),
        "--prefix".into(),
        "src/".into(),
        "--limit".into(),
        "1".into(),
    ];
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let exit = run(args.clone(), &mut output, &mut errors);
    assert_eq!(
        exit,
        ExitCode::SUCCESS,
        "{}",
        String::from_utf8_lossy(&errors)
    );
    let first_page: NativeRepositoryPathPage = serde_json::from_slice(&output)?;
    assert_eq!(first_page.entries.len(), 1);
    assert_eq!(first_page.entries[0].repository, None);
    args.extend([
        "--after-json".into(),
        serde_json::to_string(&first_page.next_after.expect("first continuation"))?,
    ]);
    output.clear();
    errors.clear();
    let exit = run(args, &mut output, &mut errors);
    let server = peer
        .join()
        .map_err(|_panic| "repository path peer panicked")?;
    assert_eq!(
        exit,
        ExitCode::SUCCESS,
        "{}",
        String::from_utf8_lossy(&errors)
    );
    server?;
    let second_page: NativeRepositoryPathPage = serde_json::from_slice(&output)?;
    assert_eq!(second_page.entries.len(), 1);
    assert_eq!(
        second_page.entries[0]
            .repository
            .as_ref()
            .map(RepoId::as_str),
        Some("corp-a")
    );
    assert!(!second_page.has_more);
    assert!(second_page.next_after.is_none());
    Ok(())
}

#[cfg(unix)]
#[test]
fn native_retained_page_emits_native_wire_and_preserves_exact_path()
-> Result<(), Box<dyn std::error::Error>> {
    use quanta_index_contract::{
        index::{
            CollectionId, Digest, IndexInstanceId, SlotId, TenantId, ViewRef, WorkspaceKey,
            WorktreeId,
        },
        native_protocol::{
            NativeIndexRequest, NativeIndexRequestEnvelope, NativeIndexResponse,
            NativeIndexResponseEnvelope, NativeReadSessionId, NativeReadSessionRequest,
            NativeReadSessionState, NativeReadSessionStatus, NativeTextPage,
        },
    };
    use std::{
        os::unix::net::UnixListener,
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("native.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let view = ViewRef {
        workspace: WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        },
        epoch: 1,
        sequence: 1,
        digest: Digest([5; 32]),
    };
    let status = NativeReadSessionStatus {
        session: NativeReadSessionId([6; 32]),
        view,
        expires_at_ms: 100,
        revision: 1,
        state: NativeReadSessionState::Active,
    };
    let instance = IndexInstanceId([7; 16]);
    let peer = std::thread::spawn(move || -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => return Err(error.to_string()),
            }
        };
        // BSD accept inherits the listener's O_NONBLOCK flag.
        stream
            .set_nonblocking(false)
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|error| error.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|error| error.to_string())?;
        let request: NativeIndexRequestEnvelope =
            quanta_index_ipc::decode_request(&mut stream).map_err(|error| error.to_string())?;
        let NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPage {
            session,
            instance: actual,
            query,
            after,
        }) = request.payload
        else {
            return Err("CLI used a non-Native page request".into());
        };
        if session != status.session
            || actual != instance
            || after.is_some()
            || query.query_text != "needle"
            || query
                .constraints
                .repo_relative_path_exact
                .as_ref()
                .map(|path| path.as_str())
                != Some("src/a.rs")
        {
            return Err("CLI changed session/query/filter authority".into());
        }
        let reply = NativeIndexResponseEnvelope {
            request_id: request.request_id,
            payload: NativeIndexResponse::TextRankedPage(NativeTextPage {
                session,
                view,
                instance,
                artifact: quanta_index_contract::index::ObjectRef(Digest([8; 32])),
                matched: 0,
                remaining: 0,
                hits: Vec::new(),
                after: None,
            }),
        };
        let encoded =
            quanta_index_ipc::encode_response(&reply).map_err(|error| error.to_string())?;
        stream
            .write_all(&encoded)
            .map_err(|error| error.to_string())
    });
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let exit = run(
        vec![
            "native".into(),
            "search-text-page".into(),
            "--socket".into(),
            socket.to_string_lossy().into_owned(),
            "--session-json".into(),
            serde_json::to_string(&status)?,
            "--instance".into(),
            "07".repeat(16),
            "--query-text".into(),
            "needle".into(),
            "--top-k".into(),
            "2".into(),
            "--path-exact".into(),
            "src/a.rs".into(),
            "--output".into(),
            "json".into(),
        ],
        &mut output,
        &mut errors,
    );
    let server = peer
        .join()
        .map_err(|_panic| "scripted Native CLI peer panicked")?;
    assert_eq!(
        exit,
        ExitCode::SUCCESS,
        "{}",
        String::from_utf8_lossy(&errors)
    );
    server?;
    let result: NativeTextPage = serde_json::from_slice(&output)?;
    assert_eq!(result.view, view);
    assert_eq!(result.session, status.session);
    assert_eq!(result.remaining, 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn native_retained_path_page_preserves_repository_cursor() -> Result<(), Box<dyn std::error::Error>>
{
    use quanta_index_contract::{
        FiniteScore, QueryConstraintSetV1, RepoId, TextQueryRequest, TextQuerySyntax,
        index::{
            CollectionId, Digest, IndexInstanceId, ObjectRef, SlotId, TenantId, ViewRef,
            WorkspaceKey, WorktreeId,
        },
        native_protocol::{
            NativeIndexRequest, NativeIndexResponse, NativeReadSessionId, NativeReadSessionRequest,
            NativeReadSessionState, NativeReadSessionStatus, NativeTextCoverage,
            NativeTextPathCursorV1, NativeTextPathPageV1, NativeTextPathRankingV1,
            NativeTextResult, native_text_query_digest,
        },
    };
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("path-page.sock");
    let view = ViewRef {
        workspace: WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        },
        epoch: 1,
        sequence: 1,
        digest: Digest([5; 32]),
    };
    let status = NativeReadSessionStatus {
        session: NativeReadSessionId([6; 32]),
        view,
        expires_at_ms: 100,
        revision: 1,
        state: NativeReadSessionState::Active,
    };
    let instance = IndexInstanceId([7; 16]);
    let query = TextQueryRequest {
        syntax: TextQuerySyntax::Native,
        query_text: "select:path needle".into(),
        constraints: QueryConstraintSetV1::unconstrained(),
        generation: None,
        generation_selector: None,
        top_k: 1,
        cursor: None,
    };
    let artifact = ObjectRef(Digest([8; 32]));
    let cursor = NativeTextPathCursorV1 {
        view,
        instance,
        query: native_text_query_digest(instance, &query)?,
        artifact,
        score: FiniteScore::new(1.0)?,
        repository: RepoId::new("corp-a")?,
        path: "src/a.rs".into(),
    };
    let page = NativeTextPathPageV1 {
        session: status.session,
        ranked: NativeTextPathRankingV1 {
            ranking: NativeTextResult {
                view,
                instance,
                coverage: NativeTextCoverage::CoreRequired,
                artifact,
                matched: 1,
                exhaustive: true,
                hits: Vec::new(),
            },
            repositories: Vec::new(),
        },
        remaining: 0,
        after: None,
    };
    let expected_cursor = cursor.clone();
    let expected_page = page.clone();
    let peer = scripted_native_peer(&socket, 1, move |request| {
        let NativeIndexRequest::ReadSession(NativeReadSessionRequest::SearchTextPathPageV1 {
            session,
            instance: actual,
            query,
            after,
        }) = request
        else {
            return Err("CLI did not use retained Native path page".into());
        };
        if session != status.session
            || actual != instance
            || query.query_text != "select:path needle"
            || query.top_k != 1
            || after != Some(expected_cursor.clone())
        {
            return Err("CLI changed path page session, query or cursor".into());
        }
        Ok(NativeIndexResponse::TextPathPageV1(expected_page.clone()))
    })?;
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let exit = run(
        vec![
            "native".into(),
            "search-text-path-page".into(),
            "--socket".into(),
            socket.to_string_lossy().into_owned(),
            "--session-json".into(),
            serde_json::to_string(&status)?,
            "--instance".into(),
            "07".repeat(16),
            "--query-text".into(),
            "select:path needle".into(),
            "--top-k".into(),
            "1".into(),
            "--after-json".into(),
            serde_json::to_string(&cursor)?,
            "--output".into(),
            "json".into(),
        ],
        &mut output,
        &mut errors,
    );
    let server = peer
        .join()
        .map_err(|_| "scripted path page peer panicked")?;
    assert_eq!(
        exit,
        ExitCode::SUCCESS,
        "{}",
        String::from_utf8_lossy(&errors)
    );
    server?;
    assert_eq!(
        serde_json::from_slice::<NativeTextPathPageV1>(&output)?,
        page
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn native_retained_profile_emits_one_existing_session_request()
-> Result<(), Box<dyn std::error::Error>> {
    use quanta_index_contract::{
        index::{
            CatalogView, CollectionId, Digest, IndexProfile, MapRoot, ObjectRef, SlotId, SourceCut,
            TenantId, ViewRef, WorkspaceKey, WorktreeId,
        },
        native_protocol::{
            NativeIndexRequest, NativeIndexRequestEnvelope, NativeIndexResponse,
            NativeIndexResponseEnvelope, NativeReadSessionId, NativeReadSessionProfile,
            NativeReadSessionRequest, NativeReadSessionState, NativeReadSessionStatus,
        },
    };
    use std::{
        os::unix::net::UnixListener,
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("profile.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let mut view = CatalogView {
        identity: ViewRef {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            sequence: 3,
            digest: Digest([0; 32]),
        },
        profile: ObjectRef(Digest([5; 32])),
        source: SourceCut {
            owners: MapRoot(None),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        },
        projections: MapRoot(None),
        members: MapRoot(None),
        coverage: MapRoot(None),
    };
    view.identity.digest = view.identity_digest()?;
    let status = NativeReadSessionStatus {
        session: NativeReadSessionId([6; 32]),
        view: view.identity,
        expires_at_ms: 100,
        revision: 1,
        state: NativeReadSessionState::Active,
    };
    let gold = NativeReadSessionProfile {
        session: status.session,
        view,
        profile: IndexProfile { instances: vec![] },
    };
    for output_mode in [Some("json"), None, Some("pretty")] {
        let ordinary = output_mode == Some("pretty");
        let response = gold.clone();
        let listener = listener.try_clone()?;
        let peer = std::thread::spawn(move || -> Result<(), String> {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => return Err(error.to_string()),
                }
            };
            // BSD accept inherits the listener's O_NONBLOCK flag.
            stream
                .set_nonblocking(false)
                .map_err(|error| error.to_string())?;
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .map_err(|error| error.to_string())?;
            let request: NativeIndexRequestEnvelope =
                quanta_index_ipc::decode_request(&mut stream).map_err(|error| error.to_string())?;
            if ordinary {
                if !matches!(request.payload, NativeIndexRequest::ReadView(quanta_index_contract::index::ViewSelection::Latest { workspace, epoch })
                if workspace == status.view.workspace && epoch == status.view.epoch)
                {
                    return Err("ordinary Native pretty changed its selected workspace".into());
                }
            } else if !matches!(request.payload, NativeIndexRequest::ReadSession(NativeReadSessionRequest::ReadProfile { session }) if session == status.session)
            {
                return Err("CLI did not read its existing retained profile".into());
            }
            let payload = if ordinary {
                NativeIndexResponse::View(response.view)
            } else {
                NativeIndexResponse::ReadSessionProfile(response)
            };
            let encoded = quanta_index_ipc::encode_response(&NativeIndexResponseEnvelope {
                request_id: request.request_id,
                payload,
            })
            .map_err(|error| error.to_string())?;
            stream
                .write_all(&encoded)
                .map_err(|error| error.to_string())
        });
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let mut args = vec!["native".into()];
        if ordinary {
            args.extend([
                "read-view".into(),
                "--workspace".into(),
                format!(
                    "{}{}{}{}",
                    "01".repeat(16),
                    "02".repeat(16),
                    "03".repeat(16),
                    "04".repeat(16)
                ),
                "--epoch".into(),
                "1".into(),
            ]);
        } else {
            args.extend([
                "read-session-profile".into(),
                "--session-json".into(),
                serde_json::to_string(&status)?,
            ]);
        }
        args.extend(["--socket".into(), socket.to_string_lossy().into_owned()]);
        if let Some(mode) = output_mode {
            args.extend(["--output".into(), mode.into()]);
        }
        let exit = run(args, &mut output, &mut errors);
        let server = peer.join().map_err(|_panic| "profile CLI peer panicked")?;
        assert_eq!(
            exit,
            ExitCode::SUCCESS,
            "{}",
            String::from_utf8_lossy(&errors)
        );
        server?;
        if ordinary {
            let rendered = String::from_utf8(output)?;
            assert!(
                rendered.starts_with("view: ViewRef { workspace: WorkspaceKey {"),
                "{rendered}"
            );
            assert!(rendered.contains("epoch: 1, sequence: 3"), "{rendered}");
            assert_eq!(
                rendered.lines().nth(1),
                Some(
                    "source: SourceCut { owners: MapRoot(None), paths: MapRoot(None), frontiers: MapRoot(None) }"
                )
            );
            assert!(serde_json::from_str::<serde_json::Value>(&rendered).is_err());
        } else {
            assert_eq!(
                serde_json::from_slice::<NativeReadSessionProfile>(&output)?,
                gold
            );
        }
    }
    Ok(())
}

#[test]
fn native_retained_commands_reject_pretty_before_socket_io()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("missing.sock");
    let status = serde_json::json!({
        "session": vec![6_u8;32],
        "view": {"workspace": {"tenant":vec![1_u8;16],"collection":vec![2_u8;16],
            "worktree":vec![3_u8;16],"slot":vec![4_u8;16]},
            "epoch":1,"sequence":1,"digest":vec![5_u8;32]},
        "expires_at_ms":100,"revision":1,"state":"Active"
    });
    for command in [
        "read-session-status",
        "read-session-release",
        "search-text-page",
        "search-hybrid-file",
    ] {
        let mut output = Vec::new();
        let mut errors = Vec::new();
        let exit = run(
            vec![
                "native".into(),
                command.into(),
                "--socket".into(),
                socket.to_string_lossy().into_owned(),
                "--session-json".into(),
                status.to_string(),
                "--output".into(),
                "pretty".into(),
            ],
            &mut output,
            &mut errors,
        );
        let message = String::from_utf8(errors)?;
        assert_eq!(exit, ExitCode::from(EXIT_USAGE), "{command}: {message}");
        assert!(output.is_empty());
        assert!(message.contains("--output json"), "{command}: {message}");
    }
    Ok(())
}

#[test]
fn known_legacy_query_contracts_are_typed_native_refusals_without_usage_or_io() {
    for (command, capability) in [
        ("symbol", "legacy.symbol"),
        ("hybrid-seed", "legacy.hybrid_seed"),
        ("explain", "legacy.explain"),
        ("go-test-trace-file", "legacy.go_test_trace_file"),
    ] {
        for prefix in [false, true] {
            let mut args = Vec::new();
            if prefix {
                args.push("native".into());
            }
            args.push(command.into());
            let (exit, stderr) = run_native(args);
            assert_eq!(exit, ExitCode::from(5), "{command}: {stderr}");
            let diagnostic: serde_json::Value =
                serde_json::from_str(&stderr).expect("one complete JSON diagnostic");
            assert_eq!(diagnostic["errorContractVersion"], 1);
            assert_eq!(diagnostic["code"], "CapabilityUnavailable");
            assert_eq!(diagnostic["capability"], capability);
        }
    }
}

#[test]
fn native_history_requires_retained_authority_instead_of_legacy_refusal() {
    for prefix in [false, true] {
        let mut args = Vec::new();
        if prefix {
            args.push("native".into());
        }
        args.push("history".into());
        let (exit, stderr) = run_native(args);
        assert_eq!(exit, ExitCode::from(EXIT_USAGE));
        assert!(stderr.contains("--socket"), "{stderr}");
        assert!(!stderr.contains("CapabilityUnavailable"), "{stderr}");
    }
}

#[test]
fn unknown_native_command_remains_usage_instead_of_capability_refusal() {
    for command in ["symobl", "cluster-membership"] {
        let (exit, stderr) = run_native(vec![command.into()]);
        assert_eq!(exit, ExitCode::from(2));
        assert!(stderr.contains(&format!("unsupported native command {command}")));
    }
}
