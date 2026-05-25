use quanta_index_contract::RepoMapQueryRequest;

use crate::CoreError;

pub struct RepoMapPolicy;

impl RepoMapPolicy {
    pub fn validate_query(request: &RepoMapQueryRequest) -> Result<(), CoreError> {
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

#[cfg(test)]
mod tests {
    //! `cargo mutants` exposed missed `==` -> `!=` mutations on the
    //! `top_k` and `token_budget` zero checks.
    //!
    //! These tests kill both directions by asserting that the policy
    //! accepts positive values and rejects zero values explicitly.

    use super::*;
    use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};

    fn base_request() -> RepoMapQueryRequest {
        RepoMapQueryRequest {
            repo_id: RepoId::new("repo"),
            revision_id: RevisionId::new("rev"),
            manifest_generation: ManifestGeneration::new(1),
            query_text: "needle".to_string(),
            top_k: 5,
            token_budget: 100,
            focus_subjects: Vec::new(),
        }
    }

    #[test]
    fn valid_request_accepted() {
        assert!(RepoMapPolicy::validate_query(&base_request()).is_ok());
    }

    #[test]
    fn top_k_zero_rejected_kills_eq_to_ne_mutation() {
        let mut req = base_request();
        req.top_k = 0;
        assert!(matches!(
            RepoMapPolicy::validate_query(&req),
            Err(CoreError::InvalidContract(msg)) if msg.contains("top_k")
        ));
    }

    #[test]
    fn top_k_nonzero_accepted() {
        let mut req = base_request();
        req.top_k = 1;
        assert!(RepoMapPolicy::validate_query(&req).is_ok());
    }

    #[test]
    fn token_budget_zero_rejected_kills_eq_to_ne_mutation() {
        let mut req = base_request();
        req.token_budget = 0;
        assert!(matches!(
            RepoMapPolicy::validate_query(&req),
            Err(CoreError::InvalidContract(msg)) if msg.contains("token_budget")
        ));
    }

    #[test]
    fn token_budget_nonzero_accepted() {
        let mut req = base_request();
        req.token_budget = 1;
        assert!(RepoMapPolicy::validate_query(&req).is_ok());
    }
}
