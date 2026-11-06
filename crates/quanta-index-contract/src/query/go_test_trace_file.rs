//! Explicit Go test failure file-search selector over two pinned hybrid lanes.

use core::fmt;
use std::collections::BTreeSet;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    ExactRepoRelativePathV1, GoTestTraceFrameClassV1, HybridQueryRequest, RepoId,
    validate_public_top_k,
};

/// Canonical source-frame hints parsed from the raw, public failure excerpt.
/// Matching text is query-bound; it does not prove an actual call-stack frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoTestTraceSourceFramesV1 {
    paths: BTreeSet<ExactRepoRelativePathV1>,
}

impl GoTestTraceSourceFramesV1 {
    /// Parse relative `*.go:<line>` tokens as ranking hints, not frame authority.
    #[must_use]
    pub fn from_failure_excerpt(text: &str) -> Self {
        let mut paths = BTreeSet::new();
        for token in text.split_whitespace() {
            let token = token.trim_matches(|ch: char| {
                matches!(ch, '(' | ')' | '[' | ']' | ',' | '"' | '\'' | '`')
            });
            let Some((raw_path, line_and_rest)) = token.split_once(".go:") else {
                continue;
            };
            if !line_and_rest.starts_with(|ch: char| ch.is_ascii_digit()) {
                continue;
            }
            let raw_path = raw_path.strip_prefix("./").unwrap_or(raw_path);
            let path = format!("{raw_path}.go");
            if path.ends_with("_test.go") {
                continue;
            }
            if let Ok(path) = ExactRepoRelativePathV1::new(path) {
                let _inserted = paths.insert(path);
            }
        }
        Self { paths }
    }

    #[must_use]
    pub fn frame_class(&self) -> GoTestTraceFrameClassV1 {
        match self.paths.len() {
            0 => GoTestTraceFrameClassV1::Unresolved,
            1 => GoTestTraceFrameClassV1::Exact,
            _ => GoTestTraceFrameClassV1::Ambiguous,
        }
    }

    #[must_use]
    pub fn paths(&self) -> &BTreeSet<ExactRepoRelativePathV1> {
        &self.paths
    }

    #[must_use]
    pub fn contains_path(&self, path: &str) -> bool {
        self.paths.iter().any(|frame| frame.as_str() == path)
    }
}

/// Maximum admitted public failure excerpt, independently of the dense input.
pub const GO_TEST_TRACE_FAILURE_MAX_BYTES_V1: usize = 16 * 1024;

/// Explicit Go test failure profile for investigating production faults.
/// Production `.go` files rank before `_test.go` files and docs. Parsed frames
/// from raw `failure_excerpt` are query-bound ranking hints, not call-stack
/// authority. `failure_source_repo_id` names the owner of a relative frame
/// when known; without it no file receives a frame hint. `query.top_k` bounds
/// each chunk lane; `top_k` bounds files.
#[derive(Clone, Debug, PartialEq)]
pub struct GoTestTraceFileQueryRequestV1 {
    pub query: HybridQueryRequest,
    pub failure_excerpt: String,
    pub failure_source_repo_id: Option<RepoId>,
    pub top_k: u32,
}

const FIELDS: &[&str] = &[
    "query",
    "failure_excerpt",
    "failure_source_repo_id",
    "top_k",
];

impl GoTestTraceFileQueryRequestV1 {
    pub fn validate_v1(&self) -> Result<(), String> {
        let chunk_top_k = validate_public_top_k(self.query.top_k)
            .map_err(|error| format!("{}: {error}", error.code()))?;
        let _lexical_top_k = validate_public_top_k(self.query.text_query.top_k)
            .map_err(|error| format!("{}: {error}", error.code()))?;
        let file_top_k = validate_public_top_k(self.top_k)
            .map_err(|error| format!("{}: {error}", error.code()))?;
        if file_top_k > chunk_top_k {
            return Err("Go trace file top_k exceeds the per-lane chunk cap".to_string());
        }
        if self.failure_excerpt.trim().is_empty()
            || self.failure_excerpt.contains('\0')
            || self.failure_excerpt.len() > GO_TEST_TRACE_FAILURE_MAX_BYTES_V1
        {
            return Err("Go trace failure_excerpt is empty or exceeds 16 KiB".to_string());
        }
        if self.query.semantic_query_text.trim().is_empty()
            || self.query.semantic_query_text.contains('\0')
            || self.query.semantic_query_text.len() > GO_TEST_TRACE_FAILURE_MAX_BYTES_V1
        {
            return Err("Go trace semantic query text is empty or exceeds 16 KiB".to_string());
        }
        if self.query.text_query.query_text.trim().is_empty()
            || self.query.text_query.query_text.contains('\0')
            || self.query.text_query.query_text.len() > GO_TEST_TRACE_FAILURE_MAX_BYTES_V1
        {
            return Err("Go trace lexical query text is empty or exceeds 16 KiB".to_string());
        }
        Ok(())
    }
}

impl Serialize for GoTestTraceFileQueryRequestV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("GoTestTraceFileQueryRequestV1", 4)?;
        state.serialize_field("query", &self.query)?;
        state.serialize_field("failure_excerpt", &self.failure_excerpt)?;
        state.serialize_field("failure_source_repo_id", &self.failure_source_repo_id)?;
        state.serialize_field("top_k", &self.top_k)?;
        state.end()
    }
}

struct RequestVisitor;

impl<'de> Visitor<'de> for RequestVisitor {
    type Value = GoTestTraceFileQueryRequestV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a GoTestTraceFileQueryRequestV1 map")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut query = None;
        let mut failure_excerpt = None;
        let mut failure_source_repo_id: Option<Option<RepoId>> = None;
        let mut top_k = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "query" => {
                    if query.is_some() {
                        return Err(de::Error::duplicate_field("query"));
                    }
                    query = Some(map.next_value()?);
                }
                "failure_excerpt" => {
                    if failure_excerpt.is_some() {
                        return Err(de::Error::duplicate_field("failure_excerpt"));
                    }
                    failure_excerpt = Some(map.next_value()?);
                }
                "failure_source_repo_id" => {
                    if failure_source_repo_id.is_some() {
                        return Err(de::Error::duplicate_field("failure_source_repo_id"));
                    }
                    failure_source_repo_id = Some(map.next_value()?);
                }
                "top_k" => {
                    if top_k.is_some() {
                        return Err(de::Error::duplicate_field("top_k"));
                    }
                    top_k = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, FIELDS)),
            }
        }
        let request = GoTestTraceFileQueryRequestV1 {
            query: query.ok_or_else(|| de::Error::missing_field("query"))?,
            failure_excerpt: failure_excerpt
                .ok_or_else(|| de::Error::missing_field("failure_excerpt"))?,
            failure_source_repo_id: failure_source_repo_id
                .ok_or_else(|| de::Error::missing_field("failure_source_repo_id"))?,
            top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
        };
        request.validate_v1().map_err(de::Error::custom)?;
        Ok(request)
    }
}

impl<'de> Deserialize<'de> for GoTestTraceFileQueryRequestV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct("GoTestTraceFileQueryRequestV1", FIELDS, RequestVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax};

    fn request() -> GoTestTraceFileQueryRequestV1 {
        GoTestTraceFileQueryRequestV1 {
            query: HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "case:no panic".to_string(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: None,
                    generation_selector: None,
                    top_k: 100,
                    cursor: None,
                },
                semantic_query_text: "panic at binding.go:42".to_string(),
                generation: None,
                generation_selector: None,
                top_k: 100,
            },
            failure_excerpt: "$ go test ./...\npanic at binding.go:42".to_string(),
            failure_source_repo_id: None,
            top_k: 20,
        }
    }

    #[test]
    fn request_roundtrips_and_refuses_unbounded_or_malformed_input() {
        let valid = request();
        let encoded = serde_json::to_string(&valid).expect("request encode");
        assert_eq!(
            serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&encoded)
                .expect("request decode"),
            valid
        );
        assert!(encoded.contains("\"failure_source_repo_id\":null"));
        let missing_owner = encoded.replacen("\"failure_source_repo_id\":null,", "", 1);
        assert!(serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&missing_owner).is_err());
        let duplicate_owner = encoded.replacen(
            "\"failure_source_repo_id\":null",
            "\"failure_source_repo_id\":null,\"failure_source_repo_id\":null",
            1,
        );
        assert!(serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&duplicate_owner).is_err());
        let mut with_owner = valid.clone();
        with_owner.failure_source_repo_id = Some(RepoId::new("source-repo").expect("owner"));
        let with_owner_wire = serde_json::to_string(&with_owner).expect("owner encode");
        assert_eq!(
            serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&with_owner_wire)
                .expect("owner decode"),
            with_owner
        );
        let mut bad = valid.clone();
        bad.top_k = 101;
        assert!(bad.validate_v1().is_err());
        bad.top_k = 20;
        bad.failure_excerpt = "x".repeat(GO_TEST_TRACE_FAILURE_MAX_BYTES_V1 + 1);
        assert!(bad.validate_v1().is_err());
        bad.failure_excerpt = "\0".to_string();
        assert!(bad.validate_v1().is_err());
        bad = valid.clone();
        bad.query.text_query.top_k = 0;
        assert!(bad.validate_v1().is_err());
        let duplicate = encoded.replacen("\"top_k\":20", "\"top_k\":20,\"top_k\":20", 1);
        assert!(serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&duplicate).is_err());
        let unknown = encoded.replacen(
            "\"failure_excerpt\":",
            "\"unknown\":1,\"failure_excerpt\":",
            1,
        );
        assert!(serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&unknown).is_err());
    }
}
