//! Current-root custody shared by the offline-state oracles.

use std::path::Path;

use quanta_index_searchd::app::runtime::StateRootLease;
use quanta_index_searchd::app::state_migration::OfflineSourceSessionV1;

/// Custody for a current root: acquire the daemon lease, then freeze. The
/// setup lease that pre-seeds the lock file (daemon-ran-here fixture state)
/// must be dropped before this runs.
pub(super) fn current_session(
    root: &Path,
) -> Result<OfflineSourceSessionV1, Box<dyn std::error::Error>> {
    let lease = StateRootLease::acquire(root)?;
    Ok(OfflineSourceSessionV1::open_current(lease)?)
}
