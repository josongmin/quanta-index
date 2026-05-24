use quanta_index_contract::{
    SearchPlaneExplainQueryRequest, SearchPlaneExplainQueryResponse, SearchPlaneHybridQueryRequest,
    SearchPlaneHybridQueryResponse,
};

use crate::error::CoreError;

pub trait HybridQueryPort: Send + Sync {
    fn hybrid_query(
        &self,
        request: SearchPlaneHybridQueryRequest,
    ) -> Result<SearchPlaneHybridQueryResponse, CoreError>;
}

pub trait ExplainQueryPort: Send + Sync {
    fn explain_query(
        &self,
        request: SearchPlaneExplainQueryRequest,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError>;
}
