//! Fixed-vocabulary observations, never service-side causal attribution.

use std::fmt::{self, Write as _};

use anyhow::Error;
use quanta_index_contract::native_protocol::NativeIndexErrorCode;
use quanta_index_sdk::native_uds::NativeUdsError;
use serde_json::{Value, json};

use super::QueryClass;

// The wire diagnostic limit is characters, including a multibyte UTF-8 scalar.
// Format directly into this bounded sink: formatting a whole Error and then
// truncating it retains an unbounded temporary allocation on the failure path.
const ERROR_CHARACTERS: usize = 512;

struct BoundedDiagnostic {
    text: String,
    remaining: usize,
}

impl fmt::Write for BoundedDiagnostic {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = value
            .char_indices()
            .nth(self.remaining)
            .map_or(value.len(), |(offset, _)| offset);
        let accepted = value.get(..end).ok_or(fmt::Error)?;
        self.text.push_str(accepted);
        self.remaining = self
            .remaining
            .checked_sub(accepted.chars().count())
            .ok_or(fmt::Error)?;
        if end == value.len() {
            Ok(())
        } else {
            Err(fmt::Error)
        }
    }
}

pub(super) fn bounded_diagnostic(arguments: fmt::Arguments<'_>) -> String {
    let mut output = BoundedDiagnostic {
        text: String::with_capacity(ERROR_CHARACTERS),
        remaining: ERROR_CHARACTERS,
    };
    // An exhausted sink returns fmt::Error to stop a streaming Display early.
    // A Display failure also leaves only its bounded prefix; this diagnostic
    // text never replaces the original typed error or controls failure priority.
    let _format_status = output.write_fmt(arguments);
    output.text
}

const CLASSES: [&str; 3] = ["unique", "broad_top20", "miss"];
const STAGES: [&str; 12] = [
    "deadline",
    "snapshot_capture",
    "unique_fixture",
    "unique_read_session_search",
    "unique_verify",
    "broad_fixture",
    "broad_read_session_search",
    "broad_verify",
    "miss_read_session_search",
    "miss_verify",
    "completion_deadline",
    "unclassified",
];
const CAUSES: [&str; 8] = [
    "local_rejection",
    "local_verification",
    "sdk_refused",
    "sdk_cancelled",
    "sdk_protocol",
    "sdk_transport",
    "sdk_peer_binding_refused",
    "unclassified",
];

#[derive(Default)]
pub(super) struct QueryFailures {
    counts: [[[u64; CAUSES.len()]; STAGES.len()]; CLASSES.len()],
    overflow: bool,
    total: u64,
}

// The SDK erases IpcError into Transport(String). Its display text cannot
// recover typed timeout/phase authority. Keep only the SDK variant; the first
// bounded error sample preserves the original text for inspection.
fn cause(stage: Option<&str>, error: &Error) -> &'static str {
    if let Some(error) = error.downcast_ref::<NativeUdsError>() {
        return match error {
            NativeUdsError::Refused(error) if error.code == NativeIndexErrorCode::Cancelled => {
                "sdk_cancelled"
            }
            NativeUdsError::Refused(_) => "sdk_refused",
            NativeUdsError::QueryRejected { .. } => "local_rejection",
            NativeUdsError::Protocol(_) => "sdk_protocol",
            NativeUdsError::Transport(_) => "sdk_transport",
            NativeUdsError::PeerBindingRefused(_) => "sdk_peer_binding_refused",
            NativeUdsError::Unknown(_) => "unclassified",
        };
    }
    match stage {
        Some(
            "deadline"
            | "snapshot_capture"
            | "unique_fixture"
            | "broad_fixture"
            | "completion_deadline",
        ) => "local_rejection",
        Some("unique_verify" | "broad_verify" | "miss_verify") => "local_verification",
        _ => "unclassified",
    }
}

impl QueryFailures {
    pub(super) fn record(&mut self, class: QueryClass, stage: Option<&str>, error: &Error) {
        let class = match class {
            QueryClass::Unique => 0,
            QueryClass::Broad => 1,
            QueryClass::Miss => 2,
        };
        // Unknown labels always consume a fixed bucket; they never allocate keys.
        let stage_index = STAGES
            .iter()
            .position(|name| Some(*name) == stage)
            .unwrap_or(STAGES.len().saturating_sub(1));
        let observed = cause(stage, error);
        let cause_index = CAUSES
            .iter()
            .position(|name| *name == observed)
            .unwrap_or(CAUSES.len().saturating_sub(1));
        let cell = self
            .counts
            .get_mut(class)
            .and_then(|rows| rows.get_mut(stage_index))
            .and_then(|row| row.get_mut(cause_index));
        if let Some(cell) = cell
            && let Some((next, total)) = cell.checked_add(1).zip(self.total.checked_add(1))
        {
            *cell = next;
            self.total = total;
            return;
        }
        self.overflow = true;
    }

    pub(super) fn detail(&self) -> Value {
        let mut rows = Vec::new();
        for (class, stages) in CLASSES.iter().zip(&self.counts) {
            for (stage, causes) in STAGES.iter().zip(stages) {
                for (cause, count) in CAUSES.iter().zip(causes) {
                    if *count != 0 {
                        rows.push(json!({"class":class,"stage":stage,"cause":cause,"count":count}));
                    }
                }
            }
        }
        json!({
            "scope":"completed query failures only; SDK boundary observations, not server root causes",
            "counter_overflow":self.overflow,
            "counted_failures":self.total,
            "rows":rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{QueryCompletion, QueryProgress};
    use super::*;
    use crate::offered_load::OfferedTask;
    use anyhow::{Context as _, Result, anyhow, ensure};
    use std::sync::atomic::Ordering;
    use std::time::Instant;

    #[derive(Debug)]
    struct StreamingError(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl fmt::Display for StreamingError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            for _ in 0..100_000 {
                let _previous = self.0.fetch_add(1, Ordering::Relaxed);
                formatter.write_str("🙂")?;
            }
            Ok(())
        }
    }

    impl std::error::Error for StreamingError {}

    #[test]
    fn bounded_error_formatting_stops_streaming_display_before_full_render() -> Result<()> {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let error = Error::new(StreamingError(std::sync::Arc::clone(&calls)));
        let text = bounded_diagnostic(format_args!("{error:#}"));
        ensure!(text == "🙂".repeat(512));
        ensure!(text.len() == 2048);
        ensure!(calls.load(Ordering::Relaxed) == 513);
        Ok(())
    }

    #[test]
    fn bounded_diagnostic_preserves_chain_and_utf8_across_format_chunks() -> Result<()> {
        let error = anyhow!("cause").context("context");
        ensure!(bounded_diagnostic(format_args!("{error:#}")) == "context: cause");
        let initial = "한".repeat(511);
        let text = bounded_diagnostic(format_args!("{initial}🙂end"));
        ensure!(text == format!("{initial}🙂"));
        ensure!(text.chars().count() == 512);
        ensure!(bounded_diagnostic(format_args!("")).is_empty());
        Ok(())
    }

    #[test]
    fn query_publisher_and_held_recorders_share_the_same_bounded_formatter() -> Result<()> {
        use super::super::{
            HeldWorkerProgress, PublisherProgress, record_held_failure, record_publisher_failure,
        };
        use std::sync::Mutex;
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let error = Error::new(StreamingError(std::sync::Arc::clone(&calls)));
        let publisher = Mutex::new(PublisherProgress::default());
        record_publisher_failure(&publisher, "snapshot_capture", &error);
        let held = Mutex::new(HeldWorkerProgress::default());
        record_held_failure(&held, &error);
        let query = QueryProgress::default();
        query.record_failure(
            OfferedTask {
                id: 1,
                scheduled: Instant::now(),
            },
            &QueryCompletion {
                class: QueryClass::Unique,
                latency_ms: 1.0,
                queue_ms: 0.0,
                failure_stage: Some("unique_read_session_search"),
                result: Err(error),
            },
        );
        let publisher_error = publisher
            .lock()
            .map_err(|error| anyhow!("publisher test lock poisoned: {error}"))?
            .failure_error
            .clone();
        let held_error = held
            .lock()
            .map_err(|error| anyhow!("held test lock poisoned: {error}"))?
            .failure_error
            .clone();
        let query = query.detail();
        for text in [
            publisher_error.as_deref(),
            held_error.as_deref(),
            query
                .pointer("/first_failure/error")
                .and_then(Value::as_str),
        ] {
            ensure!(text.context("bounded diagnostic absent")? == "🙂".repeat(512));
        }
        ensure!(calls.load(Ordering::Relaxed) == 1539);
        Ok(())
    }

    #[test]
    fn interrupted_query_progress_keeps_completed_failures_separate_from_inflight() -> Result<()> {
        let progress = QueryProgress::default();
        let task = OfferedTask {
            id: 1,
            scheduled: Instant::now(),
        };
        progress.dispatched.store(3, Ordering::Release);
        for (class, stage) in [
            (QueryClass::Unique, "deadline"),
            (QueryClass::Miss, "miss_verify"),
        ] {
            progress.record_failure(
                task,
                &QueryCompletion {
                    class,
                    latency_ms: 1.0,
                    queue_ms: 0.0,
                    failure_stage: Some(stage),
                    result: Err(anyhow!("observed failure")),
                },
            );
        }
        progress.completed.store(2, Ordering::Release);
        progress.failed.store(2, Ordering::Release);
        let detail = progress.detail();
        ensure!(detail.get("dispatched") == Some(&json!(3)));
        ensure!(detail.get("completed") == Some(&json!(2)));
        ensure!(detail.get("failed") == Some(&json!(2)));
        ensure!(detail.pointer("/failure_counts/counted_failures") == Some(&json!(2)));
        ensure!(detail.pointer("/failure_counts/counter_overflow") == Some(&json!(false)));
        Ok(())
    }

    #[test]
    fn failure_distribution_is_bounded_and_does_not_copy_first_sample() -> Result<()> {
        let mut counts = QueryFailures::default();
        counts.record(
            QueryClass::Broad,
            Some("broad_read_session_search"),
            &NativeUdsError::Transport("ipc Read timed out after 1231 ms".into()).into(),
        );
        counts.record(QueryClass::Unique, Some("deadline"), &anyhow!("expired"));
        counts.record(
            QueryClass::Miss,
            Some("miss_verify"),
            &anyhow!("unexpected hit"),
        );
        for _ in 0..10_000 {
            counts.record(
                QueryClass::Miss,
                Some("unbounded arbitrary label"),
                &anyhow!("unknown"),
            );
        }
        ensure!(
            counts.detail().get("rows")
                == Some(&json!([
                    {"class":"unique","stage":"deadline","cause":"local_rejection","count":1},
                    {"class":"broad_top20","stage":"broad_read_session_search","cause":"sdk_transport","count":1},
                    {"class":"miss","stage":"miss_verify","cause":"local_verification","count":1},
                    {"class":"miss","stage":"unclassified","cause":"unclassified","count":10000},
                ]))
        );
        Ok(())
    }

    #[test]
    fn transport_classification_never_parses_error_display_text() {
        for message in [
            "ipc Connect timed out after 1 ms",
            "ipc Read timed out after 1 ms",
            "ipc Write timed out after 1 ms",
            "ipc Read timed out after 1 ms extra",
            "ipc Read timed out after -1 ms",
            "wrapped: ipc Read timed out after 1 ms",
            "ipc Read timed out after  ms",
            "connection reset",
            "",
        ] {
            assert_eq!(
                cause(
                    Some("miss_read_session_search"),
                    &NativeUdsError::Transport(message.into()).into()
                ),
                "sdk_transport"
            );
        }
        assert_eq!(
            cause(None, &anyhow!("ipc Read timed out after 1 ms")),
            "unclassified"
        );
        assert_eq!(
            cause(None, &NativeUdsError::Protocol("timeout".into()).into()),
            "sdk_protocol"
        );
    }

    #[test]
    fn sdk_labels_do_not_invent_server_causality() {
        use quanta_index_contract::native_protocol::NativeIndexWireError;
        let error = Error::new(NativeUdsError::QueryRejected {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                quanta_index_contract::lex::LexicalErrorCode::ParseFail,
            ),
            message: "unterminated phrase".into(),
        });
        assert_eq!(
            cause(Some("miss_read_session_search"), &error),
            "local_rejection"
        );
        for (code, expected) in [
            (NativeIndexErrorCode::Overloaded, "sdk_refused"),
            (NativeIndexErrorCode::Cancelled, "sdk_cancelled"),
        ] {
            let error = Error::new(NativeUdsError::Refused(NativeIndexWireError {
                code,
                message: "ipc Read timed out after 1 ms".into(),
            }))
            .context("retained search");
            assert_eq!(cause(Some("miss_read_session_search"), &error), expected);
        }
        let error = Error::new(NativeUdsError::Transport("arbitrary display".into()))
            .context("ipc Read timed out after 1 ms");
        assert_eq!(
            cause(Some("miss_read_session_search"), &error),
            "sdk_transport"
        );
    }

    #[test]
    fn peer_binding_refusal_keeps_its_bounded_sdk_cause() {
        let error = Error::new(NativeUdsError::PeerBindingRefused(
            quanta_index_ipc::PeerBindingRefusalV1::ProcessExited,
        ))
        .context("ipc Read timed out after 1 ms");
        let mut counts = QueryFailures::default();
        counts.record(QueryClass::Broad, Some("broad_read_session_search"), &error);
        assert_eq!(
            counts.detail(),
            json!({
                "scope": "completed query failures only; SDK boundary observations, not server root causes",
                "counter_overflow": false,
                "counted_failures": 1,
                "rows": [{
                    "class": "broad_top20",
                    "stage": "broad_read_session_search",
                    "cause": "sdk_peer_binding_refused",
                    "count": 1,
                }],
            })
        );
    }

    #[test]
    fn counter_overflow_is_explicit_and_never_wraps() -> Result<()> {
        let mut counts = QueryFailures::default();
        let cell = counts
            .counts
            .first_mut()
            .and_then(|v| v.first_mut())
            .and_then(|v| v.first_mut())
            .context("counter fixture absent")?;
        *cell = u64::MAX;
        counts.record(QueryClass::Unique, Some("deadline"), &anyhow!("expired"));
        ensure!(counts.detail().get("counter_overflow") == Some(&json!(true)));
        ensure!(counts.detail().pointer("/rows/0/count") == Some(&json!(u64::MAX)));
        let mut total_overflow = QueryFailures {
            total: u64::MAX,
            ..QueryFailures::default()
        };
        total_overflow.record(
            QueryClass::Miss,
            Some("miss_verify"),
            &anyhow!("unexpected hit"),
        );
        ensure!(total_overflow.detail().get("counter_overflow") == Some(&json!(true)));
        ensure!(total_overflow.detail().get("rows") == Some(&json!([])));
        Ok(())
    }
}
