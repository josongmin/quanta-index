use quanta_index_contract::{
    SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse, SearchPlaneHybridQueryRequest,
    SearchPlaneHybridQueryResponse, SearchPlaneLexicalQueryRequest,
    SearchPlaneLexicalQueryResponse, SearchPlaneSemanticQueryRequest,
    SearchPlaneSemanticQueryResponse,
};

use crate::CoreError;

/// Driving port: lexical search requests.
pub trait SearchPlaneLexicalQueryPort {
    fn lexical_query(
        &self,
        request: SearchPlaneLexicalQueryRequest,
    ) -> Result<SearchPlaneLexicalQueryResponse, CoreError>;
}

/// Driving port: semantic search requests.
pub trait SearchPlaneSemanticQueryPort {
    fn semantic_query(
        &self,
        request: SearchPlaneSemanticQueryRequest,
    ) -> Result<SearchPlaneSemanticQueryResponse, CoreError>;
}

/// Driving port: hybrid search requests.
pub trait SearchPlaneHybridQueryPort {
    fn hybrid_query(
        &self,
        request: SearchPlaneHybridQueryRequest,
    ) -> Result<SearchPlaneHybridQueryResponse, CoreError>;
}

/// Driving port: explain requests.
pub trait SearchPlaneExplainQueryPort {
    fn explain_query(
        &self,
        request: SearchPlaneExplainQueryRequest,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError>;
}

/// Aggregate driving port implemented by the query engine adapter.
pub trait SearchPlaneQueryContractPort:
    SearchPlaneLexicalQueryPort
    + SearchPlaneSemanticQueryPort
    + SearchPlaneHybridQueryPort
    + SearchPlaneExplainQueryPort
{
}

impl<T> SearchPlaneQueryContractPort for T where
    T: SearchPlaneLexicalQueryPort
        + SearchPlaneSemanticQueryPort
        + SearchPlaneHybridQueryPort
        + SearchPlaneExplainQueryPort
{
}
