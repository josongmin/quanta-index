use core::fmt;
use std::{
    fs,
    path::{Path, PathBuf},
};

use quanta_index_contract::{RepoId, RevisionId};
use quanta_index_core::CoreError;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::model::RepoMapSnapshotV1;

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private persistence module still needs sibling-module visibility"
)]
#[derive(Clone, Debug)]
pub(crate) struct RepoMapSnapshotPersistence {
    snapshots_dir: PathBuf,
    activations_dir: PathBuf,
}

#[expect(
    clippy::redundant_pub_crate,
    reason = "crate-private persistence module still needs sibling-module visibility"
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RepoMapActivationRecordV1 {
    pub(crate) repo_id: String,
    pub(crate) revision_id: String,
    pub(crate) manifest_generation: u64,
}

const REPOMAP_ACTIVATION_RECORD_V1_FIELDS: &[&str] =
    &["repo_id", "revision_id", "manifest_generation"];

impl Serialize for RepoMapActivationRecordV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("RepoMapActivationRecordV1", 3)?;
        state.serialize_field("repo_id", &self.repo_id)?;
        state.serialize_field("revision_id", &self.revision_id)?;
        state.serialize_field("manifest_generation", &self.manifest_generation)?;
        state.end()
    }
}

struct RepoMapActivationRecordV1Visitor;

impl<'de> Visitor<'de> for RepoMapActivationRecordV1Visitor {
    type Value = RepoMapActivationRecordV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a RepoMapActivationRecordV1 map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut repo_id: Option<String> = None;
        let mut revision_id: Option<String> = None;
        let mut manifest_generation: Option<u64> = None;
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
                _other => {
                    let _: de::IgnoredAny = map.next_value()?;
                }
            }
        }
        Ok(RepoMapActivationRecordV1 {
            repo_id: repo_id.ok_or_else(|| de::Error::missing_field("repo_id"))?,
            revision_id: revision_id.ok_or_else(|| de::Error::missing_field("revision_id"))?,
            manifest_generation: manifest_generation
                .ok_or_else(|| de::Error::missing_field("manifest_generation"))?,
        })
    }
}

impl<'de> Deserialize<'de> for RepoMapActivationRecordV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_struct(
            "RepoMapActivationRecordV1",
            REPOMAP_ACTIVATION_RECORD_V1_FIELDS,
            RepoMapActivationRecordV1Visitor,
        )
    }
}

impl RepoMapSnapshotPersistence {
    pub(crate) fn open(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let root = root.as_ref();
        let snapshots_dir = root.join("snapshots");
        let activations_dir = root.join("activations");
        fs::create_dir_all(&snapshots_dir).map_err(|err| {
            CoreError::Storage(format!(
                "repomap persistence failed to create snapshot dir {}: {err}",
                snapshots_dir.display()
            ))
        })?;
        fs::create_dir_all(&activations_dir).map_err(|err| {
            CoreError::Storage(format!(
                "repomap persistence failed to create activation dir {}: {err}",
                activations_dir.display()
            ))
        })?;
        Ok(Self {
            snapshots_dir,
            activations_dir,
        })
    }

    pub(crate) fn load_snapshots(&self) -> Result<Vec<RepoMapSnapshotV1>, CoreError> {
        let mut snapshots = Vec::new();
        for path in self.list_json_files(&self.snapshots_dir)? {
            let bytes = fs::read(&path).map_err(|err| {
                CoreError::Storage(format!(
                    "repomap persistence failed to read snapshot {}: {err}",
                    path.display()
                ))
            })?;
            let snapshot = serde_json::from_slice::<RepoMapSnapshotV1>(&bytes).map_err(|err| {
                CoreError::Storage(format!(
                    "repomap persistence failed to decode snapshot {}: {err}",
                    path.display()
                ))
            })?;
            snapshots.push(snapshot);
        }
        Ok(snapshots)
    }

    pub(crate) fn load_activations(&self) -> Result<Vec<RepoMapActivationRecordV1>, CoreError> {
        let mut activations = Vec::new();
        for path in self.list_json_files(&self.activations_dir)? {
            let bytes = fs::read(&path).map_err(|err| {
                CoreError::Storage(format!(
                    "repomap persistence failed to read activation {}: {err}",
                    path.display()
                ))
            })?;
            let activation =
                serde_json::from_slice::<RepoMapActivationRecordV1>(&bytes).map_err(|err| {
                    CoreError::Storage(format!(
                        "repomap persistence failed to decode activation {}: {err}",
                        path.display()
                    ))
                })?;
            activations.push(activation);
        }
        Ok(activations)
    }

    pub(crate) fn persist_snapshot(&self, snapshot: &RepoMapSnapshotV1) -> Result<(), CoreError> {
        let path = self.snapshots_dir.join(snapshot_file_name(snapshot));
        let bytes = serde_json::to_vec_pretty(snapshot).map_err(|err| {
            CoreError::Storage(format!(
                "repomap persistence failed to encode snapshot {}: {err}",
                path.display()
            ))
        })?;
        fs::write(&path, bytes).map_err(|err| {
            CoreError::Storage(format!(
                "repomap persistence failed to write snapshot {}: {err}",
                path.display()
            ))
        })?;
        Ok(())
    }

    pub(crate) fn persist_activation(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        manifest_generation: u64,
    ) -> Result<(), CoreError> {
        let path = self
            .activations_dir
            .join(activation_file_name(repo_id, revision_id));
        let record = RepoMapActivationRecordV1 {
            repo_id: repo_id.as_str().to_string(),
            revision_id: revision_id.as_str().to_string(),
            manifest_generation,
        };
        let bytes = serde_json::to_vec_pretty(&record).map_err(|err| {
            CoreError::Storage(format!(
                "repomap persistence failed to encode activation {}: {err}",
                path.display()
            ))
        })?;
        fs::write(&path, bytes).map_err(|err| {
            CoreError::Storage(format!(
                "repomap persistence failed to write activation {}: {err}",
                path.display()
            ))
        })?;
        Ok(())
    }

    fn list_json_files(&self, dir: &Path) -> Result<Vec<PathBuf>, CoreError> {
        let mut paths = Vec::new();
        let entries = fs::read_dir(dir).map_err(|err| {
            CoreError::Storage(format!(
                "repomap persistence failed to list dir {}: {err}",
                dir.display()
            ))
        })?;
        for entry in entries {
            let entry = entry.map_err(|err| {
                CoreError::Storage(format!(
                    "repomap persistence failed to read dir entry in {}: {err}",
                    dir.display()
                ))
            })?;
            let file_type = entry.file_type().map_err(|err| {
                CoreError::Storage(format!(
                    "repomap persistence failed to inspect dir entry in {}: {err}",
                    dir.display()
                ))
            })?;
            if !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
    }
}

fn snapshot_file_name(snapshot: &RepoMapSnapshotV1) -> String {
    format!(
        "{}--{}--g{}.json",
        encode_component(snapshot.repo_id.as_str()),
        encode_component(snapshot.revision_id.as_str()),
        snapshot.manifest_generation.get()
    )
}

fn activation_file_name(repo_id: &RepoId, revision_id: &RevisionId) -> String {
    format!(
        "{}--{}.json",
        encode_component(repo_id.as_str()),
        encode_component(revision_id.as_str())
    )
}

fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_' || byte == b'.' {
            encoded.push(char::from(byte));
            continue;
        }
        encoded.push('%');
        encoded.push(hex_char(byte >> 4));
        encoded.push(hex_char(byte & 0x0F));
    }
    encoded
}

fn hex_char(nibble: u8) -> char {
    const HEX_DIGITS: [char; 16] = [
        '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F',
    ];
    HEX_DIGITS
        .get(usize::from(nibble))
        .copied()
        .map_or('0', std::convert::identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::RepoMapEntryV1;
    use quanta_index_contract::{
        ManifestGeneration, RepoMapDocType, RepoMapExactnessSummary, RepoMapGraphCoverageClass,
        RepoMapItemIndexAvailability, RepoMapRedactionState, RepoMapSnapshotMeta,
    };
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn fixture_snapshot() -> RepoMapSnapshotV1 {
        let mut contributing_signals = BTreeMap::new();
        assert_eq!(contributing_signals.insert("files".to_string(), 3), None);
        RepoMapSnapshotV1 {
            repo_id: RepoId::new("repo/a"),
            revision_id: RevisionId::new("rev:b"),
            manifest_generation: ManifestGeneration::new(11),
            snapshot_meta: RepoMapSnapshotMeta {
                snapshot_id: "snapshot-1".to_string(),
                projection_version: 2,
                authority_digest: "authority-1".to_string(),
                item_index_availability: RepoMapItemIndexAvailability::Full,
                graph_coverage_class: RepoMapGraphCoverageClass::Full,
                exactness_summary: RepoMapExactnessSummary::Exact,
            },
            entries: vec![RepoMapEntryV1 {
                subject_identity: "subject://main".to_string(),
                subject_doc_type: RepoMapDocType::File,
                subject_kind: "File".to_string(),
                owner_path: "src/main.rs".to_string(),
                score: 0.75,
                final_score_millis: 750,
                included: true,
                rank: 1,
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
            }],
        }
    }

    #[test]
    fn activation_record_v1_round_trip_json() {
        let record = RepoMapActivationRecordV1 {
            repo_id: "repo/a".to_string(),
            revision_id: "rev:b".to_string(),
            manifest_generation: 11,
        };
        let encoded = match serde_json::to_value(&record) {
            Ok(value) => value,
            Err(err) => {
                assert!(false, "failed to encode RepoMapActivationRecordV1: {err}");
                return;
            }
        };
        let decoded: RepoMapActivationRecordV1 = match serde_json::from_value(encoded) {
            Ok(value) => value,
            Err(err) => {
                assert!(false, "failed to decode RepoMapActivationRecordV1: {err}");
                return;
            }
        };
        assert_eq!(decoded, record);
    }

    #[test]
    fn snapshot_and_activation_persistence_round_trip() {
        let root = match tempdir() {
            Ok(dir) => dir,
            Err(err) => {
                assert!(false, "failed to create temp dir: {err}");
                return;
            }
        };
        let persistence = match RepoMapSnapshotPersistence::open(root.path()) {
            Ok(persistence) => persistence,
            Err(err) => {
                assert!(false, "failed to open persistence: {err}");
                return;
            }
        };
        let snapshot = fixture_snapshot();
        if let Err(err) = persistence.persist_snapshot(&snapshot) {
            assert!(false, "failed to persist snapshot: {err}");
            return;
        }
        if let Err(err) = persistence.persist_activation(
            &snapshot.repo_id,
            &snapshot.revision_id,
            snapshot.manifest_generation.get(),
        ) {
            assert!(false, "failed to persist activation: {err}");
            return;
        }
        let snapshots = match persistence.load_snapshots() {
            Ok(value) => value,
            Err(err) => {
                assert!(false, "failed to load snapshots: {err}");
                return;
            }
        };
        let activations = match persistence.load_activations() {
            Ok(value) => value,
            Err(err) => {
                assert!(false, "failed to load activations: {err}");
                return;
            }
        };
        assert_eq!(snapshots, vec![snapshot]);
        assert_eq!(
            activations,
            vec![RepoMapActivationRecordV1 {
                repo_id: "repo/a".to_string(),
                revision_id: "rev:b".to_string(),
                manifest_generation: 11,
            }]
        );
    }
}
