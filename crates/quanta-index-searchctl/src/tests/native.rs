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
        ("runtime-metadata", "legacy.runtime_metadata"),
        ("structural", "legacy.structural"),
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
