use quanta_index_contract::{LqExpr, LqQuery, ManifestGeneration};

use crate::error::CoreError;

#[derive(Debug, Default, Clone, Copy)]
pub struct LexicalPolicy;

impl LexicalPolicy {
    /// Reject queries this domain cannot serve. Mirrors the prior `QueryPolicy`
    /// semantics: `MatchAll` is rejected as policy because lexical engines have
    /// no bounded semantics for "return everything".
    pub fn validate_query(query: &LqQuery) -> Result<(), CoreError> {
        if matches!(query.expr, LqExpr::MatchAll) {
            return Err(CoreError::InvalidContract(
                "lexical: MatchAll is rejected (use explicit Raw/All/Any/Not)".to_string(),
            ));
        }
        Ok(())
    }

    /// Reject activation requests when the generation has not been materialized.
    pub fn validate_query_against_readiness(
        target: ManifestGeneration,
        materialized: Option<ManifestGeneration>,
    ) -> Result<(), CoreError> {
        match materialized {
            Some(active) if active.get() >= target.get() => Ok(()),
            Some(active) => Err(CoreError::NotReady(format!(
                "lexical: requested generation {} but materialized only up to {}",
                target.get(),
                active.get()
            ))),
            None => Err(CoreError::NotReady(
                "lexical: no materialized generation yet".to_string(),
            )),
        }
    }
}
