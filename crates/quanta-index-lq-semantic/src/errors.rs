//! Typed errors for the SEM-01 semantic vector adapter.
//!
//! Every failure path through [`crate::cosine`], [`crate::index`], and
//! [`crate::query`] maps to exactly one [`SemanticErrorCode`] variant.
//! Production code paths never panic, never silently default, never
//! return an empty `Vec` for an absent authority.
//!
//! Wire codes follow the `SEM_*` taxonomy locked in the SEM-01 spec
//! §4.1. `PlanLimitExceeded`, `IndexDeserialize`, and `IndexCorrupted`
//! mirror the sibling lexical crates so the planner can unify cap
//! surfaces.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of semantic-adapter failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SemanticErrorCode {
    /// Query vector dimension does not match the corpus dimension pinned
    /// at index build time. Per SEM-01 spec §4.1 / §8.
    SemDimMismatch,
    /// Generation is present in the catalog but the semantic sibling is
    /// not yet ready (analog of `STATE_NOT_READY: STALE_SIBLING`). Used
    /// by the future open path; reserved at MVP.
    SemNotReady,
    /// Vector contains a NaN, +inf, -inf, or has degenerate (zero) norm
    /// for cosine. Per SEM-01 spec §4.1, §5.3 step 3.
    SemInvalidVector,
    /// Caller requested a non-cosine metric. Per SEM-01 spec §3.4 / §4.1
    /// — L2 and Dot are reserved name-wise but unsupported at MVP.
    SemMetricUnsupported,
    /// ANN backend's pinned-seed contract has been violated, or the
    /// corpus size exceeded the deterministic exact-NN cutoff *and*
    /// the caller explicitly opted out of HNSW (`disable_hnsw=true`).
    /// Per SEM-01 spec §10 R-ANN-DET and §4.4.
    SemAnnNondeterministic,
    /// HNSW build / search parameters out of range. Surfaces from
    /// [`crate::hnsw::HnswParams::validate`] when `m`, `ef_construction`,
    /// or `ef_search` falls outside the documented bounds.
    SemHnswParamsInvalid,
    /// A per-query or per-corpus cap was exceeded. Carrier is always
    /// paired with a [`LimitDimension`] tag describing which cap fired.
    PlanLimitExceeded,
    /// CBOR decode failure when loading a `SemanticIndex`.
    IndexDeserialize,
    /// CBOR payload decoded but failed invariant checks (e.g. dim==0,
    /// generation==0, or vector dim mismatch within the corpus).
    IndexCorrupted,
}

impl SemanticErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::SemDimMismatch => "SEM_DIM_MISMATCH",
            Self::SemNotReady => "SEM_NOT_READY",
            Self::SemInvalidVector => "SEM_INVALID_VECTOR",
            Self::SemMetricUnsupported => "SEM_METRIC_UNSUPPORTED",
            Self::SemAnnNondeterministic => "SEM_ANN_NONDETERMINISTIC",
            Self::SemHnswParamsInvalid => "SEM_HNSW_PARAMS_INVALID",
            Self::PlanLimitExceeded => "PLAN_LIMIT_EXCEEDED",
            Self::IndexDeserialize => "INDEX_DESERIALIZE",
            Self::IndexCorrupted => "INDEX_CORRUPTED",
        }
    }

    /// Inverse of [`SemanticErrorCode::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "SEM_DIM_MISMATCH" => Self::SemDimMismatch,
            "SEM_NOT_READY" => Self::SemNotReady,
            "SEM_INVALID_VECTOR" => Self::SemInvalidVector,
            "SEM_METRIC_UNSUPPORTED" => Self::SemMetricUnsupported,
            "SEM_ANN_NONDETERMINISTIC" => Self::SemAnnNondeterministic,
            "SEM_HNSW_PARAMS_INVALID" => Self::SemHnswParamsInvalid,
            "PLAN_LIMIT_EXCEEDED" => Self::PlanLimitExceeded,
            "INDEX_DESERIALIZE" => Self::IndexDeserialize,
            "INDEX_CORRUPTED" => Self::IndexCorrupted,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for SemanticErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for SemanticErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for SemanticErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = SemanticErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SemanticErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<SemanticErrorCode, E> {
                SemanticErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<SemanticErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Closed taxonomy of cap-dimension tags carried by
/// [`SemanticErrorCode::PlanLimitExceeded`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LimitDimension {
    /// Embedding dimension exceeded [`crate::types::MAX_EMBEDDING_DIM`]
    /// (1024 at MVP per SEM-01 spec §9.2).
    EmbeddingDim,
    /// Requested `top_k` exceeded [`crate::types::MAX_TOP_K`] (`10_000`
    /// per SEM-01 spec §9.2 / DSL §13 `count:` ceiling).
    TopK,
    /// Corpus size exceeded the deterministic exact-NN cutoff. Reserved
    /// for future-build-time caps; runtime corpus overruns surface
    /// [`SemanticErrorCode::SemAnnNondeterministic`] instead.
    CorpusSize,
}

impl LimitDimension {
    /// `kebab-case` wire representation matching the DSL `dimension=`
    /// tag carried in `PLAN_LIMIT_EXCEEDED` errors.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::EmbeddingDim => "semantic.embedding_dim",
            Self::TopK => "semantic.top_k",
            Self::CorpusSize => "semantic.corpus_size",
        }
    }

    /// Inverse of [`LimitDimension::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "semantic.embedding_dim" => Self::EmbeddingDim,
            "semantic.top_k" => Self::TopK,
            "semantic.corpus_size" => Self::CorpusSize,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for LimitDimension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for LimitDimension {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for LimitDimension {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LimitDimension;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LimitDimension dotted-lowercase string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LimitDimension, E> {
                LimitDimension::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<LimitDimension>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete semantic-adapter failure carrying engineering-facing detail
/// and an optional [`LimitDimension`] tag (set only when the code is
/// [`SemanticErrorCode::PlanLimitExceeded`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SemanticError {
    pub code: SemanticErrorCode,
    pub dimension: Option<LimitDimension>,
    pub detail: Box<str>,
}

impl SemanticError {
    /// Construct an error with the given code and detail string.
    #[must_use]
    pub fn new(code: SemanticErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            dimension: None,
            detail: detail.into(),
        }
    }

    /// Construct a `PLAN_LIMIT_EXCEEDED` error tagged with the dimension
    /// that fired.
    #[must_use]
    pub fn plan_limit(dim: LimitDimension, detail: impl Into<Box<str>>) -> Self {
        Self {
            code: SemanticErrorCode::PlanLimitExceeded,
            dimension: Some(dim),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for SemanticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.dimension {
            Some(d) => write!(f, "{}[dimension={}]: {}", self.code, d, self.detail),
            None => write!(f, "{}: {}", self.code, self.detail),
        }
    }
}

impl core::error::Error for SemanticError {}

// Manual serde for SemanticError. Wire shape:
//   map { "code": <SemanticErrorCode>, "dimension": <LimitDimension|absent>, "detail": <str> }
impl serde::Serialize for SemanticError {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let n = if self.dimension.is_some() { 3 } else { 2 };
        let mut m = ser.serialize_map(Some(n))?;
        m.serialize_entry("code", &self.code)?;
        if let Some(d) = self.dimension.as_ref() {
            m.serialize_entry("dimension", d)?;
        }
        m.serialize_entry("detail", self.detail.as_ref())?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for SemanticError {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = SemanticError;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SemanticError map (code, dimension?, detail)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<SemanticError, M::Error> {
                let mut code: Option<SemanticErrorCode> = None;
                let mut dimension: Option<LimitDimension> = None;
                let mut detail: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "code" => {
                            if code.is_some() {
                                return Err(serde::de::Error::duplicate_field("code"));
                            }
                            code = Some(map.next_value()?);
                        }
                        "dimension" => {
                            if dimension.is_some() {
                                return Err(serde::de::Error::duplicate_field("dimension"));
                            }
                            dimension = Some(map.next_value()?);
                        }
                        "detail" => {
                            if detail.is_some() {
                                return Err(serde::de::Error::duplicate_field("detail"));
                            }
                            detail = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(
                                other,
                                &["code", "dimension", "detail"],
                            ));
                        }
                    }
                }
                let code = code.ok_or_else(|| serde::de::Error::missing_field("code"))?;
                let detail = detail.ok_or_else(|| serde::de::Error::missing_field("detail"))?;
                Ok(SemanticError {
                    code,
                    dimension,
                    detail: detail.into_boxed_str(),
                })
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{LimitDimension, SemanticError, SemanticErrorCode};

    const ALL_CODES: &[SemanticErrorCode] = &[
        SemanticErrorCode::SemDimMismatch,
        SemanticErrorCode::SemNotReady,
        SemanticErrorCode::SemInvalidVector,
        SemanticErrorCode::SemMetricUnsupported,
        SemanticErrorCode::SemAnnNondeterministic,
        SemanticErrorCode::SemHnswParamsInvalid,
        SemanticErrorCode::PlanLimitExceeded,
        SemanticErrorCode::IndexDeserialize,
        SemanticErrorCode::IndexCorrupted,
    ];

    const ALL_DIMS: &[LimitDimension] = &[
        LimitDimension::EmbeddingDim,
        LimitDimension::TopK,
        LimitDimension::CorpusSize,
    ];

    #[test]
    fn code_strs_are_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for c in ALL_CODES {
            let s = c.as_code_str();
            assert!(!seen.contains(&s), "duplicate code: {s}");
            seen.push(s);
        }
    }

    #[test]
    fn code_strs_roundtrip() {
        for c in ALL_CODES {
            assert_eq!(SemanticErrorCode::from_code_str(c.as_code_str()), Some(*c));
        }
    }

    #[test]
    fn code_unknown_returns_none() {
        assert_eq!(SemanticErrorCode::from_code_str("NOT_A_CODE"), None);
        assert_eq!(SemanticErrorCode::from_code_str(""), None);
    }

    #[test]
    fn five_sem_codes_present() {
        // Spec §4.1 — five new SEM_* codes must be representable.
        let want = [
            "SEM_DIM_MISMATCH",
            "SEM_NOT_READY",
            "SEM_INVALID_VECTOR",
            "SEM_METRIC_UNSUPPORTED",
            "SEM_ANN_NONDETERMINISTIC",
        ];
        for s in want {
            assert!(
                SemanticErrorCode::from_code_str(s).is_some(),
                "missing SEM_* code: {s}"
            );
        }
    }

    #[test]
    fn dim_strs_roundtrip() {
        for d in ALL_DIMS {
            assert_eq!(LimitDimension::from_code_str(d.as_code_str()), Some(*d));
        }
    }

    #[test]
    fn display_carries_dimension_when_set() {
        let e = SemanticError::plan_limit(LimitDimension::EmbeddingDim, "too big");
        let s = format!("{e}");
        assert!(s.contains("PLAN_LIMIT_EXCEEDED"));
        assert!(s.contains("semantic.embedding_dim"));
        assert!(s.contains("too big"));
    }

    #[test]
    fn display_omits_dimension_when_absent() {
        let e = SemanticError::new(SemanticErrorCode::SemDimMismatch, "127 vs 256");
        let s = format!("{e}");
        assert!(s.contains("SEM_DIM_MISMATCH"));
        assert!(!s.contains("dimension="));
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in ALL_CODES {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(c, &mut buf);
            assert!(w.is_ok(), "serialize failed for {c:?}");
            let read: Result<SemanticErrorCode, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *c),
                Err(e) => assert!(false, "deserialize failed for {c:?}: {e}"),
            }
        }
    }

    #[test]
    fn dim_serde_roundtrip_via_ciborium() {
        for d in ALL_DIMS {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(d, &mut buf);
            assert!(w.is_ok());
            let read: Result<LimitDimension, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *d),
                Err(e) => assert!(false, "deserialize failed for {d:?}: {e}"),
            }
        }
    }

    #[test]
    fn error_serde_roundtrip_with_dimension() {
        let e = SemanticError::plan_limit(LimitDimension::TopK, "k=10001");
        let mut buf: Vec<u8> = Vec::new();
        if let Err(err) = ciborium::ser::into_writer(&e, &mut buf) {
            assert!(false, "{err}");
            return;
        }
        match ciborium::de::from_reader::<SemanticError, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, e),
            Err(err) => assert!(false, "{err}"),
        }
    }

    #[test]
    fn error_serde_roundtrip_without_dimension() {
        let e = SemanticError::new(SemanticErrorCode::SemInvalidVector, "NaN at index 3");
        let mut buf: Vec<u8> = Vec::new();
        if let Err(err) = ciborium::ser::into_writer(&e, &mut buf) {
            assert!(false, "{err}");
            return;
        }
        match ciborium::de::from_reader::<SemanticError, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, e),
            Err(err) => assert!(false, "{err}"),
        }
    }
}
