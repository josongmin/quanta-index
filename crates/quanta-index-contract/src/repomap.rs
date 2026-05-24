//! RepoMap contract surface.
//!
//! Query path types are IPC-facing and therefore keep manual serde. Control
//! and source-bundle types are owner-side bootstrap DTOs for the split.

use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::ids::{ManifestGeneration, RepoId, RevisionId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapSourceBundleV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub snapshot_id: String,
    pub projection_version: u32,
    pub authority_digest: String,
    pub item_index_availability: String,
    pub graph_coverage_class: String,
    pub exactness_summary: String,
    pub entry_identities: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapActivateGenerationRequestV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub manifest_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapSnapshotMetaV1 {
    pub snapshot_id: String,
    pub projection_version: u32,
    pub authority_digest: String,
    pub item_index_availability: String,
    pub graph_coverage_class: String,
    pub exactness_summary: String,
}

const SNAPSHOT_META_FIELDS: &[&str] = &[
    "snapshot_id",
    "projection_version",
    "authority_digest",
    "item_index_availability",
    "graph_coverage_class",
    "exactness_summary",
];

impl Serialize for RepoMapSnapshotMetaV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapSnapshotMetaV1", 6)?;
        state.serialize_field("snapshot_id", &self.snapshot_id)?;
        state.serialize_field("projection_version", &self.projection_version)?;
        state.serialize_field("authority_digest", &self.authority_digest)?;
        state.serialize_field("item_index_availability", &self.item_index_availability)?;
        state.serialize_field("graph_coverage_class", &self.graph_coverage_class)?;
        state.serialize_field("exactness_summary", &self.exactness_summary)?;
        state.end()
    }
}

struct SnapshotMetaVisitor;

impl<'de> Visitor<'de> for SnapshotMetaVisitor {
    type Value = RepoMapSnapshotMetaV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapSnapshotMetaV1 map")
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
                "snapshot_id" => snapshot_id = Some(map.next_value()?),
                "projection_version" => projection_version = Some(map.next_value()?),
                "authority_digest" => authority_digest = Some(map.next_value()?),
                "item_index_availability" => item_index_availability = Some(map.next_value()?),
                "graph_coverage_class" => graph_coverage_class = Some(map.next_value()?),
                "exactness_summary" => exactness_summary = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, SNAPSHOT_META_FIELDS)),
            }
        }
        Ok(RepoMapSnapshotMetaV1 {
            snapshot_id: snapshot_id.ok_or_else(|| de::Error::missing_field("snapshot_id"))?,
            projection_version: projection_version
                .ok_or_else(|| de::Error::missing_field("projection_version"))?,
            authority_digest: authority_digest
                .ok_or_else(|| de::Error::missing_field("authority_digest"))?,
            item_index_availability: item_index_availability
                .ok_or_else(|| de::Error::missing_field("item_index_availability"))?,
            graph_coverage_class: graph_coverage_class
                .ok_or_else(|| de::Error::missing_field("graph_coverage_class"))?,
            exactness_summary: exactness_summary
                .ok_or_else(|| de::Error::missing_field("exactness_summary"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapSnapshotMetaV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapSnapshotMetaV1",
            SNAPSHOT_META_FIELDS,
            SnapshotMetaVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepoMapEntryDtoV1 {
    pub subject_identity: String,
    pub subject_kind: String,
    pub owner_path: String,
    pub score: f32,
    pub rank: u32,
}

const ENTRY_FIELDS: &[&str] = &[
    "subject_identity",
    "subject_kind",
    "owner_path",
    "score",
    "rank",
];

impl Serialize for RepoMapEntryDtoV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapEntryDtoV1", 5)?;
        state.serialize_field("subject_identity", &self.subject_identity)?;
        state.serialize_field("subject_kind", &self.subject_kind)?;
        state.serialize_field("owner_path", &self.owner_path)?;
        state.serialize_field("score", &self.score)?;
        state.serialize_field("rank", &self.rank)?;
        state.end()
    }
}

struct EntryVisitor;

impl<'de> Visitor<'de> for EntryVisitor {
    type Value = RepoMapEntryDtoV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapEntryDtoV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut subject_identity: Option<String> = None;
        let mut subject_kind: Option<String> = None;
        let mut owner_path: Option<String> = None;
        let mut score: Option<f32> = None;
        let mut rank: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "subject_identity" => subject_identity = Some(map.next_value()?),
                "subject_kind" => subject_kind = Some(map.next_value()?),
                "owner_path" => owner_path = Some(map.next_value()?),
                "score" => score = Some(map.next_value()?),
                "rank" => rank = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, ENTRY_FIELDS)),
            }
        }
        Ok(RepoMapEntryDtoV1 {
            subject_identity: subject_identity
                .ok_or_else(|| de::Error::missing_field("subject_identity"))?,
            subject_kind: subject_kind.ok_or_else(|| de::Error::missing_field("subject_kind"))?,
            owner_path: owner_path.ok_or_else(|| de::Error::missing_field("owner_path"))?,
            score: score.ok_or_else(|| de::Error::missing_field("score"))?,
            rank: rank.ok_or_else(|| de::Error::missing_field("rank"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapEntryDtoV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("RepoMapEntryDtoV1", ENTRY_FIELDS, EntryVisitor)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct RepoMapQueryRequestV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub query_text: String,
    pub top_k: u32,
    pub token_budget: u32,
    pub focus_subjects: Vec<String>,
}

const REQUEST_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "query_text",
    "top_k",
    "token_budget",
    "focus_subjects",
];

impl Serialize for RepoMapQueryRequestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapQueryRequestV1", 7)?;
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

struct RequestVisitor;

impl<'de> Visitor<'de> for RequestVisitor {
    type Value = RepoMapQueryRequestV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapQueryRequestV1 map")
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
        let mut focus_subjects: Option<Vec<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "manifest_generation" => manifest_generation = Some(map.next_value()?),
                "query_text" => query_text = Some(map.next_value()?),
                "top_k" => top_k = Some(map.next_value()?),
                "token_budget" => token_budget = Some(map.next_value()?),
                "focus_subjects" => focus_subjects = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, REQUEST_FIELDS)),
            }
        }
        Ok(RepoMapQueryRequestV1 {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            manifest_generation: manifest_generation
                .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
            query_text: query_text.ok_or_else(|| de::Error::missing_field("query_text"))?,
            top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
            token_budget: token_budget.ok_or_else(|| de::Error::missing_field("token_budget"))?,
            focus_subjects: focus_subjects
                .ok_or_else(|| de::Error::missing_field("focus_subjects"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapQueryRequestV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("RepoMapQueryRequestV1", REQUEST_FIELDS, RequestVisitor)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepoMapQueryResponseV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub snapshot_meta: RepoMapSnapshotMetaV1,
    pub entries: Vec<RepoMapEntryDtoV1>,
}

const RESPONSE_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "snapshot_meta",
    "entries",
];

impl Serialize for RepoMapQueryResponseV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapQueryResponseV1", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("snapshot_meta", &self.snapshot_meta)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct ResponseVisitor;

impl<'de> Visitor<'de> for ResponseVisitor {
    type Value = RepoMapQueryResponseV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapQueryResponseV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut snapshot_meta: Option<RepoMapSnapshotMetaV1> = None;
        let mut entries: Option<Vec<RepoMapEntryDtoV1>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "manifest_generation" => manifest_generation = Some(map.next_value()?),
                "snapshot_meta" => snapshot_meta = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, RESPONSE_FIELDS)),
            }
        }
        Ok(RepoMapQueryResponseV1 {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            manifest_generation: manifest_generation
                .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
            snapshot_meta: snapshot_meta
                .ok_or_else(|| de::Error::missing_field("snapshot_meta"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapQueryResponseV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("RepoMapQueryResponseV1", RESPONSE_FIELDS, ResponseVisitor)
    }
}
