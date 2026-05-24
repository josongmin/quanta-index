use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{GenerationPin, LexicalCandidate, SearchExplanation};

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneLexicalQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
}

const SEARCH_PLANE_LEXICAL_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results"];

impl Serialize for SearchPlaneLexicalQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneLexicalQueryResponse", 2)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("results", &self.results)?;
        state.end()
    }
}

struct SearchPlaneLexicalQueryResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneLexicalQueryResponseVisitor {
    type Value = SearchPlaneLexicalQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneLexicalQueryResponse map")
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
                        SEARCH_PLANE_LEXICAL_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        let generation = generation.ok_or_else(|| de::Error::missing_field("generation"))?;
        let results = results.ok_or_else(|| de::Error::missing_field("results"))?;
        Ok(SearchPlaneLexicalQueryResponse {
            generation,
            results,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneLexicalQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneLexicalQueryResponse",
            SEARCH_PLANE_LEXICAL_QUERY_RESPONSE_FIELDS,
            SearchPlaneLexicalQueryResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneSemanticQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
}

const SEARCH_PLANE_SEMANTIC_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results"];

impl Serialize for SearchPlaneSemanticQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneSemanticQueryResponse", 2)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("results", &self.results)?;
        state.end()
    }
}

struct SearchPlaneSemanticQueryResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneSemanticQueryResponseVisitor {
    type Value = SearchPlaneSemanticQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneSemanticQueryResponse map")
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
                        SEARCH_PLANE_SEMANTIC_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        let generation = generation.ok_or_else(|| de::Error::missing_field("generation"))?;
        let results = results.ok_or_else(|| de::Error::missing_field("results"))?;
        Ok(SearchPlaneSemanticQueryResponse {
            generation,
            results,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneSemanticQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneSemanticQueryResponse",
            SEARCH_PLANE_SEMANTIC_QUERY_RESPONSE_FIELDS,
            SearchPlaneSemanticQueryResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneHybridQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
}

const SEARCH_PLANE_HYBRID_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results"];

impl Serialize for SearchPlaneHybridQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneHybridQueryResponse", 2)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("results", &self.results)?;
        state.end()
    }
}

struct SearchPlaneHybridQueryResponseVisitor;

impl<'de> Visitor<'de> for SearchPlaneHybridQueryResponseVisitor {
    type Value = SearchPlaneHybridQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneHybridQueryResponse map")
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
                        SEARCH_PLANE_HYBRID_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        let generation = generation.ok_or_else(|| de::Error::missing_field("generation"))?;
        let results = results.ok_or_else(|| de::Error::missing_field("results"))?;
        Ok(SearchPlaneHybridQueryResponse {
            generation,
            results,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneHybridQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneHybridQueryResponse",
            SEARCH_PLANE_HYBRID_QUERY_RESPONSE_FIELDS,
            SearchPlaneHybridQueryResponseVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
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
                        SEARCH_PLANE_EXPLAIN_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        let generation = generation.ok_or_else(|| de::Error::missing_field("generation"))?;
        let explanation = explanation.ok_or_else(|| de::Error::missing_field("explanation"))?;
        Ok(SearchPlaneExplainQueryResponse {
            generation,
            explanation,
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
