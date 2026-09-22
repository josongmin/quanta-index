//! The pinned `RepoMap` snapshot executor and its RAII pin lease (S21-05).
//!
//! One handle is created inside the store's acquisition critical
//! section and holds exactly three things: the `Arc` to the immutable
//! indexed snapshot, the evidence of what was pinned, and the RAII pin
//! lease that keeps the physical artifact alive. Executing a query
//! touches only the `Arc`; the handle has no store, registry or ledger
//! reference, so a store re-lookup during execution is structurally
//! impossible. Dropping the handle — by return, cancellation or panic
//! unwind — returns the pin.
//!
//! This module is the lower layer both `store` (which mints the handle)
//! and `reader` (which re-exports it) depend on; keeping the lease and
//! the handle here is what makes the module graph acyclic.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use quanta_index_contract::{RepoMapQueryRequest, RepoMapQueryResponse};
use quanta_index_core::{CoreError, PinnedRepoMapSnapshot, RepoMapSnapshotEvidenceV1};

use crate::model::RepoMapIndexedSnapshot;
use crate::query::RepoMapQueryEngine;

/// The store's logical snapshot key: repo, revision and generation.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct RepoMapStoreKeyV1 {
    pub(crate) repo_id: String,
    pub(crate) revision_id: String,
    pub(crate) manifest_generation: u64,
}

impl RepoMapStoreKeyV1 {
    pub(crate) fn new(
        repo_id: &quanta_index_contract::RepoId,
        revision_id: &quanta_index_contract::RevisionId,
        manifest_generation: quanta_index_contract::ManifestGeneration,
    ) -> Self {
        Self {
            repo_id: repo_id.as_str().to_string(),
            revision_id: revision_id.as_str().to_string(),
            manifest_generation: manifest_generation.get(),
        }
    }
}

/// The shared pin table: how many live read views hold each snapshot.
pub(crate) type RepoMapPinTable = Arc<RwLock<BTreeMap<RepoMapStoreKeyV1, u64>>>;

/// An RAII reference to one logical generation in the pin table.
///
/// Created inside the acquisition critical section; dropped when the
/// pinned snapshot handle (and the read view holding it) goes away — by
/// normal return, cancellation or panic unwind alike — at which point the
/// reference is returned to the pin table and GC may proceed.
#[derive(Debug)]
pub(crate) struct RepoMapPinLease {
    pins: RepoMapPinTable,
    key: RepoMapStoreKeyV1,
}

impl RepoMapPinLease {
    pub(crate) fn new(pins: RepoMapPinTable, key: RepoMapStoreKeyV1) -> Result<Self, CoreError> {
        let mut guard = pins.write().map_err(|err| {
            CoreError::Storage(format!("repomap store pin table poisoned: {err}"))
        })?;
        let count = guard.entry(key.clone()).or_insert(0);
        *count = count.saturating_add(1);
        drop(guard);
        Ok(Self { pins, key })
    }
}

impl Drop for RepoMapPinLease {
    fn drop(&mut self) {
        // A drop cannot propagate a poisoned-table error; the table is
        // per-process state and a poison means the process is already
        // failing loudly elsewhere. Keep counts best-effort here and
        // fail-closed on the read paths that matter.
        if let Ok(mut guard) = self.pins.write()
            && let Some(count) = guard.get_mut(&self.key)
        {
            *count = count.saturating_sub(1);
            if *count == 0 {
                let _removed = guard.remove(&self.key);
            }
        }
    }
}

/// A pinned, immutable `RepoMap` snapshot (see the module docs).
#[derive(Debug)]
pub struct PinnedRepoMapSnapshotV1 {
    snapshot: Arc<RepoMapIndexedSnapshot>,
    evidence: RepoMapSnapshotEvidenceV1,
    _pin: RepoMapPinLease,
}

impl PinnedRepoMapSnapshotV1 {
    pub(crate) fn new(
        snapshot: Arc<RepoMapIndexedSnapshot>,
        evidence: RepoMapSnapshotEvidenceV1,
        pin: RepoMapPinLease,
    ) -> Self {
        Self {
            snapshot,
            evidence,
            _pin: pin,
        }
    }
}

impl PinnedRepoMapSnapshot for PinnedRepoMapSnapshotV1 {
    fn query(&self, request: RepoMapQueryRequest) -> Result<RepoMapQueryResponse, CoreError> {
        RepoMapQueryEngine::query(&self.snapshot, &request)
    }

    fn evidence(&self) -> &RepoMapSnapshotEvidenceV1 {
        &self.evidence
    }
}
