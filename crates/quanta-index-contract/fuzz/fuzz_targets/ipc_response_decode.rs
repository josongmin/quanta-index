#![no_main]
#![forbid(unsafe_code)]

//! Same shape as `ipc_request_decode.rs`, applied to the response side.
//!
//! Coverage expanded for the current response surfaces:
//! the ingest receipts (`BatchPublishReceipt`, `RepoMapTerminalReceiptV2`)
//! and generation admin responses (`GenerationSnapshot`,
//! `GenerationStatusReport`) all flow through here so fail-closed
//! deserialization is exercised before the daemon ever sees a real
//! payload.

use libfuzzer_sys::fuzz_target;

use quanta_index_contract::{
    BatchPublishReceipt, GenerationSnapshot, GenerationStatusReport, HybridQueryResponse,
    HybridSeedQueryResponse, RepoMapQueryResponse, RepoMapTerminalReceiptV2,
    SearchPlaneControlIpcResponse, SearchPlaneControlIpcResponseEnvelope,
    SearchPlaneExplainQueryResponse, SearchPlaneHistoryQueryResponse, SearchPlaneIngestIpcResponse,
    SearchPlaneIngestIpcResponseEnvelope, SearchPlaneIpcError, SearchPlaneQueryIpcResponse,
    SearchPlaneQueryIpcResponseEnvelope, SearchPlaneRuntimeMetadataQueryResponse,
    SearchPlaneSearchCorpusActivationCasAck, SearchPlaneSearchCorpusRollbackCasAck,
    SearchPlaneStructuralQueryResponse, SemanticQueryResponse, SymbolQueryResponse,
    TextQueryResponse,
};

fuzz_target!(|data: &[u8]| {
    // Per-variant query responses.
    let _ = ciborium::de::from_reader::<TextQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<SymbolQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<SemanticQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<HybridQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<HybridSeedQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneHistoryQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneStructuralQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<RepoMapQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneExplainQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneRuntimeMetadataQueryResponse, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneQueryIpcResponse, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneQueryIpcResponseEnvelope, _>(data);

    // Control responses, including exact composite activation/rollback
    // acknowledgements (QI-ACT-01 snapshots / status report included).
    if let Ok(ack) = ciborium::de::from_reader::<SearchPlaneSearchCorpusActivationCasAck, _>(data) {
        let _active_validation = ack.active.validate_v1();
        if let Some(previous) = ack.previous_sealed_active {
            let _previous_validation = previous.validate_v1();
        }
    }
    if let Ok(ack) = ciborium::de::from_reader::<SearchPlaneSearchCorpusRollbackCasAck, _>(data) {
        let _active_validation = ack.active.validate_v1();
        let _previous_validation = ack.previous_sealed_active.validate_v1();
    }
    let _ = ciborium::de::from_reader::<RepoMapTerminalReceiptV2, _>(data);
    let _ = ciborium::de::from_reader::<GenerationSnapshot, _>(data);
    let _ = ciborium::de::from_reader::<GenerationStatusReport, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneControlIpcResponse, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneControlIpcResponseEnvelope, _>(data);

    // Ingest receipts (QI-ING-01 + history / dirty / structural follow-ons).
    let _ = ciborium::de::from_reader::<BatchPublishReceipt, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneIngestIpcResponse, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneIngestIpcResponseEnvelope, _>(data);

    // Cross-surface typed error shape.
    let _ = ciborium::de::from_reader::<SearchPlaneIpcError, _>(data);
});
