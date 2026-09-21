//! PRE-CONTRACT-EXT scaffold tests for the canonical `lex::*` types.
//!
//! Covers:
//! * variant cardinality (every enum exposes exactly the spec'd variants)
//! * unique-code-string round-trips per enum
//! * CBOR encode/decode fixed-point per record type
//! * fail-closed deserialization for out-of-set enum values + missing fields
//!
//! Test signature follows the workspace convention
//! (`fn _ () -> Result<(), Box<dyn std::error::Error>>`) so we can use `?`
//! propagation without violating the `unwrap_used` / `expect_used` /
//! `panic` clippy lints.

#![forbid(unsafe_code)]

use quanta_index_contract::lex::{
    CommitRecord, CommitSha, CommitShaParseError, DirtyRecord, ExplanationRow, LanguageCode,
    LexicalErrorCode, ParseNode, ParseTreeRecord, SearchExplanation, SymbolKindCode, SymbolRecord,
    SymbolRelationship, SymbolSpan,
};
use quanta_index_contract::{ChunkId, RepoRelativePath, SymbolId};

type TestRes = Result<(), Box<dyn std::error::Error>>;

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buf: Vec<u8> = Vec::new();
    ciborium::ser::into_writer(value, &mut buf)?;
    Ok(buf)
}

fn decode<T>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>>
where
    T: for<'de> serde::Deserialize<'de>,
{
    Ok(ciborium::de::from_reader(bytes)?)
}

fn roundtrip_eq<T>(value: &T) -> TestRes
where
    T: serde::Serialize + for<'de> serde::Deserialize<'de> + core::fmt::Debug + PartialEq,
{
    let bytes = encode(value)?;
    let decoded: T = decode(&bytes)?;
    if &decoded != value {
        return Err(format!("round-trip mismatch: original={value:?}, decoded={decoded:?}",).into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// LanguageCode
// ---------------------------------------------------------------------------

#[test]
fn language_code_accepts_canonical_examples() -> TestRes {
    for code in ["rust", "python", "typescript", "javascript", "go", "c++23"] {
        let parsed = LanguageCode::new(code).map_err(str::to_string)?;
        if parsed.as_str() != code {
            return Err(format!("expected {code}, got {}", parsed.as_str()).into());
        }
    }
    Ok(())
}

#[test]
fn language_code_rejects_non_canonical_forms() -> TestRes {
    for bad in ["", "Rust", "RUST", "c#", "-rust", "rust lang"] {
        if LanguageCode::from_code_str(bad).is_some() {
            return Err(format!("from_code_str({bad}) accepted invalid language code").into());
        }
    }
    Ok(())
}

#[test]
fn language_code_cbor_roundtrip_examples() -> TestRes {
    for code in ["rust", "python", "cpp", "ruby3"] {
        let value = LanguageCode::new(code).map_err(str::to_string)?;
        roundtrip_eq(&value)?;
    }
    Ok(())
}

#[test]
fn language_code_cbor_rejects_unknown_string_shape() -> TestRes {
    let bytes = encode(&"Rust")?;
    let result: Result<LanguageCode, _> = ciborium::de::from_reader(bytes.as_slice());
    if result.is_ok() {
        return Err("non-canonical language code must fail-closed on the wire".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// LexicalErrorCode
// ---------------------------------------------------------------------------

#[test]
fn lexical_error_code_all_is_at_least_30() -> TestRes {
    // Spec requires "30+ SCREAMING_SNAKE_CASE variants". Keep the live
    // contract surface above that floor.
    if LexicalErrorCode::ALL.len() < 30 {
        return Err(format!("expected >= 30 variants, got {}", LexicalErrorCode::ALL.len()).into());
    }
    Ok(())
}

#[test]
fn lexical_error_code_code_strings_unique() -> TestRes {
    let mut seen: Vec<&'static str> = Vec::with_capacity(LexicalErrorCode::ALL.len());
    for variant in LexicalErrorCode::ALL {
        let code = variant.as_code_str();
        if seen.contains(&code) {
            return Err(format!("duplicate code_str: {code}").into());
        }
        seen.push(code);
    }
    Ok(())
}

#[test]
fn lexical_error_code_all_codes_screaming_snake_case() -> TestRes {
    // Per-char check: only A-Z, 0-9, '_' allowed; must be non-empty.
    for variant in LexicalErrorCode::ALL {
        let code = variant.as_code_str();
        if code.is_empty() {
            return Err(format!("variant {variant:?} has empty code_str").into());
        }
        for (idx, byte) in code.as_bytes().iter().copied().enumerate() {
            let ok = byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_';
            if !ok {
                return Err(format!(
                    "variant {variant:?} code_str byte at {idx} is not SCREAMING_SNAKE_CASE: 0x{byte:02x}",
                )
                .into());
            }
        }
    }
    Ok(())
}

#[test]
fn lexical_error_code_from_code_str_roundtrips() -> TestRes {
    for variant in LexicalErrorCode::ALL {
        let code = variant.as_code_str();
        let parsed = LexicalErrorCode::from_code_str(code);
        if parsed != Some(*variant) {
            return Err(format!("from_code_str({code}) != Some({variant:?})").into());
        }
    }
    Ok(())
}

#[test]
fn lexical_error_code_cbor_roundtrip_every_variant() -> TestRes {
    for variant in LexicalErrorCode::ALL {
        roundtrip_eq(variant)?;
    }
    Ok(())
}

#[test]
fn lexical_error_code_cbor_rejects_unknown() -> TestRes {
    let bytes = encode(&"PARSE_DOES_NOT_EXIST")?;
    let result: Result<LexicalErrorCode, _> = ciborium::de::from_reader(bytes.as_slice());
    if result.is_ok() {
        return Err("unknown code must fail-closed on the wire".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// SymbolKindCode / SymbolRelationship
// ---------------------------------------------------------------------------

#[test]
fn symbol_kind_code_accepts_canonical_examples() -> TestRes {
    for code in ["function", "method", "type_alias", "module"] {
        let parsed = SymbolKindCode::new(code).map_err(str::to_string)?;
        if parsed.as_str() != code {
            return Err(format!("expected {code}, got {}", parsed.as_str()).into());
        }
    }
    Ok(())
}

#[test]
fn symbol_kind_code_rejects_non_canonical_forms() -> TestRes {
    for bad in [
        "",
        "Function",
        "type-alias",
        "_hidden",
        "123kind",
        "bad kind",
        "http_handler2",
        "service",
    ] {
        if SymbolKindCode::from_code_str(bad).is_some() {
            return Err(format!("from_code_str({bad}) accepted invalid symbol kind").into());
        }
    }
    Ok(())
}

#[test]
fn symbol_kind_code_cbor_roundtrip_examples() -> TestRes {
    for code in ["function", "method", "type_alias", "variable"] {
        let kind = SymbolKindCode::new(code).map_err(str::to_string)?;
        roundtrip_eq(&kind)?;
    }
    Ok(())
}

#[test]
fn symbol_relationship_two_variants_and_roundtrip() -> TestRes {
    if SymbolRelationship::ALL.len() != 2 {
        return Err(format!("expected 2 variants, got {}", SymbolRelationship::ALL.len()).into());
    }
    for variant in SymbolRelationship::ALL {
        roundtrip_eq(variant)?;
    }
    Ok(())
}

#[test]
fn symbol_relationship_rejects_unknown() -> TestRes {
    if SymbolRelationship::from_code_str("Maybe").is_some() {
        return Err("unknown relationship must be rejected".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// SymbolSpan / SymbolRecord
// ---------------------------------------------------------------------------

#[test]
fn symbol_span_cbor_roundtrip() -> TestRes {
    let span = SymbolSpan {
        path: Box::from("crates/foo/src/lib.rs"),
        byte_start: 12,
        byte_end: 64,
        line_start: 3,
        line_end: 5,
    };
    roundtrip_eq(&span)
}

#[test]
fn symbol_record_cbor_roundtrip_with_parent_and_container() -> TestRes {
    let record = SymbolRecord {
        symbol_id: SymbolId::new("sym-compute"),
        repo_relative_path: RepoRelativePath::new("crates/foo/src/lib.rs"),
        language: LanguageCode::new("rust").map_err(str::to_string)?,
        symbol_kind: SymbolKindCode::new("function").map_err(str::to_string)?,
        symbol_kind_family: None,
        local_name: Box::from("compute"),
        qualified_name: Box::from("foo::math::compute"),
        signature: Some(Box::from("fn compute(x: i32) -> i32")),
        visibility: None,
        definition_span: SymbolSpan {
            path: Box::from("crates/foo/src/lib.rs"),
            byte_start: 0,
            byte_end: 42,
            line_start: 1,
            line_end: 8,
        },
        container_qualified_name: Some(Box::from("foo::math")),
        relationship: SymbolRelationship::Def,
    };
    roundtrip_eq(&record)
}

#[test]
fn symbol_record_cbor_roundtrip_with_none_optionals() -> TestRes {
    let record = SymbolRecord {
        symbol_id: SymbolId::new("sym-top-level"),
        repo_relative_path: RepoRelativePath::new("crates/foo/src/lib.rs"),
        language: LanguageCode::new("rust").map_err(str::to_string)?,
        symbol_kind: SymbolKindCode::new("constant").map_err(str::to_string)?,
        symbol_kind_family: None,
        local_name: Box::from("TOP_LEVEL"),
        qualified_name: Box::from("TOP_LEVEL"),
        signature: None,
        visibility: None,
        definition_span: SymbolSpan {
            path: Box::from("crates/foo/src/lib.rs"),
            byte_start: 100,
            byte_end: 110,
            line_start: 10,
            line_end: 10,
        },
        container_qualified_name: None,
        relationship: SymbolRelationship::Def,
    };
    roundtrip_eq(&record)
}

// ---------------------------------------------------------------------------
// CommitSha + CommitRecord
// ---------------------------------------------------------------------------

#[test]
fn commit_sha_from_hex_round_trips_canonical() -> TestRes {
    let hex = "0123456789abcdef0123456789abcdef01234567";
    let sha = CommitSha::from_hex(hex).map_err(|e| e.to_string())?;
    if sha.to_hex() != hex {
        return Err(format!("expected {hex}, got {}", sha.to_hex()).into());
    }
    roundtrip_eq(&sha)
}

#[test]
fn commit_sha_from_hex_accepts_uppercase_normalizes_lowercase() -> TestRes {
    let upper = "0123456789ABCDEF0123456789ABCDEF01234567";
    let lower = "0123456789abcdef0123456789abcdef01234567";
    let sha = CommitSha::from_hex(upper).map_err(|e| e.to_string())?;
    if sha.to_hex() != lower {
        return Err(format!("expected lower-hex {lower}, got {}", sha.to_hex()).into());
    }
    Ok(())
}

#[test]
fn commit_sha_from_hex_rejects_bad_length() -> TestRes {
    let result = CommitSha::from_hex("0123");
    match result {
        Err(CommitShaParseError::BadLength { observed: 4 }) => Ok(()),
        Err(other) => Err(format!("expected BadLength, got {other:?}").into()),
        Ok(_) => Err("expected length error".into()),
    }
}

#[test]
fn commit_sha_from_hex_rejects_non_hex() -> TestRes {
    let bad = "Z123456789abcdef0123456789abcdef01234567";
    let result = CommitSha::from_hex(bad);
    match result {
        Err(CommitShaParseError::NonHexChar { position: 0 }) => Ok(()),
        Err(other) => Err(format!("expected NonHexChar, got {other:?}").into()),
        Ok(_) => Err("expected non-hex error".into()),
    }
}

#[test]
fn commit_record_cbor_roundtrip() -> TestRes {
    let sha = CommitSha::from_hex("0123456789abcdef0123456789abcdef01234567")
        .map_err(|e| e.to_string())?;
    let parent = CommitSha::from_hex("abcdef0123456789abcdef0123456789abcdef01")
        .map_err(|e| e.to_string())?;
    let record = CommitRecord {
        wire_version: 1,
        sha,
        parents: vec![parent],
        // QI-LXB-01: triple of author / committer / applied timestamps.
        author_time_ms: 1_699_999_900_000,
        committer_time_ms: 1_699_999_999_000,
        applied_at_ms: 1_700_000_000_000,
        author: Box::from("Alice <a@example.com>"),
        author_name: Some(Box::from("Alice")),
        author_email: Some(Box::from("a@example.com")),
        committer: Box::from("Bob <b@example.com>"),
        committer_name: Some(Box::from("Bob")),
        committer_email: Some(Box::from("b@example.com")),
        message: Box::from("Initial commit"),
        is_merge: false,
        tags: vec![Box::from("v0.1.0")],
    };
    roundtrip_eq(&record)
}

#[test]
fn commit_record_cbor_rejects_missing_field() -> TestRes {
    // Encode a map missing `wire_version` and assert decode fails.
    let mut buf: Vec<u8> = Vec::new();
    ciborium::ser::into_writer(
        &ciborium::Value::Map(vec![
            (
                ciborium::Value::Text("sha".into()),
                ciborium::Value::Text("0123456789abcdef0123456789abcdef01234567".into()),
            ),
            (ciborium::Value::Text("parents".into()), ciborium::Value::Array(Vec::new())),
            (
                ciborium::Value::Text("applied_at_ms".into()),
                ciborium::Value::Integer(0i64.into()),
            ),
            (ciborium::Value::Text("author".into()), ciborium::Value::Text("a".into())),
            (ciborium::Value::Text("committer".into()), ciborium::Value::Text("c".into())),
            (ciborium::Value::Text("message".into()), ciborium::Value::Text("m".into())),
            (ciborium::Value::Text("is_merge".into()), ciborium::Value::Bool(false)),
            (ciborium::Value::Text("tags".into()), ciborium::Value::Array(Vec::new())),
        ]),
        &mut buf,
    )?;
    let result: Result<CommitRecord, _> = ciborium::de::from_reader(buf.as_slice());
    if result.is_ok() {
        return Err("missing wire_version must fail-closed".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// DirtyRecord
// ---------------------------------------------------------------------------

#[test]
fn dirty_record_cbor_roundtrip() -> TestRes {
    let mut hash = [0u8; 32];
    for (idx, slot) in hash.iter_mut().enumerate() {
        // saturating cast: idx <= 31 < 256, but we use `u8::try_from` to
        // satisfy the `as_conversions` lint.
        let byte: u8 = u8::try_from(idx & 0xff)?;
        *slot = byte;
    }
    let record = DirtyRecord {
        wire_version: 1,
        doc_id: ChunkId::new("chunk-42"),
        applied_at_ms: 1_700_000_000_000,
        payload_hash: hash,
    };
    roundtrip_eq(&record)
}

// ---------------------------------------------------------------------------
// ParseTreeRecord / ParseNode (recursive)
// ---------------------------------------------------------------------------

#[test]
fn parse_tree_record_cbor_roundtrip_flat() -> TestRes {
    let record = ParseTreeRecord {
        wire_version: 1,
        lang: LanguageCode::new("rust").map_err(str::to_string)?,
        root: ParseNode {
            kind: Box::from("source_file"),
            byte_start: 0,
            byte_end: 64,
            children: Vec::new(),
        },
        source_hash: [7u8; 32],
        // QI-LXB-01: structural role-tag schema is keyed on the parse tree;
        // empty tags for the flat roundtrip case.
        role_tag_schema_version: 0,
        role_tags: Vec::new(),
    };
    roundtrip_eq(&record)
}

#[test]
fn parse_tree_record_cbor_roundtrip_nested() -> TestRes {
    let leaf_a = ParseNode {
        kind: Box::from("identifier"),
        byte_start: 4,
        byte_end: 8,
        children: Vec::new(),
    };
    let leaf_b = ParseNode {
        kind: Box::from("string_literal"),
        byte_start: 12,
        byte_end: 20,
        children: Vec::new(),
    };
    let branch = ParseNode {
        kind: Box::from("expression"),
        byte_start: 4,
        byte_end: 20,
        children: vec![leaf_a, leaf_b],
    };
    let record = ParseTreeRecord {
        wire_version: 1,
        lang: LanguageCode::new("python").map_err(str::to_string)?,
        root: ParseNode {
            kind: Box::from("module"),
            byte_start: 0,
            byte_end: 32,
            children: vec![branch],
        },
        source_hash: [0xaa; 32],
        // QI-LXB-01: empty role tags for the nested-tree roundtrip case.
        role_tag_schema_version: 0,
        role_tags: Vec::new(),
    };
    roundtrip_eq(&record)
}

// ---------------------------------------------------------------------------
// SearchExplanation / ExplanationRow
// ---------------------------------------------------------------------------

#[test]
fn explanation_row_cbor_roundtrip() -> TestRes {
    let row = ExplanationRow {
        signal_name: Box::from("bm25"),
        signal_value: 1.25_f32,
        weight: 0.75_f32,
        contribution: 0.9375_f32,
    };
    roundtrip_eq(&row)
}

#[test]
fn search_explanation_cbor_roundtrip() -> TestRes {
    let explanation = SearchExplanation {
        planner_trace: Vec::new(),
        engines_touched: Vec::new(),
        early_stop_reason: None,
        contributions: vec![
            ExplanationRow {
                signal_name: Box::from("bm25"),
                signal_value: 1.25_f32,
                weight: 0.75_f32,
                contribution: 0.9375_f32,
            },
            ExplanationRow {
                signal_name: Box::from("symbol_match"),
                signal_value: 1.0_f32,
                weight: 0.25_f32,
                contribution: 0.25_f32,
            },
        ],
        ranker_weights_hash: [0xcc; 32],
        strategy: "default-v1".to_owned(),
        summary: String::new(),
    };
    roundtrip_eq(&explanation)
}

// QI-BB-025: the route-independent `top_k` codes live in the closed registry,
// so a wire decoder can resolve them to typed variants like any other code,
// and the former per-route `HYB_TOP_K_INVALID` is gone rather than aliased.
#[test]
fn query_contract_top_k_codes_resolve_through_the_closed_registry() -> TestRes {
    let expected = [
        (
            quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE,
            LexicalErrorCode::QueryTopKOutOfRange,
        ),
        (
            quanta_index_contract::INTERNAL_FETCH_OUT_OF_RANGE_CODE,
            LexicalErrorCode::QueryInternalFetchOutOfRange,
        ),
    ];
    for (code, variant) in expected {
        if LexicalErrorCode::from_code_str(code) != Some(variant) {
            return Err(format!("`{code}` does not resolve to {variant:?}").into());
        }
    }
    if LexicalErrorCode::from_code_str("HYB_TOP_K_INVALID").is_some() {
        return Err("retired per-route top_k code is still registered".into());
    }
    Ok(())
}
