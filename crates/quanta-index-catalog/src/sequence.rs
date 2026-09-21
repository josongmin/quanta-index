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
//!                     kind INTEGER CHECK(kind IN (1..=9)),
//!                     identity_digest BLOB CHECK(length=32),
//!                     payload_digest BLOB CHECK(length=32),
//!                     event_commitment BLOB CHECK(length=32),
//!                     row_sha256 BLOB CHECK(length=32))
//! ```
//!
//! This supersedes `catalog_sequence_v1` (which stays readable as
//! migration input only): v1 lacked the exhausted flag, the closed event
//! ledger and the allocator self-digest that SEP-21-002 requires.
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
//! A separate integrity pass verifies every operation-kind event has its
//! exact domain terminal row; domain maxima are never used to repair the
//! allocator.

use rusqlite::{Connection, OptionalExtension as _, Transaction, params};
use sha2::{Digest, Sha256};

use quanta_index_core::CoreError;

use crate::connection::{SqliteCatalog, blob32, engine_error};

const ALLOCATOR_DOMAIN: &[u8] = b"quanta-index:catalog:sequence-row:v1\0";
const EVENT_COMMITMENT_DOMAIN: &[u8] = b"quanta-index:catalog:sequence-event-commitment:v1\0";
const EVENT_ROW_DOMAIN: &[u8] = b"quanta-index:catalog:sequence-event-row:v1\0";
const FIELD_SEPARATOR: &[u8] = b"\x1f";

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
             kind INTEGER NOT NULL CHECK (kind IN (1, 2, 3, 4, 5, 6, 7, 8, 9)),
             identity_digest BLOB NOT NULL CHECK (length(identity_digest) = 32),
             payload_digest BLOB NOT NULL CHECK (length(payload_digest) = 32),
             event_commitment BLOB NOT NULL CHECK (length(event_commitment) = 32),
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
    Invalidation = 7,
    QuarantineRecord = 8,
    QuarantineDiscard = 9,
}

impl SequenceEventKindV1 {
    pub(crate) const ALL: [Self; 9] = [
        Self::OperationCommitted,
        Self::OperationRefused,
        Self::OperationAborted,
        Self::CandidateSeal,
        Self::Activation,
        Self::Rollback,
        Self::Invalidation,
        Self::QuarantineRecord,
        Self::QuarantineDiscard,
    ];

    #[must_use]
    /// The enum's discriminants are the closed 1..=9 `CHECK` set; the
    /// explicit match keeps the cast side-effect-free (no `as`).
    pub(crate) fn as_code(self) -> i64 {
        match self {
            Self::OperationCommitted => 1,
            Self::OperationRefused => 2,
            Self::OperationAborted => 3,
            Self::CandidateSeal => 4,
            Self::Activation => 5,
            Self::Rollback => 6,
            Self::Invalidation => 7,
            Self::QuarantineRecord => 8,
            Self::QuarantineDiscard => 9,
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
            7 => Ok(Self::Invalidation),
            8 => Ok(Self::QuarantineRecord),
            9 => Ok(Self::QuarantineDiscard),
            other => Err(corrupt(&format!("event kind code {other} is not known"))),
        }
    }
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

/// The event's commitment: SEP-21-002 preimage over the allocation
/// itself, the kind, and the two content digests.
fn event_commitment(
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

/// The event row's digest over every other column.
fn event_row_digest(
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
            return Err(corrupt(&format!("exhausted flag is {other}, expected 0 or 1")));
        }
    };
    let stored = blob32("allocator row digest", &digest)?;
    if allocator_digest(next, exhausted) != stored {
        return Err(corrupt("sequence allocator row does not match its own digest"));
    }
    if exhausted && next.is_some() {
        return Err(corrupt("sequence allocator is exhausted but still names a next value"));
    }
    if !exhausted && next.is_none() {
        return Err(corrupt("sequence allocator is not exhausted but names no next value"));
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
/// generic ledger only, then verify the event↔domain-row pairs.
///
/// Branches (from the ledger maximum alone — domain maxima are never
/// used): empty ledger → `(1, 0)`; `max < i64::MAX` → `(max + 1, 0)`;
/// `max == i64::MAX` → `(NULL, 1)`. An allocator row naming a `next`
/// beyond `max + 1` describes future events the ledger does not hold:
/// that is corruption (fail closed, [`CatalogRowCorrupt`]), never
/// silently pulled back — reissuing those sequences would break the
/// uniqueness receipts already depend on.
pub(crate) fn reconcile(
    connection: &mut Connection,
    path: &std::path::Path,
) -> Result<(), CoreError> {
    // The closed kind set the schema's CHECK admits must be exactly the
    // enum's; this cannot drift because the schema string is derived here
    // against itself at every open.
    let expected_check = format!("kind IN ({})", SequenceEventKindV1::check_set_sql());
    if !SCHEMA.contains(&expected_check) {
        return Err(corrupt(&format!(
            "sequence schema's kind CHECK does not match the closed event-kind enum ({expected_check})"
        )));
    }
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| engine_error("begin sequence reconcile", path, &error))?;
    let max: Option<i64> = transaction
        .query_row("SELECT MAX(sequence) FROM catalog_sequence_event_v2", [], |row| row.get(0))
        .map_err(|error| engine_error("read ledger maximum", path, &error))?;
    let (expected_next, expected_exhausted) = match max {
        None => (Some(1_i64), false),
        Some(i64::MAX) => (None, true),
        // Overflow is impossible in this arm: `i64::MAX` is handled above,
        // so `max + 1` always fits; saturating keeps the unreachable arm a
        // no-op rather than a panic path.
        Some(max) => (Some(max.saturating_add(1)), false),
    };
    let existing = read_allocator(&transaction, path)?;
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
    transaction
        .commit()
        .map_err(|error| engine_error("commit sequence reconcile", path, &error))?;
    verify_integrity(connection, path)
}

/// The integrity pass.
///
/// Every operation-kind event has its exact domain terminal row in
/// `idempotency_v2` (same sequence, terminal state matching the kind),
/// every event row matches its own digest, and every `Invalidation` event
/// names a generation whose journal rows are gone.
/// Seal/activation/rollback/quarantine events have no domain table in
/// this crate yet (their owners are later SEP-21 lanes); for them the
/// ledger row itself is the record.
pub(crate) fn verify_integrity(
    connection: &Connection,
    path: &std::path::Path,
) -> Result<(), CoreError> {
    let mut statement = connection
        .prepare(
            "SELECT sequence, kind, identity_digest, payload_digest, event_commitment, row_sha256
             FROM catalog_sequence_event_v2 ORDER BY sequence ASC",
        )
        .map_err(|error| engine_error("prepare integrity pass", path, &error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, Vec<u8>>(5)?,
            ))
        })
        .map_err(|error| engine_error("read events for integrity pass", path, &error))?;
    for row in rows {
        let (sequence, kind_code, identity, payload, commitment, row_digest) =
            row.map_err(|error| engine_error("read event row", path, &error))?;
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
        match kind {
            SequenceEventKindV1::OperationCommitted
            | SequenceEventKindV1::OperationRefused
            | SequenceEventKindV1::OperationAborted => {
                let expected_state = match kind {
                    SequenceEventKindV1::OperationCommitted => 4_i64,
                    SequenceEventKindV1::OperationRefused => 5_i64,
                    // Only `OperationAborted` reaches this arm (the outer
                    // match above); the other kinds are listed solely to
                    // keep this match exhaustive without a wildcard.
                    SequenceEventKindV1::OperationAborted
                    | SequenceEventKindV1::CandidateSeal
                    | SequenceEventKindV1::Activation
                    | SequenceEventKindV1::Rollback
                    | SequenceEventKindV1::Invalidation
                    | SequenceEventKindV1::QuarantineRecord
                    | SequenceEventKindV1::QuarantineDiscard => 6_i64,
                };
                // A record a generation GC dropped, or a terminal abort a
                // retry superseded, is exactly attributable through its
                // Invalidation event (same identity digest). Every other
                // missing pair is corruption.
                let invalidated = is_invalidated(connection, path, &identity)?;
                let paired: Option<i64> = connection
                    .query_row(
                        "SELECT state FROM idempotency_v2 WHERE durable_sequence = ?1",
                        params![sequence],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|error| engine_error("pair integrity lookup", path, &error))?;
                match (paired, invalidated) {
                    (Some(state), _) if state == expected_state => {}
                    (_, true) => {}
                    (paired, _) => {
                        return Err(corrupt(&format!(
                            "operation event {sequence} (kind {}) has no exact domain pair \
                             (row state {paired:?}, invalidated {invalidated})",
                            kind.as_code()
                        )));
                    }
                }
            }
            // Repomap domain pairing (P03): every CandidateSeal and
            // Activation event has its exact domain row in
            // `repomap_candidate_v1` / `repomap_activation_v1` (same
            // terminal sequence); every QuarantineRecord pairs the
            // incident's record sequence and QuarantineDiscard its
            // discard sequence. An Invalidation pairs the repomap
            // activation row when one names its sequence; invalidations
            // the idempotency lane attributes (generation GC) keep the
            // per-operation attribution consulted above and pair nothing
            // here. Rollback is not emitted by any current owner; the
            // ledger row and its digests are its record until one is.
            SequenceEventKindV1::CandidateSeal => {
                pair_exists(
                    connection,
                    path,
                    "SELECT 1 FROM repomap_candidate_v1 WHERE terminal_sequence = ?1",
                    sequence,
                    "candidate seal",
                )?;
            }
            SequenceEventKindV1::Activation => {
                pair_exists(
                    connection,
                    path,
                    "SELECT 1 FROM repomap_activation_v1 WHERE activation_sequence = ?1",
                    sequence,
                    "activation",
                )?;
            }
            SequenceEventKindV1::Rollback => {}
            SequenceEventKindV1::Invalidation => {
                pair_exists(
                    connection,
                    path,
                    "SELECT 1 FROM repomap_activation_v1
                     WHERE terminal_sequence = ?1 AND active = 0",
                    sequence,
                    "repomap activation invalidation",
                )
                .or_else(|invalidation_pair_error| {
                    // Not a repomap invalidation: the idempotency lane's
                    // attribution (checked per operation event above) owns
                    // it, so the pairing requirement does not apply.
                    if matches!(
                        invalidation_pair_error,
                        CoreError::Typed {
                            code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                            ..
                        }
                    ) {
                        Ok(())
                    } else {
                        Err(invalidation_pair_error)
                    }
                })?;
            }
            SequenceEventKindV1::QuarantineRecord => {
                pair_exists(
                    connection,
                    path,
                    "SELECT 1 FROM repomap_quarantine_event_v1 WHERE sequence = ?1",
                    sequence,
                    "quarantine record",
                )?;
            }
            SequenceEventKindV1::QuarantineDiscard => {
                pair_exists(
                    connection,
                    path,
                    "SELECT 1 FROM repomap_quarantine_event_v1 WHERE discard_sequence = ?1",
                    sequence,
                    "quarantine discard",
                )?;
            }
        }
    }
    Ok(())
}

/// Whether the generic ledger records a generation-GC invalidation whose
/// identity is `identity_digest` and whose payload digest matches.
///
/// The replay-floor check: only a generation GC invalidates a key's
/// replays; a superseded-abort invalidation attributes history without
/// raising the floor.
pub(crate) fn is_invalidated_for_floor(
    connection: &Connection,
    path: &std::path::Path,
    identity_digest: &[u8; 32],
    payload_digest: &[u8; 32],
) -> Result<bool, CoreError> {
    let found: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM catalog_sequence_event_v2
             WHERE kind = ?1 AND identity_digest = ?2 AND payload_digest = ?3 LIMIT 1",
            params![
                SequenceEventKindV1::Invalidation.as_code(),
                identity_digest.as_slice(),
                payload_digest.as_slice()
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| engine_error("read invalidation", path, &error))?;
    Ok(found.is_some())
}

/// Whether any invalidation event names `identity_digest` (the integrity
/// pass's attribution check).
pub(crate) fn is_invalidated(
    connection: &Connection,
    path: &std::path::Path,
    identity_digest: &[u8; 32],
) -> Result<bool, CoreError> {
    let found: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM catalog_sequence_event_v2
             WHERE kind = ?1 AND identity_digest = ?2 LIMIT 1",
            params![
                SequenceEventKindV1::Invalidation.as_code(),
                identity_digest.as_slice()
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| engine_error("read invalidation", path, &error))?;
    Ok(found.is_some())
}

/// The integrity pass's domain-pairing lookup: the named row must exist.
fn pair_exists(
    connection: &Connection,
    path: &std::path::Path,
    sql: &str,
    sequence: i64,
    label: &str,
) -> Result<(), CoreError> {
    let found: Option<i64> = connection
        .query_row(sql, params![sequence], |row| row.get(0))
        .optional()
        .map_err(|error| engine_error("pair integrity lookup", path, &error))?;
    if found.is_none() {
        return Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message: format!("catalog: {label} event {sequence} has no exact domain pair"),
        });
    }
    Ok(())
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
