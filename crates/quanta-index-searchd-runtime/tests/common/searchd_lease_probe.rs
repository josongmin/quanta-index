//! Second-owner lease probe for the real-process searchd fixture.
//!
//! Split from `searchd_binary_process.rs` because only the composite lifecycle
//! target exercises it; keeping it in the shared fixture left a dead function
//! in every other includer.

#![forbid(unsafe_code)]

use std::error::Error;
use std::path::Path;
use std::process::{Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::searchd_binary_process::{SOCKET_TIMEOUT, searchd_command, terminate_child};

/// Start a second searchd against an occupied state root and require it to
/// exit on its own before the socket timeout.
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
