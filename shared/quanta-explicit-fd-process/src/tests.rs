#[cfg(target_os = "linux")]
use super::spawn_fixed_two_arg_linux_hard_limited_v1;
use super::{
    try_duplicate_inherited_read_pipe_v1, try_duplicate_inherited_unix_datagram_v1,
    ExplicitFdChildV1, ExplicitFdProcessErrorV1, ExplicitFdSpawnRequestV1, PinnedExecutableV1,
    PinnedReadOnlyFileV1, PinnedWorkingDirectoryV1,
};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixDatagram, UnixStream};
use std::sync::OnceLock;

#[cfg(target_os = "linux")]
mod connected_peer_image_v1;
mod pinned_file_mappings_v1;
mod retained_regular_file_v1;

const STDIO_HELPER_MARKER_V1: &str = "QUANTA_EXPLICIT_FD_STDIO_HELPER_V1";
const ORIGINAL_IMAGE_HELPER_MARKER_V1: &str = "QUANTA_EXPLICIT_FD_ORIGINAL_IMAGE_HELPER_V1";
const REGULAR_FILE_HELPER_MARKER_V1: &str = "QUANTA_EXPLICIT_FD_REGULAR_FILE_HELPER_V1";
const CWD_HELPER_MARKER_V1: &str = "QUANTA_EXPLICIT_FD_CWD_HELPER_V1";
const SLEEP_HELPER_MARKER_V1: &str = "QUANTA_EXPLICIT_FD_SLEEP_HELPER_V1";
const DESCENDANT_HELPER_MARKER_V1: &str = "QUANTA_EXPLICIT_FD_DESCENDANT_HELPER_V1";
const PIPE_HELPER_FD_MARKER_V1: &str = "QUANTA_EXPLICIT_FD_PIPE_PROBE_V1";
const PIPE_HELPER_EXPECTED_MARKER_V1: &str = "QUANTA_EXPLICIT_FD_PIPE_EXPECTED_V1";
const DATAGRAM_HELPER_FD_MARKER_V1: &str = "QUANTA_EXPLICIT_FD_DATAGRAM_PROBE_V1";

#[cfg(target_os = "linux")]
#[test]
fn fixed_two_arg_worker_launch_preserves_pinned_image_limit_and_stdio_v1() {
    let path_v1 = std::path::Path::new("/bin/cat");
    let source_v1 = File::open(path_v1).expect("installed cat image");
    let pinned_v1 = PinnedExecutableV1::try_pin_v1(&source_v1, &sha256_path_v1(path_v1))
        .expect("pin exact image");
    assert!(matches!(
        spawn_fixed_two_arg_linux_hard_limited_v1(&pinned_v1, [c"cat", c"-"], 0),
        Err(ExplicitFdProcessErrorV1::InvalidChildMemoryLimit)
    ));
    let mut child_v1 =
        spawn_fixed_two_arg_linux_hard_limited_v1(&pinned_v1, [c"cat", c"-"], 1024 * 1024 * 1024)
            .expect("fixed pinned worker launch");
    let mut stdin_v1 = child_v1.take_stdin_v1().expect("child stdin");
    stdin_v1.write_all(b"fixed-shape\n").expect("write stdin");
    drop(stdin_v1);
    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("reap child");
    assert!(status_v1.success());
    assert_eq!(stdout_v1, b"fixed-shape\n");
    assert!(stderr_v1.is_empty());
}

#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_host_refuses_preexec_memory_limit_v1() {
    let executable_v1 = pin_current_test_executable_v1();
    let request_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_original_image_v1",
        ORIGINAL_IMAGE_HELPER_MARKER_V1,
        OsStr::new("1"),
    );
    assert!(matches!(
        request_v1.with_linux_hard_memory_limit_v1(1024 * 1024 * 1024),
        Err(ExplicitFdProcessErrorV1::ChildMemoryLimitUnavailable)
    ));
}
#[cfg(target_os = "linux")]
const MEMORY_LIMIT_HELPER_MARKER_V1: &str = "QUANTA_EXPLICIT_FD_MEMORY_LIMIT_PROBE_V1";

#[cfg(target_os = "linux")]
#[test]
fn pinned_child_uses_exact_hard_memory_limit_before_exec_v1() {
    let executable_v1 = pin_current_test_executable_v1();
    let request_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_hard_memory_limit_v1",
        MEMORY_LIMIT_HELPER_MARKER_V1,
        OsStr::new("1"),
    );
    assert!(matches!(
        request_v1
            .with_linux_hard_memory_limit_v1(0)
            .expect_err("zero limit must fail before spawn"),
        ExplicitFdProcessErrorV1::InvalidChildMemoryLimit
    ));
    let request_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_hard_memory_limit_v1",
        MEMORY_LIMIT_HELPER_MARKER_V1,
        OsStr::new("1"),
    );
    #[allow(clippy::unnecessary_cast, reason = "rlim_t may be narrower than u64")]
    let infinite_limit_v1 = libc::RLIM_INFINITY as u64;
    assert!(matches!(
        request_v1
            .with_linux_hard_memory_limit_v1(infinite_limit_v1)
            .expect_err("infinite limit must fail before spawn"),
        ExplicitFdProcessErrorV1::InvalidChildMemoryLimit
    ));
    let mut child_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_hard_memory_limit_v1",
        MEMORY_LIMIT_HELPER_MARKER_V1,
        OsStr::new("1"),
    )
    .with_linux_hard_memory_limit_v1(1024 * 1024 * 1024)
    .expect("finite hard limit")
    .spawn_v1()
    .expect("execute pinned image with pre-exec limit");
    drop(child_v1.take_stdin_v1());
    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("wait child");
    assert_helper_success_v1(status_v1, &stdout_v1, &stderr_v1, "hard limit helper");
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "subprocess-only test helper"]
fn child_probe_hard_memory_limit_v1() {
    std::env::var_os(MEMORY_LIMIT_HELPER_MARKER_V1).expect("memory limit helper marker");
    for resource_v1 in [libc::RLIMIT_AS, libc::RLIMIT_DATA] {
        let mut observed_v1 = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: getrlimit writes one initialized limit struct.
        assert_eq!(unsafe { libc::getrlimit(resource_v1, &mut observed_v1) }, 0);
        assert_eq!(observed_v1.rlim_cur, 1024 * 1024 * 1024);
        assert_eq!(observed_v1.rlim_max, observed_v1.rlim_cur);
    }
}

#[test]
fn inherited_unix_datagram_is_validated_and_duplicated_cloexec_v1() {
    let (inherited_v1, peer_v1) = UnixDatagram::pair().expect("create Unix datagram pair");
    clear_cloexec_v1(inherited_v1.as_raw_fd());

    let duplicate_v1 = try_duplicate_inherited_unix_datagram_v1(inherited_v1.as_raw_fd())
        .expect("duplicate inherited Unix datagram");
    // SAFETY: F_GETFD observes the owned duplicate's descriptor flags.
    let duplicate_flags_v1 = unsafe { libc::fcntl(duplicate_v1.as_raw_fd(), libc::F_GETFD) };
    assert_ne!(duplicate_flags_v1 & libc::FD_CLOEXEC, 0);
    // SAFETY: F_GETFD observes the process-owned inherited descriptor.
    let inherited_flags_v1 = unsafe { libc::fcntl(inherited_v1.as_raw_fd(), libc::F_GETFD) };
    assert_ne!(inherited_flags_v1 & libc::FD_CLOEXEC, 0);

    peer_v1.send(b"armed").expect("send peer datagram");
    let mut buffer_v1 = [0_u8; 16];
    let received_v1 = duplicate_v1
        .recv(&mut buffer_v1)
        .expect("receive inherited datagram");
    assert_eq!(&buffer_v1[..received_v1], b"armed");

    duplicate_v1
        .send(b"entered")
        .expect("send duplicate datagram");
    let received_v1 = peer_v1
        .recv(&mut buffer_v1)
        .expect("receive duplicate datagram");
    assert_eq!(&buffer_v1[..received_v1], b"entered");

    drop(duplicate_v1);
    peer_v1.send(b"original").expect("send original datagram");
    let received_v1 = inherited_v1
        .recv(&mut buffer_v1)
        .expect("inherited descriptor remains process-owned");
    assert_eq!(&buffer_v1[..received_v1], b"original");
}

#[test]
fn inherited_unix_datagram_rejects_ambient_or_unconnected_descriptors_v1() {
    assert_eq!(
        try_duplicate_inherited_unix_datagram_v1(libc::STDERR_FILENO)
            .expect_err("stdio descriptors must fail closed"),
        ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor
    );

    let (cloexec_v1, _peer_v1) = UnixDatagram::pair().expect("create CLOEXEC pair");
    set_cloexec_v1(cloexec_v1.as_raw_fd());
    assert_eq!(
        try_duplicate_inherited_unix_datagram_v1(cloexec_v1.as_raw_fd())
            .expect_err("ambient CLOEXEC socket must fail closed"),
        ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor
    );

    let unconnected_v1 = UnixDatagram::unbound().expect("create unconnected Unix datagram");
    clear_cloexec_v1(unconnected_v1.as_raw_fd());
    assert_eq!(
        try_duplicate_inherited_unix_datagram_v1(unconnected_v1.as_raw_fd())
            .expect_err("unconnected socket must fail closed"),
        ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor
    );

    let (stream_v1, _stream_peer_v1) = UnixStream::pair().expect("create Unix stream pair");
    clear_cloexec_v1(stream_v1.as_raw_fd());
    assert_eq!(
        try_duplicate_inherited_unix_datagram_v1(stream_v1.as_raw_fd())
            .expect_err("Unix stream socket must fail closed"),
        ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor
    );

    let udp_v1 = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind UDP socket");
    let udp_peer_v1 = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind UDP peer");
    udp_v1
        .connect(udp_peer_v1.local_addr().expect("read UDP peer address"))
        .expect("connect UDP socket");
    clear_cloexec_v1(udp_v1.as_raw_fd());
    assert_eq!(
        try_duplicate_inherited_unix_datagram_v1(udp_v1.as_raw_fd())
            .expect_err("non-Unix datagram socket must fail closed"),
        ExplicitFdProcessErrorV1::InvalidInheritedUnixDatagramDescriptor
    );
}

fn clear_cloexec_v1(raw_fd_v1: i32) {
    // SAFETY: F_GETFD observes the live test-owned descriptor.
    let flags_v1 = unsafe { libc::fcntl(raw_fd_v1, libc::F_GETFD) };
    assert!(flags_v1 >= 0, "read descriptor flags");
    // SAFETY: F_SETFD updates only the live test-owned descriptor flags.
    let result_v1 = unsafe { libc::fcntl(raw_fd_v1, libc::F_SETFD, flags_v1 & !libc::FD_CLOEXEC) };
    assert_eq!(result_v1, 0, "clear FD_CLOEXEC");
}

fn set_cloexec_v1(raw_fd_v1: i32) {
    // SAFETY: F_GETFD observes the live test-owned descriptor.
    let flags_v1 = unsafe { libc::fcntl(raw_fd_v1, libc::F_GETFD) };
    assert!(flags_v1 >= 0, "read descriptor flags");
    // SAFETY: F_SETFD updates only the live test-owned descriptor flags.
    let result_v1 = unsafe { libc::fcntl(raw_fd_v1, libc::F_SETFD, flags_v1 | libc::FD_CLOEXEC) };
    assert_eq!(result_v1, 0, "set FD_CLOEXEC");
}

#[test]
fn stdio_round_trip_executes_immutable_custody_v1() {
    let executable_v1 = pin_current_test_executable_v1();
    let mut child_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_stdio_round_trip_v1",
        STDIO_HELPER_MARKER_V1,
        OsStr::new("1"),
    )
    .spawn_v1()
    .expect("spawn stdio helper");
    let mut stdin_v1 = child_v1.take_stdin_v1().expect("child stdin");
    stdin_v1
        .write_all(b"custody-round-trip\n")
        .expect("write stdin");
    drop(stdin_v1);
    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("wait child");
    assert_helper_success_v1(status_v1, &stdout_v1, &stderr_v1, "stdio helper");
}

#[test]
fn in_place_source_overwrite_cannot_change_pinned_executable_v1() {
    let source_v1 = tempfile::NamedTempFile::new().expect("temporary executable");
    std::fs::copy(current_test_executable_path_v1(), source_v1.path())
        .expect("copy current test executable fixture");
    // The path remains owner-writable for the in-place mutation while the
    // descriptor supplied to the pinning API is independently read-only.
    std::fs::set_permissions(source_v1.path(), std::fs::Permissions::from_mode(0o700))
        .expect("mark executable");
    let digest_v1 = sha256_path_v1(source_v1.path());
    let source_file_v1 = File::open(source_v1.path()).expect("open executable source");
    let executable_v1 =
        PinnedExecutableV1::try_pin_v1(&source_file_v1, &digest_v1).expect("pin executable");

    let replacement_v1 = std::fs::read("/usr/bin/false").expect("read replacement fixture");
    let mut writer_v1 = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(source_v1.path())
        .expect("open mutable source");
    writer_v1
        .write_all(&replacement_v1)
        .expect("overwrite mutable source inode");
    drop(writer_v1);

    let mut child_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_original_image_v1",
        ORIGINAL_IMAGE_HELPER_MARKER_V1,
        OsStr::new("1"),
    )
    .spawn_v1()
    .expect("spawn immutable executable custody");
    drop(child_v1.take_stdin_v1());
    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("wait child");
    assert_helper_success_v1(status_v1, &stdout_v1, &stderr_v1, "original image helper");
}

#[test]
fn pathname_replacement_cannot_change_pinned_executable_v1() {
    let directory_v1 = tempfile::tempdir().expect("temporary executable directory");
    let source_path_v1 = directory_v1.path().join("service");
    let replacement_path_v1 = directory_v1.path().join("replacement");
    std::fs::copy(current_test_executable_path_v1(), &source_path_v1)
        .expect("copy original test executable");
    std::fs::set_permissions(&source_path_v1, std::fs::Permissions::from_mode(0o500))
        .expect("mark original executable");
    let source_file_v1 = File::open(&source_path_v1).expect("open original executable");
    let executable_v1 =
        PinnedExecutableV1::try_pin_v1(&source_file_v1, &sha256_path_v1(&source_path_v1))
            .expect("pin original executable");
    std::fs::copy("/usr/bin/false", &replacement_path_v1).expect("copy replacement executable");
    std::fs::set_permissions(&replacement_path_v1, std::fs::Permissions::from_mode(0o500))
        .expect("mark replacement executable");
    std::fs::rename(&replacement_path_v1, &source_path_v1).expect("replace executable pathname");

    let mut child_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_original_image_v1",
        ORIGINAL_IMAGE_HELPER_MARKER_V1,
        OsStr::new("1"),
    )
    .spawn_v1()
    .expect("spawn descriptor custody after path replacement");
    drop(child_v1.take_stdin_v1());
    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("wait child");
    assert_helper_success_v1(status_v1, &stdout_v1, &stderr_v1, "path replacement helper");
}

#[test]
fn explicit_pipe_mapping_delivers_caller_owned_bytes_v1() {
    let (read_v1, write_v1) = cloexec_pipe_v1();
    let mut writer_v1 = File::from(write_v1);
    writer_v1
        .write_all(b"pipe-challenge\n")
        .expect("write challenge pipe");
    drop(writer_v1);
    let executable_v1 = pin_current_test_executable_v1();
    let mut child_v1 = pipe_helper_request_v1(&executable_v1, "pipe-challenge\n")
        .with_child_pipe_read_v1(read_v1.as_fd(), 9)
        .expect("map explicit child pipe")
        .spawn_v1()
        .expect("spawn pipe consumer");
    drop(child_v1.take_stdin_v1());
    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("wait child");
    assert_helper_success_v1(status_v1, &stdout_v1, &stderr_v1, "pipe helper");
}

#[test]
fn explicit_unix_datagram_mapping_is_exact_and_bidirectional_v1() {
    let (child_socket_v1, harness_socket_v1) =
        UnixDatagram::pair().expect("create child control datagram pair");
    set_cloexec_v1(child_socket_v1.as_raw_fd());
    let executable_v1 = pin_current_test_executable_v1();
    let mut child_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_inherited_unix_datagram_v1",
        DATAGRAM_HELPER_FD_MARKER_V1,
        OsStr::new("9"),
    )
    .with_child_unix_datagram_v1(child_socket_v1.as_fd(), 9)
    .expect("map explicit child Unix datagram")
    .spawn_v1()
    .expect("spawn datagram consumer");
    drop(child_v1.take_stdin_v1());

    harness_socket_v1
        .send(b"harness-to-child")
        .expect("send harness challenge");
    let mut response_v1 = [0_u8; 64];
    let received_v1 = harness_socket_v1
        .recv(&mut response_v1)
        .expect("receive child response");
    assert_eq!(&response_v1[..received_v1], b"child-to-harness");

    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("wait datagram child");
    assert_helper_success_v1(status_v1, &stdout_v1, &stderr_v1, "datagram helper");

    // The launch API must preserve CLOEXEC on the parent source.
    // SAFETY: F_GETFD observes the still-live test-owned descriptor.
    let flags_v1 = unsafe { libc::fcntl(child_socket_v1.as_raw_fd(), libc::F_GETFD) };
    assert_ne!(flags_v1 & libc::FD_CLOEXEC, 0);
}

#[test]
fn explicit_unix_datagram_mapping_rejects_wrong_or_inheritable_sources_v1() {
    let executable_v1 = pin_current_test_executable_v1();
    let (socket_v1, _peer_v1) = UnixDatagram::pair().expect("create datagram pair");
    clear_cloexec_v1(socket_v1.as_raw_fd());
    let error_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_inherited_unix_datagram_v1",
        DATAGRAM_HELPER_FD_MARKER_V1,
        OsStr::new("9"),
    )
    .with_child_unix_datagram_v1(socket_v1.as_fd(), 9)
    .expect_err("inheritable parent socket must fail closed");
    assert_eq!(
        error_v1,
        ExplicitFdProcessErrorV1::SourceDescriptorNotCloseOnExec
    );

    let stream_v1 = UnixStream::pair().expect("create stream pair").0;
    set_cloexec_v1(stream_v1.as_raw_fd());
    let error_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_inherited_unix_datagram_v1",
        DATAGRAM_HELPER_FD_MARKER_V1,
        OsStr::new("9"),
    )
    .with_child_unix_datagram_v1(stream_v1.as_fd(), 9)
    .expect_err("stream source must fail closed");
    assert_eq!(
        error_v1,
        ExplicitFdProcessErrorV1::InvalidChildUnixDatagramDescriptor
    );
}

#[test]
fn parallel_pipe_mappings_do_not_cross_inherit_v1() {
    let executable_v1 = std::sync::Arc::new(pin_current_test_executable_v1());
    let workers_v1 = (0..8)
        .map(|index_v1| {
            let executable_v1 = executable_v1.clone();
            std::thread::spawn(move || {
                let (read_v1, write_v1) = cloexec_pipe_v1();
                let expected_v1 = format!("challenge-{index_v1}\n");
                let mut writer_v1 = File::from(write_v1);
                writer_v1
                    .write_all(expected_v1.as_bytes())
                    .expect("write parallel challenge");
                drop(writer_v1);
                let mut child_v1 = pipe_helper_request_v1(&executable_v1, &expected_v1)
                    .with_child_pipe_read_v1(read_v1.as_fd(), 9)
                    .expect("map parallel challenge")
                    .spawn_v1()
                    .expect("spawn parallel consumer");
                drop(child_v1.take_stdin_v1());
                let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
                let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
                let status_v1 = child_v1.wait_v1().expect("wait parallel child");
                assert_helper_success_v1(status_v1, &stdout_v1, &stderr_v1, "parallel pipe helper");
            })
        })
        .collect::<Vec<_>>();
    for worker_v1 in workers_v1 {
        worker_v1.join().expect("parallel mapping worker");
    }
}

#[test]
fn unrelated_cloexec_descriptor_is_absent_from_child_v1() {
    let unrelated_v1 =
        File::open(current_test_executable_path_v1()).expect("open unrelated descriptor");
    let current_executable_v1 = std::env::current_exe().expect("current test executable");
    let executable_v1 = pin_current_test_executable_v1();
    let mut environment_v1 = std::env::vars_os().collect::<Vec<_>>();
    environment_v1.push((
        "QUANTA_EXPLICIT_FD_NEGATIVE_PROBE_V1".into(),
        unrelated_v1.as_raw_fd().to_string().into(),
    ));
    let mut child_v1 = ExplicitFdSpawnRequestV1::try_new_v1(
        &executable_v1,
        [
            current_executable_v1.as_os_str(),
            std::ffi::OsStr::new("--exact"),
            std::ffi::OsStr::new("tests::child_probe_unrelated_fd_is_closed_v1"),
            std::ffi::OsStr::new("--ignored"),
        ],
        environment_v1,
    )
    .expect("build descriptor leak probe")
    .spawn_v1()
    .expect("spawn descriptor leak probe");
    drop(child_v1.take_stdin_v1());
    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("wait descriptor leak probe");
    assert!(
        status_v1.success(),
        "descriptor leak probe failed: stdout={} stderr={}",
        String::from_utf8_lossy(&stdout_v1),
        String::from_utf8_lossy(&stderr_v1)
    );
}

#[test]
fn child_entry_helper_duplicates_only_the_explicit_inherited_pipe_v1() {
    let (read_v1, write_v1) = cloexec_pipe_v1();
    let mut writer_v1 = File::from(write_v1);
    writer_v1
        .write_all(b"child-entry-helper\n")
        .expect("write child-entry helper fixture");
    drop(writer_v1);
    let executable_v1 = pin_current_test_executable_v1();
    let mut child_v1 = pipe_helper_request_v1(&executable_v1, "child-entry-helper\n")
        .with_child_pipe_read_v1(read_v1.as_fd(), 9)
        .expect("map child-entry helper pipe")
        .spawn_v1()
        .expect("spawn child-entry helper probe");
    drop(child_v1.take_stdin_v1());
    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("wait child-entry helper probe");
    assert!(
        status_v1.success(),
        "child-entry helper probe failed: stdout={} stderr={}",
        String::from_utf8_lossy(&stdout_v1),
        String::from_utf8_lossy(&stderr_v1)
    );
}

#[test]
#[ignore = "subprocess-only test helper"]
fn child_probe_inherited_read_pipe_v1() {
    let raw_v1 = std::env::var_os(PIPE_HELPER_FD_MARKER_V1)
        .expect("child pipe helper marker must be set by the parent test");
    let raw_v1 = raw_v1
        .to_string_lossy()
        .parse::<i32>()
        .expect("parse child pipe descriptor");
    let mut duplicate_v1 =
        try_duplicate_inherited_read_pipe_v1(raw_v1).expect("duplicate inherited read pipe");
    let mut bytes_v1 = Vec::new();
    duplicate_v1
        .read_to_end(&mut bytes_v1)
        .expect("read duplicated child pipe");
    let expected_v1 =
        std::env::var(PIPE_HELPER_EXPECTED_MARKER_V1).expect("pipe helper expected bytes marker");
    assert_eq!(bytes_v1, expected_v1.as_bytes());
    // SAFETY: F_GETFD observes the owned duplicate's descriptor flags.
    let flags_v1 = unsafe { libc::fcntl(duplicate_v1.as_raw_fd(), libc::F_GETFD) };
    assert_ne!(flags_v1 & libc::FD_CLOEXEC, 0);
}

#[test]
#[ignore = "subprocess-only test helper"]
fn child_probe_inherited_unix_datagram_v1() {
    let raw_v1 = std::env::var_os(DATAGRAM_HELPER_FD_MARKER_V1)
        .expect("child datagram helper marker must be set by the parent test")
        .to_string_lossy()
        .parse::<i32>()
        .expect("parse child datagram descriptor");
    let socket_v1 = try_duplicate_inherited_unix_datagram_v1(raw_v1)
        .expect("duplicate inherited child datagram");
    let mut challenge_v1 = [0_u8; 64];
    let received_v1 = socket_v1
        .recv(&mut challenge_v1)
        .expect("receive datagram challenge");
    assert_eq!(&challenge_v1[..received_v1], b"harness-to-child");
    socket_v1
        .send(b"child-to-harness")
        .expect("send datagram response");
}

#[test]
#[ignore = "subprocess-only test helper"]
fn child_probe_unrelated_fd_is_closed_v1() {
    let raw_v1 = std::env::var_os("QUANTA_EXPLICIT_FD_NEGATIVE_PROBE_V1")
        .expect("negative descriptor helper marker must be set by the parent test");
    let raw_v1 = raw_v1
        .to_string_lossy()
        .parse::<i32>()
        .expect("parse descriptor probe");
    // SAFETY: F_GETFD observes whether the inherited numeric descriptor exists.
    let result_v1 = unsafe { libc::fcntl(raw_v1, libc::F_GETFD) };
    assert_eq!(
        result_v1, -1,
        "unrelated parent descriptor leaked into child"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EBADF)
    );
}

#[test]
#[ignore = "subprocess-only test helper"]
fn child_probe_stdio_round_trip_v1() {
    std::env::var_os(STDIO_HELPER_MARKER_V1)
        .expect("stdio helper marker must be set by the parent test");
    let mut bytes_v1 = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes_v1)
        .expect("read helper stdin");
    assert_eq!(bytes_v1, b"custody-round-trip\n");
}

#[test]
#[ignore = "subprocess-only test helper"]
fn child_probe_original_image_v1() {
    std::env::var_os(ORIGINAL_IMAGE_HELPER_MARKER_V1)
        .expect("original-image helper marker must be set by the parent test");
}

#[test]
#[ignore = "subprocess-only test helper"]
fn child_probe_regular_file_mapping_v1() {
    let raw_v1 = std::env::var_os(REGULAR_FILE_HELPER_MARKER_V1)
        .expect("regular-file helper marker must be set by the parent test");
    let raw_v1 = raw_v1
        .to_string_lossy()
        .parse::<i32>()
        .expect("parse regular-file descriptor");
    // SAFETY: F_GETFD observes the explicit child-entry descriptor.
    let inherited_flags_v1 = unsafe { libc::fcntl(raw_v1, libc::F_GETFD) };
    assert!(inherited_flags_v1 >= 0, "regular-file mapping must exist");
    assert_eq!(inherited_flags_v1 & libc::FD_CLOEXEC, 0);
    // SAFETY: F_GETFL observes the explicit inherited regular-file descriptor.
    let access_flags_v1 = unsafe { libc::fcntl(raw_v1, libc::F_GETFL) };
    assert!(access_flags_v1 >= 0);
    assert_eq!(access_flags_v1 & libc::O_ACCMODE, libc::O_RDONLY);
    // SAFETY: F_DUPFD_CLOEXEC creates a fresh owned duplicate for the helper.
    let duplicate_v1 = unsafe { libc::fcntl(raw_v1, libc::F_DUPFD_CLOEXEC, 3) };
    assert!(duplicate_v1 >= 0, "duplicate regular-file mapping");
    // SAFETY: fcntl returned a fresh descriptor owned by this helper.
    let mut file_v1 = File::from(unsafe { OwnedFd::from_raw_fd(duplicate_v1) });
    let mut bytes_v1 = Vec::new();
    file_v1
        .read_to_end(&mut bytes_v1)
        .expect("read regular-file mapping");
    assert_eq!(bytes_v1, b"pinned-authority\n");
}

#[test]
#[ignore = "subprocess-only test helper"]
fn child_probe_working_directory_v1() {
    let expected_v1 = std::env::var_os(CWD_HELPER_MARKER_V1)
        .expect("working-directory helper marker must be set by the parent test");
    assert_eq!(
        std::env::current_dir().expect("read helper cwd"),
        std::path::PathBuf::from(expected_v1)
    );
}

#[test]
#[ignore = "subprocess-only test helper"]
fn child_probe_sleep_v1() {
    std::env::var_os(SLEEP_HELPER_MARKER_V1)
        .expect("sleep helper marker must be set by the parent test");
    std::thread::sleep(std::time::Duration::from_secs(30));
}

#[test]
#[ignore = "subprocess-only test helper"]
fn child_probe_pipe_holding_descendant_v1() {
    let fixture_v1 = std::env::var(DESCENDANT_HELPER_MARKER_V1)
        .expect("descendant helper marker must be set by the parent test");
    let (mode_v1, identity_path_v1) = fixture_v1
        .split_once('\n')
        .expect("descendant helper marker must contain mode and identity path");
    // SAFETY: the child branch invokes only libc sleep/_exit after fork. The
    // parent branch remains the test-helper process and owns the child PID.
    let descendant_pid_v1 = unsafe { libc::fork() };
    assert!(descendant_pid_v1 >= 0, "fork descendant fixture");
    if descendant_pid_v1 == 0 {
        if mode_v1 == "leader-exits" {
            // SAFETY: closing inherited stdio is async-signal-safe.
            unsafe {
                libc::close(libc::STDIN_FILENO);
                libc::close(libc::STDOUT_FILENO);
                libc::close(libc::STDERR_FILENO);
            }
        }
        // SAFETY: sleep and _exit are async-signal-safe and touch no Rust state.
        unsafe {
            libc::sleep(30);
            libc::_exit(0);
        }
    }
    std::fs::write(
        identity_path_v1,
        format!("{} {descendant_pid_v1}\n", std::process::id()),
    )
    .expect("write descendant identity");
    if mode_v1 == "leader-exits" {
        return;
    }
    std::thread::sleep(std::time::Duration::from_secs(30));
}

#[test]
fn pinned_working_directory_is_applied_by_descriptor_v1() {
    let directory_v1 = tempfile::tempdir().expect("temporary working directory");
    let directory_file_v1 = File::open(directory_v1.path()).expect("open working directory");
    let working_directory_v1 =
        PinnedWorkingDirectoryV1::try_pin_v1(&directory_file_v1).expect("pin working directory");
    let executable_v1 = pin_current_test_executable_v1();
    let canonical_directory_v1 = directory_v1.path().canonicalize().expect("canonical cwd");
    let mut child_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_working_directory_v1",
        CWD_HELPER_MARKER_V1,
        canonical_directory_v1.as_os_str(),
    )
    .with_working_directory_v1(&working_directory_v1)
    .spawn_v1()
    .expect("spawn in pinned working directory");
    drop(child_v1.take_stdin_v1());
    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("wait child");
    assert_helper_success_v1(status_v1, &stdout_v1, &stderr_v1, "cwd helper");
}

#[test]
fn inheritable_source_descriptor_is_rejected_without_parent_mutation_v1() {
    let current_executable_v1 = current_test_executable_path_v1();
    let source_v1 = File::open(&current_executable_v1).expect("open current test executable");
    // SAFETY: dup returns a new owned descriptor without FD_CLOEXEC.
    let raw_v1 = unsafe { libc::dup(source_v1.as_raw_fd()) };
    assert!(
        raw_v1 >= 0,
        "dup must create the negative fixture descriptor"
    );
    // SAFETY: successful dup returned a fresh descriptor owned by the test.
    let inheritable_v1 = unsafe { OwnedFd::from_raw_fd(raw_v1) };
    let error_v1 =
        PinnedExecutableV1::try_pin_v1(&inheritable_v1, &sha256_path_v1(&current_executable_v1))
            .expect_err("inheritable source must fail closed");
    assert_eq!(
        error_v1,
        ExplicitFdProcessErrorV1::SourceDescriptorNotCloseOnExec
    );
    // SAFETY: F_GETFD reads flags from the still-live negative fixture.
    let flags_v1 = unsafe { libc::fcntl(inheritable_v1.as_raw_fd(), libc::F_GETFD) };
    assert_eq!(
        flags_v1 & libc::FD_CLOEXEC,
        0,
        "launcher must not mutate parent flags"
    );
}

#[test]
fn writable_regular_file_cannot_enter_authority_mapping_v1() {
    let source_v1 = tempfile::NamedTempFile::new().expect("temporary authority file");
    std::fs::write(source_v1.path(), b"authority\n").expect("write authority fixture");
    let digest_v1 = sha256_path_v1(source_v1.path());
    let error_v1 = PinnedReadOnlyFileV1::try_pin_v1(source_v1.as_file(), &digest_v1)
        .expect_err("read-write descriptor must fail closed");
    assert_eq!(
        error_v1,
        ExplicitFdProcessErrorV1::SourceDescriptorNotReadOnly
    );
}

#[test]
fn dropped_child_is_killed_and_reaped_v1() {
    let executable_v1 = pin_current_test_executable_v1();
    let child_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_sleep_v1",
        SLEEP_HELPER_MARKER_V1,
        OsStr::new("1"),
    )
    .spawn_v1()
    .expect("spawn sleep child");
    let pid_v1 = child_v1.pid_v1();
    drop(child_v1);
    let mut status_v1 = 0;
    // SAFETY: a successful Drop already reaped this formerly-owned PID.
    let wait_v1 = unsafe { libc::waitpid(pid_v1, &mut status_v1, libc::WNOHANG) };
    assert_eq!(wait_v1, -1, "child Drop must leave no waitable zombie");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[test]
fn try_wait_reports_running_then_reaps_after_kill_v1() {
    let executable_v1 = pin_current_test_executable_v1();
    let mut child_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_sleep_v1",
        SLEEP_HELPER_MARKER_V1,
        OsStr::new("1"),
    )
    .spawn_v1()
    .expect("spawn sleep child");
    assert_eq!(child_v1.try_wait_v1().expect("poll live child"), None);
    child_v1.kill_v1().expect("kill live child");
    let deadline_v1 = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status_v1 = loop {
        if let Some(status_v1) = child_v1.try_wait_v1().expect("poll killed child") {
            break status_v1;
        }
        assert!(
            std::time::Instant::now() < deadline_v1,
            "killed child must become waitable within five seconds"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    assert!(!status_v1.success());
    assert_eq!(
        child_v1.try_wait_v1(),
        Err(ExplicitFdProcessErrorV1::ChildAlreadyWaited)
    );
}

#[test]
fn isolated_process_group_kill_closes_descendant_held_stdout_v1() {
    let (mut child_v1, stdout_v1, descendant_pid_v1) =
        spawn_pipe_holding_descendant_v1("hold-pipes");
    child_v1.kill_v1().expect("kill isolated process group");
    let deadline_v1 = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if child_v1
            .try_wait_v1()
            .expect("poll killed group leader")
            .is_some()
        {
            break;
        }
        if std::time::Instant::now() >= deadline_v1 {
            let leader_pid_v1 = child_v1.pid_v1();
            fallback_kill_fixture_v1(leader_pid_v1, descendant_pid_v1);
            detach_child_drop_v1(child_v1);
            panic!("reap group leader timed out");
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_stdout_closes_v1(stdout_v1, descendant_pid_v1);
}

#[cfg(target_os = "macos")]
#[test]
fn isolated_kill_after_leader_exit_reaps_zombie_v1() {
    let executable_v1 = pin_current_test_executable_v1();
    let mut child_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_original_image_v1",
        ORIGINAL_IMAGE_HELPER_MARKER_V1,
        OsStr::new("1"),
    )
    .with_isolated_process_group_v1()
    .spawn_v1()
    .expect("spawn exiting group leader");
    drop(child_v1.take_stdin_v1());
    let pid_v1 = child_v1.pid_v1();
    let deadline_v1 = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !crate::platform::observe_pid_exit_without_reap_v1(pid_v1, true)
        .expect("observe owned group leader")
    {
        assert!(
            std::time::Instant::now() < deadline_v1,
            "group leader must exit before cleanup"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    child_v1
        .kill_v1()
        .expect("kill exit-confirmed isolated group");
    assert!(
        child_v1
            .try_wait_v1()
            .expect("reap exited group leader")
            .is_some(),
        "exit-confirmed group leader must be reaped"
    );
    let mut status_v1 = 0;
    // SAFETY: try_wait_v1 reaped the owned leader before this observation.
    assert_eq!(
        unsafe { libc::waitpid(pid_v1, &mut status_v1, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}

#[test]
fn isolated_process_group_drop_closes_descendant_held_stdout_v1() {
    let (child_v1, stdout_v1, descendant_pid_v1) = spawn_pipe_holding_descendant_v1("hold-pipes");
    let leader_pid_v1 = child_v1.pid_v1();
    let (drop_tx_v1, drop_rx_v1) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        drop(child_v1);
        let _ = drop_tx_v1.send(());
    });
    if drop_rx_v1
        .recv_timeout(std::time::Duration::from_secs(5))
        .is_err()
    {
        fallback_kill_fixture_v1(leader_pid_v1, descendant_pid_v1);
        panic!("isolated process-group Drop exceeded the owner-test deadline");
    }
    assert_stdout_closes_v1(stdout_v1, descendant_pid_v1);
}

#[test]
fn isolated_wait_terminates_same_group_descendant_after_clean_leader_exit_v1() {
    let (mut child_v1, _stdout_v1, descendant_pid_v1) =
        spawn_pipe_holding_descendant_v1("leader-exits");
    let leader_pid_v1 = child_v1.pid_v1();
    let (wait_tx_v1, wait_rx_v1) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result_v1 = child_v1.wait_v1();
        let _ = wait_tx_v1.send(result_v1);
    });
    let status_v1 = match wait_rx_v1.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(result_v1) => result_v1.expect("wait clean group leader"),
        Err(error_v1) => {
            fallback_kill_fixture_v1(leader_pid_v1, descendant_pid_v1);
            panic!("clean group wait exceeded owner-test deadline: {error_v1}");
        }
    };
    assert!(
        status_v1.success(),
        "group cleanup must preserve leader status"
    );
    let deadline_v1 = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        // SAFETY: signal zero only observes the recorded fixture PID.
        let present_v1 = unsafe { libc::kill(descendant_pid_v1, 0) } == 0;
        if !present_v1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            break;
        }
        if std::time::Instant::now() >= deadline_v1 {
            // SAFETY: test-only fallback prevents a failed fixture from
            // surviving the owner test.
            unsafe { libc::kill(descendant_pid_v1, libc::SIGKILL) };
            panic!("same-group descendant survived clean leader wait");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn spawn_pipe_holding_descendant_v1(mode_v1: &str) -> (ExplicitFdChildV1, File, libc::pid_t) {
    let executable_v1 = pin_current_test_executable_v1();
    let identity_directory_v1 = tempfile::tempdir().expect("descendant identity directory");
    let identity_path_v1 = identity_directory_v1.path().join("identity-v1");
    let fixture_v1 = format!("{mode_v1}\n{}", identity_path_v1.display());
    let mut child_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::child_probe_pipe_holding_descendant_v1",
        DESCENDANT_HELPER_MARKER_V1,
        OsStr::new(&fixture_v1),
    )
    .with_isolated_process_group_v1()
    .spawn_v1()
    .expect("spawn isolated process-group fixture");
    drop(child_v1.take_stdin_v1());
    let stdout_v1 = child_v1.take_stdout_v1().expect("fixture stdout");
    let identity_deadline_v1 = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let identity_v1 = loop {
        match std::fs::read_to_string(&identity_path_v1) {
            Ok(identity_v1) if !identity_v1.trim().is_empty() => break identity_v1,
            Ok(_) | Err(_) if std::time::Instant::now() < identity_deadline_v1 => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(_) | Err(_) => {
                let leader_pid_v1 = child_v1.pid_v1();
                let _ = child_v1.kill_v1();
                fallback_kill_fixture_v1(leader_pid_v1, leader_pid_v1);
                detach_child_drop_v1(child_v1);
                panic!("descendant fixture identity timed out");
            }
        }
    };
    let mut fields_v1 = identity_v1.split_whitespace();
    let leader_pid_v1 = fields_v1
        .next()
        .expect("leader pid")
        .parse::<libc::pid_t>()
        .expect("canonical leader pid");
    let descendant_pid_v1 = fields_v1
        .next()
        .expect("descendant pid")
        .parse::<libc::pid_t>()
        .expect("canonical descendant pid");
    assert!(fields_v1.next().is_none(), "exact identity fields");
    assert_eq!(leader_pid_v1, child_v1.pid_v1());
    // SAFETY: the descendant remains live for the fixture duration, so its
    // process-group identity directly proves inheritance from the isolated
    // leader even when the leader-exits mode has already closed the leader.
    assert_eq!(
        unsafe { libc::getpgid(descendant_pid_v1) },
        child_v1.pid_v1()
    );
    (child_v1, stdout_v1, descendant_pid_v1)
}

fn fallback_kill_fixture_v1(leader_pid_v1: libc::pid_t, descendant_pid_v1: libc::pid_t) {
    // SAFETY: test-only fallback targets exact PIDs recorded from this fixture.
    unsafe {
        libc::kill(-leader_pid_v1, libc::SIGKILL);
        libc::kill(leader_pid_v1, libc::SIGKILL);
        libc::kill(descendant_pid_v1, libc::SIGKILL);
    }
}

fn detach_child_drop_v1(child_v1: ExplicitFdChildV1) {
    let _ = std::thread::Builder::new()
        .name("explicit-fd-test-cleanup-v1".into())
        .spawn(move || drop(child_v1));
}

fn assert_stdout_closes_v1(stdout_v1: File, descendant_pid_v1: libc::pid_t) {
    let (done_tx_v1, done_rx_v1) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut stdout_v1 = stdout_v1;
        let mut remainder_v1 = Vec::new();
        let result_v1 = stdout_v1.read_to_end(&mut remainder_v1);
        let _ = done_tx_v1.send((result_v1, remainder_v1));
    });
    match done_rx_v1.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok((result_v1, _remainder_v1)) => {
            result_v1.expect("read descendant-held stdout to EOF");
        }
        Err(error_v1) => {
            // SAFETY: test-only fallback prevents a failed RED fixture from
            // leaking the recorded descendant beyond the owner test.
            unsafe { libc::kill(descendant_pid_v1, libc::SIGKILL) };
            panic!("isolated group stdout did not close: {error_v1}");
        }
    }
}

fn pin_current_test_executable_v1() -> PinnedExecutableV1 {
    static PINNED_V1: OnceLock<PinnedExecutableV1> = OnceLock::new();
    PINNED_V1
        .get_or_init(|| {
            let path_v1 = current_test_executable_path_v1();
            let source_v1 = File::open(&path_v1).expect("open current test executable");
            PinnedExecutableV1::try_pin_v1(&source_v1, &sha256_path_v1(&path_v1))
                .expect("pin current test executable")
        })
        .clone()
}

fn current_test_executable_path_v1() -> std::path::PathBuf {
    std::env::current_exe().expect("current test executable")
}

fn test_helper_request_v1<'a>(
    executable_v1: &'a PinnedExecutableV1,
    exact_test_v1: &str,
    marker_key_v1: &str,
    marker_value_v1: &OsStr,
) -> ExplicitFdSpawnRequestV1<'a> {
    let current_executable_v1 = current_test_executable_path_v1();
    let mut environment_v1 = std::env::vars_os().collect::<Vec<_>>();
    environment_v1.push((marker_key_v1.into(), marker_value_v1.to_owned()));
    ExplicitFdSpawnRequestV1::try_new_v1(
        executable_v1,
        [
            current_executable_v1.as_os_str(),
            OsStr::new("--exact"),
            OsStr::new(exact_test_v1),
            OsStr::new("--ignored"),
        ],
        environment_v1,
    )
    .expect("build exact test-helper request")
}

fn pipe_helper_request_v1<'a>(
    executable_v1: &'a PinnedExecutableV1,
    expected_v1: &str,
) -> ExplicitFdSpawnRequestV1<'a> {
    let current_executable_v1 = current_test_executable_path_v1();
    let mut environment_v1 = std::env::vars_os().collect::<Vec<_>>();
    environment_v1.push((PIPE_HELPER_FD_MARKER_V1.into(), "9".into()));
    environment_v1.push((PIPE_HELPER_EXPECTED_MARKER_V1.into(), expected_v1.into()));
    ExplicitFdSpawnRequestV1::try_new_v1(
        executable_v1,
        [
            current_executable_v1.as_os_str(),
            OsStr::new("--exact"),
            OsStr::new("tests::child_probe_inherited_read_pipe_v1"),
            OsStr::new("--ignored"),
        ],
        environment_v1,
    )
    .expect("build pipe test-helper request")
}

fn cloexec_pipe_v1() -> (OwnedFd, OwnedFd) {
    let mut descriptors_v1 = [-1, -1];
    #[cfg(target_os = "linux")]
    {
        // SAFETY: pipe2 initializes two fresh CLOEXEC descriptors.
        assert_eq!(
            unsafe { libc::pipe2(descriptors_v1.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
    }
    #[cfg(target_os = "macos")]
    {
        // Test-only construction: production Darwin stdio uses private FIFOs
        // because Darwin has no atomic pipe2 CLOEXEC surface.
        // SAFETY: pipe initializes two fresh descriptors for this isolated test.
        assert_eq!(unsafe { libc::pipe(descriptors_v1.as_mut_ptr()) }, 0);
        for descriptor_v1 in descriptors_v1 {
            // SAFETY: F_SETFD marks the isolated test descriptor CLOEXEC.
            assert_eq!(
                unsafe { libc::fcntl(descriptor_v1, libc::F_SETFD, libc::FD_CLOEXEC) },
                0
            );
        }
    }
    // SAFETY: pipe/pipe2 returned two fresh descriptors owned by the test.
    let read_v1 = unsafe { OwnedFd::from_raw_fd(descriptors_v1[0]) };
    // SAFETY: pipe/pipe2 returned two fresh descriptors owned by the test.
    let write_v1 = unsafe { OwnedFd::from_raw_fd(descriptors_v1[1]) };
    (read_v1, write_v1)
}

fn read_all_v1(mut file_v1: File) -> Vec<u8> {
    let mut bytes_v1 = Vec::new();
    file_v1.read_to_end(&mut bytes_v1).expect("read child pipe");
    bytes_v1
}

fn assert_helper_success_v1(
    status_v1: std::process::ExitStatus,
    stdout_v1: &[u8],
    stderr_v1: &[u8],
    helper_v1: &str,
) {
    assert!(
        status_v1.success(),
        "{helper_v1} failed with {status_v1}: stdout={} stderr={}",
        String::from_utf8_lossy(stdout_v1),
        String::from_utf8_lossy(stderr_v1)
    );
}

fn sha256_path_v1(path_v1: &std::path::Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(std::fs::read(path_v1).expect("read digest fixture"))
    )
}
