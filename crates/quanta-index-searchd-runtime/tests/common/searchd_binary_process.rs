//! Shared real-process searchd fixture for integration tests.

#![forbid(unsafe_code)]

use std::error::Error;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_sdk::{ConnectOptions, QuantaIndex};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct SearchdBinaryProcess {
    state_root: PathBuf,
    child: Option<Child>,
}

impl SearchdBinaryProcess {
    pub(super) fn start(state_root: &Path) -> Result<Self, Box<dyn Error>> {
        Self::start_with_history_max_generations(state_root, 8)
    }

    pub(super) fn start_with_history_max_generations(
        state_root: &Path,
        max_generations: usize,
    ) -> Result<Self, Box<dyn Error>> {
        let mut child = searchd_command(state_root, max_generations).spawn()?;
        let sockets = [
            state_root.join("search-plane/query.sock"),
            state_root.join("search-plane/control.sock"),
            state_root.join("search-plane/ingest.sock"),
        ];
        let start = Instant::now();
        while start.elapsed() < SOCKET_TIMEOUT {
            if sockets
                .iter()
                .all(|socket| socket_accepts_connection(socket))
            {
                return Ok(Self {
                    state_root: state_root.to_path_buf(),
                    child: Some(child),
                });
            }
            if let Some(status) = child.try_wait()? {
                return match remove_socket_files(state_root) {
                    Ok(()) => Err(format!(
                        "searchd binary exited before opening sockets: {status}"
                    )
                    .into()),
                    Err(cleanup_error) => Err(format!(
                        "searchd binary exited before opening sockets: {status}; \
                         partial socket cleanup failed: {cleanup_error}"
                    )
                    .into()),
                };
            }
            thread::sleep(Duration::from_millis(10));
        }
        terminate_child(&mut child)?;
        remove_socket_files(state_root)?;
        Err(format!(
            "searchd binary did not open query/control/ingest sockets within {SOCKET_TIMEOUT:?}"
        )
        .into())
    }

    pub(super) fn connect(&self) -> Result<QuantaIndex, Box<dyn Error>> {
        Ok(QuantaIndex::connect(ConnectOptions::from_state_root(
            &self.state_root,
        ))?)
    }

    pub(super) fn stop(mut self) -> Result<(), Box<dyn Error>> {
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut child)?;
        }
        remove_socket_files(&self.state_root)?;
        Ok(())
    }

    #[allow(
        dead_code,
        reason = "the shared fixture's lease probe is used only by the composite lifecycle target"
    )]
    pub(super) fn require_start_failure(state_root: &Path) -> Result<Output, Box<dyn Error>> {
        let mut child = searchd_command(state_root, 8)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let start = Instant::now();
        while start.elapsed() < SOCKET_TIMEOUT {
            if child.try_wait()?.is_some() {
                return Ok(child.wait_with_output()?);
            }
            thread::sleep(Duration::from_millis(10));
        }
        terminate_child(&mut child)?;
        Err("second searchd process did not reject the occupied state root before timeout".into())
    }
}

impl Drop for SearchdBinaryProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _result = terminate_child(&mut child);
        }
        let _cleanup = remove_socket_files(&self.state_root);
    }
}

fn terminate_child(child: &mut Child) -> Result<(), Box<dyn Error>> {
    if child.try_wait()?.is_none() {
        match child.kill() {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::InvalidInput | std::io::ErrorKind::NotFound
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    let _status = child.wait()?;
    Ok(())
}

#[cfg(unix)]
fn socket_accepts_connection(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;

    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket())
        && UnixStream::connect(path).is_ok()
}

#[cfg(not(unix))]
fn socket_accepts_connection(_path: &Path) -> bool {
    false
}

fn searchd_command(state_root: &Path, max_generations: usize) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"));
    let _configured = command
        .arg("serve")
        .arg("--state-root")
        .arg(state_root)
        .env("QUANTA_INDEX_EMBEDDER", "hash")
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS",
            max_generations.to_string(),
        )
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES",
            (16 * 1024 * 1024).to_string(),
        )
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS",
            "128",
        )
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
            (256 * 1024 * 1024).to_string(),
        );
    command
}

fn remove_socket_files(state_root: &Path) -> std::io::Result<()> {
    for socket in [
        state_root.join("search-plane/query.sock"),
        state_root.join("search-plane/control.sock"),
        state_root.join("search-plane/ingest.sock"),
    ] {
        match std::fs::remove_file(socket) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::socket_accepts_connection;

    #[test]
    fn regular_file_does_not_satisfy_searchd_socket_readiness() {
        let root = tempfile::tempdir().expect("socket fixture root");
        let path = root.path().join("query.sock");
        std::fs::write(&path, b"not-a-socket").expect("regular readiness sentinel");

        assert!(!socket_accepts_connection(&path));
    }
}
