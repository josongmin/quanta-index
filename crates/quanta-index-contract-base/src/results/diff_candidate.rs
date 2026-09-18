use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::results::HistoryScoreV1;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DiffHunkSide {
    Before,
    After,
}

impl DiffHunkSide {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
        }
    }
}

impl Serialize for DiffHunkSide {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct DiffHunkSideVisitor;

impl Visitor<'_> for DiffHunkSideVisitor {
    type Value = DiffHunkSide;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DiffHunkSide string")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        match value {
            "before" => Ok(DiffHunkSide::Before),
            "after" => Ok(DiffHunkSide::After),
            other => Err(de::Error::unknown_variant(other, &["before", "after"])),
        }
    }
}

impl<'de> Deserialize<'de> for DiffHunkSide {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_str(DiffHunkSideVisitor)
    }
}

/// One diff hunk row of a history page.
///
/// `score` is the row's relevance score and is present exactly when the
/// page was served under the history route's `relevance` order; a
/// recency page carries none (QI-BB-023 follow-up #1). The page decoder
/// holds the invariant across every row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffCandidate {
    pub repo_relative_path: String,
    pub hunk_header: String,
    pub side: DiffHunkSide,
    pub line_start: u32,
    pub line_end: u32,
    pub snippet: String,
    pub score: Option<HistoryScoreV1>,
}

const DIFF_CANDIDATE_FIELDS: &[&str] = &[
    "repo_relative_path",
    "hunk_header",
    "side",
    "line_start",
    "line_end",
    "snippet",
    "score",
];

impl Serialize for DiffCandidate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = if self.score.is_some() { 7 } else { 6 };
        let mut state = serializer.serialize_struct("DiffCandidate", field_count)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("hunk_header", &self.hunk_header)?;
        state.serialize_field("side", &self.side)?;
        state.serialize_field("line_start", &self.line_start)?;
        state.serialize_field("line_end", &self.line_end)?;
        state.serialize_field("snippet", &self.snippet)?;
        if let Some(score) = &self.score {
            state.serialize_field("score", score)?;
        }
        state.end()
    }
}

struct DiffCandidateVisitor;

impl<'de> Visitor<'de> for DiffCandidateVisitor {
    type Value = DiffCandidate;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DiffCandidate map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_relative_path: Option<String> = None;
        let mut hunk_header: Option<String> = None;
        let mut side: Option<DiffHunkSide> = None;
        let mut line_start: Option<u32> = None;
        let mut line_end: Option<u32> = None;
        let mut snippet: Option<String> = None;
        let mut score: Option<HistoryScoreV1> = None;
        let mut score_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_relative_path" => repo_relative_path = Some(map.next_value()?),
                "hunk_header" => hunk_header = Some(map.next_value()?),
                "side" => side = Some(map.next_value()?),
                "line_start" => line_start = Some(map.next_value()?),
                "line_end" => line_end = Some(map.next_value()?),
                "snippet" => snippet = Some(map.next_value()?),
                "score" => {
                    if score_seen {
                        return Err(de::Error::duplicate_field("score"));
                    }
                    score_seen = true;
                    score = map.next_value()?;
                }
                other => return Err(de::Error::unknown_field(other, DIFF_CANDIDATE_FIELDS)),
            }
        }
        Ok(DiffCandidate {
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            hunk_header: hunk_header.ok_or_else(|| de::Error::missing_field("hunk_header"))?,
            side: side.ok_or_else(|| de::Error::missing_field("side"))?,
            line_start: line_start.ok_or_else(|| de::Error::missing_field("line_start"))?,
            line_end: line_end.ok_or_else(|| de::Error::missing_field("line_end"))?,
            snippet: snippet.ok_or_else(|| de::Error::missing_field("snippet"))?,
            score,
        })
    }
}

impl<'de> Deserialize<'de> for DiffCandidate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "DiffCandidate",
            DIFF_CANDIDATE_FIELDS,
            DiffCandidateVisitor,
        )
    }
}
