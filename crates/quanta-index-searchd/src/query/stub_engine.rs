use quanta_index_contract::{
    SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse, SearchPlaneHybridQueryRequest,
    SearchPlaneHybridQueryResponse, SearchPlaneLexicalQueryRequest,
    SearchPlaneLexicalQueryResponse, SearchPlaneSemanticQueryRequest,
    SearchPlaneSemanticQueryResponse,
};
use quanta_index_core::{
    CoreError, SearchPlaneExplainQueryPort, SearchPlaneHybridQueryPort,
    SearchPlaneLexicalQueryPort, SearchPlaneSemanticQueryPort,
};

pub struct StubQueryEngine;

impl SearchPlaneLexicalQueryPort for StubQueryEngine {
    fn lexical_query(
        &self,
        _request: SearchPlaneLexicalQueryRequest,
    ) -> Result<SearchPlaneLexicalQueryResponse, CoreError> {
        Err(CoreError::NotImplemented(
            "lexical query engine not wired yet".into(),
        ))
    }
}

impl SearchPlaneSemanticQueryPort for StubQueryEngine {
    fn semantic_query(
        &self,
        _request: SearchPlaneSemanticQueryRequest,
    ) -> Result<SearchPlaneSemanticQueryResponse, CoreError> {
        Err(CoreError::NotImplemented(
            "semantic query engine not wired yet".into(),
        ))
    }
}

impl SearchPlaneHybridQueryPort for StubQueryEngine {
    fn hybrid_query(
        &self,
        _request: SearchPlaneHybridQueryRequest,
    ) -> Result<SearchPlaneHybridQueryResponse, CoreError> {
        Err(CoreError::NotImplemented(
            "hybrid query engine not wired yet".into(),
        ))
    }
}

impl SearchPlaneExplainQueryPort for StubQueryEngine {
    fn explain_query(
        &self,
        _request: SearchPlaneExplainQueryRequest,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError> {
        Err(CoreError::NotImplemented(
            "explain query engine not wired yet".into(),
        ))
    }
}
