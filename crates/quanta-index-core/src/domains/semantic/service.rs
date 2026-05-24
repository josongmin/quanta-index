use quanta_index_contract::ManifestGeneration;

use crate::error::CoreError;

#[derive(Debug, Default, Clone, Copy)]
pub struct SemanticPolicy;

impl SemanticPolicy {
    pub fn validate_top_k(top_k: u32) -> Result<(), CoreError> {
        if top_k == 0 {
            return Err(CoreError::InvalidContract(
                "semantic: top_k must be > 0".to_string(),
            ));
        }
        Ok(())
    }

    pub fn validate_query_against_readiness(
        target: ManifestGeneration,
        materialized: Option<ManifestGeneration>,
    ) -> Result<(), CoreError> {
        match materialized {
            Some(active) if active.get() >= target.get() => Ok(()),
            Some(active) => Err(CoreError::NotReady(format!(
                "semantic: requested generation {} but materialized only up to {}",
                target.get(),
                active.get()
            ))),
            None => Err(CoreError::NotReady(
                "semantic: no materialized generation yet".to_string(),
            )),
        }
    }
}
