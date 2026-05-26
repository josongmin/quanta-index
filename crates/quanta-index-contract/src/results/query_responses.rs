use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    BridgeCandidatePacket, CommitCandidate, DiffCandidate, GenerationPin, LexicalCandidate,
    ManifestGeneration, RepoId, RepoRelativePath, RevisionId, StructuralCandidate,
    lex::{SymbolKindCode, SymbolKindFamily},
};

use super::SearchExplanation;

#[derive(Clone, Debug, PartialEq)]
pub struct TextQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolCandidate {
    pub candidate_id: String,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub repo_relative_path: RepoRelativePath,
    pub start_line: u32,
    pub end_line: u32,
    pub score: f32,
    pub snippet: String,
    pub symbol_kind: SymbolKindCode,
    pub symbol_kind_family: Option<SymbolKindFamily>,
}

const SYMBOL_CANDIDATE_FIELDS: &[&str] = &[
    "candidate_id",
    "repo_id",
    "revision_id",
    "manifest_generation",
    "repo_relative_path",
    "start_line",
    "end_line",
    "score",
    "snippet",
    "symbol_kind",
    "symbol_kind_family",
];

macro_rules! impl_symbol_candidate_serde {
    ($fields:ident, $visitor:ident) => {
        impl Serialize for SymbolCandidate {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut state = serializer.serialize_struct("SymbolCandidate", 11)?;
                state.serialize_field("candidate_id", &self.candidate_id)?;
                state.serialize_field("repo_id", &self.repo_id)?;
                state.serialize_field("revision_id", &self.revision_id)?;
                state.serialize_field("manifest_generation", &self.manifest_generation)?;
                state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
                state.serialize_field("start_line", &self.start_line)?;
                state.serialize_field("end_line", &self.end_line)?;
                state.serialize_field("score", &self.score)?;
                state.serialize_field("snippet", &self.snippet)?;
                state.serialize_field("symbol_kind", &self.symbol_kind)?;
                state.serialize_field("symbol_kind_family", &self.symbol_kind_family)?;
                state.end()
            }
        }

        struct $visitor;

        impl<'de> Visitor<'de> for $visitor {
            type Value = SymbolCandidate;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a SymbolCandidate map")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut candidate_id: Option<String> = None;
                let mut repo_id: Option<RepoId> = None;
                let mut revision_id: Option<RevisionId> = None;
                let mut manifest_generation: Option<ManifestGeneration> = None;
                let mut repo_relative_path: Option<RepoRelativePath> = None;
                let mut start_line: Option<u32> = None;
                let mut end_line: Option<u32> = None;
                let mut score: Option<f32> = None;
                let mut snippet: Option<String> = None;
                let mut symbol_kind: Option<SymbolKindCode> = None;
                let mut symbol_kind_family: Option<Option<SymbolKindFamily>> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "candidate_id" => {
                            if candidate_id.is_some() {
                                return Err(de::Error::duplicate_field("candidate_id"));
                            }
                            candidate_id = Some(map.next_value()?);
                        }
                        "repo_id" => {
                            if repo_id.is_some() {
                                return Err(de::Error::duplicate_field("repo_id"));
                            }
                            repo_id = Some(map.next_value()?);
                        }
                        "revision_id" => {
                            if revision_id.is_some() {
                                return Err(de::Error::duplicate_field("revision_id"));
                            }
                            revision_id = Some(map.next_value()?);
                        }
                        "manifest_generation" => {
                            if manifest_generation.is_some() {
                                return Err(de::Error::duplicate_field("manifest_generation"));
                            }
                            manifest_generation = Some(map.next_value()?);
                        }
                        "repo_relative_path" => {
                            if repo_relative_path.is_some() {
                                return Err(de::Error::duplicate_field("repo_relative_path"));
                            }
                            repo_relative_path = Some(map.next_value()?);
                        }
                        "start_line" => {
                            if start_line.is_some() {
                                return Err(de::Error::duplicate_field("start_line"));
                            }
                            start_line = Some(map.next_value()?);
                        }
                        "end_line" => {
                            if end_line.is_some() {
                                return Err(de::Error::duplicate_field("end_line"));
                            }
                            end_line = Some(map.next_value()?);
                        }
                        "score" => {
                            if score.is_some() {
                                return Err(de::Error::duplicate_field("score"));
                            }
                            score = Some(map.next_value()?);
                        }
                        "snippet" => {
                            if snippet.is_some() {
                                return Err(de::Error::duplicate_field("snippet"));
                            }
                            snippet = Some(map.next_value()?);
                        }
                        "symbol_kind" => {
                            if symbol_kind.is_some() {
                                return Err(de::Error::duplicate_field("symbol_kind"));
                            }
                            symbol_kind = Some(map.next_value()?);
                        }
                        "symbol_kind_family" => {
                            if symbol_kind_family.is_some() {
                                return Err(de::Error::duplicate_field("symbol_kind_family"));
                            }
                            symbol_kind_family = Some(map.next_value()?);
                        }
                        other => return Err(de::Error::unknown_field(other, $fields)),
                    }
                }
                Ok(SymbolCandidate {
                    candidate_id: candidate_id
                        .ok_or_else(|| de::Error::missing_field("candidate_id"))?,
                    repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
                    revision_id: revision_id
                        .ok_or_else(|| de::Error::missing_field("revision_id"))?,
                    manifest_generation: manifest_generation
                        .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
                    repo_relative_path: repo_relative_path
                        .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
                    start_line: start_line.ok_or_else(|| de::Error::missing_field("start_line"))?,
                    end_line: end_line.ok_or_else(|| de::Error::missing_field("end_line"))?,
                    score: score.ok_or_else(|| de::Error::missing_field("score"))?,
                    snippet: snippet.ok_or_else(|| de::Error::missing_field("snippet"))?,
                    symbol_kind: symbol_kind
                        .ok_or_else(|| de::Error::missing_field("symbol_kind"))?,
                    symbol_kind_family: symbol_kind_family
                        .ok_or_else(|| de::Error::missing_field("symbol_kind_family"))?,
                })
            }
        }

        impl<'de> Deserialize<'de> for SymbolCandidate {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_struct("SymbolCandidate", $fields, $visitor)
            }
        }
    };
}

impl_symbol_candidate_serde!(SYMBOL_CANDIDATE_FIELDS, SymbolCandidateVisitor);

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<SymbolCandidate>,
}

const SYMBOL_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results"];

const TEXT_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results"];

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
    pub explanation: SearchExplanation,
}

const SEMANTIC_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "explanation"];

#[derive(Clone, Debug, PartialEq)]
pub struct HybridQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
    pub explanation: SearchExplanation,
}

const HYBRID_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "explanation"];

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneHistoryQueryResponse {
    pub generation: GenerationPin,
    pub commits: Vec<CommitCandidate>,
    pub diffs: Vec<DiffCandidate>,
}

const SEARCH_PLANE_HISTORY_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "commits", "diffs"];

macro_rules! impl_generation_results_response_serde {
    ($ty:ident, $fields:ident, $visitor:ident, $result_ty:ty) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut state = serializer.serialize_struct(stringify!($ty), 2)?;
                state.serialize_field("generation", &self.generation)?;
                state.serialize_field("results", &self.results)?;
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
                let mut generation: Option<GenerationPin> = None;
                let mut results: Option<Vec<$result_ty>> = None;
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
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                Ok($ty {
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                    results: results.ok_or_else(|| de::Error::missing_field("results"))?,
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

macro_rules! impl_generation_results_explanation_response_serde {
    ($ty:ident, $fields:ident, $visitor:ident, $result_ty:ty) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut state = serializer.serialize_struct(stringify!($ty), 3)?;
                state.serialize_field("generation", &self.generation)?;
                state.serialize_field("results", &self.results)?;
                state.serialize_field("explanation", &self.explanation)?;
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
                let mut generation: Option<GenerationPin> = None;
                let mut results: Option<Vec<$result_ty>> = None;
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
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                Ok($ty {
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                    results: results.ok_or_else(|| de::Error::missing_field("results"))?,
                    explanation: explanation
                        .ok_or_else(|| de::Error::missing_field("explanation"))?,
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

macro_rules! impl_generation_payload_response_serde {
    (
        $ty:ident,
        $fields:ident,
        $visitor:ident,
        $payload_field:ident : $payload_ty:ty => $payload_name:literal
    ) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut state = serializer.serialize_struct(stringify!($ty), 2)?;
                state.serialize_field("generation", &self.generation)?;
                state.serialize_field($payload_name, &self.$payload_field)?;
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
                let mut generation: Option<GenerationPin> = None;
                let mut $payload_field: Option<$payload_ty> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "generation" => {
                            if generation.is_some() {
                                return Err(de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                        $payload_name => {
                            if $payload_field.is_some() {
                                return Err(de::Error::duplicate_field($payload_name));
                            }
                            $payload_field = Some(map.next_value()?);
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                Ok($ty {
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                    $payload_field: $payload_field
                        .ok_or_else(|| de::Error::missing_field($payload_name))?,
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

macro_rules! impl_generation_two_payload_response_serde {
    (
        $ty:ident,
        $fields:ident,
        $visitor:ident,
        $first_field:ident : $first_ty:ty => $first_name:literal,
        $second_field:ident : $second_ty:ty => $second_name:literal
    ) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                let mut state = serializer.serialize_struct(stringify!($ty), 3)?;
                state.serialize_field("generation", &self.generation)?;
                state.serialize_field($first_name, &self.$first_field)?;
                state.serialize_field($second_name, &self.$second_field)?;
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
                let mut generation: Option<GenerationPin> = None;
                let mut $first_field: Option<$first_ty> = None;
                let mut $second_field: Option<$second_ty> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "generation" => {
                            if generation.is_some() {
                                return Err(de::Error::duplicate_field("generation"));
                            }
                            generation = Some(map.next_value()?);
                        }
                        $first_name => {
                            if $first_field.is_some() {
                                return Err(de::Error::duplicate_field($first_name));
                            }
                            $first_field = Some(map.next_value()?);
                        }
                        $second_name => {
                            if $second_field.is_some() {
                                return Err(de::Error::duplicate_field($second_name));
                            }
                            $second_field = Some(map.next_value()?);
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                Ok($ty {
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                    $first_field: $first_field
                        .ok_or_else(|| de::Error::missing_field($first_name))?,
                    $second_field: $second_field
                        .ok_or_else(|| de::Error::missing_field($second_name))?,
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

impl_generation_results_response_serde!(
    TextQueryResponse,
    TEXT_QUERY_RESPONSE_FIELDS,
    TextQueryResponseVisitor,
    LexicalCandidate
);

impl_generation_results_response_serde!(
    SymbolQueryResponse,
    SYMBOL_QUERY_RESPONSE_FIELDS,
    SymbolQueryResponseVisitor,
    SymbolCandidate
);

impl_generation_results_explanation_response_serde!(
    SemanticQueryResponse,
    SEMANTIC_QUERY_RESPONSE_FIELDS,
    SemanticQueryResponseVisitor,
    LexicalCandidate
);

impl_generation_results_explanation_response_serde!(
    HybridQueryResponse,
    HYBRID_QUERY_RESPONSE_FIELDS,
    HybridQueryResponseVisitor,
    LexicalCandidate
);

impl_generation_two_payload_response_serde!(
    SearchPlaneHistoryQueryResponse,
    SEARCH_PLANE_HISTORY_QUERY_RESPONSE_FIELDS,
    SearchPlaneHistoryQueryResponseVisitor,
    commits: Vec<CommitCandidate> => "commits",
    diffs: Vec<DiffCandidate> => "diffs"
);

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneRuntimeMetadataQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
}

const SEARCH_PLANE_RUNTIME_METADATA_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results"];
impl_generation_results_response_serde!(
    SearchPlaneRuntimeMetadataQueryResponse,
    SEARCH_PLANE_RUNTIME_METADATA_QUERY_RESPONSE_FIELDS,
    SearchPlaneRuntimeMetadataQueryResponseVisitor,
    LexicalCandidate
);

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneStructuralQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<StructuralCandidate>,
}

const SEARCH_PLANE_STRUCTURAL_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results"];
impl_generation_results_response_serde!(
    SearchPlaneStructuralQueryResponse,
    SEARCH_PLANE_STRUCTURAL_QUERY_RESPONSE_FIELDS,
    SearchPlaneStructuralQueryResponseVisitor,
    StructuralCandidate
);

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneBridgeQueryResponse {
    pub generation: GenerationPin,
    pub packet: BridgeCandidatePacket,
}

const SEARCH_PLANE_BRIDGE_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "packet"];
impl_generation_payload_response_serde!(
    SearchPlaneBridgeQueryResponse,
    SEARCH_PLANE_BRIDGE_QUERY_RESPONSE_FIELDS,
    SearchPlaneBridgeQueryResponseVisitor,
    packet: BridgeCandidatePacket => "packet"
);

#[derive(Clone, Debug, PartialEq)]
pub struct SearchPlaneExplainQueryResponse {
    pub generation: GenerationPin,
    pub explanation: SearchExplanation,
}

const SEARCH_PLANE_EXPLAIN_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "explanation"];
impl_generation_payload_response_serde!(
    SearchPlaneExplainQueryResponse,
    SEARCH_PLANE_EXPLAIN_QUERY_RESPONSE_FIELDS,
    SearchPlaneExplainQueryResponseVisitor,
    explanation: SearchExplanation => "explanation"
);
