use serde::{Deserialize, Serialize};

use crate::{
    BridgeQueryRequest, CurrentGenerationRequest, GenerationSnapshot, GenerationStatusReport,
    GenerationStatusRequest, HistoryQueryRequest, HybridQueryRequest, HybridQueryResponse,
    RepoMapActivateGenerationRequest, RepoMapMutationAck, RepoMapQueryRequest,
    RepoMapQueryResponse, SearchPlaneActivateGenerationRequest, SearchPlaneActivationAck,
    SearchPlaneBridgeQueryResponse, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse, SearchPlaneHistoryQueryResponse, SearchPlaneIpcError,
    SearchPlaneSourcegraphQueryRequest, SearchPlaneSourcegraphQueryResponse,
    SearchPlaneStructuralQueryResponse, SemanticQueryRequest, SemanticQueryResponse,
    StructuralQueryRequest, SymbolQueryRequest, SymbolQueryResponse, TextQueryRequest,
    TextQueryResponse,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchPlaneQueryIpcRequestEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneQueryIpcRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum SearchPlaneQueryIpcRequest {
    Text(TextQueryRequest),
    Symbol(SymbolQueryRequest),
    Semantic(SemanticQueryRequest),
    Hybrid(HybridQueryRequest),
    History(HistoryQueryRequest),
    Structural(StructuralQueryRequest),
    Bridge(BridgeQueryRequest),
    RepoMapQuery(RepoMapQueryRequest),
    Explain(SearchPlaneExplainQueryRequest),
    Sourcegraph(SearchPlaneSourcegraphQueryRequest),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchPlaneQueryIpcResponseEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneQueryIpcResponse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum SearchPlaneQueryIpcResponse {
    Text(TextQueryResponse),
    Symbol(SymbolQueryResponse),
    Semantic(SemanticQueryResponse),
    Hybrid(HybridQueryResponse),
    History(SearchPlaneHistoryQueryResponse),
    Structural(SearchPlaneStructuralQueryResponse),
    Bridge(SearchPlaneBridgeQueryResponse),
    RepoMapQuery(RepoMapQueryResponse),
    Explain(SearchPlaneExplainQueryResponse),
    Error(SearchPlaneIpcError),
    Sourcegraph(SearchPlaneSourcegraphQueryResponse),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchPlaneControlIpcRequestEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneControlIpcRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum SearchPlaneControlIpcRequest {
    ActivateGeneration(SearchPlaneActivateGenerationRequest),
    // QI-INT-01: `RepoMapIngest(RepoMapSourceBundle)` was removed from the
    // control surface. All RepoMap bundle publishes now go through the
    // typed ingest IPC (`SearchPlaneIngestIpcRequest::PublishRepoMapBundle`)
    // — the SDK switched in QI-SDK-01 and external consumers are expected to
    // follow. Breaking-first per CLAUDE.md "compatibility preservation is
    // not the default".
    RepoMapActivate(RepoMapActivateGenerationRequest),
    /// QI-ACT-01: read-only generation admin query for one
    /// `(repo, revision, track)` triple.
    CurrentGeneration(CurrentGenerationRequest),
    /// QI-ACT-01: read-only status query returning all activated tracks for
    /// one `(repo, revision)` pair.
    GenerationStatus(GenerationStatusRequest),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchPlaneControlIpcResponseEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneControlIpcResponse,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum SearchPlaneControlIpcResponse {
    ActivationAck(SearchPlaneActivationAck),
    RepoMapMutationAck(RepoMapMutationAck),
    Error(SearchPlaneIpcError),
    /// QI-ACT-01: response to [`SearchPlaneControlIpcRequest::CurrentGeneration`].
    CurrentGenerationSnapshot(GenerationSnapshot),
    /// QI-ACT-01: response to [`SearchPlaneControlIpcRequest::GenerationStatus`].
    GenerationStatusReport(GenerationStatusReport),
}
