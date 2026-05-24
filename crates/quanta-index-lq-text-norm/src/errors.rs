//! Typed errors for the LEX-00 text normalization pipeline.
//!
//! Every tokenization / fold / language-routing failure surfaces a
//! [`LexNormError`] carrying a closed [`LexNormErrorCode`], a byte offset
//! into the original input, and a short engineering-facing `detail`.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of normalization failure reasons.
///
/// Every public entry point in this crate maps a failure to exactly one
/// variant. Adding a variant is a wire-format change; bump callers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LexNormErrorCode {
    /// Input bytes failed `str::from_utf8` (or contained a stray surrogate
    /// when the caller already had a `&str`).
    InvalidUtf8,
    /// Chunk exceeded the 64 KiB cap (or per-call cap configured by caller).
    OversizedChunk,
    /// `lang:` value resolved to a language the normalizer does not ship
    /// an analyzer for.
    NormalizerUnknownLang,
    /// `patterntype:` value is one this crate does not implement yet
    /// (currently: regex and structural — see LEX-04 / STR-01).
    UnknownPatternType,
    /// Token stream came out empty when the caller's contract required at
    /// least one token. Reserved; v1 normalizer does not enforce this yet.
    EmptyTokenStream,
}

impl LexNormErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::InvalidUtf8 => "INVALID_UTF8",
            Self::OversizedChunk => "OVERSIZED_CHUNK",
            Self::NormalizerUnknownLang => "NORMALIZER_UNKNOWN_LANG",
            Self::UnknownPatternType => "UNKNOWN_PATTERN_TYPE",
            Self::EmptyTokenStream => "EMPTY_TOKEN_STREAM",
        }
    }

    /// Inverse of [`LexNormErrorCode::as_code_str`]. Returns `None` for any
    /// string not in the closed set.
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "INVALID_UTF8" => Self::InvalidUtf8,
            "OVERSIZED_CHUNK" => Self::OversizedChunk,
            "NORMALIZER_UNKNOWN_LANG" => Self::NormalizerUnknownLang,
            "UNKNOWN_PATTERN_TYPE" => Self::UnknownPatternType,
            "EMPTY_TOKEN_STREAM" => Self::EmptyTokenStream,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for LexNormErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for LexNormErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for LexNormErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LexNormErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LexNormErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LexNormErrorCode, E> {
                LexNormErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<LexNormErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete normalization failure with byte anchor and engineering detail.
///
/// `byte_offset` is `0` for whole-input failures (e.g. unknown lang) and
/// indexes the failing byte for tokenizer-stage failures.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LexNormError {
    pub code: LexNormErrorCode,
    pub byte_offset: u32,
    pub detail: Box<str>,
}

impl LexNormError {
    /// Constructor that takes any `Into<Box<str>>` detail.
    #[must_use]
    pub fn new(code: LexNormErrorCode, byte_offset: u32, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            byte_offset,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for LexNormError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at byte {}: {}",
            self.code, self.byte_offset, self.detail
        )
    }
}

impl core::error::Error for LexNormError {}

#[cfg(test)]
mod tests {
    use super::{LexNormError, LexNormErrorCode};

    const ALL_CODES: &[LexNormErrorCode] = &[
        LexNormErrorCode::InvalidUtf8,
        LexNormErrorCode::OversizedChunk,
        LexNormErrorCode::NormalizerUnknownLang,
        LexNormErrorCode::UnknownPatternType,
        LexNormErrorCode::EmptyTokenStream,
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
            let parsed = LexNormErrorCode::from_code_str(s);
            assert_eq!(parsed, Some(*c));
        }
    }

    #[test]
    fn from_code_str_rejects_unknown() {
        assert_eq!(LexNormErrorCode::from_code_str("NOT_A_CODE"), None);
        assert_eq!(LexNormErrorCode::from_code_str(""), None);
    }

    #[test]
    fn display_includes_code_offset_and_detail() {
        let e = LexNormError::new(LexNormErrorCode::InvalidUtf8, 17, "stray surrogate");
        let s = format!("{e}");
        assert!(s.contains("INVALID_UTF8"));
        assert!(s.contains("byte 17"));
        assert!(s.contains("stray surrogate"));
    }
}
