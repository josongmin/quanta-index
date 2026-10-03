//! History and repository-metadata ingest wire DTOs.

use crate::lex::{CommitRecord, CommitSha, DiffHunkRecord};
use crate::{ManifestGeneration, RepoId, RepoRelativePath, RevisionId};
use core::fmt;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, VariantAccess, Visitor},
    ser::SerializeStruct,
};

// =============================================================================
// History ingest batch
// =============================================================================

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryRefUpsert {
    pub name: Box<str>,
    pub sha: CommitSha,
}

const HISTORY_REF_UPSERT_FIELDS: &[&str] = &["name", "sha"];

impl Serialize for HistoryRefUpsert {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HistoryRefUpsert", 2)?;
        state.serialize_field("name", self.name.as_ref())?;
        state.serialize_field("sha", &self.sha)?;
        state.end()
    }
}

struct HistoryRefUpsertVisitor;

impl<'de> Visitor<'de> for HistoryRefUpsertVisitor {
    type Value = HistoryRefUpsert;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryRefUpsert map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut name: Option<String> = None;
        let mut sha: Option<CommitSha> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "name" => {
                    if name.is_some() {
                        return Err(de::Error::duplicate_field("name"));
                    }
                    name = Some(map.next_value()?);
                }
                "sha" => {
                    if sha.is_some() {
                        return Err(de::Error::duplicate_field("sha"));
                    }
                    sha = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, HISTORY_REF_UPSERT_FIELDS)),
            }
        }
        Ok(HistoryRefUpsert {
            name: name
                .ok_or_else(|| de::Error::missing_field("name"))?
                .into_boxed_str(),
            sha: sha.ok_or_else(|| de::Error::missing_field("sha"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HistoryRefUpsert {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HistoryRefUpsert",
            HISTORY_REF_UPSERT_FIELDS,
            HistoryRefUpsertVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryRefDelete {
    pub name: Box<str>,
}

const HISTORY_REF_DELETE_FIELDS: &[&str] = &["name"];

impl Serialize for HistoryRefDelete {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HistoryRefDelete", 1)?;
        state.serialize_field("name", self.name.as_ref())?;
        state.end()
    }
}

struct HistoryRefDeleteVisitor;

impl<'de> Visitor<'de> for HistoryRefDeleteVisitor {
    type Value = HistoryRefDelete;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryRefDelete map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut name: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "name" => {
                    if name.is_some() {
                        return Err(de::Error::duplicate_field("name"));
                    }
                    name = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, HISTORY_REF_DELETE_FIELDS)),
            }
        }
        Ok(HistoryRefDelete {
            name: name
                .ok_or_else(|| de::Error::missing_field("name"))?
                .into_boxed_str(),
        })
    }
}

impl<'de> Deserialize<'de> for HistoryRefDelete {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HistoryRefDelete",
            HISTORY_REF_DELETE_FIELDS,
            HistoryRefDeleteVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HistoryRefMutation {
    Upsert(HistoryRefUpsert),
    Delete(HistoryRefDelete),
}

const HISTORY_REF_MUTATION_VARIANTS: &[&str] = &["Upsert", "Delete"];

impl Serialize for HistoryRefMutation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Upsert(payload) => {
                serializer.serialize_newtype_variant("HistoryRefMutation", 0, "Upsert", payload)
            }
            Self::Delete(payload) => {
                serializer.serialize_newtype_variant("HistoryRefMutation", 1, "Delete", payload)
            }
        }
    }
}

struct HistoryRefMutationVisitor;

impl<'de> Visitor<'de> for HistoryRefMutationVisitor {
    type Value = HistoryRefMutation;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryRefMutation enum")
    }

    fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::EnumAccess<'de>,
    {
        let (tag, variant) = data.variant::<String>()?;
        match tag.as_str() {
            "Upsert" => Ok(HistoryRefMutation::Upsert(variant.newtype_variant()?)),
            "Delete" => Ok(HistoryRefMutation::Delete(variant.newtype_variant()?)),
            other => Err(de::Error::unknown_variant(
                other,
                HISTORY_REF_MUTATION_VARIANTS,
            )),
        }
    }
}

impl<'de> Deserialize<'de> for HistoryRefMutation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_enum(
            "HistoryRefMutation",
            HISTORY_REF_MUTATION_VARIANTS,
            HistoryRefMutationVisitor,
        )
    }
}

pub type HistoryTagUpsert = HistoryRefUpsert;
pub type HistoryTagDelete = HistoryRefDelete;
pub type HistoryTagMutation = HistoryRefMutation;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryDiffHunkUpsert {
    pub commit_sha: CommitSha,
    pub file_path: Box<str>,
    pub record: DiffHunkRecord,
}

const HISTORY_DIFF_HUNK_UPSERT_FIELDS: &[&str] = &["commit_sha", "file_path", "record"];

impl Serialize for HistoryDiffHunkUpsert {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HistoryDiffHunkUpsert", 3)?;
        state.serialize_field("commit_sha", &self.commit_sha)?;
        state.serialize_field("file_path", self.file_path.as_ref())?;
        state.serialize_field("record", &self.record)?;
        state.end()
    }
}

struct HistoryDiffHunkUpsertVisitor;

impl<'de> Visitor<'de> for HistoryDiffHunkUpsertVisitor {
    type Value = HistoryDiffHunkUpsert;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryDiffHunkUpsert map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut commit_sha: Option<CommitSha> = None;
        let mut file_path: Option<String> = None;
        let mut record: Option<DiffHunkRecord> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "commit_sha" => {
                    if commit_sha.is_some() {
                        return Err(de::Error::duplicate_field("commit_sha"));
                    }
                    commit_sha = Some(map.next_value()?);
                }
                "file_path" => {
                    if file_path.is_some() {
                        return Err(de::Error::duplicate_field("file_path"));
                    }
                    file_path = Some(map.next_value()?);
                }
                "record" => {
                    if record.is_some() {
                        return Err(de::Error::duplicate_field("record"));
                    }
                    record = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        HISTORY_DIFF_HUNK_UPSERT_FIELDS,
                    ));
                }
            }
        }
        Ok(HistoryDiffHunkUpsert {
            commit_sha: commit_sha.ok_or_else(|| de::Error::missing_field("commit_sha"))?,
            file_path: file_path
                .ok_or_else(|| de::Error::missing_field("file_path"))?
                .into_boxed_str(),
            record: record.ok_or_else(|| de::Error::missing_field("record"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HistoryDiffHunkUpsert {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HistoryDiffHunkUpsert",
            HISTORY_DIFF_HUNK_UPSERT_FIELDS,
            HistoryDiffHunkUpsertVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub manifest_digest: Option<String>,
    pub batch_digest: String,
    pub commits: Vec<CommitRecord>,
    pub refs: Vec<HistoryRefMutation>,
    pub tags: Vec<HistoryTagMutation>,
    pub diff_hunks: Vec<HistoryDiffHunkUpsert>,
}

const HISTORY_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "manifest_digest",
    "batch_digest",
    "commits",
    "refs",
    "tags",
    "diff_hunks",
];

impl Serialize for HistoryIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("HistoryIngestBatch", 9)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("manifest_digest", &self.manifest_digest)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("commits", &self.commits)?;
        state.serialize_field("refs", &self.refs)?;
        state.serialize_field("tags", &self.tags)?;
        state.serialize_field("diff_hunks", &self.diff_hunks)?;
        state.end()
    }
}

struct HistoryIngestBatchVisitor;

impl<'de> Visitor<'de> for HistoryIngestBatchVisitor {
    type Value = HistoryIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a HistoryIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut manifest_digest: Option<Option<String>> = None;
        let mut batch_digest: Option<String> = None;
        let mut commits: Option<Vec<CommitRecord>> = None;
        let mut refs: Option<Vec<HistoryRefMutation>> = None;
        let mut tags: Option<Vec<HistoryTagMutation>> = None;
        let mut diff_hunks: Option<Vec<HistoryDiffHunkUpsert>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "manifest_digest" => manifest_digest = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "commits" => commits = Some(map.next_value()?),
                "refs" => refs = Some(map.next_value()?),
                "tags" => tags = Some(map.next_value()?),
                "diff_hunks" => diff_hunks = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, HISTORY_INGEST_BATCH_FIELDS)),
            }
        }
        Ok(HistoryIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            manifest_digest: manifest_digest
                .ok_or_else(|| de::Error::missing_field("manifest_digest"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            commits: commits.ok_or_else(|| de::Error::missing_field("commits"))?,
            refs: refs.ok_or_else(|| de::Error::missing_field("refs"))?,
            tags: tags.ok_or_else(|| de::Error::missing_field("tags"))?,
            diff_hunks: diff_hunks.ok_or_else(|| de::Error::missing_field("diff_hunks"))?,
        })
    }
}

impl<'de> Deserialize<'de> for HistoryIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "HistoryIngestBatch",
            HISTORY_INGEST_BATCH_FIELDS,
            HistoryIngestBatchVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoCommitRecencyEntry {
    pub source_repo_id: RepoId,
    pub latest_committer_time_ms: u64,
}

const REPO_COMMIT_RECENCY_ENTRY_FIELDS: &[&str] = &["source_repo_id", "latest_committer_time_ms"];

impl Serialize for RepoCommitRecencyEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoCommitRecencyEntry", 2)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("latest_committer_time_ms", &self.latest_committer_time_ms)?;
        state.end()
    }
}

struct RepoCommitRecencyEntryVisitor;

impl<'de> Visitor<'de> for RepoCommitRecencyEntryVisitor {
    type Value = RepoCommitRecencyEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoCommitRecencyEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_repo_id: Option<RepoId> = None;
        let mut latest_committer_time_ms: Option<u64> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "source_repo_id" => source_repo_id = Some(map.next_value()?),
                "latest_committer_time_ms" => latest_committer_time_ms = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_COMMIT_RECENCY_ENTRY_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoCommitRecencyEntry {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            latest_committer_time_ms: latest_committer_time_ms
                .ok_or_else(|| de::Error::missing_field("latest_committer_time_ms"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoCommitRecencyEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoCommitRecencyEntry",
            REPO_COMMIT_RECENCY_ENTRY_FIELDS,
            RepoCommitRecencyEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoCommitRecencyIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub entries: Vec<RepoCommitRecencyEntry>,
}

const REPO_COMMIT_RECENCY_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "batch_digest",
    "entries",
];

impl Serialize for RepoCommitRecencyIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoCommitRecencyIngestBatch", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct RepoCommitRecencyIngestBatchVisitor;

impl<'de> Visitor<'de> for RepoCommitRecencyIngestBatchVisitor {
    type Value = RepoCommitRecencyIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoCommitRecencyIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<RepoCommitRecencyEntry>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_COMMIT_RECENCY_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoCommitRecencyIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoCommitRecencyIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoCommitRecencyIngestBatch",
            REPO_COMMIT_RECENCY_INGEST_BATCH_FIELDS,
            RepoCommitRecencyIngestBatchVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMetaEntry {
    pub source_repo_id: RepoId,
    pub key: String,
    pub value: String,
}

const REPO_META_ENTRY_FIELDS: &[&str] = &["source_repo_id", "key", "value"];

impl Serialize for RepoMetaEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMetaEntry", 3)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("key", &self.key)?;
        state.serialize_field("value", &self.value)?;
        state.end()
    }
}

struct RepoMetaEntryVisitor;

impl<'de> Visitor<'de> for RepoMetaEntryVisitor {
    type Value = RepoMetaEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMetaEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_repo_id: Option<RepoId> = None;
        let mut key: Option<String> = None;
        let mut value: Option<String> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "source_repo_id" => source_repo_id = Some(map.next_value()?),
                "key" => key = Some(map.next_value()?),
                "value" => value = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(other, REPO_META_ENTRY_FIELDS));
                }
            }
        }
        Ok(RepoMetaEntry {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            key: key.ok_or_else(|| de::Error::missing_field("key"))?,
            value: value.ok_or_else(|| de::Error::missing_field("value"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMetaEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMetaEntry",
            REPO_META_ENTRY_FIELDS,
            RepoMetaEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMetaIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub entries: Vec<RepoMetaEntry>,
}

const REPO_META_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "batch_digest",
    "entries",
];

impl Serialize for RepoMetaIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMetaIngestBatch", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct RepoMetaIngestBatchVisitor;

impl<'de> Visitor<'de> for RepoMetaIngestBatchVisitor {
    type Value = RepoMetaIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMetaIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<RepoMetaEntry>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_META_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoMetaIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMetaIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMetaIngestBatch",
            REPO_META_INGEST_BATCH_FIELDS,
            RepoMetaIngestBatchVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoTopicEntry {
    pub source_repo_id: RepoId,
    pub topic: String,
}

const REPO_TOPIC_ENTRY_FIELDS: &[&str] = &["source_repo_id", "topic"];

impl Serialize for RepoTopicEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoTopicEntry", 2)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("topic", &self.topic)?;
        state.end()
    }
}

struct RepoTopicEntryVisitor;

impl<'de> Visitor<'de> for RepoTopicEntryVisitor {
    type Value = RepoTopicEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoTopicEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_repo_id: Option<RepoId> = None;
        let mut topic: Option<String> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "source_repo_id" => source_repo_id = Some(map.next_value()?),
                "topic" => topic = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(other, REPO_TOPIC_ENTRY_FIELDS));
                }
            }
        }
        Ok(RepoTopicEntry {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            topic: topic.ok_or_else(|| de::Error::missing_field("topic"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoTopicEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoTopicEntry",
            REPO_TOPIC_ENTRY_FIELDS,
            RepoTopicEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoTopicIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub entries: Vec<RepoTopicEntry>,
}

const REPO_TOPIC_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "batch_digest",
    "entries",
];

impl Serialize for RepoTopicIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoTopicIngestBatch", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct RepoTopicIngestBatchVisitor;

impl<'de> Visitor<'de> for RepoTopicIngestBatchVisitor {
    type Value = RepoTopicIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoTopicIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<RepoTopicEntry>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_TOPIC_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoTopicIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoTopicIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoTopicIngestBatch",
            REPO_TOPIC_INGEST_BATCH_FIELDS,
            RepoTopicIngestBatchVisitor,
        )
    }
}

/// One producer-published repo description, keyed by source repo.
///
/// The producer is the sole authority for the description string (e.g. the
/// code-host repo description); the search plane stores it verbatim and matches
/// it as a regex at `repo:has.description(<pattern>)` query time. Distinct from
/// [`RepoMetaEntry`] (key/value tags) and [`RepoTopicEntry`] (topic set) — the
/// description is a single free-text string per repo.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoDescriptionEntry {
    pub source_repo_id: RepoId,
    pub description: String,
}

const REPO_DESCRIPTION_ENTRY_FIELDS: &[&str] = &["source_repo_id", "description"];

impl Serialize for RepoDescriptionEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoDescriptionEntry", 2)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("description", &self.description)?;
        state.end()
    }
}

struct RepoDescriptionEntryVisitor;

impl<'de> Visitor<'de> for RepoDescriptionEntryVisitor {
    type Value = RepoDescriptionEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoDescriptionEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_repo_id: Option<RepoId> = None;
        let mut description: Option<String> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "source_repo_id" => {
                    if source_repo_id.is_some() {
                        return Err(de::Error::duplicate_field("source_repo_id"));
                    }
                    source_repo_id = Some(map.next_value()?);
                }
                "description" => {
                    if description.is_some() {
                        return Err(de::Error::duplicate_field("description"));
                    }
                    description = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_DESCRIPTION_ENTRY_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoDescriptionEntry {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            description: description.ok_or_else(|| de::Error::missing_field("description"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoDescriptionEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoDescriptionEntry",
            REPO_DESCRIPTION_ENTRY_FIELDS,
            RepoDescriptionEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoDescriptionIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub entries: Vec<RepoDescriptionEntry>,
}

const REPO_DESCRIPTION_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "batch_digest",
    "entries",
];

impl Serialize for RepoDescriptionIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoDescriptionIngestBatch", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct RepoDescriptionIngestBatchVisitor;

impl<'de> Visitor<'de> for RepoDescriptionIngestBatchVisitor {
    type Value = RepoDescriptionIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoDescriptionIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<RepoDescriptionEntry>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
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
                "batch_digest" => {
                    if batch_digest.is_some() {
                        return Err(de::Error::duplicate_field("batch_digest"));
                    }
                    batch_digest = Some(map.next_value()?);
                }
                "entries" => {
                    if entries.is_some() {
                        return Err(de::Error::duplicate_field("entries"));
                    }
                    entries = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_DESCRIPTION_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoDescriptionIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoDescriptionIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoDescriptionIngestBatch",
            REPO_DESCRIPTION_INGEST_BATCH_FIELDS,
            RepoDescriptionIngestBatchVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileOwnershipEntry {
    pub source_repo_id: RepoId,
    pub repo_relative_path: RepoRelativePath,
    pub owners: Vec<String>,
}

const FILE_OWNERSHIP_ENTRY_FIELDS: &[&str] = &["source_repo_id", "repo_relative_path", "owners"];

impl Serialize for FileOwnershipEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("FileOwnershipEntry", 3)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("owners", &self.owners)?;
        state.end()
    }
}

struct FileOwnershipEntryVisitor;

impl<'de> Visitor<'de> for FileOwnershipEntryVisitor {
    type Value = FileOwnershipEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileOwnershipEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_repo_id: Option<RepoId> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut owners: Option<Vec<String>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "source_repo_id" => source_repo_id = Some(map.next_value()?),
                "repo_relative_path" => repo_relative_path = Some(map.next_value()?),
                "owners" => owners = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(other, FILE_OWNERSHIP_ENTRY_FIELDS));
                }
            }
        }
        Ok(FileOwnershipEntry {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            owners: owners.ok_or_else(|| de::Error::missing_field("owners"))?,
        })
    }
}

impl<'de> Deserialize<'de> for FileOwnershipEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileOwnershipEntry",
            FILE_OWNERSHIP_ENTRY_FIELDS,
            FileOwnershipEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileOwnershipIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub entries: Vec<FileOwnershipEntry>,
}

const FILE_OWNERSHIP_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "batch_digest",
    "entries",
];

impl Serialize for FileOwnershipIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("FileOwnershipIngestBatch", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct FileOwnershipIngestBatchVisitor;

impl<'de> Visitor<'de> for FileOwnershipIngestBatchVisitor {
    type Value = FileOwnershipIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileOwnershipIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<FileOwnershipEntry>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        FILE_OWNERSHIP_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(FileOwnershipIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for FileOwnershipIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileOwnershipIngestBatch",
            FILE_OWNERSHIP_INGEST_BATCH_FIELDS,
            FileOwnershipIngestBatchVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FileContributorIdentityEntry {
    pub canonical: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

const FILE_CONTRIBUTOR_IDENTITY_ENTRY_FIELDS: &[&str] = &["canonical", "name", "email"];

impl Serialize for FileContributorIdentityEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("FileContributorIdentityEntry", 3)?;
        state.serialize_field("canonical", &self.canonical)?;
        state.serialize_field("name", &self.name)?;
        state.serialize_field("email", &self.email)?;
        state.end()
    }
}

struct FileContributorIdentityEntryVisitor;

impl<'de> Visitor<'de> for FileContributorIdentityEntryVisitor {
    type Value = FileContributorIdentityEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileContributorIdentityEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut canonical: Option<String> = None;
        let mut name: Option<Option<String>> = None;
        let mut email: Option<Option<String>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "canonical" => {
                    if canonical.is_some() {
                        return Err(de::Error::duplicate_field("canonical"));
                    }
                    canonical = Some(map.next_value()?);
                }
                "name" => {
                    if name.is_some() {
                        return Err(de::Error::duplicate_field("name"));
                    }
                    name = Some(map.next_value()?);
                }
                "email" => {
                    if email.is_some() {
                        return Err(de::Error::duplicate_field("email"));
                    }
                    email = Some(map.next_value()?);
                }
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        FILE_CONTRIBUTOR_IDENTITY_ENTRY_FIELDS,
                    ));
                }
            }
        }
        Ok(FileContributorIdentityEntry {
            canonical: canonical.ok_or_else(|| de::Error::missing_field("canonical"))?,
            name: name.ok_or_else(|| de::Error::missing_field("name"))?,
            email: email.ok_or_else(|| de::Error::missing_field("email"))?,
        })
    }
}

impl<'de> Deserialize<'de> for FileContributorIdentityEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileContributorIdentityEntry",
            FILE_CONTRIBUTOR_IDENTITY_ENTRY_FIELDS,
            FileContributorIdentityEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileContributorEntry {
    pub source_repo_id: RepoId,
    pub repo_relative_path: RepoRelativePath,
    pub contributors: Vec<FileContributorIdentityEntry>,
}

const FILE_CONTRIBUTOR_ENTRY_FIELDS: &[&str] =
    &["source_repo_id", "repo_relative_path", "contributors"];

impl Serialize for FileContributorEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("FileContributorEntry", 3)?;
        state.serialize_field("source_repo_id", &self.source_repo_id)?;
        state.serialize_field("repo_relative_path", &self.repo_relative_path)?;
        state.serialize_field("contributors", &self.contributors)?;
        state.end()
    }
}

struct FileContributorEntryVisitor;

impl<'de> Visitor<'de> for FileContributorEntryVisitor {
    type Value = FileContributorEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileContributorEntry map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut source_repo_id: Option<RepoId> = None;
        let mut repo_relative_path: Option<RepoRelativePath> = None;
        let mut contributors: Option<Vec<FileContributorIdentityEntry>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "source_repo_id" => source_repo_id = Some(map.next_value()?),
                "repo_relative_path" => repo_relative_path = Some(map.next_value()?),
                "contributors" => contributors = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        FILE_CONTRIBUTOR_ENTRY_FIELDS,
                    ));
                }
            }
        }
        Ok(FileContributorEntry {
            source_repo_id: source_repo_id
                .ok_or_else(|| de::Error::missing_field("source_repo_id"))?,
            repo_relative_path: repo_relative_path
                .ok_or_else(|| de::Error::missing_field("repo_relative_path"))?,
            contributors: contributors.ok_or_else(|| de::Error::missing_field("contributors"))?,
        })
    }
}

impl<'de> Deserialize<'de> for FileContributorEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileContributorEntry",
            FILE_CONTRIBUTOR_ENTRY_FIELDS,
            FileContributorEntryVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileContributorIngestBatch {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
    pub entries: Vec<FileContributorEntry>,
}

const FILE_CONTRIBUTOR_INGEST_BATCH_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "generation",
    "batch_digest",
    "entries",
];

impl Serialize for FileContributorIngestBatch {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("FileContributorIngestBatch", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("batch_digest", &self.batch_digest)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct FileContributorIngestBatchVisitor;

impl<'de> Visitor<'de> for FileContributorIngestBatchVisitor {
    type Value = FileContributorIngestBatch;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a FileContributorIngestBatch map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut generation: Option<ManifestGeneration> = None;
        let mut batch_digest: Option<String> = None;
        let mut entries: Option<Vec<FileContributorEntry>> = None;
        while let Some(field) = map.next_key::<String>()? {
            match field.as_str() {
                "repo_id" => repo_id = Some(map.next_value()?),
                "revision_id" => revision_id = Some(map.next_value()?),
                "generation" => generation = Some(map.next_value()?),
                "batch_digest" => batch_digest = Some(map.next_value()?),
                "entries" => entries = Some(map.next_value()?),
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        FILE_CONTRIBUTOR_INGEST_BATCH_FIELDS,
                    ));
                }
            }
        }
        Ok(FileContributorIngestBatch {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
            batch_digest: batch_digest.ok_or_else(|| de::Error::missing_field("batch_digest"))?,
            entries: entries.ok_or_else(|| de::Error::missing_field("entries"))?,
        })
    }
}

impl<'de> Deserialize<'de> for FileContributorIngestBatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "FileContributorIngestBatch",
            FILE_CONTRIBUTOR_INGEST_BATCH_FIELDS,
            FileContributorIngestBatchVisitor,
        )
    }
}
