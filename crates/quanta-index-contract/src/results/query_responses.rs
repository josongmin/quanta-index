use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    BridgeCandidatePacket, CommitCandidate, DiffCandidate, GenerationPin, LexicalCandidate,
    StructuralCandidate,
};

use super::SearchExplanation;

#[derive(Clone, Debug, PartialEq)]
pub struct TextQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
}

pub type SymbolQueryResponse = TextQueryResponse;

const TEXT_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results"];

impl Serialize for TextQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("TextQueryResponse", 2)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("results", &self.results)?;
        state.end()
    }
}

struct TextQueryResponseVisitor;

impl<'de> Visitor<'de> for TextQueryResponseVisitor {
    type Value = TextQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a TextQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut results: Option<Vec<LexicalCandidate>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "results" => {
                    if results.is_some() {
                        return Err(de::Error::duplicate_field("results"));
                    }
                    results = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, TEXT_QUERY_RESPONSE_FIELDS));
                }
            }
        }
        Ok(TextQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            results: results.ok_or_else(|| de::Error::missing_field("results"))?,
        })
    }
}

impl<'de> Deserialize<'de> for TextQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "TextQueryResponse",
            TEXT_QUERY_RESPONSE_FIELDS,
            TextQueryResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
    pub explanation: SearchExplanation,
}

const SEMANTIC_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "explanation"];

impl Serialize for SemanticQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticQueryResponse", 3)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("results", &self.results)?;
        state.serialize_field("explanation", &self.explanation)?;
        state.end()
    }
}

struct SemanticQueryResponseVisitor;

impl<'de> Visitor<'de> for SemanticQueryResponseVisitor {
    type Value = SemanticQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut results: Option<Vec<LexicalCandidate>> = None;
        let mut explanation: Option<SearchExplanation> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "results" => {
                    if results.is_some() {
                        return Err(de::Error::duplicate_field("results"));
                    }
                    results = Some(map.next_value()?);
                }
                "explanation" => {
                    if explanation.is_some() {
                        return Err(de::Error::duplicate_field("explanation"));
                    }
                    explanation = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            results: results.ok_or_else(|| de::Error::missing_field("results"))?,
            explanation: explanation.ok_or_else(|| de::Error::missing_field("explanation"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticQueryResponse",
            SEMANTIC_QUERY_RESPONSE_FIELDS,
            SemanticQueryResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HybridQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
    pub explanation: SearchExplanation,
}

const HYBRID_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "explanation"];

impl Serialize for HybridQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HybridQueryResponse", 3)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("results", &self.results)?;
        state.serialize_field("explanation", &self.explanation)?;
        state.end()
    }
}

struct HybridQueryResponseVisitor;

impl<'de> Visitor<'de> for HybridQueryResponseVisitor {
    type Value = HybridQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HybridQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut results: Option<Vec<LexicalCandidate>> = None;
        let mut explanation: Option<SearchExplanation> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "results" => {
                    if results.is_some() {
                        return Err(de::Error::duplicate_field("results"));
                    }
                    results = Some(map.next_value()?);
                }
                "explanation" => {
                    if explanation.is_some() {
                        return Err(de::Error::duplicate_field("explanation"));
                    }
                    explanation = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        HYBRID_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        Ok(HybridQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            results: results.ok_or_else(|| de::Error::missing_field("results"))?,
            explanation: explanation.ok_or_else(|| de::Error::missing_field("explanation"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HybridQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HybridQueryResponse",
            HYBRID_QUERY_RESPONSE_FIELDS,
            HybridQueryResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneHistoryQueryResponse {
    pub generation: GenerationPin,
    pub commits: Vec<CommitCandidate>,
    pub diffs: Vec<DiffCandidate>,
}

const SEARCH_PLANE_HISTORY_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "commits", "diffs"];

impl Serialize for SearchPlaneHistoryQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneHistoryQueryResponse", 3)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("commits", &self.commits)?;
        state.serialize_field("diffs", &self.diffs)?;
        state.end()
    }
}

struct SearchPlaneHistoryQueryResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneHistoryQueryResponseVisitor {
    type Value = SearchPlaneHistoryQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneHistoryQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut commits: Option<Vec<CommitCandidate>> = None;
        let mut diffs: Option<Vec<DiffCandidate>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => generation = Some(map.next_value()?),
                "commits" => commits = Some(map.next_value()?),
                "diffs" => diffs = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_HISTORY_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneHistoryQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            commits: commits.ok_or_else(|| de::Error::missing_field("commits"))?,
            diffs: diffs.ok_or_else(|| de::Error::missing_field("diffs"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneHistoryQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneHistoryQueryResponse",
            SEARCH_PLANE_HISTORY_QUERY_RESPONSE_FIELDS,
            SearchPlaneHistoryQueryResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneStructuralQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<StructuralCandidate>,
}

const SEARCH_PLANE_STRUCTURAL_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results"];

impl Serialize for SearchPlaneStructuralQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneStructuralQueryResponse", 2)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("results", &self.results)?;
        state.end()
    }
}

struct SearchPlaneStructuralQueryResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneStructuralQueryResponseVisitor {
    type Value = SearchPlaneStructuralQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneStructuralQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut results: Option<Vec<StructuralCandidate>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => generation = Some(map.next_value()?),
                "results" => results = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_STRUCTURAL_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneStructuralQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            results: results.ok_or_else(|| de::Error::missing_field("results"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneStructuralQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneStructuralQueryResponse",
            SEARCH_PLANE_STRUCTURAL_QUERY_RESPONSE_FIELDS,
            SearchPlaneStructuralQueryResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneBridgeQueryResponse {
    pub generation: GenerationPin,
    pub packet: BridgeCandidatePacket,
}

const SEARCH_PLANE_BRIDGE_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "packet"];

impl Serialize for SearchPlaneBridgeQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneBridgeQueryResponse", 2)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("packet", &self.packet)?;
        state.end()
    }
}

struct SearchPlaneBridgeQueryResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneBridgeQueryResponseVisitor {
    type Value = SearchPlaneBridgeQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneBridgeQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut packet: Option<BridgeCandidatePacket> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => generation = Some(map.next_value()?),
                "packet" => packet = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_BRIDGE_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneBridgeQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            packet: packet.ok_or_else(|| de::Error::missing_field("packet"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneBridgeQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneBridgeQueryResponse",
            SEARCH_PLANE_BRIDGE_QUERY_RESPONSE_FIELDS,
            SearchPlaneBridgeQueryResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneExplainQueryResponse {
    pub generation: GenerationPin,
    pub explanation: SearchExplanation,
}

const SEARCH_PLANE_EXPLAIN_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "explanation"];

impl Serialize for SearchPlaneExplainQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneExplainQueryResponse", 2)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("explanation", &self.explanation)?;
        state.end()
    }
}

struct SearchPlaneExplainQueryResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneExplainQueryResponseVisitor {
    type Value = SearchPlaneExplainQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneExplainQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut explanation: Option<SearchExplanation> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => generation = Some(map.next_value()?),
                "explanation" => explanation = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_EXPLAIN_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneExplainQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            explanation: explanation.ok_or_else(|| de::Error::missing_field("explanation"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneExplainQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneExplainQueryResponse",
            SEARCH_PLANE_EXPLAIN_QUERY_RESPONSE_FIELDS,
            SearchPlaneExplainQueryResponseVisitor,
        )
    }
}

/// Response paired with [`crate::SearchPlaneSourcegraphQueryRequest`].
///
/// PRE-CONTRACT-EXT additive. Carries a `GenerationPin` so callers can
/// verify the resolved manifest matches their request pin, plus a flat
/// `Vec<LexicalCandidate>` so downstream code can re-use the existing
/// lexical candidate plumbing.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneSourcegraphQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
}

const SEARCH_PLANE_SOURCEGRAPH_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results"];

impl Serialize for SearchPlaneSourcegraphQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneSourcegraphQueryResponse", 2)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("results", &self.results)?;
        state.end()
    }
}

struct SearchPlaneSourcegraphQueryResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneSourcegraphQueryResponseVisitor {
    type Value = SearchPlaneSourcegraphQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneSourcegraphQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut results: Option<Vec<LexicalCandidate>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "results" => {
                    if results.is_some() {
                        return Err(de::Error::duplicate_field("results"));
                    }
                    results = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_SOURCEGRAPH_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneSourcegraphQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            results: results.ok_or_else(|| de::Error::missing_field("results"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneSourcegraphQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneSourcegraphQueryResponse",
            SEARCH_PLANE_SOURCEGRAPH_QUERY_RESPONSE_FIELDS,
            SearchPlaneSourcegraphQueryResponseVisitor,
        )
    }
}

#[cfg(test)]
mod sourcegraph_response_tests {
    use super::SearchPlaneSourcegraphQueryResponse;
    use crate::{GenerationPin, ManifestGeneration, RepoId, RevisionId};

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    fn encode<T: serde::Serialize>(v: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut buf: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(v, &mut buf)?;
        Ok(buf)
    }

    fn decode<T>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        Ok(ciborium::de::from_reader(bytes)?)
    }

    fn sample_pin() -> GenerationPin {
        GenerationPin::new(
            RepoId::new("repo-x"),
            RevisionId::new("rev-y"),
            ManifestGeneration::new(13),
        )
    }

    #[test]
    fn sourcegraph_response_cbor_roundtrip() -> TestRes {
        let v = SearchPlaneSourcegraphQueryResponse {
            generation: sample_pin(),
            results: Vec::new(),
        };
        let bytes = encode(&v)?;
        let back: SearchPlaneSourcegraphQueryResponse = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_response_unknown_field_rejected() -> TestRes {
        let v = SearchPlaneSourcegraphQueryResponse {
            generation: sample_pin(),
            results: Vec::new(),
        };
        let bytes = encode(&v)?;
        let mut wire: ciborium::Value = decode(&bytes)?;
        let ciborium::Value::Map(fields) = &mut wire else {
            return Err("expected map".into());
        };
        fields.push((
            ciborium::Value::Text("__never_field".to_owned()),
            ciborium::Value::Bool(true),
        ));
        let mutated = encode(&wire)?;
        let decoded: Result<SearchPlaneSourcegraphQueryResponse, _> =
            ciborium::de::from_reader::<SearchPlaneSourcegraphQueryResponse, _>(mutated.as_slice());
        if decoded.is_ok() {
            return Err("unknown field should have been rejected".into());
        }
        Ok(())
    }
}
