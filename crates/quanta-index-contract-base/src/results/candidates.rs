use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::index_refs::{ObjectRef, OwnerKey};
use crate::{
    HighlightSpan, ManifestGeneration, PreviewMetadata, RepoId, RepoRelativePath, RevisionId,
    SourceFileRevision,
};

/// Immutable identity of the Native Source record a file row was read from.
///
/// `source` is the content-addressed Source record object (path, byte object,
/// admission and metadata) and `owner` is its publishing owner. It is not a
/// SHA-256 of the file bytes, so it never fills [`SourceFileRevision`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeSourceIdentity {
    pub owner: OwnerKey,
    pub source: ObjectRef,
}
crate::native_serde_struct!(NativeSourceIdentity {
    owner: OwnerKey,
    source: ObjectRef,
});

/// The lines a lexical row's match spans in its file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LexicalLineRange {
    pub start_line: u32,
    pub end_line: u32,
}

/// The text excerpt a lexical row shows around its match.
///
/// A row either carries all of these or none: a producer that reports no
/// excerpt leaves [`LexicalCandidate::excerpt`] absent rather than filling an
/// empty snippet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexicalMatchExcerpt {
    pub snippet: String,
    /// Byte offset of the primary matched hit within [`Self::snippet`], for UI
    /// highlight anchoring. `Some(off)` lets a consumer place a highlight without
    /// re-deriving the match from the raw snippet text (J7Q-07); `None` when the
    /// producing route carries no single lexical hit anchor (e.g. a symbol).
    /// For a path preview the offset is relative to the emitted path bytes.
    /// Equals the first [`Self::highlights`] span's `start`.
    pub snippet_hit_offset: Option<u32>,
    /// Every matched-hit byte range within [`Self::snippet`], in ascending start
    /// order, for multi-hit UI highlighting (J7Q-07). Empty when the producing
    /// route carries no lexical hit spans; a consumer renders each span verbatim
    /// without regex-parsing the snippet.
    pub highlights: Vec<HighlightSpan>,
}

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
    /// The match's lines; absent when the producer reports no line position,
    /// as for a path match. Its wire keys `start_line` and `end_line` are both
    /// present or both omitted.
    pub lines: Option<LexicalLineRange>,
    pub score: f32,
    /// The excerpt around the match; absent when the producer reports none.
    /// Its wire keys `snippet`, `snippet_hit_offset` and `highlights` are all
    /// present or all omitted.
    pub excerpt: Option<LexicalMatchExcerpt>,
    /// The Native Source record behind this row; absent on Legacy rows. Its
    /// wire key is omitted when absent.
    pub native_source: Option<NativeSourceIdentity>,
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
    "native_source",
];

impl Serialize for LexicalCandidate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.validate_source_metadata()
            .map_err(serde::ser::Error::custom)?;
        let fields = [
            (self.lines.is_some(), 2),
            (self.excerpt.is_some(), 3),
            (self.native_source.is_some(), 1),
        ]
        .iter()
        .filter(|(present, _)| *present)
        .fold(9_usize, |total, (_, keys)| total.saturating_add(*keys));
        let mut state = serializer.serialize_struct("LexicalCandidate", fields)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("source", &self.source)?;
        state.serialize_field("preview", &self.preview)?;
        state.serialize_field("candidate_id", &self.candidate_id)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        // Legacy rows keep their exact key order: lines, score, excerpt.
        if let Some(lines) = &self.lines {
            state.serialize_field("start_line", &lines.start_line)?;
            state.serialize_field("end_line", &lines.end_line)?;
        }
        state.serialize_field("score", &self.score)?;
        if let Some(excerpt) = &self.excerpt {
            state.serialize_field("snippet", &excerpt.snippet)?;
            state.serialize_field("snippet_hit_offset", &excerpt.snippet_hit_offset)?;
            state.serialize_field("highlights", &excerpt.highlights)?;
        }
        if let Some(native_source) = &self.native_source {
            state.serialize_field("native_source", native_source)?;
        }
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
        let mut native_source: Option<NativeSourceIdentity> = None;
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
                "native_source" => {
                    if native_source.is_some() {
                        return Err(de::Error::duplicate_field("native_source"));
                    }
                    native_source = Some(map.next_value()?);
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
        let score = score.ok_or_else(|| de::Error::missing_field("score"))?;
        let lines = match (start_line, finish_line) {
            (Some(start_line), Some(end_line)) => Some(LexicalLineRange {
                start_line,
                end_line,
            }),
            (None, None) => None,
            _ => {
                return Err(de::Error::custom(
                    "lexical lines need start_line and end_line together, or neither",
                ));
            }
        };
        let excerpt = match (snippet, snippet_hit_offset, highlights) {
            (Some(snippet), Some(snippet_hit_offset), Some(highlights)) => {
                Some(LexicalMatchExcerpt {
                    snippet,
                    snippet_hit_offset,
                    highlights,
                })
            }
            (None, None, None) => None,
            _ => {
                return Err(de::Error::custom(
                    "a lexical excerpt needs snippet, snippet_hit_offset and highlights \
                     together, or none of them",
                ));
            }
        };
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
            lines,
            score,
            excerpt,
            native_source,
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
        if let Some(excerpt) = &self.excerpt {
            PreviewMetadata::validate_highlight_ranges(
                &excerpt.snippet,
                excerpt.snippet_hit_offset,
                &excerpt.highlights,
            )?;
        }
        if let Some(preview) = &self.preview {
            let Some(excerpt) = &self.excerpt else {
                return Err("a preview describes the row's match excerpt, which is absent");
            };
            preview.validate_highlights(
                &excerpt.snippet,
                excerpt.snippet_hit_offset,
                &excerpt.highlights,
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
            lines: Some(LexicalLineRange {
                start_line: 1,
                end_line: 1,
            }),
            excerpt: Some(LexicalMatchExcerpt {
                snippet: String::new(),
                snippet_hit_offset: None,
                highlights: Vec::new(),
            }),
            score: 1.0,
            native_source: None,
        }
    }

    const EXCERPT_KEYS: [&str; 5] = [
        "start_line",
        "end_line",
        "snippet",
        "snippet_hit_offset",
        "highlights",
    ];

    /// A Legacy row's wire is exactly the wire it had while every excerpt
    /// field was mandatory: the same keys in the same order.
    #[test]
    fn legacy_row_wire_is_byte_identical_and_round_trips() {
        let mut legacy = candidate();
        legacy.source = None;
        legacy.score = 1.5;
        legacy.lines = Some(LexicalLineRange {
            start_line: 3,
            end_line: 4,
        });
        legacy.excerpt = Some(LexicalMatchExcerpt {
            snippet: "fn a()".into(),
            snippet_hit_offset: Some(3),
            highlights: vec![HighlightSpan { start: 3, len: 1 }],
        });
        let golden = concat!(
            r#"{"source_repo_id":"source-a","source":null,"preview":null,"#,
            r#""candidate_id":"c","repo_id":"container","revision_id":"snapshot","#,
            r#""manifest_generation":1,"repo_relative_path":"src/a.rs","#,
            r#""start_line":3,"end_line":4,"score":1.5,"snippet":"fn a()","#,
            r#""snippet_hit_offset":3,"highlights":[{"start":3,"len":1}]}"#,
        );
        assert_eq!(serde_json::to_string(&legacy).expect("encode"), golden);
        assert_eq!(
            serde_json::from_str::<LexicalCandidate>(golden).expect("decode"),
            legacy
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&legacy, &mut cbor).expect("encode");
        assert_eq!(
            ciborium::de::from_reader::<LexicalCandidate, _>(cbor.as_slice()).expect("decode"),
            legacy
        );
    }

    /// A row without lines or an excerpt omits their keys, never encoding
    /// zero lines or an empty snippet; a partial group is refused, and a path
    /// excerpt stands without lines.
    #[test]
    fn absent_lines_and_excerpt_omit_their_keys_and_partial_groups_are_refused() {
        let mut native = candidate();
        native.source = None;
        native.lines = None;
        native.excerpt = None;
        native.native_source = Some(NativeSourceIdentity {
            owner: OwnerKey {
                producer: crate::index_refs::ProducerId([5; 16]),
                stable_id: crate::index_refs::StableOwnerId([6; 16]),
            },
            source: ObjectRef(crate::index_refs::Digest([7; 32])),
        });
        let raw = serde_json::to_value(&native).expect("encode");
        for key in EXCERPT_KEYS {
            assert!(raw.get(key).is_none(), "{key} is on the wire");
        }
        assert_eq!(
            serde_json::from_value::<LexicalCandidate>(raw).expect("decode"),
            native
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&native, &mut cbor).expect("encode");
        assert_eq!(
            ciborium::de::from_reader::<LexicalCandidate, _>(cbor.as_slice()).expect("decode"),
            native
        );
        let legacy = serde_json::to_value(candidate()).expect("encode");
        for key in EXCERPT_KEYS {
            let mut partial = legacy.clone();
            let _removed = partial.as_object_mut().expect("object").remove(key);
            assert!(
                serde_json::from_value::<LexicalCandidate>(partial).is_err(),
                "a row without only {key} decoded"
            );
        }
        // A path excerpt carries no lines: its line keys stay off the wire.
        let mut path = native.clone();
        path.excerpt = Some(LexicalMatchExcerpt {
            snippet: "src/a.rs".into(),
            snippet_hit_offset: Some(4),
            highlights: vec![HighlightSpan { start: 4, len: 1 }],
        });
        let path_wire = serde_json::to_value(&path).expect("encode");
        assert!(path_wire.get("start_line").is_none() && path_wire.get("end_line").is_none());
        assert_eq!(
            path_wire.get("snippet"),
            Some(&serde_json::json!("src/a.rs"))
        );
        assert_eq!(
            serde_json::from_value::<LexicalCandidate>(path_wire).expect("decode"),
            path
        );
        // A preview describes an excerpt; it cannot ride on a row without one.
        native.preview = Some(PreviewMetadata::unavailable(
            crate::PreviewKind::SourceChunk,
            crate::PreviewUnavailableReason::WorkBudget,
            None,
        ));
        assert!(serde_json::to_string(&native).is_err());
    }

    #[test]
    fn native_source_identity_round_trips_and_stays_off_legacy_wire() {
        let legacy = candidate();
        let raw = serde_json::to_value(&legacy).expect("encode");
        assert!(raw.get("native_source").is_none());
        let mut native = legacy;
        native.source = None;
        native.native_source = Some(NativeSourceIdentity {
            owner: OwnerKey {
                producer: crate::index_refs::ProducerId([5; 16]),
                stable_id: crate::index_refs::StableOwnerId([6; 16]),
            },
            source: ObjectRef(crate::index_refs::Digest([7; 32])),
        });
        let encoded = serde_json::to_string(&native).expect("encode");
        assert_eq!(
            serde_json::from_str::<LexicalCandidate>(&encoded).expect("decode"),
            native
        );
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&native, &mut cbor).expect("encode");
        assert_eq!(
            ciborium::de::from_reader::<LexicalCandidate, _>(cbor.as_slice()).expect("decode"),
            native
        );
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
            let duplicate = format!(
                "{{\"{field}\":null,{}",
                raw.strip_prefix('{').expect("JSON object")
            );
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
