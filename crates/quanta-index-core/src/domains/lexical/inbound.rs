use quanta_index_contract::{
    SearchPlaneLexicalQueryResponse, SearchPlaneLexicalTextQueryRequestV2,
};

use crate::error::CoreError;

/// Driving port exposed by the lexical module to the UDS query path.
pub trait LexicalQueryPort: Send + Sync {
    fn lexical_query(
        &self,
        request: SearchPlaneLexicalTextQueryRequestV2,
    ) -> Result<SearchPlaneLexicalQueryResponse, CoreError>;
}
