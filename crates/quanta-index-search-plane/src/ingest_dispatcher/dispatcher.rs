//! `SearchPlaneIngestDispatcher`: routes typed ingest requests to their owner
//! ports under the idempotency record and the request budget.

use std::sync::Arc;

use quanta_index_contract::{
    BatchPublishReceipt, IngestOperationKindV1, RepoMapMutationAck, RepoMapPublishBundleRequestV2,
    RepoMapTerminalReceiptV2, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse,
    canonical_repo_map_source_bundle_digest_v2,
};
use quanta_index_core::{
    CATALOG_ROW_CORRUPT_CODE, ClaimOutcomeV1, CoreError, FileContributorIngestPort,
    FileOwnershipIngestPort, IdempotencyCatalogPort, IdempotencyKeyV1, IngestBatchBodyV1,
    OperationInspectV1, RepoCommitRecencyIngestPort, RepoDescriptionIngestPort,
    RepoMapBundleIngestPort, RepoMetaIngestPort, RepoTopicIngestPort, RequestBudgetV1,
    SearchCorpusIngestPort,
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
    /// Durable idempotency records (QI-BB-032, SEP-21 P02B): every
    /// receipt-bearing route runs intrinsic validation → terminal
    /// inspect → immutable prepare → mutable preflight → fenced claim →
    /// apply → terminal, so a replay of the same body is answered from
    /// the stored record before any preflight, provider or storage work,
    /// a forged digest is refused before anything durable, and a
    /// frozen-policy refusal is itself a terminal record the retry
    /// replays exactly.
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
    /// 1. **Intrinsic validation.** The carried `batch_digest` is
    ///    recomputed from the body; a forged digest is refused before
    ///    anything durable. Pure: no preflight, no provider, no storage.
    /// 2. **Terminal inspect** (read-only replay check). A committed or
    ///    frozen-refused record answers the replay with its stored
    ///    result — before any mutable preflight runs, so a replay never
    ///    re-inspects storage, config or base state.
    /// 3. **Immutable prepare.** The prepared row is written under the
    ///    verified body digest. A different body under the same key is a
    ///    typed conflict here, with zero mutations.
    /// 4. **Mutable preflight.** Everything the route can refuse without
    ///    mutating — shape, surface authority, resource envelope, delta
    ///    base, and the auxiliary routes' semantic validation against
    ///    the ledger — runs now. A frozen-policy refusal is recorded
    ///    terminally from the prepared mutation (no claim is ever held);
    ///    anything else returns without a terminal record.
    /// 5. **Fenced claim.** `claim_prepared` hands back the claim every
    ///    later fenced step must carry. A terminal record the inspect
    ///    missed in a race replays exactly here.
    /// 6. **Apply.** `mark_applying`, then the route materializes the
    ///    batch under the claim.
    /// 7. **Terminal.** `commit` records the versioned receipt, allocates
    ///    the global durable sequence and its journal event in one
    ///    transaction; a commit that fails after a successful apply
    ///    marks the record `Uncertain`. A typed refusal from the apply
    ///    is recorded with `record_refused` (a frozen-policy refusal,
    ///    exact-replayed by retries); an ambiguous failure marks the
    ///    record `Uncertain` for recovery.
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
        // Stage 1 — intrinsic validation: canonical decode and digest.
        let body_sha256 = verified_batch_digest_v1(body)?;
        let body: &B = body;
        let key = IdempotencyKeyV1 {
            kind: B::OPERATION,
            repo_id: body.repo_id().clone(),
            revision_id: body.revision_id().clone(),
            generation: body.generation(),
            batch_digest: body.batch_digest().to_string(),
        };
        // Stage 2 — terminal inspect before any mutable work.
        match self.idempotency.inspect(&key)? {
            OperationInspectV1::Committed {
                receipt,
                durable_sequence,
            } => return Ok(receipt.recorded_at(durable_sequence).replayed()),
            OperationInspectV1::CommittedRepoMap { .. } => {
                return Err(journal_payload_mismatch(&key));
            }
            OperationInspectV1::Refused { code, message, .. } => {
                // Frozen policy, stored verbatim: no preflight, no
                // storage re-check, no new sequence.
                return Err(CoreError::Typed { code, message });
            }
            OperationInspectV1::Absent
            | OperationInspectV1::InFlight { .. }
            | OperationInspectV1::Uncertain { .. } => {}
        }
        // Stage 3 — immutable prepare.
        let prepared = self.idempotency.prepare(
            &key,
            &body_sha256,
            &claim_owner(),
            INGEST_CLAIM_LEASE_MS,
            &body_sha256,
        )?;
        // Stage 4 — mutable preflight. A frozen-policy refusal is
        // recorded terminally from the prepared mutation: no claim is
        // ever held, and the retry replays the refusal exactly.
        if let Err(error) = preflight(body) {
            if is_frozen_policy_refusal(&error) {
                let _refused = self.idempotency.record_refused(&prepared, &error);
            }
            return Err(error);
        }
        // Stage 5 — fenced claim.
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
            ClaimOutcomeV1::ReplayRepoMap { .. } => return Err(journal_payload_mismatch(&key)),
            ClaimOutcomeV1::Claimed(claim) => claim,
        };
        // Stage 6 — apply under the fence.
        self.idempotency.mark_applying(&claim)?;
        match apply(body) {
            // Stage 7 — terminal.
            Ok(receipt) => match self.idempotency.commit(&claim, &receipt) {
                Ok(durable_sequence) => Ok(receipt.recorded_at(durable_sequence)),
                Err(error) => {
                    let _uncertain = self.idempotency.mark_uncertain(&claim);
                    Err(error)
                }
            },
            Err(error) => {
                if is_frozen_policy_refusal(&error) {
                    let _refused = self.idempotency.record_refused(&claim, &error);
                } else {
                    let _uncertain = self.idempotency.mark_uncertain(&claim);
                }
                Err(error)
            }
        }
    }

    /// Run one RepoMap V2 bundle publish under the operation journal
    /// (SEP-21 P02B). The only journal-bearing RepoMap path: the V1
    /// bundle arm below stays journal-free as the W7 removal target.
    ///
    /// Same seven stages as [`Self::publish_idempotent`], keyed by
    /// `RepoMapBundle` under the source-bundle digest. The terminal
    /// payload is the repo-map terminal receipt (committed through
    /// `commit_repomap`); the key, fence, conflict, floor and sequence
    /// authority is the journal's — the store performs the domain
    /// mutation only and owns no journal of its own on this path.
    fn publish_repomap_v2_idempotent(
        &self,
        request: &RepoMapPublishBundleRequestV2,
    ) -> Result<RepoMapTerminalReceiptV2, CoreError> {
        // Stage 1 — intrinsic validation: the carried source-bundle
        // digest is recomputed from the bundle. Pure.
        let computed = canonical_repo_map_source_bundle_digest_v2(&request.bundle)
            .map_err(|error| CoreError::InvalidContract(format!("repomap V2 publish: {error}")))?;
        if computed != request.source_bundle_digest {
            return Err(CoreError::InvalidContract(format!(
                "repomap V2 publish: source bundle digest mismatch: carried={} computed={computed}",
                request.source_bundle_digest
            )));
        }
        // The journal's 32-byte body identity is the digest itself.
        let body_sha256 = parse_source_bundle_digest_v2(&request.source_bundle_digest)?;
        let bundle = &request.bundle;
        let key = IdempotencyKeyV1 {
            kind: IngestOperationKindV1::RepoMapBundle,
            repo_id: bundle.repo_id.clone(),
            revision_id: bundle.revision_id.clone(),
            generation: bundle.manifest_generation,
            batch_digest: request.source_bundle_digest.clone(),
        };
        // Stage 2 — terminal inspect before any mutable work.
        match self.idempotency.inspect(&key)? {
            OperationInspectV1::CommittedRepoMap { receipt, .. } => {
                return Ok(replayed_repomap_receipt_v2(receipt));
            }
            OperationInspectV1::Committed { .. } => return Err(journal_payload_mismatch(&key)),
            OperationInspectV1::Refused { code, message, .. } => {
                return Err(CoreError::Typed { code, message });
            }
            OperationInspectV1::Absent
            | OperationInspectV1::InFlight { .. }
            | OperationInspectV1::Uncertain { .. } => {}
        }
        // Stage 3 — immutable prepare. Bundle domain validation lives in
        // the store's apply under the fence; there is no
        // dispatcher-level mutable preflight beyond the intrinsic
        // digest, so the prepared mutation passes straight to the
        // fenced claim.
        let _prepared = self.idempotency.prepare(
            &key,
            &body_sha256,
            &claim_owner(),
            INGEST_CLAIM_LEASE_MS,
            &body_sha256,
        )?;
        // Stage 5 — fenced claim.
        let claim = match self.idempotency.claim_prepared(
            &key,
            &body_sha256,
            &claim_owner(),
            INGEST_CLAIM_LEASE_MS,
            &body_sha256,
        )? {
            ClaimOutcomeV1::ReplayRepoMap { receipt, .. } => {
                return Ok(replayed_repomap_receipt_v2(receipt));
            }
            ClaimOutcomeV1::Replay { .. } => return Err(journal_payload_mismatch(&key)),
            ClaimOutcomeV1::Claimed(claim) => claim,
        };
        // Stage 6 — apply under the fence: the store's domain mutation.
        self.idempotency.mark_applying(&claim)?;
        match self.repomap.ingest_bundle_v2(request) {
            // Stage 7 — terminal.
            Ok(receipt) => match self.idempotency.commit_repomap(&claim, &receipt) {
                Ok(_durable_sequence) => Ok(receipt),
                Err(error) => {
                    let _uncertain = self.idempotency.mark_uncertain(&claim);
                    Err(error)
                }
            },
            Err(error) => {
                if is_frozen_policy_refusal(&error) {
                    let _refused = self.idempotency.record_refused(&claim, &error);
                } else {
                    let _uncertain = self.idempotency.mark_uncertain(&claim);
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
                    Ok(receipt) => {
                        SearchPlaneIngestIpcResponse::RepoMapReceipt(RepoMapMutationAck {
                            repo_id: bundle.repo_id,
                            revision_id: bundle.revision_id,
                            manifest_generation: bundle.manifest_generation,
                            prior_candidate_commitment: receipt.prior_candidate_commitment,
                            new_candidate_commitment: receipt.new_candidate_commitment,
                            activation_epoch: receipt.activation_epoch,
                            terminal_sequence: receipt.terminal_sequence,
                            replayed: receipt.replayed,
                        })
                    }
                    Err(err) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishRepoMapBundleV2(request) => {
                // The only journal-bearing RepoMap path (SEP-21 P02B):
                // no arm calls `ingest_bundle_v2` outside
                // `publish_repomap_v2_idempotent`.
                match self.publish_repomap_v2_idempotent(&request) {
                    Ok(receipt) => SearchPlaneIngestIpcResponse::RepoMapTerminalReceiptV2(receipt),
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

/// A typed refusal is frozen policy: recording it lets a retry replay
/// the refusal exactly with no in-progress residue. Anything else
/// (storage, ambiguity — including future [`CoreError`] shapes, which
/// fail to compile here until they are classified) marks the record
/// uncertain for recovery.
fn is_frozen_policy_refusal(error: &CoreError) -> bool {
    matches!(
        error,
        CoreError::Typed { .. } | CoreError::InvalidContract(_)
    )
}

/// A batch key holding a repo-map terminal payload (or the reverse) is
/// journal corruption: kinds and payload decoders are bound together,
/// so this arm is unreachable unless the row was written around the
/// journal.
fn journal_payload_mismatch(key: &IdempotencyKeyV1) -> CoreError {
    CoreError::Typed {
        code: CATALOG_ROW_CORRUPT_CODE,
        message: format!(
            "journal: {} batch_digest={} holds a terminal payload of another operation kind",
            key.kind, key.batch_digest
        ),
    }
}

/// Decode the `sha256:<hex>` source-bundle digest wire token to the
/// journal's 32-byte body identity. The dispatcher already proved the
/// token is the bundle's own digest, so a malformed token here is a
/// contract defect, not a producer retry.
fn parse_source_bundle_digest_v2(token: &str) -> Result<[u8; 32], CoreError> {
    let hex = token.strip_prefix("sha256:").ok_or_else(|| {
        CoreError::InvalidContract(format!(
            "repomap V2 publish: source bundle digest {token:?} has no sha256: prefix"
        ))
    })?;
    if hex.len() != 64 {
        return Err(CoreError::InvalidContract(format!(
            "repomap V2 publish: source bundle digest {token:?} is not 64 hex characters"
        )));
    }
    let mut body = [0_u8; 32];
    for (index, chunk) in hex.as_bytes().chunks_exact(2).enumerate() {
        let &[high, low] = chunk else {
            return Err(CoreError::InvalidContract(format!(
                "repomap V2 publish: source bundle digest {token:?} is not hex pairs"
            )));
        };
        let nibble = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        };
        let (Some(high), Some(low)) = (nibble(high), nibble(low)) else {
            return Err(CoreError::InvalidContract(format!(
                "repomap V2 publish: source bundle digest {token:?} is not lowercase hex"
            )));
        };
        body[index] = (high << 4) | low;
    }
    Ok(body)
}

/// The stored terminal receipt of an earlier apply, re-issued for a
/// replay of the same bundle: every commitment is the original apply's,
/// `replayed` says this call mutated nothing.
fn replayed_repomap_receipt_v2(mut receipt: RepoMapTerminalReceiptV2) -> RepoMapTerminalReceiptV2 {
    receipt.mutation.replayed = true;
    receipt
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
