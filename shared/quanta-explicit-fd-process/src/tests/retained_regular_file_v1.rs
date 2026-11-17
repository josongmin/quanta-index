use super::*;

const RETAINED_FILE_MARKER_V1: &str = "QUANTA_EXPLICIT_FD_RETAINED_FILE_V1";
const READ_ONLY_FILE_MARKER_V1: &str = "QUANTA_EXPLICIT_FD_READ_ONLY_FILE_V1";
const CHILD_CONTROL_FD_V1: i32 = 8;
const CHILD_FILE_FD_V1: i32 = 9;
const CHILD_READ_ONLY_FILE_FD_V1: i32 = 6;

fn retained_file_request_v1<'a>(
    executable_v1: &'a PinnedExecutableV1,
) -> ExplicitFdSpawnRequestV1<'a> {
    test_helper_request_v1(
        executable_v1,
        "tests::retained_regular_file_v1::child_probe_retained_file_v1",
        RETAINED_FILE_MARKER_V1,
        OsStr::new("9"),
    )
}

fn assert_cloexec_v1(file_v1: &File) {
    // SAFETY: F_GETFD only observes this live test-owned descriptor.
    let flags_v1 = unsafe { libc::fcntl(file_v1.as_raw_fd(), libc::F_GETFD) };
    assert!(flags_v1 >= 0);
    assert_ne!(flags_v1 & libc::FD_CLOEXEC, 0);
}

#[test]
fn retained_file_mapping_keeps_flock_after_parent_file_drop_v1() {
    let fixture_v1 = tempfile::NamedTempFile::new().expect("create retained file");
    let owner_v1 = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture_v1.path())
        .expect("open read-write retained file");
    assert_cloexec_v1(&owner_v1);
    // SAFETY: flock locks the live file description retained by this test.
    assert_eq!(unsafe { libc::flock(owner_v1.as_raw_fd(), libc::LOCK_EX) }, 0);

    let (child_socket_v1, parent_socket_v1) = UnixDatagram::pair().expect("control socket pair");
    set_cloexec_v1(child_socket_v1.as_raw_fd());
    parent_socket_v1
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .expect("bound child ready wait");
    let executable_v1 = pin_current_test_executable_v1();
    let mut child_v1 = retained_file_request_v1(&executable_v1)
        .with_child_unix_datagram_v1(child_socket_v1.as_fd(), CHILD_CONTROL_FD_V1)
        .expect("map exact child control")
        .with_child_retained_regular_file_v1(owner_v1.as_fd(), CHILD_FILE_FD_V1)
        .expect("map retained regular file")
        .spawn_v1()
        .expect("spawn retained file child");
    drop(child_socket_v1);
    assert_cloexec_v1(&owner_v1);

    let mut ready_v1 = [0_u8; 1];
    assert_eq!(parent_socket_v1.recv(&mut ready_v1).expect("child ready"), 1);
    assert_eq!(ready_v1, [b'R']);
    drop(owner_v1);

    let contender_v1 = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture_v1.path())
        .expect("open independent lock contender");
    assert_eq!(
        // SAFETY: contender is a distinct live open file description.
        unsafe { libc::flock(contender_v1.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        -1,
        "child must retain the original lock after parent File drop"
    );
    let lock_errno_v1 = std::io::Error::last_os_error().raw_os_error();
    assert!(
        lock_errno_v1 == Some(libc::EWOULDBLOCK) || lock_errno_v1 == Some(libc::EAGAIN),
        "unexpected lock refusal: {lock_errno_v1:?}"
    );

    parent_socket_v1.send(b"X").expect("release child");
    drop(child_v1.take_stdin_v1());
    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("reap retained file child");
    assert_helper_success_v1(status_v1, &stdout_v1, &stderr_v1, "retained file helper");

    assert_eq!(
        // SAFETY: the child has exited; the independent contender can now lock.
        unsafe { libc::flock(contender_v1.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
}

#[test]
fn retained_file_mapping_rejects_wrong_sources_and_targets_v1() {
    let executable_v1 = pin_current_test_executable_v1();
    let fixture_v1 = tempfile::NamedTempFile::new().expect("create retained file");
    let owner_v1 = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture_v1.path())
        .expect("open retained file");
    let readonly_v1 = File::open(fixture_v1.path()).expect("open read-only file");
    let directory_v1 =
        File::open(fixture_v1.path().parent().expect("parent directory")).expect("open directory");
    let (pipe_v1, _writer_v1) = cloexec_pipe_v1();
    for descriptor_v1 in [readonly_v1.as_fd(), directory_v1.as_fd(), pipe_v1.as_fd()] {
        assert_eq!(
            retained_file_request_v1(&executable_v1)
                .with_child_retained_regular_file_v1(descriptor_v1, CHILD_FILE_FD_V1)
                .expect_err("non-RW regular source must be rejected"),
            ExplicitFdProcessErrorV1::InvalidRetainedRegularFileDescriptor
        );
    }
    clear_cloexec_v1(owner_v1.as_raw_fd());
    assert_eq!(
        retained_file_request_v1(&executable_v1)
            .with_child_retained_regular_file_v1(owner_v1.as_fd(), CHILD_FILE_FD_V1)
            .expect_err("inheritable parent file must be rejected"),
        ExplicitFdProcessErrorV1::SourceDescriptorNotCloseOnExec
    );
    set_cloexec_v1(owner_v1.as_raw_fd());
    for target_v1 in [0, 1, 2] {
        assert_eq!(
            retained_file_request_v1(&executable_v1)
                .with_child_retained_regular_file_v1(owner_v1.as_fd(), target_v1)
                .expect_err("stdio target must be rejected"),
            ExplicitFdProcessErrorV1::InvalidChildDescriptor {
                child_fd_v1: target_v1
            }
        );
    }
    assert_eq!(
        retained_file_request_v1(&executable_v1)
            .with_child_retained_regular_file_v1(owner_v1.as_fd(), CHILD_FILE_FD_V1)
            .expect("first mapping")
            .with_child_retained_regular_file_v1(owner_v1.as_fd(), CHILD_FILE_FD_V1)
            .expect_err("duplicate child target must be rejected"),
        ExplicitFdProcessErrorV1::DuplicateChildDescriptor {
            child_fd_v1: CHILD_FILE_FD_V1
        }
    );

    let request_v1 = retained_file_request_v1(&executable_v1)
        .with_child_retained_regular_file_v1(owner_v1.as_fd(), CHILD_FILE_FD_V1)
        .expect("build mapping");
    clear_cloexec_v1(owner_v1.as_raw_fd());
    assert_eq!(
        request_v1
            .spawn_v1()
            .expect_err("spawn must revalidate source"),
        ExplicitFdProcessErrorV1::SourceDescriptorNotCloseOnExec
    );
    set_cloexec_v1(owner_v1.as_raw_fd());
}

#[test]
fn inherited_regular_file_requires_explicit_unsealed_rw_mapping_v1() {
    assert_eq!(
        super::super::try_duplicate_inherited_regular_file_v1(libc::STDERR_FILENO)
            .expect_err("stdio rejected"),
        ExplicitFdProcessErrorV1::InvalidInheritedRegularFileDescriptor
    );
    let fixture_v1 = tempfile::NamedTempFile::new().expect("create retained file");
    let owner_v1 = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture_v1.path())
        .expect("open retained file");
    assert_eq!(
        super::super::try_duplicate_inherited_regular_file_v1(owner_v1.as_raw_fd())
            .expect_err("CLOEXEC parent source rejected"),
        ExplicitFdProcessErrorV1::InvalidInheritedRegularFileDescriptor
    );
    clear_cloexec_v1(owner_v1.as_raw_fd());
    let duplicate_v1 = super::super::try_duplicate_inherited_regular_file_v1(owner_v1.as_raw_fd())
        .expect("duplicate explicit unsealed regular file");
    assert_cloexec_v1(&owner_v1);
    assert_cloexec_v1(&duplicate_v1);
}

#[test]
fn optional_inherited_regular_file_distinguishes_absent_from_malformed_v1() {
    let import_v1 = super::super::try_duplicate_optional_inherited_regular_file_v1;
    assert!(import_v1(i32::MAX).expect("absent descriptor").is_none());
    let (socket_v1, _peer_v1) = std::os::unix::net::UnixDatagram::pair()
        .expect("create malformed inherited descriptor");
    clear_cloexec_v1(socket_v1.as_raw_fd());
    assert_eq!(
        import_v1(socket_v1.as_raw_fd()).expect_err("present non-file descriptor"),
        ExplicitFdProcessErrorV1::InvalidInheritedRegularFileDescriptor
    );
    let fixture_v1 = tempfile::NamedTempFile::new().expect("create retained file");
    let owner_v1 = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture_v1.path())
        .expect("open retained file");
    clear_cloexec_v1(owner_v1.as_raw_fd());
    let duplicate_v1 = import_v1(owner_v1.as_raw_fd())
        .expect("import mapped descriptor")
        .expect("present descriptor");
    assert_cloexec_v1(&owner_v1);
    assert_cloexec_v1(&duplicate_v1);
}

#[test]
fn inherited_read_only_regular_file_rejects_wrong_sources_v1() {
    use super::super::try_duplicate_inherited_read_only_regular_file_v1;

    for raw_v1 in [-1, 0, 1, 2] {
        assert_eq!(
            try_duplicate_inherited_read_only_regular_file_v1(raw_v1)
                .expect_err("invalid or stdio fd must be rejected"),
            ExplicitFdProcessErrorV1::InvalidInheritedReadOnlyRegularFileDescriptor
        );
    }
    let fixture_v1 = tempfile::NamedTempFile::new().expect("create evidence source");
    let readonly_v1 = File::open(fixture_v1.path()).expect("open read-only source");
    assert_eq!(
        try_duplicate_inherited_read_only_regular_file_v1(readonly_v1.as_raw_fd())
            .expect_err("CLOEXEC source must be rejected"),
        ExplicitFdProcessErrorV1::InvalidInheritedReadOnlyRegularFileDescriptor
    );

    let readwrite_v1 = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture_v1.path())
        .expect("open read-write file");
    let directory_v1 =
        File::open(fixture_v1.path().parent().expect("fixture directory")).expect("open directory");
    let (socket_v1, _peer_v1) = UnixDatagram::pair().expect("create socket pair");
    for raw_v1 in [
        readwrite_v1.as_raw_fd(),
        directory_v1.as_raw_fd(),
        socket_v1.as_raw_fd(),
    ] {
        clear_cloexec_v1(raw_v1);
        let result_v1 = try_duplicate_inherited_read_only_regular_file_v1(raw_v1);
        set_cloexec_v1(raw_v1);
        assert_eq!(
            result_v1.expect_err("wrong inherited source must be rejected"),
            ExplicitFdProcessErrorV1::InvalidInheritedReadOnlyRegularFileDescriptor
        );
    }
}

#[test]
fn pinned_read_only_mapping_uses_inherited_child_entry_v1() {
    let fixture_v1 = tempfile::NamedTempFile::new().expect("create pinned authority");
    std::fs::write(fixture_v1.path(), b"pinned-authority\n")
        .expect("write pinned authority fixture");
    let source_v1 = File::open(fixture_v1.path()).expect("open source authority");
    let pinned_v1 =
        PinnedReadOnlyFileV1::try_pin_v1(&source_v1, &sha256_path_v1(fixture_v1.path()))
            .expect("pin read-only authority");
    let executable_v1 = pin_current_test_executable_v1();
    let mut child_v1 = test_helper_request_v1(
        &executable_v1,
        "tests::retained_regular_file_v1::child_probe_read_only_regular_file_v1",
        READ_ONLY_FILE_MARKER_V1,
        OsStr::new("6"),
    )
    .with_pinned_read_only_file_v1(&pinned_v1, CHILD_READ_ONLY_FILE_FD_V1)
    .expect("map existing pinned read-only file")
    .spawn_v1()
    .expect("spawn pinned read-only child");
    drop(child_v1.take_stdin_v1());
    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("reap read-only child");
    assert_helper_success_v1(status_v1, &stdout_v1, &stderr_v1, "read-only child entry");
}

#[test]
#[ignore = "subprocess-only test helper"]
fn child_probe_read_only_regular_file_v1() {
    let raw_v1 = std::env::var(READ_ONLY_FILE_MARKER_V1)
        .expect("read-only marker")
        .parse::<i32>()
        .expect("read-only descriptor");
    let mut file_v1 = super::super::try_duplicate_inherited_read_only_regular_file_v1(raw_v1)
        .expect("take inherited read-only regular file");
    assert_cloexec_v1(&file_v1);
    // SAFETY: F_GETFD observes the inherited descriptor after child reseal.
    let inherited_flags_v1 = unsafe { libc::fcntl(raw_v1, libc::F_GETFD) };
    assert_ne!(inherited_flags_v1 & libc::FD_CLOEXEC, 0);
    let mut bytes_v1 = Vec::new();
    file_v1
        .read_to_end(&mut bytes_v1)
        .expect("read pinned authority");
    assert_eq!(bytes_v1, b"pinned-authority\n");
}

#[test]
#[ignore = "subprocess-only test helper"]
fn child_probe_retained_file_v1() {
    let raw_v1 = std::env::var(RETAINED_FILE_MARKER_V1)
        .expect("retained file marker")
        .parse::<i32>()
        .expect("retained file descriptor");
    let duplicate_v1 = super::super::try_duplicate_inherited_regular_file_v1(raw_v1)
        .expect("take explicit retained regular file");
    let control_v1 = super::super::try_duplicate_inherited_unix_datagram_v1(CHILD_CONTROL_FD_V1)
        .expect("take explicit child control");
    assert_cloexec_v1(&duplicate_v1);
    // SAFETY: F_GETFD observes the inherited mapped descriptor after resealing.
    let inherited_flags_v1 = unsafe { libc::fcntl(raw_v1, libc::F_GETFD) };
    assert_ne!(inherited_flags_v1 & libc::FD_CLOEXEC, 0);
    control_v1.send(b"R").expect("signal child ready");
    let mut release_v1 = [0_u8; 1];
    assert_eq!(control_v1.recv(&mut release_v1).expect("parent release"), 1);
    assert_eq!(release_v1, [b'X']);
    drop(duplicate_v1);
}
