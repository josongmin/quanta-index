//! Opt-in diagnostic timers for lexical durability calls.
//!
//! These timings cover each named synchronization call, including its userspace and
//! kernel wait. They do not establish physical device completion or crash
//! consistency. A profiling run must retain stderr and the matching source.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use quanta_index_core::{
    DiagnosticAdoptionFlushReasonV1, DiagnosticContextV1, DiagnosticCounterV1,
    DiagnosticHealthCodeV1, DiagnosticLaneV1, DiagnosticObjectKindV1, DiagnosticOccupancyBucketV1,
    DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticSyncLabelV1, DiagnosticTerminalV1,
};

/// Local to one retention measurement; concurrent object store users have
/// independent work accounting.
#[derive(Default)]
pub(crate) struct ClosureReadTotalsV1 {
    pub(crate) objects_read: u64,
    pub(crate) bytes_read: u64,
    pub(crate) objects_hashed: u64,
    pub(crate) bytes_hashed: u64,
    pub(crate) objects_decoded: u64,
    pub(crate) bytes_decoded: u64,
    pub(crate) read_elapsed_ns: u64,
    pub(crate) hash_elapsed_ns: u64,
    pub(crate) decode_elapsed_ns: u64,
    pub(crate) sqlite_elapsed_ns: u64,
    pub(crate) overflowed: u16,
}

impl ClosureReadTotalsV1 {
    pub(crate) const SQLITE_ELAPSED: u16 = 1 << 0;
    pub(crate) const READ_ELAPSED: u16 = 1 << 1;
    pub(crate) const HASH_ELAPSED: u16 = 1 << 2;
    pub(crate) const DECODE_ELAPSED: u16 = 1 << 3;
    pub(crate) const SQLITE_BATCHES: u16 = 1 << 4;
    pub(crate) const SQLITE_ROWS: u16 = 1 << 5;
    pub(crate) const OBJECTS_READ: u16 = 1 << 6;
    pub(crate) const BYTES_READ: u16 = 1 << 7;
    pub(crate) const OBJECTS_HASHED: u16 = 1 << 8;
    pub(crate) const BYTES_HASHED: u16 = 1 << 9;
    pub(crate) const OBJECTS_DECODED: u16 = 1 << 10;
    pub(crate) const BYTES_DECODED: u16 = 1 << 11;
    pub(crate) const UNIQUE_REFS: u16 = 1 << 12;
    pub(crate) const DEPENDENCY_EDGES: u16 = 1 << 13;
    pub(crate) const AUTHENTICATED_BYTES: u16 = 1 << 14;

    pub(crate) fn accumulate(
        total: &mut u64,
        increment: Option<u64>,
        overflowed: &mut u16,
        field: u16,
    ) {
        if *overflowed & field != 0 {
            return;
        }
        if let Some(next) = increment.and_then(|increment| total.checked_add(increment)) {
            *total = next;
        } else {
            *overflowed |= field;
        }
    }

    pub(crate) fn accumulate_width(
        total: &mut u64,
        value: usize,
        overflowed: &mut u16,
        field: u16,
    ) {
        match u64::try_from(value) {
            Ok(value) => Self::accumulate(total, Some(value), overflowed, field),
            Err(_overflow) => *overflowed |= field,
        }
    }

    pub(crate) fn accumulate_elapsed(
        total: &mut u64,
        started: Option<Instant>,
        overflowed: &mut u16,
        field: u16,
    ) {
        match started {
            Some(started) => match u64::try_from(started.elapsed().as_nanos()) {
                Ok(value) => Self::accumulate(total, Some(value), overflowed, field),
                Err(_overflow) => *overflowed |= field,
            },
            None => *overflowed |= field,
        }
    }

    pub(crate) fn publish_overflow_health(&self, diagnostic: &DiagnosticContextV1) {
        if self.overflowed != 0 {
            diagnostic.health(DiagnosticHealthCodeV1::CounterOverflow);
        }
    }

    pub(crate) fn publish_counter(
        &self,
        diagnostic: &DiagnosticContextV1,
        span: &quanta_index_core::DiagnosticSpanGuardV1,
        key: DiagnosticCounterV1,
        value: u64,
        field: u16,
    ) {
        if self.overflowed & field == 0 {
            diagnostic.counter(span, key, value);
        }
    }
}
/// Request-owned aggregate. It is passed only into one publication's local
/// storage handles and flushed once at the owner boundary.
#[derive(Debug, Default)]
pub(crate) struct SyncTotalsV1 {
    calls: AtomicU64,
    failures: AtomicU64,
    elapsed_ns: AtomicU64,
    new_objects: AtomicU64,
    reused_objects: AtomicU64,
    map_page_mutations: AtomicU64,
    posting_mutations: AtomicU64,
    compacted_documents: AtomicU64,
    compacted_bytes: AtomicU64,
    compacted_segments: AtomicU64,
    overflow: AtomicBool,
    labels: [SyncStat; DiagnosticSyncLabelV1::ALL.len()],
    object_kinds: [ObjectKindStat; DiagnosticObjectKindV1::ALL.len()],
    adoption: [[[AdoptionStat; 6]; 6]; 3],
}

#[derive(Debug, Default)]
struct ObjectKindStat {
    new_objects: AtomicU64,
    reused_objects: AtomicU64,
}

#[derive(Debug, Default)]
struct SyncStat {
    calls: AtomicU64,
    failures: AtomicU64,
    elapsed_sum_ns: AtomicU64,
    elapsed_max_ns: AtomicU64,
}

#[derive(Debug, Default)]
struct AdoptionStat {
    groups: AtomicU64,
    failed_groups: AtomicU64,
}

const REASONS: [DiagnosticAdoptionFlushReasonV1; 3] = [
    DiagnosticAdoptionFlushReasonV1::RefLimit,
    DiagnosticAdoptionFlushReasonV1::ByteLimit,
    DiagnosticAdoptionFlushReasonV1::Explicit,
];
const BUCKETS: [DiagnosticOccupancyBucketV1; 6] = [
    DiagnosticOccupancyBucketV1::Zero,
    DiagnosticOccupancyBucketV1::UpTo25Percent,
    DiagnosticOccupancyBucketV1::UpTo50Percent,
    DiagnosticOccupancyBucketV1::UpTo75Percent,
    DiagnosticOccupancyBucketV1::UpTo100Percent,
    DiagnosticOccupancyBucketV1::Over100Percent,
];

fn bucket_index(bucket: DiagnosticOccupancyBucketV1) -> usize {
    match bucket {
        DiagnosticOccupancyBucketV1::Zero => 0,
        DiagnosticOccupancyBucketV1::UpTo25Percent => 1,
        DiagnosticOccupancyBucketV1::UpTo50Percent => 2,
        DiagnosticOccupancyBucketV1::UpTo75Percent => 3,
        DiagnosticOccupancyBucketV1::UpTo100Percent => 4,
        DiagnosticOccupancyBucketV1::Over100Percent => 5,
    }
}

fn increment(counter: &AtomicU64, amount: u64, overflow: &AtomicBool) {
    let _previous = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        let result = current.checked_add(amount);
        if result.is_none() {
            overflow.store(true, Ordering::Relaxed);
        }
        result
    });
}

pub(crate) fn enter_span(
    context: &DiagnosticContextV1,
    stage: DiagnosticStageV1,
) -> Option<DiagnosticSpanGuardV1> {
    context.enter(stage, DiagnosticLaneV1(2)).map_or_else(
        |trace_error| {
            context.health(match trace_error {
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
            });
            None
        },
        Some,
    )
}

pub(crate) fn occupancy_bucket(value: u64, cap: u64) -> DiagnosticOccupancyBucketV1 {
    if value == 0 {
        DiagnosticOccupancyBucketV1::Zero
    } else if u128::from(value).saturating_mul(4) <= u128::from(cap) {
        DiagnosticOccupancyBucketV1::UpTo25Percent
    } else if u128::from(value).saturating_mul(2) <= u128::from(cap) {
        DiagnosticOccupancyBucketV1::UpTo50Percent
    } else if u128::from(value).saturating_mul(4) <= u128::from(cap).saturating_mul(3) {
        DiagnosticOccupancyBucketV1::UpTo75Percent
    } else if value <= cap {
        DiagnosticOccupancyBucketV1::UpTo100Percent
    } else {
        DiagnosticOccupancyBucketV1::Over100Percent
    }
}

impl SyncTotalsV1 {
    pub(crate) fn record_new_object(&self, kind: DiagnosticObjectKindV1) {
        increment(&self.new_objects, 1, &self.overflow);
        if let Some(stat) = self.object_kind_stat(kind) {
            increment(&stat.new_objects, 1, &self.overflow);
        } else {
            self.overflow.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_reused_object(&self, kind: DiagnosticObjectKindV1) {
        increment(&self.reused_objects, 1, &self.overflow);
        if let Some(stat) = self.object_kind_stat(kind) {
            increment(&stat.reused_objects, 1, &self.overflow);
        } else {
            self.overflow.store(true, Ordering::Relaxed);
        }
    }

    fn object_kind_stat(&self, kind: DiagnosticObjectKindV1) -> Option<&ObjectKindStat> {
        DiagnosticObjectKindV1::ALL
            .iter()
            .position(|candidate| *candidate == kind)
            .and_then(|index| self.object_kinds.get(index))
    }

    pub(crate) fn record_map_page_mutation(&self) {
        increment(&self.map_page_mutations, 1, &self.overflow);
    }

    pub(crate) fn record_posting_mutation(&self) {
        increment(&self.posting_mutations, 1, &self.overflow);
    }

    pub(crate) fn record_compaction(&self, segments: u64, documents: u64, bytes: u64) {
        increment(&self.compacted_segments, segments, &self.overflow);
        increment(&self.compacted_documents, documents, &self.overflow);
        increment(&self.compacted_bytes, bytes, &self.overflow);
    }

    pub(crate) fn record(
        &self,
        label: DiagnosticSyncLabelV1,
        elapsed_ns: Option<u64>,
        succeeded: bool,
    ) {
        let Some(stat) = DiagnosticSyncLabelV1::ALL
            .iter()
            .position(|candidate| *candidate == label)
            .and_then(|index| self.labels.get(index))
        else {
            self.overflow.store(true, Ordering::Relaxed);
            return;
        };
        increment(&self.calls, 1, &self.overflow);
        increment(&stat.calls, 1, &self.overflow);
        if !succeeded {
            increment(&self.failures, 1, &self.overflow);
            increment(&stat.failures, 1, &self.overflow);
        }
        if let Some(elapsed_ns) = elapsed_ns {
            increment(&self.elapsed_ns, elapsed_ns, &self.overflow);
            increment(&stat.elapsed_sum_ns, elapsed_ns, &self.overflow);
            let _previous = stat.elapsed_max_ns.fetch_max(elapsed_ns, Ordering::Relaxed);
        } else {
            self.overflow.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_adoption(
        &self,
        reason: DiagnosticAdoptionFlushReasonV1,
        refs: DiagnosticOccupancyBucketV1,
        bytes: DiagnosticOccupancyBucketV1,
        succeeded: bool,
    ) {
        let reason_index = REASONS.iter().position(|candidate| *candidate == reason);
        let refs_index = bucket_index(refs);
        let bytes_index = bucket_index(bytes);
        let Some(stat) = reason_index
            .and_then(|index| self.adoption.get(index))
            .and_then(|row| row.get(refs_index))
            .and_then(|column| column.get(bytes_index))
        else {
            self.overflow.store(true, Ordering::Relaxed);
            return;
        };
        increment(&stat.groups, 1, &self.overflow);
        if !succeeded {
            increment(&stat.failed_groups, 1, &self.overflow);
        }
    }

    pub(crate) fn emit(&self, context: &DiagnosticContextV1, span: &DiagnosticSpanGuardV1) {
        for (key, value) in [
            (
                DiagnosticCounterV1::NewObjects,
                self.new_objects.load(Ordering::Relaxed),
            ),
            (
                DiagnosticCounterV1::ReusedObjects,
                self.reused_objects.load(Ordering::Relaxed),
            ),
            (
                DiagnosticCounterV1::MapPageMutations,
                self.map_page_mutations.load(Ordering::Relaxed),
            ),
            (
                DiagnosticCounterV1::PostingMutations,
                self.posting_mutations.load(Ordering::Relaxed),
            ),
            (
                DiagnosticCounterV1::CompactedSegments,
                self.compacted_segments.load(Ordering::Relaxed),
            ),
            (
                DiagnosticCounterV1::CompactedDocuments,
                self.compacted_documents.load(Ordering::Relaxed),
            ),
            (
                DiagnosticCounterV1::CompactedBytes,
                self.compacted_bytes.load(Ordering::Relaxed),
            ),
        ] {
            context.counter(span, key, value);
        }
        for (kind, stat) in DiagnosticObjectKindV1::ALL
            .into_iter()
            .zip(self.object_kinds.iter())
        {
            let new_objects = stat.new_objects.load(Ordering::Relaxed);
            let reused_objects = stat.reused_objects.load(Ordering::Relaxed);
            if new_objects != 0 || reused_objects != 0 {
                context.object_kind_aggregate(span, kind, new_objects, reused_objects);
            }
        }
        context.counter(
            span,
            DiagnosticCounterV1::SyncCalls,
            self.calls.load(Ordering::Relaxed),
        );
        context.counter(
            span,
            DiagnosticCounterV1::SyncFailures,
            self.failures.load(Ordering::Relaxed),
        );
        context.counter(
            span,
            DiagnosticCounterV1::SyncElapsedNs,
            self.elapsed_ns.load(Ordering::Relaxed),
        );
        for (label, stat) in DiagnosticSyncLabelV1::ALL
            .into_iter()
            .zip(self.labels.iter())
        {
            let calls = stat.calls.load(Ordering::Relaxed);
            if calls != 0 {
                context.sync_aggregate(
                    span,
                    label,
                    calls,
                    stat.elapsed_sum_ns.load(Ordering::Relaxed),
                    stat.elapsed_max_ns.load(Ordering::Relaxed),
                    stat.failures.load(Ordering::Relaxed),
                );
            }
        }
        for (reason, reason_stats) in REASONS.into_iter().zip(self.adoption.iter()) {
            for (refs, ref_stats) in BUCKETS.into_iter().zip(reason_stats.iter()) {
                for (bytes, stat) in BUCKETS.into_iter().zip(ref_stats.iter()) {
                    let groups = stat.groups.load(Ordering::Relaxed);
                    if groups != 0 {
                        context.adoption_group_aggregate(
                            span,
                            reason,
                            refs,
                            bytes,
                            groups,
                            stat.failed_groups.load(Ordering::Relaxed),
                        );
                    }
                }
            }
        }
        if self.overflow.load(Ordering::Relaxed) {
            context.health(DiagnosticHealthCodeV1::CounterOverflow);
        }
    }
}

pub(crate) fn observed_sync(
    meter: Option<&SyncTotalsV1>,
    label: DiagnosticSyncLabelV1,
    operation: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let started = meter.map(|_| Instant::now());
    let result = operation();
    if let (Some(meter), Some(started)) = (meter, started) {
        match u64::try_from(started.elapsed().as_nanos()) {
            Ok(elapsed_ns) => meter.record(label, Some(elapsed_ns), result.is_ok()),
            Err(_overflow) => meter.record(label, None, result.is_ok()),
        }
    }
    result
}

/// Keep the strict legacy marker until its parser is migrated, while also
/// charging the request-local aggregate when a diagnostic context exists.
pub(crate) fn observed_sync_legacy(
    meter: Option<&SyncTotalsV1>,
    label: DiagnosticSyncLabelV1,
    operation: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    timed_sync_marker(label, || observed_sync(meter, label, operation))
}

/// Observe one owner operation with explicit custody. The guard records an
/// error even when the operation returns early; capture failure is fail-open.
pub(crate) fn observe_result<T, E>(
    diagnostic: Option<&DiagnosticContextV1>,
    stage: DiagnosticStageV1,
    operation: impl FnOnce(Option<&DiagnosticContextV1>) -> Result<T, E>,
) -> Result<T, E> {
    let span = diagnostic.and_then(|context| enter_span(context, stage));
    let child = diagnostic
        .zip(span.as_ref())
        .map(|(context, span)| context.child_context(span));
    let result = operation(child.as_ref());
    if let Some(span) = span {
        let outcome = DiagnosticTerminalV1::from_result(&result);
        let _finished = span.finish(outcome);
    }
    result
}

pub(crate) fn observe_result_with_sync<T, E>(
    diagnostic: Option<&DiagnosticContextV1>,
    stage: DiagnosticStageV1,
    operation: impl FnOnce(Option<&DiagnosticContextV1>, Option<Arc<SyncTotalsV1>>) -> Result<T, E>,
) -> Result<T, E> {
    let span = diagnostic.and_then(|context| enter_span(context, stage));
    let child = diagnostic
        .zip(span.as_ref())
        .map(|(context, span)| context.child_context(span));
    let sync = child.as_ref().map(|_| Arc::new(SyncTotalsV1::default()));
    let result = operation(child.as_ref(), sync.clone());
    if let (Some(context), Some(span)) = (diagnostic, span) {
        if let Some(sync) = sync {
            sync.emit(context, &span);
        }
        let outcome = DiagnosticTerminalV1::from_result(&result);
        let _finished = span.finish(outcome);
    }
    result
}

pub(crate) fn enabled() -> bool {
    std::env::var_os("QUANTA_INDEX_CAUSAL_PROFILE_V1")
        .is_some_and(|value| value == std::ffi::OsStr::new("1"))
}

#[expect(
    clippy::print_stderr,
    reason = "The opt-in causal capture contract consumes these diagnostic markers from stderr."
)]
fn timed_sync_marker(
    label: DiagnosticSyncLabelV1,
    operation: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let started = enabled().then(Instant::now);
    let result = operation();
    if let Some(started) = started {
        eprintln!(
            "QI_CAUSAL_V1 kind=sync label={} ok={} elapsed_ns={}",
            label.as_str(),
            u8::from(result.is_ok()),
            started.elapsed().as_nanos()
        );
    }
    result
}

/// Trace one ingest/proof span without changing its result.
#[expect(
    clippy::print_stderr,
    reason = "Opt-in ingest diagnosis writes source-bound span markers."
)]
pub(crate) fn timed_work<T, E>(
    label: &'static str,
    operation: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let started = enabled().then(Instant::now);
    let result = operation();
    if let Some(started) = started {
        eprintln!(
            "QI_INGEST_TRACE_V1 label={label} ok={} elapsed_ns={}",
            u8::from(result.is_ok()),
            started.elapsed().as_nanos()
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use quanta_index_core::DiagnosticSyncLabelV1;

    use super::SyncTotalsV1;

    #[test]
    fn unrepresentable_sync_elapsed_preserves_call_without_fabricating_duration() {
        let totals = SyncTotalsV1::default();
        let label = DiagnosticSyncLabelV1::PoolSyncAll;
        totals.record(label, Some(7), true);
        totals.record(label, None, false);

        let stat = DiagnosticSyncLabelV1::ALL
            .iter()
            .position(|candidate| *candidate == label)
            .and_then(|index| totals.labels.get(index))
            .expect("closed sync label has a stat slot");
        assert_eq!(totals.calls.load(Ordering::Relaxed), 2);
        assert_eq!(totals.failures.load(Ordering::Relaxed), 1);
        assert_eq!(stat.calls.load(Ordering::Relaxed), 2);
        assert_eq!(stat.failures.load(Ordering::Relaxed), 1);
        assert_eq!(totals.elapsed_ns.load(Ordering::Relaxed), 7);
        assert_eq!(stat.elapsed_sum_ns.load(Ordering::Relaxed), 7);
        assert_eq!(stat.elapsed_max_ns.load(Ordering::Relaxed), 7);
        assert!(totals.overflow.load(Ordering::Relaxed));
    }
}
