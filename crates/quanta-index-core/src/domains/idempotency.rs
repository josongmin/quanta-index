//! Durable idempotency for ingest publishes (QI-BB-032, W2).
//!
//! A producer that loses an ack re-sends the same batch. Without a durable
//! record of what was applied, the search plane could not tell a replay
//! from a first publish: it re-derived, re-embedded and re-built. The
//! catalog behind this port records, per idempotency key, the body digest
//! and the receipt the first apply produced, so a replay with the same
//! body is answered from the record without touching storage.
//!
//! The key's `batch_digest` is the canonical digest of the batch body
//! ([`crate::IngestBatchBodyV1`]): the dispatcher
//! recomputes it and refuses a batch whose carried digest differs
//! ([`BATCH_DIGEST_MISMATCH_CODE`]) before this port is reached, so two
//! bodies can never share a key. The port still refuses a `begin` whose
//! body hash differs from the recorded one ([`BATCH_DIGEST_CONFLICT_CODE`])
//! as its own invariant: the catalog does not rely on its callers having
//! verified the digest.
//!
//! The protocol is preflight → intent → apply → finalize:
//! 1. The dispatcher verifies the digest and runs the route's storage-free
//!    preflight; a batch refused here leaves no record.
//! 2. [`IdempotencyCatalogPort::begin`] records the key and body hash as in
//!    progress (or reports the existing record).
//! 3. The caller applies the batch.
//! 4. [`IdempotencyCatalogPort::finalize`] stores the receipt and marks the
//!    record applied.
//!
//! A crash between 3 and 4 leaves an in-progress record whose next replay
//! re-runs the apply; the apply paths are idempotent per operation and the
//! sealed search-corpus path recognizes a generation already sealed under
//! its identity, so the second run converges on the same durable state
//! without re-materializing or re-embedding, and then finalizes.
//!
//! Records live and die with their generation: physical GC forgets a
//! generation's records once the generation is no longer a whole sealed
//! pair on disk ([`IdempotencyCatalogPort::generations_for_pair`] lists what
//! the sweep must reconcile).

use quanta_index_contract::{
    BatchPublishReceipt, IngestOperationKindV1, ManifestGeneration, RepoId, RevisionId,
};

use crate::error::CoreError;

/// Wire code for a batch whose carried `batch_digest` is not its body's.
///
/// The batch was forged, corrupted in transit, or mutated after its digest
/// was computed; it is refused before any record or mutation.
pub const BATCH_DIGEST_MISMATCH_CODE: &str = "BATCH_DIGEST_MISMATCH";
/// Wire code for a `begin` whose body hash differs from the one recorded
/// under the same key.
pub const BATCH_DIGEST_CONFLICT_CODE: &str = "BATCH_DIGEST_CONFLICT";
/// Wire code for a catalog row whose own digest no longer matches its
/// content (G0-C: the engine serves bit-rotted cells with a clean
/// integrity check, so the row digest is the only content check).
pub const CATALOG_ROW_CORRUPT_CODE: &str = "CATALOG_ROW_CORRUPT";
/// Wire code for a catalog write that met a held lock past its busy budget.
pub const CATALOG_BUSY_CODE: &str = "CATALOG_BUSY";

/// The durable identity of one publish.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IdempotencyKeyV1 {
    pub kind: IngestOperationKindV1,
    pub repo_id: RepoId,
    pub revision_id: RevisionId,
    pub generation: ManifestGeneration,
    /// The canonical batch digest token the batch carried and the
    /// dispatcher verified.
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
    /// apply is idempotent and converges on durable state that already
    /// exists) and finalizes.
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

    /// Every generation of the pair that holds at least one record, in
    /// ascending order, so a reclaim pass can reconcile records against
    /// what is on disk (a record of a generation that never sealed, or
    /// whose pair is no longer whole, describes nothing durable).
    fn generations_for_pair(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<ManifestGeneration>, CoreError>;

    /// Drop every record for `generation` of the pair, across every route;
    /// returns how many were dropped. Idempotent: a generation with no
    /// records is `Ok(0)`.
    fn forget_generation(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<u64, CoreError>;
}
