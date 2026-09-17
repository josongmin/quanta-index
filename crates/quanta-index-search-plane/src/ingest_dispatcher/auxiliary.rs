//! The auxiliary authority materializers (history, runtime metadata,
//! structural) and the mutation coordinator that serializes their catalog
//! writes.

use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use quanta_index_contract::{
    BatchPublishReceipt, DirtyIngestBatch, DirtyMutation, HistoryIngestBatch, HistoryRefMutation,
    RuntimeCatalogIngestBatch, SearchPlaneTrackKind, StructuralIngestBatch,
};
use quanta_index_core::{AuxiliaryAuthorityCatalogPort, CoreError};

use crate::Ledger;
use crate::auxiliary_authority::{
    history_delta_rows, history_transition, runtime_catalog_delta_rows, runtime_catalog_transition,
    runtime_dirty_delta_rows, runtime_dirty_transition, structural_delta_rows,
    structural_transition,
};
use crate::ingest_dispatcher::ports::{
    HistoryIngestPort, RuntimeMetadataIngestPort, StructuralIngestPort,
};

/// Serializes the validate → persist → apply protocol of every auxiliary
/// mutation (QI-BB-020).
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
#[derive(Debug, Default)]
pub struct AuxiliaryMutationCoordinator {
    serial: Mutex<()>,
}

impl AuxiliaryMutationCoordinator {
    #[must_use]
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(super) fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>, CoreError> {
        self.serial.lock().map_err(|err| {
            CoreError::Storage(format!(
                "auxiliary materialize: mutation coordinator poisoned: {err}"
            ))
        })
    }
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
pub struct DirectHistoryMaterializer {
    parts: AuxiliaryMaterializerParts,
}

impl DirectHistoryMaterializer {
    #[must_use]
    pub const fn new(parts: AuxiliaryMaterializerParts) -> Self {
        Self { parts }
    }
}

impl HistoryIngestPort for DirectHistoryMaterializer {
    fn publish_batch(&self, batch: &HistoryIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        const WHAT: &str = "direct history materialize";
        let _serial = self.parts.coordinator.lock()?;
        let delta = {
            let guard = self.parts.read_ledger(WHAT)?;
            let epoch =
                guard.history_next_epoch(&batch.repo_id, &batch.revision_id, batch.generation)?;
            history_transition(
                guard.history_state(&batch.repo_id, &batch.revision_id, batch.generation),
                epoch,
                batch,
            )?
        };
        let _durable = self.parts.catalog.apply(&history_delta_rows(&delta)?)?;
        {
            let mut guard = self.parts.write_ledger(WHAT)?;
            guard.apply_history_delta(&delta, Instant::now())?;
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
        let _serial = self.parts.coordinator.lock()?;
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
        let _serial = self.parts.coordinator.lock()?;
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
        let _serial = self.parts.coordinator.lock()?;
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
