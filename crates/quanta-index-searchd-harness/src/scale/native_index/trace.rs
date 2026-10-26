//! Harness-only trace spans for the fixed Index-native scale phases.

use anyhow::Result;
use quanta_index_core::{
    DiagnosticContextV1, DiagnosticLaneV1, DiagnosticPhaseV1, DiagnosticRootKindV1,
    DiagnosticSpanGuardV1, DiagnosticStageV1, DiagnosticTerminalV1,
};
use quanta_index_searchd::DiagnosticCaptureLeaseV1;

/// Keeps the producer lease and native operation root alive through all phases.
/// The child daemon has a separate process and is not instrumented by this trace.
pub(super) struct NativeTrace {
    root: Option<DiagnosticSpanGuardV1>,
    context: Option<DiagnosticContextV1>,
    // Fields drop in declaration order: keep the counted producer alive until
    // the root emits Aborted and its child context is released on unwind.
    _lease: Option<DiagnosticCaptureLeaseV1>,
}

impl NativeTrace {
    pub(super) fn enabled(&self) -> bool {
        self.context.is_some()
    }

    pub(super) fn new(lease: Option<DiagnosticCaptureLeaseV1>) -> Self {
        let root = lease.as_ref().and_then(|lease| {
            let context = lease.context();
            let entered = if context.has_parent() {
                context.enter(DiagnosticStageV1::Operation, DiagnosticLaneV1(1))
            } else {
                context.enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(1))
            };
            super::super::optional_diagnostic_span(context, entered)
        });
        let context = lease
            .as_ref()
            .zip(root.as_ref())
            .map(|(lease, root)| lease.context().child_context(root));
        Self {
            _lease: lease,
            root,
            context,
        }
    }

    /// Start a span before product work. An unknown name leaves an aborted
    /// unlabelled span instead of inventing an identity or skipping the work.
    pub(super) fn begin(&self, name: &'static str) -> Option<DiagnosticSpanGuardV1> {
        let context = self.context.as_ref()?;
        let span = super::super::optional_diagnostic_span(
            context,
            context.enter(DiagnosticStageV1::HarnessPhase, DiagnosticLaneV1(1)),
        )?;
        let phase = phase_identity(name)?;
        context.phase_identity(&span, phase);
        Some(span)
    }

    /// Finish the observer span from the already computed product result.
    /// Tracing never replaces the original success or error.
    pub(super) fn finish_phase<T>(span: Option<DiagnosticSpanGuardV1>, result: &Result<T>) {
        if let Some(span) = span {
            let _finished = span.finish(DiagnosticTerminalV1::from_result(result));
        }
    }

    /// Seal the native operation root after cleanup. On unwind, guard Drop
    /// emits Aborted and the counted lease is still released.
    pub(super) fn finish<T>(mut self, result: &Result<T>) {
        if let Some(root) = self.root.take() {
            let _finished = root.finish(DiagnosticTerminalV1::from_result(result));
        }
    }
}

fn phase_identity(name: &str) -> Option<DiagnosticPhaseV1> {
    Some(match name {
        "corpus_generation" => DiagnosticPhaseV1::NativeCorpusGeneration,
        "config_setup" => DiagnosticPhaseV1::NativeConfigSetup,
        "startup" => DiagnosticPhaseV1::NativeStartup,
        "workspace_create" => DiagnosticPhaseV1::NativeWorkspaceCreate,
        "profile_configure" => DiagnosticPhaseV1::NativeProfileConfigure,
        "full_begin_input" => DiagnosticPhaseV1::NativeFullBeginInput,
        "full_stage_sources" => DiagnosticPhaseV1::NativeFullStageSources,
        "full_event_ack" => DiagnosticPhaseV1::NativeFullEventAck,
        "first_search" => DiagnosticPhaseV1::NativeFirstSearch,
        "delta_stage_source" => DiagnosticPhaseV1::NativeDeltaStageSource,
        "delta_event_ack" => DiagnosticPhaseV1::NativeDeltaEventAck,
        "delta_search" => DiagnosticPhaseV1::NativeDeltaSearch,
        "noop_event_ack" => DiagnosticPhaseV1::NativeNoopEventAck,
        "noop_stable_search" => DiagnosticPhaseV1::NativeNoopStableSearch,
        "delete_event_ack" => DiagnosticPhaseV1::NativeDeleteEventAck,
        "deleted_search" => DiagnosticPhaseV1::NativeDeletedSearch,
        "old_pin_search" => DiagnosticPhaseV1::NativeOldPinSearch,
        "churn_cycles" => DiagnosticPhaseV1::NativeChurnCycles,
        "churn_old_view_search" => DiagnosticPhaseV1::NativeChurnOldViewSearch,
        "drain" => DiagnosticPhaseV1::NativeDrain,
        "reopen" => DiagnosticPhaseV1::NativeReopen,
        "reopen_read_head" => DiagnosticPhaseV1::NativeReopenReadHead,
        "reopen_deleted_search" => DiagnosticPhaseV1::NativeReopenDeletedSearch,
        "reopen_churn_search" => DiagnosticPhaseV1::NativeReopenChurnSearch,
        "reopen_search" => DiagnosticPhaseV1::NativeReopenSearch,
        "final_drain" => DiagnosticPhaseV1::NativeFinalDrain,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;
    use quanta_index_searchd::{
        DiagnosticCaptureLimitsV1, DiagnosticCaptureStatusV1, DiagnosticCaptureV1,
    };
    use serde_json::{Value, json};
    use std::time::Duration;

    #[test]
    fn abandoned_native_operation_emits_aborted_and_releases_capture_lease() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let output = directory.path().join("abandoned-native.jsonl");
        let mut capture =
            DiagnosticCaptureV1::start(output.clone(), DiagnosticCaptureLimitsV1::default())?;
        {
            let lease = capture.lease().context("capture lease missing")?;
            let _abandoned = NativeTrace::new(Some(lease));
        }
        let closed = capture.close(Duration::from_secs(5))?;
        anyhow::ensure!(
            closed.status == DiagnosticCaptureStatusV1::Complete,
            "abandoned root leaked a producer"
        );
        anyhow::ensure!(
            closed.events_dropped == 0 && closed.events_unwritten == 0,
            "aborted terminal was lost"
        );
        let records: Vec<Value> = std::fs::read_to_string(output)?
            .lines()
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        let root_id = records
            .iter()
            .find(|row| {
                row.pointer("/kind/type") == Some(&json!("span_begin"))
                    && row.pointer("/kind/stage") == Some(&json!("operation"))
            })
            .and_then(|row| row.pointer("/kind/span_id"))
            .context("native operation root missing")?;
        anyhow::ensure!(
            records
                .iter()
                .any(|row| row.pointer("/kind/type") == Some(&json!("span_end"))
                    && row.pointer("/kind/span_id") == Some(root_id)
                    && row.pointer("/kind/outcome") == Some(&json!("aborted"))),
            "abandoned operation was promoted to success or omitted"
        );
        Ok(())
    }

    #[test]
    fn failed_native_work_keeps_original_error_and_exact_phase_terminal() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let output = directory.path().join("native-trace.jsonl");
        let mut capture =
            DiagnosticCaptureV1::start(output.clone(), DiagnosticCaptureLimitsV1::default())?;
        let lease = capture.lease().context("capture lease missing")?;
        let trace = NativeTrace::new(Some(lease));
        let span = trace.begin("full_event_ack");
        let product: Result<()> = Err(anyhow::anyhow!("fixed native work failure"));
        NativeTrace::finish_phase(span, &product);
        trace.finish(&product);
        anyhow::ensure!(
            product
                .err()
                .context("product failure was lost")?
                .to_string()
                == "fixed native work failure",
            "original product failure changed"
        );

        let closed = capture.close(Duration::from_secs(5))?;
        anyhow::ensure!(
            closed.status == DiagnosticCaptureStatusV1::Complete,
            "capture did not complete"
        );
        anyhow::ensure!(closed.events_dropped == 0, "capture dropped events");
        anyhow::ensure!(closed.events_unwritten == 0, "capture lost output events");
        let records: Vec<Value> = std::fs::read_to_string(output)?
            .lines()
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()?;
        let phase = records
            .iter()
            .find(|record| {
                record.pointer("/kind/type") == Some(&json!("phase_identity"))
                    && record.pointer("/kind/phase") == Some(&json!("native_full_event_ack"))
            })
            .context("exact native phase identity missing")?;
        let span_id = phase
            .pointer("/kind/span_id")
            .context("phase span ID absent")?;
        anyhow::ensure!(
            records.iter().any(|record| {
                record.pointer("/kind/type") == Some(&json!("span_end"))
                    && record.pointer("/kind/span_id") == Some(span_id)
                    && record.pointer("/kind/outcome") == Some(&json!("error"))
                    && record.pointer("/kind/error_code") == Some(&json!(1))
            }),
            "failed phase did not end with the original work error"
        );
        Ok(())
    }
}
