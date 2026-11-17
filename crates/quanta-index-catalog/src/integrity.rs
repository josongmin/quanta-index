//! Cross-domain catalog integrity composition.
//!
//! Domain owners depend on the sequence ledger primitives. This module binds
//! their rows to the ledger at open without making the ledger depend on owners.

use quanta_index_core::{
    CoreError,
    index::{IndexError, MemoryLifetime, WorkBudget, WorkUsage},
};
use rusqlite::Connection;

use crate::connection::engine_error;
use crate::sequence::{
    SequenceEventKindV1, checked_event_row, event_commitment, event_raw_row,
    has_later_invalidation, verify_gc_floor_domain_integrity, verify_invalidation_target,
};

fn corrupt(message: &str) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
        message: format!("catalog: {message}"),
    }
}

/// The integrity pass.
///
/// Every event row matches its own commitment and digest. Operation-kind
/// events pair with their terminal idempotency row or an invalidation;
/// `RepoMap` candidate events pair with a self-digested candidate row whose
/// logical identity and commitment match the event. Activation and invalidation
/// events pair with their activation row by sequence, identity and commitment;
/// quarantine events pair with their incident row.
/// Rollback has no current producer and is represented only by its ledger row.
pub(crate) fn verify_integrity(
    connection: &Connection,
    path: &std::path::Path,
) -> Result<(), CoreError> {
    match verify_integrity_inner(connection, path, None) {
        Ok(()) => Ok(()),
        Err(IntegrityCheckError::Integrity(error)) => Err(error),
        Err(IntegrityCheckError::Admission(_)) => Err(CoreError::InvalidContract(
            "startup integrity pass has no WorkBudget".into(),
        )),
    }
}

pub(crate) enum IntegrityCheckError {
    Admission(IndexError),
    Integrity(CoreError),
}

impl From<CoreError> for IntegrityCheckError {
    fn from(error: CoreError) -> Self {
        Self::Integrity(error)
    }
}
impl From<IndexError> for IntegrityCheckError {
    fn from(error: IndexError) -> Self {
        Self::Admission(error)
    }
}

pub(crate) fn verify_integrity_admitted(
    connection: &Connection,
    path: &std::path::Path,
    budget: &mut WorkBudget,
) -> Result<(), IntegrityCheckError> {
    verify_integrity_inner(connection, path, Some(budget))
}

fn verify_integrity_inner(
    connection: &Connection,
    path: &std::path::Path,
    mut budget: Option<&mut WorkBudget>,
) -> Result<(), IntegrityCheckError> {
    verify_gc_floor_domain_integrity(connection, path)?;
    crate::candidate::verify_repomap_domain_integrity(connection, path)?;
    crate::idempotency::verify_terminal_domain_integrity(connection, path)?;
    let mut statement = connection
        .prepare(
            "SELECT sequence, kind, identity_digest, payload_digest, event_commitment, row_sha256
             FROM catalog_sequence_event_v2 ORDER BY sequence ASC",
        )
        .map_err(|error| engine_error("prepare integrity pass", path, &error))?;
    let mut rows = statement
        .query([])
        .map_err(|error| engine_error("read events for integrity pass", path, &error))?;
    let mut expected_sequence = Some(1_i64);
    while let Some(row) = rows
        .next()
        .map_err(|error| engine_error("read event row", path, &error))?
    {
        let _scratch = if let Some(budget) = budget.as_deref_mut() {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 256,
                ..WorkUsage::default()
            })?;
            Some(
                budget
                    .reserve_memory(512, MemoryLifetime::Temporary)?
                    .ok_or(IndexError::ResourceLimited)?,
            )
        } else {
            None
        };
        let event =
            event_raw_row(row).map_err(|error| engine_error("read event row", path, &error))?;
        let sequence = event.0;
        if expected_sequence != Some(sequence) {
            return Err(corrupt(&format!(
                "sequence event {sequence} is not the expected contiguous ledger event {expected_sequence:?}"
            )).into());
        }
        expected_sequence = sequence.checked_add(1);
        let (_, kind, identity, payload) = checked_event_row(event)?;
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
                    | SequenceEventKindV1::OperationInvalidation
                    | SequenceEventKindV1::OperationGcInvalidation
                    | SequenceEventKindV1::RepoMapInvalidation
                    | SequenceEventKindV1::RepoMapCandidateQuarantine
                    | SequenceEventKindV1::QuarantineRecord
                    | SequenceEventKindV1::QuarantineDiscard => 6_i64,
                };
                // A record a generation GC dropped, or a terminal abort a
                // retry superseded, is exactly attributable through its
                // Invalidation event (same identity digest). Every other
                // missing pair is corruption.
                let paired = crate::idempotency::verify_terminal_event_pair(
                    connection,
                    path,
                    sequence,
                    budget.as_deref_mut(),
                )?;
                if paired == Some(expected_state) {
                    continue;
                }
                let invalidated = if paired.is_none() {
                    let commitment = event_commitment(sequence, kind, &identity, &payload);
                    has_later_invalidation(connection, path, sequence, &identity, &commitment)?
                } else {
                    false
                };
                if !invalidated {
                    return Err(corrupt(&format!(
                        "operation event {sequence} (kind {}) has no exact domain pair \
                         (row state {paired:?}, invalidated {invalidated})",
                        kind.as_code()
                    ))
                    .into());
                }
            }
            // Repomap domain pairing (P03): every CandidateSeal and
            // Activation event has its exact domain row in
            // `repomap_candidate_v1` / `repomap_activation_v1` (same
            // terminal sequence); every QuarantineRecord pairs the
            // incident's record sequence and QuarantineDiscard its
            // discard sequence. RepoMapInvalidation pairs the inactive
            // activation row; RepoMapCandidateQuarantine pairs the sealed
            // candidate's quarantine sequence. OperationGcInvalidation pairs
            // a retained replay-floor row; retry supersession binds the
            // removed historical terminal through its event commitment.
            // Rollback is not emitted by any current owner; the
            // ledger row and its digests are its record until one is.
            SequenceEventKindV1::CandidateSeal
            | SequenceEventKindV1::RepoMapCandidateQuarantine => {
                crate::candidate::verify_candidate_event_pair(
                    connection,
                    path,
                    kind,
                    sequence,
                    &identity,
                    &payload,
                    budget.as_deref_mut(),
                )?
            }
            SequenceEventKindV1::Activation | SequenceEventKindV1::RepoMapInvalidation => {
                crate::candidate::verify_activation_event_pair(
                    connection,
                    path,
                    kind,
                    sequence,
                    &identity,
                    &payload,
                    budget.as_deref_mut(),
                )?;
            }
            // Invalidation carries either the immediately preceding terminal
            // event commitment or the explicit no-terminal marker. GC has a
            // distinct kind so replay-floor checks cannot mistake a retry
            // supersession for retention GC. Rollback has no current owner.
            SequenceEventKindV1::Rollback => {}
            SequenceEventKindV1::OperationInvalidation
            | SequenceEventKindV1::OperationGcInvalidation => {
                verify_invalidation_target(connection, path, sequence, kind, &identity, &payload)?;
            }
            SequenceEventKindV1::QuarantineRecord | SequenceEventKindV1::QuarantineDiscard => {
                crate::candidate::verify_quarantine_event_pair(
                    connection,
                    path,
                    kind,
                    sequence,
                    &identity,
                    &payload,
                    budget.as_deref_mut(),
                )?;
            }
        }
    }
    Ok(())
}
