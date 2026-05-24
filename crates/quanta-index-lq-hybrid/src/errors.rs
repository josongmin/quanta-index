//! Typed errors for the SEM-02 hybrid lex+sem fusion crate.
//!
//! Every failure path through [`crate::strategy`], [`crate::weighted`],
//! [`crate::executor`] maps to exactly one [`HybridErrorCode`] variant.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of hybrid fusion failures (per SEM-02 §4.1, §8).
///
/// Each variant maps to its `HYB_*` `SCREAMING_SNAKE_CASE` wire code via
/// [`HybridErrorCode::as_code_str`]. Variant names mirror the wire code
/// without the `HYB_` prefix (clippy `enum_variant_names` is silenced
/// because the wire taxonomy itself shares the `HYB_*` prefix).
#[expect(
    clippy::enum_variant_names,
    reason = "the closed wire taxonomy is `HYB_*` per SEM-02 §4.1; the variant prefix is load-bearing"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HybridErrorCode {
    /// Weights are NaN, infinite, negative, or sum to zero.
    HybInvalidWeights,
    /// Lexical and semantic sub-results pin different generations.
    HybGenMismatch,
    /// Lexical-universe filter failed to push down into a sub-query.
    HybPushdownIncomplete,
    /// `top_k == 0` or `top_k > MAX_TOP_K`.
    HybTopKInvalid,
    /// Strategy name parsed but is not wired.
    HybStrategyUnsupported,
    /// Sub-query shape invalid: e.g. score is NaN/Inf.
    HybSubqueryInvalid,
}

impl HybridErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::HybInvalidWeights => "HYB_INVALID_WEIGHTS",
            Self::HybGenMismatch => "HYB_GEN_MISMATCH",
            Self::HybPushdownIncomplete => "HYB_PUSHDOWN_INCOMPLETE",
            Self::HybTopKInvalid => "HYB_TOP_K_INVALID",
            Self::HybStrategyUnsupported => "HYB_STRATEGY_UNSUPPORTED",
            Self::HybSubqueryInvalid => "HYB_SUBQUERY_INVALID",
        }
    }

    /// Inverse of [`HybridErrorCode::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "HYB_INVALID_WEIGHTS" => Self::HybInvalidWeights,
            "HYB_GEN_MISMATCH" => Self::HybGenMismatch,
            "HYB_PUSHDOWN_INCOMPLETE" => Self::HybPushdownIncomplete,
            "HYB_TOP_K_INVALID" => Self::HybTopKInvalid,
            "HYB_STRATEGY_UNSUPPORTED" => Self::HybStrategyUnsupported,
            "HYB_SUBQUERY_INVALID" => Self::HybSubqueryInvalid,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for HybridErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for HybridErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for HybridErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = HybridErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("HybridErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<HybridErrorCode, E> {
                HybridErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<HybridErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete hybrid-fusion failure with engineer-facing detail.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HybridError {
    /// Closed-taxonomy code.
    pub code: HybridErrorCode,
    /// Engineering-facing detail.
    pub detail: Box<str>,
}

impl HybridError {
    /// Construct a hybrid error.
    #[must_use]
    pub fn new(code: HybridErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for HybridError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl core::error::Error for HybridError {}

impl serde::Serialize for HybridError {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let mut m = ser.serialize_map(Some(2))?;
        m.serialize_entry("code", &self.code)?;
        m.serialize_entry("detail", self.detail.as_ref())?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for HybridError {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = HybridError;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("HybridError map with code/detail")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<HybridError, M::Error> {
                let mut code: Option<HybridErrorCode> = None;
                let mut detail: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "code" => {
                            if code.is_some() {
                                return Err(serde::de::Error::duplicate_field("code"));
                            }
                            code = Some(map.next_value()?);
                        }
                        "detail" => {
                            if detail.is_some() {
                                return Err(serde::de::Error::duplicate_field("detail"));
                            }
                            detail = Some(map.next_value()?);
                        }
                        _ => {
                            return Err(serde::de::Error::unknown_field(
                                key.as_str(),
                                &["code", "detail"],
                            ));
                        }
                    }
                }
                let code = code.ok_or_else(|| serde::de::Error::missing_field("code"))?;
                let detail = detail.ok_or_else(|| serde::de::Error::missing_field("detail"))?;
                Ok(HybridError {
                    code,
                    detail: detail.into_boxed_str(),
                })
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{HybridError, HybridErrorCode};

    const ALL_CODES: &[HybridErrorCode] = &[
        HybridErrorCode::HybInvalidWeights,
        HybridErrorCode::HybGenMismatch,
        HybridErrorCode::HybPushdownIncomplete,
        HybridErrorCode::HybTopKInvalid,
        HybridErrorCode::HybStrategyUnsupported,
        HybridErrorCode::HybSubqueryInvalid,
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
            assert_eq!(HybridErrorCode::from_code_str(c.as_code_str()), Some(*c));
        }
    }

    #[test]
    fn code_from_unknown_is_none() {
        assert_eq!(HybridErrorCode::from_code_str("NOT_A_CODE"), None);
    }

    #[test]
    fn display_includes_code_and_detail() {
        let e = HybridError::new(HybridErrorCode::HybInvalidWeights, "negative weight");
        let s = format!("{e}");
        assert!(s.contains("HYB_INVALID_WEIGHTS"));
        assert!(s.contains("negative weight"));
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in ALL_CODES {
            let mut buf: Vec<u8> = Vec::new();
            match ciborium::ser::into_writer(c, &mut buf) {
                Ok(()) => {}
                Err(e) => {
                    assert!(false, "serialize failed for {c:?}: {e}");
                }
            }
            match ciborium::de::from_reader::<HybridErrorCode, _>(buf.as_slice()) {
                Ok(got) => assert_eq!(got, *c),
                Err(e) => assert!(false, "deserialize failed for {c:?}: {e}"),
            }
        }
    }

    #[test]
    fn error_serde_roundtrip_via_ciborium() {
        let e = HybridError::new(HybridErrorCode::HybTopKInvalid, "top_k = 0");
        let mut buf: Vec<u8> = Vec::new();
        match ciborium::ser::into_writer(&e, &mut buf) {
            Ok(()) => {}
            Err(err) => assert!(false, "serialize: {err}"),
        }
        match ciborium::de::from_reader::<HybridError, _>(buf.as_slice()) {
            Ok(got) => assert_eq!(got, e),
            Err(err) => assert!(false, "deserialize: {err}"),
        }
    }
}
