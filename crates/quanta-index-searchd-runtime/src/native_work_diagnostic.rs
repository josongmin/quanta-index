//! Optional diagnostics for Native publication and query failures.
//!
//! This observes the existing result. It never changes the reply or retries an
//! operation. Successful publications and on-time renewals read no configuration.

use std::{
    ffi::OsStr,
    io::Write as _,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use quanta_index_core::index::{
    IndexError, OperationKey, OperationStatus, OwnedWorkGrant, OwnedWorkReport, OwnedWorkTerminal,
    Refusal, SubmitOutcome, UpdateKind, WorkBudget, WorkClaim, WorkIntent,
};
use quanta_index_core::{DiagnosticCaptureIdV1, DiagnosticTraceRefV1};
use serde::Serialize;

const ENV: &str = "QUANTA_INDEX_NATIVE_WORK_DIAGNOSTICS_V1";
const EVENT: &str = "native_work_refusal";
const OWNED_EVENT: &str = "native_owned_work_refusal";
const QUERY_EVENT: &str = "native_effort_error_observed";
pub(super) const OWNED_FAILURE_SAMPLE_LIMIT: usize = 8;
static OWNED_FAILURES_SAMPLED: AtomicUsize = AtomicUsize::new(0);
static OWNED_RENEWALS_SAMPLED: AtomicUsize = AtomicUsize::new(0);
static OWNED_FAST_FENCE_RENEWALS_SAMPLED: AtomicUsize = AtomicUsize::new(0);
static QUERY_FAILURES_SAMPLED: AtomicUsize = AtomicUsize::new(0);
static QUERY_POST_TERMINAL_CANCELS_SAMPLED: AtomicUsize = AtomicUsize::new(0);

pub(super) fn capture_id_hex(capture_id: DiagnosticCaptureIdV1) -> String {
    use std::fmt::Write as _;

    let mut value = String::with_capacity(32);
    for byte in capture_id.0 {
        // Writing fixed integers into String is infallible.
        let _formatted = write!(&mut value, "{byte:02x}");
    }
    value
}

#[derive(Serialize)]
struct WorkRefusal<'a> {
    event: &'static str,
    operation_key: OperationKey,
    budget_diagnostic: &'a str,
    outcome: &'static str,
}

fn error_outcome(error: &IndexError) -> &'static str {
    match error {
        IndexError::InvalidInput(_) => "Err(InvalidInput)",
        IndexError::FocusSubjectNotFound => "Err(FocusSubjectNotFound)",
        IndexError::Conflict => "Err(Conflict)",
        IndexError::DigestConflict => "Err(DigestConflict)",
        IndexError::FenceLost => "Err(FenceLost)",
        IndexError::WorkFenceLost => "Err(WorkFenceLost)",
        IndexError::ResourceLimited => "Err(ResourceLimited)",
        IndexError::WorkLimitExceeded => "Err(WorkLimitExceeded)",
        IndexError::Cancelled => "Err(Cancelled)",
        IndexError::SnapshotUnavailable => "Err(SnapshotUnavailable)",
        IndexError::CorruptObject(_) => "Err(CorruptObject)",
        IndexError::BackendViolation(_) => "Err(BackendViolation)",
        IndexError::Storage(_) => "Err(Storage)",
        IndexError::RebuildRequired => "Err(RebuildRequired)",
        IndexError::MaintenanceRequired => "Err(MaintenanceRequired)",
    }
}

fn outcome_name(result: &Result<SubmitOutcome, IndexError>) -> Option<&'static str> {
    match result {
        Ok(SubmitOutcome::Terminal(OperationStatus::Refused(reason))) => {
            Some(refusal_outcome(*reason))
        }
        Ok(SubmitOutcome::Unknown(_)) => Some("Unknown"),
        Ok(SubmitOutcome::Terminal(OperationStatus::Unknown)) => Some("Terminal(Unknown)"),
        Err(error) => Some(error_outcome(error)),
        Ok(
            SubmitOutcome::Accepted(_)
            | SubmitOutcome::Terminal(OperationStatus::Pending(_) | OperationStatus::Published(_)),
        ) => None,
    }
}

fn refusal_outcome(reason: Refusal) -> &'static str {
    match reason {
        Refusal::Conflict => "Terminal(Refused(Conflict))",
        Refusal::FenceLost => "Terminal(Refused(FenceLost))",
        Refusal::InvalidInput => "Terminal(Refused(InvalidInput))",
        Refusal::ResourceLimited => "Terminal(Refused(ResourceLimited))",
        Refusal::BackendViolation => "Terminal(Refused(BackendViolation))",
        Refusal::RebuildRequired => "Terminal(Refused(RebuildRequired))",
        Refusal::MaintenanceRequired => "Terminal(Refused(MaintenanceRequired))",
    }
}

fn work_blocked_outcome(reason: Refusal) -> &'static str {
    match reason {
        Refusal::Conflict => "WorkBlocked(Conflict)",
        Refusal::FenceLost => "WorkBlocked(FenceLost)",
        Refusal::InvalidInput => "WorkBlocked(InvalidInput)",
        Refusal::ResourceLimited => "WorkBlocked(ResourceLimited)",
        Refusal::BackendViolation => "WorkBlocked(BackendViolation)",
        Refusal::RebuildRequired => "WorkBlocked(RebuildRequired)",
        Refusal::MaintenanceRequired => "WorkBlocked(MaintenanceRequired)",
    }
}

fn owned_failure_name(result: &Result<SubmitOutcome, IndexError>) -> Option<&'static str> {
    match result {
        Err(_) | Ok(SubmitOutcome::Terminal(OperationStatus::Refused(_))) => outcome_name(result),
        Ok(_) => None,
    }
}

fn encode(
    key: OperationKey,
    budget: &WorkBudget,
    outcome: &'static str,
) -> Result<Vec<u8>, serde_json::Error> {
    let budget_diagnostic = format!("{budget:?}");
    let event = WorkRefusal {
        event: EVENT,
        operation_key: key,
        budget_diagnostic: &budget_diagnostic,
        outcome,
    };
    serde_json::to_vec(&event)
}

pub(super) fn record_if_enabled(
    key: OperationKey,
    budget: &WorkBudget,
    result: &Result<SubmitOutcome, IndexError>,
) {
    let Some(outcome) = outcome_name(result) else {
        return;
    };
    if std::env::var_os(ENV).as_deref() != Some(OsStr::new("1")) {
        return;
    }
    let Ok(mut line) = encode(key, budget, outcome) else {
        return;
    };
    line.push(b'\n');
    let _written = std::io::stderr().lock().write_all(&line);
}

/// A process-local prefix sample. Missing records after the first eight do
/// not prove that owned work stopped failing.
fn owned_failure_sample_slot(counter: &AtomicUsize) -> Option<usize> {
    match counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |previous| {
        previous
            .checked_add(1)
            .filter(|next| *next <= OWNED_FAILURE_SAMPLE_LIMIT)
    }) {
        Ok(previous) => previous.checked_add(1),
        Err(_saturated) => None,
    }
}

#[derive(Serialize)]
struct QueryFailure<'a> {
    event: &'static str,
    request_id: u64,
    connection_id: u64,
    fast_delivered: bool,
    deep_delivered: bool,
    terminal_emitted: bool,
    outcome: &'static str,
    /// The first local budget refusal may precede this error; `None` does not
    /// identify a scheduler, provider or transport cause.
    first_local_denial: &'a str,
    sample_ordinal: usize,
    sample_limit: usize,
}

#[derive(Clone, Copy)]
pub(super) struct QueryDelivery {
    pub(super) fast: bool,
    pub(super) deep: bool,
    pub(super) terminal: bool,
}

// Cancellation after a terminal frame can be an ordinary transport race.
// Retain at most two such observations so they cannot displace all later
// query errors from the eight-record process-local sample.
fn query_failure_sample_slot(
    delivery: QueryDelivery,
    error: &IndexError,
    counter: &AtomicUsize,
    post_terminal_cancel_counter: &AtomicUsize,
) -> Option<usize> {
    if delivery.terminal && matches!(error, IndexError::Cancelled) {
        match post_terminal_cancel_counter.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |previous| previous.checked_add(1).filter(|next| *next <= 2),
        ) {
            Ok(_previous) => {}
            Err(_saturated) => return None,
        }
    }
    owned_failure_sample_slot(counter)
}

fn encode_query_failure(
    request_id: u64,
    connection_id: u64,
    delivery: QueryDelivery,
    budget: &WorkBudget,
    error: &IndexError,
    sample_ordinal: usize,
) -> Result<Vec<u8>, serde_json::Error> {
    let first_local_denial = format!("{:?}", budget.first_denial());
    serde_json::to_vec(&QueryFailure {
        event: QUERY_EVENT,
        request_id,
        connection_id,
        fast_delivered: delivery.fast,
        deep_delivered: delivery.deep,
        terminal_emitted: delivery.terminal,
        outcome: error_outcome(error),
        first_local_denial: &first_local_denial,
        sample_ordinal,
        sample_limit: OWNED_FAILURE_SAMPLE_LIMIT,
    })
}

/// Observe query errors, including errors after a terminal frame.
///
/// At most two post-terminal cancellations share the bounded eight
/// records; the observation does not identify the terminal cause.
pub(super) fn record_query_error_if_enabled(
    request_id: u64,
    connection_id: u64,
    delivery: QueryDelivery,
    budget: &WorkBudget,
    error: &IndexError,
) {
    if std::env::var_os(ENV).as_deref() != Some(OsStr::new("1")) {
        return;
    }
    let Some(sample_ordinal) = query_failure_sample_slot(
        delivery,
        error,
        &QUERY_FAILURES_SAMPLED,
        &QUERY_POST_TERMINAL_CANCELS_SAMPLED,
    ) else {
        return;
    };
    let Ok(mut line) = encode_query_failure(
        request_id,
        connection_id,
        delivery,
        budget,
        error,
        sample_ordinal,
    ) else {
        return;
    };
    line.push(b'\n');
    let _written = std::io::stderr().lock().write_all(&line);
}

// Repeated fast fence loss can race an already completed publication. Keep
// two observations, leaving space for slower renewals or other refusal classes.
// These records alone do not establish why the original fence was lost.
fn owned_renewal_sample_slot(
    fast_fence_loss: bool,
    counter: &AtomicUsize,
    fast_fence_counter: &AtomicUsize,
) -> Option<usize> {
    if fast_fence_loss {
        match fast_fence_counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |previous| {
            previous.checked_add(1).filter(|next| *next <= 2)
        }) {
            Ok(_previous) => {}
            Err(_saturated) => return None,
        }
    }
    owned_failure_sample_slot(counter)
}

/// Only the actual grant can supply execution identity. Pre-grant refusal
/// cannot accidentally borrow a previous dispatch's budget or trace span.
#[derive(Clone, Copy)]
pub(super) enum OwnedWorkObservation<'a> {
    BeforeGrant(OperationKey),
    Granted {
        grant: &'a OwnedWorkGrant,
        budget: &'a WorkBudget,
        trace: Option<DiagnosticTraceRefV1>,
    },
}

#[derive(Serialize)]
struct OwnedTraceLocation {
    capture_id: String,
    span_id: u64,
}

#[derive(Serialize)]
struct OwnedExecutionRecord<'a> {
    dispatch_ordinal: u32,
    assignment_dispatches: u32,
    assignment_deadline_ms: u64,
    claim: Option<&'a WorkClaim>,
    trace: Option<OwnedTraceLocation>,
}

#[derive(Serialize)]
struct OwnedWorkRefusal<'a> {
    event: &'static str,
    operation_key: OperationKey,
    budget_diagnostic: Option<&'a str>,
    execution: Option<OwnedExecutionRecord<'a>>,
    phase: &'static str,
    outcome: &'static str,
    sample_ordinal: usize,
    sample_limit: usize,
}

fn encode_owned(
    observation: OwnedWorkObservation<'_>,
    outcome: &'static str,
    sample_ordinal: usize,
) -> Result<Vec<u8>, serde_json::Error> {
    let (key, budget, execution) = match observation {
        OwnedWorkObservation::BeforeGrant(key) => (key, None, None),
        OwnedWorkObservation::Granted {
            grant,
            budget,
            trace,
        } => {
            let claim = match &grant.assignment.request.kind {
                UpdateKind::Materialize { claim } => Some(claim),
                UpdateKind::SourceAdvance(_)
                | UpdateKind::SourceResync { .. }
                | UpdateKind::ProjectionAdvance(_)
                | UpdateKind::Reconfigure { .. } => None,
            };
            (
                grant.assignment.request.operation,
                Some(budget),
                Some(OwnedExecutionRecord {
                    dispatch_ordinal: grant.ordinal,
                    assignment_dispatches: grant.assignment.dispatches,
                    assignment_deadline_ms: grant.assignment.deadline_ms,
                    claim,
                    trace: trace.map(|trace| OwnedTraceLocation {
                        capture_id: capture_id_hex(trace.capture_id),
                        span_id: trace.span_id.get(),
                    }),
                }),
            )
        }
    };
    let diagnostic = budget.map(|budget| format!("{budget:?}"));
    serde_json::to_vec(&OwnedWorkRefusal {
        event: OWNED_EVENT,
        operation_key: key,
        budget_diagnostic: diagnostic.as_deref(),
        execution,
        phase: if budget.is_some() {
            "granted_execution"
        } else {
            "before_grant"
        },
        outcome,
        sample_ordinal,
        sample_limit: OWNED_FAILURE_SAMPLE_LIMIT,
    })
}

pub(super) fn record_owned_if_enabled(
    observation: OwnedWorkObservation<'_>,
    result: &Result<SubmitOutcome, IndexError>,
) {
    let Some(outcome) = owned_failure_name(result) else {
        return;
    };
    record_owned_outcome_if_enabled(observation, outcome);
}

/// Record the persisted work refusal observed by grant admission, without
/// synthesizing an execution result for a job that did not start.
pub(super) fn record_owned_terminal_before_grant_if_enabled(
    key: OperationKey,
    status: OwnedWorkTerminal,
) {
    if let OwnedWorkTerminal::Blocked(reason) = status {
        record_owned_outcome_if_enabled(
            OwnedWorkObservation::BeforeGrant(key),
            work_blocked_outcome(reason),
        );
    }
}

fn record_owned_outcome_if_enabled(observation: OwnedWorkObservation<'_>, outcome: &'static str) {
    if std::env::var_os(ENV).as_deref() != Some(OsStr::new("1")) {
        return;
    }
    let Some(sample_ordinal) = owned_failure_sample_slot(&OWNED_FAILURES_SAMPLED) else {
        return;
    };
    let Ok(mut line) = encode_owned(observation, outcome, sample_ordinal) else {
        return;
    };
    line.push(b'\n');
    let _written = std::io::stderr().lock().write_all(&line);
}

#[derive(Serialize)]
struct OwnedRenewal {
    event: &'static str,
    operation_key: OperationKey,
    dispatch_ordinal: u32,
    claim_fence: u64,
    intent: WorkIntent,
    lease_ms: u64,
    interval_ms: u128,
    elapsed_ms: u128,
    phase: &'static str,
    outcome: &'static str,
    renewed_until_ms: Option<u64>,
    sample_ordinal: usize,
    sample_limit: usize,
}

/// The sampled event borrows the original grant through the whole pump run.
/// The fixed followup array stores only a pointer and this sample ordinal.
#[derive(Clone, Copy)]
pub(super) struct OwnedRenewalToken<'a> {
    grant: &'a OwnedWorkGrant,
    sample_ordinal: usize,
}

#[derive(Serialize)]
struct OwnedRenewalFollowup {
    event: &'static str,
    operation_key: OperationKey,
    dispatch_ordinal: u32,
    claim_fence: u64,
    intent: WorkIntent,
    sample_ordinal: usize,
    sample_limit: usize,
    submit_outcome: &'static str,
    interrupted: bool,
    completion_error: Option<&'static str>,
    report: &'static str,
    worker_quiesced: bool,
    finish_outcome: &'static str,
}

fn submit_outcome(result: &Result<SubmitOutcome, IndexError>) -> &'static str {
    match result {
        Ok(SubmitOutcome::Accepted(_)) => "Accepted",
        Ok(SubmitOutcome::Unknown(_)) => "Unknown",
        Ok(SubmitOutcome::Terminal(OperationStatus::Unknown)) => "Terminal(Unknown)",
        Ok(SubmitOutcome::Terminal(OperationStatus::Pending(_))) => "Terminal(Pending)",
        Ok(SubmitOutcome::Terminal(OperationStatus::Published(_))) => "Terminal(Published)",
        Ok(SubmitOutcome::Terminal(OperationStatus::Refused(reason))) => refusal_outcome(*reason),
        Err(error) => error_outcome(error),
    }
}

fn report_outcome(report: OwnedWorkReport) -> &'static str {
    match report {
        OwnedWorkReport::Reconcile => "Reconcile",
        OwnedWorkReport::Interrupted => "Interrupted",
        OwnedWorkReport::Failed(Refusal::Conflict) => "Failed(Conflict)",
        OwnedWorkReport::Failed(Refusal::FenceLost) => "Failed(FenceLost)",
        OwnedWorkReport::Failed(Refusal::InvalidInput) => "Failed(InvalidInput)",
        OwnedWorkReport::Failed(Refusal::ResourceLimited) => "Failed(ResourceLimited)",
        OwnedWorkReport::Failed(Refusal::BackendViolation) => "Failed(BackendViolation)",
        OwnedWorkReport::Failed(Refusal::RebuildRequired) => "Failed(RebuildRequired)",
        OwnedWorkReport::Failed(Refusal::MaintenanceRequired) => "Failed(MaintenanceRequired)",
    }
}

fn encode_owned_renewal_followup(
    token: OwnedRenewalToken<'_>,
    result: &Result<SubmitOutcome, IndexError>,
    interrupted: bool,
    completion_error: Option<&IndexError>,
    report: OwnedWorkReport,
    worker_quiesced: bool,
    finish: &Result<(), IndexError>,
) -> serde_json::Result<Vec<u8>> {
    let UpdateKind::Materialize { claim } = &token.grant.assignment.request.kind else {
        return Err(serde_json::Error::io(std::io::Error::other(
            "sampled renewal grant is not materialization",
        )));
    };
    serde_json::to_vec(&OwnedRenewalFollowup {
        event: "native_owned_renewal_followup",
        operation_key: token.grant.assignment.request.operation,
        dispatch_ordinal: token.grant.ordinal,
        claim_fence: claim.fence,
        intent: claim.intent,
        sample_ordinal: token.sample_ordinal,
        sample_limit: OWNED_FAILURE_SAMPLE_LIMIT,
        submit_outcome: submit_outcome(result),
        interrupted,
        completion_error: completion_error.map(error_outcome),
        report: report_outcome(report),
        worker_quiesced,
        finish_outcome: match finish {
            Ok(()) => "Ok",
            Err(error) => error_outcome(error),
        },
    })
}

pub(super) fn record_owned_renewal_followup(
    token: OwnedRenewalToken<'_>,
    result: &Result<SubmitOutcome, IndexError>,
    interrupted: bool,
    completion_error: Option<&IndexError>,
    report: OwnedWorkReport,
    worker_quiesced: bool,
    finish: &Result<(), IndexError>,
) {
    let Ok(mut line) = encode_owned_renewal_followup(
        token,
        result,
        interrupted,
        completion_error,
        report,
        worker_quiesced,
        finish,
    ) else {
        return;
    };
    line.push(b'\n');
    let _written = std::io::stderr().lock().write_all(&line);
}

/// Observe slow or refused renewals with bounded opt-in diagnostics.
///
/// At most two fast fence losses share its eight slots. This does not sample
/// successful fast renewals or move a deadline, lease or retry.
pub(super) fn record_owned_renewal_if_enabled<'a>(
    grant: &'a OwnedWorkGrant,
    lease_ms: u64,
    renewal_period: Duration,
    interval: Duration,
    elapsed: Duration,
    phase: &'static str,
    result: &Result<WorkClaim, IndexError>,
) -> Option<OwnedRenewalToken<'a>> {
    if result.is_ok() && interval < Duration::from_millis(lease_ms) && elapsed < renewal_period {
        return None;
    }
    if std::env::var_os(ENV).as_deref() != Some(OsStr::new("1")) {
        return None;
    }
    let fast_fence_loss = matches!(result, Err(IndexError::WorkFenceLost))
        && interval < Duration::from_millis(lease_ms)
        && elapsed < renewal_period;
    let sample_ordinal = owned_renewal_sample_slot(
        fast_fence_loss,
        &OWNED_RENEWALS_SAMPLED,
        &OWNED_FAST_FENCE_RENEWALS_SAMPLED,
    )?;
    let UpdateKind::Materialize { claim } = &grant.assignment.request.kind else {
        return None;
    };
    let token = OwnedRenewalToken {
        grant,
        sample_ordinal,
    };
    let (outcome, renewed_until_ms) = match result {
        Ok(claim) => ("Ok", Some(claim.lease_until_ms)),
        Err(error) => (error_outcome(error), None),
    };
    let event = OwnedRenewal {
        event: "native_owned_renewal",
        operation_key: grant.assignment.request.operation,
        dispatch_ordinal: grant.ordinal,
        claim_fence: claim.fence,
        intent: claim.intent,
        lease_ms,
        interval_ms: interval.as_millis(),
        elapsed_ms: elapsed.as_millis(),
        phase,
        outcome,
        renewed_until_ms,
        sample_ordinal,
        sample_limit: OWNED_FAILURE_SAMPLE_LIMIT,
    };
    let Ok(mut line) = serde_json::to_vec(&event) else {
        return None;
    };
    line.push(b'\n');
    match std::io::stderr().lock().write_all(&line) {
        Ok(()) => Some(token),
        Err(_diagnostic_error) => None,
    }
}

/// Fixed failure locations for the synchronous discard path. None contains an
/// operation identity, source path, input body, or backend error text.
#[derive(Clone, Copy, Debug)]
pub(crate) enum DiscardFailureStage {
    RequestCheckpoint,
    SchedulerAdmission,
    ExecuteCheckpoint,
    ReplyReservation,
    PhysicalMaintenanceAdmission,
    CatalogDiscard,
}

impl DiscardFailureStage {
    const fn as_str(self) -> &'static str {
        match self {
            Self::RequestCheckpoint => "request_checkpoint",
            Self::SchedulerAdmission => "scheduler_admission",
            Self::ExecuteCheckpoint => "execute_checkpoint",
            Self::ReplyReservation => "reply_reservation",
            Self::PhysicalMaintenanceAdmission => "physical_maintenance_admission",
            Self::CatalogDiscard => "catalog_discard",
        }
    }
}

#[derive(Serialize)]
struct DiscardRefusal<'a> {
    event: &'static str,
    stage: &'static str,
    budget_diagnostic: &'a str,
    outcome: &'static str,
}

fn encode_discard(
    stage: DiscardFailureStage,
    budget: &WorkBudget,
    error: &IndexError,
) -> Result<Vec<u8>, serde_json::Error> {
    let budget_diagnostic = format!("{budget:?}");
    serde_json::to_vec(&DiscardRefusal {
        event: "native_discard_refusal",
        stage: stage.as_str(),
        budget_diagnostic: &budget_diagnostic,
        outcome: error_outcome(error),
    })
}

pub(crate) fn record_discard_if_enabled(
    stage: DiscardFailureStage,
    budget: &WorkBudget,
    error: &IndexError,
) {
    if std::env::var_os(ENV).as_deref() != Some(OsStr::new("1")) {
        return;
    }
    let Ok(mut line) = encode_discard(stage, budget, error) else {
        return;
    };
    line.push(b'\n');
    let _written = std::io::stderr().lock().write_all(&line);
}

/// Failure-only numeric custody observations. Missing/invalid observations are
/// explicit; no path, backend message, guessed zero or device-free claim.
pub(crate) fn record_discard_physical_if_enabled(
    catalog: &quanta_index_catalog::SqliteCatalog,
    physical: &crate::publication_file_capacity::PublicationFileCapacity,
) {
    if std::env::var_os(ENV).as_deref() != Some(OsStr::new("1")) {
        return;
    }
    let file_bytes = |path: &std::path::Path| -> Result<u64, &'static str> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => Ok(metadata.len()),
            Ok(_) => Err("not_regular"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err("missing"),
            Err(_) => Err("unavailable"),
        }
    };
    let mut wal = catalog.path().as_os_str().to_os_string();
    wal.push("-wal");
    let reserve = physical
        .maintenance_available()
        .map_err(|_error| "unavailable");
    let snapshot = physical
        .capacity_snapshot()
        .map(|value| {
            (
                value.used_bytes,
                value.used_files,
                value.active_allowances,
                value.active_tickets,
                value.uncertain_pending,
                value.poisoned,
            )
        })
        .map_err(|_error| "unavailable");
    let event = serde_json::json!({
        "event": "native_discard_physical_refusal",
        "scope": "post_error_regular_file_and_reserved_custody_observation",
        "main_file_bytes": file_bytes(catalog.path()),
        "wal_file_bytes": file_bytes(std::path::Path::new(&wal)),
        "maintenance_available_bytes_files": reserve,
        "capacity_used_bytes_files_allowances_tickets_uncertain_poisoned": snapshot,
    });
    let Ok(mut line) = serde_json::to_vec(&event) else {
        return;
    };
    line.push(b'\n');
    let _written = std::io::stderr().lock().write_all(&line);
}

#[cfg(test)]
mod tests {
    use std::{sync::atomic::AtomicUsize, time::Duration};

    use quanta_index_core::{
        DiagnosticSpanIdV1, RequestBudgetV1,
        index::{
            CollectionId, Digest, IndexInstanceId, MapRoot, OwnedWorkAssignment, OwnedWorkPolicy,
            ProducerId, ProjectionId, SlotId, SourceCut, StreamId, TenantId, UpdateRequest,
            ViewRef, WorkIntent, WorkLimits, WorkScope, WorkUsage, WorkspaceKey, WorktreeId,
        },
    };

    use super::*;

    fn key() -> OperationKey {
        OperationKey {
            workspace: WorkspaceKey {
                tenant: TenantId([1; 16]),
                collection: CollectionId([2; 16]),
                worktree: WorktreeId([3; 16]),
                slot: SlotId([4; 16]),
            },
            epoch: 5,
            producer: ProducerId([6; 16]),
            stream: StreamId([7; 16]),
            stream_epoch: 8,
            sequence: 9,
        }
    }

    fn grant_fixture(budget: &WorkBudget, ordinal: u32, fence: u64) -> OwnedWorkGrant {
        let operation = key();
        OwnedWorkGrant {
            assignment: OwnedWorkAssignment {
                request: UpdateRequest {
                    operation,
                    expected_view: ViewRef {
                        workspace: operation.workspace,
                        epoch: operation.epoch,
                        sequence: 10,
                        digest: Digest([11; 32]),
                    },
                    kind: UpdateKind::Materialize {
                        claim: WorkClaim {
                            intent: WorkIntent {
                                workspace: operation.workspace,
                                epoch: operation.epoch,
                                instance: IndexInstanceId([12; 16]),
                                scope: WorkScope::ProjectionDomain {
                                    projection: ProjectionId([13; 16]),
                                    source: SourceCut {
                                        owners: MapRoot(None),
                                        paths: MapRoot(None),
                                        frontiers: MapRoot(None),
                                    },
                                    producer_policy: Digest([14; 32]),
                                    dependency_context: Digest([15; 32]),
                                },
                            },
                            fence,
                            lease_until_ms: 17_000,
                        },
                    },
                },
                policy: OwnedWorkPolicy {
                    dispatch_limits: WorkLimits {
                        reads: 1,
                        writes: 1,
                        bytes: 1,
                        mutations: 1,
                    },
                    max_dispatches: 2,
                    total_timeout_ms: 1_000,
                    lease_ms: 1_000,
                    retry_backoff_ms: 1,
                },
                deadline_ms: 18_000,
                dispatches: ordinal,
                due_ms: 16_000,
                active: true,
            },
            ordinal,
            execution_deadline: budget.request().deadline(),
        }
    }

    #[test]
    fn query_failure_records_only_fixed_identity_delivery_and_local_denial()
    -> Result<(), Box<dyn std::error::Error>> {
        let limits = WorkLimits {
            reads: 0,
            writes: 10,
            bytes: 0,
            mutations: 10,
        };
        let mut reads = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        if reads.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        }) != Err(IndexError::ResourceLimited)
        {
            return Err("query read refusal gold changed".into());
        }
        let mut bytes = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        if bytes.charge(WorkUsage {
            bytes: 1,
            ..WorkUsage::default()
        }) != Err(IndexError::ResourceLimited)
        {
            return Err("query byte refusal gold changed".into());
        }
        let cancelled_request = RequestBudgetV1::for_duration(Duration::from_secs(30));
        let cancelled = WorkBudget::with_request(limits, cancelled_request.clone());
        cancelled_request.cancel_handle().cancel();
        if cancelled.checkpoint() != Err(IndexError::Cancelled) {
            return Err("query cancellation refusal gold changed".into());
        }
        let no_local_denial = WorkBudget::with_request(
            limits,
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        for (budget, error, expected_denial, expected_outcome) in [
            (
                &reads,
                IndexError::ResourceLimited,
                "Some(Reads)",
                "Err(ResourceLimited)",
            ),
            (
                &bytes,
                IndexError::ResourceLimited,
                "Some(Bytes)",
                "Err(ResourceLimited)",
            ),
            (
                &cancelled,
                IndexError::Cancelled,
                "Some(Cancelled)",
                "Err(Cancelled)",
            ),
            (
                &no_local_denial,
                IndexError::ResourceLimited,
                "None",
                "Err(ResourceLimited)",
            ),
            (
                &no_local_denial,
                IndexError::Storage("provider-secret-must-not-appear".into()),
                "None",
                "Err(Storage)",
            ),
        ] {
            let record: serde_json::Value = serde_json::from_slice(&encode_query_failure(
                17,
                23,
                QueryDelivery {
                    fast: true,
                    deep: false,
                    terminal: true,
                },
                budget,
                &error,
                2,
            )?)?;
            if record.get("event").and_then(serde_json::Value::as_str) != Some(QUERY_EVENT)
                || record.get("request_id").and_then(serde_json::Value::as_u64) != Some(17)
                || record
                    .get("connection_id")
                    .and_then(serde_json::Value::as_u64)
                    != Some(23)
                || record
                    .get("fast_delivered")
                    .and_then(serde_json::Value::as_bool)
                    != Some(true)
                || record
                    .get("deep_delivered")
                    .and_then(serde_json::Value::as_bool)
                    != Some(false)
                || record
                    .get("terminal_emitted")
                    .and_then(serde_json::Value::as_bool)
                    != Some(true)
                || record.get("outcome").and_then(serde_json::Value::as_str)
                    != Some(expected_outcome)
                || record
                    .get("first_local_denial")
                    .and_then(serde_json::Value::as_str)
                    != Some(expected_denial)
                || record
                    .get("sample_ordinal")
                    .and_then(serde_json::Value::as_u64)
                    != Some(2)
                || record
                    .get("sample_limit")
                    .and_then(serde_json::Value::as_u64)
                    != Some(8)
                || record.as_object().map(serde_json::Map::len) != Some(10)
                || serde_json::to_string(&record)?.contains("provider-secret-must-not-appear")
            {
                return Err("query diagnostic changed fixed error observation".into());
            }
        }
        Ok(())
    }

    #[test]
    fn query_failure_prefix_does_not_exceed_eight_records() -> Result<(), Box<dyn std::error::Error>>
    {
        let counter = AtomicUsize::new(0);
        for expected in 1..=OWNED_FAILURE_SAMPLE_LIMIT {
            if owned_failure_sample_slot(&counter) != Some(expected) {
                return Err("query diagnostic prefix slot differs".into());
            }
        }
        if owned_failure_sample_slot(&counter).is_some()
            || counter.load(Ordering::Relaxed) != OWNED_FAILURE_SAMPLE_LIMIT
        {
            return Err("query diagnostic exceeded prefix bound".into());
        }
        Ok(())
    }

    #[test]
    fn post_terminal_cancellation_retains_six_other_query_error_slots()
    -> Result<(), Box<dyn std::error::Error>> {
        let total = AtomicUsize::new(0);
        let post_terminal_cancel = AtomicUsize::new(0);
        let terminal = QueryDelivery {
            fast: true,
            deep: true,
            terminal: true,
        };
        for expected in [Some(1), Some(2)] {
            if query_failure_sample_slot(
                terminal,
                &IndexError::Cancelled,
                &total,
                &post_terminal_cancel,
            ) != expected
            {
                return Err("initial post-terminal cancellations were not retained".into());
            }
        }
        for _ in 0..100 {
            if query_failure_sample_slot(
                terminal,
                &IndexError::Cancelled,
                &total,
                &post_terminal_cancel,
            )
            .is_some()
            {
                return Err("repeated post-terminal cancellation consumed other slots".into());
            }
        }
        for expected in 3..=OWNED_FAILURE_SAMPLE_LIMIT {
            let (delivery, error) = if expected == 3 {
                (
                    QueryDelivery {
                        terminal: false,
                        ..terminal
                    },
                    IndexError::Cancelled,
                )
            } else {
                (terminal, IndexError::Storage("private failure".into()))
            };
            if query_failure_sample_slot(delivery, &error, &total, &post_terminal_cancel)
                != Some(expected)
            {
                return Err("other query error was displaced from the sample".into());
            }
        }
        if query_failure_sample_slot(
            terminal,
            &IndexError::Storage("ninth failure".into()),
            &total,
            &post_terminal_cancel,
        )
        .is_some()
            || total.load(Ordering::Relaxed) != OWNED_FAILURE_SAMPLE_LIMIT
            || post_terminal_cancel.load(Ordering::Relaxed) != 2
        {
            return Err("query error sample exceeded its original bound".into());
        }
        Ok(())
    }

    #[test]
    fn repeated_fast_fence_loss_preserves_six_important_renewal_slots()
    -> Result<(), Box<dyn std::error::Error>> {
        let total = AtomicUsize::new(0);
        let fast = AtomicUsize::new(0);
        for expected in [Some(1), Some(2)] {
            if owned_renewal_sample_slot(true, &total, &fast) != expected {
                return Err("first two fast fence losses were not retained".into());
            }
        }
        for _ in 0..100 {
            if owned_renewal_sample_slot(true, &total, &fast).is_some() {
                return Err("repeated fast fence loss consumed important slots".into());
            }
        }
        for expected in 3..=8 {
            if owned_renewal_sample_slot(false, &total, &fast) != Some(expected) {
                return Err("important renewal observation was displaced".into());
            }
        }
        if owned_renewal_sample_slot(false, &total, &fast).is_some()
            || total.load(Ordering::Relaxed) != 8
            || fast.load(Ordering::Relaxed) != 2
        {
            return Err("renewal sample exceeded the original eight-record bound".into());
        }
        Ok(())
    }

    #[test]
    fn important_renewals_can_use_all_eight_slots_without_expanding_the_cap()
    -> Result<(), Box<dyn std::error::Error>> {
        let total = AtomicUsize::new(0);
        let fast = AtomicUsize::new(0);
        for expected in 1..=8 {
            if owned_renewal_sample_slot(false, &total, &fast) != Some(expected) {
                return Err("important renewals were unnecessarily restricted".into());
            }
        }
        if owned_renewal_sample_slot(true, &total, &fast).is_some()
            || owned_renewal_sample_slot(false, &total, &fast).is_some()
            || total.load(Ordering::Relaxed) != 8
        {
            return Err("later observations expanded the total sample cap".into());
        }
        Ok(())
    }

    #[test]
    fn renewal_followup_preserves_original_grant_and_outcome_variants()
    -> Result<(), Box<dyn std::error::Error>> {
        let budget = WorkBudget::with_request(
            WorkLimits {
                reads: 1,
                writes: 1,
                bytes: 1,
                mutations: 1,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(1)),
        );
        let grant = grant_fixture(&budget, 2, 73);
        let UpdateKind::Materialize { claim } = &grant.assignment.request.kind else {
            return Err("owned test grant is not materialization".into());
        };
        let token = OwnedRenewalToken {
            grant: &grant,
            sample_ordinal: 4,
        };
        let published = Ok(SubmitOutcome::Terminal(OperationStatus::Published(
            grant.assignment.request.expected_view,
        )));
        let success: serde_json::Value = serde_json::from_slice(&encode_owned_renewal_followup(
            token,
            &published,
            true,
            None,
            OwnedWorkReport::Interrupted,
            true,
            &Ok(()),
        )?)?;
        if success.get("event").and_then(serde_json::Value::as_str)
            != Some("native_owned_renewal_followup")
            || success.get("operation_key")
                != Some(&serde_json::to_value(grant.assignment.request.operation)?)
            || success.get("intent") != Some(&serde_json::to_value(claim.intent)?)
            || success
                .get("sample_ordinal")
                .and_then(serde_json::Value::as_u64)
                != Some(4)
            || success
                .get("dispatch_ordinal")
                .and_then(serde_json::Value::as_u64)
                != Some(2)
            || success
                .get("claim_fence")
                .and_then(serde_json::Value::as_u64)
                != Some(73)
            || success
                .get("submit_outcome")
                .and_then(serde_json::Value::as_str)
                != Some("Terminal(Published)")
            || success.get("report").and_then(serde_json::Value::as_str) != Some("Interrupted")
            || success
                .get("worker_quiesced")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            || success
                .get("finish_outcome")
                .and_then(serde_json::Value::as_str)
                != Some("Ok")
        {
            return Err("renewal followup lost original grant or published outcome".into());
        }
        let cancelled: Result<SubmitOutcome, IndexError> = Err(IndexError::Cancelled);
        let failure: serde_json::Value = serde_json::from_slice(&encode_owned_renewal_followup(
            token,
            &cancelled,
            true,
            Some(&IndexError::Storage("renewal failed".into())),
            OwnedWorkReport::Interrupted,
            false,
            &Err(IndexError::WorkFenceLost),
        )?)?;
        if failure
            .get("submit_outcome")
            .and_then(serde_json::Value::as_str)
            != Some("Err(Cancelled)")
            || failure
                .get("completion_error")
                .and_then(serde_json::Value::as_str)
                != Some("Err(Storage)")
            || failure
                .get("worker_quiesced")
                .and_then(serde_json::Value::as_bool)
                != Some(false)
            || failure
                .get("finish_outcome")
                .and_then(serde_json::Value::as_str)
                != Some("Err(WorkFenceLost)")
        {
            return Err("renewal followup converted an uncertain result to success".into());
        }
        Ok(())
    }

    #[test]
    fn owned_failure_sample_is_bounded_and_does_not_invent_pregrant_budget()
    -> Result<(), Box<dyn std::error::Error>> {
        let counter = AtomicUsize::new(0);
        for expected in 1..=OWNED_FAILURE_SAMPLE_LIMIT {
            if owned_failure_sample_slot(&counter) != Some(expected) {
                return Err("owned diagnostic prefix slot differs".into());
            }
        }
        if owned_failure_sample_slot(&counter).is_some()
            || counter.load(Ordering::Relaxed) != OWNED_FAILURE_SAMPLE_LIMIT
        {
            return Err("owned diagnostic exceeded process sample cap".into());
        }
        let result = Err(IndexError::ResourceLimited);
        let outcome = owned_failure_name(&result).ok_or("owned refusal was not selected")?;
        if owned_failure_name(&Ok(SubmitOutcome::Unknown(key()))).is_some() {
            return Err("owned indeterminate outcome was treated as failure".into());
        }
        let pregrant: serde_json::Value = serde_json::from_slice(&encode_owned(
            OwnedWorkObservation::BeforeGrant(key()),
            outcome,
            1,
        )?)?;
        if pregrant.get("operation_key") != Some(&serde_json::to_value(key())?)
            || pregrant.get("budget_diagnostic") != Some(&serde_json::Value::Null)
            || pregrant.get("execution") != Some(&serde_json::Value::Null)
            || pregrant.get("phase").and_then(serde_json::Value::as_str) != Some("before_grant")
            || pregrant.get("outcome").and_then(serde_json::Value::as_str)
                != Some("Err(ResourceLimited)")
            || pregrant
                .get("sample_limit")
                .and_then(serde_json::Value::as_u64)
                != Some(8)
        {
            return Err("owned pregrant diagnostic invented or omitted authority".into());
        }
        let budget = WorkBudget::with_request(
            WorkLimits {
                reads: 1,
                writes: 1,
                bytes: 1,
                mutations: 1,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(1)),
        );
        let grant = grant_fixture(&budget, 1, 16);
        let granted: serde_json::Value = serde_json::from_slice(&encode_owned(
            OwnedWorkObservation::Granted {
                grant: &grant,
                budget: &budget,
                trace: None,
            },
            outcome,
            2,
        )?)?;
        if granted.get("phase").and_then(serde_json::Value::as_str) != Some("granted_execution")
            || granted
                .get("budget_diagnostic")
                .and_then(serde_json::Value::as_str)
                .is_none()
            || granted.pointer("/execution/trace") != Some(&serde_json::Value::Null)
            || granted
                .pointer("/execution/dispatch_ordinal")
                .and_then(serde_json::Value::as_u64)
                != Some(1)
            || granted
                .pointer("/execution/claim/fence")
                .and_then(serde_json::Value::as_u64)
                != Some(16)
        {
            return Err("owned granted diagnostic lost original budget".into());
        }
        Ok(())
    }

    #[test]
    fn owned_failure_join_distinguishes_dispatch_and_capture_reuse()
    -> Result<(), Box<dyn std::error::Error>> {
        let budget = WorkBudget::with_request(
            WorkLimits {
                reads: 1,
                writes: 1,
                bytes: 1,
                mutations: 1,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(1)),
        );
        let span_id = DiagnosticSpanIdV1::from_nonzero(
            std::num::NonZeroU64::new(7).ok_or("fixed span is zero")?,
        );
        let first = grant_fixture(&budget, 1, 16);
        let second = grant_fixture(&budget, 2, 19);
        for (grant, capture, expected_ordinal, expected_fence, expected_capture) in [
            (&first, 0xab, 1, 16, "abababababababababababababababab"),
            (&second, 0xcd, 2, 19, "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"),
        ] {
            let bytes = encode_owned(
                OwnedWorkObservation::Granted {
                    grant,
                    budget: &budget,
                    trace: Some(DiagnosticTraceRefV1 {
                        capture_id: DiagnosticCaptureIdV1([capture; 16]),
                        span_id,
                    }),
                },
                "Err(ResourceLimited)",
                1,
            )?;
            let record: serde_json::Value = serde_json::from_slice(&bytes)?;
            let execution = record.get("execution").ok_or("execution missing")?;
            let UpdateKind::Materialize { claim } = &grant.assignment.request.kind else {
                return Err("fixture lacks materialization claim".into());
            };
            if record.get("operation_key") != Some(&serde_json::to_value(key())?)
                || execution
                    .get("dispatch_ordinal")
                    .and_then(serde_json::Value::as_u64)
                    != Some(expected_ordinal)
                || execution
                    .get("assignment_dispatches")
                    .and_then(serde_json::Value::as_u64)
                    != Some(expected_ordinal)
                || execution
                    .get("assignment_deadline_ms")
                    .and_then(serde_json::Value::as_u64)
                    != Some(18_000)
                || execution.get("claim") != Some(&serde_json::to_value(claim)?)
                || execution
                    .pointer("/claim/fence")
                    .and_then(serde_json::Value::as_u64)
                    != Some(expected_fence)
                || execution
                    .pointer("/trace/capture_id")
                    .and_then(serde_json::Value::as_str)
                    != Some(expected_capture)
                || execution
                    .pointer("/trace/span_id")
                    .and_then(serde_json::Value::as_u64)
                    != Some(7)
                || bytes.len() > 8 * 1024
            {
                return Err("owned failure lost bounded original grant or capture identity".into());
            }
        }
        Ok(())
    }

    #[test]
    fn pregrant_work_blocked_diagnostic_does_not_claim_operation_refusal()
    -> Result<(), Box<dyn std::error::Error>> {
        let actual_receipt = owned_failure_name(&Ok(SubmitOutcome::Terminal(
            OperationStatus::Refused(Refusal::ResourceLimited),
        )))
        .ok_or("actual refused receipt was not classified")?;
        if actual_receipt != "Terminal(Refused(ResourceLimited))" {
            return Err("actual operation refusal label changed".into());
        }
        let work_outcome = work_blocked_outcome(Refusal::ResourceLimited);
        if work_outcome == actual_receipt {
            return Err("work block was mislabeled as an operation receipt".into());
        }
        let record: serde_json::Value = serde_json::from_slice(&encode_owned(
            OwnedWorkObservation::BeforeGrant(key()),
            work_outcome,
            1,
        )?)?;
        if record.get("event").and_then(serde_json::Value::as_str) != Some(OWNED_EVENT)
            || record.get("phase").and_then(serde_json::Value::as_str) != Some("before_grant")
            || record.get("outcome").and_then(serde_json::Value::as_str)
                != Some("WorkBlocked(ResourceLimited)")
            || record.get("budget_diagnostic") != Some(&serde_json::Value::Null)
            || record.get("execution") != Some(&serde_json::Value::Null)
        {
            return Err("pregrant work block diagnostic claimed execution or a receipt".into());
        }
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "assertions compare the encoded diagnostic with fixed refusal and identity goldens"
    )]
    fn refusal_record_keeps_original_key_and_first_denied_axis()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 10,
                writes: 10,
                bytes: 4,
                mutations: 10,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        assert_eq!(
            budget.charge(WorkUsage {
                bytes: 5,
                ..WorkUsage::default()
            }),
            Err(IndexError::ResourceLimited)
        );
        let result = Ok(SubmitOutcome::Terminal(OperationStatus::Refused(
            Refusal::ResourceLimited,
        )));
        let outcome = outcome_name(&result).ok_or("refusal was not selected")?;
        let encoded = encode(key(), &budget, outcome)?;
        let record: serde_json::Value = serde_json::from_slice(&encoded)?;
        assert_eq!(
            record.get("event").and_then(serde_json::Value::as_str),
            Some(EVENT)
        );
        assert_eq!(
            record.get("operation_key"),
            Some(&serde_json::to_value(key())?)
        );
        assert_eq!(
            record.get("outcome").and_then(serde_json::Value::as_str),
            Some("Terminal(Refused(ResourceLimited))")
        );
        let diagnostic = record
            .get("budget_diagnostic")
            .and_then(serde_json::Value::as_str)
            .ok_or("budget diagnostic absent")?;
        assert!(diagnostic.contains("first_denial: \"bytes\""));
        assert!(diagnostic.contains("axis: \"bytes\""));
        assert!(diagnostic.contains("used: 0, attempted: 5, limit: 4"));
        Ok(())
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "assertions compare fixed discard diagnostic fields and independent budget gold"
    )]
    fn discard_failure_stage_and_budget_diagnostic_preserve_original_error_class()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut budget = WorkBudget::with_request(
            WorkLimits {
                reads: 10,
                writes: 10,
                bytes: 4,
                mutations: 10,
            },
            RequestBudgetV1::for_duration(Duration::from_secs(30)),
        );
        assert_eq!(
            budget.charge(WorkUsage {
                bytes: 5,
                ..WorkUsage::default()
            }),
            Err(IndexError::ResourceLimited)
        );
        let record: serde_json::Value = serde_json::from_slice(&encode_discard(
            DiscardFailureStage::CatalogDiscard,
            &budget,
            &IndexError::ResourceLimited,
        )?)?;
        assert_eq!(
            record.get("event").and_then(serde_json::Value::as_str),
            Some("native_discard_refusal")
        );
        assert_eq!(
            record.get("stage").and_then(serde_json::Value::as_str),
            Some("catalog_discard")
        );
        assert_eq!(
            record.get("outcome").and_then(serde_json::Value::as_str),
            Some("Err(ResourceLimited)")
        );
        let diagnostic = record
            .get("budget_diagnostic")
            .and_then(serde_json::Value::as_str)
            .ok_or("discard budget diagnostic absent")?;
        assert!(diagnostic.contains("first_denial: \"bytes\""));
        assert!(diagnostic.contains("used: 0, attempted: 5, limit: 4"));
        assert!(record.get("operation_key").is_none());
        assert!(record.get("source_path").is_none());
        assert_eq!(
            record
                .as_object()
                .ok_or("discard diagnostic not an object")?
                .len(),
            4
        );
        let external: serde_json::Value = serde_json::from_slice(&encode_discard(
            DiscardFailureStage::PhysicalMaintenanceAdmission,
            &WorkBudget::with_request(
                WorkLimits {
                    reads: 10,
                    writes: 10,
                    bytes: 4,
                    mutations: 10,
                },
                RequestBudgetV1::for_duration(Duration::from_secs(30)),
            ),
            &IndexError::ResourceLimited,
        )?)?;
        assert_eq!(
            external.get("stage").and_then(serde_json::Value::as_str),
            Some("physical_maintenance_admission")
        );
        assert!(
            external
                .get("budget_diagnostic")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| value.contains("first_denial: \"none\""))
        );
        Ok(())
    }

    #[test]
    fn long_error_cannot_expand_outcome_and_success_has_no_record() {
        let long_error = Err(IndexError::Storage("x".repeat(100_000)));
        let outcome = outcome_name(&long_error);
        assert_eq!(outcome, Some("Err(Storage)"));
        assert!(outcome.is_some_and(|label| label.len() <= 256));
        assert_eq!(
            outcome_name(&Ok(SubmitOutcome::Unknown(key()))),
            Some("Unknown")
        );
        assert_eq!(
            outcome_name(&Ok(SubmitOutcome::Terminal(OperationStatus::Published(
                quanta_index_core::index::ViewRef {
                    workspace: key().workspace,
                    epoch: key().epoch,
                    sequence: 1,
                    digest: quanta_index_core::index::Digest([0; 32]),
                },
            )))),
            None
        );
    }
}
