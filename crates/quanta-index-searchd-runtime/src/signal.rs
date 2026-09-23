//! The process cancellation root (SEP-21 P08 / S21-09): SIGINT and
//! SIGTERM latched into the supervisor's [`quanta_index_searchd::CancelRoot`].
//!
//! The first delivered signal requests cooperative shutdown (the
//! supervisor drains under its two-deadline policy); a second signal
//! latches an immediate abort, which the daemon answers with
//! `128 + signum` without waiting for any child. The watcher thread owns
//! nothing but the signal iterator and exits once an abort is latched.

use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

use quanta_index_searchd::CancelRoot;

/// Ownership of the installed signal watcher. Dropping it detaches the
/// watcher thread, which exits by itself once an abort is latched or the
/// process does.
pub struct SignalWatch {
    _join: std::thread::JoinHandle<()>,
}

/// Install the SIGINT/SIGTERM watcher for one cancellation root. The
/// watcher runs until an abort is latched or the process exits; there is
/// at most one per process.
pub fn install(root: CancelRoot) -> std::io::Result<SignalWatch> {
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    let spawned = std::thread::Builder::new()
        .name("searchd-signal-root".to_string())
        .spawn(move || {
            let mut first = true;
            for signal in signals.forever() {
                if first {
                    root.request_shutdown();
                    first = false;
                } else {
                    root.request_abort(signal);
                    return;
                }
            }
        });
    match spawned {
        Ok(join) => Ok(SignalWatch { _join: join }),
        Err(error) => Err(error),
    }
}
