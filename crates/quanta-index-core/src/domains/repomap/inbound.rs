use quanta_index_contract::RepoMapQueryRequest;
use quanta_index_contract::RepoMapQueryResponse;

use crate::CoreError;

pub trait RepoMapQueryPort: Send + Sync {
    fn query(&self, request: RepoMapQueryRequest) -> Result<RepoMapQueryResponse, CoreError>;
}
