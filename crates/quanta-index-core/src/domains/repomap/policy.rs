use quanta_index_contract::RepoMapQueryRequest;

use crate::CoreError;
use crate::error::validate_query_top_k;

pub struct RepoMapPolicy;

impl RepoMapPolicy {
    /// Accept or refuse a repo-map query's shape.
    ///
    /// `top_k` goes through the one validator every query route uses, so
    /// the repo-map route reports the shared `QUERY_TOP_K_OUT_OF_RANGE` code
    /// and the same public range instead of its own zero-only check
    /// (QI-BB-025; plan §11 "dispatcher-local top-k caps").
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
        let _accepted = validate_query_top_k(request.top_k)?;
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
    //! `token_budget` zero check.
    //!
    //! These tests kill both directions by asserting that the policy
    //! accepts positive values and rejects zero values explicitly, and
    //! that `top_k` is refused with the route-shared typed code at both
    //! ends of the public range.

    use super::*;
    use quanta_index_contract::{
        ManifestGeneration, PUBLIC_TOP_K_MAX, RepoId, RevisionId, TOP_K_OUT_OF_RANGE_CODE,
    };

    fn base_request() -> RepoMapQueryRequest {
        RepoMapQueryRequest {
            repo_id: RepoId::new("repo").expect("static fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("rev")
                .expect("static fixture ID satisfies canonical policy"),
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
    fn top_k_outside_the_public_range_is_refused_with_the_shared_code() {
        for refused in [0, PUBLIC_TOP_K_MAX + 1, u32::MAX] {
            let mut req = base_request();
            req.top_k = refused;
            assert!(
                matches!(
                    RepoMapPolicy::validate_query(&req),
                    Err(CoreError::Typed { code, .. }) if code == quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                        quanta_index_contract::lex::LexicalErrorCode::QueryTopKOutOfRange
                    )
                ),
                "top_k={refused} must be refused with {TOP_K_OUT_OF_RANGE_CODE}"
            );
        }
    }

    #[test]
    fn top_k_inside_the_public_range_is_accepted_at_both_ends() {
        for accepted in [1, PUBLIC_TOP_K_MAX] {
            let mut req = base_request();
            req.top_k = accepted;
            assert!(
                RepoMapPolicy::validate_query(&req).is_ok(),
                "top_k={accepted} is inside the public range"
            );
        }
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
