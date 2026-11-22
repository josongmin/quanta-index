//! Native History V1 JSON projection borrowed from the authenticated source
//! state. The source's canonical CBOR rows remain the digest and V2 fallback.

use quanta_index_contract::{
    AuxEpochV1,
    lex::{CommitRecord, CommitSha, DiffHunkRecord},
};
use quanta_index_core::{AuxiliaryRowFamilyV1, AuxiliaryRowV1, CoreError};
use serde::{
    Serialize, Serializer,
    ser::{SerializeStruct, SerializeStructVariant},
};

use crate::{
    auxiliary_authority::{commit_sha_from_bytes, decode_diff_key},
    readiness::HistoryAuthorityState,
};

use super::invalid;

pub(super) struct BorrowedNativeRow<'a> {
    key: &'a [u8],
    data: BorrowedData<'a>,
}

enum BorrowedData<'a> {
    Epoch(AuxEpochV1),
    Row(BorrowedHistoryRow<'a>),
}

enum BorrowedHistoryRow<'a> {
    Commit(&'a CommitRecord),
    Ref {
        name: &'a str,
        sha: &'a CommitSha,
    },
    Tag {
        name: &'a str,
        sha: &'a CommitSha,
    },
    DiffHunk {
        commit_sha: CommitSha,
        file_path: &'a str,
        record: &'a DiffHunkRecord,
    },
    Materialization {
        commits: bool,
        refs: bool,
        tags: bool,
        diff_hunks: bool,
    },
}

/// The fixed Native map key and any DiffHunk lookup key are funded by the
/// caller's temporary key grant. No decoded CBOR copy is made here.
pub(super) fn from_source<'a>(
    state: &'a HistoryAuthorityState,
    epoch: AuxEpochV1,
    row: &'a AuxiliaryRowV1,
) -> Result<BorrowedNativeRow<'a>, CoreError> {
    let key = &row.key.row_key;
    let data = match row.key.family {
        AuxiliaryRowFamilyV1::Epoch => {
            if !key.is_empty() {
                return Err(invalid("History epoch key must be empty"));
            }
            BorrowedData::Epoch(epoch)
        }
        AuxiliaryRowFamilyV1::Commit => {
            let sha = commit_sha_from_bytes(key)?;
            let record = state
                .commits()
                .get(&sha)
                .ok_or_else(|| invalid("History source row absent from typed state"))?;
            BorrowedData::Row(BorrowedHistoryRow::Commit(record))
        }
        AuxiliaryRowFamilyV1::Ref | AuxiliaryRowFamilyV1::Tag => {
            let name = std::str::from_utf8(key)
                .map_err(|_| invalid("History source ref or tag key is not UTF-8"))?;
            let map = if row.key.family == AuxiliaryRowFamilyV1::Ref {
                state.refs()
            } else {
                state.tags()
            };
            let sha = map
                .get(name)
                .ok_or_else(|| invalid("History source row absent from typed state"))?;
            BorrowedData::Row(if row.key.family == AuxiliaryRowFamilyV1::Ref {
                BorrowedHistoryRow::Ref { name, sha }
            } else {
                BorrowedHistoryRow::Tag { name, sha }
            })
        }
        AuxiliaryRowFamilyV1::DiffHunk => {
            let (commit_sha, file_path) = decode_diff_key(key)?;
            let lookup = crate::readiness::HistoryDiffKey::new(commit_sha, file_path);
            let record = state
                .diff_hunks()
                .get(&lookup)
                .ok_or_else(|| invalid("History source row absent from typed state"))?;
            BorrowedData::Row(BorrowedHistoryRow::DiffHunk {
                commit_sha,
                file_path,
                record,
            })
        }
        AuxiliaryRowFamilyV1::StateMeta => {
            if !key.is_empty() {
                return Err(invalid("History materialization key must be empty"));
            }
            let meta = state.meta();
            BorrowedData::Row(BorrowedHistoryRow::Materialization {
                commits: meta.commits_materialized,
                refs: meta.refs_materialized,
                tags: meta.tags_materialized,
                diff_hunks: meta.diff_hunks_materialized,
            })
        }
        _ => return Err(invalid("invalid History record family")),
    };
    Ok(BorrowedNativeRow { key, data })
}

impl Serialize for BorrowedNativeRow<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut row = serializer.serialize_struct("NativeRow", 2)?;
        row.serialize_field("key", self.key)?;
        row.serialize_field("data", &self.data)?;
        row.end()
    }
}

impl Serialize for BorrowedData<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Epoch(epoch) => {
                serializer.serialize_newtype_variant("NativeRowData", 0, "Epoch", epoch)
            }
            Self::Row(row) => serializer.serialize_newtype_variant("NativeRowData", 1, "Row", row),
        }
    }
}

struct RefUpsert<'a> {
    name: &'a str,
    sha: &'a CommitSha,
}
impl Serialize for RefUpsert<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut row = serializer.serialize_struct("HistoryRefUpsert", 2)?;
        row.serialize_field("name", self.name)?;
        row.serialize_field("sha", self.sha)?;
        row.end()
    }
}

struct DiffHunkUpsert<'a> {
    commit_sha: CommitSha,
    file_path: &'a str,
    record: &'a DiffHunkRecord,
}
impl Serialize for DiffHunkUpsert<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut row = serializer.serialize_struct("HistoryDiffHunkUpsert", 3)?;
        row.serialize_field("commit_sha", &self.commit_sha)?;
        row.serialize_field("file_path", self.file_path)?;
        row.serialize_field("record", self.record)?;
        row.end()
    }
}

impl Serialize for BorrowedHistoryRow<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Commit(record) => {
                serializer.serialize_newtype_variant("HistoryReadbackRowV1", 0, "Commit", record)
            }
            Self::Ref { name, sha } => serializer.serialize_newtype_variant(
                "HistoryReadbackRowV1",
                1,
                "Ref",
                &RefUpsert {
                    name: *name,
                    sha: *sha,
                },
            ),
            Self::Tag { name, sha } => serializer.serialize_newtype_variant(
                "HistoryReadbackRowV1",
                2,
                "Tag",
                &RefUpsert {
                    name: *name,
                    sha: *sha,
                },
            ),
            Self::DiffHunk {
                commit_sha,
                file_path,
                record,
            } => serializer.serialize_newtype_variant(
                "HistoryReadbackRowV1",
                3,
                "DiffHunk",
                &DiffHunkUpsert {
                    commit_sha: *commit_sha,
                    file_path: *file_path,
                    record: *record,
                },
            ),
            Self::Materialization {
                commits,
                refs,
                tags,
                diff_hunks,
            } => {
                let mut row = serializer.serialize_struct_variant(
                    "HistoryReadbackRowV1",
                    4,
                    "Materialization",
                    4,
                )?;
                row.serialize_field("commits", commits)?;
                row.serialize_field("refs", refs)?;
                row.serialize_field("tags", tags)?;
                row.serialize_field("diff_hunks", diff_hunks)?;
                row.end()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        auxiliary_authority::{decode_history_row, history_source::visit_history_state_rows},
        native_history::{NativeRowData, NativeRowRef},
        readiness::{HistoryDiffKey, HistoryStateMeta},
    };
    use quanta_index_contract::{DiffHunkSide, ManifestGeneration, RepoId, RevisionId};
    use quanta_index_core::{
        AuxiliaryGenerationKeyV1,
        index::{WorkBudget, WorkLimits},
    };
    use quanta_index_ipc::decode_cbor_payload;
    use std::time::{Duration, Instant};

    #[test]
    fn borrowed_state_projection_keeps_every_v1_json_byte() -> Result<(), Box<dyn std::error::Error>>
    {
        let sha = CommitSha::from_bytes([7; 20]);
        let record = CommitRecord {
            wire_version: 1,
            sha,
            parents: Vec::new(),
            author_time_ms: 1,
            committer_time_ms: 2,
            applied_at_ms: 3,
            author: "a\"\\\n".into(),
            author_name: Some("name".into()),
            author_email: None,
            committer: "c".into(),
            committer_name: None,
            committer_email: Some("c@example.test".into()),
            message: "body\n".into(),
            is_merge: false,
            tags: vec!["v1".into()],
        };
        let hunk = DiffHunkRecord {
            wire_version: 1,
            hunk_header: "@@".into(),
            side: DiffHunkSide::After,
            added_text: "added\n".into(),
            removed_text: "".into(),
            touched_text: "touched".into(),
            byte_start: 0,
            byte_end: 6,
        };
        let mut state = HistoryAuthorityState::default();
        state.restore_commit(sha, record);
        state.restore_ref("heads/main", sha);
        state.restore_tag("v1", sha);
        state.restore_diff_hunk(HistoryDiffKey::new(sha, "src/a.rs"), hunk);
        state.restore_meta(HistoryStateMeta {
            commits_materialized: true,
            refs_materialized: true,
            tags_materialized: true,
            diff_hunks_materialized: true,
        });
        let generation = AuxiliaryGenerationKeyV1 {
            repo_id: RepoId::new("borrowed-projection")?,
            revision_id: RevisionId::new("main")?,
            generation: ManifestGeneration::new(1),
        };
        let epoch = AuxEpochV1::new(2);
        let mut budget = WorkBudget::new(
            WorkLimits {
                reads: 1_000,
                writes: 1_000,
                mutations: 1_000,
                bytes: 1_000_000,
            },
            Instant::now() + Duration::from_secs(10),
        );
        let mut families = Vec::new();
        visit_history_state_rows(&generation, epoch, &state, &mut budget, &mut |row, _| {
            let previous = if row.key.family == AuxiliaryRowFamilyV1::Epoch {
                NativeRowData::Epoch(
                    decode_cbor_payload(&row.value)
                        .map_err(|error| CoreError::Storage(error.to_string()))?,
                )
            } else {
                NativeRowData::Row(decode_history_row(row)?)
            };
            let previous = serde_json::to_vec(&NativeRowRef {
                key: &row.key.row_key,
                data: &previous,
            })
            .map_err(|error| CoreError::Storage(error.to_string()))?;
            let borrowed = serde_json::to_vec(&from_source(&state, epoch, row)?)
                .map_err(|error| CoreError::Storage(error.to_string()))?;
            assert_eq!(
                borrowed, previous,
                "V1 JSON changed for {:?}",
                row.key.family
            );
            families.push(row.key.family);
            Ok(())
        })?;
        assert_eq!(families.len(), 6);
        Ok(())
    }
}
