//! Typed errors for the LEX-03 phrase position index.
//!
//! Every failure path through [`crate::builder`], [`crate::index`],
//! [`crate::phrase_query`], [`crate::adjacency_query`], and
//! [`crate::varint`] surfaces a [`PositionsError`] carrying a closed
//! [`PositionsErrorCode`].
//!
//! Production paths never panic, never silently default, and never
//! synthesize empty success in place of a real failure. Cross-chunk phrase
//! semantics (an empty match set, not an error) are encoded by the public
//! API contract of [`crate::phrase_query::PhraseMatches`], not by this
//! error type.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of phrase / adjacency / persistence failures.
///
/// Adding a variant is a wire-format change; bump callers when the closed
/// set grows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PositionsErrorCode {
    /// Generation id on the manifest moved backward relative to the in-memory
    /// authority. Per RFC § Monotonicity rules.
    StateGenerationRegression,
    /// `meta.normalizer_version` on the loaded shard does not match the
    /// expected authority. The shard must be rebuilt under the new
    /// normalizer; see RFC § Atomicity contract.
    NormalizerVersionMismatch,
    /// CBOR decode failure when loading a `PositionsIndex`.
    IndexDeserialize,
    /// On-disk posting list bytes are malformed (truncated, gap underflow,
    /// length mismatch).
    IndexCorrupted,
    /// Caller supplied an empty term, a phrase token with no chars, or a
    /// term that violates the post-normalize contract.
    InvalidTerm,
    /// Adjacency window value is outside the configured floor / ceiling.
    WindowOutOfRange,
}

impl PositionsErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::StateGenerationRegression => "STATE_GENERATION_REGRESSION",
            Self::NormalizerVersionMismatch => "NORMALIZER_VERSION_MISMATCH",
            Self::IndexDeserialize => "INDEX_DESERIALIZE",
            Self::IndexCorrupted => "INDEX_CORRUPTED",
            Self::InvalidTerm => "INVALID_TERM",
            Self::WindowOutOfRange => "WINDOW_OUT_OF_RANGE",
        }
    }

    /// Inverse of [`PositionsErrorCode::as_code_str`]. Returns `None` for
    /// any string not in the closed set.
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "STATE_GENERATION_REGRESSION" => Self::StateGenerationRegression,
            "NORMALIZER_VERSION_MISMATCH" => Self::NormalizerVersionMismatch,
            "INDEX_DESERIALIZE" => Self::IndexDeserialize,
            "INDEX_CORRUPTED" => Self::IndexCorrupted,
            "INVALID_TERM" => Self::InvalidTerm,
            "WINDOW_OUT_OF_RANGE" => Self::WindowOutOfRange,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for PositionsErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for PositionsErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for PositionsErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = PositionsErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("PositionsErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<PositionsErrorCode, E> {
                PositionsErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<PositionsErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete phrase / adjacency / persistence failure with engineering-facing
/// detail.
///
/// `detail` is a short, free-form engineering anchor; it is not parsed by
/// callers. Callers branch on `code`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PositionsError {
    pub code: PositionsErrorCode,
    pub detail: Box<str>,
}

impl PositionsError {
    /// Constructor that takes any `Into<Box<str>>` detail.
    #[must_use]
    pub fn new(code: PositionsErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for PositionsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl core::error::Error for PositionsError {}

#[cfg(test)]
mod tests {
    use super::{PositionsError, PositionsErrorCode};

    const ALL_CODES: &[PositionsErrorCode] = &[
        PositionsErrorCode::StateGenerationRegression,
        PositionsErrorCode::NormalizerVersionMismatch,
        PositionsErrorCode::IndexDeserialize,
        PositionsErrorCode::IndexCorrupted,
        PositionsErrorCode::InvalidTerm,
        PositionsErrorCode::WindowOutOfRange,
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
            let parsed = PositionsErrorCode::from_code_str(s);
            assert_eq!(parsed, Some(*c));
        }
    }

    #[test]
    fn from_code_str_rejects_unknown() {
        assert_eq!(PositionsErrorCode::from_code_str("NOT_A_CODE"), None);
        assert_eq!(PositionsErrorCode::from_code_str(""), None);
    }

    #[test]
    fn display_includes_code_and_detail() {
        let e = PositionsError::new(PositionsErrorCode::InvalidTerm, "empty term");
        let s = format!("{e}");
        assert!(s.contains("INVALID_TERM"));
        assert!(s.contains("empty term"));
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in ALL_CODES {
            let mut buf: Vec<u8> = Vec::new();
            let write = ciborium::ser::into_writer(c, &mut buf);
            assert!(write.is_ok(), "serialize failed for {c:?}");
            let read: Result<PositionsErrorCode, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *c),
                Err(e) => assert!(false, "deserialize failed for {c:?}: {e}"),
            }
        }
    }
}
