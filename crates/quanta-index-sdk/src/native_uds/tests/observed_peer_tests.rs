//! Real Linux process and UDS proof for the SDK's observed read-only peer.

use std::{
    io::{ErrorKind, Read, Write},
    os::unix::net::UnixListener,
    path::Path,
    process::{Child, Command, Stdio},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use quanta_index_contract::{
    index::{CatalogView, Digest, MapRoot, ObjectRef, SourceCut, ViewRef, ViewSelection},
    native_protocol::{
        NativeIndexRequest, NativeIndexRequestEnvelope, NativeIndexResponse,
        NativeIndexResponseEnvelope,
    },
};
use quanta_index_ipc::{PeerBindingRefusalV1, RequestBudgetV1};

use super::{key, publication_scope};
use crate::native_uds::{NativeUdsClient, NativeUdsError};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            drop(self.0.kill());
            drop(self.0.wait());
        }
    }
}

fn child_test_selector() -> TestResult<String> {
    let module = module_path!()
        .split_once("::")
        .ok_or("test module path missing")?
        .1;
    Ok(format!("{module}::observed_peer_child"))
}

fn spawn_child(socket: &Path) -> TestResult<ChildGuard> {
    let selector = child_test_selector()?;
    let mut child = ChildGuard(
        Command::new(std::env::current_exe()?)
            .args(["--exact", &selector, "--ignored", "--nocapture"])
            .env("QUANTA_SDK_OBSERVED_PEER_SOCKET_V1", socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    while !socket.exists() {
        if child.0.try_wait()?.is_some() {
            return Err("observed peer child exited before listen".into());
        }
        if Instant::now() >= deadline {
            return Err("observed peer child listen deadline".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(child)
}

fn wait_child(child: &mut ChildGuard) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.0.try_wait()? {
            if !status.success() {
                let mut diagnostic = String::new();
                if let Some(mut stderr) = child.0.stderr.take() {
                    let _read = stderr.read_to_string(&mut diagnostic)?;
                }
                return Err(format!("observed peer child failed: {status}: {diagnostic}").into());
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("observed peer child exit deadline".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn fixed_view() -> TestResult<CatalogView> {
    let empty = MapRoot(None);
    let mut view = CatalogView {
        identity: ViewRef {
            workspace: key(1).workspace,
            epoch: 1,
            sequence: 1,
            digest: Digest([0; 32]),
        },
        profile: ObjectRef(Digest([7; 32])),
        source: SourceCut {
            owners: empty,
            paths: empty,
            frontiers: empty,
        },
        projections: empty,
        members: empty,
        coverage: empty,
    };
    view.identity.digest = view.identity_digest()?;
    Ok(view)
}

#[test]
fn sdk_observed_peer_read_uses_actual_child_then_refuses_mutation_before_connect() -> TestResult {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("observed-peer.sock");
    let mut child = spawn_child(&socket)?;
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(10));
    let (memory, active, _) = publication_scope(false);
    let peer = NativeUdsClient::new(&socket).capture_peer_process_admitted_v1(&budget, &memory)?;
    assert_eq!(peer.credentials_v1().pid, Some(child.0.id()));
    let lease = peer.share_lease_v1();
    drop(peer);
    assert!(active.load(Ordering::SeqCst) > 0);

    let client = NativeUdsClient::new(&socket).with_observed_query_peer_v1(&lease);
    let selection = ViewSelection::Latest {
        workspace: key(1).workspace,
        epoch: 1,
    };
    let reply = client.request(NativeIndexRequest::ReadView(selection), &budget)?;
    let expected_view = fixed_view()?;
    assert!(matches!(reply, NativeIndexResponse::View(view) if view == expected_view));
    for mutation in [
        NativeIndexRequest::CreateWorkspace {
            workspace: key(1).workspace,
            epoch: 1,
        },
        NativeIndexRequest::BeginInput(key(1)),
    ] {
        assert!(matches!(
            client.request(mutation, &budget),
            Err(NativeUdsError::PeerBindingRefused(
                PeerBindingRefusalV1::UnsupportedRoute
            ))
        ));
    }
    drop(client);
    drop(lease);
    assert_eq!(active.load(Ordering::SeqCst), 0);

    std::fs::write(socket.with_extension("release"), b"observed")?;
    wait_child(&mut child)?;
    assert_eq!(
        std::fs::read(socket.with_extension("report"))?,
        b"capture=0 query=1 extra=0"
    );
    Ok(())
}

// The child is a separate OS process. Its reply carries no PID or image claim.
#[test]
#[ignore = "internal process for observed SDK peer regression"]
fn observed_peer_child() -> TestResult {
    let socket = std::env::var_os("QUANTA_SDK_OBSERVED_PEER_SOCKET_V1")
        .ok_or("observed peer child socket absent")?;
    let socket = Path::new(&socket);
    let listener = UnixListener::bind(socket)?;
    let (mut capture, _) = listener.accept()?;
    capture.set_read_timeout(Some(Duration::from_secs(10)))?;
    let capture_bytes = capture.read(&mut [0_u8; 1])?;
    if capture_bytes != 0 {
        return Err("peer capture sent request bytes".into());
    }
    drop(capture);

    let (mut query, _) = listener.accept()?;
    query.set_read_timeout(Some(Duration::from_secs(10)))?;
    query.set_write_timeout(Some(Duration::from_secs(10)))?;
    let request: NativeIndexRequestEnvelope = quanta_index_ipc::decode_request(&mut query)?;
    let selection = ViewSelection::Latest {
        workspace: key(1).workspace,
        epoch: 1,
    };
    if request.payload != NativeIndexRequest::ReadView(selection) {
        return Err("observed peer did not receive the exact ReadView envelope".into());
    }
    query.write_all(&quanta_index_ipc::encode_response(
        &NativeIndexResponseEnvelope {
            request_id: request.request_id,
            payload: NativeIndexResponse::View(fixed_view()?),
        },
    )?)?;
    if query.read(&mut [0_u8; 1])? != 0 {
        return Err("observed peer received another query envelope".into());
    }
    drop(query);

    let release = socket.with_extension("release");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !release.exists() {
        if Instant::now() >= deadline {
            return Err("parent did not release observed peer".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    listener.set_nonblocking(true)?;
    if !matches!(listener.accept(), Err(error) if error.kind() == ErrorKind::WouldBlock) {
        return Err("observed peer mutation opened a third connection".into());
    }
    std::fs::write(
        socket.with_extension("report"),
        b"capture=0 query=1 extra=0",
    )?;
    Ok(())
}
