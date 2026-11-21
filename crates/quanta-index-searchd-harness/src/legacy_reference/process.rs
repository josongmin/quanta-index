//! One process lifecycle owner for an already selected Legacy executable.

#![forbid(unsafe_code)]

use std::error::Error;
use std::io::{self, ErrorKind};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat};
use nix::sys::signal::{Signal, kill};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use nix::sys::socket::{getsockopt, sockopt};
use nix::sys::stat::{Mode, SFlag, fstatat};
use nix::unistd::{Pid, UnlinkatFlags, unlinkat};
#[cfg(any(target_os = "macos", test))]
use quanta_index_contract::MetricsSnapshotV1;
use quanta_index_sdk::{ConnectOptions, QuantaIndex};
use quanta_index_searchd::app::runtime::StateRootLease;

use super::binary::LegacySearchdCommand;

pub const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);
const SOCKET_DIRECTORY: &str = "search-plane";
const SOCKET_NAMES: [&str; 3] = ["query.sock", "control.sock", "ingest.sock"];

pub struct SearchdBinaryProcess {
    state_root: PathBuf,
    child: Child,
}

impl SearchdBinaryProcess {
    /// Spawn an already-built daemon command over `state_root` (TOPT-03):
    /// the umask wrapper's launch mechanics with the shared readiness
    /// wait and the shared race-tolerant cleanup. The caller owns argv
    /// (direct binary, `sh -c` wrapper); sockets, env, readiness, and
    /// termination stay in this one owner.
    pub fn start_with_command(state_root: &Path, command: Command) -> Result<Self, Box<dyn Error>> {
        Self::start_with_command_and_timeout(state_root, command, SOCKET_TIMEOUT)
    }

    pub fn start_with_command_and_timeout(
        state_root: &Path,
        mut command: Command,
        socket_timeout: Duration,
    ) -> Result<Self, Box<dyn Error>> {
        reject_active_sockets(state_root)?;
        Self::start_with_child(state_root, command.spawn()?, socket_timeout)
    }

    pub fn start_with_pinned_command(
        state_root: &Path,
        command: LegacySearchdCommand<'_>,
    ) -> Result<Self, Box<dyn Error>> {
        Self::start_with_pinned_command_and_timeout(state_root, command, SOCKET_TIMEOUT)
    }

    pub fn start_with_pinned_command_and_timeout(
        state_root: &Path,
        mut command: LegacySearchdCommand<'_>,
        socket_timeout: Duration,
    ) -> Result<Self, Box<dyn Error>> {
        reject_active_sockets(state_root)?;
        Self::start_with_child(state_root, command.spawn()?, socket_timeout)
    }

    fn start_with_child(
        state_root: &Path,
        child: Child,
        socket_timeout: Duration,
    ) -> Result<Self, Box<dyn Error>> {
        let mut process = Self {
            state_root: state_root.to_path_buf(),
            child,
        };
        wait_for_sockets_with_timeout(state_root, &mut process.child, socket_timeout)?;
        Ok(process)
    }

    pub fn connect(&self) -> Result<QuantaIndex, Box<dyn Error>> {
        Ok(QuantaIndex::connect_legacy(
            ConnectOptions::from_state_root(&self.state_root),
        )?)
    }

    #[cfg(target_os = "linux")]
    pub fn observe_peak_rss(&self, _client: &QuantaIndex) -> anyhow::Result<u64> {
        self.peak_rss_bytes()
    }

    #[cfg(target_os = "macos")]
    pub fn observe_peak_rss(&self, client: &QuantaIndex) -> anyhow::Result<u64> {
        peak_rss_from_darwin_metrics(&client.observability().metrics_snapshot()?)
    }

    #[cfg(target_os = "linux")]
    pub fn peak_rss_bytes(&self) -> anyhow::Result<u64> {
        // We still own the unreaped Child, so its PID cannot refer to a successor.
        let status = std::fs::read_to_string(format!("/proc/{}/status", self.child.id()))?;
        parse_peak_rss_bytes(&status)
    }

    pub fn stop(mut self) -> Result<(), Box<dyn Error>> {
        if let Some(status) = self.child.try_wait()? {
            return Err(format!("searchd binary exited before shutdown: {status}").into());
        }
        let pid = Pid::from_raw(i32::try_from(self.child.id())?);
        match kill(pid, Signal::SIGTERM) {
            Ok(()) | Err(Errno::ESRCH) => {}
            Err(error) => return Err(error.into()),
        }
        let status = self.wait_for_child_exit()?;
        let cleanup = remove_inactive_socket_files(&self.state_root);
        if !status.success() {
            let failure = format!("searchd binary shutdown failed: {status}");
            return Err(match cleanup {
                Ok(()) => failure,
                Err(error) => format!("{failure}; socket cleanup failed: {error}"),
            }
            .into());
        }
        cleanup?;
        Ok(())
    }

    pub fn wait_for_exit(mut self) -> Result<ExitStatus, Box<dyn Error>> {
        let status = self.wait_for_child_exit()?;
        remove_inactive_socket_files(&self.state_root)?;
        Ok(status)
    }

    /// Force a process exit in tests that must reopen without a clean shutdown.
    pub fn kill_and_wait(mut self) -> Result<(), Box<dyn Error>> {
        terminate_child(&mut self.child)?;
        let _status = self.wait_for_exit()?;
        Ok(())
    }

    fn wait_for_child_exit(&mut self) -> Result<ExitStatus, Box<dyn Error>> {
        let start = Instant::now();
        while start.elapsed() < SOCKET_TIMEOUT {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err("searchd binary did not exit within the process deadline".into())
    }
}

/// Extract the Legacy daemon's Darwin `RUSAGE_SELF.ru_maxrss` gauge.
/// The caller must capture the daemon metrics before terminating the process.
#[cfg(any(target_os = "macos", test))]
pub fn peak_rss_from_darwin_metrics(snapshot: &MetricsSnapshotV1) -> anyhow::Result<u64> {
    let mut gauges = snapshot
        .gauges
        .iter()
        .filter(|gauge| gauge.name == "process_resident_bytes");
    let value = gauges
        .next()
        .ok_or_else(|| anyhow::anyhow!("daemon metrics have no resident-bytes gauge"))?
        .value;
    if gauges.next().is_some()
        || !(1.0..=9_007_199_254_740_992.0).contains(&value)
        || value.fract() != 0.0
    {
        anyhow::bail!("daemon resident-bytes gauge is not one exact positive byte count");
    }
    Ok(format!("{value:.0}").parse()?)
}

#[cfg(any(target_os = "linux", test))]
fn parse_peak_rss_bytes(status: &str) -> anyhow::Result<u64> {
    let mut high_water = status
        .lines()
        .filter_map(|line| line.strip_prefix("VmHWM:"));
    let field = high_water
        .next()
        .ok_or_else(|| anyhow::anyhow!("daemon process status has no VmHWM field"))?;
    let mut parts = field.split_whitespace();
    let (Some(value), Some("kB"), None) = (parts.next(), parts.next(), parts.next()) else {
        anyhow::bail!("daemon VmHWM has invalid units or size");
    };
    if high_water.next().is_some() {
        anyhow::bail!("daemon process status has duplicate VmHWM fields");
    }
    value
        .parse::<u64>()?
        .checked_mul(1024)
        .filter(|bytes| *bytes > 0)
        .ok_or_else(|| anyhow::anyhow!("daemon VmHWM is zero or overflows bytes"))
}

impl Drop for SearchdBinaryProcess {
    fn drop(&mut self) {
        let _result = terminate_child(&mut self.child);
        let _cleanup = remove_inactive_socket_files(&self.state_root);
    }
}

/// Wait until `child`, a daemon over `state_root`, accepts on all three
/// sockets; on an exit or a timeout first, clean its sockets up and fail.
pub fn wait_for_sockets(state_root: &Path, child: &mut Child) -> Result<(), Box<dyn Error>> {
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
        if let Some(status) = child.try_wait()? {
            return Err(exited_before_ready(state_root, status));
        }
        let mut all_ready = true;
        for socket in &sockets {
            match socket_accepts_child(socket, child.id()) {
                Ok(true) => {}
                Ok(false) => all_ready = false,
                Err(error) => {
                    return Err(readiness_failure(state_root, child, error.to_string()));
                }
            }
        }
        if all_ready {
            // An exited child can leave accepting sockets owned by another
            // process. Check the child again before granting readiness.
            if let Some(status) = child.try_wait()? {
                return Err(exited_before_ready(state_root, status));
            }
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    let failure =
        format!("searchd binary did not open query/control/ingest sockets within {timeout:?}");
    Err(readiness_failure(state_root, child, failure))
}

fn readiness_failure(state_root: &Path, child: &mut Child, mut failure: String) -> Box<dyn Error> {
    if let Err(error) = terminate_child(child) {
        failure.push_str(&format!("; child cleanup failed: {error}"));
    }
    if let Err(error) = remove_inactive_socket_files(state_root) {
        failure.push_str(&format!("; socket cleanup failed: {error}"));
    }
    failure.into()
}

fn socket_accepts_child(path: &Path, child_pid: u32) -> Result<bool, Box<dyn Error>> {
    let SocketProbe::Connected(stream) = probe_socket(path)? else {
        return Ok(false);
    };
    #[cfg(target_os = "linux")]
    let peer_pid = getsockopt(&stream, sockopt::PeerCredentials)?.pid();
    #[cfg(target_os = "macos")]
    let peer_pid = getsockopt(&stream, sockopt::LocalPeerPid)?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        if u32::try_from(peer_pid)? != child_pid {
            return Err(format!(
                "searchd socket {path:?} belongs to peer PID {peer_pid}, not spawned child {child_pid}"
            )
            .into());
        }
        Ok(true)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (path, child_pid, stream);
        Err("searchd socket peer PID is unsupported on this platform".into())
    }
}

fn exited_before_ready(state_root: &Path, status: ExitStatus) -> Box<dyn Error> {
    match remove_inactive_socket_files(state_root) {
        Ok(()) => format!("searchd binary exited before opening sockets: {status}").into(),
        Err(cleanup_error) => format!(
            "searchd binary exited before opening sockets: {status}; \
             partial socket cleanup failed: {cleanup_error}"
        )
        .into(),
    }
}

fn reject_active_sockets(state_root: &Path) -> Result<(), Box<dyn Error>> {
    for socket in daemon_socket_paths(state_root) {
        if matches!(probe_socket(&socket)?, SocketProbe::Connected(_)) {
            return Err("searchd state root already has an accepting daemon socket".into());
        }
    }
    Ok(())
}

pub fn terminate_child(child: &mut Child) -> Result<(), Box<dyn Error>> {
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

enum SocketProbe {
    Missing,
    Stale { device: u64, inode: u64 },
    Connected(UnixStream),
}

fn probe_socket(path: &Path) -> io::Result<SocketProbe> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => metadata,
        Ok(_) => {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!("searchd socket path {path:?} exists and is not a socket"),
            ));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(SocketProbe::Missing),
        Err(error) => return Err(error),
    };
    match quanta_index_ipc::connect_unix_probe(path) {
        Ok(stream) => Ok(SocketProbe::Connected(stream)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(SocketProbe::Missing),
        Err(error) if error.kind() == ErrorKind::ConnectionRefused => Ok(SocketProbe::Stale {
            device: metadata.dev(),
            inode: metadata.ino(),
        }),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
fn socket_accepts_connection(path: &Path) -> bool {
    matches!(probe_socket(path), Ok(SocketProbe::Connected(_)))
}

/// The three sockets a daemon over `state_root` binds, in
/// (query, control, ingest) order: the one spelling of the
/// daemon-owned layout (TOPT-03: all three sockets explicit).
pub fn daemon_socket_paths(state_root: &Path) -> [PathBuf; 3] {
    let directory = state_root.join(SOCKET_DIRECTORY);
    SOCKET_NAMES.map(|name| directory.join(name))
}

pub fn remove_socket_files(state_root: &Path) -> std::io::Result<()> {
    for socket in daemon_socket_paths(state_root) {
        remove_socket_entry(&socket)?;
    }
    Ok(())
}

fn remove_inactive_socket_files(state_root: &Path) -> std::io::Result<()> {
    let mut stale_sockets = Vec::new();
    for (name, path) in SOCKET_NAMES
        .into_iter()
        .zip(daemon_socket_paths(state_root))
    {
        if matches!(probe_socket(&path)?, SocketProbe::Stale { .. }) {
            stale_sockets.push((name, path));
        }
    }
    if stale_sockets.is_empty() {
        return Ok(());
    }

    // A successor can take the root as soon as our child exits. Use its
    // production lease and pin the socket directory before any unlink.
    let lease = StateRootLease::acquire_existing(state_root).map_err(io::Error::other)?;
    let root = lease
        .borrow_root_directory_read_v1()
        .map_err(io::Error::other)?;
    let directory = match openat(
        &root,
        SOCKET_DIRECTORY,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) {
        Ok(directory) => directory,
        Err(Errno::ENOENT) => return Ok(()),
        Err(error) => {
            return Err(io::Error::new(
                io::Error::from(error).kind(),
                format!("cannot open searchd socket directory under {state_root:?}: {error}"),
            ));
        }
    };
    for (name, path) in stale_sockets {
        let SocketProbe::Stale { device, inode } = probe_socket(&path)? else {
            continue;
        };
        lease
            .require_state_root_v1(state_root)
            .map_err(io::Error::other)?;
        match fstatat(&directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(current)
                if SFlag::from_bits_truncate(current.st_mode) == SFlag::S_IFSOCK
                    && current.st_dev == device as nix::libc::dev_t
                    && current.st_ino == inode as nix::libc::ino_t =>
            {
                match unlinkat(&directory, name, UnlinkatFlags::NoRemoveDir) {
                    Ok(()) | Err(Errno::ENOENT) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(_) => {
                return Err(io::Error::other(format!(
                    "searchd socket {path:?} changed during cleanup"
                )));
            }
            Err(Errno::ENOENT) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn remove_socket_entry(socket: &Path) -> io::Result<()> {
    match std::fs::remove_file(socket) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        outcome => outcome,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::BTreeMap;
    use std::process::Command;

    use super::super::apply_searchd_env;
    use super::{
        parse_peak_rss_bytes, remove_socket_files, socket_accepts_child, socket_accepts_connection,
        terminate_child, wait_for_sockets,
    };

    #[cfg(target_os = "linux")]
    #[test]
    fn readiness_checks_the_peer_at_a_deep_socket_path() -> Result<(), Box<dyn std::error::Error>> {
        let parent = crate::private_tempdir()?;
        let socket = parent
            .path()
            .join("state-owner-".repeat(13))
            .join("query.sock");
        if socket.as_os_str().as_encoded_bytes().len() < 108 {
            return Err("fixture must exceed the Linux Unix socket address width".into());
        }
        let _listener = quanta_index_ipc::UdsServer::bind(&socket)?;
        if !socket_accepts_child(&socket, std::process::id())? {
            return Err("deep endpoint did not identify the listener process".into());
        }
        Ok(())
    }

    #[test]
    fn peak_rss_uses_the_kernel_high_water_mark() {
        assert_eq!(
            parse_peak_rss_bytes("VmHWM:\t 17 kB\nVmRSS:\t 8 kB\n").ok(),
            Some(17_408),
        );
        for status in [
            "VmRSS: 8 kB\n",
            "VmHWM: 0 kB\n",
            "VmHWM: 8 MB\n",
            "VmHWM: 8 kB extra\n",
            "VmHWM: 8 kB\nVmHWM: 9 kB\n",
            "VmHWM: 18446744073709551615 kB\n",
        ] {
            assert!(parse_peak_rss_bytes(status).is_err(), "{status}");
        }
    }

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
    fn foreign_non_socket_entries_are_preserved_without_spawning()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        for redirect in [false, true] {
            let root = tempfile::tempdir()?;
            std::fs::create_dir(root.path().join(super::SOCKET_DIRECTORY))?;
            let [query, ..] = super::daemon_socket_paths(root.path());
            let marker = root.path().join("spawned");
            if redirect {
                let sentinel = root.path().join("sentinel");
                std::fs::write(&sentinel, b"keep")?;
                symlink(sentinel, &query)?;
            } else {
                std::fs::write(&query, b"keep")?;
            }
            let mut command = Command::new("/bin/sh");
            let _configured = command
                .args(["-c", "printf spawned > \"$1\"", "invalid-socket-fixture"])
                .arg(&marker);
            let error = super::SearchdBinaryProcess::start_with_command(root.path(), command)
                .err()
                .ok_or("process started with a non-socket entry")?;
            let error = error.downcast::<std::io::Error>()?;
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert!(!marker.exists());
            assert_eq!(
                super::remove_inactive_socket_files(root.path())
                    .expect_err("automatic cleanup must preserve the invalid entry")
                    .kind(),
                std::io::ErrorKind::InvalidData
            );
            assert_eq!(std::fs::read(&query)?, b"keep");
            assert_eq!(
                std::fs::symlink_metadata(query)?.file_type().is_symlink(),
                redirect
            );
        }
        Ok(())
    }

    #[test]
    fn stale_socket_cleanup_requires_the_state_root_lease() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::net::UnixListener;

        let parent = tempfile::tempdir()?;
        let root = parent.path().join("state");
        let lease = super::StateRootLease::acquire(&root)?;
        std::fs::create_dir(root.join(super::SOCKET_DIRECTORY))?;
        let [query, _, _] = super::daemon_socket_paths(&root);
        drop(UnixListener::bind(&query)?);

        let error = super::remove_inactive_socket_files(&root)
            .expect_err("a successor's root lease must prevent cleanup");
        assert!(matches!(
            error.get_ref().and_then(|source| source.downcast_ref()),
            Some(quanta_index_core::CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootInUse,
                ..
            })
        ));
        assert!(query.exists());
        drop(lease);

        super::remove_inactive_socket_files(&root)?;
        assert!(!query.exists());
        Ok(())
    }

    #[test]
    fn stale_socket_cleanup_refuses_a_redirected_socket_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;
        use std::os::unix::net::UnixListener;

        let parent = tempfile::tempdir()?;
        let root = parent.path().join("state");
        drop(super::StateRootLease::acquire(&root)?);
        let other = parent.path().join("other");
        std::fs::create_dir(&other)?;
        let socket = other.join("query.sock");
        drop(UnixListener::bind(&socket)?);
        symlink(&other, root.join(super::SOCKET_DIRECTORY))?;

        let error = super::remove_inactive_socket_files(&root)
            .expect_err("the root lease cannot authorize unlink through a redirected directory");
        assert!(
            error
                .to_string()
                .contains("cannot open searchd socket directory")
        );
        assert!(socket.exists());
        Ok(())
    }

    #[test]
    fn readiness_timeout_preserves_the_failure_even_if_cleanup_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join(super::SOCKET_DIRECTORY))?;
        let [query, _, _] = super::daemon_socket_paths(root.path());
        std::fs::write(&query, b"keep")?;
        let mut child = Command::new("sleep").arg("60").spawn()?;
        let error = super::wait_for_sockets_with_timeout(
            root.path(),
            &mut child,
            std::time::Duration::ZERO,
        )
        .expect_err("zero deadline must fail readiness");
        let message = error.to_string();
        assert!(message.contains("within 0ns"), "{message}");
        assert!(message.contains("socket cleanup failed"), "{message}");
        assert!(child.try_wait()?.is_some());
        assert_eq!(std::fs::read(query)?, b"keep");
        Ok(())
    }

    #[test]
    fn dropping_process_owner_reaps_child_without_removing_unrelated_sockets()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::net::UnixListener;
        use std::process::Stdio;

        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("search-plane"))?;
        let listeners = super::daemon_socket_paths(root.path())
            .map(UnixListener::bind)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let mut command = Command::new("sleep");
        let _configured = command
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let process = super::SearchdBinaryProcess {
            state_root: root.path().to_path_buf(),
            child: command.spawn()?,
        };
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
        for socket in super::daemon_socket_paths(root.path()) {
            assert!(socket_accepts_connection(&socket), "{socket:?}");
        }
        drop(listeners);
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
    fn explicit_stop_rejects_an_already_failed_daemon() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let mut child = Command::new("sh").args(["-c", "exit 17"]).spawn()?;
        assert_eq!(child.wait()?.code(), Some(17));
        let process = super::SearchdBinaryProcess {
            state_root: root.path().to_path_buf(),
            child,
        };
        let error = process
            .stop()
            .expect_err("failed daemon must fail explicit stop");
        assert!(error.to_string().contains("exit status: 17"), "{error}");
        Ok(())
    }

    #[test]
    fn explicit_stop_uses_the_daemon_shutdown_status() -> Result<(), Box<dyn std::error::Error>> {
        use std::io::{BufRead as _, BufReader};
        use std::process::Stdio;

        for (exit_code, obstruct_cleanup) in [(0, false), (23, false), (23, true)] {
            let root = tempfile::tempdir()?;
            if obstruct_cleanup {
                std::fs::create_dir_all(&super::daemon_socket_paths(root.path())[0])?;
            }
            let child = Command::new("/bin/sh")
                .args([
                    "-c",
                    r#"trap 'exit "$1"' TERM; printf 'ready\n'; while :; do sleep 0.01; done"#,
                    "shutdown-fixture",
                ])
                .arg(exit_code.to_string())
                .stdout(Stdio::piped())
                .spawn()?;
            let mut process = super::SearchdBinaryProcess {
                state_root: root.path().to_path_buf(),
                child,
            };
            let stdout = process
                .child
                .stdout
                .take()
                .ok_or("missing readiness pipe")?;
            let mut readiness = String::new();
            let _read = BufReader::new(stdout).read_line(&mut readiness)?;
            assert_eq!(readiness, "ready\n");
            let stopped = process.stop();
            if exit_code == 0 {
                stopped?;
            } else {
                let error = stopped.expect_err("nonzero shutdown must be reported");
                let diagnostic = error.to_string();
                assert!(diagnostic.contains("exit status: 23"), "{diagnostic}");
                assert_eq!(
                    diagnostic.contains("socket cleanup failed"),
                    obstruct_cleanup
                );
            }
        }
        Ok(())
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

    #[test]
    fn live_sockets_refuse_a_second_process_without_deleting_the_first()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::net::UnixListener;

        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("search-plane"))?;
        let listeners = super::daemon_socket_paths(root.path())
            .map(UnixListener::bind)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let marker = root.path().join("second-spawned");
        let mut command = Command::new("/bin/sh");
        let _configured = command
            .arg("-c")
            .arg("printf spawned > \"$1\"")
            .arg("second-daemon-fixture")
            .arg(&marker);
        let error = super::SearchdBinaryProcess::start_with_command(root.path(), command)
            .err()
            .ok_or("second process unexpectedly started")?;
        assert!(error.to_string().contains("already has an accepting"));
        assert!(!marker.exists());
        for socket in super::daemon_socket_paths(root.path()) {
            assert!(socket_accepts_connection(&socket), "{socket:?}");
        }
        drop(listeners);
        Ok(())
    }

    #[test]
    fn broken_socket_namespace_is_rejected_before_spawning()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::write(root.path().join("search-plane"), b"not a directory")?;
        let marker = root.path().join("spawned");
        let mut command = Command::new("/bin/sh");
        let _configured = command
            .args(["-c", "printf spawned > \"$1\"", "broken-socket-fixture"])
            .arg(&marker);
        let error = super::SearchdBinaryProcess::start_with_command(root.path(), command)
            .err()
            .ok_or("process unexpectedly started with a broken socket namespace")?;
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::NotADirectory)
        );
        assert!(!marker.exists());
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn saturated_socket_is_preserved_when_liveness_cannot_be_confirmed()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::fd::AsRawFd as _;
        use std::os::unix::net::UnixStream;

        use nix::sys::socket::{
            AddressFamily, Backlog, SockFlag, SockType, UnixAddr, bind, listen, socket,
        };

        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("search-plane"))?;
        let path = &super::daemon_socket_paths(root.path())[0];
        let listener = socket(
            AddressFamily::Unix,
            SockType::Stream,
            SockFlag::SOCK_CLOEXEC,
            None,
        )?;
        bind(listener.as_raw_fd(), &UnixAddr::new(path)?)?;
        listen(&listener, Backlog::new(0)?)?;
        let _queued = UnixStream::connect(path)?;

        let error = super::remove_inactive_socket_files(root.path())
            .expect_err("a full queue must not be mistaken for a stale socket");
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(path.exists());
        Ok(())
    }

    #[test]
    fn exited_child_cannot_claim_or_remove_another_listeners_sockets()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::net::UnixListener;

        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("search-plane"))?;
        let listeners = super::daemon_socket_paths(root.path())
            .map(UnixListener::bind)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let mut child = Command::new("/bin/sh").args(["-c", "exit 19"]).spawn()?;
        assert_eq!(child.wait()?.code(), Some(19));
        let error = wait_for_sockets(root.path(), &mut child)
            .expect_err("a different listener cannot make an exited child ready");
        assert!(error.to_string().contains("exit status: 19"), "{error}");
        for socket in super::daemon_socket_paths(root.path()) {
            assert!(socket_accepts_connection(&socket), "{socket:?}");
        }
        drop(listeners);
        Ok(())
    }

    #[test]
    fn living_child_cannot_claim_another_listeners_sockets()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::net::UnixListener;

        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("search-plane"))?;
        let listeners = super::daemon_socket_paths(root.path())
            .map(UnixListener::bind)
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let sockets = super::daemon_socket_paths(root.path());
        assert!(socket_accepts_child(&sockets[0], std::process::id())?);
        let mut child = Command::new("sleep").arg("60").spawn()?;
        let result = wait_for_sockets(root.path(), &mut child);
        assert!(
            child.try_wait()?.is_some(),
            "readiness failure must reap child"
        );
        let error = result.expect_err("foreign listeners cannot make a child ready");
        assert!(error.to_string().contains("not spawned child"), "{error}");
        for socket in super::daemon_socket_paths(root.path()) {
            assert!(socket_accepts_connection(&socket), "{socket:?}");
        }
        drop(listeners);
        Ok(())
    }
}
