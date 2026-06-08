use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{ManifestGeneration, RepoId, RepoRelativePath, RevisionId};

#[derive(Clone, Debug, PartialEq)]
pub struct LexicalCandidate {
    pub candidate_id: String,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub repo_relative_path: RepoRelativePath,
    pub start_line: u32,
    pub end_line: u32,
    pub score: f32,
    pub snippet: String,
    /// Byte offset of the primary matched hit within [`Self::snippet`], for UI
    /// highlight anchoring. `Some(off)` lets a consumer place a highlight without
    /// re-deriving the match from the raw snippet text (J7Q-07); `None` when the
    /// producing route carries no single lexical hit anchor (e.g. a symbol or
    /// projected candidate).
    pub snippet_hit_offset: Option<u32>,
}

const LEXICAL_CANDIDATE_FIELDS: &[&str] = &[
    "candidate_id",
    "repo_id",
    "revision_id",
    "manifest_generation",
    "repo_relative_path",
    "start_line",
    "end_line",
    "score",
    "snippet",
    "snippet_hit_offset",
];

impl Serialize for LexicalCandidate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("LexicalCandidate", 10)?;
        state.serialize_field("candidate_id", &self.candidate_id)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("start_line", &self.start_line)?;
        state.serialize_field("end_line", &self.end_line)?;
        state.serialize_field("score", &self.score)?;
        state.serialize_field("snippet", &self.snippet)?;
        state.serialize_field("snippet_hit_offset", &self.snippet_hit_offset)?;
        state.end()
    }
}

struct LexicalCandidateVisitor;

impl<'de> Visitor<'de> for LexicalCandidateVisitor {
    type Value = LexicalCandidate;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a LexicalCandidate map")
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
        let mut finish_line: Option<u32> = None;
        let mut score: Option<f32> = None;
        let mut snippet: Option<String> = None;
        let mut snippet_hit_offset: Option<Option<u32>> = None;
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
                    if finish_line.is_some() {
                        return Err(de::Error::duplicate_field("end_line"));
                    }
                    finish_line = Some(map.next_value()?);
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
                "snippet_hit_offset" => {
                    if snippet_hit_offset.is_some() {
                        return Err(de::Error::duplicate_field("snippet_hit_offset"));
                    }
                    snippet_hit_offset = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, LEXICAL_CANDIDATE_FIELDS)),
            }
        }
        let candidate_id = candidate_id.ok_or_else(|| de::Error::missing_field("candidate_id"))?;
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let manifest_generation =
            manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?;
        let repo_relative_path =
            repo_relative_path.ok_or_else(|| de::Error::missing_field("repo_relative_path"))?;
        let start_line = start_line.ok_or_else(|| de::Error::missing_field("start_line"))?;
        let end_line = finish_line.ok_or_else(|| de::Error::missing_field("end_line"))?;
        let score = score.ok_or_else(|| de::Error::missing_field("score"))?;
        let snippet = snippet.ok_or_else(|| de::Error::missing_field("snippet"))?;
        let snippet_hit_offset =
            snippet_hit_offset.ok_or_else(|| de::Error::missing_field("snippet_hit_offset"))?;
        Ok(LexicalCandidate {
            candidate_id,
            repo_id,
            revision_id,
            manifest_generation,
            repo_relative_path,
            start_line,
            end_line,
            score,
            snippet,
            snippet_hit_offset,
        })
    }
}

impl<'de> Deserialize<'de> for LexicalCandidate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "LexicalCandidate",
            LEXICAL_CANDIDATE_FIELDS,
            LexicalCandidateVisitor,
        )
    }
}
