//! Typed errors for the LEX-01 scorer.
//!
//! Every failure path through [`crate::bm25`], [`crate::idf`],
//! [`crate::builder`], and [`crate::scorer`] maps to exactly one
//! [`ScorerErrorCode`] variant. Production code paths never panic, never
//! silently default, and never recompute IDF on the fly to mask a missing
//! authoritative table.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of scoring / persistence failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScorerErrorCode {
    /// Builder/table was constructed with a generation id of `0`.
    InvalidGeneration,
    /// IDF table or builder finalised over an empty corpus.
    EmptyCorpus,
    /// `Bm25Params::new` rejected `k1` outside `(0.0, 5.0]` or `b` outside
    /// `[0.0, 1.0]`, or saw a non-finite input.
    InvalidBm25Param,
    /// CBOR decode failure when loading an `IdfTable`.
    IdfTableDeserialize,
    /// Score normalization produced a non-finite or out-of-envelope value.
    ScoreNormalizationFailed,
    /// Scorer hot path saw a NaN-bearing signal.
    NanSignal,
}

impl ScorerErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::InvalidGeneration => "INVALID_GENERATION",
            Self::EmptyCorpus => "EMPTY_CORPUS",
            Self::InvalidBm25Param => "INVALID_BM25_PARAM",
            Self::IdfTableDeserialize => "IDF_TABLE_DESERIALIZE",
            Self::ScoreNormalizationFailed => "SCORE_NORMALIZATION_FAILED",
            Self::NanSignal => "NAN_SIGNAL",
        }
    }

    /// Inverse of [`ScorerErrorCode::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "INVALID_GENERATION" => Self::InvalidGeneration,
            "EMPTY_CORPUS" => Self::EmptyCorpus,
            "INVALID_BM25_PARAM" => Self::InvalidBm25Param,
            "IDF_TABLE_DESERIALIZE" => Self::IdfTableDeserialize,
            "SCORE_NORMALIZATION_FAILED" => Self::ScoreNormalizationFailed,
            "NAN_SIGNAL" => Self::NanSignal,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for ScorerErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for ScorerErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for ScorerErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = ScorerErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("ScorerErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<ScorerErrorCode, E> {
                ScorerErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<ScorerErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete scorer failure carrying engineering-facing detail.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScorerError {
    pub code: ScorerErrorCode,
    pub detail: Box<str>,
}

impl ScorerError {
    #[must_use]
    pub fn new(code: ScorerErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for ScorerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl core::error::Error for ScorerError {}

#[cfg(test)]
mod tests {
    use super::{ScorerError, ScorerErrorCode};

    const ALL_CODES: &[ScorerErrorCode] = &[
        ScorerErrorCode::InvalidGeneration,
        ScorerErrorCode::EmptyCorpus,
        ScorerErrorCode::InvalidBm25Param,
        ScorerErrorCode::IdfTableDeserialize,
        ScorerErrorCode::ScoreNormalizationFailed,
        ScorerErrorCode::NanSignal,
    ];

    #[test]
    fn code_strs_are_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for c in ALL_CODES {
            let s = c.as_code_str();
            assert!(!seen.contains(&s), "duplicate code str: {s}");
            seen.push(s);
        }
        assert_eq!(seen.len(), ALL_CODES.len());
    }

    #[test]
    fn code_strs_roundtrip_via_from_code_str() {
        for c in ALL_CODES {
            let s = c.as_code_str();
            let parsed = ScorerErrorCode::from_code_str(s);
            assert_eq!(parsed, Some(*c));
        }
    }

    #[test]
    fn from_code_str_rejects_unknown() {
        assert_eq!(ScorerErrorCode::from_code_str("NOT_A_CODE"), None);
        assert_eq!(ScorerErrorCode::from_code_str(""), None);
    }

    #[test]
    fn display_includes_code_and_detail() {
        let e = ScorerError::new(ScorerErrorCode::EmptyCorpus, "no documents seen");
        let s = format!("{e}");
        assert!(s.contains("EMPTY_CORPUS"));
        assert!(s.contains("no documents seen"));
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in ALL_CODES {
            let mut buf: Vec<u8> = Vec::new();
            let write = ciborium::ser::into_writer(c, &mut buf);
            assert!(write.is_ok(), "serialize failed for {c:?}");
            let read: Result<ScorerErrorCode, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *c),
                Err(e) => assert!(false, "deserialize failed for {c:?}: {e}"),
            }
        }
    }
}
