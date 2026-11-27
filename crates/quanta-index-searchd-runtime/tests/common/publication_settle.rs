//! Shared test-only settle of one caller-retained publish.
//!
//! A publish whose client step ends without an answer has an unknown outcome,
//! not a failed one. It is reconciled through the operation protocol under the
//! same key, exactly like an ACK-loss recovery: `status` decides, a pending
//! attempt is driven with `resume`, and a request the journal never admitted is
//! resubmitted unchanged, so no replacement operation is ever created.
//!
//! A step's outcome is unknown when the SDK reports `Unknown` for this key, or
//! when it reports a `Transport` error after that step's own client deadline
//! has passed: the request may have been admitted, so only `status` can tell.
//! A transport error inside a live deadline is a real failure.

use std::{error::Error, path::Path};

use quanta_index_contract::index::{
    OperationKey, OperationStatus, SubmitOutcome, UpdateRequest, ViewRef,
};
use quanta_index_core::{BudgetInterruptionV1, RequestBudgetV1};
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};

type SettleResult<T> = Result<T, Box<dyn Error>>;

/// Upper bound on protocol reconciliation steps for one publish.
///
/// Each step is a full request with its own deadline; there is no sleep or
/// clock between steps, so the bound only turns a non-converging operation
/// into evidence.
pub(super) const MAX_PUBLISH_RECONCILIATIONS: usize = 32;

/// Whether one step's error leaves this key's outcome unknown.
fn uncertain_error(error: &NativeUdsError, key: OperationKey, step: &RequestBudgetV1) -> bool {
    match error {
        NativeUdsError::Unknown(uncertain) => *uncertain == key,
        NativeUdsError::Transport(_) => matches!(
            step.interruption(),
            Some(BudgetInterruptionV1::DeadlineExceeded { .. })
        ),
        NativeUdsError::Protocol(_)
        | NativeUdsError::QueryRejected { .. }
        | NativeUdsError::Refused(_)
        | NativeUdsError::PeerBindingRefused(_) => false,
    }
}

fn uncertain<T>(
    result: &Result<T, NativeUdsError>,
    key: OperationKey,
    step: &RequestBudgetV1,
) -> bool {
    result
        .as_ref()
        .is_err_and(|error| uncertain_error(error, key, step))
}

/// Submit one caller-retained publish and settle it through the operation
/// protocol. `step` makes the client budget of each protocol step.
///
/// Once terminal, the receipt must be stable: `status` and a replayed `resume`
/// both return the same Published view, proving one publication for this key.
pub(super) fn submit_and_settle<P: AsRef<Path>>(
    client: &NativeUdsClient<P>,
    request: &UpdateRequest,
    step: impl Fn() -> RequestBudgetV1,
) -> SettleResult<ViewRef> {
    let key = request.operation;
    let mut budget = step();
    let mut outcome = client.submit_borrowed(request, &budget);
    for _ in 0..MAX_PUBLISH_RECONCILIATIONS {
        let status = match outcome {
            Ok(SubmitOutcome::Terminal(status)) => status,
            Ok(SubmitOutcome::Accepted(attempt)) if attempt.operation == key => {
                OperationStatus::Pending(attempt)
            }
            Ok(SubmitOutcome::Unknown(uncertain)) if uncertain == key => {
                budget = step();
                match client.status(key, &budget) {
                    Ok(status) => status,
                    Err(error) if uncertain_error(&error, key, &budget) => {
                        outcome = Ok(SubmitOutcome::Unknown(key));
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error) if uncertain_error(&error, key, &budget) => {
                outcome = Ok(SubmitOutcome::Unknown(key));
                continue;
            }
            other => return Err(format!("publish {key:?} failed: {other:?}").into()),
        };
        budget = step();
        outcome = match status {
            OperationStatus::Published(view) => {
                return confirm_single_publication(client, key, view, &step);
            }
            OperationStatus::Pending(attempt) if attempt.operation == key => {
                client.resume(key, &budget)
            }
            OperationStatus::Unknown => client.submit_borrowed(request, &budget),
            other @ (OperationStatus::Pending(_) | OperationStatus::Refused(_)) => {
                return Err(format!("publish {key:?} settled as {other:?}").into());
            }
        };
    }
    Err(format!(
        "publish {key:?} was not terminal after {MAX_PUBLISH_RECONCILIATIONS} reconciliations"
    )
    .into())
}

fn confirm_single_publication<P: AsRef<Path>>(
    client: &NativeUdsClient<P>,
    key: OperationKey,
    view: ViewRef,
    step: &impl Fn() -> RequestBudgetV1,
) -> SettleResult<ViewRef> {
    let terminal = OperationStatus::Published(view);
    let mut budget = step();
    let mut status = client.status(key, &budget);
    for _ in 0..MAX_PUBLISH_RECONCILIATIONS {
        if !uncertain(&status, key, &budget) {
            break;
        }
        budget = step();
        status = client.status(key, &budget);
    }
    budget = step();
    let mut replay = client.resume(key, &budget);
    for _ in 0..MAX_PUBLISH_RECONCILIATIONS {
        if !uncertain(&replay, key, &budget)
            && !matches!(replay, Ok(SubmitOutcome::Unknown(uncertain)) if uncertain == key)
        {
            break;
        }
        budget = step();
        replay = client.resume(key, &budget);
    }
    if status? != terminal {
        return Err(format!("{key:?} durable status differs from its Published receipt").into());
    }
    if replay? != SubmitOutcome::Terminal(terminal) {
        return Err(format!("{key:?} replay differs from its Published receipt").into());
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_contract::index::{
        CollectionId, ProducerId, SlotId, StreamId, TenantId, WorkspaceKey, WorktreeId,
    };
    use std::time::{Duration, Instant};

    fn key(sequence: u64) -> OperationKey {
        OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 1,
            producer: ProducerId([5; 16]),
            stream: StreamId([6; 16]),
            stream_epoch: 1,
            sequence,
        }
    }

    #[test]
    fn transport_error_is_unknown_only_after_the_step_deadline() {
        let expired = RequestBudgetV1::until(
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("clock has a past instant"),
        );
        let live = RequestBudgetV1::for_duration(Duration::from_secs(3_600));
        let deadline = NativeUdsError::Transport("IPC request deadline elapsed".into());
        assert!(uncertain_error(&deadline, key(2), &expired));
        assert!(!uncertain_error(&deadline, key(2), &live));
        // The SDK's own unknown outcome is uncertain only for its own key.
        assert!(uncertain_error(
            &NativeUdsError::Unknown(key(2)),
            key(2),
            &live
        ));
        assert!(!uncertain_error(
            &NativeUdsError::Unknown(key(3)),
            key(2),
            &live
        ));
        assert!(!uncertain_error(
            &NativeUdsError::Protocol("malformed".into()),
            key(2),
            &expired
        ));
    }
}
