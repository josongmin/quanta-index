use super::*;

use quanta_index_core::{
    DiagnosticCaptureIdV1, DiagnosticEmitV1, DiagnosticEventKindV1, DiagnosticLaneV1,
    DiagnosticOwnedRenewalRefusalReasonV1, DiagnosticOwnedRenewalRefusalV1, DiagnosticRootKindV1,
    DiagnosticSpanIdV1, DiagnosticStageV1, DiagnosticTerminalV1,
};
use serde_json::{Value, json};
use std::{error::Error, num::NonZeroU64, time::Duration};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

fn require(condition: bool, reason: &'static str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(reason).into())
    }
}

fn refusal_event(
    span_id: DiagnosticSpanIdV1,
    observation: DiagnosticOwnedRenewalRefusalV1,
) -> DiagnosticEmitV1 {
    DiagnosticEmitV1 {
        capture_id: DiagnosticCaptureIdV1([3; 16]),
        ts_ns: 11,
        kind: DiagnosticEventKindV1::OwnedRenewalRefusal {
            span_id,
            observation,
        },
    }
}

#[test]
fn owned_renewal_refusal_wire_has_fixed_names_nulls_and_bounds() -> TestResult {
    use DiagnosticOwnedRenewalRefusalReasonV1 as Reason;

    let span_id = DiagnosticSpanIdV1::from_nonzero(
        NonZeroU64::new(7).ok_or("fixed span id must be nonzero")?,
    );
    for (reason, name) in [
        (Reason::MissingClaim, "missing_claim"),
        (Reason::NotClaimed, "not_claimed"),
        (Reason::ClaimIdentity, "claim_identity"),
        (Reason::ClaimFence, "claim_fence"),
        (Reason::LeaseExpired, "lease_expired"),
        (Reason::MissingAssignment, "missing_assignment"),
        (Reason::InactiveAssignment, "inactive_assignment"),
        (Reason::UndispatchedAssignment, "undispatched_assignment"),
        (Reason::ExecutionExpired, "execution_expired"),
        (Reason::AssignmentClaim, "assignment_claim"),
        (Reason::GrantMismatch, "grant_mismatch"),
    ] {
        let observation = DiagnosticOwnedRenewalRefusalV1 {
            reason,
            now_ms: 100,
            lease_until_ms: (reason == Reason::LeaseExpired).then_some(100),
            deadline_ms: (reason == Reason::ExecutionExpired).then_some(100),
        };
        let record = super::super::event_json(refusal_event(span_id, observation), 1)
            .ok_or("valid refusal omitted by serializer")?;
        require(
            record.pointer("/kind/reason").and_then(Value::as_str) == Some(name),
            "refusal reason wire name changed",
        )?;
        require(
            record.pointer("/kind/span_id").and_then(Value::as_u64) == Some(7),
            "refusal lost owner span",
        )?;
    }

    let missing = DiagnosticOwnedRenewalRefusalV1 {
        reason: Reason::MissingClaim,
        now_ms: 100,
        lease_until_ms: None,
        deadline_ms: None,
    };
    let missing = super::super::wire_event_json(refusal_event(span_id, missing), 1)
        .ok_or("missing-claim refusal omitted")?;
    require(
        serde_json::to_string(&missing)?
            == r#"["e",1,"03030303030303030303030303030303",11,{"deadline_ms":null,"lease_until_ms":null,"now_ms":100,"reason":"missing_claim","span_id":7,"type":"owned_renewal_refusal"}]"#,
        "nullable refusal wire bytes changed",
    )?;

    for (reason, lease_until_ms, deadline_ms, expected) in [
        (
            Reason::LeaseExpired,
            Some(100),
            None,
            r#"["e",2,"03030303030303030303030303030303",11,{"deadline_ms":null,"lease_until_ms":100,"now_ms":100,"reason":"lease_expired","span_id":7,"type":"owned_renewal_refusal"}]"#,
        ),
        (
            Reason::ExecutionExpired,
            None,
            Some(100),
            r#"["e",2,"03030303030303030303030303030303",11,{"deadline_ms":100,"lease_until_ms":null,"now_ms":100,"reason":"execution_expired","span_id":7,"type":"owned_renewal_refusal"}]"#,
        ),
    ] {
        let observation = DiagnosticOwnedRenewalRefusalV1 {
            reason,
            now_ms: 100,
            lease_until_ms,
            deadline_ms,
        };
        let record = super::super::wire_event_json(refusal_event(span_id, observation), 2)
            .ok_or("bound refusal omitted")?;
        require(
            serde_json::to_string(&record)? == expected,
            "bound refusal wire bytes changed",
        )?;
    }

    for observation in [
        DiagnosticOwnedRenewalRefusalV1 {
            reason: Reason::LeaseExpired,
            now_ms: 99,
            lease_until_ms: Some(100),
            deadline_ms: None,
        },
        DiagnosticOwnedRenewalRefusalV1 {
            reason: Reason::LeaseExpired,
            now_ms: 100,
            lease_until_ms: None,
            deadline_ms: None,
        },
        DiagnosticOwnedRenewalRefusalV1 {
            reason: Reason::ExecutionExpired,
            now_ms: 99,
            lease_until_ms: None,
            deadline_ms: Some(100),
        },
        DiagnosticOwnedRenewalRefusalV1 {
            reason: Reason::ExecutionExpired,
            now_ms: 100,
            lease_until_ms: None,
            deadline_ms: None,
        },
    ] {
        require(
            super::super::event_json(refusal_event(span_id, observation), 3).is_none(),
            "inconsistent expiry refusal was serialized",
        )?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum CaptureCase {
    Single,
    Duplicate,
    ClosedOwner,
    SuccessfulOwner,
}

fn capture_refusal(case: CaptureCase) -> TestResult<(DiagnosticCaptureStatusV1, Vec<Value>, u64)> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("renewal.qtrace");
    let mut capture =
        DiagnosticCaptureV1::start(path.clone(), DiagnosticCaptureLimitsV1::default())?;
    let lease = capture.lease().ok_or("capture lease absent")?;
    let root = lease
        .context()
        .enter_root(DiagnosticRootKindV1::Operation, DiagnosticLaneV1(0))?;
    let parent = lease.context().child_context(&root);
    let renewal = parent.enter(DiagnosticStageV1::SqliteWork, DiagnosticLaneV1(29))?;
    let renewal_id = renewal.span_id().get();
    let child = parent.child_context(&renewal);
    let observation = DiagnosticOwnedRenewalRefusalV1 {
        reason: DiagnosticOwnedRenewalRefusalReasonV1::ClaimFence,
        now_ms: 200,
        lease_until_ms: Some(250),
        deadline_ms: Some(300),
    };
    match case {
        CaptureCase::Single => {
            child.owned_renewal_refusal(observation);
            renewal.finish(DiagnosticTerminalV1::Error { code: 1 })?;
        }
        CaptureCase::Duplicate => {
            child.owned_renewal_refusal(observation);
            child.owned_renewal_refusal(observation);
            renewal.finish(DiagnosticTerminalV1::Error { code: 1 })?;
        }
        CaptureCase::ClosedOwner => {
            renewal.finish(DiagnosticTerminalV1::Error { code: 1 })?;
            child.owned_renewal_refusal(observation);
        }
        CaptureCase::SuccessfulOwner => {
            child.owned_renewal_refusal(observation);
            renewal.finish(DiagnosticTerminalV1::Ok)?;
        }
    }
    drop(child);
    drop(parent);
    root.finish(DiagnosticTerminalV1::Ok)?;
    drop(lease);
    let closed = capture.close(Duration::from_secs(2))?;
    let records = std::fs::read_to_string(path)?
        .lines()
        .map(|line| {
            let raw = serde_json::from_str(line)?;
            Ok(crate::trace_wire::normalize_record(
                super::super::TIMELINE_SCHEMA_VERSION,
                raw,
            )?)
        })
        .collect::<TestResult<Vec<Value>>>()?;
    Ok((closed.status, records, renewal_id))
}

#[test]
fn owned_renewal_refusal_is_parented_and_balanced_in_complete_capture() -> TestResult {
    let (status, records, renewal_id) = capture_refusal(CaptureCase::Single)?;
    require(
        status == DiagnosticCaptureStatusV1::Complete,
        "single refusal capture incomplete",
    )?;
    let begin = records.iter().position(|record| {
        record.pointer("/kind/type").and_then(Value::as_str) == Some("span_begin")
            && record.pointer("/kind/span_id").and_then(Value::as_u64) == Some(renewal_id)
            && record.pointer("/kind/lane_id").and_then(Value::as_u64) == Some(29)
    });
    let refusal = records.iter().position(|record| {
        record.pointer("/kind/type").and_then(Value::as_str) == Some("owned_renewal_refusal")
            && record.pointer("/kind/span_id").and_then(Value::as_u64) == Some(renewal_id)
            && record.pointer("/kind").is_some_and(|kind| {
                kind == &json!({
                    "type": "owned_renewal_refusal", "span_id": renewal_id,
                    "reason": "claim_fence", "now_ms": 200,
                    "lease_until_ms": 250, "deadline_ms": 300,
                })
            })
    });
    let end = records.iter().position(|record| {
        record.pointer("/kind/type").and_then(Value::as_str) == Some("span_end")
            && record.pointer("/kind/span_id").and_then(Value::as_u64) == Some(renewal_id)
            && record.pointer("/kind/outcome").and_then(Value::as_str) == Some("error")
    });
    require(
        matches!((begin, refusal, end), (Some(begin), Some(refusal), Some(end)) if begin < refusal && refusal < end),
        "refusal event lacks live renewal owner or terminal order",
    )
}

#[test]
fn owned_renewal_refusal_rejects_duplicate_and_closed_owner() -> TestResult {
    for case in [
        CaptureCase::Duplicate,
        CaptureCase::ClosedOwner,
        CaptureCase::SuccessfulOwner,
    ] {
        let (status, _records, _renewal_id) = capture_refusal(case)?;
        require(
            status == DiagnosticCaptureStatusV1::Invalid,
            "duplicate, closed-owner or successful-owner refusal was accepted",
        )?;
    }
    Ok(())
}
