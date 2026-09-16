use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{ManifestGeneration, PUBLIC_TOP_K_MAX};

use crate::error::{CoreError, validate_internal_fetch_size, validate_query_top_k};

#[derive(Debug, Default, Clone, Copy)]
pub struct SemanticPolicy;

impl SemanticPolicy {
    /// The public result cap shared by every query route.
    #[must_use]
    pub const fn max_top_k() -> u32 {
        PUBLIC_TOP_K_MAX
    }

    /// Accept or refuse a caller's `top_k` under the shared contract.
    ///
    /// Delegates to the one policy every route uses so the semantic route
    /// cannot accept a value the dispatcher's continuation probe later
    /// refuses, and reports the same typed code as every other route.
    pub fn validate_top_k(top_k: u32) -> Result<(), CoreError> {
        let _accepted = validate_query_top_k(top_k)?;
        Ok(())
    }

    /// Accept or refuse the fetch size an adapter is asked to serve.
    ///
    /// Adapters receive the continuation fetch size (public cap plus one) or a
    /// hybrid over-fetch, never the caller's `top_k`; checking them against the
    /// public cap refused the public maximum one layer below where it had just
    /// been accepted.
    pub fn validate_fetch_size(fetch: u32) -> Result<(), CoreError> {
        let _accepted = validate_internal_fetch_size(fetch)?;
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
    fn validate_top_k_follows_the_shared_public_range() {
        // The public maximum is accepted; the route may not narrow it.
        assert!(SemanticPolicy::validate_top_k(PUBLIC_TOP_K_MAX).is_ok());
        // Above the ceiling and zero are refused with the one shared code.
        for refused in [0, PUBLIC_TOP_K_MAX + 1] {
            match SemanticPolicy::validate_top_k(refused) {
                Err(CoreError::Typed { code, .. }) => {
                    assert_eq!(code, quanta_index_contract::TOP_K_OUT_OF_RANGE_CODE);
                }
                other => panic!("top_k={refused} must be refused with the shared code: {other:?}"),
            }
        }
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
