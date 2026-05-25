//! Typed errors for the LEX-06 composite ranker.
//!
//! Every failure path through [`crate::weights`], [`crate::signals`],
//! [`crate::scorer`], [`crate::tiebreak`], and the explanation surface maps
//! to exactly one [`RankerErrorCode`] variant.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of ranker failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RankerErrorCode {
    /// `RankerWeightsV1::new` rejected a non-finite weight, an
    /// out-of-`[0.0, 1.0]` weight, or a sum that did not fall within the
    /// approximate-unit tolerance.
    InvalidWeights,
    /// Signal slot delivered `NaN`, `Inf`, or a value outside its declared
    /// domain envelope.
    RankInvalidSignal,
    /// CBOR decode failure when loading a `RankerWeightsV1`.
    WeightsDeserialize,
    /// Pinned `weights_hash` did not match the recomputed hash on a
    /// `CompositeScorer` instance.
    WeightsHashMismatch,
    /// CBOR encode failure while computing `weights_hash`. The codec
    /// path against `Vec<u8>` is infallible in practice; this variant
    /// exists so the function signature can propagate the typed error
    /// rather than fall back to a heuristic alternate-algorithm hash.
    WeightsEncodeFailed,
    /// Generation slot delivered `0` (reserved sentinel).
    InvalidGeneration,
}

impl RankerErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::InvalidWeights => "INVALID_WEIGHTS",
            Self::RankInvalidSignal => "RANK_INVALID_SIGNAL",
            Self::WeightsDeserialize => "WEIGHTS_DESERIALIZE",
            Self::WeightsHashMismatch => "WEIGHTS_HASH_MISMATCH",
            Self::WeightsEncodeFailed => "WEIGHTS_ENCODE_FAILED",
            Self::InvalidGeneration => "INVALID_GENERATION",
        }
    }

    /// Inverse of [`RankerErrorCode::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "INVALID_WEIGHTS" => Self::InvalidWeights,
            "RANK_INVALID_SIGNAL" => Self::RankInvalidSignal,
            "WEIGHTS_DESERIALIZE" => Self::WeightsDeserialize,
            "WEIGHTS_HASH_MISMATCH" => Self::WeightsHashMismatch,
            "WEIGHTS_ENCODE_FAILED" => Self::WeightsEncodeFailed,
            "INVALID_GENERATION" => Self::InvalidGeneration,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for RankerErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for RankerErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for RankerErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = RankerErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("RankerErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<RankerErrorCode, E> {
                RankerErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<RankerErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Named signal slots — attribution payload for `RANK_INVALID_SIGNAL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignalKind {
    /// BM25 score component from the LEX-01 lexical recall stage.
    Bm25,
    /// Path-prior classifier signal.
    PathPrior,
    /// Symbol-class boost signal.
    SymbolBoost,
    /// Document recency signal.
    Recency,
    /// `boost:` directive signal from `LqOptionSet::boost`.
    BoostDirective,
}

impl SignalKind {
    /// `snake_case` wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bm25 => "bm25",
            Self::PathPrior => "path_prior",
            Self::SymbolBoost => "symbol_boost",
            Self::Recency => "recency",
            Self::BoostDirective => "boost_directive",
        }
    }

    /// Inverse of [`SignalKind::as_str`].
    #[must_use]
    pub fn from_str_value(s: &str) -> Option<Self> {
        let v = match s {
            "bm25" => Self::Bm25,
            "path_prior" => Self::PathPrior,
            "symbol_boost" => Self::SymbolBoost,
            "recency" => Self::Recency,
            "boost_directive" => Self::BoostDirective,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for SignalKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for SignalKind {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for SignalKind {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = SignalKind;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SignalKind snake_case string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<SignalKind, E> {
                SignalKind::from_str_value(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<SignalKind>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete ranker failure with attribution payload.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RankerError {
    /// Closed-taxonomy code.
    pub code: RankerErrorCode,
    /// Signal slot attribution, when applicable.
    pub signal: Option<SignalKind>,
    /// Engineering-facing detail.
    pub detail: Box<str>,
}

impl RankerError {
    /// Construct a free-form ranker error without signal attribution.
    #[must_use]
    pub fn new(code: RankerErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            signal: None,
            detail: detail.into(),
        }
    }

    /// Construct a ranker error with signal attribution.
    #[must_use]
    pub fn signal(code: RankerErrorCode, signal: SignalKind, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            signal: Some(signal),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for RankerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.signal {
            Some(s) => write!(f, "{}[{}]: {}", self.code, s, self.detail),
            None => write!(f, "{}: {}", self.code, self.detail),
        }
    }
}

impl core::error::Error for RankerError {}

#[cfg(test)]
mod tests {
    use super::{RankerError, RankerErrorCode, SignalKind};

    const ALL_CODES: &[RankerErrorCode] = &[
        RankerErrorCode::InvalidWeights,
        RankerErrorCode::RankInvalidSignal,
        RankerErrorCode::WeightsDeserialize,
        RankerErrorCode::WeightsHashMismatch,
        RankerErrorCode::InvalidGeneration,
    ];

    const ALL_SIGNALS: &[SignalKind] = &[
        SignalKind::Bm25,
        SignalKind::PathPrior,
        SignalKind::SymbolBoost,
        SignalKind::Recency,
        SignalKind::BoostDirective,
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
    fn code_strs_roundtrip() {
        for c in ALL_CODES {
            assert_eq!(RankerErrorCode::from_code_str(c.as_code_str()), Some(*c));
        }
    }

    #[test]
    fn code_from_unknown_is_none() {
        assert_eq!(RankerErrorCode::from_code_str("NOT_A_CODE"), None);
    }

    #[test]
    fn signal_strs_are_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for s in ALL_SIGNALS {
            let v = s.as_str();
            assert!(!seen.contains(&v), "duplicate signal str: {v}");
            seen.push(v);
        }
        assert_eq!(seen.len(), ALL_SIGNALS.len());
    }

    #[test]
    fn signal_strs_roundtrip() {
        for s in ALL_SIGNALS {
            assert_eq!(SignalKind::from_str_value(s.as_str()), Some(*s));
        }
    }

    #[test]
    fn display_includes_code_and_detail() {
        let e = RankerError::new(RankerErrorCode::InvalidWeights, "sum != 1.0");
        let s = format!("{e}");
        assert!(s.contains("INVALID_WEIGHTS"));
        assert!(s.contains("sum != 1.0"));
    }

    #[test]
    fn display_includes_signal_attribution() {
        let e = RankerError::signal(
            RankerErrorCode::RankInvalidSignal,
            SignalKind::Bm25,
            "NaN observed",
        );
        let s = format!("{e}");
        assert!(s.contains("RANK_INVALID_SIGNAL"));
        assert!(s.contains("bm25"));
        assert!(s.contains("NaN observed"));
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in ALL_CODES {
            let mut buf: Vec<u8> = Vec::new();
            let write = ciborium::ser::into_writer(c, &mut buf);
            assert!(write.is_ok(), "serialize failed for {c:?}");
            let read: Result<RankerErrorCode, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *c),
                Err(e) => assert!(false, "deserialize failed for {c:?}: {e}"),
            }
        }
    }

    #[test]
    fn signal_serde_roundtrip_via_ciborium() {
        for s in ALL_SIGNALS {
            let mut buf: Vec<u8> = Vec::new();
            let write = ciborium::ser::into_writer(s, &mut buf);
            assert!(write.is_ok(), "serialize failed for {s:?}");
            let read: Result<SignalKind, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *s),
                Err(e) => assert!(false, "deserialize failed for {s:?}: {e}"),
            }
        }
    }
}
