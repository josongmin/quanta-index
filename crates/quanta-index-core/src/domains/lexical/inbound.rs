use quanta_index_contract::{TextQueryRequest, TextQueryResponse};

use crate::error::CoreError;

/// Driving port exposed by the lexical module to the UDS query path.
pub trait LexicalQueryPort: Send + Sync {
    fn lexical_query(&self, request: TextQueryRequest) -> Result<TextQueryResponse, CoreError>;
}
