use quanta_index_contract::{
    LqQuery, SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse,
    SearchPlaneHybridQueryRequest, SearchPlaneHybridQueryResponse, SearchPlaneLexicalQueryRequest,
    SearchPlaneLexicalQueryResponse, SearchPlaneSemanticQueryRequest,
    SearchPlaneSemanticQueryResponse,
};

use crate::CoreError;

pub trait SearchPlaneLexicalQueryPort {
    fn lexical_query(
        &self,
        request: SearchPlaneLexicalQueryRequest,
    ) -> Result<SearchPlaneLexicalQueryResponse, CoreError>;
}

pub trait SearchPlaneSemanticQueryPort {
    fn semantic_query(
        &self,
        request: SearchPlaneSemanticQueryRequest,
    ) -> Result<SearchPlaneSemanticQueryResponse, CoreError>;
}

pub trait SearchPlaneHybridQueryPort {
    fn hybrid_query(
        &self,
        request: SearchPlaneHybridQueryRequest,
    ) -> Result<SearchPlaneHybridQueryResponse, CoreError>;
}

pub trait SearchPlaneExplainQueryPort {
    fn explain_query(
        &self,
        request: SearchPlaneExplainQueryRequest,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError>;
}

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

pub trait SearchPlaneQueryValidator {
    fn validate_query(&self, query: &LqQuery) -> Result<(), CoreError>;
}
