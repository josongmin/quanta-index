//! Release-to-reclaim deadline for the service-owned maintenance pump.
//!
//! [E07](../../../docs/plans/oct-22-finalizing/E07-performance-operations.md#dod)
//! requires reclaimable debt to reach zero within 60 s of the last read pin
//! release. The configured cadence bounds admitted rounds when the released
//! view is reached within one Views page and maintenance rounds can run:
//!
//! - A committed release records Catalog maintenance demand. Retention runs
//!   one configured scope per round. Each release page yields one regular
//!   phase for that scope. Its next release page is reached within
//!   `2 * scopes.len() * retention.interval`. A turn that fails keeps its
//!   range and goes to the back of the rotation, so it never delays another
//!   scope's turn. A range whose epoch is no longer its workspace's current
//!   epoch is cleared.
//! - The retirement records reclaim demand. The next reclaim round, at most
//!   `reclaim.interval` later, restarts its forward sweep at the queue head, so
//!   its `max_steps_per_round` steps examine queued entries in key order from
//!   the lowest. One retired view releases at most [`MAX_VIEW_ROOTS`] roots, so
//!   that many steps reach its roots if no other entries consume the allowance.
//! - An abandoned pin adds one reclaim round before retention: read recovery
//!   settles it if its pin is reached in that round, then retention can retire
//!   the view.
//!
//! The longest configured path is therefore
//! `2 * scopes.len() * retention.interval + 2 * reclaim.interval`,
//! which product configuration must keep within
//! [`NATIVE_RELEASE_RECLAIM_DEADLINE`].
//!
//! # Capacity condition
//!
//! Draining a root queues the children it frees, and that drain records reclaim
//! demand too, so freed children below the forward cursor are reached on the
//! next round instead of at the periodic head probe. The cascade's drain time
//! is then a round count, not a probe period:
//!
//! - Let `S` be the reclaim steps the view's cascade needs: one per freed
//!   object, plus one per further `child_limit` page of an object with more
//!   children than that. A step that hits transient custody contention makes
//!   no progress and is not counted here.
//! - A round whose forward sweep reaches the queue end after it queued freed
//!   children restarts from the head within the same round, so no round ends
//!   with steps left while that work remains. With the round allowance
//!   available for those steps, the drain takes
//!   `ceil(S / max_steps_per_round)` consecutive reclaim rounds.
//!
//! E07's 60 s for a given view therefore holds when
//! `2 * scopes.len() * retention.interval + (1 + ceil(S / max_steps_per_round)) *
//! reclaim.interval` fits within [`NATIVE_RELEASE_RECLAIM_DEADLINE`] (the
//! leading round settles an abandoned pin). `S` depends on the indexed data,
//! so configuration validation checks cadence and the minimum number of
//! steps for the released view's roots. Existing debt, work admission and
//! the physical drain time depend on the actual workload.

use std::time::Duration;

use quanta_index_catalog::MAX_VIEW_ROOTS;

use crate::native_reclaim_driver::NativeReclaimConfig;
use crate::native_retention::NativeRetentionConfig;

/// Product bound from the last read pin release to zero reclaimable debt.
///
/// This is the E07 operations deadline
/// ([E07 definition of done](../../../docs/plans/oct-22-finalizing/E07-performance-operations.md#dod)).
pub const NATIVE_RELEASE_RECLAIM_DEADLINE: Duration = Duration::from_secs(60);

/// A maintenance configuration that cannot prove the release deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeReleaseDeadlineError {
    /// `2 * scopes.len() * retention.interval + 2 * reclaim.interval` exceeds the deadline.
    CadenceExceedsDeadline {
        retention_interval: Duration,
        reclaim_interval: Duration,
        deadline: Duration,
    },
    /// One reclaim round cannot examine every root one retired view queues.
    ReclaimRoundBelowViewRoots {
        max_steps_per_round: u32,
        view_roots: usize,
    },
    /// A retention page cannot hold the released view.
    RetentionPageEmpty,
}

impl std::fmt::Display for NativeReleaseDeadlineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CadenceExceedsDeadline {
                retention_interval,
                reclaim_interval,
                deadline,
            } => write!(
                formatter,
                "native release reclaim cadence exceeds the deadline: twice the configured retention scope rounds at {retention_interval:?} each plus twice reclaim interval {reclaim_interval:?} exceeds {deadline:?}"
            ),
            Self::ReclaimRoundBelowViewRoots {
                max_steps_per_round,
                view_roots,
            } => write!(
                formatter,
                "native reclaim round cannot reach a retired view's roots: {max_steps_per_round} steps per round is below {view_roots} view roots"
            ),
            Self::RetentionPageEmpty => {
                formatter.write_str("native retention page cannot hold a released view")
            }
        }
    }
}

impl std::error::Error for NativeReleaseDeadlineError {}

/// Refuse a retention and reclaim pairing whose configured admitted rounds
/// cannot meet [`NATIVE_RELEASE_RECLAIM_DEADLINE`], assuming the released view
/// is reached within its selected Views page.
pub fn validate_release_reclaim_deadline(
    retention: &NativeRetentionConfig,
    reclaim: &NativeReclaimConfig,
) -> Result<(), NativeReleaseDeadlineError> {
    // A nonempty page can hold the released view under the stated condition.
    if retention.page_limit == 0 {
        return Err(NativeReleaseDeadlineError::RetentionPageEmpty);
    }
    if !usize::try_from(reclaim.max_steps_per_round).is_ok_and(|steps| steps >= MAX_VIEW_ROOTS) {
        return Err(NativeReleaseDeadlineError::ReclaimRoundBelowViewRoots {
            max_steps_per_round: reclaim.max_steps_per_round,
            view_roots: MAX_VIEW_ROOTS,
        });
    }
    // An abandoned pin first needs reclaim settlement. The target scope may
    // need a regular phase before its release page, then reclaim drains it.
    let longest_path = u32::try_from(retention.scopes.len())
        .ok()
        .and_then(|scopes| scopes.checked_mul(2))
        .and_then(|turns| retention.interval.checked_mul(turns))
        .and_then(|retention_rounds| {
            reclaim
                .interval
                .checked_mul(2)
                .and_then(|reclaim_rounds| retention_rounds.checked_add(reclaim_rounds))
        });
    if longest_path.is_none_or(|path| path > NATIVE_RELEASE_RECLAIM_DEADLINE) {
        return Err(NativeReleaseDeadlineError::CadenceExceedsDeadline {
            retention_interval: retention.interval,
            reclaim_interval: reclaim.interval,
            deadline: NATIVE_RELEASE_RECLAIM_DEADLINE,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_retention::NativeRetentionScope;
    use crate::native_wal_reclaim::NativeWalReclaimPolicy;
    use quanta_index_core::index::{
        CollectionId, SlotId, TenantId, WorkLimits, WorkspaceKey, WorktreeId,
    };

    fn scope(epoch: u64) -> NativeRetentionScope {
        NativeRetentionScope {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch,
        }
    }

    fn retention(interval: Duration, page_limit: u32) -> NativeRetentionConfig {
        NativeRetentionConfig {
            scopes: vec![scope(1)],
            sequence_window: 1,
            page_limit,
            custody_page_limit: 1,
            interval,
            round_timeout: Duration::from_secs(2),
            work_limits: WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 536_870_912,
                mutations: 1_000_000,
            },
            head_probe_every: 2,
        }
    }

    fn reclaim(interval: Duration, max_steps_per_round: u32) -> NativeReclaimConfig {
        NativeReclaimConfig {
            interval,
            wal_reclaim: NativeWalReclaimPolicy {
                trigger_bytes: 8_388_608,
                min_interval: Duration::from_secs(1),
            },
            max_steps_per_round,
            child_limit: 32,
            work_limits: WorkLimits {
                reads: 1_000_000,
                writes: 1_000_000,
                bytes: 1_073_741_824,
                mutations: 1_000_000,
            },
            round_timeout: Duration::from_secs(2),
            head_probe_every: 2,
        }
    }

    #[test]
    fn compliant_cadence_and_round_sizes_are_accepted() -> Result<(), NativeReleaseDeadlineError> {
        validate_release_reclaim_deadline(
            &retention(Duration::from_millis(20), 1),
            &reclaim(Duration::from_millis(20), 7),
        )?;
        // Exactly at the deadline: 2 * 10 s + 2 * 20 s = 60 s.
        validate_release_reclaim_deadline(
            &retention(Duration::from_secs(10), 32),
            &reclaim(Duration::from_secs(20), 8),
        )
    }

    #[test]
    fn cadence_beyond_the_deadline_is_refused() {
        // 2 * 10 s + 2 * 20.001 s = 60.002 s.
        let refused = validate_release_reclaim_deadline(
            &retention(Duration::from_secs(10), 1),
            &reclaim(Duration::from_millis(20_001), 7),
        );
        assert_eq!(
            refused,
            Err(NativeReleaseDeadlineError::CadenceExceedsDeadline {
                retention_interval: Duration::from_secs(10),
                reclaim_interval: Duration::from_millis(20_001),
                deadline: Duration::from_secs(60),
            })
        );
    }

    #[test]
    fn two_scope_cadence_charges_both_retention_rounds() {
        let mut boundary = retention(Duration::from_secs(5), 1);
        boundary.scopes.push(scope(2));
        // 2 * 2 * 5 s + 2 * 20 s = 60 s.
        assert_eq!(
            validate_release_reclaim_deadline(&boundary, &reclaim(Duration::from_secs(20), 7)),
            Ok(())
        );

        let mut over = retention(Duration::from_millis(5_001), 1);
        over.scopes.push(scope(2));
        // 2 * 2 * 5.001 s + 2 * 20 s = 60.004 s.
        assert_eq!(
            validate_release_reclaim_deadline(&over, &reclaim(Duration::from_secs(20), 7)),
            Err(NativeReleaseDeadlineError::CadenceExceedsDeadline {
                retention_interval: Duration::from_millis(5_001),
                reclaim_interval: Duration::from_secs(20),
                deadline: NATIVE_RELEASE_RECLAIM_DEADLINE,
            })
        );
    }

    #[test]
    fn reclaim_round_below_view_roots_is_refused() {
        let refused = validate_release_reclaim_deadline(
            &retention(Duration::from_millis(20), 1),
            &reclaim(Duration::from_millis(20), 6),
        );
        assert_eq!(
            refused,
            Err(NativeReleaseDeadlineError::ReclaimRoundBelowViewRoots {
                max_steps_per_round: 6,
                view_roots: 7,
            })
        );
    }

    #[test]
    fn empty_retention_page_is_refused() {
        let refused = validate_release_reclaim_deadline(
            &retention(Duration::from_millis(20), 0),
            &reclaim(Duration::from_millis(20), 7),
        );
        assert_eq!(refused, Err(NativeReleaseDeadlineError::RetentionPageEmpty));
    }
}
