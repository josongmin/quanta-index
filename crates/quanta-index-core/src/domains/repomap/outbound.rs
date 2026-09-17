use quanta_index_contract::{RepoMapActivateGenerationRequest, RepoMapSourceBundle};

use crate::CoreError;

pub trait RepoMapBundleIngestPort: Send + Sync {
    fn ingest_bundle(&self, bundle: &RepoMapSourceBundle) -> Result<(), CoreError>;
}

pub trait RepoMapGenerationActivatePort: Send + Sync {
    fn activate_generation(
        &self,
        request: &RepoMapActivateGenerationRequest,
    ) -> Result<(), CoreError>;
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
