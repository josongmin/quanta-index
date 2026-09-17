//! `DirectSemanticMaterializer`: semantic-only ingest into a staged generation.

use std::sync::{Arc, RwLock};

use quanta_index_contract::{BatchPublishReceipt, SearchPlaneTrackKind, SemanticIngestBatch};
use quanta_index_core::{CoreError, SemanticBatchBuildPort, SemanticIngestPort};

use crate::Ledger;

/// Direct semantic batch materializer.
///
/// Writes the durable, generation-scoped semantic adapter first (rows on every
/// batch; graph + manifest + seal on `seal`), then updates the readiness
/// ledger. Durability lives entirely in the adapter's generation directories;
/// there is no journal write here. A failed durable write leaves no SEALED
/// marker and does not touch the ledger, so readiness cannot go falsely ready.
pub struct DirectSemanticMaterializer {
    builder: Arc<dyn SemanticBatchBuildPort + Send + Sync>,
    ledger: Arc<RwLock<Ledger>>,
}

impl DirectSemanticMaterializer {
    #[must_use]
    pub fn new(
        builder: Arc<dyn SemanticBatchBuildPort + Send + Sync>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self { builder, ledger }
    }
}

impl SemanticIngestPort for DirectSemanticMaterializer {
    fn publish_batch(&self, batch: &SemanticIngestBatch) -> Result<BatchPublishReceipt, CoreError> {
        self.builder.build_batch(batch)?;
        let mut guard = self.ledger.write().map_err(|err| {
            CoreError::Storage(format!(
                "direct semantic materialize: ledger poisoned: {err}"
            ))
        })?;
        guard.materialize_track(
            &batch.repo_id,
            &batch.revision_id,
            SearchPlaneTrackKind::Semantic,
            batch.generation,
            Some(batch.manifest_digest.as_str()),
        );
        if batch.seal {
            guard.seal_track_with_digest(
                &batch.repo_id,
                &batch.revision_id,
                SearchPlaneTrackKind::Semantic,
                batch.generation,
                batch.manifest_digest.as_str(),
            );
        }
        drop(guard);
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
        for _surface in &batch.clear_surfaces {
            receipt.accept_clear_surface();
        }
        if batch.seal {
            receipt.mark_sealed();
        }
        Ok(receipt)
    }
}
