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

#[cfg(test)]
mod tests {
    //! Coverage gaps surfaced by `cargo mutants` (84 mutants on
    //! `quanta-index-core`, 21 missed). The tests below kill the specific
    //! mutations:
    //!
    //! - `validate_query_vector`: `*` → `+` mutation on line 50 (`norm_sq +=
    //!   component * component`). With the multiplication, `[3.0, 4.0]`
    //!   gives `norm_sq = 25.0`; with the buggy addition it would give
    //!   `norm_sq = 14.0`. We can't observe `norm_sq` directly, but the
    //!   only behavioural exit it controls is the zero-norm rejection at
    //!   line 52. A vector like `[1.0, -1.0]` has `norm = sqrt(2)` under
    //!   multiplication (passes) but `norm = 0.0` under addition (fails) —
    //!   exercising both arms catches the swap.
    //! - `validate_top_k`: `>` → `>=` mutation on line 26.
    //!   `top_k = MAX_TOP_K` must be accepted (boundary kept by `>`),
    //!   `top_k = MAX_TOP_K + 1` must be rejected.
    //! - `validate_query_against_readiness`: `>=` → `<` mutation on line 66.
    //!   At equality `active == target` the request must succeed under
    //!   `>=`; under `<` (mutant) it would fail. Exact equality test
    //!   catches both swaps simultaneously.

    use super::*;

    #[test]
    fn validate_query_vector_kills_norm_sq_mul_to_add_mutation() {
        // Under `+`, `1 + (-1) = 0` → norm_sq is 0 → reject.
        // Under `*`, `1*1 + (-1)*(-1) = 2` → norm_sq is 2 → accept.
        // Test that the implementation accepts this vector — kills the
        // `*` → `+` mutant which would reject it.
        assert!(SemanticPolicy::validate_query_vector(&[1.0, -1.0]).is_ok());
        // Counter-check: pure-zero vector must still be rejected
        // regardless of the mutation.
        assert!(SemanticPolicy::validate_query_vector(&[0.0, 0.0]).is_err());
    }

    #[test]
    fn validate_top_k_kills_boundary_gt_to_ge_mutation() {
        // Boundary: top_k == MAX_TOP_K must be accepted under `>`.
        // Under `>=` (mutant), it would be rejected.
        assert!(SemanticPolicy::validate_top_k(MAX_TOP_K).is_ok());
        // Above the ceiling must still be rejected.
        assert!(SemanticPolicy::validate_top_k(MAX_TOP_K + 1).is_err());
        // Zero must still be rejected (covers the `== 0` arm).
        assert!(SemanticPolicy::validate_top_k(0).is_err());
    }

    #[test]
    fn validate_query_against_readiness_kills_ge_to_lt_mutation_at_equality() {
        // At exact equality `active == target` the readiness check must
        // succeed under `>=`. Under `<` (mutant), it would fail.
        let pin = ManifestGeneration::new(7);
        assert!(SemanticPolicy::validate_query_against_readiness(pin, Some(pin)).is_ok());
        // Strictly newer materialized generation: still OK.
        assert!(
            SemanticPolicy::validate_query_against_readiness(
                ManifestGeneration::new(7),
                Some(ManifestGeneration::new(8)),
            )
            .is_ok()
        );
        // Older materialized: must fail-closed.
        assert!(
            SemanticPolicy::validate_query_against_readiness(
                ManifestGeneration::new(8),
                Some(ManifestGeneration::new(7)),
            )
            .is_err()
        );
        // None: must fail-closed.
        assert!(SemanticPolicy::validate_query_against_readiness(pin, None).is_err());
    }
}
