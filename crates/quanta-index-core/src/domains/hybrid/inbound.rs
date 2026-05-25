use quanta_index_contract::{
    HybridQueryRequest, HybridQueryResponse, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse,
};

use crate::error::CoreError;

pub trait HybridQueryPort: Send + Sync {
    fn hybrid_query(&self, request: HybridQueryRequest) -> Result<HybridQueryResponse, CoreError>;
}

pub trait ExplainQueryPort: Send + Sync {
    fn explain_query(
        &self,
        request: SearchPlaneExplainQueryRequest,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError>;
}
