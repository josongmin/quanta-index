//! Wire-format row types decoded by the lexical build adapter.
//!
//! Phase 1 (D15) wire format: chunk rows are JSON-encoded into the manifest's
//! `lexical_chunk_rows` artifact. Per the workspace `rust-no-serde-derive`
//! ban, `Deserialize` is implemented by hand.

use core::fmt;

use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};

/// One lexical chunk row decoded from the manifest payload.
///
/// Fields map 1:1 to columns in the Tantivy schema (`repo_relative_path`,
/// `start_line`, `end_line`, `text`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkRow {
    pub repo_relative_path: String,
    pub start_line: u64,
    pub end_line: u64,
    pub text: String,
}

const CHUNK_ROW_FIELDS: &[&str] = &["repo_relative_path", "start_line", "end_line", "text"];

struct ChunkRowVisitor;

impl<'de> Visitor<'de> for ChunkRowVisitor {
    type Value = ChunkRow;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ChunkRow map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_relative_path: Option<String> = None;
        let mut start_line: Option<u64> = None;
        let mut end_line: Option<u64> = None;
        let mut text: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
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
                "text" => {
                    if text.is_some() {
                        return Err(de::Error::duplicate_field("text"));
                    }
                    text = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, CHUNK_ROW_FIELDS));
                }
            }
        }
        let repo_relative_path =
            repo_relative_path.ok_or_else(|| de::Error::missing_field("repo_relative_path"))?;
        let start_line = start_line.ok_or_else(|| de::Error::missing_field("start_line"))?;
        let end_line = end_line.ok_or_else(|| de::Error::missing_field("end_line"))?;
        let text = text.ok_or_else(|| de::Error::missing_field("text"))?;
        Ok(ChunkRow {
            repo_relative_path,
            start_line,
            end_line,
            text,
        })
    }
}

impl<'de> Deserialize<'de> for ChunkRow {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("ChunkRow", CHUNK_ROW_FIELDS, ChunkRowVisitor)
    }
}
