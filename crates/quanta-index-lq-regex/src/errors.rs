//! Typed errors for the LEX-04 regex executor.
//!
//! Every production-path failure maps to exactly one [`RegexErrorCode`]
//! variant carrying optional [`LimitDimension`] and [`ForbiddenKind`]
//! qualifiers. There is no untyped error path, no silent fallback, and
//! no `panic!` / `unwrap` / `expect` in the production layer.
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Closed taxonomy of regex-executor failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RegexErrorCode {
    /// `regex_syntax::parse` rejected the input as invalid RE2 syntax.
    ParseFail,
    /// The pattern uses a construct not permitted by the LQ RE2 dialect
    /// (lookbehind, lookahead, backreference, possessive group,
    /// named-capture reference, mid-pattern inline flag).
    ///
    /// The accompanying [`ForbiddenKind`] qualifier identifies which
    /// construct fired.
    ForbiddenSyntax,
    /// A resource cap was exceeded (planning-state charge, compiled engine
    /// bytes, literal bytes, candidate-set size, or verified-result storage).
    ///
    /// The accompanying [`LimitDimension`] qualifier identifies which
    /// cap fired.
    PlanLimitExceeded,
    /// The regex extracted no usable mandatory literal for the trigram
    /// prefilter (pure wildcard, unbounded alternation, etc.). Callers
    /// must drop to a verify-only path; this is NOT a silent fallback.
    RegexPrefilterUnusable,
    /// Cooperative-cancel budget elapsed during candidate verification.
    QueryTimeout,
    /// The caller's interruption check answered `true` between two
    /// candidates; verification stopped there and the caller names why.
    Interrupted,
    /// Internal failure during compilation or verification that doesn't
    /// fit any of the above buckets. Engine size refusal is a resource cap.
    ExecutionInternal,
}

impl RegexErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::ParseFail => "PARSE_FAIL",
            Self::ForbiddenSyntax => "FORBIDDEN_SYNTAX",
            Self::PlanLimitExceeded => "PLAN_LIMIT_EXCEEDED",
            Self::RegexPrefilterUnusable => "REGEX_PREFILTER_UNUSABLE",
            Self::QueryTimeout => "QUERY_TIMEOUT",
            Self::Interrupted => "INTERRUPTED",
            Self::ExecutionInternal => "EXECUTION_INTERNAL",
        }
    }

    /// Inverse of [`RegexErrorCode::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "PARSE_FAIL" => Self::ParseFail,
            "FORBIDDEN_SYNTAX" => Self::ForbiddenSyntax,
            "PLAN_LIMIT_EXCEEDED" => Self::PlanLimitExceeded,
            "REGEX_PREFILTER_UNUSABLE" => Self::RegexPrefilterUnusable,
            "QUERY_TIMEOUT" => Self::QueryTimeout,
            "INTERRUPTED" => Self::Interrupted,
            "EXECUTION_INTERNAL" => Self::ExecutionInternal,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for RegexErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for RegexErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for RegexErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = RegexErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("RegexErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<RegexErrorCode, E> {
                RegexErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<RegexErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Closed taxonomy of cap-dimension tags carried by
/// [`RegexErrorCode::PlanLimitExceeded`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LimitDimension {
    /// Input bytes refused before either regex-syntax parser allocates.
    PatternBytes,
    /// Structural planning charge exceeded the `100_000` cap.
    NfaStates,
    /// The compiled engine exceeded its configured byte ceiling.
    CompiledBytes,
    /// Extracted-literal byte budget exceeded the per-query cap.
    LiteralLen,
    /// Pre-verify candidate set exceeded the per-query cap.
    CandidateSet,
    /// Verified-result storage could not grow.
    VerifiedResults,
}

impl LimitDimension {
    /// `kebab-case` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::PatternBytes => "regex-pattern-bytes",
            Self::NfaStates => "regex-nfa-states",
            Self::CompiledBytes => "regex-compiled-bytes",
            Self::LiteralLen => "regex-literal-len",
            Self::CandidateSet => "regex-candidate-set",
            Self::VerifiedResults => "regex-verified-results",
        }
    }

    /// Inverse of [`LimitDimension::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "regex-pattern-bytes" => Self::PatternBytes,
            "regex-nfa-states" => Self::NfaStates,
            "regex-compiled-bytes" => Self::CompiledBytes,
            "regex-literal-len" => Self::LiteralLen,
            "regex-candidate-set" => Self::CandidateSet,
            "regex-verified-results" => Self::VerifiedResults,
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

/// Closed taxonomy of forbidden-construct tags carried by
/// [`RegexErrorCode::ForbiddenSyntax`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ForbiddenKind {
    /// `(?=…)` lookahead.
    Lookahead,
    /// `(?<=…)` lookbehind.
    Lookbehind,
    /// `\1`, `\2`, … backreference to a previous capture group.
    Backref,
    /// `(?>…)` possessive group.
    Possessive,
    /// `\k<name>` named-capture reference.
    NamedCaptureRef,
    /// Mid-pattern `(?i)` / `(?m)` / `(?s)` / `(?x)` inline flag switch
    /// after the first non-whitespace token.
    InlineFlagMidPattern,
    /// Unicode property class `\p{…}` rejected for v1 (see §12
    /// Q-LEX04-4); recorded so callers can audit the policy choice.
    UnicodeClass,
}

impl ForbiddenKind {
    /// `kebab-case` wire representation matching the DSL §3.4 construct
    /// names.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Lookahead => "lookahead",
            Self::Lookbehind => "lookbehind",
            Self::Backref => "backreference",
            Self::Possessive => "possessive",
            Self::NamedCaptureRef => "named-capture-ref",
            Self::InlineFlagMidPattern => "inline-flag-midpattern",
            Self::UnicodeClass => "unicode-class",
        }
    }

    /// Inverse of [`ForbiddenKind::as_code_str`].
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "lookahead" => Self::Lookahead,
            "lookbehind" => Self::Lookbehind,
            "backreference" => Self::Backref,
            "possessive" => Self::Possessive,
            "named-capture-ref" => Self::NamedCaptureRef,
            "inline-flag-midpattern" => Self::InlineFlagMidPattern,
            "unicode-class" => Self::UnicodeClass,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for ForbiddenKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for ForbiddenKind {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for ForbiddenKind {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = ForbiddenKind;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("ForbiddenKind kebab-case string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<ForbiddenKind, E> {
                ForbiddenKind::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<ForbiddenKind>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete regex-executor failure carrying engineering-facing detail
/// and optional [`LimitDimension`] / [`ForbiddenKind`] qualifiers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RegexError {
    pub code: RegexErrorCode,
    pub dimension: Option<LimitDimension>,
    pub forbidden: Option<ForbiddenKind>,
    pub detail: Box<str>,
}

impl RegexError {
    /// Construct a generic `RegexError` with no qualifier tags.
    #[must_use]
    pub fn new(code: RegexErrorCode, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            dimension: None,
            forbidden: None,
            detail: detail.into(),
        }
    }

    /// Construct a `PLAN_LIMIT_EXCEEDED` with a [`LimitDimension`] tag.
    #[must_use]
    pub fn plan_limit(dim: LimitDimension, detail: impl Into<Box<str>>) -> Self {
        Self {
            code: RegexErrorCode::PlanLimitExceeded,
            dimension: Some(dim),
            forbidden: None,
            detail: detail.into(),
        }
    }

    /// Construct a `FORBIDDEN_SYNTAX` with a [`ForbiddenKind`] tag.
    #[must_use]
    pub fn forbidden(kind: ForbiddenKind, detail: impl Into<Box<str>>) -> Self {
        Self {
            code: RegexErrorCode::ForbiddenSyntax,
            dimension: None,
            forbidden: Some(kind),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for RegexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.dimension, self.forbidden) {
            (Some(d), _) => write!(f, "{}[dimension={}]: {}", self.code, d, self.detail),
            (None, Some(k)) => write!(f, "{}[construct={}]: {}", self.code, k, self.detail),
            (None, None) => write!(f, "{}: {}", self.code, self.detail),
        }
    }
}

impl core::error::Error for RegexError {}

#[cfg(test)]
mod tests {
    use super::{ForbiddenKind, LimitDimension, RegexError, RegexErrorCode};

    const ALL_CODES: &[RegexErrorCode] = &[
        RegexErrorCode::ParseFail,
        RegexErrorCode::ForbiddenSyntax,
        RegexErrorCode::PlanLimitExceeded,
        RegexErrorCode::RegexPrefilterUnusable,
        RegexErrorCode::QueryTimeout,
        RegexErrorCode::Interrupted,
        RegexErrorCode::ExecutionInternal,
    ];

    const ALL_DIMS: &[LimitDimension] = &[
        LimitDimension::PatternBytes,
        LimitDimension::NfaStates,
        LimitDimension::CompiledBytes,
        LimitDimension::LiteralLen,
        LimitDimension::CandidateSet,
        LimitDimension::VerifiedResults,
    ];

    const ALL_FORBIDDEN: &[ForbiddenKind] = &[
        ForbiddenKind::Lookahead,
        ForbiddenKind::Lookbehind,
        ForbiddenKind::Backref,
        ForbiddenKind::Possessive,
        ForbiddenKind::NamedCaptureRef,
        ForbiddenKind::InlineFlagMidPattern,
        ForbiddenKind::UnicodeClass,
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
            assert_eq!(RegexErrorCode::from_code_str(c.as_code_str()), Some(*c));
        }
    }

    #[test]
    fn dim_strs_roundtrip() {
        for d in ALL_DIMS {
            assert_eq!(LimitDimension::from_code_str(d.as_code_str()), Some(*d));
        }
    }

    #[test]
    fn forbidden_strs_roundtrip() {
        for k in ALL_FORBIDDEN {
            assert_eq!(ForbiddenKind::from_code_str(k.as_code_str()), Some(*k));
        }
    }

    #[test]
    fn display_carries_dimension() {
        let e = RegexError::plan_limit(LimitDimension::NfaStates, "exceeded 100k");
        let s = format!("{e}");
        assert!(s.contains("PLAN_LIMIT_EXCEEDED"));
        assert!(s.contains("regex-nfa-states"));
        assert!(s.contains("exceeded 100k"));
    }

    #[test]
    fn display_carries_forbidden_kind() {
        let e = RegexError::forbidden(ForbiddenKind::Lookahead, "rejected");
        let s = format!("{e}");
        assert!(s.contains("FORBIDDEN_SYNTAX"));
        assert!(s.contains("lookahead"));
    }

    #[test]
    fn code_serde_roundtrip_via_ciborium() {
        for c in ALL_CODES {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(c, &mut buf);
            assert!(w.is_ok(), "serialize failed for {c:?}");
            let read: Result<RegexErrorCode, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *c),
                Err(e) => assert!(false, "deserialize failed for {c:?}: {e}"),
            }
        }
    }

    #[test]
    #[expect(
        clippy::panic_in_result_fn,
        reason = "typed dimension wire roundtrips assert exact decoded qualifiers"
    )]
    fn dimension_serde_roundtrip_via_ciborium() -> Result<(), Box<dyn std::error::Error>> {
        for dimension in ALL_DIMS {
            let mut bytes = Vec::new();
            ciborium::ser::into_writer(dimension, &mut bytes)?;
            let decoded: LimitDimension = ciborium::de::from_reader(bytes.as_slice())?;
            assert_eq!(decoded, *dimension);
        }
        Ok(())
    }

    #[test]
    fn forbidden_serde_roundtrip_via_ciborium() {
        for k in ALL_FORBIDDEN {
            let mut buf: Vec<u8> = Vec::new();
            let w = ciborium::ser::into_writer(k, &mut buf);
            assert!(w.is_ok(), "serialize failed for {k:?}");
            let read: Result<ForbiddenKind, _> = ciborium::de::from_reader(buf.as_slice());
            match read {
                Ok(got) => assert_eq!(got, *k),
                Err(e) => assert!(false, "deserialize failed for {k:?}: {e}"),
            }
        }
    }

    #[test]
    fn unknown_strs_return_none() {
        assert_eq!(RegexErrorCode::from_code_str("NOT_A_CODE"), None);
        assert_eq!(LimitDimension::from_code_str("not-a-dim"), None);
        assert_eq!(ForbiddenKind::from_code_str("not-a-kind"), None);
    }
}
