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
use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::readiness::keys::AuthorityKey;
use crate::readiness::serde_support::impl_struct_serde;

#[expect(
    clippy::struct_excessive_bools,
    reason = "history authority tracks four independently materialized shard families"
)]
#[derive(Clone, Debug, Default)]
pub struct HistoryAuthorityState {
    pub(super) commits: OrdMap<CommitSha, CommitRecord>,
    pub(super) refs: OrdMap<Box<str>, CommitSha>,
    pub(super) tags: OrdMap<Box<str>, CommitSha>,
    pub(super) diff_hunks: OrdMap<HistoryDiffKey, DiffHunkRecord>,
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
    pub(super) fn note_commits_materialized(&mut self) {
        self.commits_materialized = true;
    }

    pub(super) fn note_refs_materialized(&mut self) {
        self.refs_materialized = true;
    }

    pub(super) fn note_tags_materialized(&mut self) {
        self.tags_materialized = true;
    }

    pub(super) fn note_diff_hunks_materialized(&mut self) {
        self.diff_hunks_materialized = true;
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
