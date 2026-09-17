//! The history route's result order and keyset cursor (QI-BB-023).
//!
//! History results are ordered by recency, as a total order every page and
//! every restart agree on:
//!
//! - commits: `committer_time_ms` descending, then `sha` ascending;
//! - diffs: the commit's `committer_time_ms` descending, then `sha`
//!   ascending, then `file_path` ascending.
//!
//! A [`HistoryCursor`] names the last element a page returned under that
//! order; the next page holds the elements strictly after it. The cursor
//! is a keyset, not an offset: an ingest between two pages neither skips
//! nor repeats an element that was already positioned relative to it.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::lex::CommitSha;

/// The position of one history element under the recency order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryCursor {
    /// The commit's committer time; higher sorts first.
    pub committer_time_ms: u64,
    /// The commit; lower sorts first among equal times.
    pub sha: CommitSha,
    /// For diff pages, the hunk's path; lower sorts first among equal
    /// commits. Absent on commit pages.
    pub file_path: Option<String>,
}

const HISTORY_CURSOR_FIELDS: &[&str] = &["committer_time_ms", "sha", "file_path"];

impl Serialize for HistoryCursor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let field_count = if self.file_path.is_some() { 3 } else { 2 };
        let mut state = serializer.serialize_struct("HistoryCursor", field_count)?;
        state.serialize_field("committer_time_ms", &self.committer_time_ms)?;
        state.serialize_field("sha", &self.sha)?;
        if let Some(file_path) = &self.file_path {
            state.serialize_field("file_path", file_path)?;
        }
        state.end()
    }
}

struct HistoryCursorVisitor;

impl<'de> Visitor<'de> for HistoryCursorVisitor {
    type Value = HistoryCursor;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryCursor map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut committer_time_ms: Option<u64> = None;
        let mut sha: Option<CommitSha> = None;
        let mut file_path: Option<String> = None;
        let mut file_path_seen = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "committer_time_ms" => {
                    if committer_time_ms.is_some() {
                        return Err(de::Error::duplicate_field("committer_time_ms"));
                    }
                    committer_time_ms = Some(map.next_value()?);
                }
                "sha" => {
                    if sha.is_some() {
                        return Err(de::Error::duplicate_field("sha"));
                    }
                    sha = Some(map.next_value()?);
                }
                "file_path" => {
                    if file_path_seen {
                        return Err(de::Error::duplicate_field("file_path"));
                    }
                    file_path_seen = true;
                    file_path = map.next_value()?;
                }
                other => return Err(de::Error::unknown_field(other, HISTORY_CURSOR_FIELDS)),
            }
        }
        Ok(HistoryCursor {
            committer_time_ms: committer_time_ms
                .ok_or_else(|| de::Error::missing_field("committer_time_ms"))?,
            sha: sha.ok_or_else(|| de::Error::missing_field("sha"))?,
            file_path,
        })
    }
}

impl<'de> Deserialize<'de> for HistoryCursor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HistoryCursor",
            HISTORY_CURSOR_FIELDS,
            HistoryCursorVisitor,
        )
    }
}
