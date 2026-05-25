use quanta_index_contract::RepoMapQueryRequest;

use crate::CoreError;

use super::RepoMapPolicy;

pub struct RepoMapService;

impl RepoMapService {
    pub fn validate_query(request: &RepoMapQueryRequest) -> Result<(), CoreError> {
        RepoMapPolicy::validate_query(request)
    }
}
