#![no_main]
#![forbid(unsafe_code)]

//! Differential fuzz target for IPC request decoders.
//!
//! Fail-closed deserialization is a contract invariant: the manual serde
//! impls in the request DTO surface MUST reject malformed input as a typed
//! Err rather than panicking, looping, or producing placeholder values.
//! This target throws arbitrary bytes at every request decoder and lets
//! libfuzzer flag any non-Err exit path.
//!
//! Coverage expanded for QI-ING-01 / QI-ACT-01 / QI-NS-01 / QI-LXB-01:
//! the new ingest IPC surface (envelope + every variant), the generation
//! admin requests, and the in-flight history / runtime / structural
//! batches all flow through here.

use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

use quanta_index_contract::{
    CurrentGenerationRequest, DirtyIngestBatch, GenerationStatusRequest, HistoryIngestBatch,
    HistoryQueryRequest, HybridQueryRequest, HybridSeedQueryRequest, RepoMapQueryRequest,
    RepoMapSourceBundle, SearchCorpusGenerationIdentityV1, SearchCorpusIngestBatch,
    SearchPlaneActivateSearchCorpusGenerationCasRequest, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcRequestEnvelope, SearchPlaneExplainQueryRequest,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcRequestEnvelope, SearchPlaneQueryIpcRequest,
    SearchPlaneQueryIpcRequestEnvelope, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    SemanticIngestBatch, SemanticQueryRequest, StructuralIngestBatch, StructuralQueryRequest,
    SymbolQueryRequest, TextQueryRequest,
};
use quanta_index_ipc::decode_request;

fuzz_target!(|data: &[u8]| {
    // Exercise the length-prefixed IPC frame reader as well as the contract's
    // raw CBOR decoders, including its bounded compressed-request path.
    let _ = decode_request::<SearchPlaneIngestIpcRequestEnvelope, _>(&mut Cursor::new(data));

    // Query DTOs (per-variant + envelope).
    let _ = ciborium::de::from_reader::<TextQueryRequest, _>(data);
    let _ = ciborium::de::from_reader::<SymbolQueryRequest, _>(data);
    let _ = ciborium::de::from_reader::<SemanticQueryRequest, _>(data);
    let _ = ciborium::de::from_reader::<HybridQueryRequest, _>(data);
    let _ = ciborium::de::from_reader::<HybridSeedQueryRequest, _>(data);
    let _ = ciborium::de::from_reader::<HistoryQueryRequest, _>(data);
    let _ = ciborium::de::from_reader::<StructuralQueryRequest, _>(data);
    let _ = ciborium::de::from_reader::<RepoMapQueryRequest, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneExplainQueryRequest, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneQueryIpcRequest, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneQueryIpcRequestEnvelope, _>(data);

    // Control DTOs (per-variant + envelope) — includes exact composite
    // activation/rollback identities and QI-ACT-01 admin queries.
    if let Ok(identity) = ciborium::de::from_reader::<SearchCorpusGenerationIdentityV1, _>(data) {
        let _validation = identity.validate_v1();
    }
    if let Ok(request) =
        ciborium::de::from_reader::<SearchPlaneActivateSearchCorpusGenerationCasRequest, _>(data)
    {
        let _validation = request.validate_v1();
    }
    if let Ok(request) =
        ciborium::de::from_reader::<SearchPlaneRollbackSearchCorpusGenerationCasRequest, _>(data)
    {
        let _validation = request.validate_v1();
    }
    let _ = ciborium::de::from_reader::<CurrentGenerationRequest, _>(data);
    let _ = ciborium::de::from_reader::<GenerationStatusRequest, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneControlIpcRequest, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneControlIpcRequestEnvelope, _>(data);

    // QI-ING-01 ingest DTOs (per-batch + envelope). History / Dirty /
    // Structural batches are in-flight wire shapes (QI-LXB-01); the
    // fuzz target exercises them now so the fail-closed deserializer
    // invariant holds before runtime wiring lands.
    let _ = ciborium::de::from_reader::<SearchCorpusIngestBatch, _>(data);
    let _ = ciborium::de::from_reader::<SemanticIngestBatch, _>(data);
    let _ = ciborium::de::from_reader::<HistoryIngestBatch, _>(data);
    let _ = ciborium::de::from_reader::<DirtyIngestBatch, _>(data);
    let _ = ciborium::de::from_reader::<StructuralIngestBatch, _>(data);
    let _ = ciborium::de::from_reader::<RepoMapSourceBundle, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneIngestIpcRequest, _>(data);
    let _ = ciborium::de::from_reader::<SearchPlaneIngestIpcRequestEnvelope, _>(data);
});
