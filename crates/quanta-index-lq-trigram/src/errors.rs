//! Typed errors for the LEX-02 trigram index.
//!
//! Every failure path through [`crate::builder`], [`crate::index`],
//! [`crate::query`], and [`mod@crate::regex_prefilter`] maps to exactly one
//! [`TrigramErrorCode`] variant. Production code paths never panic, never
//! silently default, and never widen a cap to mask a missing authoritative
//! result.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of trigram-index failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TrigramErrorCode {
    /// Builder/index was constructed with a generation id of `0`.
    InvalidGeneration,
    /// A per-query or per-candidate-set cap was exceeded.
    ///
    /// Carrier is always paired with a [`LimitDimension`] tag describing
    /// which cap fired.
    PlanLimitExceeded,
    /// CBOR decode failure when loading a `TrigramIndex`.
    IndexDeserialize,
    /// Regex prefilter was asked to operate on an unusable literal set
    /// (e.g. a pure-wildcard regex with no extractable literal). Caller
    /// must drop to the verify-only path; this is NOT a silent fallback.
    RegexPrefilterUnusable,
    /// CBOR payload decoded but failed invariant checks.
    IndexCorrupted,
}

impl TrigramErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::InvalidGeneration => "INVALID_GENERATION",
            Self::PlanLimitExceeded => "PLAN_LIMIT_EXCEEDED",
            Self::IndexDeserialize => "INDEX_DESERIALIZE",
            Self::RegexPrefilterUnusable => "REGEX_PREFILTER_UNUSABLE",
            Self::IndexCorrupted => "INDEX_CORRUPTED",
        }
    }

    /// Inverse of [`TrigramErrorCode::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "INVALID_GENERATION" => Self::InvalidGeneration,
            "PLAN_LIMIT_EXCEEDED" => Self::PlanLimitExceeded,
            "INDEX_DESERIALIZE" => Self::IndexDeserialize,
            "REGEX_PREFILTER_UNUSABLE" => Self::RegexPrefilterUnusable,
            "INDEX_CORRUPTED" => Self::IndexCorrupted,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for TrigramErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for TrigramErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for TrigramErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = TrigramErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("TrigramErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<TrigramErrorCode, E> {
                TrigramErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<TrigramErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Closed taxonomy of cap-dimension tags carried by
/// [`TrigramErrorCode::PlanLimitExceeded`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LimitDimension {
    /// Trigram set extracted from a query exceeded the per-query trigram
    /// cap (`MAX_TRIGRAMS_PER_QUERY`).
    Trigrams,
    /// Candidate set produced before the verify step exceeded the
    /// pre-verify candidate cap (`MAX_CANDIDATE_PRE_VERIFY`).
    CandidateSet,
    /// Input bytes were not usable (caller-side cap; reserved).
    InputBytes,
}

impl LimitDimension {
    /// `kebab-case` wire representation, matching the DSL `dimension=`
    /// tag carried in `PLAN_LIMIT_EXCEEDED` errors.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Trigrams => "trigram-set",
            Self::CandidateSet => "trigram-candidate-set",
            Self::InputBytes => "input-bytes",
        }
    }

    /// Inverse of [`LimitDimension::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "trigram-set" => Self::Trigrams,
            "trigram-candidate-set" => Self::CandidateSet,
            "input-bytes" => Self::InputBytes,
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
                f.write_str("LimitDimension kebab-case string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LimitDimension, E> {
                LimitDimension::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<LimitDimension>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete trigram failure carrying engineering-facing detail and an
/// optional [`LimitDimension`] tag (set only when the code is
/// [`TrigramErrorCode::PlanLimitExceeded`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TrigramError {
    pub code: TrigramErrorCode,
    pub dimension: Option<LimitDimension>,
    pub detail: Box<str>,
}

impl TrigramError {
    #[must_use]
    pub fn new(code: TrigramErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            dimension: None,
            detail: detail.into(),
        }
    }

    #[must_use]
    pub fn plan_limit(dim: LimitDimension, detail: impl Into<Box<str>>) -> Self {
        Self {
            code: TrigramErrorCode::PlanLimitExceeded,
            dimension: Some(dim),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for TrigramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.dimension {
            Some(d) => write!(f, "{}[dimension={}]: {}", self.code, d, self.detail),
            None => write!(f, "{}: {}", self.code, self.detail),
        }
    }
}

impl core::error::Error for TrigramError {}

#[cfg(test)]
mod tests {
    use super::{LimitDimension, TrigramError, TrigramErrorCode};

    const ALL_CODES: &[TrigramErrorCode] = &[
        TrigramErrorCode::InvalidGeneration,
        TrigramErrorCode::PlanLimitExceeded,
        TrigramErrorCode::IndexDeserialize,
        TrigramErrorCode::RegexPrefilterUnusable,
        TrigramErrorCode::IndexCorrupted,
    ];

    const ALL_DIMS: &[LimitDimension] = &[
        LimitDimension::Trigrams,
        LimitDimension::CandidateSet,
        LimitDimension::InputBytes,
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
            assert_eq!(TrigramErrorCode::from_code_str(c.as_code_str()), Some(*c));
        }
    }

    #[test]
    fn code_unknown_returns_none() {
        assert_eq!(TrigramErrorCode::from_code_str("NOT_A_CODE"), None);
        assert_eq!(TrigramErrorCode::from_code_str(""), None);
    }

    #[test]
    fn dim_strs_roundtrip() {
        for d in ALL_DIMS {
            assert_eq!(LimitDimension::from_code_str(d.as_code_str()), Some(*d));
        }
    }

    #[test]
    fn display_carries_dimension_when_set() {
        let e = TrigramError::plan_limit(LimitDimension::Trigrams, "too many");
        let s = format!("{e}");
        assert!(s.contains("PLAN_LIMIT_EXCEEDED"));
        assert!(s.contains("trigram-set"));
        assert!(s.contains("too many"));
    }

    #[test]
    fn display_omits_dimension_when_absent() {
        let e = TrigramError::new(TrigramErrorCode::InvalidGeneration, "zero");
        let s = format!("{e}");
        assert!(s.contains("INVALID_GENERATION"));
        assert!(!s.contains("dimension="));
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in ALL_CODES {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(c, &mut buf);
            assert!(w.is_ok(), "serialize failed for {c:?}");
            let read: Result<TrigramErrorCode, _> = ciborium::de::from_reader(buf.as_slice());
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
}
