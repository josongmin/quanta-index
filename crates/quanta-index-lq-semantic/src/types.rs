//! Stable types shared between the cosine kernel, index, and executor.
//!
//! [`DocId`] mirrors the LEX-02/03/05 newtype shape so sibling indices
//! can join on the same id without re-implementing a parallel type.
//!
//! [`Embedding`] is the typed query / corpus vector. Its constructor
//! enforces every documented validation gate (finiteness, dim bound)
//! per SEM-01 spec §3.3 / §4.1 / §9.2 — invalid vectors never reach the
//! cosine kernel.
//!
//! [`DistanceMetric`] is the closed metric enum. Only [`DistanceMetric::Cosine`]
//! is supported at MVP; L2 and Dot return
//! [`crate::errors::SemanticErrorCode::SemMetricUnsupported`] at
//! query-construction time.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

use crate::errors::{LimitDimension, SemanticError, SemanticErrorCode};

/// Maximum embedding dimension supported at MVP.
///
/// Per SEM-01 spec §9.2: covers `text-embedding-3-small/large`,
/// `e5-mistral`, and the producer's current model. Above this,
/// [`SemanticErrorCode::PlanLimitExceeded`] with
/// [`LimitDimension::EmbeddingDim`] fires.
pub const MAX_EMBEDDING_DIM: usize = 1024;

/// Maximum `top_k` accepted by the executor. Matches the DSL §13
/// `count:` ceiling and SEM-01 spec §9.2 (`PLAN_LIMIT_EXCEEDED` at
/// `top_k > 10_000`).
pub const MAX_TOP_K: u32 = 10_000;

/// Corpus-size cutoff below which the executor uses exact NN.
///
/// Exact NN is deterministic by construction. Above this cutoff, the
/// executor fails closed with
/// [`SemanticErrorCode::SemAnnNondeterministic`] because pinned-seed
/// HNSW is not yet shipped (per SEM-01 spec §4.4 / risk R-ANN-DET). The
/// cutoff value is documented in the spec.
pub const EXACT_NN_CUTOFF: usize = 100_000;

/// Stable document identifier. Newtype around `u64`; equality and
/// ordering match the wrapped value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(pub u64);

impl DocId {
    /// Construct from a raw `u64`.
    #[must_use]
    pub const fn new(v: u64) -> Self {
        Self(v)
    }

    /// Borrow the wrapped `u64`.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for DocId {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<DocId> for u64 {
    fn from(d: DocId) -> Self {
        d.0
    }
}

impl fmt::Display for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl serde::Serialize for DocId {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_u64(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for DocId {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = DocId;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("DocId u64")
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<DocId, E> {
                Ok(DocId(v))
            }
            fn visit_u32<E: serde::de::Error>(self, v: u32) -> Result<DocId, E> {
                Ok(DocId(u64::from(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<DocId, E> {
                if v < 0 {
                    return Err(E::custom("DocId must be non-negative"));
                }
                let u = u64::try_from(v)
                    .map_err(|err| E::custom(format!("DocId out of u64 range: {err}")))?;
                Ok(DocId(u))
            }
        }
        de.deserialize_u64(V)
    }
}

/// Typed query / corpus embedding.
///
/// Construction enforces:
///
/// 1. non-empty (dim >= 1),
/// 2. dim <= [`MAX_EMBEDDING_DIM`],
/// 3. every component finite (no NaN, no `+inf`, no `-inf`).
///
/// These gates run once at construction so the cosine kernel can assume
/// validity. Zero-norm vectors are *not* rejected here (a vector of all
/// zeros is still finite-valued) — they are caught at the cosine call
/// because the cosine of a zero-norm vector is undefined.
///
/// The dim is captured into a `u32` field at construction so callers
/// observe the same numeric type the manifest pins. The cast is
/// validated against [`MAX_EMBEDDING_DIM`] before storage.
#[derive(Clone, Debug, PartialEq)]
pub struct Embedding {
    vector: Vec<f32>,
    dim: u32,
}

impl Embedding {
    /// Construct an [`Embedding`] from a raw `Vec<f32>`. Rejects empty
    /// vectors, vectors with `dim > MAX_EMBEDDING_DIM`, and vectors
    /// containing any non-finite component.
    pub fn new(vector: Vec<f32>) -> Result<Self, SemanticError> {
        if vector.is_empty() {
            return Err(SemanticError::new(
                SemanticErrorCode::SemInvalidVector,
                "embedding vector is empty",
            ));
        }
        if vector.len() > MAX_EMBEDDING_DIM {
            return Err(SemanticError::plan_limit(
                LimitDimension::EmbeddingDim,
                format!(
                    "embedding dim {} exceeds MAX_EMBEDDING_DIM {}",
                    vector.len(),
                    MAX_EMBEDDING_DIM
                ),
            ));
        }
        for (i, v) in vector.iter().enumerate() {
            if !v.is_finite() {
                return Err(SemanticError::new(
                    SemanticErrorCode::SemInvalidVector,
                    format!("embedding component {i} is non-finite: {v}"),
                ));
            }
        }
        // `vector.len()` is bounded by `MAX_EMBEDDING_DIM` (1024); the
        // `u32::try_from` is guaranteed to succeed but we keep the
        // typed conversion so a future bound bump cannot silently
        // truncate.
        let dim = u32::try_from(vector.len()).map_err(|e| {
            SemanticError::new(
                SemanticErrorCode::IndexCorrupted,
                format!("vector length {} did not fit in u32: {e}", vector.len()),
            )
        })?;
        Ok(Self { vector, dim })
    }

    /// Borrow the underlying f32 slice.
    #[must_use]
    pub fn as_slice(&self) -> &[f32] {
        &self.vector
    }

    /// Dimension of the embedding (always >= 1 and <= [`MAX_EMBEDDING_DIM`]).
    #[must_use]
    pub const fn dim(&self) -> u32 {
        self.dim
    }
}

/// Closed distance-metric enum.
///
/// Only [`DistanceMetric::Cosine`] is supported at MVP. [`DistanceMetric::L2Reserved`]
/// and [`DistanceMetric::DotReserved`] are reserved name-wise per
/// SEM-01 spec §3.4 / §4.1; attempting to use them at the query
/// surface yields [`SemanticErrorCode::SemMetricUnsupported`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DistanceMetric {
    Cosine,
    L2Reserved,
    DotReserved,
}

impl DistanceMetric {
    /// Stable wire string.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Cosine => "COSINE",
            Self::L2Reserved => "L2_RESERVED",
            Self::DotReserved => "DOT_RESERVED",
        }
    }

    /// Inverse of [`DistanceMetric::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "COSINE" => Self::Cosine,
            "L2_RESERVED" => Self::L2Reserved,
            "DOT_RESERVED" => Self::DotReserved,
            _ => return None,
        };
        Some(v)
    }

    /// `true` for [`DistanceMetric::Cosine`]; `false` for reserved
    /// variants.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Cosine)
    }

    /// Returns `Ok(())` if the metric is supported at MVP; otherwise
    /// returns [`SemanticErrorCode::SemMetricUnsupported`].
    pub fn require_supported(self) -> Result<(), SemanticError> {
        if self.is_supported() {
            Ok(())
        } else {
            Err(SemanticError::new(
                SemanticErrorCode::SemMetricUnsupported,
                format!("metric {self} is reserved but not supported at MVP"),
            ))
        }
    }
}

impl fmt::Display for DistanceMetric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for DistanceMetric {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for DistanceMetric {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = DistanceMetric;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("DistanceMetric SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<DistanceMetric, E> {
                DistanceMetric::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<DistanceMetric>"]))
            }
        }
        de.deserialize_str(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{DistanceMetric, DocId, EXACT_NN_CUTOFF, Embedding, MAX_EMBEDDING_DIM, MAX_TOP_K};
    use crate::errors::{LimitDimension, SemanticErrorCode};

    #[test]
    fn constants_match_spec() {
        assert_eq!(MAX_EMBEDDING_DIM, 1024);
        assert_eq!(MAX_TOP_K, 10_000);
        assert_eq!(EXACT_NN_CUTOFF, 100_000);
    }

    #[test]
    fn docid_roundtrip_u64() {
        let d = DocId::from(42u64);
        assert_eq!(d.get(), 42);
        let v: u64 = d.into();
        assert_eq!(v, 42);
    }

    #[test]
    fn docid_serde_roundtrip_via_ciborium() {
        let d = DocId(7);
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&d, &mut buf) {
            assert!(false, "{e}");
            return;
        }
        match ciborium::de::from_reader::<DocId, _>(buf.as_slice()) {
            Ok(g) => assert_eq!(g, d),
            Err(e) => assert!(false, "{e}"),
        }
    }

    #[test]
    fn embedding_rejects_empty() {
        match Embedding::new(Vec::new()) {
            Ok(_) => assert!(false, "must reject empty"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn embedding_rejects_oversized() {
        let v: Vec<f32> = vec![0.0_f32; MAX_EMBEDDING_DIM + 1];
        match Embedding::new(v) {
            Ok(_) => assert!(false, "must reject dim > MAX"),
            Err(e) => {
                assert_eq!(e.code, SemanticErrorCode::PlanLimitExceeded);
                assert_eq!(e.dimension, Some(LimitDimension::EmbeddingDim));
            }
        }
    }

    #[test]
    fn embedding_accepts_max_dim() {
        let v: Vec<f32> = vec![1.0_f32; MAX_EMBEDDING_DIM];
        let e = match Embedding::new(v) {
            Ok(e) => e,
            Err(err) => {
                assert!(false, "must accept max dim: {err}");
                return;
            }
        };
        let Ok(dim_u32) = u32::try_from(MAX_EMBEDDING_DIM) else {
            assert!(false, "dim should fit in u32");
            return;
        };
        assert_eq!(e.dim(), dim_u32);
        assert_eq!(e.as_slice().len(), MAX_EMBEDDING_DIM);
    }

    #[test]
    fn embedding_rejects_nan() {
        match Embedding::new(vec![1.0_f32, f32::NAN, 3.0_f32]) {
            Ok(_) => assert!(false, "must reject NaN"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn embedding_rejects_pos_inf() {
        match Embedding::new(vec![1.0_f32, f32::INFINITY, 3.0_f32]) {
            Ok(_) => assert!(false, "must reject +inf"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn embedding_rejects_neg_inf() {
        match Embedding::new(vec![1.0_f32, f32::NEG_INFINITY, 3.0_f32]) {
            Ok(_) => assert!(false, "must reject -inf"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemInvalidVector),
        }
    }

    #[test]
    fn embedding_accepts_all_zeros() {
        // All-zero vector is finite; cosine call will reject it.
        let e = match Embedding::new(vec![0.0_f32, 0.0_f32, 0.0_f32]) {
            Ok(e) => e,
            Err(err) => {
                assert!(false, "{err}");
                return;
            }
        };
        assert_eq!(e.dim(), 3);
    }

    #[test]
    fn distance_metric_only_cosine_supported() {
        assert!(DistanceMetric::Cosine.is_supported());
        assert!(!DistanceMetric::L2Reserved.is_supported());
        assert!(!DistanceMetric::DotReserved.is_supported());
    }

    #[test]
    fn distance_metric_require_supported_routes_through_code() {
        match DistanceMetric::Cosine.require_supported() {
            Ok(()) => {}
            Err(e) => assert!(false, "{e}"),
        }
        match DistanceMetric::L2Reserved.require_supported() {
            Ok(()) => assert!(false, "L2 must not be supported"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemMetricUnsupported),
        }
        match DistanceMetric::DotReserved.require_supported() {
            Ok(()) => assert!(false, "Dot must not be supported"),
            Err(e) => assert_eq!(e.code, SemanticErrorCode::SemMetricUnsupported),
        }
    }

    #[test]
    fn distance_metric_serde_roundtrip() {
        for m in [
            DistanceMetric::Cosine,
            DistanceMetric::L2Reserved,
            DistanceMetric::DotReserved,
        ] {
            let mut buf: Vec<u8> = Vec::new();
            if let Err(e) = ciborium::ser::into_writer(&m, &mut buf) {
                assert!(false, "{e}");
                continue;
            }
            match ciborium::de::from_reader::<DistanceMetric, _>(buf.as_slice()) {
                Ok(g) => assert_eq!(g, m),
                Err(e) => assert!(false, "{e}"),
            }
        }
    }
}
