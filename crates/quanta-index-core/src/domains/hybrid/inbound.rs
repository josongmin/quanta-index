use quanta_index_contract::{
    HybridQueryRequest, HybridQueryResponse, SearchPlaneExplainQueryRequest,
    SearchPlaneExplainQueryResponse,
};

use crate::error::CoreError;
use crate::request_budget::RequestBudgetV1;

pub trait HybridQueryPort: Send + Sync {
    fn hybrid_query(
        &self,
        request: HybridQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<HybridQueryResponse, CoreError>;
}

pub trait ExplainQueryPort: Send + Sync {
    fn explain_query(
        &self,
        request: SearchPlaneExplainQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SearchPlaneExplainQueryResponse, CoreError>;
}
