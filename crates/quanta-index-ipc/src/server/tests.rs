//! Transport server and peer-watch behavior regressions.

use super::{
    ClientIoPolicy, ConnectionCloseReason, IpcDispatcher, IpcError, IpcPlane, IpcServerCounters,
    PeerCredentials, PeerWatch, PeerWatchOutcome, RequestEnvelope, RequestEventStageV1,
    ResponseEnvelope, SocketPathIdentity, UdsServer, WatchEvent, WatchObserver,
    connect_before_deadline, connect_requires_completion_wait, create_connect_socket,
    decode_response, encode_request, handle_connection, send_request, send_request_observed,
    wait_for_connect,
};
use crate::socket_access::{PRIVATE_DIRECTORY_MODE, PRIVATE_SOCKET_MODE};
use rustix::fs::{OFlags, fcntl_getfl};
use rustix::io::{Errno, FdFlags, fcntl_getfd};
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_core::{RequestBudgetV1, RequestProviderStageV1};
use serde::de::{self, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::admission::{DispatchSlots, IngressBudget, ServerAdmissionPolicy, SlotRefusal};
use crate::error::MAX_FRAME_BODY_BYTES;

type TestRes = Result<(), String>;

fn test_counters() -> Arc<IpcServerCounters> {
    Arc::new(IpcServerCounters::for_plane("test"))
}

fn test_slots() -> DispatchSlots {
    DispatchSlots::for_policy(ServerAdmissionPolicy::DEFAULT)
}

fn test_policy() -> ServerAdmissionPolicy {
    ServerAdmissionPolicy::DEFAULT
}

#[test]
fn ingress_reader_rejects_an_elapsed_request_deadline() {
    let (mut stream, mut peer) = UnixStream::pair().expect("socket pair");
    peer.write_all(&[7]).expect("available input");
    let mut reader = super::IngressDeadlineReader {
        stream: &mut stream,
        deadline: Instant::now()
            .checked_sub(Duration::from_millis(1))
            .expect("current instant has a preceding millisecond"),
        saw_frame_bytes: false,
        first_read: true,
    };
    let error = reader.read(&mut [0]).expect_err("deadline precedes input");
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
}

#[test]
fn ingress_reader_uses_existing_first_timeout_and_tightens_fragment_timeout() {
    fn expect_timeout(
        socket_timeout: Duration,
        deadline_after: Duration,
        first_read: bool,
    ) -> TestRes {
        let (mut stream, peer) = UnixStream::pair().map_err(|err| err.to_string())?;
        stream
            .set_read_timeout(Some(socket_timeout))
            .map_err(|err| err.to_string())?;
        let (release_tx, release_rx) = mpsc::channel();
        let hold_peer = thread::spawn(move || {
            let _released = release_rx.recv_timeout(Duration::from_secs(5));
            drop(peer);
        });
        let mut reader = super::IngressDeadlineReader {
            stream: &mut stream,
            deadline: Instant::now() + deadline_after,
            saw_frame_bytes: !first_read,
            first_read,
        };
        let observed = reader.read(&mut [0_u8; 1]);
        let _released = release_tx.send(());
        hold_peer
            .join()
            .map_err(|panic| format!("peer hold thread panicked: {panic:?}"))?;
        if matches!(
            &observed,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                )
        ) {
            Ok(())
        } else {
            Err(format!(
                "read must time out before peer closes (socket={socket_timeout:?}, deadline={deadline_after:?}, first={first_read}): {observed:?}"
            ))
        }
    }

    let result = expect_timeout(Duration::from_millis(50), Duration::from_secs(30), true)
        .and_then(|()| expect_timeout(Duration::from_secs(30), Duration::from_millis(50), false));
    assert_test_ok(&result);
}

/// Failure-containment bound for watch-event waits: the test fails
/// instead of hanging when the watch never reports.
const WATCH_EVENT_BOUND: Duration = Duration::from_secs(30);

fn expect_watch_event(received: &mpsc::Receiver<WatchEvent>, expected: WatchEvent) -> TestRes {
    let observed = received
        .recv_timeout(WATCH_EVENT_BOUND)
        .map_err(|err| format!("test must observe watch {expected:?} before the bound: {err}"))?;
    if observed != expected {
        return Err(format!(
            "expected watch {expected:?}, observed {observed:?}"
        ));
    }
    Ok(())
}

struct ByteStringRequest(Vec<u8>);

impl Serialize for ByteStringRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(&self.0)
    }
}

#[test]
fn client_read_timeout_closes_silent_peer_with_typed_error() -> TestRes {
    let dir = private_tempdir()?;
    let socket = dir.path().join("silent-peer.sock");
    let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || -> TestRes {
        let (_stream, _address) = listener.accept().map_err(|error| error.to_string())?;
        release_rx.recv().map_err(|error| error.to_string())?;
        Ok(())
    });

    let timeout = Duration::from_millis(25);
    let policy = ClientIoPolicy::try_new(timeout).map_err(|error| error.to_string())?;
    let result = send_request::<_, TestResponseEnvelope>(
        &socket,
        &TestRequestEnvelope {
            request_id: 1,
            payload: 7,
        },
        policy,
    );
    release_tx.send(()).map_err(|error| error.to_string())?;
    let server_result = server
        .join()
        .map_err(|_panic_payload| "server panicked".to_string())?;
    server_result?;
    if !matches!(
        result,
        Err(IpcError::Timeout {
            operation: super::IpcIoOperation::Read,
            timeout: observed,
        }) if observed == timeout
    ) {
        return Err(format!("expected typed read timeout, got {result:?}"));
    }
    Ok(())
}

#[test]
fn observed_client_request_preserves_wire_result_and_nested_read_clock() -> TestRes {
    let dir = private_tempdir()?;
    let socket = dir.path().join("observed-client.sock");
    let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
    let server = thread::spawn(move || -> TestRes {
        for _ in 0..2 {
            let (mut stream, _address) = listener.accept().map_err(|error| error.to_string())?;
            let request: TestRequestEnvelope =
                super::decode_request(&mut stream).map_err(|error| error.to_string())?;
            if request
                != (TestRequestEnvelope {
                    request_id: 17,
                    payload: 23,
                })
            {
                return Err(format!("request changed under observation: {request:?}"));
            }
            let frame = super::encode_response(&TestResponseEnvelope {
                request_id: 17,
                payload: 24,
            })
            .map_err(|error| error.to_string())?;
            stream
                .write_all(&frame)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    });
    let request = TestRequestEnvelope {
        request_id: 17,
        payload: 23,
    };
    let policy =
        ClientIoPolicy::try_new(Duration::from_secs(2)).map_err(|error| error.to_string())?;
    let plain: TestResponseEnvelope =
        send_request(&socket, &request, policy)
            .map_err(|error| format!("plain IPC request failed: {error}"))?;
    let (observed, timing): (TestResponseEnvelope, _) =
        send_request_observed(&socket, &request, policy)
            .map_err(|error| format!("observed IPC request failed: {error}"))?;
    server
        .join()
        .map_err(|_panic_payload| "server panicked".to_string())??;
    if plain
        != (TestResponseEnvelope {
            request_id: 17,
            payload: 24,
        })
        || observed != plain
    {
        return Err(format!(
            "observed response changed: {plain:?} vs {observed:?}"
        ));
    }
    let disjoint = timing
        .encode_ns
        .checked_add(timing.connect_ns)
        .and_then(|value| value.checked_add(timing.write_ns))
        .and_then(|value| value.checked_add(timing.decode_call_ns))
        .ok_or_else(|| "client timing children overflowed".to_string())?;
    if timing.read_io_ns > timing.decode_call_ns || disjoint > timing.total_ns {
        return Err(format!("client timing hierarchy is invalid: {timing:?}"));
    }
    Ok(())
}

#[test]
fn observed_client_request_preserves_expired_deadline_refusal() -> TestRes {
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(1))
        .ok_or_else(|| "deadline overflow".to_string())?;
    let policy = ClientIoPolicy::try_with_deadline(deadline).map_err(|error| error.to_string())?;
    thread::sleep(Duration::from_millis(2));
    let result = send_request_observed::<_, TestResponseEnvelope>(
        Path::new("/path/that/must/not/be-connected.sock"),
        &TestRequestEnvelope {
            request_id: 17,
            payload: 23,
        },
        policy,
    );
    if !matches!(result, Err(IpcError::ClientIoDeadlineElapsed)) {
        return Err(format!("observed deadline refusal changed: {result:?}"));
    }
    Ok(())
}

#[test]
fn absolute_client_deadline_does_not_restart_at_send_boundary() -> TestRes {
    let deadline = std::time::Instant::now() + Duration::from_millis(20);
    let policy = ClientIoPolicy::try_with_deadline(deadline).map_err(|error| error.to_string())?;
    thread::sleep(Duration::from_millis(30));

    let result = send_request::<_, TestResponseEnvelope>(
        std::path::Path::new("/path/that/must/not/be-connected.sock"),
        &TestRequestEnvelope {
            request_id: 99,
            payload: 7,
        },
        policy,
    );

    if !matches!(result, Err(IpcError::ClientIoDeadlineElapsed)) {
        return Err(format!(
            "elapsed absolute deadline must fail before socket connect, got {result:?}"
        ));
    }
    Ok(())
}

#[test]
fn connect_readiness_wait_obeys_absolute_deadline() -> TestRes {
    let (mut writer, _reader) = UnixStream::pair().map_err(|error| error.to_string())?;
    writer
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let payload = vec![0x5a_u8; 64 * 1024];
    loop {
        match writer.write(&payload) {
            Ok(0) => return Err("socket send buffer closed while filling".to_string()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.to_string()),
        }
    }

    let socket = std::os::fd::OwnedFd::from(writer);
    let deadline = Instant::now() + Duration::from_millis(25);
    let started = Instant::now();
    let error = match wait_for_connect(&socket, deadline) {
        Ok(()) => return Err("saturated socket unexpectedly became writable".to_string()),
        Err(error) => error,
    };
    if error.kind() != std::io::ErrorKind::TimedOut {
        return Err(format!("expected connect readiness timeout, got {error}"));
    }
    if started.elapsed() > Duration::from_secs(1) {
        return Err("connect readiness exceeded its absolute deadline bound".to_string());
    }
    Ok(())
}

#[test]
fn connect_socket_preserves_descriptor_and_blocking_invariants() -> TestRes {
    let socket = create_connect_socket().map_err(|error| error.to_string())?;
    let descriptor_flags = fcntl_getfd(&socket).map_err(|error| error.to_string())?;
    if !descriptor_flags.contains(FdFlags::CLOEXEC) {
        return Err("connect socket must be close-on-exec".to_string());
    }
    let status_flags = fcntl_getfl(&socket).map_err(|error| error.to_string())?;
    if !status_flags.contains(OFlags::NONBLOCK) {
        return Err("connect socket must start nonblocking".to_string());
    }
    #[cfg(any(
        target_vendor = "apple",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
    ))]
    if !rustix::net::sockopt::socket_nosigpipe(&socket).map_err(|error| error.to_string())? {
        return Err("connect socket must suppress SIGPIPE".to_string());
    }
    drop(socket);

    let dir = private_tempdir()?;
    let socket_path = dir.path().join("descriptor-state.sock");
    let _listener = UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
    let stream = connect_before_deadline(&socket_path, Instant::now() + Duration::from_secs(1))
        .map_err(|error| error.to_string())?;
    let connected_flags = fcntl_getfl(&stream).map_err(|error| error.to_string())?;
    if connected_flags.contains(OFlags::NONBLOCK) {
        return Err("connected client stream must return to blocking mode".to_string());
    }
    Ok(())
}

#[test]
fn interrupted_connect_enters_completion_wait_contract() {
    assert!(connect_requires_completion_wait(Errno::INTR));
}

/// A tempdir at exactly the private directory mode: `tempfile` creates
/// under the umask, and the server refuses a wider existing directory.
fn private_tempdir() -> Result<tempfile::TempDir, String> {
    let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    Ok(dir)
}

fn typed_bind_error(result: Result<UdsServer, IpcError>) -> Result<IpcError, String> {
    match result {
        Ok(_server) => Err("bind must be refused".to_string()),
        Err(error) => Ok(error),
    }
}

fn mode_of(path: &Path) -> Result<u32, String> {
    Ok(std::fs::symlink_metadata(path)
        .map_err(|error| error.to_string())?
        .mode()
        & 0o7777)
}

// QI-BB-014: a socket path with a live listener is never taken.
#[test]
fn a_live_listener_keeps_its_path_and_a_second_bind_is_refused() -> TestRes {
    let dir = private_tempdir()?;
    let socket_path = dir.path().join("live.sock");
    let first = UdsServer::bind(&socket_path).map_err(|error| error.to_string())?;
    let identity = SocketPathIdentity::capture(&socket_path).map_err(|error| error.to_string())?;
    let refused = typed_bind_error(UdsServer::bind(&socket_path))?;
    if !matches!(refused, IpcError::SocketInUse(ref path) if path == &socket_path) {
        return Err(format!("expected SOCKET_IN_USE, got {refused}"));
    }
    if !identity
        .still_owns(&socket_path)
        .map_err(|error| error.to_string())?
    {
        return Err("the live socket must keep its inode".to_string());
    }
    // The probe's hang-up did not disturb the listener: it still accepts.
    let client = UnixStream::connect(&socket_path).map_err(|error| error.to_string())?;
    drop(client);
    drop(first);
    Ok(())
}

#[test]
fn a_stale_socket_is_reclaimed_and_the_bound_socket_is_private() -> TestRes {
    let dir = private_tempdir()?;
    let socket_dir = dir.path().join("plane");
    let socket_path = socket_dir.join("stale.sock");
    // A socket file nobody listens on any more, in a directory at the
    // private policy's exact mode.
    std::fs::create_dir_all(&socket_dir).map_err(|error| error.to_string())?;
    std::fs::set_permissions(&socket_dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let stale = UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
    drop(stale);
    if !std::fs::symlink_metadata(&socket_path)
        .map_err(|error| error.to_string())?
        .file_type()
        .is_socket()
    {
        return Err("the stale socket file must remain for the test".to_string());
    }
    let server = UdsServer::bind(&socket_path).map_err(|error| error.to_string())?;
    if mode_of(&socket_path)? != PRIVATE_SOCKET_MODE {
        return Err(format!(
            "socket mode must be {PRIVATE_SOCKET_MODE:04o}, got {:04o}",
            mode_of(&socket_path)?
        ));
    }
    let client = UnixStream::connect(&socket_path).map_err(|error| error.to_string())?;
    drop(client);
    drop(server);
    Ok(())
}

#[test]
fn a_directory_the_server_creates_is_private() -> TestRes {
    let dir = private_tempdir()?;
    let socket_dir = dir.path().join("created").join("deeper");
    let socket_path = socket_dir.join("private.sock");
    let server = UdsServer::bind(&socket_path).map_err(|error| error.to_string())?;
    if mode_of(&socket_dir)? != PRIVATE_DIRECTORY_MODE {
        return Err(format!(
            "created socket directory must be {PRIVATE_DIRECTORY_MODE:04o}, got {:04o}",
            mode_of(&socket_dir)?
        ));
    }
    drop(server);
    Ok(())
}

#[test]
fn a_socket_directory_others_can_write_is_refused_unless_sticky() -> TestRes {
    let dir = private_tempdir()?;
    let permissive = dir.path().join("permissive");
    std::fs::create_dir(&permissive).map_err(|error| error.to_string())?;
    std::fs::set_permissions(&permissive, std::fs::Permissions::from_mode(0o777))
        .map_err(|error| error.to_string())?;
    let refused = typed_bind_error(UdsServer::bind(&permissive.join("open.sock")))?;
    if !matches!(refused, IpcError::SocketPathInsecure { ref reason, .. } if reason.contains("writable by others"))
    {
        return Err(format!(
            "expected SOCKET_PATH_INSECURE for a permissive directory, got {refused}"
        ));
    }
    // The sticky bit makes a shared directory safe to bind under: others
    // cannot unlink this user's socket.
    std::fs::set_permissions(&permissive, std::fs::Permissions::from_mode(0o1777))
        .map_err(|error| error.to_string())?;
    let server =
        UdsServer::bind(&permissive.join("shared.sock")).map_err(|error| error.to_string())?;
    drop(server);
    // A group-writable directory of this user's own is refused too.
    let group = dir.path().join("group");
    std::fs::create_dir(&group).map_err(|error| error.to_string())?;
    std::fs::set_permissions(&group, std::fs::Permissions::from_mode(0o770))
        .map_err(|error| error.to_string())?;
    let refused = typed_bind_error(UdsServer::bind(&group.join("group.sock")))?;
    if !matches!(refused, IpcError::SocketPathInsecure { .. }) {
        return Err(format!(
            "expected SOCKET_PATH_INSECURE for a group-writable directory, got {refused}"
        ));
    }
    // So is one merely wider than the private policy: a directory a
    // permissive umask left at 0755 is not 0700 (QI-BB-014).
    let readable = dir.path().join("readable");
    std::fs::create_dir(&readable).map_err(|error| error.to_string())?;
    std::fs::set_permissions(&readable, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| error.to_string())?;
    let refused = typed_bind_error(UdsServer::bind(&readable.join("readable.sock")))?;
    if !matches!(refused, IpcError::SocketPathInsecure { ref reason, .. } if reason.contains("wider than the policy") && reason.contains("0700"))
    {
        return Err(format!(
            "expected SOCKET_PATH_INSECURE for a 0755 directory under a private policy, got {refused}"
        ));
    }
    std::fs::set_permissions(&readable, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let server =
        UdsServer::bind(&readable.join("readable.sock")).map_err(|error| error.to_string())?;
    drop(server);
    Ok(())
}

// A symlinked directory is judged by its target (`/tmp` is a symlink on
// macOS): a private target binds, a permissive target is refused.
#[test]
fn a_symlinked_socket_directory_is_judged_by_its_target() -> TestRes {
    let dir = private_tempdir()?;
    let private = dir.path().join("private");
    std::fs::create_dir(&private).map_err(|error| error.to_string())?;
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let private_link = dir.path().join("private-link");
    std::os::unix::fs::symlink(&private, &private_link).map_err(|error| error.to_string())?;
    let server =
        UdsServer::bind(&private_link.join("via-link.sock")).map_err(|error| error.to_string())?;
    drop(server);
    let permissive = dir.path().join("permissive");
    std::fs::create_dir(&permissive).map_err(|error| error.to_string())?;
    std::fs::set_permissions(&permissive, std::fs::Permissions::from_mode(0o777))
        .map_err(|error| error.to_string())?;
    let permissive_link = dir.path().join("permissive-link");
    std::os::unix::fs::symlink(&permissive, &permissive_link).map_err(|error| error.to_string())?;
    let refused = typed_bind_error(UdsServer::bind(&permissive_link.join("via-link.sock")))?;
    if !matches!(refused, IpcError::SocketPathInsecure { .. }) {
        return Err(format!(
            "expected SOCKET_PATH_INSECURE through a symlink to a permissive directory, got {refused}"
        ));
    }
    Ok(())
}

#[test]
fn a_regular_file_at_the_socket_path_is_refused_and_left_alone() -> TestRes {
    let dir = private_tempdir()?;
    let socket_path = dir.path().join("not-a-socket.sock");
    std::fs::write(&socket_path, b"keep me").map_err(|error| error.to_string())?;
    let refused = typed_bind_error(UdsServer::bind(&socket_path))?;
    if !matches!(refused, IpcError::SocketPathInsecure { ref reason, .. } if reason.contains("not a socket"))
    {
        return Err(format!(
            "expected SOCKET_PATH_INSECURE for a regular file, got {refused}"
        ));
    }
    if std::fs::read(&socket_path).map_err(|error| error.to_string())? != b"keep me" {
        return Err("the regular file must be untouched".to_string());
    }
    Ok(())
}

#[test]
fn dropping_superseded_server_does_not_unlink_replacement_socket() -> TestRes {
    let dir = private_tempdir()?;
    let socket_path = dir.path().join("replacement.sock");
    let superseded = UdsServer::bind(&socket_path).map_err(|error| error.to_string())?;
    std::fs::remove_file(&socket_path).map_err(|error| error.to_string())?;
    let replacement = UdsServer::bind(&socket_path).map_err(|error| error.to_string())?;

    drop(superseded);

    let client = UnixStream::connect(&socket_path)
        .map_err(|error| format!("replacement socket was unlinked: {error}"))?;
    drop(client);
    drop(replacement);
    Ok(())
}

#[test]
fn client_read_timeout_closes_partial_response_frame_with_typed_error() -> TestRes {
    let dir = private_tempdir()?;
    let socket = dir.path().join("partial-frame.sock");
    let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || -> TestRes {
        let (mut stream, _address) = listener.accept().map_err(|error| error.to_string())?;
        let _request: TestRequestEnvelope =
            super::decode_request(&mut stream).map_err(|error| error.to_string())?;
        stream
            .write_all(&10_u32.to_le_bytes())
            .map_err(|error| error.to_string())?;
        stream
            .write_all(&[0xa1, 0x01])
            .map_err(|error| error.to_string())?;
        release_rx.recv().map_err(|error| error.to_string())?;
        Ok(())
    });

    let timeout = Duration::from_millis(25);
    let policy = ClientIoPolicy::try_new(timeout).map_err(|error| error.to_string())?;
    let result = send_request::<_, TestResponseEnvelope>(
        &socket,
        &TestRequestEnvelope {
            request_id: 2,
            payload: 8,
        },
        policy,
    );
    release_tx.send(()).map_err(|error| error.to_string())?;
    let server_result = server
        .join()
        .map_err(|_panic_payload| "server panicked".to_string())?;
    server_result?;
    if !matches!(
        result,
        Err(IpcError::Timeout {
            operation: super::IpcIoOperation::Read,
            timeout: observed,
        }) if observed == timeout
    ) {
        return Err(format!(
            "expected typed partial-frame timeout, got {result:?}"
        ));
    }
    Ok(())
}

#[test]
fn client_write_timeout_closes_peer_that_never_reads_with_typed_error() -> TestRes {
    let dir = private_tempdir()?;
    let socket = dir.path().join("write-backpressure.sock");
    let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
    let (release_tx, release_rx) = mpsc::channel();
    let server = thread::spawn(move || -> TestRes {
        let (_stream, _address) = listener.accept().map_err(|error| error.to_string())?;
        release_rx.recv().map_err(|error| error.to_string())?;
        Ok(())
    });

    let timeout = Duration::from_millis(25);
    let policy = ClientIoPolicy::try_new(timeout).map_err(|error| error.to_string())?;
    let request = ByteStringRequest(vec![0x5a_u8; 8 * 1024 * 1024]);
    let frame = encode_request(&request).map_err(|error| error.to_string())?;
    if frame.len() <= 1024 * 1024 {
        return Err(format!(
            "write-backpressure fixture frame is unexpectedly small: {} bytes",
            frame.len()
        ));
    }
    let result = send_request::<_, TestResponseEnvelope>(&socket, &request, policy);
    release_tx.send(()).map_err(|error| error.to_string())?;
    let server_result = server
        .join()
        .map_err(|_panic_payload| "server panicked".to_string())?;
    server_result?;
    if !matches!(
        result,
        Err(IpcError::Timeout {
            operation: super::IpcIoOperation::Write,
            timeout: observed,
        }) if observed == timeout
    ) {
        return Err(format!("expected typed write timeout, got {result:?}"));
    }
    Ok(())
}

struct TestDispatcher;

impl IpcDispatcher<u64, u64> for TestDispatcher {
    fn dispatch(
        &self,
        _context: &super::DispatchContextV1,
        request: u64,
        _budget: &RequestBudgetV1,
    ) -> u64 {
        request.saturating_add(1)
    }
}

struct ProviderStageDispatcher;

impl IpcDispatcher<u64, u64> for ProviderStageDispatcher {
    fn dispatch(
        &self,
        _context: &super::DispatchContextV1,
        request: u64,
        budget: &RequestBudgetV1,
    ) -> u64 {
        budget.record_provider_stage_v1(RequestProviderStageV1::Started { ticket_id: 91 });
        budget.record_provider_stage_v1(RequestProviderStageV1::Returned { ticket_id: 91 });
        request.saturating_add(1)
    }
}

#[derive(Debug, PartialEq)]
struct TestRequestEnvelope {
    request_id: u64,
    payload: u64,
}

impl Serialize for TestRequestEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("TestRequestEnvelope", 2)?;
        state.serialize_field("request_id", &self.request_id)?;
        state.serialize_field("payload", &self.payload)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for TestRequestEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TestRequestEnvelopeVisitor;

        impl<'de> Visitor<'de> for TestRequestEnvelopeVisitor {
            type Value = TestRequestEnvelope;

            fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter.write_str("a TestRequestEnvelope map")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut request_id: Option<u64> = None;
                let mut payload: Option<u64> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "request_id" => {
                            if request_id.is_some() {
                                return Err(de::Error::duplicate_field("request_id"));
                            }
                            request_id = Some(map.next_value()?);
                        }
                        "payload" => {
                            if payload.is_some() {
                                return Err(de::Error::duplicate_field("payload"));
                            }
                            payload = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(de::Error::unknown_field(&key, &["request_id", "payload"]));
                        }
                    }
                }
                Ok(TestRequestEnvelope {
                    request_id: request_id.ok_or_else(|| de::Error::missing_field("request_id"))?,
                    payload: payload.ok_or_else(|| de::Error::missing_field("payload"))?,
                })
            }
        }

        deserializer.deserialize_struct(
            "TestRequestEnvelope",
            &["request_id", "payload"],
            TestRequestEnvelopeVisitor,
        )
    }
}

impl RequestEnvelope<u64> for TestRequestEnvelope {
    fn into_parts(self) -> (u64, u64) {
        (self.request_id, self.payload)
    }

    fn repo_scope(_request: &u64) -> Option<String> {
        None
    }
}

#[derive(Debug, PartialEq)]
struct TestResponseEnvelope {
    request_id: u64,
    payload: u64,
}

impl Serialize for TestResponseEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("TestResponseEnvelope", 2)?;
        state.serialize_field("request_id", &self.request_id)?;
        state.serialize_field("payload", &self.payload)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for TestResponseEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct TestResponseEnvelopeVisitor;

        impl<'de> Visitor<'de> for TestResponseEnvelopeVisitor {
            type Value = TestResponseEnvelope;

            fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter.write_str("a TestResponseEnvelope map")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut request_id: Option<u64> = None;
                let mut payload: Option<u64> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "request_id" => {
                            if request_id.is_some() {
                                return Err(de::Error::duplicate_field("request_id"));
                            }
                            request_id = Some(map.next_value()?);
                        }
                        "payload" => {
                            if payload.is_some() {
                                return Err(de::Error::duplicate_field("payload"));
                            }
                            payload = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(de::Error::unknown_field(&key, &["request_id", "payload"]));
                        }
                    }
                }
                Ok(TestResponseEnvelope {
                    request_id: request_id.ok_or_else(|| de::Error::missing_field("request_id"))?,
                    payload: payload.ok_or_else(|| de::Error::missing_field("payload"))?,
                })
            }
        }

        deserializer.deserialize_struct(
            "TestResponseEnvelope",
            &["request_id", "payload"],
            TestResponseEnvelopeVisitor,
        )
    }
}

impl ResponseEnvelope<u64> for TestResponseEnvelope {
    fn from_parts(request_id: u64, payload: u64) -> Self {
        Self {
            request_id,
            payload,
        }
    }

    fn result_too_large(_request_id: u64, _encoded_bytes: u64, _limit_bytes: u64) -> Option<Self> {
        None
    }

    fn overloaded(_request_id: u64, _refusal: &SlotRefusal) -> Option<Self> {
        None
    }
}

#[derive(Debug)]
struct DelayedTestResponseEnvelope;

impl<'de> Deserialize<'de> for DelayedTestResponseEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let _response = TestResponseEnvelope::deserialize(deserializer)?;
        thread::sleep(Duration::from_millis(60));
        Ok(Self)
    }
}

#[test]
fn absolute_client_deadline_rejects_response_completed_after_decode() -> TestRes {
    let dir = private_tempdir()?;
    let socket_path = dir.path().join("late-decode.sock");
    let listener = UnixListener::bind(&socket_path).map_err(|error| error.to_string())?;
    let server = thread::spawn(move || -> TestRes {
        let (stream, _address) = listener.accept().map_err(|error| error.to_string())?;
        let reason = handle_connection::<
            TestRequestEnvelope,
            u64,
            TestResponseEnvelope,
            u64,
            TestDispatcher,
        >(
            stream,
            &TestDispatcher,
            &test_slots(),
            test_policy(),
            super::IpcPlane::Query,
            super::PeerCredentials {
                uid: 1000,
                gid: 1000,
                pid: None,
            },
            1000,
            1,
            &AtomicBool::new(false),
            &test_counters(),
        );
        if !matches!(reason, ConnectionCloseReason::PeerClosed) {
            return Err(format!("unexpected close reason: {reason:?}"));
        }
        Ok(())
    });

    let deadline = Instant::now() + Duration::from_millis(30);
    let policy = ClientIoPolicy::try_with_deadline(deadline).map_err(|error| error.to_string())?;
    let result =
        send_request::<_, DelayedTestResponseEnvelope>(&socket_path, &test_request(17, 4), policy);
    let server_result = server
        .join()
        .map_err(|_panic_payload| "server panicked".to_string())?;
    server_result?;
    if !matches!(
        result,
        Err(IpcError::Timeout {
            operation: super::IpcIoOperation::Read,
            ..
        })
    ) {
        return Err(format!(
            "response completed after its deadline must be rejected, got {result:?}"
        ));
    }
    Ok(())
}

struct FailingResponseEnvelope;

impl Serialize for FailingResponseEnvelope {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Err(serde::ser::Error::custom(
            "simulated response encode failure",
        ))
    }
}

impl ResponseEnvelope<u64> for FailingResponseEnvelope {
    fn from_parts(_request_id: u64, _payload: u64) -> Self {
        Self
    }

    fn result_too_large(_request_id: u64, _encoded_bytes: u64, _limit_bytes: u64) -> Option<Self> {
        None
    }

    fn overloaded(_request_id: u64, _refusal: &SlotRefusal) -> Option<Self> {
        Some(Self)
    }
}

/// Encodes past the frame limit on the first serialization and answers
/// the oversize refusal with a small typed marker, so the test can see
/// that the server sent the refusal rather than closing.
enum OversizedResponseEnvelope {
    Oversized { request_id: u64 },
    Refusal { request_id: u64, encoded_bytes: u64 },
}

impl Serialize for OversizedResponseEnvelope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Oversized { request_id } => {
                let mut state = serializer.serialize_struct("OversizedResponseEnvelope", 2)?;
                state.serialize_field("request_id", request_id)?;
                let filler = vec![0_u8; MAX_FRAME_BODY_BYTES.saturating_add(1)];
                state.serialize_field("payload", &CborBytes(&filler))?;
                state.end()
            }
            Self::Refusal {
                request_id,
                encoded_bytes,
            } => {
                let mut state = serializer.serialize_struct("OversizedResponseEnvelope", 2)?;
                state.serialize_field("request_id", request_id)?;
                state.serialize_field("payload", encoded_bytes)?;
                state.end()
            }
        }
    }
}

/// Serialize a byte vector as a CBOR byte string without a `serde_bytes`
/// dependency: a `serde::Serialize` shim over `&[u8]`.
struct CborBytes<'a>(&'a [u8]);

impl Serialize for CborBytes<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(self.0)
    }
}

impl ResponseEnvelope<u64> for OversizedResponseEnvelope {
    fn from_parts(request_id: u64, _payload: u64) -> Self {
        Self::Oversized { request_id }
    }

    fn result_too_large(request_id: u64, encoded_bytes: u64, _limit_bytes: u64) -> Option<Self> {
        Some(Self::Refusal {
            request_id,
            encoded_bytes,
        })
    }

    fn overloaded(_request_id: u64, _refusal: &SlotRefusal) -> Option<Self> {
        None
    }
}

struct BlockingDispatcher {
    entered: mpsc::Sender<()>,
    gate: Arc<Barrier>,
    /// Whether, after the gate, the budget reported the peer's hang-up
    /// within a bounded wait: the cooperative cancellation signal.
    observed_cancel: Arc<AtomicBool>,
}

impl IpcDispatcher<u64, u64> for BlockingDispatcher {
    fn dispatch(
        &self,
        _context: &super::DispatchContextV1,
        request: u64,
        budget: &RequestBudgetV1,
    ) -> u64 {
        let send_result = self.entered.send(());
        assert!(
            send_result.is_ok(),
            "test must observe dispatcher entry: {send_result:?}"
        );
        let _wait = self.gate.wait();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            if budget.is_cancelled() {
                self.observed_cancel.store(true, Ordering::Release);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        request.saturating_add(1)
    }
}

struct FailingWatchDispatcher {
    entered: mpsc::Sender<()>,
    gate: Arc<Barrier>,
    callback_entered: mpsc::Sender<()>,
}

struct PanickingDispatcher;

impl IpcDispatcher<u64, u64> for PanickingDispatcher {
    fn dispatch(
        &self,
        _context: &super::DispatchContextV1,
        _request: u64,
        _budget: &RequestBudgetV1,
    ) -> u64 {
        panic!("scripted dispatcher panic");
    }
}

impl IpcDispatcher<u64, u64> for FailingWatchDispatcher {
    fn dispatch(
        &self,
        _context: &super::DispatchContextV1,
        request: u64,
        budget: &RequestBudgetV1,
    ) -> u64 {
        let callback_entered = self.callback_entered.clone();
        let _failing_waiter = budget.cancel_waiter(Arc::new(move || {
            let _reported = callback_entered.send(());
            panic!("scripted watcher cancellation failure");
        }));
        self.entered.send(()).expect("dispatcher entry is observed");
        let _wait = self.gate.wait();
        request.saturating_add(1)
    }
}

/// Signals entry, waits at the gate, then answers; records whether the
/// budget was cancelled by the time it was released.
struct HalfCloseDispatcher {
    entered: mpsc::Sender<()>,
    gate: Arc<Barrier>,
    observed_cancel: Arc<AtomicBool>,
}

impl IpcDispatcher<u64, u64> for HalfCloseDispatcher {
    fn dispatch(
        &self,
        _context: &super::DispatchContextV1,
        request: u64,
        budget: &RequestBudgetV1,
    ) -> u64 {
        let send_result = self.entered.send(());
        assert!(
            send_result.is_ok(),
            "test must observe dispatcher entry: {send_result:?}"
        );
        let _wait = self.gate.wait();
        if budget.is_cancelled() {
            self.observed_cancel.store(true, Ordering::Release);
        }
        request.saturating_add(1)
    }
}

fn test_request(request_id: u64, payload: u64) -> TestRequestEnvelope {
    TestRequestEnvelope {
        request_id,
        payload,
    }
}

fn assert_test_ok(result: &TestRes) {
    assert!(result.is_ok(), "{result:?}");
}

fn encode_test_frame(request_id: u64, payload: u64) -> Result<Vec<u8>, String> {
    encode_request(&test_request(request_id, payload))
        .map_err(|err| format!("test request must encode: {err}"))
}

// W10-R2: the id gate admits nonzero and refuses 0 typed, before
// admission, dispatch and any response exist.
#[test]
fn envelope_validation_admits_nonzero_and_refuses_zero() {
    let result = (|| -> TestRes {
        let (admitted, payload) = test_request(41, 8)
            .validated()
            .map_err(|err| format!("nonzero id must validate: {err}"))?;
        if admitted.get() != 41 || payload != 8 {
            return Err(format!(
                "validated parts must echo the envelope, got {admitted:?}/{payload}"
            ));
        }
        match test_request(0, 8).validated() {
            Err(IpcError::ZeroRequestId) => Ok(()),
            other => Err(format!("zero id must refuse typed, got {other:?}")),
        }
    })();
    assert_test_ok(&result);
}

// W10-R2: a zero id over the wire closes the connection typed, with
// no dispatch, no response bytes and a decode-failure count — the
// same refusal a corrupt frame gets.
#[test]
fn handle_connection_refuses_zero_request_id_without_dispatch_or_response() {
    let result = (|| -> TestRes {
        let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        let frame = encode_test_frame(0, 8)?;
        client.write_all(&frame).map_err(|err| err.to_string())?;
        client
            .shutdown(Shutdown::Write)
            .map_err(|err| err.to_string())?;

        let counters = test_counters();
        let reason = handle_connection::<
            TestRequestEnvelope,
            u64,
            TestResponseEnvelope,
            u64,
            TestDispatcher,
        >(
            server,
            &TestDispatcher,
            &test_slots(),
            test_policy(),
            IpcPlane::Query,
            PeerCredentials {
                uid: 0,
                gid: 0,
                pid: None,
            },
            0,
            1,
            &AtomicBool::new(false),
            &counters,
        );
        if !matches!(
            reason,
            ConnectionCloseReason::RequestDecodeFailed(IpcError::ZeroRequestId)
        ) {
            return Err(format!("unexpected close reason: {reason:?}"));
        }
        if counters.snapshot().request_decode_failures != 1 {
            return Err(format!(
                "zero-id refusal must count one decode failure, got {:?}",
                counters.snapshot()
            ));
        }
        if !counters
            .recent_request_events_v1()
            .map_err(|error| error.to_string())?
            .is_empty()
        {
            return Err("zero ID must not enter the request event ring".to_string());
        }
        let mut probe = [0u8; 1];
        match client.read(&mut probe) {
            Ok(0) => Ok(()),
            other => Err(format!(
                "refused request must get no response bytes (EOF), got {other:?}"
            )),
        }
    })();
    assert_test_ok(&result);
}

#[test]
fn handle_connection_returns_peer_closed_after_successful_round_trip() {
    let result = (|| -> TestRes {
        let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        let frame = encode_test_frame(41, 8)?;
        client.write_all(&frame).map_err(|err| err.to_string())?;
        client
            .shutdown(Shutdown::Write)
            .map_err(|err| err.to_string())?;

        let counters = test_counters();
        let worker_counters = Arc::clone(&counters);
        let handle = thread::spawn(move || {
            handle_connection::<TestRequestEnvelope, u64, TestResponseEnvelope, u64, TestDispatcher>(
                server,
                &TestDispatcher,
                &test_slots(),
                test_policy(),
                IpcPlane::Query,
                PeerCredentials {
                    uid: 0,
                    gid: 0,
                    pid: None,
                },
                0,
                1,
                &AtomicBool::new(false),
                &worker_counters,
            )
        });

        let response = decode_response::<TestResponseEnvelope, _>(&mut client)
            .map_err(|err| format!("server must write one response before closing: {err}"))?;
        let expected = TestResponseEnvelope {
            request_id: 41,
            payload: 9,
        };
        if response != expected {
            return Err(format!("unexpected response: {response:?}"));
        }
        let reason = handle
            .join()
            .map_err(|join_err| format!("server thread panicked: {join_err:?}"))?;
        if !matches!(reason, ConnectionCloseReason::PeerClosed) {
            return Err(format!("unexpected close reason: {reason:?}"));
        }
        if counters.snapshot().request_decode_failures != 0 {
            return Err(format!(
                "clean peer close counted as a decode failure: {:?}",
                counters.snapshot()
            ));
        }
        let events = counters
            .recent_request_events_v1()
            .map_err(|error| error.to_string())?;
        let stages = events.iter().map(|event| event.stage).collect::<Vec<_>>();
        if stages
            != [
                RequestEventStageV1::Validated,
                RequestEventStageV1::QueueAdmitted,
                RequestEventStageV1::DispatchStarted,
                RequestEventStageV1::DispatchReturned,
                RequestEventStageV1::ResponseWritten,
            ]
        {
            return Err(format!("round-trip event stages differ: {stages:?}"));
        }
        if events
            .iter()
            .any(|event| event.request_id.get() != 41 || event.connection_id != 1)
        {
            return Err(format!("round-trip event identity drift: {events:?}"));
        }
        Ok(())
    })();
    assert_test_ok(&result);
}

#[test]
fn a_connection_reset_is_a_close_only_before_the_next_frame() {
    let reset = || IpcError::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset));
    assert!(super::peer_closed_before_frame(&reset(), false));
    assert!(!super::peer_closed_before_frame(&reset(), true));
    assert!(super::peer_closed_before_frame(&IpcError::Truncated, false));
    assert!(!super::peer_closed_before_frame(&IpcError::Truncated, true));
    assert!(!super::peer_closed_before_frame(
        &IpcError::EmptyFrame,
        false
    ));
}

#[test]
fn a_partial_second_frame_after_a_response_counts_as_a_decode_failure() {
    let result = (|| -> TestRes {
        let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        client
            .write_all(&encode_test_frame(51, 8)?)
            .map_err(|err| err.to_string())?;

        let counters = test_counters();
        let worker_counters = Arc::clone(&counters);
        let handle = thread::spawn(move || {
            handle_connection::<TestRequestEnvelope, u64, TestResponseEnvelope, u64, TestDispatcher>(
                server,
                &TestDispatcher,
                &test_slots(),
                test_policy(),
                IpcPlane::Query,
                PeerCredentials {
                    uid: 0,
                    gid: 0,
                    pid: None,
                },
                0,
                1,
                &AtomicBool::new(false),
                &worker_counters,
            )
        });

        let response = decode_response::<TestResponseEnvelope, _>(&mut client)
            .map_err(|err| format!("first request must receive a response: {err}"))?;
        if response.request_id != 51 || response.payload != 9 {
            return Err(format!("unexpected first response: {response:?}"));
        }
        client.write_all(&[4]).map_err(|err| err.to_string())?;
        client
            .shutdown(Shutdown::Write)
            .map_err(|err| err.to_string())?;

        let reason = handle
            .join()
            .map_err(|join_err| format!("server thread panicked: {join_err:?}"))?;
        if !matches!(
            reason,
            ConnectionCloseReason::RequestDecodeFailed(IpcError::Truncated)
        ) {
            return Err(format!("partial second frame was not refused: {reason:?}"));
        }
        let observed = counters.snapshot();
        if observed.request_decode_failures != 1 || observed.requests_dispatched != 1 {
            return Err(format!("partial frame accounting drifted: {observed:?}"));
        }
        Ok(())
    })();
    assert_test_ok(&result);
}

#[test]
fn provider_stages_keep_the_admitted_envelope_and_connection_identity() {
    let result = (|| -> TestRes {
        let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        client
            .write_all(&encode_test_frame(42, 8)?)
            .map_err(|err| err.to_string())?;
        client
            .shutdown(Shutdown::Write)
            .map_err(|err| err.to_string())?;

        let counters = test_counters();
        let worker_counters = Arc::clone(&counters);
        let handle = thread::spawn(move || {
            handle_connection::<
                TestRequestEnvelope,
                u64,
                TestResponseEnvelope,
                u64,
                ProviderStageDispatcher,
            >(
                server,
                &ProviderStageDispatcher,
                &test_slots(),
                test_policy(),
                IpcPlane::Query,
                PeerCredentials {
                    uid: 0,
                    gid: 0,
                    pid: None,
                },
                0,
                7,
                &AtomicBool::new(false),
                &worker_counters,
            )
        });
        let response = decode_response::<TestResponseEnvelope, _>(&mut client)
            .map_err(|err| format!("provider-stage response: {err}"))?;
        if response
            != (TestResponseEnvelope {
                request_id: 42,
                payload: 9,
            })
        {
            return Err(format!("provider-stage response differs: {response:?}"));
        }
        let reason = handle
            .join()
            .map_err(|error| format!("provider-stage worker panicked: {error:?}"))?;
        if !matches!(reason, ConnectionCloseReason::PeerClosed) {
            return Err(format!("provider-stage close reason differs: {reason:?}"));
        }
        let events = counters
            .recent_request_events_v1()
            .map_err(|error| error.to_string())?;
        let stages = events.iter().map(|event| event.stage).collect::<Vec<_>>();
        if stages
            != [
                RequestEventStageV1::Validated,
                RequestEventStageV1::QueueAdmitted,
                RequestEventStageV1::DispatchStarted,
                RequestEventStageV1::ProviderStarted { ticket_id: 91 },
                RequestEventStageV1::ProviderReturned { ticket_id: 91 },
                RequestEventStageV1::DispatchReturned,
                RequestEventStageV1::ResponseWritten,
            ]
        {
            return Err(format!("provider-stage sequence differs: {stages:?}"));
        }
        if events
            .iter()
            .any(|event| event.request_id.get() != 42 || event.connection_id != 7)
        {
            return Err(format!("provider-stage event identity differs: {events:?}"));
        }
        Ok(())
    })();
    assert_test_ok(&result);
}

#[test]
fn ingest_window_markers_project_into_the_same_transport_ring() {
    let counters = test_counters();
    let bridge = super::ProviderEventBridgeV1 {
        sink: Arc::clone(&counters),
        request_id: std::num::NonZeroU64::new(43).expect("fixed nonzero fixture"),
        connection_id: 8,
        started: Instant::now(),
    };
    for stage in [
        RequestProviderStageV1::IngestWindowStarted { window_ordinal: 1 },
        RequestProviderStageV1::IngestWindowReturned { window_ordinal: 1 },
    ] {
        quanta_index_core::RequestStageDiagnosticPortV1::record_provider_stage_v1(&bridge, stage);
    }
    let events = counters.recent_request_events_v1().expect("ring snapshot");
    assert_eq!(events.len(), 2);
    let first = events.first().expect("first provider event");
    let second = events.get(1).expect("second provider event");
    assert_eq!(first.request_id.get(), 43);
    assert_eq!(second.request_id.get(), 43);
    assert_eq!(first.connection_id, 8);
    assert_eq!(second.connection_id, 8);
    assert_eq!(
        first.stage,
        RequestEventStageV1::IngestWindowStarted { window_ordinal: 1 }
    );
    assert_eq!(
        second.stage,
        RequestEventStageV1::IngestWindowReturned { window_ordinal: 1 }
    );
}

#[test]
fn handle_connection_surfaces_decode_failure_reason() {
    let result = (|| -> TestRes {
        let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        client
            .write_all(&[0, 0, 0, 0])
            .map_err(|err| err.to_string())?;
        client
            .shutdown(Shutdown::Write)
            .map_err(|err| err.to_string())?;

        let counters = test_counters();
        let reason = handle_connection::<
            TestRequestEnvelope,
            u64,
            TestResponseEnvelope,
            u64,
            TestDispatcher,
        >(
            server,
            &TestDispatcher,
            &test_slots(),
            test_policy(),
            IpcPlane::Query,
            PeerCredentials {
                uid: 0,
                gid: 0,
                pid: None,
            },
            0,
            1,
            &AtomicBool::new(false),
            &counters,
        );
        if !matches!(
            reason,
            ConnectionCloseReason::RequestDecodeFailed(IpcError::EmptyFrame)
        ) {
            return Err(format!("unexpected close reason: {reason:?}"));
        }
        // The failure is counted where a scrape will read it (QI-BB-015).
        let snapshot = counters.snapshot();
        if snapshot.request_decode_failures != 1 || snapshot.requests_dispatched != 0 {
            return Err(format!("decode failure must be counted once: {snapshot:?}"));
        }
        Ok(())
    })();
    assert_test_ok(&result);
}

#[test]
fn handle_connection_refuses_a_frame_before_dispatch_when_ingress_is_full() {
    let (mut client, server) = UnixStream::pair().expect("socket pair");
    let frame = encode_test_frame(12, 4).expect("request frame");
    client.write_all(&frame).expect("write request");
    client.shutdown(Shutdown::Write).expect("finish request");
    let slots = test_slots();
    let permits: Vec<_> = (0..64)
        .map(|_| {
            slots
                .try_acquire_decode(1, IpcPlane::Query)
                .expect("fill ingress")
        })
        .collect();
    let counters = test_counters();
    let reason =
        handle_connection::<TestRequestEnvelope, u64, TestResponseEnvelope, u64, TestDispatcher>(
            server,
            &TestDispatcher,
            &slots,
            test_policy(),
            IpcPlane::Query,
            PeerCredentials {
                uid: 0,
                gid: 0,
                pid: None,
            },
            0,
            1,
            &AtomicBool::new(false),
            &counters,
        );
    assert!(matches!(
        reason,
        ConnectionCloseReason::RequestDecodeFailed(IpcError::IngressSaturated { requests: 64, .. })
    ));
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.ingress_admission_refusals, 1);
    assert_eq!(snapshot.requests_overloaded, 0);
    assert_eq!(snapshot.request_decode_failures, 0);
    assert_eq!(snapshot.requests_dispatched, 0);
    drop(permits);
}

#[test]
fn control_frame_dispatches_while_query_ingress_is_saturated() {
    let ingress = Arc::new(IngressBudget::for_process());
    let query_slots =
        DispatchSlots::with_shared_ingress(ServerAdmissionPolicy::DEFAULT, Arc::clone(&ingress));
    let control_slots =
        DispatchSlots::with_shared_ingress(ServerAdmissionPolicy::SERIAL_DISPATCH, ingress);
    let query_permits: Vec<_> = (0..64)
        .map(|_| {
            query_slots
                .try_acquire_decode(
                    MAX_FRAME_BODY_BYTES
                        .checked_div(4)
                        .expect("nonzero divisor"),
                    IpcPlane::Query,
                )
                .expect("query permit")
        })
        .collect();
    let (mut client, server) = UnixStream::pair().expect("socket pair");
    client
        .write_all(&encode_test_frame(12, 4).expect("control request"))
        .expect("write control request");
    client.shutdown(Shutdown::Write).expect("finish request");
    let counters = test_counters();
    let reason =
        handle_connection::<TestRequestEnvelope, u64, TestResponseEnvelope, u64, TestDispatcher>(
            server,
            &TestDispatcher,
            &control_slots,
            test_policy(),
            IpcPlane::Control,
            PeerCredentials {
                uid: 0,
                gid: 0,
                pid: None,
            },
            0,
            1,
            &AtomicBool::new(false),
            &counters,
        );
    assert!(matches!(reason, ConnectionCloseReason::PeerClosed));
    let response: TestResponseEnvelope = decode_response(&mut client).expect("control reply");
    assert_eq!(response.request_id, 12);
    assert_eq!(response.payload, 5);
    assert_eq!(counters.snapshot().requests_dispatched, 1);
    drop(query_permits);
}

/// A response that encodes past the frame limit is answered with the
/// envelope's typed refusal, on the same connection, instead of a close.
#[test]
fn handle_connection_sends_a_typed_refusal_for_an_oversized_response() {
    let result = (|| -> TestRes {
        let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        let frame = encode_test_frame(12, 4)?;
        client.write_all(&frame).map_err(|err| err.to_string())?;
        client
            .shutdown(Shutdown::Write)
            .map_err(|err| err.to_string())?;

        let handle = thread::spawn(move || {
            handle_connection::<
                TestRequestEnvelope,
                u64,
                OversizedResponseEnvelope,
                u64,
                TestDispatcher,
            >(
                server,
                &TestDispatcher,
                &test_slots(),
                test_policy(),
                IpcPlane::Query,
                PeerCredentials {
                    uid: 0,
                    gid: 0,
                    pid: None,
                },
                0,
                1,
                &AtomicBool::new(false),
                &test_counters(),
            )
        });
        let response: TestResponseEnvelope =
            decode_response(&mut client).map_err(|err| err.to_string())?;
        let reason = handle
            .join()
            .map_err(|_panic| "server thread panicked".to_string())?;
        if response.request_id != 12 {
            return Err(format!(
                "refusal carried request_id {}",
                response.request_id
            ));
        }
        let limit = u64::try_from(MAX_FRAME_BODY_BYTES).map_err(|err| err.to_string())?;
        if response.payload <= limit {
            return Err(format!(
                "refusal must report the oversized body, got {} bytes",
                response.payload
            ));
        }
        if !matches!(reason, ConnectionCloseReason::PeerClosed) {
            return Err(format!("unexpected close reason: {reason:?}"));
        }
        Ok(())
    })();
    assert_test_ok(&result);
}

#[test]
fn handle_connection_surfaces_response_encode_failure_reason() {
    let result = (|| -> TestRes {
        let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        let frame = encode_test_frame(7, 4)?;
        client.write_all(&frame).map_err(|err| err.to_string())?;
        client
            .shutdown(Shutdown::Write)
            .map_err(|err| err.to_string())?;

        let counters = test_counters();
        let reason = handle_connection::<
            TestRequestEnvelope,
            u64,
            FailingResponseEnvelope,
            u64,
            TestDispatcher,
        >(
            server,
            &TestDispatcher,
            &test_slots(),
            test_policy(),
            super::IpcPlane::Query,
            super::PeerCredentials {
                uid: 1000,
                gid: 1000,
                pid: None,
            },
            1000,
            1,
            &AtomicBool::new(false),
            &counters,
        );
        let events = counters
            .recent_request_events_v1()
            .map_err(|error| error.to_string())?;
        if !matches!(events.last(), Some(event) if event.stage == RequestEventStageV1::ResponseEncodeFailed && event.request_id.get() == 7)
        {
            return Err(format!("encode failure event missing: {events:?}"));
        }
        if let ConnectionCloseReason::ResponseEncodeFailed(IpcError::Encode(message)) = &reason
            && message.contains("simulated response encode failure")
        {
            return Ok(());
        }
        Err(format!("unexpected close reason: {reason:?}"))
    })();
    assert_test_ok(&result);
}

#[test]
fn overload_response_encode_failure_has_encode_terminal_event() {
    let result = (|| -> TestRes {
        let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        client
            .write_all(&encode_test_frame(18, 4)?)
            .map_err(|err| err.to_string())?;
        let policy = ServerAdmissionPolicy::new(
            1,
            1,
            1,
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(1),
        )
        .map_err(|err| err.to_string())?;
        let slots = DispatchSlots::for_policy(policy);
        let _held = slots
            .acquire(Duration::ZERO, None)
            .map_err(|err| format!("first slot must admit: {err:?}"))?;
        let counters = test_counters();
        let reason = handle_connection::<
            TestRequestEnvelope,
            u64,
            FailingResponseEnvelope,
            u64,
            TestDispatcher,
        >(
            server,
            &TestDispatcher,
            &slots,
            policy,
            IpcPlane::Query,
            PeerCredentials {
                uid: 1000,
                gid: 1000,
                pid: None,
            },
            1000,
            2,
            &AtomicBool::new(false),
            &counters,
        );
        let events = counters
            .recent_request_events_v1()
            .map_err(|err| err.to_string())?;
        if !matches!(reason, ConnectionCloseReason::ResponseEncodeFailed(_)) {
            return Err(format!("wrong overload close reason: {reason:?}"));
        }
        if !matches!(events.last(), Some(event) if event.stage == RequestEventStageV1::ResponseEncodeFailed && event.request_id.get() == 18 && event.connection_id == 2)
        {
            return Err(format!("wrong overload terminal event: {events:?}"));
        }
        Ok(())
    })();
    assert_test_ok(&result);
}

#[test]
fn dispatcher_panic_emits_one_terminal_event_and_releases_slot() {
    let result = (|| -> TestRes {
        let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        client
            .write_all(&encode_test_frame(19, 4)?)
            .map_err(|err| err.to_string())?;
        let counters = test_counters();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handle_connection::<
                TestRequestEnvelope,
                u64,
                TestResponseEnvelope,
                u64,
                PanickingDispatcher,
            >(
                server,
                &PanickingDispatcher,
                &test_slots(),
                test_policy(),
                IpcPlane::Query,
                PeerCredentials {
                    uid: 1000,
                    gid: 1000,
                    pid: None,
                },
                1000,
                3,
                &AtomicBool::new(false),
                &counters,
            )
        }));
        if caught.is_ok() {
            return Err("dispatcher panic was not observed".to_owned());
        }
        let events = counters
            .recent_request_events_v1()
            .map_err(|err| err.to_string())?;
        let terminal = events
            .iter()
            .filter(|event| event.stage == RequestEventStageV1::Panicked)
            .count();
        if terminal != 1
            || !matches!(events.last(), Some(event) if event.request_id.get() == 19 && event.connection_id == 3 && event.stage == RequestEventStageV1::Panicked)
        {
            return Err(format!("panic terminal event invalid: {events:?}"));
        }
        if counters.snapshot().dispatch_in_flight != 0 {
            return Err("panic leaked an in-flight slot".to_owned());
        }
        Ok(())
    })();
    assert_test_ok(&result);
}

/// Disarming a watch on a live peer reports `Stopped`: the wake FD
/// lands at once, the thread is joined, and the budget is untouched.
#[test]
fn peer_watch_disarm_with_a_live_peer_reports_stopped() {
    let result = (|| -> TestRes {
        let (_client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        let (observer, received) = WatchObserver::channel();
        let watch = PeerWatch::arm_with_observer(
            &server,
            budget.cancel_handle(),
            test_counters(),
            observer,
        )
        .map_err(|err| err.to_string())?;
        expect_watch_event(&received, WatchEvent::Armed)?;
        let outcome = watch.disarm()?;
        if outcome != PeerWatchOutcome::Stopped {
            return Err(format!("a live peer disarms Stopped, got {outcome:?}"));
        }
        expect_watch_event(&received, WatchEvent::Stopped)?;
        expect_watch_event(&received, WatchEvent::Joined)?;
        if budget.is_cancelled() {
            return Err("disarming a live peer must not cancel".to_string());
        }
        Ok(())
    })();
    assert_test_ok(&result);
}

/// A peer that closes mid-watch reports `PeerDisconnected`: the hang-up
/// is counted on the watch thread, the budget is cancelled, and a
/// disarm after the close still reports `HungUp`.
#[test]
fn peer_watch_hangup_reports_disconnect_and_cancels() {
    let result = (|| -> TestRes {
        let (client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        let counters = test_counters();
        let (observer, received) = WatchObserver::channel();
        let watch = PeerWatch::arm_with_observer(
            &server,
            budget.cancel_handle(),
            Arc::clone(&counters),
            observer,
        )
        .map_err(|err| err.to_string())?;
        expect_watch_event(&received, WatchEvent::Armed)?;
        drop(client);
        expect_watch_event(&received, WatchEvent::PeerDisconnected)?;
        if !budget.is_cancelled() {
            return Err("a hang-up must cancel the budget".to_string());
        }
        if counters.snapshot().peer_hangup_detected != 1 {
            return Err("a hang-up is detected exactly once".to_string());
        }
        let outcome = watch.disarm()?;
        if outcome != PeerWatchOutcome::HungUp {
            return Err(format!("a closed peer disarms HungUp, got {outcome:?}"));
        }
        expect_watch_event(&received, WatchEvent::Joined)?;
        Ok(())
    })();
    assert_test_ok(&result);
}

/// A panicking watcher is never a successful disarm.
///
/// A watcher that panics after detecting a disconnect must not be
/// reported as a successful `HungUp` or `Stopped` observation. The
/// cancellation callback is the existing owner boundary that can
/// fail on the watcher thread; no production mutation hook is needed.
#[test]
fn peer_watch_thread_failure_is_not_a_successful_disarm() {
    let result = (|| -> TestRes {
        let (client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        let (callback_entered_tx, callback_entered_rx) = mpsc::channel();
        let _failing_waiter = budget.cancel_waiter(Arc::new(move || {
            let _reported = callback_entered_tx.send(());
            panic!("scripted cancellation callback failure");
        }));
        let (observer, received) = WatchObserver::channel();
        let watch = PeerWatch::arm_with_observer(
            &server,
            budget.cancel_handle(),
            test_counters(),
            observer,
        )
        .map_err(|err| err.to_string())?;
        expect_watch_event(&received, WatchEvent::Armed)?;
        drop(client);
        callback_entered_rx
            .recv_timeout(WATCH_EVENT_BOUND)
            .map_err(|err| format!("cancellation callback must enter before disarm: {err}"))?;
        let failure = watch
            .disarm()
            .expect_err("a panicking watcher cannot disarm successfully");
        if !failure.contains("peer watch thread panicked") {
            return Err(format!("watcher failure lost its cause: {failure}"));
        }
        expect_watch_event(&received, WatchEvent::Joined)?;
        Ok(())
    })();
    assert_test_ok(&result);
}

/// A half-close is never a hang-up: the close predates every arm, so
/// each watch's first observation deterministically sees it, and every
/// disarm still reports `Stopped` without cancelling.
#[test]
fn peer_watch_half_close_never_cancels_across_repeated_arms() {
    let result = (|| -> TestRes {
        for _ in 0..8 {
            let (client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            client
                .shutdown(Shutdown::Write)
                .map_err(|err| err.to_string())?;
            let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
            let (observer, received) = WatchObserver::channel();
            let watch = PeerWatch::arm_with_observer(
                &server,
                budget.cancel_handle(),
                test_counters(),
                observer,
            )
            .map_err(|err| err.to_string())?;
            // The half-close predates the arm: `Armed` proves the
            // watch observed it and stayed watching.
            expect_watch_event(&received, WatchEvent::Armed)?;
            let outcome = watch.disarm()?;
            if outcome != PeerWatchOutcome::Stopped {
                return Err(format!("a half-close disarms Stopped, got {outcome:?}"));
            }
            expect_watch_event(&received, WatchEvent::Stopped)?;
            expect_watch_event(&received, WatchEvent::Joined)?;
            if budget.is_cancelled() {
                return Err("a half-close must not cancel the budget".to_string());
            }
            drop(client);
        }
        Ok(())
    })();
    assert_test_ok(&result);
}

/// Dropping a watch without disarming — the panicking-dispatcher path —
/// still stops and joins the watcher thread.
#[test]
fn peer_watch_drop_without_disarm_stops_and_joins() {
    let result = (|| -> TestRes {
        let (_client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
        let (observer, received) = WatchObserver::channel();
        let watch = PeerWatch::arm_with_observer(
            &server,
            budget.cancel_handle(),
            test_counters(),
            observer,
        )
        .map_err(|err| err.to_string())?;
        expect_watch_event(&received, WatchEvent::Armed)?;
        drop(watch);
        expect_watch_event(&received, WatchEvent::Stopped)?;
        expect_watch_event(&received, WatchEvent::Joined)?;
        if budget.is_cancelled() {
            return Err("dropping a live watch must not cancel".to_string());
        }
        Ok(())
    })();
    assert_test_ok(&result);
}

/// Repeated arm/disarm cycles leave no thread or FD behind: a leak
/// would exhaust the process and fail the run instead of this
/// assertion.
#[test]
fn peer_watch_repeated_arms_leave_no_thread_or_fd_behind() {
    let result = (|| -> TestRes {
        let counters = test_counters();
        for _ in 0..256 {
            let (_client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
            let budget = RequestBudgetV1::for_duration(Duration::from_secs(60));
            let watch = PeerWatch::arm(&server, budget.cancel_handle(), Arc::clone(&counters))
                .map_err(|err| err.to_string())?;
            let outcome = watch.disarm()?;
            if outcome != PeerWatchOutcome::Stopped {
                return Err(format!("a live peer disarms Stopped, got {outcome:?}"));
            }
        }
        if counters.snapshot().peer_hangup_detected != 0 {
            return Err("no hang-up was ever detected".to_string());
        }
        Ok(())
    })();
    assert_test_ok(&result);
}

/// A mid-dispatch hang-up cancels the request's budget (QI-BB-002).
///
/// The peer watch notices the hang-up, the dispatcher sees the
/// cancellation at its next checkpoint, and the connection closes as a
/// hang-up instead of a failed write.
/// A peer that sent its request and shut its write side is waiting for
/// the response, not gone: the watch must not cancel it, and the
/// response must still cross the wire. The never-cancel invariant
/// itself is proven at the owner level across repeated deterministic
/// arms; this test keeps the wiring: the response still crosses and
/// the close reason stays honest.
#[test]
fn a_half_closed_peer_is_not_a_hang_up_and_still_gets_its_response() {
    let result = (|| -> TestRes {
        let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        let frame = encode_test_frame(7, 3)?;
        client.write_all(&frame).map_err(|err| err.to_string())?;
        client
            .shutdown(Shutdown::Write)
            .map_err(|err| err.to_string())?;

        let (entered_tx, entered_rx) = mpsc::channel();
        let gate = Arc::new(Barrier::new(2));
        let observed_cancel = Arc::new(AtomicBool::new(false));
        let dispatcher = HalfCloseDispatcher {
            entered: entered_tx,
            gate: Arc::clone(&gate),
            observed_cancel: Arc::clone(&observed_cancel),
        };
        let handle = thread::spawn(move || {
            handle_connection::<
                TestRequestEnvelope,
                u64,
                TestResponseEnvelope,
                u64,
                HalfCloseDispatcher,
            >(
                server,
                &dispatcher,
                &test_slots(),
                test_policy(),
                IpcPlane::Query,
                PeerCredentials {
                    uid: 0,
                    gid: 0,
                    pid: None,
                },
                0,
                1,
                &AtomicBool::new(false),
                &test_counters(),
            )
        });
        entered_rx
            .recv()
            .map_err(|err| format!("test must observe dispatcher entry: {err}"))?;
        let _wait = gate.wait();

        let response = decode_response::<TestResponseEnvelope, _>(&mut client)
            .map_err(|err| format!("a half-closed peer must still get its response: {err}"))?;
        if response
            != (TestResponseEnvelope {
                request_id: 7,
                payload: 4,
            })
        {
            return Err(format!("unexpected response: {response:?}"));
        }
        if observed_cancel.load(Ordering::Acquire) {
            return Err("a half-close must not cancel the budget".to_string());
        }
        let reason = handle
            .join()
            .map_err(|join_err| format!("server thread panicked: {join_err:?}"))?;
        if !matches!(reason, ConnectionCloseReason::PeerClosed) {
            return Err(format!("unexpected close reason: {reason:?}"));
        }
        Ok(())
    })();
    assert_test_ok(&result);
}

#[test]
fn a_peer_that_hangs_up_mid_dispatch_cancels_the_budget() {
    let result = (|| -> TestRes {
        let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        let frame = encode_test_frame(9, 1)?;
        client.write_all(&frame).map_err(|err| err.to_string())?;

        let (entered_tx, entered_rx) = mpsc::channel();
        let gate = Arc::new(Barrier::new(2));
        let observed_cancel = Arc::new(AtomicBool::new(false));
        let dispatcher = BlockingDispatcher {
            entered: entered_tx,
            gate: Arc::clone(&gate),
            observed_cancel: Arc::clone(&observed_cancel),
        };
        let counters = test_counters();
        let worker_counters = Arc::clone(&counters);
        let handle = thread::spawn(move || {
            handle_connection::<
                TestRequestEnvelope,
                u64,
                TestResponseEnvelope,
                u64,
                BlockingDispatcher,
            >(
                server,
                &dispatcher,
                &test_slots(),
                test_policy(),
                IpcPlane::Query,
                PeerCredentials {
                    uid: 0,
                    gid: 0,
                    pid: None,
                },
                0,
                1,
                &AtomicBool::new(false),
                &worker_counters,
            )
        });

        entered_rx.recv().map_err(|err| {
            format!("test must observe request decode before closing peer: {err}")
        })?;
        drop(client);
        let _wait = gate.wait();

        let reason = handle
            .join()
            .map_err(|join_err| format!("server thread panicked: {join_err:?}"))?;
        if !observed_cancel.load(Ordering::Acquire) {
            return Err("dispatcher never saw the peer's hang-up on its budget".to_string());
        }
        let events = counters
            .recent_request_events_v1()
            .map_err(|error| error.to_string())?;
        if !matches!(events.last(), Some(event) if event.stage == RequestEventStageV1::PeerCancelled && event.request_id.get() == 9)
        {
            return Err(format!("peer cancellation event missing: {events:?}"));
        }
        if matches!(reason, ConnectionCloseReason::PeerClosed) {
            return Ok(());
        }
        Err(format!("unexpected close reason: {reason:?}"))
    })();
    assert_test_ok(&result);
}

#[test]
fn handle_connection_surfaces_a_failed_peer_watch() {
    let result = (|| -> TestRes {
        let (mut client, server) = UnixStream::pair().map_err(|err| err.to_string())?;
        client
            .write_all(&encode_test_frame(11, 1)?)
            .map_err(|err| err.to_string())?;
        let (entered_tx, entered_rx) = mpsc::channel();
        let (callback_tx, callback_rx) = mpsc::channel();
        let gate = Arc::new(Barrier::new(2));
        let dispatcher = FailingWatchDispatcher {
            entered: entered_tx,
            gate: Arc::clone(&gate),
            callback_entered: callback_tx,
        };
        let handle = thread::spawn(move || {
            handle_connection::<
                TestRequestEnvelope,
                u64,
                TestResponseEnvelope,
                u64,
                FailingWatchDispatcher,
            >(
                server,
                &dispatcher,
                &test_slots(),
                test_policy(),
                IpcPlane::Query,
                PeerCredentials {
                    uid: 0,
                    gid: 0,
                    pid: None,
                },
                0,
                1,
                &AtomicBool::new(false),
                &test_counters(),
            )
        });
        entered_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|error| format!("dispatcher entry was not observed: {error}"))?;
        drop(client);
        callback_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|error| format!("watcher failure was not observed: {error}"))?;
        let _released = gate.wait();
        let reason = handle
            .join()
            .map_err(|panic| format!("connection thread panicked: {panic:?}"))?;
        if !matches!(&reason, ConnectionCloseReason::PeerWatchFailed(message) if message.contains("peer watch thread panicked"))
        {
            return Err(format!(
                "watcher failure must close the connection typed: {reason:?}"
            ));
        }
        Ok(())
    })();
    assert_test_ok(&result);
}
