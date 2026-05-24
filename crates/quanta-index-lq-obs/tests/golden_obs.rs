//! Golden snapshot for OBS-01 wire shapes.
//!
//! Pins exactly one [`SpanEvent`], one [`MetricSample`], and one
//! [`AuditEntry`] to canonical JSON. Any drift in the manual serde impls
//! reveals itself here.

use quanta_index_lq_obs::audit::{AuditEntry, AuditOutcome};
use quanta_index_lq_obs::dim::Dimensions;
use quanta_index_lq_obs::metric::{MetricKind, MetricSample};
use quanta_index_lq_obs::span::{SpanEvent, SpanKind};

fn dims() -> Dimensions {
    Dimensions::new("OBS-01", "8", "tenant-1", "repo-x", 7)
}

#[test]
fn golden_span_event_json() {
    let e = SpanEvent::new(SpanKind::Plan, 1_000, 5, 42);
    let got = match serde_json::to_string(&e) {
        Ok(s) => s,
        Err(err) => {
            assert!(false, "serialize failed: {err}");
            return;
        }
    };
    let expected = "{\"kind\":\"plan\",\"start_ms\":1000,\"duration_ms\":5,\"error_code\":null,\"budget_remaining_ms\":42,\"attributes\":{}}";
    assert_eq!(got, expected);
}

#[test]
fn golden_metric_sample_json() {
    let s = MetricSample::new("lq_query_latency_ms", MetricKind::Histogram, 42.5, dims());
    let got = match serde_json::to_string(&s) {
        Ok(j) => j,
        Err(err) => {
            assert!(false, "serialize failed: {err}");
            return;
        }
    };
    let expected = "{\"name\":\"lq_query_latency_ms\",\"kind\":\"histogram\",\"value\":42.5,\"dimensions\":{\"ticket_id\":\"OBS-01\",\"wave_id\":\"8\",\"tenant_id\":\"tenant-1\",\"repo_id\":\"repo-x\",\"generation_id\":7}}";
    assert_eq!(got, expected);
}

#[test]
fn golden_audit_entry_json() {
    let e = AuditEntry::new(
        "tenant-1",
        "user-7",
        "query",
        "0xdeadbeef",
        AuditOutcome::Granted,
        1_700_000_000_000,
    );
    let got = match serde_json::to_string(&e) {
        Ok(s) => s,
        Err(err) => {
            assert!(false, "serialize failed: {err}");
            return;
        }
    };
    let expected = "{\"tenant_id\":\"tenant-1\",\"user_id\":\"user-7\",\"action\":\"query\",\"resource\":\"0xdeadbeef\",\"outcome\":\"granted\",\"at_ms\":1700000000000}";
    assert_eq!(got, expected);
}

#[test]
fn golden_roundtrip_all_three() {
    let span = SpanEvent::new(SpanKind::HybridMerge, 0, 0, 0);
    let metric = MetricSample::new("lq_query_total", MetricKind::Counter, 1.0, dims());
    let audit = AuditEntry::new("t1", "u1", "delete", "res-1", AuditOutcome::Denied, 0);

    for (label, ok) in [
        ("span", check_roundtrip(&span)),
        ("metric", check_roundtrip(&metric)),
        ("audit", check_roundtrip(&audit)),
    ] {
        assert!(ok, "{label} roundtrip failed");
    }
}

fn check_roundtrip<T>(v: &T) -> bool
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de> + PartialEq,
{
    let Ok(buf) = serde_json::to_vec(v) else {
        return false;
    };
    let Ok(back) = serde_json::from_slice::<T>(&buf) else {
        return false;
    };
    &back == v
}
