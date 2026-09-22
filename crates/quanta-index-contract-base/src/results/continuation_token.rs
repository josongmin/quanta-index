//! Opaque continuation token (S21-06).
//!
//! The only cursor shape on the public wire: base64url text minted by the
//! server's cursor codec. Every pageable request carries at most one, and
//! every pageable response returns at most one. Producers never construct
//! meaning from it, parse it, or forge one: the token is opaque, and any
//! tampered, foreign or expired token is a typed refusal before any
//! read-view acquisition.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

/// An opaque base64url continuation token minted by the server.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ContinuationTokenV2(String);

impl ContinuationTokenV2 {
    /// Wrap wire text. Empty text is refused: a continuation token always
    /// names a boundary.
    pub fn new(token: impl Into<String>) -> Result<Self, ContinuationTokenError> {
        let token = token.into();
        if token.is_empty() {
            return Err(ContinuationTokenError::Empty);
        }
        Ok(Self(token))
    }

    /// The wire text, verbatim.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

/// Fail-closed continuation-token construction failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContinuationTokenError {
    /// The token text is empty.
    Empty,
}

impl fmt::Display for ContinuationTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("continuation token: empty token"),
        }
    }
}

impl std::error::Error for ContinuationTokenError {}

impl Serialize for ContinuationTokenV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct ContinuationTokenVisitor;

impl Visitor<'_> for ContinuationTokenVisitor {
    type Value = ContinuationTokenV2;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a non-empty continuation token string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        ContinuationTokenV2::new(value)
            .map_err(|_error| E::invalid_value(de::Unexpected::Str(value), &self))
    }
}

impl<'de> Deserialize<'de> for ContinuationTokenV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(ContinuationTokenVisitor)
    }
}
