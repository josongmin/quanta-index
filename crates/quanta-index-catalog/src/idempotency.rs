//! The operation journal (QI-BB-032, SEP-21 P02B).
//!
//! Schema (`WITHOUT ROWID`, keyed by the idempotency key):
//!
//! ```text
//! idempotency_v2(kind, repo_id, revision_id, generation, batch_digest,
//!                body_sha256 BLOB, state INTEGER, owner TEXT,
//!                lease_deadline_ms INTEGER, fence_token INTEGER,
//!                input_commitment BLOB, receipt_cbor BLOB NULL,
//!                receipt_digest BLOB NULL, durable_sequence INTEGER NULL,
//!                refusal_code TEXT NULL, refusal_message TEXT NULL,
//!                row_sha256 BLOB)
//! mutation_lease_v1(scope, owner, fence_token, deadline_ms, row_sha256)
//! catalog_fence_v1(id=1, last_fence, row_sha256)
//! ```
//!
//! Only the current journal and sequence tables are created and read.
//! Superseded local tables have no compatibility reader.
//!
//! Every terminal transition (commit, refusal, abort) allocates the
//! global sequence and appends its generic ledger event
//! ([`crate::sequence`]) in the same `BEGIN IMMEDIATE` transaction that
//! writes this table's row, so allocator, event and domain row commit or
//! roll back together. `row_sha256` commits to every other column and is
//! verified on every read; `receipt_digest` commits to the versioned
//! receipt bytes.
//!
//! Persisted receipts are canonical CBOR behind an explicit format tag
//! ([`BATCH_PUBLISH_RECEIPT_FORMAT_VERSION`]); any other version is a
//! typed refusal before any mutation — there is no dual decoder.

use std::path::Path;

use quanta_index_contract::{
    BATCH_PUBLISH_RECEIPT_FORMAT_VERSION, BatchPublishReceipt, IngestOperationKindV1,
    ManifestGeneration, REPOMAP_TERMINAL_RECEIPT_FORMAT_VERSION, RepoId, RepoMapTerminalReceiptV2,
    RevisionId, SearchPlaneErrorCodeV2,
};
use quanta_index_core::{
    CATALOG_BUSY_CODE, ClaimOutcomeV1, CoreError, IdempotencyCatalogPort, IdempotencyKeyV1,
    MutationCoordinatorPort, MutationLeaseV1, OPERATION_FENCE_LOST_CODE,
    OPERATION_REPLAY_FLOOR_CODE, OperationInspectV1, OperationJournalStateV1, PreparedMutationV1,
};
use quanta_index_ipc::{decode_cbor_payload, encode_cbor_payload};
use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::connection::{SqliteCatalog, blob32, engine_error, generation_i64};
use crate::sequence::{
    SequenceEventKindV1, append_operation_invalidation, append_sequence_event,
    is_invalidated_for_floor, verify_event_reference,
};

const ROW_DIGEST_DOMAIN: &[u8] = b"quanta-index:catalog:idempotency-row:v2\0";
const RECEIPT_DIGEST_DOMAIN: &[u8] = b"quanta-index:catalog:idempotency-receipt:v2\0";
const LEASE_ROW_DIGEST_DOMAIN: &[u8] = b"quanta-index:catalog:mutation-lease-row:v1\0";
const FENCE_ALLOCATOR_ROW_DOMAIN: &[u8] = b"quanta-index:catalog:fence-allocator-row:v1\0";
const FIELD_SEPARATOR: &[u8] = b"\x1f";

/// The current journal and durable mutation lease tables, created at open.
pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS idempotency_v2 (
                     kind TEXT NOT NULL,
                     repo_id TEXT NOT NULL,
                     revision_id TEXT NOT NULL,
                     generation INTEGER NOT NULL,
                     batch_digest TEXT NOT NULL,
                     body_sha256 BLOB NOT NULL CHECK (length(body_sha256) = 32),
                     state INTEGER NOT NULL CHECK (state IN (1, 2, 3, 4, 5, 6, 7)),
                     owner TEXT NOT NULL,
                     lease_deadline_ms INTEGER NOT NULL,
                     fence_token INTEGER NOT NULL,
                     input_commitment BLOB NOT NULL CHECK (length(input_commitment) = 32),
                     receipt_cbor BLOB,
                     receipt_digest BLOB CHECK (receipt_digest IS NULL OR length(receipt_digest) = 32),
                     durable_sequence INTEGER UNIQUE,
                     refusal_code TEXT,
                     refusal_message TEXT,
                     row_sha256 BLOB NOT NULL CHECK (length(row_sha256) = 32),
                     PRIMARY KEY (kind, repo_id, revision_id, generation, batch_digest)
                  ) WITHOUT ROWID;
                  CREATE INDEX IF NOT EXISTS idempotency_v2_by_generation
                      ON idempotency_v2 (repo_id, revision_id, generation);
                  CREATE TABLE IF NOT EXISTS mutation_lease_v1 (
                     scope TEXT NOT NULL PRIMARY KEY,
                     owner TEXT NOT NULL,
                     fence_token INTEGER NOT NULL,
                     deadline_ms INTEGER NOT NULL,
                     row_sha256 BLOB NOT NULL CHECK (length(row_sha256) = 32)
                  ) WITHOUT ROWID;
                  CREATE TABLE IF NOT EXISTS catalog_fence_v1 (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     last_fence INTEGER NOT NULL CHECK (last_fence BETWEEN 0 AND 9223372036854775807),
                     row_sha256 BLOB NOT NULL CHECK (length(row_sha256) = 32)
                  ) WITHOUT ROWID;";

/// One stored journal row, as read back and verified.
struct StoredRow {
    body_sha256: [u8; 32],
    state: OperationJournalStateV1,
    owner: String,
    lease_deadline_ms: u64,
    fence_token: u64,
    input_commitment: [u8; 32],
    receipt_cbor: Option<Vec<u8>>,
    receipt_digest: Option<[u8; 32]>,
    durable_sequence: Option<u64>,
    refusal_code: Option<String>,
    refusal_message: Option<String>,
}

fn sequence_u64(sequence: i64) -> Result<u64, CoreError> {
    u64::try_from(sequence).map_err(|error| {
        CoreError::Storage(format!(
            "catalog: durable sequence {sequence} is negative: {error}"
        ))
    })
}

fn u64_i64(label: &str, value: u64) -> Result<i64, CoreError> {
    i64::try_from(value).map_err(|error| {
        CoreError::InvalidContract(format!(
            "catalog: {label} {value} does not fit the catalog's integer column: {error}"
        ))
    })
}

/// The versioned persisted form of a receipt: the format tag followed by
/// the receipt's canonical CBOR. Infallible by construction (a
/// concatenation of encoded parts); decode refuses the other direction.
fn encode_versioned_receipt(receipt: &BatchPublishReceipt) -> Result<Vec<u8>, CoreError> {
    let cbor = encode_cbor_payload(receipt)
        .map_err(|error| CoreError::Storage(format!("catalog: encode receipt: {error}")))?;
    let mut bytes = BATCH_PUBLISH_RECEIPT_FORMAT_VERSION.to_le_bytes().to_vec();
    bytes.extend_from_slice(&cbor);
    Ok(bytes)
}

fn decode_versioned_receipt(bytes: &[u8]) -> Result<BatchPublishReceipt, CoreError> {
    // A short slice cannot carry the 4-byte format tag; `get` avoids the
    // panicking slice and the disallowed `.ok()`/unwrap fallback.
    let tag = match bytes.get(..4) {
        Some(tag) => <[u8; 4]>::try_from(tag).map_err(|_error| receipt_version_mismatch(None))?,
        None => return Err(receipt_version_mismatch(None)),
    };
    let stored_version = u32::from_le_bytes(tag);
    if stored_version != BATCH_PUBLISH_RECEIPT_FORMAT_VERSION {
        return Err(receipt_version_mismatch(Some(stored_version)));
    }
    // Unreachable in practice: the tag check above guarantees len >= 4.
    let payload = bytes
        .get(4..)
        .ok_or_else(|| corrupt_row_wip("stored receipt payload is missing"))?;
    decode_cbor_payload(payload).map_err(|error| CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!("catalog: stored receipt does not decode: {error}"),
    })
}

/// Old receipt / new runtime (and the reverse) are incompatible by
/// design: the refusal is typed and precedes any mutation; there is no
/// boot-time dual decoder and no live migration.
fn receipt_version_mismatch(stored: Option<u32>) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!(
            "catalog: stored receipt carries format version {stored:?}, but this runtime reads \
             only version {BATCH_PUBLISH_RECEIPT_FORMAT_VERSION}; receipts of another version \
             are offline-migration input, not live input",
        ),
    }
}

/// The versioned persisted form of a repo-map terminal receipt.
///
/// The format tag followed by the receipt's canonical CBOR. Same
/// incompatibility rule as [`encode_versioned_receipt`]: any other
/// version is a typed refusal before any mutation.
fn encode_versioned_repomap_receipt(
    receipt: &RepoMapTerminalReceiptV2,
) -> Result<Vec<u8>, CoreError> {
    let cbor = encode_cbor_payload(receipt).map_err(|error| {
        CoreError::Storage(format!("catalog: encode repo-map receipt: {error}"))
    })?;
    let mut bytes = REPOMAP_TERMINAL_RECEIPT_FORMAT_VERSION
        .to_le_bytes()
        .to_vec();
    bytes.extend_from_slice(&cbor);
    Ok(bytes)
}

fn decode_versioned_repomap_receipt(bytes: &[u8]) -> Result<RepoMapTerminalReceiptV2, CoreError> {
    let tag = match bytes.get(..4) {
        Some(tag) => {
            <[u8; 4]>::try_from(tag).map_err(|_error| repomap_receipt_version_mismatch(None))?
        }
        None => return Err(repomap_receipt_version_mismatch(None)),
    };
    let stored_version = u32::from_le_bytes(tag);
    if stored_version != REPOMAP_TERMINAL_RECEIPT_FORMAT_VERSION {
        return Err(repomap_receipt_version_mismatch(Some(stored_version)));
    }
    let payload = bytes
        .get(4..)
        .ok_or_else(|| corrupt_row_wip("stored repo-map receipt payload is missing"))?;
    decode_cbor_payload(payload).map_err(|error| CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!("catalog: stored repo-map receipt does not decode: {error}"),
    })
}

fn repomap_receipt_version_mismatch(stored: Option<u32>) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!(
            "catalog: stored repo-map receipt carries format version {stored:?}, but this runtime \
             reads only version {REPOMAP_TERMINAL_RECEIPT_FORMAT_VERSION}; receipts of another \
             version are offline-migration input, not live input",
        ),
    }
}

/// A decoded terminal payload: batch routes persist a batch receipt,
/// repo-map bundle keys persist the repo-map terminal receipt. The
/// journal row's kind decides the decoder — never the payload bytes.
enum TerminalReceiptPayload {
    Batch(BatchPublishReceipt),
    RepoMap(RepoMapTerminalReceiptV2),
}

fn decode_terminal_payload(
    kind: &IngestOperationKindV1,
    bytes: &[u8],
) -> Result<TerminalReceiptPayload, CoreError> {
    match kind {
        IngestOperationKindV1::RepoMapBundle => {
            decode_versioned_repomap_receipt(bytes).map(TerminalReceiptPayload::RepoMap)
        }
        IngestOperationKindV1::SearchCorpus
        | IngestOperationKindV1::History
        | IngestOperationKindV1::Dirty
        | IngestOperationKindV1::RuntimeCatalog
        | IngestOperationKindV1::Structural
        | IngestOperationKindV1::RepoCommitRecency
        | IngestOperationKindV1::RepoTopic
        | IngestOperationKindV1::RepoDescription
        | IngestOperationKindV1::FileOwnership
        | IngestOperationKindV1::FileContributor
        | IngestOperationKindV1::RepoMeta => {
            decode_versioned_receipt(bytes).map(TerminalReceiptPayload::Batch)
        }
    }
}

fn receipt_digest(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(RECEIPT_DIGEST_DOMAIN);
    hasher.update(bytes);
    hasher.finalize().into()
}

fn option_bytes(hasher: &mut Sha256, bytes: Option<&[u8]>) {
    match bytes {
        Some(bytes) => {
            hasher.update([1_u8]);
            hasher.update(bytes);
        }
        None => hasher.update([0_u8]),
    }
}

fn option_u64(hasher: &mut Sha256, value: Option<u64>) {
    match value {
        Some(value) => {
            hasher.update([1_u8]);
            hasher.update(value.to_le_bytes());
        }
        None => hasher.update([0_u8]),
    }
}

fn option_str(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update([1_u8]);
            hasher.update(value.as_bytes());
        }
        None => hasher.update([0_u8]),
    }
}

/// The digest every journal row commits to: every column in a fixed
/// order with separators.
#[expect(
    clippy::too_many_arguments,
    reason = "the digest commits to every journal column in a fixed order; one argument per column keeps the preimage auditable"
)]
fn row_digest(
    key: &IdempotencyKeyV1,
    body_sha256: &[u8; 32],
    state: OperationJournalStateV1,
    owner: &str,
    lease_deadline_ms: u64,
    fence_token: u64,
    input_commitment: &[u8; 32],
    receipt_digest: Option<&[u8; 32]>,
    durable_sequence: Option<u64>,
    refusal_code: Option<&str>,
    refusal_message: Option<&str>,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ROW_DIGEST_DOMAIN);
    hasher.update(key.kind.as_code_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(key.repo_id.as_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(key.revision_id.as_str().as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(key.generation.get().to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(key.batch_digest.as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(body_sha256);
    hasher.update(FIELD_SEPARATOR);
    hasher.update(state.as_code().to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(owner.as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(lease_deadline_ms.to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(fence_token.to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(input_commitment);
    hasher.update(FIELD_SEPARATOR);
    option_bytes(&mut hasher, receipt_digest.map(<[u8; 32]>::as_slice));
    hasher.update(FIELD_SEPARATOR);
    option_u64(&mut hasher, durable_sequence);
    hasher.update(FIELD_SEPARATOR);
    option_str(&mut hasher, refusal_code);
    hasher.update(FIELD_SEPARATOR);
    option_str(&mut hasher, refusal_message);
    hasher.finalize().into()
}

fn row_digest_of(stored: &StoredRowWitness<'_>) -> [u8; 32] {
    row_digest(
        stored.key,
        &stored.body_sha256,
        stored.state,
        &stored.owner,
        stored.lease_deadline_ms,
        stored.fence_token,
        &stored.input_commitment,
        stored.receipt_digest.as_ref(),
        stored.durable_sequence,
        stored.refusal_code.as_deref(),
        stored.refusal_message.as_deref(),
    )
}

struct StoredRowWitness<'a> {
    key: &'a IdempotencyKeyV1,
    body_sha256: [u8; 32],
    state: OperationJournalStateV1,
    owner: String,
    lease_deadline_ms: u64,
    fence_token: u64,
    input_commitment: [u8; 32],
    receipt_digest: Option<[u8; 32]>,
    durable_sequence: Option<u64>,
    refusal_code: Option<String>,
    refusal_message: Option<String>,
}

/// Read one journal row inside `connection`'s current transaction and
/// verify its digest and its internal invariants.
fn read_row(
    connection: &Connection,
    path: &Path,
    key: &IdempotencyKeyV1,
) -> Result<Option<StoredRow>, CoreError> {
    let row = connection
        .query_row(
            "SELECT body_sha256, state, owner, lease_deadline_ms, fence_token,
                    input_commitment, receipt_cbor, receipt_digest, durable_sequence,
                    refusal_code, refusal_message, row_sha256
             FROM idempotency_v2
             WHERE kind = ?1 AND repo_id = ?2 AND revision_id = ?3
               AND generation = ?4 AND batch_digest = ?5",
            params![
                key.kind.as_code_str(),
                key.repo_id.as_str(),
                key.revision_id.as_str(),
                generation_i64(key.generation)?,
                key.batch_digest.as_str(),
            ],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                    row.get::<_, Option<Vec<u8>>>(6)?,
                    row.get::<_, Option<Vec<u8>>>(7)?,
                    row.get::<_, Option<i64>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Vec<u8>>(11)?,
                ))
            },
        )
        .optional()
        .map_err(|error| engine_error("read journal record", path, &error))?;
    let Some((
        body,
        state_code,
        owner,
        lease_deadline_ms,
        fence_token,
        commitment,
        receipt_cbor,
        receipt_digest,
        durable_sequence,
        refusal_code,
        refusal_message,
        row_sha256,
    )) = row
    else {
        return Ok(None);
    };
    let body_sha256 = blob32("body digest", &body)?;
    let input_commitment = blob32("input commitment", &commitment)?;
    let stored_digest = blob32("row digest", &row_sha256)?;
    let state = OperationJournalStateV1::from_code(state_code)?;
    let lease_deadline_ms = u64::try_from(lease_deadline_ms).map_err(|error| CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!("catalog: lease deadline is negative: {error}"),
    })?;
    let fence_token = u64::try_from(fence_token).map_err(|error| CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!("catalog: fence token is negative: {error}"),
    })?;
    let durable_sequence = durable_sequence.map(sequence_u64).transpose()?;
    let receipt_digest = receipt_digest
        .map(|digest| blob32("receipt digest", &digest))
        .transpose()?;
    // Internal invariants: exactly the terminal states carry a receipt or
    // a refusal, and a sequence exists only on a terminal state whose
    // event was allocated.
    let terminal_with_sequence = matches!(
        state,
        OperationJournalStateV1::Committed
            | OperationJournalStateV1::Refused
            | OperationJournalStateV1::Aborted
    );
    if terminal_with_sequence != durable_sequence.is_some() {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message: format!(
                "catalog: journal row for {} batch_digest={} in state {state:?} has sequence \
                 {durable_sequence:?}",
                key.kind, key.batch_digest
            ),
        });
    }
    match state {
        OperationJournalStateV1::Committed => {
            if receipt_cbor.is_none() || receipt_digest.is_none() || refusal_code.is_some() {
                return Err(corrupt_row(
                    key,
                    "committed row lacks receipt or carries a refusal",
                ));
            }
        }
        OperationJournalStateV1::Refused => {
            if receipt_cbor.is_some() || refusal_code.is_none() {
                return Err(corrupt_row(
                    key,
                    "refused row lacks a refusal code or carries a receipt",
                ));
            }
        }
        OperationJournalStateV1::Prepared
        | OperationJournalStateV1::Claimed
        | OperationJournalStateV1::Applying
        | OperationJournalStateV1::Aborted
        | OperationJournalStateV1::Uncertain => {
            if receipt_cbor.is_some() || refusal_code.is_some() {
                return Err(corrupt_row(
                    key,
                    "non-terminal row carries terminal payload",
                ));
            }
        }
    }
    let witness = StoredRowWitness {
        key,
        body_sha256,
        state,
        owner,
        lease_deadline_ms,
        fence_token,
        input_commitment,
        receipt_digest,
        durable_sequence,
        refusal_code,
        refusal_message,
    };
    if row_digest_of(&witness) != stored_digest {
        return Err(corrupt_row(key, "does not match its own digest"));
    }
    let stored = StoredRow {
        body_sha256,
        state,
        owner: witness.owner,
        lease_deadline_ms,
        fence_token,
        input_commitment,
        receipt_cbor,
        receipt_digest,
        durable_sequence,
        refusal_code: witness.refusal_code,
        refusal_message: witness.refusal_message,
    };
    verify_terminal_row_event(connection, path, key, &stored)?;
    Ok(Some(stored))
}

fn terminal_event_payload(
    key: &IdempotencyKeyV1,
    stored: &StoredRow,
) -> Result<Option<(SequenceEventKindV1, [u8; 32])>, CoreError> {
    let event = match stored.state {
        OperationJournalStateV1::Committed => {
            let bytes = stored
                .receipt_cbor
                .as_deref()
                .ok_or_else(|| corrupt_row(key, "has no committed receipt bytes"))?;
            if stored.receipt_digest != Some(receipt_digest(bytes)) {
                return Err(corrupt_row(
                    key,
                    "receipt bytes disagree with receipt digest",
                ));
            }
            (
                SequenceEventKindV1::OperationCommitted,
                payload_digest_of_parts(&[bytes]),
            )
        }
        OperationJournalStateV1::Refused => {
            let code = stored
                .refusal_code
                .as_deref()
                .ok_or_else(|| corrupt_row(key, "has no refusal code"))?;
            let message = stored
                .refusal_message
                .as_deref()
                .ok_or_else(|| corrupt_row(key, "has no refusal message"))?;
            (
                SequenceEventKindV1::OperationRefused,
                payload_digest_of_parts(&[code.as_bytes(), message.as_bytes()]),
            )
        }
        OperationJournalStateV1::Aborted => (
            SequenceEventKindV1::OperationAborted,
            payload_digest_of_parts(&[&stored.fence_token.to_le_bytes()]),
        ),
        OperationJournalStateV1::Prepared
        | OperationJournalStateV1::Claimed
        | OperationJournalStateV1::Applying
        | OperationJournalStateV1::Uncertain => return Ok(None),
    };
    Ok(Some(event))
}

fn verify_terminal_row_event(
    connection: &Connection,
    path: &Path,
    key: &IdempotencyKeyV1,
    stored: &StoredRow,
) -> Result<(), CoreError> {
    let Some((kind, payload)) = terminal_event_payload(key, stored)? else {
        return Ok(());
    };
    let sequence = stored
        .durable_sequence
        .ok_or_else(|| corrupt_row(key, "has no terminal sequence"))?;
    let sequence = i64::try_from(sequence)
        .map_err(|_error| corrupt_row(key, "terminal sequence does not fit i64"))?;
    verify_event_reference(
        connection,
        path,
        kind,
        sequence,
        &key.identity_digest(),
        &payload,
    )
}

fn corrupt_row(key: &IdempotencyKeyV1, what: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!(
            "catalog: journal row for {} batch_digest={} for repo={} revision={} generation={} {what}",
            key.kind,
            key.batch_digest,
            key.repo_id.as_str(),
            key.revision_id.as_str(),
            key.generation.get(),
        ),
    }
}

fn conflict(key: &IdempotencyKeyV1) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::BatchDigestConflict,
        message: format!(
            "{} batch_digest={} for repo={} revision={} generation={} was already published with a different body; a batch digest names one immutable body",
            key.kind,
            key.batch_digest,
            key.repo_id.as_str(),
            key.revision_id.as_str(),
            key.generation.get()
        ),
    }
}

fn replay_floor(key: &IdempotencyKeyV1) -> CoreError {
    CoreError::Typed {
        code: OPERATION_REPLAY_FLOOR_CODE,
        message: format!(
            "{} batch_digest={} for repo={} revision={} generation={} was invalidated below the \
             journal's replay floor (its generation was forgotten); a retry below the floor is \
             refused rather than re-executed",
            key.kind,
            key.batch_digest,
            key.repo_id.as_str(),
            key.revision_id.as_str(),
            key.generation.get()
        ),
    }
}

fn fence_lost(key: &IdempotencyKeyV1, what: &str) -> CoreError {
    CoreError::Typed {
        code: OPERATION_FENCE_LOST_CODE,
        message: format!(
            "journal: {} batch_digest={} {what}; the claim's fence is no longer honored",
            key.kind, key.batch_digest
        ),
    }
}

fn busy(what: &str) -> CoreError {
    CoreError::Typed {
        code: CATALOG_BUSY_CODE,
        message: format!("catalog: {what} met a live claim past the busy budget"),
    }
}

/// Refusal of a typed error to its frozen (code, message) pair.
fn refusal_of(error: &CoreError) -> Result<(String, String), CoreError> {
    match error {
        CoreError::Typed { code, message } => Ok((code.as_wire_str().to_string(), message.clone())),
        other @ (CoreError::InvalidContract(_)
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)) => Err(CoreError::InvalidContract(format!(
            "catalog: record_refused takes a frozen-policy typed refusal, got {other:?}"
        ))),
    }
}

fn refusal_from(wire_code: &str, message: &str) -> CoreError {
    SearchPlaneErrorCodeV2::from_wire_str(wire_code).map_or_else(
        || CoreError::Typed {
            code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message: format!("catalog: stored refusal code {wire_code:?} is not a known wire code"),
        },
        |code| CoreError::Typed {
            code,
            message: message.to_string(),
        },
    )
}

/// The payload digest of an operation terminal event: the versioned
/// receipt bytes (committed) or the frozen refusal pair (refused) or the
/// fence that was aborted (aborted).
fn payload_digest_of_parts(parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"quanta-index:catalog:operation-payload:v1\0");
    for part in parts {
        hasher.update(part);
        hasher.update(FIELD_SEPARATOR);
    }
    hasher.finalize().into()
}

fn fence_allocator_digest(last_fence: i64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(FENCE_ALLOCATOR_ROW_DOMAIN);
    hasher.update(last_fence.to_le_bytes());
    hasher.finalize().into()
}

fn read_fence_allocator(connection: &Connection, path: &Path) -> Result<i64, CoreError> {
    let row: Option<(i64, Vec<u8>)> = connection
        .query_row(
            "SELECT last_fence, row_sha256 FROM catalog_fence_v1 WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| engine_error("read fence allocator", path, &error))?;
    let (last_fence, digest) = row.ok_or_else(|| {
        CoreError::Storage(format!(
            "catalog: {} has no durable fence allocator row",
            path.display()
        ))
    })?;
    if last_fence < 0
        || blob32("fence allocator digest", &digest)? != fence_allocator_digest(last_fence)
    {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message: "catalog: fence allocator row is corrupt".to_string(),
        });
    }
    Ok(last_fence)
}

/// Seed only a genuinely new root; an existing root without this allocator
/// requires an explicit offline migration rather than silent fence reuse.
pub(crate) fn seed_fence_allocator(
    connection: &Connection,
    path: &Path,
    fresh_root: bool,
) -> Result<(), CoreError> {
    let schema: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'catalog_fence_v1'",
            [],
            |row| row.get(0),
        )
        .map_err(|error| engine_error("read installed fence schema", path, &error))?;
    let compact = schema.split_whitespace().collect::<String>();
    for required in [
        "idINTEGERPRIMARYKEYCHECK(id=1)",
        "last_fenceINTEGERNOTNULLCHECK(last_fenceBETWEEN0AND9223372036854775807)",
        "row_sha256BLOBNOTNULLCHECK(length(row_sha256)=32)",
        "WITHOUTROWID",
    ] {
        if !compact.contains(required) {
            return Err(CoreError::Storage(format!(
                "catalog: {} has an unsupported fence allocator schema",
                path.display()
            )));
        }
    }
    if fresh_root {
        let _seeded = connection
            .execute(
                "INSERT INTO catalog_fence_v1 (id, last_fence, row_sha256)
                 VALUES (1, 0, ?1)",
                params![fence_allocator_digest(0).as_slice()],
            )
            .map_err(|error| engine_error("seed fence allocator", path, &error))?;
    }
    let last_fence = read_fence_allocator(connection, path)?;
    let journal_maximum: Option<i64> = connection
        .query_row("SELECT MAX(fence_token) FROM idempotency_v2", [], |row| {
            row.get(0)
        })
        .map_err(|error| engine_error("read journal fence high-water", path, &error))?;
    let lease_maximum: Option<i64> = connection
        .query_row(
            "SELECT MAX(fence_token) FROM mutation_lease_v1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| engine_error("read mutation-lease fence high-water", path, &error))?;
    for (table, maximum) in [
        ("idempotency_v2", journal_maximum),
        ("mutation_lease_v1", lease_maximum),
    ] {
        if maximum.is_some_and(|maximum| maximum > last_fence) {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: format!(
                    "catalog: fence allocator is below the retained {table} high-water"
                ),
            });
        }
    }
    Ok(())
}

/// Allocate a root-global, never-reused fence in the owning write transaction.
fn next_fence_token(
    transaction: &rusqlite::Transaction<'_>,
    path: &Path,
) -> Result<u64, CoreError> {
    let last_fence = read_fence_allocator(transaction, path)?;
    let next = last_fence
        .checked_add(1)
        .ok_or_else(|| CoreError::Storage("catalog: fence allocator is exhausted".to_string()))?;
    let updated = transaction
        .execute(
            "UPDATE catalog_fence_v1 SET last_fence = ?1, row_sha256 = ?2
             WHERE id = 1 AND last_fence = ?3 AND row_sha256 = ?4",
            params![
                next,
                fence_allocator_digest(next).as_slice(),
                last_fence,
                fence_allocator_digest(last_fence).as_slice(),
            ],
        )
        .map_err(|error| engine_error("advance fence allocator", path, &error))?;
    if updated != 1 {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message: "catalog: fence allocator CAS did not advance exactly one row".to_string(),
        });
    }
    u64::try_from(next)
        .map_err(|error| CoreError::Storage(format!("catalog: fence conversion: {error}")))
}

/// Verify the claim against the stored row and the fence.
fn check_claim(
    stored: &StoredRow,
    claim: &PreparedMutationV1,
    what: &str,
) -> Result<(), CoreError> {
    // `stored.input_commitment` is the journal column that the claim's
    // `epoch_commitment` field was written into; the differing names are
    // intentional (column vs. claim vocabulary), not a typo.
    #[expect(
        clippy::suspicious_operation_groupings,
        reason = "the journal column is `input_commitment` and the claim field is `epoch_commitment`; the cross-name comparison is the intended identity check"
    )]
    if stored.body_sha256 != claim.body_sha256 || stored.input_commitment != claim.epoch_commitment
    {
        return Err(fence_lost(&claim.key, what));
    }
    if stored.fence_token != claim.fence_token || stored.owner != claim.owner {
        return Err(fence_lost(&claim.key, what));
    }
    Ok(())
}

/// Recover every unfinished journal row at catalog open (S21-04 crash
/// recovery).
///
/// The state root has exactly one writer process at a time — the daemon
/// lease enforces it — so a `Prepared`, `Claimed` or `Applying` row found
/// while opening the catalog belongs to a process that crashed or was
/// killed mid-operation. Such a row can never reach a terminal state on
/// its own, and leaving it behind would answer every retry with
/// `CATALOG_BUSY` until its lease expired.
///
/// Each unfinished row is therefore aborted here, inside the caller's
/// `BEGIN IMMEDIATE` startup transaction: an `OperationAborted` event attributes
/// the transition in the generic ledger, and the row itself becomes
/// `Aborted`, which `claim_prepared` already treats as superseded. The
/// committed/refused history is untouched.
pub(crate) fn recover_unfinished_rows(
    transaction: &rusqlite::Transaction<'_>,
    path: &Path,
) -> Result<u64, CoreError> {
    use quanta_index_core::OperationJournalStateV1 as State;
    let unfinished = [
        State::Prepared.as_code(),
        State::Claimed.as_code(),
        State::Applying.as_code(),
    ];
    let mut recovered: u64 = 0;
    for state_code in unfinished {
        let keys = unfinished_keys(transaction, path, state_code)?;
        for key in keys {
            let stored = read_row(transaction, path, &key)?
                .ok_or_else(|| corrupt_row(&key, "unfinished row vanished mid-recovery"))?;
            let identity = key.identity_digest();
            let payload = payload_digest_of_parts(&[&stored.fence_token.to_le_bytes()]);
            let sequence = append_sequence_event(
                transaction,
                SequenceEventKindV1::OperationAborted,
                &identity,
                &payload,
            )?;
            let aborted = StoredRowWitness {
                state: State::Aborted,
                receipt_digest: None,
                durable_sequence: Some(
                    u64::try_from(sequence)
                        .map_err(|_error| corrupt_row(&key, "aborted sequence does not fit u64"))?,
                ),
                refusal_code: None,
                refusal_message: None,
                ..StoredRowWitness {
                    key: &key,
                    body_sha256: stored.body_sha256,
                    state: stored.state,
                    owner: stored.owner.clone(),
                    lease_deadline_ms: stored.lease_deadline_ms,
                    fence_token: stored.fence_token,
                    input_commitment: stored.input_commitment,
                    receipt_digest: stored.receipt_digest,
                    durable_sequence: stored.durable_sequence,
                    refusal_code: stored.refusal_code.clone(),
                    refusal_message: stored.refusal_message.clone(),
                }
            };
            let _written = transaction
                .execute(
                    "UPDATE idempotency_v2 SET state = ?1, durable_sequence = ?2, row_sha256 = ?3
                     WHERE kind = ?4 AND repo_id = ?5 AND revision_id = ?6 AND generation = ?7
                       AND batch_digest = ?8",
                    params![
                        State::Aborted.as_code(),
                        sequence,
                        row_digest_of(&aborted).as_slice(),
                        key.kind.as_code_str(),
                        key.repo_id.as_str(),
                        key.revision_id.as_str(),
                        i64::try_from(key.generation.get()).map_err(|_error| {
                            corrupt_row(&key, "generation does not fit the catalog")
                        })?,
                        key.batch_digest.as_str(),
                    ],
                )
                .map_err(|error| engine_error("abort unfinished journal row", path, &error))?;
            recovered = recovered.saturating_add(1);
        }
    }
    Ok(recovered)
}

type StoredKeyRaw = (String, String, String, i64, String);

fn stored_key_from_raw(raw: StoredKeyRaw) -> Result<IdempotencyKeyV1, CoreError> {
    let (kind, repo_id, revision_id, generation, batch_digest) = raw;
    let kind = quanta_index_core::ingest_kind_from_code_str(kind.as_str())?;
    let generation = u64::try_from(generation).map_err(|_error| {
        CoreError::Storage("catalog: negative generation in journal".to_string())
    })?;
    Ok(IdempotencyKeyV1 {
        kind,
        repo_id: RepoId::new(repo_id.as_str()).map_err(|error| {
            CoreError::Storage(format!(
                "catalog: journal row holds an invalid repo ID: {error}"
            ))
        })?,
        revision_id: RevisionId::new(revision_id.as_str()).map_err(|error| {
            CoreError::Storage(format!(
                "catalog: journal row holds an invalid revision ID: {error}"
            ))
        })?,
        generation: ManifestGeneration::new(generation),
        batch_digest,
    })
}

/// Check the row-to-ledger direction before the forward event pass. A terminal
/// row cannot borrow an event of another kind or omit its event entirely.
pub(crate) fn verify_terminal_domain_integrity(
    connection: &Connection,
    path: &Path,
) -> Result<(), CoreError> {
    let invalid: Option<i64> = connection
        .query_row(
            "SELECT COALESCE(i.durable_sequence, -1) FROM idempotency_v2 AS i
             WHERE (i.state IN (?1, ?2, ?3) AND
                    (i.durable_sequence IS NULL OR NOT EXISTS (
                        SELECT 1 FROM catalog_sequence_event_v2 AS e
                        WHERE e.sequence = i.durable_sequence
                          AND e.kind = CASE i.state
                              WHEN ?1 THEN ?4 WHEN ?2 THEN ?5 WHEN ?3 THEN ?6 END
                    )))
                OR (i.state NOT IN (?1, ?2, ?3) AND i.durable_sequence IS NOT NULL)
             LIMIT 1",
            params![
                OperationJournalStateV1::Committed.as_code(),
                OperationJournalStateV1::Refused.as_code(),
                OperationJournalStateV1::Aborted.as_code(),
                SequenceEventKindV1::OperationCommitted.as_code(),
                SequenceEventKindV1::OperationRefused.as_code(),
                SequenceEventKindV1::OperationAborted.as_code(),
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| engine_error("verify terminal journal event references", path, &error))?;
    if let Some(sequence) = invalid {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message: format!(
                "catalog: journal row at sequence {sequence} has a missing or wrong-kind event"
            ),
        });
    }
    Ok(())
}

/// Verify a current terminal row against its canonical key, row digest and
/// exact event identity/payload. An invalidated historical event may have no
/// surviving row and is handled by the ledger caller.
pub(crate) fn verify_terminal_event_pair(
    connection: &Connection,
    path: &Path,
    sequence: i64,
) -> Result<Option<i64>, CoreError> {
    let raw: Option<StoredKeyRaw> = connection
        .query_row(
            "SELECT kind, repo_id, revision_id, generation, batch_digest
             FROM idempotency_v2 WHERE durable_sequence = ?1",
            params![sequence],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| engine_error("read terminal journal event pair", path, &error))?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    let key = stored_key_from_raw(raw)?;
    let stored = read_row(connection, path, &key)?
        .ok_or_else(|| corrupt_row(&key, "disappeared during terminal event pairing"))?;
    Ok(Some(stored.state.as_code()))
}

fn unfinished_keys(
    transaction: &rusqlite::Transaction<'_>,
    path: &Path,
    state_code: i64,
) -> Result<Vec<IdempotencyKeyV1>, CoreError> {
    let mut statement = transaction
        .prepare(
            "SELECT kind, repo_id, revision_id, generation, batch_digest
             FROM idempotency_v2 WHERE state = ?1",
        )
        .map_err(|error| engine_error("prepare unfinished row scan", path, &error))?;
    let rows = statement
        .query_map(params![state_code], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|error| engine_error("scan unfinished rows", path, &error))?;
    let mut keys = Vec::new();
    for row in rows {
        keys.push(stored_key_from_raw(row.map_err(|error| {
            engine_error("read unfinished row", path, &error)
        })?)?);
    }
    Ok(keys)
}

/// Release every durable mutation lease at catalog open (S21-04/S21-10
/// crash recovery).
///
/// The state root admits one writer process at a time, so a lease row
/// found while opening belongs to a process that is gone: it can never
/// release its own lease and would answer every later mutation with
/// `CATALOG_BUSY` until its deadline passed. Clearing the table here
/// restores the invariant "one live writer" for the process that is
/// actually running.
pub(crate) fn release_stale_mutation_leases(
    transaction: &rusqlite::Transaction<'_>,
    path: &Path,
) -> Result<u64, CoreError> {
    let scopes: Vec<String> = {
        let mut statement = transaction
            .prepare("SELECT scope FROM mutation_lease_v1")
            .map_err(|error| engine_error("prepare stale lease scan", path, &error))?;
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| engine_error("scan stale leases", path, &error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| engine_error("read stale lease scope", path, &error))?
    };
    for scope in &scopes {
        if read_lease_row(transaction, path, scope)?.is_none() {
            return Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: format!("catalog: stale mutation lease {scope} disappeared during scan"),
            });
        }
    }
    let released = transaction
        .execute("DELETE FROM mutation_lease_v1", [])
        .map_err(|error| engine_error("release stale mutation leases", path, &error))?;
    Ok(u64::try_from(released).map_or(u64::MAX, |released| released))
}

#[expect(
    clippy::too_many_arguments,
    reason = "the INSERT binds one value per journal column; one argument per column keeps the statement auditable"
)]
fn write_row(
    transaction: &rusqlite::Transaction<'_>,
    key: &IdempotencyKeyV1,
    body_sha256: &[u8; 32],
    state: OperationJournalStateV1,
    owner: &str,
    lease_deadline_ms: u64,
    fence_token: u64,
    input_commitment: &[u8; 32],
    receipt_cbor: Option<&[u8]>,
    receipt_digest: Option<&[u8; 32]>,
    durable_sequence: Option<u64>,
    refusal_code: Option<&str>,
    refusal_message: Option<&str>,
) -> Result<usize, CoreError> {
    let digest = row_digest(
        key,
        body_sha256,
        state,
        owner,
        lease_deadline_ms,
        fence_token,
        input_commitment,
        receipt_digest,
        durable_sequence,
        refusal_code,
        refusal_message,
    );
    transaction
        .execute(
            "INSERT INTO idempotency_v2
                 (kind, repo_id, revision_id, generation, batch_digest, body_sha256, state,
                  owner, lease_deadline_ms, fence_token, input_commitment, receipt_cbor,
                  receipt_digest, durable_sequence, refusal_code, refusal_message, row_sha256)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT (kind, repo_id, revision_id, generation, batch_digest) DO UPDATE SET
                 state = excluded.state, owner = excluded.owner,
                 lease_deadline_ms = excluded.lease_deadline_ms,
                 fence_token = excluded.fence_token,
                 input_commitment = excluded.input_commitment,
                 receipt_cbor = excluded.receipt_cbor,
                 receipt_digest = excluded.receipt_digest,
                 durable_sequence = excluded.durable_sequence,
                 refusal_code = excluded.refusal_code,
                 refusal_message = excluded.refusal_message,
                 row_sha256 = excluded.row_sha256",
            params![
                key.kind.as_code_str(),
                key.repo_id.as_str(),
                key.revision_id.as_str(),
                generation_i64(key.generation)?,
                key.batch_digest.as_str(),
                body_sha256.as_slice(),
                state.as_code(),
                owner,
                u64_i64("lease deadline", lease_deadline_ms)?,
                u64_i64("fence token", fence_token)?,
                input_commitment.as_slice(),
                receipt_cbor,
                receipt_digest.map(<[u8; 32]>::as_slice),
                durable_sequence
                    .map(|sequence| u64_i64("durable sequence", sequence))
                    .transpose()?,
                refusal_code,
                refusal_message,
                digest.as_slice(),
            ],
        )
        .map_err(|error| engine_error("write journal record", Path::new(":catalog:"), &error))
}

fn inspect_stored(
    key: &IdempotencyKeyV1,
    stored: &StoredRow,
) -> Result<OperationInspectV1, CoreError> {
    Ok(match stored.state {
        OperationJournalStateV1::Committed => {
            let bytes = stored
                .receipt_cbor
                .as_deref()
                .ok_or_else(|| corrupt_row_wip("committed row has no receipt bytes"))?;
            if let (Some(_sequence), Some(digest)) =
                (stored.durable_sequence, stored.receipt_digest)
                && receipt_digest(bytes) != digest
            {
                return Err(corrupt_row_wip(
                    "receipt bytes do not match the receipt digest",
                ));
            }
            let durable_sequence = stored
                .durable_sequence
                .ok_or_else(|| corrupt_row_wip("committed row has no sequence"))?;
            match decode_terminal_payload(&key.kind, bytes)? {
                TerminalReceiptPayload::Batch(receipt) => OperationInspectV1::Committed {
                    receipt,
                    durable_sequence,
                },
                TerminalReceiptPayload::RepoMap(receipt) => OperationInspectV1::CommittedRepoMap {
                    receipt,
                    durable_sequence,
                },
            }
        }
        OperationJournalStateV1::Refused => OperationInspectV1::Refused {
            code: SearchPlaneErrorCodeV2::from_wire_str(
                stored
                    .refusal_code
                    .as_deref()
                    .ok_or_else(|| corrupt_row_wip("refused row has no code"))?,
            )
            .ok_or_else(|| corrupt_row_wip("refused row has an unknown code"))?,
            message: stored.refusal_message.clone().unwrap_or_default(),
            durable_sequence: stored
                .durable_sequence
                .ok_or_else(|| corrupt_row_wip("refused row has no sequence"))?,
        },
        OperationJournalStateV1::Aborted => OperationInspectV1::Absent,
        OperationJournalStateV1::Uncertain => OperationInspectV1::Uncertain {
            owner: stored.owner.clone(),
        },
        state @ (OperationJournalStateV1::Prepared
        | OperationJournalStateV1::Claimed
        | OperationJournalStateV1::Applying) => OperationInspectV1::InFlight {
            state,
            owner: stored.owner.clone(),
            fence_token: stored.fence_token,
            lease_deadline_ms: stored.lease_deadline_ms,
        },
    })
}

fn corrupt_row_wip(what: &str) -> CoreError {
    CoreError::Typed {
        code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!("catalog: {what}"),
    }
}

impl IdempotencyCatalogPort for SqliteCatalog {
    fn inspect(&self, key: &IdempotencyKeyV1) -> Result<OperationInspectV1, CoreError> {
        let connection = self.lock()?;
        read_row(&connection, &self.path, key)?.map_or(Ok(OperationInspectV1::Absent), |stored| {
            inspect_stored(key, &stored)
        })
    }

    fn prepare(
        &self,
        key: &IdempotencyKeyV1,
        body_sha256: &[u8; 32],
        owner: &str,
        lease_deadline_ms: u64,
        epoch_commitment: &[u8; 32],
    ) -> Result<PreparedMutationV1, CoreError> {
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin transaction", &path, &error))?;
        // Replay floor first, exactly as `claim_prepared`: an invalidated
        // key refuses before any storage work.
        if is_invalidated_for_floor(&transaction, &path, &key.identity_digest())? {
            return Err(replay_floor(key));
        }
        let now = self.clock.now_unix_ms();
        let outcome = match read_row(&transaction, &path, key)? {
            None => prepare_fresh(
                &transaction,
                &path,
                key,
                body_sha256,
                owner,
                lease_deadline_ms,
                epoch_commitment,
            )?,
            Some(stored) => {
                if stored.body_sha256 != *body_sha256 {
                    return Err(conflict(key));
                }
                match stored.state {
                    OperationJournalStateV1::Committed | OperationJournalStateV1::Refused => {
                        // The inspect missed a terminal row in a race: the
                        // retry replays instead of preparing.
                        return Err(fence_lost(key, "prepare met a terminal record"));
                    }
                    OperationJournalStateV1::Prepared => {
                        if stored.input_commitment != *epoch_commitment {
                            return Err(fence_lost(key, "prepare met a drifted prepared record"));
                        }
                        if stored.owner != owner && stored.lease_deadline_ms > now {
                            return Err(busy("prepare met a live prepare"));
                        }
                        if stored.owner == owner {
                            // Idempotent prepare: the same owner retrying
                            // after a lost prepare answer reuses its row.
                            PreparedMutationV1 {
                                key: key.clone(),
                                body_sha256: stored.body_sha256,
                                owner: stored.owner.clone(),
                                fence_token: stored.fence_token,
                                lease_deadline_ms: stored.lease_deadline_ms,
                                epoch_commitment: stored.input_commitment,
                            }
                        } else {
                            // Expired foreign prepare: take over fresh.
                            prepare_fresh(
                                &transaction,
                                &path,
                                key,
                                body_sha256,
                                owner,
                                lease_deadline_ms,
                                epoch_commitment,
                            )?
                        }
                    }
                    OperationJournalStateV1::Claimed | OperationJournalStateV1::Applying => {
                        if stored.lease_deadline_ms > now {
                            return Err(busy("prepare met a live claim"));
                        }
                        // Expired lease: abort with its ledger event, then
                        // prepare fresh — the same takeover `claim_prepared`
                        // performs, minus the claim.
                        let identity = key.identity_digest();
                        let payload = payload_digest_of_parts(&[&stored.fence_token.to_le_bytes()]);
                        let sequence = append_sequence_event(
                            &transaction,
                            SequenceEventKindV1::OperationAborted,
                            &identity,
                            &payload,
                        )?;
                        let _aborted = write_row(
                            &transaction,
                            key,
                            &stored.body_sha256,
                            OperationJournalStateV1::Aborted,
                            &stored.owner,
                            stored.lease_deadline_ms,
                            stored.fence_token,
                            &stored.input_commitment,
                            None,
                            None,
                            Some(sequence_u64(sequence)?),
                            None,
                            None,
                        )?;
                        let _invalidation = append_operation_invalidation(
                            &transaction,
                            SequenceEventKindV1::OperationInvalidation,
                            &identity,
                        )?;
                        prepare_fresh(
                            &transaction,
                            &path,
                            key,
                            body_sha256,
                            owner,
                            lease_deadline_ms,
                            epoch_commitment,
                        )?
                    }
                    OperationJournalStateV1::Aborted | OperationJournalStateV1::Uncertain => {
                        // Superseded: the event ledger keeps the terminal
                        // history (attributable through this invalidation),
                        // and the row is reclaimed by the new prepare.
                        let _invalidation = append_operation_invalidation(
                            &transaction,
                            SequenceEventKindV1::OperationInvalidation,
                            &key.identity_digest(),
                        )?;
                        prepare_fresh(
                            &transaction,
                            &path,
                            key,
                            body_sha256,
                            owner,
                            lease_deadline_ms,
                            epoch_commitment,
                        )?
                    }
                }
            }
        };
        transaction
            .commit()
            .map_err(|error| engine_error("commit prepare", &path, &error))?;
        drop(connection);
        Ok(outcome)
    }

    fn claim_prepared(
        &self,
        key: &IdempotencyKeyV1,
        body_sha256: &[u8; 32],
        owner: &str,
        lease_deadline_ms: u64,
        epoch_commitment: &[u8; 32],
    ) -> Result<ClaimOutcomeV1, CoreError> {
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin transaction", &path, &error))?;
        // Replay floor first: an invalidated key refuses before any
        // storage work. Only the retained GC floor row, paired with its
        // target-bound ledger event, blocks replay; retry supersession does
        // not raise that floor.
        if is_invalidated_for_floor(&transaction, &path, &key.identity_digest())? {
            return Err(replay_floor(key));
        }
        let now = self.clock.now_unix_ms();
        let outcome = match read_row(&transaction, &path, key)? {
            None => {
                let fence = next_fence_token(&transaction, &path)?;
                // Prepared (immutable prepare) → Claimed (fenced claim) in
                // one transaction: an outsider only ever sees the claim.
                let _inserted = write_row(
                    &transaction,
                    key,
                    body_sha256,
                    OperationJournalStateV1::Prepared,
                    owner,
                    lease_deadline_ms,
                    fence,
                    epoch_commitment,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
                let _claimed = write_row(
                    &transaction,
                    key,
                    body_sha256,
                    OperationJournalStateV1::Claimed,
                    owner,
                    lease_deadline_ms,
                    fence,
                    epoch_commitment,
                    None,
                    None,
                    None,
                    None,
                    None,
                )?;
                ClaimOutcomeV1::Claimed(PreparedMutationV1 {
                    key: key.clone(),
                    body_sha256: *body_sha256,
                    owner: owner.to_string(),
                    fence_token: fence,
                    lease_deadline_ms,
                    epoch_commitment: *epoch_commitment,
                })
            }
            Some(stored) => {
                if stored.body_sha256 != *body_sha256 {
                    return Err(conflict(key));
                }
                match stored.state {
                    OperationJournalStateV1::Committed => {
                        let bytes = stored.receipt_cbor.as_deref().ok_or_else(|| {
                            corrupt_row(key, "committed row has no receipt bytes")
                        })?;
                        let durable_sequence = stored
                            .durable_sequence
                            .ok_or_else(|| corrupt_row(key, "committed row has no sequence"))?;
                        match decode_terminal_payload(&key.kind, bytes)? {
                            TerminalReceiptPayload::Batch(receipt) => ClaimOutcomeV1::Replay {
                                receipt,
                                durable_sequence,
                            },
                            TerminalReceiptPayload::RepoMap(receipt) => {
                                ClaimOutcomeV1::ReplayRepoMap {
                                    receipt,
                                    durable_sequence,
                                }
                            }
                        }
                    }
                    OperationJournalStateV1::Refused => {
                        return Err(refusal_from(
                            stored.refusal_code.as_deref().ok_or_else(|| {
                                corrupt_row(key, "refused row has no refusal code")
                            })?,
                            stored.refusal_message.as_deref().unwrap_or_default(),
                        ));
                    }
                    OperationJournalStateV1::Claimed | OperationJournalStateV1::Applying => {
                        if stored.lease_deadline_ms > now {
                            return Err(busy("claim_prepared met a live claim"));
                        }
                        // Expired lease of our own or another owner:
                        // recover it to Aborted, then take over.
                        let identity = key.identity_digest();
                        let payload = payload_digest_of_parts(&[&stored.fence_token.to_le_bytes()]);
                        let sequence = append_sequence_event(
                            &transaction,
                            SequenceEventKindV1::OperationAborted,
                            &identity,
                            &payload,
                        )?;
                        let _aborted = write_row(
                            &transaction,
                            key,
                            body_sha256,
                            OperationJournalStateV1::Aborted,
                            &stored.owner,
                            stored.lease_deadline_ms,
                            stored.fence_token,
                            &stored.input_commitment,
                            None,
                            None,
                            Some(sequence_u64(sequence)?),
                            None,
                            None,
                        )?;
                        let _invalidation = append_operation_invalidation(
                            &transaction,
                            SequenceEventKindV1::OperationInvalidation,
                            &identity,
                        )?;
                        claim_fresh(
                            &transaction,
                            &path,
                            key,
                            body_sha256,
                            owner,
                            lease_deadline_ms,
                            epoch_commitment,
                        )?
                    }
                    OperationJournalStateV1::Aborted | OperationJournalStateV1::Uncertain => {
                        // Aborted/Uncertain records are superseded: the
                        // event ledger keeps their terminal history
                        // (attributable through this invalidation), and
                        // the row is reclaimed by the new claim.
                        let _invalidation = append_operation_invalidation(
                            &transaction,
                            SequenceEventKindV1::OperationInvalidation,
                            &key.identity_digest(),
                        )?;
                        claim_fresh(
                            &transaction,
                            &path,
                            key,
                            body_sha256,
                            owner,
                            lease_deadline_ms,
                            epoch_commitment,
                        )?
                    }
                    OperationJournalStateV1::Prepared => {
                        // A prepared row no worker finished claiming: the
                        // prepare is immutable, so the claim continues —
                        // unless another owner is still inside its live
                        // prepare window, in which case the claim waits.
                        if stored.owner != owner && stored.lease_deadline_ms > now {
                            return Err(busy("claim_prepared met a live prepare"));
                        }
                        claim_fresh(
                            &transaction,
                            &path,
                            key,
                            body_sha256,
                            owner,
                            lease_deadline_ms,
                            epoch_commitment,
                        )?
                    }
                }
            }
        };
        transaction
            .commit()
            .map_err(|error| engine_error("commit claim", &path, &error))?;
        drop(connection);
        Ok(outcome)
    }

    fn mark_applying(&self, claim: &PreparedMutationV1) -> Result<(), CoreError> {
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin transaction", &path, &error))?;
        let stored = read_row(&transaction, &path, &claim.key)?
            .ok_or_else(|| fence_lost(&claim.key, "mark_applying found no record"))?;
        check_claim(&stored, claim, "mark_applying met a drifted record")?;
        if stored.state != OperationJournalStateV1::Claimed {
            return Err(fence_lost(
                &claim.key,
                "mark_applying met a non-claimed record",
            ));
        }
        let _written = write_row(
            &transaction,
            &claim.key,
            &claim.body_sha256,
            OperationJournalStateV1::Applying,
            &claim.owner,
            claim.lease_deadline_ms,
            claim.fence_token,
            &claim.epoch_commitment,
            None,
            None,
            None,
            None,
            None,
        )?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit mark_applying", &path, &error))?;
        drop(connection);
        Ok(())
    }

    fn record_refused(
        &self,
        claim: &PreparedMutationV1,
        refusal: &CoreError,
    ) -> Result<u64, CoreError> {
        let (code, message) = refusal_of(refusal)?;
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin transaction", &path, &error))?;
        let stored = read_row(&transaction, &path, &claim.key)?
            .ok_or_else(|| fence_lost(&claim.key, "record_refused found no record"))?;
        check_claim(&stored, claim, "record_refused met a drifted record")?;
        // A frozen refusal lands from the immutable prepared mutation
        // (`Prepared → Refused`, no claim ever held) or from the
        // applying claim (`Applying → Refused`). A bare `Claimed`
        // mutation cannot refuse: the worker has not started applying,
        // so there is no typed apply outcome to freeze.
        if !matches!(
            stored.state,
            OperationJournalStateV1::Prepared | OperationJournalStateV1::Applying
        ) {
            return Err(fence_lost(
                &claim.key,
                "record_refused met a record outside its prepare or apply",
            ));
        }
        let payload = payload_digest_of_parts(&[code.as_bytes(), message.as_bytes()]);
        let sequence = append_sequence_event(
            &transaction,
            SequenceEventKindV1::OperationRefused,
            &claim.key.identity_digest(),
            &payload,
        )?;
        let _written = write_row(
            &transaction,
            &claim.key,
            &claim.body_sha256,
            OperationJournalStateV1::Refused,
            &claim.owner,
            claim.lease_deadline_ms,
            claim.fence_token,
            &claim.epoch_commitment,
            None,
            None,
            Some(sequence_u64(sequence)?),
            Some(code.as_str()),
            Some(message.as_str()),
        )?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit record_refused", &path, &error))?;
        drop(connection);
        sequence_u64(sequence)
    }

    fn commit(
        &self,
        claim: &PreparedMutationV1,
        receipt: &BatchPublishReceipt,
    ) -> Result<u64, CoreError> {
        if claim.key.kind == IngestOperationKindV1::RepoMapBundle {
            return Err(CoreError::InvalidContract(
                "catalog: commit takes a batch receipt; repo-map bundle keys commit through \
                 commit_repomap"
                    .to_string(),
            ));
        }
        let receipt_cbor = encode_versioned_receipt(receipt)?;
        let receipt_digest = receipt_digest(&receipt_cbor);
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin transaction", &path, &error))?;
        let stored = read_row(&transaction, &path, &claim.key)?
            .ok_or_else(|| fence_lost(&claim.key, "commit found no record"))?;
        check_claim(&stored, claim, "commit met a drifted record")?;
        if !matches!(
            stored.state,
            OperationJournalStateV1::Applying | OperationJournalStateV1::Uncertain
        ) {
            return Err(fence_lost(
                &claim.key,
                "commit met a record outside its apply",
            ));
        }
        let payload = payload_digest_of_parts(&[&receipt_cbor]);
        let sequence = append_sequence_event(
            &transaction,
            SequenceEventKindV1::OperationCommitted,
            &claim.key.identity_digest(),
            &payload,
        )?;
        let _written = write_row(
            &transaction,
            &claim.key,
            &claim.body_sha256,
            OperationJournalStateV1::Committed,
            &claim.owner,
            claim.lease_deadline_ms,
            claim.fence_token,
            &claim.epoch_commitment,
            Some(&receipt_cbor),
            Some(&receipt_digest),
            Some(sequence_u64(sequence)?),
            None,
            None,
        )?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit record", &path, &error))?;
        drop(connection);
        sequence_u64(sequence)
    }

    fn commit_repomap(
        &self,
        claim: &PreparedMutationV1,
        receipt: &RepoMapTerminalReceiptV2,
    ) -> Result<u64, CoreError> {
        if claim.key.kind != IngestOperationKindV1::RepoMapBundle {
            return Err(CoreError::InvalidContract(
                "catalog: commit_repomap takes a repo-map bundle key; batch keys commit through \
                 commit"
                    .to_string(),
            ));
        }
        let receipt_cbor = encode_versioned_repomap_receipt(receipt)?;
        let digest = receipt_digest(&receipt_cbor);
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin transaction", &path, &error))?;
        let stored = read_row(&transaction, &path, &claim.key)?
            .ok_or_else(|| fence_lost(&claim.key, "commit_repomap found no record"))?;
        check_claim(&stored, claim, "commit_repomap met a drifted record")?;
        if !matches!(
            stored.state,
            OperationJournalStateV1::Applying | OperationJournalStateV1::Uncertain
        ) {
            return Err(fence_lost(
                &claim.key,
                "commit_repomap met a record outside its apply",
            ));
        }
        let payload = payload_digest_of_parts(&[&receipt_cbor]);
        let sequence = append_sequence_event(
            &transaction,
            SequenceEventKindV1::OperationCommitted,
            &claim.key.identity_digest(),
            &payload,
        )?;
        let _written = write_row(
            &transaction,
            &claim.key,
            &claim.body_sha256,
            OperationJournalStateV1::Committed,
            &claim.owner,
            claim.lease_deadline_ms,
            claim.fence_token,
            &claim.epoch_commitment,
            Some(&receipt_cbor),
            Some(&digest),
            Some(sequence_u64(sequence)?),
            None,
            None,
        )?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit repo-map record", &path, &error))?;
        drop(connection);
        sequence_u64(sequence)
    }

    fn mark_uncertain(&self, claim: &PreparedMutationV1) -> Result<(), CoreError> {
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin transaction", &path, &error))?;
        let stored = read_row(&transaction, &path, &claim.key)?
            .ok_or_else(|| fence_lost(&claim.key, "mark_uncertain found no record"))?;
        check_claim(&stored, claim, "mark_uncertain met a drifted record")?;
        // Ambiguity exists only once the worker started applying
        // (`Applying → Uncertain`); a bare claim or a prepare cannot be
        // uncertain, and re-marking an uncertain row is idempotent.
        if !matches!(
            stored.state,
            OperationJournalStateV1::Applying | OperationJournalStateV1::Uncertain
        ) {
            return Err(fence_lost(
                &claim.key,
                "mark_uncertain met a record outside its apply",
            ));
        }
        let _written = write_row(
            &transaction,
            &claim.key,
            &claim.body_sha256,
            OperationJournalStateV1::Uncertain,
            &claim.owner,
            claim.lease_deadline_ms,
            claim.fence_token,
            &claim.epoch_commitment,
            None,
            None,
            None,
            None,
            None,
        )?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit mark_uncertain", &path, &error))?;
        drop(connection);
        Ok(())
    }

    fn recover(&self, key: &IdempotencyKeyV1) -> Result<OperationInspectV1, CoreError> {
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin transaction", &path, &error))?;
        let Some(stored) = read_row(&transaction, &path, key)? else {
            return Ok(OperationInspectV1::Absent);
        };
        let now = self.clock.now_unix_ms();
        let expired = stored.lease_deadline_ms <= now;
        match stored.state {
            OperationJournalStateV1::Claimed | OperationJournalStateV1::Applying if !expired => {
                let outcome = inspect_stored(key, &stored)?;
                transaction
                    .commit()
                    .map_err(|error| engine_error("commit recover", &path, &error))?;
                drop(connection);
                return Ok(outcome);
            }
            OperationJournalStateV1::Uncertain if !expired => {
                // An uncertain record is resolvable only by a retry; the
                // worker that marked it uncertain already gave the row
                // up, so recovery aborts it rather than leaving
                // indefinite intent.
            }
            OperationJournalStateV1::Committed
            | OperationJournalStateV1::Refused
            | OperationJournalStateV1::Aborted => {
                let outcome = inspect_stored(key, &stored)?;
                transaction
                    .commit()
                    .map_err(|error| engine_error("commit recover", &path, &error))?;
                drop(connection);
                return Ok(outcome);
            }
            OperationJournalStateV1::Prepared
            | OperationJournalStateV1::Claimed
            | OperationJournalStateV1::Applying
            | OperationJournalStateV1::Uncertain => {}
        }
        // Expired or uncertain: terminal abort with its ledger event, so
        // the next claim of this key starts from a resolved record.
        let payload = payload_digest_of_parts(&[&stored.fence_token.to_le_bytes()]);
        let sequence = append_sequence_event(
            &transaction,
            SequenceEventKindV1::OperationAborted,
            &key.identity_digest(),
            &payload,
        )?;
        let _written = write_row(
            &transaction,
            key,
            &stored.body_sha256,
            OperationJournalStateV1::Aborted,
            &stored.owner,
            stored.lease_deadline_ms,
            stored.fence_token,
            &stored.input_commitment,
            None,
            None,
            Some(sequence_u64(sequence)?),
            None,
            None,
        )?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit recover-abort", &path, &error))?;
        drop(connection);
        Ok(OperationInspectV1::Absent)
    }

    fn generations_for_pair(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<ManifestGeneration>, CoreError> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT generation FROM idempotency_v2
                 WHERE repo_id = ?1 AND revision_id = ?2
                 ORDER BY generation ASC",
            )
            .map_err(|error| engine_error("prepare generations for pair", &self.path, &error))?;
        let rows = statement
            .query_map(params![repo_id.as_str(), revision_id.as_str()], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|error| engine_error("list generations for pair", &self.path, &error))?;
        let mut generations = Vec::new();
        for row in rows {
            let generation =
                row.map_err(|error| engine_error("read generation row", &self.path, &error))?;
            let generation = u64::try_from(generation).map_err(|error| CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: format!("catalog: generation column holds {generation}: {error}"),
            })?;
            generations.push(ManifestGeneration::new(generation));
        }
        drop(statement);
        drop(connection);
        Ok(generations)
    }

    fn forget_generation(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<u64, CoreError> {
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin transaction", &path, &error))?;
        let keys: Vec<IdempotencyKeyV1> = {
            let mut statement = transaction
                .prepare(
                    "SELECT kind, batch_digest FROM idempotency_v2
                     WHERE repo_id = ?1 AND revision_id = ?2 AND generation = ?3",
                )
                .map_err(|error| engine_error("prepare forget generation", &path, &error))?;
            let rows = statement
                .query_map(
                    params![
                        repo_id.as_str(),
                        revision_id.as_str(),
                        generation_i64(generation)?
                    ],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .map_err(|error| engine_error("list forget generation", &path, &error))?;
            let mut keys = Vec::new();
            for row in rows {
                let (kind, batch_digest) =
                    row.map_err(|error| engine_error("read forget generation row", &path, &error))?;
                keys.push(IdempotencyKeyV1 {
                    kind: quanta_index_core::ingest_kind_from_code_str(&kind)?,
                    repo_id: repo_id.clone(),
                    revision_id: revision_id.clone(),
                    generation,
                    batch_digest,
                });
            }
            keys
        };
        if keys.is_empty() {
            transaction
                .commit()
                .map_err(|error| engine_error("commit forget generation", &path, &error))?;
            drop(connection);
            return Ok(0);
        }
        let removed = transaction
            .execute(
                "DELETE FROM idempotency_v2
                 WHERE repo_id = ?1 AND revision_id = ?2 AND generation = ?3",
                params![
                    repo_id.as_str(),
                    revision_id.as_str(),
                    generation_i64(generation)?
                ],
            )
            .map_err(|error| engine_error("forget generation", &path, &error))?;
        let removed = u64::try_from(removed).map_err(|error| {
            CoreError::Storage(format!("catalog: removed-row count overflow: {error}"))
        })?;
        let listed = u64::try_from(keys.len()).map_err(|error| {
            CoreError::Storage(format!("catalog: listed-row count overflow: {error}"))
        })?;
        if removed != listed {
            return Err(CoreError::Storage(format!(
                "catalog: forget generation deleted {removed} rows but listed {listed}"
            )));
        }
        // Each dropped record gets a GC-only invalidation. Its payload
        // binds the latest terminal commitment, or the explicit no-terminal
        // marker for an unfinished row, without retaining a second row owner.
        for key in &keys {
            let _invalidated = append_operation_invalidation(
                &transaction,
                SequenceEventKindV1::OperationGcInvalidation,
                &key.identity_digest(),
            )?;
        }
        transaction
            .commit()
            .map_err(|error| engine_error("commit forget generation", &path, &error))?;
        drop(connection);
        // `removed` is already a u64 row count; no conversion is needed.
        Ok(removed)
    }
}

/// Write a fresh immutable prepared row: the `prepare` half of the
/// protocol. No lease is held yet; the row waits for mutable preflight
/// and the fenced claim.
fn prepare_fresh(
    transaction: &rusqlite::Transaction<'_>,
    path: &Path,
    key: &IdempotencyKeyV1,
    body_sha256: &[u8; 32],
    owner: &str,
    lease_deadline_ms: u64,
    epoch_commitment: &[u8; 32],
) -> Result<PreparedMutationV1, CoreError> {
    let fence = next_fence_token(transaction, path)?;
    let _written = write_row(
        transaction,
        key,
        body_sha256,
        OperationJournalStateV1::Prepared,
        owner,
        lease_deadline_ms,
        fence,
        epoch_commitment,
        None,
        None,
        None,
        None,
        None,
    )?;
    Ok(PreparedMutationV1 {
        key: key.clone(),
        body_sha256: *body_sha256,
        owner: owner.to_string(),
        fence_token: fence,
        lease_deadline_ms,
        epoch_commitment: *epoch_commitment,
    })
}

/// Write a fresh claim over a superseded (Prepared/Aborted/Uncertain)
/// record.
fn claim_fresh(
    transaction: &rusqlite::Transaction<'_>,
    path: &Path,
    key: &IdempotencyKeyV1,
    body_sha256: &[u8; 32],
    owner: &str,
    lease_deadline_ms: u64,
    epoch_commitment: &[u8; 32],
) -> Result<ClaimOutcomeV1, CoreError> {
    let fence = next_fence_token(transaction, path)?;
    let _written = write_row(
        transaction,
        key,
        body_sha256,
        OperationJournalStateV1::Claimed,
        owner,
        lease_deadline_ms,
        fence,
        epoch_commitment,
        None,
        None,
        None,
        None,
        None,
    )?;
    Ok(ClaimOutcomeV1::Claimed(PreparedMutationV1 {
        key: key.clone(),
        body_sha256: *body_sha256,
        owner: owner.to_string(),
        fence_token: fence,
        lease_deadline_ms,
        epoch_commitment: *epoch_commitment,
    }))
}

fn lease_row_digest(scope: &str, owner: &str, fence_token: i64, deadline_ms: i64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(LEASE_ROW_DIGEST_DOMAIN);
    hasher.update(scope.as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(owner.as_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(fence_token.to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(deadline_ms.to_le_bytes());
    hasher.finalize().into()
}

fn read_lease_row(
    connection: &Connection,
    path: &Path,
    scope: &str,
) -> Result<Option<(String, u64, u64)>, CoreError> {
    let row: Option<(String, i64, i64, Vec<u8>)> = connection
        .query_row(
            "SELECT owner, fence_token, deadline_ms, row_sha256
             FROM mutation_lease_v1 WHERE scope = ?1",
            params![scope],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| engine_error("read mutation lease", path, &error))?;
    let Some((owner, fence, deadline, digest)) = row else {
        return Ok(None);
    };
    let digest = blob32("mutation lease row digest", &digest)?;
    if fence <= 0 || deadline < 0 || digest != lease_row_digest(scope, &owner, fence, deadline) {
        return Err(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message: format!("catalog: mutation lease {scope} has a corrupt row"),
        });
    }
    let fence = u64::try_from(fence)
        .map_err(|error| CoreError::Storage(format!("catalog: lease fence conversion: {error}")))?;
    let deadline = u64::try_from(deadline).map_err(|error| {
        CoreError::Storage(format!("catalog: lease deadline conversion: {error}"))
    })?;
    Ok(Some((owner, fence, deadline)))
}

impl MutationCoordinatorPort for SqliteCatalog {
    fn enter(&self, scope: &str, owner: &str, lease_ms: u64) -> Result<MutationLeaseV1, CoreError> {
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin lease transaction", &path, &error))?;
        let now = self.clock.now_unix_ms();
        let deadline = now.saturating_add(lease_ms);
        let existing = read_lease_row(&transaction, &path, scope)?;
        if let Some((lease_owner, _fence, lease_deadline)) = &existing {
            let live = *lease_deadline > now;
            if live && lease_owner != owner {
                return Err(busy("mutation coordinator enter"));
            }
        }
        let fence = next_fence_token(&transaction, &path)?;
        let fence_i64 = u64_i64("lease fence", fence)?;
        let deadline_i64 = u64_i64("lease deadline", deadline)?;
        let _written = transaction
            .execute(
                "INSERT INTO mutation_lease_v1 (scope, owner, fence_token, deadline_ms, row_sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (scope) DO UPDATE SET
                     owner = excluded.owner, fence_token = excluded.fence_token,
                     deadline_ms = excluded.deadline_ms, row_sha256 = excluded.row_sha256",
                params![
                    scope,
                    owner,
                    fence_i64,
                    deadline_i64,
                    lease_row_digest(scope, owner, fence_i64, deadline_i64).as_slice(),
                ],
            )
            .map_err(|error| engine_error("write mutation lease", &path, &error))?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit mutation lease", &path, &error))?;
        drop(connection);
        Ok(MutationLeaseV1 {
            scope: scope.to_string(),
            owner: owner.to_string(),
            fence_token: fence,
            deadline_ms: deadline,
        })
    }

    fn release(&self, lease: &MutationLeaseV1) -> Result<(), CoreError> {
        let mut connection = self.lock()?;
        let path = self.path.clone();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin lease release", &path, &error))?;
        let held = read_lease_row(&transaction, &path, &lease.scope)?;
        if !matches!(
            held,
            Some((ref owner, fence, deadline))
                if owner == &lease.owner
                    && fence == lease.fence_token
                    && deadline == lease.deadline_ms
        ) {
            return Err(CoreError::Typed {
                code: OPERATION_FENCE_LOST_CODE,
                message: format!(
                    "catalog: mutation lease {} held by {} is no longer this worker's",
                    lease.scope, lease.owner
                ),
            });
        }
        let removed = transaction
            .execute(
                "DELETE FROM mutation_lease_v1
                 WHERE scope = ?1 AND owner = ?2 AND fence_token = ?3",
                params![
                    lease.scope,
                    lease.owner,
                    u64_i64("lease fence", lease.fence_token)?,
                ],
            )
            .map_err(|error| engine_error("release mutation lease", &path, &error))?;
        if removed != 1 {
            return Err(CoreError::Typed {
                code: OPERATION_FENCE_LOST_CODE,
                message: format!(
                    "catalog: mutation lease {} held by {} is no longer this worker's",
                    lease.scope, lease.owner
                ),
            });
        }
        transaction
            .commit()
            .map_err(|error| engine_error("commit lease release", &path, &error))?;
        drop(connection);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::Duration;

    use quanta_index_contract::{
        BATCH_PUBLISH_RECEIPT_FORMAT_VERSION, BatchPublishReceipt, IngestOperationKindV1,
        ManifestGeneration, RepoId, RevisionId, SearchPlaneErrorCodeV2,
    };
    use quanta_index_core::{
        ClaimOutcomeV1, CoreError, IdempotencyCatalogPort, IdempotencyKeyV1,
        MutationCoordinatorPort, OPERATION_FENCE_LOST_CODE,
    };
    use rusqlite::params;

    use super::{
        decode_versioned_receipt, encode_versioned_receipt, fence_allocator_digest,
        payload_digest_of_parts, read_row, receipt_digest, row_digest,
    };
    use crate::connection::SqliteCatalog;
    use crate::sequence::{SequenceEventKindV1, event_commitment, event_row_digest};

    #[test]
    fn foreign_receipt_tag_refuses_decode_typed() -> Result<(), Box<dyn Error>> {
        let receipt =
            BatchPublishReceipt::empty_for(ManifestGeneration::new(1), None, "digest".to_string());
        let mut bytes = encode_versioned_receipt(&receipt)?;
        let tag = bytes.get_mut(0).ok_or("versioned receipt has no tag")?;
        *tag ^= 0x40;
        if !matches!(
            decode_versioned_receipt(&bytes),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message,
            }) if message.contains("format version")
        ) {
            return Err("foreign receipt version must refuse before payload decode".into());
        }
        Ok(())
    }

    #[test]
    fn mutation_lease_fences_advance_across_release_and_reopen() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        let first = catalog.enter("scope", "owner", 100)?;
        catalog.release(&first)?;
        let second = catalog.enter("scope", "owner", 100)?;
        if second.fence_token <= first.fence_token {
            return Err("released lease reused its fence".into());
        }
        if !matches!(
            catalog.release(&first),
            Err(CoreError::Typed { code, .. }) if code == OPERATION_FENCE_LOST_CODE
        ) {
            return Err("old lease released a new same-owner lease".into());
        }
        drop(catalog);
        let reopened = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        let third = reopened.enter("scope", "owner", 100)?;
        if third.fence_token <= second.fence_token {
            return Err("reopened allocator regressed its fence".into());
        }
        if !matches!(
            reopened.release(&second),
            Err(CoreError::Typed { code, .. }) if code == OPERATION_FENCE_LOST_CODE
        ) {
            return Err("pre-reopen lease released a new same-owner lease".into());
        }
        Ok(())
    }

    #[test]
    fn corrupted_lease_deadline_refuses_takeover_and_startup_cleanup() -> Result<(), Box<dyn Error>>
    {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        let lease = catalog.enter("scope", "owner", 60_000)?;
        {
            let connection = catalog.lock()?;
            let changed = connection.execute(
                "UPDATE mutation_lease_v1 SET deadline_ms = 0 WHERE scope = 'scope'",
                [],
            )?;
            if changed != 1 {
                return Err("fixture must damage one lease deadline".into());
            }
            drop(connection);
        }
        if !matches!(
            catalog.enter("scope", "other", 60_000),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                ..
            })
        ) {
            return Err("damaged live lease must not be stolen".into());
        }
        if !matches!(
            catalog.release(&lease),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                ..
            })
        ) {
            return Err("damaged lease must not be released as valid".into());
        }
        let catalog_path = catalog.path().to_path_buf();
        drop(catalog);
        if !matches!(
            SqliteCatalog::open(root.path(), Duration::from_millis(100)),
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                ..
            })
        ) {
            return Err("startup must not erase a damaged lease".into());
        }
        let connection = rusqlite::Connection::open(catalog_path)?;
        let retained: i64 = connection.query_row(
            "SELECT COUNT(*) FROM mutation_lease_v1 WHERE scope = 'scope'",
            [],
            |row| row.get(0),
        )?;
        if retained != 1 {
            return Err("failed startup erased the damaged lease".into());
        }
        Ok(())
    }

    #[test]
    fn missing_or_regressed_fence_allocator_refuses_before_recovery() -> Result<(), Box<dyn Error>>
    {
        for remove_table in [false, true] {
            let root = tempfile::tempdir()?;
            let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
            let key = IdempotencyKeyV1 {
                kind: IngestOperationKindV1::History,
                repo_id: RepoId::new("repo")?,
                revision_id: RevisionId::new("revision")?,
                generation: ManifestGeneration::new(1),
                batch_digest: "digest".to_string(),
            };
            let body = [1_u8; 32];
            if !matches!(
                catalog.claim_prepared(&key, &body, "owner", 0, &body)?,
                ClaimOutcomeV1::Claimed(_)
            ) {
                return Err("fixture expected a fresh claim".into());
            }
            {
                let connection = catalog.lock()?;
                if remove_table {
                    connection.execute_batch("DROP TABLE catalog_fence_v1")?;
                } else {
                    let changed = connection.execute(
                        "UPDATE catalog_fence_v1 SET last_fence = 0, row_sha256 = ?1 WHERE id = 1",
                        params![fence_allocator_digest(0).as_slice()],
                    )?;
                    if changed != 1 {
                        return Err("fixture must regress one fence allocator row".into());
                    }
                }
                drop(connection);
            }
            let catalog_path = catalog.path().to_path_buf();
            drop(catalog);
            let opened = SqliteCatalog::open(root.path(), Duration::from_millis(100));
            let refused_as_expected = match opened {
                Err(CoreError::Storage(message)) if remove_table => {
                    message.contains("no durable fence allocator row")
                }
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    message,
                }) if !remove_table => message.contains("high-water"),
                _ => false,
            };
            if !refused_as_expected {
                return Err(format!(
                    "missing/regressed fence allocator must refuse (removed={remove_table})"
                )
                .into());
            }
            let connection = rusqlite::Connection::open(catalog_path)?;
            if remove_table {
                let recreated: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'table' AND name = 'catalog_fence_v1'",
                    [],
                    |row| row.get(0),
                )?;
                if recreated != 0 {
                    return Err("failed open must roll back allocator schema creation".into());
                }
            }
            let events: i64 = connection.query_row(
                "SELECT COUNT(*) FROM catalog_sequence_event_v2",
                [],
                |row| row.get(0),
            )?;
            if events != 0 {
                return Err("fence refusal must precede crash-recovery abort".into());
            }
        }
        Ok(())
    }

    #[test]
    fn exhausted_fence_allocator_refuses_without_operation_mutation() -> Result<(), Box<dyn Error>>
    {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        {
            let connection = catalog.lock()?;
            let changed = connection.execute(
                "UPDATE catalog_fence_v1 SET last_fence = ?1, row_sha256 = ?2 WHERE id = 1",
                params![i64::MAX, fence_allocator_digest(i64::MAX).as_slice()],
            )?;
            if changed != 1 {
                return Err("fixture must exhaust one fence allocator".into());
            }
            drop(connection);
        }
        let key = IdempotencyKeyV1 {
            kind: IngestOperationKindV1::History,
            repo_id: RepoId::new("repo")?,
            revision_id: RevisionId::new("revision")?,
            generation: ManifestGeneration::new(1),
            batch_digest: "digest".to_string(),
        };
        let body = [1_u8; 32];
        if !matches!(
            catalog.claim_prepared(&key, &body, "owner", 0, &body),
            Err(CoreError::Storage(message)) if message.contains("fence allocator is exhausted")
        ) {
            return Err("exhausted fence allocator must refuse a fresh claim".into());
        }
        let connection = catalog.lock()?;
        let journal_rows: i64 =
            connection.query_row("SELECT COUNT(*) FROM idempotency_v2", [], |row| row.get(0))?;
        if journal_rows != 0 {
            return Err("fence exhaustion inserted a journal row".into());
        }
        drop(connection);
        Ok(())
    }

    #[test]
    fn fully_redigested_foreign_receipt_refuses_replay_without_mutation()
    -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        let key = IdempotencyKeyV1 {
            kind: IngestOperationKindV1::History,
            repo_id: RepoId::new("repo")?,
            revision_id: RevisionId::new("revision")?,
            generation: ManifestGeneration::new(1),
            batch_digest: "digest".to_string(),
        };
        let body = [1_u8; 32];
        let claim =
            match catalog.claim_prepared(&key, &body, "owner", i64::MAX.unsigned_abs(), &body)? {
                ClaimOutcomeV1::Claimed(claim) => claim,
                ClaimOutcomeV1::Replay { .. } | ClaimOutcomeV1::ReplayRepoMap { .. } => {
                    return Err("fixture expected a fresh claim".into());
                }
            };
        catalog.mark_applying(&claim)?;
        let mut receipt =
            BatchPublishReceipt::empty_for(ManifestGeneration::new(1), None, "digest".to_string());
        receipt.accept_replace_scope();
        let sequence = i64::try_from(catalog.commit(&claim, &receipt)?)?;
        {
            let mut connection = catalog.lock()?;
            let transaction = connection.transaction()?;
            let stored = read_row(&transaction, catalog.path(), &key)?
                .ok_or("committed fixture row disappeared")?;
            let mut foreign = stored
                .receipt_cbor
                .clone()
                .ok_or("committed fixture has no receipt")?;
            let future_version = BATCH_PUBLISH_RECEIPT_FORMAT_VERSION
                .checked_add(1)
                .ok_or("receipt version cannot advance")?;
            foreign
                .get_mut(..4)
                .ok_or("versioned receipt has no tag")?
                .copy_from_slice(&future_version.to_le_bytes());
            let foreign_digest = receipt_digest(&foreign);
            let journal_digest = row_digest(
                &key,
                &stored.body_sha256,
                stored.state,
                &stored.owner,
                stored.lease_deadline_ms,
                stored.fence_token,
                &stored.input_commitment,
                Some(&foreign_digest),
                stored.durable_sequence,
                stored.refusal_code.as_deref(),
                stored.refusal_message.as_deref(),
            );
            let payload = payload_digest_of_parts(&[&foreign]);
            let identity = key.identity_digest();
            let kind = SequenceEventKindV1::OperationCommitted;
            let commitment = event_commitment(sequence, kind, &identity, &payload);
            let event_digest = event_row_digest(sequence, kind, &identity, &payload, &commitment);
            let updated_row = transaction.execute(
                "UPDATE idempotency_v2
                 SET receipt_cbor = ?1, receipt_digest = ?2, row_sha256 = ?3
                 WHERE kind = ?4 AND repo_id = ?5 AND revision_id = ?6
                   AND generation = 1 AND batch_digest = ?7",
                params![
                    &foreign,
                    foreign_digest.as_slice(),
                    journal_digest.as_slice(),
                    key.kind.as_code_str(),
                    key.repo_id.as_str(),
                    key.revision_id.as_str(),
                    key.batch_digest.as_str(),
                ],
            )?;
            let updated_event = transaction.execute(
                "UPDATE catalog_sequence_event_v2
                 SET payload_digest = ?1, event_commitment = ?2, row_sha256 = ?3
                 WHERE sequence = ?4",
                params![
                    payload.as_slice(),
                    commitment.as_slice(),
                    event_digest.as_slice(),
                    sequence,
                ],
            )?;
            if updated_row != 1 || updated_event != 1 {
                return Err("fixture must rewrite exactly one row and event".into());
            }
            transaction.commit()?;
            drop(connection);
        }
        drop(catalog);
        let reopened = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        let before = reopened.sequence_allocator()?;
        let refused = reopened.claim_prepared(&key, &body, "retry", i64::MAX.unsigned_abs(), &body);
        if !matches!(
            refused,
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message,
            }) if message.contains("format version")
        ) {
            return Err("coherently redigested foreign receipt must refuse by version".into());
        }
        if reopened.sequence_allocator()? != before {
            return Err("foreign-version replay allocated a ledger event".into());
        }
        Ok(())
    }
}
