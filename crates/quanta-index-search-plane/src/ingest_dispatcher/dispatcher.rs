//! `SearchPlaneIngestDispatcher`: routes typed ingest requests to their owner
//! ports under the idempotency record and the request budget.

use std::sync::Arc;

use quanta_index_contract::{
    BatchPublishReceipt, RepoMapMutationAck, SearchPlaneIngestIpcRequest,
    SearchPlaneIngestIpcResponse,
};
use quanta_index_core::{
    ClaimOutcomeV1, CoreError, FileContributorIngestPort, FileOwnershipIngestPort,
    IdempotencyCatalogPort, IdempotencyKeyV1, IngestBatchBodyV1, OperationInspectV1,
    RepoCommitRecencyIngestPort, RepoDescriptionIngestPort, RepoMapBundleIngestPort,
    RepoMetaIngestPort, RepoTopicIngestPort, RequestBudgetV1, SearchCorpusIngestPort,
};
use quanta_index_ipc::{BatchDigestVerdictV1, verify_batch_digest_v1};

use crate::ingest_dispatcher::errors::core_error_to_ipc;
use crate::ingest_dispatcher::ports::{
    HistoryIngestPort, RuntimeMetadataIngestPort, StructuralIngestPort,
};

// =============================================================================
// Top-level dispatcher
// =============================================================================

/// How long one ingest claim holds its journal lease before recovery can
/// abort it.
const INGEST_CLAIM_LEASE_MS: u64 = 30_000;

/// The owner identity every ingest claim of this process carries.
fn claim_owner() -> String {
    format!("searchd-ingest-{}", std::process::id())
}

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
    /// goes through digest verification → preflight → intent → apply →
    /// finalize, so a replay of the same body is answered from the record,
    /// a forged digest is refused before anything durable, and a refused
    /// batch leaves no record.
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

    /// Run one receipt-bearing publish under the operation journal
    /// (QI-BB-032, SEP-21 P02B), in this fixed order:
    ///
    /// 1. **Digest verification.** The carried `batch_digest` is recomputed
    ///    from the body; a forged digest is refused before anything
    ///    durable.
    /// 2. **Preflight.** Everything the route can refuse without mutating —
    ///    shape, surface authority, resource envelope, delta base, and the
    ///    auxiliary routes' semantic validation against the ledger — runs
    ///    now, so a refused batch leaves no record and zero bytes changed.
    /// 3. **Inspect** (read-only replay check). A committed record answers
    ///    the replay with its recorded receipt and nothing runs.
    /// 4. **Fenced claim.** `claim_prepared` writes the immutable prepared
    ///    row and hands back the [`PreparedMutationV1`] every later
    ///    fenced step must carry. A terminal refusal replays exactly; a
    ///    different body under the same key is a conflict; an invalidated
    ///    key is below the replay floor.
    /// 5. **Apply.** The route materializes the batch under the claim.
    /// 6. **Terminal.** `commit` records the versioned receipt, allocates
    ///    the global durable sequence and its journal event in one
    ///    transaction. A typed refusal from the apply is recorded with
    ///    `record_refused` (a frozen-policy refusal, exact-replayed by
    ///    retries); an ambiguous terminal failure marks the record
    ///    `Uncertain` for recovery.
    ///
    /// Because the digest is the body's, two bodies can never share a key;
    /// the catalog's own different-body refusal is its invariant, not a
    /// path this dispatcher can reach.
    fn publish_idempotent<B: IngestBatchBodyV1 + serde::Serialize>(
        &self,
        body: &mut B,
        preflight: impl FnOnce(&B) -> Result<(), CoreError>,
        apply: impl FnOnce(&B) -> Result<BatchPublishReceipt, CoreError>,
    ) -> Result<BatchPublishReceipt, CoreError> {
        let body_sha256 = verified_batch_digest_v1(body)?;
        let body: &B = body;
        preflight(body)?;
        let key = IdempotencyKeyV1 {
            kind: B::OPERATION,
            repo_id: body.repo_id().clone(),
            revision_id: body.revision_id().clone(),
            generation: body.generation(),
            batch_digest: body.batch_digest().to_string(),
        };
        // Replay-first: the read-only inspect answers a committed replay
        // before any storage work.
        if let OperationInspectV1::Committed {
            receipt,
            durable_sequence,
        } = self.idempotency.inspect(&key)?
        {
            return Ok(receipt.recorded_at(durable_sequence).replayed());
        }
        let claim = match self.idempotency.claim_prepared(
            &key,
            &body_sha256,
            &claim_owner(),
            INGEST_CLAIM_LEASE_MS,
            &body_sha256,
        )? {
            ClaimOutcomeV1::Replay {
                receipt,
                durable_sequence,
            } => return Ok(receipt.recorded_at(durable_sequence).replayed()),
            ClaimOutcomeV1::Claimed(claim) => claim,
        };
        self.idempotency.mark_applying(&claim)?;
        match apply(body) {
            Ok(receipt) => match self.idempotency.commit(&claim, &receipt) {
                Ok(durable_sequence) => Ok(receipt.recorded_at(durable_sequence)),
                Err(error) => {
                    let _uncertain = self.idempotency.mark_uncertain(&claim);
                    Err(error)
                }
            },
            Err(error) => {
                // A typed refusal is frozen policy: record it so a retry
                // replays the refusal exactly and no in-progress residue
                // survives. Anything else (storage, ambiguity) is marked
                // uncertain for recovery.
                #[expect(
                    clippy::wildcard_enum_match_arm,
                    reason = "the catch-all is the recovery boundary: every non-typed CoreError shape must land in `mark_uncertain`, including future variants"
                )]
                match &error {
                    CoreError::Typed { .. } | CoreError::InvalidContract(_) => {
                        let _refused = self.idempotency.record_refused(&claim, &error);
                    }
                    _ => {
                        let _uncertain = self.idempotency.mark_uncertain(&claim);
                    }
                }
                Err(error)
            }
        }
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
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(mut batch) => {
                match self.publish_idempotent(
                    &mut batch,
                    |batch| self.lexical.preflight_batch(batch),
                    |batch| self.lexical.publish_batch(batch),
                ) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::SearchCorpusReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishHistoryBatch(mut batch) => {
                match self.publish_idempotent(&mut batch, no_storage_free_preflight, |batch| {
                    self.history.publish_batch(batch)
                }) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::HistoryReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoCommitRecencyBatch(mut batch) => {
                match self.publish_idempotent(&mut batch, no_storage_free_preflight, |batch| {
                    self.repo_commit_recency.publish_batch(batch)
                }) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoCommitRecencyReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoTopicBatch(mut batch) => {
                match self.publish_idempotent(&mut batch, no_storage_free_preflight, |batch| {
                    self.repo_topic.publish_batch(batch)
                }) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoTopicReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoDescriptionBatch(mut batch) => {
                match self.publish_idempotent(&mut batch, no_storage_free_preflight, |batch| {
                    self.repo_description.publish_batch(batch)
                }) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoDescriptionReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishFileOwnershipBatch(mut batch) => {
                match self.publish_idempotent(&mut batch, no_storage_free_preflight, |batch| {
                    self.file_ownership.publish_batch(batch)
                }) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::FileOwnershipReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishFileContributorBatch(mut batch) => {
                match self.publish_idempotent(&mut batch, no_storage_free_preflight, |batch| {
                    self.file_contributor.publish_batch(batch)
                }) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::FileContributorReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoMetaBatch(mut batch) => {
                match self.publish_idempotent(&mut batch, no_storage_free_preflight, |batch| {
                    self.repo_meta.publish_batch(batch)
                }) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoMetaReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishDirtyBatch(mut batch) => {
                match self.publish_idempotent(&mut batch, no_storage_free_preflight, |batch| {
                    self.runtime.publish_batch(batch)
                }) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::DirtyReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRuntimeCatalogBatch(mut batch) => {
                match self.publish_idempotent(&mut batch, no_storage_free_preflight, |batch| {
                    self.runtime.publish_catalog_batch(batch)
                }) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RuntimeCatalogReceipt(receipt),
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishStructuralBatch(mut batch) => {
                match self.publish_idempotent(&mut batch, no_storage_free_preflight, |batch| {
                    self.structural.publish_batch(batch)
                }) {
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

/// The preflight of a route whose refusals are only discoverable inside
/// its apply.
///
/// The auxiliary authority routes compute their delta against the ledger
/// and refuse from there. A typed refusal inside such an apply leaves an
/// in-progress record that dies with its generation at the next reclaim
/// pass; the digest is verified before the record either way.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature is the route preflight contract every route is dispatched through; this route has nothing storage-free to refuse"
)]
fn no_storage_free_preflight<B: IngestBatchBodyV1>(_body: &B) -> Result<(), CoreError> {
    Ok(())
}

/// Recompute `body`'s canonical digest and require the carried token to be
/// it (QI-BB-032).
///
/// The verified bytes are the idempotency record's body hash. A body that
/// cannot be encoded is a contract defect of the batch.
fn verified_batch_digest_v1<B: IngestBatchBodyV1 + serde::Serialize>(
    body: &mut B,
) -> Result<[u8; 32], CoreError> {
    let verdict = verify_batch_digest_v1(body).map_err(|err| {
        CoreError::InvalidContract(format!(
            "ingest: encode {} batch body for its digest: {err}",
            B::OPERATION
        ))
    })?;
    match verdict {
        BatchDigestVerdictV1::Verified(digest) => Ok(digest),
        BatchDigestVerdictV1::Mismatch { carried, expected } => Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::BatchDigestMismatch,
            message: format!(
                "{} batch for repo={} revision={} generation={} carries batch_digest={carried} but its body digests to {expected}; a batch digest is the canonical digest of the body it names, computed after the body is final",
                B::OPERATION,
                body.repo_id().as_str(),
                body.revision_id().as_str(),
                body.generation().get(),
            ),
        }),
    }
}
