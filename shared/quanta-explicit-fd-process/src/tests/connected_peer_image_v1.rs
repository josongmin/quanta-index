//! Image-object verification is separate from authenticating a connected process.
use super::*;
use crate::ObservedExecutableImageV1;
use std::os::unix::fs::MetadataExt;

#[test]
fn loaded_descriptor_accepts_original_and_refuses_an_equal_byte_copy_v1() {
    let directory = tempfile::tempdir().expect("image fixture directory");
    let original = directory.path().join("original");
    let copied = directory.path().join("copied");
    let bytes = b"#!/bin/sh\nexit 0\n";
    std::fs::write(&original, bytes).expect("fixed original bytes");
    std::fs::write(&copied, bytes).expect("same bytes in a different object");
    for path in [&original, &copied] {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .expect("executable fixture mode");
    }
    let source = File::open(&original).expect("open exact original");
    let other = File::open(&copied).expect("open equal-byte foreign image");
    let pinned = PinnedExecutableV1::try_pin_v1(&source, &sha256_path_v1(&original))
        .expect("canonical installed image pin");
    pinned
        .verify_loaded_image_descriptor_v1(&source)
        .expect("same installed object");
    assert_eq!(
        pinned.verify_loaded_image_descriptor_v1(&other),
        Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift),
        "equal bytes do not replace the retained executable object",
    );
}

#[test]
fn loaded_original_mutation_refuses_without_invalidating_the_sealed_snapshot_v1() {
    let directory = tempfile::tempdir().expect("image fixture directory");
    let original = directory.path().join("original");
    std::fs::write(&original, b"#!/bin/sh\nexit 0\n").expect("fixed image bytes");
    std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o700))
        .expect("fixture executable mode");
    let source = File::open(&original).expect("open original");
    let pinned = PinnedExecutableV1::try_pin_v1(&source, &sha256_path_v1(&original))
        .expect("canonical installed image pin");
    OpenOptions::new()
        .append(true)
        .open(&original)
        .expect("mutable original")
        .write_all(b"# changed after pin\n")
        .expect("change the original size");
    assert_eq!(
        pinned.verify_loaded_image_descriptor_v1(&source),
        Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift),
    );
    pinned
        .verify_v1()
        .expect("the existing sealed launch owner remains intact");
}

#[test]
fn actual_sealed_child_image_matches_after_original_source_mutation_v1() {
    let directory = tempfile::tempdir().expect("sealed child fixture directory");
    let original = directory.path().join("cat");
    let copied = std::fs::copy("/bin/cat", &original).expect("actual executable fixture");
    assert!(copied > 0);
    std::fs::set_permissions(&original, std::fs::Permissions::from_mode(0o700))
        .expect("fixture executable mode");
    let source = File::open(&original).expect("actual original image FD");
    let pinned = PinnedExecutableV1::try_pin_v1(&source, &sha256_path_v1(&original))
        .expect("pin actual cat image");
    OpenOptions::new()
        .append(true)
        .open(&original)
        .expect("original fixture remains separate")
        .write_all(b"changed original")
        .expect("mutate only the original source object");
    let mut child =
        spawn_fixed_two_arg_linux_hard_limited_v1(&pinned, [c"cat", c"-"], 1024 * 1024 * 1024)
            .expect("existing canonical sealed spawn");
    let loaded = File::open(format!("/proc/{}/exe", child.pid_v1()))
        .expect("kernel-opened actual child image");
    pinned
        .verify_loaded_image_descriptor_v1(&loaded)
        .expect("same immutable sealed image, despite changed original");
    let mut stdin = child.take_stdin_v1().expect("actual child stdin");
    stdin
        .write_all(b"same installed image\n")
        .expect("bounded child input");
    drop(stdin);
    let stdout = read_all_v1(child.take_stdout_v1().expect("actual stdout"));
    let stderr = read_all_v1(child.take_stderr_v1().expect("actual stderr"));
    assert!(child.wait_v1().expect("reap actual child").success());
    assert_eq!(stdout, b"same installed image\n");
    assert!(stderr.is_empty());
}

#[test]
fn observed_root_identity_matches_only_the_retained_directory_object_v1() {
    let first = tempfile::tempdir().expect("expected startup root");
    let second = tempfile::tempdir().expect("foreign root");
    let expected = File::open(first.path()).expect("expected startup root FD");
    let foreign = File::open(second.path()).expect("foreign root FD");
    let pinned = PinnedWorkingDirectoryV1::try_pin_v1(&expected).expect("canonical root pin");
    let expected = expected.metadata().expect("actual expected root metadata");
    let foreign = foreign.metadata().expect("actual foreign root metadata");
    pinned
        .verify_observed_directory_identity_v1(expected.dev(), expected.ino())
        .expect("exact retained startup root");
    assert_eq!(
        pinned.verify_observed_directory_identity_v1(foreign.dev(), foreign.ino()),
        Err(ExplicitFdProcessErrorV1::WorkingDirectoryIdentityDrift),
    );
}

#[test]
fn observed_image_retains_first_identity_without_issuing_an_installed_pin_v1() {
    use std::os::fd::AsFd;
    let directory = tempfile::tempdir().expect("observation fixture directory");
    let original = directory.path().join("image");
    let copied = directory.path().join("same-byte-image");
    for path in [&original, &copied] {
        std::fs::write(path, b"#!/bin/sh\nexit 0\n").expect("fixed executable bytes");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .expect("executable observation");
    }
    let observation = ObservedExecutableImageV1::try_from_owned_descriptor_v1(
        File::open(&original).expect("owned actual image FD").into(),
    )
    .expect("canonical observation without digest or installed permission");
    let live = File::open(&original).expect("same object");
    observation
        .verify_live_descriptor_v1(live.as_fd())
        .expect("exact first observation");
    let copied = File::open(&copied).expect("equal-byte other object");
    assert_eq!(
        observation.verify_live_descriptor_v1(copied.as_fd()),
        Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift)
    );
    OpenOptions::new()
        .append(true)
        .open(&original)
        .expect("mutable fixture image")
        .write_all(b"changed after observation")
        .expect("change retained object");
    assert_eq!(
        observation.verify_live_descriptor_v1(live.as_fd()),
        Err(ExplicitFdProcessErrorV1::ExecutableIdentityDrift)
    );
}
