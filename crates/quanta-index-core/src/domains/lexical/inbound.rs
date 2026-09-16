use quanta_index_contract::{TextQueryRequest, TextQueryResponse};

use crate::error::CoreError;
use crate::request_budget::RequestBudgetV1;

/// Driving port exposed by the lexical module to the UDS query path.
pub trait LexicalQueryPort: Send + Sync {
    fn lexical_query(
        &self,
        request: TextQueryRequest,
        budget: &RequestBudgetV1,
    ) -> Result<TextQueryResponse, CoreError>;
}
