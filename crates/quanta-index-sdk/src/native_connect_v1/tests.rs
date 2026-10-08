use super::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

// A typed inline test double, not a shared allocator or Core/Source proof.
// Real shared-header construction and alias lifetime belong to the receiver.
struct InlineOwner(QuantaIndexClientPayloadV1);
impl Deref for InlineOwner {
    type Target = QuantaIndexClientPayloadV1;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
struct Funding {
    count: Rc<Cell<u32>>,
}
impl Drop for Funding {
    fn drop(&mut self) {
        self.count.set(0);
    }
}
#[derive(Clone, Copy, Default)]
enum BirthMode {
    #[default]
    Normal,
    Skip,
    Repeat,
    Lie,
    SkipHeader,
}
#[derive(Default)]
struct Admission {
    polls: u64,
    births: usize,
    headers: usize,
    alive: Rc<Cell<u32>>,
    fail_work: Option<Box<u8>>,
    early_path: Option<Box<u8>>,
    late_path: Option<Box<u8>>,
    early_header: Option<Box<u8>>,
    late_header: Option<Box<u8>>,
    final_work: Option<Box<u8>>,
    mode: BirthMode,
}
impl NativeSdkConnectAdmissionV1 for Admission {
    type OriginalError = Box<u8>;
    type Funding = Funding;
    type Shared = InlineOwner;
    fn consume_connect_work_v1(&mut self, _: u64) -> Result<(), Box<u8>> {
        self.polls = self.polls.checked_add(1).expect("fixture poll count");
        if let Some(cause) = self.fail_work.take() {
            return Err(cause);
        }
        if self.headers > 0
            && let Some(cause) = self.final_work.take()
        {
            return Err(cause);
        }
        Ok(())
    }
    fn admit_path_birth_v1(
        &mut self,
        _: usize,
        funding: &mut Option<Funding>,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Box<u8>> {
        if let Some(cause) = self.early_path.take() {
            return Err(cause);
        }
        if matches!(self.mode, BirthMode::Skip) {
            return Ok(false);
        }
        self.births = self.births.checked_add(1).expect("fixture birth count");
        let success = birth();
        if success {
            let bank = funding.get_or_insert_with(|| Funding {
                count: Rc::clone(&self.alive),
            });
            bank.count.set(1);
        }
        if matches!(self.mode, BirthMode::Repeat) && birth() {
            return Err(Box::new(250));
        }
        if let Some(cause) = self.late_path.take() {
            return Err(cause);
        }
        Ok(if matches!(self.mode, BirthMode::Lie) {
            !success
        } else {
            success
        })
    }
    fn birth_client_into_slots_v1(
        &mut self,
        payload: &mut Option<QuantaIndexClientPayloadV1>,
        shared: &mut Option<InlineOwner>,
        _: &mut Option<Funding>,
    ) -> Result<(), Box<u8>> {
        if let Some(cause) = self.early_header.take() {
            return Err(cause);
        }
        if matches!(self.mode, BirthMode::SkipHeader) {
            return Ok(());
        }
        self.headers = self.headers.checked_add(1).expect("fixture header count");
        *shared = payload.take().map(InlineOwner);
        if let Some(cause) = self.late_header.take() {
            return Err(cause);
        }
        Ok(())
    }
}
type Data = NativeSdkConnectDataV1<Box<u8>, Funding, InlineOwner>;

#[test]
fn native_policy_rejects_invalid_deadline_and_timeout_before_any_path_birth() {
    for elapsed in [false, true] {
        let mut data = Data::new_v1();
        let mut admission = Admission::default();
        let options = ConnectOptions::borrow_state_root_v1(Path::new("unused"));
        let options = if elapsed {
            options.with_request_io_deadline(
                Instant::now()
                    .checked_sub(Duration::from_secs(1))
                    .expect("past instant"),
            )
        } else {
            options.with_request_io_timeout(Duration::ZERO)
        };
        assert_eq!(
            try_connect_native_into_v1(options, ClientProfile::Full, &mut admission, &mut data),
            Err(NativeSdkConnectRefusalV1::OperationRefused)
        );
        assert!(
            matches!(data.failure_v1(), Some(NativeSdkConnectFailureV1::IoPolicy(cause)) if matches!((elapsed,cause), (true,IpcError::ClientIoDeadlineElapsed)|(false,IpcError::InvalidClientIoTimeout)))
        );
        assert_eq!((admission.births, admission.headers), (0, 0));
        assert!(data.paths.iter().all(|path| path.capacity() == 0));
        let polls = admission.polls;
        assert_eq!(
            try_connect_native_into_v1(
                ConnectOptions::borrow_state_root_v1(Path::new("retry")),
                ClientProfile::Full,
                &mut admission,
                &mut data
            ),
            Err(NativeSdkConnectRefusalV1::UsedData)
        );
        assert_eq!(admission.polls, polls);
    }
}

#[test]
fn native_refusal_keeps_full_noncopy_error_and_partial_paths() {
    for stage in 0..6 {
        let cause = Box::new(67);
        let pointer = std::ptr::from_ref(cause.as_ref());
        let mut admission = Admission::default();
        match stage {
            0 => admission.fail_work = Some(cause),
            1 => admission.early_path = Some(cause),
            2 => admission.late_path = Some(cause),
            3 => admission.early_header = Some(cause),
            4 => admission.late_header = Some(cause),
            _ => admission.final_work = Some(cause),
        }
        let mut data = Data::new_v1();
        assert_eq!(
            try_connect_native_into_v1(
                ConnectOptions::borrow_state_root_v1(Path::new("root")),
                ClientProfile::Full,
                &mut admission,
                &mut data
            ),
            Err(NativeSdkConnectRefusalV1::OperationRefused)
        );
        assert!(
            matches!(data.failure_v1(), Some(NativeSdkConnectFailureV1::Admission(cause)) if std::ptr::from_ref(cause.as_ref()) == pointer && **cause == 67)
        );
        assert!(data.client_v1().is_none());
        if stage == 2 {
            assert!(data.paths[0].capacity() > 0);
            assert!(data.paths[0].as_os_str().is_empty());
        }
        if stage == 3 {
            assert!(data.payload.is_some());
            assert!(data.shared.is_none());
        }
        if stage >= 4 {
            assert!(data.payload.is_none());
            assert!(data.shared.is_some());
        }
        let polls = admission.polls;
        let births = admission.births;
        assert_eq!(
            try_connect_native_into_v1(
                ConnectOptions::borrow_state_root_v1(Path::new("retry")),
                ClientProfile::Full,
                &mut admission,
                &mut data
            ),
            Err(NativeSdkConnectRefusalV1::UsedData)
        );
        assert_eq!((admission.polls, admission.births), (polls, births));
        let alive = Rc::clone(&admission.alive);
        drop(admission);
        if stage >= 2 {
            assert_eq!(alive.get(), 1);
        }
        drop(data);
        assert_eq!(alive.get(), 0);
    }
}

#[test]
fn native_path_callback_protocol_cannot_publish_an_unborn_client() {
    for stage in 0..4 {
        let mut admission = Admission::default();
        match stage {
            0 => admission.mode = BirthMode::Skip,
            1 => admission.mode = BirthMode::Repeat,
            2 => admission.mode = BirthMode::Lie,
            _ => admission.mode = BirthMode::SkipHeader,
        }
        let mut data = Data::new_v1();
        assert_eq!(
            try_connect_native_into_v1(
                ConnectOptions::borrow_state_root_v1(Path::new("root")),
                ClientProfile::Full,
                &mut admission,
                &mut data
            ),
            Err(NativeSdkConnectRefusalV1::OperationRefused)
        );
        assert!(matches!(
            data.failure_v1(),
            Some(NativeSdkConnectFailureV1::InvalidNativeProducer)
        ));
        assert!(data.client_v1().is_none());
    }
}

#[test]
fn physical_reserve_error_survives_a_later_original_admission_refusal() {
    let cause = Box::new(71);
    let pointer = std::ptr::from_ref(cause.as_ref());
    let mut admission = Admission {
        late_path: Some(cause),
        ..Admission::default()
    };
    let mut path = PathBuf::new();
    let mut reserve_failure = None;
    let mut funding = None;
    let result = reserve_path_v1(
        usize::MAX,
        &mut path,
        &mut reserve_failure,
        &mut funding,
        &mut admission,
    );
    assert!(
        matches!(result, Err(NativeSdkConnectFailureV1::Admission(cause)) if std::ptr::from_ref(cause.as_ref()) == pointer)
    );
    assert!(
        reserve_failure.is_some(),
        "actual TryReserveError must not be replaced by the later source cause"
    );
    assert_eq!(path.capacity(), 0);
}

#[test]
fn query_only_never_builds_unused_planes_and_output_transfer_is_pure() {
    let mut admission = Admission::default();
    let mut data = Data::new_v1();
    let deadline = Instant::now() + Duration::from_secs(5);
    let options = ConnectOptions::borrow_state_root_v1(Path::new("missing-root"))
        .with_borrowed_control_socket_v1(Path::new("ignored-control"))
        .with_borrowed_ingest_socket_v1(Path::new("ignored-ingest"))
        .with_request_io_deadline(deadline);
    assert!(
        try_connect_native_into_v1(options, ClientProfile::QueryOnly, &mut admission, &mut data)
            .is_ok()
    );
    assert_eq!((admission.births, admission.headers), (1, 1));
    let client = data.client_v1().expect("producer-complete client");
    assert_eq!(client.test_next_request_id(), 1);
    assert_eq!(client.test_next_request_id(), 2);
    assert!(matches!(
        client.generations().status(
            crate::RepoId::new("r").unwrap(),
            crate::RevisionId::new("v").unwrap()
        ),
        Err(crate::SdkError::PlaneUnavailable { plane: "control" })
    ));
    let mut output = None;
    assert!(data.complete_into_slot_v1(&mut output).is_ok());
    let pointer = std::ptr::from_ref(&output.as_ref().expect("client").0);
    assert_eq!(
        data.complete_into_slot_v1(&mut output),
        Err(NativeSdkConnectRefusalV1::OccupiedOutput)
    );
    assert_eq!(
        std::ptr::from_ref(&output.as_ref().expect("client").0),
        pointer
    );
    let alive = Rc::clone(&admission.alive);
    drop(admission);
    assert_eq!(alive.get(), 1);
    drop(output);
    assert_eq!(
        alive.get(),
        1,
        "external funding survives successful transfer and client destruction"
    );
    drop(data);
    assert_eq!(alive.get(), 0);
}

#[test]
fn missing_full_profile_socket_keeps_earlier_path_and_exact_failure() {
    let mut data = Data::new_v1();
    let mut admission = Admission::default();
    let options = ConnectOptions::borrow_query_socket_v1(Path::new("query.sock"));
    assert_eq!(
        try_connect_native_into_v1(options, ClientProfile::Full, &mut admission, &mut data),
        Err(NativeSdkConnectRefusalV1::OperationRefused)
    );
    assert_eq!(data.paths[0], Path::new("query.sock"));
    assert!(matches!(
        data.failure_v1(),
        Some(NativeSdkConnectFailureV1::SocketUnresolved(
            "control socket unresolved: set state root or explicit control socket"
        ))
    ));
    assert_eq!((admission.births, admission.headers), (1, 0));
}

#[test]
fn admitted_paths_match_std_path_semantics_including_non_unicode_unix_bytes() {
    use std::os::unix::ffi::OsStringExt as _;
    let raw = PathBuf::from(std::ffi::OsString::from_vec(b"root-\xff".to_vec()));
    let roots = [
        Path::new(""),
        Path::new("/"),
        Path::new("root/"),
        Path::new("root//"),
        Path::new("root/.."),
        raw.as_path(),
    ];
    for input in roots {
        for source in [SocketSourceV1::Explicit, SocketSourceV1::StateRoot] {
            for kind in [
                SocketKindV1::Query,
                SocketKindV1::Control,
                SocketKindV1::Ingest,
            ] {
                let expected = if matches!(source, SocketSourceV1::Explicit) {
                    input.to_path_buf()
                } else {
                    input.join(kind.suffix_v1())
                };
                let mut path = PathBuf::new();
                let mut failure = None;
                let mut funding = None;
                let mut admission = Admission::default();
                let bytes = socket_path_capacity_v1(input, source, kind).expect("small path");
                assert!(
                    reserve_path_v1(bytes, &mut path, &mut failure, &mut funding, &mut admission)
                        .is_ok()
                );
                fill_socket_path_v1(input, source, kind, &mut path);
                assert_eq!(
                    path.as_os_str().as_encoded_bytes(),
                    expected.as_os_str().as_encoded_bytes()
                );
                assert_eq!(
                    path.capacity(),
                    bytes,
                    "canonical fill must not allocate after admission"
                );
            }
        }
    }
}

#[test]
fn elapsed_absolute_deadline_is_not_restarted_after_native_construction() {
    let deadline = Instant::now() + Duration::from_millis(30);
    let options = ConnectOptions::borrow_state_root_v1(Path::new("/missing-native-connect-root"))
        .with_request_io_deadline(deadline);
    let mut admission = Admission::default();
    let mut data = Data::new_v1();
    assert!(
        try_connect_native_into_v1(options, ClientProfile::Full, &mut admission, &mut data).is_ok()
    );
    while Instant::now() <= deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    let result = data
        .client_v1()
        .expect("producer-complete client")
        .observability()
        .metrics_snapshot();
    assert!(
        matches!(
            result,
            Err(crate::SdkError::Transport(
                IpcError::ClientIoDeadlineElapsed
            ))
        ),
        "must reject before trying the missing socket: {result:?}"
    );
}

fn serve_once<R, S, F>(
    listener: std::os::unix::net::UnixListener,
    response: F,
) -> std::thread::JoinHandle<Result<(), IpcError>>
where
    R: serde::de::DeserializeOwned,
    S: serde::Serialize,
    F: FnOnce(R) -> S + Send + 'static,
{
    std::thread::spawn(move || {
        use std::io::Write as _;
        listener.set_nonblocking(true).map_err(IpcError::Io)?;
        let deadline = Instant::now()
            .checked_add(Duration::from_secs(2))
            .ok_or_else(|| IpcError::Io(std::io::Error::other("fixture deadline overflow")))?;
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(cause)
                    if cause.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(cause) => return Err(IpcError::Io(cause)),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .map_err(IpcError::Io)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(1)))
            .map_err(IpcError::Io)?;
        let request = quanta_index_ipc::decode_request(&mut stream)?;
        let bytes = quanta_index_ipc::encode_response(&response(request))?;
        stream.write_all(&bytes).map_err(IpcError::Io)
    })
}

#[test]
fn native_payload_uses_same_three_uds_routes_codec_binding_and_request_counter() {
    use quanta_index_contract::*;
    use std::os::unix::net::UnixListener;
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    std::fs::create_dir(root.join("search-plane")).unwrap();
    let query = UnixListener::bind(root.join("search-plane/query.sock")).unwrap();
    let control_path = root.join("control-override.sock");
    let control = UnixListener::bind(&control_path).unwrap();
    let ingest = UnixListener::bind(root.join("search-plane/ingest.sock")).unwrap();
    let refusal = |message: &str| SearchPlaneIpcError {
        code: SearchPlaneErrorCodeV2::InvalidRequest,
        message: message.to_owned(),
        repair: None,
    };
    let query_error = refusal("query-route");
    let control_error = refusal("control-route");
    let ingest_error = refusal("ingest-route");
    let query_server = serve_once(query, move |request: SearchPlaneQueryIpcRequestEnvelope| {
        assert_eq!(request.request_id, 1);
        assert!(matches!(
            request.payload,
            SearchPlaneQueryIpcRequest::Text(_)
        ));
        SearchPlaneQueryIpcResponseEnvelope {
            request_id: request.request_id,
            payload: SearchPlaneQueryIpcResponse::Error(query_error),
        }
    });
    let control_server = serve_once(
        control,
        move |request: SearchPlaneControlIpcRequestEnvelope| {
            assert_eq!(request.request_id, 2);
            assert!(matches!(
                request.payload,
                SearchPlaneControlIpcRequest::MetricsSnapshot(_)
            ));
            SearchPlaneControlIpcResponseEnvelope {
                request_id: request.request_id,
                payload: SearchPlaneControlIpcResponse::Error(control_error),
            }
        },
    );
    let ingest_server = serve_once(
        ingest,
        move |request: SearchPlaneIngestIpcRequestEnvelope| {
            assert_eq!(request.request_id, 3);
            assert!(matches!(
                request.payload,
                SearchPlaneIngestIpcRequest::PublishDirtyBatch(_)
            ));
            SearchPlaneIngestIpcResponseEnvelope {
                request_id: request.request_id,
                payload: SearchPlaneIngestIpcResponse::Error(ingest_error),
            }
        },
    );
    let mut admission = Admission::default();
    let mut data = Data::new_v1();
    let options = ConnectOptions::borrow_state_root_v1(root)
        .with_borrowed_control_socket_v1(&control_path)
        .with_request_io_timeout(Duration::from_secs(1));
    assert!(
        try_connect_native_into_v1(options, ClientProfile::Full, &mut admission, &mut data).is_ok()
    );
    let client = data.client_v1().expect("producer-complete client");
    let repo = RepoId::new("repo").unwrap();
    let revision = RevisionId::new("revision").unwrap();
    let pin = GenerationPin {
        repo_id: repo.clone(),
        revision_id: revision.clone(),
        manifest_generation: ManifestGeneration::new(1),
    };
    let query_result = client
        .lexical()
        .query()
        .native("sample")
        .pinned(pin)
        .top_k(1)
        .execute();
    let control_result = client.observability().metrics_snapshot();
    let ingest_result = client.runtime().publish_dirty(
        &crate::DirtyBatch::new(repo, revision, ManifestGeneration::new(1), 1)
            .delete(ChunkId::new("doc:removed")),
    );
    // Join all bounded servers before assertions, including on SDK failure.
    assert!(query_server.join().expect("query server").is_ok());
    assert!(control_server.join().expect("control server").is_ok());
    assert!(ingest_server.join().expect("ingest server").is_ok());
    assert!(
        matches!(query_result,Err(crate::SdkError::Remote {message,..}) if message == "query-route")
    );
    assert!(
        matches!(control_result,Err(crate::SdkError::Remote {message,..}) if message == "control-route")
    );
    assert!(
        matches!(ingest_result,Err(crate::SdkError::Remote {message,..}) if message == "ingest-route")
    );
}
