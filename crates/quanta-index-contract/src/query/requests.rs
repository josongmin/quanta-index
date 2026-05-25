use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{BridgeTarget, LexicalCandidate, SemanticVectorRef};

use super::{GenerationPin, GenerationSelector, TextQueryRequest, TextQuerySyntax};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticCandidateScope {
    pub syntax: TextQuerySyntax,
    pub query_text: String,
    pub generation: Option<GenerationPin>,
    pub generation_selector: Option<GenerationSelector>,
}

const SEMANTIC_CANDIDATE_SCOPE_FIELDS: &[&str] =
    &["syntax", "query_text", "generation", "generation_selector"];

impl Serialize for SemanticCandidateScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 2;
        if self.generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.generation_selector.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("SemanticCandidateScope", field_count)?;
        state.serialize_field("syntax", &self.syntax)?;
        state.serialize_field("query_text", &self.query_text)?;
        if let Some(generation) = &self.generation {
            state.serialize_field("generation", generation)?;
        }
        if let Some(generation_selector) = &self.generation_selector {
            state.serialize_field("generation_selector", generation_selector)?;
        }
        state.end()
    }
}

struct SemanticCandidateScopeVisitor;

impl<'de> Visitor<'de> for SemanticCandidateScopeVisitor {
    type Value = SemanticCandidateScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticCandidateScope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut syntax: Option<TextQuerySyntax> = None;
        let mut query_text: Option<String> = None;
        let mut generation: Option<GenerationPin> = None;
        let mut generation_seen = false;
        let mut generation_selector: Option<GenerationSelector> = None;
        let mut generation_selector_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "syntax" => {
                    if syntax.is_some() {
                        return Err(de::Error::duplicate_field("syntax"));
                    }
                    syntax = Some(map.next_value()?);
                }
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
                "generation_selector" => {
                    if generation_selector_seen {
                        return Err(de::Error::duplicate_field("generation_selector"));
                    }
                    generation_selector_seen = true;
                    generation_selector = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEMANTIC_CANDIDATE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticCandidateScope {
            syntax: syntax.ok_or_else(|| de::Error::missing_field("syntax"))?,
            query_text: query_text.ok_or_else(|| de::Error::missing_field("query_text"))?,
            generation,
            generation_selector,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticCandidateScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticCandidateScope",
            SEMANTIC_CANDIDATE_SCOPE_FIELDS,
            SemanticCandidateScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticQueryRequest {
    /// QI-QRY-01 phase 2: optional text fallback for callers that encode the
    /// query vector as a space-separated decimal list (legacy CLI path).
    /// SDK callers using `query_vector_ref` should pass `None` — the field
    /// is no longer populated with `String::new()` as a filler.
    pub query_text: Option<String>,
    pub query_vector: Option<Vec<f32>>,
    pub query_vector_ref: Option<SemanticVectorRef>,
    pub generation: Option<GenerationPin>,
    pub generation_selector: Option<GenerationSelector>,
    pub scope: Option<SemanticCandidateScope>,
    pub top_k: u32,
}

const SEMANTIC_QUERY_REQUEST_FIELDS: &[&str] = &[
    "query_text",
    "query_vector",
    "query_vector_ref",
    "generation",
    "generation_selector",
    "scope",
    "top_k",
];

impl Serialize for SemanticQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 1;
        if self.query_text.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.generation_selector.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.scope.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.query_vector.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.query_vector_ref.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("SemanticQueryRequest", field_count)?;
        if let Some(query_text) = &self.query_text {
            state.serialize_field("query_text", query_text)?;
        }
        if let Some(query_vector) = &self.query_vector {
            state.serialize_field("query_vector", query_vector)?;
        }
        if let Some(query_vector_ref) = &self.query_vector_ref {
            state.serialize_field("query_vector_ref", query_vector_ref)?;
        }
        if let Some(generation) = &self.generation {
            state.serialize_field("generation", generation)?;
        }
        if let Some(generation_selector) = &self.generation_selector {
            state.serialize_field("generation_selector", generation_selector)?;
        }
        if let Some(scope) = &self.scope {
            state.serialize_field("scope", scope)?;
        }
        state.serialize_field("top_k", &self.top_k)?;
        state.end()
    }
}

struct SemanticQueryRequestVisitor;

impl<'de> Visitor<'de> for SemanticQueryRequestVisitor {
    type Value = SemanticQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut query_text: Option<String> = None;
        let mut query_vector: Option<Vec<f32>> = None;
        let mut query_vector_ref: Option<SemanticVectorRef> = None;
        let mut generation: Option<GenerationPin> = None;
        let mut generation_seen = false;
        let mut generation_selector: Option<GenerationSelector> = None;
        let mut generation_selector_seen = false;
        let mut scope: Option<Option<SemanticCandidateScope>> = None;
        let mut top_k: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "query_text" => {
                    if query_text.is_some() {
                        return Err(de::Error::duplicate_field("query_text"));
                    }
                    query_text = Some(map.next_value()?);
                }
                "query_vector" => {
                    if query_vector.is_some() {
                        return Err(de::Error::duplicate_field("query_vector"));
                    }
                    query_vector = Some(map.next_value()?);
                }
                "query_vector_ref" => {
                    if query_vector_ref.is_some() {
                        return Err(de::Error::duplicate_field("query_vector_ref"));
                    }
                    query_vector_ref = Some(map.next_value()?);
                }
                "generation" => {
                    if generation_seen {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation_seen = true;
                    generation = Some(map.next_value()?);
                }
                "generation_selector" => {
                    if generation_selector_seen {
                        return Err(de::Error::duplicate_field("generation_selector"));
                    }
                    generation_selector_seen = true;
                    generation_selector = Some(map.next_value()?);
                }
                "scope" => {
                    if scope.is_some() {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope = Some(Some(map.next_value()?));
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
                        SEMANTIC_QUERY_REQUEST_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticQueryRequest {
            query_text,
            query_vector,
            query_vector_ref,
            generation,
            generation_selector,
            scope: scope.unwrap_or(None),
            top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticQueryRequest",
            SEMANTIC_QUERY_REQUEST_FIELDS,
            SemanticQueryRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HybridQueryRequest {
    pub text_query: TextQueryRequest,
    /// QI-QRY-01 phase 2: optional text fallback for the semantic leg of a
    /// hybrid query (legacy CLI path that encodes the vector as a
    /// space-separated decimal list). SDK callers using `semantic_vector_ref`
    /// should pass `None` — the field is no longer populated with
    /// `String::new()` as a filler.
    pub semantic_query_text: Option<String>,
    pub semantic_vector: Option<Vec<f32>>,
    pub semantic_vector_ref: Option<SemanticVectorRef>,
    pub generation: Option<GenerationPin>,
    pub generation_selector: Option<GenerationSelector>,
    pub top_k: u32,
}

const HYBRID_QUERY_REQUEST_FIELDS: &[&str] = &[
    "text_query",
    "semantic_query_text",
    "semantic_vector",
    "semantic_vector_ref",
    "generation",
    "generation_selector",
    "top_k",
];

impl Serialize for HybridQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 2;
        if self.semantic_query_text.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.generation_selector.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.semantic_vector.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.semantic_vector_ref.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("HybridQueryRequest", field_count)?;
        state.serialize_field("text_query", &self.text_query)?;
        if let Some(semantic_query_text) = &self.semantic_query_text {
            state.serialize_field("semantic_query_text", semantic_query_text)?;
        }
        if let Some(semantic_vector) = &self.semantic_vector {
            state.serialize_field("semantic_vector", semantic_vector)?;
        }
        if let Some(semantic_vector_ref) = &self.semantic_vector_ref {
            state.serialize_field("semantic_vector_ref", semantic_vector_ref)?;
        }
        if let Some(generation) = &self.generation {
            state.serialize_field("generation", generation)?;
        }
        if let Some(generation_selector) = &self.generation_selector {
            state.serialize_field("generation_selector", generation_selector)?;
        }
        state.serialize_field("top_k", &self.top_k)?;
        state.end()
    }
}

struct HybridQueryRequestVisitor;

impl<'de> Visitor<'de> for HybridQueryRequestVisitor {
    type Value = HybridQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HybridQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut text_query: Option<TextQueryRequest> = None;
        let mut semantic_query_text: Option<String> = None;
        let mut semantic_vector: Option<Vec<f32>> = None;
        let mut semantic_vector_ref: Option<SemanticVectorRef> = None;
        let mut generation: Option<GenerationPin> = None;
        let mut generation_seen = false;
        let mut generation_selector: Option<GenerationSelector> = None;
        let mut generation_selector_seen = false;
        let mut top_k: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "text_query" => {
                    if text_query.is_some() {
                        return Err(de::Error::duplicate_field("text_query"));
                    }
                    text_query = Some(map.next_value()?);
                }
                "semantic_query_text" => {
                    if semantic_query_text.is_some() {
                        return Err(de::Error::duplicate_field("semantic_query_text"));
                    }
                    semantic_query_text = Some(map.next_value()?);
                }
                "semantic_vector" => {
                    if semantic_vector.is_some() {
                        return Err(de::Error::duplicate_field("semantic_vector"));
                    }
                    semantic_vector = Some(map.next_value()?);
                }
                "semantic_vector_ref" => {
                    if semantic_vector_ref.is_some() {
                        return Err(de::Error::duplicate_field("semantic_vector_ref"));
                    }
                    semantic_vector_ref = Some(map.next_value()?);
                }
                "generation" => {
                    if generation_seen {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation_seen = true;
                    generation = Some(map.next_value()?);
                }
                "generation_selector" => {
                    if generation_selector_seen {
                        return Err(de::Error::duplicate_field("generation_selector"));
                    }
                    generation_selector_seen = true;
                    generation_selector = Some(map.next_value()?);
                }
                "top_k" => {
                    if top_k.is_some() {
                        return Err(de::Error::duplicate_field("top_k"));
                    }
                    top_k = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, HYBRID_QUERY_REQUEST_FIELDS));
                }
            }
        }
        Ok(HybridQueryRequest {
            text_query: text_query.ok_or_else(|| de::Error::missing_field("text_query"))?,
            semantic_query_text,
            semantic_vector,
            semantic_vector_ref,
            generation,
            generation_selector,
            top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HybridQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HybridQueryRequest",
            HYBRID_QUERY_REQUEST_FIELDS,
            HybridQueryRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolQueryRequest {
    pub syntax: TextQuerySyntax,
    pub query_text: String,
    pub generation: Option<GenerationPin>,
    pub generation_selector: Option<GenerationSelector>,
    /// QI-QRY-01: required result cap. Wire field is mandatory; missing
    /// `top_k` fails-closed at deserialization via `missing_field`. No
    /// caller-side default — the SDK builder enforces this is set.
    pub top_k: u32,
}

const SYMBOL_QUERY_REQUEST_FIELDS: &[&str] = &[
    "syntax",
    "query_text",
    "generation",
    "generation_selector",
    "top_k",
];

impl Serialize for SymbolQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 3;
        if self.generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.generation_selector.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("SymbolQueryRequest", field_count)?;
        state.serialize_field("syntax", &self.syntax)?;
        state.serialize_field("query_text", &self.query_text)?;
        if let Some(generation) = &self.generation {
            state.serialize_field("generation", generation)?;
        }
        if let Some(generation_selector) = &self.generation_selector {
            state.serialize_field("generation_selector", generation_selector)?;
        }
        state.serialize_field("top_k", &self.top_k)?;
        state.end()
    }
}

struct SymbolQueryRequestVisitor;

impl<'de> Visitor<'de> for SymbolQueryRequestVisitor {
    type Value = SymbolQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SymbolQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut syntax: Option<TextQuerySyntax> = None;
        let mut query_text: Option<String> = None;
        let mut generation: Option<GenerationPin> = None;
        let mut generation_seen = false;
        let mut generation_selector: Option<GenerationSelector> = None;
        let mut generation_selector_seen = false;
        let mut top_k: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "syntax" => {
                    if syntax.is_some() {
                        return Err(de::Error::duplicate_field("syntax"));
                    }
                    syntax = Some(map.next_value()?);
                }
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
                "generation_selector" => {
                    if generation_selector_seen {
                        return Err(de::Error::duplicate_field("generation_selector"));
                    }
                    generation_selector_seen = true;
                    generation_selector = Some(map.next_value()?);
                }
                "top_k" => {
                    if top_k.is_some() {
                        return Err(de::Error::duplicate_field("top_k"));
                    }
                    top_k = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, SYMBOL_QUERY_REQUEST_FIELDS));
                }
            }
        }
        Ok(SymbolQueryRequest {
            syntax: syntax.ok_or_else(|| de::Error::missing_field("syntax"))?,
            query_text: query_text.ok_or_else(|| de::Error::missing_field("query_text"))?,
            generation,
            generation_selector,
            top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SymbolQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SymbolQueryRequest",
            SYMBOL_QUERY_REQUEST_FIELDS,
            SymbolQueryRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryQueryRequest {
    pub text_query: TextQueryRequest,
}

const HISTORY_QUERY_REQUEST_FIELDS: &[&str] = &["text_query"];

impl Serialize for HistoryQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HistoryQueryRequest", 1)?;
        state.serialize_field("text_query", &self.text_query)?;
        state.end()
    }
}

struct HistoryQueryRequestVisitor;

impl<'de> Visitor<'de> for HistoryQueryRequestVisitor {
    type Value = HistoryQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut text_query: Option<TextQueryRequest> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "text_query" => text_query = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        HISTORY_QUERY_REQUEST_FIELDS,
                    ));
                }
            }
        }
        Ok(HistoryQueryRequest {
            text_query: text_query.ok_or_else(|| de::Error::missing_field("text_query"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HistoryQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HistoryQueryRequest",
            HISTORY_QUERY_REQUEST_FIELDS,
            HistoryQueryRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralQueryRequest {
    pub text_query: TextQueryRequest,
}

const STRUCTURAL_QUERY_REQUEST_FIELDS: &[&str] = &["text_query"];

impl Serialize for StructuralQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralQueryRequest", 1)?;
        state.serialize_field("text_query", &self.text_query)?;
        state.end()
    }
}

struct StructuralQueryRequestVisitor;

impl<'de> Visitor<'de> for StructuralQueryRequestVisitor {
    type Value = StructuralQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a StructuralQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut text_query: Option<TextQueryRequest> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "text_query" => text_query = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        STRUCTURAL_QUERY_REQUEST_FIELDS,
                    ));
                }
            }
        }
        Ok(StructuralQueryRequest {
            text_query: text_query.ok_or_else(|| de::Error::missing_field("text_query"))?,
        })
    }
}

impl<'de> Deserialize<'de> for StructuralQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "StructuralQueryRequest",
            STRUCTURAL_QUERY_REQUEST_FIELDS,
            StructuralQueryRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeQueryRequest {
    pub text_query: TextQueryRequest,
    pub target: BridgeTarget,
}

const BRIDGE_QUERY_REQUEST_FIELDS: &[&str] = &["text_query", "target"];

impl Serialize for BridgeQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("BridgeQueryRequest", 2)?;
        state.serialize_field("text_query", &self.text_query)?;
        state.serialize_field("target", &self.target)?;
        state.end()
    }
}

struct BridgeQueryRequestVisitor;

impl<'de> Visitor<'de> for BridgeQueryRequestVisitor {
    type Value = BridgeQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a BridgeQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut text_query: Option<TextQueryRequest> = None;
        let mut target: Option<BridgeTarget> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "text_query" => text_query = Some(map.next_value()?),
                "target" => target = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(other, BRIDGE_QUERY_REQUEST_FIELDS));
                }
            }
        }
        Ok(BridgeQueryRequest {
            text_query: text_query.ok_or_else(|| de::Error::missing_field("text_query"))?,
            target: target.ok_or_else(|| de::Error::missing_field("target"))?,
        })
    }
}

impl<'de> Deserialize<'de> for BridgeQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "BridgeQueryRequest",
            BRIDGE_QUERY_REQUEST_FIELDS,
            BridgeQueryRequestVisitor,
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

/// Sourcegraph-syntax query request.
///
/// PRE-CONTRACT-EXT additive: payload is a raw Sourcegraph-syntax string.
/// The search-plane is responsible for parsing `source_syntax` against the
/// Sourcegraph DSL pinned to `sg_version`; producers may carry a
/// `GenerationPin` to fence the query against a specific manifest
/// generation. Wire-compat is preserved by appending to the split
/// query-plane IPC request enum.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchPlaneSourcegraphQueryRequest {
    pub source_syntax: Box<str>,
    pub sg_version: Box<str>,
    pub generation: Option<GenerationPin>,
    pub top_k: u32,
}

const SEARCH_PLANE_SOURCEGRAPH_QUERY_REQUEST_FIELDS: &[&str] =
    &["source_syntax", "sg_version", "generation", "top_k"];

impl Serialize for SearchPlaneSourcegraphQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 3;
        if self.generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state =
            serializer.serialize_struct("SearchPlaneSourcegraphQueryRequest", field_count)?;
        state.serialize_field("source_syntax", self.source_syntax.as_ref())?;
        state.serialize_field("sg_version", self.sg_version.as_ref())?;
        if let Some(generation) = &self.generation {
            state.serialize_field("generation", generation)?;
        }
        state.serialize_field("top_k", &self.top_k)?;
        state.end()
    }
}

struct SearchPlaneSourcegraphQueryRequestVisitor;

impl<'de> Visitor<'de> for SearchPlaneSourcegraphQueryRequestVisitor {
    type Value = SearchPlaneSourcegraphQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SearchPlaneSourcegraphQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_syntax: Option<Box<str>> = None;
        let mut sg_version: Option<Box<str>> = None;
        let mut generation: Option<GenerationPin> = None;
        let mut generation_seen = false;
        let mut top_k: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "source_syntax" => {
                    if source_syntax.is_some() {
                        return Err(de::Error::duplicate_field("source_syntax"));
                    }
                    let raw: String = map.next_value()?;
                    source_syntax = Some(raw.into_boxed_str());
                }
                "sg_version" => {
                    if sg_version.is_some() {
                        return Err(de::Error::duplicate_field("sg_version"));
                    }
                    let raw: String = map.next_value()?;
                    sg_version = Some(raw.into_boxed_str());
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
                        SEARCH_PLANE_SOURCEGRAPH_QUERY_REQUEST_FIELDS,
                    ));
                }
            }
        }
        Ok(SearchPlaneSourcegraphQueryRequest {
            source_syntax: source_syntax
                .ok_or_else(|| de::Error::missing_field("source_syntax"))?,
            sg_version: sg_version.ok_or_else(|| de::Error::missing_field("sg_version"))?,
            generation,
            top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SearchPlaneSourcegraphQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SearchPlaneSourcegraphQueryRequest",
            SEARCH_PLANE_SOURCEGRAPH_QUERY_REQUEST_FIELDS,
            SearchPlaneSourcegraphQueryRequestVisitor,
        )
    }
}

#[cfg(test)]
mod sourcegraph_tests {
    use super::SearchPlaneSourcegraphQueryRequest;
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
    fn sourcegraph_request_cbor_roundtrip() -> TestRes {
        let v = SearchPlaneSourcegraphQueryRequest {
            source_syntax: "repo:^example file:foo \"bar\"".into(),
            sg_version: "5.4.0".into(),
            generation: Some(sample_pin()),
            top_k: 25,
        };
        let bytes = encode(&v)?;
        let back: SearchPlaneSourcegraphQueryRequest = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_request_cbor_roundtrip_no_generation() -> TestRes {
        let v = SearchPlaneSourcegraphQueryRequest {
            source_syntax: "test".into(),
            sg_version: "5.4.0".into(),
            generation: None,
            top_k: 10,
        };
        let bytes = encode(&v)?;
        let back: SearchPlaneSourcegraphQueryRequest = decode(&bytes)?;
        if back != v {
            return Err("roundtrip mismatch".into());
        }
        Ok(())
    }

    #[test]
    fn sourcegraph_request_unknown_field_rejected() -> TestRes {
        let v = SearchPlaneSourcegraphQueryRequest {
            source_syntax: "test".into(),
            sg_version: "5.4.0".into(),
            generation: None,
            top_k: 5,
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
        let decoded: Result<SearchPlaneSourcegraphQueryRequest, _> =
            ciborium::de::from_reader::<SearchPlaneSourcegraphQueryRequest, _>(mutated.as_slice());
        if decoded.is_ok() {
            return Err("unknown field should have been rejected".into());
        }
        Ok(())
    }
}
