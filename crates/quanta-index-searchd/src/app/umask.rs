//! The daemon's file-creation mask (QI-BB-014).
//!
//! Every path the daemon creates under its state root — index and dataset
//! directories, catalogs, sidecars, sockets — is the daemon's alone. The
//! state root and the socket directory are created with explicit modes
//! and re-read to prove them, but the trees beneath them are created by
//! the adapters under the process's inherited umask, so a permissive
//! umask (`000`, say, from a careless unit file) would leave those trees
//! world-writable inside a `0700` root. The daemon entry therefore sets
//! its own mask before anything is created: owner-only, `077`. In-process
//! test harnesses do not call this — a process-wide mask is the daemon
//! entry's decision, not a library's.

use rustix::fs::Mode;

/// The mask the daemon runs under: group and other bits are never set on
/// anything it creates.
pub const DAEMON_UMASK: u32 = 0o077;
/// The mask as the platform's raw mode type.
const DAEMON_UMASK_RAW: rustix::fs::RawMode = 0o077;

/// Set the process umask to [`DAEMON_UMASK`]; returns the mask that was in
/// force, so the boot log can name what the daemon inherited.
#[must_use]
pub fn harden_umask() -> u32 {
    u32::from(rustix::process::umask(Mode::from_raw_mode(DAEMON_UMASK_RAW)).as_raw_mode())
}
