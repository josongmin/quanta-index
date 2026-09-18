//! History authority state: commits, refs, tags, diff hunks per generation.
//!
//! The record maps are persistent (structurally shared): cloning a state
//! is `O(1)` and a mutation rewrites only the paths it touches, which is
//! what lets the ledger retain superseded epoch snapshots beside the
//! current one at the cost of the deltas alone (QI-BB-020 W2).

use std::collections::BTreeMap;
use std::fmt;

use imbl::OrdMap;
use quanta_index_contract::lex::{CommitRecord, CommitSha, DiffHunkRecord};
use quanta_index_core::CoreError;
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::auxiliary_authority::{HistoryDelta, RefChange};
use crate::readiness::keys::AuthorityKey;
use crate::readiness::serde_support::impl_struct_serde;

fn history_ref_not_found(message: String) -> CoreError {
    CoreError::Typed {
        code: "HISTORY_REF_NOT_FOUND".to_string(),
        message,
    }
}

/// One generation's commits, refs, tags and diff hunks.
///
/// The record maps are private: every write goes through a method here,
/// so the parent/target checks the channel ops need and the delta
/// application the catalog needs are the only two ways a record lands.
#[expect(
    clippy::struct_excessive_bools,
    reason = "history authority tracks four independently materialized shard families"
)]
#[derive(Clone, Debug, Default)]
pub struct HistoryAuthorityState {
    commits: OrdMap<CommitSha, CommitRecord>,
    refs: OrdMap<Box<str>, CommitSha>,
    tags: OrdMap<Box<str>, CommitSha>,
    diff_hunks: OrdMap<HistoryDiffKey, DiffHunkRecord>,
    commits_materialized: bool,
    refs_materialized: bool,
    tags_materialized: bool,
    diff_hunks_materialized: bool,
}

/// The part of a history generation's state that is not a record: which
/// shard families it has materialized.
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per independently materialized shard family, mirrored from the state"
)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct HistoryStateMeta {
    pub(crate) commits_materialized: bool,
    pub(crate) refs_materialized: bool,
    pub(crate) tags_materialized: bool,
    pub(crate) diff_hunks_materialized: bool,
}

impl HistoryAuthorityState {
    /// Apply a validated delta: the transition already checked every
    /// record against the state it was computed from, so the rows land
    /// as they are and the meta is the delta's.
    pub(crate) fn apply_delta(&mut self, delta: &HistoryDelta) {
        for record in &delta.commits {
            self.restore_commit(record.sha, record.clone());
        }
        for (changes, map) in [(&delta.refs, &mut self.refs), (&delta.tags, &mut self.tags)] {
            for change in changes {
                match change {
                    RefChange::Upsert(name, sha) => {
                        let _previous = map.insert(name.clone(), *sha);
                    }
                    RefChange::Delete(name) => {
                        let _removed = map.remove(name.as_ref());
                    }
                }
            }
        }
        for (key, record) in &delta.diff_hunks {
            self.restore_diff_hunk(key.clone(), record.clone());
        }
        self.restore_meta(delta.meta);
    }

    /// Upsert one commit whose parents must already be known (the
    /// channel-op path); marks commits materialized.
    pub(crate) fn upsert_commit(&mut self, record: CommitRecord) -> Result<(), CoreError> {
        self.commits_materialized = true;
        for parent in &record.parents {
            if !self.commits.contains_key(parent) {
                return Err(CoreError::Typed {
                    code: "HISTORY_COMMIT_PARENT_UNKNOWN".to_string(),
                    message: format!(
                        "history ingest: parent {} missing before child {}",
                        parent, record.sha
                    ),
                });
            }
        }
        self.restore_commit(record.sha, record);
        Ok(())
    }

    /// Point a ref at a known commit; marks refs materialized.
    pub(crate) fn upsert_ref(&mut self, name: &str, sha: CommitSha) -> Result<(), CoreError> {
        self.refs_materialized = true;
        if !self.commits.contains_key(&sha) {
            return Err(history_ref_not_found(format!(
                "history ingest: ref `{name}` points to unknown commit {sha}"
            )));
        }
        self.restore_ref(name, sha);
        Ok(())
    }

    /// Drop a ref (absent is fine); marks refs materialized.
    pub(crate) fn delete_ref(&mut self, name: &str) {
        self.refs_materialized = true;
        let _removed = self.refs.remove(name);
    }

    /// Point a tag at a known commit; marks tags materialized.
    pub(crate) fn upsert_tag(&mut self, name: &str, sha: CommitSha) -> Result<(), CoreError> {
        self.tags_materialized = true;
        if !self.commits.contains_key(&sha) {
            return Err(history_ref_not_found(format!(
                "history ingest: tag `{name}` points to unknown commit {sha}"
            )));
        }
        self.restore_tag(name, sha);
        Ok(())
    }

    /// Drop a tag (absent is fine); marks tags materialized.
    pub(crate) fn delete_tag(&mut self, name: &str) {
        self.tags_materialized = true;
        let _removed = self.tags.remove(name);
    }

    /// Upsert one diff hunk of a known commit; marks diff hunks
    /// materialized.
    pub(crate) fn upsert_diff_hunk(
        &mut self,
        key: HistoryDiffKey,
        record: DiffHunkRecord,
    ) -> Result<(), CoreError> {
        self.diff_hunks_materialized = true;
        if !self.commits.contains_key(&key.commit_sha) {
            return Err(history_ref_not_found(format!(
                "history ingest: diff hunk for unknown commit {}",
                key.commit_sha
            )));
        }
        self.restore_diff_hunk(key, record);
        Ok(())
    }

    /// The materialization flags.
    #[must_use]
    pub(crate) const fn meta(&self) -> HistoryStateMeta {
        HistoryStateMeta {
            commits_materialized: self.commits_materialized,
            refs_materialized: self.refs_materialized,
            tags_materialized: self.tags_materialized,
            diff_hunks_materialized: self.diff_hunks_materialized,
        }
    }

    pub(crate) const fn restore_meta(&mut self, meta: HistoryStateMeta) {
        self.commits_materialized = meta.commits_materialized;
        self.refs_materialized = meta.refs_materialized;
        self.tags_materialized = meta.tags_materialized;
        self.diff_hunks_materialized = meta.diff_hunks_materialized;
    }

    pub(crate) fn restore_commit(&mut self, sha: CommitSha, record: CommitRecord) {
        let _previous = self.commits.insert(sha, record);
    }

    pub(crate) fn restore_ref(&mut self, name: &str, sha: CommitSha) {
        let _previous = self.refs.insert(name.into(), sha);
    }

    pub(crate) fn restore_tag(&mut self, name: &str, sha: CommitSha) {
        let _previous = self.tags.insert(name.into(), sha);
    }

    pub(crate) fn restore_diff_hunk(&mut self, key: HistoryDiffKey, record: DiffHunkRecord) {
        let _previous = self.diff_hunks.insert(key, record);
    }

    #[must_use]
    pub fn commits(&self) -> &OrdMap<CommitSha, CommitRecord> {
        &self.commits
    }

    #[must_use]
    pub fn refs(&self) -> &OrdMap<Box<str>, CommitSha> {
        &self.refs
    }

    #[must_use]
    pub fn tags(&self) -> &OrdMap<Box<str>, CommitSha> {
        &self.tags
    }

    #[must_use]
    pub fn diff_hunks(&self) -> &OrdMap<HistoryDiffKey, DiffHunkRecord> {
        &self.diff_hunks
    }

    #[must_use]
    pub const fn commits_materialized(&self) -> bool {
        self.commits_materialized
    }

    #[must_use]
    pub const fn refs_materialized(&self) -> bool {
        self.refs_materialized
    }

    #[must_use]
    pub const fn tags_materialized(&self) -> bool {
        self.tags_materialized
    }

    #[must_use]
    pub const fn diff_hunks_materialized(&self) -> bool {
        self.diff_hunks_materialized
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HistoryDiffKey {
    pub(super) commit_sha: CommitSha,
    pub(super) file_path: Box<str>,
}

impl HistoryDiffKey {
    #[must_use]
    pub(crate) fn new(commit_sha: CommitSha, file_path: &str) -> Self {
        Self {
            commit_sha,
            file_path: file_path.into(),
        }
    }

    #[must_use]
    pub fn commit_sha(&self) -> CommitSha {
        self.commit_sha
    }

    #[must_use]
    pub fn file_path(&self) -> &str {
        self.file_path.as_ref()
    }
}

/// The text of one diff hunk: the path, the hunk header and the three
/// hunk sides, one per line.
///
/// One definition for what the history route's recency filter matches
/// and what the history text index scores, so a hunk matched on this
/// text is the hunk scored on it.
#[must_use]
pub(crate) fn history_diff_search_text(key: &HistoryDiffKey, record: &DiffHunkRecord) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        key.file_path(),
        record.hunk_header,
        record.added_text,
        record.removed_text,
        record.touched_text
    )
}

#[derive(Clone, Debug, Default)]
pub(super) struct HistoryAuthoritySnapshot {
    pub(super) entries: BTreeMap<AuthorityKey, HistoryAuthorityState>,
}

impl_struct_serde!(HistoryStateMeta {
    commits_materialized: bool,
    refs_materialized: bool,
    tags_materialized: bool,
    diff_hunks_materialized: bool,
});
impl_struct_serde!(HistoryAuthorityState {
    commits: OrdMap<CommitSha, CommitRecord>,
    refs: OrdMap<Box<str>, CommitSha>,
    tags: OrdMap<Box<str>, CommitSha>,
    diff_hunks: OrdMap<HistoryDiffKey, DiffHunkRecord>,
    commits_materialized: bool,
    refs_materialized: bool,
    tags_materialized: bool,
    diff_hunks_materialized: bool,
});

impl_struct_serde!(HistoryDiffKey {
    commit_sha: CommitSha,
    file_path: Box<str>,
});

impl_struct_serde!(HistoryAuthoritySnapshot {
    entries: BTreeMap<AuthorityKey, HistoryAuthorityState>,
});
