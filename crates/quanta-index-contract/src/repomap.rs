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
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use quanta_index_contract_base::ids::{ManifestGeneration, RepoId, RevisionId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapSymbolRecordDto {
    pub subject_identity: String,
    pub subject_doc_type: String,
    pub subject_kind: String,
    pub symbol_name: String,
    pub owner_path: String,
}

const REPOMAP_SYMBOL_RECORD_DTO_V1_FIELDS: &[&str] = &[
    "subject_identity",
    "subject_doc_type",
    "subject_kind",
    "symbol_name",
    "owner_path",
];

impl Serialize for RepoMapSymbolRecordDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapSymbolRecordDto", 5)?;
        state.serialize_field("subject_identity", &self.subject_identity)?;
        state.serialize_field("subject_doc_type", &self.subject_doc_type)?;
        state.serialize_field("subject_kind", &self.subject_kind)?;
        state.serialize_field("symbol_name", &self.symbol_name)?;
        state.serialize_field("owner_path", &self.owner_path)?;
        state.end()
    }
}

struct RepoMapSymbolRecordDtoV1Visitor;

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
        let mut subject_doc_type: Option<String> = None;
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
        let subject_kind = subject_kind.ok_or_else(|| de::Error::missing_field("subject_kind"))?;
        let symbol_name = symbol_name.ok_or_else(|| de::Error::missing_field("symbol_name"))?;
        let owner_path = owner_path.ok_or_else(|| de::Error::missing_field("owner_path"))?;
        Ok(RepoMapSymbolRecordDto {
            subject_identity,
            subject_doc_type,
            subject_kind,
            symbol_name,
            owner_path,
        })
    }
}

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapFileIndexRecord {
    pub file_identity: String,
    pub file_path: String,
    pub file_kind: String,
    pub line_count: u32,
    pub symbol_records: Vec<RepoMapSymbolRecordDto>,
}

const REPOMAP_FILE_INDEX_RECORD_V1_FIELDS: &[&str] = &[
    "file_identity",
    "file_path",
    "file_kind",
    "line_count",
    "symbol_records",
];

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

struct RepoMapFileIndexRecordV1Visitor;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapGraphEdgeDto {
    pub from_identity: String,
    pub to_identity: String,
    pub edge_kind: String,
}

const REPOMAP_GRAPH_EDGE_DTO_V1_FIELDS: &[&str] = &["from_identity", "to_identity", "edge_kind"];

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

struct RepoMapGraphEdgeDtoV1Visitor;

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
        let mut edge_kind: Option<String> = None;
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapChunkRecordDto {
    pub subject_identity: String,
    pub owner_path: String,
    pub token_count: u32,
    pub preview_text: String,
    pub exactness: String,
}

const REPOMAP_CHUNK_RECORD_DTO_V1_FIELDS: &[&str] = &[
    "subject_identity",
    "owner_path",
    "token_count",
    "preview_text",
    "exactness",
];

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

struct RepoMapChunkRecordDtoV1Visitor;

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
        let mut exactness: Option<String> = None;
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
pub struct RepoMapSourceBundle {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub snapshot_id: String,
    pub projection_version: u32,
    pub authority_digest: String,
    pub item_index_availability: String,
    pub graph_coverage_class: String,
    pub exactness_summary: String,
    pub redaction_state: String,
    pub file_indices: Vec<RepoMapFileIndexRecord>,
    pub call_edges: Vec<RepoMapGraphEdgeDto>,
    pub import_edges: Vec<RepoMapGraphEdgeDto>,
    pub chunk_records: Vec<RepoMapChunkRecordDto>,
}

const REPOMAP_SOURCE_BUNDLE_V1_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "snapshot_id",
    "projection_version",
    "authority_digest",
    "item_index_availability",
    "graph_coverage_class",
    "exactness_summary",
    "redaction_state",
    "file_indices",
    "call_edges",
    "import_edges",
    "chunk_records",
];

impl Serialize for RepoMapSourceBundle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapSourceBundle", 14)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("snapshot_id", &self.snapshot_id)?;
        state.serialize_field("projection_version", &self.projection_version)?;
        state.serialize_field("authority_digest", &self.authority_digest)?;
        state.serialize_field("item_index_availability", &self.item_index_availability)?;
        state.serialize_field("graph_coverage_class", &self.graph_coverage_class)?;
        state.serialize_field("exactness_summary", &self.exactness_summary)?;
        state.serialize_field("redaction_state", &self.redaction_state)?;
        state.serialize_field("file_indices", &self.file_indices)?;
        state.serialize_field("call_edges", &self.call_edges)?;
        state.serialize_field("import_edges", &self.import_edges)?;
        state.serialize_field("chunk_records", &self.chunk_records)?;
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
        let mut snapshot_id: Option<String> = None;
        let mut projection_version: Option<u32> = None;
        let mut authority_digest: Option<String> = None;
        let mut item_index_availability: Option<String> = None;
        let mut graph_coverage_class: Option<String> = None;
        let mut exactness_summary: Option<String> = None;
        let mut redaction_state: Option<String> = None;
        let mut file_indices: Option<Vec<RepoMapFileIndexRecord>> = None;
        let mut call_edges: Option<Vec<RepoMapGraphEdgeDto>> = None;
        let mut import_edges: Option<Vec<RepoMapGraphEdgeDto>> = None;
        let mut chunk_records: Option<Vec<RepoMapChunkRecordDto>> = None;
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
                "redaction_state" => {
                    if redaction_state.is_some() {
                        return Err(de::Error::duplicate_field("redaction_state"));
                    }
                    redaction_state = Some(map.next_value()?);
                }
                "file_indices" => {
                    if file_indices.is_some() {
                        return Err(de::Error::duplicate_field("file_indices"));
                    }
                    file_indices = Some(map.next_value()?);
                }
                "call_edges" => {
                    if call_edges.is_some() {
                        return Err(de::Error::duplicate_field("call_edges"));
                    }
                    call_edges = Some(map.next_value()?);
                }
                "import_edges" => {
                    if import_edges.is_some() {
                        return Err(de::Error::duplicate_field("import_edges"));
                    }
                    import_edges = Some(map.next_value()?);
                }
                "chunk_records" => {
                    if chunk_records.is_some() {
                        return Err(de::Error::duplicate_field("chunk_records"));
                    }
                    chunk_records = Some(map.next_value()?);
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
        let redaction_state =
            redaction_state.ok_or_else(|| de::Error::missing_field("redaction_state"))?;
        let file_indices = file_indices.ok_or_else(|| de::Error::missing_field("file_indices"))?;
        let call_edges = call_edges.ok_or_else(|| de::Error::missing_field("call_edges"))?;
        let import_edges = import_edges.ok_or_else(|| de::Error::missing_field("import_edges"))?;
        let chunk_records =
            chunk_records.ok_or_else(|| de::Error::missing_field("chunk_records"))?;
        Ok(RepoMapSourceBundle {
            repo_id,
            revision_id,
            manifest_generation,
            snapshot_id,
            projection_version,
            authority_digest,
            item_index_availability,
            graph_coverage_class,
            exactness_summary,
            redaction_state,
            file_indices,
            call_edges,
            import_edges,
            chunk_records,
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
    pub item_index_availability: String,
    pub graph_coverage_class: String,
    pub exactness_summary: String,
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
        let mut item_index_availability: Option<String> = None;
        let mut graph_coverage_class: Option<String> = None;
        let mut exactness_summary: Option<String> = None;
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

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct RepoMapFocusSubjectDto {
    pub subject_identity: String,
    pub subject_doc_type: String,
}

const REPOMAP_FOCUS_SUBJECT_DTO_V1_FIELDS: &[&str] = &["subject_identity", "subject_doc_type"];

impl Serialize for RepoMapFocusSubjectDto {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
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
        let mut subject_doc_type: Option<String> = None;
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

#[derive(Clone, Debug, PartialEq)]
pub struct RepoMapEntryDto {
    pub subject_identity: String,
    pub subject_doc_type: String,
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
    pub redaction_state: String,
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
        let mut subject_doc_type: Option<String> = None;
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
        let mut redaction_state: Option<String> = None;
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
        let subject_kind = subject_kind.ok_or_else(|| de::Error::missing_field("subject_kind"))?;
        let owner_path = owner_path.ok_or_else(|| de::Error::missing_field("owner_path"))?;
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
        let projection_evidence_kind = projection_evidence_kind
            .ok_or_else(|| de::Error::missing_field("projection_evidence_kind"))?;
        let projection_authority_artifact_id = projection_authority_artifact_id
            .ok_or_else(|| de::Error::missing_field("projection_authority_artifact_id"))?;
        let projection_authority_digest = projection_authority_digest
            .ok_or_else(|| de::Error::missing_field("projection_authority_digest"))?;
        let projection_status =
            projection_status.ok_or_else(|| de::Error::missing_field("projection_status"))?;
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
        ManifestGeneration, RepoId, RepoMapActivateGenerationRequest, RepoMapChunkRecordDto,
        RepoMapEntryDto, RepoMapFileIndexRecord, RepoMapFocusSubjectDto, RepoMapGraphEdgeDto,
        RepoMapMutationAck, RepoMapQueryRequest, RepoMapQueryResponse, RepoMapSnapshotMeta,
        RepoMapSourceBundle, RepoMapSymbolRecordDto, RevisionId,
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

    fn sample_symbol_record() -> RepoMapSymbolRecordDto {
        RepoMapSymbolRecordDto {
            subject_identity: "sym::ident".into(),
            subject_doc_type: "rust".into(),
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
            edge_kind: "call".into(),
        }
    }

    fn sample_chunk_record() -> RepoMapChunkRecordDto {
        RepoMapChunkRecordDto {
            subject_identity: "chunk::ident".into(),
            owner_path: "src/lib.rs".into(),
            token_count: 128,
            preview_text: "fn do_thing() {}".into(),
            exactness: "exact".into(),
        }
    }

    fn sample_focus_subject() -> RepoMapFocusSubjectDto {
        RepoMapFocusSubjectDto {
            subject_identity: "focus::ident".into(),
            subject_doc_type: "rust".into(),
        }
    }

    fn sample_snapshot_meta() -> RepoMapSnapshotMeta {
        RepoMapSnapshotMeta {
            snapshot_id: "snap-1".into(),
            projection_version: 7,
            authority_digest: "blake3:deadbeef".into(),
            item_index_availability: "full".into(),
            graph_coverage_class: "complete".into(),
            exactness_summary: "exact".into(),
        }
    }

    fn sample_entry() -> RepoMapEntryDto {
        let contributing_signals = BTreeMap::from([
            ("centrality".to_owned(), 100_i64),
            ("recency".to_owned(), -3_i64),
        ]);
        RepoMapEntryDto {
            subject_identity: "entry::ident".into(),
            subject_doc_type: "rust".into(),
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
            redaction_state: "none".into(),
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
        RepoMapSourceBundle {
            repo_id: sample_repo_id(),
            revision_id: sample_revision_id(),
            manifest_generation: sample_manifest_generation(),
            snapshot_id: "snap-1".into(),
            projection_version: 3,
            authority_digest: "blake3:feedface".into(),
            item_index_availability: "full".into(),
            graph_coverage_class: "complete".into(),
            exactness_summary: "exact".into(),
            redaction_state: "none".into(),
            file_indices: vec![sample_file_index_record()],
            call_edges: vec![sample_graph_edge()],
            import_edges: vec![sample_graph_edge()],
            chunk_records: vec![sample_chunk_record()],
        }
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
}
