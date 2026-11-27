//! Stock SQLite scratch COMMIT ordering under an isolated OS write refusal.
//!
//! The file-size limit is process-wide. Run this fixture in one test-harness
//! child and install it only around the COMMIT being tested.

use std::{
    fs,
    path::Path,
    process::Command,
    sync::{Arc, atomic::Ordering},
};

use quanta_index_core::index::{IndexError, ScratchSpacePort};
use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};

use super::*;

const CHILD_ENV: &str = "QUANTA_STORAGE_STOCK_COMMIT_CHILD";
const CHILD_OK: &str = "QUANTA_STORAGE_STOCK_COMMIT_OK";
const TEST_NAME: &str = "map::tests::scratch_transactions::real_commit_io_refusal_precedes_physical_install_and_discards_scratch";

/// Restore the limit before any physical object install or scratch cleanup.
/// The ignored SIGXFSZ disposition is inherited from the child launcher.
fn refuse_regular_file_writes<T>(run: impl FnOnce() -> T) -> Result<T, Box<dyn std::error::Error>> {
    let previous = getrlimit(Resource::Fsize);
    setrlimit(
        Resource::Fsize,
        Rlimit {
            current: Some(0),
            maximum: previous.maximum,
        },
    )?;
    let result = run();
    setrlimit(Resource::Fsize, previous)?;
    Ok(result)
}

fn assert_allocations_retired(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        check!(
            entry.file_name().as_os_str() == std::ffi::OsStr::new(".lease"),
            "scratch allocation survived close and cleanup: {:?}",
            entry.path()
        );
    }
    Ok(())
}

#[test]
fn real_commit_io_refusal_precedes_physical_install_and_discards_scratch()
-> Result<(), Box<dyn std::error::Error>> {
    if std::env::var_os(CHILD_ENV).is_none() {
        // An ignored signal disposition survives exec. Limit changes then
        // affect only this child, never parallel tests in the parent harness.
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg("trap '' XFSZ || exit 99; exec \"$@\"")
            .arg("sh")
            .arg(std::env::current_exe()?)
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_ENV, "1")
            .output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        check!(
            output.status.success() && stdout.contains(CHILD_OK),
            "isolated stock COMMIT fixture failed: status={:?}, stdout={stdout}, stderr={stderr}",
            output.status
        );
        return Ok(());
    }

    let directory = tempfile::tempdir()?;
    let scratch_root = directory.path().join("scratch");
    let space: Arc<dyn ScratchSpacePort> = Arc::new(crate::FileScratchSpace::open_standalone(
        &scratch_root,
        4 * 1024 * 1024,
    )?);
    let store = FileObjectStore::open_with_scratch(directory.path().join("objects"), space)?;
    let sink = RecordingSink::default();
    let mut work = budget()?;

    // Insertion succeeds before the OS refuses a real SQLite COMMIT write.
    let scratch = MapScratch::open(&store, &mut work)?;
    scratch.begin_batch(&work)?;
    scratch.insert_edit(
        &OrdinalMapEdit::unscoped(
            0,
            MapEdit {
                key: b"commit-fault".to_vec(),
                expected: None,
                next: None,
            },
        )?,
        &mut work,
    )?;
    let failed = refuse_regular_file_writes(|| scratch.commit_batch(&work))?;
    check!(matches!(failed, Err(IndexError::Storage(_))));
    check_eq!(sink.1.load(Ordering::SeqCst), 0);
    check_eq!(sink.2.load(Ordering::SeqCst), 0);
    drop(scratch); // journal_mode=OFF forbids rollback-and-reuse.
    assert_allocations_retired(&scratch_root)?;

    // A successful earlier group survives a later group's actual COMMIT failure.
    let first = store.prepare_object(b"first committed group", &[], &mut work)?;
    let first_reference = first.registration.reference;
    let second = store.prepare_object(b"second final group", &[], &mut work)?;
    let second_reference = second.registration.reference;
    let third = store.prepare_object(b"third refused group", &[], &mut work)?;
    let third_reference = third.registration.reference;
    let scratch = MapScratch::open(&store, &mut work)?;
    scratch.begin_batch(&work)?;
    scratch.set_mark(first_reference, 1, &mut work)?;
    // The first committed group also carries the next group's mark.
    scratch.set_mark(second_reference, 2, &mut work)?;
    let mut first_chunk = MapBuffer::new(MAX_INSTALL_BATCH_INPUTS);
    first_chunk.push(first, &work)?;
    flush_reachable_chunk(
        &store,
        &mut first_chunk,
        &sink,
        &mut work,
        |_objects, budget| scratch.commit_batch(budget),
    )?;
    check!(first_chunk.is_empty());
    check_eq!(sink.1.load(Ordering::SeqCst), 1);
    check_eq!(sink.2.load(Ordering::SeqCst), 1);
    check!(store.path_for(first_reference).is_file());

    // COMMIT with no active transaction must not issue a SQLite write.
    let mut second_chunk = MapBuffer::new(MAX_INSTALL_BATCH_INPUTS);
    second_chunk.push(second, &work)?;
    flush_reachable_chunk(
        &store,
        &mut second_chunk,
        &sink,
        &mut work,
        |_objects, budget| {
            refuse_regular_file_writes(|| scratch.commit_batch(budget))
                .map_err(|error| IndexError::Storage(format!("set file-size limit: {error}")))?
        },
    )?;
    check!(second_chunk.is_empty());
    check_eq!(sink.1.load(Ordering::SeqCst), 2);
    check_eq!(sink.2.load(Ordering::SeqCst), 2);
    check!(store.path_for(second_reference).is_file());

    scratch.begin_batch(&work)?;
    scratch.set_mark(third_reference, 1, &mut work)?;
    let mut third_chunk = MapBuffer::new(MAX_INSTALL_BATCH_INPUTS);
    third_chunk.push(third, &work)?;
    let refused = flush_reachable_chunk(
        &store,
        &mut third_chunk,
        &sink,
        &mut work,
        |_objects, budget| {
            refuse_regular_file_writes(|| scratch.commit_batch(budget))
                .map_err(|error| IndexError::Storage(format!("set file-size limit: {error}")))?
        },
    );
    check!(matches!(refused, Err(IndexError::Storage(_))));
    check_eq!(third_chunk.len(), 1);
    check_eq!(sink.1.load(Ordering::SeqCst), 2);
    check_eq!(sink.2.load(Ordering::SeqCst), 2);
    check!(store.path_for(first_reference).is_file());
    check!(store.path_for(second_reference).is_file());
    check!(!store.path_for(third_reference).exists());
    drop(scratch);
    assert_allocations_retired(&scratch_root)?;
    println!("{CHILD_OK}");
    Ok(())
}
