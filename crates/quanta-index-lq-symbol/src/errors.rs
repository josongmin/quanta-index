//! Typed errors for the LEX-05 symbol index.
//!
//! Every failure path in [`crate::extractor`], [`crate::registry`], and
//! [`crate::index`] maps to exactly one [`SymbolErrorCode`] variant. No
//! silent failure, no silent fallback, no panic.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of symbol-index failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolErrorCode {
    /// A precondition for a read or extract is not yet satisfied. Paired
    /// with a sub-tag in the error detail (see
    /// [`SymbolError::lang_unsupported`]).
    StateNotReady,
    /// The requested language has no registered extractor. Carried under
    /// [`SymbolErrorCode::StateNotReady`] with detail prefix
    /// `SYMBOL_LANG_UNSUPPORTED`.
    SymbolLangUnsupported,
    /// Extractor parsed the source but produced a typed failure.
    ExtractorParseFail,
    /// CBOR decode failure when loading a `SymbolIndex`.
    IndexDeserialize,
    /// CBOR payload decoded but failed invariant checks.
    IndexCorrupted,
    /// A document handed to the extractor or index is malformed (e.g.
    /// invalid UTF-8 where UTF-8 is required, byte offsets that overflow
    /// `u32`, span end before start).
    InvalidDocument,
}

impl SymbolErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::StateNotReady => "STATE_NOT_READY",
            Self::SymbolLangUnsupported => "SYMBOL_LANG_UNSUPPORTED",
            Self::ExtractorParseFail => "EXTRACTOR_PARSE_FAIL",
            Self::IndexDeserialize => "INDEX_DESERIALIZE",
            Self::IndexCorrupted => "INDEX_CORRUPTED",
            Self::InvalidDocument => "INVALID_DOCUMENT",
        }
    }

    /// Inverse of [`SymbolErrorCode::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "STATE_NOT_READY" => Self::StateNotReady,
            "SYMBOL_LANG_UNSUPPORTED" => Self::SymbolLangUnsupported,
            "EXTRACTOR_PARSE_FAIL" => Self::ExtractorParseFail,
            "INDEX_DESERIALIZE" => Self::IndexDeserialize,
            "INDEX_CORRUPTED" => Self::IndexCorrupted,
            "INVALID_DOCUMENT" => Self::InvalidDocument,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for SymbolErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for SymbolErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for SymbolErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = SymbolErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("SymbolErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<SymbolErrorCode, E> {
                SymbolErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<SymbolErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete symbol-index failure with engineering-facing detail.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SymbolError {
    pub code: SymbolErrorCode,
    pub detail: Box<str>,
}

impl SymbolError {
    /// Construct an error with the given code and detail string.
    #[must_use]
    pub fn new(code: SymbolErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    /// Convenience constructor for the locked-spec
    /// `STATE_NOT_READY: SYMBOL_LANG_UNSUPPORTED{lang_id}` shape.
    ///
    /// Carries `SymbolErrorCode::StateNotReady` as the wire code and
    /// `SYMBOL_LANG_UNSUPPORTED{...}` as the detail prefix; callers
    /// pattern-match either via [`SymbolError::is_lang_unsupported`] or via
    /// the detail prefix.
    #[must_use]
    pub fn lang_unsupported(lang_id: &str) -> Self {
        Self {
            code: SymbolErrorCode::StateNotReady,
            detail: format!("SYMBOL_LANG_UNSUPPORTED{{lang_id=\"{lang_id}\"}}").into_boxed_str(),
        }
    }

    /// `true` if the error carries the locked `SYMBOL_LANG_UNSUPPORTED`
    /// sub-tag under `STATE_NOT_READY`.
    #[must_use]
    pub fn is_lang_unsupported(&self) -> bool {
        self.code == SymbolErrorCode::StateNotReady
            && self.detail.starts_with("SYMBOL_LANG_UNSUPPORTED")
    }
}

impl fmt::Display for SymbolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl core::error::Error for SymbolError {}

#[cfg(test)]
mod tests {
    use super::{SymbolError, SymbolErrorCode};

    const ALL_CODES: &[SymbolErrorCode] = &[
        SymbolErrorCode::StateNotReady,
        SymbolErrorCode::SymbolLangUnsupported,
        SymbolErrorCode::ExtractorParseFail,
        SymbolErrorCode::IndexDeserialize,
        SymbolErrorCode::IndexCorrupted,
        SymbolErrorCode::InvalidDocument,
    ];

    #[test]
    fn code_strs_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for c in ALL_CODES {
            let s = c.as_code_str();
            assert!(!seen.contains(&s), "duplicate code str: {s}");
            seen.push(s);
        }
    }

    #[test]
    fn code_roundtrip() {
        for c in ALL_CODES {
            assert_eq!(SymbolErrorCode::from_code_str(c.as_code_str()), Some(*c));
        }
    }

    #[test]
    fn unknown_code_returns_none() {
        assert_eq!(SymbolErrorCode::from_code_str("NOT_A_CODE"), None);
        assert_eq!(SymbolErrorCode::from_code_str(""), None);
    }

    #[test]
    fn lang_unsupported_carries_state_not_ready_code() {
        let e = SymbolError::lang_unsupported("ruby");
        assert_eq!(e.code, SymbolErrorCode::StateNotReady);
        assert!(e.detail.contains("SYMBOL_LANG_UNSUPPORTED"));
        assert!(e.detail.contains("ruby"));
        assert!(e.is_lang_unsupported());
    }

    #[test]
    fn display_round_trip_contains_code_and_detail() {
        let e = SymbolError::new(SymbolErrorCode::InvalidDocument, "no utf-8");
        let s = format!("{e}");
        assert!(s.contains("INVALID_DOCUMENT"));
        assert!(s.contains("no utf-8"));
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in ALL_CODES {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(c, &mut buf);
            assert!(w.is_ok(), "serialize failed for {c:?}");
            let read: Result<SymbolErrorCode, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *c),
                Err(e) => assert!(false, "deserialize failed for {c:?}: {e}"),
            }
        }
    }
}
