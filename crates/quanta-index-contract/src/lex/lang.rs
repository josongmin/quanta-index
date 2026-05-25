//! Closed `LangId` enum.
//!
//! Wire pin: [`docs/ssot/producer-handoff.md`](../../../../docs/ssot/producer-handoff.md)
//! §4.1 ship set.
//!
//! Growing the set requires a coordinated producer + search-side cutover per
//! producer-handoff.md §4.3. Out-of-set values are rejected at deserialization
//! per producer-handoff.md §6 (`SYMBOL_RECORD_INVALID` / `STR_PARSE_TREE_DECODE_FAIL`).

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

/// Producer-supplied language identity.
///
/// Closed set per [`docs/ssot/producer-handoff.md`](../../../../docs/ssot/producer-handoff.md)
/// §4.1. Wire form is the SCREAMING-`PascalCase` string returned by
/// [`LangId::as_code_str`]; deserialization is an exact-match over that table.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum LangId {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
}

impl LangId {
    /// Every variant, in declaration order. The order is load-bearing for
    /// tests (cardinality + uniqueness) and downstream consumers that want
    /// stable iteration.
    pub const ALL: &'static [Self] = &[
        Self::Rust,
        Self::Python,
        Self::TypeScript,
        Self::JavaScript,
        Self::Go,
    ];

    /// Wire-form string for the variant.
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::TypeScript => "TypeScript",
            Self::JavaScript => "JavaScript",
            Self::Go => "Go",
        }
    }

    /// Parse the canonical wire form; returns `None` for any out-of-set
    /// value. No silent fallback per `CLAUDE.md` Safety rules.
    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        match value {
            "Rust" => Some(Self::Rust),
            "Python" => Some(Self::Python),
            "TypeScript" => Some(Self::TypeScript),
            "JavaScript" => Some(Self::JavaScript),
            "Go" => Some(Self::Go),
            _ => None,
        }
    }
}

impl fmt::Display for LangId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

impl Serialize for LangId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_code_str())
    }
}

struct LangIdVisitor;

impl Visitor<'_> for LangIdVisitor {
    type Value = LangId;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LangId code string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        LangId::from_code_str(value).ok_or_else(|| {
            de::Error::unknown_variant(value, &["Rust", "Python", "TypeScript", "JavaScript", "Go"])
        })
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_str(value.as_str())
    }
}

impl<'de> Deserialize<'de> for LangId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(LangIdVisitor)
    }
}

// Unit tests live in `tests/lex_lang.rs` so they share the dev-dependency
// surface (`ciborium`) without leaking it into the library build, and so they
// can use the workspace's standard `Result<(), Box<dyn Error>>` test signature
// without violating the no-unwrap / no-panic clippy lints.
