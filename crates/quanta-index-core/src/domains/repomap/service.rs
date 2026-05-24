use quanta_index_contract::RepoMapQueryRequestV1;

use crate::CoreError;

use super::RepoMapPolicy;

pub struct RepoMapService;

impl RepoMapService {
    pub fn validate_query(request: &RepoMapQueryRequestV1) -> Result<(), CoreError> {
        RepoMapPolicy::validate_query(request)
    }
}
