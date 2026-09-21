//! The `RepoMap` inbound read ports (S21-05): acquisition and pinned
//! execution are separate traits.
//!
//! A query route acquires a real immutable handle once per read view,
//! inside one critical section of the owning authority, and can never
//! re-enter an ambient store during execution. The pinned snapshot the
//! acquire port returns holds the `Arc` to the indexed artifact and the
//! evidence of what was pinned; it is the only thing a query executes
//! against. No trait here exposes an adapter concrete type.

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapQueryRequest, RepoMapQueryResponse, RevisionId,
};

use crate::CoreError;

/// What one pinned `RepoMap` snapshot proves about itself.
///
/// The identity it was acquired under, the candidate commitment of the
/// artifact it holds, and the activation epoch that was serving when it
/// was pinned. Carried by the read view; never re-derived from a store
/// after acquisition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapSnapshotEvidenceV1 {
    pub repo_id: String,
    pub revision_id: String,
    pub manifest_generation: u64,
    /// The sealed candidate commitment, as its wire string.
    pub candidate_commitment: String,
    /// The activation epoch the acquisition observed.
    pub activation_epoch: u64,
}

/// What a read view asks the `RepoMap` authority to pin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapSnapshotAcquireV1 {
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub manifest_generation: ManifestGeneration,
}

/// A pinned, immutable `RepoMap` snapshot: acquired once per read view,
/// executed without any store re-entry.
///
/// The handle keeps its physical artifact alive for as long as it is
/// held; retirement, GC and compaction of the underlying object defer to
/// that pin. Dropping the handle (including on cancel or panic unwind)
/// returns the pin.
pub trait PinnedRepoMapSnapshot: Send + Sync {
    /// Execute one query against the pinned artifact. Structurally unable
    /// to consult a store, registry or ledger.
    fn query(&self, request: RepoMapQueryRequest) -> Result<RepoMapQueryResponse, CoreError>;

    /// What this handle pinned, for view evidence and provenance.
    fn evidence(&self) -> &RepoMapSnapshotEvidenceV1;
}

/// The acquisition side of the `RepoMap` read path: resolve the active
/// snapshot for one logical identity in a single critical section and
/// return it pinned.
///
/// An implementation must acquire the active identity, its candidate
/// commitment, its activation epoch and the artifact reference together;
/// a generation that is not the serving head is refused typed rather
/// than resolved to another generation.
pub trait RepoMapSnapshotAcquirePort: Send + Sync {
    fn acquire(
        &self,
        acquire: RepoMapSnapshotAcquireV1,
    ) -> Result<Box<dyn PinnedRepoMapSnapshot>, CoreError>;
}
