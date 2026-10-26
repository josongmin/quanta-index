//! Durable assignments and process-local grants for catalog-owned enrichment work.
use std::time::Instant;

use super::{IndexError, ProducerId, Refusal, UpdateRequest, ViewRef, WorkIntent, WorkLimits};

/// Process-local discovery result. Only expiry of the fresh discovery budget
/// permits a cadence retry; usage, cancellation and storage errors stay errors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OwnedReadyOutcome {
    Ready(Vec<WorkIntent>),
    RetryDeadline,
}

/// An unchanged assignment that cannot dispatch at this observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnedWorkDefer {
    Active,
    NotDue,
    LeaseExpired,
}

/// A terminal state observed by grant admission, excluding unstarted and live claims.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnedWorkTerminal {
    Completed(ViewRef),
    Superseded,
    Blocked(Refusal),
}

/// The original Catalog decision after resource reservation.
///
/// No variant creates a new work state or converts a benign deferral into a resource refusal.
/// Terminal states exclude pending work and live claims by construction.
/// These process-local decisions are not persisted.
#[expect(
    clippy::large_enum_variant,
    reason = "the bounded grant stays inline instead of adding an unadmitted allocation to owned dispatch"
)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OwnedWorkBegin {
    Granted(OwnedWorkGrant),
    Deferred(OwnedWorkDefer),
    Terminal(OwnedWorkTerminal),
}

/// External ingress must not use this identity. Catalog admits it only when it
/// matches a live owned assignment; Kernel exposes a separate internal path.
pub const OWNED_WORK_PRODUCER: ProducerId = ProducerId(*b"qidx-owned-work!");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnedWorkPolicy {
    pub dispatch_limits: WorkLimits,
    pub max_dispatches: u32,
    pub total_timeout_ms: u64,
    pub lease_ms: u64,
    pub retry_backoff_ms: u64,
}

impl OwnedWorkPolicy {
    pub fn validate(&self) -> Result<(), IndexError> {
        if !(1..=64).contains(&self.max_dispatches)
            || !(1..=3_600_000).contains(&self.total_timeout_ms)
            || !(1..=300_000).contains(&self.lease_ms)
            || !(1..=300_000).contains(&self.retry_backoff_ms)
        {
            return Err(IndexError::InvalidInput("invalid owned work policy".into()));
        }
        for limit in [
            self.dispatch_limits.reads,
            self.dispatch_limits.writes,
            self.dispatch_limits.bytes,
            self.dispatch_limits.mutations,
        ] {
            if limit == 0 || limit.checked_mul(u64::from(self.max_dispatches)).is_none() {
                return Err(IndexError::InvalidInput(
                    "invalid owned work allowance".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Immutable request and frozen allowances survive process restart. Dispatch
/// allocations are charged before execution and never refunded after a crash.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedWorkAssignment {
    pub request: UpdateRequest,
    pub policy: OwnedWorkPolicy,
    pub deadline_ms: u64,
    pub dispatches: u32,
    pub due_ms: u64,
    pub active: bool,
}

/// An ephemeral, process-local execution grant.
///
/// Catalog anchors execution time to its durable-clock sample; commit and
/// delivery consume this same deadline.
/// Only the assignment is persisted. Never serialize a monotonic instant.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedWorkGrant {
    pub assignment: OwnedWorkAssignment,
    pub ordinal: u32,
    pub execution_deadline: Instant,
}

/// Catalog reconciles the original operation receipt before applying a report.
///
/// A terminal `Conflict` or `FenceLost` may replace the old operation with a fresh
/// claim and key under the original assignment limits. Its receipt stays terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnedWorkReport {
    Reconcile,
    Interrupted,
    Failed(Refusal),
}

quanta_index_contract::native_serde_struct!(OwnedWorkPolicy {
    dispatch_limits: WorkLimits,
    max_dispatches: u32,
    total_timeout_ms: u64,
    lease_ms: u64,
    retry_backoff_ms: u64,
});
quanta_index_contract::native_serde_struct!(OwnedWorkAssignment {
    request: UpdateRequest,
    policy: OwnedWorkPolicy,
    deadline_ms: u64,
    dispatches: u32,
    due_ms: u64,
    active: bool,
});

#[cfg(test)]
mod tests {
    macro_rules! check_eq {
        ($left:expr, $right:expr $(,)?) => {{
            match (&$left, &$right) {
                (left, right) if !(*left == *right) => {
                    return Err(format!(
                        "assertion failed: left == right; left: {left:?}, right: {right:?}"
                    )
                    .into());
                }
                _ => {}
            }
        }};
    }

    use super::super::{
        CollectionId, Digest, IndexInstanceId, ObjectRef, OperationKey, OwnerInputBinding,
        OwnerKey, SlotId, StableOwnerId, StreamId, TenantId, UpdateKind, ViewRef, WorkClaim,
        WorkIntent, WorkScope, WorkspaceKey, WorktreeId,
    };
    use super::*;

    fn policy() -> OwnedWorkPolicy {
        OwnedWorkPolicy {
            dispatch_limits: WorkLimits {
                reads: 100,
                writes: 200,
                bytes: 1_024,
                mutations: 50,
            },
            max_dispatches: 4,
            total_timeout_ms: 60_000,
            lease_ms: 10_000,
            retry_backoff_ms: 1_000,
        }
    }

    #[test]
    fn owned_policy_round_trip_preserves_frozen_allowances()
    -> Result<(), Box<dyn std::error::Error>> {
        let expected = policy();
        expected.validate()?;
        let wire = serde_json::to_vec(&expected)?;
        let restored: OwnedWorkPolicy = serde_json::from_slice(&wire)?;
        check_eq!(restored, expected);
        restored.validate()?;
        Ok(())
    }

    #[test]
    fn owned_assignment_round_trip_retains_original_operation_and_policy()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = WorkspaceKey {
            tenant: TenantId([1; 16]),
            collection: CollectionId([2; 16]),
            worktree: WorktreeId([3; 16]),
            slot: SlotId([4; 16]),
        };
        let original = OperationKey {
            workspace,
            epoch: 7,
            producer: OWNED_WORK_PRODUCER,
            stream: StreamId([5; 16]),
            stream_epoch: 2,
            sequence: 9,
        };
        let assignment = OwnedWorkAssignment {
            request: UpdateRequest {
                operation: original,
                expected_view: ViewRef {
                    workspace,
                    epoch: 7,
                    sequence: 6,
                    digest: Digest([6; 32]),
                },
                kind: UpdateKind::Materialize {
                    claim: WorkClaim {
                        intent: WorkIntent {
                            workspace,
                            epoch: 7,
                            instance: IndexInstanceId([8; 16]),
                            scope: WorkScope::Owner {
                                owner: OwnerKey {
                                    producer: ProducerId([9; 16]),
                                    stable_id: StableOwnerId([10; 16]),
                                },
                                expected: OwnerInputBinding {
                                    source: Some(ObjectRef(Digest([7; 32]))),
                                    projection: None,
                                    producer_context: Digest([11; 32]),
                                    derivation_policy: Digest([12; 32]),
                                },
                            },
                        },
                        fence: 3,
                        lease_until_ms: 30_000,
                    },
                },
            },
            policy: policy(),
            deadline_ms: 80_000,
            dispatches: 2,
            due_ms: 20_000,
            active: true,
        };
        let wire = serde_json::to_vec(&assignment)?;
        let restored: OwnedWorkAssignment = serde_json::from_slice(&wire)?;
        check_eq!(restored, assignment);
        check_eq!(restored.request.operation, original);
        restored.policy.validate()?;
        Ok(())
    }

    #[test]
    fn owned_policy_refuses_zero_out_of_range_and_overflowing_allowances() {
        let valid = policy();
        for max_dispatches in [0, 65] {
            assert!(matches!(
                OwnedWorkPolicy {
                    max_dispatches,
                    ..valid
                }
                .validate(),
                Err(IndexError::InvalidInput(_))
            ));
        }
        for total_timeout_ms in [0, 3_600_001] {
            assert!(matches!(
                OwnedWorkPolicy {
                    total_timeout_ms,
                    ..valid
                }
                .validate(),
                Err(IndexError::InvalidInput(_))
            ));
        }
        for lease_ms in [0, 300_001] {
            assert!(matches!(
                OwnedWorkPolicy { lease_ms, ..valid }.validate(),
                Err(IndexError::InvalidInput(_))
            ));
        }
        for retry_backoff_ms in [0, 300_001] {
            assert!(matches!(
                OwnedWorkPolicy {
                    retry_backoff_ms,
                    ..valid
                }
                .validate(),
                Err(IndexError::InvalidInput(_))
            ));
        }
        for dispatch_limits in [
            WorkLimits {
                reads: 0,
                ..valid.dispatch_limits
            },
            WorkLimits {
                writes: 0,
                ..valid.dispatch_limits
            },
            WorkLimits {
                bytes: 0,
                ..valid.dispatch_limits
            },
            WorkLimits {
                mutations: 0,
                ..valid.dispatch_limits
            },
            WorkLimits {
                reads: u64::MAX,
                ..valid.dispatch_limits
            },
        ] {
            assert!(matches!(
                OwnedWorkPolicy {
                    dispatch_limits,
                    ..valid
                }
                .validate(),
                Err(IndexError::InvalidInput(_))
            ));
        }
    }
}
