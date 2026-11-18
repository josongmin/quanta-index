use super::*;

fn observation() -> Result<NativeRootScopedViewV1, Box<dyn std::error::Error>> {
    let operation = key(77);
    let mut view = CatalogView {
        identity: ViewRef {
            workspace: operation.workspace,
            epoch: operation.epoch,
            sequence: 123,
            digest: Digest([0; 32]),
        },
        profile: ObjectRef(Digest([11; 32])),
        source: SourceCut {
            owners: MapRoot(Some(ObjectRef(Digest([12; 32])))),
            paths: MapRoot(None),
            frontiers: MapRoot(None),
        },
        projections: MapRoot(Some(ObjectRef(Digest([13; 32])))),
        members: MapRoot(None),
        coverage: MapRoot(None),
    };
    view.identity.digest = view.identity_digest()?;
    Ok(NativeRootScopedViewV1 {
        view,
        root_nonce: Digest([31; 32]),
        root_device: 0x1234,
        root_inode: 0x5678,
    })
}

fn exchange_request<R>(
    expected_request: NativeIndexRequest,
    response: Option<NativeIndexResponse>,
    call: impl FnOnce(&NativeUdsClient<PathBuf>) -> R,
) -> Result<(R, usize), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("root-scope.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let peer = std::thread::spawn(move || -> Result<usize, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(e) => return Err(e.to_string()),
            }
        };
        stream.set_nonblocking(false).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|e| e.to_string())?;
        let request: NativeIndexRequestEnvelope =
            quanta_index_ipc::decode_request(&mut stream).map_err(|e| e.to_string())?;
        if request.payload != expected_request {
            return Err("root observation changed the selected request".into());
        }
        let Some(payload) = response else {
            return Ok(0);
        };
        let frame = quanta_index_ipc::encode_response(&NativeIndexResponseEnvelope {
            request_id: request.request_id,
            payload,
        })
        .map_err(|e| e.to_string())?;
        let body = frame.len().checked_sub(4).ok_or("short frame")?;
        stream.write_all(&frame).map_err(|e| e.to_string())?;
        Ok(body)
    });
    let result = call(&NativeUdsClient::new(socket));
    Ok((
        result,
        peer.join()
            .map_err(|_| "root observation peer panicked")??,
    ))
}

fn exchange<R>(
    selection: ViewSelection,
    response: Option<NativeIndexResponse>,
    call: impl FnOnce(&NativeUdsClient<PathBuf>) -> R,
) -> Result<(R, usize), Box<dyn std::error::Error>> {
    exchange_request(
        NativeIndexRequest::ObserveRootView(selection),
        response,
        call,
    )
}

#[test]
fn protected_repomap_client_uses_exact_pair_and_rejects_zero_member()
-> Result<(), Box<dyn std::error::Error>> {
    let repo = quanta_index_contract::RepoId::new("repo")?;
    let revision = quanta_index_contract::RevisionId::new("revision")?;
    let request = NativeIndexRequest::ResolveProtectedRepoMapHead {
        repo_id: repo.clone(),
        revision_id: revision.clone(),
    };
    let expected = quanta_index_contract::native_protocol::NativeProtectedRepoMapResolutionV1 {
        root: observation()?,
        instance: IndexInstanceId([81; 16]),
        member_binding: ObjectRef(Digest([82; 32])),
    };
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (result, _) = exchange_request(
        request.clone(),
        Some(NativeIndexResponse::ProtectedRepoMapHead(expected.clone())),
        |client| client.resolve_protected_repomap_head_v1(repo.clone(), revision.clone(), &budget),
    )?;
    assert_eq!(result?, expected);
    let mut wrong = expected;
    wrong.member_binding = ObjectRef(Digest([0; 32]));
    let (result, _) = exchange_request(
        request,
        Some(NativeIndexResponse::ProtectedRepoMapHead(wrong)),
        |client| client.resolve_protected_repomap_head_v1(repo, revision, &budget),
    )?;
    assert!(matches!(result, Err(NativeUdsError::Protocol(_))));
    Ok(())
}

#[test]
fn protected_repomap_admitted_decoder_accepts_contract_wire()
-> Result<(), Box<dyn std::error::Error>> {
    let root = observation()?;
    let view = root.view.identity;
    let instance = IndexInstanceId([81; 16]);
    let member_binding = ObjectRef(Digest([82; 32]));
    for payload in [
        NativeIndexResponse::ProtectedRepoMapHead(
            quanta_index_contract::native_protocol::NativeProtectedRepoMapResolutionV1 {
                root,
                instance,
                member_binding,
            },
        ),
        NativeIndexResponse::ImportedRepoMapMember(
            quanta_index_contract::native_protocol::NativeImportedRepoMapMemberReadV1 {
                view,
                instance,
                member_binding,
            },
        ),
    ] {
        let envelope = NativeIndexResponseEnvelope {
            request_id: 71,
            payload,
        };
        let frame = quanta_index_ipc::encode_response(&envelope)?;
        let body = frame.get(4..).ok_or("short response frame")?;
        assert_eq!(admitted_reply_v1::decode(body)?, envelope);
    }
    Ok(())
}

#[test]
fn imported_repomap_member_admitted_keeps_exact_reply_charged_until_drop()
-> Result<(), Box<dyn std::error::Error>> {
    let view = observation()?.view.identity;
    let instance = IndexInstanceId([81; 16]);
    let expected = quanta_index_contract::native_protocol::NativeImportedRepoMapMemberReadV1 {
        view,
        instance,
        member_binding: ObjectRef(Digest([82; 32])),
    };
    let request = NativeIndexRequest::ReadImportedRepoMapMember { view, instance };
    let response = NativeIndexResponse::ImportedRepoMapMember(expected);
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));

    let (memory, active, admissions) = publication_scope(false);
    let (owned, body_bytes) =
        exchange_request(request.clone(), Some(response.clone()), |client| {
            client.request_publication_admitted(request.clone(), &budget, &memory)
        })?;
    let owned = owned?;
    assert_eq!(owned.response(), &response);
    assert!(body_bytes > 0 && admissions.load(AtomicOrdering::SeqCst) > 0);
    assert!(active.load(AtomicOrdering::SeqCst) > 0);
    drop(owned);
    assert_eq!(active.load(AtomicOrdering::SeqCst), 0);

    let mut wrong_view = expected;
    wrong_view.view.sequence += 1;
    let mut wrong_instance = expected;
    wrong_instance.instance = IndexInstanceId([83; 16]);
    let mut zero_member = expected;
    zero_member.member_binding = ObjectRef(Digest([0; 32]));
    for (case, reply) in [
        (
            "foreign view",
            NativeIndexResponse::ImportedRepoMapMember(wrong_view),
        ),
        (
            "foreign instance",
            NativeIndexResponse::ImportedRepoMapMember(wrong_instance),
        ),
        (
            "zero member",
            NativeIndexResponse::ImportedRepoMapMember(zero_member),
        ),
        (
            "wrong variant",
            NativeIndexResponse::RootScopedView(observation()?),
        ),
    ] {
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let (memory, active, admissions) = publication_scope(false);
        let (result, body_bytes) = exchange_request(request.clone(), Some(reply), |client| {
            client.request_publication_admitted(request.clone(), &budget, &memory)
        })?;
        assert!(matches!(result, Err(NativeUdsError::Protocol(_))), "{case}");
        assert!(
            body_bytes > 0 && admissions.load(AtomicOrdering::SeqCst) > 0,
            "{case}"
        );
        assert_eq!(active.load(AtomicOrdering::SeqCst), 0, "{case}");
    }

    let refusal = NativeIndexWireError {
        code: NativeIndexErrorCode::Conflict,
        message: "fixed imported RepoMap member refusal".into(),
    };
    let (memory, active, admissions) = publication_scope(false);
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (owned, body_bytes) = exchange_request(
        request.clone(),
        Some(NativeIndexResponse::Error(refusal.clone())),
        |client| client.request_publication_admitted(request, &budget, &memory),
    )?;
    let owned = owned?;
    assert!(matches!(owned.response(), NativeIndexResponse::Error(actual) if actual == &refusal));
    assert!(body_bytes > 0 && admissions.load(AtomicOrdering::SeqCst) > 0);
    assert!(active.load(AtomicOrdering::SeqCst) > 0);
    drop(owned);
    assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
    Ok(())
}

#[test]
fn root_observation_ordinary_and_admitted_decoders_keep_exact_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let expected = observation()?;
    let envelope = NativeIndexResponseEnvelope {
        request_id: 71,
        payload: NativeIndexResponse::RootScopedView(expected.clone()),
    };
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&envelope, &mut bytes)?;
    assert_eq!(admitted_reply_v1::decode(&bytes)?, envelope);
    for selection in [
        ViewSelection::Exact(expected.view.identity),
        ViewSelection::Latest {
            workspace: expected.view.identity.workspace,
            epoch: expected.view.identity.epoch,
        },
    ] {
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let (result, _) = exchange(selection, Some(envelope.payload.clone()), |client| {
            client.observe_root_view(selection, &budget)
        })?;
        assert_eq!(result?, expected);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn root_observation_admitted_borrows_peer_and_dto_until_last_grant_drop()
-> Result<(), Box<dyn std::error::Error>> {
    let expected = observation()?;
    let selection = ViewSelection::Exact(expected.view.identity);
    let response = NativeIndexResponse::RootScopedView(expected);
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (memory, active, _) = publication_scope(false);
    let (result, body_bytes) = exchange(selection, Some(response.clone()), |client| {
        client.observe_root_view_admitted(selection, &budget, &memory)
    })?;
    let result = result?;
    assert_eq!(result.response(), &response);
    assert_eq!(result.peer_credentials().pid, Some(std::process::id()));
    let reply_charge = u64::try_from(body_bytes)? + admitted_reply_v1::decoded_retained_limit()?;
    let charged = active.load(AtomicOrdering::SeqCst);
    assert!(
        charged > reply_charge,
        "connected process custody keeps its own Original grant"
    );
    assert_eq!(
        result.peer_process_v1().credentials_v1(),
        result.peer_credentials()
    );
    assert!(result.peer_process_v1().pidfd_v1().is_ok());
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(result.response(), &mut bytes)?;
    assert_eq!(active.load(AtomicOrdering::SeqCst), charged);
    drop(result);
    assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
    Ok(())
}

#[test]
fn root_observation_refuses_zero_nonce_foreign_selection_and_bad_digest()
-> Result<(), Box<dyn std::error::Error>> {
    let expected = observation()?;
    let selection = ViewSelection::Exact(expected.view.identity);
    for case in 0..5 {
        let mut wrong = expected.clone();
        match case {
            0 => wrong.root_nonce = Digest([0; 32]),
            1 => {
                wrong.view.identity.workspace.slot = SlotId([99; 16]);
                wrong.view.identity.digest = wrong.view.identity_digest()?;
            }
            2 => {
                wrong.view.identity.epoch += 1;
                wrong.view.identity.digest = wrong.view.identity_digest()?;
            }
            3 => wrong.view.identity.digest.0[0] ^= 1,
            4 => {}
            _ => unreachable!(),
        }
        let response = if case == 4 {
            NativeIndexResponse::View(wrong.view)
        } else {
            NativeIndexResponse::RootScopedView(wrong)
        };
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let (result, _) = exchange(selection, Some(response.clone()), |client| {
            client.observe_root_view(selection, &budget)
        })?;
        assert!(
            matches!(result, Err(NativeUdsError::Protocol(_))),
            "ordinary case {case}"
        );
        #[cfg(target_os = "linux")]
        {
            let (memory, active, _) = publication_scope(false);
            let (result, _) = exchange(selection, Some(response), |client| {
                client.observe_root_view_admitted(selection, &budget, &memory)
            })?;
            assert!(
                matches!(result, Err(NativeUdsError::Protocol(_))),
                "admitted case {case}"
            );
            assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
        }
    }
    Ok(())
}

#[test]
fn root_observation_expiry_cancel_and_memory_denial_connect_no_socket()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("no-connect.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let client = NativeUdsClient::new(socket);
    let selection = ViewSelection::Exact(observation()?.view.identity);
    let expired = RequestBudgetV1::until(Instant::now() - Duration::from_secs(1));
    let cancelled = RequestBudgetV1::for_duration(Duration::from_secs(5));
    cancelled.cancel_handle().cancel();
    for budget in [&expired, &cancelled] {
        let (memory, active, admissions) = publication_scope(false);
        assert!(client.observe_root_view(selection, budget).is_err());
        assert!(
            client
                .observe_root_view_admitted(selection, budget, &memory)
                .is_err()
        );
        assert_eq!(admissions.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
        assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
    }
    let (memory, active, admissions) = publication_scope(true);
    let result = client.observe_root_view_admitted(
        selection,
        &RequestBudgetV1::for_duration(Duration::from_secs(5)),
        &memory,
    );
    assert!(result.is_err() && !matches!(result, Err(NativeUdsError::Unknown(_))));
    assert_eq!(admissions.load(AtomicOrdering::SeqCst), 1);
    assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
    assert!(matches!(listener.accept(),Err(e) if e.kind()==std::io::ErrorKind::WouldBlock));
    Ok(())
}

#[test]
fn root_observation_transport_loss_is_never_an_unknown_write()
-> Result<(), Box<dyn std::error::Error>> {
    let selection = ViewSelection::Exact(observation()?.view.identity);
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (result, _) = exchange(selection, None, |client| {
        client.observe_root_view(selection, &budget)
    })?;
    assert!(matches!(result, Err(NativeUdsError::Transport(_))));
    Ok(())
}

#[test]
fn root_observation_preserves_canonical_refusals_and_charged_error_reply()
-> Result<(), Box<dyn std::error::Error>> {
    let selection = ViewSelection::Exact(observation()?.view.identity);
    let error = NativeIndexWireError {
        code: NativeIndexErrorCode::Conflict,
        message: "fixed root observation refusal".into(),
    };
    let response = NativeIndexResponse::Error(error.clone());
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (result, _) = exchange(selection, Some(response.clone()), |client| {
        client.observe_root_view(selection, &budget)
    })?;
    assert!(matches!(result,Err(NativeUdsError::Refused(actual)) if actual==error));
    #[cfg(target_os = "linux")]
    {
        let (memory, active, _) = publication_scope(false);
        let (result, _) = exchange(selection, Some(response.clone()), |client| {
            client.observe_root_view_admitted(selection, &budget, &memory)
        })?;
        let result = result?;
        assert_eq!(result.response(), &response);
        assert!(active.load(AtomicOrdering::SeqCst) > 0);
        drop(result);
        assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
    }
    Ok(())
}

// Classifier coverage is distinct from the IPC actual-process regressions.
#[test]
fn prewrite_peer_refusal_is_not_submit_unknown_v1() {
    use quanta_index_ipc::PeerBindingRefusalV1 as Refusal;
    let operation = key(93);
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(3));
    for refusal in [
        Refusal::Unsupported,
        Refusal::UnsupportedRoute,
        Refusal::ProcessExited,
        Refusal::ForeignProcess,
        Refusal::InvalidKernelCredentials,
        Refusal::Kernel {
            operation: "poll",
            errno: 5,
        },
    ] {
        let result = classify_submit_result(
            Err(NativeUdsError::PeerBindingRefused(refusal)),
            operation,
            &budget,
        );
        assert!(
            matches!(result, Err(NativeUdsError::PeerBindingRefused(actual)) if actual == refusal)
        );
        assert!(
            matches!(classify_write_stage_result(operation, &budget, Err(NativeUdsError::PeerBindingRefused(refusal))),
            Err(NativeUdsError::PeerBindingRefused(actual)) if actual == refusal)
        );
        assert!(
            matches!(admitted_write_error_v1(operation, NativeUdsError::PeerBindingRefused(refusal)),
            NativeUdsError::PeerBindingRefused(actual) if actual == refusal)
        );
    }
    assert!(
        matches!(classify_submit_result(Err(NativeUdsError::Transport("response lost after send".into())), operation, &budget),
        Err(NativeUdsError::Unknown(actual)) if actual == operation)
    );
    assert!(
        matches!(admitted_write_error_v1(operation, NativeUdsError::Transport("response lost after send".into())),
        NativeUdsError::Unknown(actual) if actual == operation)
    );
}
