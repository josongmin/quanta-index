//! Optional diagnostics for Native publication and query failures.
//!
//! This observes the existing result. It never changes the reply or retries an
//! operation. Successful publications and on-time renewals read no configuration.

use std::{
    cell::Cell,
    ffi::OsStr,
    io::Write as _,
    sync::atomic::{AtomicU16, AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use quanta_index_core::index::{
    IndexError, NativeReadSettlement, NativeReadSettlementBlocker, NativeReadSettlementStatus,
    OperationKey, OperationStatus, OwnedWorkGrant, OwnedWorkReport, OwnedWorkTerminal, Refusal,
    SubmitOutcome, UpdateKind, WorkBudget, WorkClaim, WorkIntent, ports::TerminalCustodyPage,
};
use quanta_index_core::{DiagnosticCaptureIdV1, DiagnosticTraceRefV1};
use quanta_index_kernel::QueryExecutionTimings;
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
static ORDINARY_SEARCH_FAILURES_SAMPLED: AtomicUsize = AtomicUsize::new(0);
static RETAINED_SEARCH_FAILURES_SAMPLED: AtomicUsize = AtomicUsize::new(0);
static RETAINED_SEARCH_TIMINGS_SAMPLED: AtomicUsize = AtomicUsize::new(0);
// Only the first sixteen fresh-client requests pay for a clock. Classify the
// completed observations into separate eight-record success and error caps.
static FIRST_RETAINED_SEARCH_TIMING_PERMITS: AtomicUsize = AtomicUsize::new(0);
static FIRST_RETAINED_SEARCH_SUCCESSES_SAMPLED: AtomicUsize = AtomicUsize::new(0);
static FIRST_RETAINED_SEARCH_ERRORS_SAMPLED: AtomicUsize = AtomicUsize::new(0);
static RETAINED_CONTROL_FAILURES_SAMPLED: AtomicUsize = AtomicUsize::new(0);
static READ_VIEW_FAILURES_SAMPLED: AtomicUsize = AtomicUsize::new(0);
static SETTLEMENT_BLOCKERS_SAMPLED: AtomicU16 = AtomicU16::new(0);
static TERMINAL_CUSTODY_POLLS: AtomicU16 = AtomicU16::new(0);

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReadFailureRoute {
    OrdinarySearch,
    RetainedSearch,
    RetainedControl,
    ReadView,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ReadFailureStage {
    Checkpoint,
    ReplyReservation,
    ControlAdmission,
    InteractiveAdmission,
    SessionCustody,
    QueryPreparation,
    QueryExecution,
    ResultConversion,
    ViewCustody,
}

#[derive(Clone, Copy)]
pub(super) enum SessionCustodyPart {
    RegistryLookup,
    EntryPre,
    ExitPost,
}

impl SessionCustodyPart {
    const fn index(self) -> usize {
        match self {
            Self::RegistryLookup => 0,
            Self::EntryPre => 1,
            Self::ExitPost => 2,
        }
    }
}

/// Optional timing for retained SearchText requests. The publisher reuses its
/// client request counter; offered queries start new counters. Later requests
/// retain failure-prefix timing: reserving those before execution would let
/// high-volume successes exhaust slots before a later failure occurs.
pub(super) struct RetainedSearchTimer {
    stage: Cell<ReadFailureStage>,
    entered: Cell<Option<Instant>>,
    elapsed: Cell<[Duration; 7]>,
    session_custody_part: Cell<SessionCustodyPart>,
    session_custody_elapsed: Cell<[Option<Duration>; 3]>,
    query_execution_timings: Cell<Option<QueryExecutionTimings>>,
    ticket_validation_timings: Cell<Option<quanta_index_catalog::TicketValidationTimings>>,
    first_request_permit: Option<usize>,
}

impl RetainedSearchTimer {
    pub(super) fn new(context: &quanta_index_ipc::DispatchContextV1) -> Self {
        let diagnostic = std::env::var_os(ENV).as_deref() == Some(OsStr::new("1"));
        let first_request = context.request_id.get() == 1;
        let first_request_permit = if diagnostic && first_request {
            prefix_sample_slot(&FIRST_RETAINED_SEARCH_TIMING_PERMITS, 16)
        } else {
            None
        };
        let enabled = diagnostic && (!first_request || first_request_permit.is_some());
        Self {
            stage: Cell::new(ReadFailureStage::SessionCustody),
            entered: Cell::new(enabled.then(Instant::now)),
            elapsed: Cell::new([Duration::ZERO; 7]),
            session_custody_part: Cell::new(SessionCustodyPart::RegistryLookup),
            session_custody_elapsed: Cell::new([None; 3]),
            query_execution_timings: Cell::new(None),
            ticket_validation_timings: Cell::new(None),
            first_request_permit,
        }
    }

    pub(super) fn stage(&self) -> ReadFailureStage {
        self.stage.get()
    }

    pub(super) fn is_enabled(&self) -> bool {
        self.entered.get().is_some()
    }

    pub(super) fn record_execution_timings(&self, timings: QueryExecutionTimings) {
        self.query_execution_timings.set(Some(timings));
    }

    pub(super) fn record_ticket_validation_timings(
        &self,
        timings: quanta_index_catalog::TicketValidationTimings,
    ) {
        self.ticket_validation_timings.set(Some(timings));
    }

    fn ticket_validation_detail_us(&self) -> Option<serde_json::Value> {
        let timings = self.ticket_validation_timings.get()?;
        Some(serde_json::json!({
            "phases": [
                "session_guard", "connection_acquisition", "begin",
                "authenticated_rows", "durable_clock", "commit",
            ],
            "elapsed_us": timings.elapsed.map(duration_us),
            "calls": timings.calls,
            "failed": timings.failed,
            "connection_busy_polls": timings.connection_busy_polls,
            "connection_retry_sleep_us": duration_us(timings.connection_retry_sleep),
        }))
    }

    pub(super) fn mark_session_custody(&self, part: SessionCustodyPart) {
        self.mark(ReadFailureStage::SessionCustody);
        self.session_custody_part.set(part);
    }

    pub(super) fn mark(&self, next: ReadFailureStage) {
        if let Some(entered) = self.entered.get() {
            let now = Instant::now();
            self.entered.set(Some(now));
            let (elapsed, detail) = self.elapsed_through(now.saturating_duration_since(entered));
            self.elapsed.set(elapsed);
            self.session_custody_elapsed.set(detail);
        }
        self.stage.set(next);
    }

    fn elapsed_through(&self, elapsed_here: Duration) -> ([Duration; 7], [Option<Duration>; 3]) {
        let mut elapsed = self.elapsed.get();
        let mut detail = self.session_custody_elapsed.get();
        let stage = self.stage.get();
        if let Some(bucket) = elapsed.get_mut(retained_stage_index(stage)) {
            *bucket = bucket.saturating_add(elapsed_here);
        }
        if matches!(stage, ReadFailureStage::SessionCustody)
            && let Some(bucket) = detail.get_mut(self.session_custody_part.get().index())
        {
            *bucket = Some(bucket.unwrap_or_default().saturating_add(elapsed_here));
        }
        (elapsed, detail)
    }

    pub(super) fn record_result(
        &self,
        context: &quanta_index_ipc::DispatchContextV1,
        budget: &WorkBudget,
        error: Option<&IndexError>,
    ) {
        if context.request_id.get() == 1 {
            self.record_first_result(context, budget, error);
        } else if let Some(error) = error {
            self.record_failure(context, budget, error);
        }
    }

    fn record_first_result(
        &self,
        context: &quanta_index_ipc::DispatchContextV1,
        budget: &WorkBudget,
        error: Option<&IndexError>,
    ) {
        if self.first_request_permit.is_none() {
            return;
        }
        let Some(entered) = self.entered.get() else {
            return;
        };
        let Some(sample_ordinal) = first_retained_result_sample_slot(
            error.is_some(),
            &FIRST_RETAINED_SEARCH_SUCCESSES_SAMPLED,
            &FIRST_RETAINED_SEARCH_ERRORS_SAMPLED,
        ) else {
            return;
        };
        let (elapsed_us, session_custody_us) = self.finished_elapsed_us(entered);
        let elapsed = retained_elapsed_json(elapsed_us);
        let session_custody_detail_us =
            Self::session_custody_detail_us(elapsed_us, session_custody_us);
        let query_execution_detail_us = self.execution_detail_us(elapsed_us);
        let event = serde_json::json!({
            "event": "native_retained_search_timing",
            "sample_population": "first_request",
            "result": if error.is_some() { "error" } else { "success" },
            "request_id": context.request_id.get(),
            "connection_id": context.connection_id,
            "stage": self.stage(),
            "outcome": error.map_or("success", error_outcome),
            "first_local_denial": format!("{:?}", budget.first_denial()),
            "elapsed_us": elapsed,
            "session_custody_detail_us": session_custody_detail_us,
            "query_execution_detail_us": query_execution_detail_us,
            "ticket_validation_detail_us": self.ticket_validation_detail_us(),
            "sample_ordinal": sample_ordinal,
            "sample_limit": OWNED_FAILURE_SAMPLE_LIMIT,
        });
        write_diagnostic_event(&event);
    }

    fn record_failure(
        &self,
        context: &quanta_index_ipc::DispatchContextV1,
        budget: &WorkBudget,
        error: &IndexError,
    ) {
        let Some(entered) = self.entered.get() else {
            return;
        };
        let Some(sample_ordinal) = owned_failure_sample_slot(&RETAINED_SEARCH_TIMINGS_SAMPLED)
        else {
            return;
        };
        let (elapsed_us, session_custody_us) = self.finished_elapsed_us(entered);
        let elapsed = retained_elapsed_json(elapsed_us);
        let session_custody_detail_us =
            Self::session_custody_detail_us(elapsed_us, session_custody_us);
        let query_execution_detail_us = self.execution_detail_us(elapsed_us);
        let event = serde_json::json!({
            "event": "native_retained_search_timing",
            "request_id": context.request_id.get(),
            "connection_id": context.connection_id,
            "stage": self.stage(),
            "outcome": error_outcome(error),
            "first_local_denial": format!("{:?}", budget.first_denial()),
            "elapsed_us": elapsed,
            "session_custody_detail_us": session_custody_detail_us,
            "query_execution_detail_us": query_execution_detail_us,
            "ticket_validation_detail_us": self.ticket_validation_detail_us(),
            "sample_ordinal": sample_ordinal,
            "sample_limit": OWNED_FAILURE_SAMPLE_LIMIT,
        });
        write_diagnostic_event(&event);
    }

    fn finished_elapsed_us(&self, entered: Instant) -> ([u64; 7], [Option<u64>; 3]) {
        let (elapsed, detail) =
            self.elapsed_through(Instant::now().saturating_duration_since(entered));
        (
            elapsed.map(duration_us),
            detail.map(|part| part.map(duration_us)),
        )
    }

    fn session_custody_detail_us(
        elapsed_us: [u64; 7],
        detail_us: [Option<u64>; 3],
    ) -> Option<serde_json::Value> {
        if detail_us.iter().all(Option::is_none) {
            return None;
        }
        let [registry_lookup, entry_pre, exit_post] = detail_us;
        let measured = registry_lookup
            .unwrap_or(0)
            .saturating_add(entry_pre.unwrap_or(0))
            .saturating_add(exit_post.unwrap_or(0));
        let total = *elapsed_us.get(retained_stage_index(ReadFailureStage::SessionCustody))?;
        Some(serde_json::json!({
            "registry_lookup": registry_lookup,
            "entry_pre": entry_pre,
            "exit_post": exit_post,
            "other": total.saturating_sub(measured),
        }))
    }

    fn execution_detail_us(&self, elapsed_us: [u64; 7]) -> Option<serde_json::Value> {
        let timings = self.query_execution_timings.get()?;
        let before = timings.before_ticket_validation.map(duration_us);
        let reader = timings.reader_query.map(duration_us);
        let after = timings.after_ticket_validation.map(duration_us);
        let measured = before
            .unwrap_or(0)
            .saturating_add(reader.unwrap_or(0))
            .saturating_add(after.unwrap_or(0));
        let total = elapsed_us
            .get(retained_stage_index(ReadFailureStage::QueryExecution))
            .copied()?;
        Some(serde_json::json!({
            "before_ticket_validation": before,
            "reader_query": reader,
            "after_ticket_validation": after,
            "other": total.saturating_sub(measured),
        }))
    }
}

fn duration_us(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_mul(1_000_000)
        .saturating_add(u64::from(duration.subsec_micros()))
}

fn write_diagnostic_event(event: &serde_json::Value) {
    let Ok(mut line) = serde_json::to_vec(event) else {
        return;
    };
    line.push(b'\n');
    let _written = std::io::stderr().lock().write_all(&line);
}

fn retained_elapsed_json(elapsed: [u64; 7]) -> serde_json::Value {
    let [
        control_admission,
        session_custody,
        query_preparation,
        interactive_admission,
        query_execution,
        result_conversion,
        checkpoint,
    ] = elapsed;
    serde_json::json!({
        "control_admission": control_admission,
        "session_custody": session_custody,
        "query_preparation": query_preparation,
        "interactive_admission": interactive_admission,
        "query_execution": query_execution,
        "result_conversion": result_conversion,
        "checkpoint": checkpoint,
    })
}

fn retained_stage_index(stage: ReadFailureStage) -> usize {
    match stage {
        ReadFailureStage::ControlAdmission => 0,
        ReadFailureStage::SessionCustody => 1,
        ReadFailureStage::QueryPreparation => 2,
        ReadFailureStage::InteractiveAdmission => 3,
        ReadFailureStage::QueryExecution => 4,
        ReadFailureStage::ResultConversion => 5,
        ReadFailureStage::Checkpoint
        | ReadFailureStage::ReplyReservation
        | ReadFailureStage::ViewCustody => 6,
    }
}

pub(super) fn record_read_error_if_enabled(
    context: &quanta_index_ipc::DispatchContextV1,
    route: ReadFailureRoute,
    stage: ReadFailureStage,
    budget: &WorkBudget,
    error: &IndexError,
) {
    if std::env::var_os(ENV).as_deref() != Some(OsStr::new("1")) {
        return;
    }
    let counter = match route {
        ReadFailureRoute::OrdinarySearch => &ORDINARY_SEARCH_FAILURES_SAMPLED,
        ReadFailureRoute::RetainedSearch => &RETAINED_SEARCH_FAILURES_SAMPLED,
        ReadFailureRoute::RetainedControl => &RETAINED_CONTROL_FAILURES_SAMPLED,
        ReadFailureRoute::ReadView => &READ_VIEW_FAILURES_SAMPLED,
    };
    let Some(sample_ordinal) = owned_failure_sample_slot(counter) else {
        return;
    };
    let event = serde_json::json!({
        "event": "native_read_error_observed",
        "request_id": context.request_id.get(),
        "connection_id": context.connection_id,
        "route": route,
        "stage": stage,
        "outcome": error_outcome(error),
        "first_local_denial": format!("{:?}", budget.first_denial()),
        "sample_ordinal": sample_ordinal,
        "sample_limit": OWNED_FAILURE_SAMPLE_LIMIT,
    });
    let Ok(mut line) = serde_json::to_vec(&event) else {
        return;
    };
    line.push(b'\n');
    let _written = std::io::stderr().lock().write_all(&line);
}

/// Keep one observation of each blocker per process, with bounded progress
/// samples for terminal custody. The returned SDK state and Catalog snapshot
/// that decided it remain unchanged.
pub(super) fn record_settlement_if_enabled(
    context: &quanta_index_ipc::DispatchContextV1,
    observation: &NativeReadSettlement,
) {
    let slot = match observation.status {
        NativeReadSettlementStatus::Pending(blocker) => match blocker {
            NativeReadSettlementBlocker::TargetPin => 0,
            NativeReadSettlementBlocker::ExactFloor => 1,
            NativeReadSettlementBlocker::WorkBelowFloor => 2,
            NativeReadSettlementBlocker::TerminalWork => 3,
            NativeReadSettlementBlocker::TerminalCustody { .. } => 4,
            NativeReadSettlementBlocker::UnretiredView => 5,
            NativeReadSettlementBlocker::ReclaimQueue => 6,
            NativeReadSettlementBlocker::ObjectLifecycle => 7,
            NativeReadSettlementBlocker::UnreferencedObject => 8,
        },
        NativeReadSettlementStatus::Incomplete => 15,
        NativeReadSettlementStatus::SettledAtSnapshot => return,
    };
    if std::env::var_os(ENV).as_deref() != Some(OsStr::new("1")) {
        return;
    }
    let custody_sample =
        if let NativeReadSettlementStatus::Pending(NativeReadSettlementBlocker::TerminalCustody {
            oldest_ready_sequence,
        }) = observation.status
        {
            let Ok(previous) = TERMINAL_CUSTODY_POLLS.fetch_update(
                Ordering::Relaxed,
                Ordering::Relaxed,
                |count| count.checked_add(1).filter(|next| *next <= 60),
            ) else {
                return;
            };
            let ordinal = previous.saturating_add(1);
            if !matches!(ordinal, 1 | 16 | 32 | 48 | 60) {
                return;
            }
            Some((ordinal, oldest_ready_sequence))
        } else {
            let Some(mask) = 1_u16.checked_shl(slot) else {
                return;
            };
            if SETTLEMENT_BLOCKERS_SAMPLED.fetch_or(mask, Ordering::Relaxed) & mask != 0 {
                return;
            }
            None
        };
    let mut event = serde_json::json!({
        "event": "native_read_settlement_blocked",
        "request_id": context.request_id.get(),
        "connection_id": context.connection_id,
        "status": format!("{:?}", observation.status),
        "target_sequence": observation.scope.target.sequence,
        "head_sequence": observation.head.sequence,
        "exact_floor": observation.exact_floor,
        "has_remaining_target_pins": observation.has_remaining_target_pins,
    });
    if let Some((ordinal, oldest_ready_sequence)) = custody_sample
        && let Some(fields) = event.as_object_mut()
    {
        let _prior = fields.insert(
            "status".into(),
            serde_json::json!("Pending(TerminalCustody)"),
        );
        let _prior = fields.insert("terminal_custody_poll".into(), serde_json::json!(ordinal));
        let _prior = fields.insert(
            "oldest_ready_sequence".into(),
            serde_json::json!(oldest_ready_sequence),
        );
    }
    let Ok(mut line) = serde_json::to_vec(&event) else {
        return;
    };
    line.push(b'\n');
    let _written = std::io::stderr().lock().write_all(&line);
}

/// Cumulative, bounded maintenance samples. No Catalog read or write is
/// added, and the pump never renews a phase budget to collect a sample.
pub(super) struct RetentionProgress {
    next_sample: Instant,
    remaining_samples: u8,
    counts: RetentionProgressCounts,
}

#[derive(Default, Serialize)]
struct RetentionProgressCounts {
    admission_refusals: u64,
    floor_rounds: u64,
    floor_refusals: u64,
    custody_probe_refusals: u64,
    custody_release_refusals: u64,
    custody_pages: u64,
    custody_rows: u64,
    oldest_ready_sequence: Option<u64>,
    oldest_operation_sequence: Option<u64>,
    oldest_released_rows: u64,
    oldest_has_more: Option<bool>,
}

impl RetentionProgress {
    const INTERVAL: Duration = Duration::from_secs(45);

    pub(super) fn new_if_enabled() -> Option<Self> {
        if std::env::var_os(ENV).as_deref() != Some(OsStr::new("1")) {
            return None;
        }
        Some(Self {
            next_sample: Instant::now().checked_add(Self::INTERVAL)?,
            remaining_samples: 16,
            counts: RetentionProgressCounts::default(),
        })
    }

    pub(super) fn admission_refused(&mut self) {
        self.counts.admission_refusals = self.counts.admission_refusals.saturating_add(1);
        self.emit_due();
    }

    pub(super) fn floor_result(&mut self, refused: bool) {
        self.counts.floor_rounds = self.counts.floor_rounds.saturating_add(1);
        if refused {
            self.counts.floor_refusals = self.counts.floor_refusals.saturating_add(1);
        }
    }

    pub(super) fn custody_probe_refused(&mut self) {
        self.counts.custody_probe_refusals = self.counts.custody_probe_refusals.saturating_add(1);
    }

    pub(super) fn oldest_candidate(&mut self, ready: u64, operation: u64) {
        if self.counts.oldest_ready_sequence != Some(ready) {
            self.counts.oldest_ready_sequence = Some(ready);
            self.counts.oldest_operation_sequence = Some(operation);
            self.counts.oldest_released_rows = 0;
            self.counts.oldest_has_more = None;
        }
    }

    pub(super) fn custody_release(&mut self, ready: u64, page: Option<&TerminalCustodyPage>) {
        let Some(page) = page else {
            self.counts.custody_release_refusals =
                self.counts.custody_release_refusals.saturating_add(1);
            return;
        };
        let released = u64::from(page.released);
        self.counts.custody_pages = self.counts.custody_pages.saturating_add(1);
        self.counts.custody_rows = self.counts.custody_rows.saturating_add(released);
        if self.counts.oldest_ready_sequence == Some(ready) {
            self.counts.oldest_released_rows =
                self.counts.oldest_released_rows.saturating_add(released);
            self.counts.oldest_has_more = Some(page.has_more);
        }
    }

    pub(super) fn emit_due(&mut self) {
        if self.remaining_samples == 0 {
            return;
        }
        let now = Instant::now();
        if now < self.next_sample {
            return;
        }
        let Some(next_sample) = now.checked_add(Self::INTERVAL) else {
            self.remaining_samples = 0;
            return;
        };
        self.next_sample = next_sample;
        self.remaining_samples = self.remaining_samples.saturating_sub(1);
        let event = serde_json::json!({
            "event": "native_retention_progress",
            "cumulative": &self.counts,
        });
        let Ok(mut line) = serde_json::to_vec(&event) else {
            return;
        };
        line.push(b'\n');
        let _written = std::io::stderr().lock().write_all(&line);
    }
}

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
    prefix_sample_slot(counter, OWNED_FAILURE_SAMPLE_LIMIT)
}

fn prefix_sample_slot(counter: &AtomicUsize, limit: usize) -> Option<usize> {
    match counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |previous| {
        previous.checked_add(1).filter(|next| *next <= limit)
    }) {
        Ok(previous) => previous.checked_add(1),
        Err(_saturated) => None,
    }
}

fn first_retained_result_sample_slot(
    is_error: bool,
    successes: &AtomicUsize,
    errors: &AtomicUsize,
) -> Option<usize> {
    owned_failure_sample_slot(if is_error { errors } else { successes })
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

    #[test]
    fn retained_search_timer_accumulates_reentered_stages() -> Result<(), IndexError> {
        let prior = Duration::from_millis(10);
        let entered = Instant::now()
            .checked_sub(prior)
            .ok_or(IndexError::ResourceLimited)?;
        let timer = RetainedSearchTimer {
            stage: Cell::new(ReadFailureStage::SessionCustody),
            entered: Cell::new(Some(entered)),
            elapsed: Cell::new([Duration::ZERO; 7]),
            session_custody_part: Cell::new(SessionCustodyPart::RegistryLookup),
            session_custody_elapsed: Cell::new([None; 3]),
            query_execution_timings: Cell::new(None),
            ticket_validation_timings: Cell::new(None),
            first_request_permit: None,
        };
        timer.mark(ReadFailureStage::ControlAdmission);
        assert!(matches!(timer.stage(), ReadFailureStage::ControlAdmission));
        let first = timer.elapsed.get();
        let slot = retained_stage_index(ReadFailureStage::SessionCustody);
        assert!(first.get(slot).is_some_and(|elapsed| *elapsed >= prior));
        timer.mark(ReadFailureStage::SessionCustody);
        timer.entered.set(Some(entered));
        timer.mark(ReadFailureStage::QueryPreparation);
        let second = timer.elapsed.get();
        assert!(
            second
                .get(slot)
                .is_some_and(|elapsed| *elapsed >= prior.saturating_mul(2))
        );
        Ok(())
    }

    #[test]
    fn retained_execution_detail_preserves_aggregate_and_partial_steps() {
        let timer = RetainedSearchTimer {
            stage: Cell::new(ReadFailureStage::QueryExecution),
            entered: Cell::new(None),
            elapsed: Cell::new([Duration::ZERO; 7]),
            session_custody_part: Cell::new(SessionCustodyPart::RegistryLookup),
            session_custody_elapsed: Cell::new([None; 3]),
            query_execution_timings: Cell::new(None),
            ticket_validation_timings: Cell::new(None),
            first_request_permit: None,
        };
        let mut elapsed = [0_u64; 7];
        if let Some(slot) = elapsed.get_mut(retained_stage_index(ReadFailureStage::QueryExecution))
        {
            *slot = 100;
        }
        assert!(timer.execution_detail_us(elapsed).is_none());
        timer.record_execution_timings(QueryExecutionTimings {
            before_ticket_validation: Some(Duration::from_micros(20)),
            reader_query: Some(Duration::from_micros(30)),
            after_ticket_validation: None,
        });
        let detail = timer.execution_detail_us(elapsed);
        assert_eq!(
            detail
                .as_ref()
                .and_then(|value| value.get("before_ticket_validation")),
            Some(&serde_json::json!(20))
        );
        assert_eq!(
            detail.as_ref().and_then(|value| value.get("reader_query")),
            Some(&serde_json::json!(30))
        );
        assert_eq!(
            detail
                .as_ref()
                .and_then(|value| value.get("after_ticket_validation")),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(
            detail.as_ref().and_then(|value| value.get("other")),
            Some(&serde_json::json!(50))
        );
        assert_eq!(
            elapsed.get(retained_stage_index(ReadFailureStage::QueryExecution)),
            Some(&100)
        );
        assert!(timer.ticket_validation_detail_us().is_none());
        let mut ticket = quanta_index_catalog::TicketValidationTimings::default();
        if let (Some(elapsed), Some(calls), Some(failed)) = (
            ticket.elapsed.get_mut(1),
            ticket.calls.get_mut(1),
            ticket.failed.get_mut(1),
        ) {
            *elapsed = Duration::from_micros(37);
            *calls = 2;
            *failed = 1;
        }
        timer.record_ticket_validation_timings(ticket);
        assert_eq!(
            timer.ticket_validation_detail_us(),
            Some(serde_json::json!({
                "phases": [
                    "session_guard", "connection_acquisition", "begin",
                    "authenticated_rows", "durable_clock", "commit",
                ],
                "elapsed_us": [0, 37, 0, 0, 0, 0],
                "calls": [0, 2, 0, 0, 0, 0],
                "failed": [0, 1, 0, 0, 0, 0],
                "connection_busy_polls": 0,
                "connection_retry_sleep_us": 0,
            }))
        );
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "the serialized fields are independent test oracles"
    )]
    fn retained_session_custody_detail_tracks_existing_transitions() -> Result<(), &'static str> {
        let selected = || RetainedSearchTimer {
            stage: Cell::new(ReadFailureStage::SessionCustody),
            entered: Cell::new(Some(Instant::now())),
            elapsed: Cell::new([Duration::ZERO; 7]),
            session_custody_part: Cell::new(SessionCustodyPart::RegistryLookup),
            session_custody_elapsed: Cell::new([None; 3]),
            query_execution_timings: Cell::new(None),
            ticket_validation_timings: Cell::new(None),
            first_request_permit: None,
        };

        let success = selected();
        success.mark(ReadFailureStage::ControlAdmission);
        success.mark_session_custody(SessionCustodyPart::EntryPre);
        success.mark(ReadFailureStage::QueryPreparation);
        success.mark(ReadFailureStage::InteractiveAdmission);
        success.mark(ReadFailureStage::QueryExecution);
        success.mark(ReadFailureStage::Checkpoint);
        success.mark_session_custody(SessionCustodyPart::ExitPost);
        let entered = success.entered.get().ok_or("selected timer has no clock")?;
        let (elapsed, parts) = success.finished_elapsed_us(entered);
        assert!(parts.iter().all(Option::is_some));
        let detail = RetainedSearchTimer::session_custody_detail_us(elapsed, parts)
            .ok_or("missing selected session detail")?;
        let other = detail
            .get("other")
            .and_then(serde_json::Value::as_u64)
            .ok_or("missing session rounding remainder")?;
        let measured: u64 = parts.iter().flatten().copied().sum();
        assert_eq!(
            Some(measured.saturating_add(other)),
            elapsed
                .get(retained_stage_index(ReadFailureStage::SessionCustody))
                .copied()
        );
        assert_eq!(
            serde_json::to_value(success.stage()).ok(),
            Some(serde_json::json!("session_custody"))
        );

        let failed = selected();
        failed.mark(ReadFailureStage::ControlAdmission);
        failed.mark_session_custody(SessionCustodyPart::EntryPre);
        failed.mark(ReadFailureStage::QueryPreparation);
        failed.mark(ReadFailureStage::QueryExecution);
        let entered = failed.entered.get().ok_or("selected timer has no clock")?;
        let (_elapsed, [lookup, entry, exit]) = failed.finished_elapsed_us(entered);
        assert!(lookup.is_some() && entry.is_some());
        assert_eq!(exit, None);
        assert_eq!(
            serde_json::to_value(failed.stage()).ok(),
            Some(serde_json::json!("query_execution"))
        );
        Ok(())
    }

    #[test]
    fn first_retained_request_timing_has_bounded_distinct_results() {
        let permits = AtomicUsize::new(0);
        let successes = AtomicUsize::new(0);
        let errors = AtomicUsize::new(0);
        for expected in 1..=16 {
            assert_eq!(prefix_sample_slot(&permits, 16), Some(expected));
        }
        assert_eq!(prefix_sample_slot(&permits, 16), None);
        for expected in 1..=OWNED_FAILURE_SAMPLE_LIMIT {
            assert_eq!(
                first_retained_result_sample_slot(false, &successes, &errors),
                Some(expected)
            );
            assert_eq!(
                first_retained_result_sample_slot(true, &successes, &errors),
                Some(expected)
            );
        }
        assert_eq!(
            first_retained_result_sample_slot(false, &successes, &errors),
            None
        );
        assert_eq!(
            first_retained_result_sample_slot(true, &successes, &errors),
            None
        );
    }

    #[test]
    fn retention_progress_counts_committed_pages_and_oldest_separately() {
        let mut progress = RetentionProgress {
            next_sample: Instant::now(),
            remaining_samples: 0,
            counts: RetentionProgressCounts::default(),
        };
        progress.admission_refused();
        progress.floor_result(true);
        progress.floor_result(false);
        progress.custody_probe_refused();
        progress.oldest_candidate(41, 2);
        let page = TerminalCustodyPage {
            released: 32,
            has_more: true,
        };
        progress.custody_release(41, None);
        progress.custody_release(41, Some(&page));
        progress.custody_release(42, Some(&page));
        progress.oldest_candidate(41, 2);
        assert_eq!(progress.counts.admission_refusals, 1);
        assert_eq!(progress.counts.floor_rounds, 2);
        assert_eq!(progress.counts.floor_refusals, 1);
        assert_eq!(progress.counts.custody_probe_refusals, 1);
        assert_eq!(progress.counts.custody_release_refusals, 1);
        assert_eq!(progress.counts.custody_pages, 2);
        assert_eq!(progress.counts.custody_rows, 64);
        assert_eq!(progress.counts.oldest_released_rows, 32);
        assert_eq!(progress.counts.oldest_has_more, Some(true));
        progress.oldest_candidate(42, 3);
        assert_eq!(progress.counts.oldest_released_rows, 0);
        assert_eq!(progress.counts.oldest_has_more, None);
    }

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
