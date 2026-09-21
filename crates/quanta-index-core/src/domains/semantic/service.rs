use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use quanta_index_contract::lex::LexicalErrorCode;
use quanta_index_contract::{EmbeddingNormalization, PUBLIC_TOP_K_MAX};

use crate::domains::observability::{MetricPointV1, MetricSourcePort};
use crate::domains::semantic::outbound::TextEmbeddingProvider;
use crate::error::{CoreError, validate_internal_fetch_size, validate_query_top_k};
use crate::request_budget::RequestBudgetV1;

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
    raw_norms: Arc<RawNormTallies>,
}

/// How far the raw provider's vectors were from unit before the wrapper
/// normalized them (QI-BB-031 보완 #4).
///
/// Every raw vector of every batch the wrapper returned is counted; a
/// batch it refused records nothing. "Off unit" is a norm outside
/// [`L2_UNIT_NORM_TOLERANCE`] of one: a provider that promises unit
/// vectors and drifts shows here before any consumer depends on it.
#[derive(Debug, Default)]
pub struct RawNormTallies {
    normalized: AtomicU64,
    off_unit: AtomicU64,
    /// Bits of the largest `|norm - 1|` seen: a non-negative `f64`'s bits
    /// order like its value, so `fetch_max` keeps the maximum.
    max_deviation_bits: AtomicU64,
}

impl RawNormTallies {
    fn record(&self, normalized: u64, off_unit: u64, max_deviation: f64) {
        let _prior = self.normalized.fetch_add(normalized, Ordering::Relaxed);
        let _prior = self.off_unit.fetch_add(off_unit, Ordering::Relaxed);
        let _prior = self
            .max_deviation_bits
            .fetch_max(max_deviation.to_bits(), Ordering::Relaxed);
    }
}

impl MetricSourcePort for RawNormTallies {
    /// `semantic_embedding_raw_vectors_normalized_total`,
    /// `semantic_embedding_raw_vectors_off_unit_total` and the gauge
    /// `semantic_embedding_raw_norm_deviation_max`.
    fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
        Ok(vec![
            MetricPointV1::counter(
                "semantic_embedding_raw_vectors_normalized_total",
                self.normalized.load(Ordering::Relaxed),
            ),
            MetricPointV1::counter(
                "semantic_embedding_raw_vectors_off_unit_total",
                self.off_unit.load(Ordering::Relaxed),
            ),
            MetricPointV1::gauge(
                "semantic_embedding_raw_norm_deviation_max",
                f64::from_bits(self.max_deviation_bits.load(Ordering::Relaxed)),
            ),
        ])
    }
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
        Ok(Self {
            inner,
            raw_norms: Arc::new(RawNormTallies::default()),
        })
    }

    /// The raw-norm tallies, for the metrics scrape.
    #[must_use]
    pub fn raw_norm_tallies(&self) -> Arc<RawNormTallies> {
        Arc::clone(&self.raw_norms)
    }
}

impl<P: TextEmbeddingProvider> L2UnitEmbeddingProvider<P> {
    /// Validate the raw batch and normalize every vector in place: the one
    /// normalization both the budgeted and the plain batch go through.
    fn normalize_batch(
        &self,
        texts: &[&str],
        mut vectors: Vec<Vec<f32>>,
    ) -> Result<Vec<Vec<f32>>, CoreError> {
        if vectors.len() != texts.len() {
            return Err(CoreError::Storage(format!(
                "semantic: provider {} returned {} vectors for {} texts",
                self.inner.model_id(),
                vectors.len(),
                texts.len()
            )));
        }
        let dimension = self.inner.dimension();
        let mut off_unit = 0_u64;
        let mut max_deviation = 0.0_f64;
        for vector in &mut vectors {
            SemanticPolicy::validate_embedding_vector_v1(
                vector,
                dimension,
                EmbeddingNormalization::None,
            )?;
            let deviation = (l2_norm_v1(vector)? - 1.0).abs();
            if deviation > L2_UNIT_NORM_TOLERANCE {
                off_unit = off_unit.saturating_add(1);
            }
            max_deviation = max_deviation.max(deviation);
            SemanticPolicy::normalize_l2_unit_v1(vector)?;
            SemanticPolicy::validate_embedding_vector_v1(
                vector,
                dimension,
                EmbeddingNormalization::L2Unit,
            )?;
        }
        let normalized = u64::try_from(vectors.len()).map_err(|error| {
            CoreError::Storage(format!("semantic: batch size overflow: {error}"))
        })?;
        self.raw_norms.record(normalized, off_unit, max_deviation);
        Ok(vectors)
    }
}

impl<P: TextEmbeddingProvider> TextEmbeddingProvider for L2UnitEmbeddingProvider<P> {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, CoreError> {
        let vectors = self.inner.embed_batch(texts)?;
        self.normalize_batch(texts, vectors)
    }

    fn embed_batch_within(
        &self,
        texts: &[&str],
        budget: &RequestBudgetV1,
    ) -> Result<Vec<Vec<f32>>, CoreError> {
        let vectors = self.inner.embed_batch_within(texts, budget)?;
        self.normalize_batch(texts, vectors)
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
        code: LexicalErrorCode::SemInvalidVector.into(),
        message,
    }
}

#[cfg(test)]
mod tests {
    //! Coverage gaps surfaced by `cargo mutants` (84 mutants on
    //! `quanta-index-core`, 21 missed). The tests below kill the specific
    //! mutations:
    //!
    //! - the L2 norm's `*` → `+` mutation (`sum += component * component`):
    //!   `[1.0, -1.0]` has norm `sqrt(2)` under multiplication and passes a
    //!   raw contract, but sums to zero under addition and would be refused
    //!   as zero-norm — exercising both arms catches the swap.
    //! - `validate_top_k`: `>` → `>=` mutation on line 26.
    //!   `top_k = MAX_TOP_K` must be accepted (boundary kept by `>`),
    //!   `top_k = MAX_TOP_K + 1` must be rejected.

    use super::*;

    #[test]
    fn the_norm_squares_its_components() {
        // Under `+`, `1 + (-1) = 0` → zero norm → refused; under `*` the
        // norm is sqrt(2) and a raw contract accepts it.
        assert!(
            SemanticPolicy::validate_embedding_vector_v1(
                &[1.0, -1.0],
                2,
                quanta_index_contract::EmbeddingNormalization::None
            )
            .is_ok()
        );
        assert!(
            SemanticPolicy::validate_embedding_vector_v1(
                &[0.0, 0.0],
                2,
                quanta_index_contract::EmbeddingNormalization::None
            )
            .is_err()
        );
    }

    #[test]
    fn validate_top_k_follows_the_shared_public_range() {
        // The public maximum is accepted; the route may not narrow it.
        assert!(SemanticPolicy::validate_top_k(PUBLIC_TOP_K_MAX).is_ok());
        // Above the ceiling and zero are refused with the one shared code.
        for refused in [0, PUBLIC_TOP_K_MAX + 1] {
            match SemanticPolicy::validate_top_k(refused) {
                Err(CoreError::Typed { code, .. }) => {
                    assert_eq!(
                        code,
                        quanta_index_contract::SearchPlaneErrorCodeV2::Lexical(
                            LexicalErrorCode::QueryTopKOutOfRange
                        )
                    );
                }
                other => panic!("top_k={refused} must be refused with the shared code: {other:?}"),
            }
        }
    }
}
