//! `patterntype:` mode matrix for the text normalizer.
//!
//! Mirrors `dsl.md` §4 — PRE-NORM owns the query-side copy of this enum;
//! this crate's copy governs *write/read-time text* normalization. The two
//! copies are kept in sync by spec, not by import (orthogonal pipelines).
//!
//! D18 — hand-rolled serde; no proc-macro derives.

use core::fmt;

/// Pattern mode for the text normalization branch.
///
/// `Literal` bypasses the identifier splitter; `Keyword` and `Standard`
/// engage it. `Regexp` and `Structural` are deferred to LEX-04 / STR-01.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PatternType {
    Literal,
    Keyword,
    Standard,
    Regexp,
    Structural,
}

/// Per the RFC pin: `standard` is the default when `patterntype:` is absent.
pub const DEFAULT_PATTERN_TYPE: PatternType = PatternType::Standard;

impl PatternType {
    /// Lowercase wire string (`literal`, `keyword`, `standard`,
    /// `regexp`, `structural`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Keyword => "keyword",
            Self::Standard => "standard",
            Self::Regexp => "regexp",
            Self::Structural => "structural",
        }
    }

    /// Inverse of [`PatternType::as_str`]. Returns `None` on unknown.
    ///
    /// Named `parse_str` (not `from_str`) so it does not collide with the
    /// `std::str::FromStr` trait method.
    #[must_use]
    pub fn parse_str(s: &str) -> Option<Self> {
        let v = match s {
            "literal" => Self::Literal,
            "keyword" => Self::Keyword,
            "standard" => Self::Standard,
            "regexp" => Self::Regexp,
            "structural" => Self::Structural,
            _ => return None,
        };
        Some(v)
    }
}

impl fmt::Display for PatternType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl serde::Serialize for PatternType {
    fn serialize<S>(&self, ser: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ser.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for PatternType {
    fn deserialize<D>(de: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = PatternType;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("PatternType lowercase string")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<PatternType, E> {
                PatternType::parse_str(v).ok_or_else(|| E::unknown_variant(v, &["<PatternType>"]))
            }
        }
        de.deserialize_str(V)
    }
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_PATTERN_TYPE, PatternType};

    const ALL: &[PatternType] = &[
        PatternType::Literal,
        PatternType::Keyword,
        PatternType::Standard,
        PatternType::Regexp,
        PatternType::Structural,
    ];

    #[test]
    fn strs_are_unique() {
        let mut seen: Vec<&'static str> = Vec::new();
        for p in ALL {
            let s = p.as_str();
            assert!(!seen.contains(&s));
            seen.push(s);
        }
        assert_eq!(seen.len(), ALL.len());
    }

    #[test]
    fn strs_roundtrip() {
        for p in ALL {
            assert_eq!(PatternType::parse_str(p.as_str()), Some(*p));
        }
    }

    #[test]
    fn default_is_standard() {
        assert_eq!(DEFAULT_PATTERN_TYPE, PatternType::Standard);
    }

    #[test]
    fn unknown_str_rejected() {
        assert_eq!(PatternType::parse_str("fuzzy"), None);
        assert_eq!(PatternType::parse_str(""), None);
    }
}
