use quanta_index_contract::{SemanticQueryRequest, SemanticQueryResponse};

use crate::error::CoreError;

pub trait SemanticQueryPort: Send + Sync {
    fn semantic_query(
        &self,
        request: SemanticQueryRequest,
    ) -> Result<SemanticQueryResponse, CoreError>;
}
