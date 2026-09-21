//! The auxiliary authority materializers (history, runtime metadata,
//! structural) and the mutation coordinator that serializes their catalog
//! writes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use quanta_index_contract::{
    AuxEpochV1, BatchPublishReceipt, DirtyIngestBatch, DirtyMutation, HistoryIngestBatch,
    HistoryRefMutation, RuntimeCatalogIngestBatch, SearchPlaneTrackKind, StructuralIngestBatch,
};
use quanta_index_core::{
    AuxiliaryAuthorityCatalogPort, CoreError, HistoryTextBuildV1, HistoryTextDocKeyV1,
    HistoryTextDocV1, HistoryTextEpochStatusV1, HistoryTextIndexPort,
};

use crate::Ledger;
use crate::auxiliary_authority::{
    history_delta_rows, history_transition, runtime_catalog_delta_rows, runtime_catalog_transition,
    runtime_dirty_delta_rows, runtime_dirty_transition, structural_delta_rows,
    structural_transition,
};
use crate::history_text::HistoryTextIndexParts;
use crate::ingest_dispatcher::ports::{
    HistoryIngestPort, RuntimeMetadataIngestPort, StructuralIngestPort,
};
use crate::readiness::{HistoryAuthorityState, HistoryDelta, history_diff_search_text};

/// Serializes the validate → persist → apply protocol of every auxiliary
/// mutation (QI-BB-020) under the durable `MutationCoordinatorV1`
/// (SEP-21 P02B).
///
/// A transition is validated against the ledger under its read lock and
/// stamped with the next epoch of its generation and domain, made
/// durable in the catalog together with that epoch, then applied under
/// the write lock at exactly that epoch (W2). Two mutations interleaving
/// between those steps could validate against a state the other is about
/// to change — or claim the same epoch — so every auxiliary materializer
/// and the search-corpus path that owns the chunk universe take this lock
/// for the whole protocol. Queries never take it: they clone a snapshot
/// under the ledger's read lock and scan outside it.
///
/// When a durable coordinator port is wired (production), the guard also
/// holds the state-root-global mutation lease, so a second *process*
/// against the same state root is refused `CATALOG_BUSY` rather than
/// interleaving; a crash releases the lease at its deadline. The
/// in-process mutex remains for same-process epoch ordering — the ledger
/// epochs it protects are process-local state.
pub struct AuxiliaryMutationCoordinator {
    serial: Mutex<()>,
    durable: Option<Arc<dyn quanta_index_core::MutationCoordinatorPort + Send + Sync>>,
}

/// The guard the materializers hold for the whole protocol: the
/// in-process serial slot plus, when wired, the durable lease (released
/// on drop; a failed release is a fence loss the next `enter` surfaces
/// typed, and the lease dies at its deadline regardless).
pub(super) struct CoordinatorGuard<'a> {
    _serial: std::sync::MutexGuard<'a, ()>,
    lease: Option<(
        Arc<dyn quanta_index_core::MutationCoordinatorPort + Send + Sync>,
        quanta_index_core::MutationLeaseV1,
    )>,
}

impl Drop for CoordinatorGuard<'_> {
    fn drop(&mut self) {
        if let Some((port, lease)) = &self.lease {
            let _released = port.release(lease);
        }
    }
}

impl AuxiliaryMutationCoordinator {
    /// A coordinator with only in-process serialization (tests and
    /// single-process tooling).
    #[must_use]
    pub fn shared() -> Arc<Self> {
        Arc::new(Self {
            serial: Mutex::new(()),
            durable: None,
        })
    }

    /// A coordinator that also machine-enforces exclusivity through the
    /// state-root-global durable mutation lease (production wiring).
    #[must_use]
    pub fn durable(
        port: Arc<dyn quanta_index_core::MutationCoordinatorPort + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self {
            serial: Mutex::new(()),
            durable: Some(port),
        })
    }

    pub(super) fn lock(&self, owner: &str) -> Result<CoordinatorGuard<'_>, CoreError> {
        let serial = self.serial.lock().map_err(|err| {
            CoreError::Storage(format!(
                "auxiliary materialize: mutation coordinator poisoned: {err}"
            ))
        })?;
        let lease = match &self.durable {
            None => None,
            Some(port) => {
                let lease = port.enter("auxiliary-mutation", owner, 30_000)?;
                Some((Arc::clone(port), lease))
            }
        };
        Ok(CoordinatorGuard { _serial: serial, lease })
    }
}

/// The owner identity auxiliary mutations of this process carry.
pub(super) fn coordinator_owner() -> String {
    format!("searchd-aux-{}", std::process::id())
}

/// The catalog and the ledger every auxiliary materializer writes through.
#[derive(Clone)]
pub struct AuxiliaryMaterializerParts {
    pub catalog: Arc<dyn AuxiliaryAuthorityCatalogPort + Send + Sync>,
    pub coordinator: Arc<AuxiliaryMutationCoordinator>,
    pub ledger: Arc<RwLock<Ledger>>,
}

impl AuxiliaryMaterializerParts {
    fn read_ledger(&self, what: &str) -> Result<std::sync::RwLockReadGuard<'_, Ledger>, CoreError> {
        self.ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("{what}: ledger poisoned: {err}")))
    }

    fn write_ledger(
        &self,
        what: &str,
    ) -> Result<std::sync::RwLockWriteGuard<'_, Ledger>, CoreError> {
        self.ledger
            .write()
            .map_err(|err| CoreError::Storage(format!("{what}: ledger poisoned: {err}")))
    }
}

/// Direct history materializer. History is auxiliary and non-activation
/// blocking; its rows are durable before its receipt and visible only
/// after (QI-BB-020).
///
/// With a history text index wired (QI-BB-023 follow-up #1) the index of
/// the epoch the batch produces is published before the rows: the rows
/// and the index of one epoch are one snapshot, and an epoch number is
/// only ever claimed with both. After the epoch is visible, every index
/// of the generation the ledger no longer retains — pruned by this
/// mutation, or left by a restart or a crash — is retired and discarded
/// unless a reader still holds it.
pub struct DirectHistoryMaterializer {
    parts: AuxiliaryMaterializerParts,
    history_text: Option<HistoryTextIndexParts>,
}

impl DirectHistoryMaterializer {
    #[must_use]
    pub const fn new(parts: AuxiliaryMaterializerParts) -> Self {
        Self {
            parts,
            history_text: None,
        }
    }

    /// Wire the history text index every published epoch also builds.
    #[must_use]
    pub fn with_history_text(mut self, history_text: HistoryTextIndexParts) -> Self {
        self.history_text = Some(history_text);
        self
    }
}

/// What the epoch's text index build takes.
///
/// The current epoch's index as a base plus the delta's documents, or —
/// when the current epoch has no servable index (the generation predates
/// the index, or its index was built under another normalizer) — every
/// document of the state after the delta.
fn plan_history_text_build(
    port: &dyn HistoryTextIndexPort,
    current: Option<(AuxEpochV1, &HistoryAuthorityState)>,
    delta: &HistoryDelta,
) -> Result<HistoryTextBuildV1, CoreError> {
    let Some((base, state)) = current else {
        return Ok(HistoryTextBuildV1::Full {
            docs: history_text_docs_of_delta(delta, None)?,
        });
    };
    match port.epoch_status(&delta.generation, base)? {
        HistoryTextEpochStatusV1::Servable => Ok(HistoryTextBuildV1::Incremental {
            base,
            upserts: history_text_docs_of_delta(delta, Some(state))?,
        }),
        HistoryTextEpochStatusV1::Absent | HistoryTextEpochStatusV1::Unsupported { .. } => {
            let mut docs: BTreeMap<HistoryTextDocKeyV1, HistoryTextDocV1> = BTreeMap::new();
            for record in state.commits().values() {
                let doc = commit_text_doc(record);
                let _replaced = docs.insert(doc.key.clone(), doc);
            }
            for (key, record) in state.diff_hunks() {
                let commit = state.commits().get(&key.commit_sha()).ok_or_else(|| {
                    CoreError::Storage(format!(
                        "history text index: diff hunk {}:{} names a commit absent from the state",
                        key.commit_sha(),
                        key.file_path()
                    ))
                })?;
                let doc = HistoryTextDocV1 {
                    key: HistoryTextDocKeyV1::Diff {
                        sha: key.commit_sha(),
                        file_path: key.file_path().to_string(),
                    },
                    committer_time_ms: commit.committer_time_ms,
                    text: history_diff_search_text(key, record),
                };
                let _replaced = docs.insert(doc.key.clone(), doc);
            }
            for doc in history_text_docs_of_delta(delta, Some(state))? {
                let _replaced = docs.insert(doc.key.clone(), doc);
            }
            Ok(HistoryTextBuildV1::Full {
                docs: docs.into_values().collect(),
            })
        }
    }
}

fn commit_text_doc(record: &quanta_index_contract::lex::CommitRecord) -> HistoryTextDocV1 {
    HistoryTextDocV1 {
        key: HistoryTextDocKeyV1::Commit { sha: record.sha },
        committer_time_ms: record.committer_time_ms,
        text: record.message.to_string(),
    }
}

/// The documents one history delta upserts.
///
/// Its commits and its diff hunks, each hunk stamped with its commit's
/// time (from the delta, else from the state the delta was validated
/// against).
fn history_text_docs_of_delta(
    delta: &HistoryDelta,
    state: Option<&HistoryAuthorityState>,
) -> Result<Vec<HistoryTextDocV1>, CoreError> {
    let mut docs: Vec<HistoryTextDocV1> = delta.commits.iter().map(commit_text_doc).collect();
    for (key, record) in &delta.diff_hunks {
        let committer_time_ms = delta
            .commits
            .iter()
            .find(|commit| commit.sha == key.commit_sha())
            .map(|commit| commit.committer_time_ms)
            .or_else(|| {
                state.and_then(|state| {
                    state
                        .commits()
                        .get(&key.commit_sha())
                        .map(|commit| commit.committer_time_ms)
                })
            })
            .ok_or_else(|| {
                CoreError::Storage(format!(
                    "history text index: diff hunk {}:{} names a commit the transition did not validate",
                    key.commit_sha(),
                    key.file_path()
                ))
            })?;
        docs.push(HistoryTextDocV1 {
            key: HistoryTextDocKeyV1::Diff {
                sha: key.commit_sha(),
                file_path: key.file_path().to_string(),
            },
            committer_time_ms,
            text: history_diff_search_text(key, record),
        });
    }
    Ok(docs)
}

impl HistoryIngestPort for DirectHistoryMaterializer {
    fn publish_batch(&self, batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        const WHAT: &str = "direct history materialize";
        let _serial = self.parts.coordinator.lock(&coordinator_owner())?;
        // The next epoch and the current snapshot are read under the
        // ledger lock; the transition and the index plan run on that
        // immutable snapshot after it, the coordinator lock keeping any
        // other mutation from changing what they validated against.
        let (epoch, current) = {
            let guard = self.parts.read_ledger(WHAT)?;
            let epoch =
                guard.history_next_epoch(&batch.repo_id, &batch.revision_id, batch.generation)?;
            let current = guard.history_read_at(
                &batch.repo_id,
                &batch.revision_id,
                batch.generation,
                None,
                Instant::now(),
            )?;
            drop(guard);
            (epoch, current)
        };
        let delta = history_transition(
            current.as_ref().map(|read| read.state.as_ref()),
            epoch,
            batch,
        )?;
        let text_build = self
            .history_text
            .as_ref()
            .map(|parts| {
                plan_history_text_build(
                    parts.port.as_ref(),
                    current
                        .as_ref()
                        .map(|read| (read.epoch, read.state.as_ref())),
                    &delta,
                )
            })
            .transpose()?;
        if let (Some(parts), Some(build)) = (&self.history_text, text_build) {
            let _receipt = parts
                .port
                .publish_epoch(&delta.generation, delta.epoch, build)?;
        }
        let _durable = self.parts.catalog.apply(&history_delta_rows(&delta)?)?;
        {
            let mut guard = self.parts.write_ledger(WHAT)?;
            guard.apply_history_delta(&delta, Instant::now())?;
        }
        if let Some(parts) = &self.history_text {
            let retained = self
                .parts
                .read_ledger(WHAT)?
                .history_retained_epochs(
                    &delta.generation.repo_id,
                    &delta.generation.revision_id,
                    delta.generation.generation,
                )
                .ok_or_else(|| {
                    CoreError::Storage(format!(
                        "{WHAT}: generation {} vanished from the ledger after its delta applied",
                        delta.generation.generation.get()
                    ))
                })?;
            // The delta is durable and served from here on: epochs still
            // held by a reader, and any discard the storage fails, are left
            // to the next mutation of the generation, which reconciles
            // again; a refusal fails closed.
            parts.reconcile_after_durable(&delta.generation, &retained)?;
        }
        let mut receipt = BatchPublishReceipt::empty_for(
            batch.generation,
            batch.manifest_digest.clone(),
            batch.batch_digest.clone(),
        );
        for _record in &batch.commits {
            receipt.accept_replace_scope();
        }
        for mutation in batch.refs.iter().chain(batch.tags.iter()) {
            match mutation {
                HistoryRefMutation::Upsert(_) => receipt.accept_replace_scope(),
                HistoryRefMutation::Delete(_) => receipt.accept_tombstone_scope(),
            }
        }
        for _record in &batch.diff_hunks {
            receipt.accept_replace_scope();
        }
        Ok(receipt)
    }
}

/// Direct dirty-overlay and runtime catalog materializer. Runtime state
/// remains auxiliary and non-activation-blocking; its rows are durable
/// before its receipt and visible only after (QI-BB-020).
pub struct DirectRuntimeMetadataMaterializer {
    parts: AuxiliaryMaterializerParts,
}

impl DirectRuntimeMetadataMaterializer {
    #[must_use]
    pub const fn new(parts: AuxiliaryMaterializerParts) -> Self {
        Self { parts }
    }
}

pub(super) fn dirty_publish_receipt_v1(batch: &DirtyIngestBatch) -> BatchPublishReceipt {
    let mut receipt =
        BatchPublishReceipt::empty_for(batch.generation, None, batch.batch_digest.clone());
    for entry in &batch.entries {
        match entry {
            DirtyMutation::Upsert(_) => receipt.accept_replace_scope(),
            DirtyMutation::Delete(_) => receipt.accept_tombstone_scope(),
        }
    }
    receipt
}

impl RuntimeMetadataIngestPort for DirectRuntimeMetadataMaterializer {
    fn publish_batch(&self, batch: &DirtyIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        const WHAT: &str = "direct dirty materialize";
        let _serial = self.parts.coordinator.lock(&coordinator_owner())?;
        let delta = {
            let guard = self.parts.read_ledger(WHAT)?;
            let epoch =
                guard.runtime_next_epoch(&batch.repo_id, &batch.revision_id, batch.generation)?;
            runtime_dirty_transition(
                guard.runtime_state(&batch.repo_id, &batch.revision_id, batch.generation),
                epoch,
                batch,
            )
        };
        let _durable = self
            .parts
            .catalog
            .apply(&runtime_dirty_delta_rows(&delta)?)?;
        {
            let mut guard = self.parts.write_ledger(WHAT)?;
            guard.apply_runtime_dirty_delta(&delta, Instant::now())?;
        }
        Ok(dirty_publish_receipt_v1(batch))
    }

    fn publish_catalog_batch(
        &self,
        batch: &RuntimeCatalogIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        const WHAT: &str = "direct runtime catalog materialize";
        let _serial = self.parts.coordinator.lock(&coordinator_owner())?;
        let delta = {
            let guard = self.parts.read_ledger(WHAT)?;
            let epoch =
                guard.runtime_next_epoch(&batch.repo_id, &batch.revision_id, batch.generation)?;
            runtime_catalog_transition(
                guard.structural_state(&batch.repo_id, &batch.revision_id, batch.generation),
                guard.runtime_state(&batch.repo_id, &batch.revision_id, batch.generation),
                epoch,
                batch,
            )?
        };
        let _durable = self
            .parts
            .catalog
            .apply(&runtime_catalog_delta_rows(&delta)?)?;
        {
            let mut guard = self.parts.write_ledger(WHAT)?;
            guard.apply_runtime_catalog_delta(&delta, Instant::now())?;
        }
        let mut receipt =
            BatchPublishReceipt::empty_for(batch.generation, None, batch.batch_digest.clone());
        let accepted = [
            batch.changed_entries.len(),
            batch.facet_entries.len(),
            batch.snapshot_entries.len(),
            batch.affected_entries.len(),
            batch.invalidated_by_entries.len(),
        ]
        .into_iter()
        .fold(0_usize, usize::saturating_add);
        for _record in 0..accepted {
            receipt.accept_replace_scope();
        }
        Ok(receipt)
    }
}

/// Direct structural materializer. Structural readiness is first-class;
/// parse trees and the structural track's state are durable before the
/// receipt and visible only after (QI-BB-020).
pub struct DirectStructuralMaterializer {
    parts: AuxiliaryMaterializerParts,
}

impl DirectStructuralMaterializer {
    #[must_use]
    pub const fn new(parts: AuxiliaryMaterializerParts) -> Self {
        Self { parts }
    }
}

impl StructuralIngestPort for DirectStructuralMaterializer {
    fn publish_batch(
        &self,
        batch: &StructuralIngestBatch,
    ) -> Result<BatchPublishReceipt, CoreError> {
        const WHAT: &str = "direct structural materialize";
        let _serial = self.parts.coordinator.lock(&coordinator_owner())?;
        let delta = {
            let guard = self.parts.read_ledger(WHAT)?;
            let epoch = guard.structural_next_epoch(
                &batch.repo_id,
                &batch.revision_id,
                batch.generation,
            )?;
            structural_transition(
                guard.structural_state(&batch.repo_id, &batch.revision_id, batch.generation),
                guard.track_state(
                    &batch.repo_id,
                    &batch.revision_id,
                    SearchPlaneTrackKind::Structural,
                ),
                epoch,
                batch,
            )?
        };
        let _durable = self.parts.catalog.apply(&structural_delta_rows(&delta)?)?;
        {
            let mut guard = self.parts.write_ledger(WHAT)?;
            guard.apply_structural_trees_delta(&delta, Instant::now())?;
        }
        let mut receipt = BatchPublishReceipt::empty_for(
            batch.generation,
            Some(batch.manifest_digest.clone()),
            batch.batch_digest.clone(),
        );
        for _scope in &batch.replace_scopes {
            receipt.accept_replace_scope();
        }
        for _scope in &batch.tombstone_scopes {
            receipt.accept_tombstone_scope();
        }
        if batch.seal {
            receipt.mark_sealed();
        }
        Ok(receipt)
    }
}
