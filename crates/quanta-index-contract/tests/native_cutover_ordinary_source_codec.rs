//! Syntax custody only: these facts do not grant publication or recovery authority.
use quanta_index_contract::native_cutover_evidence_v1::NativeCutoverEvidenceSourceFactV1;
use serde_json::{Value, json};

fn replay(binding: Value) -> Value {
    json!({"Replay": {
        "workspace_root": "/workspace",
        "contract_key": "contract",
        "aggregate_identity_digest": "aggregate",
        "closeout_id": "ordinary",
        "attempt_id": "attempt",
        "format_version": 4,
        "state_revision": 1,
        "phase": "Acked",
        "native_update_request": null,
        "published_snapshot_id": 42,
        "prepared_artifact_digest": null,
        "source_bound_aggregate_receipt_digest": null,
        "terminal_aggregate_receipt_digest": "terminal",
        "terminal_projection_receipt_digest": "projection",
        "checkpoint_ack": null,
        "ordinary_source_binding": binding,
        "created_at_epoch_ms": 1,
        "updated_at_epoch_ms": 2
    }})
}

fn binding() -> Value {
    json!({
        "transition_digest": vec![b'a'; 64],
        "completion_pointer_digest": vec![b'b'; 64],
        "physical_target_source_digest": vec![b'c'; 64],
        "target_snapshot_id": 42
    })
}

#[test]
fn ordinary_source_replay_preserves_fixed_binding_in_json_and_cbor() {
    let gold = replay(binding());
    let decoded: NativeCutoverEvidenceSourceFactV1 =
        serde_json::from_value(gold.clone()).expect("producer's exact ordinary Source binding");
    assert_eq!(serde_json::to_value(&decoded).expect("JSON encode"), gold);
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(&decoded, &mut bytes).expect("CBOR encode");
    let roundtrip: NativeCutoverEvidenceSourceFactV1 =
        ciborium::de::from_reader(bytes.as_slice()).expect("CBOR decode");
    assert_eq!(roundtrip, decoded);
    assert_eq!(serde_json::to_value(roundtrip).expect("JSON encode"), gold);
}

#[test]
fn ordinary_source_replay_preserves_explicit_absence_without_inference() {
    let gold = replay(Value::Null);
    let decoded: NativeCutoverEvidenceSourceFactV1 =
        serde_json::from_value(gold.clone()).expect("explicitly unbound replay");
    assert_eq!(serde_json::to_value(decoded).expect("JSON encode"), gold);
}

#[test]
fn ordinary_source_binding_refuses_missing_extra_short_and_wrong_typed_fields() {
    for field in [
        "transition_digest",
        "completion_pointer_digest",
        "physical_target_source_digest",
        "target_snapshot_id",
    ] {
        let mut incomplete = binding();
        assert!(
            incomplete
                .as_object_mut()
                .expect("fixed object")
                .remove(field)
                .is_some()
        );
        assert!(
            serde_json::from_value::<NativeCutoverEvidenceSourceFactV1>(replay(incomplete))
                .is_err()
        );
    }
    let mut extra = binding();
    extra["untrusted_authority"] = json!(true);
    assert!(serde_json::from_value::<NativeCutoverEvidenceSourceFactV1>(replay(extra)).is_err());
    for field in [
        "transition_digest",
        "completion_pointer_digest",
        "physical_target_source_digest",
    ] {
        let mut short = binding();
        short[field] = json!(vec![b'a'; 63]);
        assert!(
            serde_json::from_value::<NativeCutoverEvidenceSourceFactV1>(replay(short)).is_err()
        );
    }
    let mut wrong = binding();
    wrong["target_snapshot_id"] = json!("42");
    assert!(serde_json::from_value::<NativeCutoverEvidenceSourceFactV1>(replay(wrong)).is_err());
}

#[test]
fn ordinary_source_replay_requires_explicit_binding_field() {
    let mut missing = replay(Value::Null);
    assert!(
        missing["Replay"]
            .as_object_mut()
            .expect("fixed object")
            .remove("ordinary_source_binding")
            .is_some()
    );
    assert!(serde_json::from_value::<NativeCutoverEvidenceSourceFactV1>(missing).is_err());
}
