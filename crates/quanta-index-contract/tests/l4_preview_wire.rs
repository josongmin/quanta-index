//! Exercise the response codec used by IPC, including the symbol DTO boundary.
#![expect(
    clippy::expect_used,
    reason = "fixed wire fixture and codec assertions"
)]
use quanta_index_contract::{LexicalCandidate, SymbolCandidate};
use serde_json::{Value, json};

fn source_excerpt() -> Value {
    let digest = [3_u8; 32];
    let source = json!({
        "file": {"source_repo_id": "source", "repo_relative_path": "src/a.rs"},
        "revision_id": "source-r1", "source_sha256": digest,
    });
    json!({
        "source_repo_id": "source", "source": source,
        "preview": {
            "kind": "source_chunk", "source": source, "chunk_start_byte": 100,
            "original_focus": {"start": 13, "end": 19},
            "original_context": {"start": 10, "end": 19},
            "normalized_focus": {"start": 13, "end": 19},
            "normalization_equivalent": false, "unavailable_reason": null,
        },
        "candidate_id": "c", "repo_id": "container", "revision_id": "snapshot",
        "manifest_generation": 1, "repo_relative_path": "src/a.rs",
        "start_line": 1, "end_line": 1, "score": 1.0, "snippet": "é needle",
    })
}

fn cbor(value: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(value, &mut bytes).expect("encode wire map");
    bytes
}

#[test]
fn symbol_cbor_rejects_context_length_and_focus_boundary_mismatch() {
    let mut valid = source_excerpt();
    let fields = valid.as_object_mut().expect("map");
    let _kind = fields.insert("symbol_kind".into(), json!("function"));
    let _family = fields.insert("symbol_kind_family".into(), Value::Null);
    let row: SymbolCandidate =
        ciborium::de::from_reader(cbor(&valid).as_slice()).expect("valid excerpt");
    assert_eq!(row.snippet, "é needle");
    for (pointer, forged) in [
        ("/preview/original_context/end", json!(20)),
        ("/preview/original_focus/start", json!(11)),
        ("/snippet", json!("")),
    ] {
        let mut wire = valid.clone();
        *wire.pointer_mut(pointer).expect("field") = forged;
        assert!(ciborium::de::from_reader::<SymbolCandidate, _>(cbor(&wire).as_slice()).is_err());
    }
    let mut row = row;
    row.snippet.clear();
    assert!(ciborium::ser::into_writer(&row, Vec::new()).is_err());
}

#[test]
fn lexical_cbor_rejects_invalid_highlight_boundaries() {
    let mut valid = source_excerpt();
    let fields = valid.as_object_mut().expect("map");
    let _offset = fields.insert("snippet_hit_offset".into(), json!(3));
    let _highlights = fields.insert("highlights".into(), json!([{"start": 3, "len": 6}]));
    let row: LexicalCandidate =
        ciborium::de::from_reader(cbor(&valid).as_slice()).expect("valid excerpt");
    assert_eq!(row.snippet, "é needle");
    for (pointer, forged) in [
        ("/highlights/0/start", json!(1)),
        ("/highlights/0/len", json!(7)),
        ("/snippet_hit_offset", json!(4)),
    ] {
        let mut wire = valid.clone();
        *wire.pointer_mut(pointer).expect("field") = forged;
        assert!(ciborium::de::from_reader::<LexicalCandidate, _>(cbor(&wire).as_slice()).is_err());
    }
}
