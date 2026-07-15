use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    CommitCandidate, DiffCandidate, GenerationPin, LexicalCandidate, ManifestGeneration,
    OwnerDocKind, QueryResultWindowV1, RepoId, RepoRelativePath, RevisionId, SemanticCorpusKindV1,
    StructuralCandidate,
    lex::{SymbolKindCode, SymbolKindFamily},
};

use super::SearchExplanation;

#[derive(Clone, Debug, PartialEq)]
pub struct TextQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
    pub window: QueryResultWindowV1,
    pub file_owner_rows: Option<Vec<FileOwnerProjectionRow>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FileOwnerProjectionRow {
    pub candidate_id: String,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub repo_relative_path: RepoRelativePath,
    pub owners: Vec<String>,
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
    pub window: QueryResultWindowV1,
}

const SYMBOL_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "window"];

const TEXT_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "window", "file_owner_rows"];
const FILE_OWNER_PROJECTION_ROW_FIELDS: &[&str] = &[
    "candidate_id",
    "repo_id",
    "revision_id",
    "manifest_generation",
    "repo_relative_path",
    "owners",
];

#[derive(Clone, Debug, PartialEq)]
pub struct SemanticQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
    pub window: QueryResultWindowV1,
    pub explanation: SearchExplanation,
}

const SEMANTIC_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "window", "explanation"];

#[derive(Clone, Debug, PartialEq)]
pub struct HybridQueryResponse {
    pub generation: GenerationPin,
    pub results: Vec<LexicalCandidate>,
    pub window: QueryResultWindowV1,
    pub explanation: SearchExplanation,
}

const HYBRID_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "results", "window", "explanation"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HybridSeedLane {
    Lexical,
    Semantic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedLaneV2 {
    Exact,
    Bm25,
    Dense,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeedContributionV2 {
    pub lane: SeedLaneV2,
    pub rank: u32,
    pub raw_score: Option<f32>,
    pub corpus_kind: Option<SemanticCorpusKindV1>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeedCandidateV2 {
    pub record_id: String,
    pub entity_id: String,
    pub owner_kind: OwnerDocKind,
    pub corpus_kind: Option<SemanticCorpusKindV1>,
    pub repo_relative_path: RepoRelativePath,
    pub snippet: String,
    pub seed_rank: u32,
    pub contributions: Vec<SeedContributionV2>,
    pub degraded_reasons: Vec<String>,
}

/// Canonical identity for ranking and deduplicating [`SeedCandidateV2`] values.
///
/// `entity_id` is opaque outside its owner domain. Including `owner_kind` in
/// the ordered key prevents unrelated records with equal display text from
/// collapsing during RRF fusion.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SeedFusionIdentityV2 {
    owner_kind: OwnerDocKind,
    entity_id: String,
}

impl SeedFusionIdentityV2 {
    #[must_use]
    pub const fn new(owner_kind: OwnerDocKind, entity_id: String) -> Self {
        Self {
            owner_kind,
            entity_id,
        }
    }

    #[must_use]
    pub const fn owner_kind(&self) -> OwnerDocKind {
        self.owner_kind
    }

    #[must_use]
    pub fn entity_id(&self) -> &str {
        &self.entity_id
    }

    /// Compare borrowed identity parts without allocating temporary keys.
    ///
    /// The order is exactly the derived [`Ord`] order of this type:
    /// `owner_kind` first, followed by the opaque `entity_id`.
    #[must_use]
    pub fn cmp_parts(
        left_owner_kind: OwnerDocKind,
        left_entity_id: &str,
        right_owner_kind: OwnerDocKind,
        right_entity_id: &str,
    ) -> core::cmp::Ordering {
        left_owner_kind
            .cmp(&right_owner_kind)
            .then_with(|| left_entity_id.cmp(right_entity_id))
    }
}

impl From<&SeedCandidateV2> for SeedFusionIdentityV2 {
    fn from(candidate: &SeedCandidateV2) -> Self {
        Self::new(candidate.owner_kind, candidate.entity_id.clone())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HybridSeedCandidate {
    pub candidate: LexicalCandidate,
    pub seed_rank: u32,
    pub lexical_rank: Option<u32>,
    pub lexical_score_raw: Option<f32>,
    pub semantic_rank: Option<u32>,
    pub semantic_score_raw: Option<f32>,
    pub source_lanes: Vec<HybridSeedLane>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HybridSeedQueryResponse {
    pub generation: GenerationPin,
    pub manifest_digest: String,
    pub seed_candidates: Vec<HybridSeedCandidate>,
    pub seed_candidates_v2: Option<Vec<SeedCandidateV2>>,
    pub window: QueryResultWindowV1,
    pub explanation: SearchExplanation,
}

const HYBRID_SEED_QUERY_RESPONSE_FIELDS: &[&str] = &[
    "generation",
    "manifest_digest",
    "seed_candidates",
    "seed_candidates_v2",
    "window",
    "explanation",
];
const HYBRID_SEED_CANDIDATE_FIELDS: &[&str] = &[
    "candidate",
    "seed_rank",
    "lexical_rank",
    "lexical_score_raw",
    "semantic_rank",
    "semantic_score_raw",
    "source_lanes",
];
const SEED_CONTRIBUTION_V2_FIELDS: &[&str] = &["lane", "rank", "raw_score", "corpus_kind"];
const SEED_CANDIDATE_V2_FIELDS: &[&str] = &[
    "record_id",
    "entity_id",
    "owner_kind",
    "corpus_kind",
    "repo_relative_path",
    "snippet",
    "seed_rank",
    "contributions",
    "degraded_reasons",
];

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
                let mut state = serializer.serialize_struct(stringify!($ty), 3)?;
                state.serialize_field("generation", &self.generation)?;
                state.serialize_field("results", &self.results)?;
                state.serialize_field("window", &self.window)?;
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
                let mut window: Option<QueryResultWindowV1> = None;
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
                        "window" => {
                            if window.is_some() {
                                return Err(de::Error::duplicate_field("window"));
                            }
                            window = Some(map.next_value()?);
                        }
                        other => {
                            return Err(de::Error::unknown_field(other, $fields));
                        }
                    }
                }
                let results = results.ok_or_else(|| de::Error::missing_field("results"))?;
                let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
                if usize::try_from(window.returned()).ok() != Some(results.len()) {
                    return Err(de::Error::custom(
                        "query result window returned count does not match results length",
                    ));
                }
                Ok($ty {
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                    results,
                    window,
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
                let mut state = serializer.serialize_struct(stringify!($ty), 4)?;
                state.serialize_field("generation", &self.generation)?;
                state.serialize_field("results", &self.results)?;
                state.serialize_field("window", &self.window)?;
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
                let mut window: Option<QueryResultWindowV1> = None;
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
                        "window" => {
                            if window.is_some() {
                                return Err(de::Error::duplicate_field("window"));
                            }
                            window = Some(map.next_value()?);
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
                let results = results.ok_or_else(|| de::Error::missing_field("results"))?;
                let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
                if usize::try_from(window.returned()).ok() != Some(results.len()) {
                    return Err(de::Error::custom(
                        "query result window returned count does not match results length",
                    ));
                }
                Ok($ty {
                    generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
                    results,
                    window,
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

macro_rules! impl_generation_results_unwindowed_response_serde {
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
                        other => return Err(de::Error::unknown_field(other, $fields)),
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

impl Serialize for FileOwnerProjectionRow {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("FileOwnerProjectionRow", 6)?;
        state.serialize_field("candidate_id", &self.candidate_id)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("owners", &self.owners)?;
        state.end()
    }
}

struct FileOwnerProjectionRowVisitor;

impl<'de> Visitor<'de> for FileOwnerProjectionRowVisitor {
    type Value = FileOwnerProjectionRow;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileOwnerProjectionRow map")
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
        let mut owners: Option<Vec<String>> = None;
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
                "owners" => {
                    if owners.is_some() {
                        return Err(de::Error::duplicate_field("owners"));
                    }
                    owners = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        FILE_OWNER_PROJECTION_ROW_FIELDS,
                    ));
                }
            }
        }
        Ok(FileOwnerProjectionRow {
            candidate_id: candidate_id.ok_or_else(|| de::Error::missing_field("candidate_id"))?,
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            manifest_generation: manifest_generation
                .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            owners: owners.ok_or_else(|| de::Error::missing_field("owners"))?,
        })
    }
}

impl<'de> Deserialize<'de> for FileOwnerProjectionRow {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileOwnerProjectionRow",
            FILE_OWNER_PROJECTION_ROW_FIELDS,
            FileOwnerProjectionRowVisitor,
        )
    }
}

impl Serialize for TextQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count = 3usize;
        if self.file_owner_rows.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("TextQueryResponse", field_count)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("results", &self.results)?;
        state.serialize_field("window", &self.window)?;
        if let Some(file_owner_rows) = &self.file_owner_rows {
            state.serialize_field("file_owner_rows", file_owner_rows)?;
        }
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
        let mut window: Option<QueryResultWindowV1> = None;
        let mut file_owner_rows: Option<Vec<FileOwnerProjectionRow>> = None;
        let mut file_owner_rows_seen = false;
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
                "window" => {
                    if window.is_some() {
                        return Err(de::Error::duplicate_field("window"));
                    }
                    window = Some(map.next_value()?);
                }
                "file_owner_rows" => {
                    if file_owner_rows_seen {
                        return Err(de::Error::duplicate_field("file_owner_rows"));
                    }
                    file_owner_rows_seen = true;
                    file_owner_rows = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, TEXT_QUERY_RESPONSE_FIELDS));
                }
            }
        }
        let results = results.ok_or_else(|| de::Error::missing_field("results"))?;
        let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
        if usize::try_from(window.returned()).ok() != Some(results.len()) {
            return Err(de::Error::custom(
                "query result window returned count does not match results length",
            ));
        }
        Ok(TextQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            results,
            window,
            file_owner_rows,
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

impl Serialize for HybridSeedLane {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Lexical => "Lexical",
            Self::Semantic => "Semantic",
        })
    }
}

impl<'de> Deserialize<'de> for HybridSeedLane {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HybridSeedLaneVisitor;

        impl Visitor<'_> for HybridSeedLaneVisitor {
            type Value = HybridSeedLane;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("string enum HybridSeedLane")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "Lexical" => Ok(HybridSeedLane::Lexical),
                    "Semantic" => Ok(HybridSeedLane::Semantic),
                    other => Err(de::Error::unknown_variant(other, &["Lexical", "Semantic"])),
                }
            }
        }

        deserializer.deserialize_str(HybridSeedLaneVisitor)
    }
}

impl Serialize for SeedLaneV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(match self {
            Self::Exact => "Exact",
            Self::Bm25 => "Bm25",
            Self::Dense => "Dense",
        })
    }
}

impl<'de> Deserialize<'de> for SeedLaneV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SeedLaneV2Visitor;

        impl Visitor<'_> for SeedLaneV2Visitor {
            type Value = SeedLaneV2;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("string enum SeedLaneV2")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match value {
                    "Exact" => Ok(SeedLaneV2::Exact),
                    "Bm25" => Ok(SeedLaneV2::Bm25),
                    "Dense" => Ok(SeedLaneV2::Dense),
                    other => Err(de::Error::unknown_variant(
                        other,
                        &["Exact", "Bm25", "Dense"],
                    )),
                }
            }
        }

        deserializer.deserialize_str(SeedLaneV2Visitor)
    }
}

impl Serialize for SeedContributionV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count = 2usize;
        if self.raw_score.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.corpus_kind.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("SeedContributionV2", field_count)?;
        state.serialize_field("lane", &self.lane)?;
        state.serialize_field("rank", &self.rank)?;
        if let Some(raw_score) = &self.raw_score {
            state.serialize_field("raw_score", raw_score)?;
        }
        if let Some(corpus_kind) = &self.corpus_kind {
            state.serialize_field("corpus_kind", corpus_kind)?;
        }
        state.end()
    }
}

struct SeedContributionV2Visitor;

impl<'de> Visitor<'de> for SeedContributionV2Visitor {
    type Value = SeedContributionV2;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SeedContributionV2 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut lane: Option<SeedLaneV2> = None;
        let mut rank: Option<u32> = None;
        let mut raw_score: Option<f32> = None;
        let mut raw_score_seen = false;
        let mut corpus_kind: Option<Option<SemanticCorpusKindV1>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "lane" => {
                    if lane.is_some() {
                        return Err(de::Error::duplicate_field("lane"));
                    }
                    lane = Some(map.next_value()?);
                }
                "rank" => {
                    if rank.is_some() {
                        return Err(de::Error::duplicate_field("rank"));
                    }
                    rank = Some(map.next_value()?);
                }
                "raw_score" => {
                    if raw_score_seen {
                        return Err(de::Error::duplicate_field("raw_score"));
                    }
                    raw_score_seen = true;
                    raw_score = Some(map.next_value()?);
                }
                "corpus_kind" => {
                    if corpus_kind.is_some() {
                        return Err(de::Error::duplicate_field("corpus_kind"));
                    }
                    corpus_kind = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, SEED_CONTRIBUTION_V2_FIELDS));
                }
            }
        }
        Ok(SeedContributionV2 {
            lane: lane.ok_or_else(|| de::Error::missing_field("lane"))?,
            rank: rank.ok_or_else(|| de::Error::missing_field("rank"))?,
            raw_score,
            corpus_kind: corpus_kind.unwrap_or(None),
        })
    }
}

impl<'de> Deserialize<'de> for SeedContributionV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SeedContributionV2",
            SEED_CONTRIBUTION_V2_FIELDS,
            SeedContributionV2Visitor,
        )
    }
}

impl Serialize for SeedCandidateV2 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("SeedCandidateV2", 9)?;
        state.serialize_field("record_id", &self.record_id)?;
        state.serialize_field("entity_id", &self.entity_id)?;
        state.serialize_field("owner_kind", &self.owner_kind)?;
        state.serialize_field("corpus_kind", &self.corpus_kind)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("snippet", &self.snippet)?;
        state.serialize_field("seed_rank", &self.seed_rank)?;
        state.serialize_field("contributions", &self.contributions)?;
        state.serialize_field("degraded_reasons", &self.degraded_reasons)?;
        state.end()
    }
}

struct SeedCandidateV2Visitor;

impl<'de> Visitor<'de> for SeedCandidateV2Visitor {
    type Value = SeedCandidateV2;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a SeedCandidateV2 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut record_id: Option<String> = None;
        let mut entity_id: Option<String> = None;
        let mut owner_kind: Option<OwnerDocKind> = None;
        let mut corpus_kind: Option<Option<SemanticCorpusKindV1>> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut snippet: Option<String> = None;
        let mut seed_rank: Option<u32> = None;
        let mut contributions: Option<Vec<SeedContributionV2>> = None;
        let mut degraded_reasons: Option<Vec<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "record_id" => {
                    if record_id.is_some() {
                        return Err(de::Error::duplicate_field("record_id"));
                    }
                    record_id = Some(map.next_value()?);
                }
                "entity_id" => {
                    if entity_id.is_some() {
                        return Err(de::Error::duplicate_field("entity_id"));
                    }
                    entity_id = Some(map.next_value()?);
                }
                "owner_kind" => {
                    if owner_kind.is_some() {
                        return Err(de::Error::duplicate_field("owner_kind"));
                    }
                    owner_kind = Some(map.next_value()?);
                }
                "corpus_kind" => {
                    if corpus_kind.is_some() {
                        return Err(de::Error::duplicate_field("corpus_kind"));
                    }
                    corpus_kind = Some(map.next_value()?);
                }
                "repo_relative_path" => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(map.next_value()?);
                }
                "snippet" => {
                    if snippet.is_some() {
                        return Err(de::Error::duplicate_field("snippet"));
                    }
                    snippet = Some(map.next_value()?);
                }
                "seed_rank" => {
                    if seed_rank.is_some() {
                        return Err(de::Error::duplicate_field("seed_rank"));
                    }
                    seed_rank = Some(map.next_value()?);
                }
                "contributions" => {
                    if contributions.is_some() {
                        return Err(de::Error::duplicate_field("contributions"));
                    }
                    contributions = Some(map.next_value()?);
                }
                "degraded_reasons" => {
                    if degraded_reasons.is_some() {
                        return Err(de::Error::duplicate_field("degraded_reasons"));
                    }
                    degraded_reasons = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, SEED_CANDIDATE_V2_FIELDS));
                }
            }
        }
        Ok(SeedCandidateV2 {
            record_id: record_id.ok_or_else(|| de::Error::missing_field("record_id"))?,
            entity_id: entity_id.ok_or_else(|| de::Error::missing_field("entity_id"))?,
            owner_kind: owner_kind.ok_or_else(|| de::Error::missing_field("owner_kind"))?,
            corpus_kind: corpus_kind.unwrap_or(None),
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            snippet: snippet.ok_or_else(|| de::Error::missing_field("snippet"))?,
            seed_rank: seed_rank.ok_or_else(|| de::Error::missing_field("seed_rank"))?,
            contributions: contributions
                .ok_or_else(|| de::Error::missing_field("contributions"))?,
            degraded_reasons: degraded_reasons
                .ok_or_else(|| de::Error::missing_field("degraded_reasons"))?,
        })
    }
}

impl<'de> Deserialize<'de> for SeedCandidateV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "SeedCandidateV2",
            SEED_CANDIDATE_V2_FIELDS,
            SeedCandidateV2Visitor,
        )
    }
}

impl Serialize for HybridSeedCandidate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count = 3usize;
        if self.lexical_rank.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.lexical_score_raw.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.semantic_rank.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.semantic_score_raw.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("HybridSeedCandidate", field_count)?;
        state.serialize_field("candidate", &self.candidate)?;
        state.serialize_field("seed_rank", &self.seed_rank)?;
        if let Some(lexical_rank) = &self.lexical_rank {
            state.serialize_field("lexical_rank", lexical_rank)?;
        }
        if let Some(lexical_score_raw) = &self.lexical_score_raw {
            state.serialize_field("lexical_score_raw", lexical_score_raw)?;
        }
        if let Some(semantic_rank) = &self.semantic_rank {
            state.serialize_field("semantic_rank", semantic_rank)?;
        }
        if let Some(semantic_score_raw) = &self.semantic_score_raw {
            state.serialize_field("semantic_score_raw", semantic_score_raw)?;
        }
        state.serialize_field("source_lanes", &self.source_lanes)?;
        state.end()
    }
}

struct HybridSeedCandidateVisitor;

impl<'de> Visitor<'de> for HybridSeedCandidateVisitor {
    type Value = HybridSeedCandidate;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HybridSeedCandidate map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut candidate: Option<LexicalCandidate> = None;
        let mut seed_rank: Option<u32> = None;
        let mut lexical_rank: Option<u32> = None;
        let mut lexical_rank_seen = false;
        let mut lexical_score_raw: Option<f32> = None;
        let mut lexical_score_raw_seen = false;
        let mut semantic_rank: Option<u32> = None;
        let mut semantic_rank_seen = false;
        let mut semantic_score_raw: Option<f32> = None;
        let mut semantic_score_raw_seen = false;
        let mut source_lanes: Option<Vec<HybridSeedLane>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "candidate" => {
                    if candidate.is_some() {
                        return Err(de::Error::duplicate_field("candidate"));
                    }
                    candidate = Some(map.next_value()?);
                }
                "seed_rank" => {
                    if seed_rank.is_some() {
                        return Err(de::Error::duplicate_field("seed_rank"));
                    }
                    seed_rank = Some(map.next_value()?);
                }
                "lexical_rank" => {
                    if lexical_rank_seen {
                        return Err(de::Error::duplicate_field("lexical_rank"));
                    }
                    lexical_rank_seen = true;
                    lexical_rank = Some(map.next_value()?);
                }
                "lexical_score_raw" => {
                    if lexical_score_raw_seen {
                        return Err(de::Error::duplicate_field("lexical_score_raw"));
                    }
                    lexical_score_raw_seen = true;
                    lexical_score_raw = Some(map.next_value()?);
                }
                "semantic_rank" => {
                    if semantic_rank_seen {
                        return Err(de::Error::duplicate_field("semantic_rank"));
                    }
                    semantic_rank_seen = true;
                    semantic_rank = Some(map.next_value()?);
                }
                "semantic_score_raw" => {
                    if semantic_score_raw_seen {
                        return Err(de::Error::duplicate_field("semantic_score_raw"));
                    }
                    semantic_score_raw_seen = true;
                    semantic_score_raw = Some(map.next_value()?);
                }
                "source_lanes" => {
                    if source_lanes.is_some() {
                        return Err(de::Error::duplicate_field("source_lanes"));
                    }
                    source_lanes = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        HYBRID_SEED_CANDIDATE_FIELDS,
                    ));
                }
            }
        }
        Ok(HybridSeedCandidate {
            candidate: candidate.ok_or_else(|| de::Error::missing_field("candidate"))?,
            seed_rank: seed_rank.ok_or_else(|| de::Error::missing_field("seed_rank"))?,
            lexical_rank,
            lexical_score_raw,
            semantic_rank,
            semantic_score_raw,
            source_lanes: source_lanes.ok_or_else(|| de::Error::missing_field("source_lanes"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HybridSeedCandidate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HybridSeedCandidate",
            HYBRID_SEED_CANDIDATE_FIELDS,
            HybridSeedCandidateVisitor,
        )
    }
}

impl Serialize for HybridSeedQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count = 5usize;
        if self.seed_candidates_v2.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("HybridSeedQueryResponse", field_count)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("seed_candidates", &self.seed_candidates)?;
        if let Some(seed_candidates_v2) = &self.seed_candidates_v2 {
            state.serialize_field("seed_candidates_v2", seed_candidates_v2)?;
        }
        state.serialize_field("window", &self.window)?;
        state.serialize_field("explanation", &self.explanation)?;
        state.end()
    }
}

struct HybridSeedQueryResponseVisitor;

impl<'de> Visitor<'de> for HybridSeedQueryResponseVisitor {
    type Value = HybridSeedQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HybridSeedQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut manifest_digest: Option<String> = None;
        let mut seed_candidates: Option<Vec<HybridSeedCandidate>> = None;
        let mut seed_candidates_v2: Option<Vec<SeedCandidateV2>> = None;
        let mut seed_candidates_v2_seen = false;
        let mut window: Option<QueryResultWindowV1> = None;
        let mut explanation: Option<SearchExplanation> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                "seed_candidates" => {
                    if seed_candidates.is_some() {
                        return Err(de::Error::duplicate_field("seed_candidates"));
                    }
                    seed_candidates = Some(map.next_value()?);
                }
                "seed_candidates_v2" => {
                    if seed_candidates_v2_seen {
                        return Err(de::Error::duplicate_field("seed_candidates_v2"));
                    }
                    seed_candidates_v2_seen = true;
                    seed_candidates_v2 = Some(map.next_value()?);
                }
                "window" => {
                    if window.is_some() {
                        return Err(de::Error::duplicate_field("window"));
                    }
                    window = Some(map.next_value()?);
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
                        HYBRID_SEED_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        let seed_candidates =
            seed_candidates.ok_or_else(|| de::Error::missing_field("seed_candidates"))?;
        let window = window.ok_or_else(|| de::Error::missing_field("window"))?;
        let returned_len = seed_candidates_v2
            .as_ref()
            .map_or(seed_candidates.len(), Vec::len);
        if usize::try_from(window.returned()).ok() != Some(returned_len) {
            return Err(de::Error::custom(
                "hybrid seed window returned count does not match active seed result length",
            ));
        }
        Ok(HybridSeedQueryResponse {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            seed_candidates,
            seed_candidates_v2,
            window,
            explanation: explanation.ok_or_else(|| de::Error::missing_field("explanation"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HybridSeedQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HybridSeedQueryResponse",
            HYBRID_SEED_QUERY_RESPONSE_FIELDS,
            HybridSeedQueryResponseVisitor,
        )
    }
}

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
impl_generation_results_unwindowed_response_serde!(
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
impl_generation_results_unwindowed_response_serde!(
    SearchPlaneStructuralQueryResponse,
    SEARCH_PLANE_STRUCTURAL_QUERY_RESPONSE_FIELDS,
    SearchPlaneStructuralQueryResponseVisitor,
    StructuralCandidate
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GenerationPin, ManifestGeneration, RepoId, RevisionId};

    fn sample_generation_pin() -> GenerationPin {
        GenerationPin::new(
            RepoId::new("repo-seed"),
            RevisionId::new("rev-seed"),
            ManifestGeneration::new(7),
        )
    }

    fn sample_seed_candidate() -> HybridSeedCandidate {
        HybridSeedCandidate {
            candidate: LexicalCandidate {
                candidate_id: "lex-1".to_string(),
                repo_id: RepoId::new("repo-seed"),
                revision_id: RevisionId::new("rev-seed"),
                manifest_generation: ManifestGeneration::new(7),
                repo_relative_path: RepoRelativePath::new("src/lib.rs"),
                start_line: 10,
                end_line: 12,
                score: 0.75,
                snippet: "fn demo() {}".to_string(),
                snippet_hit_offset: None,
                highlights: Vec::new(),
            },
            seed_rank: 1,
            lexical_rank: Some(1),
            lexical_score_raw: Some(0.75),
            semantic_rank: None,
            semantic_score_raw: None,
            source_lanes: vec![HybridSeedLane::Lexical],
        }
    }

    fn sample_seed_candidate_v2() -> SeedCandidateV2 {
        SeedCandidateV2 {
            record_id: "semantic-source:symbol-card:symbol:demo".to_string(),
            entity_id: "symbol:demo".to_string(),
            owner_kind: OwnerDocKind::Symbol,
            corpus_kind: Some(SemanticCorpusKindV1::SymbolCard),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            snippet: "symbol: symbol:demo".to_string(),
            seed_rank: 1,
            contributions: vec![SeedContributionV2 {
                lane: SeedLaneV2::Dense,
                rank: 2,
                raw_score: Some(0.5),
                corpus_kind: Some(SemanticCorpusKindV1::SymbolCard),
            }],
            degraded_reasons: vec!["lexical_only_owner_kind_fallback".to_string()],
        }
    }

    #[test]
    fn seed_fusion_identity_borrowed_comparator_matches_owned_order_v2() {
        let identities = [
            SeedFusionIdentityV2::new(OwnerDocKind::Symbol, "z".to_string()),
            SeedFusionIdentityV2::new(OwnerDocKind::Chunk, "a".to_string()),
            SeedFusionIdentityV2::new(OwnerDocKind::Symbol, "a".to_string()),
            SeedFusionIdentityV2::new(OwnerDocKind::Module, "z".to_string()),
        ];

        for left in &identities {
            for right in &identities {
                assert_eq!(
                    left.cmp(right),
                    SeedFusionIdentityV2::cmp_parts(
                        left.owner_kind(),
                        left.entity_id(),
                        right.owner_kind(),
                        right.entity_id(),
                    )
                );
            }
        }
    }

    #[test]
    fn hybrid_seed_query_response_without_required_window_fails_closed() {
        let value = serde_json::json!({
            "generation": {
                "repo_id": "repo-seed",
                "revision_id": "rev-seed",
                "manifest_generation": 7
            },
            "manifest_digest": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "seed_candidates": [{
                "candidate": {
                    "candidate_id": "lex-1",
                    "repo_id": "repo-seed",
                    "revision_id": "rev-seed",
                    "manifest_generation": 7,
                    "repo_relative_path": "src/lib.rs",
                    "start_line": 10,
                    "end_line": 12,
                    "score": 0.75,
                    "snippet": "fn demo() {}",
                    "snippet_hit_offset": null,
                    "highlights": []
                },
                "seed_rank": 1,
                "lexical_rank": 1,
                "lexical_score_raw": 0.75,
                "source_lanes": ["Lexical"]
            }],
            "explanation": serde_json::to_value(SearchExplanation::default())
                .expect("default explanation must serialize")
        });
        let decoded = serde_json::from_value::<HybridSeedQueryResponse>(value);
        assert!(
            decoded.is_err(),
            "payload without mandatory result-window semantics must fail closed"
        );
    }

    #[test]
    fn hybrid_seed_query_response_round_trips_with_v2_candidates() {
        let response = HybridSeedQueryResponse {
            generation: sample_generation_pin(),
            manifest_digest: "a".repeat(64),
            seed_candidates: vec![sample_seed_candidate()],
            seed_candidates_v2: Some(vec![sample_seed_candidate_v2()]),
            window: QueryResultWindowV1::exact(1),
            explanation: SearchExplanation::default(),
        };

        let decoded_json: HybridSeedQueryResponse = serde_json::from_value(
            serde_json::to_value(&response).expect("response must serialize to JSON"),
        )
        .expect("response must deserialize from JSON");
        assert_eq!(decoded_json, response);

        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&response, &mut cbor).expect("response must serialize to CBOR");
        let decoded_cbor: HybridSeedQueryResponse = ciborium::de::from_reader(cbor.as_slice())
            .expect("response must deserialize from CBOR");
        assert_eq!(decoded_cbor, response);
    }
}
