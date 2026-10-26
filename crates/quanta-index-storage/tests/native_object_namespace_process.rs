//! Cooperative cross-process ownership of the immutable object namespace.
//! No physical reclamation is exercised here.

#[path = "common/namespace_authority.rs"]
mod namespace_authority;
use namespace_authority::managed as open_managed;

use quanta_index_core::index::IndexError;
use quanta_index_storage::{FileObjectStore, FileScratchSpace};
use std::{
    error::Error,
    io,
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const CHILD_ROOT: &str = "QI_OBJECT_NAMESPACE_CHILD_ROOT";
const CHILD_MODE: &str = "QI_OBJECT_NAMESPACE_CHILD_MODE";
const CHILD_EXPECT_OPEN: &str = "QI_OBJECT_NAMESPACE_CHILD_EXPECT_OPEN";

fn scratch(parent: &Path) -> TestResult<Arc<FileScratchSpace>> {
    Ok(Arc::new(FileScratchSpace::open_standalone(
        parent.join("scratch"),
        1_000_000,
    )?))
}

fn probe_in_child(root: &Path, mode: &str, expect_open: bool) -> TestResult {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--ignored")
        .arg("--exact")
        .arg("child_namespace_probe")
        .arg("--nocapture")
        .env(CHILD_ROOT, root)
        .env(CHILD_MODE, mode)
        .env(CHILD_EXPECT_OPEN, if expect_open { "yes" } else { "no" })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now()
        .checked_add(Duration::from_secs(10))
        .ok_or_else(|| io::Error::other("namespace child timeout overflow"))?;
    loop {
        if child.try_wait()?.is_some() {
            let output = child.wait_with_output()?;
            if !output.status.success() {
                return Err(io::Error::other(format!(
                    "cross-process {mode} probe (expected open={expect_open}) failed: status={} stdout={} stderr={}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ))
                .into());
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            child.kill()?;
            let _output = child.wait_with_output()?;
            return Err(io::Error::other(format!(
                "cross-process {mode} namespace probe timed out"
            ))
            .into());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "run only as a subprocess of a namespace ownership test"]
fn child_namespace_probe() -> TestResult {
    let root = std::env::var_os(CHILD_ROOT)
        .ok_or_else(|| io::Error::other("namespace child root was not supplied"))?;
    let mode = std::env::var(CHILD_MODE)?;
    let expect_open = std::env::var(CHILD_EXPECT_OPEN)? == "yes";
    let root = Path::new(&root);
    let opened = match mode.as_str() {
        "shared" => FileObjectStore::open(root),
        "managed" => {
            let parent = root
                .parent()
                .ok_or_else(|| io::Error::other("root has no parent"))?;
            let scratch = Arc::new(FileScratchSpace::open_standalone(
                parent.join("child-scratch"),
                1_000_000,
            )?);
            open_managed(root, scratch)
        }
        _ => return Err(io::Error::other("invalid child probe mode").into()),
    };
    let matches_expected = matches!(
        (expect_open, &opened),
        (true, Ok(_)) | (false, Err(IndexError::ResourceLimited))
    );
    if !matches_expected {
        return Err(io::Error::other(format!(
            "child {mode} open result disagreed with expected open={expect_open} (denial must be ResourceLimited): {opened:?}"
        ))
        .into());
    }
    Ok(())
}

#[test]
fn shared_openers_refuse_cross_process_managed_until_last_drop() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("store");
    let scratch = scratch(parent.path())?;
    let first = FileObjectStore::open_with_scratch(&root, scratch.clone())?;
    let second = FileObjectStore::open_with_scratch(&root, scratch)?;

    probe_in_child(&root, "shared", true)?;
    probe_in_child(&root, "managed", false)?;
    drop(first);
    probe_in_child(&root, "managed", false)?;
    drop(second);
    probe_in_child(&root, "managed", true)?;
    Ok(())
}

#[test]
fn managed_owner_refuses_cross_process_openers_until_last_arc_drop() -> TestResult {
    let parent = tempfile::tempdir()?;
    let root = parent.path().join("store");
    let scratch = scratch(parent.path())?;
    let managed = Arc::new(open_managed(&root, scratch)?);
    let clone = managed.clone();

    probe_in_child(&root, "shared", false)?;
    probe_in_child(&root, "managed", false)?;
    drop(managed);
    probe_in_child(&root, "shared", false)?;
    drop(clone);
    probe_in_child(&root, "shared", true)?;
    probe_in_child(&root, "managed", true)?;
    Ok(())
}
