use super::*;
use std::collections::BTreeMap;

/// Retained only to encode and reject frames from the retired query protocol.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct RepoMapFocusSubjectDto {
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
pub(crate) struct RepoMapEntryDto {
    pub subject_identity: String,
    pub subject_doc_type: RepoMapDocType,
    pub subject_kind: String,
    pub owner_path: String,
    pub score: f32,
    pub final_score_millis: u32,
    /// The entry's 1-based position on the returned page. Every returned
    /// entry is included; the response counts the rest (QI-BB-008).
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
        let mut state = serializer.serialize_struct("RepoMapEntryDto", 18)?;
        state.serialize_field("subject_identity", &self.subject_identity)?;
        state.serialize_field("subject_doc_type", &self.subject_doc_type)?;
        state.serialize_field("subject_kind", &self.subject_kind)?;
        state.serialize_field("owner_path", &self.owner_path)?;
        state.serialize_field("score", &self.score)?;
        state.serialize_field("final_score_millis", &self.final_score_millis)?;
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
pub(crate) struct RepoMapQueryRequest {
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
        state.serialize_field(
            "top_k",
            &quanta_index_contract_base::query::wire_top_k(self.top_k, serde::ser::Error::custom)?,
        )?;
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
pub(crate) struct RepoMapQueryResponse {
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
