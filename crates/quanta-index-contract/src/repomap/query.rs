use core::fmt;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::GenerationPin;

#[derive(Clone, Debug, PartialEq)]
pub struct RepoMapEntryDtoV1 {
    pub subject_identity: String,
    pub subject_kind: String,
    pub score: f32,
    pub rank: u32,
}

const REPO_MAP_ENTRY_FIELDS: &[&str] = &["subject_identity", "subject_kind", "score", "rank"];

impl Serialize for RepoMapEntryDtoV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapEntryDtoV1", 4)?;
        state.serialize_field("subject_identity", &self.subject_identity)?;
        state.serialize_field("subject_kind", &self.subject_kind)?;
        state.serialize_field("score", &self.score)?;
        state.serialize_field("rank", &self.rank)?;
        state.end()
    }
}

struct RepoMapEntryVisitor;

impl<'de> Visitor<'de> for RepoMapEntryVisitor {
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
        let mut score: Option<f32> = None;
        let mut rank: Option<u32> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "subject_identity" => {
                    if subject_identity.is_some() {
                        return Err(de::Error::duplicate_field("subject_identity"));
                    }
                    subject_identity = Some(map.next_value()?);
                }
                "subject_kind" => {
                    if subject_kind.is_some() {
                        return Err(de::Error::duplicate_field("subject_kind"));
                    }
                    subject_kind = Some(map.next_value()?);
                }
                "score" => {
                    if score.is_some() {
                        return Err(de::Error::duplicate_field("score"));
                    }
                    score = Some(map.next_value()?);
                }
                "rank" => {
                    if rank.is_some() {
                        return Err(de::Error::duplicate_field("rank"));
                    }
                    rank = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, REPO_MAP_ENTRY_FIELDS)),
            }
        }
        Ok(RepoMapEntryDtoV1 {
            subject_identity: subject_identity
                .ok_or_else(|| de::Error::missing_field("subject_identity"))?,
            subject_kind: subject_kind.ok_or_else(|| de::Error::missing_field("subject_kind"))?,
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
        deserializer.deserialize_struct("RepoMapEntryDtoV1", REPO_MAP_ENTRY_FIELDS, RepoMapEntryVisitor)
    }
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

const REPO_MAP_SNAPSHOT_META_FIELDS: &[&str] = &[
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

struct RepoMapSnapshotMetaVisitor;

impl<'de> Visitor<'de> for RepoMapSnapshotMetaVisitor {
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
                        REPO_MAP_SNAPSHOT_META_FIELDS,
                    ));
                }
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
            REPO_MAP_SNAPSHOT_META_FIELDS,
            RepoMapSnapshotMetaVisitor,
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapQueryRequestV1 {
    pub generation: GenerationPin,
    pub query_text: String,
    pub top_k: u32,
    pub token_budget: u32,
    pub focus_subjects: Vec<String>,
}

const REPO_MAP_QUERY_REQUEST_FIELDS: &[&str] = &[
    "generation",
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
        let mut state = serializer.serialize_struct("RepoMapQueryRequestV1", 5)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("query_text", &self.query_text)?;
        state.serialize_field("top_k", &self.top_k)?;
        state.serialize_field("token_budget", &self.token_budget)?;
        state.serialize_field("focus_subjects", &self.focus_subjects)?;
        state.end()
    }
}

struct RepoMapQueryRequestVisitor;

impl<'de> Visitor<'de> for RepoMapQueryRequestVisitor {
    type Value = RepoMapQueryRequestV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapQueryRequestV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut query_text: Option<String> = None;
        let mut top_k: Option<u32> = None;
        let mut token_budget: Option<u32> = None;
        let mut focus_subjects: Option<Vec<String>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
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
                        REPO_MAP_QUERY_REQUEST_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoMapQueryRequestV1 {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
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
        deserializer.deserialize_struct(
            "RepoMapQueryRequestV1",
            REPO_MAP_QUERY_REQUEST_FIELDS,
            RepoMapQueryRequestVisitor,
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepoMapQueryResponseV1 {
    pub generation: GenerationPin,
    pub snapshot_meta: RepoMapSnapshotMetaV1,
    pub entries: Vec<RepoMapEntryDtoV1>,
}

const REPO_MAP_QUERY_RESPONSE_FIELDS: &[&str] = &["generation", "snapshot_meta", "entries"];

impl Serialize for RepoMapQueryResponseV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapQueryResponseV1", 3)?;
        state.serialize_field("generation", &self.generation)?;
        state.serialize_field("snapshot_meta", &self.snapshot_meta)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct RepoMapQueryResponseVisitor;

impl<'de> Visitor<'de> for RepoMapQueryResponseVisitor {
    type Value = RepoMapQueryResponseV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapQueryResponseV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut generation: Option<GenerationPin> = None;
        let mut snapshot_meta: Option<RepoMapSnapshotMetaV1> = None;
        let mut entries: Option<Vec<RepoMapEntryDtoV1>> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "generation" => {
                    if generation.is_some() {
                        return Err(de::Error::duplicate_field("generation"));
                    }
                    generation = Some(map.next_value()?);
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
                other => {
                    return Err(de::Error::unknown_field(
                        other,
                        REPO_MAP_QUERY_RESPONSE_FIELDS,
                    ));
                }
            }
        }
        Ok(RepoMapQueryResponseV1 {
            generation: generation.ok_or_else(|| de::Error::missing_field("generation"))?,
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
        deserializer.deserialize_struct(
            "RepoMapQueryResponseV1",
            REPO_MAP_QUERY_RESPONSE_FIELDS,
            RepoMapQueryResponseVisitor,
        )
    }
}
