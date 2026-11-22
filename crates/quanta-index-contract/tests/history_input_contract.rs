//! Codec-only RED: it cannot establish source completeness, Catalog publication, or read correctness.
use quanta_index_contract::index::{InputDomain, UpdateKind};

const HISTORY_DOMAIN: &str = r#""History""#;
const HISTORY_ADVANCE: &str = r#"{"HistoryAdvance":{"instance":[3,3,3,3,3,3,3,3,3,3,3,3,3,3,3,3],"expected":null,"next":{"generation":{"repo_id":"r","revision_id":"v","generation":1},"durable_epoch":2,"snapshot_digest":[7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7,7],"replay_start":[8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8,8]},"input":[9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9,9],"profile":[10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10,10]}}"#;

#[test]
fn history_input_domain_has_closed_unit_wire() {
    let domain: InputDomain =
        serde_json::from_str(HISTORY_DOMAIN).expect("History domain must decode");
    assert_eq!(
        serde_json::to_value(&domain).expect("JSON encode"),
        serde_json::from_str::<serde_json::Value>(HISTORY_DOMAIN).expect("fixed gold")
    );
    let mut cbor = Vec::new();
    ciborium::ser::into_writer(&domain, &mut cbor).expect("CBOR encode");
    let decoded: InputDomain = ciborium::de::from_reader(cbor.as_slice()).expect("CBOR decode");
    assert_eq!(decoded, domain);
    for invalid in [r#""history""#, r#"{"History":{}}"#, r#"{"Unknown":null}"#] {
        assert!(
            serde_json::from_str::<InputDomain>(invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn history_advance_has_closed_cut_and_input_wire() {
    let gold: serde_json::Value = serde_json::from_str(HISTORY_ADVANCE).expect("fixed gold");
    let kind: UpdateKind =
        serde_json::from_str(HISTORY_ADVANCE).expect("HistoryAdvance must decode");
    assert_eq!(serde_json::to_value(&kind).expect("JSON encode"), gold);
    let mut cbor = Vec::new();
    ciborium::ser::into_writer(&kind, &mut cbor).expect("CBOR encode");
    let decoded: UpdateKind = ciborium::de::from_reader(cbor.as_slice()).expect("CBOR decode");
    assert_eq!(decoded, kind);

    for field in ["instance", "expected", "next", "input", "profile"] {
        let mut missing = gold.clone();
        assert!(
            missing["HistoryAdvance"]
                .as_object_mut()
                .expect("record")
                .remove(field)
                .is_some()
        );
        assert!(
            serde_json::from_value::<UpdateKind>(missing).is_err(),
            "missing {field}"
        );
    }
    for field in [
        "generation",
        "durable_epoch",
        "snapshot_digest",
        "replay_start",
    ] {
        let mut missing = gold.clone();
        assert!(
            missing["HistoryAdvance"]["next"]
                .as_object_mut()
                .expect("cut")
                .remove(field)
                .is_some()
        );
        assert!(
            serde_json::from_value::<UpdateKind>(missing).is_err(),
            "missing {field}"
        );
    }
    for path in ["snapshot_digest", "replay_start"] {
        let mut short = gold.clone();
        short["HistoryAdvance"]["next"][path] = serde_json::json!(vec![7_u8; 31]);
        assert!(
            serde_json::from_value::<UpdateKind>(short).is_err(),
            "short {path}"
        );
    }
    let mut short_input = gold.clone();
    short_input["HistoryAdvance"]["input"] = serde_json::json!(vec![9_u8; 31]);
    assert!(
        serde_json::from_value::<UpdateKind>(short_input).is_err(),
        "short input object reference"
    );
    let mut extra = gold;
    extra["HistoryAdvance"]["next"]["foreign"] = serde_json::json!(1);
    assert!(
        serde_json::from_value::<UpdateKind>(extra).is_err(),
        "unknown cut field"
    );
}

#[test]
fn history_append_has_distinct_closed_wire_without_next_or_ack() {
    let advance: serde_json::Value = serde_json::from_str(HISTORY_ADVANCE).expect("fixed gold");
    let append = serde_json::json!({
        "HistoryAppend": {
            "instance": advance["HistoryAdvance"]["instance"],
            "expected": advance["HistoryAdvance"]["next"],
            "input": advance["HistoryAdvance"]["input"],
            "profile": advance["HistoryAdvance"]["profile"]
        }
    });
    let kind: UpdateKind = serde_json::from_value(append.clone()).expect("append wire");
    assert_eq!(serde_json::to_value(&kind).expect("encode"), append);
    let mut cbor = Vec::new();
    ciborium::ser::into_writer(&kind, &mut cbor).expect("CBOR encode");
    let decoded: UpdateKind = ciborium::de::from_reader(cbor.as_slice()).expect("CBOR decode");
    assert_eq!(decoded, kind);
    for field in ["instance", "expected", "input", "profile"] {
        let mut missing = append.clone();
        assert!(
            missing["HistoryAppend"]
                .as_object_mut()
                .expect("append record")
                .remove(field)
                .is_some()
        );
        assert!(
            serde_json::from_value::<UpdateKind>(missing).is_err(),
            "missing {field}"
        );
    }
    for forbidden in ["next", "ack"] {
        let mut extra = append.clone();
        extra["HistoryAppend"][forbidden] = serde_json::json!(null);
        assert!(
            serde_json::from_value::<UpdateKind>(extra).is_err(),
            "foreign {forbidden}"
        );
    }
}

#[test]
fn history_input_context_matches_independent_framed_sha256_gold() {
    use quanta_index_contract::index::HistorySourceCutV1;
    let wire: serde_json::Value = serde_json::from_str(HISTORY_ADVANCE).expect("fixed gold");
    let cut: HistorySourceCutV1 =
        serde_json::from_value(wire["HistoryAdvance"]["next"].clone()).expect("fixed cut");
    let digest = cut.input_context_v1().expect("fixed context");
    let hex: String = digest.0.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(
        hex,
        "a85048f6a5e9182d36bbe6513a69dcd2ff11889fcf2769a6a84aaa5204d627b6"
    );
    for (field, value) in [
        ("durable_epoch", serde_json::json!(3)),
        ("snapshot_digest", serde_json::json!(vec![9_u8; 32])),
        ("replay_start", serde_json::json!(vec![10_u8; 32])),
    ] {
        let mut changed = wire["HistoryAdvance"]["next"].clone();
        changed[field] = value;
        let other: HistorySourceCutV1 = serde_json::from_value(changed).expect("changed cut");
        assert_ne!(other.input_context_v1().expect("changed context"), digest);
    }
    for (field, value) in [
        ("repo_id", serde_json::json!("r2")),
        ("revision_id", serde_json::json!("v2")),
        ("generation", serde_json::json!(2)),
    ] {
        let mut changed = wire["HistoryAdvance"]["next"].clone();
        changed["generation"][field] = value;
        let other: HistorySourceCutV1 =
            serde_json::from_value(changed).expect("changed generation");
        assert_ne!(other.input_context_v1().expect("changed context"), digest);
    }
}
