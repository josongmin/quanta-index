use std::{
    fs,
    path::{Path, PathBuf},
};

use quanta_index_contract::{RepoId, RevisionId};
use quanta_index_core::CoreError;
use serde::{Deserialize, Serialize};

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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RepoMapActivationRecordV1 {
    pub(crate) repo_id: String,
    pub(crate) revision_id: String,
    pub(crate) manifest_generation: u64,
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
