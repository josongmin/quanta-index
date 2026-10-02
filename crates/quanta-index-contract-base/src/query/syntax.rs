use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

/// Maximum number of distinct positive terms in one product code search.
pub const MAX_CODE_SEARCH_TERMS: usize = 32;

/// Maximum UTF-8 byte length of one product code-search literal.
pub const MAX_CODE_SEARCH_TERM_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextQuerySyntax {
    /// Product code search: ranked distinct files, with bare terms matched
    /// as verified literal substrings within the same source file.
    CodeSearch,
    Native,
    Sourcegraph,
}

impl TextQuerySyntax {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodeSearch => "code_search",
            Self::Native => "native",
            Self::Sourcegraph => "sourcegraph",
        }
    }

    #[must_use]
    pub fn from_str_value(value: &str) -> Option<Self> {
        let syntax = match value {
            "code_search" => Self::CodeSearch,
            "native" => Self::Native,
            "sourcegraph" => Self::Sourcegraph,
            _ => return None,
        };
        Some(syntax)
    }
}

impl Serialize for TextQuerySyntax {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct TextQuerySyntaxVisitor;

impl Visitor<'_> for TextQuerySyntaxVisitor {
    type Value = TextQuerySyntax;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a TextQuerySyntax string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        TextQuerySyntax::from_str_value(value).ok_or_else(|| {
            de::Error::unknown_variant(value, &["code_search", "native", "sourcegraph"])
        })
    }
}

impl<'de> Deserialize<'de> for TextQuerySyntax {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(TextQuerySyntaxVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::TextQuerySyntax;

    #[test]
    fn code_search_wire_spelling_is_exact() -> Result<(), Box<dyn std::error::Error>> {
        let encoded = serde_json::to_string(&TextQuerySyntax::CodeSearch)?;
        assert_eq!(encoded, "\"code_search\"");
        assert_eq!(
            serde_json::from_str::<TextQuerySyntax>(&encoded)?,
            TextQuerySyntax::CodeSearch
        );
        assert!(serde_json::from_str::<TextQuerySyntax>("\"CodeSearch\"").is_err());
        Ok(())
    }
}
