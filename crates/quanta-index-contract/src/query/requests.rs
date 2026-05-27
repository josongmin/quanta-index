use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::LexicalCandidate;

use super::{GenerationPin, GenerationSelector, TextQueryRequest, TextQuerySyntax};

/// Semantic query request (LXE-01 §3: lexical scope unified on
/// [`TextQueryRequest`]).
///
/// `lexical_scope` carries the optional lexical pre-filter used to restrict
/// the semantic recall set. It is the canonical `TextQueryRequest` carrier —
/// the prior `SemanticCandidateScope` mirror has been deleted. Callers that
/// previously passed a scope must construct a `TextQueryRequest` with the
/// desired `syntax`, `query_text`, `generation`/`generation_selector`, and
/// `top_k` (the candidate cap for the lexical leg).
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticQueryRequest {
    pub query_text: String,
    pub generation: Option<GenerationPin>,
    pub generation_selector: Option<GenerationSelector>,
    /// LXE-01 §3: lexical pre-filter for the semantic recall set. Replaces
    /// the deleted `SemanticCandidateScope` dual surface. When `Some`, the
    /// search-plane uses the `TextQueryRequest` to compute lexical
    /// candidates that bound the semantic search.
    pub lexical_scope: Option<TextQueryRequest>,
    pub top_k: u32,
}

const SEMANTIC_QUERY_REQUEST_FIELDS: &[&str] = &[
    "query_text",
    "generation",
    "generation_selector",
    "lexical_scope",
    "top_k",
];

macro_rules! impl_semantic_query_request_serde {
    ($fields:ident, $visitor:ident) => {
        impl Serialize for SemanticQueryRequest {
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
                if self.lexical_scope.is_some() {
                    field_count = field_count.saturating_add(1);
                }
                let mut state = serializer.serialize_struct("SemanticQueryRequest", field_count)?;
                state.serialize_field("query_text", &self.query_text)?;
                if let Some(generation) = &self.generation {
                    state.serialize_field("generation", generation)?;
                }
                if let Some(generation_selector) = &self.generation_selector {
                    state.serialize_field("generation_selector", generation_selector)?;
                }
                if let Some(lexical_scope) = &self.lexical_scope {
                    state.serialize_field("lexical_scope", lexical_scope)?;
                }
                state.serialize_field("top_k", &self.top_k)?;
                state.end()
            }
        }

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
            type Value = SemanticQueryRequest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a SemanticQueryRequest map")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut query_text: Option<String> = None;
                let mut query_text_seen = false;
                let mut generation: Option<GenerationPin> = None;
                let mut generation_seen = false;
                let mut generation_selector: Option<GenerationSelector> = None;
                let mut generation_selector_seen = false;
                let mut lexical_scope: Option<TextQueryRequest> = None;
                let mut lexical_scope_seen = false;
                let mut top_k: Option<u32> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "query_text" => {
                            if query_text_seen {
                                return Err(de::Error::duplicate_field("query_text"));
                            }
                            query_text_seen = true;
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
                        "lexical_scope" => {
                            if lexical_scope_seen {
                                return Err(de::Error::duplicate_field("lexical_scope"));
                            }
                            lexical_scope_seen = true;
                            lexical_scope = Some(map.next_value()?);
                        }
                        "top_k" => {
                            if top_k.is_some() {
                                return Err(de::Error::duplicate_field("top_k"));
                            }
                            top_k = Some(map.next_value()?);
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                Ok(SemanticQueryRequest {
                    query_text: query_text.ok_or_else(|| de::Error::missing_field("query_text"))?,
                    generation,
                    generation_selector,
                    lexical_scope,
                    top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
                })
            }
        }

        impl<'de> Deserialize<'de> for SemanticQueryRequest {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_struct("SemanticQueryRequest", $fields, $visitor)
            }
        }
    };
}

impl_semantic_query_request_serde!(SEMANTIC_QUERY_REQUEST_FIELDS, SemanticQueryRequestVisitor);

#[derive(Clone, Debug, PartialEq)]
pub struct HybridQueryRequest {
    pub text_query: TextQueryRequest,
    pub semantic_query_text: String,
    pub generation: Option<GenerationPin>,
    pub generation_selector: Option<GenerationSelector>,
    pub top_k: u32,
}

const HYBRID_QUERY_REQUEST_FIELDS: &[&str] = &[
    "text_query",
    "semantic_query_text",
    "generation",
    "generation_selector",
    "top_k",
];

macro_rules! impl_hybrid_query_request_serde {
    ($fields:ident, $visitor:ident) => {
        impl Serialize for HybridQueryRequest {
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
                let mut state = serializer.serialize_struct("HybridQueryRequest", field_count)?;
                state.serialize_field("text_query", &self.text_query)?;
                state.serialize_field("semantic_query_text", &self.semantic_query_text)?;
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

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
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
                let mut semantic_query_text_seen = false;
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
                            if semantic_query_text_seen {
                                return Err(de::Error::duplicate_field("semantic_query_text"));
                            }
                            semantic_query_text_seen = true;
                            semantic_query_text = Some(map.next_value()?);
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
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                Ok(HybridQueryRequest {
                    text_query: text_query.ok_or_else(|| de::Error::missing_field("text_query"))?,
                    semantic_query_text: semantic_query_text
                        .ok_or_else(|| de::Error::missing_field("semantic_query_text"))?,
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
                deserializer.deserialize_struct("HybridQueryRequest", $fields, $visitor)
            }
        }
    };
}

impl_hybrid_query_request_serde!(HYBRID_QUERY_REQUEST_FIELDS, HybridQueryRequestVisitor);

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

macro_rules! impl_symbol_query_request_serde {
    ($fields:ident, $visitor:ident) => {
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

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
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
                            return Err(de::Error::unknown_field(other, $fields));
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
                deserializer.deserialize_struct("SymbolQueryRequest", $fields, $visitor)
            }
        }
    };
}

impl_symbol_query_request_serde!(SYMBOL_QUERY_REQUEST_FIELDS, SymbolQueryRequestVisitor);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryQueryRequest {
    pub text_query: TextQueryRequest,
}

const HISTORY_QUERY_REQUEST_FIELDS: &[&str] = &["text_query"];

macro_rules! impl_text_query_wrapper_serde {
    ($ty:ident, $fields:ident, $visitor:ident) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut state = serializer.serialize_struct(stringify!($ty), 1)?;
                state.serialize_field("text_query", &self.text_query)?;
                state.end()
            }
        }

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
            type Value = $ty;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!("a ", stringify!($ty), " map"))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut text_query: Option<TextQueryRequest> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "text_query" => {
                            if text_query.is_some() {
                                return Err(de::Error::duplicate_field("text_query"));
                            }
                            text_query = Some(map.next_value()?);
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                Ok($ty {
                    text_query: text_query.ok_or_else(|| de::Error::missing_field("text_query"))?,
                })
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_struct(stringify!($ty), $fields, $visitor)
            }
        }
    };
}

macro_rules! impl_two_required_field_serde {
    (
        $ty:ident,
        $fields:ident,
        $visitor:ident,
        $field1:ident : $field1_ty:ty => $field1_name:literal,
        $field2:ident : $field2_ty:ty => $field2_name:literal
    ) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut state = serializer.serialize_struct(stringify!($ty), 2)?;
                state.serialize_field($field1_name, &self.$field1)?;
                state.serialize_field($field2_name, &self.$field2)?;
                state.end()
            }
        }

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
            type Value = $ty;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!("a ", stringify!($ty), " map"))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut $field1: Option<$field1_ty> = None;
                let mut $field2: Option<$field2_ty> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        $field1_name => {
                            if $field1.is_some() {
                                return Err(de::Error::duplicate_field($field1_name));
                            }
                            $field1 = Some(map.next_value()?);
                        }
                        $field2_name => {
                            if $field2.is_some() {
                                return Err(de::Error::duplicate_field($field2_name));
                            }
                            $field2 = Some(map.next_value()?);
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                Ok($ty {
                    $field1: $field1.ok_or_else(|| de::Error::missing_field($field1_name))?,
                    $field2: $field2.ok_or_else(|| de::Error::missing_field($field2_name))?,
                })
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_struct(stringify!($ty), $fields, $visitor)
            }
        }
    };
}
impl_text_query_wrapper_serde!(
    HistoryQueryRequest,
    HISTORY_QUERY_REQUEST_FIELDS,
    HistoryQueryRequestVisitor
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeMetadataQueryRequest {
    pub text_query: TextQueryRequest,
}

const RUNTIME_METADATA_QUERY_REQUEST_FIELDS: &[&str] = &["text_query"];
impl_text_query_wrapper_serde!(
    RuntimeMetadataQueryRequest,
    RUNTIME_METADATA_QUERY_REQUEST_FIELDS,
    RuntimeMetadataQueryRequestVisitor
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralQueryRequest {
    pub text_query: TextQueryRequest,
}

const STRUCTURAL_QUERY_REQUEST_FIELDS: &[&str] = &["text_query"];
impl_text_query_wrapper_serde!(
    StructuralQueryRequest,
    STRUCTURAL_QUERY_REQUEST_FIELDS,
    StructuralQueryRequestVisitor
);

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneExplainQueryRequest {
    pub generation: GenerationPin,
    pub candidate: LexicalCandidate,
}

const SEARCH_PLANE_EXPLAIN_QUERY_REQUEST_FIELDS: &[&str] = &["generation", "candidate"];
impl_two_required_field_serde!(
    SearchPlaneExplainQueryRequest,
    SEARCH_PLANE_EXPLAIN_QUERY_REQUEST_FIELDS,
    SearchPlaneExplainQueryRequestVisitor,
    generation: GenerationPin => "generation",
    candidate: LexicalCandidate => "candidate"
);
