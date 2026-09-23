//! W10-R2 request-correlation gate over real envelopes.
//!
//! A JSON request envelope with id 0 decodes (the wire shape is frozen
//! `u64`) and then refuses typed at the transport's `validated` gate,
//! while a nonzero id validates through with its payload intact.

use quanta_index_contract::{
    QueryConstraintSetV1, SearchPlaneQueryIpcRequest, SearchPlaneQueryIpcRequestEnvelope,
    TextQueryRequest, TextQuerySyntax,
};
use quanta_index_ipc::{IpcError, RequestEnvelope};

use crate::query_dispatcher::tests::support::common::TestResult;

fn text_envelope(request_id: u64) -> SearchPlaneQueryIpcRequestEnvelope {
    SearchPlaneQueryIpcRequestEnvelope {
        request_id,
        payload: SearchPlaneQueryIpcRequest::Text(TextQueryRequest {
            syntax: TextQuerySyntax::Native,
            query_text: "needle".to_string(),
            constraints: QueryConstraintSetV1::unconstrained(),
            generation: None,
            generation_selector: None,
            top_k: 5,
            cursor: None,
        }),
    }
}

// CASE-COVERS: W10-R2 — a JSON envelope with id 0 refuses typed.
#[test]
fn json_envelope_with_zero_id_refuses_typed() -> TestResult {
    let json = serde_json::to_value(text_envelope(0))?;
    let decoded: SearchPlaneQueryIpcRequestEnvelope = serde_json::from_value(json)?;
    match decoded.validated() {
        Err(IpcError::ZeroRequestId) => Ok(()),
        other => Err(format!("zero id must refuse typed, got {other:?}").into()),
    }
}

// CASE-COVERS: W10-R2 — a JSON envelope with a nonzero id validates
// through with its payload intact.
#[test]
fn json_envelope_with_nonzero_id_validates_with_payload() -> TestResult {
    let json = serde_json::to_value(text_envelope(41))?;
    let decoded: SearchPlaneQueryIpcRequestEnvelope = serde_json::from_value(json)?;
    let (admitted, payload) = decoded
        .validated()
        .map_err(|err| format!("nonzero id must validate: {err}"))?;
    if admitted.get() != 41 {
        return Err(format!("validated id must echo 41, got {admitted:?}").into());
    }
    let SearchPlaneQueryIpcRequest::Text(text) = &payload else {
        return Err(format!("payload must survive validation, got {payload:?}").into());
    };
    if text.query_text != "needle" || text.top_k != 5 {
        return Err(format!("payload must be intact, got {text:?}").into());
    }
    Ok(())
}
