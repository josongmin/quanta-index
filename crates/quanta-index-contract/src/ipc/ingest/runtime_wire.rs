//! Dirty, runtime-catalog and structural ingest wire DTOs.

use super::{BatchIngestMode, SearchScopeKey};
use crate::lex::{DirtyRecord, ParseTreeRecord};
use crate::{ChunkId, ManifestGeneration, RepoId, RevisionId};
use core::fmt;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, VariantAccess, Visitor},
    ser::SerializeStruct,
};

// =============================================================================
// Runtime dirty ingest batch
// =============================================================================

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyDelete {
    pub doc_id: ChunkId,
}

const DIRTY_DELETE_FIELDS: &[&str] = &["doc_id"];

impl Serialize for DirtyDelete {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DirtyDelete", 1)?;
        state.serialize_field("doc_id", &self.doc_id)?;
        state.end()
    }
}

struct DirtyDeleteVisitor;

impl<'de> Visitor<'de> for DirtyDeleteVisitor {
    type Value = DirtyDelete;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DirtyDelete map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut doc_id: Option<ChunkId> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "doc_id" => doc_id = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, DIRTY_DELETE_FIELDS)),
            }
        }
        Ok(DirtyDelete {
            doc_id: doc_id.ok_or_else(|| de::Error::missing_field("doc_id"))?,
        })
    }
}

impl<'de> Deserialize<'de> for DirtyDelete {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("DirtyDelete", DIRTY_DELETE_FIELDS, DirtyDeleteVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirtyMutation {
    Upsert(DirtyRecord),
    Delete(DirtyDelete),
}

const DIRTY_MUTATION_VARIANTS: &[&str] = &["Upsert", "Delete"];

impl Serialize for DirtyMutation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Upsert(payload) => {
                serializer.serialize_newtype_variant("DirtyMutation", 0, "Upsert", payload)
            }
            Self::Delete(payload) => {
                serializer.serialize_newtype_variant("DirtyMutation", 1, "Delete", payload)
            }
        }
    }
}

struct DirtyMutationVisitor;

impl<'de> Visitor<'de> for DirtyMutationVisitor {
    type Value = DirtyMutation;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DirtyMutation enum")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::EnumAccess<'de>,
    {
        let (tag, variant) = data.variant::<String>()?;
        match tag.as_str() {
            "Upsert" => Ok(DirtyMutation::Upsert(variant.newtype_variant()?)),
            "Delete" => Ok(DirtyMutation::Delete(variant.newtype_variant()?)),
            other => Err(de::Error::unknown_variant(other, DIRTY_MUTATION_VARIANTS)),
        }
    }
}

impl<'de> Deserialize<'de> for DirtyMutation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_enum(
            "DirtyMutation",
            DIRTY_MUTATION_VARIANTS,
            DirtyMutationVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirtyIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub overlay_epoch_ms: u64,
    pub batch_digest: String,
    pub entries: Vec<DirtyMutation>,
}

const DIRTY_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "overlay_epoch_ms",
    "batch_digest",
    "entries",
];

impl Serialize for DirtyIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DirtyIngestBatch", 6)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("overlay_epoch_ms", &self.overlay_epoch_ms)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct DirtyIngestBatchVisitor;

impl<'de> Visitor<'de> for DirtyIngestBatchVisitor {
    type Value = DirtyIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a DirtyIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut overlay_epoch_ms: Option<u64> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<DirtyMutation>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "overlay_epoch_ms" => overlay_epoch_ms = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, DIRTY_INGEST_BATCH_FIELDS)),
            }
        }
        Ok(DirtyIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            overlay_epoch_ms: overlay_epoch_ms
                .ok_or_else(|| de::Error::missing_field("overlay_epoch_ms"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for DirtyIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "DirtyIngestBatch",
            DIRTY_INGEST_BATCH_FIELDS,
            DirtyIngestBatchVisitor,
        )
    }
}

// =============================================================================
// Runtime catalog ingest batch
// =============================================================================

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeChangedRecord {
    pub doc_id: ChunkId,
    pub applied_at_ms: u64,
    pub payload_hash: [u8; 32],
}

const RUNTIME_CHANGED_RECORD_FIELDS: &[&str] = &["doc_id", "applied_at_ms", "payload_hash"];

impl Serialize for RuntimeChangedRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RuntimeChangedRecord", 3)?;
        state.serialize_field("doc_id", &self.doc_id)?;
        state.serialize_field("applied_at_ms", &self.applied_at_ms)?;
        state.serialize_field("payload_hash", &self.payload_hash)?;
        state.end()
    }
}

struct RuntimeChangedRecordVisitor;

impl<'de> Visitor<'de> for RuntimeChangedRecordVisitor {
    type Value = RuntimeChangedRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RuntimeChangedRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut doc_id: Option<ChunkId> = None;
        let mut applied_at_ms: Option<u64> = None;
        let mut payload_hash: Option<[u8; 32]> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "doc_id" => doc_id = Some(map.next_value()?),
                "applied_at_ms" => applied_at_ms = Some(map.next_value()?),
                "payload_hash" => payload_hash = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        RUNTIME_CHANGED_RECORD_FIELDS,
                    ));
                }
            }
        }
        Ok(RuntimeChangedRecord {
            doc_id: doc_id.ok_or_else(|| de::Error::missing_field("doc_id"))?,
            applied_at_ms: applied_at_ms
                .ok_or_else(|| de::Error::missing_field("applied_at_ms"))?,
            payload_hash: payload_hash.ok_or_else(|| de::Error::missing_field("payload_hash"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RuntimeChangedRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RuntimeChangedRecord",
            RUNTIME_CHANGED_RECORD_FIELDS,
            RuntimeChangedRecordVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeDocFacetRecord {
    pub doc_id: ChunkId,
    pub owner: Option<String>,
    pub service: Option<String>,
    pub layer: Option<String>,
    pub surface: Option<String>,
}

const RUNTIME_DOC_FACET_RECORD_FIELDS: &[&str] =
    &["doc_id", "owner", "service", "layer", "surface"];

impl Serialize for RuntimeDocFacetRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RuntimeDocFacetRecord", 5)?;
        state.serialize_field("doc_id", &self.doc_id)?;
        state.serialize_field("owner", &self.owner)?;
        state.serialize_field("service", &self.service)?;
        state.serialize_field("layer", &self.layer)?;
        state.serialize_field("surface", &self.surface)?;
        state.end()
    }
}

struct RuntimeDocFacetRecordVisitor;

impl<'de> Visitor<'de> for RuntimeDocFacetRecordVisitor {
    type Value = RuntimeDocFacetRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RuntimeDocFacetRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut doc_id: Option<ChunkId> = None;
        let mut owner: Option<String> = None;
        let mut service: Option<String> = None;
        let mut layer: Option<String> = None;
        let mut surface: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "doc_id" => doc_id = Some(map.next_value()?),
                "owner" => owner = Some(map.next_value()?),
                "service" => service = Some(map.next_value()?),
                "layer" => layer = Some(map.next_value()?),
                "surface" => surface = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        RUNTIME_DOC_FACET_RECORD_FIELDS,
                    ));
                }
            }
        }
        Ok(RuntimeDocFacetRecord {
            doc_id: doc_id.ok_or_else(|| de::Error::missing_field("doc_id"))?,
            owner,
            service,
            layer,
            surface,
        })
    }
}

impl<'de> Deserialize<'de> for RuntimeDocFacetRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RuntimeDocFacetRecord",
            RUNTIME_DOC_FACET_RECORD_FIELDS,
            RuntimeDocFacetRecordVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeSnapshotRecord {
    pub name: String,
    pub doc_ids: Vec<ChunkId>,
}

const RUNTIME_SNAPSHOT_RECORD_FIELDS: &[&str] = &["name", "doc_ids"];

impl Serialize for RuntimeSnapshotRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RuntimeSnapshotRecord", 2)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("doc_ids", &self.doc_ids)?;
        state.end()
    }
}

struct RuntimeSnapshotRecordVisitor;

impl<'de> Visitor<'de> for RuntimeSnapshotRecordVisitor {
    type Value = RuntimeSnapshotRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RuntimeSnapshotRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut name: Option<String> = None;
        let mut doc_ids: Option<Vec<ChunkId>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "name" => name = Some(map.next_value()?),
                "doc_ids" => doc_ids = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        RUNTIME_SNAPSHOT_RECORD_FIELDS,
                    ));
                }
            }
        }
        Ok(RuntimeSnapshotRecord {
            name: name.ok_or_else(|| de::Error::missing_field("name"))?,
            doc_ids: doc_ids.ok_or_else(|| de::Error::missing_field("doc_ids"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RuntimeSnapshotRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RuntimeSnapshotRecord",
            RUNTIME_SNAPSHOT_RECORD_FIELDS,
            RuntimeSnapshotRecordVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeEdgeAuthorityRecord {
    pub key: String,
    pub doc_ids: Vec<ChunkId>,
}

const RUNTIME_EDGE_AUTHORITY_RECORD_FIELDS: &[&str] = &["key", "doc_ids"];

impl Serialize for RuntimeEdgeAuthorityRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RuntimeEdgeAuthorityRecord", 2)?;
        state.serialize_field("key", &self.key)?;
        state.serialize_field("doc_ids", &self.doc_ids)?;
        state.end()
    }
}

struct RuntimeEdgeAuthorityRecordVisitor;

impl<'de> Visitor<'de> for RuntimeEdgeAuthorityRecordVisitor {
    type Value = RuntimeEdgeAuthorityRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RuntimeEdgeAuthorityRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut key: Option<String> = None;
        let mut doc_ids: Option<Vec<ChunkId>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "key" => key = Some(map.next_value()?),
                "doc_ids" => doc_ids = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        RUNTIME_EDGE_AUTHORITY_RECORD_FIELDS,
                    ));
                }
            }
        }
        Ok(RuntimeEdgeAuthorityRecord {
            key: key.ok_or_else(|| de::Error::missing_field("key"))?,
            doc_ids: doc_ids.ok_or_else(|| de::Error::missing_field("doc_ids"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RuntimeEdgeAuthorityRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RuntimeEdgeAuthorityRecord",
            RUNTIME_EDGE_AUTHORITY_RECORD_FIELDS,
            RuntimeEdgeAuthorityRecordVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeCatalogIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub overlay_epoch_ms: u64,
    pub batch_digest: String,
    pub producer_head_applied_at_ms: u64,
    pub generation_materialized_at_ms: u64,
    pub changed_entries: Vec<RuntimeChangedRecord>,
    pub facet_entries: Vec<RuntimeDocFacetRecord>,
    pub snapshot_entries: Vec<RuntimeSnapshotRecord>,
    pub affected_entries: Vec<RuntimeEdgeAuthorityRecord>,
    pub invalidated_by_entries: Vec<RuntimeEdgeAuthorityRecord>,
}

const RUNTIME_CATALOG_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "overlay_epoch_ms",
    "batch_digest",
    "producer_head_applied_at_ms",
    "generation_materialized_at_ms",
    "changed_entries",
    "facet_entries",
    "snapshot_entries",
    "affected_entries",
    "invalidated_by_entries",
];

impl Serialize for RuntimeCatalogIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RuntimeCatalogIngestBatch", 12)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("overlay_epoch_ms", &self.overlay_epoch_ms)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field(
            "producer_head_applied_at_ms",
            &self.producer_head_applied_at_ms,
        )?;
        state.serialize_field(
            "generation_materialized_at_ms",
            &self.generation_materialized_at_ms,
        )?;
        state.serialize_field("changed_entries", &self.changed_entries)?;
        state.serialize_field("facet_entries", &self.facet_entries)?;
        state.serialize_field("snapshot_entries", &self.snapshot_entries)?;
        state.serialize_field("affected_entries", &self.affected_entries)?;
        state.serialize_field("invalidated_by_entries", &self.invalidated_by_entries)?;
        state.end()
    }
}

struct RuntimeCatalogIngestBatchVisitor;

impl<'de> Visitor<'de> for RuntimeCatalogIngestBatchVisitor {
    type Value = RuntimeCatalogIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RuntimeCatalogIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut overlay_epoch_ms: Option<u64> = None;
        let mut batch_digest: Option<String> = None;
        let mut producer_head_applied_at_ms: Option<u64> = None;
        let mut generation_materialized_at_ms: Option<u64> = None;
        let mut changed_entries: Option<Vec<RuntimeChangedRecord>> = None;
        let mut facet_entries: Option<Vec<RuntimeDocFacetRecord>> = None;
        let mut snapshot_entries: Option<Vec<RuntimeSnapshotRecord>> = None;
        let mut affected_entries: Option<Vec<RuntimeEdgeAuthorityRecord>> = None;
        let mut invalidated_by_entries: Option<Vec<RuntimeEdgeAuthorityRecord>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "overlay_epoch_ms" => overlay_epoch_ms = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "producer_head_applied_at_ms" => {
                    producer_head_applied_at_ms = Some(map.next_value()?);
                }
                "generation_materialized_at_ms" => {
                    generation_materialized_at_ms = Some(map.next_value()?);
                }
                "changed_entries" => changed_entries = Some(map.next_value()?),
                "facet_entries" => facet_entries = Some(map.next_value()?),
                "snapshot_entries" => snapshot_entries = Some(map.next_value()?),
                "affected_entries" => affected_entries = Some(map.next_value()?),
                "invalidated_by_entries" => invalidated_by_entries = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        RUNTIME_CATALOG_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(RuntimeCatalogIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            overlay_epoch_ms: overlay_epoch_ms
                .ok_or_else(|| de::Error::missing_field("overlay_epoch_ms"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            producer_head_applied_at_ms: producer_head_applied_at_ms
                .ok_or_else(|| de::Error::missing_field("producer_head_applied_at_ms"))?,
            generation_materialized_at_ms: generation_materialized_at_ms
                .ok_or_else(|| de::Error::missing_field("generation_materialized_at_ms"))?,
            changed_entries: changed_entries
                .ok_or_else(|| de::Error::missing_field("changed_entries"))?,
            facet_entries: facet_entries
                .ok_or_else(|| de::Error::missing_field("facet_entries"))?,
            snapshot_entries: snapshot_entries
                .ok_or_else(|| de::Error::missing_field("snapshot_entries"))?,
            affected_entries: affected_entries
                .ok_or_else(|| de::Error::missing_field("affected_entries"))?,
            invalidated_by_entries: invalidated_by_entries
                .ok_or_else(|| de::Error::missing_field("invalidated_by_entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RuntimeCatalogIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RuntimeCatalogIngestBatch",
            RUNTIME_CATALOG_INGEST_BATCH_FIELDS,
            RuntimeCatalogIngestBatchVisitor,
        )
    }
}

// =============================================================================
// Structural ingest batch
// =============================================================================

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralTreeRecord {
    pub chunk_id: ChunkId,
    pub record: ParseTreeRecord,
}

const STRUCTURAL_TREE_RECORD_FIELDS: &[&str] = &["chunk_id", "record"];

impl Serialize for StructuralTreeRecord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralTreeRecord", 2)?;
        state.serialize_field("chunk_id", &self.chunk_id)?;
        state.serialize_field("record", &self.record)?;
        state.end()
    }
}

struct StructuralTreeRecordVisitor;

impl<'de> Visitor<'de> for StructuralTreeRecordVisitor {
    type Value = StructuralTreeRecord;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a StructuralTreeRecord map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut chunk_id: Option<ChunkId> = None;
        let mut record: Option<ParseTreeRecord> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "chunk_id" => chunk_id = Some(map.next_value()?),
                "record" => record = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        STRUCTURAL_TREE_RECORD_FIELDS,
                    ));
                }
            }
        }
        Ok(StructuralTreeRecord {
            chunk_id: chunk_id.ok_or_else(|| de::Error::missing_field("chunk_id"))?,
            record: record.ok_or_else(|| de::Error::missing_field("record"))?,
        })
    }
}

impl<'de> Deserialize<'de> for StructuralTreeRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "StructuralTreeRecord",
            STRUCTURAL_TREE_RECORD_FIELDS,
            StructuralTreeRecordVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralReplaceScope {
    pub scope: SearchScopeKey,
    pub scope_digest: String,
    pub trees: Vec<StructuralTreeRecord>,
}

const STRUCTURAL_REPLACE_SCOPE_FIELDS: &[&str] = &["scope", "scope_digest", "trees"];

impl Serialize for StructuralReplaceScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralReplaceScope", 3)?;
        state.serialize_field("scope", &self.scope)?;
        state.serialize_field("scope_digest", &self.scope_digest)?;
        state.serialize_field("trees", &self.trees)?;
        state.end()
    }
}

struct StructuralReplaceScopeVisitor;

impl<'de> Visitor<'de> for StructuralReplaceScopeVisitor {
    type Value = StructuralReplaceScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a StructuralReplaceScope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut scope: Option<SearchScopeKey> = None;
        let mut scope_digest: Option<String> = None;
        let mut trees: Option<Vec<StructuralTreeRecord>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "scope" => {
                    if scope.is_some() {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope = Some(map.next_value()?);
                }
                "scope_digest" => {
                    if scope_digest.is_some() {
                        return Err(de::Error::duplicate_field("scope_digest"));
                    }
                    scope_digest = Some(map.next_value()?);
                }
                "trees" => {
                    if trees.is_some() {
                        return Err(de::Error::duplicate_field("trees"));
                    }
                    trees = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        STRUCTURAL_REPLACE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(StructuralReplaceScope {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
            scope_digest: scope_digest.ok_or_else(|| de::Error::missing_field("scope_digest"))?,
            trees: trees.ok_or_else(|| de::Error::missing_field("trees"))?,
        })
    }
}

impl<'de> Deserialize<'de> for StructuralReplaceScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "StructuralReplaceScope",
            STRUCTURAL_REPLACE_SCOPE_FIELDS,
            StructuralReplaceScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralTombstoneScope {
    pub scope: SearchScopeKey,
}

const STRUCTURAL_TOMBSTONE_SCOPE_FIELDS: &[&str] = &["scope"];

impl Serialize for StructuralTombstoneScope {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralTombstoneScope", 1)?;
        state.serialize_field("scope", &self.scope)?;
        state.end()
    }
}

struct StructuralTombstoneScopeVisitor;

impl<'de> Visitor<'de> for StructuralTombstoneScopeVisitor {
    type Value = StructuralTombstoneScope;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a StructuralTombstoneScope map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut scope: Option<SearchScopeKey> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "scope" => {
                    if scope.is_some() {
                        return Err(de::Error::duplicate_field("scope"));
                    }
                    scope = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        STRUCTURAL_TOMBSTONE_SCOPE_FIELDS,
                    ));
                }
            }
        }
        Ok(StructuralTombstoneScope {
            scope: scope.ok_or_else(|| de::Error::missing_field("scope"))?,
        })
    }
}

impl<'de> Deserialize<'de> for StructuralTombstoneScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "StructuralTombstoneScope",
            STRUCTURAL_TOMBSTONE_SCOPE_FIELDS,
            StructuralTombstoneScopeVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructuralIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub base_generation: Option<ManifestGeneration>,
    pub manifest_digest: String,
    pub batch_digest: String,
    pub mode: BatchIngestMode,
    pub replace_scopes: Vec<StructuralReplaceScope>,
    pub tombstone_scopes: Vec<StructuralTombstoneScope>,
    pub seal: bool,
}

const STRUCTURAL_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "base_generation",
    "manifest_digest",
    "batch_digest",
    "mode",
    "replace_scopes",
    "tombstone_scopes",
    "seal",
];

impl Serialize for StructuralIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("StructuralIngestBatch", 10)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("base_generation", &self.base_generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("replace_scopes", &self.replace_scopes)?;
        state.serialize_field("tombstone_scopes", &self.tombstone_scopes)?;
        state.serialize_field("seal", &self.seal)?;
        state.end()
    }
}

struct StructuralIngestBatchVisitor;

impl<'de> Visitor<'de> for StructuralIngestBatchVisitor {
    type Value = StructuralIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a StructuralIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut base_generation: Option<Option<ManifestGeneration>> = None;
        let mut manifest_digest: Option<String> = None;
        let mut batch_digest: Option<String> = None;
        let mut mode: Option<BatchIngestMode> = None;
        let mut replace_scopes: Option<Vec<StructuralReplaceScope>> = None;
        let mut tombstone_scopes: Option<Vec<StructuralTombstoneScope>> = None;
        let mut seal: Option<bool> = None;
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
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
                }
                "base_generation" => {
                    if base_generation.is_some() {
                        return Err(de::Error::duplicate_field("base_generation"));
                    }
                    base_generation = Some(map.next_value()?);
                }
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                "batch_digest" => {
                    if batch_digest.is_some() {
                        return Err(de::Error::duplicate_field("batch_digest"));
                    }
                    batch_digest = Some(map.next_value()?);
                }
                "mode" => {
                    if mode.is_some() {
                        return Err(de::Error::duplicate_field("mode"));
                    }
                    mode = Some(map.next_value()?);
                }
                "replace_scopes" => {
                    if replace_scopes.is_some() {
                        return Err(de::Error::duplicate_field("replace_scopes"));
                    }
                    replace_scopes = Some(map.next_value()?);
                }
                "tombstone_scopes" => {
                    if tombstone_scopes.is_some() {
                        return Err(de::Error::duplicate_field("tombstone_scopes"));
                    }
                    tombstone_scopes = Some(map.next_value()?);
                }
                "seal" => {
                    if seal.is_some() {
                        return Err(de::Error::duplicate_field("seal"));
                    }
                    seal = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        STRUCTURAL_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(StructuralIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            base_generation: base_generation
                .ok_or_else(|| de::Error::missing_field("base_generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            mode: mode.ok_or_else(|| de::Error::missing_field("mode"))?,
            replace_scopes: replace_scopes
                .ok_or_else(|| de::Error::missing_field("replace_scopes"))?,
            tombstone_scopes: tombstone_scopes
                .ok_or_else(|| de::Error::missing_field("tombstone_scopes"))?,
            seal: seal.ok_or_else(|| de::Error::missing_field("seal"))?,
        })
    }
}

impl<'de> Deserialize<'de> for StructuralIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "StructuralIngestBatch",
            STRUCTURAL_INGEST_BATCH_FIELDS,
            StructuralIngestBatchVisitor,
        )
    }
}
