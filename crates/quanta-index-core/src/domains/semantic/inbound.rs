use quanta_index_contract::{SearchPlaneSemanticQueryRequest, SearchPlaneSemanticQueryResponse};

use crate::error::CoreError;

pub trait SemanticQueryPort: Send + Sync {
    fn semantic_query(
        &self,
        request: SearchPlaneSemanticQueryRequest,
    ) -> Result<SearchPlaneSemanticQueryResponse, CoreError>;
}
