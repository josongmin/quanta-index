//! `LanguageCode` open wire type.
//!
//! Wire form is a producer-authored canonical lowercase code string such as
//! `rust`, `python`, `typescript`, `javascript`, `go`, `java`, `cpp`.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

/// Producer-supplied language identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct LanguageCode(Box<str>);

impl LanguageCode {
    #[must_use]
    pub fn from_code_str(value: &str) -> Option<Self> {
        Self::new(value).into_iter().next()
    }

    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        validate_language_code(value.as_str())?;
        Ok(Self(value.into_boxed_str()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    #[must_use]
    pub fn into_inner(self) -> Box<str> {
        self.0
    }
}

impl fmt::Display for LanguageCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for LanguageCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct LanguageCodeVisitor;

impl Visitor<'_> for LanguageCodeVisitor {
    type Value = LanguageCode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a canonical lowercase language code string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        LanguageCode::new(value).map_err(de::Error::custom)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        LanguageCode::new(value).map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for LanguageCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_string(LanguageCodeVisitor)
    }
}

fn validate_language_code(value: &str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("language code must not be empty");
    }
    let Some(first) = value.as_bytes().first().copied() else {
        return Err("language code must not be empty");
    };
    if !first.is_ascii_lowercase() {
        return Err("language code must start with a lowercase ASCII letter");
    }
    for byte in value.bytes() {
        let ok = byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || byte == b'-'
            || byte == b'_'
            || byte == b'+';
        if !ok {
            return Err("language code must be lowercase ASCII with '-', '_', or '+'");
        }
    }
    Ok(())
}
