use quanta_index_contract::{SemanticQueryRequest, SemanticQueryResponse};

use crate::error::CoreError;
use crate::request_budget::RequestBudgetV1;

pub trait SemanticQueryPort: Send + Sync {
    fn semantic_query(
        &self,
        request: SemanticQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<SemanticQueryResponse, CoreError>;
}
