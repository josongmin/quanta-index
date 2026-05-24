use quanta_index_contract::{SearchPlaneLexicalQueryRequest, SearchPlaneLexicalQueryResponse};

use crate::error::CoreError;

/// Driving port exposed by the lexical module to the UDS query path.
pub trait LexicalQueryPort: Send + Sync {
    fn lexical_query(
        &self,
        request: SearchPlaneLexicalQueryRequest,
    ) -> Result<SearchPlaneLexicalQueryResponse, CoreError>;
}
