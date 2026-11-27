//! Native lifecycle custody against the Cargo-registered product daemon.

use std::fs;
use std::path::Path;

use quanta_index_contract::index::{ViewRef, ViewSelection};
use quanta_index_contract::native_protocol::{NativeIndexRequest, NativeIndexResponse};
use quanta_index_sdk::native_uds::NativeUdsClient;
use quanta_index_searchd_harness::native_index::NativeSession;

use super::{TestResult, budget, config, require, workspace};

fn product_binary() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_quanta-index-searchd-fixture"))
}

fn start_owned() -> TestResult<NativeSession> {
    Ok(NativeSession::start_owned(
        product_binary(),
        |root, socket| Ok(config(root, socket)),
    )?)
}

fn assert_serving(session: &NativeSession) -> TestResult<ViewRef> {
    let client = NativeUdsClient::new(session.socket_path().to_path_buf());
    let NativeIndexResponse::Created(view) = client.request(
        NativeIndexRequest::CreateWorkspace {
            workspace: workspace(),
            epoch: 1,
        },
        &budget(),
    )?
    else {
        return Err("Native workspace creation returned an unexpected reply".into());
    };
    require(session.state_root().is_dir(), "Native state root is absent")?;
    require(
        client
            .read_view(ViewSelection::Exact(view), &budget())?
            .identity
            == view,
        "Native daemon did not serve its created view",
    )?;
    Ok(view)
}

#[test]
fn stop_releases_the_daemon_and_removes_the_owned_tempdir() -> TestResult {
    let session = start_owned()?;
    let _served_view = assert_serving(&session)?;
    let root = session
        .state_root()
        .parent()
        .ok_or("state root has no parent")?
        .to_path_buf();
    session.stop()?;
    require(!root.exists(), "stop did not remove the owned Native root")
}

#[test]
fn drop_without_stop_still_removes_the_owned_tempdir() -> TestResult {
    let session = start_owned()?;
    let _served_view = assert_serving(&session)?;
    let root = session
        .state_root()
        .parent()
        .ok_or("state root has no parent")?
        .to_path_buf();
    drop(session);
    require(!root.exists(), "drop did not remove the owned Native root")
}

#[test]
fn stop_never_removes_a_caller_owned_root() -> TestResult {
    let caller = tempfile::tempdir()?;
    let marker = caller.path().join("caller.marker");
    fs::write(&marker, b"caller-owned")?;
    let session = NativeSession::start_in(product_binary(), caller.path(), |root, socket| {
        Ok(config(root, socket))
    })?;
    let _served_view = assert_serving(&session)?;
    let state_root = session.state_root().to_path_buf();
    session.stop()?;
    require(
        caller.path().is_dir() && state_root.is_dir() && fs::read(&marker)? == b"caller-owned",
        "Native stop removed or changed a caller-owned root",
    )
}

#[test]
fn borrowed_state_root_reopens_after_control_files_are_removed() -> TestResult {
    let caller = tempfile::tempdir()?;
    let first = NativeSession::start_in(product_binary(), caller.path(), |root, socket| {
        Ok(config(root, socket))
    })?;
    let first_view = assert_serving(&first)?;
    let state_root = first.state_root().to_path_buf();
    let first_control_dir = first
        .socket_path()
        .parent()
        .ok_or("first socket has no parent")?
        .to_path_buf();
    require(
        !first_control_dir.starts_with(caller.path()),
        "borrowed state path lengthened the control socket path",
    )?;
    first.stop()?;
    require(
        !first_control_dir.exists(),
        "first session control files remain",
    )?;

    let second = NativeSession::start_in(product_binary(), caller.path(), |root, socket| {
        Ok(config(root, socket))
    })?;
    require(
        second.state_root() == state_root,
        "borrowed state root changed",
    )?;
    let second_control_dir = second
        .socket_path()
        .parent()
        .ok_or("second socket has no parent")?
        .to_path_buf();
    require(
        second_control_dir != first_control_dir,
        "reopened session reused prior control directory",
    )?;
    let client = NativeUdsClient::new(second.socket_path().to_path_buf());
    require(
        client
            .read_view(ViewSelection::Exact(first_view), &budget())?
            .identity
            == first_view,
        "reopened Native daemon did not serve the first view",
    )?;
    second.stop()?;
    require(
        !second_control_dir.exists(),
        "second session control files remain",
    )?;
    require(state_root.is_dir(), "borrowed state root was removed")
}
