//! Shared real-process searchd fixture for integration tests.

#![forbid(unsafe_code)]

use std::error::Error;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_sdk::{ConnectOptions, QuantaIndex};

pub(super) const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);

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
        Self::start_with_command(state_root, searchd_command(state_root, max_generations))
    }

    /// Spawn an already-built daemon command over `state_root` (TOPT-03):
    /// the umask wrapper's launch mechanics with the shared readiness
    /// wait and the shared race-tolerant cleanup. The caller owns argv
    /// (direct binary, `sh -c` wrapper); sockets, env, readiness, and
    /// termination stay in this one owner.
    pub(super) fn start_with_command(
        state_root: &Path,
        mut command: Command,
    ) -> Result<Self, Box<dyn Error>> {
        let mut child = command.spawn()?;
        wait_for_sockets(state_root, &mut child)?;
        Ok(Self {
            state_root: state_root.to_path_buf(),
            child: Some(child),
        })
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
}

impl Drop for SearchdBinaryProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _result = terminate_child(&mut child);
        }
        let _cleanup = remove_socket_files(&self.state_root);
    }
}

/// Wait until `child`, a daemon over `state_root`, accepts on all three
/// sockets; on an exit or a timeout first, clean its sockets up and fail.
pub(super) fn wait_for_sockets(state_root: &Path, child: &mut Child) -> Result<(), Box<dyn Error>> {
    let sockets = daemon_socket_paths(state_root);
    let start = Instant::now();
    while start.elapsed() < SOCKET_TIMEOUT {
        if sockets
            .iter()
            .all(|socket| socket_accepts_connection(socket))
        {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return match remove_socket_files(state_root) {
                Ok(()) => {
                    Err(format!("searchd binary exited before opening sockets: {status}").into())
                }
                Err(cleanup_error) => Err(format!(
                    "searchd binary exited before opening sockets: {status}; \
                     partial socket cleanup failed: {cleanup_error}"
                )
                .into()),
            };
        }
        thread::sleep(Duration::from_millis(10));
    }
    terminate_child(child)?;
    remove_socket_files(state_root)?;
    Err(format!(
        "searchd binary did not open query/control/ingest sockets within {SOCKET_TIMEOUT:?}"
    )
    .into())
}

pub(super) fn terminate_child(child: &mut Child) -> Result<(), Box<dyn Error>> {
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

/// The daemon binary under test. Integration-test only: cargo sets
/// `CARGO_BIN_EXE_*` when it builds this package's test binaries.
pub(super) fn searchd_binary_path() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_quanta-index-searchd"))
}

/// The env a daemon under test boots with: the deterministic embedder
/// and the required history retention. Shared by the direct spawn and
/// the umask wrapper so the two never disagree (TOPT-03).
pub(super) fn apply_searchd_env(command: &mut Command, max_generations: usize) {
    let _configured = command
        .env("QUANTA_INDEX_EMBEDDER", "hash-dev")
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
}

/// The three sockets a daemon over `state_root` binds, in
/// (query, control, ingest) order: the one spelling of the
/// daemon-owned layout (TOPT-03: all three sockets explicit).
pub(super) fn daemon_socket_paths(state_root: &Path) -> [PathBuf; 3] {
    [
        state_root.join("search-plane/query.sock"),
        state_root.join("search-plane/control.sock"),
        state_root.join("search-plane/ingest.sock"),
    ]
}

pub(super) fn searchd_command(state_root: &Path, max_generations: usize) -> Command {
    let mut command = Command::new(searchd_binary_path());
    let _configured = command.arg("serve").arg("--state-root").arg(state_root);
    apply_searchd_env(&mut command, max_generations);
    command
}

pub(super) fn remove_socket_files(state_root: &Path) -> std::io::Result<()> {
    for socket in daemon_socket_paths(state_root) {
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
    use std::process::Command;

    use super::{socket_accepts_connection, terminate_child, wait_for_sockets};

    #[test]
    fn regular_file_does_not_satisfy_searchd_socket_readiness() {
        let root = tempfile::tempdir().expect("socket fixture root");
        let path = root.path().join("query.sock");
        std::fs::write(&path, b"not-a-socket").expect("regular readiness sentinel");

        assert!(!socket_accepts_connection(&path));
    }

    #[test]
    fn terminating_an_already_exited_child_is_not_an_error() {
        // The kill/exit race (TOPT-03/D2 proof): a reaped child must
        // clean up `Ok` so teardown never masks the scenario result.
        let mut child = Command::new("true").spawn().expect("spawn true");
        let status = child.wait().expect("reap true");
        assert!(status.success());
        terminate_child(&mut child).expect("already-exited cleanup is Ok");
    }

    #[test]
    fn terminating_a_child_racing_exit_is_not_an_error() {
        // The same race from the other side: terminate while the child
        // may still be running, a zombie, or just reaped — every
        // interleaving reports `Ok`.
        let mut child = Command::new("true").spawn().expect("spawn true");
        terminate_child(&mut child).expect("racing-exit cleanup is Ok");
    }

    #[test]
    fn readiness_failure_names_the_exited_child_status() {
        // A child that dies before its sockets appear (TOPT-03 proof):
        // the readiness wait fails fast with the original terminal
        // status instead of timing out or masking it.
        let root = tempfile::tempdir().expect("socket fixture root");
        let mut child = Command::new("false").spawn().expect("spawn false");
        let error = wait_for_sockets(root.path(), &mut child).expect_err("no sockets ever appear");
        let text = error.to_string();
        assert!(
            text.contains("exited before opening sockets") && text.contains("exit status: 1"),
            "readiness failure preserves the child status: {text}"
        );
    }
}
