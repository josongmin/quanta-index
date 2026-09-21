//! QI-BB-014 — the daemon's own directories and sockets are private
//! whatever the umask it inherits.
//!
//! The real `quanta-index-searchd` binary is started through a shell that
//! first sets `umask 000` (every created file would be world-writable
//! without an explicit mode), with a state root that does not exist yet.
//! The oracle is `stat`, independent of the daemon: the state root it
//! created, the socket directory under it and the three sockets must come
//! back `0700` / `0700` / `0600`, and the state-root lock `0600`.
//!
//! The second proof is the refusal: a pre-created state root the umask
//! left at `0777` is refused typed (`STATE_ROOT_INSECURE`) before any
//! socket exists, and the daemon exits non-zero naming the mode.

#![forbid(unsafe_code)]
#![cfg(unix)]

use std::error::Error;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

const SOCKET_TIMEOUT: Duration = Duration::from_secs(30);
const EXIT_TIMEOUT: Duration = Duration::from_secs(30);

/// The daemon binary, run under `umask 000` by a shell, with the env the
/// binary needs to boot (retention is required; the embedder is named).
fn searchd_under_umask(state_root: &Path, umask: &str) -> Command {
    let mut command = Command::new("sh");
    let _configured = command
        .arg("-c")
        .arg(r#"umask "$1" && exec "$2" serve --state-root "$3""#)
        .arg("umask-hardening")
        .arg(umask)
        .arg(env!("CARGO_BIN_EXE_quanta-index-searchd"))
        .arg(state_root)
        .env("QUANTA_INDEX_EMBEDDER", "hash-dev")
        .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_GENERATIONS", "8")
        .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_BYTES", (16 * 1024 * 1024).to_string())
        .env("QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_REVISION_PAIRS", "128")
        .env(
            "QUANTA_INDEX_SEARCH_CORPUS_HISTORY_MAX_TOTAL_BYTES",
            (256 * 1024 * 1024).to_string(),
        )
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    command
}

fn mode_of(path: &Path) -> TestResult<u32> {
    Ok(std::fs::symlink_metadata(path)?.mode() & 0o7777)
}

fn socket_accepts_connection(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt as _;
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_socket())
        && UnixStream::connect(path).is_ok()
}

fn terminate(child: &mut Child) -> TestResult {
    if child.try_wait()?.is_none() {
        child.kill()?;
    }
    let _status = child.wait()?;
    Ok(())
}

/// Wait until every socket answers a connect, or the child exits.
fn wait_for_sockets(child: &mut Child, sockets: &[std::path::PathBuf]) -> TestResult {
    let started = Instant::now();
    while started.elapsed() < SOCKET_TIMEOUT {
        if sockets
            .iter()
            .all(|socket| socket_accepts_connection(socket))
        {
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!("the daemon exited before its sockets opened: {status}").into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    terminate(child)?;
    Err(format!("the daemon did not open its sockets within {SOCKET_TIMEOUT:?}").into())
}

/// Under `umask 000`, everything the daemon creates is still private.
#[test]
fn a_permissive_umask_leaves_every_daemon_owned_path_private() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    // A root the daemon has to create itself, two levels down.
    let state_root = parent.path().join("nested").join("state");
    let mut child = searchd_under_umask(&state_root, "000").spawn()?;
    let sockets = [
        state_root.join("search-plane/query.sock"),
        state_root.join("search-plane/control.sock"),
        state_root.join("search-plane/ingest.sock"),
    ];
    let outcome = (|| -> TestResult {
        wait_for_sockets(&mut child, &sockets)?;
        for directory in [
            parent.path().join("nested"),
            state_root.clone(),
            state_root.join("search-plane"),
        ] {
            let mode = mode_of(&directory)?;
            if mode != 0o700 {
                return Err(format!(
                    "{} is {mode:04o} under umask 000; the daemon must create it 0700",
                    directory.display()
                )
                .into());
            }
        }
        for socket in &sockets {
            let mode = mode_of(socket)?;
            if mode != 0o600 {
                return Err(format!(
                    "{} is {mode:04o} under umask 000; a private socket is 0600",
                    socket.display()
                )
                .into());
            }
        }
        let lock = state_root.join(".searchd-state-root.lock");
        let lock_mode = mode_of(&lock)?;
        if lock_mode != 0o600 {
            return Err(format!("the state-root lock is {lock_mode:04o}, not 0600").into());
        }
        // Everything the adapters created beneath the root — catalogs,
        // authority stores, the semantic migration root — carries no
        // group/other bit either: the daemon's own mask, not the inherited
        // one, governs them.
        let mut pending = vec![state_root.clone()];
        let mut inspected = 0_usize;
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory)? {
                let entry = entry?;
                let path = entry.path();
                let metadata = std::fs::symlink_metadata(&path)?;
                let mode = metadata.mode() & 0o7777;
                if mode & 0o077 != 0 {
                    return Err(format!(
                        "{} is {mode:04o} under umask 000; nothing the daemon creates carries a group/other bit",
                        path.display()
                    )
                    .into());
                }
                inspected += 1;
                if metadata.is_dir() {
                    pending.push(path);
                }
            }
        }
        if inspected < 5 {
            return Err(format!("the daemon created its trees ({inspected} entries seen)").into());
        }
        Ok(())
    })();
    terminate(&mut child)?;
    outcome
}

/// A pre-created root the umask left world-writable is refused typed
/// before any socket exists, and nothing is created under it.
#[test]
fn a_world_writable_pre_created_state_root_is_refused_typed_before_any_socket() -> TestResult {
    let parent = tempfile::tempdir()?;
    let state_root = parent.path().join("state");
    std::fs::create_dir(&state_root)?;
    std::fs::set_permissions(&state_root, std::fs::Permissions::from_mode(0o777))?;
    let mut child = searchd_under_umask(&state_root, "000").spawn()?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() > EXIT_TIMEOUT {
            terminate(&mut child)?;
            return Err("the daemon kept running on a world-writable state root".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _read = std::io::Read::read_to_string(&mut pipe, &mut stderr)?;
    }
    if status.success() {
        return Err("a world-writable state root must refuse boot".into());
    }
    if !stderr.contains("STATE_ROOT_INSECURE") || !stderr.contains("mode 0777") {
        return Err(format!("the refusal names the code and the mode: {stderr}").into());
    }
    if state_root.join("search-plane").exists() {
        return Err("a refused boot creates no socket directory".into());
    }
    let entries: Vec<_> = std::fs::read_dir(&state_root)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<_, _>>()?;
    if !entries.is_empty() {
        return Err(format!("a refused boot leaves the root untouched, found {entries:?}").into());
    }
    Ok(())
}
