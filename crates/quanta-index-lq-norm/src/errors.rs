//! Typed errors and span carrier for the LQ canonical normalizer.
//!
//! `LqSpan` carries byte offsets back into the original raw input so error
//! envelopes and downstream tooling (conformance runner, explanation
//! emission) can attribute every AST node to a source range. Spans are
//! opaque to the normalizer's semantic equality — two normalized queries
//! that differ only in their `source_span` remain equal under
//! `canonical_hash` (proof: hasher excludes span fields).
//!
//! D18 — every wire shape is hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Half-open byte range `[start, end)` into the raw input.
///
/// Empty spans (`start == end`) are legal for nodes that were synthesized
/// by the normalizer (e.g. injected implicit `AND`) — see
/// [`LqSpan::synthetic`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LqSpan {
    start: u32,
    end: u32,
}

impl LqSpan {
    /// Construct a span over `[start, end)`. Caller is responsible for
    /// `start <= end`; debug-asserts in tests.
    #[must_use]
    pub const fn new(start: u32, end: u32) -> Self {
        debug_assert!(start <= end, "LqSpan: start must be <= end");
        Self { start, end }
    }

    /// Zero-width span anchored at end-of-input. Used by parser when it
    /// expected more tokens but hit EOF; carries `input.len()` so error
    /// messages can render `^` at the right column.
    #[must_use]
    pub const fn eof(at: u32) -> Self {
        Self { start: at, end: at }
    }

    /// Zero-width span at a single offset; for normalizer-synthesized
    /// nodes that have no direct source counterpart.
    #[must_use]
    pub const fn synthetic(at: u32) -> Self {
        Self { start: at, end: at }
    }

    #[must_use]
    pub const fn start(self) -> u32 {
        self.start
    }

    #[must_use]
    pub const fn end(self) -> u32 {
        self.end
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

impl fmt::Display for LqSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}..{})", self.start, self.end)
    }
}

/// Closed taxonomy of parse / normalize / hash failure reasons.
///
/// Every parser, normalizer, and hasher failure path maps to exactly one
/// variant of this enum. `LqParseErrorCode` will be relocated to the
/// contract crate by PRE-CONTRACT-EXT; until then it lives here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LqParseErrorCode {
    /// Input exceeded 16 KiB raw-byte cap.
    LimitExceededBytes,
    /// Recursive-descent depth exceeded 32.
    LimitExceededDepth,
    /// Boolean fan-out per node exceeded 64.
    LimitExceededFanout,
    /// Regex pre-compile NFA state estimator exceeded `100_000`.
    LimitExceededNfa,
    /// Structural `match { ... }` node count exceeded 256.
    LimitExceededStructural,
    /// Tokenizer rejected a byte / escape / glyph it could not classify.
    TokenInvalid,
    /// Syntax explicitly forbidden by RFC: backref, lookahead, lookbehind,
    /// possessive, named-capture-ref, mid-pattern inline flag, generic `@`
    /// outside `repo:<pat>@<rev>` sugar, `~` fuzzy operator, alias outside
    /// `match{}`.
    ForbiddenSyntax,
    /// Filter name not in the closed registry.
    UnknownFilter,
    /// Filter value failed its pinned value-grammar (e.g. duplicate `type:`,
    /// non-numeric `count:`).
    InvalidFilterValue,
    /// Input parsed to a query with zero atoms when at least one was
    /// required (e.g. `into:codeql` standalone).
    EmptyQuery,
    /// Unterminated `"…"` phrase or `'…'` raw string at EOF.
    UnclosedQuote,
    /// Regex source failed `regex_syntax`-level parse.
    RegexParse,
    /// `patterntype:` value not in `{literal,keyword,standard,regexp,structural}`.
    InvalidPatternType,
    /// Combination of constructs that is grammar-legal in isolation but
    /// semantically disallowed (e.g. `into:codeql` + `type:diff`).
    UnsupportedCombo,
    /// Generic parser miss when no more specific code applies (mismatched
    /// paren, trailing operator, etc.).
    SyntaxError,
}

impl LqParseErrorCode {
    /// `SCREAMING_SNAKE_CASE` wire representation.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::LimitExceededBytes => "LIMIT_EXCEEDED_BYTES",
            Self::LimitExceededDepth => "LIMIT_EXCEEDED_DEPTH",
            Self::LimitExceededFanout => "LIMIT_EXCEEDED_FANOUT",
            Self::LimitExceededNfa => "LIMIT_EXCEEDED_NFA",
            Self::LimitExceededStructural => "LIMIT_EXCEEDED_STRUCTURAL",
            Self::TokenInvalid => "TOKEN_INVALID",
            Self::ForbiddenSyntax => "FORBIDDEN_SYNTAX",
            Self::UnknownFilter => "UNKNOWN_FILTER",
            Self::InvalidFilterValue => "INVALID_FILTER_VALUE",
            Self::EmptyQuery => "EMPTY_QUERY",
            Self::UnclosedQuote => "UNCLOSED_QUOTE",
            Self::RegexParse => "REGEX_PARSE",
            Self::InvalidPatternType => "INVALID_PATTERN_TYPE",
            Self::UnsupportedCombo => "UNSUPPORTED_COMBO",
            Self::SyntaxError => "SYNTAX_ERROR",
        }
    }

    /// Inverse of [`LqParseErrorCode::as_code_str`]. Returns `None` for any
    /// string not in the closed set.
    #[must_use]
    pub fn from_code_str(s: &str) -> Option<Self> {
        let v = match s {
            "LIMIT_EXCEEDED_BYTES" => Self::LimitExceededBytes,
            "LIMIT_EXCEEDED_DEPTH" => Self::LimitExceededDepth,
            "LIMIT_EXCEEDED_FANOUT" => Self::LimitExceededFanout,
            "LIMIT_EXCEEDED_NFA" => Self::LimitExceededNfa,
            "LIMIT_EXCEEDED_STRUCTURAL" => Self::LimitExceededStructural,
            "TOKEN_INVALID" => Self::TokenInvalid,
            "FORBIDDEN_SYNTAX" => Self::ForbiddenSyntax,
            "UNKNOWN_FILTER" => Self::UnknownFilter,
            "INVALID_FILTER_VALUE" => Self::InvalidFilterValue,
            "EMPTY_QUERY" => Self::EmptyQuery,
            "UNCLOSED_QUOTE" => Self::UnclosedQuote,
            "REGEX_PARSE" => Self::RegexParse,
            "INVALID_PATTERN_TYPE" => Self::InvalidPatternType,
            "UNSUPPORTED_COMBO" => Self::UnsupportedCombo,
            "SYNTAX_ERROR" => Self::SyntaxError,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for LqParseErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_code_str())
    }
}

impl serde::Serialize for LqParseErrorCode {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_code_str())
    }
}

impl<'de> serde::Deserialize<'de> for LqParseErrorCode {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = LqParseErrorCode;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("LqParseErrorCode SCREAMING_SNAKE_CASE string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<LqParseErrorCode, E> {
                LqParseErrorCode::from_code_str(v)
                    .ok_or_else(|| E::unknown_variant(v, &["<LqParseErrorCode>"]))
            }
        }
        de.deserialize_str(V)
    }
}

/// Concrete parse / normalize failure carrying span anchor and human detail.
///
/// `detail` is a short engineering-facing reason string boxed to keep the
/// error 8-byte aligned and < 32 bytes on the stack so `Result<T, _>` cost
/// stays uniform regardless of the error path frequency.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LqParseError {
    pub code: LqParseErrorCode,
    pub span: LqSpan,
    pub detail: Box<str>,
}

impl LqParseError {
    #[must_use]
    pub fn new(code: LqParseErrorCode, span: LqSpan, detail: impl Into<Box<str>>) -> Self {
        Self {
            code,
            span,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for LqParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}: {}", self.code, self.span, self.detail)
    }
}

impl core::error::Error for LqParseError {}

#[cfg(test)]
mod tests {
    use super::{LqParseError, LqParseErrorCode, LqSpan};

    const ALL_CODES: &[LqParseErrorCode] = &[
        LqParseErrorCode::LimitExceededBytes,
        LqParseErrorCode::LimitExceededDepth,
        LqParseErrorCode::LimitExceededFanout,
        LqParseErrorCode::LimitExceededNfa,
        LqParseErrorCode::LimitExceededStructural,
        LqParseErrorCode::TokenInvalid,
        LqParseErrorCode::ForbiddenSyntax,
        LqParseErrorCode::UnknownFilter,
        LqParseErrorCode::InvalidFilterValue,
        LqParseErrorCode::EmptyQuery,
        LqParseErrorCode::UnclosedQuote,
        LqParseErrorCode::RegexParse,
        LqParseErrorCode::InvalidPatternType,
        LqParseErrorCode::UnsupportedCombo,
        LqParseErrorCode::SyntaxError,
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
            let parsed = LqParseErrorCode::from_code_str(s);
            assert_eq!(parsed, Some(*c));
        }
    }

    #[test]
    fn from_code_str_rejects_unknown() {
        assert_eq!(LqParseErrorCode::from_code_str("NOT_A_CODE"), None);
        assert_eq!(LqParseErrorCode::from_code_str(""), None);
    }

    #[test]
    fn display_includes_code_span_and_detail() {
        let e = LqParseError::new(
            LqParseErrorCode::UnknownFilter,
            LqSpan::new(3, 7),
            "no such filter: foo",
        );
        let s = format!("{e}");
        assert!(s.contains("UNKNOWN_FILTER"));
        assert!(s.contains("[3..7)"));
        assert!(s.contains("no such filter: foo"));
    }
}
