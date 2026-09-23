//! TOPT-06/TH-3: the harness owns its tempdir cradle-to-grave.
//!
//! An owned tempdir disappears on explicit `stop` and on plain drop
//! alike; a caller-owned root passed to `boot_in` instead survives
//! `stop`, because the daemon never deletes directories it did not
//! create.

#![forbid(unsafe_code)]

use anyhow::{Result as AnyResult, ensure};
use quanta_index_searchd_harness::E2eRuntime;

#[test]
fn stop_releases_the_daemon_and_removes_the_owned_tempdir() -> AnyResult<()> {
    let mut runtime = E2eRuntime::boot()?;
    runtime.start()?;
    let root = runtime.state_root().to_path_buf();
    ensure!(root.is_dir(), "boot serves its owned tempdir");
    runtime.stop()?;
    ensure!(!root.exists(), "stop removes the owned tempdir");
    Ok(())
}

#[test]
fn drop_without_stop_still_removes_the_owned_tempdir() -> AnyResult<()> {
    let mut runtime = E2eRuntime::boot()?;
    runtime.start()?;
    let root = runtime.state_root().to_path_buf();
    drop(runtime);
    ensure!(!root.exists(), "drop removes the owned tempdir");
    Ok(())
}

#[test]
fn stop_never_removes_a_caller_owned_root() -> AnyResult<()> {
    let dir = quanta_index_searchd_harness::private_tempdir()?;
    let mut runtime = E2eRuntime::boot_in(dir.path())?;
    runtime.start()?;
    runtime.stop()?;
    ensure!(
        dir.path().is_dir(),
        "a caller-owned root survives stop; the daemon deletes only what it created"
    );
    Ok(())
}
