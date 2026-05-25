use serde::{Deserialize, Serialize};

use crate::{
    BridgeQueryRequest, HistoryQueryRequest, HybridQueryRequest, HybridQueryResponse,
    RepoMapActivateGenerationRequestV1, RepoMapMutationAckV1, RepoMapQueryRequestV1,
    RepoMapQueryResponseV1, RepoMapSourceBundleV1, SearchPlaneActivateGenerationRequest,
    SearchPlaneActivationAck, SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse,
    SearchPlaneHistoryQueryResponse, SearchPlaneIpcError, SearchPlaneSourcegraphQueryRequest,
    SearchPlaneSourcegraphQueryResponse, SearchPlaneStructuralQueryResponse,
    SemanticQueryRequest, SemanticQueryResponse, SourcegraphQueryRequest, StructuralQueryRequest,
    SymbolQueryRequest, SymbolQueryResponse, TextQueryRequest, TextQueryResponse,
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
    RepoMapQuery(RepoMapQueryRequestV1),
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
    RepoMapQuery(RepoMapQueryResponseV1),
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
    RepoMapIngest(RepoMapSourceBundleV1),
    RepoMapActivate(RepoMapActivateGenerationRequestV1),
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
    RepoMapMutationAck(RepoMapMutationAckV1),
    Error(SearchPlaneIpcError),
}
