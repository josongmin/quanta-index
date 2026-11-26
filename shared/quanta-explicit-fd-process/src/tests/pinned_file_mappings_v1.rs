use super::*;

#[test]
fn pinned_read_only_mapping_rejects_later_source_mutation_v1() {
    let source_v1 = tempfile::NamedTempFile::new().expect("temporary authority file");
    std::fs::write(source_v1.path(), b"pinned-authority\n").expect("write authority fixture");
    let digest_v1 = sha256_path_v1(source_v1.path());
    let source_file_v1 = File::open(source_v1.path()).expect("open authority source");
    let authority_v1 =
        PinnedReadOnlyFileV1::try_pin_v1(&source_file_v1, &digest_v1).expect("pin authority file");
    std::fs::write(source_v1.path(), b"mutated-authority\n").expect("mutate source inode");

    let executable_v1 = pin_current_test_executable_v1();
    assert_pinned_file_mapping_v1(&executable_v1, &authority_v1);
}

#[cfg(target_os = "linux")]
#[test]
fn pinned_read_only_mappings_have_independent_cursors_for_every_spawn_v1() {
    use std::io::{Seek, SeekFrom};

    let source_v1 = tempfile::NamedTempFile::new().expect("temporary authority file");
    std::fs::write(source_v1.path(), b"pinned-authority\n").expect("write authority bytes");
    let mut file_v1 = File::open(source_v1.path()).expect("open authority source");
    file_v1.seek(SeekFrom::Start(5)).expect("set caller cursor");
    let authority_v1 =
        PinnedReadOnlyFileV1::try_pin_v1(&file_v1, &sha256_path_v1(source_v1.path()))
            .expect("pin bytes independently of caller cursor");
    let executable_v1 = pin_current_test_executable_v1();
    for _ in 0..2 {
        assert_pinned_file_mapping_v1(&executable_v1, &authority_v1);
    }
    std::thread::scope(|scope_v1| {
        for _ in 0..2 {
            scope_v1.spawn(|| assert_pinned_file_mapping_v1(&executable_v1, &authority_v1));
        }
    });
    assert_eq!(file_v1.stream_position().expect("caller cursor"), 5);
}

fn assert_pinned_file_mapping_v1(
    executable_v1: &PinnedExecutableV1,
    authority_v1: &PinnedReadOnlyFileV1,
) {
    let mut child_v1 = test_helper_request_v1(
        executable_v1,
        "tests::child_probe_regular_file_mapping_v1",
        REGULAR_FILE_HELPER_MARKER_V1,
        OsStr::new("9"),
    )
    .with_pinned_read_only_file_v1(authority_v1, 9)
    .expect("map pinned authority")
    .spawn_v1()
    .expect("spawn mapped authority reader");
    drop(child_v1.take_stdin_v1());
    let stdout_v1 = read_all_v1(child_v1.take_stdout_v1().expect("child stdout"));
    let stderr_v1 = read_all_v1(child_v1.take_stderr_v1().expect("child stderr"));
    let status_v1 = child_v1.wait_v1().expect("wait child");
    assert_helper_success_v1(status_v1, &stdout_v1, &stderr_v1, "regular file helper");
}
