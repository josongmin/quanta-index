//! Real kernel write failures on the offline destination path.
//!
//! The ENOSPC case is opt-in: its caller supplies a disposable, separately
//! mounted, bounded filesystem. Neither case changes the caller's process
//! limits or fills the host temporary filesystem.

use super::*;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::unix::fs::MetadataExt as _;

const FIRST_OBJECT: &str = "activations/000-first.bin";
const SHORT_CHILD: &str = "QUANTA_INDEX_OFFLINE_SHORT_WRITE_CHILD";
const ENOSPC_CHILD: &str = "QUANTA_INDEX_OFFLINE_ENOSPC_CHILD";
const CHILD_SOURCE: &str = "QUANTA_INDEX_OFFLINE_FAULT_SOURCE";
const CHILD_DESTINATION: &str = "QUANTA_INDEX_OFFLINE_FAULT_DESTINATION";
#[path = "../common/physical_enospc.rs"]
mod physical_enospc;
const SHORT_TEST: &str = "physical_os_faults::kernel_file_limit_interrupts_offline_first_copy";
const ENOSPC_TEST: &str =
    "physical_os_faults::actual_bounded_volume_enospc_preserves_source_and_unpublished_target";

fn child_paths() -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let source = std::env::var_os(CHILD_SOURCE).ok_or("offline fault child source absent")?;
    let destination =
        std::env::var_os(CHILD_DESTINATION).ok_or("offline fault child destination absent")?;
    Ok((PathBuf::from(source), PathBuf::from(destination)))
}

fn assert_exact_child(output: &std::process::Output) -> TestResult {
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success()
        || !stdout.contains("running 1 test")
        || !stdout.contains("test result: ok. 1 passed; 0 failed;")
    {
        return Err(format!(
            "offline fault child did not pass its exact test: status={}; stdout={}; stderr={}",
            output.status,
            stdout,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

fn prepared_source(first_bytes: usize) -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let source = private_root()?;
    build_live_root(source.path())?;
    fs::write(source.path().join(FIRST_OBJECT), vec![b'Z'; first_bytes])?;
    // Stabilize the fixture's lease entry before taking its independent
    // source fingerprint in the parent process.
    drop(StateRootLease::acquire(source.path())?);
    Ok(source)
}

fn assert_kernel_write_error(
    error: &anyhow::Error,
    errno: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    match error.downcast_ref::<CoreError>() {
        Some(CoreError::StorageIo {
            raw_os_error: Some(observed),
            message,
            ..
        }) if *observed == errno && message.contains("write staged object") => Ok(()),
        other => Err(format!(
            "offline copy did not observe kernel write errno {errno}: cause={other:?}; error={error:#}"
        )
        .into()),
    }
}

fn assert_unpublished(destination: &Path) -> TestResult {
    let staging = staging_directory_for_v1(destination);
    if destination.try_exists()?
        || !staging.is_dir()
        || staging.join(STATE_ROOT_MANIFEST_FILE_NAME).try_exists()?
        || staging.join("state-backup-manifest-v1.txt").try_exists()?
        || staging.join(FIRST_OBJECT).try_exists()?
    {
        return Err("failed offline copy published a target or left a completed object".into());
    }
    Ok(())
}

#[test]
fn kernel_file_limit_interrupts_offline_first_copy() -> TestResult {
    if std::env::var_os(SHORT_CHILD).as_deref() == Some(std::ffi::OsStr::new("1")) {
        let (source, destination) = child_paths()?;
        let error = run_offline_state_command_with_v1(
            &backup_command(&source, &destination),
            &NoStateMigrationFaultsV1,
        )
        .expect_err("offline first copy completed under a one-block file limit");
        assert_kernel_write_error(&error, rustix::io::Errno::FBIG.raw_os_error())?;
        assert_unpublished(&destination)?;
        return Ok(());
    }

    let source = prepared_source(2 * 1024 * 1024)?;
    let destination_parent = private_root()?;
    let destination = destination_parent.path().join("short-write-backup");
    let source_before = freeze_current(source.path())?;
    let names_before = tree_names_outside_catalog(source.path())?;
    // The filesystem freeze deliberately excludes SQLite's live subtree.
    // Keep its logical contents and the lease bytes as separate oracles.
    let catalog_before = live_catalog_digest(source.path())?;
    let lease_before = fs::read(source.path().join(STATE_ROOT_LEASE_FILE_NAME))?;
    assert_no_source_markers(source.path())?;
    let original = fs::read(source.path().join(FIRST_OBJECT))?;
    let output = Command::new("/bin/sh")
        .args([
            "-c",
            "trap '' XFSZ || exit 96; ulimit -f 1 || exit 97; exec \"$@\"",
            "offline-short-write",
        ])
        .arg(std::env::current_exe()?)
        .args(["--exact", SHORT_TEST, "--test-threads=1"])
        .env(SHORT_CHILD, "1")
        .env(CHILD_SOURCE, source.path())
        .env(CHILD_DESTINATION, &destination)
        .output()?;
    assert_exact_child(&output)?;
    assert_unpublished(&destination)?;
    if freeze_current(source.path())? != source_before
        || tree_names_outside_catalog(source.path())? != names_before
        || live_catalog_digest(source.path())? != catalog_before
        || fs::read(source.path().join(STATE_ROOT_LEASE_FILE_NAME))? != lease_before
        || fs::read(source.path().join(FIRST_OBJECT))? != original
    {
        return Err("kernel short write changed the offline source".into());
    }
    assert_no_source_markers(source.path())?;
    let staging = staging_directory_for_v1(&destination);
    drop(StateRootLease::acquire(&staging)?);
    // An operator must remove the interrupted staging entry before retry.
    fs::remove_dir_all(&staging)?;
    make_backup(source.path(), &destination)?;
    if !destination.join("state-backup-manifest-v1.txt").is_file() {
        return Err("clean retry did not publish a backup manifest".into());
    }
    Ok(())
}

fn fill_private_volume(directory: &Path) -> TestResult {
    const TARGET_FREE: u64 = 8 * 1024 * 1024;
    const MIN_INITIAL_FREE: u64 = 24 * 1024 * 1024;
    let path = directory.join("owned-space-pressure.bin");
    let mut filler = OpenOptions::new().write(true).create_new(true).open(path)?;
    let device = filler.metadata()?.dev();
    let mut random = File::open("/dev/urandom")?;
    let mut chunk = vec![0_u8; 1024 * 1024];
    let mut initial = true;
    for _ in 0..128 {
        let volume = rustix::fs::fstatvfs(&filler)?;
        let total = volume
            .f_blocks
            .checked_mul(volume.f_frsize)
            .ok_or("bounded volume capacity overflow while filling")?;
        let available = volume
            .f_bavail
            .checked_mul(volume.f_frsize)
            .ok_or("bounded volume free-space overflow while filling")?;
        if total > 128 * 1024 * 1024 || filler.metadata()?.dev() != device {
            return Err("ENOSPC filler lost its bounded filesystem identity".into());
        }
        if initial && available < MIN_INITIAL_FREE {
            return Err("bounded filesystem lacked the required clean setup space".into());
        }
        initial = false;
        if available <= TARGET_FREE {
            return Ok(());
        }
        let remaining = available
            .checked_sub(TARGET_FREE)
            .ok_or("bounded volume free-space window underflow")?;
        let chunk_limit = u64::try_from(chunk.len())?;
        let amount = usize::try_from(remaining.min(chunk_limit))?;
        let bytes = chunk
            .get_mut(..amount)
            .ok_or("ENOSPC filler exceeded its supplied buffer")?;
        random.read_exact(bytes)?;
        filler.write_all(bytes)?;
        filler.sync_all()?;
    }
    Err("bounded filesystem did not reach the required free-space window".into())
}

#[test]
#[ignore = "requires QUANTA_INDEX_PHYSICAL_ENOSPC_ROOT on an isolated 32..=128 MiB mount"]
fn actual_bounded_volume_enospc_preserves_source_and_unpublished_target() -> TestResult {
    let root = physical_enospc::bounded_volume_root()?;
    if std::env::var_os(ENOSPC_CHILD).as_deref() == Some(std::ffi::OsStr::new("1")) {
        let (source, destination) = child_paths()?;
        let parent = destination
            .parent()
            .ok_or("offline ENOSPC target parent absent")?;
        let dedicated_name = parent
            .file_name()
            .ok_or("offline ENOSPC dedicated directory name absent")?
            .to_string_lossy();
        if parent.parent() != Some(root.as_path())
            || !dedicated_name.starts_with("qidx-offline-enospc-")
            || fs::canonicalize(parent)? != parent
            || fs::metadata(parent)?.dev() != fs::metadata(&root)?.dev()
            || fs::metadata(&source)?.dev() == fs::metadata(&root)?.dev()
            || destination.try_exists()?
        {
            return Err("offline ENOSPC child destination escaped its bounded mount".into());
        }
        fill_private_volume(parent)?;
        let error = run_offline_state_command_with_v1(
            &backup_command(&source, &destination),
            &NoStateMigrationFaultsV1,
        )
        .expect_err("offline first copy succeeded despite bounded volume exhaustion");
        assert_kernel_write_error(&error, rustix::io::Errno::NOSPC.raw_os_error())?;
        assert_unpublished(&destination)?;
        return Ok(());
    }

    let source = prepared_source(16 * 1024 * 1024)?;
    if fs::metadata(source.path())?.dev() == fs::metadata(&root)?.dev()
        || fs::metadata(std::env::current_exe()?)?.dev() == fs::metadata(&root)?.dev()
    {
        return Err("ENOSPC source or test executable is on the bounded volume".into());
    }
    let destination_parent = tempfile::Builder::new()
        .prefix("qidx-offline-enospc-")
        .tempdir_in(&root)?;
    let destination = destination_parent.path().join("enospc-backup");
    let source_before = freeze_current(source.path())?;
    let names_before = tree_names_outside_catalog(source.path())?;
    // The filesystem freeze deliberately excludes SQLite's live subtree.
    // Keep its logical contents and the lease bytes as separate oracles.
    let catalog_before = live_catalog_digest(source.path())?;
    let lease_before = fs::read(source.path().join(STATE_ROOT_LEASE_FILE_NAME))?;
    assert_no_source_markers(source.path())?;
    let original = fs::read(source.path().join(FIRST_OBJECT))?;
    let output = Command::new(std::env::current_exe()?)
        .args(["--exact", ENOSPC_TEST, "--ignored", "--test-threads=1"])
        .env(ENOSPC_CHILD, "1")
        .env(CHILD_SOURCE, source.path())
        .env(CHILD_DESTINATION, &destination)
        .output()?;
    assert_exact_child(&output)?;

    // Free only this test's filler before checking restart behavior. The
    // failed child has exited, so any process-lifetime uncertain lease has
    // also ended; the original staged inode must be reacquirable.
    fs::remove_file(destination_parent.path().join("owned-space-pressure.bin"))?;
    File::open(destination_parent.path())?.sync_all()?;
    assert_unpublished(&destination)?;
    if freeze_current(source.path())? != source_before
        || tree_names_outside_catalog(source.path())? != names_before
        || live_catalog_digest(source.path())? != catalog_before
        || fs::read(source.path().join(STATE_ROOT_LEASE_FILE_NAME))? != lease_before
        || fs::read(source.path().join(FIRST_OBJECT))? != original
    {
        return Err("kernel ENOSPC changed the offline source".into());
    }
    assert_no_source_markers(source.path())?;
    let staging = staging_directory_for_v1(&destination);
    drop(StateRootLease::acquire(&staging)?);
    fs::remove_dir_all(&staging)?;
    make_backup(source.path(), &destination)?;
    if !destination.join("state-backup-manifest-v1.txt").is_file() {
        return Err("post-ENOSPC clean retry did not publish a backup manifest".into());
    }
    Ok(())
}
