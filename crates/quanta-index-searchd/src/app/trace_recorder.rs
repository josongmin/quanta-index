//! Bounded process-local causal timeline recorder.
//!
//! Producers only perform a bounded `try_send`. A single writer assigns
//! `output_seq`, serializes JSONL, and owns the final footer. A successful
//! `close` proves all producer leases have ended and the writer has joined.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use quanta_index_core::diagnostic_trace::{
    DiagnosticAnnIndexKindV1, DiagnosticAnnRegionPhaseV1, DiagnosticCaptureIdV1, DiagnosticClockV1,
    DiagnosticContextV1, DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticEventSinkV1,
    DiagnosticRootKindV1, DiagnosticTerminalV1,
};
use serde_json::{Map, Value, json};

const FLUSH_INTERVAL: Duration = Duration::from_millis(100);

/// Hard resource limits for one JSONL capture.
#[derive(Clone, Copy, Debug)]
pub struct DiagnosticCaptureLimitsV1 {
    pub queue_records: usize,
    pub max_record_bytes: usize,
    pub max_output_bytes: u64,
}

impl Default for DiagnosticCaptureLimitsV1 {
    fn default() -> Self {
        Self {
            queue_records: 4096,
            max_record_bytes: 4096,
            max_output_bytes: 67_108_864,
        }
    }
}

impl DiagnosticCaptureLimitsV1 {
    pub const MAX_OUTPUT_BYTES: u64 = 1_073_741_824;

    pub fn with_max_output_bytes(mut self, max_output_bytes: u64) -> io::Result<Self> {
        self.max_output_bytes = max_output_bytes;
        self.validate()
    }

    fn validate(self) -> io::Result<Self> {
        if self.queue_records == 0
            || self.queue_records > 1_048_576
            || self.max_record_bytes < 512
            || self.max_record_bytes > 1_048_576
            || self.max_output_bytes > Self::MAX_OUTPUT_BYTES
            || self.max_output_bytes
                < match u64::try_from(self.max_record_bytes) {
                    Ok(record_bytes) => record_bytes.saturating_add(2048),
                    Err(_overflow) => u64::MAX,
                }
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid diagnostic capture limits",
            ));
        }
        Ok(self)
    }
}

#[derive(Debug)]
struct MonotonicClock(Instant);

impl DiagnosticClockV1 for MonotonicClock {
    fn now_ns(&self) -> u64 {
        let elapsed = self.0.elapsed();
        elapsed
            .as_secs()
            .saturating_mul(1_000_000_000)
            .saturating_add(u64::from(elapsed.subsec_nanos()))
    }
}

#[derive(Debug, Default)]
struct SinkCounts {
    attempted: AtomicU64,
    accepted: AtomicU64,
    dropped: AtomicU64,
    overflow: AtomicBool,
    health_invalid: AtomicBool,
}

fn increment(counter: &AtomicU64, overflow: &AtomicBool) {
    let result = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1));
    if result.is_err() {
        overflow.store(true, Ordering::Release);
    }
}

#[derive(Debug)]
struct QueueSink {
    sender: SyncSender<QueueItem>,
    counts: Arc<SinkCounts>,
    closed: AtomicBool,
}

impl DiagnosticEventSinkV1 for QueueSink {
    fn try_emit(&self, event: DiagnosticEmitV1) -> bool {
        if matches!(event.kind, DiagnosticEventKindV1::Health { .. }) {
            self.counts.health_invalid.store(true, Ordering::Release);
        }
        increment(&self.counts.attempted, &self.counts.overflow);
        if self.closed.load(Ordering::Acquire) {
            increment(&self.counts.dropped, &self.counts.overflow);
            return false;
        }
        match self.sender.try_send(QueueItem::Event(event)) {
            Ok(()) => {
                increment(&self.counts.accepted, &self.counts.overflow);
                true
            }
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                increment(&self.counts.dropped, &self.counts.overflow);
                false
            }
        }
    }
}

#[derive(Debug)]
enum QueueItem {
    Event(DiagnosticEmitV1),
    Seal,
}

/// One counted producer lease. Context clones must remain within this lease.
#[derive(Debug)]
pub struct DiagnosticCaptureLeaseV1 {
    context: DiagnosticContextV1,
    leases: Arc<AtomicUsize>,
}

impl DiagnosticCaptureLeaseV1 {
    #[must_use]
    pub fn context(&self) -> &DiagnosticContextV1 {
        &self.context
    }

    /// Derive a counted lease whose spans are children of this capture span.
    #[must_use]
    pub fn child_of(&self, span: &quanta_index_core::DiagnosticSpanGuardV1) -> Option<Self> {
        if !self.context.owns_span(span) {
            return None;
        }
        if self
            .leases
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .is_err()
        {
            self.context.health(
                quanta_index_core::diagnostic_trace::DiagnosticHealthCodeV1::CounterOverflow,
            );
            return None;
        }
        Some(Self {
            context: self.context.child_context(span),
            leases: Arc::clone(&self.leases),
        })
    }
}

impl Clone for DiagnosticCaptureLeaseV1 {
    fn clone(&self) -> Self {
        let added = self
            .leases
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1));
        assert!(added.is_ok(), "diagnostic lease count exhausted");
        Self {
            context: self.context.clone(),
            leases: Arc::clone(&self.leases),
        }
    }
}

impl Drop for DiagnosticCaptureLeaseV1 {
    fn drop(&mut self) {
        let _previous = self.leases.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The only result that authorizes consuming the finalized output file.
#[derive(Clone, Debug)]
pub struct DiagnosticCaptureClosedV1 {
    pub output: PathBuf,
    pub capture_id: DiagnosticCaptureIdV1,
    pub status: DiagnosticCaptureStatusV1,
    pub events_attempted: u64,
    pub events_accepted: u64,
    pub events_dropped: u64,
    pub events_written: u64,
    pub events_unwritten: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticCaptureStatusV1 {
    Complete,
    Partial,
    Invalid,
}

/// Preserve product flow while marking a failed diagnostic span as invalid.
pub(crate) fn trace_span_or_health(
    context: &DiagnosticContextV1,
    result: Result<
        quanta_index_core::DiagnosticSpanGuardV1,
        quanta_index_core::DiagnosticTraceErrorV1,
    >,
) -> Option<quanta_index_core::DiagnosticSpanGuardV1> {
    match result {
        Ok(span) => Some(span),
        Err(error) => {
            use quanta_index_core::diagnostic_trace::DiagnosticHealthCodeV1;
            let code = match error {
                quanta_index_core::DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                    DiagnosticHealthCodeV1::CounterOverflow
                }
                quanta_index_core::DiagnosticTraceErrorV1::ClockRegressed => {
                    DiagnosticHealthCodeV1::ClockRegressed
                }
                quanta_index_core::DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                    DiagnosticHealthCodeV1::ForeignCaptureSpan
                }
                quanta_index_core::DiagnosticTraceErrorV1::MissingParentSpan => {
                    DiagnosticHealthCodeV1::MissingParentSpan
                }
            };
            context.health(code);
            None
        }
    }
}

#[derive(Debug)]
pub enum DiagnosticCaptureCloseErrorV1 {
    PendingLeases(usize),
    PendingContexts(usize),
    WriterTimeout,
    Writer(io::Error),
}

impl std::fmt::Display for DiagnosticCaptureCloseErrorV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PendingLeases(count) => write!(
                formatter,
                "diagnostic capture has {count} active producer leases"
            ),
            Self::PendingContexts(count) => {
                write!(formatter, "diagnostic capture has {count} escaped contexts")
            }
            Self::WriterTimeout => {
                formatter.write_str("diagnostic writer did not close within deadline")
            }
            Self::Writer(error) => write!(formatter, "diagnostic writer failed: {error}"),
        }
    }
}

impl std::error::Error for DiagnosticCaptureCloseErrorV1 {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Writer(error) => Some(error),
            Self::PendingLeases(_) | Self::PendingContexts(_) | Self::WriterTimeout => None,
        }
    }
}

/// Owns one writer and its capture context. `close` can be retried after a
/// pending lease or a slow writer; it never publishes a path before join.
pub struct DiagnosticCaptureV1 {
    output: PathBuf,
    capture_id: DiagnosticCaptureIdV1,
    context: DiagnosticContextV1,
    sink: Arc<QueueSink>,
    sender: SyncSender<QueueItem>,
    leases: Arc<AtomicUsize>,
    writer: Option<JoinHandle<io::Result<WriterSummary>>>,
    sealing: bool,
}

impl DiagnosticCaptureV1 {
    pub fn start(output: PathBuf, limits: DiagnosticCaptureLimitsV1) -> io::Result<Self> {
        Self::start_with_clock_and_writer_hook(
            output,
            limits,
            Arc::new(MonotonicClock(Instant::now())),
            None,
        )
    }

    fn start_with_clock_and_writer_hook(
        output: PathBuf,
        limits: DiagnosticCaptureLimitsV1,
        clock: Arc<dyn DiagnosticClockV1>,
        writer_hook: Option<Box<dyn FnOnce() + Send>>,
    ) -> io::Result<Self> {
        let limits = limits.validate()?;
        let mut id = [0_u8; 16];
        File::open("/dev/urandom")?.read_exact(&mut id)?;
        let capture_id = DiagnosticCaptureIdV1(id);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)?;
        let (sender, receiver) = mpsc::sync_channel(limits.queue_records);
        let counts = Arc::new(SinkCounts::default());
        let leases = Arc::new(AtomicUsize::new(0));
        let sink = Arc::new(QueueSink {
            sender: sender.clone(),
            counts: Arc::clone(&counts),
            closed: AtomicBool::new(false),
        });
        let context = DiagnosticContextV1::new(capture_id, Arc::clone(&clock), sink.clone());
        let writer_leases = Arc::clone(&leases);
        let writer = thread::Builder::new()
            .name("diagnostic-timeline-writer".into())
            .spawn(move || {
                if let Some(hook) = writer_hook {
                    hook();
                }
                write_capture(
                    file,
                    &receiver,
                    capture_id,
                    limits,
                    counts.as_ref(),
                    writer_leases.as_ref(),
                    clock.as_ref(),
                )
            });
        let writer = match writer {
            Ok(writer) => writer,
            Err(error) => {
                let _cleanup = std::fs::remove_file(&output);
                return Err(error);
            }
        };
        Ok(Self {
            output,
            capture_id,
            context,
            sink,
            sender,
            leases,
            writer: Some(writer),
            sealing: false,
        })
    }

    #[must_use]
    pub fn lease(&self) -> Option<DiagnosticCaptureLeaseV1> {
        if self.sealing {
            return None;
        }
        if self
            .leases
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .is_err()
        {
            self.context.health(
                quanta_index_core::diagnostic_trace::DiagnosticHealthCodeV1::CounterOverflow,
            );
            return None;
        }
        Some(DiagnosticCaptureLeaseV1 {
            context: self.context.clone(),
            leases: Arc::clone(&self.leases),
        })
    }

    #[must_use]
    pub fn capture_id(&self) -> DiagnosticCaptureIdV1 {
        self.capture_id
    }

    /// Wait at most `timeout` for queue admission and writer completion.
    /// A timeout leaves this capture retryable, with no complete result.
    pub fn close(
        &mut self,
        timeout: Duration,
    ) -> Result<DiagnosticCaptureClosedV1, DiagnosticCaptureCloseErrorV1> {
        let leases = self.leases.load(Ordering::Acquire);
        if leases != 0 {
            return Err(DiagnosticCaptureCloseErrorV1::PendingLeases(leases));
        }
        // Core owns the inner Arc. A cloned context without a lease is an
        // escaped producer and would invalidate the close barrier.
        let contexts = self.context.strong_count();
        if contexts != 1 {
            return Err(DiagnosticCaptureCloseErrorV1::PendingContexts(
                contexts.saturating_sub(1),
            ));
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(Instant::now);
        if !self.sealing {
            loop {
                match self.sender.try_send(QueueItem::Seal) {
                    Ok(()) => {
                        self.sealing = true;
                        self.sink.closed.store(true, Ordering::Release);
                        break;
                    }
                    Err(TrySendError::Full(_)) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(TrySendError::Full(_)) => {
                        return Err(DiagnosticCaptureCloseErrorV1::WriterTimeout);
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        self.sealing = true;
                        break;
                    }
                }
            }
        }
        let Some(handle) = self.writer.as_ref() else {
            return Err(DiagnosticCaptureCloseErrorV1::Writer(io::Error::other(
                "capture already closed",
            )));
        };
        while !handle.is_finished() {
            if Instant::now() >= deadline {
                return Err(DiagnosticCaptureCloseErrorV1::WriterTimeout);
            }
            thread::sleep(Duration::from_millis(1));
        }
        let Some(handle) = self.writer.take() else {
            return Err(DiagnosticCaptureCloseErrorV1::Writer(io::Error::other(
                "diagnostic writer missing",
            )));
        };
        let summary = handle
            .join()
            .map_err(|_panic| {
                DiagnosticCaptureCloseErrorV1::Writer(io::Error::other(
                    "diagnostic writer panicked",
                ))
            })?
            .map_err(DiagnosticCaptureCloseErrorV1::Writer)?;
        Ok(DiagnosticCaptureClosedV1 {
            output: self.output.clone(),
            capture_id: self.capture_id,
            status: summary.status,
            events_attempted: summary.attempted,
            events_accepted: summary.accepted,
            events_dropped: summary.dropped,
            events_written: summary.written,
            events_unwritten: summary.unwritten,
        })
    }
}

#[derive(Debug)]
struct WriterSummary {
    status: DiagnosticCaptureStatusV1,
    attempted: u64,
    accepted: u64,
    dropped: u64,
    written: u64,
    unwritten: u64,
}

const TIMELINE_SCHEMA_VERSION: u64 = 4;

fn write_capture(
    file: File,
    receiver: &Receiver<QueueItem>,
    capture_id: DiagnosticCaptureIdV1,
    limits: DiagnosticCaptureLimitsV1,
    counts: &SinkCounts,
    leases: &AtomicUsize,
    clock: &dyn DiagnosticClockV1,
) -> io::Result<WriterSummary> {
    let mut output = BufWriter::new(file);
    let mut bytes = 0_u64;
    let capture_hex = hex_id(capture_id.0);
    let header = json!({"record_type":"header", "schema_version":TIMELINE_SCHEMA_VERSION, "capture_id":capture_hex.as_str(),
        "stream_id":capture_hex.as_str(), "clock_domain":capture_hex.as_str(), "process_instance":capture_hex.as_str(),
        "started_ns":0, "max_record_bytes":limits.max_record_bytes,
        "max_counters_per_event":1, "coverage":[]});
    if serde_json::to_vec(&header)
        .map_err(io::Error::other)?
        .len()
        .saturating_add(1)
        > limits.max_record_bytes
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "diagnostic header exceeds record cap",
        ));
    }
    write_line(&mut output, &header, &mut bytes)?;
    let mut written = 0_u64;
    let mut unwritten = 0_u64;
    let mut max_event_ns = 0_u64;
    let mut open = BTreeSet::new();
    // The output cap bounds this set by the number of written begin records.
    // `open` alone cannot detect a span ID reused after its first end.
    let mut seen = BTreeSet::new();
    // A counter may be emitted more than once for one owner span. The number
    // of keys is bounded by the number of written records under the output cap.
    let mut counter_totals = BTreeMap::new();
    // The writer alone owns this buffer. Keep the exact serialized bytes until
    // the per-record and total-output caps have been checked.
    let mut encoded = Vec::new();
    let mut sealed = false;
    let mut invalid = false;
    // Make accepted records visible during a long, quiet capture. This is a
    // buffered flush only: crash durability remains outside the capture
    // contract, and fsync is reserved for the completed footer.
    let mut next_flush = Instant::now()
        .checked_add(FLUSH_INTERVAL)
        .unwrap_or_else(Instant::now);
    loop {
        let now = Instant::now();
        if now >= next_flush {
            output.flush()?;
            next_flush = now.checked_add(FLUSH_INTERVAL).unwrap_or(now);
        }
        let wait = next_flush.saturating_duration_since(Instant::now());
        let item = match receiver.recv_timeout(wait) {
            Ok(item) => item,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let QueueItem::Event(event) = item else {
            sealed = true;
            break;
        };
        invalid |= matches!(event.kind, DiagnosticEventKindV1::Health { .. });
        let Some(seq) = written.checked_add(1) else {
            unwritten = unwritten.saturating_add(1);
            continue;
        };
        let Some(record) = event_json(event, seq) else {
            unwritten = unwritten.saturating_add(1);
            continue;
        };
        encoded.clear();
        serde_json::to_writer(&mut encoded, &record).map_err(io::Error::other)?;
        let record_bytes = encoded.len().saturating_add(1);
        let size = match u64::try_from(record_bytes) {
            Ok(size) => size,
            Err(_overflow) => return Err(io::Error::other("diagnostic record size exceeds u64")),
        };
        if record_bytes > limits.max_record_bytes
            || bytes.saturating_add(size).saturating_add(1024) > limits.max_output_bytes
        {
            unwritten = unwritten.saturating_add(1);
            continue;
        }
        output.write_all(&encoded)?;
        output.write_all(b"\n")?;
        bytes = bytes.saturating_add(size);
        written = seq;
        max_event_ns = max_event_ns.max(event.ts_ns);
        match event.kind {
            DiagnosticEventKindV1::SpanBegin { span_id, .. } => {
                if !seen.insert(span_id.get()) {
                    invalid = true;
                }
                if !open.insert(span_id.get()) {
                    invalid = true;
                }
            }
            DiagnosticEventKindV1::SpanEnd { span_id, .. } => {
                if !open.remove(&span_id.get()) {
                    invalid = true;
                }
            }
            DiagnosticEventKindV1::Counter {
                span_id,
                key,
                value,
            } => {
                let Some(key_name) = known_counter_name(key) else {
                    invalid = true;
                    continue;
                };
                let total = counter_totals
                    .entry((span_id.get(), key_name))
                    .or_insert(0_u64);
                if let Some(next) = total.checked_add(value) {
                    *total = next;
                } else {
                    invalid = true;
                }
            }
            DiagnosticEventKindV1::RequestIdentity { .. }
            | DiagnosticEventKindV1::PhaseIdentity { .. }
            | DiagnosticEventKindV1::AnnRegionObservation { .. }
            | DiagnosticEventKindV1::SqliteSyncObservation { .. }
            | DiagnosticEventKindV1::SyncAggregate { .. }
            | DiagnosticEventKindV1::ObjectKindAggregate { .. }
            | DiagnosticEventKindV1::AdoptionGroupAggregate { .. }
            | DiagnosticEventKindV1::Bm25CensusAggregate { .. }
            | DiagnosticEventKindV1::Bm25BoundaryAggregate { .. }
            | DiagnosticEventKindV1::Bm25LiveBuildAggregate { .. }
            | DiagnosticEventKindV1::Link { .. }
            | DiagnosticEventKindV1::Health { .. } => {}
        }
    }
    let attempted = counts.attempted.load(Ordering::Acquire);
    let accepted = counts.accepted.load(Ordering::Acquire);
    let dropped = counts.dropped.load(Ordering::Acquire);
    unwritten = unwritten.max(accepted.saturating_sub(written));
    let counter_overflow = counts.overflow.load(Ordering::Acquire);
    let valid_accounting = !counter_overflow
        && attempted.checked_sub(accepted) == Some(dropped)
        && accepted.checked_sub(written) == Some(unwritten);
    let producer_leases = leases.load(Ordering::Acquire);
    let closed_ns = clock.now_ns();
    let complete = sealed
        && valid_accounting
        && producer_leases == 0
        && closed_ns >= max_event_ns
        && dropped == 0
        && unwritten == 0
        && open.is_empty();
    let status = if invalid || counter_overflow || counts.health_invalid.load(Ordering::Acquire) {
        DiagnosticCaptureStatusV1::Invalid
    } else if complete {
        DiagnosticCaptureStatusV1::Complete
    } else {
        DiagnosticCaptureStatusV1::Partial
    };
    let footer = json!({"record_type":"footer", "schema_version":TIMELINE_SCHEMA_VERSION, "capture_id":capture_hex.as_str(),
        "closed_ns":closed_ns, "status":match status { DiagnosticCaptureStatusV1::Complete => "complete", DiagnosticCaptureStatusV1::Partial => "partial", DiagnosticCaptureStatusV1::Invalid => "invalid" },
        "close_reason":if sealed { "normal" } else { "capture_cancelled" }, "last_output_seq":written, "producer_leases":producer_leases,
        "events_attempted":attempted, "events_accepted":accepted, "events_dropped":dropped,
        "events_written":written, "events_unwritten":unwritten, "open_spans":open.len()});
    let footer_size = serde_json::to_vec(&footer)
        .map_err(io::Error::other)?
        .len()
        .saturating_add(1);
    if footer_size > limits.max_record_bytes
        || bytes.saturating_add(match u64::try_from(footer_size) {
            Ok(size) => size,
            Err(_overflow) => return Err(io::Error::other("diagnostic footer size exceeds u64")),
        }) > limits.max_output_bytes
    {
        return Err(io::Error::other("diagnostic footer exceeds output cap"));
    }
    write_line(&mut output, &footer, &mut bytes)?;
    output.flush()?;
    output.get_ref().sync_all()?;
    Ok(WriterSummary {
        status,
        attempted,
        accepted,
        dropped,
        written,
        unwritten,
    })
}

fn write_line(writer: &mut impl Write, value: &Value, bytes: &mut u64) -> io::Result<()> {
    let encoded = serde_json::to_vec(value).map_err(io::Error::other)?;
    writer.write_all(&encoded)?;
    writer.write_all(b"\n")?;
    let line_bytes = u64::try_from(encoded.len().saturating_add(1))
        .map_err(|error| io::Error::other(format!("diagnostic line size exceeds u64: {error}")))?;
    *bytes = bytes.saturating_add(line_bytes);
    Ok(())
}

fn event_json(event: DiagnosticEmitV1, output_seq: u64) -> Option<Value> {
    let kind = match event.kind {
        DiagnosticEventKindV1::SpanBegin {
            span_id,
            parent_span_id,
            root_kind,
            stage,
            lane,
        } => {
            let mut map = Map::new();
            let _previous = map.insert("type".into(), json!("span_begin"));
            let _previous = map.insert("span_id".into(), json!(span_id.get()));
            let _previous = map.insert("stage".into(), json!(known_stage_name(stage)?));
            let _previous = map.insert("lane_id".into(), json!(lane.0));
            if let Some(id) = parent_span_id {
                let _previous = map.insert("parent_span_id".into(), json!(id.get()));
            }
            if let Some(root) = root_kind {
                let _previous = map.insert(
                    "root_kind".into(),
                    json!(match root {
                        DiagnosticRootKindV1::Request => "request",
                        DiagnosticRootKindV1::Operation => "operation",
                        DiagnosticRootKindV1::Capture => "capture",
                    }),
                );
            }
            Value::Object(map)
        }
        DiagnosticEventKindV1::SpanEnd { span_id, outcome } => {
            let mut map = Map::new();
            let _previous = map.insert("type".into(), json!("span_end"));
            let _previous = map.insert("span_id".into(), json!(span_id.get()));
            let (name, code) = match outcome {
                DiagnosticTerminalV1::Ok => ("ok", None),
                DiagnosticTerminalV1::Error { code } => ("error", Some(code)),
                DiagnosticTerminalV1::Cancelled { code } => ("cancelled", Some(code)),
                DiagnosticTerminalV1::Aborted => ("aborted", None),
            };
            let _previous = map.insert("outcome".into(), json!(name));
            if let Some(code) = code {
                let _previous = map.insert("error_code".into(), json!(code));
            }
            Value::Object(map)
        }
        DiagnosticEventKindV1::RequestIdentity {
            span_id,
            request_id,
            plane,
            connection_id,
        } => json!({"type":"request_identity", "span_id":span_id.get(), "request_id":request_id,
                "plane":match plane { quanta_index_core::DiagnosticPlaneV1::Query => "query", quanta_index_core::DiagnosticPlaneV1::Control => "control", quanta_index_core::DiagnosticPlaneV1::Ingest => "ingest" }, "connection_id":connection_id}),
        DiagnosticEventKindV1::PhaseIdentity { span_id, phase } => {
            use quanta_index_core::DiagnosticPhaseV1;
            let name = match phase {
                DiagnosticPhaseV1::FullIngestSeal => "full_ingest_seal",
                DiagnosticPhaseV1::FullIngest => "full_ingest",
                DiagnosticPhaseV1::FullSeal => "full_seal",
                DiagnosticPhaseV1::FullActivate => "full_activate",
                DiagnosticPhaseV1::DeltaIngestSeal => "delta_ingest_seal",
                DiagnosticPhaseV1::DeltaActivate => "delta_activate",
                DiagnosticPhaseV1::NoopSeal => "noop_seal",
                DiagnosticPhaseV1::NoopActivate => "noop_activate",
                DiagnosticPhaseV1::DeleteSeal => "delete_seal",
                DiagnosticPhaseV1::DeleteActivate => "delete_activate",
                DiagnosticPhaseV1::SameProcessReopen => "same_process_reopen",
                DiagnosticPhaseV1::NativeCorpusGeneration => "native_corpus_generation",
                DiagnosticPhaseV1::NativeConfigSetup => "native_config_setup",
                DiagnosticPhaseV1::NativeStartup => "native_startup",
                DiagnosticPhaseV1::NativeWorkspaceCreate => "native_workspace_create",
                DiagnosticPhaseV1::NativeProfileConfigure => "native_profile_configure",
                DiagnosticPhaseV1::NativeFullBeginInput => "native_full_begin_input",
                DiagnosticPhaseV1::NativeFullStageSources => "native_full_stage_sources",
                DiagnosticPhaseV1::NativeFullEventAck => "native_full_event_ack",
                DiagnosticPhaseV1::NativeFirstSearch => "native_first_search",
                DiagnosticPhaseV1::NativeDeltaStageSource => "native_delta_stage_source",
                DiagnosticPhaseV1::NativeDeltaEventAck => "native_delta_event_ack",
                DiagnosticPhaseV1::NativeDeltaSearch => "native_delta_search",
                DiagnosticPhaseV1::NativeNoopEventAck => "native_noop_event_ack",
                DiagnosticPhaseV1::NativeNoopStableSearch => "native_noop_stable_search",
                DiagnosticPhaseV1::NativeDeleteEventAck => "native_delete_event_ack",
                DiagnosticPhaseV1::NativeDeletedSearch => "native_deleted_search",
                DiagnosticPhaseV1::NativeOldPinSearch => "native_old_pin_search",
                DiagnosticPhaseV1::NativeChurnCycles => "native_churn_cycles",
                DiagnosticPhaseV1::NativeChurnOldViewSearch => "native_churn_old_view_search",
                DiagnosticPhaseV1::NativeDrain => "native_drain",
                DiagnosticPhaseV1::NativeReopen => "native_reopen",
                DiagnosticPhaseV1::NativeReopenReadHead => "native_reopen_read_head",
                DiagnosticPhaseV1::NativeReopenDeletedSearch => "native_reopen_deleted_search",
                DiagnosticPhaseV1::NativeReopenChurnSearch => "native_reopen_churn_search",
                DiagnosticPhaseV1::NativeReopenSearch => "native_reopen_search",
                DiagnosticPhaseV1::NativeFinalDrain => "native_final_drain",
            };
            json!({"type":"phase_identity", "span_id":span_id.get(), "phase":name})
        }
        DiagnosticEventKindV1::Counter {
            span_id,
            key,
            value,
        } => {
            json!({"type":"counter", "span_id":span_id.get(), "key":known_counter_name(key)?, "value":value})
        }
        DiagnosticEventKindV1::AnnRegionObservation {
            span_id,
            observation,
        } => {
            if !(1..=8192).contains(&observation.rows)
                || !(1..=256).contains(&observation.files)
                || !(1..=288).contains(&observation.chunks)
                || observation.chunks < observation.files
            {
                return None;
            }
            let (phase, descriptor_ref) = match (observation.phase, observation.descriptor_ref) {
                (DiagnosticAnnRegionPhaseV1::Installed, None) => ("installed", None),
                (DiagnosticAnnRegionPhaseV1::SelectedQuery, Some(reference)) => {
                    ("selected_query", Some(hex_ref(reference)))
                }
                _ => return None,
            };
            let index_kind = match observation.index_kind {
                DiagnosticAnnIndexKindV1::Flat => "flat",
                DiagnosticAnnIndexKindV1::IvfHnswSq => "ivf_hnsw_sq",
            };
            json!({"type":"ann_region_observation", "span_id":span_id.get(),
                "phase":phase, "fileset_ref":hex_ref(observation.fileset_ref),
                "descriptor_ref":descriptor_ref, "ordinal":observation.ordinal,
                "rows":observation.rows, "index_kind":index_kind,
                "files":observation.files, "chunks":observation.chunks})
        }
        DiagnosticEventKindV1::SqliteSyncObservation {
            span_id,
            observation,
        } => json!({"type":"sqlite_sync_observation", "span_id":span_id.get(),
        "observation": {
            "observed":observation.observed,
            "same_native_fd":observation.same_native_fd,
            "ambiguous":observation.ambiguous,
            "full_requested":observation.full_requested,
            "full_default_syscall":observation.full_default_syscall,
            "full_result":observation.full_result,
            "full_errno":observation.full_errno,
            "posix_result":observation.posix_result,
            "posix_errno":observation.posix_errno,
        }}),
        DiagnosticEventKindV1::SyncAggregate {
            span_id,
            label,
            calls,
            elapsed_sum_ns,
            elapsed_max_ns,
            failures,
        } => json!({"type":"sync_aggregate", "span_id":span_id.get(), "label":label.as_str(),
            "calls":calls, "elapsed_sum_ns":elapsed_sum_ns, "elapsed_max_ns":elapsed_max_ns,
            "failures":failures}),
        DiagnosticEventKindV1::ObjectKindAggregate {
            span_id,
            kind,
            new_objects,
            reused_objects,
        } => json!({"type":"object_kind_aggregate", "span_id":span_id.get(), "kind":kind.as_str(),
            "new_objects":new_objects, "reused_objects":reused_objects}),
        DiagnosticEventKindV1::AdoptionGroupAggregate {
            span_id,
            reason,
            refs,
            bytes,
            groups,
            failed_groups,
        } => {
            use quanta_index_core::diagnostic_trace::DiagnosticAdoptionFlushReasonV1;
            let reason = match reason {
                DiagnosticAdoptionFlushReasonV1::RefLimit => "ref_limit",
                DiagnosticAdoptionFlushReasonV1::ByteLimit => "byte_limit",
                DiagnosticAdoptionFlushReasonV1::Explicit => "explicit",
            };
            json!({"type":"adoption_group_aggregate", "span_id":span_id.get(), "reason":reason,
                "refs_bucket":occupancy_bucket_name(refs), "bytes_bucket":occupancy_bucket_name(bytes),
                "groups":groups, "failed_groups":failed_groups})
        }
        DiagnosticEventKindV1::Bm25CensusAggregate {
            span_id,
            calls,
            elapsed_ns,
            docs,
            encoded_bytes,
        } => json!({"type":"bm25_census_aggregate", "span_id":span_id.get(), "calls":calls,
            "elapsed_ns":elapsed_ns, "docs":docs, "encoded_bytes":encoded_bytes}),
        DiagnosticEventKindV1::Bm25BoundaryAggregate {
            span_id,
            label,
            calls,
            elapsed_sum_ns,
            logical_bytes,
            index_files,
        } => json!({"type":"bm25_boundary_aggregate", "span_id":span_id.get(),
            "label":label.as_str(), "calls":calls, "elapsed_sum_ns":elapsed_sum_ns,
            "logical_bytes":logical_bytes, "index_files":index_files}),
        DiagnosticEventKindV1::Bm25LiveBuildAggregate {
            span_id,
            calls,
            elapsed_ns,
            reused_segments,
            changed_segments,
            new_segments,
            mask_docs,
            new_census_docs,
            newly_dead_docs,
            logical_census_bytes,
            changed_ns,
            new_ns,
            death_ns,
            correction_keys,
            segment_fanout,
            retained_estimate_bytes,
        } => json!({"type":"bm25_live_build_aggregate", "span_id":span_id.get(),
            "calls":calls, "elapsed_ns":elapsed_ns, "reused_segments":reused_segments,
            "changed_segments":changed_segments, "new_segments":new_segments, "mask_docs":mask_docs,
            "new_census_docs":new_census_docs, "newly_dead_docs":newly_dead_docs,
            "logical_census_bytes":logical_census_bytes, "changed_ns":changed_ns, "new_ns":new_ns,
            "death_ns":death_ns, "correction_keys":correction_keys, "segment_fanout":segment_fanout,
            "retained_estimate_bytes":retained_estimate_bytes}),
        DiagnosticEventKindV1::Link { span_id, target } => {
            json!({"type":"link", "span_id":span_id.get(), "target_capture_id":hex_id(target.capture_id.0), "target_span_id":target.span_id.get()})
        }
        DiagnosticEventKindV1::Health { code } => {
            let code = match code {
                quanta_index_core::diagnostic_trace::DiagnosticHealthCodeV1::ForeignCaptureSpan => {
                    "foreign_capture_span"
                }
                quanta_index_core::diagnostic_trace::DiagnosticHealthCodeV1::MissingParentSpan => {
                    "missing_parent_span"
                }
                quanta_index_core::diagnostic_trace::DiagnosticHealthCodeV1::ClockRegressed => {
                    "clock_regressed"
                }
                quanta_index_core::diagnostic_trace::DiagnosticHealthCodeV1::CounterOverflow => {
                    "counter_overflow"
                }
                quanta_index_core::diagnostic_trace::DiagnosticHealthCodeV1::InvalidAttemptToken => {
                    "invalid_attempt_token"
                }
                quanta_index_core::diagnostic_trace::DiagnosticHealthCodeV1::DuplicateAttemptToken => {
                    "duplicate_attempt_token"
                }
                quanta_index_core::diagnostic_trace::DiagnosticHealthCodeV1::UnlinkedAttempt => {
                    "unlinked_attempt"
                }
            };
            json!({"type":"health", "code":code})
        }
    };
    Some(
        json!({"record_type":"event", "output_seq":output_seq, "capture_id":hex_id(event.capture_id.0), "ts_ns":event.ts_ns, "kind":kind}),
    )
}

fn known_stage_name(stage: quanta_index_core::DiagnosticStageV1) -> Option<&'static str> {
    use quanta_index_core::DiagnosticStageV1;
    match stage {
        DiagnosticStageV1::Request => Some("request"),
        DiagnosticStageV1::Operation => Some("operation"),
        DiagnosticStageV1::ClientAttempt => Some("client_attempt"),
        DiagnosticStageV1::Ingress => Some("ingress"),
        DiagnosticStageV1::QueueWait => Some("queue_wait"),
        DiagnosticStageV1::Dispatch => Some("dispatch"),
        DiagnosticStageV1::Backend => Some("backend"),
        DiagnosticStageV1::Encode => Some("encode"),
        DiagnosticStageV1::Write => Some("write"),
        DiagnosticStageV1::BuildPreflight => Some("build_preflight"),
        DiagnosticStageV1::BuildPrepare => Some("build_prepare"),
        DiagnosticStageV1::SemanticPrepare => Some("semantic_prepare"),
        DiagnosticStageV1::SemanticStream => Some("semantic_stream"),
        DiagnosticStageV1::SemanticDelete => Some("semantic_delete"),
        DiagnosticStageV1::SemanticAppend => Some("semantic_append"),
        DiagnosticStageV1::SemanticSeal => Some("semantic_seal"),
        DiagnosticStageV1::SemanticPromote => Some("semantic_promote"),
        DiagnosticStageV1::SemanticBuild => Some("semantic_build"),
        DiagnosticStageV1::LexicalBuild => Some("lexical_build"),
        DiagnosticStageV1::NativeTextPostings => Some("native_text_postings"),
        DiagnosticStageV1::NativeTextRoot => Some("native_text_root"),
        DiagnosticStageV1::NativeCoverageScan => Some("native_coverage_scan"),
        DiagnosticStageV1::NativeCoverageMapApply => Some("native_coverage_map_apply"),
        DiagnosticStageV1::NativeSearchablePrepare => Some("native_searchable_prepare"),
        DiagnosticStageV1::NativeCoverageSave => Some("native_coverage_save"),
        DiagnosticStageV1::NativeMemberAssemble => Some("native_member_assemble"),
        DiagnosticStageV1::Bm25CensusBuild => Some("bm25_census_build"),
        DiagnosticStageV1::Bm25LiveBuild => Some("bm25_live_build"),
        DiagnosticStageV1::Bm25Boundary => Some("bm25_boundary"),
        DiagnosticStageV1::Finalize => Some("finalize"),
        DiagnosticStageV1::Activate => Some("activate"),
        DiagnosticStageV1::CatalogLockWait => Some("catalog_lock_wait"),
        DiagnosticStageV1::CatalogLockHold => Some("catalog_lock_hold"),
        DiagnosticStageV1::RetentionPairLockWait => Some("retention_pair_lock_wait"),
        DiagnosticStageV1::RetentionPairLockHold => Some("retention_pair_lock_hold"),
        DiagnosticStageV1::RetentionRootLockWait => Some("retention_root_lock_wait"),
        DiagnosticStageV1::RetentionRootLockHold => Some("retention_root_lock_hold"),
        DiagnosticStageV1::RetentionRootSnapshot => Some("retention_root_snapshot"),
        DiagnosticStageV1::SourceTransactionOne => Some("source_transaction_one"),
        DiagnosticStageV1::SourceTransactionTwo => Some("source_transaction_two"),
        DiagnosticStageV1::NativeSourceApply => Some("native_source_apply"),
        DiagnosticStageV1::NativeSourcePathInstall => Some("native_source_path_install"),
        DiagnosticStageV1::NativeSourceMapApply => Some("native_source_map_apply"),
        DiagnosticStageV1::RetentionPlan => Some("retention_plan"),
        DiagnosticStageV1::RetentionMeasure => Some("retention_measure"),
        DiagnosticStageV1::RetentionWrite => Some("retention_write"),
        DiagnosticStageV1::RetentionReap => Some("retention_reap"),
        DiagnosticStageV1::ClosureRead => Some("closure_read"),
        DiagnosticStageV1::ClosureHash => Some("closure_hash"),
        DiagnosticStageV1::ClosureDecode => Some("closure_decode"),
        DiagnosticStageV1::SqliteWork => Some("sqlite_work"),
        DiagnosticStageV1::PhysicalInodeCensus => Some("physical_inode_census"),
        DiagnosticStageV1::ManagedObjectReserve => Some("managed_object_reserve"),
        DiagnosticStageV1::ManagedObjectConfirm => Some("managed_object_confirm"),
        DiagnosticStageV1::NativePublish => Some("native_publish"),
        DiagnosticStageV1::FilePublish => Some("file_publish"),
        DiagnosticStageV1::LexicalSeal => Some("lexical_seal"),
        DiagnosticStageV1::LexicalProof => Some("lexical_proof"),
        DiagnosticStageV1::Compaction => Some("compaction"),
        DiagnosticStageV1::QueryParse => Some("query_parse"),
        DiagnosticStageV1::QueryPlan => Some("query_plan"),
        DiagnosticStageV1::QueryReadView => Some("query_read_view"),
        DiagnosticStageV1::QueryOverlaySnapshot => Some("query_overlay_snapshot"),
        DiagnosticStageV1::QueryFast => Some("query_fast"),
        DiagnosticStageV1::QueryDeep => Some("query_deep"),
        DiagnosticStageV1::QueryLexical => Some("query_lexical"),
        DiagnosticStageV1::QueryEmbedding => Some("query_embedding"),
        DiagnosticStageV1::QueryOverlayEmbedding => Some("query_overlay_embedding"),
        DiagnosticStageV1::QueryCandidate => Some("query_candidate"),
        DiagnosticStageV1::QueryVerify => Some("query_verify"),
        DiagnosticStageV1::QueryRank => Some("query_rank"),
        DiagnosticStageV1::QueryProject => Some("query_project"),
        DiagnosticStageV1::HarnessPhase => Some("harness_phase"),
        DiagnosticStageV1::ObserverSetup => Some("observer_setup"),
        DiagnosticStageV1::ObserverSample => Some("observer_sample"),
        DiagnosticStageV1::ObserverTeardown => Some("observer_teardown"),
        DiagnosticStageV1::ArtifactWrite => Some("artifact_write"),
        DiagnosticStageV1::RuntimeBoot => Some("runtime_boot"),
        DiagnosticStageV1::BootRestore => Some("boot_restore"),
        DiagnosticStageV1::BootReclaim => Some("boot_reclaim"),
        DiagnosticStageV1::BootInventory => Some("boot_inventory"),
        DiagnosticStageV1::BootActiveProof => Some("boot_active_proof"),
        DiagnosticStageV1::RuntimeMaintenance => Some("runtime_maintenance"),
        DiagnosticStageV1::RuntimeStop => Some("runtime_stop"),
        DiagnosticStageV1::RuntimeJoin => Some("runtime_join"),
        _ => None,
    }
}

fn known_counter_name(counter: quanta_index_core::DiagnosticCounterV1) -> Option<&'static str> {
    use quanta_index_core::DiagnosticCounterV1;
    match counter {
        DiagnosticCounterV1::FilesVisited => Some("files_visited"),
        DiagnosticCounterV1::FilesRead => Some("files_read"),
        DiagnosticCounterV1::FilesHashed => Some("files_hashed"),
        DiagnosticCounterV1::FilesDecoded => Some("files_decoded"),
        DiagnosticCounterV1::InodesVisited => Some("inodes_visited"),
        DiagnosticCounterV1::UniqueInodes => Some("unique_inodes"),
        DiagnosticCounterV1::BytesRead => Some("bytes_read"),
        DiagnosticCounterV1::BytesHashed => Some("bytes_hashed"),
        DiagnosticCounterV1::BytesDecoded => Some("bytes_decoded"),
        DiagnosticCounterV1::WorkReads => Some("work_reads"),
        DiagnosticCounterV1::WorkWrites => Some("work_writes"),
        DiagnosticCounterV1::WorkBytes => Some("work_bytes"),
        DiagnosticCounterV1::WorkMutations => Some("work_mutations"),
        DiagnosticCounterV1::SqliteRows => Some("sqlite_rows"),
        DiagnosticCounterV1::SqliteBatches => Some("sqlite_batches"),
        DiagnosticCounterV1::SqliteWriteCalls => Some("sqlite_write_calls"),
        DiagnosticCounterV1::SqliteWriteRequestedBytes => Some("sqlite_write_requested_bytes"),
        DiagnosticCounterV1::SqliteWriteFailures => Some("sqlite_write_failures"),
        DiagnosticCounterV1::SqliteWalWriteCalls => Some("sqlite_wal_write_calls"),
        DiagnosticCounterV1::SqliteMainWriteCalls => Some("sqlite_main_write_calls"),
        DiagnosticCounterV1::SqliteOtherWriteCalls => Some("sqlite_other_write_calls"),
        DiagnosticCounterV1::SqliteElapsedNs => Some("sqlite_elapsed_ns"),
        DiagnosticCounterV1::QueryPostingCalls => Some("query_posting_calls"),
        DiagnosticCounterV1::QueryPostingHits => Some("query_posting_hits"),
        DiagnosticCounterV1::QueryPostingElapsedNs => Some("query_posting_elapsed_ns"),
        DiagnosticCounterV1::QueryPhraseChecks => Some("query_phrase_checks"),
        DiagnosticCounterV1::QueryPhraseElapsedNs => Some("query_phrase_elapsed_ns"),
        DiagnosticCounterV1::Candidates => Some("candidates"),
        DiagnosticCounterV1::CandidatesVerified => Some("candidates_verified"),
        DiagnosticCounterV1::Matches => Some("matches"),
        DiagnosticCounterV1::RejectedAtGuard => Some("rejected_at_guard"),
        DiagnosticCounterV1::SyncCalls => Some("sync_calls"),
        DiagnosticCounterV1::SyncFailures => Some("sync_failures"),
        DiagnosticCounterV1::SyncElapsedNs => Some("sync_elapsed_ns"),
        DiagnosticCounterV1::SqliteSyncOverwrite => Some("sqlite_sync_overwrite"),
        DiagnosticCounterV1::SqliteSyncGrowth => Some("sqlite_sync_growth"),
        DiagnosticCounterV1::SqliteSyncTruncate => Some("sqlite_sync_truncate"),
        DiagnosticCounterV1::SqliteSyncCreate => Some("sqlite_sync_create"),
        DiagnosticCounterV1::SqliteSyncNoMutation => Some("sqlite_sync_no_mutation"),
        DiagnosticCounterV1::SqliteSyncUnclassified => Some("sqlite_sync_unclassified"),
        DiagnosticCounterV1::NewObjects => Some("new_objects"),
        DiagnosticCounterV1::ReusedObjects => Some("reused_objects"),
        DiagnosticCounterV1::CompactedDocuments => Some("compacted_documents"),
        DiagnosticCounterV1::CompactedSegments => Some("compacted_segments"),
        DiagnosticCounterV1::CompactedBytes => Some("compacted_bytes"),
        DiagnosticCounterV1::PostingMutations => Some("posting_mutations"),
        DiagnosticCounterV1::MapPageMutations => Some("map_page_mutations"),
        DiagnosticCounterV1::ManagedObjectInputs => Some("managed_object_inputs"),
        DiagnosticCounterV1::InvalidAttemptTokens => Some("invalid_attempt_tokens"),
        DiagnosticCounterV1::UniqueClosureRefs => Some("unique_closure_refs"),
        DiagnosticCounterV1::ClosureDependencyEdges => Some("closure_dependency_edges"),
        DiagnosticCounterV1::ClosureAuthenticatedBytes => Some("closure_authenticated_bytes"),
        DiagnosticCounterV1::PhysicalBytesMeasured => Some("physical_bytes_measured"),
        DiagnosticCounterV1::RetainedPairs => Some("retained_pairs"),
        DiagnosticCounterV1::MeasureCalls => Some("measure_calls"),
        DiagnosticCounterV1::RemovedRoots => Some("removed_roots"),
        DiagnosticCounterV1::SemanticWindows => Some("semantic_windows"),
        DiagnosticCounterV1::SemanticOwnerScopes => Some("semantic_owner_scopes"),
        DiagnosticCounterV1::SemanticDeleteCalls => Some("semantic_delete_calls"),
        DiagnosticCounterV1::SemanticAppendCalls => Some("semantic_append_calls"),
        DiagnosticCounterV1::SemanticRowsAppended => Some("semantic_rows_appended"),
        DiagnosticCounterV1::SemanticRowsDeleted => Some("semantic_rows_deleted"),
        DiagnosticCounterV1::SemanticVectorBytes => Some("semantic_vector_bytes"),
        DiagnosticCounterV1::QueryAdmittedLimit => Some("query_admitted_limit"),
        DiagnosticCounterV1::QueryReadLimit => Some("query_read_limit"),
        DiagnosticCounterV1::QueryByteLimit => Some("query_byte_limit"),
        DiagnosticCounterV1::RegexLiteralCount => Some("regex_literal_count"),
        DiagnosticCounterV1::RegexAlternativeCount => Some("regex_alternative_count"),
        DiagnosticCounterV1::RegexNecessaryGramCount => Some("regex_necessary_gram_count"),
        DiagnosticCounterV1::ClosureReadElapsedNs => Some("closure_read_elapsed_ns"),
        DiagnosticCounterV1::ClosureHashElapsedNs => Some("closure_hash_elapsed_ns"),
        DiagnosticCounterV1::ClosureDecodeElapsedNs => Some("closure_decode_elapsed_ns"),
        DiagnosticCounterV1::QueueDropped => Some("queue_dropped"),
        DiagnosticCounterV1::QueueAccepted => Some("queue_accepted"),
        DiagnosticCounterV1::OutputWritten => Some("output_written"),
        DiagnosticCounterV1::OutputUnwritten => Some("output_unwritten"),
        _ => None,
    }
}

const fn occupancy_bucket_name(
    bucket: quanta_index_core::diagnostic_trace::DiagnosticOccupancyBucketV1,
) -> &'static str {
    use quanta_index_core::diagnostic_trace::DiagnosticOccupancyBucketV1;
    match bucket {
        DiagnosticOccupancyBucketV1::Zero => "zero",
        DiagnosticOccupancyBucketV1::UpTo25Percent => "up_to_25_percent",
        DiagnosticOccupancyBucketV1::UpTo50Percent => "up_to_50_percent",
        DiagnosticOccupancyBucketV1::UpTo75Percent => "up_to_75_percent",
        DiagnosticOccupancyBucketV1::UpTo100Percent => "up_to_100_percent",
        DiagnosticOccupancyBucketV1::Over100Percent => "over_100_percent",
    }
}

fn hex_id(id: [u8; 16]) -> String {
    let mut out = String::with_capacity(32);
    for byte in id {
        use std::fmt::Write as _;
        let _written = write!(out, "{byte:02x}");
    }
    out
}

fn hex_ref(reference: [u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in reference {
        use std::fmt::Write as _;
        let _written = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quanta_index_core::diagnostic_trace::{
        DiagnosticAdoptionFlushReasonV1, DiagnosticAnnRegionObservationV1,
        DiagnosticBm25BoundaryLabelV1, DiagnosticHealthCodeV1, DiagnosticObjectKindV1,
        DiagnosticOccupancyBucketV1, DiagnosticSqliteSyncObservationV1, DiagnosticSyncLabelV1,
    };
    use quanta_index_core::{
        DiagnosticCounterV1, DiagnosticLaneV1, DiagnosticRootKindV1, DiagnosticSpanIdV1,
        DiagnosticStageV1, DiagnosticTerminalV1,
    };
    use std::num::NonZeroU64;

    #[test]
    fn stage_and_counter_names_match_complete_prechange_vocabulary() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../../tools/benchmark/retrieval/causal_timeline.schema.json"
        ))
        .expect("timeline schema");
        let stage_golden = [
            (DiagnosticStageV1::Request, "request"),
            (DiagnosticStageV1::Operation, "operation"),
            (DiagnosticStageV1::ClientAttempt, "client_attempt"),
            (DiagnosticStageV1::Ingress, "ingress"),
            (DiagnosticStageV1::QueueWait, "queue_wait"),
            (DiagnosticStageV1::Dispatch, "dispatch"),
            (DiagnosticStageV1::Backend, "backend"),
            (DiagnosticStageV1::Encode, "encode"),
            (DiagnosticStageV1::Write, "write"),
            (DiagnosticStageV1::BuildPreflight, "build_preflight"),
            (DiagnosticStageV1::BuildPrepare, "build_prepare"),
            (DiagnosticStageV1::SemanticPrepare, "semantic_prepare"),
            (DiagnosticStageV1::SemanticStream, "semantic_stream"),
            (DiagnosticStageV1::SemanticDelete, "semantic_delete"),
            (DiagnosticStageV1::SemanticAppend, "semantic_append"),
            (DiagnosticStageV1::SemanticSeal, "semantic_seal"),
            (DiagnosticStageV1::SemanticPromote, "semantic_promote"),
            (DiagnosticStageV1::SemanticBuild, "semantic_build"),
            (DiagnosticStageV1::LexicalBuild, "lexical_build"),
            (
                DiagnosticStageV1::NativeTextPostings,
                "native_text_postings",
            ),
            (DiagnosticStageV1::NativeTextRoot, "native_text_root"),
            (
                DiagnosticStageV1::NativeCoverageScan,
                "native_coverage_scan",
            ),
            (
                DiagnosticStageV1::NativeCoverageMapApply,
                "native_coverage_map_apply",
            ),
            (
                DiagnosticStageV1::NativeSearchablePrepare,
                "native_searchable_prepare",
            ),
            (
                DiagnosticStageV1::NativeCoverageSave,
                "native_coverage_save",
            ),
            (
                DiagnosticStageV1::NativeMemberAssemble,
                "native_member_assemble",
            ),
            (DiagnosticStageV1::Bm25CensusBuild, "bm25_census_build"),
            (DiagnosticStageV1::Bm25LiveBuild, "bm25_live_build"),
            (DiagnosticStageV1::Bm25Boundary, "bm25_boundary"),
            (DiagnosticStageV1::Finalize, "finalize"),
            (DiagnosticStageV1::Activate, "activate"),
            (DiagnosticStageV1::CatalogLockWait, "catalog_lock_wait"),
            (DiagnosticStageV1::CatalogLockHold, "catalog_lock_hold"),
            (
                DiagnosticStageV1::RetentionPairLockWait,
                "retention_pair_lock_wait",
            ),
            (
                DiagnosticStageV1::RetentionPairLockHold,
                "retention_pair_lock_hold",
            ),
            (
                DiagnosticStageV1::RetentionRootLockWait,
                "retention_root_lock_wait",
            ),
            (
                DiagnosticStageV1::RetentionRootLockHold,
                "retention_root_lock_hold",
            ),
            (
                DiagnosticStageV1::RetentionRootSnapshot,
                "retention_root_snapshot",
            ),
            (
                DiagnosticStageV1::SourceTransactionOne,
                "source_transaction_one",
            ),
            (
                DiagnosticStageV1::SourceTransactionTwo,
                "source_transaction_two",
            ),
            (DiagnosticStageV1::NativeSourceApply, "native_source_apply"),
            (
                DiagnosticStageV1::NativeSourcePathInstall,
                "native_source_path_install",
            ),
            (
                DiagnosticStageV1::NativeSourceMapApply,
                "native_source_map_apply",
            ),
            (DiagnosticStageV1::RetentionPlan, "retention_plan"),
            (DiagnosticStageV1::RetentionMeasure, "retention_measure"),
            (DiagnosticStageV1::RetentionWrite, "retention_write"),
            (DiagnosticStageV1::RetentionReap, "retention_reap"),
            (DiagnosticStageV1::ClosureRead, "closure_read"),
            (DiagnosticStageV1::ClosureHash, "closure_hash"),
            (DiagnosticStageV1::ClosureDecode, "closure_decode"),
            (DiagnosticStageV1::SqliteWork, "sqlite_work"),
            (
                DiagnosticStageV1::PhysicalInodeCensus,
                "physical_inode_census",
            ),
            (
                DiagnosticStageV1::ManagedObjectReserve,
                "managed_object_reserve",
            ),
            (
                DiagnosticStageV1::ManagedObjectConfirm,
                "managed_object_confirm",
            ),
            (DiagnosticStageV1::NativePublish, "native_publish"),
            (DiagnosticStageV1::FilePublish, "file_publish"),
            (DiagnosticStageV1::LexicalSeal, "lexical_seal"),
            (DiagnosticStageV1::LexicalProof, "lexical_proof"),
            (DiagnosticStageV1::Compaction, "compaction"),
            (DiagnosticStageV1::QueryParse, "query_parse"),
            (DiagnosticStageV1::QueryPlan, "query_plan"),
            (DiagnosticStageV1::QueryReadView, "query_read_view"),
            (
                DiagnosticStageV1::QueryOverlaySnapshot,
                "query_overlay_snapshot",
            ),
            (DiagnosticStageV1::QueryFast, "query_fast"),
            (DiagnosticStageV1::QueryDeep, "query_deep"),
            (DiagnosticStageV1::QueryLexical, "query_lexical"),
            (DiagnosticStageV1::QueryEmbedding, "query_embedding"),
            (
                DiagnosticStageV1::QueryOverlayEmbedding,
                "query_overlay_embedding",
            ),
            (DiagnosticStageV1::QueryCandidate, "query_candidate"),
            (DiagnosticStageV1::QueryVerify, "query_verify"),
            (DiagnosticStageV1::QueryRank, "query_rank"),
            (DiagnosticStageV1::QueryProject, "query_project"),
            (DiagnosticStageV1::HarnessPhase, "harness_phase"),
            (DiagnosticStageV1::ObserverSetup, "observer_setup"),
            (DiagnosticStageV1::ObserverSample, "observer_sample"),
            (DiagnosticStageV1::ObserverTeardown, "observer_teardown"),
            (DiagnosticStageV1::ArtifactWrite, "artifact_write"),
            (DiagnosticStageV1::RuntimeBoot, "runtime_boot"),
            (DiagnosticStageV1::BootRestore, "boot_restore"),
            (DiagnosticStageV1::BootReclaim, "boot_reclaim"),
            (DiagnosticStageV1::BootInventory, "boot_inventory"),
            (DiagnosticStageV1::BootActiveProof, "boot_active_proof"),
            (DiagnosticStageV1::RuntimeMaintenance, "runtime_maintenance"),
            (DiagnosticStageV1::RuntimeStop, "runtime_stop"),
            (DiagnosticStageV1::RuntimeJoin, "runtime_join"),
        ];
        for (variant, name) in stage_golden {
            assert_eq!(known_stage_name(variant), Some(name));
        }
        let mut actual: Vec<_> = stage_golden.into_iter().map(|(_, name)| name).collect();
        actual.sort_unstable();
        let mut expected: Vec<_> = json_path(
            &schema,
            &["$defs", "spanBegin", "properties", "stage", "enum"],
        )
        .as_array()
        .expect("closed vocabulary")
        .iter()
        .map(|name| name.as_str().expect("name"))
        .collect();
        expected.sort_unstable();
        assert_eq!(actual, expected);
        let counter_golden = [
            (DiagnosticCounterV1::FilesVisited, "files_visited"),
            (DiagnosticCounterV1::FilesRead, "files_read"),
            (DiagnosticCounterV1::FilesHashed, "files_hashed"),
            (DiagnosticCounterV1::FilesDecoded, "files_decoded"),
            (DiagnosticCounterV1::InodesVisited, "inodes_visited"),
            (DiagnosticCounterV1::UniqueInodes, "unique_inodes"),
            (DiagnosticCounterV1::BytesRead, "bytes_read"),
            (DiagnosticCounterV1::BytesHashed, "bytes_hashed"),
            (DiagnosticCounterV1::BytesDecoded, "bytes_decoded"),
            (DiagnosticCounterV1::WorkReads, "work_reads"),
            (DiagnosticCounterV1::WorkWrites, "work_writes"),
            (DiagnosticCounterV1::WorkBytes, "work_bytes"),
            (DiagnosticCounterV1::WorkMutations, "work_mutations"),
            (DiagnosticCounterV1::SqliteRows, "sqlite_rows"),
            (DiagnosticCounterV1::SqliteBatches, "sqlite_batches"),
            (DiagnosticCounterV1::SqliteWriteCalls, "sqlite_write_calls"),
            (
                DiagnosticCounterV1::SqliteWriteRequestedBytes,
                "sqlite_write_requested_bytes",
            ),
            (
                DiagnosticCounterV1::SqliteWriteFailures,
                "sqlite_write_failures",
            ),
            (
                DiagnosticCounterV1::SqliteWalWriteCalls,
                "sqlite_wal_write_calls",
            ),
            (
                DiagnosticCounterV1::SqliteMainWriteCalls,
                "sqlite_main_write_calls",
            ),
            (
                DiagnosticCounterV1::SqliteOtherWriteCalls,
                "sqlite_other_write_calls",
            ),
            (DiagnosticCounterV1::SqliteElapsedNs, "sqlite_elapsed_ns"),
            (
                DiagnosticCounterV1::QueryPostingCalls,
                "query_posting_calls",
            ),
            (DiagnosticCounterV1::QueryPostingHits, "query_posting_hits"),
            (
                DiagnosticCounterV1::QueryPostingElapsedNs,
                "query_posting_elapsed_ns",
            ),
            (
                DiagnosticCounterV1::QueryPhraseChecks,
                "query_phrase_checks",
            ),
            (
                DiagnosticCounterV1::QueryPhraseElapsedNs,
                "query_phrase_elapsed_ns",
            ),
            (DiagnosticCounterV1::Candidates, "candidates"),
            (
                DiagnosticCounterV1::CandidatesVerified,
                "candidates_verified",
            ),
            (DiagnosticCounterV1::Matches, "matches"),
            (DiagnosticCounterV1::RejectedAtGuard, "rejected_at_guard"),
            (DiagnosticCounterV1::SyncCalls, "sync_calls"),
            (DiagnosticCounterV1::SyncFailures, "sync_failures"),
            (DiagnosticCounterV1::SyncElapsedNs, "sync_elapsed_ns"),
            (
                DiagnosticCounterV1::SqliteSyncOverwrite,
                "sqlite_sync_overwrite",
            ),
            (DiagnosticCounterV1::SqliteSyncGrowth, "sqlite_sync_growth"),
            (
                DiagnosticCounterV1::SqliteSyncTruncate,
                "sqlite_sync_truncate",
            ),
            (DiagnosticCounterV1::SqliteSyncCreate, "sqlite_sync_create"),
            (
                DiagnosticCounterV1::SqliteSyncNoMutation,
                "sqlite_sync_no_mutation",
            ),
            (
                DiagnosticCounterV1::SqliteSyncUnclassified,
                "sqlite_sync_unclassified",
            ),
            (DiagnosticCounterV1::NewObjects, "new_objects"),
            (DiagnosticCounterV1::ReusedObjects, "reused_objects"),
            (
                DiagnosticCounterV1::CompactedDocuments,
                "compacted_documents",
            ),
            (DiagnosticCounterV1::CompactedSegments, "compacted_segments"),
            (DiagnosticCounterV1::CompactedBytes, "compacted_bytes"),
            (DiagnosticCounterV1::PostingMutations, "posting_mutations"),
            (DiagnosticCounterV1::MapPageMutations, "map_page_mutations"),
            (
                DiagnosticCounterV1::ManagedObjectInputs,
                "managed_object_inputs",
            ),
            (
                DiagnosticCounterV1::InvalidAttemptTokens,
                "invalid_attempt_tokens",
            ),
            (
                DiagnosticCounterV1::UniqueClosureRefs,
                "unique_closure_refs",
            ),
            (
                DiagnosticCounterV1::ClosureDependencyEdges,
                "closure_dependency_edges",
            ),
            (
                DiagnosticCounterV1::ClosureAuthenticatedBytes,
                "closure_authenticated_bytes",
            ),
            (
                DiagnosticCounterV1::PhysicalBytesMeasured,
                "physical_bytes_measured",
            ),
            (DiagnosticCounterV1::RetainedPairs, "retained_pairs"),
            (DiagnosticCounterV1::MeasureCalls, "measure_calls"),
            (DiagnosticCounterV1::RemovedRoots, "removed_roots"),
            (DiagnosticCounterV1::SemanticWindows, "semantic_windows"),
            (
                DiagnosticCounterV1::SemanticOwnerScopes,
                "semantic_owner_scopes",
            ),
            (
                DiagnosticCounterV1::SemanticDeleteCalls,
                "semantic_delete_calls",
            ),
            (
                DiagnosticCounterV1::SemanticAppendCalls,
                "semantic_append_calls",
            ),
            (
                DiagnosticCounterV1::SemanticRowsAppended,
                "semantic_rows_appended",
            ),
            (
                DiagnosticCounterV1::SemanticRowsDeleted,
                "semantic_rows_deleted",
            ),
            (
                DiagnosticCounterV1::SemanticVectorBytes,
                "semantic_vector_bytes",
            ),
            (
                DiagnosticCounterV1::QueryAdmittedLimit,
                "query_admitted_limit",
            ),
            (DiagnosticCounterV1::QueryReadLimit, "query_read_limit"),
            (DiagnosticCounterV1::QueryByteLimit, "query_byte_limit"),
            (
                DiagnosticCounterV1::RegexLiteralCount,
                "regex_literal_count",
            ),
            (
                DiagnosticCounterV1::RegexAlternativeCount,
                "regex_alternative_count",
            ),
            (
                DiagnosticCounterV1::RegexNecessaryGramCount,
                "regex_necessary_gram_count",
            ),
            (
                DiagnosticCounterV1::ClosureReadElapsedNs,
                "closure_read_elapsed_ns",
            ),
            (
                DiagnosticCounterV1::ClosureHashElapsedNs,
                "closure_hash_elapsed_ns",
            ),
            (
                DiagnosticCounterV1::ClosureDecodeElapsedNs,
                "closure_decode_elapsed_ns",
            ),
            (DiagnosticCounterV1::QueueDropped, "queue_dropped"),
            (DiagnosticCounterV1::QueueAccepted, "queue_accepted"),
            (DiagnosticCounterV1::OutputWritten, "output_written"),
            (DiagnosticCounterV1::OutputUnwritten, "output_unwritten"),
        ];
        for (variant, name) in counter_golden {
            assert_eq!(known_counter_name(variant), Some(name));
        }
        let mut actual: Vec<_> = counter_golden.into_iter().map(|(_, name)| name).collect();
        actual.sort_unstable();
        let mut expected: Vec<_> = json_path(&schema, &["$defs", "counterKey", "enum"])
            .as_array()
            .expect("closed vocabulary")
            .iter()
            .map(|name| name.as_str().expect("name"))
            .collect();
        expected.sort_unstable();
        assert_eq!(actual, expected);
    }

    #[test]
    fn reused_writer_buffer_preserves_fixed_event_wire_bytes() {
        let span_id = DiagnosticSpanIdV1::from_nonzero(NonZeroU64::new(7).expect("span"));
        let mut encoded = Vec::new();
        for (kind, output_seq, expected) in [
            (
                DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id: None,
                    root_kind: None,
                    stage: DiagnosticStageV1::ManagedObjectReserve,
                    lane: DiagnosticLaneV1(3),
                },
                1,
                br#"{"capture_id":"03030303030303030303030303030303","kind":{"lane_id":3,"span_id":7,"stage":"managed_object_reserve","type":"span_begin"},"output_seq":1,"record_type":"event","ts_ns":11}"#.as_slice(),
            ),
            (
                DiagnosticEventKindV1::Counter {
                    span_id,
                    key: DiagnosticCounterV1::ManagedObjectInputs,
                    value: 9,
                },
                2,
                br#"{"capture_id":"03030303030303030303030303030303","kind":{"key":"managed_object_inputs","span_id":7,"type":"counter","value":9},"output_seq":2,"record_type":"event","ts_ns":11}"#.as_slice(),
            ),
        ] {
            let record = event_json(
                DiagnosticEmitV1 {
                    capture_id: DiagnosticCaptureIdV1([3; 16]),
                    ts_ns: 11,
                    kind,
                },
                output_seq,
            )
            .expect("closed event");
            encoded.clear();
            serde_json::to_writer(&mut encoded, &record).expect("encode event");
            assert_eq!(encoded.as_slice(), expected);
        }
    }
    #[test]
    fn output_cap_preserves_default_and_rejects_out_of_range_values() {
        let default = DiagnosticCaptureLimitsV1::default();
        assert_eq!(default.max_output_bytes, 67_108_864);
        assert!(default.with_max_output_bytes(6144).is_ok());
        assert!(default.with_max_output_bytes(1_073_741_824).is_ok());
        assert!(default.with_max_output_bytes(6143).is_err());
        assert!(default.with_max_output_bytes(1_073_741_825).is_err());
    }

    #[test]
    fn native_source_phase_names_record_errors_in_a_complete_capture() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("trace.jsonl");
        let mut capture =
            DiagnosticCaptureV1::start(path.clone(), DiagnosticCaptureLimitsV1::default())
                .expect("start");
        let lease = capture.lease().expect("lease");
        let root = lease
            .context()
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(3))
            .expect("root");
        let context = lease.context().child_context(&root);
        for (stage, outcome) in [
            (
                DiagnosticStageV1::NativeSourceApply,
                DiagnosticTerminalV1::Error { code: 1 },
            ),
            (
                DiagnosticStageV1::NativeSourcePathInstall,
                DiagnosticTerminalV1::Ok,
            ),
            (
                DiagnosticStageV1::NativeSourceMapApply,
                DiagnosticTerminalV1::Error { code: 1 },
            ),
        ] {
            context
                .enter(stage, DiagnosticLaneV1(3))
                .expect("phase")
                .finish(outcome)
                .expect("finish phase");
        }
        root.finish(DiagnosticTerminalV1::Error { code: 1 })
            .expect("finish root");
        drop(context);
        drop(lease);
        let closed = capture.close(Duration::from_secs(2)).expect("close");
        assert_eq!(closed.status, DiagnosticCaptureStatusV1::Complete);
        assert_eq!(closed.events_written, 8);
        assert_eq!(closed.events_attempted, 8);
        let text = std::fs::read_to_string(path).expect("read");
        let records: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        let stages: Vec<&str> = records
            .iter()
            .filter_map(|row| {
                row.get("kind")
                    .and_then(|kind| kind.get("stage"))
                    .and_then(Value::as_str)
            })
            .collect();
        assert_eq!(
            stages,
            [
                "operation",
                "native_source_apply",
                "native_source_path_install",
                "native_source_map_apply"
            ]
        );
        assert_eq!(
            records
                .iter()
                .filter(|row| row
                    .get("kind")
                    .and_then(|kind| kind.get("outcome"))
                    .and_then(Value::as_str)
                    == Some("error"))
                .count(),
            3
        );
    }

    fn json_path<'a>(value: &'a Value, keys: &[&str]) -> &'a Value {
        keys.iter()
            .fold(value, |node, key| node.get(*key).expect("schema path"))
    }

    #[derive(Debug)]
    struct FakeClock(AtomicU64);

    impl DiagnosticClockV1 for FakeClock {
        fn now_ns(&self) -> u64 {
            self.0.load(Ordering::Acquire)
        }
    }

    #[derive(Debug)]
    struct SequencedClock(AtomicUsize);

    impl DiagnosticClockV1 for SequencedClock {
        fn now_ns(&self) -> u64 {
            match self.0.fetch_add(1, Ordering::AcqRel) {
                1 => 9,
                _ => 10,
            }
        }
    }

    #[derive(Debug)]
    struct NullSink;

    impl DiagnosticEventSinkV1 for NullSink {
        fn try_emit(&self, _event: DiagnosticEmitV1) -> bool {
            true
        }
    }

    #[test]
    fn native_phase_identity_wire_names_match_closed_schema() {
        use quanta_index_core::DiagnosticPhaseV1;

        let schema: Value = serde_json::from_str(include_str!(
            "../../../../tools/benchmark/retrieval/causal_timeline.schema.json"
        ))
        .expect("timeline schema");
        let admitted = json_path(
            &schema,
            &["$defs", "phaseIdentity", "properties", "phase", "enum"],
        )
        .as_array()
        .expect("closed phase names");
        let span_id = DiagnosticSpanIdV1::from_nonzero(NonZeroU64::new(7).expect("span"));
        let names = [
            (
                DiagnosticPhaseV1::NativeCorpusGeneration,
                "native_corpus_generation",
            ),
            (DiagnosticPhaseV1::NativeConfigSetup, "native_config_setup"),
            (DiagnosticPhaseV1::NativeStartup, "native_startup"),
            (
                DiagnosticPhaseV1::NativeWorkspaceCreate,
                "native_workspace_create",
            ),
            (
                DiagnosticPhaseV1::NativeProfileConfigure,
                "native_profile_configure",
            ),
            (
                DiagnosticPhaseV1::NativeFullBeginInput,
                "native_full_begin_input",
            ),
            (
                DiagnosticPhaseV1::NativeFullStageSources,
                "native_full_stage_sources",
            ),
            (
                DiagnosticPhaseV1::NativeFullEventAck,
                "native_full_event_ack",
            ),
            (DiagnosticPhaseV1::NativeFirstSearch, "native_first_search"),
            (
                DiagnosticPhaseV1::NativeDeltaStageSource,
                "native_delta_stage_source",
            ),
            (
                DiagnosticPhaseV1::NativeDeltaEventAck,
                "native_delta_event_ack",
            ),
            (DiagnosticPhaseV1::NativeDeltaSearch, "native_delta_search"),
            (
                DiagnosticPhaseV1::NativeNoopEventAck,
                "native_noop_event_ack",
            ),
            (
                DiagnosticPhaseV1::NativeNoopStableSearch,
                "native_noop_stable_search",
            ),
            (
                DiagnosticPhaseV1::NativeDeleteEventAck,
                "native_delete_event_ack",
            ),
            (
                DiagnosticPhaseV1::NativeDeletedSearch,
                "native_deleted_search",
            ),
            (
                DiagnosticPhaseV1::NativeOldPinSearch,
                "native_old_pin_search",
            ),
            (DiagnosticPhaseV1::NativeChurnCycles, "native_churn_cycles"),
            (
                DiagnosticPhaseV1::NativeChurnOldViewSearch,
                "native_churn_old_view_search",
            ),
            (DiagnosticPhaseV1::NativeDrain, "native_drain"),
            (DiagnosticPhaseV1::NativeReopen, "native_reopen"),
            (
                DiagnosticPhaseV1::NativeReopenReadHead,
                "native_reopen_read_head",
            ),
            (
                DiagnosticPhaseV1::NativeReopenDeletedSearch,
                "native_reopen_deleted_search",
            ),
            (
                DiagnosticPhaseV1::NativeReopenChurnSearch,
                "native_reopen_churn_search",
            ),
            (
                DiagnosticPhaseV1::NativeReopenSearch,
                "native_reopen_search",
            ),
            (DiagnosticPhaseV1::NativeFinalDrain, "native_final_drain"),
        ];
        assert_eq!(names.len(), 26);
        for (phase, expected) in names {
            let record = event_json(
                DiagnosticEmitV1 {
                    capture_id: DiagnosticCaptureIdV1([3; 16]),
                    ts_ns: 11,
                    kind: DiagnosticEventKindV1::PhaseIdentity { span_id, phase },
                },
                1,
            )
            .expect("native phase event");
            assert_eq!(
                record.get("kind"),
                Some(&json!({"type":"phase_identity", "span_id":7, "phase":expected}))
            );
            assert!(
                admitted.contains(&json!(expected)),
                "schema omits {expected}"
            );
        }
    }

    #[test]
    fn ann_region_observation_has_closed_schema_three_identity_fields() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../../tools/benchmark/retrieval/causal_timeline.schema.json"
        ))
        .expect("timeline schema");
        assert_eq!(
            *json_path(
                &schema,
                &["$defs", "header", "properties", "schema_version", "const"]
            ),
            json!(4)
        );
        let span_id = DiagnosticSpanIdV1::from_nonzero(NonZeroU64::new(7).expect("span"));
        let emit = |observation| {
            event_json(
                DiagnosticEmitV1 {
                    capture_id: DiagnosticCaptureIdV1([3; 16]),
                    ts_ns: 11,
                    kind: DiagnosticEventKindV1::AnnRegionObservation {
                        span_id,
                        observation,
                    },
                },
                1,
            )
        };
        let installed = DiagnosticAnnRegionObservationV1 {
            phase: DiagnosticAnnRegionPhaseV1::Installed,
            fileset_ref: [0xab; 32],
            descriptor_ref: None,
            ordinal: 7,
            rows: 256,
            index_kind: DiagnosticAnnIndexKindV1::IvfHnswSq,
            files: 2,
            chunks: 3,
        };
        assert_eq!(
            *json_path(&emit(installed).expect("installed event"), &["kind"]),
            json!({"type":"ann_region_observation", "span_id":7,
                "phase":"installed", "fileset_ref":"ab".repeat(32),
                "descriptor_ref":null, "ordinal":7, "rows":256,
                "index_kind":"ivf_hnsw_sq", "files":2, "chunks":3})
        );
        let selected = DiagnosticAnnRegionObservationV1 {
            phase: DiagnosticAnnRegionPhaseV1::SelectedQuery,
            descriptor_ref: Some([0xcd; 32]),
            index_kind: DiagnosticAnnIndexKindV1::Flat,
            ..installed
        };
        assert_eq!(
            *json_path(&emit(selected).expect("query event"), &["kind"]),
            json!({"type":"ann_region_observation", "span_id":7,
                "phase":"selected_query", "fileset_ref":"ab".repeat(32),
                "descriptor_ref":"cd".repeat(32), "ordinal":7, "rows":256,
                "index_kind":"flat", "files":2, "chunks":3})
        );
        assert!(
            emit(DiagnosticAnnRegionObservationV1 {
                descriptor_ref: Some([0xcd; 32]),
                ..installed
            })
            .is_none()
        );
        assert!(
            emit(DiagnosticAnnRegionObservationV1 {
                chunks: 1,
                ..installed
            })
            .is_none()
        );
        assert!(
            json_path(&schema, &["$defs", "event", "properties", "kind", "oneOf"])
                .as_array()
                .expect("closed event variants")
                .contains(&json!({"$ref":"#/$defs/annRegionObservation"}))
        );
    }

    #[test]
    fn sqlite_sync_observation_records_unknown_and_syscall_options() {
        let span_id = DiagnosticSpanIdV1::from_nonzero(NonZeroU64::new(7).expect("span"));
        let emit = |observation| {
            event_json(
                DiagnosticEmitV1 {
                    capture_id: DiagnosticCaptureIdV1([3; 16]),
                    ts_ns: 11,
                    kind: DiagnosticEventKindV1::SqliteSyncObservation {
                        span_id,
                        observation,
                    },
                },
                1,
            )
            .expect("SQLite sync event")
        };
        let unknown = DiagnosticSqliteSyncObservationV1::default();
        assert_eq!(
            *json_path(&emit(unknown), &["kind", "observation"]),
            json!({"observed":false, "same_native_fd":false, "ambiguous":false,
                "full_requested":false, "full_default_syscall":false,
                "full_result":null, "full_errno":null,
                "posix_result":null, "posix_errno":null})
        );
        assert_eq!(
            *json_path(
                &emit(DiagnosticSqliteSyncObservationV1 {
                    observed: true,
                    same_native_fd: true,
                    full_requested: true,
                    full_default_syscall: true,
                    full_result: Some(-1),
                    full_errno: Some(45),
                    posix_result: Some(0),
                    posix_errno: Some(0),
                    ..unknown
                }),
                &["kind"]
            ),
            json!({"type":"sqlite_sync_observation", "span_id":7,
                "observation":{"observed":true, "same_native_fd":true,
                    "ambiguous":false, "full_requested":true,
                    "full_default_syscall":true, "full_result":-1,
                    "full_errno":45, "posix_result":0, "posix_errno":0}})
        );
    }

    #[test]
    fn aggregate_event_values_match_the_closed_timeline_schema() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../../tools/benchmark/retrieval/causal_timeline.schema.json"
        ))
        .expect("timeline schema");
        let span_id = DiagnosticSpanIdV1::from_nonzero(NonZeroU64::new(7).expect("span"));
        let event = |kind| DiagnosticEmitV1 {
            capture_id: DiagnosticCaptureIdV1([3; 16]),
            ts_ns: 11,
            kind,
        };
        for (stage, name) in [
            (
                DiagnosticStageV1::NativeTextPostings,
                "native_text_postings",
            ),
            (DiagnosticStageV1::NativeTextRoot, "native_text_root"),
            (
                DiagnosticStageV1::NativeCoverageScan,
                "native_coverage_scan",
            ),
            (
                DiagnosticStageV1::NativeCoverageMapApply,
                "native_coverage_map_apply",
            ),
            (
                DiagnosticStageV1::NativeSearchablePrepare,
                "native_searchable_prepare",
            ),
            (
                DiagnosticStageV1::NativeCoverageSave,
                "native_coverage_save",
            ),
            (
                DiagnosticStageV1::NativeMemberAssemble,
                "native_member_assemble",
            ),
            (
                DiagnosticStageV1::ManagedObjectReserve,
                "managed_object_reserve",
            ),
            (
                DiagnosticStageV1::ManagedObjectConfirm,
                "managed_object_confirm",
            ),
            (DiagnosticStageV1::LexicalSeal, "lexical_seal"),
            (DiagnosticStageV1::LexicalProof, "lexical_proof"),
            (DiagnosticStageV1::Bm25CensusBuild, "bm25_census_build"),
            (DiagnosticStageV1::Bm25LiveBuild, "bm25_live_build"),
            (DiagnosticStageV1::Bm25Boundary, "bm25_boundary"),
            (DiagnosticStageV1::CatalogLockWait, "catalog_lock_wait"),
            (DiagnosticStageV1::CatalogLockHold, "catalog_lock_hold"),
            (DiagnosticStageV1::ArtifactWrite, "artifact_write"),
        ] {
            let record = event_json(
                event(DiagnosticEventKindV1::SpanBegin {
                    span_id,
                    parent_span_id: None,
                    root_kind: None,
                    stage,
                    lane: DiagnosticLaneV1(2),
                }),
                1,
            )
            .expect("lexical stage event");
            assert_eq!(
                *record.get("kind").expect("record kind"),
                json!({"type":"span_begin", "span_id":7, "stage":name, "lane_id":2})
            );
            assert!(
                json_path(
                    &schema,
                    &["$defs", "spanBegin", "properties", "stage", "enum"]
                )
                .as_array()
                .expect("closed stages")
                .contains(&json!(name))
            );
        }
        let schema_labels = json_path(
            &schema,
            &["$defs", "syncAggregate", "properties", "label", "enum"],
        )
        .as_array()
        .expect("closed sync labels");
        let labels: Vec<Value> = DiagnosticSyncLabelV1::ALL
            .iter()
            .map(|label| json!(label.as_str()))
            .collect();
        assert_eq!(schema_labels, &labels);
        for label in DiagnosticSyncLabelV1::ALL {
            let record = event_json(
                event(DiagnosticEventKindV1::SyncAggregate {
                    span_id,
                    label,
                    calls: 3,
                    elapsed_sum_ns: 20,
                    elapsed_max_ns: 12,
                    failures: 1,
                }),
                1,
            )
            .expect("sync event");
            assert_eq!(
                *record.get("kind").expect("record kind"),
                json!({"type":"sync_aggregate", "span_id":7, "label":label.as_str(),
                    "calls":3, "elapsed_sum_ns":20, "elapsed_max_ns":12, "failures":1})
            );
        }

        let schema_kinds = json_path(
            &schema,
            &["$defs", "objectKindAggregate", "properties", "kind", "enum"],
        )
        .as_array()
        .expect("closed object kinds");
        let kinds: Vec<Value> = DiagnosticObjectKindV1::ALL
            .iter()
            .map(|kind| json!(kind.as_str()))
            .collect();
        assert_eq!(schema_kinds, &kinds);
        for kind in DiagnosticObjectKindV1::ALL {
            let record = event_json(
                event(DiagnosticEventKindV1::ObjectKindAggregate {
                    span_id,
                    kind,
                    new_objects: 5,
                    reused_objects: 2,
                }),
                4,
            )
            .expect("object kind event");
            assert_eq!(
                *record.get("kind").expect("record kind"),
                json!({"type":"object_kind_aggregate", "span_id":7, "kind":kind.as_str(),
                    "new_objects":5, "reused_objects":2})
            );
        }

        let census = event_json(
            event(DiagnosticEventKindV1::Bm25CensusAggregate {
                span_id,
                calls: 1,
                elapsed_ns: 13,
                docs: 5,
                encoded_bytes: 89,
            }),
            5,
        )
        .expect("BM25 census event");
        assert_eq!(
            *census.get("kind").expect("census kind"),
            json!({"type":"bm25_census_aggregate", "span_id":7, "calls":1,
                "elapsed_ns":13, "docs":5, "encoded_bytes":89})
        );
        assert_eq!(
            *json_path(&schema, &["$defs", "bm25CensusAggregate", "required"]),
            json!([
                "type",
                "span_id",
                "calls",
                "elapsed_ns",
                "docs",
                "encoded_bytes"
            ])
        );
        let boundary = event_json(
            event(DiagnosticEventKindV1::Bm25BoundaryAggregate {
                span_id,
                label: DiagnosticBm25BoundaryLabelV1::BaseRead,
                calls: 1,
                elapsed_sum_ns: 21,
                logical_bytes: 144,
                index_files: 3,
            }),
            6,
        )
        .expect("BM25 boundary event");
        assert_eq!(
            *boundary.get("kind").expect("boundary kind"),
            json!({"type":"bm25_boundary_aggregate", "span_id":7, "label":"base_read",
                "calls":1, "elapsed_sum_ns":21, "logical_bytes":144, "index_files":3})
        );
        let live = event_json(
            event(DiagnosticEventKindV1::Bm25LiveBuildAggregate {
                span_id,
                calls: 1,
                elapsed_ns: 55,
                reused_segments: 1,
                changed_segments: 2,
                new_segments: 3,
                mask_docs: 7,
                new_census_docs: 8,
                newly_dead_docs: 1,
                logical_census_bytes: 987,
                changed_ns: 12,
                new_ns: 13,
                death_ns: 2,
                correction_keys: 4,
                segment_fanout: 6,
                retained_estimate_bytes: 1024,
            }),
            7,
        )
        .expect("BM25 live build event");
        assert_eq!(
            *live.get("kind").expect("live kind"),
            json!({"type":"bm25_live_build_aggregate", "span_id":7, "calls":1,
                "elapsed_ns":55, "reused_segments":1, "changed_segments":2, "new_segments":3,
                "mask_docs":7, "new_census_docs":8, "newly_dead_docs":1,
                "logical_census_bytes":987, "changed_ns":12, "new_ns":13, "death_ns":2,
                "correction_keys":4, "segment_fanout":6, "retained_estimate_bytes":1024})
        );

        let reasons = [
            (DiagnosticAdoptionFlushReasonV1::RefLimit, "ref_limit"),
            (DiagnosticAdoptionFlushReasonV1::ByteLimit, "byte_limit"),
            (DiagnosticAdoptionFlushReasonV1::Explicit, "explicit"),
        ];
        let buckets = [
            (DiagnosticOccupancyBucketV1::Zero, "zero"),
            (
                DiagnosticOccupancyBucketV1::UpTo25Percent,
                "up_to_25_percent",
            ),
            (
                DiagnosticOccupancyBucketV1::UpTo50Percent,
                "up_to_50_percent",
            ),
            (
                DiagnosticOccupancyBucketV1::UpTo75Percent,
                "up_to_75_percent",
            ),
            (
                DiagnosticOccupancyBucketV1::UpTo100Percent,
                "up_to_100_percent",
            ),
            (
                DiagnosticOccupancyBucketV1::Over100Percent,
                "over_100_percent",
            ),
        ];
        assert_eq!(
            *json_path(
                &schema,
                &[
                    "$defs",
                    "adoptionGroupAggregate",
                    "properties",
                    "reason",
                    "enum"
                ]
            ),
            json!(reasons.map(|(_, name)| name))
        );
        for property in ["refs_bucket", "bytes_bucket"] {
            assert_eq!(
                *json_path(
                    &schema,
                    &[
                        "$defs",
                        "adoptionGroupAggregate",
                        "properties",
                        property,
                        "enum"
                    ]
                ),
                json!(buckets.map(|(_, name)| name))
            );
        }
        for (reason, name) in reasons {
            for (bucket, bucket_name) in buckets {
                let record = event_json(
                    event(DiagnosticEventKindV1::AdoptionGroupAggregate {
                        span_id,
                        reason,
                        refs: bucket,
                        bytes: bucket,
                        groups: 4,
                        failed_groups: 1,
                    }),
                    2,
                )
                .expect("adoption event");
                assert_eq!(
                    *record.get("kind").expect("record kind"),
                    json!({"type":"adoption_group_aggregate", "span_id":7, "reason":name,
                        "refs_bucket":bucket_name, "bytes_bucket":bucket_name,
                        "groups":4, "failed_groups":1})
                );
            }
        }
        for (key, name) in [
            (DiagnosticCounterV1::WorkReads, "work_reads"),
            (DiagnosticCounterV1::WorkWrites, "work_writes"),
            (DiagnosticCounterV1::WorkBytes, "work_bytes"),
            (DiagnosticCounterV1::WorkMutations, "work_mutations"),
            (DiagnosticCounterV1::SqliteWriteCalls, "sqlite_write_calls"),
            (
                DiagnosticCounterV1::SqliteWriteRequestedBytes,
                "sqlite_write_requested_bytes",
            ),
            (
                DiagnosticCounterV1::SqliteWriteFailures,
                "sqlite_write_failures",
            ),
            (
                DiagnosticCounterV1::SqliteWalWriteCalls,
                "sqlite_wal_write_calls",
            ),
            (
                DiagnosticCounterV1::SqliteMainWriteCalls,
                "sqlite_main_write_calls",
            ),
            (
                DiagnosticCounterV1::SqliteOtherWriteCalls,
                "sqlite_other_write_calls",
            ),
            (
                DiagnosticCounterV1::SqliteSyncOverwrite,
                "sqlite_sync_overwrite",
            ),
            (DiagnosticCounterV1::SqliteSyncGrowth, "sqlite_sync_growth"),
            (
                DiagnosticCounterV1::SqliteSyncTruncate,
                "sqlite_sync_truncate",
            ),
            (DiagnosticCounterV1::SqliteSyncCreate, "sqlite_sync_create"),
            (
                DiagnosticCounterV1::SqliteSyncNoMutation,
                "sqlite_sync_no_mutation",
            ),
            (
                DiagnosticCounterV1::SqliteSyncUnclassified,
                "sqlite_sync_unclassified",
            ),
            (
                DiagnosticCounterV1::ManagedObjectInputs,
                "managed_object_inputs",
            ),
            (DiagnosticCounterV1::CompactedSegments, "compacted_segments"),
            (
                DiagnosticCounterV1::UniqueClosureRefs,
                "unique_closure_refs",
            ),
            (
                DiagnosticCounterV1::ClosureDependencyEdges,
                "closure_dependency_edges",
            ),
            (
                DiagnosticCounterV1::ClosureAuthenticatedBytes,
                "closure_authenticated_bytes",
            ),
        ] {
            let record = event_json(
                event(DiagnosticEventKindV1::Counter {
                    span_id,
                    key,
                    value: 9,
                }),
                3,
            )
            .expect("closure counter");
            assert_eq!(
                *record.get("kind").expect("record kind"),
                json!({"type":"counter", "span_id":7, "key":name, "value":9})
            );
            assert!(
                json_path(&schema, &["$defs", "counterKey", "enum"])
                    .as_array()
                    .expect("closed counter keys")
                    .contains(&json!(name))
            );
        }
    }

    #[test]
    fn capture_close_requires_lease_quiescence_and_writes_footer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trace.jsonl");
        let mut capture =
            DiagnosticCaptureV1::start(path.clone(), DiagnosticCaptureLimitsV1::default())
                .expect("start");
        let lease = capture.lease().expect("lease");
        let span = lease
            .context()
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))
            .expect("span");
        assert!(matches!(
            capture.close(Duration::from_millis(1)),
            Err(DiagnosticCaptureCloseErrorV1::PendingLeases(1))
        ));
        span.finish(DiagnosticTerminalV1::Ok).expect("finish");
        drop(lease);
        let closed = capture.close(Duration::from_secs(2)).expect("close");
        assert_eq!(closed.status, DiagnosticCaptureStatusV1::Complete);
        assert_eq!(closed.events_attempted, 2);
        assert_eq!(closed.events_written, 2);
        let text = std::fs::read_to_string(path).expect("read");
        let records: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        assert_eq!(records.len(), 4);
        assert_eq!(
            *records
                .first()
                .and_then(|record| record.get("record_type"))
                .expect("record field"),
            "header"
        );
        assert_eq!(
            *records
                .first()
                .and_then(|record| record.get("schema_version"))
                .expect("record field"),
            TIMELINE_SCHEMA_VERSION
        );
        assert_eq!(
            *records
                .get(1)
                .and_then(|record| record.get("output_seq"))
                .expect("record field"),
            1
        );
        assert_eq!(
            *records
                .get(2)
                .and_then(|record| record.get("output_seq"))
                .expect("record field"),
            2
        );
        assert_eq!(
            *records
                .get(3)
                .and_then(|record| record.get("schema_version"))
                .expect("record field"),
            TIMELINE_SCHEMA_VERSION
        );
        assert_eq!(
            *records
                .get(3)
                .and_then(|record| record.get("status"))
                .expect("record field"),
            "complete"
        );
    }

    #[test]
    fn duplicate_begin_or_end_without_begin_cannot_close_complete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let span_id = DiagnosticSpanIdV1::from_nonzero(NonZeroU64::new(1).expect("span id"));
        let begin = DiagnosticEventKindV1::SpanBegin {
            span_id,
            parent_span_id: None,
            root_kind: Some(DiagnosticRootKindV1::Operation),
            stage: DiagnosticStageV1::Operation,
            lane: DiagnosticLaneV1(0),
        };
        let end = DiagnosticEventKindV1::SpanEnd {
            span_id,
            outcome: DiagnosticTerminalV1::Ok,
        };
        for (name, kinds) in [
            ("duplicate-begin", vec![begin, begin, end]),
            ("reused-span-id", vec![begin, end, begin, end]),
            ("end-without-begin", vec![end]),
        ] {
            let path = dir.path().join(format!("{name}.jsonl"));
            let mut capture =
                DiagnosticCaptureV1::start(path.clone(), DiagnosticCaptureLimitsV1::default())
                    .expect("start");
            for kind in kinds {
                assert!(capture.sink.try_emit(DiagnosticEmitV1 {
                    capture_id: capture.capture_id(),
                    ts_ns: 0,
                    kind,
                }));
            }
            let closed = capture.close(Duration::from_secs(2)).expect("close");
            assert_eq!(closed.status, DiagnosticCaptureStatusV1::Invalid, "{name}");
            assert_eq!(closed.events_dropped, 0, "{name}");
            let text = std::fs::read_to_string(path).expect("read");
            let footer: Value =
                serde_json::from_str(text.lines().last().expect("footer")).expect("footer json");
            assert_eq!(
                *footer.get("status").expect("footer status"),
                "invalid",
                "{name}"
            );
        }
    }

    #[test]
    fn producer_accounting_overflow_invalidates_capture() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut capture = DiagnosticCaptureV1::start(
            dir.path().join("counter-overflow.jsonl"),
            DiagnosticCaptureLimitsV1::default(),
        )
        .expect("start");
        capture
            .sink
            .counts
            .attempted
            .store(u64::MAX, Ordering::Release);
        increment(
            &capture.sink.counts.attempted,
            &capture.sink.counts.overflow,
        );

        let closed = capture.close(Duration::from_secs(2)).expect("close");
        assert_eq!(closed.status, DiagnosticCaptureStatusV1::Invalid);
        assert_eq!(closed.events_attempted, u64::MAX);
        assert_eq!(closed.events_written, 0);
    }

    #[test]
    fn escaped_context_refuses_complete_close() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut capture = DiagnosticCaptureV1::start(
            dir.path().join("trace.jsonl"),
            DiagnosticCaptureLimitsV1::default(),
        )
        .expect("start");
        let lease = capture.lease().expect("lease");
        let escaped = lease.context().clone();
        drop(lease);
        assert!(matches!(
            capture.close(Duration::from_millis(1)),
            Err(DiagnosticCaptureCloseErrorV1::PendingContexts(1))
        ));
        drop(escaped);
        assert!(capture.close(Duration::from_secs(2)).is_ok());
    }

    #[test]
    fn idle_writer_flushes_accepted_events_before_close() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trace.jsonl");
        let mut capture =
            DiagnosticCaptureV1::start(path.clone(), DiagnosticCaptureLimitsV1::default())
                .expect("start");
        let lease = capture.lease().expect("lease");
        let span = lease
            .context()
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))
            .expect("span");
        span.finish(DiagnosticTerminalV1::Ok).expect("finish");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let text = std::fs::read_to_string(&path).expect("read live capture");
            if text.lines().count() >= 3 {
                assert!(!text.contains("\"record_type\":\"footer\""));
                break;
            }
            assert!(Instant::now() < deadline, "idle writer did not flush");
            thread::sleep(Duration::from_millis(10));
        }
        drop(lease);
        assert_eq!(
            capture.close(Duration::from_secs(2)).expect("close").status,
            DiagnosticCaptureStatusV1::Complete
        );
    }

    #[test]
    fn foreign_span_health_invalidates_footer_without_product_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trace.jsonl");
        let mut capture =
            DiagnosticCaptureV1::start(path.clone(), DiagnosticCaptureLimitsV1::default())
                .expect("start");
        let mut foreign_capture = DiagnosticCaptureV1::start(
            dir.path().join("foreign.jsonl"),
            DiagnosticCaptureLimitsV1::default(),
        )
        .expect("foreign start");
        let lease = capture.lease().expect("lease");
        let foreign_lease = foreign_capture.lease().expect("foreign lease");
        let foreign_span = foreign_lease
            .context()
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("foreign span");
        let child = lease.context().child_context(&foreign_span);
        assert!(
            child
                .enter(
                    quanta_index_core::DiagnosticStageV1::RuntimeBoot,
                    DiagnosticLaneV1(0)
                )
                .is_err()
        );
        drop(child);
        foreign_span
            .finish(DiagnosticTerminalV1::Ok)
            .expect("finish foreign span");
        drop(foreign_lease);
        drop(lease);
        let closed = capture.close(Duration::from_secs(2)).expect("close");
        assert_eq!(closed.status, DiagnosticCaptureStatusV1::Invalid);
        let text = std::fs::read_to_string(path).expect("read");
        assert!(text.contains("\"code\":\"foreign_capture_span\""));
        assert!(text.contains("\"status\":\"invalid\""));
        assert_eq!(
            foreign_capture
                .close(Duration::from_secs(2))
                .expect("foreign close")
                .status,
            DiagnosticCaptureStatusV1::Complete
        );
    }

    #[test]
    fn same_public_id_foreign_session_cannot_acquire_child_lease() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut capture = DiagnosticCaptureV1::start(
            dir.path().join("trace.jsonl"),
            DiagnosticCaptureLimitsV1::default(),
        )
        .expect("start");
        let lease = capture.lease().expect("lease");
        let foreign = DiagnosticContextV1::new(
            capture.capture_id(),
            Arc::new(FakeClock(AtomicU64::new(10))),
            Arc::new(NullSink),
        );
        let span = foreign
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("foreign span");
        assert!(lease.child_of(&span).is_none());
        assert_eq!(capture.leases.load(Ordering::Acquire), 1);
        drop(span);
        drop(lease);
        assert_eq!(
            capture.close(Duration::from_secs(2)).expect("close").status,
            DiagnosticCaptureStatusV1::Complete
        );
    }

    #[test]
    fn regressed_finish_keeps_footer_invalid_without_fabricated_end() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trace.jsonl");
        let mut capture = DiagnosticCaptureV1::start_with_clock_and_writer_hook(
            path.clone(),
            DiagnosticCaptureLimitsV1::default(),
            Arc::new(SequencedClock(AtomicUsize::new(0))),
            None,
        )
        .expect("start");
        let lease = capture.lease().expect("lease");
        let span = lease
            .context()
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("span");
        assert!(matches!(
            span.finish(DiagnosticTerminalV1::Ok),
            Err(quanta_index_core::DiagnosticTraceErrorV1::ClockRegressed)
        ));
        drop(lease);
        let closed = capture.close(Duration::from_secs(2)).expect("close");
        assert_eq!(closed.status, DiagnosticCaptureStatusV1::Invalid);
        let text = std::fs::read_to_string(path).expect("read");
        assert!(text.contains("\"code\":\"clock_regressed\""));
        assert!(!text.contains("\"type\":\"span_end\""));
    }

    #[test]
    fn repeated_counter_sum_overflow_invalidates_footer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut capture = DiagnosticCaptureV1::start(
            dir.path().join("trace.jsonl"),
            DiagnosticCaptureLimitsV1::default(),
        )
        .expect("start");
        let lease = capture.lease().expect("lease");
        let root = lease
            .context()
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))
            .expect("root");
        lease.context().counter(
            &root,
            quanta_index_core::DiagnosticCounterV1::FilesRead,
            u64::MAX,
        );
        lease
            .context()
            .counter(&root, quanta_index_core::DiagnosticCounterV1::FilesRead, 1);
        root.finish(DiagnosticTerminalV1::Ok).expect("finish");
        drop(lease);
        assert_eq!(
            capture.close(Duration::from_secs(2)).expect("close").status,
            DiagnosticCaptureStatusV1::Invalid
        );
    }

    #[test]
    fn output_cap_creates_truthful_partial_footer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let limits = DiagnosticCaptureLimitsV1 {
            queue_records: 1024,
            max_record_bytes: 512,
            max_output_bytes: 2560,
        };
        let mut capture =
            DiagnosticCaptureV1::start(dir.path().join("trace.jsonl"), limits).expect("start");
        let lease = capture.lease().expect("lease");
        for _ in 0..100 {
            let span = lease
                .context()
                .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))
                .expect("span");
            span.finish(DiagnosticTerminalV1::Ok).expect("finish");
        }
        drop(lease);
        let closed = capture.close(Duration::from_secs(2)).expect("close");
        assert_eq!(closed.status, DiagnosticCaptureStatusV1::Partial);
        assert!(closed.events_unwritten > 0);
        assert_eq!(
            closed.events_attempted,
            closed.events_accepted.saturating_add(closed.events_dropped)
        );
        assert_eq!(
            closed.events_accepted,
            closed
                .events_written
                .saturating_add(closed.events_unwritten)
        );
    }

    #[test]
    fn fake_clock_and_stalled_writer_preserve_retryable_close_barrier() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trace.jsonl");
        let clock = Arc::new(FakeClock(AtomicU64::new(10)));
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let mut capture = DiagnosticCaptureV1::start_with_clock_and_writer_hook(
            path.clone(),
            DiagnosticCaptureLimitsV1::default(),
            clock.clone(),
            Some(Box::new(move || {
                release_rx.recv().expect("release writer");
            })),
        )
        .expect("start");
        let lease = capture.lease().expect("lease");
        let span = lease
            .context()
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))
            .expect("span");
        clock.0.store(25, Ordering::Release);
        span.finish(DiagnosticTerminalV1::Ok).expect("finish");
        drop(lease);
        assert!(matches!(
            capture.close(Duration::from_millis(1)),
            Err(DiagnosticCaptureCloseErrorV1::WriterTimeout)
        ));
        assert!(capture.lease().is_none());
        release_tx.send(()).expect("release");
        let closed = capture.close(Duration::from_secs(2)).expect("retry close");
        assert_eq!(closed.status, DiagnosticCaptureStatusV1::Complete);
        let text = std::fs::read_to_string(path).expect("read");
        let records: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        assert_eq!(
            *records
                .get(1)
                .and_then(|record| record.get("ts_ns"))
                .expect("record field"),
            10
        );
        assert_eq!(
            *records
                .get(2)
                .and_then(|record| record.get("ts_ns"))
                .expect("record field"),
            25
        );
        assert_eq!(
            *records
                .get(3)
                .and_then(|record| record.get("closed_ns"))
                .expect("record field"),
            25
        );
    }

    #[test]
    fn full_queue_reports_loss_and_never_complete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let limits = DiagnosticCaptureLimitsV1 {
            queue_records: 1,
            ..DiagnosticCaptureLimitsV1::default()
        };
        let mut capture = DiagnosticCaptureV1::start_with_clock_and_writer_hook(
            dir.path().join("trace.jsonl"),
            limits,
            Arc::new(FakeClock(AtomicU64::new(1))),
            Some(Box::new(move || {
                release_rx.recv().expect("release writer");
            })),
        )
        .expect("start");
        let lease = capture.lease().expect("lease");
        let span = lease
            .context()
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))
            .expect("span");
        span.finish(DiagnosticTerminalV1::Ok).expect("finish");
        drop(lease);
        assert!(matches!(
            capture.close(Duration::from_millis(1)),
            Err(DiagnosticCaptureCloseErrorV1::WriterTimeout)
        ));
        release_tx.send(()).expect("release");
        let closed = capture.close(Duration::from_secs(2)).expect("retry close");
        assert_eq!(closed.status, DiagnosticCaptureStatusV1::Partial);
        assert_eq!(closed.events_attempted, 2);
        assert_eq!(closed.events_dropped, 1);
        assert_eq!(closed.events_accepted, 1);
    }

    #[test]
    fn dropped_health_event_still_invalidates_footer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trace.jsonl");
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let limits = DiagnosticCaptureLimitsV1 {
            queue_records: 1,
            ..DiagnosticCaptureLimitsV1::default()
        };
        let mut capture = DiagnosticCaptureV1::start_with_clock_and_writer_hook(
            path.clone(),
            limits,
            Arc::new(FakeClock(AtomicU64::new(1))),
            Some(Box::new(move || {
                release_rx.recv().expect("release writer");
            })),
        )
        .expect("start");
        let lease = capture.lease().expect("lease");
        let span = lease
            .context()
            .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))
            .expect("span");
        lease
            .context()
            .health(DiagnosticHealthCodeV1::ForeignCaptureSpan);
        span.finish(DiagnosticTerminalV1::Ok).expect("finish");
        drop(lease);
        release_tx.send(()).expect("release");
        let closed = capture.close(Duration::from_secs(2)).expect("close");
        assert_eq!(closed.status, DiagnosticCaptureStatusV1::Invalid);
        assert!(closed.events_dropped >= 1);
        assert!(
            !std::fs::read_to_string(path)
                .expect("read")
                .contains("foreign_capture_span")
        );
    }

    #[test]
    fn output_capped_health_event_still_invalidates_footer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("trace.jsonl");
        let limits = DiagnosticCaptureLimitsV1 {
            queue_records: 1024,
            max_record_bytes: 512,
            max_output_bytes: 2560,
        };
        let mut capture = DiagnosticCaptureV1::start(path.clone(), limits).expect("start");
        let lease = capture.lease().expect("lease");
        for _ in 0..100 {
            let span = lease
                .context()
                .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))
                .expect("span");
            span.finish(DiagnosticTerminalV1::Ok).expect("finish");
        }
        lease
            .context()
            .health(DiagnosticHealthCodeV1::CounterOverflow);
        drop(lease);
        let closed = capture.close(Duration::from_secs(2)).expect("close");
        assert_eq!(closed.status, DiagnosticCaptureStatusV1::Invalid);
        assert!(closed.events_unwritten > 0);
        assert!(
            !std::fs::read_to_string(path)
                .expect("read")
                .contains("counter_overflow")
        );
    }
}
