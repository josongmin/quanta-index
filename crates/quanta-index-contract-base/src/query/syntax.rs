use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextQuerySyntax {
    Native,
    Sourcegraph,
}

impl TextQuerySyntax {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Sourcegraph => "sourcegraph",
        }
    }

    #[must_use]
    pub fn from_str_value(value: &str) -> Option<Self> {
        let syntax = match value {
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
        TextQuerySyntax::from_str_value(value)
            .ok_or_else(|| de::Error::unknown_variant(value, &["native", "sourcegraph"]))
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
