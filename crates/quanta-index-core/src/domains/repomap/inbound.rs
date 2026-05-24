use quanta_index_contract::RepoMapQueryRequestV1;
use quanta_index_contract::RepoMapQueryResponseV1;

use crate::CoreError;

pub trait RepoMapQueryPort: Send + Sync {
    fn query(&self, request: RepoMapQueryRequestV1) -> Result<RepoMapQueryResponseV1, CoreError>;
}
