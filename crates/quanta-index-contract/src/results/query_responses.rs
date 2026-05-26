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

struct SymbolCandidateVisitor;

impl<'de> Visitor<'de> for SymbolCandidateVisitor {
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
                other => return Err(de::Error::unknown_field(other, SYMBOL_CANDIDATE_FIELDS)),
            }
        }
        Ok(SymbolCandidate {
            candidate_id: candidate_id.ok_or_else(|| de::Error::missing_field("candidate_id"))?,
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            manifest_generation: manifest_generation
                .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            start_line: start_line.ok_or_else(|| de::Error::missing_field("start_line"))?,
            end_line: end_line.ok_or_else(|| de::Error::missing_field("end_line"))?,
            score: score.ok_or_else(|| de::Error::missing_field("score"))?,
            snippet: snippet.ok_or_else(|| de::Error::missing_field("snippet"))?,
            symbol_kind: symbol_kind.ok_or_else(|| de::Error::missing_field("symbol_kind"))?,
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
        deserializer.deserialize_struct(
            "SymbolCandidate",
            SYMBOL_CANDIDATE_FIELDS,
            SymbolCandidateVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SymbolQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<SymbolCandidate>,
}

const SYMBOL_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results"];

impl Serialize for SymbolQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SymbolQueryResponse", 2)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("results", &self.results)?;
        state.end()
    }
}

struct SymbolQueryResponseVisitor;

impl<'de> Visitor<'de> for SymbolQueryResponseVisitor {
    type Value = SymbolQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SymbolQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut results: Option<Vec<SymbolCandidate>> = None;
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
                        SYMBOL_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        Ok(SymbolQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            results: results.ok_or_else(|| de::Error::missing_field("results"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SymbolQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SymbolQueryResponse",
            SYMBOL_QUERY_RESPONSE_FIELDS,
            SymbolQueryResponseVisitor,
        )
    }
}

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
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "commits" => {
                    if commits.is_some() {
                        return Err(de::Error::duplicate_field("commits"));
                    }
                    commits = Some(map.next_value()?);
                }
                "diffs" => {
                    if diffs.is_some() {
                        return Err(de::Error::duplicate_field("diffs"));
                    }
                    diffs = Some(map.next_value()?);
                }
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
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "packet" => {
                    if packet.is_some() {
                        return Err(de::Error::duplicate_field("packet"));
                    }
                    packet = Some(map.next_value()?);
                }
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
