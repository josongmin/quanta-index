use quanta_index_contract::RepoMapQueryRequestV1;

use crate::CoreError;

pub struct RepoMapPolicy;

impl RepoMapPolicy {
    pub fn validate_query(request: &RepoMapQueryRequestV1) -> Result<(), CoreError> {
        if request.repo_id.as_str().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap query: repo_id must not be empty".to_string(),
            ));
        }
        if request.revision_id.as_str().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap query: revision_id must not be empty".to_string(),
            ));
        }
        if request.query_text.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap query: query_text must not be empty".to_string(),
            ));
        }
        if request.top_k == 0 {
            return Err(CoreError::InvalidContract(
                "repomap query: top_k must be > 0".to_string(),
            ));
        }
        if request.token_budget == 0 {
            return Err(CoreError::InvalidContract(
                "repomap query: token_budget must be > 0".to_string(),
            ));
        }
        Ok(())
    }
}
