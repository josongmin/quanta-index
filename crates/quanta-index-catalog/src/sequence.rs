//! The state-root-global sequence authority and its generic event ledger
//! (SEP-21-002, P02B).
//!
//! Schema:
//!
//! ```text
//! catalog_sequence_v2(id INTEGER PK CHECK(id=1), next INTEGER NULL,
//!                     exhausted INTEGER NOT NULL CHECK(exhausted IN (0,1)),
//!                     row_sha256 BLOB CHECK(length(row_sha256)=32))
//! catalog_sequence_event_v2(sequence INTEGER UNIQUE
//!                     CHECK(sequence BETWEEN 1 AND 9223372036854775807),
//!                     kind INTEGER CHECK(kind IN (1..=12)),
//!                     identity_digest BLOB CHECK(length=32),
//!                     payload_digest BLOB CHECK(length=32),
//!                     event_commitment BLOB CHECK(length=32),
//!                     row_sha256 BLOB CHECK(length=32))
//! operation_gc_floor_v1(identity_digest BLOB PRIMARY KEY,
//!                     invalidation_sequence INTEGER UNIQUE,
//!                     target_commitment BLOB CHECK(length=32),
//!                     row_sha256 BLOB CHECK(length=32))
//! ```
//!
//! Only the current allocator, event ledger and GC floor domain are read.
//! Their self-digests are mandatory; no legacy allocator is accepted.
//!
//! The only allocation path is [`append_sequence_event`]: it reads the
//! allocator row (verifying its self-digest), refuses
//! `SEQUENCE_EXHAUSTED` before any mutation once `exhausted = 1`, issues
//! `next` (committing `next = NULL, exhausted = 1` together with the
//! `i64::MAX` event), and inserts the ledger event — all inside the
//! caller's `BEGIN IMMEDIATE` transaction, which also commits the domain
//! terminal row. A rollback leaves allocator, event and domain row all
//! absent (zero rows).
//!
//! Open/restore reconciliation reads the generic ledger only: empty →
//! `(next = 1, exhausted = 0)`; `max < i64::MAX` → `(next = max + 1,
//! exhausted = 0)`; `max == i64::MAX` → `(next = NULL, exhausted = 1)`.
//! A separate integrity pass rejects gaps in the append-only ledger and
//! verifies each event's domain pair; domain maxima are never used to repair
//! the allocator.

use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use sha2::{Digest, Sha256};

use quanta_index_core::CoreError;

use crate::connection::{SqliteCatalog, blob32, engine_error};

const ALLOCATOR_DOMAIN: &[u8] = b"quanta-index:catalog:sequence-row:v1\0";
const EVENT_COMMITMENT_DOMAIN: &[u8] = b"quanta-index:catalog:sequence-event-commitment:v1\0";
const EVENT_ROW_DOMAIN: &[u8] = b"quanta-index:catalog:sequence-event-row:v1\0";
const GC_FLOOR_ROW_DOMAIN: &[u8] = b"quanta-index:catalog:operation-gc-floor-row:v1\0";
const FIELD_SEPARATOR: &[u8] = b"\x1f";
pub(crate) const NO_TERMINAL_TARGET: [u8; 32] = [0_u8; 32];

/// The tables, created at open.
pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS catalog_sequence_v2 (
             id INTEGER PRIMARY KEY CHECK (id = 1),
             next INTEGER,
             exhausted INTEGER NOT NULL CHECK (exhausted IN (0, 1)),
             row_sha256 BLOB NOT NULL CHECK (length(row_sha256) = 32)
         );
         CREATE TABLE IF NOT EXISTS catalog_sequence_event_v2 (
             sequence INTEGER PRIMARY KEY
                 CHECK (sequence BETWEEN 1 AND 9223372036854775807),
             kind INTEGER NOT NULL CHECK (kind IN (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12)),
             identity_digest BLOB NOT NULL CHECK (length(identity_digest) = 32),
             payload_digest BLOB NOT NULL CHECK (length(payload_digest) = 32),
             event_commitment BLOB NOT NULL CHECK (length(event_commitment) = 32),
             row_sha256 BLOB NOT NULL CHECK (length(row_sha256) = 32)
         ) WITHOUT ROWID;
         CREATE INDEX IF NOT EXISTS catalog_sequence_event_v2_identity_sequence
             ON catalog_sequence_event_v2 (kind, identity_digest, sequence);
         CREATE TABLE IF NOT EXISTS operation_gc_floor_v1 (
             identity_digest BLOB PRIMARY KEY CHECK (length(identity_digest) = 32),
             invalidation_sequence INTEGER NOT NULL UNIQUE CHECK (invalidation_sequence > 0),
             target_commitment BLOB NOT NULL CHECK (length(target_commitment) = 32),
             row_sha256 BLOB NOT NULL CHECK (length(row_sha256) = 32)
         ) WITHOUT ROWID;";

/// The closed set of event kinds the generic ledger admits (SEP-21-002).
///
/// The numeric codes are the DB `CHECK` set; the parity between this enum
/// and the schema string is asserted at open
/// ([`SequenceEventKindV1::check_set_sql`] vs the schema text) and by
/// test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SequenceEventKindV1 {
    OperationCommitted = 1,
    OperationRefused = 2,
    OperationAborted = 3,
    CandidateSeal = 4,
    Activation = 5,
    Rollback = 6,
    OperationInvalidation = 7,
    QuarantineRecord = 8,
    QuarantineDiscard = 9,
    RepoMapInvalidation = 10,
    RepoMapCandidateQuarantine = 11,
    OperationGcInvalidation = 12,
}

impl SequenceEventKindV1 {
    pub(crate) const ALL: [Self; 12] = [
        Self::OperationCommitted,
        Self::OperationRefused,
        Self::OperationAborted,
        Self::CandidateSeal,
        Self::Activation,
        Self::Rollback,
        Self::OperationInvalidation,
        Self::QuarantineRecord,
        Self::QuarantineDiscard,
        Self::RepoMapInvalidation,
        Self::RepoMapCandidateQuarantine,
        Self::OperationGcInvalidation,
    ];

    #[must_use]
    /// The enum's discriminants are the closed 1..=12 `CHECK` set; the
    /// explicit match keeps the cast side-effect-free (no `as`).
    pub(crate) fn as_code(self) -> i64 {
        match self {
            Self::OperationCommitted => 1,
            Self::OperationRefused => 2,
            Self::OperationAborted => 3,
            Self::CandidateSeal => 4,
            Self::Activation => 5,
            Self::Rollback => 6,
            Self::OperationInvalidation => 7,
            Self::QuarantineRecord => 8,
            Self::QuarantineDiscard => 9,
            Self::RepoMapInvalidation => 10,
            Self::RepoMapCandidateQuarantine => 11,
            Self::OperationGcInvalidation => 12,
        }
    }

    /// The `CHECK (kind IN (...))` list the schema must carry, derived
    /// from the enum so the two cannot drift.
    #[must_use]
    pub(crate) fn check_set_sql() -> String {
        Self::ALL
            .iter()
            .map(|kind| kind.as_code().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub(crate) fn from_code(code: i64) -> Result<Self, CoreError> {
        match code {
            1 => Ok(Self::OperationCommitted),
            2 => Ok(Self::OperationRefused),
            3 => Ok(Self::OperationAborted),
            4 => Ok(Self::CandidateSeal),
            5 => Ok(Self::Activation),
            6 => Ok(Self::Rollback),
            7 => Ok(Self::OperationInvalidation),
            8 => Ok(Self::QuarantineRecord),
            9 => Ok(Self::QuarantineDiscard),
            10 => Ok(Self::RepoMapInvalidation),
            11 => Ok(Self::RepoMapCandidateQuarantine),
            12 => Ok(Self::OperationGcInvalidation),
            other => Err(corrupt(&format!("event kind code {other} is not known"))),
        }
    }
}

/// Refuse incompatible sequence schemas before recovery.
///
/// The filename and table name are not migration selectors; only this
/// build's exact installed allocator, event, index and floor definitions
/// are current.
pub(crate) fn verify_installed_schema(
    connection: &Connection,
    path: &std::path::Path,
) -> Result<(), CoreError> {
    let expected = format!("kind IN ({})", SequenceEventKindV1::check_set_sql());
    if !SCHEMA.contains(&expected) {
        return Err(corrupt("sequence schema and event-kind enum disagree"));
    }
    crate::connection::verify_installed_schema_objects(
        connection,
        path,
        SCHEMA,
        &[
            ("table", "catalog_sequence_v2", "sequence allocator"),
            ("table", "catalog_sequence_event_v2", "event-kind"),
            (
                "index",
                "catalog_sequence_event_v2_identity_sequence",
                "sequence identity index",
            ),
            ("table", "operation_gc_floor_v1", "operation-GC-floor"),
        ],
    )
}

fn corrupt(message: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!("catalog: {message}"),
    }
}

fn exhausted_error() -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::SequenceExhausted,
        message:
            "catalog: the global sequence is exhausted; no further durable event can be issued"
                .to_string(),
    }
}

/// The allocator row's self-digest over `(next, exhausted)` under
/// SEP-21-002 preimage separation.
fn allocator_digest(next: Option<i64>, exhausted: bool) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ALLOCATOR_DOMAIN);
    match next {
        Some(next) => {
            hasher.update([1_u8]);
            hasher.update(next.to_le_bytes());
        }
        None => hasher.update([0_u8]),
    }
    hasher.update(FIELD_SEPARATOR);
    hasher.update([u8::from(exhausted)]);
    hasher.finalize().into()
}

/// The event's commitment: SEP-21-002 preimage over the allocation,
/// kind, and two content digests.
///
/// Infallible by construction: all inputs are fixed-width bytes and the
/// hasher performs no encoding.
pub(crate) fn event_commitment(
    sequence: i64,
    kind: SequenceEventKindV1,
    identity: &[u8; 32],
    payload: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(EVENT_COMMITMENT_DOMAIN);
    hasher.update(sequence.to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(kind.as_code().to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(identity);
    hasher.update(FIELD_SEPARATOR);
    hasher.update(payload);
    hasher.finalize().into()
}

/// The event row's digest over every other column. Infallible by construction:
/// all inputs are fixed-width bytes and the hasher performs no encoding.
pub(crate) fn event_row_digest(
    sequence: i64,
    kind: SequenceEventKindV1,
    identity: &[u8; 32],
    payload: &[u8; 32],
    commitment: &[u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(EVENT_ROW_DOMAIN);
    hasher.update(sequence.to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(kind.as_code().to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(identity);
    hasher.update(FIELD_SEPARATOR);
    hasher.update(payload);
    hasher.update(FIELD_SEPARATOR);
    hasher.update(commitment);
    hasher.finalize().into()
}

fn gc_floor_row_digest(sequence: i64, identity: &[u8; 32], target: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(GC_FLOOR_ROW_DOMAIN);
    hasher.update(sequence.to_le_bytes());
    hasher.update(FIELD_SEPARATOR);
    hasher.update(identity);
    hasher.update(FIELD_SEPARATOR);
    hasher.update(target);
    hasher.finalize().into()
}

fn read_gc_floor(
    connection: &Connection,
    path: &std::path::Path,
    identity: &[u8; 32],
) -> Result<Option<(i64, [u8; 32])>, CoreError> {
    let row: Option<(i64, Vec<u8>, Vec<u8>)> = connection
        .query_row(
            "SELECT invalidation_sequence, target_commitment, row_sha256
             FROM operation_gc_floor_v1 WHERE identity_digest = ?1",
            params![identity.as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| engine_error("read operation GC floor", path, &error))?;
    let Some((sequence, target, digest)) = row else {
        return Ok(None);
    };
    let target = blob32("operation GC target", &target)?;
    let digest = blob32("operation GC row digest", &digest)?;
    if digest != gc_floor_row_digest(sequence, identity, &target) {
        return Err(corrupt("operation GC floor row has a wrong digest"));
    }
    verify_event_reference(
        connection,
        path,
        SequenceEventKindV1::OperationGcInvalidation,
        sequence,
        identity,
        &target,
    )?;
    Ok(Some((sequence, target)))
}

/// Check both directions of the durable replay-floor relation before recovery.
pub(crate) fn verify_gc_floor_domain_integrity(
    connection: &Connection,
    path: &std::path::Path,
) -> Result<(), CoreError> {
    let mut statement = connection
        .prepare("SELECT identity_digest FROM operation_gc_floor_v1")
        .map_err(|error| engine_error("prepare operation GC floor scan", path, &error))?;
    let identities = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .map_err(|error| engine_error("scan operation GC floors", path, &error))?;
    for identity in identities {
        let identity =
            identity.map_err(|error| engine_error("read GC floor identity", path, &error))?;
        let identity = blob32("operation GC identity", &identity)?;
        if read_gc_floor(connection, path, &identity)?.is_none() {
            return Err(corrupt(
                "operation GC floor disappeared during integrity scan",
            ));
        }
    }
    let missing_floor: Option<i64> = connection
        .query_row(
            "SELECT e.sequence FROM catalog_sequence_event_v2 AS e
             WHERE e.kind = ?1 AND NOT EXISTS (
                 SELECT 1 FROM operation_gc_floor_v1 AS f
                 WHERE f.invalidation_sequence = e.sequence
             ) LIMIT 1",
            params![SequenceEventKindV1::OperationGcInvalidation.as_code()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| engine_error("find GC event without floor", path, &error))?;
    if let Some(sequence) = missing_floor {
        return Err(corrupt(&format!(
            "operation GC event {sequence} has no durable floor row"
        )));
    }
    Ok(())
}

struct AllocatorRow {
    next: Option<i64>,
    exhausted: bool,
}

fn read_allocator(
    connection: &Connection,
    path: &std::path::Path,
) -> Result<AllocatorRow, CoreError> {
    let row = connection
        .query_row(
            "SELECT next, exhausted, row_sha256 FROM catalog_sequence_v2 WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .map_err(|error| engine_error("read sequence allocator", path, &error))?;
    let (next, exhausted, digest) = row;
    let exhausted = match exhausted {
        0 => false,
        1 => true,
        other => {
            return Err(corrupt(&format!(
                "exhausted flag is {other}, expected 0 or 1"
            )));
        }
    };
    let stored = blob32("allocator row digest", &digest)?;
    if allocator_digest(next, exhausted) != stored {
        return Err(corrupt(
            "sequence allocator row does not match its own digest",
        ));
    }
    if exhausted && next.is_some() {
        return Err(corrupt(
            "sequence allocator is exhausted but still names a next value",
        ));
    }
    if !exhausted && next.is_none() {
        return Err(corrupt(
            "sequence allocator is not exhausted but names no next value",
        ));
    }
    Ok(AllocatorRow { next, exhausted })
}

/// The only allocation path (crate-private by design).
///
/// Verify the allocator, refuse `SEQUENCE_EXHAUSTED` before any mutation,
/// issue the next sequence and append its generic ledger event — inside
/// the caller's `BEGIN IMMEDIATE` transaction, which must also commit the
/// domain terminal row in the same commit.
pub(crate) fn append_sequence_event(
    transaction: &Transaction<'_>,
    kind: SequenceEventKindV1,
    identity_digest: &[u8; 32],
    payload_digest: &[u8; 32],
) -> Result<i64, CoreError> {
    let path = std::path::Path::new(":catalog:");
    let allocator = read_allocator(transaction, path)?;
    if allocator.exhausted {
        return Err(exhausted_error());
    }
    let sequence = allocator
        .next
        .ok_or_else(|| corrupt("allocator names no next value"))?;
    let is_max = sequence == i64::MAX;
    // Overflow is impossible here: `is_max` excludes `i64::MAX`, so
    // `sequence + 1` always fits; saturating keeps the unreachable arm a
    // no-op rather than a panic path.
    let new_next = if is_max {
        None
    } else {
        Some(sequence.saturating_add(1))
    };
    let new_exhausted = is_max;
    let commitment = event_commitment(sequence, kind, identity_digest, payload_digest);
    let row_digest = event_row_digest(sequence, kind, identity_digest, payload_digest, &commitment);
    let _inserted = transaction
        .execute(
            "INSERT INTO catalog_sequence_event_v2
                 (sequence, kind, identity_digest, payload_digest, event_commitment, row_sha256)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                sequence,
                kind.as_code(),
                identity_digest.as_slice(),
                payload_digest.as_slice(),
                commitment.as_slice(),
                row_digest.as_slice(),
            ],
        )
        .map_err(|error| engine_error("append sequence event", path, &error))?;
    let _updated = transaction
        .execute(
            "UPDATE catalog_sequence_v2
             SET next = ?1, exhausted = ?2, row_sha256 = ?3 WHERE id = 1",
            params![
                new_next,
                i64::from(new_exhausted),
                allocator_digest(new_next, new_exhausted).as_slice(),
            ],
        )
        .map_err(|error| engine_error("advance sequence allocator", path, &error))?;
    Ok(sequence)
}

/// Seed the allocator row on first open (`next = 1`, not exhausted), with
/// its self-digest; an existing row is left to [`reconcile`].
pub(crate) fn seed_allocator(
    connection: &Connection,
    path: &std::path::Path,
) -> Result<(), CoreError> {
    let _seeded = connection
        .execute(
            "INSERT OR IGNORE INTO catalog_sequence_v2 (id, next, exhausted, row_sha256)
             VALUES (1, 1, 0, ?1)",
            params![allocator_digest(Some(1), false).as_slice()],
        )
        .map_err(|error| engine_error("seed sequence allocator", path, &error))?;
    Ok(())
}

/// The open/restore reconciliation: recompute the allocator from the
/// generic ledger only. The caller verifies event↔domain-row pairs before
/// committing the same startup transaction.
///
/// Branches (from the ledger maximum alone — domain maxima are never
/// used): empty ledger → `(1, 0)`; `max < i64::MAX` → `(max + 1, 0)`;
/// `max == i64::MAX` → `(NULL, 1)`. An allocator row naming a `next`
/// beyond `max + 1` describes future events the ledger does not hold:
/// that is corruption (fail closed, [`CatalogRowCorrupt`]), never
/// silently pulled back — reissuing those sequences would break the
/// uniqueness receipts already depend on. An exhausted allocator likewise
/// cannot be reopened unless the ledger holds the final sequence.
pub(crate) fn reconcile(
    transaction: &rusqlite::Transaction<'_>,
    path: &std::path::Path,
) -> Result<(), CoreError> {
    let max: Option<i64> = transaction
        .query_row(
            "SELECT MAX(sequence) FROM catalog_sequence_event_v2",
            [],
            |row| row.get(0),
        )
        .map_err(|error| engine_error("read ledger maximum", path, &error))?;
    let (expected_next, expected_exhausted) = match max {
        None => (Some(1_i64), false),
        Some(i64::MAX) => (None, true),
        // Overflow is impossible in this arm: `i64::MAX` is handled above,
        // so `max + 1` always fits; saturating keeps the unreachable arm a
        // no-op rather than a panic path.
        Some(max) => (Some(max.saturating_add(1)), false),
    };
    let existing = read_allocator(transaction, path)?;
    if existing.exhausted && !expected_exhausted {
        return Err(corrupt(&format!(
            "sequence allocator is exhausted but the generic ledger holds max={max:?}; future events are absent"
        )));
    }
    if let (Some(stored_next), Some(expected)) = (existing.next, expected_next)
        && stored_next > expected
    {
        return Err(corrupt(&format!(
            "sequence allocator names next={stored_next} but the generic ledger holds max={max:?}; \
             future events are absent"
        )));
    }
    if existing.next != expected_next || existing.exhausted != expected_exhausted {
        let _reconciled = transaction
            .execute(
                "UPDATE catalog_sequence_v2 SET next = ?1, exhausted = ?2, row_sha256 = ?3
                 WHERE id = 1",
                params![
                    expected_next,
                    i64::from(expected_exhausted),
                    allocator_digest(expected_next, expected_exhausted).as_slice(),
                ],
            )
            .map_err(|error| engine_error("reconcile sequence allocator", path, &error))?;
    }
    Ok(())
}

pub(crate) type EventRawRow = (i64, i64, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>);

pub(crate) fn event_raw_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventRawRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

pub(crate) fn checked_event_row(
    raw: EventRawRow,
) -> Result<(i64, SequenceEventKindV1, [u8; 32], [u8; 32]), CoreError> {
    let (sequence, kind_code, identity, payload, commitment, row_digest) = raw;
    let kind = SequenceEventKindV1::from_code(kind_code)?;
    let identity = blob32("event identity digest", &identity)?;
    let payload = blob32("event payload digest", &payload)?;
    let commitment = blob32("event commitment", &commitment)?;
    let stored = blob32("event row digest", &row_digest)?;
    if event_commitment(sequence, kind, &identity, &payload) != commitment
        || event_row_digest(sequence, kind, &identity, &payload, &commitment) != stored
    {
        return Err(corrupt(&format!(
            "sequence event {sequence} does not match its commitment or row digest"
        )));
    }
    Ok((sequence, kind, identity, payload))
}

/// Verify a domain row's reverse reference to the exact self-digested event.
/// The forward ledger pass alone cannot detect a forged domain row borrowing
/// an existing sequence from a different event kind.
pub(crate) fn verify_event_reference(
    connection: &Connection,
    path: &std::path::Path,
    expected_kind: SequenceEventKindV1,
    sequence: i64,
    expected_identity: &[u8; 32],
    expected_payload: &[u8; 32],
) -> Result<(), CoreError> {
    let event = connection
        .query_row(
            "SELECT sequence, kind, identity_digest, payload_digest, event_commitment, row_sha256
             FROM catalog_sequence_event_v2 WHERE sequence = ?1",
            params![sequence],
            event_raw_row,
        )
        .optional()
        .map_err(|error| engine_error("read referenced sequence event", path, &error))?
        .ok_or_else(|| corrupt(&format!("domain row references missing event {sequence}")))?;
    let (_, kind, identity, payload) = checked_event_row(event)?;
    if kind != expected_kind || identity != *expected_identity || payload != *expected_payload {
        return Err(corrupt(&format!(
            "domain row disagrees with referenced event {sequence}"
        )));
    }
    Ok(())
}

/// Whether the verified catalog domain owns a generation-GC replay floor.
///
/// The replay-floor check: only a generation GC invalidates a key's
/// replays; a superseded-abort invalidation attributes history without
/// raising the floor.
pub(crate) fn is_invalidated_for_floor(
    connection: &Connection,
    path: &std::path::Path,
    identity_digest: &[u8; 32],
) -> Result<bool, CoreError> {
    if read_gc_floor(connection, path, identity_digest)?.is_some() {
        return Ok(true);
    }
    let orphan_event: Option<i64> = connection
        .query_row(
            "SELECT sequence FROM catalog_sequence_event_v2
             WHERE kind = ?1 AND identity_digest = ?2 LIMIT 1",
            params![
                SequenceEventKindV1::OperationGcInvalidation.as_code(),
                identity_digest.as_slice(),
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| engine_error("check GC floor absence", path, &error))?;
    if let Some(sequence) = orphan_event {
        return Err(corrupt(&format!(
            "operation GC event {sequence} lost its replay-floor row"
        )));
    }
    Ok(false)
}

/// Whether a later invalidation event names the operation identity.
///
/// One invalidation attributes at most the immediately preceding terminal
/// event of the same identity. Earlier invalidations and intervening terminal
/// events cannot excuse a missing domain row.
pub(crate) fn has_later_invalidation(
    connection: &Connection,
    path: &std::path::Path,
    terminal_sequence: i64,
    identity_digest: &[u8; 32],
    target_commitment: &[u8; 32],
) -> Result<bool, CoreError> {
    let next_invalidation: Option<(i64, Vec<u8>)> = connection
        .query_row(
            "SELECT sequence, payload_digest FROM catalog_sequence_event_v2
             WHERE kind IN (?1, ?2) AND identity_digest = ?3 AND sequence > ?4
             ORDER BY sequence ASC LIMIT 1",
            params![
                SequenceEventKindV1::OperationInvalidation.as_code(),
                SequenceEventKindV1::OperationGcInvalidation.as_code(),
                identity_digest.as_slice(),
                terminal_sequence,
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| engine_error("read invalidation", path, &error))?;
    let Some((invalidation_sequence, payload)) = next_invalidation else {
        return Ok(false);
    };
    if blob32("invalidation target", &payload)? != *target_commitment {
        return Ok(false);
    }
    let intervening_terminal: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM catalog_sequence_event_v2
             WHERE kind IN (?1, ?2, ?3) AND identity_digest = ?4
               AND sequence > ?5 AND sequence < ?6 LIMIT 1",
            params![
                SequenceEventKindV1::OperationCommitted.as_code(),
                SequenceEventKindV1::OperationRefused.as_code(),
                SequenceEventKindV1::OperationAborted.as_code(),
                identity_digest.as_slice(),
                terminal_sequence,
                invalidation_sequence,
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| engine_error("read intervening terminal event", path, &error))?;
    Ok(intervening_terminal.is_none())
}

/// Check the invalidation-to-terminal direction of the ledger relation.
///
/// A target is the latest terminal since the prior invalidation of this
/// identity. A nonterminal row has no terminal event and uses the explicit
/// marker. The forward terminal scan checks the other direction.
pub(crate) fn verify_invalidation_target(
    connection: &Connection,
    path: &std::path::Path,
    sequence: i64,
    kind: SequenceEventKindV1,
    identity: &[u8; 32],
    payload: &[u8; 32],
) -> Result<(), CoreError> {
    if kind == SequenceEventKindV1::OperationGcInvalidation {
        match read_gc_floor(connection, path, identity)? {
            Some((floor_sequence, floor_target))
                if floor_sequence == sequence && floor_target == *payload => {}
            _ => {
                return Err(corrupt(&format!(
                    "operation GC event {sequence} has no exact floor row"
                )));
            }
        }
    }
    let (expected, target_sequence) = prior_operation_target(connection, path, sequence, identity)?;
    if *payload != expected {
        return Err(corrupt(&format!(
            "operation invalidation {sequence} does not bind its prior terminal event"
        )));
    }
    if let Some(target_sequence) = target_sequence {
        let retained: Option<i64> = connection
            .query_row(
                "SELECT 1 FROM idempotency_v2 WHERE durable_sequence = ?1 LIMIT 1",
                params![target_sequence],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| engine_error("read invalidated journal row", path, &error))?;
        if retained.is_some() {
            return Err(corrupt(&format!(
                "operation invalidation {sequence} targets a retained terminal row"
            )));
        }
    }
    Ok(())
}

fn prior_operation_target(
    connection: &Connection,
    path: &std::path::Path,
    before_sequence: i64,
    identity: &[u8; 32],
) -> Result<([u8; 32], Option<i64>), CoreError> {
    let previous = connection
        .query_row(
            "SELECT sequence, kind, identity_digest, payload_digest, event_commitment, row_sha256
             FROM catalog_sequence_event_v2
             WHERE identity_digest = ?1 AND sequence < ?2
               AND kind IN (?3, ?4, ?5, ?6, ?7)
             ORDER BY sequence DESC LIMIT 1",
            params![
                identity.as_slice(),
                before_sequence,
                SequenceEventKindV1::OperationCommitted.as_code(),
                SequenceEventKindV1::OperationRefused.as_code(),
                SequenceEventKindV1::OperationAborted.as_code(),
                SequenceEventKindV1::OperationInvalidation.as_code(),
                SequenceEventKindV1::OperationGcInvalidation.as_code(),
            ],
            event_raw_row,
        )
        .optional()
        .map_err(|error| engine_error("read invalidation predecessor", path, &error))?;
    let result = if let Some(previous) = previous {
        let (prior_sequence, prior_kind, prior_identity, prior_payload) =
            checked_event_row(previous)?;
        match prior_kind {
            SequenceEventKindV1::OperationCommitted
            | SequenceEventKindV1::OperationRefused
            | SequenceEventKindV1::OperationAborted => (
                event_commitment(prior_sequence, prior_kind, &prior_identity, &prior_payload),
                Some(prior_sequence),
            ),
            SequenceEventKindV1::OperationInvalidation
            | SequenceEventKindV1::OperationGcInvalidation => (NO_TERMINAL_TARGET, None),
            SequenceEventKindV1::CandidateSeal
            | SequenceEventKindV1::Activation
            | SequenceEventKindV1::Rollback
            | SequenceEventKindV1::QuarantineRecord
            | SequenceEventKindV1::QuarantineDiscard
            | SequenceEventKindV1::RepoMapInvalidation
            | SequenceEventKindV1::RepoMapCandidateQuarantine => {
                return Err(corrupt(
                    "invalidation predecessor has an unrelated event kind",
                ));
            }
        }
    } else {
        (NO_TERMINAL_TARGET, None)
    };
    Ok(result)
}

/// Append the owning invalidation envelope with its target commitment.
///
/// The caller must remove or replace the matching journal row in this same
/// transaction. The generic event ledger remains the sole sequence owner.
pub(crate) fn append_operation_invalidation(
    transaction: &Transaction<'_>,
    kind: SequenceEventKindV1,
    identity: &[u8; 32],
) -> Result<i64, CoreError> {
    if !matches!(
        kind,
        SequenceEventKindV1::OperationInvalidation | SequenceEventKindV1::OperationGcInvalidation
    ) {
        return Err(corrupt(
            "non-invalidation kind reached operation invalidation writer",
        ));
    }
    let path = std::path::Path::new(":catalog:");
    if kind == SequenceEventKindV1::OperationGcInvalidation
        && is_invalidated_for_floor(transaction, path, identity)?
    {
        return Err(corrupt("operation identity already has a GC invalidation"));
    }
    let (payload, _target) = prior_operation_target(transaction, path, i64::MAX, identity)?;
    let sequence = append_sequence_event(transaction, kind, identity, &payload)?;
    if kind == SequenceEventKindV1::OperationGcInvalidation {
        let _inserted = transaction
            .execute(
                "INSERT INTO operation_gc_floor_v1
                     (identity_digest, invalidation_sequence, target_commitment, row_sha256)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    identity.as_slice(),
                    sequence,
                    payload.as_slice(),
                    gc_floor_row_digest(sequence, identity, &payload).as_slice(),
                ],
            )
            .map_err(|error| engine_error("write operation GC floor", path, &error))?;
    }
    Ok(sequence)
}

impl SqliteCatalog {
    /// Expose the allocator snapshot for tests and diagnostics.
    pub fn sequence_allocator(&self) -> Result<(Option<u64>, bool), CoreError> {
        // The connection guard is a temporary of this one statement, so
        // its significant Drop (releasing the catalog lock) happens right
        // after the read instead of at function end.
        let (next, exhausted) = read_allocator(&*self.lock()?, &self.path)
            .map(|allocator| (allocator.next, allocator.exhausted))?;
        Ok((
            next.map(u64::try_from)
                .transpose()
                .map_err(|_error| corrupt("allocator next does not fit u64"))?,
            exhausted,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::Duration;

    use quanta_index_contract::{
        BatchPublishReceipt, IngestOperationKindV1, ManifestGeneration, RepoId, RevisionId,
        SearchPlaneErrorCodeV2,
    };
    use quanta_index_core::{
        ClaimOutcomeV1, IdempotencyCatalogPort, IdempotencyKeyV1, OPERATION_FENCE_LOST_CODE,
    };

    use super::{SequenceEventKindV1, append_sequence_event, event_commitment, event_row_digest};
    use crate::connection::{CATALOG_FILE_NAME, SqliteCatalog, blob32, catalog_dir};

    type TestResult = Result<(), Box<dyn Error>>;

    #[test]
    fn orphan_repomap_invalidation_refuses_reopen() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        {
            let mut connection = catalog.lock()?;
            let transaction = connection.transaction()?;
            let _event = append_sequence_event(
                &transaction,
                SequenceEventKindV1::RepoMapInvalidation,
                &[1_u8; 32],
                &[2_u8; 32],
            )?;
            transaction.commit()?;
            drop(connection);
        }
        drop(catalog);
        let reopened = SqliteCatalog::open(root.path(), Duration::from_millis(100));
        if !matches!(
            reopened,
            Err(quanta_index_core::CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                ..
            })
        ) {
            return Err("orphan RepoMap invalidation must refuse reopen".into());
        }
        Ok(())
    }

    #[test]
    fn orphan_repomap_candidate_quarantine_refuses_reopen() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        {
            let mut connection = catalog.lock()?;
            let transaction = connection.transaction()?;
            let _event = append_sequence_event(
                &transaction,
                SequenceEventKindV1::RepoMapCandidateQuarantine,
                &[1_u8; 32],
                &[2_u8; 32],
            )?;
            transaction.commit()?;
            drop(connection);
        }
        drop(catalog);
        let reopened = SqliteCatalog::open(root.path(), Duration::from_millis(100));
        if !matches!(
            reopened,
            Err(quanta_index_core::CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                ..
            })
        ) {
            return Err("orphan candidate quarantine must refuse reopen".into());
        }
        Ok(())
    }

    #[test]
    fn missing_middle_activation_event_refuses_reopen() -> TestResult {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        let _first = catalog.seal_repomap_candidate(
            "repo",
            "revision",
            1,
            &[1_u8; 32],
            &[2_u8; 32],
            &[3_u8; 32],
            4,
            "{}",
        )?;
        let activation =
            catalog.activate_repomap_candidate("repo", "revision", 1, &[1_u8; 32], None)?;
        let later = catalog.seal_repomap_candidate(
            "repo",
            "revision",
            2,
            &[4_u8; 32],
            &[5_u8; 32],
            &[6_u8; 32],
            7,
            "{}",
        )?;
        if later.terminal_sequence <= activation.terminal_sequence {
            return Err("fixture needs a later ledger event".into());
        }
        {
            let connection = catalog.lock()?;
            let deleted = connection.execute(
                "DELETE FROM catalog_sequence_event_v2 WHERE sequence = ?1",
                rusqlite::params![activation.terminal_sequence],
            )?;
            if deleted != 1 {
                return Err("fixture must delete exactly one activation event".into());
            }
            drop(connection);
        }
        drop(catalog);
        let reopened = SqliteCatalog::open(root.path(), Duration::from_millis(100));
        if !matches!(
            reopened,
            Err(quanta_index_core::CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                ..
            })
        ) {
            return Err("missing middle activation event must refuse reopen".into());
        }
        Ok(())
    }

    #[test]
    fn missing_unpaired_ledger_event_refuses_reopen() -> TestResult {
        for missing_index in [0_usize, 1] {
            let root = tempfile::tempdir()?;
            let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
            let sequences = {
                let mut connection = catalog.lock()?;
                let transaction = connection.transaction()?;
                let mut sequences = [0_i64; 3];
                for sequence in &mut sequences {
                    *sequence = append_sequence_event(
                        &transaction,
                        SequenceEventKindV1::Rollback,
                        &[1_u8; 32],
                        &[2_u8; 32],
                    )?;
                }
                transaction.commit()?;
                drop(connection);
                sequences
            };
            {
                let connection = catalog.lock()?;
                let deleted = connection.execute(
                    "DELETE FROM catalog_sequence_event_v2 WHERE sequence = ?1",
                    rusqlite::params![
                        sequences
                            .get(missing_index)
                            .ok_or("missing ledger fixture sequence")?
                    ],
                )?;
                if deleted != 1 {
                    return Err("fixture must delete exactly one ledger event".into());
                }
                drop(connection);
            }
            drop(catalog);
            if !matches!(
                SqliteCatalog::open(root.path(), Duration::from_millis(100)),
                Err(quanta_index_core::CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    ..
                })
            ) {
                return Err(format!(
                    "missing unpaired event at position {missing_index} must refuse reopen"
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn committed_operation_event_binds_kind_identity_and_payload() -> TestResult {
        #[derive(Debug)]
        enum Mutation {
            Kind,
            Identity,
            Payload,
        }
        for mutation in [Mutation::Kind, Mutation::Identity, Mutation::Payload] {
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
            let claim = match catalog.claim_prepared(
                &key,
                &body,
                "owner",
                i64::MAX.unsigned_abs(),
                &body,
            )? {
                ClaimOutcomeV1::Claimed(claim) => claim,
                ClaimOutcomeV1::Replay { .. } | ClaimOutcomeV1::ReplayRepoMap { .. } => {
                    return Err("fixture expected a fresh operation claim".into());
                }
            };
            catalog.mark_applying(&claim)?;
            let mut receipt = BatchPublishReceipt::empty_for(
                ManifestGeneration::new(1),
                None,
                "digest".to_string(),
            );
            receipt.accept_replace_scope();
            let sequence = i64::try_from(catalog.commit(&claim, &receipt)?)?;
            {
                let connection = catalog.lock()?;
                let (identity, payload): (Vec<u8>, Vec<u8>) = connection.query_row(
                    "SELECT identity_digest, payload_digest FROM catalog_sequence_event_v2
                 WHERE sequence = ?1",
                    rusqlite::params![sequence],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                let mut identity = blob32("fixture event identity", &identity)?;
                let mut payload = blob32("fixture event payload", &payload)?;
                let kind = match mutation {
                    Mutation::Kind => SequenceEventKindV1::Rollback,
                    Mutation::Identity => {
                        identity = [9_u8; 32];
                        SequenceEventKindV1::OperationCommitted
                    }
                    Mutation::Payload => {
                        payload = [9_u8; 32];
                        SequenceEventKindV1::OperationCommitted
                    }
                };
                let commitment = event_commitment(sequence, kind, &identity, &payload);
                let digest = event_row_digest(sequence, kind, &identity, &payload, &commitment);
                let updated = connection.execute(
                    "UPDATE catalog_sequence_event_v2
                 SET kind = ?1, identity_digest = ?2, payload_digest = ?3,
                     event_commitment = ?4, row_sha256 = ?5
                 WHERE sequence = ?6",
                    rusqlite::params![
                        kind.as_code(),
                        identity.as_slice(),
                        payload.as_slice(),
                        commitment.as_slice(),
                        digest.as_slice(),
                        sequence,
                    ],
                )?;
                if updated != 1 {
                    return Err("fixture must rewrite one committed operation event".into());
                }
                drop(connection);
            }
            let replay =
                catalog.claim_prepared(&key, &body, "retry", i64::MAX.unsigned_abs(), &body);
            if !matches!(
                replay,
                Err(quanta_index_core::CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    ..
                })
            ) {
                return Err(
                    format!("committed journal replay accepted {mutation:?} event drift").into(),
                );
            }
            drop(catalog);
            if !matches!(
                SqliteCatalog::open(root.path(), Duration::from_millis(100)),
                Err(quanta_index_core::CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    ..
                })
            ) {
                return Err(
                    format!("committed journal row accepted {mutation:?} event drift").into(),
                );
            }
        }
        Ok(())
    }

    #[test]
    fn invalidation_cannot_attribute_a_later_unpaired_operation_event() -> TestResult {
        for orphan_after_gc in [false, true] {
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
            let claim = match catalog.claim_prepared(
                &key,
                &body,
                "owner",
                i64::MAX.unsigned_abs(),
                &body,
            )? {
                ClaimOutcomeV1::Claimed(claim) => claim,
                ClaimOutcomeV1::Replay { .. } | ClaimOutcomeV1::ReplayRepoMap { .. } => {
                    return Err("fixture expected a fresh claim".into());
                }
            };
            catalog.mark_applying(&claim)?;
            let mut receipt = BatchPublishReceipt::empty_for(
                ManifestGeneration::new(1),
                None,
                "digest".to_string(),
            );
            receipt.accept_replace_scope();
            let _committed = catalog.commit(&claim, &receipt)?;
            if !orphan_after_gc {
                append_orphan_operation_event(&catalog, &key)?;
            }
            if catalog.forget_generation(&key.repo_id, &key.revision_id, key.generation)? != 1 {
                return Err("fixture must invalidate one committed row".into());
            }
            if orphan_after_gc {
                append_orphan_operation_event(&catalog, &key)?;
            }
            drop(catalog);
            if !matches!(
                SqliteCatalog::open(root.path(), Duration::from_millis(100)),
                Err(quanta_index_core::CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    ..
                })
            ) {
                return Err(format!(
                    "invalidation accepted an orphan event (after_gc={orphan_after_gc})"
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn invalidation_binds_removed_terminal_event_commitment() -> TestResult {
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
        if catalog.forget_generation(&key.repo_id, &key.revision_id, key.generation)? != 1 {
            return Err("fixture must remove one terminal row".into());
        }
        {
            let connection = catalog.lock()?;
            let identity = key.identity_digest();
            let payload = [9_u8; 32];
            let kind = SequenceEventKindV1::OperationCommitted;
            let commitment = event_commitment(sequence, kind, &identity, &payload);
            let digest = event_row_digest(sequence, kind, &identity, &payload, &commitment);
            let changed = connection.execute(
                "UPDATE catalog_sequence_event_v2
                 SET payload_digest = ?1, event_commitment = ?2, row_sha256 = ?3
                 WHERE sequence = ?4",
                rusqlite::params![
                    payload.as_slice(),
                    commitment.as_slice(),
                    digest.as_slice(),
                    sequence,
                ],
            )?;
            if changed != 1 {
                return Err("fixture must rewrite exactly one terminal event".into());
            }
            drop(connection);
        }
        drop(catalog);
        if !matches!(
            SqliteCatalog::open(root.path(), Duration::from_millis(100)),
            Err(quanta_index_core::CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                ..
            })
        ) {
            return Err("GC invalidation accepted a replaced terminal commitment".into());
        }
        Ok(())
    }

    #[test]
    fn gc_floor_cannot_be_recast_as_retry_supersession() -> TestResult {
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
        let _committed = catalog.commit(&claim, &receipt)?;
        if catalog.forget_generation(&key.repo_id, &key.revision_id, key.generation)? != 1 {
            return Err("fixture must GC one terminal row".into());
        }
        {
            let connection = catalog.lock()?;
            let (sequence, payload): (i64, Vec<u8>) = connection.query_row(
                "SELECT sequence, payload_digest FROM catalog_sequence_event_v2
                 WHERE kind = ?1 AND identity_digest = ?2",
                rusqlite::params![
                    SequenceEventKindV1::OperationGcInvalidation.as_code(),
                    key.identity_digest().as_slice(),
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let identity = key.identity_digest();
            let payload = blob32("GC payload", &payload)?;
            let kind = SequenceEventKindV1::OperationInvalidation;
            let commitment = event_commitment(sequence, kind, &identity, &payload);
            let digest = event_row_digest(sequence, kind, &identity, &payload, &commitment);
            let changed = connection.execute(
                "UPDATE catalog_sequence_event_v2
                 SET kind = ?1, event_commitment = ?2, row_sha256 = ?3
                 WHERE sequence = ?4",
                rusqlite::params![
                    kind.as_code(),
                    commitment.as_slice(),
                    digest.as_slice(),
                    sequence,
                ],
            )?;
            if changed != 1 {
                return Err("fixture must rewrite exactly one GC event".into());
            }
            drop(connection);
        }
        drop(catalog);
        if !matches!(
            SqliteCatalog::open(root.path(), Duration::from_millis(100)),
            Err(quanta_index_core::CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                ..
            })
        ) {
            return Err("GC floor was erased by a self-digested kind substitution".into());
        }
        Ok(())
    }

    #[test]
    fn deleting_gc_floor_refuses_replay_and_reopen() -> TestResult {
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
        let _prepared = catalog.prepare(&key, &body, "owner", i64::MAX.unsigned_abs(), &body)?;
        if catalog.forget_generation(&key.repo_id, &key.revision_id, key.generation)? != 1 {
            return Err("fixture must GC one nonterminal row".into());
        }
        {
            let connection = catalog.lock()?;
            let target: Vec<u8> = connection.query_row(
                "SELECT target_commitment FROM operation_gc_floor_v1
                 WHERE identity_digest = ?1",
                rusqlite::params![key.identity_digest().as_slice()],
                |row| row.get(0),
            )?;
            if blob32("nonterminal GC target", &target)? != super::NO_TERMINAL_TARGET {
                return Err("nonterminal GC must carry the no-target marker".into());
            }
            let deleted = connection.execute(
                "DELETE FROM operation_gc_floor_v1 WHERE identity_digest = ?1",
                rusqlite::params![key.identity_digest().as_slice()],
            )?;
            if deleted != 1 {
                return Err("fixture must delete exactly one GC floor".into());
            }
            drop(connection);
        }
        if !matches!(
            catalog.claim_prepared(&key, &body, "retry", i64::MAX.unsigned_abs(), &body),
            Err(quanta_index_core::CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                ..
            })
        ) {
            return Err("missing GC floor must not become a fresh claim".into());
        }
        drop(catalog);
        if !matches!(
            SqliteCatalog::open(root.path(), Duration::from_millis(100)),
            Err(quanta_index_core::CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                ..
            })
        ) {
            return Err("missing GC floor must refuse catalog reopen".into());
        }
        Ok(())
    }

    fn append_orphan_operation_event(
        catalog: &SqliteCatalog,
        key: &IdempotencyKeyV1,
    ) -> TestResult {
        let mut connection = catalog.lock()?;
        let transaction = connection.transaction()?;
        let _orphan = append_sequence_event(
            &transaction,
            SequenceEventKindV1::OperationCommitted,
            &key.identity_digest(),
            &[9_u8; 32],
        )?;
        transaction.commit()?;
        drop(connection);
        Ok(())
    }

    #[test]
    fn expired_claim_takeover_keeps_its_aborted_event_attributable() -> TestResult {
        for (prepare_takeover, foreign_owner) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
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
            let first = match catalog.claim_prepared(&key, &body, "owner", 0, &body)? {
                ClaimOutcomeV1::Claimed(claim) => claim,
                ClaimOutcomeV1::Replay { .. } | ClaimOutcomeV1::ReplayRepoMap { .. } => {
                    return Err("fixture expected an expired first claim".into());
                }
            };
            let retry_owner = if foreign_owner { "retry" } else { "owner" };
            let next_fence = if prepare_takeover {
                catalog
                    .prepare(&key, &body, retry_owner, i64::MAX.unsigned_abs(), &body)?
                    .fence_token
            } else {
                let second = catalog.claim_prepared(
                    &key,
                    &body,
                    retry_owner,
                    i64::MAX.unsigned_abs(),
                    &body,
                )?;
                match second {
                    ClaimOutcomeV1::Claimed(claim) => claim.fence_token,
                    ClaimOutcomeV1::Replay { .. } | ClaimOutcomeV1::ReplayRepoMap { .. } => {
                        return Err("fixture expected a fresh takeover claim".into());
                    }
                }
            };
            if next_fence <= first.fence_token {
                return Err("takeover must advance the durable fence".into());
            }
            if !matches!(
                catalog.mark_applying(&first),
                Err(quanta_index_core::CoreError::Typed { code, .. })
                    if code == OPERATION_FENCE_LOST_CODE
            ) {
                return Err("stale claim must lose its fence after takeover".into());
            }
            if catalog.sequence_allocator()? != (Some(3), false) {
                return Err("takeover must append abort and invalidation atomically".into());
            }
            drop(catalog);
            let _reopened = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        }
        Ok(())
    }

    #[test]
    fn prior_event_kind_schema_refuses_open_before_allocator_seed() -> TestResult {
        let root = tempfile::tempdir()?;
        let directory = catalog_dir(root.path());
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(CATALOG_FILE_NAME);
        let connection = rusqlite::Connection::open(&path)?;
        connection.execute_batch(
            "CREATE TABLE catalog_sequence_event_v2 (
                 sequence INTEGER PRIMARY KEY CHECK (sequence BETWEEN 1 AND 9223372036854775807),
                 kind INTEGER NOT NULL CHECK (kind IN (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11)),
                 identity_digest BLOB NOT NULL CHECK (length(identity_digest) = 32),
                 payload_digest BLOB NOT NULL CHECK (length(payload_digest) = 32),
                 event_commitment BLOB NOT NULL CHECK (length(event_commitment) = 32),
                 row_sha256 BLOB NOT NULL CHECK (length(row_sha256) = 32)
             ) WITHOUT ROWID;",
        )?;
        drop(connection);
        let opened = SqliteCatalog::open(root.path(), Duration::from_millis(100));
        if !matches!(
            opened,
            Err(quanta_index_core::CoreError::Storage(message))
                if message.contains("unsupported event-kind schema")
        ) {
            return Err("prior event-kind schema must refuse open".into());
        }
        let connection = rusqlite::Connection::open(path)?;
        let allocator_tables: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'catalog_sequence_v2'",
            [],
            |row| row.get(0),
        )?;
        if allocator_tables != 0 {
            return Err("schema refusal must roll back allocator schema creation".into());
        }
        Ok(())
    }

    #[test]
    fn incomplete_gc_floor_schema_refuses_open_before_allocator_seed() -> TestResult {
        let root = tempfile::tempdir()?;
        let directory = catalog_dir(root.path());
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(CATALOG_FILE_NAME);
        let connection = rusqlite::Connection::open(&path)?;
        connection.execute_batch(
            "CREATE TABLE operation_gc_floor_v1 (
                 identity_digest BLOB PRIMARY KEY,
                 invalidation_sequence INTEGER NOT NULL,
                 target_commitment BLOB NOT NULL,
                 row_sha256 BLOB NOT NULL
             ) WITHOUT ROWID;",
        )?;
        drop(connection);
        if !matches!(
            SqliteCatalog::open(root.path(), Duration::from_millis(100)),
            Err(quanta_index_core::CoreError::Storage(message))
                if message.contains("unsupported operation-GC-floor schema")
        ) {
            return Err("incomplete GC floor schema must refuse before recovery".into());
        }
        let connection = rusqlite::Connection::open(path)?;
        let allocator_tables: i64 = connection.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'catalog_sequence_v2'",
            [],
            |row| row.get(0),
        )?;
        if allocator_tables != 0 {
            return Err("schema refusal must roll back allocator schema creation".into());
        }
        Ok(())
    }
}
