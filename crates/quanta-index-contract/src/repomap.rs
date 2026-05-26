//! `RepoMap` contract surface.
//!
//! Every DTO below has a hand-rolled `serde::Serialize` / `serde::Deserialize`
//! impl. Proc-macro derive is banned workspace-wide (semgrep rule
//! `rust-no-serde-derive`) because derive expansion dominates cold-build time
//! and hides the wire shape from review. The manual impls mirror the pattern
//! used by `crate::ipc::envelopes`: `serialize_struct` in field-declaration
//! order, a `Visitor::visit_map` deserializer that rejects unknown fields and
//! duplicate fields fail-closed, and `missing_field` errors for any required
//! field absent from the wire.

use core::fmt;
use std::collections::BTreeMap;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, VariantAccess, Visitor},
    ser::SerializeStruct,
};

use crate::lex::{LanguageCode, SymbolKindCode};
use crate::{ChunkId, FileId, RepoRelativePath, SymbolId};
use quanta_index_contract_base::ids::{ManifestGeneration, RepoId, RevisionId};

macro_rules! repomap_string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $code:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            const VARIANTS: &'static [&'static str] = &[$($code),+];

            #[must_use]
            pub const fn as_code_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $code),+
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_code_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct EnumVisitor;

                impl Visitor<'_> for EnumVisitor {
                    type Value = $name;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str(concat!("a ", stringify!($name), " string"))
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        match value {
                            $($code => Ok($name::$variant),)+
                            other => Err(de::Error::unknown_variant(other, $name::VARIANTS)),
                        }
                    }

                    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        self.visit_str(value.as_str())
                    }
                }

                deserializer.deserialize_str(EnumVisitor)
            }
        }
    };
}

repomap_string_enum! {
    pub enum RepoMapDocType {
        File => "File",
        Module => "Module",
        Symbol => "Symbol",
        Chunk => "Chunk",
    }
}

repomap_string_enum! {
    pub enum RepoMapEdgeKind {
        Call => "Call",
        Import => "Import",
        Contains => "Contains",
        OwnsChunk => "OwnsChunk",
        DependsOn => "DependsOn",
    }
}

repomap_string_enum! {
    pub enum RepoMapChunkExactness {
        Exact => "Exact",
        Approximate => "Approximate",
    }
}

repomap_string_enum! {
    pub enum RepoMapItemIndexAvailability {
        Unavailable => "Unavailable",
        Partial => "Partial",
        Available => "Available",
        Full => "Full",
    }
}

repomap_string_enum! {
    pub enum RepoMapGraphCoverageClass {
        Partial => "Partial",
        Complete => "Complete",
        Full => "Full",
    }
}

repomap_string_enum! {
    pub enum RepoMapExactnessSummary {
        Approximate => "Approximate",
        Mixed => "Mixed",
        Exact => "Exact",
    }
}

repomap_string_enum! {
    pub enum RepoMapRedactionState {
        Unredacted => "Unredacted",
        Redacted => "Redacted",
    }
}

string_newtype!(RepoMapModuleId);

fn reject_empty_string<E>(field: &'static str, value: &str) -> Result<(), E>
where
    E: serde::ser::Error,
{
    if value.is_empty() {
        return Err(E::custom(format!("{field} must not be empty")));
    }
    Ok(())
}

fn require_non_empty_string<E>(field: &'static str, value: String) -> Result<String, E>
where
    E: de::Error,
{
    if value.is_empty() {
        return Err(E::custom(format!("{field} must not be empty")));
    }
    Ok(value)
}

// Legacy flat repomap DTOs remain only for local serde regression coverage; the
// public authority surface is the typed graph snapshot carried by
// RepoMapSourceBundle.
#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct RepoMapSymbolRecordDto {
    pub subject_identity: String,
    pub subject_doc_type: RepoMapDocType,
    pub subject_kind: String,
    pub symbol_name: String,
    pub owner_path: String,
}

#[cfg(test)]
const REPOMAP_SYMBOL_RECORD_DTO_V1_FIELDS: &[&str] = &[
    "subject_identity",
    "subject_doc_type",
    "subject_kind",
    "symbol_name",
    "owner_path",
];

#[cfg(test)]
impl Serialize for RepoMapSymbolRecordDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        reject_empty_string::<S::Error>("subject_identity", self.subject_identity.as_str())?;
        reject_empty_string::<S::Error>("subject_kind", self.subject_kind.as_str())?;
        reject_empty_string::<S::Error>("symbol_name", self.symbol_name.as_str())?;
        reject_empty_string::<S::Error>("owner_path", self.owner_path.as_str())?;
        let mut state = serializer.serialize_struct("RepoMapSymbolRecordDto", 5)?;
        state.serialize_field("subject_identity", &self.subject_identity)?;
        state.serialize_field("subject_doc_type", &self.subject_doc_type)?;
        state.serialize_field("subject_kind", &self.subject_kind)?;
        state.serialize_field("symbol_name", &self.symbol_name)?;
        state.serialize_field("owner_path", &self.owner_path)?;
        state.end()
    }
}

#[cfg(test)]
struct RepoMapSymbolRecordDtoV1Visitor;

#[cfg(test)]
impl<'de> Visitor<'de> for RepoMapSymbolRecordDtoV1Visitor {
    type Value = RepoMapSymbolRecordDto;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapSymbolRecordDto map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut subject_identity: Option<String> = None;
        let mut subject_doc_type: Option<RepoMapDocType> = None;
        let mut subject_kind: Option<String> = None;
        let mut symbol_name: Option<String> = None;
        let mut owner_path: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "subject_identity" => {
                    if subject_identity.is_some() {
                        return Err(de::Error::duplicate_field("subject_identity"));
                    }
                    subject_identity = Some(map.next_value()?);
                }
                "subject_doc_type" => {
                    if subject_doc_type.is_some() {
                        return Err(de::Error::duplicate_field("subject_doc_type"));
                    }
                    subject_doc_type = Some(map.next_value()?);
                }
                "subject_kind" => {
                    if subject_kind.is_some() {
                        return Err(de::Error::duplicate_field("subject_kind"));
                    }
                    subject_kind = Some(map.next_value()?);
                }
                "symbol_name" => {
                    if symbol_name.is_some() {
                        return Err(de::Error::duplicate_field("symbol_name"));
                    }
                    symbol_name = Some(map.next_value()?);
                }
                "owner_path" => {
                    if owner_path.is_some() {
                        return Err(de::Error::duplicate_field("owner_path"));
                    }
                    owner_path = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_SYMBOL_RECORD_DTO_V1_FIELDS,
                    ));
                }
            }
        }
        let subject_identity =
            subject_identity.ok_or_else(|| de::Error::missing_field("subject_identity"))?;
        let subject_doc_type =
            subject_doc_type.ok_or_else(|| de::Error::missing_field("subject_doc_type"))?;
        let subject_identity = require_non_empty_string("subject_identity", subject_identity)?;
        let subject_kind = require_non_empty_string(
            "subject_kind",
            subject_kind.ok_or_else(|| de::Error::missing_field("subject_kind"))?,
        )?;
        let symbol_name = require_non_empty_string(
            "symbol_name",
            symbol_name.ok_or_else(|| de::Error::missing_field("symbol_name"))?,
        )?;
        let owner_path = require_non_empty_string(
            "owner_path",
            owner_path.ok_or_else(|| de::Error::missing_field("owner_path"))?,
        )?;
        Ok(RepoMapSymbolRecordDto {
            subject_identity,
            subject_doc_type,
            subject_kind,
            symbol_name,
            owner_path,
        })
    }
}

#[cfg(test)]
impl<'de> Deserialize<'de> for RepoMapSymbolRecordDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapSymbolRecordDto",
            REPOMAP_SYMBOL_RECORD_DTO_V1_FIELDS,
            RepoMapSymbolRecordDtoV1Visitor,
        )
    }
}
#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct RepoMapFileIndexRecord {
    pub file_identity: String,
    pub file_path: String,
    pub file_kind: String,
    pub line_count: u32,
    pub symbol_records: Vec<RepoMapSymbolRecordDto>,
}

#[cfg(test)]
const REPOMAP_FILE_INDEX_RECORD_V1_FIELDS: &[&str] = &[
    "file_identity",
    "file_path",
    "file_kind",
    "line_count",
    "symbol_records",
];

#[cfg(test)]
impl Serialize for RepoMapFileIndexRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapFileIndexRecord", 5)?;
        state.serialize_field("file_identity", &self.file_identity)?;
        state.serialize_field("file_path", &self.file_path)?;
        state.serialize_field("file_kind", &self.file_kind)?;
        state.serialize_field("line_count", &self.line_count)?;
        state.serialize_field("symbol_records", &self.symbol_records)?;
        state.end()
    }
}

#[cfg(test)]
struct RepoMapFileIndexRecordV1Visitor;

#[cfg(test)]
impl<'de> Visitor<'de> for RepoMapFileIndexRecordV1Visitor {
    type Value = RepoMapFileIndexRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapFileIndexRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut file_identity: Option<String> = None;
        let mut file_path: Option<String> = None;
        let mut file_kind: Option<String> = None;
        let mut line_count: Option<u32> = None;
        let mut symbol_records: Option<Vec<RepoMapSymbolRecordDto>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "file_identity" => {
                    if file_identity.is_some() {
                        return Err(de::Error::duplicate_field("file_identity"));
                    }
                    file_identity = Some(map.next_value()?);
                }
                "file_path" => {
                    if file_path.is_some() {
                        return Err(de::Error::duplicate_field("file_path"));
                    }
                    file_path = Some(map.next_value()?);
                }
                "file_kind" => {
                    if file_kind.is_some() {
                        return Err(de::Error::duplicate_field("file_kind"));
                    }
                    file_kind = Some(map.next_value()?);
                }
                "line_count" => {
                    if line_count.is_some() {
                        return Err(de::Error::duplicate_field("line_count"));
                    }
                    line_count = Some(map.next_value()?);
                }
                "symbol_records" => {
                    if symbol_records.is_some() {
                        return Err(de::Error::duplicate_field("symbol_records"));
                    }
                    symbol_records = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_FILE_INDEX_RECORD_V1_FIELDS,
                    ));
                }
            }
        }
        let file_identity =
            file_identity.ok_or_else(|| de::Error::missing_field("file_identity"))?;
        let file_path = file_path.ok_or_else(|| de::Error::missing_field("file_path"))?;
        let file_kind = file_kind.ok_or_else(|| de::Error::missing_field("file_kind"))?;
        let line_count = line_count.ok_or_else(|| de::Error::missing_field("line_count"))?;
        let symbol_records =
            symbol_records.ok_or_else(|| de::Error::missing_field("symbol_records"))?;
        Ok(RepoMapFileIndexRecord {
            file_identity,
            file_path,
            file_kind,
            line_count,
            symbol_records,
        })
    }
}

#[cfg(test)]
impl<'de> Deserialize<'de> for RepoMapFileIndexRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapFileIndexRecord",
            REPOMAP_FILE_INDEX_RECORD_V1_FIELDS,
            RepoMapFileIndexRecordV1Visitor,
        )
    }
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct RepoMapGraphEdgeDto {
    pub from_identity: String,
    pub to_identity: String,
    pub edge_kind: RepoMapEdgeKind,
}

#[cfg(test)]
const REPOMAP_GRAPH_EDGE_DTO_V1_FIELDS: &[&str] = &["from_identity", "to_identity", "edge_kind"];

#[cfg(test)]
impl Serialize for RepoMapGraphEdgeDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapGraphEdgeDto", 3)?;
        state.serialize_field("from_identity", &self.from_identity)?;
        state.serialize_field("to_identity", &self.to_identity)?;
        state.serialize_field("edge_kind", &self.edge_kind)?;
        state.end()
    }
}

#[cfg(test)]
struct RepoMapGraphEdgeDtoV1Visitor;

#[cfg(test)]
impl<'de> Visitor<'de> for RepoMapGraphEdgeDtoV1Visitor {
    type Value = RepoMapGraphEdgeDto;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapGraphEdgeDto map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut from_identity: Option<String> = None;
        let mut to_identity: Option<String> = None;
        let mut edge_kind: Option<RepoMapEdgeKind> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "from_identity" => {
                    if from_identity.is_some() {
                        return Err(de::Error::duplicate_field("from_identity"));
                    }
                    from_identity = Some(map.next_value()?);
                }
                "to_identity" => {
                    if to_identity.is_some() {
                        return Err(de::Error::duplicate_field("to_identity"));
                    }
                    to_identity = Some(map.next_value()?);
                }
                "edge_kind" => {
                    if edge_kind.is_some() {
                        return Err(de::Error::duplicate_field("edge_kind"));
                    }
                    edge_kind = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_GRAPH_EDGE_DTO_V1_FIELDS,
                    ));
                }
            }
        }
        let from_identity =
            from_identity.ok_or_else(|| de::Error::missing_field("from_identity"))?;
        let to_identity = to_identity.ok_or_else(|| de::Error::missing_field("to_identity"))?;
        let edge_kind = edge_kind.ok_or_else(|| de::Error::missing_field("edge_kind"))?;
        Ok(RepoMapGraphEdgeDto {
            from_identity,
            to_identity,
            edge_kind,
        })
    }
}

#[cfg(test)]
impl<'de> Deserialize<'de> for RepoMapGraphEdgeDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapGraphEdgeDto",
            REPOMAP_GRAPH_EDGE_DTO_V1_FIELDS,
            RepoMapGraphEdgeDtoV1Visitor,
        )
    }
}

#[cfg(test)]
#[derive(Clone, Debug, Eq, PartialEq)]
struct RepoMapChunkRecordDto {
    pub subject_identity: String,
    pub owner_path: String,
    pub token_count: u32,
    pub preview_text: String,
    pub exactness: RepoMapChunkExactness,
}

#[cfg(test)]
const REPOMAP_CHUNK_RECORD_DTO_V1_FIELDS: &[&str] = &[
    "subject_identity",
    "owner_path",
    "token_count",
    "preview_text",
    "exactness",
];

#[cfg(test)]
impl Serialize for RepoMapChunkRecordDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapChunkRecordDto", 5)?;
        state.serialize_field("subject_identity", &self.subject_identity)?;
        state.serialize_field("owner_path", &self.owner_path)?;
        state.serialize_field("token_count", &self.token_count)?;
        state.serialize_field("preview_text", &self.preview_text)?;
        state.serialize_field("exactness", &self.exactness)?;
        state.end()
    }
}

#[cfg(test)]
struct RepoMapChunkRecordDtoV1Visitor;

#[cfg(test)]
impl<'de> Visitor<'de> for RepoMapChunkRecordDtoV1Visitor {
    type Value = RepoMapChunkRecordDto;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapChunkRecordDto map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut subject_identity: Option<String> = None;
        let mut owner_path: Option<String> = None;
        let mut token_count: Option<u32> = None;
        let mut preview_text: Option<String> = None;
        let mut exactness: Option<RepoMapChunkExactness> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "subject_identity" => {
                    if subject_identity.is_some() {
                        return Err(de::Error::duplicate_field("subject_identity"));
                    }
                    subject_identity = Some(map.next_value()?);
                }
                "owner_path" => {
                    if owner_path.is_some() {
                        return Err(de::Error::duplicate_field("owner_path"));
                    }
                    owner_path = Some(map.next_value()?);
                }
                "token_count" => {
                    if token_count.is_some() {
                        return Err(de::Error::duplicate_field("token_count"));
                    }
                    token_count = Some(map.next_value()?);
                }
                "preview_text" => {
                    if preview_text.is_some() {
                        return Err(de::Error::duplicate_field("preview_text"));
                    }
                    preview_text = Some(map.next_value()?);
                }
                "exactness" => {
                    if exactness.is_some() {
                        return Err(de::Error::duplicate_field("exactness"));
                    }
                    exactness = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_CHUNK_RECORD_DTO_V1_FIELDS,
                    ));
                }
            }
        }
        let subject_identity =
            subject_identity.ok_or_else(|| de::Error::missing_field("subject_identity"))?;
        let owner_path = owner_path.ok_or_else(|| de::Error::missing_field("owner_path"))?;
        let token_count = token_count.ok_or_else(|| de::Error::missing_field("token_count"))?;
        let preview_text = preview_text.ok_or_else(|| de::Error::missing_field("preview_text"))?;
        let exactness = exactness.ok_or_else(|| de::Error::missing_field("exactness"))?;
        Ok(RepoMapChunkRecordDto {
            subject_identity,
            owner_path,
            token_count,
            preview_text,
            exactness,
        })
    }
}

#[cfg(test)]
impl<'de> Deserialize<'de> for RepoMapChunkRecordDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapChunkRecordDto",
            REPOMAP_CHUNK_RECORD_DTO_V1_FIELDS,
            RepoMapChunkRecordDtoV1Visitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapGraphCoverage {
    pub item_index_availability: RepoMapItemIndexAvailability,
    pub graph_coverage_class: RepoMapGraphCoverageClass,
}

const REPOMAP_GRAPH_COVERAGE_FIELDS: &[&str] = &["item_index_availability", "graph_coverage_class"];

impl Serialize for RepoMapGraphCoverage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapGraphCoverage", 2)?;
        state.serialize_field("item_index_availability", &self.item_index_availability)?;
        state.serialize_field("graph_coverage_class", &self.graph_coverage_class)?;
        state.end()
    }
}

struct RepoMapGraphCoverageVisitor;

impl<'de> Visitor<'de> for RepoMapGraphCoverageVisitor {
    type Value = RepoMapGraphCoverage;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapGraphCoverage map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut item_index_availability: Option<RepoMapItemIndexAvailability> = None;
        let mut graph_coverage_class: Option<RepoMapGraphCoverageClass> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "item_index_availability" => {
                    if item_index_availability.is_some() {
                        return Err(de::Error::duplicate_field("item_index_availability"));
                    }
                    item_index_availability = Some(map.next_value()?);
                }
                "graph_coverage_class" => {
                    if graph_coverage_class.is_some() {
                        return Err(de::Error::duplicate_field("graph_coverage_class"));
                    }
                    graph_coverage_class = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_GRAPH_COVERAGE_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoMapGraphCoverage {
            item_index_availability: item_index_availability
                .ok_or_else(|| de::Error::missing_field("item_index_availability"))?,
            graph_coverage_class: graph_coverage_class
                .ok_or_else(|| de::Error::missing_field("graph_coverage_class"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapGraphCoverage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapGraphCoverage",
            REPOMAP_GRAPH_COVERAGE_FIELDS,
            RepoMapGraphCoverageVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum RepoMapNodeRef {
    File(FileId),
    Module(RepoMapModuleId),
    Symbol(SymbolId),
    Chunk(ChunkId),
}

const REPOMAP_NODE_REF_VARIANTS: &[&str] = &["File", "Module", "Symbol", "Chunk"];

impl Serialize for RepoMapNodeRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::File(file_id) => {
                serializer.serialize_newtype_variant("RepoMapNodeRef", 0, "File", file_id)
            }
            Self::Module(module_id) => {
                serializer.serialize_newtype_variant("RepoMapNodeRef", 1, "Module", module_id)
            }
            Self::Symbol(symbol_id) => {
                serializer.serialize_newtype_variant("RepoMapNodeRef", 2, "Symbol", symbol_id)
            }
            Self::Chunk(chunk_id) => {
                serializer.serialize_newtype_variant("RepoMapNodeRef", 3, "Chunk", chunk_id)
            }
        }
    }
}

struct RepoMapNodeRefVisitor;

impl<'de> Visitor<'de> for RepoMapNodeRefVisitor {
    type Value = RepoMapNodeRef;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapNodeRef enum")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: de::EnumAccess<'de>,
    {
        let (variant, access) = data.variant::<String>()?;
        match variant.as_str() {
            "File" => Ok(RepoMapNodeRef::File(access.newtype_variant()?)),
            "Module" => Ok(RepoMapNodeRef::Module(access.newtype_variant()?)),
            "Symbol" => Ok(RepoMapNodeRef::Symbol(access.newtype_variant()?)),
            "Chunk" => Ok(RepoMapNodeRef::Chunk(access.newtype_variant()?)),
            other => Err(de::Error::unknown_variant(other, REPOMAP_NODE_REF_VARIANTS)),
        }
    }
}

impl<'de> Deserialize<'de> for RepoMapNodeRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_enum(
            "RepoMapNodeRef",
            REPOMAP_NODE_REF_VARIANTS,
            RepoMapNodeRefVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapFileNode {
    pub file_id: FileId,
    pub repo_relative_path: RepoRelativePath,
    pub line_count: u32,
}

const REPOMAP_FILE_NODE_FIELDS: &[&str] = &["file_id", "repo_relative_path", "line_count"];

impl Serialize for RepoMapFileNode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapFileNode", 3)?;
        state.serialize_field("file_id", &self.file_id)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("line_count", &self.line_count)?;
        state.end()
    }
}

struct RepoMapFileNodeVisitor;

impl<'de> Visitor<'de> for RepoMapFileNodeVisitor {
    type Value = RepoMapFileNode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapFileNode map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut file_id: Option<FileId> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut line_count: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "file_id" => {
                    if file_id.is_some() {
                        return Err(de::Error::duplicate_field("file_id"));
                    }
                    file_id = Some(map.next_value()?);
                }
                "repo_relative_path" => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(map.next_value()?);
                }
                "line_count" => {
                    if line_count.is_some() {
                        return Err(de::Error::duplicate_field("line_count"));
                    }
                    line_count = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, REPOMAP_FILE_NODE_FIELDS)),
            }
        }
        Ok(RepoMapFileNode {
            file_id: file_id.ok_or_else(|| de::Error::missing_field("file_id"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            line_count: line_count.ok_or_else(|| de::Error::missing_field("line_count"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapFileNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapFileNode",
            REPOMAP_FILE_NODE_FIELDS,
            RepoMapFileNodeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapModuleNode {
    pub module_id: RepoMapModuleId,
    pub repo_relative_path: RepoRelativePath,
    pub qualified_name: String,
}

const REPOMAP_MODULE_NODE_FIELDS: &[&str] = &["module_id", "repo_relative_path", "qualified_name"];

impl Serialize for RepoMapModuleNode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapModuleNode", 3)?;
        state.serialize_field("module_id", &self.module_id)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("qualified_name", &self.qualified_name)?;
        state.end()
    }
}

struct RepoMapModuleNodeVisitor;

impl<'de> Visitor<'de> for RepoMapModuleNodeVisitor {
    type Value = RepoMapModuleNode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapModuleNode map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut module_id: Option<RepoMapModuleId> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut qualified_name: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "module_id" => {
                    if module_id.is_some() {
                        return Err(de::Error::duplicate_field("module_id"));
                    }
                    module_id = Some(map.next_value()?);
                }
                "repo_relative_path" => {
                    if repo_relative_path.is_some() {
                        return Err(de::Error::duplicate_field("repo_relative_path"));
                    }
                    repo_relative_path = Some(map.next_value()?);
                }
                "qualified_name" => {
                    if qualified_name.is_some() {
                        return Err(de::Error::duplicate_field("qualified_name"));
                    }
                    qualified_name = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, REPOMAP_MODULE_NODE_FIELDS)),
            }
        }
        Ok(RepoMapModuleNode {
            module_id: module_id.ok_or_else(|| de::Error::missing_field("module_id"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            qualified_name: qualified_name
                .ok_or_else(|| de::Error::missing_field("qualified_name"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapModuleNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapModuleNode",
            REPOMAP_MODULE_NODE_FIELDS,
            RepoMapModuleNodeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapSymbolNode {
    pub symbol_id: SymbolId,
    pub owner_path: RepoRelativePath,
    pub local_name: String,
    pub qualified_name: String,
    pub symbol_kind: SymbolKindCode,
}

const REPOMAP_SYMBOL_NODE_FIELDS: &[&str] = &[
    "symbol_id",
    "owner_path",
    "local_name",
    "qualified_name",
    "symbol_kind",
];

impl Serialize for RepoMapSymbolNode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapSymbolNode", 5)?;
        state.serialize_field("symbol_id", &self.symbol_id)?;
        state.serialize_field("owner_path", &self.owner_path)?;
        state.serialize_field("local_name", &self.local_name)?;
        state.serialize_field("qualified_name", &self.qualified_name)?;
        state.serialize_field("symbol_kind", &self.symbol_kind)?;
        state.end()
    }
}

struct RepoMapSymbolNodeVisitor;

impl<'de> Visitor<'de> for RepoMapSymbolNodeVisitor {
    type Value = RepoMapSymbolNode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapSymbolNode map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut symbol_id: Option<SymbolId> = None;
        let mut owner_path: Option<RepoRelativePath> = None;
        let mut local_name: Option<String> = None;
        let mut qualified_name: Option<String> = None;
        let mut symbol_kind: Option<SymbolKindCode> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "symbol_id" => {
                    if symbol_id.is_some() {
                        return Err(de::Error::duplicate_field("symbol_id"));
                    }
                    symbol_id = Some(map.next_value()?);
                }
                "owner_path" => {
                    if owner_path.is_some() {
                        return Err(de::Error::duplicate_field("owner_path"));
                    }
                    owner_path = Some(map.next_value()?);
                }
                "local_name" => {
                    if local_name.is_some() {
                        return Err(de::Error::duplicate_field("local_name"));
                    }
                    local_name = Some(map.next_value()?);
                }
                "qualified_name" => {
                    if qualified_name.is_some() {
                        return Err(de::Error::duplicate_field("qualified_name"));
                    }
                    qualified_name = Some(map.next_value()?);
                }
                "symbol_kind" => {
                    if symbol_kind.is_some() {
                        return Err(de::Error::duplicate_field("symbol_kind"));
                    }
                    symbol_kind = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, REPOMAP_SYMBOL_NODE_FIELDS)),
            }
        }
        Ok(RepoMapSymbolNode {
            symbol_id: symbol_id.ok_or_else(|| de::Error::missing_field("symbol_id"))?,
            owner_path: owner_path.ok_or_else(|| de::Error::missing_field("owner_path"))?,
            local_name: local_name.ok_or_else(|| de::Error::missing_field("local_name"))?,
            qualified_name: qualified_name
                .ok_or_else(|| de::Error::missing_field("qualified_name"))?,
            symbol_kind: symbol_kind.ok_or_else(|| de::Error::missing_field("symbol_kind"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapSymbolNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapSymbolNode",
            REPOMAP_SYMBOL_NODE_FIELDS,
            RepoMapSymbolNodeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapChunkNode {
    pub chunk_id: ChunkId,
    pub owner_path: RepoRelativePath,
    pub language: LanguageCode,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub end_line: u32,
    pub token_count: u32,
    pub preview_text: String,
    pub exactness: RepoMapChunkExactness,
}

const REPOMAP_CHUNK_NODE_FIELDS: &[&str] = &[
    "chunk_id",
    "owner_path",
    "language",
    "start_byte",
    "end_byte",
    "start_line",
    "end_line",
    "token_count",
    "preview_text",
    "exactness",
];

impl Serialize for RepoMapChunkNode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapChunkNode", 10)?;
        state.serialize_field("chunk_id", &self.chunk_id)?;
        state.serialize_field("owner_path", &self.owner_path)?;
        state.serialize_field("language", &self.language)?;
        state.serialize_field("start_byte", &self.start_byte)?;
        state.serialize_field("end_byte", &self.end_byte)?;
        state.serialize_field("start_line", &self.start_line)?;
        state.serialize_field("end_line", &self.end_line)?;
        state.serialize_field("token_count", &self.token_count)?;
        state.serialize_field("preview_text", &self.preview_text)?;
        state.serialize_field("exactness", &self.exactness)?;
        state.end()
    }
}

struct RepoMapChunkNodeVisitor;

impl<'de> Visitor<'de> for RepoMapChunkNodeVisitor {
    type Value = RepoMapChunkNode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapChunkNode map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut chunk_id: Option<ChunkId> = None;
        let mut owner_path: Option<RepoRelativePath> = None;
        let mut language: Option<LanguageCode> = None;
        let mut start_byte: Option<u32> = None;
        let mut end_byte: Option<u32> = None;
        let mut start_line: Option<u32> = None;
        let mut end_line: Option<u32> = None;
        let mut token_count: Option<u32> = None;
        let mut preview_text: Option<String> = None;
        let mut exactness: Option<RepoMapChunkExactness> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "chunk_id" => {
                    if chunk_id.is_some() {
                        return Err(de::Error::duplicate_field("chunk_id"));
                    }
                    chunk_id = Some(map.next_value()?);
                }
                "owner_path" => {
                    if owner_path.is_some() {
                        return Err(de::Error::duplicate_field("owner_path"));
                    }
                    owner_path = Some(map.next_value()?);
                }
                "language" => {
                    if language.is_some() {
                        return Err(de::Error::duplicate_field("language"));
                    }
                    language = Some(map.next_value()?);
                }
                "start_byte" => {
                    if start_byte.is_some() {
                        return Err(de::Error::duplicate_field("start_byte"));
                    }
                    start_byte = Some(map.next_value()?);
                }
                "end_byte" => {
                    if end_byte.is_some() {
                        return Err(de::Error::duplicate_field("end_byte"));
                    }
                    end_byte = Some(map.next_value()?);
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
                "token_count" => {
                    if token_count.is_some() {
                        return Err(de::Error::duplicate_field("token_count"));
                    }
                    token_count = Some(map.next_value()?);
                }
                "preview_text" => {
                    if preview_text.is_some() {
                        return Err(de::Error::duplicate_field("preview_text"));
                    }
                    preview_text = Some(map.next_value()?);
                }
                "exactness" => {
                    if exactness.is_some() {
                        return Err(de::Error::duplicate_field("exactness"));
                    }
                    exactness = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, REPOMAP_CHUNK_NODE_FIELDS)),
            }
        }
        Ok(RepoMapChunkNode {
            chunk_id: chunk_id.ok_or_else(|| de::Error::missing_field("chunk_id"))?,
            owner_path: owner_path.ok_or_else(|| de::Error::missing_field("owner_path"))?,
            language: language.ok_or_else(|| de::Error::missing_field("language"))?,
            start_byte: start_byte.ok_or_else(|| de::Error::missing_field("start_byte"))?,
            end_byte: end_byte.ok_or_else(|| de::Error::missing_field("end_byte"))?,
            start_line: start_line.ok_or_else(|| de::Error::missing_field("start_line"))?,
            end_line: end_line.ok_or_else(|| de::Error::missing_field("end_line"))?,
            token_count: token_count.ok_or_else(|| de::Error::missing_field("token_count"))?,
            preview_text: preview_text.ok_or_else(|| de::Error::missing_field("preview_text"))?,
            exactness: exactness.ok_or_else(|| de::Error::missing_field("exactness"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapChunkNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapChunkNode",
            REPOMAP_CHUNK_NODE_FIELDS,
            RepoMapChunkNodeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepoMapNode {
    File(RepoMapFileNode),
    Module(RepoMapModuleNode),
    Symbol(RepoMapSymbolNode),
    Chunk(RepoMapChunkNode),
}

const REPOMAP_NODE_VARIANTS: &[&str] = &["File", "Module", "Symbol", "Chunk"];

impl Serialize for RepoMapNode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::File(node) => {
                serializer.serialize_newtype_variant("RepoMapNode", 0, "File", node)
            }
            Self::Module(node) => {
                serializer.serialize_newtype_variant("RepoMapNode", 1, "Module", node)
            }
            Self::Symbol(node) => {
                serializer.serialize_newtype_variant("RepoMapNode", 2, "Symbol", node)
            }
            Self::Chunk(node) => {
                serializer.serialize_newtype_variant("RepoMapNode", 3, "Chunk", node)
            }
        }
    }
}

struct RepoMapNodeVisitor;

impl<'de> Visitor<'de> for RepoMapNodeVisitor {
    type Value = RepoMapNode;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapNode enum")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: de::EnumAccess<'de>,
    {
        let (variant, access) = data.variant::<String>()?;
        match variant.as_str() {
            "File" => Ok(RepoMapNode::File(access.newtype_variant()?)),
            "Module" => Ok(RepoMapNode::Module(access.newtype_variant()?)),
            "Symbol" => Ok(RepoMapNode::Symbol(access.newtype_variant()?)),
            "Chunk" => Ok(RepoMapNode::Chunk(access.newtype_variant()?)),
            other => Err(de::Error::unknown_variant(other, REPOMAP_NODE_VARIANTS)),
        }
    }
}

impl<'de> Deserialize<'de> for RepoMapNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_enum("RepoMapNode", REPOMAP_NODE_VARIANTS, RepoMapNodeVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapContainsEdge {
    pub container: RepoMapNodeRef,
    pub contained: RepoMapNodeRef,
}

const REPOMAP_CONTAINS_EDGE_FIELDS: &[&str] = &["container", "contained"];

impl Serialize for RepoMapContainsEdge {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapContainsEdge", 2)?;
        state.serialize_field("container", &self.container)?;
        state.serialize_field("contained", &self.contained)?;
        state.end()
    }
}

struct RepoMapContainsEdgeVisitor;

impl<'de> Visitor<'de> for RepoMapContainsEdgeVisitor {
    type Value = RepoMapContainsEdge;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapContainsEdge map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut container_node: Option<RepoMapNodeRef> = None;
        let mut child_node: Option<RepoMapNodeRef> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "container" => {
                    if container_node.is_some() {
                        return Err(de::Error::duplicate_field("container"));
                    }
                    container_node = Some(map.next_value()?);
                }
                "contained" => {
                    if child_node.is_some() {
                        return Err(de::Error::duplicate_field("contained"));
                    }
                    child_node = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_CONTAINS_EDGE_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoMapContainsEdge {
            container: container_node.ok_or_else(|| de::Error::missing_field("container"))?,
            contained: child_node.ok_or_else(|| de::Error::missing_field("contained"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapContainsEdge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapContainsEdge",
            REPOMAP_CONTAINS_EDGE_FIELDS,
            RepoMapContainsEdgeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapCallEdge {
    pub caller: RepoMapNodeRef,
    pub callee: RepoMapNodeRef,
}

const REPOMAP_CALL_EDGE_FIELDS: &[&str] = &["caller", "callee"];

impl Serialize for RepoMapCallEdge {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapCallEdge", 2)?;
        state.serialize_field("caller", &self.caller)?;
        state.serialize_field("callee", &self.callee)?;
        state.end()
    }
}

struct RepoMapCallEdgeVisitor;

impl<'de> Visitor<'de> for RepoMapCallEdgeVisitor {
    type Value = RepoMapCallEdge;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapCallEdge map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_node: Option<RepoMapNodeRef> = None;
        let mut target_node: Option<RepoMapNodeRef> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "caller" => {
                    if source_node.is_some() {
                        return Err(de::Error::duplicate_field("caller"));
                    }
                    source_node = Some(map.next_value()?);
                }
                "callee" => {
                    if target_node.is_some() {
                        return Err(de::Error::duplicate_field("callee"));
                    }
                    target_node = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, REPOMAP_CALL_EDGE_FIELDS)),
            }
        }
        Ok(RepoMapCallEdge {
            caller: source_node.ok_or_else(|| de::Error::missing_field("caller"))?,
            callee: target_node.ok_or_else(|| de::Error::missing_field("callee"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapCallEdge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapCallEdge",
            REPOMAP_CALL_EDGE_FIELDS,
            RepoMapCallEdgeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapImportEdge {
    pub importer: RepoMapNodeRef,
    pub imported: RepoMapNodeRef,
}

const REPOMAP_IMPORT_EDGE_FIELDS: &[&str] = &["importer", "imported"];

impl Serialize for RepoMapImportEdge {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapImportEdge", 2)?;
        state.serialize_field("importer", &self.importer)?;
        state.serialize_field("imported", &self.imported)?;
        state.end()
    }
}

struct RepoMapImportEdgeVisitor;

impl<'de> Visitor<'de> for RepoMapImportEdgeVisitor {
    type Value = RepoMapImportEdge;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapImportEdge map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut importing_node: Option<RepoMapNodeRef> = None;
        let mut import_target: Option<RepoMapNodeRef> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "importer" => {
                    if importing_node.is_some() {
                        return Err(de::Error::duplicate_field("importer"));
                    }
                    importing_node = Some(map.next_value()?);
                }
                "imported" => {
                    if import_target.is_some() {
                        return Err(de::Error::duplicate_field("imported"));
                    }
                    import_target = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, REPOMAP_IMPORT_EDGE_FIELDS)),
            }
        }
        Ok(RepoMapImportEdge {
            importer: importing_node.ok_or_else(|| de::Error::missing_field("importer"))?,
            imported: import_target.ok_or_else(|| de::Error::missing_field("imported"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapImportEdge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapImportEdge",
            REPOMAP_IMPORT_EDGE_FIELDS,
            RepoMapImportEdgeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapOwnsChunkEdge {
    pub owner: RepoMapNodeRef,
    pub chunk: RepoMapNodeRef,
}

const REPOMAP_OWNS_CHUNK_EDGE_FIELDS: &[&str] = &["owner", "chunk"];

impl Serialize for RepoMapOwnsChunkEdge {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapOwnsChunkEdge", 2)?;
        state.serialize_field("owner", &self.owner)?;
        state.serialize_field("chunk", &self.chunk)?;
        state.end()
    }
}

struct RepoMapOwnsChunkEdgeVisitor;

impl<'de> Visitor<'de> for RepoMapOwnsChunkEdgeVisitor {
    type Value = RepoMapOwnsChunkEdge;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapOwnsChunkEdge map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut owner: Option<RepoMapNodeRef> = None;
        let mut chunk: Option<RepoMapNodeRef> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "owner" => {
                    if owner.is_some() {
                        return Err(de::Error::duplicate_field("owner"));
                    }
                    owner = Some(map.next_value()?);
                }
                "chunk" => {
                    if chunk.is_some() {
                        return Err(de::Error::duplicate_field("chunk"));
                    }
                    chunk = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_OWNS_CHUNK_EDGE_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoMapOwnsChunkEdge {
            owner: owner.ok_or_else(|| de::Error::missing_field("owner"))?,
            chunk: chunk.ok_or_else(|| de::Error::missing_field("chunk"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapOwnsChunkEdge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapOwnsChunkEdge",
            REPOMAP_OWNS_CHUNK_EDGE_FIELDS,
            RepoMapOwnsChunkEdgeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapDependsOnEdge {
    pub dependent: RepoMapNodeRef,
    pub dependency: RepoMapNodeRef,
}

const REPOMAP_DEPENDS_ON_EDGE_FIELDS: &[&str] = &["dependent", "dependency"];

impl Serialize for RepoMapDependsOnEdge {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapDependsOnEdge", 2)?;
        state.serialize_field("dependent", &self.dependent)?;
        state.serialize_field("dependency", &self.dependency)?;
        state.end()
    }
}

struct RepoMapDependsOnEdgeVisitor;

impl<'de> Visitor<'de> for RepoMapDependsOnEdgeVisitor {
    type Value = RepoMapDependsOnEdge;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapDependsOnEdge map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut dependent: Option<RepoMapNodeRef> = None;
        let mut dependency: Option<RepoMapNodeRef> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "dependent" => {
                    if dependent.is_some() {
                        return Err(de::Error::duplicate_field("dependent"));
                    }
                    dependent = Some(map.next_value()?);
                }
                "dependency" => {
                    if dependency.is_some() {
                        return Err(de::Error::duplicate_field("dependency"));
                    }
                    dependency = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_DEPENDS_ON_EDGE_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoMapDependsOnEdge {
            dependent: dependent.ok_or_else(|| de::Error::missing_field("dependent"))?,
            dependency: dependency.ok_or_else(|| de::Error::missing_field("dependency"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapDependsOnEdge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapDependsOnEdge",
            REPOMAP_DEPENDS_ON_EDGE_FIELDS,
            RepoMapDependsOnEdgeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RepoMapEdge {
    Contains(RepoMapContainsEdge),
    Call(RepoMapCallEdge),
    Import(RepoMapImportEdge),
    OwnsChunk(RepoMapOwnsChunkEdge),
    DependsOn(RepoMapDependsOnEdge),
}

const REPOMAP_EDGE_VARIANTS: &[&str] = &["Contains", "Call", "Import", "OwnsChunk", "DependsOn"];

impl Serialize for RepoMapEdge {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Contains(edge) => {
                serializer.serialize_newtype_variant("RepoMapEdge", 0, "Contains", edge)
            }
            Self::Call(edge) => {
                serializer.serialize_newtype_variant("RepoMapEdge", 1, "Call", edge)
            }
            Self::Import(edge) => {
                serializer.serialize_newtype_variant("RepoMapEdge", 2, "Import", edge)
            }
            Self::OwnsChunk(edge) => {
                serializer.serialize_newtype_variant("RepoMapEdge", 3, "OwnsChunk", edge)
            }
            Self::DependsOn(edge) => {
                serializer.serialize_newtype_variant("RepoMapEdge", 4, "DependsOn", edge)
            }
        }
    }
}

struct RepoMapEdgeVisitor;

impl<'de> Visitor<'de> for RepoMapEdgeVisitor {
    type Value = RepoMapEdge;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapEdge enum")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: de::EnumAccess<'de>,
    {
        let (variant, access) = data.variant::<String>()?;
        match variant.as_str() {
            "Contains" => Ok(RepoMapEdge::Contains(access.newtype_variant()?)),
            "Call" => Ok(RepoMapEdge::Call(access.newtype_variant()?)),
            "Import" => Ok(RepoMapEdge::Import(access.newtype_variant()?)),
            "OwnsChunk" => Ok(RepoMapEdge::OwnsChunk(access.newtype_variant()?)),
            "DependsOn" => Ok(RepoMapEdge::DependsOn(access.newtype_variant()?)),
            other => Err(de::Error::unknown_variant(other, REPOMAP_EDGE_VARIANTS)),
        }
    }
}

impl<'de> Deserialize<'de> for RepoMapEdge {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_enum("RepoMapEdge", REPOMAP_EDGE_VARIANTS, RepoMapEdgeVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapSourceBundle {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
    pub snapshot_id: String,
    pub projection_version: u32,
    pub authority_digest: String,
    pub graph_coverage: RepoMapGraphCoverage,
    pub exactness_summary: RepoMapExactnessSummary,
    pub redaction_state: RepoMapRedactionState,
    pub nodes: Vec<RepoMapNode>,
    pub edges: Vec<RepoMapEdge>,
}

impl RepoMapSourceBundle {
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "constructor mirrors the stable source-bundle contract fields before node and edge accumulation begins"
    )]
    pub fn new(
        repo_id: RepoId,
        revision_id: RevisionId,
        manifest_generation: ManifestGeneration,
        manifest_digest: impl Into<String>,
        snapshot_id: impl Into<String>,
        projection_version: u32,
        authority_digest: impl Into<String>,
        graph_coverage: RepoMapGraphCoverage,
        exactness_summary: RepoMapExactnessSummary,
        redaction_state: RepoMapRedactionState,
    ) -> Self {
        Self {
            repo_id,
            revision_id,
            manifest_generation,
            manifest_digest: manifest_digest.into(),
            snapshot_id: snapshot_id.into(),
            projection_version,
            authority_digest: authority_digest.into(),
            graph_coverage,
            exactness_summary,
            redaction_state,
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_node(mut self, node: RepoMapNode) -> Self {
        self.nodes.push(node);
        self
    }

    #[must_use]
    pub fn with_edge(mut self, edge: RepoMapEdge) -> Self {
        self.edges.push(edge);
        self
    }
}

const REPOMAP_SOURCE_BUNDLE_V1_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "manifest_digest",
    "snapshot_id",
    "projection_version",
    "authority_digest",
    "graph_coverage",
    "exactness_summary",
    "redaction_state",
    "nodes",
    "edges",
];

impl Serialize for RepoMapSourceBundle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapSourceBundle", 12)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("snapshot_id", &self.snapshot_id)?;
        state.serialize_field("projection_version", &self.projection_version)?;
        state.serialize_field("authority_digest", &self.authority_digest)?;
        state.serialize_field("graph_coverage", &self.graph_coverage)?;
        state.serialize_field("exactness_summary", &self.exactness_summary)?;
        state.serialize_field("redaction_state", &self.redaction_state)?;
        state.serialize_field("nodes", &self.nodes)?;
        state.serialize_field("edges", &self.edges)?;
        state.end()
    }
}

struct RepoMapSourceBundleV1Visitor;

impl<'de> Visitor<'de> for RepoMapSourceBundleV1Visitor {
    type Value = RepoMapSourceBundle;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapSourceBundle map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut manifest_digest: Option<String> = None;
        let mut snapshot_id: Option<String> = None;
        let mut projection_version: Option<u32> = None;
        let mut authority_digest: Option<String> = None;
        let mut graph_coverage: Option<RepoMapGraphCoverage> = None;
        let mut exactness_summary: Option<RepoMapExactnessSummary> = None;
        let mut redaction_state: Option<RepoMapRedactionState> = None;
        let mut nodes: Option<Vec<RepoMapNode>> = None;
        let mut edges: Option<Vec<RepoMapEdge>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
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
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                "snapshot_id" => {
                    if snapshot_id.is_some() {
                        return Err(de::Error::duplicate_field("snapshot_id"));
                    }
                    snapshot_id = Some(map.next_value()?);
                }
                "projection_version" => {
                    if projection_version.is_some() {
                        return Err(de::Error::duplicate_field("projection_version"));
                    }
                    projection_version = Some(map.next_value()?);
                }
                "authority_digest" => {
                    if authority_digest.is_some() {
                        return Err(de::Error::duplicate_field("authority_digest"));
                    }
                    authority_digest = Some(map.next_value()?);
                }
                "graph_coverage" => {
                    if graph_coverage.is_some() {
                        return Err(de::Error::duplicate_field("graph_coverage"));
                    }
                    graph_coverage = Some(map.next_value()?);
                }
                "exactness_summary" => {
                    if exactness_summary.is_some() {
                        return Err(de::Error::duplicate_field("exactness_summary"));
                    }
                    exactness_summary = Some(map.next_value()?);
                }
                "redaction_state" => {
                    if redaction_state.is_some() {
                        return Err(de::Error::duplicate_field("redaction_state"));
                    }
                    redaction_state = Some(map.next_value()?);
                }
                "nodes" => {
                    if nodes.is_some() {
                        return Err(de::Error::duplicate_field("nodes"));
                    }
                    nodes = Some(map.next_value()?);
                }
                "edges" => {
                    if edges.is_some() {
                        return Err(de::Error::duplicate_field("edges"));
                    }
                    edges = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_SOURCE_BUNDLE_V1_FIELDS,
                    ));
                }
            }
        }
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let manifest_generation =
            manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?;
        let manifest_digest =
            manifest_digest.ok_or_else(|| de::Error::missing_field("manifest_digest"))?;
        let snapshot_id = snapshot_id.ok_or_else(|| de::Error::missing_field("snapshot_id"))?;
        let projection_version =
            projection_version.ok_or_else(|| de::Error::missing_field("projection_version"))?;
        let authority_digest =
            authority_digest.ok_or_else(|| de::Error::missing_field("authority_digest"))?;
        let graph_coverage =
            graph_coverage.ok_or_else(|| de::Error::missing_field("graph_coverage"))?;
        let exactness_summary =
            exactness_summary.ok_or_else(|| de::Error::missing_field("exactness_summary"))?;
        let redaction_state =
            redaction_state.ok_or_else(|| de::Error::missing_field("redaction_state"))?;
        let nodes = nodes.ok_or_else(|| de::Error::missing_field("nodes"))?;
        let edges = edges.ok_or_else(|| de::Error::missing_field("edges"))?;
        Ok(RepoMapSourceBundle {
            repo_id,
            revision_id,
            manifest_generation,
            manifest_digest,
            snapshot_id,
            projection_version,
            authority_digest,
            graph_coverage,
            exactness_summary,
            redaction_state,
            nodes,
            edges,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapSourceBundle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapSourceBundle",
            REPOMAP_SOURCE_BUNDLE_V1_FIELDS,
            RepoMapSourceBundleV1Visitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapActivateGenerationRequest {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
}

const REPOMAP_ACTIVATE_GENERATION_REQUEST_V1_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "manifest_digest",
];

impl Serialize for RepoMapActivateGenerationRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapActivateGenerationRequest", 4)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.end()
    }
}

struct RepoMapActivateGenerationRequestV1Visitor;

impl<'de> Visitor<'de> for RepoMapActivateGenerationRequestV1Visitor {
    type Value = RepoMapActivateGenerationRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapActivateGenerationRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut manifest_digest: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
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
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_ACTIVATE_GENERATION_REQUEST_V1_FIELDS,
                    ));
                }
            }
        }
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let manifest_generation =
            manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?;
        let manifest_digest =
            manifest_digest.ok_or_else(|| de::Error::missing_field("manifest_digest"))?;
        Ok(RepoMapActivateGenerationRequest {
            repo_id,
            revision_id,
            manifest_generation,
            manifest_digest,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapActivateGenerationRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapActivateGenerationRequest",
            REPOMAP_ACTIVATE_GENERATION_REQUEST_V1_FIELDS,
            RepoMapActivateGenerationRequestV1Visitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapMutationAck {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
}

const REPOMAP_MUTATION_ACK_V1_FIELDS: &[&str] = &["repo_id", "revision_id", "manifest_generation"];

impl Serialize for RepoMapMutationAck {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapMutationAck", 3)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.end()
    }
}

struct RepoMapMutationAckV1Visitor;

impl<'de> Visitor<'de> for RepoMapMutationAckV1Visitor {
    type Value = RepoMapMutationAck;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapMutationAck map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
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
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_MUTATION_ACK_V1_FIELDS,
                    ));
                }
            }
        }
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let manifest_generation =
            manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?;
        Ok(RepoMapMutationAck {
            repo_id,
            revision_id,
            manifest_generation,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapMutationAck {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapMutationAck",
            REPOMAP_MUTATION_ACK_V1_FIELDS,
            RepoMapMutationAckV1Visitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapSnapshotMeta {
    pub snapshot_id: String,
    pub projection_version: u32,
    pub authority_digest: String,
    pub item_index_availability: RepoMapItemIndexAvailability,
    pub graph_coverage_class: RepoMapGraphCoverageClass,
    pub exactness_summary: RepoMapExactnessSummary,
}

const REPOMAP_SNAPSHOT_META_V1_FIELDS: &[&str] = &[
    "snapshot_id",
    "projection_version",
    "authority_digest",
    "item_index_availability",
    "graph_coverage_class",
    "exactness_summary",
];

impl Serialize for RepoMapSnapshotMeta {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapSnapshotMeta", 6)?;
        state.serialize_field("snapshot_id", &self.snapshot_id)?;
        state.serialize_field("projection_version", &self.projection_version)?;
        state.serialize_field("authority_digest", &self.authority_digest)?;
        state.serialize_field("item_index_availability", &self.item_index_availability)?;
        state.serialize_field("graph_coverage_class", &self.graph_coverage_class)?;
        state.serialize_field("exactness_summary", &self.exactness_summary)?;
        state.end()
    }
}

struct RepoMapSnapshotMetaV1Visitor;

impl<'de> Visitor<'de> for RepoMapSnapshotMetaV1Visitor {
    type Value = RepoMapSnapshotMeta;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapSnapshotMeta map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut snapshot_id: Option<String> = None;
        let mut projection_version: Option<u32> = None;
        let mut authority_digest: Option<String> = None;
        let mut item_index_availability: Option<RepoMapItemIndexAvailability> = None;
        let mut graph_coverage_class: Option<RepoMapGraphCoverageClass> = None;
        let mut exactness_summary: Option<RepoMapExactnessSummary> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "snapshot_id" => {
                    if snapshot_id.is_some() {
                        return Err(de::Error::duplicate_field("snapshot_id"));
                    }
                    snapshot_id = Some(map.next_value()?);
                }
                "projection_version" => {
                    if projection_version.is_some() {
                        return Err(de::Error::duplicate_field("projection_version"));
                    }
                    projection_version = Some(map.next_value()?);
                }
                "authority_digest" => {
                    if authority_digest.is_some() {
                        return Err(de::Error::duplicate_field("authority_digest"));
                    }
                    authority_digest = Some(map.next_value()?);
                }
                "item_index_availability" => {
                    if item_index_availability.is_some() {
                        return Err(de::Error::duplicate_field("item_index_availability"));
                    }
                    item_index_availability = Some(map.next_value()?);
                }
                "graph_coverage_class" => {
                    if graph_coverage_class.is_some() {
                        return Err(de::Error::duplicate_field("graph_coverage_class"));
                    }
                    graph_coverage_class = Some(map.next_value()?);
                }
                "exactness_summary" => {
                    if exactness_summary.is_some() {
                        return Err(de::Error::duplicate_field("exactness_summary"));
                    }
                    exactness_summary = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_SNAPSHOT_META_V1_FIELDS,
                    ));
                }
            }
        }
        let snapshot_id = snapshot_id.ok_or_else(|| de::Error::missing_field("snapshot_id"))?;
        let projection_version =
            projection_version.ok_or_else(|| de::Error::missing_field("projection_version"))?;
        let authority_digest =
            authority_digest.ok_or_else(|| de::Error::missing_field("authority_digest"))?;
        let item_index_availability = item_index_availability
            .ok_or_else(|| de::Error::missing_field("item_index_availability"))?;
        let graph_coverage_class =
            graph_coverage_class.ok_or_else(|| de::Error::missing_field("graph_coverage_class"))?;
        let exactness_summary =
            exactness_summary.ok_or_else(|| de::Error::missing_field("exactness_summary"))?;
        Ok(RepoMapSnapshotMeta {
            snapshot_id,
            projection_version,
            authority_digest,
            item_index_availability,
            graph_coverage_class,
            exactness_summary,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapSnapshotMeta {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapSnapshotMeta",
            REPOMAP_SNAPSHOT_META_V1_FIELDS,
            RepoMapSnapshotMetaV1Visitor,
        )
    }
}

/// Query-scoped subject selector.
///
/// This remains public because query consumers still send focus hints over the
/// query IPC. It is not part of the typed producer publish authority surface,
/// which lives under `RepoMapSourceBundle` / `RepoMapNode` / `RepoMapEdge`.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RepoMapFocusSubjectDto {
    pub subject_identity: String,
    pub subject_doc_type: RepoMapDocType,
}

const REPOMAP_FOCUS_SUBJECT_DTO_V1_FIELDS: &[&str] = &["subject_identity", "subject_doc_type"];

impl Serialize for RepoMapFocusSubjectDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        reject_empty_string::<S::Error>("subject_identity", self.subject_identity.as_str())?;
        let mut state = serializer.serialize_struct("RepoMapFocusSubjectDto", 2)?;
        state.serialize_field("subject_identity", &self.subject_identity)?;
        state.serialize_field("subject_doc_type", &self.subject_doc_type)?;
        state.end()
    }
}

struct RepoMapFocusSubjectDtoV1Visitor;

impl<'de> Visitor<'de> for RepoMapFocusSubjectDtoV1Visitor {
    type Value = RepoMapFocusSubjectDto;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapFocusSubjectDto map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut subject_identity: Option<String> = None;
        let mut subject_doc_type: Option<RepoMapDocType> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "subject_identity" => {
                    if subject_identity.is_some() {
                        return Err(de::Error::duplicate_field("subject_identity"));
                    }
                    subject_identity = Some(map.next_value()?);
                }
                "subject_doc_type" => {
                    if subject_doc_type.is_some() {
                        return Err(de::Error::duplicate_field("subject_doc_type"));
                    }
                    subject_doc_type = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_FOCUS_SUBJECT_DTO_V1_FIELDS,
                    ));
                }
            }
        }
        let subject_identity =
            subject_identity.ok_or_else(|| de::Error::missing_field("subject_identity"))?;
        let subject_doc_type =
            subject_doc_type.ok_or_else(|| de::Error::missing_field("subject_doc_type"))?;
        let subject_identity = require_non_empty_string("subject_identity", subject_identity)?;
        Ok(RepoMapFocusSubjectDto {
            subject_identity,
            subject_doc_type,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapFocusSubjectDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapFocusSubjectDto",
            REPOMAP_FOCUS_SUBJECT_DTO_V1_FIELDS,
            RepoMapFocusSubjectDtoV1Visitor,
        )
    }
}

/// Flat query projection row returned by `RepoMapQueryResponse`.
///
/// The typed producer handoff is already `RepoMapSourceBundle` and its typed
/// node/edge graph. This row shape remains public only because query/search
/// consumers still materialize flat projection entries.
#[derive(Clone, Debug, PartialEq)]
pub struct RepoMapEntryDto {
    pub subject_identity: String,
    pub subject_doc_type: RepoMapDocType,
    pub subject_kind: String,
    pub owner_path: String,
    pub score: f32,
    pub final_score_millis: u32,
    pub included: bool,
    pub rank: u32,
    pub importance_score_millis: u32,
    pub utility_score_millis: u32,
    pub freshness_score_millis: u32,
    pub evidence_priority_millis: u32,
    pub token_budget_hint: u32,
    pub contributing_signals: BTreeMap<String, i64>,
    pub projection_evidence_kind: String,
    pub projection_authority_artifact_id: String,
    pub projection_authority_digest: String,
    pub projection_status: String,
    pub redaction_state: RepoMapRedactionState,
}

const REPOMAP_ENTRY_DTO_V1_FIELDS: &[&str] = &[
    "subject_identity",
    "subject_doc_type",
    "subject_kind",
    "owner_path",
    "score",
    "final_score_millis",
    "included",
    "rank",
    "importance_score_millis",
    "utility_score_millis",
    "freshness_score_millis",
    "evidence_priority_millis",
    "token_budget_hint",
    "contributing_signals",
    "projection_evidence_kind",
    "projection_authority_artifact_id",
    "projection_authority_digest",
    "projection_status",
    "redaction_state",
];

impl Serialize for RepoMapEntryDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        reject_empty_string::<S::Error>("subject_identity", self.subject_identity.as_str())?;
        reject_empty_string::<S::Error>("subject_kind", self.subject_kind.as_str())?;
        reject_empty_string::<S::Error>("owner_path", self.owner_path.as_str())?;
        reject_empty_string::<S::Error>(
            "projection_evidence_kind",
            self.projection_evidence_kind.as_str(),
        )?;
        reject_empty_string::<S::Error>(
            "projection_authority_artifact_id",
            self.projection_authority_artifact_id.as_str(),
        )?;
        reject_empty_string::<S::Error>(
            "projection_authority_digest",
            self.projection_authority_digest.as_str(),
        )?;
        reject_empty_string::<S::Error>("projection_status", self.projection_status.as_str())?;
        let mut state = serializer.serialize_struct("RepoMapEntryDto", 19)?;
        state.serialize_field("subject_identity", &self.subject_identity)?;
        state.serialize_field("subject_doc_type", &self.subject_doc_type)?;
        state.serialize_field("subject_kind", &self.subject_kind)?;
        state.serialize_field("owner_path", &self.owner_path)?;
        state.serialize_field("score", &self.score)?;
        state.serialize_field("final_score_millis", &self.final_score_millis)?;
        state.serialize_field("included", &self.included)?;
        state.serialize_field("rank", &self.rank)?;
        state.serialize_field("importance_score_millis", &self.importance_score_millis)?;
        state.serialize_field("utility_score_millis", &self.utility_score_millis)?;
        state.serialize_field("freshness_score_millis", &self.freshness_score_millis)?;
        state.serialize_field("evidence_priority_millis", &self.evidence_priority_millis)?;
        state.serialize_field("token_budget_hint", &self.token_budget_hint)?;
        state.serialize_field("contributing_signals", &self.contributing_signals)?;
        state.serialize_field("projection_evidence_kind", &self.projection_evidence_kind)?;
        state.serialize_field(
            "projection_authority_artifact_id",
            &self.projection_authority_artifact_id,
        )?;
        state.serialize_field(
            "projection_authority_digest",
            &self.projection_authority_digest,
        )?;
        state.serialize_field("projection_status", &self.projection_status)?;
        state.serialize_field("redaction_state", &self.redaction_state)?;
        state.end()
    }
}

struct RepoMapEntryDtoV1Visitor;

impl<'de> Visitor<'de> for RepoMapEntryDtoV1Visitor {
    type Value = RepoMapEntryDto;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapEntryDto map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut subject_identity: Option<String> = None;
        let mut subject_doc_type: Option<RepoMapDocType> = None;
        let mut subject_kind: Option<String> = None;
        let mut owner_path: Option<String> = None;
        let mut score: Option<f32> = None;
        let mut final_score_millis: Option<u32> = None;
        let mut included: Option<bool> = None;
        let mut rank: Option<u32> = None;
        let mut importance_score_millis: Option<u32> = None;
        let mut utility_score_millis: Option<u32> = None;
        let mut freshness_score_millis: Option<u32> = None;
        let mut evidence_priority_millis: Option<u32> = None;
        let mut token_budget_hint: Option<u32> = None;
        let mut contributing_signals: Option<BTreeMap<String, i64>> = None;
        let mut projection_evidence_kind: Option<String> = None;
        let mut projection_authority_artifact_id: Option<String> = None;
        let mut projection_authority_digest: Option<String> = None;
        let mut projection_status: Option<String> = None;
        let mut redaction_state: Option<RepoMapRedactionState> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "subject_identity" => {
                    if subject_identity.is_some() {
                        return Err(de::Error::duplicate_field("subject_identity"));
                    }
                    subject_identity = Some(map.next_value()?);
                }
                "subject_doc_type" => {
                    if subject_doc_type.is_some() {
                        return Err(de::Error::duplicate_field("subject_doc_type"));
                    }
                    subject_doc_type = Some(map.next_value()?);
                }
                "subject_kind" => {
                    if subject_kind.is_some() {
                        return Err(de::Error::duplicate_field("subject_kind"));
                    }
                    subject_kind = Some(map.next_value()?);
                }
                "owner_path" => {
                    if owner_path.is_some() {
                        return Err(de::Error::duplicate_field("owner_path"));
                    }
                    owner_path = Some(map.next_value()?);
                }
                "score" => {
                    if score.is_some() {
                        return Err(de::Error::duplicate_field("score"));
                    }
                    score = Some(map.next_value()?);
                }
                "final_score_millis" => {
                    if final_score_millis.is_some() {
                        return Err(de::Error::duplicate_field("final_score_millis"));
                    }
                    final_score_millis = Some(map.next_value()?);
                }
                "included" => {
                    if included.is_some() {
                        return Err(de::Error::duplicate_field("included"));
                    }
                    included = Some(map.next_value()?);
                }
                "rank" => {
                    if rank.is_some() {
                        return Err(de::Error::duplicate_field("rank"));
                    }
                    rank = Some(map.next_value()?);
                }
                "importance_score_millis" => {
                    if importance_score_millis.is_some() {
                        return Err(de::Error::duplicate_field("importance_score_millis"));
                    }
                    importance_score_millis = Some(map.next_value()?);
                }
                "utility_score_millis" => {
                    if utility_score_millis.is_some() {
                        return Err(de::Error::duplicate_field("utility_score_millis"));
                    }
                    utility_score_millis = Some(map.next_value()?);
                }
                "freshness_score_millis" => {
                    if freshness_score_millis.is_some() {
                        return Err(de::Error::duplicate_field("freshness_score_millis"));
                    }
                    freshness_score_millis = Some(map.next_value()?);
                }
                "evidence_priority_millis" => {
                    if evidence_priority_millis.is_some() {
                        return Err(de::Error::duplicate_field("evidence_priority_millis"));
                    }
                    evidence_priority_millis = Some(map.next_value()?);
                }
                "token_budget_hint" => {
                    if token_budget_hint.is_some() {
                        return Err(de::Error::duplicate_field("token_budget_hint"));
                    }
                    token_budget_hint = Some(map.next_value()?);
                }
                "contributing_signals" => {
                    if contributing_signals.is_some() {
                        return Err(de::Error::duplicate_field("contributing_signals"));
                    }
                    contributing_signals = Some(map.next_value()?);
                }
                "projection_evidence_kind" => {
                    if projection_evidence_kind.is_some() {
                        return Err(de::Error::duplicate_field("projection_evidence_kind"));
                    }
                    projection_evidence_kind = Some(map.next_value()?);
                }
                "projection_authority_artifact_id" => {
                    if projection_authority_artifact_id.is_some() {
                        return Err(de::Error::duplicate_field(
                            "projection_authority_artifact_id",
                        ));
                    }
                    projection_authority_artifact_id = Some(map.next_value()?);
                }
                "projection_authority_digest" => {
                    if projection_authority_digest.is_some() {
                        return Err(de::Error::duplicate_field("projection_authority_digest"));
                    }
                    projection_authority_digest = Some(map.next_value()?);
                }
                "projection_status" => {
                    if projection_status.is_some() {
                        return Err(de::Error::duplicate_field("projection_status"));
                    }
                    projection_status = Some(map.next_value()?);
                }
                "redaction_state" => {
                    if redaction_state.is_some() {
                        return Err(de::Error::duplicate_field("redaction_state"));
                    }
                    redaction_state = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(other, REPOMAP_ENTRY_DTO_V1_FIELDS));
                }
            }
        }
        let subject_identity =
            subject_identity.ok_or_else(|| de::Error::missing_field("subject_identity"))?;
        let subject_doc_type =
            subject_doc_type.ok_or_else(|| de::Error::missing_field("subject_doc_type"))?;
        let subject_identity = require_non_empty_string("subject_identity", subject_identity)?;
        let subject_kind = require_non_empty_string(
            "subject_kind",
            subject_kind.ok_or_else(|| de::Error::missing_field("subject_kind"))?,
        )?;
        let owner_path = require_non_empty_string(
            "owner_path",
            owner_path.ok_or_else(|| de::Error::missing_field("owner_path"))?,
        )?;
        let score = score.ok_or_else(|| de::Error::missing_field("score"))?;
        let final_score_millis =
            final_score_millis.ok_or_else(|| de::Error::missing_field("final_score_millis"))?;
        let included = included.ok_or_else(|| de::Error::missing_field("included"))?;
        let rank = rank.ok_or_else(|| de::Error::missing_field("rank"))?;
        let importance_score_millis = importance_score_millis
            .ok_or_else(|| de::Error::missing_field("importance_score_millis"))?;
        let utility_score_millis =
            utility_score_millis.ok_or_else(|| de::Error::missing_field("utility_score_millis"))?;
        let freshness_score_millis = freshness_score_millis
            .ok_or_else(|| de::Error::missing_field("freshness_score_millis"))?;
        let evidence_priority_millis = evidence_priority_millis
            .ok_or_else(|| de::Error::missing_field("evidence_priority_millis"))?;
        let token_budget_hint =
            token_budget_hint.ok_or_else(|| de::Error::missing_field("token_budget_hint"))?;
        let contributing_signals =
            contributing_signals.ok_or_else(|| de::Error::missing_field("contributing_signals"))?;
        let projection_evidence_kind = require_non_empty_string(
            "projection_evidence_kind",
            projection_evidence_kind
                .ok_or_else(|| de::Error::missing_field("projection_evidence_kind"))?,
        )?;
        let projection_authority_artifact_id = require_non_empty_string(
            "projection_authority_artifact_id",
            projection_authority_artifact_id
                .ok_or_else(|| de::Error::missing_field("projection_authority_artifact_id"))?,
        )?;
        let projection_authority_digest = require_non_empty_string(
            "projection_authority_digest",
            projection_authority_digest
                .ok_or_else(|| de::Error::missing_field("projection_authority_digest"))?,
        )?;
        let projection_status = require_non_empty_string(
            "projection_status",
            projection_status.ok_or_else(|| de::Error::missing_field("projection_status"))?,
        )?;
        let redaction_state =
            redaction_state.ok_or_else(|| de::Error::missing_field("redaction_state"))?;
        Ok(RepoMapEntryDto {
            subject_identity,
            subject_doc_type,
            subject_kind,
            owner_path,
            score,
            final_score_millis,
            included,
            rank,
            importance_score_millis,
            utility_score_millis,
            freshness_score_millis,
            evidence_priority_millis,
            token_budget_hint,
            contributing_signals,
            projection_evidence_kind,
            projection_authority_artifact_id,
            projection_authority_digest,
            projection_status,
            redaction_state,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapEntryDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapEntryDto",
            REPOMAP_ENTRY_DTO_V1_FIELDS,
            RepoMapEntryDtoV1Visitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RepoMapQueryRequest {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub query_text: String,
    pub top_k: u32,
    pub token_budget: u32,
    pub focus_subjects: Vec<RepoMapFocusSubjectDto>,
}

const REPOMAP_QUERY_REQUEST_V1_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "query_text",
    "top_k",
    "token_budget",
    "focus_subjects",
];

impl Serialize for RepoMapQueryRequest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapQueryRequest", 7)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("query_text", &self.query_text)?;
        state.serialize_field("top_k", &self.top_k)?;
        state.serialize_field("token_budget", &self.token_budget)?;
        state.serialize_field("focus_subjects", &self.focus_subjects)?;
        state.end()
    }
}

struct RepoMapQueryRequestV1Visitor;

impl<'de> Visitor<'de> for RepoMapQueryRequestV1Visitor {
    type Value = RepoMapQueryRequest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapQueryRequest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut query_text: Option<String> = None;
        let mut top_k: Option<u32> = None;
        let mut token_budget: Option<u32> = None;
        let mut focus_subjects: Option<Vec<RepoMapFocusSubjectDto>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
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
                "query_text" => {
                    if query_text.is_some() {
                        return Err(de::Error::duplicate_field("query_text"));
                    }
                    query_text = Some(map.next_value()?);
                }
                "top_k" => {
                    if top_k.is_some() {
                        return Err(de::Error::duplicate_field("top_k"));
                    }
                    top_k = Some(map.next_value()?);
                }
                "token_budget" => {
                    if token_budget.is_some() {
                        return Err(de::Error::duplicate_field("token_budget"));
                    }
                    token_budget = Some(map.next_value()?);
                }
                "focus_subjects" => {
                    if focus_subjects.is_some() {
                        return Err(de::Error::duplicate_field("focus_subjects"));
                    }
                    focus_subjects = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_QUERY_REQUEST_V1_FIELDS,
                    ));
                }
            }
        }
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let manifest_generation =
            manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?;
        let query_text = query_text.ok_or_else(|| de::Error::missing_field("query_text"))?;
        let top_k = top_k.ok_or_else(|| de::Error::missing_field("top_k"))?;
        let token_budget = token_budget.ok_or_else(|| de::Error::missing_field("token_budget"))?;
        let focus_subjects =
            focus_subjects.ok_or_else(|| de::Error::missing_field("focus_subjects"))?;
        Ok(RepoMapQueryRequest {
            repo_id,
            revision_id,
            manifest_generation,
            query_text,
            top_k,
            token_budget,
            focus_subjects,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapQueryRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapQueryRequest",
            REPOMAP_QUERY_REQUEST_V1_FIELDS,
            RepoMapQueryRequestV1Visitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepoMapQueryResponse {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub snapshot_meta: RepoMapSnapshotMeta,
    pub entries: Vec<RepoMapEntryDto>,
    pub dropped_entries_count: u32,
    pub drop_reason_codes: Vec<String>,
    pub degraded_reason_codes: Vec<String>,
}

const REPOMAP_QUERY_RESPONSE_V1_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "snapshot_meta",
    "entries",
    "dropped_entries_count",
    "drop_reason_codes",
    "degraded_reason_codes",
];

impl Serialize for RepoMapQueryResponse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapQueryResponse", 8)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("snapshot_meta", &self.snapshot_meta)?;
        state.serialize_field("entries", &self.entries)?;
        state.serialize_field("dropped_entries_count", &self.dropped_entries_count)?;
        state.serialize_field("drop_reason_codes", &self.drop_reason_codes)?;
        state.serialize_field("degraded_reason_codes", &self.degraded_reason_codes)?;
        state.end()
    }
}

struct RepoMapQueryResponseV1Visitor;

impl<'de> Visitor<'de> for RepoMapQueryResponseV1Visitor {
    type Value = RepoMapQueryResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapQueryResponse map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut snapshot_meta: Option<RepoMapSnapshotMeta> = None;
        let mut entries: Option<Vec<RepoMapEntryDto>> = None;
        let mut dropped_entries_count: Option<u32> = None;
        let mut drop_reason_codes: Option<Vec<String>> = None;
        let mut degraded_reason_codes: Option<Vec<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
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
                "snapshot_meta" => {
                    if snapshot_meta.is_some() {
                        return Err(de::Error::duplicate_field("snapshot_meta"));
                    }
                    snapshot_meta = Some(map.next_value()?);
                }
                "entries" => {
                    if entries.is_some() {
                        return Err(de::Error::duplicate_field("entries"));
                    }
                    entries = Some(map.next_value()?);
                }
                "dropped_entries_count" => {
                    if dropped_entries_count.is_some() {
                        return Err(de::Error::duplicate_field("dropped_entries_count"));
                    }
                    dropped_entries_count = Some(map.next_value()?);
                }
                "drop_reason_codes" => {
                    if drop_reason_codes.is_some() {
                        return Err(de::Error::duplicate_field("drop_reason_codes"));
                    }
                    drop_reason_codes = Some(map.next_value()?);
                }
                "degraded_reason_codes" => {
                    if degraded_reason_codes.is_some() {
                        return Err(de::Error::duplicate_field("degraded_reason_codes"));
                    }
                    degraded_reason_codes = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPOMAP_QUERY_RESPONSE_V1_FIELDS,
                    ));
                }
            }
        }
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let manifest_generation =
            manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?;
        let snapshot_meta =
            snapshot_meta.ok_or_else(|| de::Error::missing_field("snapshot_meta"))?;
        let entries = entries.ok_or_else(|| de::Error::missing_field("entries"))?;
        let dropped_entries_count = dropped_entries_count
            .ok_or_else(|| de::Error::missing_field("dropped_entries_count"))?;
        let drop_reason_codes =
            drop_reason_codes.ok_or_else(|| de::Error::missing_field("drop_reason_codes"))?;
        let degraded_reason_codes = degraded_reason_codes
            .ok_or_else(|| de::Error::missing_field("degraded_reason_codes"))?;
        Ok(RepoMapQueryResponse {
            repo_id,
            revision_id,
            manifest_generation,
            snapshot_meta,
            entries,
            dropped_entries_count,
            drop_reason_codes,
            degraded_reason_codes,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapQueryResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapQueryResponse",
            REPOMAP_QUERY_RESPONSE_V1_FIELDS,
            RepoMapQueryResponseV1Visitor,
        )
    }
}

#[cfg(test)]
mod tests {
    //! CBOR round-trip coverage for the hand-rolled serde impls above.
    //!
    //! Each test encodes a fully-populated instance through `ciborium` and
    //! decodes it back, asserting structural equality. This pins the on-wire
    //! shape (field names, field count, ordering at encode time) against
    //! accidental drift from the prior derived impls.

    use super::{
        ChunkId, FileId, LanguageCode, ManifestGeneration, RepoId,
        RepoMapActivateGenerationRequest, RepoMapCallEdge, RepoMapChunkExactness, RepoMapChunkNode,
        RepoMapChunkRecordDto, RepoMapContainsEdge, RepoMapDependsOnEdge, RepoMapDocType,
        RepoMapEdge, RepoMapEdgeKind, RepoMapEntryDto, RepoMapExactnessSummary,
        RepoMapFileIndexRecord, RepoMapFileNode, RepoMapFocusSubjectDto, RepoMapGraphCoverage,
        RepoMapGraphCoverageClass, RepoMapGraphEdgeDto, RepoMapImportEdge,
        RepoMapItemIndexAvailability, RepoMapModuleId, RepoMapModuleNode, RepoMapMutationAck,
        RepoMapNode, RepoMapNodeRef, RepoMapOwnsChunkEdge, RepoMapQueryRequest,
        RepoMapQueryResponse, RepoMapRedactionState, RepoMapSnapshotMeta, RepoMapSourceBundle,
        RepoMapSymbolNode, RepoMapSymbolRecordDto, RepoRelativePath, RevisionId, SymbolId,
        SymbolKindCode,
    };
    use std::collections::BTreeMap;

    type TestRes = Result<(), Box<dyn std::error::Error>>;

    fn encode<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        let mut buf: Vec<u8> = Vec::new();
        ciborium::ser::into_writer(value, &mut buf)?;
        Ok(buf)
    }

    fn decode<T>(bytes: &[u8]) -> Result<T, Box<dyn std::error::Error>>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        Ok(ciborium::de::from_reader(bytes)?)
    }

    fn roundtrip_eq<T>(value: &T) -> TestRes
    where
        T: serde::Serialize + for<'de> serde::Deserialize<'de> + core::fmt::Debug + PartialEq,
    {
        let bytes = encode(value)?;
        let decoded: T = decode(&bytes)?;
        if &decoded != value {
            return Err(
                format!("round-trip mismatch: original={value:?}, decoded={decoded:?}").into(),
            );
        }
        Ok(())
    }

    fn overwrite_text_field(
        wire: &mut ciborium::Value,
        field: &str,
        value: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let ciborium::Value::Map(fields) = wire else {
            return Err("expected map".into());
        };
        for (key, current) in fields.iter_mut() {
            if matches!(key, ciborium::Value::Text(text) if text == field) {
                *current = ciborium::Value::Text(value.to_owned());
                return Ok(());
            }
        }
        Err(format!("missing field {field}").into())
    }

    fn sample_symbol_record() -> RepoMapSymbolRecordDto {
        RepoMapSymbolRecordDto {
            subject_identity: "sym::ident".into(),
            subject_doc_type: RepoMapDocType::Symbol,
            subject_kind: "function".into(),
            symbol_name: "do_thing".into(),
            owner_path: "src/lib.rs".into(),
        }
    }

    fn sample_file_index_record() -> RepoMapFileIndexRecord {
        RepoMapFileIndexRecord {
            file_identity: "file::ident".into(),
            file_path: "src/lib.rs".into(),
            file_kind: "rust".into(),
            line_count: 42,
            symbol_records: vec![sample_symbol_record()],
        }
    }

    fn sample_graph_edge() -> RepoMapGraphEdgeDto {
        RepoMapGraphEdgeDto {
            from_identity: "from::ident".into(),
            to_identity: "to::ident".into(),
            edge_kind: RepoMapEdgeKind::Call,
        }
    }

    fn sample_chunk_record() -> RepoMapChunkRecordDto {
        RepoMapChunkRecordDto {
            subject_identity: "chunk::ident".into(),
            owner_path: "src/lib.rs".into(),
            token_count: 128,
            preview_text: "fn do_thing() {}".into(),
            exactness: RepoMapChunkExactness::Exact,
        }
    }

    fn sample_focus_subject() -> RepoMapFocusSubjectDto {
        RepoMapFocusSubjectDto {
            subject_identity: "focus::ident".into(),
            subject_doc_type: RepoMapDocType::Symbol,
        }
    }

    fn rust_language() -> LanguageCode {
        match LanguageCode::new("rust") {
            Ok(value) => value,
            Err(err) => {
                assert!(false, "sample uses canonical rust language code: {err}");
                std::process::abort();
            }
        }
    }

    fn module_symbol_kind() -> SymbolKindCode {
        match SymbolKindCode::new("struct") {
            Ok(value) => value,
            Err(err) => {
                assert!(false, "sample uses canonical symbol kind: {err}");
                std::process::abort();
            }
        }
    }

    fn sample_graph_coverage() -> RepoMapGraphCoverage {
        RepoMapGraphCoverage {
            item_index_availability: RepoMapItemIndexAvailability::Full,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
        }
    }

    fn sample_file_node() -> RepoMapFileNode {
        RepoMapFileNode {
            file_id: FileId::new("file::ident"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            line_count: 42,
        }
    }

    fn sample_module_node() -> RepoMapModuleNode {
        RepoMapModuleNode {
            module_id: RepoMapModuleId::new("module::ident"),
            repo_relative_path: RepoRelativePath::new("src/lib.rs"),
            qualified_name: "crate::lib".into(),
        }
    }

    fn sample_symbol_node() -> RepoMapSymbolNode {
        RepoMapSymbolNode {
            symbol_id: SymbolId::new("symbol::ident"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            local_name: "do_thing".into(),
            qualified_name: "crate::lib::do_thing".into(),
            symbol_kind: module_symbol_kind(),
        }
    }

    fn sample_chunk_node() -> RepoMapChunkNode {
        RepoMapChunkNode {
            chunk_id: ChunkId::new("chunk::ident"),
            owner_path: RepoRelativePath::new("src/lib.rs"),
            language: rust_language(),
            start_byte: 10,
            end_byte: 42,
            start_line: 2,
            end_line: 6,
            token_count: 128,
            preview_text: "fn do_thing() {}".into(),
            exactness: RepoMapChunkExactness::Exact,
        }
    }

    fn sample_node_ref() -> RepoMapNodeRef {
        RepoMapNodeRef::Symbol(SymbolId::new("symbol::ident"))
    }

    fn sample_node() -> RepoMapNode {
        RepoMapNode::Symbol(sample_symbol_node())
    }

    fn sample_edge() -> RepoMapEdge {
        RepoMapEdge::Call(RepoMapCallEdge {
            caller: RepoMapNodeRef::Symbol(SymbolId::new("symbol::caller")),
            callee: RepoMapNodeRef::File(FileId::new("file::callee")),
        })
    }

    fn sample_snapshot_meta() -> RepoMapSnapshotMeta {
        RepoMapSnapshotMeta {
            snapshot_id: "snap-1".into(),
            projection_version: 7,
            authority_digest: "blake3:deadbeef".into(),
            item_index_availability: RepoMapItemIndexAvailability::Full,
            graph_coverage_class: RepoMapGraphCoverageClass::Complete,
            exactness_summary: RepoMapExactnessSummary::Exact,
        }
    }

    fn sample_entry() -> RepoMapEntryDto {
        let contributing_signals = BTreeMap::from([
            ("centrality".to_owned(), 100_i64),
            ("recency".to_owned(), -3_i64),
        ]);
        RepoMapEntryDto {
            subject_identity: "entry::ident".into(),
            subject_doc_type: RepoMapDocType::Symbol,
            subject_kind: "function".into(),
            owner_path: "src/lib.rs".into(),
            score: 0.875_f32,
            final_score_millis: 875,
            included: true,
            rank: 1,
            importance_score_millis: 500,
            utility_score_millis: 400,
            freshness_score_millis: 300,
            evidence_priority_millis: 200,
            token_budget_hint: 1024,
            contributing_signals,
            projection_evidence_kind: "authoritative".into(),
            projection_authority_artifact_id: "art-1".into(),
            projection_authority_digest: "blake3:cafebabe".into(),
            projection_status: "ok".into(),
            redaction_state: RepoMapRedactionState::Unredacted,
        }
    }

    fn sample_repo_id() -> RepoId {
        RepoId::new("repo-1")
    }

    fn sample_revision_id() -> RevisionId {
        RevisionId::new("rev-1")
    }

    fn sample_manifest_generation() -> ManifestGeneration {
        ManifestGeneration::new(11)
    }

    fn sample_source_bundle() -> RepoMapSourceBundle {
        RepoMapSourceBundle::new(
            sample_repo_id(),
            sample_revision_id(),
            sample_manifest_generation(),
            "blake3:manifest",
            "snap-1",
            3,
            "blake3:feedface",
            sample_graph_coverage(),
            RepoMapExactnessSummary::Exact,
            RepoMapRedactionState::Unredacted,
        )
        .with_node(RepoMapNode::File(sample_file_node()))
        .with_node(RepoMapNode::Module(sample_module_node()))
        .with_node(RepoMapNode::Symbol(sample_symbol_node()))
        .with_node(RepoMapNode::Chunk(sample_chunk_node()))
        .with_edge(RepoMapEdge::Contains(RepoMapContainsEdge {
            container: RepoMapNodeRef::File(FileId::new("file::ident")),
            contained: RepoMapNodeRef::Symbol(SymbolId::new("symbol::ident")),
        }))
        .with_edge(sample_edge())
        .with_edge(RepoMapEdge::Import(RepoMapImportEdge {
            importer: RepoMapNodeRef::Module(RepoMapModuleId::new("module::ident")),
            imported: RepoMapNodeRef::File(FileId::new("file::ident")),
        }))
        .with_edge(RepoMapEdge::OwnsChunk(RepoMapOwnsChunkEdge {
            owner: RepoMapNodeRef::Symbol(SymbolId::new("symbol::ident")),
            chunk: RepoMapNodeRef::Chunk(ChunkId::new("chunk::ident")),
        }))
        .with_edge(RepoMapEdge::DependsOn(RepoMapDependsOnEdge {
            dependent: RepoMapNodeRef::File(FileId::new("file::ident")),
            dependency: RepoMapNodeRef::Module(RepoMapModuleId::new("module::ident")),
        }))
    }

    fn sample_activate_request() -> RepoMapActivateGenerationRequest {
        RepoMapActivateGenerationRequest {
            repo_id: sample_repo_id(),
            revision_id: sample_revision_id(),
            manifest_generation: sample_manifest_generation(),
            manifest_digest: "blake3:1234".into(),
        }
    }

    fn sample_mutation_ack() -> RepoMapMutationAck {
        RepoMapMutationAck {
            repo_id: sample_repo_id(),
            revision_id: sample_revision_id(),
            manifest_generation: sample_manifest_generation(),
        }
    }

    fn sample_query_request() -> RepoMapQueryRequest {
        RepoMapQueryRequest {
            repo_id: sample_repo_id(),
            revision_id: sample_revision_id(),
            manifest_generation: sample_manifest_generation(),
            query_text: "find me".into(),
            top_k: 25,
            token_budget: 4096,
            focus_subjects: vec![sample_focus_subject()],
        }
    }

    fn sample_query_response() -> RepoMapQueryResponse {
        RepoMapQueryResponse {
            repo_id: sample_repo_id(),
            revision_id: sample_revision_id(),
            manifest_generation: sample_manifest_generation(),
            snapshot_meta: sample_snapshot_meta(),
            entries: vec![sample_entry()],
            dropped_entries_count: 2,
            drop_reason_codes: vec!["token_budget".into()],
            degraded_reason_codes: vec!["partial_authority".into()],
        }
    }

    #[test]
    fn cbor_roundtrip_symbol_record() -> TestRes {
        roundtrip_eq(&sample_symbol_record())
    }

    #[test]
    fn cbor_roundtrip_file_index_record() -> TestRes {
        roundtrip_eq(&sample_file_index_record())
    }

    #[test]
    fn cbor_roundtrip_graph_edge() -> TestRes {
        roundtrip_eq(&sample_graph_edge())
    }

    #[test]
    fn cbor_roundtrip_chunk_record() -> TestRes {
        roundtrip_eq(&sample_chunk_record())
    }

    #[test]
    fn cbor_roundtrip_focus_subject() -> TestRes {
        roundtrip_eq(&sample_focus_subject())
    }

    #[test]
    fn cbor_roundtrip_graph_coverage() -> TestRes {
        roundtrip_eq(&sample_graph_coverage())
    }

    #[test]
    fn cbor_roundtrip_node_ref() -> TestRes {
        roundtrip_eq(&sample_node_ref())
    }

    #[test]
    fn cbor_roundtrip_node() -> TestRes {
        roundtrip_eq(&sample_node())
    }

    #[test]
    fn cbor_roundtrip_edge() -> TestRes {
        roundtrip_eq(&sample_edge())
    }

    #[test]
    fn cbor_roundtrip_snapshot_meta() -> TestRes {
        roundtrip_eq(&sample_snapshot_meta())
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "ciborium f16 path uses aarch64 inline asm that Miri cannot execute; f32 score serde is exercised in stable tests + fuzz"
    )]
    fn cbor_roundtrip_entry() -> TestRes {
        roundtrip_eq(&sample_entry())
    }

    #[test]
    fn cbor_roundtrip_source_bundle() -> TestRes {
        roundtrip_eq(&sample_source_bundle())
    }

    #[test]
    fn cbor_roundtrip_activate_generation_request() -> TestRes {
        roundtrip_eq(&sample_activate_request())
    }

    #[test]
    fn cbor_roundtrip_mutation_ack() -> TestRes {
        roundtrip_eq(&sample_mutation_ack())
    }

    #[test]
    fn cbor_roundtrip_query_request() -> TestRes {
        roundtrip_eq(&sample_query_request())
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "ciborium f16 path uses aarch64 inline asm that Miri cannot execute; f32 score serde is exercised in stable tests + fuzz"
    )]
    fn cbor_roundtrip_query_response() -> TestRes {
        roundtrip_eq(&sample_query_response())
    }

    #[test]
    fn focus_subject_rejects_empty_subject_identity_on_serialize() {
        let mut subject = sample_focus_subject();
        subject.subject_identity.clear();
        let Err(err) = encode(&subject) else {
            assert!(false, "empty focus subject identity must fail closed");
            std::process::abort();
        };
        assert!(
            err.to_string()
                .contains("subject_identity must not be empty")
        );
    }

    #[test]
    fn focus_subject_rejects_empty_subject_identity_on_deserialize() -> TestRes {
        let bytes = encode(&sample_focus_subject())?;
        let mut wire: ciborium::Value = decode(&bytes)?;
        overwrite_text_field(&mut wire, "subject_identity", "")?;
        let mut mutated = Vec::new();
        ciborium::ser::into_writer(&wire, &mut mutated)?;
        let Err(err) = decode::<RepoMapFocusSubjectDto>(&mutated) else {
            return Err("empty focus subject identity must fail closed".into());
        };
        if !err
            .to_string()
            .contains("subject_identity must not be empty")
        {
            return Err(format!("unexpected focus subject decode error: {err}").into());
        }
        Ok(())
    }

    #[test]
    fn entry_rejects_empty_projection_status_on_serialize() {
        let mut entry = sample_entry();
        entry.projection_status.clear();
        let Err(err) = encode(&entry) else {
            assert!(false, "empty projection status must fail closed");
            std::process::abort();
        };
        assert!(
            err.to_string()
                .contains("projection_status must not be empty")
        );
    }

    #[test]
    fn entry_rejects_empty_projection_status_on_deserialize() -> TestRes {
        let bytes = encode(&sample_entry())?;
        let mut wire: ciborium::Value = decode(&bytes)?;
        overwrite_text_field(&mut wire, "projection_status", "")?;
        let mut mutated = Vec::new();
        ciborium::ser::into_writer(&wire, &mut mutated)?;
        let Err(err) = decode::<RepoMapEntryDto>(&mutated) else {
            return Err("empty projection status must fail closed".into());
        };
        if !err
            .to_string()
            .contains("projection_status must not be empty")
        {
            return Err(format!("unexpected entry decode error: {err}").into());
        }
        Ok(())
    }
}
