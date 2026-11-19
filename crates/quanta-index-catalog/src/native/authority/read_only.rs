//! A coherent existing-root Catalog read without DDL, recovery or source writes.
//! Physical immutable-object authentication belongs to the object-store owner.

use std::{
    fs,
    os::unix::{ffi::OsStrExt as _, fs::MetadataExt as _},
    path::Path,
    time::Duration,
};

use quanta_index_contract::{
    AuxEpochV1, BatchPublishReceipt, CandidateCommitmentV1, IngestOperationKindV1,
    ManifestGeneration, RepoId, RepoMapMutationPhaseV2, RepoMapTerminalReceiptV2, RevisionId,
    SearchPlaneErrorCodeV2,
    index::{
        HistorySnapshotCheckpointV1, HistorySourceCutV1, HistorySourceInputV1,
        MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1, NativeProtectedSourceHeadV1,
        NativeProtectedSourceRoutesV1,
    },
    native_cutover_evidence_v1::NativeCutoverEvidenceLegacyHistoryV1,
};
use quanta_index_core::{
    AuxiliaryGenerationKeyV1, AuxiliaryRowFamilyV1, AuxiliaryRowV1, CoreError, IdempotencyKeyV1,
    OperationJournalStateV1,
    domains::auxiliary::HistorySnapshotDigestV1,
    index::{
        CatalogView, Digest, ImmutableOperation, IndexError, IngressRef, MemoryLease,
        MemoryLifetime, ObjectNamespaceBinding, OperationKey, OperationStatus, ViewRef, WorkBudget,
        WorkUsage, WorkspaceKey,
    },
};
use rusqlite::{
    Connection, OpenFlags, OptionalExtension as _, config::DbConfig, params, types::ValueRef,
};

use super::{
    NativeBootManifest, boot_manifest, capture_format, decode, encode, operation_key, owned_work,
    read_operation, read_work_by_key, reclaim, retention, sql, uint, workspace_key,
};
use crate::{
    CATALOG_FILE_NAME, CatalogReadOnlyIoAdmission, candidate::ActiveRepoMapReadbackV1, catalog_dir,
    idempotency,
};

#[path = "read_only/history_pages.rs"]
mod history_pages;
pub use history_pages::visit_legacy_history_checkpoint_pages_v1;
#[path = "read_only/protected_native_snapshot.rs"]
mod protected_native_snapshot;
#[cfg(test)]
pub(super) use protected_native_snapshot::visit_native_protected_snapshot_connection_v1;
pub use protected_native_snapshot::{
    NativeProtectedSnapshotWitnessV1, read_native_protected_snapshot_witness_v1,
    visit_native_protected_snapshot_rows_v1,
};
#[cfg(test)]
pub(super) use protected_native_snapshot::{
    compare_native_protected_snapshot_connection_v1, visit_native_protected_snapshot_connection_v1,
};
#[path = "read_only/protected_native_install.rs"]
mod protected_native_install;
#[cfg(test)]
pub(super) use protected_native_install::verify_imported_exact_views_v1;
#[cfg(test)]
pub(super) use protected_native_install::verify_imported_exact_views_v1;
pub use protected_native_install::verify_native_protected_snapshot_install_v1;

/// Metadata observed in one `SQLite` read transaction. This is not a physical
/// object proof, a producer completion seal or a deployable boot receipt.
pub struct NativeBootCatalogSnapshot {
    pub root_nonce: [u8; 32],
    pub selected: Option<NativeBootManifest>,
    pub namespace: Option<ObjectNamespaceBinding>,
    history_predecessors: Vec<NativeBootManifest>,
    _history_allocation: Option<MemoryLease>,
    _allocation: Option<MemoryLease>,
}

impl NativeBootCatalogSnapshot {
    /// Published operations strictly between the attested anchor and selected head.
    #[must_use]
    pub fn history_predecessors(&self) -> &[NativeBootManifest] {
        &self.history_predecessors
    }
}

/// One admitted read result. The requested candidate cannot be moved away
/// from the retained memory charge owned by the accompanying snapshot.
pub struct NativeBootCatalogCandidate {
    // Data must be dropped before its retained allocation in `snapshot`.
    manifest: NativeBootManifest,
    snapshot: NativeBootCatalogSnapshot,
}

impl NativeBootCatalogCandidate {
    #[must_use]
    pub const fn snapshot(&self) -> &NativeBootCatalogSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub const fn manifest(&self) -> &NativeBootManifest {
        &self.manifest
    }
}

/// The durable Published operation and its fixed metadata survive retirement
/// of old query objects. This result creates no query pin or object custody.
pub struct NativeBootPublishedOperation {
    operation: ImmutableOperation,
    publication: CatalogView,
    // An independently persisted current Native row from the same Catalog
    // observation. A matching operation key is deduplicated before SQL.
    additional: Option<(ImmutableOperation, CatalogView)>,
    snapshot: NativeBootCatalogSnapshot,
}

impl NativeBootPublishedOperation {
    #[must_use]
    pub const fn operation(&self) -> &ImmutableOperation {
        &self.operation
    }
    #[must_use]
    pub const fn publication(&self) -> ViewRef {
        self.publication.identity
    }
    /// Metadata from the same durable Published row, independent of Exact retention.
    #[must_use]
    pub const fn publication_metadata(&self) -> &CatalogView {
        &self.publication
    }
    #[must_use]
    pub const fn snapshot(&self) -> &NativeBootCatalogSnapshot {
        &self.snapshot
    }

    /// Return only an actual Published row read in this snapshot. A selected
    /// BOOT row and a historical FULL row cannot stand in for a different key.
    #[must_use]
    pub fn published_for(&self, key: OperationKey) -> Option<(&ImmutableOperation, &CatalogView)> {
        if self.operation.key == key {
            return Some((&self.operation, &self.publication));
        }
        self.additional
            .as_ref()
            .and_then(|(operation, publication)| {
                (operation.key == key).then_some((operation, publication))
            })
    }
}

/// One actual Native root observation after canonical quiescence checks.
/// The caller still retains its root lease and compares this observation with
/// the independent Source/Host/SDK scope before any selection publication.
pub struct NativeBootQuiescentCatalogV1 {
    pub snapshot: NativeBootCatalogSnapshot,
    pub current_head: CatalogView,
}

/// Read-only observation of the held Legacy Catalog journal and ledger.
/// This is data under a retained Original charge, not Source authorization.
pub struct LegacyCatalogQuiescenceV1 {
    pub terminal_rows: u64,
    pub last_fence: u64,
    /// High-water of retained committed SearchCorpus records; the producer
    /// must independently prove its complete replay/floor and snapshot scope.
    pub latest_retained_search_corpus_sequence: Option<u64>,
    has_history_rows: bool,
    active_repomap: Option<ActiveRepoMapReadbackV1>,
    _repomap_allocation: Option<MemoryLease>,
    _allocation: MemoryLease,
}

impl LegacyCatalogQuiescenceV1 {
    /// Whether the admitted Legacy source contains durable History rows.
    #[must_use]
    pub const fn has_history_rows_v1(&self) -> bool {
        self.has_history_rows
    }

    #[must_use]
    pub fn active_repomap_candidate_v1(&self) -> Option<&crate::RepoMapCandidateRowV1> {
        self.active_repomap.as_ref().map(|active| &active.candidate)
    }

    /// Compare the supplied receipts with the candidate seal and active head
    /// from this admitted snapshot. Physical object and metadata verification
    /// remain with the RepoMap store.
    pub fn require_active_repomap_ack_v1(
        &self,
        publish: &RepoMapTerminalReceiptV2,
        activate: &RepoMapTerminalReceiptV2,
        budget: &mut WorkBudget,
    ) -> Result<&crate::RepoMapCandidateRowV1, IndexError> {
        let active = self
            .active_repomap
            .as_ref()
            .ok_or(IndexError::DigestConflict)?;
        require_active_repomap_ack_row_v1(active, publish, activate, budget)
    }
}

fn require_active_repomap_ack_row_v1<'a>(
    active: &'a ActiveRepoMapReadbackV1,
    publish: &RepoMapTerminalReceiptV2,
    activate: &RepoMapTerminalReceiptV2,
    budget: &mut WorkBudget,
) -> Result<&'a crate::RepoMapCandidateRowV1, IndexError> {
    let candidate = &active.candidate;
    let head = &active.activation;
    let publish_ack = &publish.mutation;
    let activate_ack = &activate.mutation;
    let bytes = [publish_ack, activate_ack]
        .into_iter()
        .try_fold(256_u64, |total, ack| {
            [
                ack.repo_id.as_str(),
                ack.revision_id.as_str(),
                &ack.new_candidate_commitment,
                ack.prior_candidate_commitment
                    .as_deref()
                    .unwrap_or_default(),
            ]
            .into_iter()
            .try_fold(total, |total, field| {
                total.checked_add(field.len().try_into().ok()?)
            })
        })
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        bytes,
        ..WorkUsage::default()
    })?;
    let parse = |wire: &str| {
        CandidateCommitmentV1::from_wire_str(wire)
            .map(|commitment| *commitment.as_bytes())
            .map_err(|_| IndexError::DigestConflict)
    };
    if publish.phase != RepoMapMutationPhaseV2::Publish
        || activate.phase != RepoMapMutationPhaseV2::Activate
        || publish_ack.prior_candidate_commitment.is_some()
        || publish_ack.activation_epoch != 0
        || u64::try_from(candidate.terminal_sequence).ok() != Some(publish_ack.terminal_sequence)
        || u64::try_from(head.terminal_sequence).ok() != Some(activate_ack.terminal_sequence)
        || head.activation_sequence != head.terminal_sequence
        || head.epoch != activate_ack.activation_epoch
        || activate_ack
            .prior_candidate_commitment
            .as_deref()
            .map(parse)
            .transpose()?
            != active.prior_commitment
    {
        return Err(IndexError::DigestConflict);
    }
    for ack in [publish_ack, activate_ack] {
        if ack.repo_id.as_str() != candidate.repo_id
            || ack.revision_id.as_str() != candidate.revision_id
            || ack.manifest_generation.get() != candidate.manifest_generation
            || parse(&ack.new_candidate_commitment)? != candidate.candidate_commitment
        {
            return Err(IndexError::DigestConflict);
        }
    }
    Ok(candidate)
}

/// Rejoin the two Original RepoMap terminal receipts to one current candidate
/// and activation in a held Legacy Catalog snapshot that may contain other
/// active Source and RepoMap revisions. The caller still verifies the active
/// candidate's immutable physical payload under the same root lease.
pub fn verify_legacy_active_repomap_receipts_with_siblings_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    publish: &RepoMapTerminalReceiptV2,
    activate: &RepoMapTerminalReceiptV2,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let repo = &publish.mutation.repo_id;
    let revision = &publish.mutation.revision_id;
    with_admitted_connection(
        state_root,
        io,
        budget,
        0,
        |error| error,
        |connection, path, budget, _| {
            configure_read_connection(connection, budget)?;
            idempotency::verify_installed_schema(connection, path).map_err(core)?;
            crate::sequence::verify_installed_schema(connection, path).map_err(core)?;
            crate::auxiliary::verify_installed_schema(connection, path).map_err(core)?;
            crate::candidate::verify_installed_schema(connection, path).map_err(core)?;
            let _journal = idempotency::visit_admitted_quiescent_journal_v1(
                connection,
                path,
                budget,
                &mut |_, _, _| Ok(()),
            )
            .map_err(journal_read_error)?;
            crate::sequence::verify_allocator_exact(connection, path).map_err(core)?;
            crate::integrity::verify_integrity_admitted(connection, path, budget).map_err(
                |error| match error {
                    crate::integrity::IntegrityCheckError::Admission(error) => error,
                    crate::integrity::IntegrityCheckError::Integrity(error) => core(error),
                },
            )?;
            let row_bytes: i64 = connection
                .query_row(
                    "SELECT 2 * (length(CAST(a.repo_id AS BLOB)) + length(CAST(a.revision_id AS BLOB)))
                            + COALESCE(length(CAST(a.invalidation_reason AS BLOB)), 0)
                            + COALESCE(length(CAST(c.projection_meta AS BLOB)), 0)
                            + COALESCE(length(CAST(p.invalidation_reason AS BLOB)), 0)
                            + COALESCE(length(CAST(pc.projection_meta AS BLOB)), 0)
                     FROM repomap_activation_v1 AS a
                     JOIN repomap_candidate_v1 AS c
                       ON (c.repo_id,c.revision_id,c.manifest_generation)
                        = (a.repo_id,a.revision_id,a.manifest_generation)
                     LEFT JOIN repomap_activation_v1 AS p
                       ON (p.repo_id,p.revision_id,p.epoch)
                        = (a.repo_id,a.revision_id,a.epoch - 1)
                     LEFT JOIN repomap_candidate_v1 AS pc
                       ON (pc.repo_id,pc.revision_id,pc.manifest_generation)
                        = (p.repo_id,p.revision_id,p.manifest_generation)
                     WHERE a.repo_id=?1 AND a.revision_id=?2 AND a.active=1",
                    params![repo.as_str(), revision.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| sql(&error))?
                .ok_or(IndexError::DigestConflict)?;
            let memory_bytes = u64::try_from(row_bytes)
                .map_err(|_| IndexError::ResourceLimited)?
                .checked_mul(8)
                .and_then(|bytes| bytes.checked_add(8192))
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                reads: 1,
                bytes: memory_bytes,
                ..WorkUsage::default()
            })?;
            let _row_memory = budget
                .reserve_memory(memory_bytes, MemoryLifetime::Temporary)?
                .ok_or(IndexError::ResourceLimited)?;
            let active =
                crate::candidate::read_active_recovery_candidate_v1(connection, repo, revision)
                    .map_err(core)?
                    .ok_or(IndexError::DigestConflict)?;
            let _candidate = require_active_repomap_ack_row_v1(&active, publish, activate, budget)?;
            connection
                .execute_batch("COMMIT")
                .map_err(|error| sql(&error))?;
            Ok(())
        },
    )
}

/// Complete physical ledger observation for a held Legacy Catalog. SearchCorpus
/// receipts and active RepoMap rows are borrowed only during their callbacks;
/// neither callback may issue Source authority. Counts become final only after
/// the read transaction, SQLite close and physical input check all succeed.
pub struct LegacyActiveCatalogCensusV1 {
    pub terminal_journal_rows: u64,
    pub published_search_corpus_rows: u64,
    pub active_repomap_heads: u64,
    pub last_fence: u64,
    /// Global physical History presence in the same held Catalog snapshot.
    pub has_history_rows: bool,
}

pub fn visit_legacy_active_catalog_heads_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    budget: &mut WorkBudget,
    visit_search_corpus: &mut dyn FnMut(
        &IdempotencyKeyV1,
        &BatchPublishReceipt,
        u64,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
    visit_repomap: &mut dyn FnMut(
        &crate::RepoMapCandidateRowV1,
        &crate::RepoMapActivationRowV1,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
) -> Result<LegacyActiveCatalogCensusV1, IndexError> {
    with_admitted_connection(
        state_root,
        io,
        budget,
        0,
        |error| error,
        |connection, path, budget, _| {
            configure_read_connection(connection, budget)?;
            idempotency::verify_installed_schema(connection, path).map_err(core)?;
            crate::sequence::verify_installed_schema(connection, path).map_err(core)?;
            crate::auxiliary::verify_installed_schema(connection, path).map_err(core)?;
            crate::candidate::verify_installed_schema(connection, path).map_err(core)?;
            if crate::candidate::has_unresolved_candidate_v1(connection, path).map_err(core)? {
                return Err(IndexError::Conflict);
            }
            let mut published_search_corpus_rows = 0_u64;
            let (terminal_journal_rows, last_fence) =
                idempotency::visit_admitted_quiescent_journal_v1(
                    connection,
                    path,
                    budget,
                    &mut |key, inspected, budget| {
                        if key.kind == IngestOperationKindV1::SearchCorpus
                            && let quanta_index_core::OperationInspectV1::Committed {
                                receipt,
                                durable_sequence,
                            } = inspected
                        {
                            visit_search_corpus(key, receipt, *durable_sequence, budget)
                                .map_err(idempotency::AdmittedJournalError::Admission)?;
                            published_search_corpus_rows = published_search_corpus_rows
                                .checked_add(1)
                                .ok_or(IndexError::ResourceLimited)?;
                        }
                        Ok(())
                    },
                )
                .map_err(journal_read_error)?;
            crate::sequence::verify_allocator_exact(connection, path).map_err(core)?;
            crate::integrity::verify_integrity_admitted(connection, path, budget).map_err(
                |error| match error {
                    crate::integrity::IntegrityCheckError::Admission(error) => error,
                    crate::integrity::IntegrityCheckError::Integrity(error) => core(error),
                },
            )?;

            let mut statement = connection
                .prepare(
                    "SELECT repo_id, revision_id FROM repomap_activation_v1
                     WHERE active=1 ORDER BY repo_id, revision_id",
                )
                .map_err(|error| sql(&error))?;
            let mut rows = statement.query([]).map_err(|error| sql(&error))?;
            let mut active_repomap_heads = 0_u64;
            while let Some(row) = rows.next().map_err(|error| sql(&error))? {
                budget.checkpoint()?;
                budget.charge(WorkUsage {
                    reads: 1,
                    bytes: 128,
                    ..WorkUsage::default()
                })?;
                let raw_repo = match row.get_ref(0).map_err(|error| sql(&error))? {
                    ValueRef::Text(value) => value,
                    _ => return Err(IndexError::DigestConflict),
                };
                let raw_revision = match row.get_ref(1).map_err(|error| sql(&error))? {
                    ValueRef::Text(value) => value,
                    _ => return Err(IndexError::DigestConflict),
                };
                let width = raw_repo
                    .len()
                    .checked_add(raw_revision.len())
                    .and_then(|n| n.checked_mul(8))
                    .and_then(|n| n.checked_add(4096))
                    .ok_or(IndexError::ResourceLimited)?;
                let _key_memory = budget
                    .reserve_memory(
                        u64::try_from(width).map_err(|_| IndexError::ResourceLimited)?,
                        MemoryLifetime::Temporary,
                    )?
                    .ok_or(IndexError::ResourceLimited)?;
                let repo = RepoId::new(
                    std::str::from_utf8(raw_repo).map_err(|_| IndexError::DigestConflict)?,
                )
                .map_err(|_| IndexError::DigestConflict)?;
                let revision = RevisionId::new(
                    std::str::from_utf8(raw_revision).map_err(|_| IndexError::DigestConflict)?,
                )
                .map_err(|_| IndexError::DigestConflict)?;
                let projection_bytes: i64 = connection
                    .query_row(
                        "SELECT 2 * (length(CAST(a.repo_id AS BLOB)) + length(CAST(a.revision_id AS BLOB)))
                                + COALESCE(length(CAST(a.invalidation_reason AS BLOB)), 0)
                                + COALESCE(length(CAST(c.projection_meta AS BLOB)), 0)
                                + COALESCE(length(CAST(p.invalidation_reason AS BLOB)), 0)
                                + COALESCE(length(CAST(pc.projection_meta AS BLOB)), 0)
                         FROM repomap_activation_v1 AS a
                         JOIN repomap_candidate_v1 AS c
                           ON (c.repo_id,c.revision_id,c.manifest_generation)
                            = (a.repo_id,a.revision_id,a.manifest_generation)
                         LEFT JOIN repomap_activation_v1 AS p
                           ON (p.repo_id,p.revision_id,p.epoch)
                            = (a.repo_id,a.revision_id,a.epoch - 1)
                         LEFT JOIN repomap_candidate_v1 AS pc
                           ON (pc.repo_id,pc.revision_id,pc.manifest_generation)
                            = (p.repo_id,p.revision_id,p.manifest_generation)
                         WHERE a.repo_id=?1 AND a.revision_id=?2 AND a.active=1",
                        params![repo.as_str(), revision.as_str()],
                        |row| row.get(0),
                    )
                    .map_err(|error| sql(&error))?;
                let projection_bytes = u64::try_from(projection_bytes)
                    .map_err(|_| IndexError::ResourceLimited)?
                    .checked_mul(8)
                    .and_then(|n| n.checked_add(8192))
                    .ok_or(IndexError::ResourceLimited)?;
                budget.charge(WorkUsage {
                    reads: 1,
                    bytes: projection_bytes,
                    ..WorkUsage::default()
                })?;
                let _row_memory = budget
                    .reserve_memory(projection_bytes, MemoryLifetime::Temporary)?
                    .ok_or(IndexError::ResourceLimited)?;
                let active = crate::candidate::read_active_recovery_candidate_v1(
                    connection, &repo, &revision,
                )
                .map_err(core)?
                .ok_or(IndexError::DigestConflict)?;
                if active.candidate.repo_id != repo.as_str()
                    || active.candidate.revision_id != revision.as_str()
                    || active.activation.repo_id != repo.as_str()
                    || active.activation.revision_id != revision.as_str()
                {
                    return Err(IndexError::DigestConflict);
                }
                visit_repomap(&active.candidate, &active.activation, budget)?;
                active_repomap_heads = active_repomap_heads
                    .checked_add(1)
                    .ok_or(IndexError::ResourceLimited)?;
            }
            drop(rows);
            drop(statement);
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 8,
                ..WorkUsage::default()
            })?;
            let has_history_rows: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM auxiliary_rows_v1 WHERE domain='history' LIMIT 1)",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            connection
                .execute_batch("COMMIT")
                .map_err(|error| sql(&error))?;
            Ok(LegacyActiveCatalogCensusV1 {
                terminal_journal_rows,
                published_search_corpus_rows,
                active_repomap_heads,
                last_fence: u64::try_from(last_fence).map_err(|_| IndexError::ResourceLimited)?,
                has_history_rows,
            })
        },
    )
}

/// Canonically verified Legacy SearchCorpus receipt from an existing Catalog.
/// The lease retains the result's Original memory admission.
pub struct VerifiedLegacyTerminalBatch {
    receipt: BatchPublishReceipt,
    _allocation: MemoryLease,
    _receipt_allocation: MemoryLease,
}

impl VerifiedLegacyTerminalBatch {
    #[must_use]
    pub const fn receipt(&self) -> &BatchPublishReceipt {
        &self.receipt
    }
}

/// A journal refusal retains its Core error code instead of stringifying it.
#[derive(Debug)]
pub enum TerminalBatchReadError {
    Admission(IndexError),
    Journal(CoreError),
    Close(IndexError),
    Unknown,
    NonCommitted(OperationJournalStateV1),
    ExpectedMismatch(&'static str),
}

impl From<IndexError> for TerminalBatchReadError {
    fn from(error: IndexError) -> Self {
        Self::Admission(error)
    }
}

impl std::fmt::Display for TerminalBatchReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Admission(error) => write!(formatter, "Catalog read admission: {error}"),
            Self::Journal(error) => write!(formatter, "Catalog journal: {error}"),
            Self::Close(error) => write!(formatter, "Catalog close: {error}"),
            Self::Unknown => formatter.write_str("Catalog has no terminal batch"),
            Self::NonCommitted(state) => write!(formatter, "Catalog batch is {state:?}"),
            Self::ExpectedMismatch(field) => write!(formatter, "Catalog ACK differs at {field}"),
        }
    }
}

impl std::error::Error for TerminalBatchReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Admission(error) | Self::Close(error) => Some(error),
            Self::Journal(error) => Some(error),
            Self::Unknown | Self::NonCommitted(_) | Self::ExpectedMismatch(_) => None,
        }
    }
}

/// Read the committed SearchCorpus receipt under the caller's held Legacy
/// root and physical read admission. The key's repo and revision must come
/// from the actual publish ACK's lexical scope, not a snapshot generation.
/// No Native capture marker, nonce, selection or object row is required.
pub fn verify_legacy_terminal_batch_receipt(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    key: &IdempotencyKeyV1,
    expected_ack: &BatchPublishReceipt,
    budget: &mut WorkBudget,
) -> Result<VerifiedLegacyTerminalBatch, TerminalBatchReadError> {
    if key.kind != IngestOperationKindV1::SearchCorpus {
        return Err(TerminalBatchReadError::ExpectedMismatch("kind"));
    }
    if key.generation != expected_ack.generation {
        return Err(TerminalBatchReadError::ExpectedMismatch("generation"));
    }
    if key.batch_digest != expected_ack.batch_digest {
        return Err(TerminalBatchReadError::ExpectedMismatch("batch_digest"));
    }
    with_admitted_connection(
        state_root,
        io,
        budget,
        32_768,
        TerminalBatchReadError::Close,
        |connection, path, budget, allocation| {
            let allocation = allocation.ok_or(TerminalBatchReadError::Admission(
                IndexError::ResourceLimited,
            ))?;
            configure_read_connection(connection, budget)?;
            idempotency::verify_installed_schema(connection, path)
                .map_err(TerminalBatchReadError::Journal)?;
            crate::sequence::verify_installed_schema(connection, path)
                .map_err(TerminalBatchReadError::Journal)?;
            let (receipt, row_allocation) =
                verify_terminal_receipt_in_snapshot(connection, path, key, expected_ack, budget)?;
            Ok(VerifiedLegacyTerminalBatch {
                receipt,
                _allocation: allocation,
                _receipt_allocation: row_allocation,
            })
        },
    )
}

fn verify_terminal_receipt_in_snapshot(
    connection: &Connection,
    path: &Path,
    key: &IdempotencyKeyV1,
    expected_ack: &BatchPublishReceipt,
    budget: &mut WorkBudget,
) -> Result<(BatchPublishReceipt, MemoryLease), TerminalBatchReadError> {
    let (state, inspected, row_allocation) =
        idempotency::inspect_admitted_terminal(connection, path, key, budget).map_err(|error| {
            match error {
                idempotency::AdmittedJournalError::Admission(error) => {
                    TerminalBatchReadError::Admission(error)
                }
                idempotency::AdmittedJournalError::Journal(error) => {
                    TerminalBatchReadError::Journal(error)
                }
            }
        })?;
    let state = state.ok_or(TerminalBatchReadError::Unknown)?;
    let row_allocation = row_allocation.ok_or(TerminalBatchReadError::Admission(
        IndexError::ResourceLimited,
    ))?;
    let (receipt, sequence) = match inspected {
        quanta_index_core::OperationInspectV1::Committed {
            receipt,
            durable_sequence,
        } => (receipt, durable_sequence),
        _ => return Err(TerminalBatchReadError::NonCommitted(state)),
    };
    if receipt.durable_sequence != 0 || !receipt.applied || sequence == 0 {
        return Err(TerminalBatchReadError::Journal(CoreError::Typed {
            code: SearchPlaneErrorCodeV2::CatalogRowCorrupt,
            message: "stored committed receipt has invalid pre-sequence state".into(),
        }));
    }
    let actual = receipt.recorded_at(sequence);
    macro_rules! equal {
        ($field:ident) => {
            if actual.$field != expected_ack.$field {
                return Err(TerminalBatchReadError::ExpectedMismatch(stringify!($field)));
            }
        };
    }
    equal!(generation);
    equal!(manifest_digest);
    equal!(batch_digest);
    equal!(accepted_replace_scopes);
    equal!(accepted_tombstone_scopes);
    equal!(accepted_semantic_replace_scopes);
    equal!(accepted_semantic_tombstone_scopes);
    equal!(accepted_clear_surfaces);
    equal!(sealed);
    equal!(semantic_content);
    equal!(durable_sequence);
    // `applied` is false on a genuine replay ACK; it is not custody.
    Ok((actual, row_allocation))
}

/// Check the held Legacy source Catalog without startup recovery mutations.
/// Every current journal row must be terminal and pair with its canonical
/// sequence event; no mutation lease may remain.
pub fn verify_legacy_catalog_quiescence_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    expected_repo: &RepoId,
    expected_revision: &RevisionId,
    budget: &mut WorkBudget,
) -> Result<LegacyCatalogQuiescenceV1, IndexError> {
    with_admitted_connection(
        state_root,
        io,
        budget,
        16_384,
        |error| error,
        |connection, path, budget, allocation| {
            let allocation = allocation.ok_or(IndexError::ResourceLimited)?;
            configure_read_connection(connection, budget)?;
            budget.charge(WorkUsage {
                reads: 16,
                bytes: 4096,
                ..WorkUsage::default()
            })?;
            verify_legacy_source_scope_v1(connection, path, expected_repo, expected_revision)?;
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 8,
                ..WorkUsage::default()
            })?;
            let has_history_rows: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM auxiliary_rows_v1 WHERE domain='history' LIMIT 1)",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?;
            // Read the widths of the latest activation and its candidate in
            // the same snapshot. Historical rows get their own temporary
            // grant during the ledger pass; they don't size the returned row.
            let active_repomap_bytes: i64 = connection
                .query_row(
                    "SELECT 2 * (length(CAST(a.repo_id AS BLOB)) + length(CAST(a.revision_id AS BLOB)))
                            + COALESCE(length(CAST(a.invalidation_reason AS BLOB)), 0)
                            + COALESCE(length(CAST(c.projection_meta AS BLOB)), 0)
                            + COALESCE(length(CAST(pc.projection_meta AS BLOB)), 0)
                            + COALESCE(length(CAST(p.invalidation_reason AS BLOB)), 0)
                     FROM repomap_activation_v1 AS a LEFT JOIN repomap_candidate_v1 AS c
                       ON (c.repo_id, c.revision_id, c.manifest_generation)
                        = (a.repo_id, a.revision_id, a.manifest_generation)
                     LEFT JOIN repomap_activation_v1 AS p
                       ON (p.repo_id, p.revision_id, p.epoch) = (a.repo_id, a.revision_id, a.epoch - 1)
                     LEFT JOIN repomap_candidate_v1 AS pc
                       ON (pc.repo_id, pc.revision_id, pc.manifest_generation)
                        = (p.repo_id, p.revision_id, p.manifest_generation)
                     WHERE a.repo_id = ?1 AND a.revision_id = ?2 ORDER BY a.epoch DESC LIMIT 1",
                    params![expected_repo.as_str(), expected_revision.as_str()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| sql(&error))?
                .unwrap_or(0);
            let repomap_bytes = u64::try_from(active_repomap_bytes)
                .map_err(|_| IndexError::ResourceLimited)?
                .checked_mul(4)
                .and_then(|bytes| bytes.checked_add(4096))
                .ok_or(IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                reads: 12,
                bytes: repomap_bytes,
                ..WorkUsage::default()
            })?;
            let repomap_allocation = budget
                .reserve_memory(repomap_bytes, MemoryLifetime::Retained)?
                .ok_or(IndexError::ResourceLimited)?;
            let (terminal_rows, last_fence, latest_retained_search_corpus_sequence) =
                idempotency::verify_admitted_quiescent_journal(
                    connection,
                    path,
                    expected_repo,
                    budget,
                )
                .map_err(|error| match error {
                    idempotency::AdmittedJournalError::Admission(error) => error,
                    idempotency::AdmittedJournalError::Journal(error) => core(error),
                })?;
            crate::sequence::verify_allocator_exact(connection, path).map_err(core)?;
            crate::integrity::verify_integrity_admitted(connection, path, budget).map_err(
                |error| match error {
                    crate::integrity::IntegrityCheckError::Admission(error) => error,
                    crate::integrity::IntegrityCheckError::Integrity(error) => core(error),
                },
            )?;
            let active_repomap = crate::candidate::read_active_recovery_candidate_v1(
                connection,
                expected_repo,
                expected_revision,
            )
            .map_err(core)?;
            let repomap_allocation = active_repomap.as_ref().map(|_| repomap_allocation);
            connection
                .execute_batch("COMMIT")
                .map_err(|error| engine(&error))?;
            Ok(LegacyCatalogQuiescenceV1 {
                terminal_rows,
                latest_retained_search_corpus_sequence,
                has_history_rows,
                last_fence: u64::try_from(last_fence)
                    .map_err(|_error| IndexError::ResourceLimited)?,
                active_repomap,
                _repomap_allocation: repomap_allocation,
                _allocation: allocation,
            })
        },
    )
}

fn verify_legacy_source_scope_v1(
    connection: &Connection,
    path: &Path,
    expected_repo: &RepoId,
    expected_revision: &RevisionId,
) -> Result<(), IndexError> {
    idempotency::verify_installed_schema(connection, path).map_err(core)?;
    crate::sequence::verify_installed_schema(connection, path).map_err(core)?;
    crate::auxiliary::verify_installed_schema(connection, path).map_err(core)?;
    crate::candidate::verify_installed_schema(connection, path).map_err(core)?;
    if crate::candidate::has_unresolved_source_scope(
        connection,
        path,
        expected_repo,
        expected_revision,
    )
    .map_err(core)?
    {
        return Err(IndexError::Conflict);
    }
    // Retained History may belong to an older revision of this repository.
    // Runtime, Structural and track rows still name the current Source scope.
    let foreign_domain: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM auxiliary_rows_v1
                           WHERE repo_id<>?1 OR (domain<>'history' AND revision_id<>?2) LIMIT 1)
                 OR EXISTS(SELECT 1 FROM auxiliary_tracks_v1
                           WHERE repo_id<>?1 OR revision_id<>?2 LIMIT 1)",
            params![expected_repo.as_str(), expected_revision.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    if foreign_domain {
        return Err(IndexError::Conflict);
    }
    Ok(())
}

/// Historical History reads are provisional. The selected Source and active
/// RepoMap are checked independently by recovery quiescence before BOOT.
fn verify_legacy_history_repo_scope_v1(
    connection: &Connection,
    path: &Path,
    expected_repo: &RepoId,
) -> Result<(), IndexError> {
    idempotency::verify_installed_schema(connection, path).map_err(core)?;
    crate::sequence::verify_installed_schema(connection, path).map_err(core)?;
    crate::auxiliary::verify_installed_schema(connection, path).map_err(core)?;
    crate::candidate::verify_installed_schema(connection, path).map_err(core)?;
    let foreign_or_unsettled: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM auxiliary_rows_v1 WHERE repo_id<>?1 LIMIT 1)
                 OR EXISTS(SELECT 1 FROM auxiliary_tracks_v1 WHERE repo_id<>?1 LIMIT 1)
                 OR EXISTS(SELECT 1 FROM repomap_candidate_v1
                           WHERE repo_id<>?1 OR state=1 LIMIT 1)
                 OR EXISTS(SELECT 1 FROM repomap_activation_v1 WHERE repo_id<>?1 LIMIT 1)",
            params![expected_repo.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    if foreign_or_unsettled {
        return Err(IndexError::Conflict);
    }
    Ok(())
}

/// Visit a held Legacy History generation under physical read-only admission.
/// The callback output is provisional until the transaction and physical
/// Catalog input checks finish; this reader does not issue a cut or ACK.
pub fn visit_legacy_history_snapshot_rows_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    generation: &AuxiliaryGenerationKeyV1,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(AuxiliaryRowV1, &mut WorkBudget) -> Result<(), CoreError>,
) -> Result<(), IndexError> {
    with_quiescent_legacy_history_v1(
        state_root,
        io,
        &generation.repo_id,
        budget,
        |connection, path, budget| {
            crate::auxiliary::scan_history_generation_v1(
                connection, path, generation, budget, visit,
            )
            .map_err(core)
        },
    )
}

/// Authenticated row-set and journal data from the held Legacy generation.
/// The producer issues the first replay boundary when it adopts this witness.
pub struct LegacyHistorySnapshotObservationV1 {
    pub durable_epoch: AuxEpochV1,
    pub snapshot_digest: Digest,
    pub row_count: u64,
    pub terminal_receipt: BatchPublishReceipt,
    _receipt_allocation: MemoryLease,
}

pub fn observe_legacy_history_snapshot_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    generation: &AuxiliaryGenerationKeyV1,
    budget: &mut WorkBudget,
) -> Result<LegacyHistorySnapshotObservationV1, IndexError> {
    visit_legacy_history_checkpoint_rows_v1(state_root, io, generation, budget, &mut |_, _| Ok(()))
}

/// Visit physical History rows and seal their observation with the actual
/// latest applied ACK from the same quiescent Catalog transaction. Staged
/// pages stay provisional until this function closes and validates the source.
pub fn visit_legacy_history_checkpoint_rows_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    generation: &AuxiliaryGenerationKeyV1,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(AuxiliaryRowV1, &mut WorkBudget) -> Result<(), CoreError>,
) -> Result<LegacyHistorySnapshotObservationV1, IndexError> {
    with_quiescent_legacy_history_v1(
        state_root,
        io,
        &generation.repo_id,
        budget,
        |connection, path, budget| {
            observe_history_in_snapshot(connection, path, generation, budget, visit)
        },
    )
}

fn observe_history_in_snapshot(
    connection: &Connection,
    path: &Path,
    generation: &AuxiliaryGenerationKeyV1,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(AuxiliaryRowV1, &mut WorkBudget) -> Result<(), CoreError>,
) -> Result<LegacyHistorySnapshotObservationV1, IndexError> {
    let (durable_epoch, snapshot_digest, row_count) =
        read_legacy_history_row_commitment(connection, path, generation, budget, visit)?;
    let (terminal_receipt, allocation) =
        idempotency::latest_applied_history_receipt_in_snapshot_v1(
            connection, path, generation, budget,
        )
        .map_err(journal_read_error)?
        .ok_or(IndexError::SnapshotUnavailable)?;
    Ok(LegacyHistorySnapshotObservationV1 {
        durable_epoch,
        snapshot_digest,
        row_count,
        terminal_receipt,
        _receipt_allocation: allocation,
    })
}

fn journal_read_error(error: idempotency::AdmittedJournalError) -> IndexError {
    match error {
        idempotency::AdmittedJournalError::Admission(error) => error,
        idempotency::AdmittedJournalError::Journal(error) => core(error),
    }
}

/// Producer namespace admitted by the offline handoff under retained custody.
#[derive(Clone, Copy)]
pub struct LegacyHistoryProducerScopeV1<'a> {
    pub writer_state_root: &'a str,
    pub workspace_root: &'a str,
    pub contract_key: &'a str,
}

pub enum LegacyHistorySourceProofV1<'a> {
    Replay(&'a HistorySourceInputV1),
    Snapshot {
        checkpoint: &'a HistorySnapshotCheckpointV1,
        required: Option<&'a NativeCutoverEvidenceLegacyHistoryV1>,
    },
}

/// Visit every physical History `(revision, generation)` key for one repository
/// under the same existing-root, quiescent Catalog read used by BOOT. Each key
/// is lent only for the callback; a caller retaining it must fund its own copy.
/// The returned count is valid only after the read transaction closes. This
/// census does not authenticate the rows or terminal ACK of each generation.
pub fn visit_legacy_history_generation_keys_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    repo: &RepoId,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(&AuxiliaryGenerationKeyV1, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<u64, IndexError> {
    with_quiescent_legacy_history_v1(state_root, io, repo, budget, |connection, _path, budget| {
        scan_legacy_history_generation_rows_v1(connection, repo, budget, |row, budget| {
            let raw_revision = match row.get_ref(0).map_err(|error| sql(&error))? {
                ValueRef::Text(bytes) => bytes,
                _ => return Err(IndexError::DigestConflict),
            };
            let backing = std::mem::size_of::<AuxiliaryGenerationKeyV1>()
                .checked_add(repo.as_str().len())
                .and_then(|bytes| bytes.checked_add(raw_revision.len()))
                .ok_or(IndexError::ResourceLimited)?;
            let backing = u64::try_from(backing).map_err(|_| IndexError::ResourceLimited)?;
            budget.charge(WorkUsage {
                bytes: backing,
                ..WorkUsage::default()
            })?;
            let _key_backing = budget
                .reserve_memory(backing, MemoryLifetime::Temporary)?
                .ok_or(IndexError::ResourceLimited)?;
            let revision =
                std::str::from_utf8(raw_revision).map_err(|_| IndexError::DigestConflict)?;
            let generation = uint(row.get::<_, i64>(1).map_err(|error| sql(&error))?)?;
            let key = AuxiliaryGenerationKeyV1 {
                repo_id: repo.clone(),
                revision_id: RevisionId::new(revision).map_err(|_| IndexError::DigestConflict)?,
                generation: ManifestGeneration::new(generation),
            };
            visit(&key, budget)
        })
    })
}

/// Count or lend physical keys through the same primary-key-ordered cursor.
fn scan_legacy_history_generation_rows_v1(
    connection: &Connection,
    repo: &RepoId,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(&rusqlite::Row<'_>, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<u64, IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 8,
        ..WorkUsage::default()
    })?;
    let mut statement = connection
        .prepare(
            "SELECT revision_id, generation FROM auxiliary_rows_v1
             WHERE domain='history' AND repo_id=?1
             GROUP BY revision_id, generation
             ORDER BY revision_id, generation",
        )
        .map_err(|error| sql(&error))?;
    let mut rows = statement
        .query(params![repo.as_str()])
        .map_err(|error| sql(&error))?;
    let mut count = 0_u64;
    loop {
        budget.charge(WorkUsage {
            reads: 1,
            bytes: 8,
            ..WorkUsage::default()
        })?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            break;
        };
        count = count.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        visit(row, budget)?;
    }
    Ok(count)
}

/// Match every retained History generation to its complete producer inventory
/// in one held Legacy Catalog snapshot. The caller supplies the producer
/// fence and retains root custody through the selection handoff.
pub fn verify_legacy_history_sources_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    repo: &RepoId,
    revision: &RevisionId,
    producer: LegacyHistoryProducerScopeV1<'_>,
    sources: &[(&HistorySourceCutV1, &HistorySourceInputV1)],
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    verify_legacy_history_source_inventory_v1(
        state_root,
        io,
        repo,
        producer,
        budget,
        |budget, verify| {
            for (cut, input) in sources {
                if &cut.generation.revision_id != revision {
                    return Err(IndexError::DigestConflict);
                }
                verify(cut, LegacyHistorySourceProofV1::Replay(input), budget)?;
            }
            Ok(())
        },
    )
}

/// Stream the ordered generation proofs under one held Legacy Catalog read.
/// The source callback lends one carrier at a time to the independent census.
pub fn verify_legacy_history_source_inventory_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    repo: &RepoId,
    producer: LegacyHistoryProducerScopeV1<'_>,
    budget: &mut WorkBudget,
    sources: impl FnOnce(
        &mut WorkBudget,
        &mut LegacyHistorySourceVerifier<'_>,
    ) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    with_quiescent_legacy_history_v1(state_root, io, repo, budget, |connection, path, budget| {
        let retained_count =
            scan_legacy_history_generation_rows_v1(connection, repo, budget, |_, _| Ok(()))?;
        let mut count = 0u64;
        let mut previous: Option<(ManifestGeneration, RevisionId, MemoryLease)> = None;
        sources(budget, &mut |cut, input, budget| {
            if count == retained_count
                || &cut.generation.repo_id != repo
                || previous.as_ref().is_some_and(|(generation, revision, _)| {
                    (*generation, revision) >= cut.generation.bootstrap_order_key_v1()
                })
            {
                return Err(IndexError::DigestConflict);
            }
            drop(previous.take());
            match input {
                LegacyHistorySourceProofV1::Replay(input) => {
                    verify_legacy_history_source_v1(
                        connection, path, producer, cut, input, budget,
                    )?;
                }
                LegacyHistorySourceProofV1::Snapshot {
                    checkpoint,
                    required,
                } => {
                    let legacy_root = state_root.to_str().ok_or(IndexError::DigestConflict)?;
                    verify_legacy_history_checkpoint(
                        connection,
                        path,
                        [
                            producer.writer_state_root,
                            producer.workspace_root,
                            producer.contract_key,
                            legacy_root,
                        ],
                        cut,
                        checkpoint,
                        required,
                        budget,
                    )?;
                }
            }
            let revision = &cut.generation.revision_id;
            let revision_bytes = std::mem::size_of::<RevisionId>()
                .checked_add(revision.as_str().len())
                .ok_or(IndexError::ResourceLimited)?;
            let allocation = budget
                .reserve_memory(
                    u64::try_from(revision_bytes).map_err(|_| IndexError::ResourceLimited)?,
                    MemoryLifetime::Retained,
                )?
                .ok_or(IndexError::ResourceLimited)?;
            previous = Some((cut.generation.generation, revision.clone(), allocation));
            count += 1;
            Ok(())
        })?;
        if count != retained_count {
            return Err(IndexError::DigestConflict);
        }
        Ok(())
    })
}

type LegacyHistorySourceVerifier<'a> = dyn FnMut(
        &HistorySourceCutV1,
        LegacyHistorySourceProofV1<'_>,
        &mut WorkBudget,
    ) -> Result<(), IndexError>
    + 'a;

fn verify_legacy_history_checkpoint(
    connection: &Connection,
    path: &Path,
    scope: [&str; 4],
    cut: &HistorySourceCutV1,
    checkpoint: &HistorySnapshotCheckpointV1,
    required: Option<&NativeCutoverEvidenceLegacyHistoryV1>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    checkpoint
        .validate_shape()
        .map_err(|error| IndexError::InvalidInput(error.into()))?;
    if checkpoint.cut != *cut {
        return Err(IndexError::DigestConflict);
    }
    let actual = observe_history_in_snapshot(
        connection,
        path,
        &cut.generation,
        budget,
        &mut |_, _| Ok(()),
    )?;
    if actual.durable_epoch != cut.durable_epoch
        || actual.snapshot_digest != cut.snapshot_digest
        || actual.row_count != checkpoint.row_count
        || actual.terminal_receipt != checkpoint.terminal_ack
    {
        return Err(IndexError::DigestConflict);
    }
    let replay = quanta_index_ipc::history_snapshot_checkpoint_replay_with_progress_v1(
        scope,
        &cut.generation,
        actual.durable_epoch,
        actual.snapshot_digest,
        &actual.terminal_receipt,
        &mut |bytes| {
            budget.charge(WorkUsage {
                bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })
        },
    )
    .map_err(history_hash_error)?;
    if replay != cut.replay_start {
        return Err(IndexError::DigestConflict);
    }
    if let Some(required) = required {
        verify_required_checkpoint_ack(connection, path, cut, required, budget)?;
    }
    Ok(())
}

fn verify_required_checkpoint_ack(
    connection: &Connection,
    path: &Path,
    cut: &HistorySourceCutV1,
    required: &NativeCutoverEvidenceLegacyHistoryV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    if required.generation != cut.generation {
        return Err(IndexError::DigestConflict);
    }
    let key_bytes = cut
        .generation
        .repo_id
        .as_str()
        .len()
        .saturating_add(cut.generation.revision_id.as_str().len())
        .saturating_add(required.batch_digest.len());
    let _memory = budget.reserve_memory(
        u64::try_from(key_bytes).map_err(|_| IndexError::ResourceLimited)?,
        MemoryLifetime::Temporary,
    )?;
    let key = IdempotencyKeyV1 {
        kind: IngestOperationKindV1::History,
        repo_id: cut.generation.repo_id.clone(),
        revision_id: cut.generation.revision_id.clone(),
        generation: cut.generation.generation,
        batch_digest: required.batch_digest.clone(),
    };
    let (_, receipt, _receipt_memory) =
        idempotency::inspect_admitted_terminal(connection, path, &key, budget)
            .map_err(journal_read_error)?;
    if !matches!(receipt, quanta_index_core::OperationInspectV1::Committed { receipt, durable_sequence }
        if durable_sequence == required.durable_sequence && receipt.applied
            && receipt.generation == key.generation && receipt.batch_digest == key.batch_digest)
    {
        return Err(IndexError::DigestConflict);
    }
    Ok(())
}

fn verify_legacy_history_source_v1(
    connection: &Connection,
    path: &Path,
    producer: LegacyHistoryProducerScopeV1<'_>,
    cut: &HistorySourceCutV1,
    input: &HistorySourceInputV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    input
        .validate_shape()
        .map_err(|error| IndexError::InvalidInput(error.into()))?;
    let fence = &input.fence;
    if fence.generation != cut.generation
        || fence.final_replay != cut.replay_start
        || fence.publication_count != cut.durable_epoch.get()
        || u64::try_from(input.publications.len()).ok() != Some(fence.publication_count)
    {
        return Err(IndexError::DigestConflict);
    }
    let mut replay = fence.first_replay;
    let mut sequence = 0;
    for (index, publication) in input.publications.iter().enumerate() {
        budget.checkpoint()?;
        let batch = &publication.batch;
        let ack = &publication.ack;
        if publication
            .ordinal
            .checked_sub(1)
            .and_then(|ordinal| usize::try_from(ordinal).ok())
            != Some(index)
            || publication.before != replay
            || publication.after == replay
            || publication.after.0 == [0; 32]
            || ack.durable_sequence <= sequence
            || batch.repo_id != cut.generation.repo_id
            || batch.revision_id != cut.generation.revision_id
            || batch.generation != cut.generation.generation
            || batch.manifest_digest != fence.manifest_digest
            || batch.generation != ack.generation
            || batch.batch_digest != ack.batch_digest
            || batch.manifest_digest != ack.manifest_digest
        {
            return Err(IndexError::DigestConflict);
        }
        let mut progress = |bytes: usize| {
            budget.charge(WorkUsage {
                bytes: u64::try_from(bytes).map_err(|_| IndexError::ResourceLimited)?,
                ..WorkUsage::default()
            })
        };
        if index == 0 {
            let genesis = quanta_index_ipc::history_replay_genesis_with_progress_v1(
                [
                    producer.writer_state_root,
                    producer.workspace_root,
                    producer.contract_key,
                    cut.generation.repo_id.as_str(),
                    cut.generation.revision_id.as_str(),
                ],
                batch,
                ack,
                &mut progress,
            )
            .map_err(history_hash_error)?;
            if genesis != fence.first_replay {
                return Err(IndexError::DigestConflict);
            }
        }
        let digest =
            quanta_index_ipc::canonical_batch_digest_with_progress_v1(batch, &mut progress)
                .map_err(history_hash_error)?;
        if !quanta_index_ipc::matches_batch_digest_token_v1(&batch.batch_digest, &digest) {
            return Err(IndexError::DigestConflict);
        }
        let next_replay = quanta_index_ipc::history_replay_advance_with_progress_v1(
            replay,
            publication.ordinal,
            batch,
            ack,
            &mut progress,
        )
        .map_err(history_hash_error)?;
        if next_replay != publication.after {
            return Err(IndexError::DigestConflict);
        }
        let key_bytes = batch
            .repo_id
            .as_str()
            .len()
            .saturating_add(batch.revision_id.as_str().len())
            .saturating_add(batch.batch_digest.len());
        let _key = budget.reserve_memory(
            u64::try_from(key_bytes).map_err(|_| IndexError::ResourceLimited)?,
            MemoryLifetime::Temporary,
        )?;
        let key = IdempotencyKeyV1 {
            kind: IngestOperationKindV1::History,
            repo_id: batch.repo_id.clone(),
            revision_id: batch.revision_id.clone(),
            generation: batch.generation,
            batch_digest: batch.batch_digest.clone(),
        };
        let _verified = verify_terminal_receipt_in_snapshot(connection, path, &key, ack, budget)
            .map_err(|error| match error {
                TerminalBatchReadError::Admission(error) | TerminalBatchReadError::Close(error) => {
                    error
                }
                TerminalBatchReadError::Journal(error) => core(error),
                _ => IndexError::DigestConflict,
            })?;
        replay = next_replay;
        sequence = ack.durable_sequence;
    }
    if replay != fence.final_replay || sequence != fence.final_durable_sequence {
        return Err(IndexError::DigestConflict);
    }
    let (epoch, digest, _) = read_legacy_history_row_commitment(
        connection,
        path,
        &cut.generation,
        budget,
        &mut |_, _| Ok(()),
    )?;
    if epoch != cut.durable_epoch || digest != cut.snapshot_digest {
        return Err(IndexError::DigestConflict);
    }
    Ok(())
}

fn read_legacy_history_row_commitment(
    connection: &Connection,
    path: &Path,
    generation: &AuxiliaryGenerationKeyV1,
    budget: &mut WorkBudget,
    visit: &mut dyn FnMut(AuxiliaryRowV1, &mut WorkBudget) -> Result<(), CoreError>,
) -> Result<(AuxEpochV1, Digest, u64), IndexError> {
    let mut hash = HistorySnapshotDigestV1::new(generation);
    let mut epoch = None;
    let mut count = 0_u64;
    crate::auxiliary::scan_history_generation_v1(
        connection,
        path,
        generation,
        budget,
        &mut |row, budget| {
            count = count
                .checked_add(1)
                .ok_or_else(|| CoreError::Storage("Legacy History row count overflow".into()))?;
            if row.key.family == AuxiliaryRowFamilyV1::Epoch {
                if epoch.is_some() || !row.key.row_key.is_empty() {
                    return Err(CoreError::Storage(
                        "invalid Legacy History epoch row".into(),
                    ));
                }
                let value: AuxEpochV1 = quanta_index_ipc::decode_cbor_payload(&row.value)
                    .map_err(|error| CoreError::Storage(error.to_string()))?;
                epoch = Some(value);
            }
            hash.push_row(row.key.family, &row.key.row_key, &row.value);
            visit(row, budget)
        },
    )
    .map_err(core)?;
    Ok((
        epoch.ok_or(IndexError::SnapshotUnavailable)?,
        hash.finish(),
        count,
    ))
}

fn history_hash_error(
    error: quanta_index_ipc::CanonicalBatchDigestProgressError<IndexError>,
) -> IndexError {
    match error {
        quanta_index_ipc::CanonicalBatchDigestProgressError::Progress(error) => error,
        quanta_index_ipc::CanonicalBatchDigestProgressError::Encode(error) => {
            IndexError::Storage(error.to_string())
        }
    }
}

fn with_quiescent_legacy_history_v1<T>(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    repo: &RepoId,
    budget: &mut WorkBudget,
    inspect: impl FnOnce(&Connection, &Path, &mut WorkBudget) -> Result<T, IndexError>,
) -> Result<T, IndexError> {
    with_admitted_connection(
        state_root,
        io,
        budget,
        0,
        |error| error,
        |connection, path, budget, _allocation| {
            configure_read_connection(connection, budget)?;
            budget.charge(WorkUsage {
                reads: 16,
                bytes: 4096,
                ..WorkUsage::default()
            })?;
            verify_legacy_history_repo_scope_v1(connection, path, repo)?;
            let _quiescent_journal =
                idempotency::verify_admitted_quiescent_journal(connection, path, repo, budget)
                    .map_err(|error| match error {
                        idempotency::AdmittedJournalError::Admission(error) => error,
                        idempotency::AdmittedJournalError::Journal(error) => core(error),
                    })?;
            crate::sequence::verify_allocator_exact(connection, path).map_err(core)?;
            crate::integrity::verify_integrity_admitted(connection, path, budget).map_err(
                |error| match error {
                    crate::integrity::IntegrityCheckError::Admission(error) => error,
                    crate::integrity::IntegrityCheckError::Integrity(error) => core(error),
                },
            )?;
            let value = inspect(connection, path, budget)?;
            connection
                .execute_batch("COMMIT")
                .map_err(|error| engine(&error))?;
            Ok(value)
        },
    )
}

#[derive(Clone, Copy)]
enum RollbackCheck<'a> {
    None,
    Expected(&'a NativeBootManifest),
    Selected,
    SelectedWithRoutes(&'a NativeProtectedSourceRoutesV1),
    SelectedWithHistory(OperationKey),
    SelectedWithHistoryRoutes(OperationKey, &'a NativeProtectedSourceRoutesV1),
}

type Verification = (
    NativeBootCatalogSnapshot,
    Option<NativeBootManifest>,
    Option<(ImmutableOperation, CatalogView)>,
    Option<(ImmutableOperation, CatalogView)>,
);

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    bytes: u64,
    modified_seconds: i64,
    modified_nanos: i64,
}

fn input(path: &Path, optional: bool) -> Result<Option<FileIdentity>, IndexError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(IndexError::Storage(format!(
                "inspect Catalog input: {error}"
            )));
        }
    };
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(IndexError::Storage(
            "Catalog verification input must be a private, owned, single-link regular file".into(),
        ));
    }
    Ok(Some(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        bytes: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanos: metadata.mtime_nsec(),
    }))
}

fn core(error: CoreError) -> IndexError {
    match error {
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
            ..
        } => IndexError::RebuildRequired,
        CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogBusy,
            ..
        } => IndexError::Conflict,
        other @ (CoreError::InvalidContract(_)
        | CoreError::Typed { .. }
        | CoreError::NotReady(_)
        | CoreError::NotImplemented(_)
        | CoreError::NotFound(_)
        | CoreError::Storage(_)
        | CoreError::StorageIo { .. }) => IndexError::Storage(other.to_string()),
    }
}

fn engine(error: &rusqlite::Error) -> IndexError {
    if matches!(error, rusqlite::Error::SqliteFailure(failure, _)
        if matches!(failure.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked))
    {
        IndexError::Conflict
    } else {
        sql(error)
    }
}

/// Percent-encode raw filesystem bytes; a filename cannot inject URI options.
fn main_only_uri(path: &Path) -> String {
    let mut uri = String::from("file:");
    for byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(byte) {
            uri.push(char::from(*byte));
        } else {
            uri.push('%');
            // Nibbles are always in range, without unchecked indexing.
            for nibble in [*byte >> 4, *byte & 15] {
                let digit = if nibble < 10 {
                    b'0'.wrapping_add(nibble)
                } else {
                    b'A'.wrapping_add(nibble.wrapping_sub(10))
                };
                uri.push(char::from(digit));
            }
        }
    }
    uri.push_str("?immutable=1");
    uri
}

/// Verify existing Catalog metadata without mutation.
///
/// Read schema, nonce, selected publication/custody and saved namespace binding.
/// The mandatory I/O owner retains the existing root
/// lease, authenticates native file opens and forbids every source mutation.
/// No fallback/default VFS, CREATE, recovery, allocator seed or marker write.
///
/// `SQLite` needs a write-capable main FD for its exclusive POSIX lock. The VFS
/// denies writes on that handle even outside this scope. Exclusive locking
/// rebuilds the WAL index in memory, including crash-left committed frames;
/// checkpoint-on-close is disabled before the first read. Only when the WAL is
/// absent or zero bytes does the native-locked main-only URI avoid `SQLite`'s
/// attempt to create an empty WAL. Nonempty WAL is never hidden. Input identity
/// is rechecked after close, so a WAL born during acquisition refuses the result.
pub fn verify_native_boot_catalog(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    budget: &mut WorkBudget,
) -> Result<NativeBootCatalogSnapshot, IndexError> {
    Ok(verify(
        state_root,
        io,
        None,
        RollbackCheck::None,
        None,
        None,
        budget,
    )?
    .0)
}

/// Check the Catalog side of a rollback while the caller retains its root lease.
/// The producer fence and cross-store delivery reconciliation are separate authorities.
pub fn verify_native_rollback_catalog(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    baseline: &NativeBootManifest,
    budget: &mut WorkBudget,
) -> Result<NativeBootCatalogSnapshot, IndexError> {
    Ok(verify(
        state_root,
        io,
        None,
        RollbackCheck::Expected(baseline),
        None,
        None,
        budget,
    )?
    .0)
}

/// Observe one offline Native root and require every retained operation/work
/// to be terminal. Unlike rollback, published views after the selected BOOT
/// are permitted through the actual current head in the expected workspace.
/// Historical epochs remain valid within this same root. This is Catalog
/// data, not a Source fence or a cutover permit.
pub fn verify_native_source_root_quiescence_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    expected_workspace: WorkspaceKey,
    budget: &mut WorkBudget,
) -> Result<NativeBootQuiescentCatalogV1, IndexError> {
    verify_native_source_root_quiescence_with_visitor_v1(
        state_root,
        io,
        expected_workspace,
        None,
        budget,
        None,
    )
}

/// Observe an offline selected Native root with every protected workspace
/// authenticated against its exact currently selected route head.
pub fn verify_native_source_root_quiescence_with_routes_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    expected_workspace: WorkspaceKey,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
) -> Result<NativeBootQuiescentCatalogV1, IndexError> {
    verify_native_source_root_quiescence_with_visitor_v1(
        state_root,
        io,
        expected_workspace,
        Some(routes),
        budget,
        None,
    )
}

/// Visit every actually Published operation from the held root after the
/// complete canonical quiescence scan has succeeded. Each callback receives
/// the same transaction's BOOT stamp and current head; it must independently
/// join Source custody and cannot treat this Catalog DATA as a permit. Callback
/// observations are incomplete until this function returns `Ok`, including
/// its final close and input-identity checks.
pub fn verify_native_source_root_publications_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    expected_workspace: WorkspaceKey,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(
        &NativeBootCatalogSnapshot,
        &CatalogView,
        &ImmutableOperation,
        &CatalogView,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
) -> Result<NativeBootQuiescentCatalogV1, IndexError> {
    verify_native_source_root_quiescence_with_visitor_v1(
        state_root,
        io,
        expected_workspace,
        None,
        budget,
        Some(&mut visit),
    )
}

/// Visit all durable Published rows from every selected protected workspace.
/// The route body must first be physically authenticated by the caller.
pub fn verify_native_source_root_publications_with_routes_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    expected_workspace: WorkspaceKey,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(
        &NativeBootCatalogSnapshot,
        &CatalogView,
        &ImmutableOperation,
        &CatalogView,
        &mut WorkBudget,
    ) -> Result<(), IndexError>,
) -> Result<NativeBootQuiescentCatalogV1, IndexError> {
    verify_native_source_root_quiescence_with_visitor_v1(
        state_root,
        io,
        expected_workspace,
        Some(routes),
        budget,
        Some(&mut visit),
    )
}

type PublishedVisitorV1<'a> = dyn FnMut(
        &NativeBootCatalogSnapshot,
        &CatalogView,
        &ImmutableOperation,
        &CatalogView,
        &mut WorkBudget,
    ) -> Result<(), IndexError>
    + 'a;

fn verify_native_source_root_quiescence_with_visitor_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    expected_workspace: WorkspaceKey,
    routes: Option<&NativeProtectedSourceRoutesV1>,
    budget: &mut WorkBudget,
    mut visit: Option<&mut PublishedVisitorV1<'_>>,
) -> Result<NativeBootQuiescentCatalogV1, IndexError> {
    with_admitted_connection(
        state_root,
        io,
        budget,
        32_768,
        |error| error,
        |connection, path, budget, allocation| {
            let (snapshot, _, _, _) = inspect(
                connection,
                path,
                None,
                RollbackCheck::None,
                None,
                None,
                budget,
                allocation,
            )?;
            let selected = snapshot
                .selected
                .as_ref()
                .ok_or(IndexError::SnapshotUnavailable)?;
            let current_head = if let Some(routes) = routes {
                if selected.operation.key.workspace != expected_workspace
                    || selected.publication.identity.workspace != expected_workspace
                {
                    return Err(IndexError::DigestConflict);
                }
                let route_ref = selected
                    .protected_source_routes
                    .ok_or(IndexError::SnapshotUnavailable)?;
                if usize::try_from(route_ref.count).ok() != Some(routes.heads.len()) {
                    return Err(IndexError::DigestConflict);
                }
                boot_manifest::validate_custody(connection, selected, budget)?;
                let _verified = boot_manifest::validate_protected_routes(
                    connection,
                    selected.operation.key,
                    route_ref.object,
                    routes,
                    true,
                    None,
                    budget,
                )?;
                let head = retention::charged_current_view(connection, expected_workspace, budget)?
                    .ok_or(IndexError::SnapshotUnavailable)?;
                head
            } else {
                check_source_root_quiescence(connection, selected, expected_workspace, budget)?
            };
            if let Some(visit) = visit.as_deref_mut() {
                visit_quiescent_published_rows_with_routes_v1(
                    connection,
                    expected_workspace,
                    routes,
                    budget,
                    |operation, publication, budget| {
                        let head = routes
                            .and_then(|routes| {
                                routes
                                    .heads
                                    .iter()
                                    .find(|head| {
                                        head.operation.key.workspace == operation.key.workspace
                                    })
                                    .map(|head| &head.publication)
                                    .or_else(|| {
                                        routes
                                            .repomap_heads
                                            .iter()
                                            .find(|head| {
                                                head.operation.key.workspace
                                                    == operation.key.workspace
                                            })
                                            .map(|head| &head.publication)
                                    })
                            })
                            .unwrap_or(&current_head);
                        visit(&snapshot, head, operation, publication, budget)
                    },
                )?;
            }
            Ok(NativeBootQuiescentCatalogV1 {
                snapshot,
                current_head,
            })
        },
    )
}

/// The first pass checked every operation, work row, foreign scope and floor.
/// This second pass borrows each actual Published row without building a new
/// retained inventory. The same SQLite transaction prevents an intervening
/// head change; close and file-input checks still run before success returns.
#[cfg(test)]
pub(super) fn visit_quiescent_published_rows_v1(
    connection: &Connection,
    workspace: WorkspaceKey,
    budget: &mut WorkBudget,
    visit: impl FnMut(&ImmutableOperation, &CatalogView, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    visit_quiescent_published_rows_with_routes_v1(connection, workspace, None, budget, visit)
}

pub(super) fn visit_quiescent_published_rows_with_routes_v1(
    connection: &Connection,
    workspace: WorkspaceKey,
    routes: Option<&NativeProtectedSourceRoutesV1>,
    budget: &mut WorkBudget,
    mut visit: impl FnMut(&ImmutableOperation, &CatalogView, &mut WorkBudget) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let mut statement = connection
        .prepare("SELECT operation FROM native_operation_v1 ORDER BY operation_id")
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    loop {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            break;
        };
        let ValueRef::Blob(key) = row.get_ref(0).map_err(|error| sql(&error))? else {
            return Err(IndexError::Storage(
                "Native publication operation key is not a blob".into(),
            ));
        };
        if key.is_empty() || key.len() > 4096 {
            return Err(IndexError::Storage(
                "Native publication operation key is out of bounds".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let key: OperationKey = decode(key)?;
        let (_row_loan, observed) = read_quiescent_operation_v1(connection, key, routes, budget)?;
        if observed.immutable.key.workspace != workspace
            && !routes.is_some_and(|routes| {
                routes
                    .heads
                    .iter()
                    .any(|head| head.operation.key.workspace == observed.immutable.key.workspace)
                    || routes.repomap_heads.iter().any(|head| {
                        head.operation.key.workspace == observed.immutable.key.workspace
                    })
            })
        {
            return Err(IndexError::Conflict);
        }
        if matches!(&observed.status, OperationStatus::Published(_)) {
            let (operation, publication) = published_quiescent_row_v1(observed)?;
            visit(&operation, &publication, budget)?;
            budget.checkpoint()?;
        } else if !matches!(&observed.status, OperationStatus::Refused(_)) {
            return Err(IndexError::Conflict);
        }
    }
    budget.checkpoint()
}

/// Inspect a published candidate alongside the persisted selection in one read transaction.
///
/// The candidate is logical metadata only: callers authenticate its objects before a later mutable
/// selection under the same state-root lease.
pub fn verify_native_boot_candidate(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    operation: OperationKey,
    budget: &mut WorkBudget,
) -> Result<NativeBootCatalogCandidate, IndexError> {
    let (snapshot, candidate, _, _) = verify(
        state_root,
        io,
        Some(operation),
        RollbackCheck::None,
        None,
        None,
        budget,
    )?;
    let manifest = candidate.ok_or_else(|| {
        IndexError::BackendViolation("requested boot candidate was not read".into())
    })?;
    Ok(NativeBootCatalogCandidate { manifest, snapshot })
}

/// Candidate BOOT requires the complete selected publication to be the
/// charged current Catalog head and a quiescent root in the same read-only
/// snapshot. A historical Published operation is insufficient, including an
/// exact retry.
pub fn verify_native_boot_current_candidate(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    operation: OperationKey,
    budget: &mut WorkBudget,
) -> Result<NativeBootCatalogCandidate, IndexError> {
    with_admitted_connection(
        state_root,
        io,
        budget,
        32_768,
        |error| error,
        |connection, path, budget, allocation| {
            let (snapshot, candidate, _, _) = inspect(
                connection,
                path,
                Some(operation),
                RollbackCheck::None,
                None,
                None,
                budget,
                allocation,
            )?;
            let manifest = candidate.ok_or(IndexError::SnapshotUnavailable)?;
            let current = retention::charged_current_view(connection, operation.workspace, budget)?
                .ok_or(IndexError::SnapshotUnavailable)?;
            if current != manifest.publication {
                return Err(IndexError::Conflict);
            }
            check_quiescent_rows(connection, operation.workspace, current.identity, budget)?;
            Ok(NativeBootCatalogCandidate { manifest, snapshot })
        },
    )
}

/// Inspect only the retained operation row using the existing read-only
/// connection, authenticated VFS and Original budget. No old request/view
/// object is required for an already Published operation.
pub fn verify_native_published_operation(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    operation: OperationKey,
    budget: &mut WorkBudget,
) -> Result<NativeBootPublishedOperation, IndexError> {
    let (snapshot, _, published, additional) = verify(
        state_root,
        io,
        None,
        RollbackCheck::None,
        Some(operation),
        None,
        budget,
    )?;
    let (operation, publication) = published.ok_or_else(|| {
        IndexError::BackendViolation("requested Published operation was not read".into())
    })?;
    Ok(NativeBootPublishedOperation {
        operation,
        publication,
        additional,
        snapshot,
    })
}

/// Resolve an exact historical view only through its durable Published
/// operation. Query-view retention is deliberately not used: the view row may
/// already have retired while the operation's publication remains durable.
pub fn verify_native_published_view_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    expected: ViewRef,
    budget: &mut WorkBudget,
) -> Result<CatalogView, IndexError> {
    verify_native_published_view_impl_v1(state_root, io, expected, None, budget)
}

/// Resolve a historical Published view on an authenticated selected Native
/// route. The caller verifies the immutable route object in the held root;
/// this read rechecks its selected Catalog relation and complete quiescence.
pub fn verify_native_published_view_with_routes_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    expected: ViewRef,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
) -> Result<CatalogView, IndexError> {
    verify_native_published_view_impl_v1(state_root, io, expected, Some(routes), budget)
}

fn verify_native_published_view_impl_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    expected: ViewRef,
    routes: Option<&NativeProtectedSourceRoutesV1>,
    budget: &mut WorkBudget,
) -> Result<CatalogView, IndexError> {
    with_admitted_connection(
        state_root,
        io,
        budget,
        0,
        |error| error,
        |connection, path, budget, _| {
            configure_read_connection(connection, budget)?;
            if capture_format::preflight_capture_format(connection, path).map_err(core)? {
                return Err(IndexError::RebuildRequired);
            }
            let _stamp = capture_format::verify_stamp(connection, path).map_err(core)?;
            verify_native_published_view_connection_v1(connection, expected, routes, budget)
        },
    )
}

pub(super) fn verify_native_published_view_connection_v1(
    connection: &Connection,
    expected: ViewRef,
    routes: Option<&NativeProtectedSourceRoutesV1>,
    budget: &mut WorkBudget,
) -> Result<CatalogView, IndexError> {
    if let Some(routes) = routes {
        let selected =
            boot_manifest::read_stored(connection)?.ok_or(IndexError::SnapshotUnavailable)?;
        boot_manifest::validate_custody(connection, &selected, budget)?;
        let binding = selected
            .protected_source_routes
            .ok_or(IndexError::SnapshotUnavailable)?;
        if usize::try_from(binding.count).ok() != Some(routes.heads.len()) {
            return Err(IndexError::DigestConflict);
        }
        let _validated = boot_manifest::validate_protected_routes(
            connection,
            selected.operation.key,
            binding.object,
            routes,
            true,
            None,
            budget,
        )?;
        if expected.workspace != selected.operation.key.workspace
            && !routes
                .heads
                .iter()
                .any(|head| head.operation.key.workspace == expected.workspace)
            && !routes
                .repomap_heads
                .iter()
                .any(|head| head.operation.key.workspace == expected.workspace)
        {
            return Err(IndexError::SnapshotUnavailable);
        }
    }
    let mut statement = connection
        .prepare("SELECT operation FROM native_operation_v1 ORDER BY operation_id")
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    let mut found = None;
    loop {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            break;
        };
        let ValueRef::Blob(key_bytes) = row.get_ref(0).map_err(|error| sql(&error))? else {
            return Err(IndexError::Storage(
                "Native operation key type mismatch".into(),
            ));
        };
        if key_bytes.is_empty() || key_bytes.len() > 4096 {
            return Err(IndexError::Storage(
                "Native operation key size mismatch".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let key: OperationKey = decode(key_bytes)?;
        if key.workspace != expected.workspace || key.epoch != expected.epoch {
            continue;
        }
        let (_row_loan, observed) = read_quiescent_operation_v1(connection, key, routes, budget)?;
        if observed.status == OperationStatus::Published(expected) {
            let (_, publication) = if routes.is_some() {
                published_quiescent_row_v1(observed)?
            } else {
                read_published_operation(connection, key, budget)?
            };
            if publication.identity != expected || found.replace(publication).is_some() {
                return Err(IndexError::DigestConflict);
            }
        }
    }
    budget.checkpoint()?;
    found.ok_or(IndexError::SnapshotUnavailable)
}

/// Validate a physically decoded selected route set against the persisted
/// Catalog selection and every durable Published/Exact row. The caller first
/// verifies the immutable object bytes and signed producer scope.
pub fn verify_native_protected_source_routes_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    verify_native_protected_source_routes_inner_v1(state_root, io, manifest, routes, false, budget)
}

/// Recheck every selected route against the current workspace heads and
/// quiescent Catalog state immediately before candidate selection.
pub fn verify_native_protected_source_routes_if_current_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    verify_native_protected_source_routes_inner_v1(state_root, io, manifest, routes, true, budget)
}

fn verify_native_protected_source_routes_inner_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    require_current: bool,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    with_admitted_connection(
        state_root,
        io,
        budget,
        0,
        |error| error,
        |connection, path, budget, _| {
            configure_read_connection(connection, budget)?;
            if capture_format::preflight_capture_format(connection, path).map_err(core)? {
                return Err(IndexError::RebuildRequired);
            }
            let _stamp = capture_format::verify_stamp(connection, path).map_err(core)?;
            if boot_manifest::read_stored(connection)?.as_ref() != Some(manifest) {
                return Err(IndexError::DigestConflict);
            }
            boot_manifest::validate_custody(connection, manifest, budget)?;
            let selected = manifest
                .protected_source_routes
                .ok_or(IndexError::SnapshotUnavailable)?;
            if usize::try_from(selected.count).ok() != Some(routes.heads.len()) {
                return Err(IndexError::DigestConflict);
            }
            let _verified_routes = boot_manifest::validate_protected_routes(
                connection,
                manifest.operation.key,
                selected.object,
                routes,
                require_current,
                None,
                budget,
            )?;
            budget.checkpoint()
        },
    )
}

/// Resolve a source revision to its exact selected head only after the
/// immutable route body and Catalog selection have been rejoined. This does
/// not infer a workspace key from the repository or revision spelling.
pub fn lookup_native_protected_source_head_v1<'a>(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    manifest: &NativeBootManifest,
    routes: &'a NativeProtectedSourceRoutesV1,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    budget: &mut WorkBudget,
) -> Result<&'a NativeProtectedSourceHeadV1, IndexError> {
    verify_native_protected_source_routes_v1(state_root, io, manifest, routes, budget)?;
    routes
        .heads
        .iter()
        .find(|head| &head.repo_id == repo_id && &head.revision_id == revision_id)
        .ok_or(IndexError::SnapshotUnavailable)
}

/// Resolve one protected pair while it is still a current head. The selected
/// manifest equality and every route's Catalog relation are checked in the
/// same admitted read-only transaction before this exact pair is returned.
pub fn resolve_native_protected_source_head_if_current_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    manifest: &NativeBootManifest,
    routes: &NativeProtectedSourceRoutesV1,
    repo_id: &RepoId,
    revision_id: &RevisionId,
    budget: &mut WorkBudget,
) -> Result<CatalogView, IndexError> {
    verify_native_protected_source_routes_if_current_v1(state_root, io, manifest, routes, budget)?;
    routes
        .heads
        .iter()
        .find(|head| &head.repo_id == repo_id && &head.revision_id == revision_id)
        .map(|head| head.publication.clone())
        .ok_or(IndexError::SnapshotUnavailable)
}

/// Observe the Original FULL and the independently keyed current Native
/// SourceAdvance publication with the installed selection under one read-only
/// Catalog snapshot. Equal keys cause one SQL lookup; unequal keys require two
/// actual Published rows. Physical object and Source/Host custody stay with the
/// caller's existing authorities.
pub fn verify_native_rollback_with_source_publications(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    full_operation: OperationKey,
    current_operation: OperationKey,
    budget: &mut WorkBudget,
) -> Result<NativeBootPublishedOperation, IndexError> {
    verify_rollback_source_publications(
        state_root,
        io,
        full_operation,
        current_operation,
        RollbackCheck::Selected,
        budget,
    )
}

/// Observe all operations between the authenticated Source/AST anchor and BOOT
/// in the same Catalog transaction. The caller verifies their immutable requests.
pub fn verify_native_legacy_history_recovery_publications_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    full_operation: OperationKey,
    anchor_operation: OperationKey,
    budget: &mut WorkBudget,
) -> Result<NativeBootPublishedOperation, IndexError> {
    verify_rollback_source_publications(
        state_root,
        io,
        full_operation,
        anchor_operation,
        RollbackCheck::SelectedWithHistory(anchor_operation),
        budget,
    )
}

/// Reobserve the primary History chain while authenticating the exact
/// protected sibling workspaces in the selected BOOT route object.
pub fn verify_native_legacy_history_recovery_publications_with_routes_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    full_operation: OperationKey,
    anchor_operation: OperationKey,
    routes: &NativeProtectedSourceRoutesV1,
    budget: &mut WorkBudget,
) -> Result<NativeBootPublishedOperation, IndexError> {
    verify_native_recovery_publications_with_routes_v1(
        state_root,
        io,
        full_operation,
        anchor_operation,
        routes,
        true,
        budget,
    )
}

/// Observe the primary Source/AST anchor under the exact protected sibling
/// route set. History predecessors are captured only when requested.
pub fn verify_native_recovery_publications_with_routes_v1(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    full_operation: OperationKey,
    anchor_operation: OperationKey,
    routes: &NativeProtectedSourceRoutesV1,
    capture_history: bool,
    budget: &mut WorkBudget,
) -> Result<NativeBootPublishedOperation, IndexError> {
    verify_rollback_source_publications(
        state_root,
        io,
        full_operation,
        anchor_operation,
        if capture_history {
            RollbackCheck::SelectedWithHistoryRoutes(anchor_operation, routes)
        } else {
            RollbackCheck::SelectedWithRoutes(routes)
        },
        budget,
    )
}

fn verify_rollback_source_publications(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    full_operation: OperationKey,
    current_operation: OperationKey,
    rollback: RollbackCheck<'_>,
    budget: &mut WorkBudget,
) -> Result<NativeBootPublishedOperation, IndexError> {
    if full_operation.workspace != current_operation.workspace
        || full_operation.epoch != current_operation.epoch
    {
        return Err(IndexError::DigestConflict);
    }
    let (snapshot, _, published, additional) = verify(
        state_root,
        io,
        None,
        rollback,
        Some(full_operation),
        (current_operation != full_operation).then_some(current_operation),
        budget,
    )?;
    let (operation, publication) = published.ok_or(IndexError::SnapshotUnavailable)?;
    if operation.key != full_operation
        || additional
            .as_ref()
            .is_some_and(|(current, _)| current.key != current_operation)
    {
        return Err(IndexError::DigestConflict);
    }
    Ok(NativeBootPublishedOperation {
        operation,
        publication,
        additional,
        snapshot,
    })
}

fn verify(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    candidate: Option<OperationKey>,
    rollback: RollbackCheck<'_>,
    published_operation: Option<OperationKey>,
    additional_operation: Option<OperationKey>,
    budget: &mut WorkBudget,
) -> Result<Verification, IndexError> {
    with_admitted_connection(
        state_root,
        io,
        budget,
        if additional_operation.is_some() {
            49_152
        } else if candidate.is_some() || published_operation.is_some() {
            32_768
        } else {
            16_384
        },
        |error| error,
        |connection, path, budget, allocation| {
            inspect(
                connection,
                path,
                candidate,
                rollback,
                published_operation,
                additional_operation,
                budget,
                allocation,
            )
        },
    )
}

/// One physical connection path for Native and Legacy read-only inspection.
/// `inspect` runs inside this connection; its first operation establishes one
/// Deferred snapshot. The Native caller keeps its existing error precedence.
fn with_admitted_connection<T, E, F>(
    state_root: &Path,
    io: &dyn CatalogReadOnlyIoAdmission,
    budget: &mut WorkBudget,
    retained_bytes: u64,
    map_close: fn(IndexError) -> E,
    inspect: F,
) -> Result<T, E>
where
    E: From<IndexError>,
    F: FnOnce(&Connection, &Path, &mut WorkBudget, Option<MemoryLease>) -> Result<T, E>,
{
    budget.charge(WorkUsage {
        reads: 2,
        ..WorkUsage::default()
    })?;
    let path_bytes = u64::try_from(state_root.as_os_str().len())
        .map_err(|_error| IndexError::ResourceLimited)?
        .checked_mul(8)
        .and_then(|bytes| bytes.checked_add(2048))
        .ok_or(IndexError::ResourceLimited)?;
    let _paths = budget.reserve_memory(path_bytes, MemoryLifetime::Temporary)?;
    let _scope = crate::io_admission::ScopeLease::enter_read_only(io, budget.request());
    let path = catalog_dir(state_root).join(CATALOG_FILE_NAME);
    let wal_path = path.with_file_name(format!("{CATALOG_FILE_NAME}-wal"));
    let main = input(&path, false)?.ok_or(IndexError::SnapshotUnavailable)?;
    let wal = input(&wal_path, true)?;
    if main.bytes < 100 {
        return Err(IndexError::Storage("Catalog header is truncated".into()).into());
    }
    let input_bytes = main
        .bytes
        .checked_add(wal.map_or(0, |file| file.bytes))
        .ok_or(IndexError::ResourceLimited)?;
    // Admission covers the complete possible WAL recovery scan. The heap WAL
    // index has bounded per-frame storage; the cache and canonical metadata
    // scratch add a fixed envelope. This is logical allocation admission, not
    // a claim about whole-process RSS or SQLite's global allocator.
    budget.charge(WorkUsage {
        reads: 256,
        bytes: input_bytes,
        ..WorkUsage::default()
    })?;
    let envelope = input_bytes
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(2 * 1024 * 1024))
        .ok_or(IndexError::ResourceLimited)?;
    let _scratch = budget.reserve_memory(envelope, MemoryLifetime::Temporary)?;
    // Materialized results keep their return allowance with the snapshot. A
    // visitor-only read keeps no allocation in its return value.
    let allocation = if retained_bytes == 0 {
        None
    } else {
        budget.reserve_memory(retained_bytes, MemoryLifetime::Retained)?
    };
    let main_only = wal.is_none_or(|file| file.bytes == 0);
    let uri = main_only.then(|| main_only_uri(&path));
    let open_path = uri.as_ref().map_or(path.as_path(), |uri| Path::new(uri));
    let connection = Connection::open_with_flags_and_vfs(
        open_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | if main_only {
                OpenFlags::SQLITE_OPEN_URI
            } else {
                OpenFlags::empty()
            },
        io.read_only_vfs_name(),
    )
    .map_err(|error| engine(&error))?;
    let result = inspect(&connection, &path, budget, allocation);
    // close() is explicit: no successful verification can hide a close error.
    let closed = connection
        .close()
        .map_err(|(_connection, error)| map_close(sql(&error)));
    budget.checkpoint()?;
    if input(&path, false)? != Some(main) || input(&wal_path, true)? != wal {
        return Err(
            IndexError::Storage("Catalog inputs changed during verification".into()).into(),
        );
    }
    let value = result?;
    closed?;
    Ok(value)
}

fn inspect(
    connection: &Connection,
    path: &Path,
    candidate: Option<OperationKey>,
    rollback: RollbackCheck<'_>,
    published_operation: Option<OperationKey>,
    additional_operation: Option<OperationKey>,
    budget: &mut WorkBudget,
    allocation: Option<MemoryLease>,
) -> Result<Verification, IndexError> {
    configure_read_connection(connection, budget)?;
    if capture_format::preflight_capture_format(connection, path).map_err(core)? {
        return Err(IndexError::RebuildRequired);
    }
    crate::idempotency::verify_installed_schema(connection, path).map_err(core)?;
    crate::sequence::verify_installed_schema(connection, path).map_err(core)?;
    crate::auxiliary::verify_installed_schema(connection, path).map_err(core)?;
    crate::candidate::verify_installed_schema(connection, path).map_err(core)?;
    let root_nonce = capture_format::verify_stamp(connection, path).map_err(core)?;
    let selected = boot_manifest::read_stored(connection)?;
    if let Some(selected) = selected.as_ref() {
        boot_manifest::validate_custody(connection, selected, budget)?;
    }
    let mut history_predecessors = Vec::new();
    let mut history_allocation = None;
    match rollback {
        RollbackCheck::None => {}
        RollbackCheck::Expected(baseline) => check_rollback(connection, baseline, budget)?,
        RollbackCheck::Selected => check_rollback(
            connection,
            selected.as_ref().ok_or(IndexError::SnapshotUnavailable)?,
            budget,
        )?,
        RollbackCheck::SelectedWithRoutes(routes) => check_rollback_with_publications(
            connection,
            selected.as_ref().ok_or(IndexError::SnapshotUnavailable)?,
            Some(routes),
            budget,
            |_, _| Ok(()),
        )?,
        RollbackCheck::SelectedWithHistory(anchor)
        | RollbackCheck::SelectedWithHistoryRoutes(anchor, _) => {
            let routes = match rollback {
                RollbackCheck::SelectedWithHistoryRoutes(_, routes) => Some(routes),
                _ => None,
            };
            let selected = selected.as_ref().ok_or(IndexError::SnapshotUnavailable)?;
            let (_, anchor) = read_published_operation(connection, anchor, budget)?;
            let head = selected.publication.identity;
            let prior = anchor.identity;
            if prior.workspace != head.workspace || prior.epoch != head.epoch {
                return Err(IndexError::DigestConflict);
            }
            let first = prior
                .sequence
                .checked_add(1)
                .ok_or(IndexError::DigestConflict)?;
            let count = head
                .sequence
                .checked_sub(first)
                .ok_or(IndexError::DigestConflict)?;
            let bytes = count
                .checked_mul(std::mem::size_of::<NativeBootManifest>() as u64)
                .ok_or(IndexError::ResourceLimited)?;
            let count = usize::try_from(count).map_err(|_| IndexError::ResourceLimited)?;
            if count > 0 {
                history_allocation = budget.reserve_memory(bytes, MemoryLifetime::Retained)?;
                history_predecessors
                    .try_reserve_exact(count)
                    .map_err(|_| IndexError::ResourceLimited)?;
            }
            check_rollback_with_publications(
                connection,
                selected,
                routes,
                budget,
                |operation, publication| {
                    if operation.key.workspace != head.workspace {
                        return Ok(());
                    }
                    let view = publication.identity;
                    if view.epoch != head.epoch || view.sequence < first {
                        return Ok(());
                    }
                    if view.sequence == head.sequence {
                        return if operation == selected.operation
                            && publication == selected.publication
                        {
                            Ok(())
                        } else {
                            Err(IndexError::DigestConflict)
                        };
                    }
                    if history_predecessors.len() == count {
                        return Err(IndexError::DigestConflict);
                    }
                    history_predecessors.push(NativeBootManifest {
                        operation,
                        publication,
                        protected_source_routes: None,
                    });
                    Ok(())
                },
            )?;
            history_predecessors
                .sort_unstable_by_key(|manifest| manifest.publication.identity.sequence);
            if history_predecessors.len() != count
                || history_predecessors
                    .iter()
                    .zip(first..head.sequence)
                    .any(|(manifest, sequence)| manifest.publication.identity.sequence != sequence)
            {
                return Err(IndexError::DigestConflict);
            }
        }
    }
    let candidate = candidate
        .map(|key| read_candidate(connection, key, selected.as_ref(), budget))
        .transpose()?;
    let published_operation = published_operation
        .map(|key| read_published_operation(connection, key, budget))
        .transpose()?;
    let additional_operation = additional_operation
        .map(|key| read_published_operation(connection, key, budget))
        .transpose()?;
    budget.charge(WorkUsage {
        reads: 4,
        bytes: 1024,
        ..WorkUsage::default()
    })?;
    let namespace = if reclaim::is_bound(connection)? {
        Some(reclaim::bound_namespace(connection)?)
    } else {
        None
    };
    budget.checkpoint()?;
    Ok((
        NativeBootCatalogSnapshot {
            root_nonce,
            selected,
            namespace,
            history_predecessors,
            _history_allocation: history_allocation,
            _allocation: allocation,
        },
        candidate,
        published_operation,
        additional_operation,
    ))
}

fn configure_read_connection(
    connection: &Connection,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    let configured = connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
        .map_err(|error| sql(&error))?;
    let defensive = connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(|error| sql(&error))?;
    let trusted = connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)
        .map_err(|error| sql(&error))?;
    if !configured || !defensive || trusted {
        return Err(IndexError::BackendViolation(
            "SQLite declined the required verification connection configuration".into(),
        ));
    }
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(|error| sql(&error))?;
    let request = budget.request().clone();
    connection
        .progress_handler(
            1024,
            Some(move || request.checkpoint("verify native Catalog").is_err()),
        )
        .map_err(|error| sql(&error))?;
    // These pragmas change only connection state. Native guards remain the
    // final authority even if a caller accidentally disables query_only.
    connection
        .execute_batch(
            "PRAGMA locking_mode=EXCLUSIVE; PRAGMA query_only=ON;
             PRAGMA temp_store=MEMORY; PRAGMA mmap_size=0; PRAGMA cache_size=-1024;
             BEGIN;",
        )
        .map_err(|error| sql(&error))?;
    Ok(())
}

pub(super) fn read_published_operation(
    connection: &Connection,
    key: OperationKey,
    budget: &mut WorkBudget,
) -> Result<(ImmutableOperation, CatalogView), IndexError> {
    boot_manifest::charge_operation(connection, key, budget)?;
    let row = read_operation(connection, key)?.ok_or(IndexError::SnapshotUnavailable)?;
    let OperationStatus::Published(view) = row.status else {
        return Err(IndexError::Conflict);
    };
    if view.workspace != key.workspace || view.epoch != key.epoch || view.digest.0 == [0; 32] {
        return Err(IndexError::DigestConflict);
    }
    let publication = row.published_view.ok_or_else(|| {
        IndexError::Storage("Published operation lacks persisted metadata".into())
    })?;
    if publication.identity != view {
        return Err(IndexError::DigestConflict);
    }
    Ok((row.immutable, publication))
}

// Only a selected protected route can carry historical terminal rows beyond
// BOOT's 4 KiB operation scratch limit. Fund the actual row before decoding it
// and keep the lease through the caller's publication callback.
fn read_quiescent_operation_v1(
    connection: &Connection,
    key: OperationKey,
    routes: Option<&NativeProtectedSourceRoutesV1>,
    budget: &mut WorkBudget,
) -> Result<(Option<MemoryLease>, super::OperationRow), IndexError> {
    if routes.is_none() {
        boot_manifest::charge_operation(connection, key, budget)?;
        return Ok((
            None,
            read_operation(connection, key)?.ok_or(IndexError::SnapshotUnavailable)?,
        ));
    }
    budget.charge(WorkUsage {
        reads: 1,
        ..WorkUsage::default()
    })?;
    let sizes: Option<(i64, i64, i64)> = connection
        .query_row(
            "SELECT CASE WHEN typeof(workspace)='blob' AND length(workspace) BETWEEN 1 AND 4096 THEN length(workspace) END,
                    CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND ?2 THEN length(payload) END,
                    CASE WHEN typeof(row_digest)='blob' AND length(row_digest)=32 THEN 32 END
             FROM native_operation_v1 WHERE operation=?1",
            params![
                operation_key(key)?,
                i64::try_from(MAX_NATIVE_PROTECTED_SNAPSHOT_BYTES_V1)
                    .map_err(|_| IndexError::ResourceLimited)?,
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let (workspace, payload, digest) = sizes.ok_or(IndexError::SnapshotUnavailable)?;
    let payload = uint(payload)?;
    if payload <= 4096 {
        boot_manifest::charge_operation(connection, key, budget)?;
        return Ok((
            None,
            read_operation(connection, key)?.ok_or(IndexError::SnapshotUnavailable)?,
        ));
    }
    let digest = uint(digest)?;
    let bytes = uint(workspace)?
        .checked_add(payload)
        .and_then(|value| value.checked_add(digest))
        .ok_or(IndexError::ResourceLimited)?;
    budget.charge(WorkUsage {
        reads: 1,
        bytes,
        ..WorkUsage::default()
    })?;
    let backing = payload
        .checked_mul(8)
        .and_then(|value| value.checked_add(4096))
        .ok_or(IndexError::ResourceLimited)?;
    let loan = budget
        .reserve_memory(backing, MemoryLifetime::Temporary)?
        .ok_or(IndexError::ResourceLimited)?;
    let row = read_operation(connection, key)?.ok_or(IndexError::SnapshotUnavailable)?;
    Ok((Some(loan), row))
}

fn published_quiescent_row_v1(
    row: super::OperationRow,
) -> Result<(ImmutableOperation, CatalogView), IndexError> {
    let OperationStatus::Published(view) = row.status else {
        return Err(IndexError::Conflict);
    };
    if view.workspace != row.immutable.key.workspace
        || view.epoch != row.immutable.key.epoch
        || view.digest.0 == [0; 32]
    {
        return Err(IndexError::DigestConflict);
    }
    let publication = row.published_view.ok_or(IndexError::SnapshotUnavailable)?;
    if publication.identity != view {
        return Err(IndexError::DigestConflict);
    }
    Ok((row.immutable, publication))
}

pub(super) fn check_rollback(
    connection: &Connection,
    baseline: &NativeBootManifest,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    check_rollback_with_publications(connection, baseline, None, budget, |_, _| Ok(()))
}

fn check_rollback_with_publications(
    connection: &Connection,
    baseline: &NativeBootManifest,
    routes: Option<&NativeProtectedSourceRoutesV1>,
    budget: &mut WorkBudget,
    visit_published: impl FnMut(ImmutableOperation, CatalogView) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    budget.charge(WorkUsage {
        reads: 1,
        bytes: 4096,
        ..WorkUsage::default()
    })?;
    // A sibling workspace is admitted only through the exact selected route
    // object, never inferred from its workspace key or current head.
    if boot_manifest::read_stored(connection)?.as_ref() != Some(baseline) {
        return Err(IndexError::Conflict);
    }
    boot_manifest::validate_custody(connection, baseline, budget)?;
    if let Some(routes) = routes {
        let route_ref = baseline
            .protected_source_routes
            .ok_or(IndexError::SnapshotUnavailable)?;
        if usize::try_from(route_ref.count).ok() != Some(routes.heads.len()) {
            return Err(IndexError::DigestConflict);
        }
        let _binding = boot_manifest::validate_protected_routes(
            connection,
            baseline.operation.key,
            route_ref.object,
            routes,
            true,
            None,
            budget,
        )?;
    }
    let workspace = workspace_key(baseline.publication.identity.workspace)?;
    budget.charge(WorkUsage {
        reads: 4,
        bytes: 8192,
        ..WorkUsage::default()
    })?;
    let size: Option<i64> = connection
        .query_row(
            "SELECT CASE WHEN typeof(payload)='blob' AND length(payload) BETWEEN 1 AND 4096
         THEN length(payload) END FROM native_workspace_v1 WHERE workspace=?1",
            params![&workspace],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql(&error))?;
    let _size = uint(size.ok_or(IndexError::SnapshotUnavailable)?)?;
    if retention::charged_current_view(connection, baseline.publication.identity.workspace, budget)?
        .as_ref()
        != Some(&baseline.publication)
    {
        return Err(IndexError::Conflict);
    }
    check_quiescent_rows_with_publications_impl(
        connection,
        baseline.publication.identity.workspace,
        baseline.publication.identity,
        routes,
        None,
        budget,
        visit_published,
    )
}

/// The Source caller supplies the workspace; the selected BOOT and actual
/// current head are independently observed in this Catalog read transaction.
pub(super) fn check_source_root_quiescence(
    connection: &Connection,
    selected: &NativeBootManifest,
    expected_workspace: WorkspaceKey,
    budget: &mut WorkBudget,
) -> Result<CatalogView, IndexError> {
    if selected.operation.key.workspace != expected_workspace
        || selected.publication.identity.workspace != expected_workspace
    {
        return Err(IndexError::DigestConflict);
    }
    budget.charge(WorkUsage {
        reads: 4,
        bytes: 8192,
        ..WorkUsage::default()
    })?;
    let current_head = retention::charged_current_view(connection, expected_workspace, budget)?
        .ok_or(IndexError::SnapshotUnavailable)?;
    check_quiescent_rows(
        connection,
        expected_workspace,
        current_head.identity,
        budget,
    )?;
    Ok(current_head)
}

/// Canonical one-root scan shared by backward rollback and forward recovery.
/// The caller selects the publication ceiling from the actual same-transaction
/// head, or from the exact rollback baseline, before entering this scan.
pub(super) fn check_quiescent_rows(
    connection: &Connection,
    workspace: WorkspaceKey,
    maximum: ViewRef,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    check_quiescent_rows_with_publications(connection, workspace, maximum, budget, |_, _| Ok(()))
}

pub(super) fn check_quiescent_protected_routes(
    connection: &Connection,
    primary: ViewRef,
    routes: &NativeProtectedSourceRoutesV1,
    allowed_ingress: Option<IngressRef>,
    budget: &mut WorkBudget,
) -> Result<(), IndexError> {
    routes.validate().map_err(|_| IndexError::DigestConflict)?;
    check_quiescent_rows_with_publications_impl(
        connection,
        primary.workspace,
        primary,
        Some(routes),
        allowed_ingress,
        budget,
        |_, _| Ok(()),
    )
}

fn check_quiescent_rows_with_publications(
    connection: &Connection,
    workspace: WorkspaceKey,
    maximum: ViewRef,
    budget: &mut WorkBudget,
    mut visit_published: impl FnMut(ImmutableOperation, CatalogView) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    check_quiescent_rows_with_publications_impl(
        connection,
        workspace,
        maximum,
        None,
        None,
        budget,
        &mut visit_published,
    )
}

fn check_quiescent_rows_with_publications_impl(
    connection: &Connection,
    workspace: WorkspaceKey,
    maximum: ViewRef,
    routes: Option<&NativeProtectedSourceRoutesV1>,
    allowed_ingress: Option<IngressRef>,
    budget: &mut WorkBudget,
    mut visit_published: impl FnMut(ImmutableOperation, CatalogView) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    let workspace_key = workspace_key(workspace)?;
    let scratch_bytes = owned_work::MAX_OWNED_WORK_ROW_BYTES
        .checked_add(32_768)
        .ok_or(IndexError::ResourceLimited)?;
    let _scratch = budget.reserve_memory(scratch_bytes, MemoryLifetime::Temporary)?;
    let mut _ceilings_memory = None;
    let mut ceilings = Vec::<&ViewRef>::new();
    let blocked: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM native_workspace_v1 WHERE workspace<>?1 LIMIT 1)
             OR EXISTS(SELECT 1 FROM native_ingress_v1 LIMIT 1)",
            params![&workspace_key],
            |row| row.get(0),
        )
        .map_err(|error| sql(&error))?;
    if routes.is_none() && blocked {
        return Err(IndexError::Conflict);
    }
    if let Some(routes) = routes {
        let ingress_blocked: bool = if let Some(allowed) = allowed_ingress {
            let expected = operation_key(allowed.operation)?;
            connection.query_row(
                "SELECT (SELECT count(*) FROM native_ingress_v1)<>1 OR EXISTS(SELECT 1 FROM native_ingress_v1 WHERE operation<>?1 LIMIT 1)",
                params![&expected], |row| row.get(0),
            ).map_err(|error| sql(&error))?
        } else {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM native_ingress_v1 LIMIT 1)",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| sql(&error))?
        };
        if ingress_blocked {
            return Err(IndexError::Conflict);
        }
        let count = routes
            .heads
            .len()
            .checked_add(routes.repomap_heads.len())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = count
            .checked_mul(std::mem::size_of::<&ViewRef>())
            .ok_or(IndexError::ResourceLimited)?;
        let bytes = u64::try_from(bytes).map_err(|_width| IndexError::ResourceLimited)?;
        _ceilings_memory = budget.reserve_memory(bytes, MemoryLifetime::Temporary)?;
        ceilings
            .try_reserve_exact(count)
            .map_err(|_allocation| IndexError::ResourceLimited)?;
        ceilings.extend(routes.heads.iter().map(|head| &head.publication.identity));
        ceilings.extend(
            routes
                .repomap_heads
                .iter()
                .map(|head| &head.publication.identity),
        );
        ceilings.sort_unstable_by_key(|head| head.workspace);
        if ceilings
            .windows(2)
            .any(|pair| matches!(pair, [left, right] if left.workspace == right.workspace && left != right))
        {
            return Err(IndexError::DigestConflict);
        }
        ceilings.dedup_by_key(|head| head.workspace);
    }
    let routed_ceiling = |key| -> Option<ViewRef> {
        ceilings
            .binary_search_by_key(&key, |head| head.workspace)
            .map_or(None, |index| ceilings.get(index).map(|head| **head))
    };
    let ceiling_for = |key| routed_ceiling(key).or_else(|| (key == workspace).then_some(maximum));
    if routes.is_some() {
        let mut statement = connection
            .prepare("SELECT workspace FROM native_workspace_v1 ORDER BY workspace")
            .map_err(|error| sql(&error))?;
        let mut rows = statement.query([]).map_err(|error| sql(&error))?;
        let mut seen = 0usize;
        while let Some(row) = rows.next().map_err(|error| sql(&error))? {
            budget.charge(WorkUsage {
                reads: 1,
                bytes: 4096,
                ..WorkUsage::default()
            })?;
            let ValueRef::Blob(bytes) = row.get_ref(0).map_err(|error| sql(&error))? else {
                return Err(IndexError::DigestConflict);
            };
            if bytes.is_empty() || bytes.len() > 4096 {
                return Err(IndexError::DigestConflict);
            }
            let key: WorkspaceKey = decode(bytes)?;
            if ceiling_for(key).is_none() {
                return Err(IndexError::Conflict);
            }
            seen = seen.checked_add(1).ok_or(IndexError::ResourceLimited)?;
        }
        let primary_is_routed = routed_ceiling(workspace).is_some();
        let expected = ceilings
            .len()
            .checked_add(usize::from(!primary_is_routed))
            .ok_or(IndexError::ResourceLimited)?;
        if seen != expected {
            return Err(IndexError::Conflict);
        }
        if !primary_is_routed {
            retention::verify_source_floors(connection, workspace, budget)?;
        }
        for head in &ceilings {
            retention::verify_source_floors(connection, head.workspace, budget)?;
        }
    } else {
        retention::verify_source_floors(connection, workspace, budget)?;
    }

    // Validate canonical rows, not just denormalized status columns. Charge
    // each row and its bounded lookup before decoding any owned payload.
    let mut statement = connection
        .prepare("SELECT operation FROM native_operation_v1 ORDER BY operation_id")
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    loop {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            break;
        };
        let ValueRef::Blob(key) = row.get_ref(0).map_err(|error| sql(&error))? else {
            return Err(IndexError::Storage(
                "rollback operation key is not a blob".into(),
            ));
        };
        if key.is_empty() || key.len() > 4096 {
            return Err(IndexError::Storage(
                "rollback operation key is out of bounds".into(),
            ));
        }
        budget.charge(WorkUsage {
            bytes: 4096,
            ..WorkUsage::default()
        })?;
        let key: OperationKey = decode(key)?;
        let (_row_loan, operation) = read_quiescent_operation_v1(connection, key, routes, budget)?;
        let Some(ceiling) = ceiling_for(operation.immutable.key.workspace) else {
            return Err(IndexError::Conflict);
        };
        match operation.status {
            OperationStatus::Published(view) if settled_view_within_head(view, ceiling) => {
                let publication = operation
                    .published_view
                    .ok_or(IndexError::SnapshotUnavailable)?;
                visit_published(operation.immutable, publication)?;
            }
            OperationStatus::Refused(_) => {}
            OperationStatus::Unknown
            | OperationStatus::Pending(_)
            | OperationStatus::Published(_) => return Err(IndexError::Conflict),
        }
    }
    drop(rows);
    drop(statement);
    let mut statement = connection
        .prepare("SELECT work,length(payload) FROM native_work_v1 ORDER BY work")
        .map_err(|error| sql(&error))?;
    let mut rows = statement.query([]).map_err(|error| sql(&error))?;
    loop {
        budget.charge(WorkUsage {
            reads: 1,
            ..WorkUsage::default()
        })?;
        let Some(row) = rows.next().map_err(|error| sql(&error))? else {
            break;
        };
        let ValueRef::Blob(key) = row.get_ref(0).map_err(|error| sql(&error))? else {
            return Err(IndexError::Storage(
                "rollback work key is not a blob".into(),
            ));
        };
        let payload = uint(row.get(1).map_err(|error| sql(&error))?)?;
        if key.is_empty()
            || key.len() > 4096
            || payload == 0
            || payload > owned_work::MAX_OWNED_WORK_ROW_BYTES
        {
            return Err(IndexError::Storage(
                "rollback work row is out of bounds".into(),
            ));
        }
        budget.charge(WorkUsage {
            reads: 2,
            bytes: payload
                .checked_add(4096)
                .ok_or(IndexError::ResourceLimited)?,
            ..WorkUsage::default()
        })?;
        let work =
            read_work_by_key(connection, key, None)?.ok_or(IndexError::SnapshotUnavailable)?;
        let Some(ceiling) = ceiling_for(work.intent.workspace) else {
            return Err(IndexError::Conflict);
        };
        match work.status {
            quanta_index_core::index::WorkStatus::Completed(view)
                if settled_view_within_head(view, ceiling) => {}
            quanta_index_core::index::WorkStatus::Superseded => {}
            _ => return Err(IndexError::Conflict),
        }
    }
    budget.checkpoint()
}

fn settled_view_within_head(view: ViewRef, head: ViewRef) -> bool {
    view.workspace == head.workspace
        && (view == head || (view.epoch, view.sequence) < (head.epoch, head.sequence))
}

fn read_candidate(
    connection: &Connection,
    key: OperationKey,
    selected: Option<&NativeBootManifest>,
    budget: &mut WorkBudget,
) -> Result<NativeBootManifest, IndexError> {
    boot_manifest::charge_operation(connection, key, budget)?;
    let row = read_operation(connection, key)?.ok_or(IndexError::SnapshotUnavailable)?;
    let OperationStatus::Published(identity) = row.status else {
        return Err(IndexError::Conflict);
    };
    if identity.workspace != key.workspace || identity.epoch != key.epoch {
        return Err(IndexError::Storage(
            "boot candidate publication scope mismatch".into(),
        ));
    }
    if let Some(selected) = selected.filter(|value| value.operation.key == key) {
        if selected.operation != row.immutable || selected.publication.identity != identity {
            return Err(IndexError::Storage(
                "selected boot operation changed".into(),
            ));
        }
        return Ok(selected.clone());
    }
    let publication = boot_manifest::load_publication(connection, identity, budget)?;
    Ok(NativeBootManifest {
        operation: row.immutable,
        publication,
        protected_source_routes: None,
    })
}
