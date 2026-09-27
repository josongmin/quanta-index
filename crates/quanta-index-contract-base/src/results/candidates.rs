use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    HighlightSpan, ManifestGeneration, PreviewMetadata, RepoId, RepoRelativePath, RevisionId,
    SourceFileRevision,
};

#[derive(Clone, Debug, PartialEq)]
pub struct LexicalCandidate {
    pub source_repo_id: RepoId,
    pub source: Option<SourceFileRevision>,
    pub preview: Option<PreviewMetadata>,
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
    /// projected candidate). Equals the first [`Self::highlights`] span's `start`.
    pub snippet_hit_offset: Option<u32>,
    /// Every matched-hit byte range within [`Self::snippet`], in ascending start
    /// order, for multi-hit UI highlighting (J7Q-07). Empty when the producing
    /// route carries no lexical hit spans; a consumer renders each span verbatim
    /// without regex-parsing the snippet.
    pub highlights: Vec<HighlightSpan>,
}

const LEXICAL_CANDIDATE_FIELDS: &[&str] = &[
    "source_repo_id",
    "source",
    "preview",
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
    "highlights",
];

impl Serialize for LexicalCandidate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_source_metadata()
            .map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct("LexicalCandidate", 14)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("source", &self.source)?;
        state.serialize_field("preview", &self.preview)?;
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
        state.serialize_field("highlights", &self.highlights)?;
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
        let mut source_repo_id: Option<RepoId> = None;
        let mut source: Option<Option<SourceFileRevision>> = None;
        let mut preview: Option<Option<PreviewMetadata>> = None;
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
        let mut highlights: Option<Vec<HighlightSpan>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "source_repo_id" => {
                    if source_repo_id.is_some() {
                        return Err(de::Error::duplicate_field("source_repo_id"));
                    }
                    source_repo_id = Some(map.next_value()?);
                }
                "source" => {
                    if source.is_some() {
                        return Err(de::Error::duplicate_field("source"));
                    }
                    source = Some(map.next_value()?);
                }
                "preview" => {
                    if preview.is_some() {
                        return Err(de::Error::duplicate_field("preview"));
                    }
                    preview = Some(map.next_value()?);
                }
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
                "highlights" => {
                    if highlights.is_some() {
                        return Err(de::Error::duplicate_field("highlights"));
                    }
                    highlights = Some(map.next_value()?);
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
        let highlights = highlights.ok_or_else(|| de::Error::missing_field("highlights"))?;
        let value = LexicalCandidate {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            source: source.ok_or_else(|| de::Error::missing_field("source"))?,
            preview: preview.ok_or_else(|| de::Error::missing_field("preview"))?,
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
            highlights,
        };
        value
            .validate_source_metadata()
            .map_err(de::Error::custom)?;
        Ok(value)
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

impl LexicalCandidate {
    /// Check identity and emitted-byte consistency; immutable bytes remain reader-owned.
    pub fn validate_source_metadata(&self) -> Result<(), &'static str> {
        if let Some(source) = &self.source {
            if source.file.source_repo_id != self.source_repo_id {
                return Err("candidate source repo disagrees with source revision identity");
            }
            source.validate()?;
            if source.file.repo_relative_path != self.repo_relative_path {
                return Err("candidate path disagrees with source file identity");
            }
        }
        PreviewMetadata::validate_highlight_ranges(
            &self.snippet,
            self.snippet_hit_offset,
            &self.highlights,
        )?;
        if let Some(preview) = &self.preview {
            preview.validate_highlights(
                &self.snippet,
                self.snippet_hit_offset,
                &self.highlights,
            )?;
            if preview.source != self.source
                && (preview.unavailable_reason.is_none() || preview.source.is_some())
            {
                return Err("preview source disagrees with candidate source identity");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod l3_tests {
    use super::*;
    use crate::{PreviewKind, PreviewUnavailableReason, SourceFileKey};

    fn candidate() -> LexicalCandidate {
        LexicalCandidate {
            source_repo_id: RepoId::new("source-a").expect("repo"),
            source: Some(SourceFileRevision {
                file: SourceFileKey {
                    source_repo_id: RepoId::new("source-a").expect("repo"),
                    repo_relative_path: RepoRelativePath::new("src/a.rs"),
                },
                revision_id: RevisionId::new("source-rev").expect("rev"),
                source_sha256: [3; 32],
            }),
            preview: None,
            candidate_id: "c".into(),
            repo_id: RepoId::new("container").expect("repo"),
            revision_id: RevisionId::new("snapshot").expect("rev"),
            manifest_generation: ManifestGeneration::new(1),
            repo_relative_path: RepoRelativePath::new("src/a.rs"),
            start_line: 1,
            end_line: 1,
            score: 1.0,
            snippet: String::new(),
            snippet_hit_offset: None,
            highlights: Vec::new(),
        }
    }

    #[test]
    fn l3_source_wire_requires_explicit_keys_and_preserves_containing_pin() {
        let row = candidate();
        let raw = serde_json::to_string(&row).expect("encode");
        assert_eq!(
            serde_json::from_str::<LexicalCandidate>(&raw).expect("decode"),
            row
        );
        assert_eq!(row.order_key().source_repo_id, "source-a");
        assert_eq!(row.repo_id.as_str(), "container");
        for field in ["source_repo_id", "source", "preview"] {
            let mut value = serde_json::to_value(&row).expect("encode");
            let _removed = value.as_object_mut().expect("object").remove(field);
            assert!(serde_json::from_value::<LexicalCandidate>(value).is_err());
            let duplicate = format!("{{\"{field}\":null,{}", &raw[1..]);
            assert!(serde_json::from_str::<LexicalCandidate>(&duplicate).is_err());
        }
        let mut unbound = row;
        unbound.source = None;
        assert_eq!(unbound.order_key().source_repo_id, "source-a");
        assert!(serde_json::to_string(&unbound).is_ok());
    }

    #[test]
    fn l3_candidate_rejects_source_path_and_preview_identity_disagreement() {
        let mut row = candidate();
        row.repo_relative_path = RepoRelativePath::new("wrong.rs");
        assert!(serde_json::to_string(&row).is_err());
        let mut row = candidate();
        let mut wrong = row.source.clone().expect("source");
        wrong.source_sha256 = [9; 32];
        row.preview = Some(PreviewMetadata::unavailable(
            PreviewKind::SourceChunk,
            PreviewUnavailableReason::WorkBudget,
            Some(wrong),
        ));
        assert!(serde_json::to_string(&row).is_err());
        row.preview = Some(PreviewMetadata::unavailable(
            PreviewKind::SourceChunk,
            PreviewUnavailableReason::WorkBudget,
            None,
        ));
        assert!(serde_json::to_string(&row).is_ok());
    }
}
