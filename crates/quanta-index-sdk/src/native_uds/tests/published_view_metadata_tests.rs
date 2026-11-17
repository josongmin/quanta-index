//! Metadata DTO decoding is independent of history query admission.
use super::*;

fn metadata_reply() -> Result<OperationReply<CatalogView>, Box<dyn std::error::Error>> {
    let operation = key(77);
    let mut view = CatalogView {
        identity: ViewRef {
            workspace: operation.workspace,
            epoch: operation.epoch,
            sequence: 777,
            digest: Digest([0; 32]),
        },
        profile: ObjectRef(Digest([17; 32])),
        source: SourceCut {
            owners: MapRoot(Some(ObjectRef(Digest([11; 32])))),
            paths: MapRoot(Some(ObjectRef(Digest([12; 32])))),
            frontiers: MapRoot(Some(ObjectRef(Digest([13; 32])))),
        },
        projections: MapRoot(Some(ObjectRef(Digest([14; 32])))),
        members: MapRoot(Some(ObjectRef(Digest([15; 32])))),
        coverage: MapRoot(Some(ObjectRef(Digest([16; 32])))),
    };
    view.identity.digest = view.identity_digest()?;
    Ok(OperationReply {
        operation,
        payload: view,
    })
}

#[test]
fn metadata_admitted_decoder_preserves_operation_and_all_catalog_roots()
-> Result<(), Box<dyn std::error::Error>> {
    let expected = NativeIndexResponseEnvelope {
        request_id: 91,
        payload: NativeIndexResponse::PublishedViewMetadata(metadata_reply()?),
    };
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&expected, &mut bytes)?;
    // The canonical DTO codec independently establishes this wire shape;
    // the existing admitted decoder must retain every named field.
    let ordinary: NativeIndexResponseEnvelope = ciborium::de::from_reader(bytes.as_slice())?;
    assert_eq!(ordinary, expected);
    let admitted = admitted_reply_v1::decode(&bytes)?;
    assert_eq!(admitted, expected);
    Ok(())
}

// Reuse the existing publication_scope counter/funder fixture from this parent;
// no Original issuer or Catalog/storage behavior is mocked here.
fn exchange_metadata<R>(
    operation: OperationKey,
    response: Option<NativeIndexResponse>,
    call: impl FnOnce(&NativeUdsClient<std::path::PathBuf>) -> R,
) -> Result<(R, usize), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("metadata.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let peer = std::thread::spawn(move || -> Result<usize, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err("metadata client did not connect".into());
                    }
                    std::thread::sleep(Duration::from_millis(2));
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
        if request.payload != NativeIndexRequest::PublishedViewMetadata(operation) {
            return Err("metadata request changed the operation or acquired a view".into());
        }
        let Some(response) = response else {
            return Ok(0);
        };
        let frame = quanta_index_ipc::encode_response(&NativeIndexResponseEnvelope {
            request_id: request.request_id,
            payload: response,
        })
        .map_err(|error| error.to_string())?;
        let body = frame
            .len()
            .checked_sub(4)
            .ok_or("metadata response header missing")?;
        stream
            .write_all(&frame)
            .map_err(|error| error.to_string())?;
        Ok(body)
    });
    let result = call(&NativeUdsClient::new(socket));
    let bytes = peer.join().map_err(|_| "metadata peer panicked")??;
    Ok((result, bytes))
}

#[test]
fn metadata_ordinary_and_admitted_preserve_full_result_and_original_grants()
-> Result<(), Box<dyn std::error::Error>> {
    let expected = metadata_reply()?;
    let operation = expected.operation;
    let response = NativeIndexResponse::PublishedViewMetadata(expected.clone());
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (ordinary, _) = exchange_metadata(operation, Some(response.clone()), |client| {
        client.published_view_metadata(operation, &budget)
    })?;
    assert_eq!(ordinary?, expected.payload);
    let (memory, active, admissions) = publication_scope(false);
    let (admitted, body_bytes) = exchange_metadata(operation, Some(response.clone()), |client| {
        client.published_view_metadata_admitted(operation, &budget, &memory)
    })?;
    let admitted = admitted?;
    let charged = u64::try_from(body_bytes)? + admitted_reply_v1::decoded_retained_limit()?;
    assert_eq!(active.load(AtomicOrdering::SeqCst), charged);
    assert!(admissions.load(AtomicOrdering::SeqCst) >= 3);
    let mut serialized = Vec::new();
    ciborium::ser::into_writer(admitted.response(), &mut serialized)?;
    assert_eq!(admitted.response(), &response);
    assert_eq!(active.load(AtomicOrdering::SeqCst), charged);
    drop(admitted);
    assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
    Ok(())
}

#[test]
fn metadata_refuses_cross_operation_namespace_digest_and_unbound_view()
-> Result<(), Box<dyn std::error::Error>> {
    let valid = metadata_reply()?;
    for case in 0..6 {
        let mut wrong = valid.clone();
        match case {
            0 => wrong.operation.producer = ProducerId([99; 16]),
            1 => {
                wrong.payload.identity.workspace.slot = SlotId([99; 16]);
                wrong.payload.identity.digest = wrong.payload.identity_digest()?;
            }
            2 => {
                wrong.payload.identity.epoch += 1;
                wrong.payload.identity.digest = wrong.payload.identity_digest()?;
            }
            3 => wrong.payload.identity.digest.0[0] ^= 1,
            4 => wrong.payload.source.frontiers = MapRoot(None),
            5 => {}
            _ => unreachable!(),
        }
        let response = if case == 5 {
            NativeIndexResponse::View(wrong.payload)
        } else {
            NativeIndexResponse::PublishedViewMetadata(wrong)
        };
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let (ordinary, _) = exchange_metadata(valid.operation, Some(response.clone()), |client| {
            client.published_view_metadata(valid.operation, &budget)
        })?;
        assert!(
            matches!(ordinary, Err(NativeUdsError::Protocol(_))),
            "ordinary metadata case {case}"
        );
        let (memory, active, _) = publication_scope(false);
        let (admitted, _) = exchange_metadata(valid.operation, Some(response), |client| {
            client.published_view_metadata_admitted(valid.operation, &budget, &memory)
        })?;
        assert!(
            matches!(admitted, Err(NativeUdsError::Protocol(_))),
            "admitted metadata case {case}"
        );
        assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
    }
    Ok(())
}

#[test]
fn metadata_keeps_existing_typed_refusals_without_write_unknown()
-> Result<(), Box<dyn std::error::Error>> {
    for code in [
        NativeIndexErrorCode::SnapshotUnavailable,
        NativeIndexErrorCode::Conflict,
        NativeIndexErrorCode::CorruptObject,
    ] {
        let operation = key(77);
        let error = NativeIndexWireError {
            code,
            message: "retained operation metadata refused".into(),
        };
        let response = NativeIndexResponse::Error(error.clone());
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let (ordinary, _) = exchange_metadata(operation, Some(response.clone()), |client| {
            client.published_view_metadata(operation, &budget)
        })?;
        assert!(matches!(ordinary, Err(NativeUdsError::Refused(actual)) if actual == error));
        let (memory, active, _) = publication_scope(false);
        let (admitted, _) = exchange_metadata(operation, Some(response.clone()), |client| {
            client.published_view_metadata_admitted(operation, &budget, &memory)
        })?;
        let admitted = admitted?;
        assert_eq!(admitted.response(), &response);
        assert!(active.load(AtomicOrdering::SeqCst) > 0);
        drop(admitted);
        assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
    }
    Ok(())
}

#[test]
fn metadata_expired_and_cancelled_reads_admit_nothing_before_connect()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("metadata-no-connect.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let client = NativeUdsClient::new(socket);
    let operation = key(77);
    let expired = RequestBudgetV1::until(Instant::now() - Duration::from_secs(1));
    let cancelled = RequestBudgetV1::for_duration(Duration::from_secs(5));
    cancelled.cancel_handle().cancel();
    for budget in [&expired, &cancelled] {
        let (memory, active, admissions) = publication_scope(false);
        let ordinary = client.published_view_metadata(operation, budget);
        let admitted = client.published_view_metadata_admitted(operation, budget, &memory);
        let raw_admitted = client.request_publication_admitted(
            NativeIndexRequest::PublishedViewMetadata(operation),
            budget,
            &memory,
        );
        if std::ptr::eq(budget, &expired) {
            assert!(matches!(ordinary, Err(NativeUdsError::Transport(_))));
            assert!(matches!(admitted, Err(NativeUdsError::Transport(_))));
            assert!(matches!(raw_admitted, Err(NativeUdsError::Transport(_))));
        } else {
            assert!(
                matches!(ordinary, Err(NativeUdsError::Refused(ref e)) if e.code == NativeIndexErrorCode::Cancelled)
            );
            assert!(
                matches!(admitted, Err(NativeUdsError::Refused(ref e)) if e.code == NativeIndexErrorCode::Cancelled)
            );
            assert!(
                matches!(raw_admitted, Err(NativeUdsError::Refused(ref e)) if e.code == NativeIndexErrorCode::Cancelled)
            );
        }
        assert_eq!(admissions.load(AtomicOrdering::SeqCst), 0);
        assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
        assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    }
    Ok(())
}

#[test]
fn metadata_memory_refusal_precedes_connect() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("metadata-memory-denied.sock");
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let client = NativeUdsClient::new(socket);
    let (memory, active, admissions) = publication_scope(true);
    let result = client.published_view_metadata_admitted(
        key(77),
        &RequestBudgetV1::for_duration(Duration::from_secs(5)),
        &memory,
    );
    assert!(result.is_err() && !matches!(result, Err(NativeUdsError::Unknown(_))));
    assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
    assert_eq!(admissions.load(AtomicOrdering::SeqCst), 1);
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    Ok(())
}

#[test]
fn metadata_read_ack_loss_is_not_an_unknown_write() -> Result<(), Box<dyn std::error::Error>> {
    let operation = key(77);
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (ordinary, _) = exchange_metadata(operation, None, |client| {
        client.published_view_metadata(operation, &budget)
    })?;
    assert!(matches!(ordinary, Err(NativeUdsError::Transport(_))));
    let (memory, active, _) = publication_scope(false);
    let (admitted, _) = exchange_metadata(operation, None, |client| {
        client.published_view_metadata_admitted(operation, &budget, &memory)
    })?;
    assert!(matches!(admitted, Err(NativeUdsError::Transport(_))));
    assert_eq!(active.load(AtomicOrdering::SeqCst), 0);
    Ok(())
}
