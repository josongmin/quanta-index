//! `SearchPlaneIngestDispatcher`: routes typed ingest requests to their owner
//! ports under the idempotency record and the request budget.

use std::sync::{Arc, Mutex};

use quanta_index_contract::{
    BatchPublishReceipt, IngestOperationKindV1, RepoMapPublishBundleRequestV2,
    RepoMapTerminalReceiptV2, SearchPlaneIngestIpcRequest, SearchPlaneIngestIpcResponse,
    canonical_repo_map_source_bundle_digest_v2,
};
use quanta_index_core::{
    CATALOG_ROW_CORRUPT_CODE, ClaimOutcomeV1, CoreError, FileContributorIngestPort,
    FileOwnershipIngestPort, IdempotencyCatalogPort, IdempotencyKeyV1, IngestBatchBodyV1,
    OperationInspectV1, RepoCommitRecencyIngestPort, RepoDescriptionIngestPort,
    RepoMapBundleIngestPort, RepoMetaIngestPort, RepoTopicIngestPort, RequestBudgetV1,
    SearchCorpusIngestPort, now_unix_ms,
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
/// abort it. The catalog takes an absolute Unix-millisecond deadline.
const INGEST_CLAIM_LEASE_MS: u64 = 30_000;

fn ingest_claim_deadline_ms() -> Result<u64, CoreError> {
    now_unix_ms()
        .checked_add(INGEST_CLAIM_LEASE_MS)
        .ok_or_else(|| CoreError::Storage("ingest claim deadline overflow".to_string()))
}

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
    source_publication: Arc<dyn quanta_index_core::SourcePublicationCatalogPort>,
    source_authority: Arc<dyn super::ports::SearchCorpusAuthorityInspectPort>,
    source_upload: Option<Arc<dyn quanta_index_core::SourcePublicationUploadPort>>,
    source_upload_commit: Mutex<()>,
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
        source_publication: Arc<dyn quanta_index_core::SourcePublicationCatalogPort>,
        source_authority: Arc<dyn super::ports::SearchCorpusAuthorityInspectPort>,
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
            source_publication,
            source_authority,
            source_upload: None,
            source_upload_commit: Mutex::new(()),
        }
    }

    #[must_use]
    pub fn with_source_upload(
        mut self,
        upload: Arc<dyn quanta_index_core::SourcePublicationUploadPort>,
    ) -> Self {
        self.source_upload = Some(upload);
        self
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
    ///    the ledger — runs now. Non-source frozen-policy refusals are
    ///    recorded terminally. Source refusals retain Prepared, so a retry
    ///    cannot permanently strand an earlier Pending event.
    /// 5. **Fenced claim.** `claim_prepared` hands back the claim every
    ///    later fenced step must carry. A terminal record the inspect
    ///    missed in a race replays exactly here.
    /// 6. **Apply.** `mark_applying`, then the route materializes the
    ///    batch under the claim.
    /// 7. **Terminal.** `commit` records the versioned receipt, allocates
    ///    the global durable sequence and its journal event in one
    ///    transaction; a commit that fails after a successful apply
    ///    marks the record `Uncertain`. Source-corpus apply errors are
    ///    also uncertain: a typed error can follow reservation or partial
    ///    track work, so it is not a terminal nonpublication proof. Other
    ///    routes retain their frozen-policy typed-refusal contract.
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
        self.publish_idempotent_verified(body, body_sha256, preflight, apply)
    }

    /// Continue after this exact body was verified. The source-event path
    /// verifies before its replay lookup and can pass the same digest here;
    /// no caller may mutate the batch between that verification and apply.
    fn publish_idempotent_verified<B: IngestBatchBodyV1>(
        &self,
        body: &B,
        body_sha256: [u8; 32],
        preflight: impl FnOnce(&B) -> Result<(), CoreError>,
        apply: impl FnOnce(&B) -> Result<BatchPublishReceipt, CoreError>,
    ) -> Result<BatchPublishReceipt, CoreError> {
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
        // Stage 3 — immutable prepare. The catalog compares this value to
        // Unix time; passing the lease duration here expires immediately.
        let owner = claim_owner();
        let prepare_deadline_ms = ingest_claim_deadline_ms()?;
        let prepared = self.idempotency.prepare(
            &key,
            &body_sha256,
            &owner,
            prepare_deadline_ms,
            &body_sha256,
        )?;
        // Stage 4 — source preflight remains retryable. A prior attempt can
        // already own a Pending event and partially materialized tracks; a
        // current policy/base refusal cannot prove terminal nonpublication.
        // Leave Prepared for the existing fenced retry path. Other routes
        // keep their immutable policy-refusal receipts.
        if let Err(error) = preflight(body) {
            if B::OPERATION != IngestOperationKindV1::SearchCorpus
                && is_frozen_policy_refusal(&error)
            {
                let _refused = self.idempotency.record_refused(&prepared, &error)?;
            }
            return Err(error);
        }
        // Stage 5 — fenced claim.
        // Preflight can take time. Start the applying claim's lease when it
        // is actually taken, rather than spending it during preflight.
        let claim_deadline_ms = ingest_claim_deadline_ms()?;
        let claim = match self.idempotency.claim_prepared(
            &key,
            &body_sha256,
            &owner,
            claim_deadline_ms,
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
                    self.idempotency.mark_uncertain(&claim)?;
                    Err(error)
                }
            },
            Err(error) => {
                // Once source materialization has started, even a typed
                // conflict can be transient (for example a pinned repair).
                // Preserve the reservation and retry its original journal;
                // freezing this error would strand the stream permanently.
                if B::OPERATION != IngestOperationKindV1::SearchCorpus
                    && is_frozen_policy_refusal(&error)
                {
                    let _refused = self.idempotency.record_refused(&claim, &error)?;
                } else {
                    self.idempotency.mark_uncertain(&claim)?;
                }
                Err(error)
            }
        }
    }

    /// Run one `RepoMap` V2 bundle publish under the operation journal
    /// (SEP-21 P02B). The only journal-bearing `RepoMap` path: the V1
    /// bundle arm below stays journal-free as the W7 removal target.
    ///
    fn publish_source_idempotent(
        &self,
        batch: &mut quanta_index_contract::SearchCorpusIngestBatch,
        apply: impl FnOnce(
            &quanta_index_contract::SearchCorpusIngestBatch,
            &mut quanta_index_core::PublicationValidationOwner,
        ) -> Result<BatchPublishReceipt, CoreError>,
    ) -> Result<
        (
            BatchPublishReceipt,
            quanta_index_contract::SourcePublicationBinding,
        ),
        CoreError,
    > {
        let body_digest = verified_batch_digest_v1(batch)?;
        batch
            .validate_v1()
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        batch
            .validate_surface_mutations_v1()
            .map_err(|error| CoreError::InvalidContract(error.to_string()))?;
        if let Some(record) = self
            .source_publication
            .inspect_source_event(&batch.repo_id, &batch.source_event)?
        {
            if record.phase != quanta_index_core::SourceEventPhaseV1::Pending {
                let target = &record.binding.target;
                if self.source_authority.inspect_sealed_search_corpus(
                    &target.repo_id,
                    &target.revision_id,
                    target.manifest_generation,
                    &target.manifest_digest,
                )? != crate::SealedSearchCorpusAuthorityStateV1::Exact
                {
                    return Err(CoreError::Typed {
                        code: quanta_index_contract::SearchPlaneErrorCodeV2::UnknownGeneration,
                        message: "source event original generation is no longer retained".into(),
                    });
                }
            }
            match self.idempotency.inspect(&record.binding.journal_key)? {
                OperationInspectV1::Committed {
                    receipt,
                    durable_sequence,
                } => {
                    let target = &record.binding.target;
                    if self.source_authority.inspect_sealed_search_corpus(
                        &target.repo_id,
                        &target.revision_id,
                        target.manifest_generation,
                        &target.manifest_digest,
                    )? != crate::SealedSearchCorpusAuthorityStateV1::Exact
                    {
                        return Err(CoreError::Typed {
                            code: quanta_index_contract::SearchPlaneErrorCodeV2::UnknownGeneration,
                            message: "source event original generation is no longer retained"
                                .into(),
                        });
                    }
                    let reconciled = self.source_publication.reconcile_source_event(
                        &batch.repo_id,
                        &batch.source_event,
                        self.idempotency.as_ref(),
                    )?;
                    return Ok((
                        receipt.recorded_at(durable_sequence).replayed(),
                        publication_binding(&reconciled.binding),
                    ));
                }
                OperationInspectV1::CommittedRepoMap { .. } => {
                    return Err(journal_payload_mismatch(&record.binding.journal_key));
                }
                OperationInspectV1::Refused { code, message, .. } => {
                    return Err(CoreError::Typed { code, message });
                }
                OperationInspectV1::Absent
                | OperationInspectV1::InFlight { .. }
                | OperationInspectV1::Uncertain { .. } => {
                    if record.phase != quanta_index_core::SourceEventPhaseV1::Pending {
                        return Err(CoreError::Typed {
                            code: quanta_index_contract::SearchPlaneErrorCodeV2::UnknownGeneration,
                            message:
                                "source event original journal or generation is no longer available"
                                    .into(),
                        });
                    }
                    if record.binding.journal_key.revision_id != batch.revision_id
                        || record.binding.journal_key.generation != batch.generation
                        || record.binding.journal_key.batch_digest != batch.batch_digest
                    {
                        return Err(CoreError::Typed { code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogBusy, message: "source event is pending; reconcile the original publication without retargeting it".into() });
                    }
                }
            }
        }
        // The materializer reserves the event under its operation lock after
        // physical repair admission and before either track mutates. Reserving
        // here would consume the stream even when apply refuses a damaged
        // target before doing any work. Replay and reconciliation still use
        // this same catalog and the original operation journal.
        // This scope dies with this attempt, including refusals and replay
        // races. The synchronous journal stages borrow it sequentially.
        let validation =
            std::cell::RefCell::new(quanta_index_core::PublicationValidationOwner::default());
        let receipt = self.publish_idempotent_verified(
            batch,
            body_digest,
            |batch| {
                let mut owner = validation.try_borrow_mut().map_err(|error| {
                    CoreError::Storage(format!("source publication proof custody: {error}"))
                })?;
                self.lexical.preflight_batch_with_owner(batch, &mut owner)
            },
            |batch| {
                let mut owner = validation.try_borrow_mut().map_err(|error| {
                    CoreError::Storage(format!("source publication proof custody: {error}"))
                })?;
                apply(batch, &mut owner)
            },
        )?;
        let reconciled = self.source_publication.reconcile_source_event(
            &batch.repo_id,
            &batch.source_event,
            self.idempotency.as_ref(),
        )?;
        Ok((receipt, publication_binding(&reconciled.binding)))
    }

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
        let owner = claim_owner();
        let prepare_deadline_ms = ingest_claim_deadline_ms()?;
        let _prepared = self.idempotency.prepare(
            &key,
            &body_sha256,
            &owner,
            prepare_deadline_ms,
            &body_sha256,
        )?;
        // Stage 5 — fenced claim.
        let claim_deadline_ms = ingest_claim_deadline_ms()?;
        let claim = match self.idempotency.claim_prepared(
            &key,
            &body_sha256,
            &owner,
            claim_deadline_ms,
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
        match self.repomap.ingest_bundle(request) {
            // Stage 7 — terminal.
            Ok(receipt) => match self.idempotency.commit_repomap(&claim, &receipt) {
                Ok(_durable_sequence) => Ok(receipt),
                Err(error) => {
                    self.idempotency.mark_uncertain(&claim)?;
                    Err(error)
                }
            },
            Err(error) => {
                if is_frozen_policy_refusal(&error) {
                    let _refused = self.idempotency.record_refused(&claim, &error)?;
                } else {
                    self.idempotency.mark_uncertain(&claim)?;
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
            SearchPlaneIngestIpcRequest::StageSourcePublication(part) => {
                let result = self
                    .source_upload
                    .as_ref()
                    .ok_or_else(|| {
                        CoreError::InvalidContract(
                            "source publication staging is unavailable".into(),
                        )
                    })
                    .and_then(|upload| upload.stage(&part, budget));
                match result {
                    Ok(ack) => SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(ack),
                    Err(error) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(error)),
                }
            }
            SearchPlaneIngestIpcRequest::DiscardSourcePublicationUpload(identity) => {
                let result = self
                    .source_upload
                    .as_ref()
                    .ok_or_else(|| {
                        CoreError::InvalidContract(
                            "source publication staging is unavailable".into(),
                        )
                    })
                    .and_then(|upload| upload.discard(identity));
                match result {
                    Ok(()) => SearchPlaneIngestIpcResponse::SourcePublicationUploadAck(
                        quanta_index_contract::SourcePublicationUploadAck {
                            identity,
                            next_offset: 0,
                        },
                    ),
                    Err(error) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(error)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishStagedSourcePublication(commit) => {
                let result = self
                    .source_upload_commit
                    .try_lock()
                    .map_err(|_error| CoreError::Typed {
                        code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogBusy,
                        message: "another staged source publication is materializing".into(),
                    })
                    .and_then(|_operation| {
                        let upload = self.source_upload.as_ref().ok_or_else(|| {
                            CoreError::InvalidContract(
                                "source publication staging is unavailable".into(),
                            )
                        })?;
                        let batch = upload.load(commit.identity, budget)?;
                        if quanta_index_contract::SourcePublicationBinding::for_batch(&batch)
                            != commit.publication
                        {
                            return Err(CoreError::InvalidContract(
                                "staged source publication binding mismatch".into(),
                            ));
                        }
                        let response = self.dispatch(
                            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(batch),
                            budget,
                        );
                        if matches!(
                            &response,
                            SearchPlaneIngestIpcResponse::SearchCorpusReceipt(_)
                        ) {
                            upload.discard(commit.identity)?;
                        }
                        Ok(response)
                    });
                match result {
                    Ok(response) => response,
                    Err(error) => SearchPlaneIngestIpcResponse::Error(core_error_to_ipc(error)),
                }
            }
            SearchPlaneIngestIpcRequest::PublishSearchCorpusBatch(mut batch) => {
                let mut observation = None;
                match self.publish_source_idempotent(&mut batch, |batch, owner| {
                    let outcome = self
                        .lexical
                        .publish_batch_with_owner(batch, budget, owner)?;
                    outcome
                        .publication
                        .validate_receipt(
                            &quanta_index_contract::SourcePublicationBinding::for_batch(batch),
                            batch.seal,
                            &outcome.receipt,
                        )
                        .map_err(CoreError::InvalidContract)?;
                    observation = outcome.observation;
                    Ok(outcome.receipt)
                }) {
                    Ok((receipt, publication)) => {
                        if !receipt.applied {
                            observation =
                                Some(quanta_index_contract::SearchCorpusIngestObservation {
                                    request_id: budget.response_request_id(),
                                    repo_id: batch.repo_id.clone(),
                                    revision_id: batch.revision_id.clone(),
                                    generation: batch.generation,
                                    batch_digest: batch.batch_digest.clone(),
                                    status:
                                        quanta_index_contract::IngestObservationStatus::Replayed,
                                    semantic: None,
                                    lexical_build_ns: None,
                                    lexical_stages: None,
                                    finalize_ns: None,
                                    activation_ns: None,
                                });
                        }
                        SearchPlaneIngestIpcResponse::SearchCorpusReceipt(
                            quanta_index_contract::SearchCorpusPublishOutcome {
                                publication,
                                receipt,
                                observation,
                            },
                        )
                    }
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

/// Only a typed refusal is frozen policy.
///
/// The catalog's terminal refusal transition requires `CoreError::Typed`;
/// an internal contract error stays retryable.
///
/// Recording it lets a retry replay the refusal exactly with no
/// in-progress residue. Other apply failures mark the record uncertain;
/// preflight failures leave a prepared row for a later attempt.
fn is_frozen_policy_refusal(error: &CoreError) -> bool {
    matches!(error, CoreError::Typed { .. })
}

/// A batch key holding a repo-map terminal payload (or the reverse).
///
/// That is journal corruption: kinds and payload decoders are bound
/// together, so this arm is unreachable unless the row was written
/// around the journal.
fn journal_payload_mismatch(key: &IdempotencyKeyV1) -> CoreError {
    CoreError::Typed {
        code: CATALOG_ROW_CORRUPT_CODE,
        message: format!(
            "journal: {} batch_digest={} holds a terminal payload of another operation kind",
            key.kind, key.batch_digest
        ),
    }
}

/// Decode the `sha256:<hex>` source-bundle digest wire token.
///
/// The target is the journal's 32-byte body identity. The dispatcher
/// already proved the token is the bundle's own digest, so a malformed
/// token here is a contract defect, not a producer retry.
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
    // `hex` is exactly 64 bytes, so there are exactly 32 pairs for the
    // 32 slots; `zip` ends the loop with the shorter side, which cannot
    // happen here.
    for (slot, chunk) in body.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
        let &[high, low] = chunk else {
            return Err(CoreError::InvalidContract(format!(
                "repomap V2 publish: source bundle digest {token:?} is not hex pairs"
            )));
        };
        let nibble = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte.wrapping_sub(b'0')),
            b'a'..=b'f' => Some(byte.wrapping_sub(b'a').wrapping_add(10)),
            _ => None,
        };
        let (Some(high), Some(low)) = (nibble(high), nibble(low)) else {
            return Err(CoreError::InvalidContract(format!(
                "repomap V2 publish: source bundle digest {token:?} is not lowercase hex"
            )));
        };
        *slot = (high << 4) | low;
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

fn publication_binding(
    binding: &quanta_index_core::SourceEventBindingV1,
) -> quanta_index_contract::SourcePublicationBinding {
    quanta_index_contract::SourcePublicationBinding {
        event: binding.event.clone(),
        target: binding.target.clone(),
        batch_digest: binding.journal_key.batch_digest.clone(),
    }
}
