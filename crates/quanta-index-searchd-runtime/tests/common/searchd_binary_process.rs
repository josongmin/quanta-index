//! Shared real-process searchd fixture for integration tests.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_sdk::{ConnectOptions, QuantaIndex};

const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct SearchdBinaryProcess {
    state_root: PathBuf,
    child: Option<Child>,
}

impl SearchdBinaryProcess {
    pub(super) fn start(state_root: &Path) -> Result<Self, Box<dyn Error>> {
        let mut child = searchd_command(state_root).spawn()?;
        let sockets = [
            state_root.join("search-plane/query.sock"),
            state_root.join("search-plane/control.sock"),
            state_root.join("search-plane/ingest.sock"),
        ];
        let start = Instant::now();
        while start.elapsed() < SOCKET_TIMEOUT {
            if sockets.iter().all(|socket| socket.exists()) {
                return Ok(Self {
                    state_root: state_root.to_path_buf(),
                    child: Some(child),
                });
            }
            if let Some(status) = child.try_wait()? {
                return Err(
                    format!("searchd binary exited before opening sockets: {status}").into(),
                );
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
        let mut child = searchd_command(state_root)
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
        child.kill()?;
    }
    let _status = child.wait()?;
    Ok(())
}

fn searchd_command(state_root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_quanta-index-searchd"));
    let _configured = command
        .arg("serve")
        .arg("--state-root")
        .arg(state_root)
        .env("QUANTA_INDEX_EMBEDDER", "hash")
        .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS", "8")
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
