use core::fmt;
use std::collections::BTreeMap;

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapDocType, RepoMapEntryDto, RepoMapRedactionState,
    RepoMapSnapshotMeta, RevisionId,
};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

#[derive(Clone, Debug, PartialEq)]
pub struct RepoMapEntry {
    pub subject_identity: String,
    pub subject_doc_type: RepoMapDocType,
    pub subject_kind: String,
    pub owner_path: String,
    pub score: f32,
    pub final_score_millis: u32,
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
    pub search_text: String,
    pub source_symbol_count: u32,
    pub source_chunk_token_total: u32,
    pub source_call_incoming_edges: u32,
    pub source_call_outgoing_edges: u32,
    pub source_import_incoming_edges: u32,
    pub source_import_outgoing_edges: u32,
}

const REPOMAP_ENTRY_FIELDS: &[&str] = &[
    "subject_identity",
    "subject_doc_type",
    "subject_kind",
    "owner_path",
    "score",
    "final_score_millis",
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
    "search_text",
    "source_symbol_count",
    "source_chunk_token_total",
    "source_call_incoming_edges",
    "source_call_outgoing_edges",
    "source_import_incoming_edges",
    "source_import_outgoing_edges",
];

impl Serialize for RepoMapEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapEntry", 24)?;
        state.serialize_field("subject_identity", &self.subject_identity)?;
        state.serialize_field("subject_doc_type", &self.subject_doc_type)?;
        state.serialize_field("subject_kind", &self.subject_kind)?;
        state.serialize_field("owner_path", &self.owner_path)?;
        state.serialize_field("score", &self.score)?;
        state.serialize_field("final_score_millis", &self.final_score_millis)?;
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
        state.serialize_field("search_text", &self.search_text)?;
        state.serialize_field("source_symbol_count", &self.source_symbol_count)?;
        state.serialize_field("source_chunk_token_total", &self.source_chunk_token_total)?;
        state.serialize_field(
            "source_call_incoming_edges",
            &self.source_call_incoming_edges,
        )?;
        state.serialize_field(
            "source_call_outgoing_edges",
            &self.source_call_outgoing_edges,
        )?;
        state.serialize_field(
            "source_import_incoming_edges",
            &self.source_import_incoming_edges,
        )?;
        state.serialize_field(
            "source_import_outgoing_edges",
            &self.source_import_outgoing_edges,
        )?;
        state.end()
    }
}

struct RepoMapEntryVisitor;

impl<'de> Visitor<'de> for RepoMapEntryVisitor {
    type Value = RepoMapEntry;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapEntry map")
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
        let mut search_text = String::new();
        let mut search_text_seen = false;
        let mut source_symbol_count = 0u32;
        let mut source_symbol_count_seen = false;
        let mut source_chunk_token_total = 0u32;
        let mut source_chunk_token_total_seen = false;
        let mut source_call_incoming_edges = 0u32;
        let mut source_call_incoming_edges_seen = false;
        let mut source_call_outgoing_edges = 0u32;
        let mut source_call_outgoing_edges_seen = false;
        let mut source_import_incoming_edges = 0u32;
        let mut source_import_incoming_edges_seen = false;
        let mut source_import_outgoing_edges = 0u32;
        let mut source_import_outgoing_edges_seen = false;
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
                "search_text" => {
                    if search_text_seen {
                        return Err(de::Error::duplicate_field("search_text"));
                    }
                    search_text_seen = true;
                    search_text = map.next_value()?;
                }
                "source_symbol_count" => {
                    if source_symbol_count_seen {
                        return Err(de::Error::duplicate_field("source_symbol_count"));
                    }
                    source_symbol_count_seen = true;
                    source_symbol_count = map.next_value()?;
                }
                "source_chunk_token_total" => {
                    if source_chunk_token_total_seen {
                        return Err(de::Error::duplicate_field("source_chunk_token_total"));
                    }
                    source_chunk_token_total_seen = true;
                    source_chunk_token_total = map.next_value()?;
                }
                "source_call_incoming_edges" => {
                    if source_call_incoming_edges_seen {
                        return Err(de::Error::duplicate_field("source_call_incoming_edges"));
                    }
                    source_call_incoming_edges_seen = true;
                    source_call_incoming_edges = map.next_value()?;
                }
                "source_call_outgoing_edges" => {
                    if source_call_outgoing_edges_seen {
                        return Err(de::Error::duplicate_field("source_call_outgoing_edges"));
                    }
                    source_call_outgoing_edges_seen = true;
                    source_call_outgoing_edges = map.next_value()?;
                }
                "source_import_incoming_edges" => {
                    if source_import_incoming_edges_seen {
                        return Err(de::Error::duplicate_field("source_import_incoming_edges"));
                    }
                    source_import_incoming_edges_seen = true;
                    source_import_incoming_edges = map.next_value()?;
                }
                "source_import_outgoing_edges" => {
                    if source_import_outgoing_edges_seen {
                        return Err(de::Error::duplicate_field("source_import_outgoing_edges"));
                    }
                    source_import_outgoing_edges_seen = true;
                    source_import_outgoing_edges = map.next_value()?;
                }
                _other => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        Ok(RepoMapEntry {
            subject_identity: subject_identity
                .ok_or_else(|| de::Error::missing_field("subject_identity"))?,
            subject_doc_type: subject_doc_type
                .ok_or_else(|| de::Error::missing_field("subject_doc_type"))?,
            subject_kind: subject_kind.ok_or_else(|| de::Error::missing_field("subject_kind"))?,
            owner_path: owner_path.ok_or_else(|| de::Error::missing_field("owner_path"))?,
            score: score.ok_or_else(|| de::Error::missing_field("score"))?,
            final_score_millis: final_score_millis
                .ok_or_else(|| de::Error::missing_field("final_score_millis"))?,
            importance_score_millis: importance_score_millis
                .ok_or_else(|| de::Error::missing_field("importance_score_millis"))?,
            utility_score_millis: utility_score_millis
                .ok_or_else(|| de::Error::missing_field("utility_score_millis"))?,
            freshness_score_millis: freshness_score_millis
                .ok_or_else(|| de::Error::missing_field("freshness_score_millis"))?,
            evidence_priority_millis: evidence_priority_millis
                .ok_or_else(|| de::Error::missing_field("evidence_priority_millis"))?,
            token_budget_hint: token_budget_hint
                .ok_or_else(|| de::Error::missing_field("token_budget_hint"))?,
            contributing_signals: contributing_signals
                .ok_or_else(|| de::Error::missing_field("contributing_signals"))?,
            projection_evidence_kind: projection_evidence_kind
                .ok_or_else(|| de::Error::missing_field("projection_evidence_kind"))?,
            projection_authority_artifact_id: projection_authority_artifact_id
                .ok_or_else(|| de::Error::missing_field("projection_authority_artifact_id"))?,
            projection_authority_digest: projection_authority_digest
                .ok_or_else(|| de::Error::missing_field("projection_authority_digest"))?,
            projection_status: projection_status
                .ok_or_else(|| de::Error::missing_field("projection_status"))?,
            redaction_state: redaction_state
                .ok_or_else(|| de::Error::missing_field("redaction_state"))?,
            search_text,
            source_symbol_count,
            source_chunk_token_total,
            source_call_incoming_edges,
            source_call_outgoing_edges,
            source_import_incoming_edges,
            source_import_outgoing_edges,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct("RepoMapEntry", REPOMAP_ENTRY_FIELDS, RepoMapEntryVisitor)
    }
}

impl RepoMapEntry {
    #[must_use]
    /// The wire row for this entry at query-time `rank`.
    pub fn to_dto(&self, rank: u32) -> RepoMapEntryDto {
        RepoMapEntryDto {
            subject_identity: self.subject_identity.clone(),
            subject_doc_type: self.subject_doc_type,
            subject_kind: self.subject_kind.clone(),
            owner_path: self.owner_path.clone(),
            score: self.score,
            final_score_millis: self.final_score_millis,
            rank,
            importance_score_millis: self.importance_score_millis,
            utility_score_millis: self.utility_score_millis,
            freshness_score_millis: self.freshness_score_millis,
            evidence_priority_millis: self.evidence_priority_millis,
            token_budget_hint: self.token_budget_hint,
            contributing_signals: self.contributing_signals.clone(),
            projection_evidence_kind: self.projection_evidence_kind.clone(),
            projection_authority_artifact_id: self.projection_authority_artifact_id.clone(),
            projection_authority_digest: self.projection_authority_digest.clone(),
            projection_status: self.projection_status.clone(),
            redaction_state: self.redaction_state,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepoMapSnapshot {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
    pub snapshot_meta: RepoMapSnapshotMeta,
    pub entries: Vec<RepoMapEntry>,
}

/// A snapshot with what a query needs precomputed once (QI-BB-008): the
/// folded search text of every entry, the position of every subject and the
/// positions under every owner path.
#[derive(Debug)]
pub struct RepoMapIndexedSnapshot {
    pub snapshot: RepoMapSnapshot,
    pub index: RepoMapSnapshotIndex,
}

/// Query-time lookups over a snapshot's entries, by position.
#[derive(Debug, Default)]
pub struct RepoMapSnapshotIndex {
    /// `search_text` lower-cased, one per entry, so a query matches terms
    /// without allocating per entry.
    pub folded_search_text: Vec<String>,
    /// Entry position by subject identity, then document type, so a lookup
    /// borrows the request's strings instead of allocating a key.
    pub by_subject: BTreeMap<String, BTreeMap<RepoMapDocType, usize>>,
    pub by_owner_path: BTreeMap<String, Vec<usize>>,
}

impl RepoMapIndexedSnapshot {
    #[must_use]
    pub fn new(snapshot: RepoMapSnapshot) -> Self {
        let index = RepoMapSnapshotIndex::build(&snapshot.entries);
        Self { snapshot, index }
    }
}

impl RepoMapSnapshotIndex {
    #[must_use]
    pub fn build(entries: &[RepoMapEntry]) -> Self {
        let mut index = Self {
            folded_search_text: Vec::with_capacity(entries.len()),
            by_subject: BTreeMap::new(),
            by_owner_path: BTreeMap::new(),
        };
        for (position, entry) in entries.iter().enumerate() {
            index
                .folded_search_text
                .push(entry.search_text.to_ascii_lowercase());
            let _prior = index
                .by_subject
                .entry(entry.subject_identity.clone())
                .or_default()
                .insert(entry.subject_doc_type, position);
            index
                .by_owner_path
                .entry(entry.owner_path.clone())
                .or_default()
                .push(position);
        }
        index
    }
}

const REPOMAP_SNAPSHOT_FIELDS: &[&str] = &[
    "repo_id",
    "revision_id",
    "manifest_generation",
    "snapshot_meta",
    "entries",
];

impl Serialize for RepoMapSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapSnapshot", 5)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.serialize_field("snapshot_meta", &self.snapshot_meta)?;
        state.serialize_field("entries", &self.entries)?;
        state.end()
    }
}

struct RepoMapSnapshotVisitor;

impl<'de> Visitor<'de> for RepoMapSnapshotVisitor {
    type Value = RepoMapSnapshot;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapSnapshot map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<RepoId> = None;
        let mut revision_id: Option<RevisionId> = None;
        let mut manifest_generation: Option<ManifestGeneration> = None;
        let mut snapshot_meta: Option<RepoMapSnapshotMeta> = None;
        let mut entries: Option<Vec<RepoMapEntry>> = None;
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
                _other => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        Ok(RepoMapSnapshot {
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

impl<'de> Deserialize<'de> for RepoMapSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapSnapshot",
            REPOMAP_SNAPSHOT_FIELDS,
            RepoMapSnapshotVisitor,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture_entry() -> RepoMapEntry {
        let mut contributing_signals = BTreeMap::new();
        assert_eq!(contributing_signals.insert("files".to_string(), 3), None);
        RepoMapEntry {
            subject_identity: "subject://main".to_string(),
            subject_doc_type: RepoMapDocType::File,
            subject_kind: "File".to_string(),
            owner_path: "src/main.rs".to_string(),
            score: 0.75,
            final_score_millis: 750,
            importance_score_millis: 800,
            utility_score_millis: 700,
            freshness_score_millis: 650,
            evidence_priority_millis: 600,
            token_budget_hint: 512,
            contributing_signals,
            projection_evidence_kind: "bundle".to_string(),
            projection_authority_artifact_id: "artifact-1".to_string(),
            projection_authority_digest: "digest-1".to_string(),
            projection_status: "fresh".to_string(),
            redaction_state: RepoMapRedactionState::Unredacted,
            search_text: "main file".to_string(),
            source_symbol_count: 2,
            source_chunk_token_total: 120,
            source_call_incoming_edges: 4,
            source_call_outgoing_edges: 5,
            source_import_incoming_edges: 6,
            source_import_outgoing_edges: 7,
        }
    }

    fn fixture_snapshot() -> RepoMapSnapshot {
        RepoMapSnapshot {
            repo_id: RepoId::new("repo-1"),
            revision_id: RevisionId::new("rev-1"),
            manifest_generation: ManifestGeneration::new(11),
            snapshot_meta: RepoMapSnapshotMeta {
                snapshot_id: "snapshot-1".to_string(),
                projection_version: 2,
                authority_digest: "authority-1".to_string(),
                item_index_availability: quanta_index_contract::RepoMapItemIndexAvailability::Full,
                graph_coverage_class: quanta_index_contract::RepoMapGraphCoverageClass::Full,
                exactness_summary: quanta_index_contract::RepoMapExactnessSummary::Exact,
            },
            entries: vec![fixture_entry()],
        }
    }

    #[test]
    fn repomap_entry_round_trip_json() {
        let entry = fixture_entry();
        let encoded = match serde_json::to_value(&entry) {
            Ok(value) => value,
            Err(err) => {
                assert!(false, "failed to encode RepoMapEntry: {err}");
                return;
            }
        };
        let decoded: RepoMapEntry = match serde_json::from_value(encoded) {
            Ok(value) => value,
            Err(err) => {
                assert!(false, "failed to decode RepoMapEntry: {err}");
                return;
            }
        };
        assert_eq!(decoded, entry);
    }

    // A pre-QI-BB-008 file carried the query-time `included`/`rank` on
    // every entry; they are ignored, not refused, so those files still load.
    #[test]
    fn repomap_entry_defaults_missing_search_and_source_fields() {
        let value = json!({
            "subject_identity": "subject://main",
            "subject_doc_type": "File",
            "subject_kind": "File",
            "owner_path": "src/main.rs",
            "score": 0.75,
            "final_score_millis": 750,
            "included": true,
            "rank": 1,
            "importance_score_millis": 800,
            "utility_score_millis": 700,
            "freshness_score_millis": 650,
            "evidence_priority_millis": 600,
            "token_budget_hint": 512,
            "contributing_signals": {
                "files": 3
            },
            "projection_evidence_kind": "bundle",
            "projection_authority_artifact_id": "artifact-1",
            "projection_authority_digest": "digest-1",
            "projection_status": "fresh",
            "redaction_state": "Unredacted"
        });
        let decoded: RepoMapEntry = match serde_json::from_value(value) {
            Ok(value) => value,
            Err(err) => {
                assert!(false, "failed to decode RepoMapEntry with defaults: {err}");
                return;
            }
        };
        assert_eq!(decoded.search_text, "");
        assert_eq!(decoded.source_symbol_count, 0);
        assert_eq!(decoded.source_chunk_token_total, 0);
        assert_eq!(decoded.source_call_incoming_edges, 0);
        assert_eq!(decoded.source_call_outgoing_edges, 0);
        assert_eq!(decoded.source_import_incoming_edges, 0);
        assert_eq!(decoded.source_import_outgoing_edges, 0);
    }

    #[test]
    fn repomap_snapshot_round_trip_json() {
        let snapshot = fixture_snapshot();
        let encoded = match serde_json::to_value(&snapshot) {
            Ok(value) => value,
            Err(err) => {
                assert!(false, "failed to encode RepoMapSnapshot: {err}");
                return;
            }
        };
        let decoded: RepoMapSnapshot = match serde_json::from_value(encoded) {
            Ok(value) => value,
            Err(err) => {
                assert!(false, "failed to decode RepoMapSnapshot: {err}");
                return;
            }
        };
        assert_eq!(decoded, snapshot);
    }
}
