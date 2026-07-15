//! Canonical query-time constraints shared by every retrieval lane.

use core::fmt;
use std::collections::BTreeSet;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

/// Producer-supplied language identity.
///
/// The wire form is a canonical lowercase code such as `rust`, `python`, or
/// `typescript`. This type lives in `contract-base` because both the request
/// leaf and the heavier query envelopes depend on the same authority.
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

/// Deterministic query constraints applied by every candidate-generation leg.
///
/// `language_any_of` is an OR-set. An empty set means unconstrained. The
/// `BTreeSet` is the canonical wire/order authority, so builder insertion order
/// cannot alter request digests or replay identity.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QueryConstraintSetV1 {
    pub language_any_of: BTreeSet<LanguageCode>,
}

const QUERY_CONSTRAINT_SET_V1_FIELDS: &[&str] = &["language_any_of"];

impl Serialize for QueryConstraintSetV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("QueryConstraintSetV1", 1)?;
        state.serialize_field("language_any_of", &self.language_any_of)?;
        state.end()
    }
}

struct QueryConstraintSetV1Visitor;

impl<'de> Visitor<'de> for QueryConstraintSetV1Visitor {
    type Value = QueryConstraintSetV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a QueryConstraintSetV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut language_any_of: Option<BTreeSet<LanguageCode>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "language_any_of" => {
                    if language_any_of.is_some() {
                        return Err(de::Error::duplicate_field("language_any_of"));
                    }
                    language_any_of = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        QUERY_CONSTRAINT_SET_V1_FIELDS,
                    ));
                }
            }
        }
        Ok(QueryConstraintSetV1 {
            language_any_of: language_any_of
                .ok_or_else(|| de::Error::missing_field("language_any_of"))?,
        })
    }
}

impl<'de> Deserialize<'de> for QueryConstraintSetV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "QueryConstraintSetV1",
            QUERY_CONSTRAINT_SET_V1_FIELDS,
            QueryConstraintSetV1Visitor,
        )
    }
}

impl QueryConstraintSetV1 {
    #[must_use]
    pub fn unconstrained() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_languages(language_any_of: impl IntoIterator<Item = LanguageCode>) -> Self {
        Self {
            language_any_of: language_any_of.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn is_unconstrained(&self) -> bool {
        self.language_any_of.is_empty()
    }

    /// Intersection composition used when a request carries both typed
    /// constraints and DSL `lang:` filters. Empty on either side means that
    /// side imposes no restriction.
    #[must_use]
    pub fn intersect(&self, other: &Self) -> Self {
        match (
            self.language_any_of.is_empty(),
            other.language_any_of.is_empty(),
        ) {
            (true, true) => Self::unconstrained(),
            (true, false) => other.clone(),
            (false, true) => self.clone(),
            (false, false) => Self {
                language_any_of: self
                    .language_any_of
                    .intersection(&other.language_any_of)
                    .cloned()
                    .collect(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LanguageCode, QueryConstraintSetV1};

    #[test]
    fn language_constraints_are_canonical_and_intersect_as_or_sets() {
        let rust = LanguageCode::new("rust").expect("valid language");
        let python = LanguageCode::new("python").expect("valid language");
        let left = QueryConstraintSetV1::from_languages([rust.clone(), python, rust.clone()]);
        assert_eq!(
            left.language_any_of
                .iter()
                .map(LanguageCode::as_str)
                .collect::<Vec<_>>(),
            vec!["python", "rust"]
        );
        let right = QueryConstraintSetV1::from_languages([rust.clone()]);
        assert_eq!(
            left.intersect(&right).language_any_of,
            QueryConstraintSetV1::from_languages([rust]).language_any_of
        );
        assert_eq!(left.intersect(&QueryConstraintSetV1::unconstrained()), left);
    }

    #[test]
    fn language_code_rejects_noncanonical_values() {
        for invalid in ["", "Rust", "rust script", "rust/"] {
            assert!(LanguageCode::new(invalid).is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn constraints_preserve_canonical_wire_shape_and_reject_unknown_fields() {
        let constraints = QueryConstraintSetV1::from_languages([
            LanguageCode::new("rust").expect("valid language"),
            LanguageCode::new("python").expect("valid language"),
        ]);
        let encoded = serde_json::to_string(&constraints).expect("serialize constraints");
        assert_eq!(encoded, r#"{"language_any_of":["python","rust"]}"#);
        assert_eq!(
            serde_json::from_str::<QueryConstraintSetV1>(&encoded)
                .expect("deserialize constraints"),
            constraints
        );
        assert!(
            serde_json::from_str::<QueryConstraintSetV1>(
                r#"{"language_any_of":[],"unexpected":true}"#,
            )
            .is_err()
        );
    }
}
