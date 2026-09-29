//! Typed errors for the STR-01 structural pattern engine.
//!
//! Every failure path in [`crate::pattern`] and [`crate::matcher`] maps to
//! exactly one [`StructuralErrorCode`] variant.
//! No silent failure, no silent fallback, no panic.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of structural-engine failures. Wire shape is
/// `SCREAMING_SNAKE_CASE` per RFC § Error Code Taxonomy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StructuralErrorCode {
    /// Structural pattern body fails the §8.1 mini-language grammar
    /// (unbalanced `{`, malformed metavariable, etc.).
    StrParseFail,
    /// Metavariable used in unsupported position — e.g. nested inside
    /// another metavariable, used outside `match { … }`, or referenced
    /// by `where` without ever being bound.
    StrInvalidMetavar,
    /// Typed structural hole kind is outside the current closed executable set
    /// or is not valid for the current authority route.
    StrHoleKindUnsupported,
    /// Explicit `lang:<id>` outside the §4.10 ship set.
    StrLangNotSupported,
    /// Structural pattern exceeds the 256-node, 16-depth, or 32-metavar
    /// cap. Carries the offending [`LimitDimension`] in the error payload.
    PlanLimitExceeded,
    /// A structural `where` regex is malformed or outside the shared dialect.
    RegexInvalidPattern,
    /// A structural `where` regex exceeded the shared regex executor's cap.
    RegexPlanLimitExceeded,
    /// The shared regex executor failed for a non-input, non-resource reason.
    RegexExecutionInternal,
}

impl StructuralErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::StrParseFail => "STR_PARSE_FAIL",
            Self::StrInvalidMetavar => "STR_INVALID_METAVAR",
            Self::StrHoleKindUnsupported => "STR_HOLE_KIND_UNSUPPORTED",
            Self::StrLangNotSupported => "STR_LANG_NOT_SUPPORTED",
            Self::PlanLimitExceeded => "PLAN_LIMIT_EXCEEDED",
            Self::RegexInvalidPattern => "REGEX_INVALID_PATTERN",
            Self::RegexPlanLimitExceeded => "REGEX_PLAN_LIMIT_EXCEEDED",
            Self::RegexExecutionInternal => "REGEX_EXECUTION_INTERNAL",
        }
    }

    /// Inverse of [`StructuralErrorCode::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "STR_PARSE_FAIL" => Self::StrParseFail,
            "STR_INVALID_METAVAR" => Self::StrInvalidMetavar,
            "STR_HOLE_KIND_UNSUPPORTED" => Self::StrHoleKindUnsupported,
            "STR_LANG_NOT_SUPPORTED" => Self::StrLangNotSupported,
            "PLAN_LIMIT_EXCEEDED" => Self::PlanLimitExceeded,
            "REGEX_INVALID_PATTERN" => Self::RegexInvalidPattern,
            "REGEX_PLAN_LIMIT_EXCEEDED" => Self::RegexPlanLimitExceeded,
            "REGEX_EXECUTION_INTERNAL" => Self::RegexExecutionInternal,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for StructuralErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for StructuralErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for StructuralErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = StructuralErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("StructuralErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<StructuralErrorCode, E> {
                StructuralErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<StructuralErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Pattern-cap dimension carried by [`StructuralError`] when the offending
/// code is [`StructuralErrorCode::PlanLimitExceeded`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LimitDimension {
    /// Pattern node count exceeds [`crate::types::MAX_STRUCTURAL_NODES`].
    NodeCount,
    /// Pattern nesting depth exceeds [`crate::types::MAX_DEPTH`].
    Depth,
    /// Distinct metavariable count exceeds
    /// [`crate::types::MAX_METAVARS_PER_PATTERN`].
    MetavarCount,
}

impl LimitDimension {
    /// `SCREAMING_SNAKE_CASE` wire string.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::NodeCount => "NODE_COUNT",
            Self::Depth => "DEPTH",
            Self::MetavarCount => "METAVAR_COUNT",
        }
    }

    /// Inverse of [`LimitDimension::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "NODE_COUNT" => Self::NodeCount,
            "DEPTH" => Self::Depth,
            "METAVAR_COUNT" => Self::MetavarCount,
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
                f.write_str("LimitDimension SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LimitDimension, E> {
                LimitDimension::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<LimitDimension>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete structural-engine failure with engineering-facing detail.
///
/// `dimension` is `Some(_)` exactly when `code` is
/// [`StructuralErrorCode::PlanLimitExceeded`]; the constructor enforces
/// this invariant.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StructuralError {
    pub code: StructuralErrorCode,
    pub dimension: Option<LimitDimension>,
    pub detail: Box<str>,
}

impl StructuralError {
    /// Generic constructor for non-limit errors. Use
    /// [`StructuralError::plan_limit_exceeded`] for cap violations so the
    /// dimension is always attached.
    #[must_use]
    pub fn new(code: StructuralErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            dimension: None,
            detail: detail.into(),
        }
    }

    /// Construct a `PLAN_LIMIT_EXCEEDED` error carrying its dimension.
    #[must_use]
    pub fn plan_limit_exceeded(dimension: LimitDimension, detail: impl Into<Box<str>>) -> Self {
        Self {
            code: StructuralErrorCode::PlanLimitExceeded,
            dimension: Some(dimension),
            detail: detail.into(),
        }
    }

    /// Convenience: `STR_LANG_NOT_SUPPORTED{lang}` with structured detail.
    #[must_use]
    pub fn lang_not_supported(lang_code: &str) -> Self {
        Self {
            code: StructuralErrorCode::StrLangNotSupported,
            dimension: None,
            detail: format!("STR_LANG_NOT_SUPPORTED{{lang=\"{lang_code}\"}}").into_boxed_str(),
        }
    }

    /// Convenience: `STR_INVALID_METAVAR{ref}` with structured detail.
    #[must_use]
    pub fn invalid_metavar(metavar_name: &str) -> Self {
        Self {
            code: StructuralErrorCode::StrInvalidMetavar,
            dimension: None,
            detail: format!("STR_INVALID_METAVAR{{ref=\"{metavar_name}\"}}").into_boxed_str(),
        }
    }

    /// Convenience: `STR_HOLE_KIND_UNSUPPORTED{kind}` with structured detail.
    #[must_use]
    pub fn hole_kind_unsupported(kind: &str) -> Self {
        Self {
            code: StructuralErrorCode::StrHoleKindUnsupported,
            dimension: None,
            detail: format!("STR_HOLE_KIND_UNSUPPORTED{{kind=\"{kind}\"}}").into_boxed_str(),
        }
    }

    /// Convenience: `STR_PARSE_FAIL` with the offending offset.
    #[must_use]
    pub fn parse_fail(offset: usize, detail: impl AsRef<str>) -> Self {
        Self {
            code: StructuralErrorCode::StrParseFail,
            dimension: None,
            detail: format!(
                "STR_PARSE_FAIL{{offset={offset}, detail=\"{}\"}}",
                detail.as_ref()
            )
            .into_boxed_str(),
        }
    }
}

impl fmt::Display for StructuralError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.dimension {
            Some(d) => write!(f, "{}: dimension={} {}", self.code, d, self.detail),
            None => write!(f, "{}: {}", self.code, self.detail),
        }
    }
}

impl core::error::Error for StructuralError {}

impl serde::Serialize for StructuralError {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap as _;
        let n = if self.dimension.is_some() { 3 } else { 2 };
        let mut m = ser.serialize_map(Some(n))?;
        m.serialize_entry("code", &self.code)?;
        if let Some(d) = self.dimension {
            m.serialize_entry("dimension", &d)?;
        }
        m.serialize_entry("detail", self.detail.as_ref())?;
        m.end()
    }
}

impl<'de> serde::Deserialize<'de> for StructuralError {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl<'d> serde::de::Visitor<'d> for V {
            type Value = StructuralError;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("StructuralError map (code, dimension?, detail)")
            }
            fn visit_map<M: serde::de::MapAccess<'d>>(
                self,
                mut map: M,
            ) -> Result<StructuralError, M::Error> {
                let mut code: Option<StructuralErrorCode> = None;
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
                Ok(StructuralError {
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
    use super::{LimitDimension, StructuralError, StructuralErrorCode};

    const ALL_CODES: &[StructuralErrorCode] = &[
        StructuralErrorCode::StrParseFail,
        StructuralErrorCode::StrInvalidMetavar,
        StructuralErrorCode::StrHoleKindUnsupported,
        StructuralErrorCode::StrLangNotSupported,
        StructuralErrorCode::PlanLimitExceeded,
        StructuralErrorCode::RegexInvalidPattern,
        StructuralErrorCode::RegexPlanLimitExceeded,
        StructuralErrorCode::RegexExecutionInternal,
    ];

    const ALL_DIMENSIONS: &[LimitDimension] = &[
        LimitDimension::NodeCount,
        LimitDimension::Depth,
        LimitDimension::MetavarCount,
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
            assert_eq!(
                StructuralErrorCode::from_code_str(c.as_code_str()),
                Some(*c)
            );
        }
    }

    #[test]
    fn dimension_roundtrip() {
        for d in ALL_DIMENSIONS {
            assert_eq!(LimitDimension::from_code_str(d.as_code_str()), Some(*d));
        }
    }

    #[test]
    fn unknown_code_returns_none() {
        assert_eq!(StructuralErrorCode::from_code_str("NOT_A_CODE"), None);
    }

    #[test]
    fn plan_limit_exceeded_carries_dimension() {
        let e = StructuralError::plan_limit_exceeded(LimitDimension::NodeCount, "300>256");
        assert_eq!(e.code, StructuralErrorCode::PlanLimitExceeded);
        assert_eq!(e.dimension, Some(LimitDimension::NodeCount));
        assert!(e.detail.contains("300>256"));
    }

    #[test]
    fn lang_not_supported_constructor() {
        let e = StructuralError::lang_not_supported("CPP");
        assert_eq!(e.code, StructuralErrorCode::StrLangNotSupported);
        assert!(e.detail.contains("CPP"));
    }

    #[test]
    fn invalid_metavar_constructor() {
        let e = StructuralError::invalid_metavar("X");
        assert_eq!(e.code, StructuralErrorCode::StrInvalidMetavar);
        assert!(e.detail.contains('X'));
    }

    #[test]
    fn hole_kind_unsupported_constructor() {
        let e = StructuralError::hole_kind_unsupported("expr");
        assert_eq!(e.code, StructuralErrorCode::StrHoleKindUnsupported);
        assert!(e.detail.contains("expr"));
    }

    #[test]
    fn parse_fail_constructor_records_offset() {
        let e = StructuralError::parse_fail(7, "unbalanced");
        assert_eq!(e.code, StructuralErrorCode::StrParseFail);
        assert!(e.detail.contains("offset=7"));
        assert!(e.detail.contains("unbalanced"));
    }

    #[test]
    fn display_includes_dimension_for_limit_error() {
        let e = StructuralError::plan_limit_exceeded(LimitDimension::Depth, "17>16");
        let s = format!("{e}");
        assert!(s.contains("PLAN_LIMIT_EXCEEDED"));
        assert!(s.contains("DEPTH"));
        assert!(s.contains("17>16"));
    }

    #[test]
    fn display_omits_dimension_for_non_limit_error() {
        let e = StructuralError::new(StructuralErrorCode::StrParseFail, "bad");
        let s = format!("{e}");
        assert!(s.contains("STR_PARSE_FAIL"));
        assert!(!s.contains("dimension"));
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in ALL_CODES {
            let mut buf: Vec<u8> = Vec::new();
            if let Err(e) = ciborium::ser::into_writer(c, &mut buf) {
                assert!(false, "serialize failed for {c:?}: {e}");
            }
            let read: Result<StructuralErrorCode, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *c),
                Err(e) => assert!(false, "deserialize failed for {c:?}: {e}"),
            }
        }
    }

    #[test]
    fn error_serde_roundtrip_with_dimension() {
        let e = StructuralError::plan_limit_exceeded(LimitDimension::NodeCount, "300>256");
        let mut buf: Vec<u8> = Vec::new();
        if let Err(err) = ciborium::ser::into_writer(&e, &mut buf) {
            assert!(false, "{err}");
        }
        let got: Result<StructuralError, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, e),
            Err(err) => assert!(false, "{err}"),
        }
    }

    #[test]
    fn error_serde_roundtrip_without_dimension() {
        let e = StructuralError::new(StructuralErrorCode::StrInvalidMetavar, "bad ref");
        let mut buf: Vec<u8> = Vec::new();
        if let Err(err) = ciborium::ser::into_writer(&e, &mut buf) {
            assert!(false, "{err}");
        }
        let got: Result<StructuralError, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, e),
            Err(err) => assert!(false, "{err}"),
        }
    }
}
