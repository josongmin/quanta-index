//! Durable operation journal for ingest publishes (QI-BB-032, SEP-21 P02B).
//!
//! A producer that loses an ack re-sends the same batch. Without a durable
//! record of what was applied, the search plane could not tell a replay
//! from a first publish. The catalog behind this port records, per
//! idempotency key, the body digest and the receipt the first apply
//! produced, so a replay with the same body is answered from the record
//! without touching storage.
//!
//! The protocol (SEP-21-002) is replay-first, immutable-prepare,
//! fenced-claim, terminally-classified:
//!
//! 1. **Digest verification** (dispatcher): the carried `batch_digest` is
//!    recomputed from the body; a forged digest is refused before this
//!    port is reached ([`BATCH_DIGEST_MISMATCH_CODE`]).
//! 2. **Preflight** (dispatcher): every refusal the route can make without
//!    mutating — including the auxiliary routes' semantic validation
//!    against the ledger — runs before any record exists.
//! 3. [`IdempotencyCatalogPort::inspect`] (read-only): a committed record
//!    answers the replay here; nothing is written.
//! 4. [`IdempotencyCatalogPort::claim_prepared`]: an immutable prepared
//!    row is written under the verified body digest with an owner, a
//!    lease deadline, a fence token and an input commitment.
//! 5. **Apply**: the route materializes the batch under the claim
//!    ([`PreparedMutationV1::verify`] detects drift).
//! 6. Terminal: [`IdempotencyCatalogPort::commit`] (fenced, allocates the
//!    global durable sequence and its journal event in one transaction),
//!    [`IdempotencyCatalogPort::record_refused`] (frozen-policy refusal),
//!    or [`IdempotencyCatalogPort::mark_uncertain`] when the terminal
//!    attempt itself failed ambiguously; [`IdempotencyCatalogPort::recover`]
//!    resolves expired or uncertain records to `Aborted` so a retry can
//!    claim fresh.
//!
//! A stale worker — one whose lease expired and whose record was recovered
//! or re-claimed — cannot commit: [`IdempotencyCatalogPort::commit`] checks
//! the fence token and refuses [`OperationFenceLost`](crate::CoreError).
//!
//! Records live and die with their generation, but a forgotten generation
//! raises the replay floor: a retry of an operation below the floor is
//! refused with `OPERATION_REPLAY_FLOOR` rather than silently re-executed
//! ([`IdempotencyCatalogPort::claim_prepared`]).

use std::time::{SystemTime, UNIX_EPOCH};

use quanta_index_contract::{
    BatchPublishReceipt, IngestOperationKindV1, ManifestGeneration, RepoId, RevisionId,
};
use sha2::{Digest, Sha256};

use crate::error::CoreError;

/// Wire code for a batch whose carried `batch_digest` is not its body's.
pub const BATCH_DIGEST_MISMATCH_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::BatchDigestMismatch;
/// Wire code for a `claim_prepared` whose body hash differs from the one
/// recorded under the same key.
pub const BATCH_DIGEST_CONFLICT_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::BatchDigestConflict;
/// Wire code for a catalog row whose own digest no longer matches its
/// content (G0-C).
pub const CATALOG_ROW_CORRUPT_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt;
/// Wire code for a catalog write that met a held lock past its busy budget.
pub const CATALOG_BUSY_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::CatalogBusy;
/// Wire code for a commit or refusal from a worker whose fence the journal
/// no longer honors (stale owner, expired lease, recovered record).
pub const OPERATION_FENCE_LOST_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::OperationFenceLost;
/// Wire code for a retry of an operation the journal already invalidated
/// below the replay floor (a forgotten generation's records).
pub const OPERATION_REPLAY_FLOOR_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::OperationReplayFloor;
/// Wire code for an allocation past `i64::MAX` on the global sequence.
pub const SEQUENCE_EXHAUSTED_CODE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::SequenceExhausted;

/// Parse an [`IngestOperationKindV1`] from its code string (the journal's
/// `kind` column). An unknown code is corruption, not a fallback.
pub fn ingest_kind_from_code_str(
    kind_text: &str,
) -> Result<quanta_index_contract::IngestOperationKindV1, CoreError> {
    use quanta_index_contract::IngestOperationKindV1 as K;
    match kind_text {
        "search-corpus" => Ok(K::SearchCorpus),
        "history" => Ok(K::History),
        "dirty" => Ok(K::Dirty),
        "runtime-catalog" => Ok(K::RuntimeCatalog),
        "structural" => Ok(K::Structural),
        "repo-commit-recency" => Ok(K::RepoCommitRecency),
        "repo-topic" => Ok(K::RepoTopic),
        "repo-description" => Ok(K::RepoDescription),
        "file-ownership" => Ok(K::FileOwnership),
        "file-contributor" => Ok(K::FileContributor),
        "repo-meta" => Ok(K::RepoMeta),
        other => Err(CoreError::Typed {
            code: CATALOG_ROW_CORRUPT_CODE,
            message: format!("catalog: journal kind column holds {other:?}, not a known route"),
        }),
    }
}

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

impl IdempotencyKeyV1 {
    /// The 32-byte identity the generic sequence ledger records for this
    /// key's terminal event: every field of the key in a fixed order.
    ///
    /// Infallible by construction: SHA-256 over fixed domain separation
    /// and borrowed bytes.
    #[must_use]
    pub fn identity_digest(&self) -> [u8; 32] {
        identity_digest_of(
            self.kind.as_code_str(),
            self.repo_id.as_str(),
            self.revision_id.as_str(),
            self.generation.get(),
            self.batch_digest.as_str(),
        )
    }
}

/// SHA-256 over the operation identity fields with domain separation.
fn identity_digest_of(
    kind: &str,
    repo_id: &str,
    revision_id: &str,
    generation: u64,
    batch_digest: &str,
) -> [u8; 32] {
    digest_of_parts(
        b"quanta-index:catalog:operation-identity:v1\0",
        &[
            kind.as_bytes(),
            repo_id.as_bytes(),
            revision_id.as_bytes(),
            &generation.to_le_bytes(),
            batch_digest.as_bytes(),
        ],
    )
}

fn digest_of_parts(domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for part in parts {
        hasher.update(part);
        hasher.update(b"\x1f");
    }
    hasher.finalize().into()
}

/// The typed state of one journal record (SEP-21-002).
///
/// The numeric codes are the catalog's `state` column and the DB `CHECK`
/// set; the closed transition table is
/// [`OperationJournalStateV1::transition_allowed`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationJournalStateV1 {
    /// An immutable prepared row exists; nobody holds the claim.
    Prepared = 1,
    /// A worker holds the claim under `fence_token` and `lease_deadline_ms`.
    Claimed = 2,
    /// The holder is mutating durable state under its fence.
    Applying = 3,
    /// Terminal: applied, receipt recorded, sequence allocated.
    Committed = 4,
    /// Terminal: frozen-policy refusal, exact-replayed on retry.
    Refused = 5,
    /// Terminal: recovered away (expired lease / superseded claim).
    Aborted = 6,
    /// The terminal attempt failed ambiguously; recovery must resolve it.
    Uncertain = 7,
}

impl OperationJournalStateV1 {
    /// Every state, in column-code order (the DB `CHECK` parity set).
    pub const ALL: [Self; 7] = [
        Self::Prepared,
        Self::Claimed,
        Self::Applying,
        Self::Committed,
        Self::Refused,
        Self::Aborted,
        Self::Uncertain,
    ];

    #[must_use]
    pub const fn as_code(self) -> i64 {
        match self {
            Self::Prepared => 1,
            Self::Claimed => 2,
            Self::Applying => 3,
            Self::Committed => 4,
            Self::Refused => 5,
            Self::Aborted => 6,
            Self::Uncertain => 7,
        }
    }

    /// The code the catalog's `CHECK` clause admits.
    #[must_use]
    pub fn check_set_sql() -> &'static str {
        "1,2,3,4,5,6,7"
    }

    /// Parse the catalog's column code; any other value is corruption.
    pub fn from_code(code: i64) -> Result<Self, CoreError> {
        match code {
            1 => Ok(Self::Prepared),
            2 => Ok(Self::Claimed),
            3 => Ok(Self::Applying),
            4 => Ok(Self::Committed),
            5 => Ok(Self::Refused),
            6 => Ok(Self::Aborted),
            7 => Ok(Self::Uncertain),
            other => Err(CoreError::Typed {
                code: CATALOG_ROW_CORRUPT_CODE,
                message: format!("catalog: journal state code {other} is not a known state"),
            }),
        }
    }

    /// The closed transition table: `from → to` is legal exactly when this
    /// returns `true`. `Committed`, `Refused` and `Aborted` are terminal.
    #[must_use]
    pub fn transition_allowed(from: Self, to: Self) -> bool {
        use OperationJournalStateV1 as S;
        matches!(
            (from, to),
            (S::Prepared, S::Claimed | S::Refused | S::Aborted)
                | (
                    S::Claimed,
                    S::Applying | S::Refused | S::Aborted | S::Uncertain
                )
                | (
                    S::Applying,
                    S::Committed | S::Refused | S::Aborted | S::Uncertain
                )
                | (S::Uncertain, S::Committed | S::Aborted)
        )
    }

    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Committed | Self::Refused | Self::Aborted)
    }
}

/// Wall-clock milliseconds since the Unix epoch; `lease_deadline_ms` and
/// `deadline_ms` are compared against it.
#[must_use]
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        // A pre-epoch clock reads as the epoch; a duration wider than u64
        // milliseconds cannot occur on supported targets, saturate instead
        // of truncating.
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).map_or(u64::MAX, |millis| millis)
        })
}

/// The immutable claim one publish mutates under (SEP-21-002).
///
/// `epoch_commitment` binds the validated input the claim was prepared
/// against (the verified body digest on the ingest routes); the catalog
/// re-checks it at every fenced step, so a worker that drifted — or a
/// caller replaying a stale claim after the record moved on — is refused
/// with `OPERATION_FENCE_LOST` before any mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedMutationV1 {
    pub key: IdempotencyKeyV1,
    pub body_sha256: [u8; 32],
    pub owner: String,
    pub fence_token: u64,
    pub lease_deadline_ms: u64,
    pub epoch_commitment: [u8; 32],
}

impl PreparedMutationV1 {
    /// Verify `other` describes this same claim with no drift. Called at
    /// apply time; drift is a fence loss, not a conflict.
    pub fn verify(&self, other: &PreparedMutationV1) -> Result<(), CoreError> {
        if self.key != other.key
            || self.body_sha256 != other.body_sha256
            || self.fence_token != other.fence_token
            || self.epoch_commitment != other.epoch_commitment
        {
            return Err(CoreError::Typed {
                code: OPERATION_FENCE_LOST_CODE,
                message: format!(
                    "journal: claim for {} batch_digest={} drifted from the prepared record",
                    self.key.kind, self.key.batch_digest
                ),
            });
        }
        Ok(())
    }
}

/// What [`IdempotencyCatalogPort::inspect`] found for a key. Read-only:
/// inspect never mutates, so the dispatcher can answer replays before any
/// storage work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OperationInspectV1 {
    /// No record and no invalidation: a first publish.
    Absent,
    /// A committed record with the same body: this publish is a replay.
    Committed {
        receipt: BatchPublishReceipt,
        durable_sequence: u64,
    },
    /// A record exists mid-protocol under `state`.
    InFlight {
        state: OperationJournalStateV1,
        owner: String,
        fence_token: u64,
        lease_deadline_ms: u64,
    },
    /// A terminal frozen-policy refusal: the retry replays it exactly.
    /// The code is the closed enum; stored wire text decodes fail-closed.
    Refused {
        code: quanta_index_contract::SearchPlaneErrorCodeV2,
        message: String,
    },
    /// The terminal attempt failed ambiguously and recovery has not
    /// resolved it yet.
    Uncertain { owner: String },
}

/// What [`IdempotencyCatalogPort::claim_prepared`] returned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClaimOutcomeV1 {
    /// The caller now holds the claim; every later fenced step must carry
    /// this exact [`PreparedMutationV1`].
    Claimed(PreparedMutationV1),
    /// A committed record with the same body answered the claim: replay.
    Replay {
        receipt: BatchPublishReceipt,
        durable_sequence: u64,
    },
}

/// Durable operation journal, one record per [`IdempotencyKeyV1`]
/// (QI-BB-032, SEP-21 P02B).
///
/// The adapter owns the storage engine. Every row carries its own digest
/// and receipt digest (G0-C), commits under full synchronization inside a
/// single `BEGIN IMMEDIATE` transaction that also allocates the global
/// sequence and appends the generic ledger event, and a write that meets
/// a held lock past the busy budget surfaces typed rather than blocking.
pub trait IdempotencyCatalogPort: Send + Sync {
    /// Read the record for `key` without mutating anything. A committed
    /// record answers a replay here; an invalidated key (below the replay
    /// floor) refuses `OPERATION_REPLAY_FLOOR` before any storage work.
    fn inspect(&self, key: &IdempotencyKeyV1) -> Result<OperationInspectV1, CoreError>;

    /// Write (or take over) the immutable prepared row for `key` under
    /// `body_sha256` and return the claim. Outcomes:
    /// - a committed same-body record → [`ClaimOutcomeV1::Replay`];
    /// - a terminal refusal with the same body → that refusal, exact;
    /// - a different body under the same key →
    ///   [`BATCH_DIGEST_CONFLICT_CODE`], nothing written;
    /// - an invalidated key → [`OPERATION_REPLAY_FLOOR_CODE`];
    /// - a live unexpired claim held by another owner →
    ///   [`CATALOG_BUSY_CODE`].
    fn claim_prepared(
        &self,
        key: &IdempotencyKeyV1,
        body_sha256: &[u8; 32],
        owner: &str,
        lease_deadline_ms: u64,
        epoch_commitment: &[u8; 32],
    ) -> Result<ClaimOutcomeV1, CoreError>;

    /// Transition the record `Claimed → Applying` under the claim's fence.
    /// A stale fence or a drifted commitment is `OPERATION_FENCE_LOST`.
    fn mark_applying(&self, claim: &PreparedMutationV1) -> Result<(), CoreError>;

    /// Terminal: record a frozen-policy refusal under the claim's fence
    /// and return the allocated durable sequence. The refusal is
    /// exact-replayed by later same-body attempts.
    fn record_refused(
        &self,
        claim: &PreparedMutationV1,
        refusal: &CoreError,
    ) -> Result<u64, CoreError>;

    /// Terminal: record the receipt of a completed apply under the claim's
    /// fence, allocating the global durable sequence and its
    /// `OperationCommitted` event in the same transaction. A stale fence
    /// (recovered, re-claimed or expired-then-aborted record) is
    /// `OPERATION_FENCE_LOST` and mutates nothing.
    fn commit(
        &self,
        claim: &PreparedMutationV1,
        receipt: &BatchPublishReceipt,
    ) -> Result<u64, CoreError>;

    /// Mark the terminal attempt ambiguous (`→ Uncertain`) under the
    /// claim's fence: the worker cannot know whether its commit landed.
    fn mark_uncertain(&self, claim: &PreparedMutationV1) -> Result<(), CoreError>;

    /// Resolve a record whose lease expired or whose terminal attempt was
    /// uncertain: expired or uncertain records move to terminal `Aborted`
    /// (with an `OperationAborted` event) and the outcome is
    /// [`OperationInspectV1::Absent`], so the caller re-claims fresh. A
    /// record still inside a live lease is returned as-is.
    fn recover(&self, key: &IdempotencyKeyV1) -> Result<OperationInspectV1, CoreError>;

    /// Every generation of the pair that holds at least one record, in
    /// ascending order, so a reclaim pass can reconcile records against
    /// what is on disk.
    fn generations_for_pair(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<ManifestGeneration>, CoreError>;

    /// Drop every record for `generation` of the pair, across every route,
    /// and record the generation's invalidation in the generic ledger —
    /// which raises the replay floor for every key of that generation.
    /// Returns how many records were dropped; idempotent.
    fn forget_generation(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<u64, CoreError>;
}

/// A durable cross-process mutation lease (SEP-21 `MutationCoordinatorV1`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationLeaseV1 {
    pub scope: String,
    pub owner: String,
    pub fence_token: u64,
    pub deadline_ms: u64,
}

/// The state-root-global durable mutation coordinator
/// (`MutationCoordinatorV1`).
///
/// Every ingest, control and background mutation that must be
/// machine-enforced as exclusive across processes takes a lease from this
/// port, not a per-process mutex.
///
/// `enter` refuses with [`CATALOG_BUSY_CODE`] while another live lease
/// holds the scope; `release` refuses with
/// [`OPERATION_FENCE_LOST_CODE`](crate::CoreError) when the lease is no
/// longer the holder's, so a stalled worker cannot release a successor's
/// lease.
pub trait MutationCoordinatorPort: Send + Sync {
    fn enter(&self, scope: &str, owner: &str, lease_ms: u64) -> Result<MutationLeaseV1, CoreError>;

    fn release(&self, lease: &MutationLeaseV1) -> Result<(), CoreError>;
}
