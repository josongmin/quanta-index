#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use quanta_index_contract::{
    BatchIngestMode, CapabilityStatusV1, ManifestGeneration, OwnerDocKind, RawFallbackReasonV1,
    RepoId, RepoRelativePath, RevisionId, SearchCorpusIngestBatch, SemanticCorpusKindV1,
    SemanticSourceRecordV1, SemanticSourceReplaceScopeV1, SemanticSourceScopeKeyV1, SourceRoleV1,
    validate_semantic_source_record_v1,
};
use serde_json::json;

type TestRes = Result<(), Box<dyn std::error::Error>>;

fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buf = Vec::new();
    ciborium::ser::into_writer(value, &mut buf)?;
    Ok(buf)
}

fn decode<T>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>>
where
    T: for<'de> serde::Deserialize<'de>,
{
    Ok(ciborium::de::from_reader(bytes)?)
}

fn cbor_map_fields_mut(
    value: &mut ciborium::Value,
) -> Result<&mut Vec<(ciborium::Value, ciborium::Value)>, Box<dyn std::error::Error>> {
    if let ciborium::Value::Map(fields) = value {
        Ok(fields)
    } else {
        Err(format!("expected CBOR map, got {value:?}").into())
    }
}

fn remove_cbor_text_field(
    fields: &mut Vec<(ciborium::Value, ciborium::Value)>,
    field_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let before = fields.len();
    fields
        .retain(|(key, _value)| !matches!(key, ciborium::Value::Text(text) if text == field_name));
    if fields.len() == before {
        return Err(format!("field `{field_name}` not found").into());
    }
    Ok(())
}

fn fixture_scope_key(owner_id: &str) -> SemanticSourceScopeKeyV1 {
    SemanticSourceScopeKeyV1 {
        corpus_kind: SemanticCorpusKindV1::SymbolCard,
        owner_kind: OwnerDocKind::Symbol,
        owner_id: owner_id.to_string(),
    }
}

fn fixture_semantic_source_record() -> SemanticSourceRecordV1 {
    SemanticSourceRecordV1 {
        record_id: "record-1".to_string(),
        corpus_kind: SemanticCorpusKindV1::SymbolCard,
        owner_kind: OwnerDocKind::Symbol,
        owner_id: "symbol-1".to_string(),
        source_doc_id: "doc-1".to_string(),
        parent_owner_id: None,
        repo_relative_path: RepoRelativePath::new("src/lib.rs"),
        language: Some("rust".to_string()),
        package: Some("crate".to_string()),
        symbol_kind: Some("function".to_string()),
        visibility: Some("pub".to_string()),
        source_role: SourceRoleV1::CardText,
        generated: false,
        capability_status: CapabilityStatusV1::Full,
        raw_fallback_reason: None,
        authority_digest: "auth:sha256:1".to_string(),
        render_policy_digest: "render:sha256:1".to_string(),
        card_schema_version: 1,
        text: "pub fn demo() {}".to_string(),
    }
}

fn fixture_search_corpus_batch() -> SearchCorpusIngestBatch {
    SearchCorpusIngestBatch {
        repo_id: RepoId::new("repo-1").expect("static fixture ID satisfies canonical policy"),
        revision_id: RevisionId::new("rev-1")
            .expect("static fixture ID satisfies canonical policy"),
        generation: ManifestGeneration::new(7),
        base_generation: None,
        manifest_digest: "manifest:lex".to_string(),
        batch_digest: "batch:lex".to_string(),
        mode: BatchIngestMode::ReplaceGeneration,
        bundle_payload: None,
        clear_surfaces: Vec::new(),
        replace_scopes: Vec::new(),
        tombstone_scopes: Vec::new(),
        semantic_replace_scopes: vec![SemanticSourceReplaceScopeV1 {
            scope: fixture_scope_key("symbol-1"),
            scope_digest: "scope:semantic:1".to_string(),
            sources: vec![fixture_semantic_source_record()],
            cluster_memberships: Vec::new(),
        }],
        semantic_tombstone_scopes: vec![fixture_scope_key("symbol-2")],
        seal: true,
    }
}

#[test]
fn old_batch_json_and_cbor_without_semantic_fields_still_decode() -> TestRes {
    let batch = fixture_search_corpus_batch();

    let mut json_value = serde_json::to_value(&batch)?;
    let json_fields = json_value
        .as_object_mut()
        .ok_or_else(|| "expected JSON object".to_string())?;
    let json_replace = json_fields.remove("semantic_replace_scopes");
    let json_tombstone = json_fields.remove("semantic_tombstone_scopes");
    if json_replace.is_none() || json_tombstone.is_none() {
        return Err("semantic fields missing from JSON fixture".into());
    }
    let decoded_json: SearchCorpusIngestBatch = serde_json::from_value(json_value)?;
    if !decoded_json.semantic_replace_scopes.is_empty()
        || !decoded_json.semantic_tombstone_scopes.is_empty()
    {
        return Err("JSON decode must default missing semantic fields to empty vecs".into());
    }
    if decoded_json.repo_id != batch.repo_id
        || decoded_json.revision_id != batch.revision_id
        || decoded_json.generation != batch.generation
        || decoded_json.manifest_digest != batch.manifest_digest
    {
        return Err("JSON decode must preserve non-semantic batch fields".into());
    }

    let mut cbor_value: ciborium::Value = decode(&encode(&batch)?)?;
    let cbor_fields = cbor_map_fields_mut(&mut cbor_value)?;
    remove_cbor_text_field(cbor_fields, "semantic_replace_scopes")?;
    remove_cbor_text_field(cbor_fields, "semantic_tombstone_scopes")?;
    let decoded_cbor: SearchCorpusIngestBatch = decode(&encode(&cbor_value)?)?;
    if !decoded_cbor.semantic_replace_scopes.is_empty()
        || !decoded_cbor.semantic_tombstone_scopes.is_empty()
    {
        return Err("CBOR decode must default missing semantic fields to empty vecs".into());
    }
    if decoded_cbor.repo_id != batch.repo_id
        || decoded_cbor.revision_id != batch.revision_id
        || decoded_cbor.generation != batch.generation
        || decoded_cbor.manifest_digest != batch.manifest_digest
    {
        return Err("CBOR decode must preserve non-semantic batch fields".into());
    }

    Ok(())
}

#[test]
fn semantic_source_record_round_trips() -> TestRes {
    let record = fixture_semantic_source_record();

    let decoded_json: SemanticSourceRecordV1 =
        serde_json::from_value(serde_json::to_value(&record)?)?;
    if decoded_json != record {
        return Err(format!(
            "JSON round-trip mismatch: original={record:?}, decoded={decoded_json:?}"
        )
        .into());
    }

    let decoded_cbor: SemanticSourceRecordV1 = decode(&encode(&record)?)?;
    if decoded_cbor != record {
        return Err(format!(
            "CBOR round-trip mismatch: original={record:?}, decoded={decoded_cbor:?}"
        )
        .into());
    }

    Ok(())
}

#[test]
fn invalid_symbol_card_body_role_fails_validation() -> TestRes {
    let mut record = fixture_semantic_source_record();
    record.source_role = SourceRoleV1::DocumentText;

    match validate_semantic_source_record_v1(&record) {
        Ok(()) => Err("SymbolCard + non-CardText must fail validation".into()),
        Err(message) => {
            if !message.contains("CardText") {
                return Err(format!("unexpected validation message: {message}").into());
            }
            Ok(())
        }
    }
}

#[test]
fn raw_code_fallback_without_reason_fails() -> TestRes {
    let mut record = fixture_semantic_source_record();
    record.corpus_kind = SemanticCorpusKindV1::RawCodeFallback;
    record.source_role = SourceRoleV1::RawFallbackText;
    record.parent_owner_id = Some("symbol-parent".to_string());
    record.raw_fallback_reason = None;

    match validate_semantic_source_record_v1(&record) {
        Ok(()) => Err("RawCodeFallback without reason must fail validation".into()),
        Err(message) => {
            if !message.contains("raw_fallback_reason") {
                return Err(format!("unexpected validation message: {message}").into());
            }
            Ok(())
        }
    }
}

#[test]
fn unknown_corpus_kind_wire_value_fails() -> TestRes {
    let value = json!({
        "record_id": "record-1",
        "corpus_kind": "UnknownCorpusKind",
        "owner_kind": "Symbol",
        "owner_id": "symbol-1",
        "source_doc_id": "doc-1",
        "parent_owner_id": null,
        "repo_relative_path": "src/lib.rs",
        "language": "rust",
        "package": "crate",
        "symbol_kind": "function",
        "visibility": "pub",
        "source_role": "CardText",
        "generated": false,
        "capability_status": "Full",
        "raw_fallback_reason": null,
        "authority_digest": "auth:sha256:1",
        "render_policy_digest": "render:sha256:1",
        "card_schema_version": 1,
        "text": "pub fn demo() {}"
    });

    let result: Result<SemanticSourceRecordV1, _> = serde_json::from_value(value);
    match result {
        Ok(decoded) => Err(format!("decode unexpectedly succeeded: {decoded:?}").into()),
        Err(err) => {
            let message = err.to_string();
            if !message.contains("unknown variant") {
                return Err(format!("unexpected decode error: {message}").into());
            }
            Ok(())
        }
    }
}

#[test]
fn semantic_source_record_has_no_vector_field() -> TestRes {
    let record = fixture_semantic_source_record();
    let SemanticSourceRecordV1 {
        record_id,
        corpus_kind,
        owner_kind,
        owner_id,
        source_doc_id,
        parent_owner_id,
        repo_relative_path,
        language,
        package,
        symbol_kind,
        visibility,
        source_role,
        generated,
        capability_status,
        raw_fallback_reason,
        authority_digest,
        render_policy_digest,
        card_schema_version,
        text,
    } = record;
    let rebuilt = SemanticSourceRecordV1 {
        record_id,
        corpus_kind,
        owner_kind,
        owner_id,
        source_doc_id,
        parent_owner_id,
        repo_relative_path,
        language,
        package,
        symbol_kind,
        visibility,
        source_role,
        generated,
        capability_status,
        raw_fallback_reason,
        authority_digest,
        render_policy_digest,
        card_schema_version,
        text,
    };

    let value = serde_json::to_value(&rebuilt)?;
    let fields = value
        .as_object()
        .ok_or_else(|| "expected JSON object".to_string())?;
    if fields.contains_key("vector") || fields.contains_key("embedding") {
        return Err("SemanticSourceRecordV1 must not expose a vector field".into());
    }
    let actual = fields.keys().cloned().collect::<BTreeSet<_>>();
    let expected = BTreeSet::from([
        "record_id".to_string(),
        "corpus_kind".to_string(),
        "owner_kind".to_string(),
        "owner_id".to_string(),
        "source_doc_id".to_string(),
        "parent_owner_id".to_string(),
        "repo_relative_path".to_string(),
        "language".to_string(),
        "package".to_string(),
        "symbol_kind".to_string(),
        "visibility".to_string(),
        "source_role".to_string(),
        "generated".to_string(),
        "capability_status".to_string(),
        "raw_fallback_reason".to_string(),
        "authority_digest".to_string(),
        "render_policy_digest".to_string(),
        "card_schema_version".to_string(),
        "text".to_string(),
    ]);
    if actual != expected {
        return Err(format!("unexpected SemanticSourceRecordV1 field set: {actual:?}").into());
    }

    Ok(())
}

#[test]
fn raw_code_fallback_without_parent_owner_id_fails() -> TestRes {
    let mut record = fixture_semantic_source_record();
    record.corpus_kind = SemanticCorpusKindV1::RawCodeFallback;
    record.source_role = SourceRoleV1::RawFallbackText;
    record.raw_fallback_reason = Some(RawFallbackReasonV1::MacroHeavy);
    record.parent_owner_id = None;

    match validate_semantic_source_record_v1(&record) {
        Ok(()) => Err("RawCodeFallback without parent_owner_id must fail validation".into()),
        Err(message) => {
            if !message.contains("parent_owner_id") {
                return Err(format!("unexpected validation message: {message}").into());
            }
            Ok(())
        }
    }
}
