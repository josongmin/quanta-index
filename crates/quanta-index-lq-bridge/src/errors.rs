//! Typed errors for the BRIDGE-01 Sourcegraph → LQ translator.
//!
//! Every refusal path in [`crate::syntax`], [`crate::translate_query`],
//! [`crate::version`], and [`crate::candidate`] maps to exactly one
//! [`BridgeErrorCode`] variant. No silent failure, no silent fallback,
//! no panic. Per CLAUDE.md § Agent change posture (`breaking-first`)
//! and per FS-GAP-2 closure ([feature-scope.md § 1.5](../../../../docs/plans/may-24-lexical-indexing-sourcegraph/feature-scope.md)).
//!
//! D18 — every wire shape is hand-rolled `impl serde::Serialize` /
//! `Deserialize`; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of bridge translator failures.
///
/// Wire form is `SCREAMING_SNAKE_CASE`. The five variants here cover the
/// translator-local refusals locked in
/// [BRIDGE-01 § 8.1](../../../../docs/plans/may-24-lexical-indexing-sourcegraph/tickets/BRIDGE-01.md);
/// downstream sink / overflow / provenance codes from § 8.1 are out of scope
/// for this translator crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[expect(
    clippy::enum_variant_names,
    reason = "BRIDGE-01 § 8.1 locks the wire variant names; the shared `Bridge` prefix matches the SCREAMING_SNAKE_CASE wire codes and is the documented public surface."
)]
pub enum BridgeErrorCode {
    /// Sourcegraph filter name has no LQ projection.
    BridgeUnsupportedFilter,
    /// Sourcegraph directive (e.g. `index:no`, fuzzy `~`, generic `@`)
    /// is refused outright by LQ.
    BridgeUnsupportedDirective,
    /// Sourcegraph filter resolves to ≥ 2 LQ targets (defensive
    /// fail-closed).
    BridgeAmbiguousFilter,
    /// Sourcegraph version pin shape is malformed or unsupported.
    BridgeVersionPin,
    /// Translator hit an unexpected internal failure while lowering a
    /// well-formed Sourcegraph construct (e.g. malformed token stream
    /// that survived earlier validation). Distinct from
    /// [`Self::BridgeUnsupportedFilter`]: this is a parser/translator
    /// invariant break, not a Sourcegraph-side decision.
    BridgeTranslateFail,
}

impl BridgeErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::BridgeUnsupportedFilter => "BRIDGE_UNSUPPORTED_FILTER",
            Self::BridgeUnsupportedDirective => "BRIDGE_UNSUPPORTED_DIRECTIVE",
            Self::BridgeAmbiguousFilter => "BRIDGE_AMBIGUOUS_FILTER",
            Self::BridgeVersionPin => "BRIDGE_VERSION_PIN",
            Self::BridgeTranslateFail => "BRIDGE_TRANSLATE_FAIL",
        }
    }

    /// Inverse of [`BridgeErrorCode::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "BRIDGE_UNSUPPORTED_FILTER" => Self::BridgeUnsupportedFilter,
            "BRIDGE_UNSUPPORTED_DIRECTIVE" => Self::BridgeUnsupportedDirective,
            "BRIDGE_AMBIGUOUS_FILTER" => Self::BridgeAmbiguousFilter,
            "BRIDGE_VERSION_PIN" => Self::BridgeVersionPin,
            "BRIDGE_TRANSLATE_FAIL" => Self::BridgeTranslateFail,
            _ => return None,
        };
        Some(v)
    }

    /// Every variant, in declaration order. Used by exhaustive tests.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::BridgeUnsupportedFilter,
            Self::BridgeUnsupportedDirective,
            Self::BridgeAmbiguousFilter,
            Self::BridgeVersionPin,
            Self::BridgeTranslateFail,
        ]
    }
}

impl fmt::Display for BridgeErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for BridgeErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for BridgeErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = BridgeErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("BridgeErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<BridgeErrorCode, E> {
                BridgeErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<BridgeErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete bridge translator failure with engineering-facing detail.
///
/// `source_construct` carries the offending Sourcegraph token / filter /
/// directive when available (e.g. `Some("index:no")` for the
/// `index:no` refusal); `None` when the failure is not bound to a
/// concrete construct (e.g. a malformed version pin string).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BridgeError {
    pub code: BridgeErrorCode,
    pub source_construct: Option<Box<str>>,
    pub detail: Box<str>,
}

impl BridgeError {
    /// Construct an error with the given code, optional source
    /// construct, and detail.
    #[must_use]
    pub fn new(
        code: BridgeErrorCode,
        source_construct: Option<Box<str>>,
        detail: impl Into<Box<str>>,
    ) -> Self {
        Self {
            code,
            source_construct,
            detail: detail.into(),
        }
    }

    /// Convenience constructor for `BRIDGE_UNSUPPORTED_FILTER` with the
    /// offending filter name (e.g. `"r"`, `"colorscheme"`).
    #[must_use]
    pub fn unsupported_filter(filter: &str, detail: impl Into<Box<str>>) -> Self {
        Self::new(
            BridgeErrorCode::BridgeUnsupportedFilter,
            Some(Box::<str>::from(filter)),
            detail,
        )
    }

    /// Convenience constructor for `BRIDGE_UNSUPPORTED_DIRECTIVE` with
    /// the offending directive text (e.g. `"index:no"`, `"~fooBar"`).
    #[must_use]
    pub fn unsupported_directive(construct: &str, detail: impl Into<Box<str>>) -> Self {
        Self::new(
            BridgeErrorCode::BridgeUnsupportedDirective,
            Some(Box::<str>::from(construct)),
            detail,
        )
    }

    /// Convenience constructor for `BRIDGE_AMBIGUOUS_FILTER`.
    #[must_use]
    pub fn ambiguous_filter(filter: &str, detail: impl Into<Box<str>>) -> Self {
        Self::new(
            BridgeErrorCode::BridgeAmbiguousFilter,
            Some(Box::<str>::from(filter)),
            detail,
        )
    }

    /// Convenience constructor for `BRIDGE_VERSION_PIN`.
    #[must_use]
    pub fn version_pin(raw: &str, detail: impl Into<Box<str>>) -> Self {
        Self::new(
            BridgeErrorCode::BridgeVersionPin,
            Some(Box::<str>::from(raw)),
            detail,
        )
    }

    /// Convenience constructor for `BRIDGE_TRANSLATE_FAIL`.
    #[must_use]
    pub fn translate_fail(detail: impl Into<Box<str>>) -> Self {
        Self::new(BridgeErrorCode::BridgeTranslateFail, None, detail)
    }
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.source_construct.as_ref() {
            Some(sc) => write!(f, "{}[{}]: {}", self.code, sc, self.detail),
            None => write!(f, "{}: {}", self.code, self.detail),
        }
    }
}

impl core::error::Error for BridgeError {}

impl serde::Serialize for BridgeError {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let n = if self.source_construct.is_some() {
            3
        } else {
            2
        };
        let mut m = ser.serialize_map(Some(n))?;
        m.serialize_entry("code", &self.code)?;
        if let Some(sc) = self.source_construct.as_ref() {
            m.serialize_entry("source_construct", sc.as_ref())?;
        }
        m.serialize_entry("detail", self.detail.as_ref())?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for BridgeError {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = BridgeError;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("BridgeError map (code, source_construct?, detail)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<BridgeError, M::Error> {
                let mut code: Option<BridgeErrorCode> = None;
                let mut source_construct: Option<String> = None;
                let mut detail: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "code" => {
                            if code.is_some() {
                                return Err(serde::de::Error::duplicate_field("code"));
                            }
                            code = Some(map.next_value()?);
                        }
                        "source_construct" => {
                            if source_construct.is_some() {
                                return Err(serde::de::Error::duplicate_field("source_construct"));
                            }
                            source_construct = Some(map.next_value()?);
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
                                &["code", "source_construct", "detail"],
                            ));
                        }
                    }
                }
                let code = code.ok_or_else(|| serde::de::Error::missing_field("code"))?;
                let detail = detail.ok_or_else(|| serde::de::Error::missing_field("detail"))?;
                Ok(BridgeError {
                    code,
                    source_construct: source_construct.map(String::into_boxed_str),
                    detail: detail.into_boxed_str(),
                })
            }
        }
        de.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{BridgeError, BridgeErrorCode};

    #[test]
    fn code_strs_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for c in BridgeErrorCode::all() {
            let s = c.as_code_str();
            assert!(!seen.contains(&s), "duplicate code str: {s}");
            seen.push(s);
        }
    }

    #[test]
    fn code_roundtrip() {
        for c in BridgeErrorCode::all() {
            assert_eq!(BridgeErrorCode::from_code_str(c.as_code_str()), Some(*c));
        }
    }

    #[test]
    fn unknown_code_returns_none() {
        assert_eq!(BridgeErrorCode::from_code_str("NOT_A_CODE"), None);
        assert_eq!(BridgeErrorCode::from_code_str(""), None);
    }

    #[test]
    fn display_with_source_construct() {
        let e = BridgeError::unsupported_filter("r", "no LQ projection");
        let s = format!("{e}");
        assert!(s.contains("BRIDGE_UNSUPPORTED_FILTER"));
        assert!(s.contains("[r]"));
        assert!(s.contains("no LQ projection"));
    }

    #[test]
    fn display_without_source_construct() {
        let e = BridgeError::translate_fail("invariant break");
        let s = format!("{e}");
        assert!(s.contains("BRIDGE_TRANSLATE_FAIL"));
        assert!(s.contains("invariant break"));
        assert!(!s.contains("[]"));
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in BridgeErrorCode::all() {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(c, &mut buf);
            assert!(w.is_ok(), "serialize failed for {c:?}");
            let read: Result<BridgeErrorCode, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *c),
                Err(e) => assert!(false, "deserialize failed for {c:?}: {e}"),
            }
        }
    }

    #[test]
    fn error_serde_roundtrip_with_construct() {
        let e = BridgeError::unsupported_directive("index:no", "search-plane is index-only");
        let mut buf: Vec<u8> = Vec::new();
        if let Err(err) = ciborium::ser::into_writer(&e, &mut buf) {
            assert!(false, "{err}");
        }
        let got: Result<BridgeError, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, e),
            Err(err) => assert!(false, "{err}"),
        }
    }

    #[test]
    fn error_serde_roundtrip_without_construct() {
        let e = BridgeError::translate_fail("oops");
        let mut buf: Vec<u8> = Vec::new();
        if let Err(err) = ciborium::ser::into_writer(&e, &mut buf) {
            assert!(false, "{err}");
        }
        let got: Result<BridgeError, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, e),
            Err(err) => assert!(false, "{err}"),
        }
    }

    #[test]
    fn error_impls_std_error() {
        fn assert_error<T: core::error::Error>(_: &T) {}
        let e = BridgeError::translate_fail("x");
        assert_error(&e);
    }
}
