//! Native process capture custody; no product authority or retry policy.

use quanta_index_core::{
    DiagnosticContextV1, DiagnosticCounterV1, DiagnosticHealthCodeV1, DiagnosticLaneV1,
    DiagnosticRootKindV1, DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1,
};
use quanta_index_searchd::DiagnosticCaptureLeaseV1;
use quanta_index_storage::{ObjectIoObservation, ObjectIoSnapshot};

pub(super) struct NativeCapture {
    context: Option<DiagnosticContextV1>,
    root: Option<DiagnosticSpanGuardV1>,
    lease: DiagnosticCaptureLeaseV1,
}

impl NativeCapture {
    pub(super) fn new(lease: DiagnosticCaptureLeaseV1) -> Self {
        let context = lease.context();
        let root = crate::runtime_diagnostic_span(
            context,
            context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0)),
        );
        let context = root.as_ref().map(|root| context.child_context(root));
        Self {
            context,
            root,
            lease,
        }
    }

    pub(super) fn context(&self) -> Option<&DiagnosticContextV1> {
        self.context.as_ref()
    }

    /// Emit cumulative install IO after the service, pump and worker have
    /// drained. These counters exclude constructor, GC and Catalog/WAL IO.
    pub(super) fn record_object_io(&self, observer: &ObjectIoObservation) {
        let snapshot = match observer.snapshot() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let reason = match error {
                    quanta_index_core::index::IndexError::InvalidInput(_) => "not_quiescent",
                    quanta_index_core::index::IndexError::BackendViolation(_) => {
                        "invalid_accounting"
                    }
                    quanta_index_core::index::IndexError::Storage(_) => "observer_poisoned",
                    quanta_index_core::index::IndexError::FocusSubjectNotFound
                    | quanta_index_core::index::IndexError::Conflict
                    | quanta_index_core::index::IndexError::DigestConflict
                    | quanta_index_core::index::IndexError::FenceLost
                    | quanta_index_core::index::IndexError::WorkFenceLost
                    | quanta_index_core::index::IndexError::ResourceLimited
                    | quanta_index_core::index::IndexError::WorkLimitExceeded
                    | quanta_index_core::index::IndexError::Cancelled
                    | quanta_index_core::index::IndexError::SnapshotUnavailable
                    | quanta_index_core::index::IndexError::CorruptObject(_)
                    | quanta_index_core::index::IndexError::RebuildRequired
                    | quanta_index_core::index::IndexError::MaintenanceRequired => "snapshot_error",
                };
                self.log_object_io_unavailable(reason);
                return;
            }
        };
        let sync = snapshot
            .file_sync_calls
            .checked_add(snapshot.directory_sync_calls)
            .and_then(|calls| calls.checked_add(snapshot.barrier_calls))
            .zip(
                snapshot
                    .failed_file_syncs
                    .checked_add(snapshot.failed_directory_syncs)
                    .and_then(|failures| failures.checked_add(snapshot.failed_barriers)),
            )
            .zip(
                snapshot
                    .file_sync_elapsed
                    .checked_add(snapshot.directory_sync_elapsed)
                    .and_then(|elapsed| elapsed.checked_add(snapshot.barrier_elapsed))
                    .map(|elapsed| elapsed.as_nanos()),
            );
        let Some(((calls, failures), elapsed_ns)) = sync else {
            if let Some(context) = self.context() {
                context.health(DiagnosticHealthCodeV1::CounterOverflow);
            }
            self.log_object_io_unavailable("sync_aggregate_overflow");
            return;
        };
        let Ok(elapsed_ns) = u64::try_from(elapsed_ns) else {
            if let Some(context) = self.context() {
                context.health(DiagnosticHealthCodeV1::CounterOverflow);
            }
            self.log_object_io_unavailable("sync_elapsed_overflow");
            return;
        };
        if let (Some(context), Some(root)) = (self.context(), self.root.as_ref()) {
            context.counter(root, DiagnosticCounterV1::SyncCalls, calls);
            context.counter(root, DiagnosticCounterV1::SyncFailures, failures);
            context.counter(root, DiagnosticCounterV1::SyncElapsedNs, elapsed_ns);
        }
        self.log_object_io_snapshot(&snapshot);
    }

    fn capture_id_hex(&self) -> String {
        crate::native_work_diagnostic::capture_id_hex(self.lease.context().capture_id())
    }

    fn log_object_io_unavailable(&self, reason: &str) {
        use std::io::Write as _;

        let _written = writeln!(
            std::io::stderr().lock(),
            "native-object-io capture_id={} status=unavailable reason={reason}",
            self.capture_id_hex()
        );
    }

    fn log_object_io_snapshot(&self, snapshot: &ObjectIoSnapshot) {
        use std::io::Write as _;

        let _written = writeln!(
            std::io::stderr().lock(),
            "native-object-io capture_id={} status=quiescent write_calls={} written_bytes={} failed_writes={} enospc_writes={} file_sync_calls={} failed_file_syncs={} directory_sync_calls={} failed_directory_syncs={} barrier_calls={} failed_barriers={} reserve_calls={} reserve_objects={} single_object_reserves={} failed_reserves={} confirm_calls={} confirm_objects={} single_object_confirms={} failed_confirms={} write_elapsed_ns={} file_sync_elapsed_ns={} directory_sync_elapsed_ns={} barrier_elapsed_ns={} reserve_elapsed_ns={} confirm_elapsed_ns={}",
            self.capture_id_hex(),
            snapshot.write_calls,
            snapshot.written_bytes,
            snapshot.failed_writes,
            snapshot.enospc_writes,
            snapshot.file_sync_calls,
            snapshot.failed_file_syncs,
            snapshot.directory_sync_calls,
            snapshot.failed_directory_syncs,
            snapshot.barrier_calls,
            snapshot.failed_barriers,
            snapshot.reserve_calls,
            snapshot.reserve_objects,
            snapshot.single_object_reserves,
            snapshot.failed_reserves,
            snapshot.confirm_calls,
            snapshot.confirm_objects,
            snapshot.single_object_confirms,
            snapshot.failed_confirms,
            snapshot.write_elapsed.as_nanos(),
            snapshot.file_sync_elapsed.as_nanos(),
            snapshot.directory_sync_elapsed.as_nanos(),
            snapshot.barrier_elapsed.as_nanos(),
            snapshot.reserve_elapsed.as_nanos(),
            snapshot.confirm_elapsed.as_nanos(),
        );
    }

    pub(super) fn finish(mut self, result: &anyhow::Result<u8>) {
        if let Some(root) = self.root.take() {
            let terminal = match result {
                Ok(0) => DiagnosticTerminalV1::Ok,
                Ok(_) => DiagnosticTerminalV1::Aborted,
                Err(_) => DiagnosticTerminalV1::Error { code: 1 },
            };
            let _finished = root.finish(terminal);
        }
    }
}

pub(super) fn begin(
    context: Option<&DiagnosticContextV1>,
    stage: DiagnosticStageV1,
) -> Option<DiagnosticSpanGuardV1> {
    let context = context?;
    crate::runtime_diagnostic_span(context, context.enter(stage, DiagnosticLaneV1(0)))
}

pub(super) fn finish<T, E>(span: Option<DiagnosticSpanGuardV1>, result: &Result<T, E>) {
    if let Some(span) = span {
        let _finished = span.finish(DiagnosticTerminalV1::from_result(result));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context as _, Result};
    use quanta_index_searchd::{DiagnosticCaptureLimitsV1, DiagnosticCaptureV1};
    use serde_json::Value;
    use std::time::Duration;

    #[test]
    fn native_capture_lease_and_escaped_context_prevent_premature_close() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("native.jsonl");
        let mut recorder = DiagnosticCaptureV1::start(path, DiagnosticCaptureLimitsV1::default())?;
        let capture = NativeCapture::new(recorder.lease().context("capture lease absent")?);
        let escaped = capture.context().context("operation root absent")?.clone();
        anyhow::ensure!(recorder.close(Duration::from_millis(1)).is_err());
        capture.finish(&Ok(0));
        anyhow::ensure!(recorder.close(Duration::from_millis(1)).is_err());
        drop(escaped);
        let _closed = recorder.close(Duration::from_secs(2))?;
        Ok(())
    }

    #[test]
    fn nonzero_native_exit_never_emits_successful_operation() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("native.jsonl");
        let mut recorder =
            DiagnosticCaptureV1::start(path.clone(), DiagnosticCaptureLimitsV1::default())?;
        let capture = NativeCapture::new(recorder.lease().context("capture lease absent")?);
        capture.finish(&Ok(143));
        let _closed = recorder.close(Duration::from_secs(2))?;
        let rows: Vec<Value> = std::fs::read_to_string(path)?
            .lines()
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        anyhow::ensure!(
            rows.iter()
                .any(|row| row.pointer("/kind/outcome").and_then(Value::as_str) == Some("aborted"))
        );
        anyhow::ensure!(
            !rows
                .iter()
                .any(|row| row.pointer("/kind/outcome").and_then(Value::as_str) == Some("ok"))
        );
        Ok(())
    }
}
