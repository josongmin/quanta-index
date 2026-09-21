use std::collections::BTreeSet;

use quanta_index_contract::{SearchPlaneErrorCodeV2, SearchPlaneIpcError, lex::LexicalErrorCode};

#[test]
fn all_is_unique_and_every_wire_code_roundtrips_exactly() {
    let mut wires = BTreeSet::new();
    for code in SearchPlaneErrorCodeV2::ALL {
        let wire = code.as_wire_str();
        assert!(wires.insert(wire), "duplicate wire code: {wire}");
        assert_eq!(SearchPlaneErrorCodeV2::from_wire_str(wire), Some(*code));

        let encoded = serde_json::to_string(code).expect("serialize code");
        assert_eq!(encoded, format!("\"{wire}\""));
        let decoded: SearchPlaneErrorCodeV2 =
            serde_json::from_str(&encoded).expect("deserialize accepted code");
        assert_eq!(decoded, *code);
    }
    assert_eq!(wires.len(), SearchPlaneErrorCodeV2::ALL.len());
}

#[test]
fn lexical_codes_are_nested_in_memory_and_flat_on_wire() {
    for lexical in LexicalErrorCode::ALL {
        let code = SearchPlaneErrorCodeV2::Lexical(*lexical);
        assert_eq!(code.as_wire_str(), lexical.as_code_str());
        assert_eq!(SearchPlaneErrorCodeV2::from_wire_str(lexical.as_code_str()), Some(code));
    }
}

#[test]
fn unknown_retired_and_non_exact_codes_fail_closed() {
    for refused in [
        "BAD_REQUEST",
        "UNKNOWN",
        "not_ready",
        "NOT_READY ",
        " NotReady",
        "",
    ] {
        assert_eq!(SearchPlaneErrorCodeV2::from_wire_str(refused), None);
        assert!(
            serde_json::from_str::<SearchPlaneErrorCodeV2>(&format!("\"{refused}\"")).is_err(),
            "decoder accepted {refused:?}"
        );
    }
}

#[test]
fn ipc_error_decode_rejects_unknown_and_bad_request_codes() {
    for refused in ["BAD_REQUEST", "UNKNOWN", "not_ready"] {
        let json = format!(r#"{{"code":"{refused}","message":"refused"}}"#);
        assert!(
            serde_json::from_str::<SearchPlaneIpcError>(&json).is_err(),
            "IPC decoder accepted {refused:?}"
        );
    }

    let accepted: SearchPlaneIpcError =
        serde_json::from_str(r#"{"code":"NOT_READY","message":"warming"}"#)
            .expect("accepted closed code");
    assert_eq!(accepted.code, SearchPlaneErrorCodeV2::NotReady);
}
