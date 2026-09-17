use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::SemanticCorpusKindV1;
use crate::{HybridCandidateV1, LexicalCandidate};

use super::{
    GenerationPin, GenerationSelector, HistoryCursor, QueryConstraintSetV1, TextQueryRequest,
    TextQuerySyntax,
};

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
    /// Applied identically to the semantic lane and any lexical scope lane.
    pub constraints: QueryConstraintSetV1,
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
    "constraints",
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
                let mut field_count: usize = 3;
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
                state.serialize_field("constraints", &self.constraints)?;
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
                let mut constraints: Option<QueryConstraintSetV1> = None;
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
                        "constraints" => {
                            if constraints.is_some() {
                                return Err(de::Error::duplicate_field("constraints"));
                            }
                            constraints = Some(map.next_value()?);
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
                    constraints: constraints
                        .ok_or_else(|| de::Error::missing_field("constraints"))?,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SemanticSeedCorpusBudgetV1 {
    pub corpus_kind: SemanticCorpusKindV1,
    pub top_k: u32,
}

const SEMANTIC_SEED_CORPUS_BUDGET_V1_FIELDS: &[&str] = &["corpus_kind", "top_k"];

impl Serialize for SemanticSeedCorpusBudgetV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SemanticSeedCorpusBudgetV1", 2)?;
        state.serialize_field("corpus_kind", &self.corpus_kind)?;
        state.serialize_field("top_k", &self.top_k)?;
        state.end()
    }
}

struct SemanticSeedCorpusBudgetV1Visitor;

impl<'de> Visitor<'de> for SemanticSeedCorpusBudgetV1Visitor {
    type Value = SemanticSeedCorpusBudgetV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SemanticSeedCorpusBudgetV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut corpus_kind = None;
        let mut top_k = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "corpus_kind" => {
                    if corpus_kind.is_some() {
                        return Err(de::Error::duplicate_field("corpus_kind"));
                    }
                    corpus_kind = Some(map.next_value()?);
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
                        SEMANTIC_SEED_CORPUS_BUDGET_V1_FIELDS,
                    ));
                }
            }
        }
        Ok(SemanticSeedCorpusBudgetV1 {
            corpus_kind: corpus_kind.ok_or_else(|| de::Error::missing_field("corpus_kind"))?,
            top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SemanticSeedCorpusBudgetV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SemanticSeedCorpusBudgetV1",
            SEMANTIC_SEED_CORPUS_BUDGET_V1_FIELDS,
            SemanticSeedCorpusBudgetV1Visitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HybridSeedQueryRequest {
    pub text_query: TextQueryRequest,
    pub semantic_query_text: String,
    pub generation: Option<GenerationPin>,
    pub generation_selector: Option<GenerationSelector>,
    /// Independent storage-prefiltered dense lanes. Empty retains the legacy
    /// global dense lane during the SCV2 migration window.
    pub dense_corpora: Vec<SemanticSeedCorpusBudgetV1>,
    pub top_k: u32,
}

const HYBRID_SEED_QUERY_REQUEST_FIELDS: &[&str] = &[
    "text_query",
    "semantic_query_text",
    "generation",
    "generation_selector",
    "dense_corpora",
    "top_k",
];

macro_rules! impl_hybrid_seed_query_request_serde {
    ($fields:ident, $visitor:ident) => {
        impl Serialize for HybridSeedQueryRequest {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut field_count: usize = 4;
                if self.generation.is_some() {
                    field_count = field_count.saturating_add(1);
                }
                if self.generation_selector.is_some() {
                    field_count = field_count.saturating_add(1);
                }
                let mut state =
                    serializer.serialize_struct("HybridSeedQueryRequest", field_count)?;
                state.serialize_field("text_query", &self.text_query)?;
                state.serialize_field("semantic_query_text", &self.semantic_query_text)?;
                if let Some(generation) = &self.generation {
                    state.serialize_field("generation", generation)?;
                }
                if let Some(generation_selector) = &self.generation_selector {
                    state.serialize_field("generation_selector", generation_selector)?;
                }
                state.serialize_field("dense_corpora", &self.dense_corpora)?;
                state.serialize_field("top_k", &self.top_k)?;
                state.end()
            }
        }

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
            type Value = HybridSeedQueryRequest;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a HybridSeedQueryRequest map")
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
                let mut dense_corpora: Option<Vec<SemanticSeedCorpusBudgetV1>> = None;
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
                        "dense_corpora" => {
                            if dense_corpora.is_some() {
                                return Err(de::Error::duplicate_field("dense_corpora"));
                            }
                            dense_corpora = Some(map.next_value()?);
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
                Ok(HybridSeedQueryRequest {
                    text_query: text_query.ok_or_else(|| de::Error::missing_field("text_query"))?,
                    semantic_query_text: semantic_query_text
                        .ok_or_else(|| de::Error::missing_field("semantic_query_text"))?,
                    generation,
                    generation_selector,
                    dense_corpora: dense_corpora.unwrap_or_default(),
                    top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
                })
            }
        }

        impl<'de> Deserialize<'de> for HybridSeedQueryRequest {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_struct("HybridSeedQueryRequest", $fields, $visitor)
            }
        }
    };
}

impl_hybrid_seed_query_request_serde!(
    HYBRID_SEED_QUERY_REQUEST_FIELDS,
    HybridSeedQueryRequestVisitor
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolQueryRequest {
    pub syntax: TextQuerySyntax,
    pub query_text: String,
    pub constraints: QueryConstraintSetV1,
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
    "constraints",
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
                let mut field_count: usize = 4;
                if self.generation.is_some() {
                    field_count = field_count.saturating_add(1);
                }
                if self.generation_selector.is_some() {
                    field_count = field_count.saturating_add(1);
                }
                let mut state = serializer.serialize_struct("SymbolQueryRequest", field_count)?;
                state.serialize_field("syntax", &self.syntax)?;
                state.serialize_field("query_text", &self.query_text)?;
                state.serialize_field("constraints", &self.constraints)?;
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
                let mut constraints: Option<QueryConstraintSetV1> = None;
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
                        "constraints" => {
                            if constraints.is_some() {
                                return Err(de::Error::duplicate_field("constraints"));
                            }
                            constraints = Some(map.next_value()?);
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
                    constraints: constraints
                        .ok_or_else(|| de::Error::missing_field("constraints"))?,
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

/// A history query: the text query and, for every page after the first,
/// the cursor the previous page returned (QI-BB-023).
///
/// Results are ordered by recency under the total order documented on
/// [`HistoryCursor`]; `top_k` bounds one page. A continuation is served
/// from the history authority epoch its cursor names (QI-BB-020 W2), so
/// the pages of one walk partition one snapshot; a cursor whose epoch the
/// plane no longer retains is refused `AUX_EPOCH_EXPIRED`, one naming an
/// epoch the plane never produced `AUX_EPOCH_UNKNOWN`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryQueryRequest {
    pub text_query: TextQueryRequest,
    pub cursor: Option<HistoryCursor>,
}

const HISTORY_QUERY_REQUEST_FIELDS: &[&str] = &["text_query", "cursor"];

impl Serialize for HistoryQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = if self.cursor.is_some() { 2 } else { 1 };
        let mut state = serializer.serialize_struct("HistoryQueryRequest", field_count)?;
        state.serialize_field("text_query", &self.text_query)?;
        if let Some(cursor) = &self.cursor {
            state.serialize_field("cursor", cursor)?;
        }
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
        let mut cursor: Option<HistoryCursor> = None;
        let mut cursor_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "text_query" => {
                    if text_query.is_some() {
                        return Err(de::Error::duplicate_field("text_query"));
                    }
                    text_query = Some(map.next_value()?);
                }
                "cursor" => {
                    if cursor_seen {
                        return Err(de::Error::duplicate_field("cursor"));
                    }
                    cursor_seen = true;
                    cursor = map.next_value()?;
                }
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
            cursor,
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

/// The candidate an explain names: the row as the route that ranked it
/// carried it (QI-BB-022).
///
/// A lexical, semantic or symbol-projected row is a [`LexicalCandidate`]
/// whose carried `score` is the score one plan emitted. A hybrid row is a
/// [`HybridCandidateV1`], whose provenance — the RRF score and the per-lane
/// ranks and raw scores — the explain reconciles lane by lane; it explains
/// only under the query it was fused for, so it requires `text_query`.
///
/// Encoded adjacently tagged, `kind` before `payload`.
#[derive(Clone, Debug, PartialEq)]
pub enum ExplainCandidateV1 {
    Lexical(LexicalCandidate),
    Hybrid(HybridCandidateV1),
}

impl ExplainCandidateV1 {
    /// The lane row the explain traces: the candidate itself, or the row
    /// the hybrid fusion carried for it.
    #[must_use]
    pub const fn lexical_row(&self) -> &LexicalCandidate {
        match self {
            Self::Lexical(candidate) => candidate,
            Self::Hybrid(hybrid) => &hybrid.candidate,
        }
    }
}

impl From<LexicalCandidate> for ExplainCandidateV1 {
    fn from(candidate: LexicalCandidate) -> Self {
        Self::Lexical(candidate)
    }
}

impl From<HybridCandidateV1> for ExplainCandidateV1 {
    fn from(candidate: HybridCandidateV1) -> Self {
        Self::Hybrid(candidate)
    }
}

const EXPLAIN_CANDIDATE_V1_VARIANTS: &[&str] = &["Lexical", "Hybrid"];
const EXPLAIN_CANDIDATE_V1_FIELDS: &[&str] = &["kind", "payload"];

impl Serialize for ExplainCandidateV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ExplainCandidateV1", 2)?;
        match self {
            Self::Lexical(candidate) => {
                state.serialize_field("kind", "Lexical")?;
                state.serialize_field("payload", candidate)?;
            }
            Self::Hybrid(candidate) => {
                state.serialize_field("kind", "Hybrid")?;
                state.serialize_field("payload", candidate)?;
            }
        }
        state.end()
    }
}

struct ExplainCandidateV1Visitor;

impl<'de> Visitor<'de> for ExplainCandidateV1Visitor {
    type Value = ExplainCandidateV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an ExplainCandidateV1 map with kind before payload")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind: Option<String> = None;
        let mut payload: Option<ExplainCandidateV1> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => {
                    if kind.is_some() {
                        return Err(de::Error::duplicate_field("kind"));
                    }
                    kind = Some(map.next_value()?);
                }
                "payload" => {
                    if payload.is_some() {
                        return Err(de::Error::duplicate_field("payload"));
                    }
                    let decoded = match kind.as_deref() {
                        Some("Lexical") => ExplainCandidateV1::Lexical(map.next_value()?),
                        Some("Hybrid") => ExplainCandidateV1::Hybrid(map.next_value()?),
                        Some(other) => {
                            return Err(de::Error::unknown_variant(
                                other,
                                EXPLAIN_CANDIDATE_V1_VARIANTS,
                            ));
                        }
                        None => {
                            return Err(de::Error::custom(
                                "ExplainCandidateV1 payload arrived before kind; canonical adjacent-tag order is required",
                            ));
                        }
                    };
                    payload = Some(decoded);
                }
                other => {
                    return Err(de::Error::unknown_field(other, EXPLAIN_CANDIDATE_V1_FIELDS));
                }
            }
        }
        let kind = kind.ok_or_else(|| de::Error::missing_field("kind"))?;
        match payload {
            Some(candidate) => Ok(candidate),
            None if EXPLAIN_CANDIDATE_V1_VARIANTS.contains(&kind.as_str()) => {
                Err(de::Error::missing_field("payload"))
            }
            None => Err(de::Error::unknown_variant(
                kind.as_str(),
                EXPLAIN_CANDIDATE_V1_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for ExplainCandidateV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "ExplainCandidateV1",
            EXPLAIN_CANDIDATE_V1_FIELDS,
            ExplainCandidateV1Visitor,
        )
    }
}

/// Explain one candidate at one generation (QI-BB-022).
///
/// Without `text_query` the answer is presence only: an exact lookup of the
/// candidate id. With it, the plane lowers the same plan the search ran and
/// traces the score the lexical engine emits for exactly this candidate
/// under it; for a hybrid candidate it also reconciles the carried lane
/// provenance and the RRF score. The query's own `generation` must be
/// absent or equal to `generation`, and it must carry no selector: the
/// explain names its generation once. A hybrid candidate requires
/// `text_query` (checked on encode and decode).
#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneExplainQueryRequest {
    pub generation: GenerationPin,
    pub candidate: ExplainCandidateV1,
    pub text_query: Option<TextQueryRequest>,
}

const SEARCH_PLANE_EXPLAIN_QUERY_REQUEST_FIELDS: &[&str] =
    &["generation", "candidate", "text_query"];

/// Why an explain request is not one the plane can answer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExplainRequestPolicyErrorV1 {
    HybridCandidateWithoutQuery,
}

impl fmt::Display for ExplainRequestPolicyErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HybridCandidateWithoutQuery => formatter.write_str(
                "a hybrid candidate is explained under the query it was fused for; text_query is required",
            ),
        }
    }
}

impl std::error::Error for ExplainRequestPolicyErrorV1 {}

impl SearchPlaneExplainQueryRequest {
    /// Check the request invariants documented on the type.
    pub const fn validate_v1(&self) -> Result<(), ExplainRequestPolicyErrorV1> {
        match (&self.candidate, &self.text_query) {
            (ExplainCandidateV1::Hybrid(_), None) => {
                Err(ExplainRequestPolicyErrorV1::HybridCandidateWithoutQuery)
            }
            (ExplainCandidateV1::Lexical(_), _) | (ExplainCandidateV1::Hybrid(_), Some(_)) => {
                Ok(())
            }
        }
    }
}

impl Serialize for SearchPlaneExplainQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        let field_count = if self.text_query.is_some() { 3 } else { 2 };
        let mut state =
            serializer.serialize_struct("SearchPlaneExplainQueryRequest", field_count)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("candidate", &self.candidate)?;
        if let Some(text_query) = &self.text_query {
            state.serialize_field("text_query", text_query)?;
        }
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
        let mut candidate: Option<ExplainCandidateV1> = None;
        let mut text_query: Option<TextQueryRequest> = None;
        let mut text_query_seen = false;
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
                "text_query" => {
                    if text_query_seen {
                        return Err(de::Error::duplicate_field("text_query"));
                    }
                    text_query_seen = true;
                    text_query = map.next_value()?;
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        SEARCH_PLANE_EXPLAIN_QUERY_REQUEST_FIELDS,
                    ));
                }
            }
        }
        let request = SearchPlaneExplainQueryRequest {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            candidate: candidate.ok_or_else(|| de::Error::missing_field("candidate"))?,
            text_query,
        };
        request.validate_v1().map_err(de::Error::custom)?;
        Ok(request)
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
