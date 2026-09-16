use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{EmbeddingNormalization, ManifestGeneration, PUBLIC_TOP_K_MAX};

use crate::domains::semantic::outbound::TextEmbeddingProvider;
use crate::error::{CoreError, validate_internal_fetch_size, validate_query_top_k};

/// How far from 1.0 a unit vector's L2 norm may be before it is not one.
///
/// `f32` components normalized through an `f64` accumulator land within a
/// few ULPs; the gross defects this guards against (a vector scaled by 0.5,
/// a same-length cache corruption, a provider that never normalized) miss by
/// orders of magnitude more.
pub const L2_UNIT_NORM_TOLERANCE: f64 = 1e-3;

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

impl SemanticPolicy {
    /// Prove one embedding vector is what its contract says it is
    /// (QI-BB-031): exactly `dimension` finite components with a non-zero
    /// norm, and under [`EmbeddingNormalization::L2Unit`] a norm within
    /// [`L2_UNIT_NORM_TOLERANCE`] of one. Applied to fresh provider output,
    /// to cache hits, and to every ingested row, so the same defect is
    /// refused the same way on every path.
    pub fn validate_embedding_vector_v1(
        vector: &[f32],
        dimension: usize,
        normalization: EmbeddingNormalization,
    ) -> Result<(), CoreError> {
        if vector.len() != dimension {
            return Err(invalid_vector(format!(
                "semantic: embedding vector has {} components, contract dimension is {dimension}",
                vector.len()
            )));
        }
        let norm = l2_norm_v1(vector)?;
        if norm <= 0.0 {
            return Err(invalid_vector(
                "semantic: embedding vector has zero norm".to_string(),
            ));
        }
        if normalization == EmbeddingNormalization::L2Unit
            && (norm - 1.0).abs() > L2_UNIT_NORM_TOLERANCE
        {
            return Err(invalid_vector(format!(
                "semantic: embedding vector norm {norm} is not unit under the L2Unit contract (tolerance {L2_UNIT_NORM_TOLERANCE})"
            )));
        }
        Ok(())
    }

    /// Scale `vector` to unit L2 norm in place, deterministically: the norm
    /// is accumulated in `f64` and every component is divided by it. A
    /// vector with a non-finite component or zero norm cannot be normalized
    /// and is refused typed rather than turned into NaNs.
    pub fn normalize_l2_unit_v1(vector: &mut [f32]) -> Result<(), CoreError> {
        let norm = l2_norm_v1(vector)?;
        if norm <= 0.0 {
            return Err(invalid_vector(
                "semantic: cannot normalize a zero-norm embedding vector".to_string(),
            ));
        }
        for component in vector.iter_mut() {
            *component = narrow_unit_component(f64::from(*component) / norm);
        }
        Ok(())
    }
}

/// Narrow one unit-scaled component to `f32`.
///
/// The division of a finite `f32` by a positive finite `f64` norm is finite
/// and no larger in magnitude than one, so the narrowing cannot overflow; it
/// may round, which is the point of normalizing through `f64`.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "a unit-scaled component is within [-1, 1]; narrowing to f32 only rounds"
)]
fn narrow_unit_component(scaled: f64) -> f32 {
    scaled as f32
}

/// L2 norm accumulated in `f64`; refuses non-finite components typed.
fn l2_norm_v1(vector: &[f32]) -> Result<f64, CoreError> {
    let mut norm_sq = 0.0_f64;
    for (index, component) in vector.iter().enumerate() {
        if !component.is_finite() {
            return Err(invalid_vector(format!(
                "semantic: embedding vector component {index} is not finite ({component})"
            )));
        }
        let value = f64::from(*component);
        norm_sq += value * value;
    }
    if !norm_sq.is_finite() {
        return Err(invalid_vector(
            "semantic: embedding vector norm overflowed".to_string(),
        ));
    }
    Ok(norm_sq.sqrt())
}

/// Wraps a raw provider so every vector it returns is unit-normalized and
/// proven so (QI-BB-031).
///
/// The composition root applies this to every provider before either the
/// corpus derivation or the query path sees it, so both paths normalize
/// with the same code and record [`EmbeddingNormalization::L2Unit`]
/// truthfully. A raw vector with a non-finite component, a zero norm or the
/// wrong dimension fails the whole batch closed.
pub struct L2UnitEmbeddingProvider<P: TextEmbeddingProvider> {
    inner: P,
}

impl<P: TextEmbeddingProvider> L2UnitEmbeddingProvider<P> {
    /// Wrap a raw provider. A provider that already promises `L2Unit` is
    /// refused: stacking normalizers would hide which layer is trusted.
    pub fn new(inner: P) -> Result<Self, CoreError> {
        if inner.normalization() != EmbeddingNormalization::None {
            return Err(CoreError::InvalidContract(format!(
                "semantic: L2Unit wrapper requires a raw provider; {} already promises {:?}",
                inner.model_id(),
                inner.normalization()
            )));
        }
        Ok(Self { inner })
    }
}

impl<P: TextEmbeddingProvider> TextEmbeddingProvider for L2UnitEmbeddingProvider<P> {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        let mut vectors = self.inner.embed_batch(texts)?;
        if vectors.len() != texts.len() {
            return Err(CoreError::Storage(format!(
                "semantic: provider {} returned {} vectors for {} texts",
                self.inner.model_id(),
                vectors.len(),
                texts.len()
            )));
        }
        let dimension = self.inner.dimension();
        for vector in &mut vectors {
            SemanticPolicy::validate_embedding_vector_v1(
                vector,
                dimension,
                EmbeddingNormalization::None,
            )?;
            SemanticPolicy::normalize_l2_unit_v1(vector)?;
            SemanticPolicy::validate_embedding_vector_v1(
                vector,
                dimension,
                EmbeddingNormalization::L2Unit,
            )?;
        }
        Ok(vectors)
    }

    fn model_id(&self) -> &str {
        self.inner.model_id()
    }

    fn model_revision(&self) -> &str {
        self.inner.model_revision()
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    fn normalization(&self) -> EmbeddingNormalization {
        EmbeddingNormalization::L2Unit
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
