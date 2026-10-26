//! Same-key observation of a Native publication whose submit ACK is uncertain.

use std::{thread, time::Duration};

use anyhow::{Context as _, Result};
use quanta_index_contract::index::{
    OperationKey, OperationStatus, SubmitOutcome, UpdateRequest, ViewRef,
};
use quanta_index_core::RequestBudgetV1;
use quanta_index_sdk::native_uds::{NativeUdsClient, NativeUdsError};
use serde_json::{Value, json};

const STATUS_POLL_INTERVAL: Duration = Duration::from_millis(100);
// Profile, full, delta, frontier-only and delete, then at most 256 churn writes.
const MAX_PUBLICATION_RECORDS: usize = 261;

#[derive(Clone, Copy)]
pub(super) struct PublicationLabel {
    pub(super) phase: &'static str,
    pub(super) cycle: Option<u32>,
}

#[derive(Default)]
pub(super) struct PublicationAudit {
    records: Vec<Value>,
}

impl PublicationAudit {
    pub(super) fn detail(&self) -> Value {
        Value::Array(self.records.clone())
    }

    fn begin(
        &mut self,
        phase: &'static str,
        cycle: Option<u32>,
        key: OperationKey,
    ) -> Result<usize> {
        anyhow::ensure!(
            self.records.len() < MAX_PUBLICATION_RECORDS,
            "native publication observation bound exceeded"
        );
        let index = self.records.len();
        self.records.push(json!({
            "phase":phase,
            "cycle":cycle,
            "operation_key":key,
            "submit_count":0,
            "submit_outcome":null,
            "status_checks":0,
            "last_status":null,
            "completion":null,
        }));
        Ok(index)
    }

    fn field(&mut self, index: usize, name: &'static str, value: Value) -> Result<()> {
        let record = self
            .records
            .get_mut(index)
            .context("publication record missing")?;
        let fields = record
            .as_object_mut()
            .context("publication record malformed")?;
        let _previous = fields.insert(name.into(), value);
        Ok(())
    }
}

pub(super) fn publish(
    client: &NativeUdsClient,
    request: UpdateRequest,
    label: PublicationLabel,
    audit: &mut PublicationAudit,
) -> Result<ViewRef> {
    let budget = RequestBudgetV1::for_duration(super::REQUEST_TIMEOUT);
    let key = request.operation;
    reconcile_submit(
        key,
        label,
        audit,
        &budget,
        |budget| client.submit(request, budget),
        |key, budget| client.status(key, budget),
    )
}

fn reconcile_submit(
    key: OperationKey,
    label: PublicationLabel,
    audit: &mut PublicationAudit,
    budget: &RequestBudgetV1,
    submit: impl FnOnce(&RequestBudgetV1) -> std::result::Result<SubmitOutcome, NativeUdsError>,
    mut status: impl FnMut(
        OperationKey,
        &RequestBudgetV1,
    ) -> std::result::Result<OperationStatus, NativeUdsError>,
) -> Result<ViewRef> {
    let index = audit.begin(label.phase, label.cycle, key)?;
    budget.checkpoint("native_publication_submit")?;
    audit.field(index, "submit_count", json!(1))?;
    let submitted = submit(budget);
    match submitted {
        Ok(SubmitOutcome::Terminal(OperationStatus::Published(view))) => {
            audit.field(index, "submit_outcome", json!("terminal_published"))?;
            budget.checkpoint("native_publication_submit_reply")?;
            audit.field(index, "completion", json!("immediate_ack"))?;
            Ok(view)
        }
        Ok(SubmitOutcome::Terminal(OperationStatus::Refused(reason))) => {
            audit.field(index, "submit_outcome", json!("terminal_refused"))?;
            anyhow::bail!("native submit refused for {key:?}: {reason:?}")
        }
        Ok(SubmitOutcome::Accepted(attempt)) => {
            audit.field(index, "submit_outcome", json!("accepted"))?;
            anyhow::ensure!(
                attempt.operation == key,
                "native accepted a different operation key"
            );
            observe_terminal(key, index, audit, budget, &mut status)
        }
        Ok(SubmitOutcome::Unknown(observed)) => {
            audit.field(index, "submit_outcome", json!("unknown_response"))?;
            anyhow::ensure!(
                observed == key,
                "native submit returned a different unknown key"
            );
            observe_terminal(key, index, audit, budget, &mut status)
        }
        Err(NativeUdsError::Unknown(observed)) => {
            audit.field(index, "submit_outcome", json!("unknown_error"))?;
            anyhow::ensure!(
                observed == key,
                "native SDK returned a different unknown key"
            );
            observe_terminal(key, index, audit, budget, &mut status)
        }
        Ok(SubmitOutcome::Terminal(other)) => {
            audit.field(
                index,
                "submit_outcome",
                json!(format!("invalid_terminal:{other:?}")),
            )?;
            anyhow::bail!("native submit returned invalid terminal for {key:?}: {other:?}")
        }
        Err(error) => {
            audit.field(index, "submit_outcome", json!(format!("error:{error:?}")))?;
            Err(error.into())
        }
    }
}

fn observe_terminal(
    key: OperationKey,
    index: usize,
    audit: &mut PublicationAudit,
    budget: &RequestBudgetV1,
    status: &mut impl FnMut(
        OperationKey,
        &RequestBudgetV1,
    ) -> std::result::Result<OperationStatus, NativeUdsError>,
) -> Result<ViewRef> {
    loop {
        budget.checkpoint("native_publication_status")?;
        let checks = audit
            .records
            .get(index)
            .context("publication record missing")?["status_checks"]
            .as_u64()
            .context("publication status count malformed")?
            .checked_add(1)
            .context("publication status count overflow")?;
        audit.field(index, "status_checks", json!(checks))?;
        let observed = match status(key, budget) {
            Ok(observed) => observed,
            Err(error) => {
                audit.field(index, "last_status", json!(format!("error:{error:?}")))?;
                return Err(error)
                    .with_context(|| format!("native status failed for original key {key:?}"));
            }
        };
        match observed {
            OperationStatus::Published(view) => {
                audit.field(index, "last_status", json!("published"))?;
                budget.checkpoint("native_publication_status_reply")?;
                anyhow::ensure!(
                    view.workspace == key.workspace && view.epoch == key.epoch,
                    "native status published a different workspace or epoch"
                );
                audit.field(index, "completion", json!("reconciled_status"))?;
                return Ok(view);
            }
            OperationStatus::Refused(reason) => {
                audit.field(index, "last_status", json!("refused"))?;
                anyhow::bail!("native publication refused for original key {key:?}: {reason:?}")
            }
            OperationStatus::Unknown => {
                audit.field(index, "last_status", json!("unknown"))?;
                anyhow::bail!("native publication original key remains unknown: {key:?}")
            }
            OperationStatus::Pending(attempt) => {
                anyhow::ensure!(
                    attempt.operation == key,
                    "native status changed operation key"
                );
                audit.field(index, "last_status", json!("pending"))?;
                let remaining = budget
                    .deadline()
                    .saturating_duration_since(std::time::Instant::now());
                budget.checkpoint("native_publication_pending")?;
                thread::sleep(remaining.min(STATUS_POLL_INTERVAL));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::super::product_tests::required_field;
    use super::*;
    use quanta_index_contract::index::{AttemptId, AttemptRef, Digest, Refusal};

    fn published_view(key: OperationKey) -> ViewRef {
        ViewRef {
            workspace: key.workspace,
            epoch: key.epoch,
            sequence: key.sequence,
            digest: Digest([9; 32]),
        }
    }

    #[test]
    fn unknown_ack_observes_only_original_key_and_budget_until_published() -> Result<()> {
        let key = super::super::operation(2);
        let original = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let submit_count = Cell::new(0_u32);
        let status_count = Cell::new(0_u32);
        let same_budget = Cell::new(true);
        let same_key = Cell::new(true);
        let mut audit = PublicationAudit::default();
        let view = reconcile_submit(
            key,
            PublicationLabel {
                phase: "full_event_ack",
                cycle: None,
            },
            &mut audit,
            &original,
            |observed| {
                submit_count.set(submit_count.get() + 1);
                same_budget.set(same_budget.get() && observed.same_request_as(&original));
                Err(NativeUdsError::Unknown(key))
            },
            |observed_key, observed| {
                same_key.set(same_key.get() && observed_key == key);
                same_budget.set(same_budget.get() && observed.same_request_as(&original));
                status_count.set(status_count.get() + 1);
                if status_count.get() == 1 {
                    Ok(OperationStatus::Pending(AttemptRef {
                        operation: key,
                        attempt: AttemptId([8; 16]),
                        fence: 1,
                    }))
                } else {
                    Ok(OperationStatus::Published(published_view(key)))
                }
            },
        )?;
        anyhow::ensure!(view == published_view(key), "wrong reconciled view");
        anyhow::ensure!(submit_count.get() == 1, "submit was repeated");
        anyhow::ensure!(status_count.get() == 2, "pending was not polled once");
        anyhow::ensure!(same_key.get(), "status used another key");
        anyhow::ensure!(same_budget.get(), "request budget changed");
        anyhow::ensure!(required_field(&audit.detail(), "/0/submit_outcome")? == "unknown_error");
        anyhow::ensure!(required_field(&audit.detail(), "/0/completion")? == "reconciled_status");
        anyhow::ensure!(required_field(&audit.detail(), "/0/status_checks")? == 2);
        Ok(())
    }

    #[test]
    fn immediate_ack_never_looks_up_status() -> Result<()> {
        let key = super::super::operation(1);
        let mut audit = PublicationAudit::default();
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let calls = Cell::new(0_u32);
        let view = reconcile_submit(
            key,
            PublicationLabel {
                phase: "profile_configure",
                cycle: None,
            },
            &mut audit,
            &budget,
            |_budget| {
                Ok(SubmitOutcome::Terminal(OperationStatus::Published(
                    published_view(key),
                )))
            },
            |_key, _budget| {
                calls.set(calls.get() + 1);
                Ok(OperationStatus::Unknown)
            },
        )?;
        anyhow::ensure!(view == published_view(key), "wrong immediate view");
        anyhow::ensure!(calls.get() == 0, "immediate ACK was queried");
        anyhow::ensure!(required_field(&audit.detail(), "/0/completion")? == "immediate_ack");
        anyhow::ensure!(required_field(&audit.detail(), "/0/status_checks")? == 0);
        Ok(())
    }

    #[test]
    fn accepted_and_refused_status_do_not_publish() -> Result<()> {
        let key = super::super::operation(3);
        let mut audit = PublicationAudit::default();
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let status_count = Cell::new(0_u32);
        let same_key = Cell::new(true);
        let result = reconcile_submit(
            key,
            PublicationLabel {
                phase: "delta_event_ack",
                cycle: None,
            },
            &mut audit,
            &budget,
            |_budget| {
                Ok(SubmitOutcome::Accepted(AttemptRef {
                    operation: key,
                    attempt: AttemptId([8; 16]),
                    fence: 1,
                }))
            },
            |observed, _budget| {
                same_key.set(same_key.get() && observed == key);
                status_count.set(status_count.get() + 1);
                Ok(OperationStatus::Refused(Refusal::ResourceLimited))
            },
        );
        let error = result.err().context("refused status became success")?;
        anyhow::ensure!(
            error.to_string().contains("ResourceLimited"),
            "refusal reason was lost: {error:#}"
        );
        anyhow::ensure!(status_count.get() == 1, "refused status was repeated");
        anyhow::ensure!(same_key.get(), "status used another key");
        anyhow::ensure!(required_field(&audit.detail(), "/0/submit_outcome")? == "accepted");
        anyhow::ensure!(required_field(&audit.detail(), "/0/last_status")? == "refused");
        anyhow::ensure!(required_field(&audit.detail(), "/0/completion")?.is_null());
        Ok(())
    }

    #[test]
    fn unknown_status_and_status_transport_error_remain_failures() -> Result<()> {
        let key = super::super::operation(2);
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let mut unknown_audit = PublicationAudit::default();
        let unknown = reconcile_submit(
            key,
            PublicationLabel {
                phase: "full_event_ack",
                cycle: None,
            },
            &mut unknown_audit,
            &budget,
            |_budget| Ok(SubmitOutcome::Unknown(key)),
            |_key, _budget| Ok(OperationStatus::Unknown),
        );
        anyhow::ensure!(unknown.is_err(), "unknown status became success");
        anyhow::ensure!(required_field(&unknown_audit.detail(), "/0/last_status")? == "unknown");
        anyhow::ensure!(required_field(&unknown_audit.detail(), "/0/completion")?.is_null());

        let mut failed_audit = PublicationAudit::default();
        let failure = reconcile_submit(
            key,
            PublicationLabel {
                phase: "full_event_ack",
                cycle: None,
            },
            &mut failed_audit,
            &budget,
            |_budget| Err(NativeUdsError::Unknown(key)),
            |_key, _budget| {
                Err(NativeUdsError::Transport(
                    "injected status read failure".into(),
                ))
            },
        );
        let error = failure
            .err()
            .context("status transport failure became success")?;
        anyhow::ensure!(
            format!("{error:#}").contains("injected status read failure"),
            "status error was lost: {error:#}"
        );
        anyhow::ensure!(required_field(&failed_audit.detail(), "/0/status_checks")? == 1);
        anyhow::ensure!(required_field(&failed_audit.detail(), "/0/completion")?.is_null());
        Ok(())
    }

    #[test]
    fn pending_cancellation_stops_without_another_submit_or_status() -> Result<()> {
        let key = super::super::operation(2);
        let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
        let cancel = budget.cancel_handle();
        let calls = Cell::new(0_u32);
        let mut audit = PublicationAudit::default();
        let result = reconcile_submit(
            key,
            PublicationLabel {
                phase: "full_event_ack",
                cycle: None,
            },
            &mut audit,
            &budget,
            |_budget| {
                calls.set(calls.get() + 1);
                Ok(SubmitOutcome::Unknown(key))
            },
            |_key, _budget| {
                calls.set(calls.get() + 1);
                cancel.cancel();
                Ok(OperationStatus::Pending(AttemptRef {
                    operation: key,
                    attempt: AttemptId([8; 16]),
                    fence: 1,
                }))
            },
        );
        anyhow::ensure!(
            result.is_err(),
            "cancelled pending operation became success"
        );
        anyhow::ensure!(calls.get() == 2, "cancelled operation was retried");
        anyhow::ensure!(required_field(&audit.detail(), "/0/last_status")? == "pending");
        anyhow::ensure!(required_field(&audit.detail(), "/0/completion")?.is_null());
        Ok(())
    }

    #[test]
    fn cancelled_terminal_reply_cannot_complete_publication() -> Result<()> {
        let key = super::super::operation(2);
        for reconcile in [false, true] {
            let budget = RequestBudgetV1::for_duration(Duration::from_secs(5));
            let cancel = budget.cancel_handle();
            let mut audit = PublicationAudit::default();
            let result = reconcile_submit(
                key,
                PublicationLabel {
                    phase: "full_event_ack",
                    cycle: None,
                },
                &mut audit,
                &budget,
                |_budget| {
                    if reconcile {
                        Ok(SubmitOutcome::Unknown(key))
                    } else {
                        cancel.cancel();
                        Ok(SubmitOutcome::Terminal(OperationStatus::Published(
                            published_view(key),
                        )))
                    }
                },
                |_key, _budget| {
                    cancel.cancel();
                    Ok(OperationStatus::Published(published_view(key)))
                },
            );
            anyhow::ensure!(result.is_err(), "cancelled terminal became success");
            anyhow::ensure!(required_field(&audit.detail(), "/0/completion")?.is_null());
            anyhow::ensure!(
                required_field(&audit.detail(), "/0/status_checks")? == u64::from(reconcile)
            );
        }
        Ok(())
    }

    #[test]
    fn expired_budget_never_submits_or_queries() -> Result<()> {
        let key = super::super::operation(2);
        let mut audit = PublicationAudit::default();
        let budget = RequestBudgetV1::for_duration(Duration::ZERO);
        let calls = Cell::new(0_u32);
        let result = reconcile_submit(
            key,
            PublicationLabel {
                phase: "full_event_ack",
                cycle: None,
            },
            &mut audit,
            &budget,
            |_budget| {
                calls.set(calls.get() + 1);
                Ok(SubmitOutcome::Unknown(key))
            },
            |_key, _budget| {
                calls.set(calls.get() + 1);
                Ok(OperationStatus::Published(published_view(key)))
            },
        );
        anyhow::ensure!(result.is_err(), "expired request published");
        anyhow::ensure!(calls.get() == 0, "expired request crossed IPC");
        anyhow::ensure!(required_field(&audit.detail(), "/0/submit_count")? == 0);
        Ok(())
    }
}
