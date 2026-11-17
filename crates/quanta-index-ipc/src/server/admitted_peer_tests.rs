use super::admitted_client_tests::scope;
use super::*;
use std::sync::atomic::Ordering;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

// A refused client can close before accept returns. Darwin then refuses
// SO_RCVTIMEO with EINVAL, even though reading EOF is valid. Probe the bounded
// byte/EOF contract without configuring a timeout on a disconnected peer.
fn first_probe_byte(mut stream: UnixStream) -> std::io::Result<usize> {
    stream.set_nonblocking(true)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match stream.read(&mut [0_u8; 1]) {
            Err(error)
                if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
            result => return result,
        }
        if Instant::now() >= deadline {
            return Err(std::io::Error::new(
                ErrorKind::TimedOut,
                "peer byte probe deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn closed_peer_probe_reads_eof_without_reconfiguring_socket_timeout() -> TestResult {
    let (client, server) = UnixStream::pair()?;
    drop(client);
    assert_eq!(first_probe_byte(server)?, 0);
    Ok(())
}

#[test]
fn missing_or_zero_kernel_pid_is_typed_unsupported() {
    for pid in [None, Some(0)] {
        let result = require_observed_peer_pid_v1(crate::PeerCredentials {
            uid: 7,
            gid: 8,
            pid,
        });
        assert!(matches!(result, Err(IpcError::Io(e)) if e.kind()==ErrorKind::Unsupported));
    }
    let peer = crate::PeerCredentials {
        uid: 0,
        gid: 0,
        pid: Some(17),
    };
    assert_eq!(
        require_observed_peer_pid_v1(peer).expect("nonzero PID is available"),
        peer
    );
}

#[cfg(target_os = "linux")]
#[test]
fn connect_only_peer_capture_sends_no_request_and_retains_grant() -> TestResult {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("peer-connect-only.sock");
    let listener = UnixListener::bind(&socket)?;
    let server = std::thread::spawn(move || -> Result<usize, std::io::Error> {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        let mut byte = [0_u8; 1];
        stream.read(&mut byte)
    });
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(3));
    let (memory, active) = scope(false);
    let peer = capture_connected_peer_process_with_budget_v1(&socket, &budget, &memory)?;
    assert!(peer.credentials_v1().pid.is_some_and(|pid| pid != 0));
    assert!(active.load(Ordering::SeqCst) > 0);
    assert_eq!(server.join().map_err(|_| "peer server panicked")??, 0);
    drop(peer);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    Ok(())
}

#[test]
fn failed_connected_peer_observation_writes_no_request_bytes() -> TestResult {
    struct RefusingPeer;
    impl ClientResponseDecode for RefusingPeer {
        type Output = ();
        fn connected(&self, _stream: &UnixStream) -> Result<(), IpcError> {
            Err(IpcError::Io(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "scripted peer lookup refusal",
            )))
        }
        fn read<R: Read>(&self, _reader: &mut R) -> Result<(), IpcError> {
            panic!("peer refusal must precede response decoding")
        }
    }
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("peer-refuse.sock");
    let listener = UnixListener::bind(&socket)
        .map_err(|error| format!("bind peer socket {}: {error}", socket.display()))?;
    let server = std::thread::spawn(move || -> Result<usize, std::io::Error> {
        let (stream, _) = listener.accept()?;
        first_probe_byte(stream)
    });
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(3));
    let (memory, active) = scope(false);
    let result = send_request_inner::<u64, u64, _>(
        &socket,
        &7,
        None,
        ClientIoPolicy::try_with_deadline(budget.deadline())?,
        None,
        Some(&budget),
        Some(&memory),
        &RefusingPeer,
    );
    assert!(matches!(result,Err(IpcError::Io(e)) if e.kind()==ErrorKind::PermissionDenied));
    assert_eq!(server.join().map_err(|_| "peer server panicked")??, 0);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    Ok(())
}

#[cfg(target_os = "linux")]
struct ActualPeerChildGuardV1(std::process::Child);
#[cfg(target_os = "linux")]
impl Drop for ActualPeerChildGuardV1 {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            drop(self.0.kill());
            drop(self.0.wait());
        }
    }
}

#[cfg(target_os = "linux")]
fn actual_peer_child_v1(
    socket: &Path,
    connections: u8,
    probe: bool,
) -> TestResult<ActualPeerChildGuardV1> {
    use std::process::{Command, Stdio};
    let module = module_path!()
        .split_once("::")
        .ok_or("test module path missing")?
        .1;
    let selector = format!("{module}::actual_peer_process_child_v1");
    let mut child = ActualPeerChildGuardV1(
        Command::new(std::env::current_exe()?)
            .args(["--exact", selector.as_str(), "--ignored", "--nocapture"])
            .env("QUANTA_IPC_PEER_TEST_SOCKET_V1", socket)
            .env(
                "QUANTA_IPC_PEER_TEST_CONNECTIONS_V1",
                connections.to_string(),
            )
            .env(
                "QUANTA_IPC_PEER_TEST_PROBE_V1",
                if probe { "1" } else { "0" },
            )
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.exists() {
        if child.0.try_wait()?.is_some() {
            return Err("peer child exited before listen".into());
        }
        if Instant::now() >= deadline {
            return Err("peer child listen deadline".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(child)
}

#[cfg(target_os = "linux")]
fn wait_actual_peer_child_v1(child: &mut ActualPeerChildGuardV1) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.0.try_wait()? {
            if !status.success() {
                return Err("actual peer child failed".into());
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("peer child exit deadline".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

// This child runs in a distinct OS process. It never reports its PID/UID in
// the response; the parent must obtain those from the connected socket.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "internal child process for actual connected-peer regression"]
fn actual_peer_process_child_v1() -> TestResult {
    let socket =
        std::env::var_os("QUANTA_IPC_PEER_TEST_SOCKET_V1").ok_or("child socket missing")?;
    let listener = UnixListener::bind(Path::new(&socket))?;
    let connections = std::env::var("QUANTA_IPC_PEER_TEST_CONNECTIONS_V1")?.parse::<u8>()?;
    if std::env::var("QUANTA_IPC_PEER_TEST_PROBE_V1")?.as_str() == "1" {
        std::fs::write(Path::new(&socket).with_extension("ready"), b"listening")?;
    }
    if !(1..=2).contains(&connections) {
        return Err("invalid child connection count".into());
    }
    for _ in 0..connections {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        if std::env::var("QUANTA_IPC_PEER_TEST_PROBE_V1")?.as_str() == "1" {
            let received = stream.read(&mut [0_u8; 1])?;
            std::fs::write(
                Path::new(&socket).with_extension("received"),
                received.to_string(),
            )?;
            return Ok(());
        }
        let request: u64 = crate::decode_request(&mut stream)?;
        assert_eq!(request, 7);
        stream.write_all(&crate::encode_response(&602214_u64)?)?;
    }
    // Remain alive until the parent checks the kernel process handle. No
    // response field carries a PID, UID, process token, or executable path.
    let release = Path::new(&socket).with_extension("release");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !release.exists() {
        let exec_marker = Path::new(&socket).with_extension("exec");
        if exec_marker.exists() {
            use std::os::unix::process::CommandExt;
            let image = std::fs::read_to_string(&exec_marker)?;
            std::fs::remove_file(Path::new(&socket))?;
            drop(listener);
            let module = module_path!()
                .split_once("::")
                .ok_or("test module path missing")?
                .1;
            let selector = format!("{module}::actual_peer_process_child_v1");
            let cause = std::process::Command::new(image)
                .args(["--exact", selector.as_str(), "--ignored", "--nocapture"])
                .env("QUANTA_IPC_PEER_TEST_CONNECTIONS_V1", "1")
                .env("QUANTA_IPC_PEER_TEST_PROBE_V1", "1")
                .exec();
            return Err(cause.into());
        }
        if Instant::now() >= deadline {
            return Err("parent did not retire the peer observation".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn admitted_peer_and_exact_body_come_from_one_actual_child_connection() -> TestResult {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("child-peer.sock");
    let mut child = actual_peer_child_v1(&socket, 2, false)?;
    let expected_pid = child.0.id();
    assert_ne!(expected_pid, std::process::id());
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (memory, active) = scope(false);
    let (body, peer) =
        send_request_body_with_budget_admitted_peer_v1(&socket, &7_u64, &budget, &memory)?;
    assert_eq!(peer.credentials_v1().pid, Some(expected_pid));
    let (a, _b) = UnixStream::pair()?;
    assert_eq!(
        peer.credentials_v1().uid,
        KernelPeerCredentials.peer_credentials(&a)?.uid
    );
    use std::os::fd::AsRawFd;
    let info = std::fs::read_to_string(format!(
        "/proc/self/fdinfo/{}",
        peer.pidfd_v1()?.as_raw_fd()
    ))?;
    let kernel_pid = info
        .lines()
        .find_map(|line| line.strip_prefix("Pid:"))
        .ok_or("kernel pidfd info omitted process identity")?
        .trim()
        .parse::<u32>()?;
    assert_eq!(
        kernel_pid, expected_pid,
        "pidfd belongs to the actual child, not response DATA"
    );
    assert_eq!(
        ciborium::de::from_reader::<u64, _>(body.as_slice())?,
        602214
    );
    assert!(
        active.load(Ordering::SeqCst) > u64::try_from(body.as_slice().len())?,
        "both raw reply and connected process owner retain their actual grants"
    );
    drop(body);
    assert!(
        active.load(Ordering::SeqCst)
            > u64::try_from(crate::MemoryLease::allocation_backing_layout()?.size(),)?,
        "the image-bearing peer payload exceeds the old lease payload and is fully prepaid"
    );
    assert!(
        active.load(Ordering::SeqCst) > 0,
        "process FD owner keeps the same Original charge after raw body retirement"
    );
    let lease = peer.share_lease_v1();
    let alias = lease.clone();
    assert!(lease.same_capture_v1(&alias));
    let (second_body, second_peer) =
        send_request_body_with_budget_admitted_peer_v1(&socket, &7_u64, &budget, &memory)?;
    let independent = second_peer.share_lease_v1();
    assert_eq!(
        lease.credentials_v1(),
        independent.credentials_v1(),
        "same actual child process"
    );
    assert!(
        !lease.same_capture_v1(&independent),
        "independent socket captures are not aliases even with identical credentials"
    );
    drop(second_body);
    drop(second_peer);
    drop(independent);
    drop(peer);
    assert!(
        active.load(Ordering::SeqCst) > 0,
        "shared immutable process lease retains the same issuer"
    );
    drop(lease);
    assert!(
        active.load(Ordering::SeqCst) > 0,
        "last actual-capture alias retains its issuing grant"
    );
    drop(alias);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    std::fs::write(socket.with_extension("release"), b"observed")?;
    wait_actual_peer_child_v1(&mut child)?;
    Ok(())
}

#[cfg(any(
    target_vendor = "apple",
    target_os = "android",
    target_os = "dragonfly",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd"
))]
#[test]
fn kernel_without_peer_pid_refuses_before_request_write() -> TestResult {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("no-peer-pid.sock");
    let listener = UnixListener::bind(&socket)
        .map_err(|error| format!("bind peer socket {}: {error}", socket.display()))?;
    let server = std::thread::spawn(move || -> Result<usize, std::io::Error> {
        let (stream, _) = listener.accept()?;
        first_probe_byte(stream)
    });
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(3));
    let (memory, active) = scope(false);
    let result = send_request_body_with_budget_admitted_peer_v1(&socket, &7_u64, &budget, &memory);
    assert!(matches!(result,Err(IpcError::Io(e)) if e.kind()==ErrorKind::Unsupported));
    assert_eq!(server.join().map_err(|_| "peer server panicked")??, 0);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn connected_process_denial_and_foreign_credentials_create_no_process_owner() -> TestResult {
    let (stream, _other) = UnixStream::pair()?;
    let actual = KernelPeerCredentials.peer_credentials(&stream)?;
    let (denied, denied_active) = scope(true);
    let result = crate::KernelConnectedPeerProcessV1::try_capture_v1(&stream, actual, &denied);
    assert!(matches!(result, Err(IpcError::FrameMemoryLimited { .. })));
    assert_eq!(denied_active.load(Ordering::SeqCst), 0);
    let (memory, active) = scope(false);
    let mut foreign = actual;
    foreign.pid = Some(
        actual
            .pid
            .ok_or("actual peer PID missing")?
            .checked_add(1)
            .ok_or("PID overflow")?,
    );
    let result = crate::KernelConnectedPeerProcessV1::try_capture_v1(&stream, foreign, &memory);
    assert!(matches!(result, Err(IpcError::Io(e)) if e.kind() == ErrorKind::InvalidData));
    assert_eq!(active.load(Ordering::SeqCst), 0);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn bound_admitted_request_reuses_actual_child_and_last_process_grant_v1() -> TestResult {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("bound-peer.sock");
    let mut child = actual_peer_child_v1(&socket, 2, false)?;
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (memory, active) = scope(false);
    let (body, peer) =
        send_request_body_with_budget_admitted_peer_v1(&socket, &7_u64, &budget, &memory)?;
    let lease = peer.share_lease_v1();
    drop(body);
    drop(peer);
    let bound = send_request_body_with_budget_admitted_bound_peer_v1(
        &socket, &7_u64, &budget, &memory, &lease,
    )?;
    assert_eq!(
        ciborium::de::from_reader::<u64, _>(bound.as_slice())?,
        602214
    );
    assert_eq!(lease.credentials_v1().pid, Some(child.0.id()));
    drop(bound);
    assert!(
        active.load(Ordering::SeqCst) > 0,
        "process guard survives both raw replies"
    );
    drop(lease);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    std::fs::write(socket.with_extension("release"), b"observed")?;
    wait_actual_peer_child_v1(&mut child)
}

#[cfg(target_os = "linux")]
#[test]
fn bound_typed_read_reuses_actual_child_and_process_grant_v1() -> TestResult {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("bound-typed-peer.sock");
    let mut child = actual_peer_child_v1(&socket, 2, false)?;
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (memory, active) = scope(false);
    let (first, first_peer) =
        send_request_body_with_budget_admitted_peer_v1(&socket, &7_u64, &budget, &memory)?;
    let lease = first_peer.share_lease_v1();
    drop(first);
    drop(first_peer);
    let reply: u64 = send_request_with_budget_bound_peer_v1(&socket, &7_u64, &budget, &lease)?;
    assert_eq!(reply, 602214);
    assert_eq!(lease.credentials_v1().pid, Some(child.0.id()));
    drop(lease);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    std::fs::write(socket.with_extension("release"), b"observed")?;
    wait_actual_peer_child_v1(&mut child)
}

#[cfg(target_os = "linux")]
fn assert_replaced_socket_same_image_process_gets_zero_bytes_v1(
    send: impl FnOnce(
        &Path,
        &RequestBudgetV1,
        &MemoryScope,
        &crate::KernelConnectedPeerProcessLeaseV1,
    ) -> Result<(), IpcError>,
) -> TestResult {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("replace-peer.sock");
    let mut original = actual_peer_child_v1(&socket, 1, false)?;
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (memory, active) = scope(false);
    let (body, peer) =
        send_request_body_with_budget_admitted_peer_v1(&socket, &7_u64, &budget, &memory)?;
    let lease = peer.share_lease_v1();
    drop(body);
    drop(peer);
    std::fs::remove_file(&socket)?;
    let mut replacement = actual_peer_child_v1(&socket, 1, true)?;
    assert_ne!(original.0.id(), replacement.0.id());
    assert!(
        original.0.try_wait()?.is_none(),
        "original process stays alive during replacement"
    );
    let result = send(&socket, &budget, &memory, &lease);
    assert!(matches!(
        result,
        Err(IpcError::PeerBindingRefused(
            crate::PeerBindingRefusalV1::ForeignProcess
        ))
    ));
    wait_actual_peer_child_v1(&mut replacement)?;
    assert_eq!(
        std::fs::read_to_string(socket.with_extension("received"))?,
        "0"
    );
    assert!(original.0.try_wait()?.is_none());
    drop(lease);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    std::fs::write(socket.with_extension("release"), b"observed")?;
    wait_actual_peer_child_v1(&mut original)
}

#[cfg(target_os = "linux")]
#[test]
fn replaced_socket_same_image_process_gets_zero_request_bytes_v1() -> TestResult {
    assert_replaced_socket_same_image_process_gets_zero_bytes_v1(|socket, budget, memory, lease| {
        send_request_body_with_budget_admitted_bound_peer_v1(socket, &7_u64, budget, memory, lease)
            .map(|_body| ())
    })
}

#[cfg(target_os = "linux")]
#[test]
fn replaced_socket_typed_bound_read_sends_zero_request_bytes_v1() -> TestResult {
    assert_replaced_socket_same_image_process_gets_zero_bytes_v1(
        |socket, budget, _memory, lease| {
            send_request_with_budget_bound_peer_v1::<u64, u64>(socket, &7, budget, lease)
                .map(|_reply| ())
        },
    )
}

#[cfg(target_os = "linux")]
#[test]
fn exited_original_process_refuses_before_reconnect_v1() -> TestResult {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("exited-peer.sock");
    let mut child = actual_peer_child_v1(&socket, 1, false)?;
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (memory, active) = scope(false);
    let (body, peer) =
        send_request_body_with_budget_admitted_peer_v1(&socket, &7_u64, &budget, &memory)?;
    let lease = peer.share_lease_v1();
    drop(body);
    drop(peer);
    std::fs::write(socket.with_extension("release"), b"observed")?;
    wait_actual_peer_child_v1(&mut child)?;
    std::fs::remove_file(&socket)?;
    let replacement = UnixListener::bind(&socket)?;
    replacement.set_nonblocking(true)?;
    let result = send_request_body_with_budget_admitted_bound_peer_v1(
        &socket, &7_u64, &budget, &memory, &lease,
    );
    assert!(matches!(
        result,
        Err(IpcError::PeerBindingRefused(
            crate::PeerBindingRefusalV1::ProcessExited
        ))
    ));
    assert!(matches!(replacement.accept(), Err(e) if e.kind() == ErrorKind::WouldBlock));
    drop(lease);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn same_process_exec_image_change_gets_zero_request_bytes_v1() -> TestResult {
    let directory = tempfile::tempdir()?;
    let socket = directory.path().join("exec-peer.sock");
    let replacement_image = directory.path().join("other-peer-image");
    assert!(std::fs::copy(std::env::current_exe()?, &replacement_image)? > 0);
    let mut child = actual_peer_child_v1(&socket, 1, false)?;
    let original_pid = child.0.id();
    let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
    let (memory, active) = scope(false);
    let (body, peer) =
        send_request_body_with_budget_admitted_peer_v1(&socket, &7_u64, &budget, &memory)?;
    let lease = peer.share_lease_v1();
    let initial_check = lease.with_live_executable_v1(|_retained| Ok::<(), std::io::Error>(()))?;
    initial_check?;
    drop(body);
    drop(peer);
    std::fs::write(
        socket.with_extension("exec"),
        replacement_image
            .to_str()
            .ok_or("image path is not UTF-8")?,
    )?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !socket.with_extension("ready").exists() {
        if child.0.try_wait()?.is_some() {
            return Err("peer exited before replacement exec".into());
        }
        if Instant::now() >= deadline {
            return Err("peer exec readiness deadline".into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(lease.credentials_v1().pid, Some(original_pid));
    assert!(
        child.0.try_wait()?.is_none(),
        "the original process instance is still alive after exec"
    );
    let policy_called = std::cell::Cell::new(false);
    let policy = lease.with_live_executable_v1(|_| {
        policy_called.set(true);
        Ok::<(), std::io::Error>(())
    });
    assert!(matches!(
        policy,
        Err(crate::PeerBindingRefusalV1::Image(
            quanta_explicit_fd_process::ExplicitFdProcessErrorV1::ExecutableIdentityDrift,
        ))
    ));
    assert!(
        !policy_called.get(),
        "another image cannot replace the retained image at installed-policy admission"
    );
    let result = send_request_body_with_budget_admitted_bound_peer_v1(
        &socket, &7_u64, &budget, &memory, &lease,
    );
    assert!(matches!(
        result,
        Err(IpcError::PeerBindingRefused(
            crate::PeerBindingRefusalV1::Image(
                quanta_explicit_fd_process::ExplicitFdProcessErrorV1::ExecutableIdentityDrift,
            )
        ))
    ));
    wait_actual_peer_child_v1(&mut child)?;
    assert_eq!(
        std::fs::read_to_string(socket.with_extension("received"))?,
        "0"
    );
    drop(lease);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    Ok(())
}
