//! QI-BB-014 — the daemon's own directories and sockets are private
//! whatever the umask it inherits.
//!
//! A pinned external Legacy `quanta-index-searchd` binary is started through
//! a shell that first sets `umask 000` (every created file would be world-writable
//! without an explicit mode), with a state root that does not exist yet.
//! The oracle is `stat`, independent of the daemon: the state root it
//! created, the socket directory under it and the three sockets must come
//! back `0700` / `0700` / `0600`, and the state-root lock `0600`.

#![forbid(unsafe_code)]
#![cfg(unix)]

use std::error::Error;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::process::Stdio;

use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, SearchdBinaryProcess, daemon_socket_paths,
};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn mode_of(path: &Path) -> TestResult<u32> {
    Ok(std::fs::symlink_metadata(path)?.mode() & 0o7777)
}

/// Under `umask 000`, everything the daemon creates is still private.
#[test]
fn a_permissive_umask_leaves_every_daemon_owned_path_private() -> TestResult {
    let parent = quanta_index_searchd_harness::private_tempdir()?;
    // A root the daemon has to create itself, two levels down.
    let state_root = parent.path().join("nested").join("state");
    let binary = LegacySearchdBinary::from_environment()?;
    let mut command = binary.command_under_umask(&state_root, "000", 8)?;
    let _configured = command.stdout(Stdio::null()).stderr(Stdio::piped());
    let process = SearchdBinaryProcess::start_with_pinned_command(&state_root, command)?;
    let sockets = daemon_socket_paths(&state_root);
    let outcome = (|| -> TestResult {
        binary.verify()?;
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
    // Result-preserving cleanup (TOPT-03): a scenario failure is the
    // signal and survives a teardown failure; a clean scenario reports
    // the stop result.
    let stop_result = process.stop();
    outcome.and(stop_result)
}
