//! Canonical query-time constraints shared by every retrieval lane.

use core::fmt;
use std::collections::BTreeSet;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

const MAX_EXACT_REPO_RELATIVE_PATH_BYTES_V1: usize = 4096;

/// Validated, exact repository-relative path used as a query constraint.
///
/// This is deliberately distinct from the permissive persisted
/// [`crate::RepoRelativePath`] identity. Query paths cross an untrusted request
/// boundary and must be lexical relative paths before any storage adapter can
/// compile them into a pre-ranking predicate.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExactRepoRelativePathV1(Box<str>);

impl ExactRepoRelativePathV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        validate_exact_repo_relative_path_v1(value.as_str())?;
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

impl fmt::Display for ExactRepoRelativePathV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for ExactRepoRelativePathV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct ExactRepoRelativePathV1Visitor;

impl Visitor<'_> for ExactRepoRelativePathV1Visitor {
    type Value = ExactRepoRelativePathV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a canonical repository-relative path string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        ExactRepoRelativePathV1::new(value).map_err(de::Error::custom)
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        ExactRepoRelativePathV1::new(value).map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for ExactRepoRelativePathV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_string(ExactRepoRelativePathV1Visitor)
    }
}

fn validate_exact_repo_relative_path_v1(value: &str) -> Result<(), &'static str> {
    if value.is_empty() {
        return Err("exact repository-relative path must not be empty");
    }
    if value.len() > MAX_EXACT_REPO_RELATIVE_PATH_BYTES_V1 {
        return Err("exact repository-relative path exceeds 4096 bytes");
    }
    if value.starts_with('/') {
        return Err("exact repository-relative path must not be absolute");
    }
    if value.contains('\\') {
        return Err("exact repository-relative path must use '/' separators");
    }
    if value.chars().any(char::is_control) {
        return Err("exact repository-relative path must not contain control characters");
    }
    if value.as_bytes().get(1) == Some(&b':')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
    {
        return Err("exact repository-relative path must not use a drive prefix");
    }
    for segment in value.split('/') {
        if segment.is_empty() {
            return Err("exact repository-relative path must not contain empty segments");
        }
        if matches!(segment, "." | "..") {
            return Err("exact repository-relative path must not contain '.' or '..' segments");
        }
    }
    Ok(())
}

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
/// `language_any_of` is an OR-set; an empty set leaves that axis unconstrained.
/// `repo_relative_path_exact` is a single validated equality constraint. The
/// `BTreeSet` and conditional path field are the canonical wire/order authority,
/// so builder insertion order cannot alter request digests or replay identity.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct QueryConstraintSetV1 {
    pub language_any_of: BTreeSet<LanguageCode>,
    pub repo_relative_path_exact: Option<ExactRepoRelativePathV1>,
}

const QUERY_CONSTRAINT_SET_V1_FIELDS: &[&str] = &["language_any_of", "repo_relative_path_exact"];

impl Serialize for QueryConstraintSetV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = if self.repo_relative_path_exact.is_some() {
            2
        } else {
            1
        };
        let mut state = serializer.serialize_struct("QueryConstraintSetV1", field_count)?;
        state.serialize_field("language_any_of", &self.language_any_of)?;
        if let Some(path) = &self.repo_relative_path_exact {
            state.serialize_field("repo_relative_path_exact", path)?;
        }
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
        let mut repo_relative_path_exact: Option<ExactRepoRelativePathV1> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "language_any_of" => {
                    if language_any_of.is_some() {
                        return Err(de::Error::duplicate_field("language_any_of"));
                    }
                    language_any_of = Some(map.next_value()?);
                }
                "repo_relative_path_exact" => {
                    if repo_relative_path_exact.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path_exact"));
                    }
                    repo_relative_path_exact = Some(map.next_value()?);
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
            repo_relative_path_exact,
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
            repo_relative_path_exact: None,
        }
    }

    #[must_use]
    pub fn from_exact_repo_relative_path(path: ExactRepoRelativePathV1) -> Self {
        Self {
            language_any_of: BTreeSet::new(),
            repo_relative_path_exact: Some(path),
        }
    }

    #[must_use]
    pub fn with_languages(
        mut self,
        language_any_of: impl IntoIterator<Item = LanguageCode>,
    ) -> Self {
        self.language_any_of = language_any_of.into_iter().collect();
        self
    }

    #[must_use]
    pub fn with_exact_repo_relative_path(mut self, path: ExactRepoRelativePathV1) -> Self {
        self.repo_relative_path_exact = Some(path);
        self
    }

    #[must_use]
    pub fn is_unconstrained(&self) -> bool {
        self.language_any_of.is_empty() && self.repo_relative_path_exact.is_none()
    }

    /// Intersect independently authored constraint sets without losing a
    /// contradiction. An unconstrained axis on either side imposes no
    /// restriction; disjoint constrained axes return an explicit verdict.
    #[must_use]
    pub fn intersect(&self, other: &Self) -> QueryConstraintIntersectionV1 {
        let language_any_of = match (
            self.language_any_of.is_empty(),
            other.language_any_of.is_empty(),
        ) {
            (true, true) => BTreeSet::new(),
            (true, false) => other.language_any_of.clone(),
            (false, true) => self.language_any_of.clone(),
            (false, false) => {
                let intersection: BTreeSet<LanguageCode> = self
                    .language_any_of
                    .intersection(&other.language_any_of)
                    .cloned()
                    .collect();
                if intersection.is_empty() {
                    return QueryConstraintIntersectionV1::Contradiction;
                }
                intersection
            }
        };
        let repo_relative_path_exact = match (
            &self.repo_relative_path_exact,
            &other.repo_relative_path_exact,
        ) {
            (None, None) => None,
            (Some(path), None) | (None, Some(path)) => Some(path.clone()),
            (Some(left), Some(right)) if left == right => Some(left.clone()),
            (Some(_), Some(_)) => return QueryConstraintIntersectionV1::Contradiction,
        };
        QueryConstraintIntersectionV1::Compatible(Self {
            language_any_of,
            repo_relative_path_exact,
        })
    }
}

/// Explicit result of composing two independent query-constraint surfaces.
///
/// A contradiction must never be encoded as an empty constraint set because
/// empty means unconstrained on the wire and would widen the query.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QueryConstraintIntersectionV1 {
    Compatible(QueryConstraintSetV1),
    Contradiction,
}

#[cfg(test)]
mod tests {
    use super::{
        ExactRepoRelativePathV1, LanguageCode, MAX_EXACT_REPO_RELATIVE_PATH_BYTES_V1,
        QueryConstraintIntersectionV1, QueryConstraintSetV1,
    };

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
            left.intersect(&right),
            QueryConstraintIntersectionV1::Compatible(QueryConstraintSetV1::from_languages([rust]))
        );
        assert_eq!(
            left.intersect(&QueryConstraintSetV1::unconstrained()),
            QueryConstraintIntersectionV1::Compatible(left)
        );
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

    #[test]
    fn exact_path_is_validated_and_has_conditional_wire_shape() {
        let path = ExactRepoRelativePathV1::new("src/검색/lib.rs").expect("valid exact path");
        assert_eq!(path.as_str(), "src/검색/lib.rs");
        for valid_literal in [
            "src/star*.rs",
            "src/close).rs",
            "src/single'quote.rs",
            "src/double\"quote.rs",
            "src/space name.rs",
        ] {
            assert!(
                ExactRepoRelativePathV1::new(valid_literal).is_ok(),
                "valid Unix filename characters must remain literal: {valid_literal:?}"
            );
        }
        for invalid in [
            "",
            "/src/lib.rs",
            "C:/src/lib.rs",
            "src\\lib.rs",
            "src//lib.rs",
            "./src/lib.rs",
            "src/../lib.rs",
            "src/lib.rs/",
            "src/\0lib.rs",
        ] {
            assert!(
                ExactRepoRelativePathV1::new(invalid).is_err(),
                "accepted invalid exact path {invalid:?}"
            );
        }
        assert!(
            ExactRepoRelativePathV1::new(
                "a".repeat(MAX_EXACT_REPO_RELATIVE_PATH_BYTES_V1.saturating_add(1)),
            )
            .is_err(),
            "oversized exact path must fail before storage query compilation"
        );

        let old_wire = QueryConstraintSetV1::unconstrained();
        assert_eq!(
            serde_json::to_string(&old_wire).expect("serialize old shape"),
            r#"{"language_any_of":[]}"#
        );
        let constrained = QueryConstraintSetV1::from_exact_repo_relative_path(path);
        let encoded = serde_json::to_string(&constrained).expect("serialize path constraint");
        assert_eq!(
            encoded,
            r#"{"language_any_of":[],"repo_relative_path_exact":"src/검색/lib.rs"}"#
        );
        assert_eq!(
            serde_json::from_str::<QueryConstraintSetV1>(&encoded)
                .expect("deserialize path constraint"),
            constrained
        );
        assert!(
            serde_json::from_str::<QueryConstraintSetV1>(
                r#"{"language_any_of":[],"repo_relative_path_exact":"../lib.rs"}"#,
            )
            .is_err(),
            "wire decoder must revalidate exact paths"
        );
    }

    #[test]
    fn intersection_preserves_axes_and_represents_contradiction() {
        let rust = LanguageCode::new("rust").expect("valid language");
        let left_path = ExactRepoRelativePathV1::new("left/lib.rs").expect("valid path");
        let right_path = ExactRepoRelativePathV1::new("right/lib.rs").expect("valid path");
        let combined = QueryConstraintSetV1::from_languages([rust.clone()]).intersect(
            &QueryConstraintSetV1::from_exact_repo_relative_path(left_path.clone()),
        );
        assert_eq!(
            combined,
            QueryConstraintIntersectionV1::Compatible(
                QueryConstraintSetV1::from_languages([rust.clone()])
                    .with_exact_repo_relative_path(left_path.clone())
            )
        );
        assert_eq!(
            QueryConstraintSetV1::from_exact_repo_relative_path(left_path).intersect(
                &QueryConstraintSetV1::from_exact_repo_relative_path(right_path),
            ),
            QueryConstraintIntersectionV1::Contradiction
        );
        assert_eq!(
            QueryConstraintSetV1::from_languages([rust]).intersect(
                &QueryConstraintSetV1::from_languages([
                    LanguageCode::new("python").expect("valid language")
                ]),
            ),
            QueryConstraintIntersectionV1::Contradiction
        );
    }
}
