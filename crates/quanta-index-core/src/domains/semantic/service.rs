use quanta_index_contract::ManifestGeneration;
use quanta_index_contract::lex::LexicalErrorCode;

use crate::error::CoreError;

#[derive(Debug, Default, Clone, Copy)]
pub struct SemanticPolicy;

const MAX_TOP_K: u32 = 10_000;

impl SemanticPolicy {
    #[must_use]
    pub const fn max_top_k() -> u32 {
        MAX_TOP_K
    }

    pub fn validate_top_k(top_k: u32) -> Result<(), CoreError> {
        if top_k == 0 {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::InvalidFilterValue
                    .as_code_str()
                    .to_string(),
                message: format!("semantic: top_k must be within 1..={MAX_TOP_K}, got {top_k}"),
            });
        }
        if top_k > MAX_TOP_K {
            return Err(CoreError::Typed {
                code: LexicalErrorCode::PlanLimitExceeded
                    .as_code_str()
                    .to_string(),
                message: format!("semantic: top_k {top_k} exceeds ceiling {MAX_TOP_K}"),
            });
        }
        Ok(())
    }

    pub fn validate_query_vector(query_vector: &[f32]) -> Result<(), CoreError> {
        if query_vector.is_empty() {
            return Err(invalid_vector(
                "semantic: query vector must not be empty".to_string(),
            ));
        }
        let mut norm_sq = 0.0_f32;
        for component in query_vector {
            if !component.is_finite() {
                return Err(invalid_vector(format!(
                    "semantic: query vector contains non-finite component {component}"
                )));
            }
            norm_sq += component * component;
        }
        if !norm_sq.is_finite() || norm_sq <= 0.0 {
            return Err(invalid_vector(
                "semantic: query vector must contain at least one non-zero finite component"
                    .to_string(),
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

fn invalid_vector(message: String) -> CoreError {
    CoreError::Typed {
        code: LexicalErrorCode::SemInvalidVector.as_code_str().to_string(),
        message,
    }
}
