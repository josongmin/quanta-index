//! Maintenance that a committed Catalog change makes due.
//!
//! Releasing a pin, transferring an abandoned pin, or retiring views does not
//! itself run any maintenance. The runtime pump runs bounded rounds on fixed
//! intervals, and its sweeps otherwise revisit blocked views, abandoned pins
//! and the reclaim queue head only on periodic head probes. This
//! process-local record directs the pump to prioritize those revisits. It is
//! written only after the owning transaction commits and grants no authority.
//! The Catalog remains the durable owner of pins, sessions and the reclaim
//! queue. The periodic sweeps still cover omitted or lost hints.

use std::collections::{BTreeMap, BTreeSet};

use quanta_index_core::index::{IndexError, WorkspaceKey};

use super::{SqliteCatalog, decode, uint};

/// Accumulated, not yet consumed, maintenance demand.
#[derive(Debug, Default)]
pub(crate) struct MaintenanceDemandState {
    released: BTreeMap<(Vec<u8>, i64), BTreeSet<i64>>,
    read_recovery: bool,
    reclaim: bool,
}

// Match the largest read-session recovery pin page. Excess hints fall back to
// the durable ordinary Views sweep; retain older sequences first so a new high
// release cannot displace a lower view that the forward cursor passed.
const MAX_RELEASED_SEQUENCES_PER_SCOPE: usize = 256;

/// Inclusive bounds of consecutively released views in one retention scope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleasedViewRange {
    pub workspace: WorkspaceKey,
    pub epoch: u64,
    pub first_sequence: u64,
    pub last_sequence: u64,
}

/// Maintenance that the pump's next due rounds must perform.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MaintenanceDemand {
    /// Disjoint release ranges to revisit, sorted by scope and sequence.
    pub released_views: Vec<ReleasedViewRange>,
    /// Abandoned pins await settlement: run read-session recovery.
    pub read_recovery: bool,
    /// Retired views queued reclaim work: probe the reclaim queue head.
    pub reclaim: bool,
}

impl MaintenanceDemandState {
    fn try_take(&mut self) -> Result<MaintenanceDemand, IndexError> {
        let mut released_views = Vec::new();
        let capacity = self
            .released
            .values()
            .try_fold(0usize, |total, sequences| {
                total
                    .checked_add(sequences.len())
                    .ok_or(IndexError::ResourceLimited)
            })?;
        released_views
            .try_reserve_exact(capacity)
            .map_err(|_allocation_error| IndexError::ResourceLimited)?;
        for ((workspace, epoch), sequences) in &self.released {
            let workspace = decode::<WorkspaceKey>(workspace)?;
            let epoch = uint(*epoch)?;
            let mut range: Option<(u64, u64)> = None;
            for sequence in sequences {
                let sequence = uint(*sequence)?;
                match range {
                    Some((first, last)) if last.checked_add(1) == Some(sequence) => {
                        range = Some((first, sequence));
                    }
                    Some((first, last)) => {
                        released_views.push(ReleasedViewRange {
                            workspace,
                            epoch,
                            first_sequence: first,
                            last_sequence: last,
                        });
                        range = Some((sequence, sequence));
                    }
                    None => range = Some((sequence, sequence)),
                }
            }
            if let Some((first_sequence, last_sequence)) = range {
                released_views.push(ReleasedViewRange {
                    workspace,
                    epoch,
                    first_sequence,
                    last_sequence,
                });
            }
        }
        let demand = MaintenanceDemand {
            released_views,
            read_recovery: self.read_recovery,
            reclaim: self.reclaim,
        };
        *self = Self::default();
        Ok(demand)
    }
}

impl SqliteCatalog {
    fn demand(&self) -> std::sync::MutexGuard<'_, MaintenanceDemandState> {
        // The state contains ranges and flags. Its entries remain valid
        // when a panic elsewhere poisons the mutex.
        match self.maintenance_demand.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Record the exact view sequence of a committed pin deletion.
    pub(crate) fn record_released_pin(&self, workspace: &[u8], epoch: i64, sequence: i64) {
        let mut demand = self.demand();
        let sequences = demand
            .released
            .entry((workspace.to_vec(), epoch))
            .or_default();
        if sequences.contains(&sequence) {
            return;
        }
        if sequences.len() == MAX_RELEASED_SEQUENCES_PER_SCOPE {
            let Some(&latest) = sequences.last() else {
                return;
            };
            if sequence >= latest {
                return;
            }
            let _removed = sequences.remove(&latest);
        }
        let _inserted = sequences.insert(sequence);
    }

    /// Record that abandoned pins await settlement by read recovery.
    pub(crate) fn record_read_recovery_demand(&self) {
        self.demand().read_recovery = true;
    }

    /// Record that committed view retirement queued reclaim work.
    pub(crate) fn record_reclaim_demand(&self) {
        self.demand().reclaim = true;
    }

    /// Take and clear the maintenance made due since the previous call.
    pub fn take_maintenance_demand(&self) -> Result<MaintenanceDemand, IndexError> {
        self.demand().try_take()
    }
}

#[cfg(test)]
mod tests {
    use quanta_index_core::index::{CollectionId, SlotId, TenantId, WorktreeId};

    use super::{super::workspace_key, *};

    #[test]
    fn failed_take_preserves_released_views_and_flags() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        // The bad row sorts after the valid one, so conversion fails only
        // after the output vector has already received both valid ranges.
        let invalid_scope = (workspace_key(workspace)?, 9);
        let mut state = MaintenanceDemandState {
            released: BTreeMap::from([
                ((workspace_key(workspace)?, 7), BTreeSet::from([3, 4, 9])),
                (invalid_scope.clone(), BTreeSet::from([-1])),
            ]),
            read_recovery: true,
            reclaim: true,
        };

        if !matches!(state.try_take(), Err(IndexError::Storage(_))) {
            return Err("invalid view sequence did not fail the demand drain".into());
        }
        if state.released.remove(&invalid_scope).is_none() {
            return Err("failed drain consumed queued views".into());
        }
        let expected = MaintenanceDemand {
            released_views: vec![
                ReleasedViewRange {
                    workspace,
                    epoch: 7,
                    first_sequence: 3,
                    last_sequence: 4,
                },
                ReleasedViewRange {
                    workspace,
                    epoch: 7,
                    first_sequence: 9,
                    last_sequence: 9,
                },
            ],
            read_recovery: true,
            reclaim: true,
        };
        if state.try_take()? != expected || state.try_take()? != MaintenanceDemand::default() {
            return Err("retry did not deliver the remaining demand exactly once".into());
        }
        Ok(())
    }
}
