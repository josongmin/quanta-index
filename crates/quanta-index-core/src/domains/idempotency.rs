//! Durable idempotency for ingest publishes (QI-BB-032, W2).
//!
//! A producer that loses an ack re-sends the same batch. Without a durable
//! record of what was applied, the search plane could not tell a replay
//! from a first publish: it re-derived, re-embedded and re-built, and a
//! different body under the same key was applied as if it were the same.
//! The catalog behind this port records, per idempotency key, the canonical
//! body hash and the receipt the first apply produced, so a replay with the
//! same body is answered from the record without touching storage and a
//! replay with a different body is refused typed before any mutation.
//!
//! The protocol is intent → apply → finalize:
//! 1. [`IdempotencyCatalogPort::begin`] records the key and body hash as in
//!    progress (or reports the existing record).
//! 2. The caller applies the batch.
//! 3. [`IdempotencyCatalogPort::finalize`] stores the receipt and marks the
//!    record applied.
//!
//! A crash between 2 and 3 leaves an in-progress record whose next replay
//! re-runs the apply; the apply paths are idempotent per operation and the
//! seal path recognizes a generation already sealed under its identity, so
//! the second run converges on the same durable state and then finalizes.

use std::fmt;

use quanta_index_contract::{BatchPublishReceipt, ManifestGeneration, RepoId, RevisionId};

use crate::error::CoreError;

/// Wire code for a replay whose body differs from the recorded one.
pub const BATCH_DIGEST_CONFLICT_CODE: &str = "BATCH_DIGEST_CONFLICT";
/// Wire code for a catalog row whose own digest no longer matches its
/// content (G0-C: the engine serves bit-rotted cells with a clean
/// integrity check, so the row digest is the only content check).
pub const CATALOG_ROW_CORRUPT_CODE: &str = "CATALOG_ROW_CORRUPT";
/// Wire code for a catalog write that met a held lock past its busy budget.
pub const CATALOG_BUSY_CODE: &str = "CATALOG_BUSY";

/// Which ingest route a key belongs to.
///
/// The same `(repo, revision, generation, batch_digest)` under two routes
/// are two records. The repo-map bundle route answers with a mutation ack,
/// not a receipt, and names no batch digest; it is outside the catalog until
/// it does.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum IngestOperationKindV1 {
    SearchCorpus,
    History,
    Dirty,
    RuntimeCatalog,
    Structural,
    RepoCommitRecency,
    RepoTopic,
    RepoDescription,
    FileOwnership,
    FileContributor,
    RepoMeta,
}

impl IngestOperationKindV1 {
    #[must_use]
    pub const fn as_code_str(self) -> &'static str {
        match self {
            Self::SearchCorpus => "search-corpus",
            Self::History => "history",
            Self::Dirty => "dirty",
            Self::RuntimeCatalog => "runtime-catalog",
            Self::Structural => "structural",
            Self::RepoCommitRecency => "repo-commit-recency",
            Self::RepoTopic => "repo-topic",
            Self::RepoDescription => "repo-description",
            Self::FileOwnership => "file-ownership",
            Self::FileContributor => "file-contributor",
            Self::RepoMeta => "repo-meta",
        }
    }
}

impl fmt::Display for IngestOperationKindV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_code_str())
    }
}

/// The durable identity of one publish.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IdempotencyKeyV1 {
    pub kind: IngestOperationKindV1,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    pub batch_digest: String,
}

/// What `begin` found or created for a key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdempotencyBeginV1 {
    /// No record existed; one is now in progress under `body_sha256`. The
    /// caller applies and then finalizes.
    Fresh,
    /// A record with the same body was already finalized: this publish is a
    /// replay, answered with the recorded receipt and no mutation.
    Replay {
        receipt: BatchPublishReceipt,
        durable_sequence: u64,
    },
    /// A record with the same body exists but was never finalized: a prior
    /// attempt crashed or is still running. The caller re-applies (the
    /// apply is idempotent) and finalizes.
    Resume,
}

/// Durable idempotency records, one per [`IdempotencyKeyV1`] (QI-BB-032).
///
/// The adapter owns the storage engine. Every row carries its own body
/// digest and receipt digest (G0-C: the engine does not detect cell
/// bit-rot), commits under full synchronization, and a write that meets a
/// held lock surfaces typed rather than blocking past the deadline.
pub trait IdempotencyCatalogPort: Send + Sync {
    /// Record `key` as in progress under `body_sha256`, or report the
    /// existing record. A record under a different body is refused with
    /// [`BATCH_DIGEST_CONFLICT_CODE`] and nothing is written.
    fn begin(
        &self,
        key: &IdempotencyKeyV1,
        body_sha256: &[u8; 32],
    ) -> Result<IdempotencyBeginV1, CoreError>;

    /// Store the receipt of a completed apply and mark the record applied;
    /// returns the record's durable sequence. Finalizing a key that was
    /// never begun, or was begun under a different body, is an error.
    fn finalize(
        &self,
        key: &IdempotencyKeyV1,
        body_sha256: &[u8; 32],
        receipt: &BatchPublishReceipt,
    ) -> Result<u64, CoreError>;

    /// Drop every record for `generation` of the pair, once the generation
    /// itself is reaped; the records outlive nothing they describe.
    fn forget_generation(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<u64, CoreError>;
}
