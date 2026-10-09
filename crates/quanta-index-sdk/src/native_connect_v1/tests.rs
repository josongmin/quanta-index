use super::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

// A typed inline test double, not a shared allocator or Core/Source proof.
// Real shared-header construction and alias lifetime belong to the receiver.
struct InlineOwner {
    payload: QuantaIndexClientPayloadV1,
    _funding: Funding,
}
impl Deref for InlineOwner {
    type Target = QuantaIndexClientPayloadV1;
    fn deref(&self) -> &Self::Target {
        &self.payload
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
    NoFunding,
    HeaderOnly,
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
    path_receipts: [Option<bool>; 2],
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
        self.path_receipts[0] = Some(success);
        let receipt = if matches!(self.mode, BirthMode::Repeat) {
            let receipt = birth();
            self.path_receipts[1] = Some(receipt);
            receipt
        } else {
            success
        };
        if receipt && !matches!(self.mode, BirthMode::NoFunding) {
            let bank = funding.get_or_insert_with(|| Funding {
                count: Rc::clone(&self.alive),
            });
            bank.count.set(1);
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
        funding: &mut Option<Funding>,
    ) -> Result<(), Box<u8>> {
        if let Some(cause) = self.early_header.take() {
            return Err(cause);
        }
        if matches!(self.mode, BirthMode::SkipHeader) {
            return Ok(());
        }
        if matches!(self.mode, BirthMode::HeaderOnly) && funding.is_none() {
            *funding = Some(Funding {
                count: Rc::clone(&self.alive),
            });
            self.alive.set(1);
        }
        self.headers = self.headers.checked_add(1).expect("fixture header count");
        if payload.is_some() && funding.is_some() {
            *shared = Some(InlineOwner {
                payload: payload.take().expect("checked SDK payload"),
                _funding: funding.take().expect("checked actual funding"),
            });
        }
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
            assert!(data.funding.is_some());
        }
        if stage >= 4 {
            assert!(data.payload.is_none());
            assert!(data.shared.is_some());
            assert!(data.funding.is_none());
            assert!(data.complete_shared_v1().is_none());
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
    let pointer = std::ptr::from_ref(&output.as_ref().expect("client").payload);
    assert_eq!(
        data.complete_into_slot_v1(&mut output),
        Err(NativeSdkConnectRefusalV1::OccupiedOutput)
    );
    assert_eq!(
        std::ptr::from_ref(&output.as_ref().expect("client").payload),
        pointer
    );
    let alive = Rc::clone(&admission.alive);
    drop(admission);
    assert_eq!(alive.get(), 1);
    drop(data);
    assert_eq!(
        alive.get(),
        1,
        "funding moved into the handle survives source DATA destruction"
    );
    drop(output);
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

// Source-only diagnostic probes. These mocks exercise SDK protocol/custody;
// they do not qualify genuine Core handles, funding, or Original Source.
#[test]
fn repeated_success_preserves_first_commit_receipt_and_external_funding() {
    let mut admission = Admission {
        mode: BirthMode::Repeat,
        ..Admission::default()
    };
    let alive = Rc::clone(&admission.alive);
    let mut path = PathBuf::new();
    let mut failure = None;
    let mut funding = None;
    assert!(matches!(
        reserve_path_v1(9, &mut path, &mut failure, &mut funding, &mut admission),
        Err(NativeSdkConnectFailureV1::InvalidNativeProducer)
    ));
    assert_eq!(admission.path_receipts, [Some(true), Some(true)]);
    assert_eq!(admission.births, 1);
    assert_eq!(path.capacity(), 9);
    assert!(path.as_os_str().is_empty());
    assert!(failure.is_none());
    assert_eq!(
        alive.get(),
        1,
        "live backing keeps its first commit funding"
    );
    drop(path);
    assert_eq!(alive.get(), 1);
    drop(funding);
    assert_eq!(alive.get(), 0);
}

#[test]
fn repeated_reserve_failure_preserves_first_false_and_full_native_error() {
    let expected = PathBuf::new()
        .try_reserve_exact(usize::MAX)
        .expect_err("capacity overflow");
    let mut admission = Admission {
        mode: BirthMode::Repeat,
        ..Admission::default()
    };
    let mut path = PathBuf::new();
    let mut failure = None;
    let mut funding = None;
    assert!(matches!(
        reserve_path_v1(
            usize::MAX,
            &mut path,
            &mut failure,
            &mut funding,
            &mut admission,
        ),
        Err(NativeSdkConnectFailureV1::InvalidNativeProducer)
    ));
    assert_eq!(admission.path_receipts, [Some(false), Some(false)]);
    assert_eq!(failure.as_ref(), Some(&expected));
    assert_eq!(path.capacity(), 0);
    assert!(funding.is_none());
}

#[test]
fn zero_path_reserve_never_calls_admission_or_changes_external_slots() {
    let mut admission = Admission {
        mode: BirthMode::Repeat,
        ..Admission::default()
    };
    let mut path = PathBuf::new();
    let mut failure = None;
    let mut funding = None;
    assert!(reserve_path_v1(0, &mut path, &mut failure, &mut funding, &mut admission).is_ok());
    assert_eq!(admission.path_receipts, [None, None]);
    assert_eq!(
        (admission.polls, admission.births, admission.headers),
        (0, 0, 0)
    );
    assert_eq!(path.capacity(), 0);
    assert!(failure.is_none());
    assert!(funding.is_none());
}

#[test]
fn dishonest_reports_refuse_without_erasing_physical_state_or_error() {
    for (bytes, mode, receipt, capacity) in [
        (9, BirthMode::Skip, None, 0),
        (9, BirthMode::Lie, Some(true), 9),
        (usize::MAX, BirthMode::Lie, Some(false), 0),
    ] {
        let mut admission = Admission {
            mode,
            ..Admission::default()
        };
        let mut path = PathBuf::new();
        let mut failure = None;
        let mut funding = None;
        assert!(matches!(
            reserve_path_v1(bytes, &mut path, &mut failure, &mut funding, &mut admission),
            Err(NativeSdkConnectFailureV1::InvalidNativeProducer)
        ));
        assert_eq!(admission.path_receipts, [receipt, None]);
        assert_eq!(path.capacity(), capacity);
        assert_eq!(funding.is_some(), receipt == Some(true));
        assert_eq!(failure.is_some(), receipt == Some(false));
        drop(path);
        drop(funding);
    }
}

#[test]
fn late_admission_wins_without_erasing_first_receipt_or_native_error() {
    for bytes in [9, usize::MAX] {
        let cause = Box::new(109_u8);
        let pointer = std::ptr::from_ref(cause.as_ref());
        let mut admission = Admission {
            mode: BirthMode::Repeat,
            late_path: Some(cause),
            ..Admission::default()
        };
        let mut path = PathBuf::new();
        let mut failure = None;
        let mut funding = None;
        assert!(matches!(
            reserve_path_v1(bytes, &mut path, &mut failure, &mut funding, &mut admission),
            Err(NativeSdkConnectFailureV1::Admission(cause))
                if std::ptr::from_ref(cause.as_ref()) == pointer
        ));
        let success = bytes == 9;
        assert_eq!(admission.path_receipts, [Some(success), Some(success)]);
        assert_eq!(path.capacity(), if success { 9 } else { 0 });
        assert_eq!(failure.is_some(), !success);
        assert_eq!(funding.is_some(), success);
        drop(path);
        drop(funding);
    }
}

// Intentionally has no Deref implementation. This is an inline type witness,
// not the genuine future funded Core handle or its admitted payload read.
struct OpaqueOwner {
    _payload: QuantaIndexClientPayloadV1,
    _funding: Funding,
}
#[derive(Default)]
struct OpaqueAdmission {
    inner: Admission,
}
impl NativeSdkConnectAdmissionV1 for OpaqueAdmission {
    type OriginalError = Box<u8>;
    type Funding = Funding;
    type Shared = OpaqueOwner;
    fn consume_connect_work_v1(&mut self, units: u64) -> Result<(), Box<u8>> {
        self.inner.consume_connect_work_v1(units)
    }
    fn admit_path_birth_v1(
        &mut self,
        bytes: usize,
        funding: &mut Option<Funding>,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Box<u8>> {
        self.inner.admit_path_birth_v1(bytes, funding, birth)
    }
    fn birth_client_into_slots_v1(
        &mut self,
        payload: &mut Option<QuantaIndexClientPayloadV1>,
        shared: &mut Option<OpaqueOwner>,
        funding: &mut Option<Funding>,
    ) -> Result<(), Box<u8>> {
        if matches!(self.inner.mode, BirthMode::SkipHeader) {
            return Ok(());
        }
        self.inner.headers = self
            .inner
            .headers
            .checked_add(1)
            .expect("fixture header count");
        if payload.is_some() && funding.is_some() {
            *shared = Some(OpaqueOwner {
                _payload: payload.take().expect("checked SDK payload"),
                _funding: funding.take().expect("checked actual funding"),
            });
        }
        if let Some(cause) = self.inner.late_header.take() {
            return Err(cause);
        }
        Ok(())
    }
}
type OpaqueData = NativeSdkConnectDataV1<Box<u8>, Funding, OpaqueOwner>;

#[test]
fn opaque_shared_completion_borrows_only_complete_handles_and_preserves_slots() {
    let mut admission = OpaqueAdmission::default();
    let mut data = OpaqueData::new_v1();
    assert!(!data.is_complete_v1());
    assert!(data.complete_shared_v1().is_none());
    try_connect_native_into_v1(
        ConnectOptions::borrow_state_root_v1(Path::new("root")),
        ClientProfile::QueryOnly,
        &mut admission,
        &mut data,
    )
    .expect("opaque handle needs no Deref");
    assert!(data.is_complete_v1());
    let pointer = std::ptr::from_ref(data.complete_shared_v1().expect("completed opaque handle"));

    let mut other = OpaqueData::new_v1();
    let mut other_admission = OpaqueAdmission::default();
    try_connect_native_into_v1(
        ConnectOptions::borrow_state_root_v1(Path::new("other-root")),
        ClientProfile::QueryOnly,
        &mut other_admission,
        &mut other,
    )
    .expect("other opaque handle");
    let mut output = None;
    other
        .complete_into_slot_v1(&mut output)
        .expect("pure transfer");
    let output_pointer = std::ptr::from_ref(output.as_ref().expect("prior opaque output"));
    assert_eq!(
        data.complete_into_slot_v1(&mut output),
        Err(NativeSdkConnectRefusalV1::OccupiedOutput)
    );
    assert_eq!(
        std::ptr::from_ref(data.complete_shared_v1().expect("unchanged opaque handle")),
        pointer
    );
    assert_eq!(
        std::ptr::from_ref(output.as_ref().expect("prior output")),
        output_pointer
    );
    drop(output.take());
    data.complete_into_slot_v1(&mut output)
        .expect("pure opaque transfer");
    assert!(!data.is_complete_v1());
    assert!(data.complete_shared_v1().is_none());

    for skipped_header in [false, true] {
        let mut data = OpaqueData::new_v1();
        let mut admission = OpaqueAdmission::default();
        if skipped_header {
            admission.inner.mode = BirthMode::SkipHeader;
        } else {
            admission.inner.late_header = Some(Box::new(113));
        }
        assert_eq!(
            try_connect_native_into_v1(
                ConnectOptions::borrow_state_root_v1(Path::new("refused-root")),
                ClientProfile::QueryOnly,
                &mut admission,
                &mut data,
            ),
            Err(NativeSdkConnectRefusalV1::OperationRefused)
        );
        assert!(!data.is_complete_v1());
        assert!(data.complete_shared_v1().is_none());
        assert_eq!(data.shared.is_some(), !skipped_header);
        let original = std::ptr::from_ref(data.failure_v1().expect("original refusal"));
        assert_eq!(
            data.complete_into_slot_v1(&mut output),
            Err(NativeSdkConnectRefusalV1::OccupiedOutput)
        );
        assert_eq!(
            std::ptr::from_ref(data.failure_v1().expect("preserved refusal")),
            original
        );
    }
}

struct UnfundedOwner {
    _payload: QuantaIndexClientPayloadV1,
}

#[derive(Default)]
struct RetainedFundingAdmission {
    inner: Admission,
}

impl NativeSdkConnectAdmissionV1 for RetainedFundingAdmission {
    type OriginalError = Box<u8>;
    type Funding = Funding;
    type Shared = UnfundedOwner;

    fn consume_connect_work_v1(&mut self, units: u64) -> Result<(), Box<u8>> {
        self.inner.consume_connect_work_v1(units)
    }

    fn admit_path_birth_v1(
        &mut self,
        bytes: usize,
        funding: &mut Option<Funding>,
        birth: &mut dyn FnMut() -> bool,
    ) -> Result<bool, Box<u8>> {
        self.inner.admit_path_birth_v1(bytes, funding, birth)
    }

    fn birth_client_into_slots_v1(
        &mut self,
        payload: &mut Option<QuantaIndexClientPayloadV1>,
        shared: &mut Option<UnfundedOwner>,
        _: &mut Option<Funding>,
    ) -> Result<(), Box<u8>> {
        self.inner.headers = self
            .inner
            .headers
            .checked_add(1)
            .expect("fixture header count");
        *shared = payload
            .take()
            .map(|payload| UnfundedOwner { _payload: payload });
        Ok(())
    }
}

#[test]
fn host_cannot_publish_handle_while_funding_remains_in_data() {
    let mut admission = RetainedFundingAdmission::default();
    let alive = Rc::clone(&admission.inner.alive);
    let mut data = NativeSdkConnectDataV1::new_v1();
    assert_eq!(
        try_connect_native_into_v1(
            ConnectOptions::borrow_state_root_v1(Path::new("root")),
            ClientProfile::QueryOnly,
            &mut admission,
            &mut data,
        ),
        Err(NativeSdkConnectRefusalV1::OperationRefused)
    );
    assert!(matches!(
        data.failure_v1(),
        Some(NativeSdkConnectFailureV1::InvalidNativeProducer)
    ));
    assert!(
        data.shared.is_some(),
        "the physical candidate stays in DATA"
    );
    assert!(data.funding.is_some(), "the actual bank stays in DATA");
    assert!(!data.is_complete_v1());
    assert!(data.complete_shared_v1().is_none());
    let mut output = None;
    assert_eq!(
        data.complete_into_slot_v1(&mut output),
        Err(NativeSdkConnectRefusalV1::MissingResult)
    );
    assert!(output.is_none());
    drop(admission);
    assert_eq!(alive.get(), 1);
    drop(data);
    assert_eq!(alive.get(), 0);
}

#[test]
fn host_cannot_publish_without_a_path_funding_bank() {
    let mut admission = Admission {
        mode: BirthMode::NoFunding,
        ..Admission::default()
    };
    let mut data = Data::new_v1();
    assert_eq!(
        try_connect_native_into_v1(
            ConnectOptions::borrow_state_root_v1(Path::new("root")),
            ClientProfile::QueryOnly,
            &mut admission,
            &mut data,
        ),
        Err(NativeSdkConnectRefusalV1::OperationRefused)
    );
    assert!(matches!(
        data.failure_v1(),
        Some(NativeSdkConnectFailureV1::InvalidNativeProducer)
    ));
    assert_eq!(admission.headers, 0);
    assert!(data.paths[0].capacity() > 0);
    assert!(data.paths[0].as_os_str().is_empty());
    assert!(data.payload.is_none());
    assert!(data.shared.is_none());
    assert!(data.funding.is_none());
    assert!(!data.is_complete_v1());
    assert!(data.complete_shared_v1().is_none());
}

#[test]
fn zero_byte_explicit_path_can_be_funded_by_header_birth() {
    let mut admission = Admission {
        mode: BirthMode::HeaderOnly,
        ..Admission::default()
    };
    let alive = Rc::clone(&admission.alive);
    let mut data = Data::new_v1();
    assert!(
        try_connect_native_into_v1(
            ConnectOptions::borrow_query_socket_v1(Path::new("")),
            ClientProfile::QueryOnly,
            &mut admission,
            &mut data,
        )
        .is_ok()
    );
    assert_eq!((admission.births, admission.headers), (0, 1));
    assert!(data.is_complete_v1());
    let mut output = None;
    data.complete_into_slot_v1(&mut output)
        .expect("header-funded completion");
    drop(admission);
    drop(data);
    assert_eq!(alive.get(), 1);
    drop(output);
    assert_eq!(alive.get(), 0);
}
