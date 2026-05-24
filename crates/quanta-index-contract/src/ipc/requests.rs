use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{LexicalCandidate, LqFilterSet, LqQuery, GenerationPin};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneLexicalQueryRequest {
    pub query: LqQuery,
    pub generation: Option<GenerationPin>,
}

const SEARCH_PLANE_LEXICAL_QUERY_REQUEST_FIELDS: &[&str] = &["query", "generation"];

impl Serialize for SearchPlaneLexicalQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 1;
        if self.generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state =
            serializer.serialize_struct("SearchPlaneLexicalQueryRequest", field_count)?;
        state.serialize_field("query", &self.query)?;
        if let Some(generation) = &self.generation {
            state.serialize_field("generation", generation)?;
        }
        state.end()
    }
}

struct SearchPlaneLexicalQueryRequestVisitor;

impl<'de> Visitor<'de> for SearchPlaneLexicalQueryRequestVisitor {
    type Value = SearchPlaneLexicalQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneLexicalQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut query: Option<LqQuery> = None;
        let mut generation: Option<GenerationPin> = None;
        let mut generation_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "query" => {
                    if query.is_some() {
                        return Err(de::Error::duplicate_field("query"));
                    }
                    query = Some(map.next_value()?);
                }
                "generation" => {
                    if generation_seen {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation_seen = true;
                    generation = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_LEXICAL_QUERY_REQUEST_FIELDS,
                    ));
                }
            }
        }
        let query = query.ok_or_else(|| de::Error::missing_field("query"))?;
        Ok(SearchPlaneLexicalQueryRequest { query, generation })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneLexicalQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneLexicalQueryRequest",
            SEARCH_PLANE_LEXICAL_QUERY_REQUEST_FIELDS,
            SearchPlaneLexicalQueryRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneSemanticQueryRequest {
    pub query_text: String,
    pub generation: Option<GenerationPin>,
    pub lexical_filters: LqFilterSet,
    pub top_k: u32,
}

const SEARCH_PLANE_SEMANTIC_QUERY_REQUEST_FIELDS: &[&str] =
    &["query_text", "generation", "lexical_filters", "top_k"];

impl Serialize for SearchPlaneSemanticQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 3;
        if self.generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state =
            serializer.serialize_struct("SearchPlaneSemanticQueryRequest", field_count)?;
        state.serialize_field("query_text", &self.query_text)?;
        if let Some(generation) = &self.generation {
            state.serialize_field("generation", generation)?;
        }
        state.serialize_field("lexical_filters", &self.lexical_filters)?;
        state.serialize_field("top_k", &self.top_k)?;
        state.end()
    }
}

struct SearchPlaneSemanticQueryRequestVisitor;

impl<'de> Visitor<'de> for SearchPlaneSemanticQueryRequestVisitor {
    type Value = SearchPlaneSemanticQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneSemanticQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut query_text: Option<String> = None;
        let mut generation: Option<GenerationPin> = None;
        let mut generation_seen = false;
        let mut lexical_filters: Option<LqFilterSet> = None;
        let mut top_k: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "query_text" => {
                    if query_text.is_some() {
                        return Err(de::Error::duplicate_field("query_text"));
                    }
                    query_text = Some(map.next_value()?);
                }
                "generation" => {
                    if generation_seen {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation_seen = true;
                    generation = Some(map.next_value()?);
                }
                "lexical_filters" => {
                    if lexical_filters.is_some() {
                        return Err(de::Error::duplicate_field("lexical_filters"));
                    }
                    lexical_filters = Some(map.next_value()?);
                }
                "top_k" => {
                    if top_k.is_some() {
                        return Err(de::Error::duplicate_field("top_k"));
                    }
                    top_k = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_SEMANTIC_QUERY_REQUEST_FIELDS,
                    ));
                }
            }
        }
        let query_text = query_text.ok_or_else(|| de::Error::missing_field("query_text"))?;
        let lexical_filters =
            lexical_filters.ok_or_else(|| de::Error::missing_field("lexical_filters"))?;
        let top_k = top_k.ok_or_else(|| de::Error::missing_field("top_k"))?;
        Ok(SearchPlaneSemanticQueryRequest {
            query_text,
            generation,
            lexical_filters,
            top_k,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneSemanticQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneSemanticQueryRequest",
            SEARCH_PLANE_SEMANTIC_QUERY_REQUEST_FIELDS,
            SearchPlaneSemanticQueryRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneHybridQueryRequest {
    pub lexical_query: LqQuery,
    pub semantic_query_text: String,
    pub generation: Option<GenerationPin>,
    pub top_k: u32,
}

const SEARCH_PLANE_HYBRID_QUERY_REQUEST_FIELDS: &[&str] = &[
    "lexical_query",
    "semantic_query_text",
    "generation",
    "top_k",
];

impl Serialize for SearchPlaneHybridQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 3;
        if self.generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state =
            serializer.serialize_struct("SearchPlaneHybridQueryRequest", field_count)?;
        state.serialize_field("lexical_query", &self.lexical_query)?;
        state.serialize_field("semantic_query_text", &self.semantic_query_text)?;
        if let Some(generation) = &self.generation {
            state.serialize_field("generation", generation)?;
        }
        state.serialize_field("top_k", &self.top_k)?;
        state.end()
    }
}

struct SearchPlaneHybridQueryRequestVisitor;

impl<'de> Visitor<'de> for SearchPlaneHybridQueryRequestVisitor {
    type Value = SearchPlaneHybridQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneHybridQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut lexical_query: Option<LqQuery> = None;
        let mut semantic_query_text: Option<String> = None;
        let mut generation: Option<GenerationPin> = None;
        let mut generation_seen = false;
        let mut top_k: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "lexical_query" => {
                    if lexical_query.is_some() {
                        return Err(de::Error::duplicate_field("lexical_query"));
                    }
                    lexical_query = Some(map.next_value()?);
                }
                "semantic_query_text" => {
                    if semantic_query_text.is_some() {
                        return Err(de::Error::duplicate_field("semantic_query_text"));
                    }
                    semantic_query_text = Some(map.next_value()?);
                }
                "generation" => {
                    if generation_seen {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation_seen = true;
                    generation = Some(map.next_value()?);
                }
                "top_k" => {
                    if top_k.is_some() {
                        return Err(de::Error::duplicate_field("top_k"));
                    }
                    top_k = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_HYBRID_QUERY_REQUEST_FIELDS,
                    ));
                }
            }
        }
        let lexical_query =
            lexical_query.ok_or_else(|| de::Error::missing_field("lexical_query"))?;
        let semantic_query_text =
            semantic_query_text.ok_or_else(|| de::Error::missing_field("semantic_query_text"))?;
        let top_k = top_k.ok_or_else(|| de::Error::missing_field("top_k"))?;
        Ok(SearchPlaneHybridQueryRequest {
            lexical_query,
            semantic_query_text,
            generation,
            top_k,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneHybridQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneHybridQueryRequest",
            SEARCH_PLANE_HYBRID_QUERY_REQUEST_FIELDS,
            SearchPlaneHybridQueryRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneExplainQueryRequest {
    pub generation: GenerationPin,
    pub candidate: LexicalCandidate,
}

const SEARCH_PLANE_EXPLAIN_QUERY_REQUEST_FIELDS: &[&str] = &["generation", "candidate"];

impl Serialize for SearchPlaneExplainQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SearchPlaneExplainQueryRequest", 2)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("candidate", &self.candidate)?;
        state.end()
    }
}

struct SearchPlaneExplainQueryRequestVisitor;

impl<'de> Visitor<'de> for SearchPlaneExplainQueryRequestVisitor {
    type Value = SearchPlaneExplainQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneExplainQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut candidate: Option<LexicalCandidate> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "candidate" => {
                    if candidate.is_some() {
                        return Err(de::Error::duplicate_field("candidate"));
                    }
                    candidate = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_EXPLAIN_QUERY_REQUEST_FIELDS,
                    ));
                }
            }
        }
        let generation = generation.ok_or_else(|| de::Error::missing_field("generation"))?;
        let candidate = candidate.ok_or_else(|| de::Error::missing_field("candidate"))?;
        Ok(SearchPlaneExplainQueryRequest {
            generation,
            candidate,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneExplainQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneExplainQueryRequest",
            SEARCH_PLANE_EXPLAIN_QUERY_REQUEST_FIELDS,
            SearchPlaneExplainQueryRequestVisitor,
        )
    }
}
