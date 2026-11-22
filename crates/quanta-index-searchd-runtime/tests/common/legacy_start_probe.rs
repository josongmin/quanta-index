//! Bounded command probes for the pinned Legacy daemon.

#![forbid(unsafe_code)]

use std::error::Error;
use std::io::{Read as _, Seek as _};
use std::path::Path;
use std::process::Output;
use std::thread;
use std::time::{Duration, Instant};

use quanta_index_searchd_harness::legacy_reference::{
    LegacySearchdBinary, LegacySearchdCommand, SOCKET_TIMEOUT, terminate_child,
};

/// Capture a nonzero exit before the daemon reaches readiness.
pub(super) fn require_start_failure(
    binary: &LegacySearchdBinary,
    state_root: &Path,
    max_generations: usize,
) -> Result<Output, Box<dyn Error>> {
    let mut command = binary.command()?;
    let _configured = command.arg("serve").arg("--state-root").arg(state_root);
    let _configured = command.apply_searchd_env(max_generations);
    wait_for_refusal(binary, command)
}

/// Reap a pinned daemon command and preserve its refusal output.
pub(super) fn wait_for_refusal(
    binary: &LegacySearchdBinary,
    command: LegacySearchdCommand<'_>,
) -> Result<Output, Box<dyn Error>> {
    let output = wait_for_command_exit(binary, command)?;
    if output.status.success() {
        return Err("old-Q command unexpectedly succeeded".into());
    }
    Ok(output)
}

/// Wait for a command receipt without leaving a timed-out child running.
pub(super) fn wait_for_command_exit(
    binary: &LegacySearchdBinary,
    mut command: LegacySearchdCommand<'_>,
) -> Result<Output, Box<dyn Error>> {
    // A child must be able to finish writing while the parent polls for exit.
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = command
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .spawn()?;
    let start = Instant::now();
    while start.elapsed() < SOCKET_TIMEOUT {
        if let Some(status) = child.try_wait()? {
            stdout.rewind()?;
            stderr.rewind()?;
            let mut output = Output {
                status,
                stdout: Vec::new(),
                stderr: Vec::new(),
            };
            let _read_stdout = stdout.read_to_end(&mut output.stdout)?;
            let _read_stderr = stderr.read_to_end(&mut output.stderr)?;
            binary.verify()?;
            return Ok(output);
        }
        thread::sleep(Duration::from_millis(10));
    }
    terminate_child(&mut child)?;
    Err("old-Q command did not exit before the process timeout".into())
}

#[test]
fn captures_both_streams_beyond_pipe_capacity() -> Result<(), Box<dyn Error>> {
    use sha2::{Digest as _, Sha256};

    let shell = Path::new("/bin/sh").canonicalize()?;
    let digest = format!("{:x}", Sha256::digest(std::fs::read(&shell)?));
    let binary = LegacySearchdBinary::from_path_and_sha(&shell, &digest)?;
    let mut command = binary.command()?;
    let _configured = command.args([
        "-c",
        "printf '%0131072d' 0; printf '%0131072d' 0 >&2; exit 7",
    ]);

    let output = wait_for_refusal(&binary, command)?;
    assert_eq!(output.status.code(), Some(7));
    for stream in [&output.stdout, &output.stderr] {
        assert_eq!(stream.len(), 131_072);
        assert!(stream.iter().all(|byte| *byte == b'0'));
    }
    Ok(())
}
