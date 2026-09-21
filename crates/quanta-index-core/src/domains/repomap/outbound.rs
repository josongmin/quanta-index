use quanta_index_contract::{RepoMapActivateGenerationRequest, RepoMapSourceBundle};

use crate::CoreError;

/// Identity/content-bound durable receipt for a `RepoMap` publish,
/// activate or rollback (S21-02).
///
/// The dispatcher copies these fields into the wire
/// [`quanta_index_contract::RepoMapMutationAck`] verbatim; the store is
/// their authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapMutationReceiptV1 {
    /// Wire hex of the prior activation's candidate commitment; `None`
    /// when no activation existed.
    pub prior_candidate_commitment: Option<String>,
    /// Wire hex of the new candidate commitment this receipt binds.
    pub new_candidate_commitment: String,
    /// Monotonic activation epoch for the repo/revision pair after the
    /// mutation; `0` for a publish that activated nothing.
    pub activation_epoch: u64,
    /// Global terminal sequence (P02B allocator) that committed the
    /// mutation, or the original sequence a replayed receipt carried.
    pub terminal_sequence: u64,
    /// `true` when the original durable receipt was replayed unchanged.
    pub replayed: bool,
}

pub trait RepoMapBundleIngestPort: Send + Sync {
    fn ingest_bundle(
        &self,
        bundle: &RepoMapSourceBundle,
    ) -> Result<RepoMapMutationReceiptV1, CoreError>;
}

/// The `RepoMap` store's quarantine, listed and discarded one file at a
/// time (QI-BB-026).
///
/// A discard names one file exactly as [`Self::quarantined_files`] listed
/// it, name and reason; an implementation removes only a regular file that
/// sits in its quarantine directory right now under that recorded reason,
/// refuses anything else typed under
/// [`QUARANTINE_TARGET_NOT_QUARANTINED_CODE`](crate::QUARANTINE_TARGET_NOT_QUARANTINED_CODE),
/// and answers `Absent` for a name that is already gone.
pub trait RepoMapQuarantinePort: Send + Sync {
    fn quarantined_files(&self) -> Result<Vec<QuarantinedRepoMapFileV1>, CoreError>;

    fn discard_quarantined_file(
        &self,
        entry: &QuarantinedRepoMapFileV1,
    ) -> Result<crate::domains::generation::QuarantineDiscardOutcomeV1, CoreError>;
}

pub trait RepoMapGenerationActivatePort: Send + Sync {
    fn activate_generation(
        &self,
        request: &RepoMapActivateGenerationRequest,
    ) -> Result<RepoMapMutationReceiptV1, CoreError>;
}

/// What a `RepoMap` store found on disk when it opened (QI-BB-008).
///
/// One file the store cannot trust is quarantined with its reason and the
/// rest of the store opens; this report is where that decision becomes
/// visible instead of a whole-root failure or a silent skip.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RepoMapOpenReportV1 {
    pub snapshots_loaded: u64,
    /// Snapshot files from before the digest envelope, rewritten under it.
    pub snapshots_migrated: u64,
    pub activations_loaded: u64,
    /// Temporaries a crashed write left behind, removed at open.
    pub stale_temporaries_removed: u64,
    /// Files moved aside because they did not decode, did not match their
    /// digest, or sat under a name that is not theirs.
    pub quarantined: Vec<QuarantinedRepoMapFileV1>,
    /// Activations whose snapshot file is gone: the repo answers `NOT_FOUND`
    /// until it is activated again.
    pub activations_without_snapshot: Vec<String>,
}

/// One file a `RepoMap` store moved aside at open, and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuarantinedRepoMapFileV1 {
    pub file_name: String,
    pub reason: String,
}
