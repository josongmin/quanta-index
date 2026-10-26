//! Shared real-process searchd fixture for integration tests.

#![forbid(unsafe_code)]

use std::error::Error;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_sdk::{ConnectOptions, QuantaIndex};

pub(super) const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct SearchdBinaryProcess {
    state_root: PathBuf,
    child: Child,
}

impl SearchdBinaryProcess {
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
        command: Command,
    ) -> Result<Self, Box<dyn Error>> {
        Self::start_with_command_and_timeout(state_root, command, SOCKET_TIMEOUT)
    }

    pub(super) fn start_with_command_and_timeout(
        state_root: &Path,
        mut command: Command,
        socket_timeout: Duration,
    ) -> Result<Self, Box<dyn Error>> {
        let mut process = Self {
            state_root: state_root.to_path_buf(),
            child: command.spawn()?,
        };
        wait_for_sockets_with_timeout(state_root, &mut process.child, socket_timeout)?;
        Ok(process)
    }

    pub(super) fn connect(&self) -> Result<QuantaIndex, Box<dyn Error>> {
        Ok(QuantaIndex::connect(ConnectOptions::from_state_root(
            &self.state_root,
        ))?)
    }

    pub(super) fn stop(mut self) -> Result<(), Box<dyn Error>> {
        terminate_child(&mut self.child)?;
        let _status = self.wait_for_exit()?;
        Ok(())
    }

    pub(super) fn wait_for_exit(mut self) -> Result<ExitStatus, Box<dyn Error>> {
        let start = Instant::now();
        while start.elapsed() < SOCKET_TIMEOUT {
            if let Some(status) = self.child.try_wait()? {
                remove_socket_files(&self.state_root)?;
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err("searchd binary did not exit within the process deadline".into())
    }
}

impl Drop for SearchdBinaryProcess {
    fn drop(&mut self) {
        let _result = terminate_child(&mut self.child);
        let _cleanup = remove_socket_files(&self.state_root);
    }
}

/// Wait until `child`, a daemon over `state_root`, accepts on all three
/// sockets; on an exit or a timeout first, clean its sockets up and fail.
pub(super) fn wait_for_sockets(state_root: &Path, child: &mut Child) -> Result<(), Box<dyn Error>> {
    wait_for_sockets_with_timeout(state_root, child, SOCKET_TIMEOUT)
}

fn wait_for_sockets_with_timeout(
    state_root: &Path,
    child: &mut Child,
    timeout: Duration,
) -> Result<(), Box<dyn Error>> {
    let sockets = daemon_socket_paths(state_root);
    let start = Instant::now();
    while start.elapsed() < timeout {
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
    Err(
        format!("searchd binary did not open query/control/ingest sockets within {timeout:?}")
            .into(),
    )
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

#[path = "searchd_fixture_env.rs"]
mod searchd_fixture_env;
pub(super) use searchd_fixture_env::apply_searchd_env;

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
    use std::collections::BTreeMap;
    use std::process::Command;

    use super::{
        apply_searchd_env, remove_socket_files, socket_accepts_connection, terminate_child,
        wait_for_sockets,
    };

    #[test]
    fn child_command_carries_all_explicit_physical_fixture_limits()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut command = Command::new("unused-searchd-test-child");
        apply_searchd_env(&mut command, 8);
        let actual = command
            .get_envs()
            .filter_map(|(key, value)| {
                key.to_str()
                    .filter(|key| key.starts_with("QUANTA_INDEX_LEGACY_PHYSICAL_"))
                    .map(|key| (key.to_owned(), value))
            })
            .map(|(key, value)| {
                let value = value.ok_or("physical fixture env was removed")?;
                Ok((
                    key,
                    value
                        .to_str()
                        .ok_or("physical env is not UTF-8")?
                        .to_owned(),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, Box<dyn std::error::Error>>>()?;
        let expected = [
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_BYTES", "17179869184"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_FILES", "200000"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING", "100000"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_ENTRIES", "400000"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_DEPTH", "64"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PENDING_FRAMES", "4096"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_READER_HANDLES", "4096"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_TRACKED_ENTRIES", "100000"),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_MAX_PROCESS_ROOTS", "256"),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_GLOBAL_MAX_SQLITE_HANDLES",
                "8192",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_BYTES",
                "1073741824",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_MAINTENANCE_RESERVED_FILES",
                "1000",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_BYTES",
                "1073741824",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_CONTROL_RESERVED_FILES",
                "10000",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_BYTES",
                "4294967296",
            ),
            ("QUANTA_INDEX_LEGACY_PHYSICAL_WORK_RESERVED_FILES", "30000"),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_BYTES",
                "1073741824",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_HISTORY_RESERVED_FILES",
                "10000",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_DURABLE_CONTROL_FILE_MAX_BYTES",
                "1048576",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CONTROL_FILE_MAX_BYTES",
                "67108864",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_REPOMAP_CANDIDATE_FILE_MAX_BYTES",
                "536870912",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_BYTES",
                "2147483648",
            ),
            (
                "QUANTA_INDEX_LEGACY_PHYSICAL_CATALOG_PARENT_RESERVED_FILES",
                "10000",
            ),
        ];
        if actual.len() != expected.len() {
            return Err(format!(
                "expected 23 physical child env values, got {}",
                actual.len()
            )
            .into());
        }
        for (key, value) in expected {
            if actual.get(key).map(String::as_str) != Some(value) {
                return Err(format!("physical child env differs at {key}").into());
            }
        }
        Ok(())
    }

    #[test]
    fn regular_file_does_not_satisfy_searchd_socket_readiness() {
        let root = tempfile::tempdir().expect("socket fixture root");
        let path = root.path().join("query.sock");
        std::fs::write(&path, b"not-a-socket").expect("regular readiness sentinel");

        assert!(!socket_accepts_connection(&path));
    }

    #[test]
    fn dropping_process_owner_reaps_child_without_explicit_stop()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::net::UnixListener;
        use std::process::Stdio;

        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("search-plane"))?;
        let _listeners = super::daemon_socket_paths(root.path())
            .map(UnixListener::bind)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let mut command = Command::new("sleep");
        let _configured = command
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let process = super::SearchdBinaryProcess::start_with_command(root.path(), command)?;
        let pid = process.child.id();
        drop(process);

        let alive = Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if alive.success() {
            return Err(format!("dropped process owner left child {pid} running").into());
        }
        let remaining = super::daemon_socket_paths(root.path())
            .into_iter()
            .filter(|path| path.exists())
            .collect::<Vec<_>>();
        if !remaining.is_empty() {
            return Err(
                format!("dropped process owner retained socket entries: {remaining:?}").into(),
            );
        }
        Ok(())
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
    fn socket_cleanup_removes_bound_entries_and_tolerates_absence() {
        // TOPT-06/item-5 proof: stop removes the three daemon sockets,
        // and a second cleanup (drop after stop, refused boot) is `Ok`.
        let root = tempfile::tempdir().expect("socket fixture root");
        let plane = root.path().join("search-plane");
        std::fs::create_dir(&plane).expect("socket directory");
        for name in ["query.sock", "control.sock", "ingest.sock"] {
            std::fs::write(plane.join(name), b"stale").expect("stale socket entry");
        }
        remove_socket_files(root.path()).expect("entries are removed");
        for name in ["query.sock", "control.sock", "ingest.sock"] {
            assert!(
                !plane.join(name).exists(),
                "cleanup removes every socket entry"
            );
        }
        remove_socket_files(root.path()).expect("absent sockets are not an error");
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
