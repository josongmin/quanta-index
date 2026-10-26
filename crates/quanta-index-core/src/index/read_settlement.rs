//! A Catalog snapshot of retained read custody and reclaimable logical debt.
//! Physical file and writer settlement remains the storage owner's authority.

use super::{IndexError, ReadSessionLease, ReadTicketLease, ViewRef, WorkBudget, WorkspaceKey};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReadSettlementScope {
    pub workspace: WorkspaceKey,
    pub epoch: u64,
    pub target: ViewRef,
}

impl NativeReadSettlementScope {
    pub fn validate(self) -> Result<(), IndexError> {
        if self.target.workspace != self.workspace || self.target.epoch != self.epoch {
            return Err(IndexError::InvalidInput(
                "read settlement target is outside the requested scope".into(),
            ));
        }
        Ok(())
    }
}

/// Produced only after a known committed deletion of the exact held ticket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReadReleaseObservation {
    pub scope: NativeReadSettlementScope,
    pub has_remaining_target_pins: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeReadSettlementStatus {
    Pending,
    Incomplete,
    SettledAtSnapshot,
}

/// A point-in-time logical observation. It does not certify a physical peak or
/// subsequent publications, and it is not a substitute for release custody.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeReadSettlement {
    pub scope: NativeReadSettlementScope,
    pub head: ViewRef,
    pub exact_floor: u64,
    /// Current target pin presence in this Catalog snapshot. A release ACK's
    /// earlier observation remains separate and immutable.
    pub has_remaining_target_pins: bool,
    pub status: NativeReadSettlementStatus,
}

pub trait CatalogReadSettlementPort: Send + Sync {
    fn release_held_ticket(
        &self,
        session: &dyn ReadSessionLease,
        ticket: ReadTicketLease,
        scope: NativeReadSettlementScope,
        budget: &mut WorkBudget,
    ) -> Result<NativeReadReleaseObservation, IndexError>;

    fn observe_read_settlement(
        &self,
        session: &dyn ReadSessionLease,
        scope: NativeReadSettlementScope,
        budget: &mut WorkBudget,
    ) -> Result<NativeReadSettlement, IndexError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{CollectionId, Digest, SlotId, TenantId, WorktreeId};

    #[test]
    fn target_must_belong_to_exact_requested_scope() {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let target = ViewRef {
            workspace,
            epoch: 7,
            sequence: 11,
            digest: Digest([5; 32]),
        };
        let scope = NativeReadSettlementScope {
            workspace,
            epoch: 7,
            target,
        };
        assert_eq!(scope.validate(), Ok(()));
        assert!(matches!(
            NativeReadSettlementScope { epoch: 8, ..scope }.validate(),
            Err(IndexError::InvalidInput(_))
        ));
        assert!(matches!(
            NativeReadSettlementScope {
                workspace: WorkspaceKey {
                    slot: SlotId([9; 16]),
                    ..workspace
                },
                ..scope
            }
            .validate(),
            Err(IndexError::InvalidInput(_))
        ));
    }
}
