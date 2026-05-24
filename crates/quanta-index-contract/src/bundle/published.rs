use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    BundleArtifactRef, BundleMode, GenerationId, ManifestDigest, ManifestGeneration, RepoId,
    RevisionId,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedBundleOutbox {
    pub outbox_id: String,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_digest: ManifestDigest,
    pub bundle_schema_version: u32,
    pub prepared_at_ms: u64,
    pub mode: BundleMode,
    pub manifest_ref: BundleArtifactRef,
    pub base_generation: Option<ManifestGeneration>,
    pub changed_artifact_mask: u64,
}

const PREPARED_BUNDLE_OUTBOX_FIELDS: &[&str] = &[
    "outbox_id",
    "repo_id",
    "revision_id",
    "manifest_digest",
    "bundle_schema_version",
    "prepared_at_ms",
    "mode",
    "manifest_ref",
    "base_generation",
    "changed_artifact_mask",
];

impl Serialize for PreparedBundleOutbox {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 9;
        if self.base_generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("PreparedBundleOutbox", field_count)?;
        state.serialize_field("outbox_id", &self.outbox_id)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("bundle_schema_version", &self.bundle_schema_version)?;
        state.serialize_field("prepared_at_ms", &self.prepared_at_ms)?;
        state.serialize_field("mode", &self.mode)?;
        state.serialize_field("manifest_ref", &self.manifest_ref)?;
        if let Some(base_generation) = &self.base_generation {
            state.serialize_field("base_generation", base_generation)?;
        }
        state.serialize_field("changed_artifact_mask", &self.changed_artifact_mask)?;
        state.end()
    }
}

struct PreparedBundleOutboxVisitor;

impl<'de> Visitor<'de> for PreparedBundleOutboxVisitor {
    type Value = PreparedBundleOutbox;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PreparedBundleOutbox map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut outbox_id: Option<String> = None;
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_digest: Option<ManifestDigest> = None;
        let mut bundle_schema_version: Option<u32> = None;
        let mut prepared_at_ms: Option<u64> = None;
        let mut mode: Option<BundleMode> = None;
        let mut manifest_ref: Option<BundleArtifactRef> = None;
        let mut base_generation: Option<ManifestGeneration> = None;
        let mut base_generation_seen = false;
        let mut changed_artifact_mask: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "outbox_id" => {
                    if outbox_id.is_some() {
                        return Err(de::Error::duplicate_field("outbox_id"));
                    }
                    outbox_id = Some(map.next_value()?);
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
                "manifest_digest" => {
                    if manifest_digest.is_some() {
                        return Err(de::Error::duplicate_field("manifest_digest"));
                    }
                    manifest_digest = Some(map.next_value()?);
                }
                "bundle_schema_version" => {
                    if bundle_schema_version.is_some() {
                        return Err(de::Error::duplicate_field("bundle_schema_version"));
                    }
                    bundle_schema_version = Some(map.next_value()?);
                }
                "prepared_at_ms" => {
                    if prepared_at_ms.is_some() {
                        return Err(de::Error::duplicate_field("prepared_at_ms"));
                    }
                    prepared_at_ms = Some(map.next_value()?);
                }
                "mode" => {
                    if mode.is_some() {
                        return Err(de::Error::duplicate_field("mode"));
                    }
                    mode = Some(map.next_value()?);
                }
                "manifest_ref" => {
                    if manifest_ref.is_some() {
                        return Err(de::Error::duplicate_field("manifest_ref"));
                    }
                    manifest_ref = Some(map.next_value()?);
                }
                "base_generation" => {
                    if base_generation_seen {
                        return Err(de::Error::duplicate_field("base_generation"));
                    }
                    base_generation_seen = true;
                    base_generation = Some(map.next_value()?);
                }
                "changed_artifact_mask" => {
                    if changed_artifact_mask.is_some() {
                        return Err(de::Error::duplicate_field("changed_artifact_mask"));
                    }
                    changed_artifact_mask = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        PREPARED_BUNDLE_OUTBOX_FIELDS,
                    ));
                }
            }
        }
        let outbox_id = outbox_id.ok_or_else(|| de::Error::missing_field("outbox_id"))?;
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let manifest_digest =
            manifest_digest.ok_or_else(|| de::Error::missing_field("manifest_digest"))?;
        let bundle_schema_version = bundle_schema_version
            .ok_or_else(|| de::Error::missing_field("bundle_schema_version"))?;
        let prepared_at_ms =
            prepared_at_ms.ok_or_else(|| de::Error::missing_field("prepared_at_ms"))?;
        let mode = mode.ok_or_else(|| de::Error::missing_field("mode"))?;
        let manifest_ref = manifest_ref.ok_or_else(|| de::Error::missing_field("manifest_ref"))?;
        let changed_artifact_mask = changed_artifact_mask
            .ok_or_else(|| de::Error::missing_field("changed_artifact_mask"))?;
        Ok(PreparedBundleOutbox {
            outbox_id,
            repo_id,
            revision_id,
            manifest_digest,
            bundle_schema_version,
            prepared_at_ms,
            mode,
            manifest_ref,
            base_generation,
            changed_artifact_mask,
        })
    }
}

impl<'de> Deserialize<'de> for PreparedBundleOutbox {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PreparedBundleOutbox",
            PREPARED_BUNDLE_OUTBOX_FIELDS,
            PreparedBundleOutboxVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedGenerationSet {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub lexical_generation: GenerationId,
    pub symbol_generation: GenerationId,
    pub structural_generation: Option<GenerationId>,
    pub history_generation: Option<GenerationId>,
    pub semantic_generation: Option<GenerationId>,
    pub metadata_generation: Option<GenerationId>,
}

const PUBLISHED_GENERATION_SET_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "lexical_generation",
    "symbol_generation",
    "structural_generation",
    "history_generation",
    "semantic_generation",
    "metadata_generation",
];

impl Serialize for PublishedGenerationSet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 5;
        if self.structural_generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.history_generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.semantic_generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.metadata_generation.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state = serializer.serialize_struct("PublishedGenerationSet", field_count)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("lexical_generation", &self.lexical_generation)?;
        state.serialize_field("symbol_generation", &self.symbol_generation)?;
        if let Some(structural_generation) = &self.structural_generation {
            state.serialize_field("structural_generation", structural_generation)?;
        }
        if let Some(history_generation) = &self.history_generation {
            state.serialize_field("history_generation", history_generation)?;
        }
        if let Some(semantic_generation) = &self.semantic_generation {
            state.serialize_field("semantic_generation", semantic_generation)?;
        }
        if let Some(metadata_generation) = &self.metadata_generation {
            state.serialize_field("metadata_generation", metadata_generation)?;
        }
        state.end()
    }
}

struct PublishedGenerationSetVisitor;

impl<'de> Visitor<'de> for PublishedGenerationSetVisitor {
    type Value = PublishedGenerationSet;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PublishedGenerationSet map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut lexical_generation: Option<GenerationId> = None;
        let mut symbol_generation: Option<GenerationId> = None;
        let mut structural_generation: Option<GenerationId> = None;
        let mut structural_generation_seen = false;
        let mut history_generation: Option<GenerationId> = None;
        let mut history_generation_seen = false;
        let mut semantic_generation: Option<GenerationId> = None;
        let mut semantic_generation_seen = false;
        let mut metadata_generation: Option<GenerationId> = None;
        let mut metadata_generation_seen = false;
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
                "lexical_generation" => {
                    if lexical_generation.is_some() {
                        return Err(de::Error::duplicate_field("lexical_generation"));
                    }
                    lexical_generation = Some(map.next_value()?);
                }
                "symbol_generation" => {
                    if symbol_generation.is_some() {
                        return Err(de::Error::duplicate_field("symbol_generation"));
                    }
                    symbol_generation = Some(map.next_value()?);
                }
                "structural_generation" => {
                    if structural_generation_seen {
                        return Err(de::Error::duplicate_field("structural_generation"));
                    }
                    structural_generation_seen = true;
                    structural_generation = Some(map.next_value()?);
                }
                "history_generation" => {
                    if history_generation_seen {
                        return Err(de::Error::duplicate_field("history_generation"));
                    }
                    history_generation_seen = true;
                    history_generation = Some(map.next_value()?);
                }
                "semantic_generation" => {
                    if semantic_generation_seen {
                        return Err(de::Error::duplicate_field("semantic_generation"));
                    }
                    semantic_generation_seen = true;
                    semantic_generation = Some(map.next_value()?);
                }
                "metadata_generation" => {
                    if metadata_generation_seen {
                        return Err(de::Error::duplicate_field("metadata_generation"));
                    }
                    metadata_generation_seen = true;
                    metadata_generation = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        PUBLISHED_GENERATION_SET_FIELDS,
                    ));
                }
            }
        }
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let manifest_generation =
            manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?;
        let lexical_generation =
            lexical_generation.ok_or_else(|| de::Error::missing_field("lexical_generation"))?;
        let symbol_generation =
            symbol_generation.ok_or_else(|| de::Error::missing_field("symbol_generation"))?;
        Ok(PublishedGenerationSet {
            repo_id,
            revision_id,
            manifest_generation,
            lexical_generation,
            symbol_generation,
            structural_generation,
            history_generation,
            semantic_generation,
            metadata_generation,
        })
    }
}

impl<'de> Deserialize<'de> for PublishedGenerationSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PublishedGenerationSet",
            PUBLISHED_GENERATION_SET_FIELDS,
            PublishedGenerationSetVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedSearchBundleManifest {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub bundle_schema_version: u32,
    pub lexical_chunk_rows: BundleArtifactRef,
    pub symbol_rows: BundleArtifactRef,
    pub metadata_rows: Option<BundleArtifactRef>,
    pub graph_rows: Option<BundleArtifactRef>,
    pub embedding_input_views: Option<BundleArtifactRef>,
    pub embedding_records: Option<BundleArtifactRef>,
    pub mutation_delta: Option<BundleArtifactRef>,
}

const PUBLISHED_SEARCH_BUNDLE_MANIFEST_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "bundle_schema_version",
    "lexical_chunk_rows",
    "symbol_rows",
    "metadata_rows",
    "graph_rows",
    "embedding_input_views",
    "embedding_records",
    "mutation_delta",
];

impl Serialize for PublishedSearchBundleManifest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut field_count: usize = 6;
        if self.metadata_rows.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.graph_rows.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.embedding_input_views.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.embedding_records.is_some() {
            field_count = field_count.saturating_add(1);
        }
        if self.mutation_delta.is_some() {
            field_count = field_count.saturating_add(1);
        }
        let mut state =
            serializer.serialize_struct("PublishedSearchBundleManifest", field_count)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("bundle_schema_version", &self.bundle_schema_version)?;
        state.serialize_field("lexical_chunk_rows", &self.lexical_chunk_rows)?;
        state.serialize_field("symbol_rows", &self.symbol_rows)?;
        if let Some(metadata_rows) = &self.metadata_rows {
            state.serialize_field("metadata_rows", metadata_rows)?;
        }
        if let Some(graph_rows) = &self.graph_rows {
            state.serialize_field("graph_rows", graph_rows)?;
        }
        if let Some(embedding_input_views) = &self.embedding_input_views {
            state.serialize_field("embedding_input_views", embedding_input_views)?;
        }
        if let Some(embedding_records) = &self.embedding_records {
            state.serialize_field("embedding_records", embedding_records)?;
        }
        if let Some(mutation_delta) = &self.mutation_delta {
            state.serialize_field("mutation_delta", mutation_delta)?;
        }
        state.end()
    }
}

struct PublishedSearchBundleManifestVisitor;

impl<'de> Visitor<'de> for PublishedSearchBundleManifestVisitor {
    type Value = PublishedSearchBundleManifest;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a PublishedSearchBundleManifest map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut bundle_schema_version: Option<u32> = None;
        let mut lexical_chunk_rows: Option<BundleArtifactRef> = None;
        let mut symbol_rows: Option<BundleArtifactRef> = None;
        let mut metadata_rows: Option<BundleArtifactRef> = None;
        let mut metadata_rows_seen = false;
        let mut graph_rows: Option<BundleArtifactRef> = None;
        let mut graph_rows_seen = false;
        let mut embedding_input_views: Option<BundleArtifactRef> = None;
        let mut embedding_input_views_seen = false;
        let mut embedding_records: Option<BundleArtifactRef> = None;
        let mut embedding_records_seen = false;
        let mut mutation_delta: Option<BundleArtifactRef> = None;
        let mut mutation_delta_seen = false;
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
                "bundle_schema_version" => {
                    if bundle_schema_version.is_some() {
                        return Err(de::Error::duplicate_field("bundle_schema_version"));
                    }
                    bundle_schema_version = Some(map.next_value()?);
                }
                "lexical_chunk_rows" => {
                    if lexical_chunk_rows.is_some() {
                        return Err(de::Error::duplicate_field("lexical_chunk_rows"));
                    }
                    lexical_chunk_rows = Some(map.next_value()?);
                }
                "symbol_rows" => {
                    if symbol_rows.is_some() {
                        return Err(de::Error::duplicate_field("symbol_rows"));
                    }
                    symbol_rows = Some(map.next_value()?);
                }
                "metadata_rows" => {
                    if metadata_rows_seen {
                        return Err(de::Error::duplicate_field("metadata_rows"));
                    }
                    metadata_rows_seen = true;
                    metadata_rows = Some(map.next_value()?);
                }
                "graph_rows" => {
                    if graph_rows_seen {
                        return Err(de::Error::duplicate_field("graph_rows"));
                    }
                    graph_rows_seen = true;
                    graph_rows = Some(map.next_value()?);
                }
                "embedding_input_views" => {
                    if embedding_input_views_seen {
                        return Err(de::Error::duplicate_field("embedding_input_views"));
                    }
                    embedding_input_views_seen = true;
                    embedding_input_views = Some(map.next_value()?);
                }
                "embedding_records" => {
                    if embedding_records_seen {
                        return Err(de::Error::duplicate_field("embedding_records"));
                    }
                    embedding_records_seen = true;
                    embedding_records = Some(map.next_value()?);
                }
                "mutation_delta" => {
                    if mutation_delta_seen {
                        return Err(de::Error::duplicate_field("mutation_delta"));
                    }
                    mutation_delta_seen = true;
                    mutation_delta = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        PUBLISHED_SEARCH_BUNDLE_MANIFEST_FIELDS,
                    ));
                }
            }
        }
        let repo_id = repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?;
        let revision_id = revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?;
        let manifest_generation =
            manifest_generation.ok_or_else(|| de::Error::missing_field("manifest_generation"))?;
        let bundle_schema_version = bundle_schema_version
            .ok_or_else(|| de::Error::missing_field("bundle_schema_version"))?;
        let lexical_chunk_rows =
            lexical_chunk_rows.ok_or_else(|| de::Error::missing_field("lexical_chunk_rows"))?;
        let symbol_rows = symbol_rows.ok_or_else(|| de::Error::missing_field("symbol_rows"))?;
        Ok(PublishedSearchBundleManifest {
            repo_id,
            revision_id,
            manifest_generation,
            bundle_schema_version,
            lexical_chunk_rows,
            symbol_rows,
            metadata_rows,
            graph_rows,
            embedding_input_views,
            embedding_records,
            mutation_delta,
        })
    }
}

impl<'de> Deserialize<'de> for PublishedSearchBundleManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "PublishedSearchBundleManifest",
            PUBLISHED_SEARCH_BUNDLE_MANIFEST_FIELDS,
            PublishedSearchBundleManifestVisitor,
        )
    }
}
