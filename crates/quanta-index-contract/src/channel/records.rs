use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::RepoRelativePath;
use crate::lex::{LangId, SymbolKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChunkRecord {
    pub repo_relative_path: RepoRelativePath,
    pub language: Box<str>,
    pub start_line: u32,
    pub end_line: u32,
    pub snippet: Box<str>,
}

const CHUNK_RECORD_FIELDS: &[&str] = &[
    "repo_relative_path",
    "language",
    "start_line",
    "end_line",
    "snippet",
];

impl Serialize for ChunkRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("ChunkRecord", 5)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("language", self.language.as_ref())?;
        state.serialize_field("start_line", &self.start_line)?;
        state.serialize_field("end_line", &self.end_line)?;
        state.serialize_field("snippet", self.snippet.as_ref())?;
        state.end()
    }
}

struct ChunkRecordVisitor;

impl<'de> Visitor<'de> for ChunkRecordVisitor {
    type Value = ChunkRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a ChunkRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut language: Option<String> = None;
        let mut start_line: Option<u32> = None;
        let mut end_line: Option<u32> = None;
        let mut snippet: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_relative_path" => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(map.next_value()?);
                }
                "language" => {
                    if language.is_some() {
                        return Err(de::Error::duplicate_field("language"));
                    }
                    language = Some(map.next_value()?);
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
                "snippet" => {
                    if snippet.is_some() {
                        return Err(de::Error::duplicate_field("snippet"));
                    }
                    snippet = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, CHUNK_RECORD_FIELDS)),
            }
        }
        Ok(ChunkRecord {
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            language: language
                .ok_or_else(|| de::Error::missing_field("language"))?
                .into_boxed_str(),
            start_line: start_line.ok_or_else(|| de::Error::missing_field("start_line"))?,
            end_line: end_line.ok_or_else(|| de::Error::missing_field("end_line"))?,
            snippet: snippet
                .ok_or_else(|| de::Error::missing_field("snippet"))?
                .into_boxed_str(),
        })
    }
}

impl<'de> Deserialize<'de> for ChunkRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("ChunkRecord", CHUNK_RECORD_FIELDS, ChunkRecordVisitor)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmbeddingRecord {
    pub owner_kind: Box<str>,
    pub owner_id: Box<str>,
    pub repo_relative_path: RepoRelativePath,
    pub language: LangId,
    pub symbol_kind: Option<SymbolKind>,
    pub start_line: u32,
    pub end_line: u32,
    pub snippet: Box<str>,
    pub vector: Vec<f32>,
}

const EMBEDDING_RECORD_FIELDS: &[&str] = &[
    "owner_kind",
    "owner_id",
    "repo_relative_path",
    "language",
    "symbol_kind",
    "start_line",
    "end_line",
    "snippet",
    "vector",
];

impl Serialize for EmbeddingRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("EmbeddingRecord", 9)?;
        state.serialize_field("owner_kind", self.owner_kind.as_ref())?;
        state.serialize_field("owner_id", self.owner_id.as_ref())?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("language", &self.language)?;
        state.serialize_field("symbol_kind", &self.symbol_kind)?;
        state.serialize_field("start_line", &self.start_line)?;
        state.serialize_field("end_line", &self.end_line)?;
        state.serialize_field("snippet", self.snippet.as_ref())?;
        state.serialize_field("vector", &self.vector)?;
        state.end()
    }
}

struct EmbeddingRecordVisitor;

impl<'de> Visitor<'de> for EmbeddingRecordVisitor {
    type Value = EmbeddingRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an EmbeddingRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut owner_kind: Option<String> = None;
        let mut owner_id: Option<String> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut language: Option<LangId> = None;
        let mut symbol_kind: Option<Option<SymbolKind>> = None;
        let mut start_line: Option<u32> = None;
        let mut end_line: Option<u32> = None;
        let mut snippet: Option<String> = None;
        let mut vector: Option<Vec<f32>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "owner_kind" => {
                    if owner_kind.is_some() {
                        return Err(de::Error::duplicate_field("owner_kind"));
                    }
                    owner_kind = Some(map.next_value()?);
                }
                "owner_id" => {
                    if owner_id.is_some() {
                        return Err(de::Error::duplicate_field("owner_id"));
                    }
                    owner_id = Some(map.next_value()?);
                }
                "repo_relative_path" => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(map.next_value()?);
                }
                "language" => {
                    if language.is_some() {
                        return Err(de::Error::duplicate_field("language"));
                    }
                    language = Some(map.next_value()?);
                }
                "symbol_kind" => {
                    if symbol_kind.is_some() {
                        return Err(de::Error::duplicate_field("symbol_kind"));
                    }
                    symbol_kind = Some(map.next_value()?);
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
                "snippet" => {
                    if snippet.is_some() {
                        return Err(de::Error::duplicate_field("snippet"));
                    }
                    snippet = Some(map.next_value()?);
                }
                "vector" => {
                    if vector.is_some() {
                        return Err(de::Error::duplicate_field("vector"));
                    }
                    vector = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, EMBEDDING_RECORD_FIELDS)),
            }
        }
        Ok(EmbeddingRecord {
            owner_kind: owner_kind
                .ok_or_else(|| de::Error::missing_field("owner_kind"))?
                .into_boxed_str(),
            owner_id: owner_id
                .ok_or_else(|| de::Error::missing_field("owner_id"))?
                .into_boxed_str(),
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            language: language.ok_or_else(|| de::Error::missing_field("language"))?,
            symbol_kind: symbol_kind.ok_or_else(|| de::Error::missing_field("symbol_kind"))?,
            start_line: start_line.ok_or_else(|| de::Error::missing_field("start_line"))?,
            end_line: end_line.ok_or_else(|| de::Error::missing_field("end_line"))?,
            snippet: snippet
                .ok_or_else(|| de::Error::missing_field("snippet"))?
                .into_boxed_str(),
            vector: vector.ok_or_else(|| de::Error::missing_field("vector"))?,
        })
    }
}

impl<'de> Deserialize<'de> for EmbeddingRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "EmbeddingRecord",
            EMBEDDING_RECORD_FIELDS,
            EmbeddingRecordVisitor,
        )
    }
}
