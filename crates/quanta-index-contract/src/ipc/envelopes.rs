use crate::{
    SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse, SearchPlaneHybridQueryRequest,
    SearchPlaneHybridQueryResponse, SearchPlaneLexicalQueryRequest,
    SearchPlaneLexicalQueryResponse, SearchPlaneSemanticQueryRequest,
    SearchPlaneSemanticQueryResponse,
};

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneIpcRequestEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneIpcRequest,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaneIpcRequest {
    Lexical(SearchPlaneLexicalQueryRequest),
    Semantic(SearchPlaneSemanticQueryRequest),
    Hybrid(SearchPlaneHybridQueryRequest),
    Explain(SearchPlaneExplainQueryRequest),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneIpcResponseEnvelope {
    pub request_id: u64,
    pub payload: SearchPlaneIpcResponse,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SearchPlaneIpcResponse {
    Lexical(SearchPlaneLexicalQueryResponse),
    Semantic(SearchPlaneSemanticQueryResponse),
    Hybrid(SearchPlaneHybridQueryResponse),
    Explain(SearchPlaneExplainQueryResponse),
    Error(SearchPlaneIpcError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneIpcError {
    pub code: String,
    pub message: String,
}
