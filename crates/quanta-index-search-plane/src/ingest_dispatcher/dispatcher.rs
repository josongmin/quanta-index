//! `SearchPlaneIngestDispatcher`: routes typed ingest requests to their owner
//! ports under the idempotency record and the request budget.

use std::sync::Arc;

use quanta_index_contract::{
    BatchPublishReceipt, ManifestGeneration, RepoId, RepoMapMutationAck, RevisionId,
    SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse,
};
use quanta_index_core::{
    CoreError, FileContributorIngestPort, FileOwnershipIngestPort, IdempotencyBeginV1,
    IdempotencyCatalogPort, IdempotencyKeyV1, IngestOperationKindV1, RepoCommitRecencyIngestPort,
    RepoDescriptionIngestPort, RepoMapBundleIngestPort, RepoMetaIngestPort, RepoTopicIngestPort,
    RequestBudgetV1, SearchCorpusIngestPort,
};

use crate::ingest_dispatcher::errors::core_error_to_ipc;
use crate::ingest_dispatcher::ports::{
    HistoryIngestPort, RuntimeMetadataIngestPort, StructuralIngestPort,
};

// =============================================================================
// Top-level dispatcher
// =============================================================================

/// Routes typed ingest requests to the appropriate domain port. Mirrors the
/// shape of [`crate::SearchPlaneControlDispatcher`] / [`crate::SearchPlaneDispatcher`]
/// for the new ingest surface (QI-RT-01).
pub struct SearchPlaneIngestDispatcher {
    lexical: Arc<dyn SearchCorpusIngestPort + Send + Sync>,
    history: Arc<dyn HistoryIngestPort + Send + Sync>,
    repo_commit_recency: Arc<dyn RepoCommitRecencyIngestPort + Send + Sync>,
    repo_topic: Arc<dyn RepoTopicIngestPort + Send + Sync>,
    repo_description: Arc<dyn RepoDescriptionIngestPort + Send + Sync>,
    file_ownership: Arc<dyn FileOwnershipIngestPort + Send + Sync>,
    file_contributor: Arc<dyn FileContributorIngestPort + Send + Sync>,
    repo_meta: Arc<dyn RepoMetaIngestPort + Send + Sync>,
    runtime: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
    structural: Arc<dyn StructuralIngestPort + Send + Sync>,
    repomap: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    /// Durable idempotency records (QI-BB-032): every receipt-bearing route
    /// goes through intent → apply → finalize, so a replay of the same body
    /// is answered from the record and a different body under the same key
    /// is refused before any mutation.
    idempotency: Arc<dyn IdempotencyCatalogPort + Send + Sync>,
}

impl SearchPlaneIngestDispatcher {
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "composition-root wiring of one Arc<dyn ...Port> per ingest authority; bundling into a struct is a separate refactor"
    )]
    pub fn new(
        lexical: Arc<dyn SearchCorpusIngestPort + Send + Sync>,
        history: Arc<dyn HistoryIngestPort + Send + Sync>,
        repo_commit_recency: Arc<dyn RepoCommitRecencyIngestPort + Send + Sync>,
        repo_topic: Arc<dyn RepoTopicIngestPort + Send + Sync>,
        repo_description: Arc<dyn RepoDescriptionIngestPort + Send + Sync>,
        file_ownership: Arc<dyn FileOwnershipIngestPort + Send + Sync>,
        file_contributor: Arc<dyn FileContributorIngestPort + Send + Sync>,
        repo_meta: Arc<dyn RepoMetaIngestPort + Send + Sync>,
        runtime: Arc<dyn RuntimeMetadataIngestPort + Send + Sync>,
        structural: Arc<dyn StructuralIngestPort + Send + Sync>,
        repomap: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
        idempotency: Arc<dyn IdempotencyCatalogPort + Send + Sync>,
    ) -> Self {
        Self {
            lexical,
            history,
            repo_commit_recency,
            repo_topic,
            repo_description,
            file_ownership,
            file_contributor,
            repo_meta,
            runtime,
            structural,
            repomap,
            idempotency,
        }
    }

    /// Run one receipt-bearing publish under its idempotency record
    /// (QI-BB-032).
    ///
    /// The canonical body is hashed, the record is begun, and then: a
    /// finalized record with the same body answers with the recorded receipt
    /// marked `applied = false` and nothing runs; a record with a different
    /// body is refused typed by the catalog; otherwise `apply` runs and the
    /// receipt it produces is recorded, stamped with the catalog's durable
    /// sequence, and returned as `applied = true`. A record left in progress
    /// by a crash re-runs `apply`, which every route makes idempotent.
    fn publish_idempotent<B: serde::Serialize>(
        &self,
        kind: IngestOperationKindV1,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        batch_digest: &str,
        body: &B,
        apply: impl FnOnce() -> Result<BatchPublishReceipt, CoreError>,
    ) -> Result<BatchPublishReceipt, CoreError> {
        let key = IdempotencyKeyV1 {
            kind,
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            generation,
            batch_digest: batch_digest.to_string(),
        };
        let body_sha256 = canonical_body_sha256_v1(kind, body)?;
        match self.idempotency.begin(&key, &body_sha256)? {
            IdempotencyBeginV1::Replay {
                receipt,
                durable_sequence,
            } => return Ok(receipt.recorded_at(durable_sequence).replayed()),
            IdempotencyBeginV1::Fresh | IdempotencyBeginV1::Resume => {}
        }
        let receipt = apply()?;
        let durable_sequence = self.idempotency.finalize(&key, &body_sha256, &receipt)?;
        Ok(receipt.recorded_at(durable_sequence))
    }

    /// Serve one ingest request (QI-BB-002).
    ///
    /// The budget is checked once, at entry: a batch whose producer hung up
    /// or whose deadline passed while it queued for the serial dispatch slot
    /// is refused before any track mutates, so the producer's retry starts
    /// from zero bytes changed. Once admitted, a publish runs to its durable
    /// end under the dispatcher's ownership — a batch applied halfway and
    /// then abandoned would leave the generation in a shape no retry can
    /// reason about.
    #[must_use]
    pub fn dispatch(
        &self,
        request: SearchPlaneIngestIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneIngestIpcResponse {
        if let Err(err) = budget.checkpoint("ingest:entry") {
            return SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err));
        }
        match request {
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::SearchCorpus,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.lexical.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::History,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.history.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::HistoryReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::RepoCommitRecency,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.repo_commit_recency.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::RepoTopic,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.repo_topic.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoTopicReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::RepoDescription,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.repo_description.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::FileOwnership,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.file_ownership.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::FileOwnershipReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::FileContributor,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.file_contributor.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::RepoMeta,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.repo_meta.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoMetaReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::Dirty,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.runtime.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::DirtyReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::RuntimeCatalog,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.runtime.publish_catalog_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(batch) => {
                match self.publish_idempotent(
                    IngestOperationKindV1::Structural,
                    &batch.repo_id,
                    &batch.revision_id,
                    batch.generation,
                    &batch.batch_digest,
                    &batch,
                    || self.structural.publish_batch(&batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::StructuralReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoMapBundle(bundle) => {
                match self.repomap.ingest_bundle(&bundle) {
                    Ok(()) => SearchPlaneIngestIpcResponse::RepoMapReceipt(RepoMapMutationAck {
                        repo_id: bundle.repo_id,
                        revision_id: bundle.revision_id,
                        manifest_generation: bundle.manifest_generation,
                    }),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            } // QI-LXB-01 / QI-HIST-01 / QI-RT-02 / QI-STR-02: history /
              // dirty / structural batches now have first-class arms above.
              // No fallback arm needed.
        }
    }
}

/// SHA-256 over the batch's canonical CBOR encoding under a per-route
/// domain: the identity a replay must match byte for byte.
fn canonical_body_sha256_v1<B: serde::Serialize>(
    kind: IngestOperationKindV1,
    body: &B,
) -> Result<[u8; 32], CoreError> {
    use sha2::Digest as _;
    let encoded = quanta_index_ipc::encode_cbor_payload(body).map_err(|err| {
        CoreError::Storage(format!(
            "ingest idempotency: encode {kind} batch body: {err}"
        ))
    })?;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"quanta-index:ingest-batch-body:v1\0");
    hasher.update(kind.as_code_str().as_bytes());
    hasher.update(b"\x1f");
    hasher.update(&encoded);
    Ok(hasher.finalize().into())
}
