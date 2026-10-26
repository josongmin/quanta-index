//! Closed JSON admission shape for the canonical `NativeStubRequest::Preflight` wire.
//!
//! The native Serde DTOs in this crate remain the decode authority. This
//! projection lets external SDK decoders enforce the same record and scalar
//! shape before sending a request to that authority.

use serde_json::{Value, json};

fn closed(properties: Value, required: &[&str]) -> Value {
    Value::Object(serde_json::Map::from_iter([
        ("type".into(), Value::String("object".into())),
        ("properties".into(), properties),
        ("required".into(), json!(required)),
        ("additionalProperties".into(), Value::Bool(false)),
    ]))
}

fn unsigned(maximum: u64) -> Value {
    json!({"type": "integer", "minimum": 0, "maximum": maximum})
}

fn bytes(length: usize) -> Value {
    json!({
        "type": "array",
        "minItems": length,
        "maxItems": length,
        "items": unsigned(u8::MAX.into()),
    })
}

fn workspace_key() -> Value {
    closed(
        json!({
            "tenant": bytes(16),
            "collection": bytes(16),
            "worktree": bytes(16),
            "slot": bytes(16),
        }),
        &["tenant", "collection", "worktree", "slot"],
    )
}

fn overlay_document() -> Value {
    closed(
        json!({
            "session": closed(
                json!({
                    "workspace": workspace_key(),
                    "publisher": bytes(32),
                    "epoch": unsigned(u64::MAX),
                }),
                &["workspace", "publisher", "epoch"],
            ),
            "owner": closed(
                json!({"producer": bytes(16), "stable_id": bytes(16)}),
                &["producer", "stable_id"],
            ),
        }),
        &["session", "owner"],
    )
}

fn view_ref() -> Value {
    closed(
        json!({
            "workspace": workspace_key(),
            "epoch": unsigned(u64::MAX),
            "sequence": unsigned(u64::MAX),
            "digest": bytes(32),
        }),
        &["workspace", "epoch", "sequence", "digest"],
    )
}

fn binding() -> Value {
    closed(
        json!({
            "document": overlay_document(),
            "revision": unsigned(u64::MAX),
            "selected": view_ref(),
            "source": bytes(32),
            "language": {"type": "string"},
        }),
        &["document", "revision", "selected", "source", "language"],
    )
}

fn item() -> Value {
    closed(
        json!({
            "stable_id": bytes(32),
            "ast_digest": bytes(32),
            "start": unsigned(u32::MAX.into()),
            "end": unsigned(u32::MAX.into()),
            "label": {"type": "string"},
        }),
        &["stable_id", "ast_digest", "start", "end", "label"],
    )
}

fn position() -> Value {
    closed(
        json!({
            "line": unsigned(u32::MAX.into()),
            "column": unsigned(u32::MAX.into()),
        }),
        &["line", "column"],
    )
}

fn edit() -> Value {
    closed(
        json!({
            "target_id": bytes(32),
            "expected_ast": bytes(32),
            "range": closed(
                json!({"start": position(), "end": position()}),
                &["start", "end"],
            ),
            "replacement": {"type": "string"},
        }),
        &["target_id", "expected_ast", "range", "replacement"],
    )
}

/// JSON schema for the externally tagged, closed native Preflight request.
///
/// The nested DTO shapes follow the `native_serde_*` declarations beside the
/// types; changing those declarations requires updating this projection.
#[must_use]
pub fn native_stub_preflight_schema_v1() -> Value {
    closed(
        json!({
            "Preflight": closed(
                json!({
                    "binding": binding(),
                    "items": {"type": "array", "items": item()},
                    "selected_id": bytes(32),
                    "edits": {"type": "array", "items": edit()},
                    "unit": {"type": "string", "enum": ["Utf8Bytes", "Utf16Units"]},
                }),
                &["binding", "items", "selected_id", "edits", "unit"],
            ),
        }),
        &["Preflight"],
    )
}
