//! `RepoMap` sealed-candidate, activation and quarantine authority
//! (SEP-21 S21-01B/S21-02, P03).
//!
//! Schema:
//!
//! ```text
//! repomap_candidate_v1(repo_id, revision_id, manifest_generation,
//!                      candidate_commitment BLOB CHECK(len=32),
//!                      object_address BLOB CHECK(len=32),
//!                      content_digest BLOB CHECK(len=32),
//!                      byte_size, state CHECK(state IN (1..=4)),
//!                      terminal_sequence UNIQUE, quarantine_sequence UNIQUE NULL,
//!                      row_sha256 CHECK(len=32),
//!                      UNIQUE(repo_id, revision_id, manifest_generation))
//! repomap_activation_v1(repo_id, revision_id UNIQUE(repo_id, revision_id),
//!                       epoch CHECK(epoch >= 1), manifest_generation,
//!                       candidate_commitment CHECK(len=32),
//!                       active CHECK(active IN (0,1)), invalidation_reason,
//!                       terminal_sequence UNIQUE, row_sha256 CHECK(len=32))
//! repomap_quarantine_event_v1(incident_digest PK CHECK(len=32),
//!                       payload_digest CHECK(len=32), envelope_bytes,
//!                       incident_time_unix_nanos, sequence UNIQUE, reason_code,
//!                       source_path, discarded CHECK(discarded IN (0,1)),
//!                       row_sha256 CHECK(len=32))
//! ```
//!
//! The `SQLite` catalog is the sole candidate and activation visibility authority:
//! the filesystem holds immutable derived projections only. Every mutation
//! allocates its terminal sequence through the crate-private
//! [`append_sequence_event`] allocator inside the same `BEGIN IMMEDIATE`
//! transaction that writes the domain row, so a rollback leaves allocator,
//! generic event and domain row all absent (zero rows).
//!
//! Closed candidate state table (one row per logical generation key):
//!
//! ```text
//! Absent -> Sealed(1)                    (seal; UNIQUE logical key)
//! Sealed -> Activated(2)                 (activate; commitment-bound CAS)
//! Activated -> ActivationInvalidated(3)  (supersede, loss, corruption)
//! Sealed|Activated -> Quarantined(4)     (terminal)
//! ```
//!
//! `ActivationInvalidated` never returns to `Activated`: re-activation of a
//! superseded generation requires a fresh sealed candidate at a new logical
//! generation (publish-only), which is exactly the no-resurrection rule.

use rusqlite::{Connection, OptionalExtension as _, params};
use sha2::{Digest, Sha256};
use std::num::NonZeroU64;

use quanta_index_contract::{CandidateCommitmentV1, RepoMapExpectedActiveV2};
use quanta_index_core::CoreError;

use crate::connection::{SqliteCatalog, blob32, engine_error};
use crate::sequence::{SequenceEventKindV1, append_sequence_event};

const CANDIDATE_ROW_DOMAIN: &[u8] = b"quanta-index:catalog:repomap-candidate-row:v1\0";
const ACTIVATION_ROW_DOMAIN: &[u8] = b"quanta-index:catalog:repomap-activation-row:v1\0";
const QUARANTINE_ROW_DOMAIN: &[u8] = b"quanta-index:catalog:repomap-quarantine-row:v1\0";
const LOGICAL_KEY_DOMAIN: &[u8] = b"quanta-index:catalog:repomap-logical-key:v1\0";
const FIELD_SEPARATOR: &[u8] = b"\x1f";

/// The tables, created at open.
pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS repomap_candidate_v1 (
             repo_id TEXT NOT NULL,
             revision_id TEXT NOT NULL,
             manifest_generation INTEGER NOT NULL CHECK (manifest_generation >= 0),
             candidate_commitment BLOB NOT NULL CHECK (length(candidate_commitment) = 32),
             object_address BLOB NOT NULL CHECK (length(object_address) = 32),
             content_digest BLOB NOT NULL CHECK (length(content_digest) = 32),
             byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
             projection_meta TEXT NOT NULL,
             state INTEGER NOT NULL CHECK (state IN (1, 2, 3, 4)),
             terminal_sequence INTEGER NOT NULL UNIQUE
                 CHECK (terminal_sequence BETWEEN 1 AND 9223372036854775807),
             quarantine_sequence INTEGER UNIQUE
                 CHECK (quarantine_sequence IS NULL OR
                        quarantine_sequence BETWEEN 1 AND 9223372036854775807),
             row_sha256 BLOB NOT NULL CHECK (length(row_sha256) = 32),
             CHECK ((state = 4) = (quarantine_sequence IS NOT NULL)),
             CHECK (quarantine_sequence IS NULL OR quarantine_sequence > terminal_sequence),
             UNIQUE (repo_id, revision_id, manifest_generation)
         );
         CREATE TABLE IF NOT EXISTS repomap_activation_v1 (
             repo_id TEXT NOT NULL,
             revision_id TEXT NOT NULL,
             epoch INTEGER NOT NULL CHECK (epoch >= 1),
             manifest_generation INTEGER NOT NULL CHECK (manifest_generation >= 0),
             candidate_commitment BLOB NOT NULL CHECK (length(candidate_commitment) = 32),
             active INTEGER NOT NULL CHECK (active IN (0, 1)),
             invalidation_reason TEXT,
             activation_sequence INTEGER NOT NULL UNIQUE
                 CHECK (activation_sequence BETWEEN 1 AND 9223372036854775807),
             terminal_sequence INTEGER NOT NULL UNIQUE
                 CHECK (terminal_sequence BETWEEN 1 AND 9223372036854775807),
             row_sha256 BLOB NOT NULL CHECK (length(row_sha256) = 32),
             UNIQUE (repo_id, revision_id, epoch)
         );
         CREATE TABLE IF NOT EXISTS repomap_quarantine_event_v1 (
             incident_digest BLOB NOT NULL PRIMARY KEY
                 CHECK (length(incident_digest) = 32),
             payload_digest BLOB NOT NULL CHECK (length(payload_digest) = 32),
             envelope_bytes BLOB NOT NULL,
             incident_time_unix_nanos INTEGER NOT NULL CHECK (incident_time_unix_nanos >= 0),
             envelope_digest BLOB NOT NULL CHECK (length(envelope_digest) = 32),
             sequence INTEGER NOT NULL UNIQUE
                 CHECK (sequence BETWEEN 1 AND 9223372036854775807),
             reason_code TEXT NOT NULL,
             source_path TEXT NOT NULL,
             discarded INTEGER NOT NULL CHECK (discarded IN (0, 1)),
             discard_sequence INTEGER UNIQUE
                 CHECK (discard_sequence IS NULL
                        OR (discard_sequence BETWEEN 1 AND 9223372036854775807
                            AND discarded = 1)),
             row_sha256 BLOB NOT NULL CHECK (length(row_sha256) = 32),
             CHECK ((discarded = 1) = (discard_sequence IS NOT NULL)),
             CHECK (discard_sequence IS NULL OR discard_sequence > sequence)
         ) WITHOUT ROWID;";

/// Refuse incompatible installed candidate and quarantine tables.
///
/// `CREATE IF NOT EXISTS` cannot add a new `CHECK` to an existing table.
/// This service has no silent schema migration reader.
pub(crate) fn verify_installed_schema(
    connection: &Connection,
    path: &std::path::Path,
) -> Result<(), CoreError> {
    let compact = |schema: &str| schema.split_whitespace().collect::<String>();
    for (table, label) in [
        ("repomap_candidate_v1", "repomap candidate"),
        ("repomap_quarantine_event_v1", "repomap quarantine"),
    ] {
        let expected = SCHEMA
            .split(';')
            .find(|statement| statement.contains(&format!("CREATE TABLE IF NOT EXISTS {table}")))
            .ok_or_else(|| corrupt(&format!("{label} schema has no table definition")))?
            .trim()
            .replacen("CREATE TABLE IF NOT EXISTS", "CREATE TABLE", 1);
        let installed: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                params![table],
                |row| row.get(0),
            )
            .map_err(|error| {
                engine_error(&format!("read installed {label} schema"), path, &error)
            })?;
        if compact(&installed) != compact(&expected) {
            return Err(CoreError::Storage(format!(
                "catalog: {} has an unsupported {label} schema; this build has no migration reader",
                path.display()
            )));
        }
    }
    Ok(())
}

/// The closed candidate state set (SEP-21 S21-02 target state machine).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RepoMapCandidateStateV1 {
    Sealed = 1,
    Activated = 2,
    ActivationInvalidated = 3,
    Quarantined = 4,
}

impl RepoMapCandidateStateV1 {
    fn as_code(self) -> i64 {
        match self {
            Self::Sealed => 1,
            Self::Activated => 2,
            Self::ActivationInvalidated => 3,
            Self::Quarantined => 4,
        }
    }

    fn from_code(code: i64) -> Result<Self, CoreError> {
        match code {
            1 => Ok(Self::Sealed),
            2 => Ok(Self::Activated),
            3 => Ok(Self::ActivationInvalidated),
            4 => Ok(Self::Quarantined),
            other => Err(corrupt(&format!(
                "candidate state code {other} is not known"
            ))),
        }
    }
}

/// One durable sealed-candidate row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapCandidateRowV1 {
    pub repo_id: String,
    pub revision_id: String,
    pub manifest_generation: u64,
    pub candidate_commitment: [u8; 32],
    pub object_address: [u8; 32],
    pub content_digest: [u8; 32],
    pub byte_size: u64,
    /// The bundle-declared projection metadata the sealed candidate's
    /// query projection is rebuilt from at boot (JSON; covered by the row
    /// digest).
    pub projection_meta: String,
    pub state: RepoMapCandidateStateV1,
    /// The `CandidateSeal` event remains stable across state transitions.
    pub terminal_sequence: i64,
    /// The distinct terminal event for a sealed-only quarantine.
    pub quarantine_sequence: Option<i64>,
}

/// One durable activation row (the serve-head pointer for a repo/revision).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapActivationRowV1 {
    pub repo_id: String,
    pub revision_id: String,
    pub epoch: u64,
    pub manifest_generation: u64,
    pub candidate_commitment: [u8; 32],
    pub active: bool,
    pub invalidation_reason: Option<String>,
    /// The `Activation` event that created this row (stable across a
    /// later invalidation, which only advances `terminal_sequence`).
    pub activation_sequence: i64,
    pub terminal_sequence: i64,
}

/// One durable quarantine incident with its exact canonical envelope bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepoMapQuarantineIncidentRowV1 {
    /// Stable identity of the incident: the evidence digest (derivable
    /// before sequence allocation, so an exact retry finds this row).
    pub incident_digest: [u8; 32],
    pub payload_digest: [u8; 32],
    pub envelope_bytes: Vec<u8>,
    /// Digest of the canonical incident envelope itself — the content
    /// address the durable projection lives at.
    pub envelope_digest: [u8; 32],
    pub incident_time_unix_nanos: i64,
    pub sequence: i64,
    pub reason_code: String,
    pub source_path: String,
    pub discarded: bool,
    pub discard_sequence: Option<i64>,
}

/// What a seal committed: the terminal sequence, and whether the original
/// receipt was replayed rather than a new event allocated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SealOutcomeV1 {
    pub terminal_sequence: i64,
    pub replayed: bool,
}

/// What an activation committed: sequence, the new epoch, the superseded
/// commitment (if any), and whether the original receipt was replayed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationOutcomeV1 {
    pub terminal_sequence: i64,
    pub epoch: u64,
    pub prior_candidate_commitment: Option<[u8; 32]>,
    pub replayed: bool,
}

fn corrupt(message: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!("catalog: {message}"),
    }
}

fn typed(code: quanta_index_contract::SearchPlaneErrorCodeV2, message: String) -> CoreError {
    CoreError::Typed { code, message }
}

/// Domain-separated digest over the logical generation key. Used as the
/// generic ledger event's identity digest so replay/conflict decisions and
/// ledger attribution share one identity.
///
/// Infallible by construction: SHA-256 over length-framed validated strings.
fn logical_key_digest(repo_id: &str, revision_id: &str, generation: u64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(LOGICAL_KEY_DOMAIN);
    hasher.update(repo_id.len().to_le_bytes());
    hasher.update(repo_id.as_bytes());
    hasher.update(revision_id.len().to_le_bytes());
    hasher.update(revision_id.as_bytes());
    hasher.update(generation.to_le_bytes());
    hasher.finalize().into()
}

/// Infallible by construction: SHA-256 over length-framed fields.
fn hash_fields(domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for part in parts {
        hasher.update(part.len().to_le_bytes());
        hasher.update(part);
        hasher.update(FIELD_SEPARATOR);
    }
    hasher.finalize().into()
}

fn candidate_row_digest(row: &RepoMapCandidateRowV1) -> [u8; 32] {
    let generation = row.manifest_generation.to_le_bytes();
    let byte_size = row.byte_size.to_le_bytes();
    let state = row.state.as_code().to_le_bytes();
    let sequence = row.terminal_sequence.to_le_bytes();
    let quarantine_sequence = row.quarantine_sequence.map_or([0_u8; 8], i64::to_le_bytes);
    hash_fields(
        CANDIDATE_ROW_DOMAIN,
        &[
            row.repo_id.as_bytes(),
            row.revision_id.as_bytes(),
            &generation,
            &row.candidate_commitment,
            &row.object_address,
            &row.content_digest,
            &byte_size,
            row.projection_meta.as_bytes(),
            &state,
            &sequence,
            &quarantine_sequence,
        ],
    )
}

fn activation_row_digest(row: &RepoMapActivationRowV1) -> [u8; 32] {
    let epoch = row.epoch.to_le_bytes();
    let generation = row.manifest_generation.to_le_bytes();
    let active = [u8::from(row.active)];
    let sequence = row.terminal_sequence.to_le_bytes();
    hash_fields(
        ACTIVATION_ROW_DOMAIN,
        &[
            row.repo_id.as_bytes(),
            row.revision_id.as_bytes(),
            &epoch,
            &generation,
            &row.candidate_commitment,
            &active,
            row.invalidation_reason
                .as_deref()
                .unwrap_or_default()
                .as_bytes(),
            &sequence,
        ],
    )
}

fn quarantine_row_digest(row: &RepoMapQuarantineIncidentRowV1) -> [u8; 32] {
    let time = row.incident_time_unix_nanos.to_le_bytes();
    let sequence = row.sequence.to_le_bytes();
    let discarded = [u8::from(row.discarded)];
    hash_fields(
        QUARANTINE_ROW_DOMAIN,
        &[
            &row.incident_digest,
            &row.payload_digest,
            &row.envelope_bytes,
            &row.envelope_digest,
            &time,
            &sequence,
            row.reason_code.as_bytes(),
            row.source_path.as_bytes(),
            &discarded,
            &row.discard_sequence.map_or([0_u8; 8], i64::to_le_bytes),
        ],
    )
}

type QuarantineRawRow = (
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    Vec<u8>,
    i64,
    i64,
    String,
    String,
    i64,
    Option<i64>,
    Vec<u8>,
);

fn quarantine_raw_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QuarantineRawRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ))
}

fn checked_quarantine_row(
    raw: QuarantineRawRow,
) -> Result<RepoMapQuarantineIncidentRowV1, CoreError> {
    let (
        digest,
        payload,
        envelope,
        envelope_digest,
        time,
        sequence,
        reason_code,
        source_path,
        discarded,
        discard_sequence,
        row_digest,
    ) = raw;
    let discarded = match discarded {
        0 => false,
        1 => true,
        other => return Err(corrupt(&format!("quarantine discarded flag is {other}"))),
    };
    if discarded != discard_sequence.is_some()
        || discard_sequence.is_some_and(|discard_sequence| discard_sequence <= sequence)
    {
        return Err(corrupt(
            "quarantine discard state or sequence is inconsistent",
        ));
    }
    let incident = RepoMapQuarantineIncidentRowV1 {
        incident_digest: blob32("incident digest", &digest)?,
        payload_digest: blob32("incident payload digest", &payload)?,
        envelope_bytes: envelope,
        envelope_digest: blob32("quarantine envelope digest", &envelope_digest)?,
        incident_time_unix_nanos: time,
        sequence,
        reason_code,
        source_path,
        discarded,
        discard_sequence,
    };
    if quarantine_row_digest(&incident) != blob32("incident row digest", &row_digest)? {
        return Err(corrupt(
            "quarantine incident row does not match its own digest",
        ));
    }
    Ok(incident)
}

fn read_candidate_row(
    transaction: &Connection,
    repo_id: &str,
    revision_id: &str,
    generation: u64,
) -> Result<Option<RepoMapCandidateRowV1>, CoreError> {
    let path = std::path::Path::new(":catalog:");
    let generation_i64 = i64::try_from(generation).map_err(|error| {
        CoreError::InvalidContract(format!(
            "catalog: generation {generation} does not fit the catalog's integer column: {error}"
        ))
    })?;
    let fetched = transaction
        .query_row(
            "SELECT repo_id, revision_id, manifest_generation, candidate_commitment,
                    object_address, content_digest, byte_size, projection_meta, state,
                    terminal_sequence, quarantine_sequence, row_sha256
             FROM repomap_candidate_v1
             WHERE repo_id = ?1 AND revision_id = ?2 AND manifest_generation = ?3",
            params![repo_id, revision_id, generation_i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Vec<u8>>(11)?,
                ))
            },
        )
        .optional()
        .map_err(|error| engine_error("read repomap candidate", path, &error))?;
    let Some((
        stored_repo,
        stored_revision,
        stored_generation,
        commitment,
        address,
        content,
        byte_size,
        projection_meta,
        state,
        sequence,
        quarantine_sequence,
        digest,
    )) = fetched
    else {
        return Ok(None);
    };
    let row = RepoMapCandidateRowV1 {
        repo_id: stored_repo,
        revision_id: stored_revision,
        manifest_generation: u64::try_from(stored_generation)
            .map_err(|_error| corrupt("candidate manifest_generation does not fit u64"))?,
        candidate_commitment: blob32("candidate commitment", &commitment)?,
        object_address: blob32("candidate object address", &address)?,
        content_digest: blob32("candidate content digest", &content)?,
        byte_size: u64::try_from(byte_size)
            .map_err(|_error| corrupt("candidate byte_size does not fit u64"))?,
        projection_meta,
        state: RepoMapCandidateStateV1::from_code(state)?,
        terminal_sequence: sequence,
        quarantine_sequence,
    };
    let stored = blob32("candidate row digest", &digest)?;
    if candidate_row_digest(&row) != stored {
        return Err(corrupt(
            "repomap candidate row does not match its own digest",
        ));
    }
    Ok(Some(row))
}

/// Verify a candidate event against the canonical candidate row, not merely
/// against a sequence column. Both seal and quarantine events bind the same
/// logical key and compiled candidate commitment.
pub(crate) fn verify_candidate_event_pair(
    connection: &Connection,
    path: &std::path::Path,
    kind: SequenceEventKindV1,
    sequence: i64,
    identity: &[u8; 32],
    payload: &[u8; 32],
) -> Result<(), CoreError> {
    let (sql, label) = match kind {
        SequenceEventKindV1::CandidateSeal => (
            "SELECT repo_id, revision_id, manifest_generation FROM repomap_candidate_v1
             WHERE terminal_sequence = ?1",
            "candidate seal",
        ),
        SequenceEventKindV1::RepoMapCandidateQuarantine => (
            "SELECT repo_id, revision_id, manifest_generation FROM repomap_candidate_v1
             WHERE quarantine_sequence = ?1 AND state = 4",
            "candidate quarantine",
        ),
        SequenceEventKindV1::OperationCommitted
        | SequenceEventKindV1::OperationRefused
        | SequenceEventKindV1::OperationAborted
        | SequenceEventKindV1::Activation
        | SequenceEventKindV1::Rollback
        | SequenceEventKindV1::OperationInvalidation
        | SequenceEventKindV1::QuarantineRecord
        | SequenceEventKindV1::QuarantineDiscard
        | SequenceEventKindV1::RepoMapInvalidation => {
            return Err(corrupt(
                "non-candidate event reached candidate pair verifier",
            ));
        }
    };
    let key: Option<(String, String, i64)> = connection
        .query_row(sql, params![sequence], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()
        .map_err(|error| engine_error("read candidate event pair", path, &error))?;
    let Some((repo_id, revision_id, generation)) = key else {
        return Err(corrupt(&format!(
            "{label} event {sequence} has no exact domain pair"
        )));
    };
    let generation = u64::try_from(generation)
        .map_err(|_error| corrupt("candidate event generation does not fit u64"))?;
    let candidate = read_candidate_row(connection, &repo_id, &revision_id, generation)?
        .ok_or_else(|| corrupt("candidate event pair disappeared during integrity check"))?;
    if logical_key_digest(&repo_id, &revision_id, generation) != *identity
        || candidate.candidate_commitment != *payload
    {
        return Err(corrupt(&format!(
            "{label} event {sequence} disagrees with its candidate identity or commitment"
        )));
    }
    Ok(())
}

fn read_activation_row(
    connection: &Connection,
    repo_id: &str,
    revision_id: &str,
) -> Result<Option<RepoMapActivationRowV1>, CoreError> {
    let path = std::path::Path::new(":catalog:");
    let fetched = connection
        .query_row(
            "SELECT repo_id, revision_id, epoch, manifest_generation, candidate_commitment,
                    active, invalidation_reason, activation_sequence, terminal_sequence,
                    row_sha256
             FROM repomap_activation_v1
             WHERE repo_id = ?1 AND revision_id = ?2
             ORDER BY epoch DESC LIMIT 1",
            params![repo_id, revision_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Vec<u8>>(9)?,
                ))
            },
        )
        .optional()
        .map_err(|error| engine_error("read repomap activation", path, &error))?;
    let Some((
        stored_repo,
        stored_revision,
        epoch,
        generation,
        commitment,
        active,
        reason,
        activation_sequence,
        sequence,
        digest,
    )) = fetched
    else {
        return Ok(None);
    };
    let active = match active {
        0 => false,
        1 => true,
        other => {
            return Err(corrupt(&format!(
                "activation active flag is {other}, expected 0 or 1"
            )));
        }
    };
    let row = RepoMapActivationRowV1 {
        repo_id: stored_repo,
        revision_id: stored_revision,
        epoch: u64::try_from(epoch)
            .map_err(|_error| corrupt("activation epoch does not fit u64"))?,
        manifest_generation: u64::try_from(generation)
            .map_err(|_error| corrupt("activation generation does not fit u64"))?,
        candidate_commitment: blob32("activation commitment", &commitment)?,
        active,
        invalidation_reason: reason,
        activation_sequence,
        terminal_sequence: sequence,
    };
    let stored = blob32("activation row digest", &digest)?;
    if activation_row_digest(&row) != stored {
        return Err(corrupt(
            "repomap activation row does not match its own digest",
        ));
    }
    Ok(Some(row))
}

fn read_replayed_prior_activation_commitment(
    connection: &Connection,
    repo_id: &str,
    revision_id: &str,
    active_epoch: u64,
) -> Result<Option<[u8; 32]>, CoreError> {
    let Some(prior_epoch) = active_epoch.checked_sub(1).filter(|epoch| *epoch > 0) else {
        return Ok(None);
    };
    let prior_epoch = i64::try_from(prior_epoch)
        .map_err(|_error| corrupt("prior activation epoch does not fit i64"))?;
    let fetched = connection
        .query_row(
            "SELECT repo_id, revision_id, epoch, manifest_generation, candidate_commitment,
                    active, invalidation_reason, activation_sequence, terminal_sequence,
                    row_sha256
             FROM repomap_activation_v1
             WHERE repo_id = ?1 AND revision_id = ?2 AND epoch = ?3",
            params![repo_id, revision_id, prior_epoch],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Vec<u8>>(9)?,
                ))
            },
        )
        .optional()
        .map_err(|error| {
            engine_error(
                "read prior repomap activation",
                std::path::Path::new(":catalog:"),
                &error,
            )
        })?;
    let Some((
        stored_repo,
        stored_revision,
        epoch,
        generation,
        commitment,
        active,
        reason,
        activation_sequence,
        terminal_sequence,
        digest,
    )) = fetched
    else {
        return Err(corrupt(
            "active replay is missing its prior activation epoch",
        ));
    };
    let active = match active {
        0 => false,
        1 => true,
        other => return Err(corrupt(&format!("prior activation active flag is {other}"))),
    };
    let row = RepoMapActivationRowV1 {
        repo_id: stored_repo,
        revision_id: stored_revision,
        epoch: u64::try_from(epoch)
            .map_err(|_error| corrupt("prior activation epoch does not fit u64"))?,
        manifest_generation: u64::try_from(generation)
            .map_err(|_error| corrupt("prior activation generation does not fit u64"))?,
        candidate_commitment: blob32("prior activation commitment", &commitment)?,
        active,
        invalidation_reason: reason,
        activation_sequence,
        terminal_sequence,
    };
    if activation_row_digest(&row) != blob32("prior activation row digest", &digest)? {
        return Err(corrupt(
            "prior activation row does not match its own digest",
        ));
    }
    Ok((row.invalidation_reason.as_deref() == Some("superseded"))
        .then_some(row.candidate_commitment))
}

impl SqliteCatalog {
    /// Seal a candidate under its logical generation key.
    ///
    /// Same logical key + same commitment replays the original receipt
    /// (`replayed = true`, no new sequence); same logical key + different
    /// commitment is `CANDIDATE_COMMITMENT_CONFLICT`; a fresh key allocates
    /// one `CandidateSeal` event and inserts the domain row in one
    /// transaction.
    #[expect(
        clippy::too_many_arguments,
        reason = "seal mirrors the frozen candidate contract fields; grouping them would fork the P02A compiled-candidate type"
    )]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the transaction that borrows the connection; the suggested early drop would break the borrow"
    )]
    pub fn seal_repomap_candidate(
        &self,
        repo_id: &str,
        revision_id: &str,
        manifest_generation: u64,
        candidate_commitment: &[u8; 32],
        object_address: &[u8; 32],
        content_digest: &[u8; 32],
        byte_size: u64,
        projection_meta: &str,
    ) -> Result<SealOutcomeV1, CoreError> {
        let mut connection = self.lock()?;
        let path = std::path::Path::new(":catalog:");
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin repomap seal", path, &error))?;
        if let Some(existing) =
            read_candidate_row(&transaction, repo_id, revision_id, manifest_generation)?
        {
            if existing.candidate_commitment == *candidate_commitment {
                if existing.object_address != *object_address
                    || existing.content_digest != *content_digest
                    || existing.byte_size != byte_size
                    || existing.projection_meta != projection_meta
                {
                    return Err(typed(
                        quanta_index_contract::SearchPlaneErrorCodeV2::CandidateCommitmentConflict,
                        format!(
                            "catalog: logical generation repo={repo_id} revision={revision_id} \
                             generation={manifest_generation} replays the compiled commitment \
                             with different durable candidate custody"
                        ),
                    ));
                }
                return Ok(SealOutcomeV1 {
                    terminal_sequence: existing.terminal_sequence,
                    replayed: true,
                });
            }
            return Err(typed(
                quanta_index_contract::SearchPlaneErrorCodeV2::CandidateCommitmentConflict,
                format!(
                    "catalog: logical generation repo={repo_id} revision={revision_id} \
                     generation={manifest_generation} is already sealed with a different \
                     candidate commitment"
                ),
            ));
        }
        let identity = logical_key_digest(repo_id, revision_id, manifest_generation);
        let sequence = append_sequence_event(
            &transaction,
            SequenceEventKindV1::CandidateSeal,
            &identity,
            candidate_commitment,
        )?;
        let row = RepoMapCandidateRowV1 {
            repo_id: repo_id.to_string(),
            revision_id: revision_id.to_string(),
            manifest_generation,
            candidate_commitment: *candidate_commitment,
            object_address: *object_address,
            content_digest: *content_digest,
            byte_size,
            projection_meta: projection_meta.to_string(),
            state: RepoMapCandidateStateV1::Sealed,
            terminal_sequence: sequence,
            quarantine_sequence: None,
        };
        let generation_i64 = i64::try_from(manifest_generation).map_err(|error| {
            CoreError::InvalidContract(format!(
                "catalog: generation {manifest_generation} does not fit the catalog: {error}"
            ))
        })?;
        let _inserted = transaction
            .execute(
                "INSERT INTO repomap_candidate_v1
                     (repo_id, revision_id, manifest_generation, candidate_commitment,
                      object_address, content_digest, byte_size, projection_meta, state,
                      terminal_sequence, row_sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    row.repo_id,
                    row.revision_id,
                    generation_i64,
                    row.candidate_commitment.as_slice(),
                    row.object_address.as_slice(),
                    row.content_digest.as_slice(),
                    i64::try_from(row.byte_size).map_err(|error| {
                        CoreError::InvalidContract(format!(
                            "catalog: candidate byte size {} does not fit the catalog: {error}",
                            row.byte_size
                        ))
                    })?,
                    row.projection_meta,
                    row.state.as_code(),
                    row.terminal_sequence,
                    candidate_row_digest(&row).as_slice(),
                ],
            )
            .map_err(|error| engine_error("insert repomap candidate", path, &error))?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit repomap seal", path, &error))?;
        Ok(SealOutcomeV1 {
            terminal_sequence: sequence,
            replayed: false,
        })
    }

    /// Activate a sealed candidate under target binding and prior-head CAS.
    ///
    /// `expected_commitment` is the exact candidate commitment the caller
    /// derived from the sealed object; a mismatch is
    /// `ACTIVATION_CAS_CONFLICT`. A target that is not sealed (absent,
    /// invalidated, quarantined) is `ACTIVATION_TARGET_NOT_SEALED`.
    /// `expected_active` must match the current catalog row exactly before
    /// sequence allocation. An absent expectation only matches no active row.
    /// Re-activating the exact active candidate replays the original
    /// receipt without a new sequence. Superseding an active activation
    /// invalidates the superseded candidate durably in the same
    /// transaction.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the transaction that borrows the connection; the suggested early drop would break the borrow"
    )]
    pub fn activate_repomap_candidate(
        &self,
        repo_id: &str,
        revision_id: &str,
        manifest_generation: u64,
        expected_commitment: &[u8; 32],
        expected_active: Option<&RepoMapExpectedActiveV2>,
    ) -> Result<ActivationOutcomeV1, CoreError> {
        let mut connection = self.lock()?;
        let path = std::path::Path::new(":catalog:");
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin repomap activate", path, &error))?;
        let Some(candidate) =
            read_candidate_row(&transaction, repo_id, revision_id, manifest_generation)?
        else {
            return Err(typed(
                quanta_index_contract::SearchPlaneErrorCodeV2::ActivationTargetNotSealed,
                format!(
                    "catalog: no sealed candidate for repo={repo_id} revision={revision_id} \
                     generation={manifest_generation}"
                ),
            ));
        };
        if candidate.candidate_commitment != *expected_commitment {
            return Err(typed(
                quanta_index_contract::SearchPlaneErrorCodeV2::ActivationCasConflict,
                format!(
                    "catalog: activation of repo={repo_id} revision={revision_id} \
                     generation={manifest_generation} names a commitment that is not the \
                     sealed candidate's"
                ),
            ));
        }
        if candidate.state == RepoMapCandidateStateV1::ActivationInvalidated
            || candidate.state == RepoMapCandidateStateV1::Quarantined
        {
            return Err(typed(
                quanta_index_contract::SearchPlaneErrorCodeV2::ActivationTargetNotSealed,
                format!(
                    "catalog: candidate repo={repo_id} revision={revision_id} \
                     generation={manifest_generation} is {:?} and cannot be activated",
                    candidate.state
                ),
            ));
        }
        let existing = read_activation_row(&transaction, repo_id, revision_id)?;
        if candidate.state == RepoMapCandidateStateV1::Activated
            && existing.as_ref().is_some_and(|row| {
                row.active
                    && row.manifest_generation == manifest_generation
                    && row.candidate_commitment == *expected_commitment
            })
        {
            let Some(active) = existing else {
                return Err(corrupt("replay check found no activation row"));
            };
            let replayed_prior = read_replayed_prior_activation_commitment(
                &transaction,
                repo_id,
                revision_id,
                active.epoch,
            )?;
            let replayed_expected = replayed_prior
                .map(|commitment| {
                    let epoch = active
                        .epoch
                        .checked_sub(1)
                        .and_then(NonZeroU64::new)
                        .ok_or_else(|| corrupt("replayed prior activation epoch is absent"))?;
                    Ok::<_, CoreError>(RepoMapExpectedActiveV2::new(
                        epoch,
                        CandidateCommitmentV1::from_bytes(commitment),
                    ))
                })
                .transpose()?;
            if expected_active != replayed_expected.as_ref() {
                return Err(typed(
                    quanta_index_contract::SearchPlaneErrorCodeV2::ActivationCasConflict,
                    "catalog: replay expectation differs from the committed prior head".to_string(),
                ));
            }
            return Ok(ActivationOutcomeV1 {
                terminal_sequence: active.terminal_sequence,
                epoch: active.epoch,
                prior_candidate_commitment: replayed_prior,
                replayed: true,
            });
        }
        let actual_active = existing
            .as_ref()
            .filter(|row| row.active)
            .map(|row| {
                let epoch = NonZeroU64::new(row.epoch)
                    .ok_or_else(|| corrupt("active activation epoch is zero"))?;
                Ok::<_, CoreError>(RepoMapExpectedActiveV2::new(
                    epoch,
                    CandidateCommitmentV1::from_bytes(row.candidate_commitment),
                ))
            })
            .transpose()?;
        if expected_active != actual_active.as_ref() {
            return Err(typed(
                quanta_index_contract::SearchPlaneErrorCodeV2::ActivationCasConflict,
                "catalog: expected active head differs from the current head".to_string(),
            ));
        }
        let prior_commitment = existing
            .as_ref()
            .filter(|row| row.active)
            .map(|row| row.candidate_commitment);
        let prior_epoch = existing.as_ref().map_or(0, |row| row.epoch);
        let epoch = prior_epoch
            .checked_add(1)
            .ok_or_else(|| corrupt("activation epoch overflow"))?;
        let identity = logical_key_digest(repo_id, revision_id, manifest_generation);
        let sequence = append_sequence_event(
            &transaction,
            SequenceEventKindV1::Activation,
            &identity,
            expected_commitment,
        )?;
        // Supersede the prior activation and its candidate in the same
        // transaction when one is active. The prior activation row becomes
        // an append-only history entry: its terminal_sequence is the
        // Invalidation event's sequence, so the ledger integrity pass keeps
        // an exact domain pair for BOTH activation events.
        if let Some(prior) = existing.as_ref().filter(|row| row.active) {
            let prior_identity =
                logical_key_digest(repo_id, revision_id, prior.manifest_generation);
            let invalidation_sequence = append_sequence_event(
                &transaction,
                SequenceEventKindV1::RepoMapInvalidation,
                &prior_identity,
                &prior.candidate_commitment,
            )?;
            let retired = RepoMapActivationRowV1 {
                active: false,
                invalidation_reason: Some("superseded".to_string()),
                terminal_sequence: invalidation_sequence,
                ..prior.clone()
            };
            let _retired_row = transaction
                .execute(
                    "UPDATE repomap_activation_v1 SET
                         active = 0, invalidation_reason = 'superseded',
                         terminal_sequence = ?1, row_sha256 = ?2
                     WHERE repo_id = ?3 AND revision_id = ?4 AND epoch = ?5",
                    params![
                        invalidation_sequence,
                        activation_row_digest(&retired).as_slice(),
                        repo_id,
                        revision_id,
                        i64::try_from(retired.epoch)
                            .map_err(|_error| corrupt("activation epoch does not fit i64"))?,
                    ],
                )
                .map_err(|error| engine_error("retire superseded activation", path, &error))?;
            let superseded = read_candidate_row(
                &transaction,
                repo_id,
                revision_id,
                prior.manifest_generation,
            )?
            .ok_or_else(|| {
                corrupt(&format!(
                    "prior activation names generation {} with no candidate row",
                    prior.manifest_generation
                ))
            })?;
            let superseded_row = RepoMapCandidateRowV1 {
                state: RepoMapCandidateStateV1::ActivationInvalidated,
                ..superseded
            };
            let _updated = transaction
                .execute(
                    "UPDATE repomap_candidate_v1 SET state = 3, row_sha256 = ?1 WHERE
                         repo_id = ?2 AND revision_id = ?3 AND manifest_generation = ?4",
                    params![
                        candidate_row_digest(&superseded_row).as_slice(),
                        repo_id,
                        revision_id,
                        i64::try_from(superseded_row.manifest_generation).map_err(|error| {
                            corrupt(&format!(
                                "prior activation generation does not fit i64: {error}"
                            ))
                        })?,
                    ],
                )
                .map_err(|error| engine_error("supersede prior candidate", path, &error))?;
        }
        let generation_i64 = i64::try_from(manifest_generation).map_err(|error| {
            CoreError::InvalidContract(format!(
                "catalog: generation {manifest_generation} does not fit the catalog: {error}"
            ))
        })?;
        let activated_row = RepoMapCandidateRowV1 {
            state: RepoMapCandidateStateV1::Activated,
            ..candidate
        };
        let _activated = transaction
            .execute(
                "UPDATE repomap_candidate_v1 SET state = 2, row_sha256 = ?1 WHERE
                     repo_id = ?2 AND revision_id = ?3 AND manifest_generation = ?4",
                params![
                    candidate_row_digest(&activated_row).as_slice(),
                    repo_id,
                    revision_id,
                    generation_i64,
                ],
            )
            .map_err(|error| engine_error("activate repomap candidate", path, &error))?;
        let activation = RepoMapActivationRowV1 {
            repo_id: repo_id.to_string(),
            revision_id: revision_id.to_string(),
            epoch,
            manifest_generation,
            candidate_commitment: *expected_commitment,
            active: true,
            invalidation_reason: None,
            activation_sequence: sequence,
            terminal_sequence: sequence,
        };
        let _written = transaction
            .execute(
                "INSERT INTO repomap_activation_v1
                     (repo_id, revision_id, epoch, manifest_generation, candidate_commitment,
                      active, invalidation_reason, activation_sequence, terminal_sequence,
                      row_sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, NULL, ?6, ?6, ?7)
                 ON CONFLICT (repo_id, revision_id, epoch) DO UPDATE SET
                     manifest_generation = excluded.manifest_generation,
                     candidate_commitment = excluded.candidate_commitment,
                     active = 1,
                     invalidation_reason = NULL,
                     activation_sequence = excluded.activation_sequence,
                     terminal_sequence = excluded.terminal_sequence,
                     row_sha256 = excluded.row_sha256",
                params![
                    activation.repo_id,
                    activation.revision_id,
                    i64::try_from(epoch).map_err(|_error| corrupt("epoch does not fit i64"))?,
                    generation_i64,
                    activation.candidate_commitment.as_slice(),
                    activation.terminal_sequence,
                    activation_row_digest(&activation).as_slice(),
                ],
            )
            .map_err(|error| engine_error("write repomap activation", path, &error))?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit repomap activate", path, &error))?;
        Ok(ActivationOutcomeV1 {
            terminal_sequence: sequence,
            epoch,
            prior_candidate_commitment: prior_commitment,
            replayed: false,
        })
    }

    /// Durably invalidate a candidate's activation (loss, corruption,
    /// supersede-by-rollback is a fresh activation, not this). The row
    /// never returns to activated without a fresh sealed candidate.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the transaction that borrows the connection; the suggested early drop would break the borrow"
    )]
    pub fn invalidate_repomap_activation(
        &self,
        repo_id: &str,
        revision_id: &str,
        manifest_generation: u64,
        reason: &str,
    ) -> Result<i64, CoreError> {
        let mut connection = self.lock()?;
        let path = std::path::Path::new(":catalog:");
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin repomap invalidation", path, &error))?;
        let Some(active_row) = read_activation_row(&transaction, repo_id, revision_id)? else {
            return Err(typed(
                quanta_index_contract::SearchPlaneErrorCodeV2::ActivationTargetNotSealed,
                format!(
                    "catalog: no activation row to invalidate for repo={repo_id} \
                     revision={revision_id}"
                ),
            ));
        };
        if !active_row.active {
            return Err(typed(
                quanta_index_contract::SearchPlaneErrorCodeV2::ActivationTargetNotSealed,
                format!(
                    "catalog: activation for repo={repo_id} revision={revision_id} is already \
                     invalidated ({})",
                    active_row
                        .invalidation_reason
                        .unwrap_or_else(|| "reason not recorded".to_string())
                ),
            ));
        }
        let identity = logical_key_digest(repo_id, revision_id, manifest_generation);
        let sequence = append_sequence_event(
            &transaction,
            SequenceEventKindV1::RepoMapInvalidation,
            &identity,
            &active_row.candidate_commitment,
        )?;
        let invalidated = RepoMapActivationRowV1 {
            active: false,
            invalidation_reason: Some(reason.to_string()),
            terminal_sequence: sequence,
            ..active_row
        };
        let _invalidated = transaction
            .execute(
                "UPDATE repomap_activation_v1
                 SET active = 0, invalidation_reason = ?1,
                     terminal_sequence = ?2,
                     row_sha256 = ?3
                 WHERE repo_id = ?4 AND revision_id = ?5",
                params![
                    reason,
                    sequence,
                    activation_row_digest(&invalidated).as_slice(),
                    repo_id,
                    revision_id,
                ],
            )
            .map_err(|error| engine_error("invalidate repomap activation", path, &error))?;
        let generation_i64 = i64::try_from(manifest_generation).map_err(|error| {
            CoreError::InvalidContract(format!(
                "catalog: generation {manifest_generation} does not fit the catalog: {error}"
            ))
        })?;
        let invalidated_candidate =
            read_candidate_row(&transaction, repo_id, revision_id, manifest_generation)?
                .ok_or_else(|| corrupt("invalidation names a generation with no candidate row"))?;
        let invalidated_candidate = RepoMapCandidateRowV1 {
            state: RepoMapCandidateStateV1::ActivationInvalidated,
            ..invalidated_candidate
        };
        let _marked = transaction
            .execute(
                "UPDATE repomap_candidate_v1 SET state = 3, row_sha256 = ?1 WHERE
                     repo_id = ?2 AND revision_id = ?3 AND manifest_generation = ?4",
                params![
                    candidate_row_digest(&invalidated_candidate).as_slice(),
                    repo_id,
                    revision_id,
                    generation_i64,
                ],
            )
            .map_err(|error| engine_error("mark repomap candidate invalidated", path, &error))?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit repomap invalidation", path, &error))?;
        Ok(sequence)
    }

    /// Record a quarantine incident, preserving the exact canonical
    /// envelope bytes, payload digest, incident time and sequence.
    ///
    /// The envelope embeds the terminal sequence, so it is built by
    /// `build_envelope` inside the allocating transaction: allocator,
    /// generic event, domain row and envelope bytes commit atomically. A
    /// repeated identical incident digest returns the stored row: the
    /// original time and sequence are replayed, never regenerated.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the transaction that borrows the connection; the suggested early drop would break the borrow"
    )]
    pub fn record_repomap_quarantine_incident(
        &self,
        incident_digest: &[u8; 32],
        payload_digest: &[u8; 32],
        incident_time_unix_nanos: i64,
        reason_code: &str,
        source_path: &str,
        build_envelope: &(dyn Fn(i64) -> Result<([u8; 32], Vec<u8>), CoreError> + '_),
    ) -> Result<RepoMapQuarantineIncidentRowV1, CoreError> {
        let mut connection = self.lock()?;
        let path = std::path::Path::new(":catalog:");
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin repomap quarantine record", path, &error))?;
        let existing: Option<QuarantineRawRow> = transaction
            .query_row(
                "SELECT incident_digest, payload_digest, envelope_bytes, envelope_digest,
                        incident_time_unix_nanos, sequence, reason_code, source_path,
                        discarded, discard_sequence, row_sha256
                 FROM repomap_quarantine_event_v1
                 WHERE incident_digest = ?1",
                params![incident_digest.as_slice()],
                quarantine_raw_row,
            )
            .optional()
            .map_err(|error| engine_error("read repomap quarantine incident", path, &error))?;
        if let Some(raw) = existing {
            // Exact retry returns the self-digested durable row, including
            // original observation fields and a later discard tombstone.
            return checked_quarantine_row(raw);
        }
        let sequence = append_sequence_event(
            &transaction,
            SequenceEventKindV1::QuarantineRecord,
            incident_digest,
            payload_digest,
        )?;
        let (envelope_digest, envelope_bytes) = build_envelope(sequence)?;
        let row = RepoMapQuarantineIncidentRowV1 {
            incident_digest: *incident_digest,
            payload_digest: *payload_digest,
            envelope_bytes,
            envelope_digest,
            incident_time_unix_nanos,
            sequence,
            reason_code: reason_code.to_string(),
            source_path: source_path.to_string(),
            discarded: false,
            discard_sequence: None,
        };
        let _inserted = transaction
            .execute(
                "INSERT INTO repomap_quarantine_event_v1
                     (incident_digest, payload_digest, envelope_bytes, envelope_digest,
                      incident_time_unix_nanos, sequence, reason_code, source_path, discarded,
                      row_sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
                params![
                    row.incident_digest.as_slice(),
                    row.payload_digest.as_slice(),
                    row.envelope_bytes,
                    row.envelope_digest.as_slice(),
                    row.incident_time_unix_nanos,
                    row.sequence,
                    row.reason_code,
                    row.source_path,
                    quarantine_row_digest(&row).as_slice(),
                ],
            )
            .map_err(|error| engine_error("insert repomap quarantine incident", path, &error))?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit repomap quarantine record", path, &error))?;
        Ok(row)
    }

    /// Journal a quarantine discard tombstone: the incident/event row stays
    /// durable, only the payload becomes reclaimable.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the transaction that borrows the connection; the suggested early drop would break the borrow"
    )]
    pub fn discard_repomap_quarantine_payload(
        &self,
        incident_digest: &[u8; 32],
    ) -> Result<i64, CoreError> {
        let mut connection = self.lock()?;
        let path = std::path::Path::new(":catalog:");
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin repomap quarantine discard", path, &error))?;
        let fetched = transaction
            .query_row(
                "SELECT incident_digest, payload_digest, envelope_bytes, envelope_digest,
                        incident_time_unix_nanos, sequence, reason_code, source_path, discarded,
                        discard_sequence, row_sha256
                 FROM repomap_quarantine_event_v1 WHERE incident_digest = ?1",
                params![incident_digest.as_slice()],
                quarantine_raw_row,
            )
            .optional()
            .map_err(|error| engine_error("read repomap quarantine incident", path, &error))?;
        let Some(raw) = fetched else {
            return Err(typed(
                quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
                "catalog: no quarantine incident under that digest".to_string(),
            ));
        };
        let prior = checked_quarantine_row(raw)?;
        if prior.discarded {
            // Already tombstoned: exact replay of the discard receipt.
            return prior
                .discard_sequence
                .ok_or_else(|| corrupt("discarded quarantine incident has no discard sequence"));
        }
        let sequence = append_sequence_event(
            &transaction,
            SequenceEventKindV1::QuarantineDiscard,
            incident_digest,
            &prior.payload_digest,
        )?;
        let row = RepoMapQuarantineIncidentRowV1 {
            discarded: true,
            discard_sequence: Some(sequence),
            ..prior
        };
        let _updated = transaction
            .execute(
                "UPDATE repomap_quarantine_event_v1
                 SET discarded = 1, discard_sequence = ?1, row_sha256 = ?2
                 WHERE incident_digest = ?3",
                params![
                    sequence,
                    quarantine_row_digest(&row).as_slice(),
                    incident_digest.as_slice(),
                ],
            )
            .map_err(|error| engine_error("tombstone quarantine incident", path, &error))?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit repomap quarantine discard", path, &error))?;
        Ok(sequence)
    }

    /// Mark a sealed (never-activated) candidate quarantined: terminal
    /// state, one candidate-quarantine event, no activation row touched.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the transaction that borrows the connection; the suggested early drop would break the borrow"
    )]
    pub fn quarantine_repomap_candidate(
        &self,
        repo_id: &str,
        revision_id: &str,
        manifest_generation: u64,
    ) -> Result<i64, CoreError> {
        let mut connection = self.lock()?;
        let path = std::path::Path::new(":catalog:");
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| engine_error("begin repomap candidate quarantine", path, &error))?;
        let Some(candidate) =
            read_candidate_row(&transaction, repo_id, revision_id, manifest_generation)?
        else {
            return Err(typed(
                quanta_index_contract::SearchPlaneErrorCodeV2::ActivationTargetNotSealed,
                format!(
                    "catalog: no candidate to quarantine for repo={repo_id} \
                     revision={revision_id} generation={manifest_generation}"
                ),
            ));
        };
        if candidate.state == RepoMapCandidateStateV1::Quarantined {
            // Exact replay of the terminal state.
            return candidate
                .quarantine_sequence
                .ok_or_else(|| corrupt("quarantined candidate has no quarantine sequence"));
        }
        if candidate.state != RepoMapCandidateStateV1::Sealed {
            return Err(typed(
                quanta_index_contract::SearchPlaneErrorCodeV2::ActivationTargetNotSealed,
                format!(
                    "catalog: cannot quarantine candidate in state {:?} for repo={repo_id} \
                     revision={revision_id} generation={manifest_generation}",
                    candidate.state
                ),
            ));
        }
        let identity = logical_key_digest(repo_id, revision_id, manifest_generation);
        let sequence = append_sequence_event(
            &transaction,
            SequenceEventKindV1::RepoMapCandidateQuarantine,
            &identity,
            &candidate.candidate_commitment,
        )?;
        let generation_i64 = i64::try_from(manifest_generation).map_err(|error| {
            CoreError::InvalidContract(format!(
                "catalog: generation {manifest_generation} does not fit the catalog: {error}"
            ))
        })?;
        let quarantined_row = RepoMapCandidateRowV1 {
            state: RepoMapCandidateStateV1::Quarantined,
            quarantine_sequence: Some(sequence),
            ..candidate
        };
        let _marked = transaction
            .execute(
                "UPDATE repomap_candidate_v1 SET state = 4, quarantine_sequence = ?1,
                     row_sha256 = ?2 WHERE
                     repo_id = ?3 AND revision_id = ?4 AND manifest_generation = ?5",
                params![
                    sequence,
                    candidate_row_digest(&quarantined_row).as_slice(),
                    repo_id,
                    revision_id,
                    generation_i64,
                ],
            )
            .map_err(|error| engine_error("quarantine repomap candidate", path, &error))?;
        transaction
            .commit()
            .map_err(|error| engine_error("commit repomap candidate quarantine", path, &error))?;
        Ok(sequence)
    }

    /// The durable candidate row for one logical generation key, if any.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the transaction that borrows the connection; the suggested early drop would break the borrow"
    )]
    pub fn repomap_candidate_row(
        &self,
        repo_id: &str,
        revision_id: &str,
        manifest_generation: u64,
    ) -> Result<Option<RepoMapCandidateRowV1>, CoreError> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction()
            .map_err(|error| engine_error("begin candidate read", &self.path, &error))?;
        read_candidate_row(&transaction, repo_id, revision_id, manifest_generation)
    }

    /// The durable activation row for one repo/revision pair, if any.
    pub fn repomap_activation_row(
        &self,
        repo_id: &str,
        revision_id: &str,
    ) -> Result<Option<RepoMapActivationRowV1>, CoreError> {
        let connection = self.lock()?;
        read_activation_row(&connection, repo_id, revision_id)
    }

    /// Every durable candidate row, ascending by terminal sequence.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the connection borrow; kept explicit for symmetry with the mutating paths"
    )]
    pub fn repomap_candidate_rows(&self) -> Result<Vec<RepoMapCandidateRowV1>, CoreError> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT repo_id, revision_id, manifest_generation, candidate_commitment,
                        object_address, content_digest, byte_size, projection_meta, state,
                        terminal_sequence, quarantine_sequence, row_sha256
                 FROM repomap_candidate_v1 ORDER BY terminal_sequence ASC",
            )
            .map_err(|error| engine_error("prepare candidate listing", &self.path, &error))?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, Vec<u8>>(11)?,
                ))
            })
            .map_err(|error| engine_error("list candidates", &self.path, &error))?;
        let mut out = Vec::new();
        for row in rows {
            let (
                repo_id,
                revision_id,
                generation,
                commitment,
                address,
                content,
                byte_size,
                projection_meta,
                state,
                sequence,
                quarantine_sequence,
                digest,
            ) = row.map_err(|error| engine_error("read candidate row", &self.path, &error))?;
            let candidate = RepoMapCandidateRowV1 {
                repo_id,
                revision_id,
                manifest_generation: u64::try_from(generation)
                    .map_err(|_error| corrupt("candidate generation does not fit u64"))?,
                candidate_commitment: blob32("candidate commitment", &commitment)?,
                object_address: blob32("candidate object address", &address)?,
                content_digest: blob32("candidate content digest", &content)?,
                byte_size: u64::try_from(byte_size)
                    .map_err(|_error| corrupt("candidate byte_size does not fit u64"))?,
                projection_meta,
                state: RepoMapCandidateStateV1::from_code(state)?,
                terminal_sequence: sequence,
                quarantine_sequence,
            };
            let stored = blob32("candidate row digest", &digest)?;
            if candidate_row_digest(&candidate) != stored {
                return Err(corrupt(
                    "repomap candidate row does not match its own digest",
                ));
            }
            out.push(candidate);
        }
        Ok(out)
    }

    /// Every durable quarantine incident, ascending by sequence.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the guard must outlive the connection borrow; kept explicit for symmetry with the mutating paths"
    )]
    pub fn repomap_quarantine_incidents(
        &self,
    ) -> Result<Vec<RepoMapQuarantineIncidentRowV1>, CoreError> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT incident_digest, payload_digest, envelope_bytes, envelope_digest,
                        incident_time_unix_nanos, sequence, reason_code, source_path, discarded,
                        discard_sequence, row_sha256
                 FROM repomap_quarantine_event_v1 ORDER BY sequence ASC",
            )
            .map_err(|error| engine_error("prepare incident listing", &self.path, &error))?;
        let rows = statement
            .query_map([], quarantine_raw_row)
            .map_err(|error| engine_error("list incidents", &self.path, &error))?;
        let mut out = Vec::new();
        for row in rows {
            let raw = row.map_err(|error| engine_error("read incident row", &self.path, &error))?;
            out.push(checked_quarantine_row(raw)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::time::Duration;

    use quanta_index_contract::SearchPlaneErrorCodeV2;
    use quanta_index_core::CoreError;
    use rusqlite::params;

    use super::{
        RepoMapCandidateRowV1, RepoMapCandidateStateV1, SqliteCatalog, candidate_row_digest,
        logical_key_digest, read_candidate_row,
    };
    use crate::connection::{CATALOG_FILE_NAME, catalog_dir};
    use crate::sequence::{SequenceEventKindV1, append_sequence_event};

    #[test]
    fn quarantine_incident_retry_returns_the_stored_row_even_after_discard()
    -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        let digest = [1_u8; 32];
        let first = catalog.record_repomap_quarantine_incident(
            &digest,
            &[2_u8; 32],
            42,
            "original-reason",
            "original-path",
            &|_sequence| Ok(([3_u8; 32], vec![4_u8])),
        )?;
        let replay = catalog.record_repomap_quarantine_incident(
            &digest,
            &[9_u8; 32],
            99,
            "different-reason",
            "different-path",
            &|_sequence| Err(CoreError::Storage("replay rebuilt envelope".to_string())),
        )?;
        if replay != first {
            return Err("incident retry did not return the stored original row".into());
        }
        let discard_sequence = catalog.discard_repomap_quarantine_payload(&digest)?;
        let replay_after_discard = catalog.record_repomap_quarantine_incident(
            &digest,
            &[9_u8; 32],
            100,
            "different-reason",
            "different-path",
            &|_sequence| Err(CoreError::Storage("replay rebuilt envelope".to_string())),
        )?;
        let listed = catalog.repomap_quarantine_incidents()?;
        if listed.as_slice() != [replay_after_discard.clone()]
            || !replay_after_discard.discarded
            || replay_after_discard.discard_sequence != Some(discard_sequence)
        {
            return Err("discarded incident retry did not return durable tombstone".into());
        }
        Ok(())
    }

    #[test]
    fn discarded_incident_without_discard_sequence_refuses_replay() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        let digest = [5_u8; 32];
        let incident = catalog.record_repomap_quarantine_incident(
            &digest,
            &[6_u8; 32],
            42,
            "reason",
            "path",
            &|_sequence| Ok(([7_u8; 32], vec![8_u8])),
        )?;
        let malformed = super::RepoMapQuarantineIncidentRowV1 {
            discarded: true,
            discard_sequence: None,
            ..incident
        };
        {
            let connection = catalog.lock()?;
            let malformed_update = connection.execute(
                "UPDATE repomap_quarantine_event_v1
                 SET discarded = 1, row_sha256 = ?1 WHERE incident_digest = ?2",
                params![
                    super::quarantine_row_digest(&malformed).as_slice(),
                    digest.as_slice()
                ],
            );
            if malformed_update.is_ok() {
                return Err("current schema must reject a missing discard sequence".into());
            }
            // Model a self-digested row imported from an older or tampered DB;
            // the current schema independently rejects this state on writes.
            connection.execute_batch("PRAGMA ignore_check_constraints = ON")?;
            let changed = connection.execute(
                "UPDATE repomap_quarantine_event_v1
                 SET discarded = 1, row_sha256 = ?1 WHERE incident_digest = ?2",
                params![
                    super::quarantine_row_digest(&malformed).as_slice(),
                    digest.as_slice()
                ],
            )?;
            connection.execute_batch("PRAGMA ignore_check_constraints = OFF")?;
            drop(connection);
            if changed != 1 {
                return Err("hostile fixture must update one incident".into());
            }
        }
        let replay = catalog.discard_repomap_quarantine_payload(&digest);
        if !matches!(
            replay,
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                ..
            })
        ) {
            return Err("missing discard sequence must not replay the record sequence".into());
        }
        Ok(())
    }

    #[test]
    fn quarantine_schema_without_discard_order_refuses_open() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let directory = catalog_dir(root.path());
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(CATALOG_FILE_NAME);
        let connection = rusqlite::Connection::open(&path)?;
        let old_schema = super::SCHEMA.replace(
            ",
             CHECK ((discarded = 1) = (discard_sequence IS NOT NULL)),
             CHECK (discard_sequence IS NULL OR discard_sequence > sequence)",
            "",
        );
        connection.execute_batch(&old_schema)?;
        drop(connection);

        let reopened = SqliteCatalog::open(root.path(), Duration::from_millis(100));
        if !matches!(
            reopened,
            Err(CoreError::Storage(message)) if message.contains("unsupported repomap quarantine schema")
        ) {
            return Err("quarantine schema without discard order must refuse open".into());
        }
        let connection = rusqlite::Connection::open(path)?;
        let allocator_rows: i64 =
            connection.query_row("SELECT COUNT(*) FROM catalog_sequence_v2", [], |row| {
                row.get(0)
            })?;
        if allocator_rows != 0 {
            return Err("quarantine schema refusal must precede allocator seed".into());
        }
        Ok(())
    }

    #[test]
    fn sealed_candidate_quarantine_replays_and_reopens_with_both_event_pairs()
    -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        let seal = catalog.seal_repomap_candidate(
            "repo",
            "revision",
            1,
            &[1_u8; 32],
            &[2_u8; 32],
            &[3_u8; 32],
            4,
            "{}",
        )?;
        let quarantine = catalog.quarantine_repomap_candidate("repo", "revision", 1)?;
        if seal.terminal_sequence == quarantine
            || catalog.quarantine_repomap_candidate("repo", "revision", 1)? != quarantine
        {
            return Err("quarantine did not allocate and replay its own sequence".into());
        }
        drop(catalog);

        let reopened = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        let row = reopened
            .repomap_candidate_row("repo", "revision", 1)?
            .ok_or("sealed candidate disappeared after reopen")?;
        if row.state != RepoMapCandidateStateV1::Quarantined
            || row.terminal_sequence != seal.terminal_sequence
            || row.quarantine_sequence != Some(quarantine)
            || reopened.quarantine_repomap_candidate("repo", "revision", 1)? != quarantine
        {
            return Err("candidate quarantine pair changed after reopen".into());
        }
        Ok(())
    }

    #[test]
    fn quarantine_event_must_bind_candidate_identity_and_commitment_at_reopen()
    -> Result<(), Box<dyn Error>> {
        for wrong_identity in [true, false] {
            let root = tempfile::tempdir()?;
            let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
            let _seal = catalog.seal_repomap_candidate(
                "repo",
                "revision",
                1,
                &[1_u8; 32],
                &[2_u8; 32],
                &[3_u8; 32],
                4,
                "{}",
            )?;
            {
                let mut connection = catalog.lock()?;
                let transaction = connection.transaction()?;
                let candidate = read_candidate_row(&transaction, "repo", "revision", 1)?
                    .ok_or("sealed candidate is absent")?;
                let identity = if wrong_identity {
                    logical_key_digest("another-repo", "revision", 1)
                } else {
                    logical_key_digest("repo", "revision", 1)
                };
                let payload = if wrong_identity {
                    candidate.candidate_commitment
                } else {
                    [9_u8; 32]
                };
                let sequence = append_sequence_event(
                    &transaction,
                    SequenceEventKindV1::RepoMapCandidateQuarantine,
                    &identity,
                    &payload,
                )?;
                let quarantined = RepoMapCandidateRowV1 {
                    state: RepoMapCandidateStateV1::Quarantined,
                    quarantine_sequence: Some(sequence),
                    ..candidate
                };
                let changed = transaction.execute(
                    "UPDATE repomap_candidate_v1 SET state = 4, quarantine_sequence = ?1,
                     row_sha256 = ?2 WHERE repo_id = 'repo' AND revision_id = 'revision'
                     AND manifest_generation = 1",
                    params![sequence, candidate_row_digest(&quarantined).as_slice()],
                )?;
                if changed != 1 {
                    return Err("hostile fixture must change exactly one candidate".into());
                }
                transaction.commit()?;
                drop(connection);
            }
            drop(catalog);
            let reopened = SqliteCatalog::open(root.path(), Duration::from_millis(100));
            if !matches!(
                reopened,
                Err(CoreError::Typed {
                    code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    ..
                })
            ) {
                return Err(format!(
                    "quarantine event with wrong {} must refuse reopen",
                    if wrong_identity {
                        "identity"
                    } else {
                        "commitment"
                    }
                )
                .into());
            }
        }
        Ok(())
    }

    #[test]
    fn seal_event_must_bind_candidate_identity_at_reopen() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        {
            let mut connection = catalog.lock()?;
            let transaction = connection.transaction()?;
            let sequence = append_sequence_event(
                &transaction,
                SequenceEventKindV1::CandidateSeal,
                &logical_key_digest("another-repo", "revision", 1),
                &[1_u8; 32],
            )?;
            let candidate = RepoMapCandidateRowV1 {
                repo_id: "repo".to_string(),
                revision_id: "revision".to_string(),
                manifest_generation: 1,
                candidate_commitment: [1_u8; 32],
                object_address: [2_u8; 32],
                content_digest: [3_u8; 32],
                byte_size: 4,
                projection_meta: "{}".to_string(),
                state: RepoMapCandidateStateV1::Sealed,
                terminal_sequence: sequence,
                quarantine_sequence: None,
            };
            let inserted = transaction.execute(
                "INSERT INTO repomap_candidate_v1
                 (repo_id, revision_id, manifest_generation, candidate_commitment,
                  object_address, content_digest, byte_size, projection_meta, state,
                  terminal_sequence, row_sha256)
                 VALUES (?1, ?2, 1, ?3, ?4, ?5, 4, '{}', 1, ?6, ?7)",
                params![
                    candidate.repo_id,
                    candidate.revision_id,
                    candidate.candidate_commitment.as_slice(),
                    candidate.object_address.as_slice(),
                    candidate.content_digest.as_slice(),
                    sequence,
                    candidate_row_digest(&candidate).as_slice(),
                ],
            )?;
            if inserted != 1 {
                return Err("hostile fixture must insert one candidate".into());
            }
            transaction.commit()?;
            drop(connection);
        }
        drop(catalog);
        let reopened = SqliteCatalog::open(root.path(), Duration::from_millis(100));
        if !matches!(
            reopened,
            Err(CoreError::Typed {
                code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                ..
            })
        ) {
            return Err("seal event with wrong identity must refuse reopen".into());
        }
        Ok(())
    }

    #[test]
    fn quarantine_sequence_must_follow_the_candidate_seal() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let catalog = SqliteCatalog::open(root.path(), Duration::from_millis(100))?;
        let mut connection = catalog.lock()?;
        let transaction = connection.transaction()?;
        let identity = logical_key_digest("repo", "revision", 1);
        let quarantine_sequence = append_sequence_event(
            &transaction,
            SequenceEventKindV1::RepoMapCandidateQuarantine,
            &identity,
            &[1_u8; 32],
        )?;
        let seal_sequence = append_sequence_event(
            &transaction,
            SequenceEventKindV1::CandidateSeal,
            &identity,
            &[1_u8; 32],
        )?;
        let candidate = RepoMapCandidateRowV1 {
            repo_id: "repo".to_string(),
            revision_id: "revision".to_string(),
            manifest_generation: 1,
            candidate_commitment: [1_u8; 32],
            object_address: [2_u8; 32],
            content_digest: [3_u8; 32],
            byte_size: 4,
            projection_meta: "{}".to_string(),
            state: RepoMapCandidateStateV1::Quarantined,
            terminal_sequence: seal_sequence,
            quarantine_sequence: Some(quarantine_sequence),
        };
        let reversed = transaction.execute(
            "INSERT INTO repomap_candidate_v1
             (repo_id, revision_id, manifest_generation, candidate_commitment,
              object_address, content_digest, byte_size, projection_meta, state,
              terminal_sequence, quarantine_sequence, row_sha256)
             VALUES (?1, ?2, 1, ?3, ?4, ?5, 4, '{}', 4, ?6, ?7, ?8)",
            params![
                candidate.repo_id,
                candidate.revision_id,
                candidate.candidate_commitment.as_slice(),
                candidate.object_address.as_slice(),
                candidate.content_digest.as_slice(),
                seal_sequence,
                quarantine_sequence,
                candidate_row_digest(&candidate).as_slice(),
            ],
        );
        if reversed.is_ok() {
            return Err("quarantine before seal must violate the catalog state order".into());
        }
        drop(transaction);
        drop(connection);
        Ok(())
    }

    #[test]
    fn candidate_schema_without_quarantine_order_refuses_open() -> Result<(), Box<dyn Error>> {
        let root = tempfile::tempdir()?;
        let directory = catalog_dir(root.path());
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(CATALOG_FILE_NAME);
        let connection = rusqlite::Connection::open(&path)?;
        let old_schema = super::SCHEMA.replace(
            "CHECK (quarantine_sequence IS NULL OR quarantine_sequence > terminal_sequence),",
            "",
        );
        connection.execute_batch(&old_schema)?;
        drop(connection);

        let reopened = SqliteCatalog::open(root.path(), Duration::from_millis(100));
        if !matches!(
            reopened,
            Err(CoreError::Storage(message)) if message.contains("unsupported repomap candidate schema")
        ) {
            return Err("candidate schema without quarantine ordering must refuse open".into());
        }
        let connection = rusqlite::Connection::open(path)?;
        let allocator_rows: i64 =
            connection.query_row("SELECT COUNT(*) FROM catalog_sequence_v2", [], |row| {
                row.get(0)
            })?;
        if allocator_rows != 0 {
            return Err("candidate schema refusal must precede allocator seed".into());
        }
        Ok(())
    }
}
