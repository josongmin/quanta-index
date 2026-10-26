//! Typed envelopes over the existing bounded CBOR/UDS transport.

use quanta_index_contract::{
    DiagnosticAttemptFieldV1,
    native_protocol::{
        NativeIndexErrorCode, NativeIndexRequest, NativeIndexRequestEnvelope, NativeIndexResponse,
        NativeIndexResponseEnvelope, NativeIndexWireError,
    },
};

use crate::{RequestEnvelope, ResponseEnvelope, SlotRefusal};

impl RequestEnvelope<NativeIndexRequest> for NativeIndexRequestEnvelope {
    fn into_parts(self) -> (u64, NativeIndexRequest, DiagnosticAttemptFieldV1) {
        (
            self.request_id,
            self.payload,
            DiagnosticAttemptFieldV1::Absent,
        )
    }

    fn repo_scope(_request: &NativeIndexRequest) -> Option<String> {
        // The service bounds concurrent dispatch globally. Workspace ordering
        // belongs to catalog CAS and fences, not a transport repository key.
        None
    }

    fn streaming(request: &NativeIndexRequest) -> bool {
        matches!(request, NativeIndexRequest::QueryEffortStream { .. })
    }
}

impl ResponseEnvelope<NativeIndexResponse> for NativeIndexResponseEnvelope {
    fn from_parts(request_id: u64, payload: NativeIndexResponse) -> Self {
        Self {
            request_id,
            payload,
        }
    }

    fn stream_unsupported(request_id: u64) -> Option<Self> {
        Some(Self {
            request_id,
            payload: NativeIndexResponse::Error(NativeIndexWireError {
                code: NativeIndexErrorCode::CapabilityUnavailable,
                message: "native effort stream is unavailable".into(),
            }),
        })
    }

    fn stream_stage(payload: &NativeIndexResponse) -> bool {
        matches!(
            payload,
            NativeIndexResponse::TextRanked(_)
                | NativeIndexResponse::SemanticRanked(_)
                | NativeIndexResponse::OverlayTextRanked(_)
                | NativeIndexResponse::OverlaySemanticRanked(_)
        )
    }

    fn stream_terminal(payload: &NativeIndexResponse) -> bool {
        matches!(
            payload,
            NativeIndexResponse::EffortDone(_) | NativeIndexResponse::Error(_)
        )
    }

    fn result_too_large(request_id: u64, encoded_bytes: u64, limit_bytes: u64) -> Option<Self> {
        Some(Self {
            request_id,
            payload: NativeIndexResponse::Error(NativeIndexWireError {
                code: NativeIndexErrorCode::ResponseTooLarge,
                message: format!("response {encoded_bytes} bytes exceeds {limit_bytes}"),
            }),
        })
    }

    fn overloaded(request_id: u64, _refusal: &SlotRefusal) -> Option<Self> {
        Some(Self {
            request_id,
            payload: NativeIndexResponse::Error(NativeIndexWireError {
                code: NativeIndexErrorCode::Overloaded,
                message: "native index dispatch admission exhausted".into(),
            }),
        })
    }
}
