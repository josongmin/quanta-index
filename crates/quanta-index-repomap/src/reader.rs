//! The pinned `RepoMap` snapshot executor (S21-05).
//!
//! One handle is created inside the store's acquisition critical
//! section and holds exactly three things: the `Arc` to the immutable
//! indexed snapshot, the evidence of what was pinned, and the RAII pin
//! lease that keeps the physical artifact alive. Executing a query
//! touches only the `Arc`; the handle has no store, registry or ledger
//! reference, so a store re-lookup during execution is structurally
//! impossible. Dropping the handle — by return, cancellation or panic
//! unwind — returns the pin.

use std::sync::Arc;

use quanta_index_contract::{RepoMapQueryRequest, RepoMapQueryResponse};
use quanta_index_core::{CoreError, PinnedRepoMapSnapshot, RepoMapSnapshotEvidenceV1};

use crate::model::RepoMapIndexedSnapshot;
use crate::query::RepoMapQueryEngine;
use crate::store::RepoMapPinLease;

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
