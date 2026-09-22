//! The `RepoMap` generation store under SQLite-catalog authority
//! (SEP-21 S21-01B/S21-02, P03).
//!
//! The catalog is the sole candidate/activation visibility authority; the
//! filesystem under the layout root holds immutable content-addressed
//! object projections only. Publish seals a candidate object and commits
//! its catalog row (one allocator event + domain row per transaction);
//! activation is a content-bound CAS over the exact sealed commitment;
//! the in-memory registry is a cache published only after the catalog
//! commit. A legacy V1 layout root (`activations/`/`snapshots/`
//! directories) is never mutated here: every mutation is refused typed
//! with `STATE_ROOT_FORMAT_UNSUPPORTED` before anything changes, and the
//! legacy bytes/inodes/mtimes stay untouched for the P10 offline
//! importer.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use quanta_index_contract::{
    CandidateCommitmentV1, LogicalGenerationIdentityV1, ManifestGeneration, RepoId,
    RepositoryRevisionIdentityV1, RevisionId,
};
use quanta_index_contract::{
    RepoMapActivateGenerationRequest, RepoMapActivateGenerationRequestV2, RepoMapMutationAck,
    RepoMapMutationPhaseV2, RepoMapPublishBundleRequestV2, RepoMapSourceBundle,
    RepoMapTerminalReceiptV2, canonical_repo_map_source_bundle_digest_v2,
};
use quanta_index_core::{
    CoreError, QuarantineDiscardOutcomeV1, QuarantinedRepoMapFileV1, RepoMapBundleIngestPort,
    RepoMapGenerationActivatePort, RepoMapMutationReceiptV1, RepoMapOpenReportV1,
    RepoMapQuarantinePort,
};
use quanta_index_core::{
    PinnedRepoMapSnapshot, RepoMapSnapshotAcquirePort, RepoMapSnapshotAcquireV1,
    RepoMapSnapshotEvidenceV1,
};

use quanta_index_catalog::SqliteCatalog;

use crate::materializer::{
    CandidateProjectionMetaV1, RepoMapGraphCompiler, decode_compiled_payload,
    snapshot_from_projection,
};
use crate::model::{RepoMapIndexedSnapshot, RepoMapSnapshot};
use crate::object_store::{
    AFTER_CATALOG_COMMIT, LEGACY_V1_DIR_NAMES, RepoMapObjectStore, exit_at_crash_boundary,
};
use crate::pinned::{PinnedRepoMapSnapshotV1, RepoMapPinLease, RepoMapStoreKeyV1};

#[derive(Debug)]
pub struct RepoMapGenerationStore {
    root: PathBuf,
    catalog: Arc<SqliteCatalog>,
    objects: Option<RepoMapObjectStore>,
    snapshots: RwLock<BTreeMap<RepoMapStoreKeyV1, Arc<RepoMapIndexedSnapshot>>>,
    activated: RwLock<BTreeMap<(String, String), ActivatedHeadV1>>,
    /// The pin table: how many pinned read views hold each logical
    /// generation (S21-05). Physical GC and republish defer to it.
    pins: Arc<RwLock<BTreeMap<RepoMapStoreKeyV1, u64>>>,
}

/// The serving head of one repo/revision as the acquisition critical
/// section sees it: the active generation with the candidate commitment
/// and activation epoch a pinned view carries as evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ActivatedHeadV1 {
    manifest_generation: u64,
    epoch: u64,
    candidate_commitment: [u8; 32],
}

/// What one GC pass over retired candidates did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RepoMapGcOutcomeV1 {
    /// Retired (activation-invalidated) candidates the pass considered.
    pub considered: u64,
    /// Objects physically reclaimed this pass.
    pub reclaimed: u64,
    /// Candidates kept because a pinned read view still holds them.
    pub deferred_pinned: u64,
}

/// A store opened from its root and catalog, with what the open found.
#[derive(Debug)]
pub struct OpenedRepoMapStore {
    pub store: RepoMapGenerationStore,
    pub report: RepoMapOpenReportV1,
}

fn storage_poisoned(what: &str, err: &dyn std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("repomap store {what} poisoned: {err}"))
}

fn legacy_root_refusal(root: &Path) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::StateRootFormatUnsupported,
        message: format!(
            "repomap store refuses to mutate legacy layout root {} ({} directories present); \
             the P10 offline importer owns legacy artifacts",
            root.display(),
            LEGACY_V1_DIR_NAMES.join("/")
        ),
    }
}

fn logical_identity(
    repo_id: &RepoId,
    revision_id: &RevisionId,
    generation: ManifestGeneration,
) -> LogicalGenerationIdentityV1 {
    LogicalGenerationIdentityV1::new(
        RepositoryRevisionIdentityV1::new(repo_id.clone(), revision_id.clone()),
        generation.get(),
    )
}

impl RepoMapGenerationStore {
    /// Open the store under `root` with the shared durable catalog.
    ///
    /// Reconciles the catalog's candidate/activation/invalidation rows
    /// against the immutable object projections exactly: every activated
    /// candidate whose object verifies is published to the registry and
    /// the activation is served; every candidate whose object is missing
    /// or corrupt gets a durable quarantine incident (envelope + payload
    /// projections, then source unlink), its activation is durably
    /// invalidated when one is active, and nothing about it is ever
    /// served or re-activated by file reappearance alone.
    pub fn open(
        root: impl AsRef<Path>,
        catalog: Arc<SqliteCatalog>,
    ) -> Result<OpenedRepoMapStore, CoreError> {
        let root = root.as_ref().to_path_buf();
        let mut report = RepoMapOpenReportV1::default();
        let legacy = LEGACY_V1_DIR_NAMES
            .iter()
            .any(|name| root.join(name).exists());
        if legacy {
            // A legacy V1 root is observed, never touched: no object store
            // is created, no directory is made, every mutation is refused.
            return Ok(OpenedRepoMapStore {
                store: Self {
                    root,
                    catalog,
                    objects: None,
                    snapshots: RwLock::new(BTreeMap::new()),
                    activated: RwLock::new(BTreeMap::new()),
                    pins: Arc::new(RwLock::new(BTreeMap::new())),
                },
                report,
            });
        }
        let objects = RepoMapObjectStore::open(&root)?;
        let store = Self {
            root,
            catalog,
            objects: Some(objects),
            snapshots: RwLock::new(BTreeMap::new()),
            activated: RwLock::new(BTreeMap::new()),
            pins: Arc::new(RwLock::new(BTreeMap::new())),
        };
        store.reconcile(&mut report)?;
        Ok(OpenedRepoMapStore { store, report })
    }

    /// The boot/open reconcile: catalog rows against object digests.
    fn reconcile(&self, report: &mut RepoMapOpenReportV1) -> Result<(), CoreError> {
        let objects = self
            .objects
            .as_ref()
            .ok_or_else(|| legacy_root_refusal(&self.root))?;
        let candidates = self.catalog.repomap_candidate_rows()?;
        for candidate in &candidates {
            // A quarantined candidate was already unlinked durably; an
            // activation-invalidated (retired) candidate may additionally
            // have had its object reclaimed by the pin-gated GC pass
            // (S21-05). Neither is serveable and neither may resurrect,
            // so neither is verified or loaded here: absence is the
            // expected terminal condition, not corruption.
            if candidate.state == quanta_index_catalog::RepoMapCandidateStateV1::Quarantined
                || candidate.state
                    == quanta_index_catalog::RepoMapCandidateStateV1::ActivationInvalidated
            {
                continue;
            }
            let repo_id =
                RepoId::new(candidate.repo_id.as_str()).map_err(|error| CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    message: format!("repomap candidate row holds an invalid repo ID: {error}"),
                })?;
            let revision_id = RevisionId::new(candidate.revision_id.as_str()).map_err(|error| {
                CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    message: format!("repomap candidate row holds an invalid revision ID: {error}"),
                }
            })?;
            let digest = quanta_index_contract::CandidateObjectDigestV1::from_bytes(
                candidate.object_address,
            );
            match objects.verify(digest, &candidate.candidate_commitment) {
                Ok(envelope) => {
                    let meta =
                        CandidateProjectionMetaV1::from_json(candidate.projection_meta.as_str())?;
                    let (_graph, projection) =
                        decode_compiled_payload(envelope.compiled_payload())?;
                    let snapshot = snapshot_from_projection(
                        &repo_id,
                        &revision_id,
                        ManifestGeneration::new(candidate.manifest_generation),
                        &meta,
                        &projection,
                    );
                    let key = RepoMapStoreKeyV1::new(
                        &repo_id,
                        &revision_id,
                        ManifestGeneration::new(candidate.manifest_generation),
                    );
                    let indexed = Arc::new(RepoMapIndexedSnapshot::new(snapshot));
                    let _prior = self
                        .snapshots
                        .write()
                        .map_err(|err| storage_poisoned("registry", &err))?
                        .insert(key, indexed);
                    report.snapshots_loaded = report.snapshots_loaded.saturating_add(1);
                }
                Err(failure) => {
                    // Durable quarantine: catalog incident (exact envelope
                    // bytes, digest, sequence), projections, then unlink.
                    self.quarantine_candidate_object(candidate, &failure, report)?;
                }
            }
        }
        // Serve-head pointers: only a verified, still-activated candidate
        // is active truth.
        let mut reactivations: Vec<((String, String), ActivatedHeadV1)> = Vec::new();
        for candidate in &candidates {
            if candidate.state != quanta_index_catalog::RepoMapCandidateStateV1::Activated {
                continue;
            }
            if let Some(activation) = self.catalog.repomap_activation_row(
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
            )? && activation.active
                && activation.candidate_commitment == candidate.candidate_commitment
                && activation.manifest_generation == candidate.manifest_generation
            {
                report.activations_loaded = report.activations_loaded.saturating_add(1);
                reactivations.push((
                    (candidate.repo_id.clone(), candidate.revision_id.clone()),
                    ActivatedHeadV1 {
                        manifest_generation: candidate.manifest_generation,
                        epoch: activation.epoch,
                        candidate_commitment: activation.candidate_commitment,
                    },
                ));
            }
        }
        if !reactivations.is_empty() {
            let mut activated = self
                .activated
                .write()
                .map_err(|err| storage_poisoned("activation map", &err))?;
            for (key, generation) in reactivations {
                let _prior = activated.insert(key, generation);
            }
        }
        Ok(())
    }

    /// The durable quarantine flow for one candidate object that failed
    /// verification: incident envelope + payload projections first, then
    /// the source unlink, then the durable invalidation. A crash at any
    /// boundary leaves the catalog row (already durable) as authority:
    /// the candidate is unserveable and never resurrects by file
    /// reappearance.
    fn quarantine_candidate_object(
        &self,
        candidate: &quanta_index_catalog::RepoMapCandidateRowV1,
        failure: &crate::object_store::ObjectVerificationFailureV1,
        report: &mut RepoMapOpenReportV1,
    ) -> Result<(), CoreError> {
        let objects = self
            .objects
            .as_ref()
            .ok_or_else(|| legacy_root_refusal(&self.root))?;
        let digest =
            quanta_index_contract::CandidateObjectDigestV1::from_bytes(candidate.object_address);
        let address = crate::layout_v3::CandidateObjectAddressV1::new(digest);
        let relative = address.relative_path();
        let components: Vec<Vec<u8>> = relative
            .components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_string_lossy()
                    .into_owned()
                    .into_bytes()
            })
            .collect();
        let payload_digest = failure
            .raw_bytes
            .as_deref()
            .map(quanta_index_contract::QuarantinePayloadDigestV1::for_payload);
        let observed_nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0_u128, |elapsed| elapsed.as_nanos());
        let observed_nanos = u64::try_from(observed_nanos).map_or(0, |nanos| nanos);
        let evidence = quanta_index_contract::QuarantineObservationEvidenceV1::new(
            components,
            failure.observed_byte_size,
            payload_digest,
            Some(digest),
            failure.reason,
            objects.uuid_commitment(),
        )
        .map_err(|error| {
            CoreError::Storage(format!("repomap quarantine evidence refused: {error}"))
        })?;
        let incident_digest = *evidence
            .digest()
            .map_err(|error| {
                CoreError::Storage(format!(
                    "repomap quarantine evidence digest refused: {error}"
                ))
            })?
            .as_bytes();
        let source_path = relative.to_string_lossy().into_owned();
        let reason_code = failure.reason.code();
        let reason_text = format!("{}/{}", u64::from(failure.reason.code()), failure.detail);
        let incident = self.catalog.record_repomap_quarantine_incident(
            &incident_digest,
            payload_digest
                .unwrap_or_else(|| {
                    quanta_index_contract::QuarantinePayloadDigestV1::from_bytes([0_u8; 32])
                })
                .as_bytes(),
            i64::try_from(observed_nanos).map_or(0, |nanos| nanos),
            &format!("quarantine-reason-{reason_code}"),
            source_path.as_str(),
            &|sequence| {
                let incident = quanta_index_contract::QuarantineIncidentV1::new(
                    u64::try_from(sequence).map_or(0, |sequence| sequence),
                    observed_nanos,
                    evidence.clone(),
                )
                .map_err(|error| {
                    CoreError::Storage(format!("repomap quarantine incident refused: {error}"))
                })?;
                let digest = incident.digest().map_err(|error| {
                    CoreError::Storage(format!(
                        "repomap quarantine envelope digest refused: {error}"
                    ))
                })?;
                let bytes = incident.encode_canonical().map_err(|error| {
                    CoreError::Storage(format!(
                        "repomap quarantine envelope encode refused: {error}"
                    ))
                })?;
                Ok((*digest.as_bytes(), bytes))
            },
        )?;
        let quarantine_entry = quarantine_entry_name(&incident.incident_digest);
        let incident = quanta_index_contract::QuarantineIncidentV1::new(
            u64::try_from(incident.sequence).map_or(0, |sequence| sequence),
            observed_nanos,
            evidence,
        )
        .map_err(|error| {
            CoreError::Storage(format!("repomap quarantine incident refused: {error}"))
        })?;
        // Durable projections (create-new, exact-byte replay, fsync), then
        // the source unlink + source-directory fsync.
        let _projected = objects.project_quarantine(&incident, failure.raw_bytes.as_deref())?;
        objects.unlink_object(digest)?;
        // Durable invalidation: an active activation becomes unserveable;
        // a sealed-only candidate becomes terminal-quarantined.
        if let Some(activation) = self
            .catalog
            .repomap_activation_row(candidate.repo_id.as_str(), candidate.revision_id.as_str())?
            && activation.active
            && activation.manifest_generation == candidate.manifest_generation
        {
            let _invalidated = self.catalog.invalidate_repomap_activation(
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
                candidate.manifest_generation,
                &reason_text,
            )?;
            report.activations_without_snapshot.push(format!(
                "repo={} revision={} generation={} reason={}",
                candidate.repo_id,
                candidate.revision_id,
                candidate.manifest_generation,
                reason_text
            ));
        } else {
            let _quarantined = self.catalog.quarantine_repomap_candidate(
                candidate.repo_id.as_str(),
                candidate.revision_id.as_str(),
                candidate.manifest_generation,
            )?;
        }
        report.quarantined.push(QuarantinedRepoMapFileV1 {
            file_name: quarantine_entry,
            reason: format!("quarantine-reason-{reason_code}"),
        });
        Ok(())
    }

    pub fn ingest_bundle(
        &self,
        bundle: &RepoMapSourceBundle,
    ) -> Result<RepoMapMutationReceiptV1, CoreError> {
        self.ingest_bundle_with_meta_v2(bundle, CandidateProjectionMetaV1::from_bundle(bundle))
    }

    fn ingest_bundle_with_meta_v2(
        &self,
        bundle: &RepoMapSourceBundle,
        meta: CandidateProjectionMetaV1,
    ) -> Result<RepoMapMutationReceiptV1, CoreError> {
        if bundle.manifest_digest.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap ingest: manifest_digest must not be empty".to_string(),
            ));
        }
        if bundle.authority_digest.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap ingest: authority_digest must not be empty".to_string(),
            ));
        }
        if bundle.nodes.is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap ingest: nodes must not be empty".to_string(),
            ));
        }
        let objects = self
            .objects
            .as_ref()
            .ok_or_else(|| legacy_root_refusal(&self.root))?;
        // Attach fence (S21-05): a pinned logical generation may not have
        // its physical artifact swapped underneath an in-flight read
        // view, so a republish of a pinned generation is refused typed
        // before any object, catalog or registry mutation.
        {
            let publish_key = RepoMapStoreKeyV1::new(
                &bundle.repo_id,
                &bundle.revision_id,
                bundle.manifest_generation,
            );
            let pins = self
                .pins
                .read()
                .map_err(|err| storage_poisoned("pin table", &err))?;
            if pins.get(&publish_key).copied().unwrap_or(0) > 0 {
                return Err(CoreError::InvalidContract(format!(
                    "repomap publish: generation {} of repo={} revision={} is pinned by an \
                     in-flight read; republishing would swap the artifact under a logical pin",
                    bundle.manifest_generation.get(),
                    bundle.repo_id.as_str(),
                    bundle.revision_id.as_str()
                )));
            }
        }
        let meta_json = meta.to_json()?;
        // Compile (typed refusal ⇒ zero object/catalog/registry mutation)
        // and seal the immutable object; only then commit the catalog row.
        let candidate = RepoMapGraphCompiler::with_default_budget()
            .compile(bundle)
            .map_err(|refusal| {
                CoreError::InvalidContract(format!("repomap compile refused: {refusal}"))
            })?;
        let identity = logical_identity(
            &bundle.repo_id,
            &bundle.revision_id,
            bundle.manifest_generation,
        );
        let envelope = candidate.envelope(identity).map_err(|error| {
            CoreError::InvalidContract(format!("repomap candidate envelope refused: {error}"))
        })?;
        let _sealed = objects.seal(&envelope)?;
        let commitment = envelope
            .commitment()
            .map_err(|error| CoreError::Storage(format!("repomap commitment refused: {error}")))?;
        let object_digest = envelope.object_digest().map_err(|error| {
            CoreError::Storage(format!("repomap object address refused: {error}"))
        })?;
        let content_digest = envelope.artifact().content_digest();
        let outcome = self.catalog.seal_repomap_candidate(
            bundle.repo_id.as_str(),
            bundle.revision_id.as_str(),
            bundle.manifest_generation.get(),
            commitment.as_bytes(),
            object_digest.as_bytes(),
            content_digest.as_bytes(),
            candidate.byte_size(),
            meta_json.as_str(),
        )?;
        exit_at_crash_boundary(AFTER_CATALOG_COMMIT);
        // Registry publish is cache-only, strictly after the commit.
        {
            let snapshot = snapshot_from_projection(
                &bundle.repo_id,
                &bundle.revision_id,
                bundle.manifest_generation,
                &meta,
                candidate.projection(),
            );
            let key = RepoMapStoreKeyV1::new(
                &bundle.repo_id,
                &bundle.revision_id,
                bundle.manifest_generation,
            );
            let indexed = Arc::new(RepoMapIndexedSnapshot::new(snapshot));
            let mut guard = self
                .snapshots
                .write()
                .map_err(|err| storage_poisoned("registry", &err))?;
            let _prior = guard.insert(key, indexed);
        }
        let activation = self
            .catalog
            .repomap_activation_row(bundle.repo_id.as_str(), bundle.revision_id.as_str())?;
        Ok(receipt(
            activation
                .as_ref()
                .filter(|row| row.active)
                .map(|row| CandidateCommitmentV1::from_bytes(row.candidate_commitment)),
            commitment,
            activation.as_ref().map_or(0, |row| row.epoch),
            outcome.terminal_sequence,
            outcome.replayed,
        ))
    }

    pub fn ingest_bundle_v2(
        &self,
        request: &RepoMapPublishBundleRequestV2,
    ) -> Result<RepoMapTerminalReceiptV2, CoreError> {
        let computed = canonical_repo_map_source_bundle_digest_v2(&request.bundle)
            .map_err(|error| CoreError::InvalidContract(format!("repomap V2 ingest: {error}")))?;
        if computed != request.source_bundle_digest {
            return Err(CoreError::InvalidContract(format!(
                "repomap V2 ingest: source bundle digest mismatch: expected={} observed={}",
                request.source_bundle_digest, computed
            )));
        }
        let meta =
            CandidateProjectionMetaV1::from_bundle_with_source_digest_v2(&request.bundle, computed);
        let receipt = self.ingest_bundle_with_meta_v2(&request.bundle, meta)?;
        self.validate_published_candidate_custody_v2(
            &request.bundle,
            request.source_bundle_digest.as_str(),
        )?;
        Ok(terminal_publish_receipt_v2(
            &request.bundle,
            request.source_bundle_digest.clone(),
            receipt,
        ))
    }

    /// Re-read the durable catalog row before issuing a V2 terminal receipt.
    ///
    /// Candidate replay is keyed by the compiled candidate commitment. Some
    /// source-bundle axes do not change those compiled bytes, so commitment
    /// equality alone cannot authorize a receipt for the replaying request.
    /// The receipt is issued only when the catalog-retained axes are exactly
    /// those of this request.
    fn validate_published_candidate_custody_v2(
        &self,
        bundle: &RepoMapSourceBundle,
        source_bundle_digest: &str,
    ) -> Result<(), CoreError> {
        let candidate = self
            .catalog
            .repomap_candidate_row(
                bundle.repo_id.as_str(),
                bundle.revision_id.as_str(),
                bundle.manifest_generation.get(),
            )?
            .ok_or_else(|| {
                CoreError::Storage(
                    "repomap V2 ingest: candidate disappeared after durable seal".to_string(),
                )
            })?;
        let meta = CandidateProjectionMetaV1::from_json(candidate.projection_meta.as_str())?;
        if meta.manifest_digest.as_deref() != Some(bundle.manifest_digest.as_str())
            || meta.snapshot_id != bundle.snapshot_id
            || meta.projection_version != bundle.projection_version
            || meta.authority_digest != bundle.authority_digest
            || meta.source_bundle_digest.as_deref() != Some(source_bundle_digest)
        {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CandidateCommitmentConflict,
                message: format!(
                    "repomap V2 ingest: durable candidate custody differs from replay request for repo={} revision={} generation={}",
                    bundle.repo_id.as_str(),
                    bundle.revision_id.as_str(),
                    bundle.manifest_generation.get(),
                ),
            });
        }
        Ok(())
    }

    /// Insert a snapshot without a sealed candidate.
    ///
    /// Memory-only and refused on persisted stores: without a sealed
    /// candidate there is no catalog row to publish under, and the
    /// registry is a cache of durable rows, never an authority.
    pub fn insert_snapshot(&self, _snapshot: RepoMapSnapshot) -> Result<(), CoreError> {
        Err(CoreError::InvalidContract(
            "repomap store: insert_snapshot is gone; publish seals a candidate through the \
             catalog instead"
                .to_string(),
        ))
    }

    /// Activate one generation for its repo and revision under a
    /// content-bound CAS: the sealed object at the catalog row's address
    /// must verify and its exact commitment becomes the CAS token.
    pub fn activate_generation(
        &self,
        request: &RepoMapActivateGenerationRequest,
    ) -> Result<RepoMapMutationReceiptV1, CoreError> {
        if request.manifest_digest.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap activate: manifest_digest must not be empty".to_string(),
            ));
        }
        let objects = self
            .objects
            .as_ref()
            .ok_or_else(|| legacy_root_refusal(&self.root))?;
        let Some(candidate_row) = self.catalog.repomap_candidate_row(
            request.repo_id.as_str(),
            request.revision_id.as_str(),
            request.manifest_generation.get(),
        )?
        else {
            return Err(CoreError::NotFound(format!(
                "repomap activate: no snapshot for repo={} revision={} generation={}",
                request.repo_id.as_str(),
                request.revision_id.as_str(),
                request.manifest_generation.get()
            )));
        };
        // Verify the object against the catalog row first: activation can
        // only name bytes that pass the full security + commitment chain.
        let digest = quanta_index_contract::CandidateObjectDigestV1::from_bytes(
            candidate_row.object_address,
        );
        let envelope = objects
            .verify(digest, &candidate_row.candidate_commitment)
            .map_err(|failure| CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: format!(
                    "repomap activate: sealed object for repo={} revision={} generation={} \
                         failed verification: {}/{}",
                    request.repo_id.as_str(),
                    request.revision_id.as_str(),
                    request.manifest_generation.get(),
                    u64::from(failure.reason.code()),
                    failure.detail
                ),
            })?;
        let commitment = envelope
            .commitment()
            .map_err(|error| CoreError::Storage(format!("repomap commitment refused: {error}")))?;
        let outcome = self.catalog.activate_repomap_candidate(
            request.repo_id.as_str(),
            request.revision_id.as_str(),
            request.manifest_generation.get(),
            commitment.as_bytes(),
        )?;
        // Registry publish after the commit; rebuild from the object when
        // the snapshot is not resident yet (e.g. restart then activate a
        // still-sealed candidate).
        {
            let key = RepoMapStoreKeyV1::new(
                &request.repo_id,
                &request.revision_id,
                request.manifest_generation,
            );
            let mut guard = self
                .snapshots
                .write()
                .map_err(|err| storage_poisoned("registry", &err))?;
            if let std::collections::btree_map::Entry::Vacant(entry) = guard.entry(key) {
                let meta =
                    CandidateProjectionMetaV1::from_json(candidate_row.projection_meta.as_str())?;
                let (_graph, projection) = decode_compiled_payload(envelope.compiled_payload())?;
                let snapshot = snapshot_from_projection(
                    &request.repo_id,
                    &request.revision_id,
                    request.manifest_generation,
                    &meta,
                    &projection,
                );
                let _prior = entry.insert(Arc::new(RepoMapIndexedSnapshot::new(snapshot)));
            }
        }
        if outcome.replayed {
            return Ok(receipt(
                outcome
                    .prior_candidate_commitment
                    .map(CandidateCommitmentV1::from_bytes),
                commitment,
                outcome.epoch,
                outcome.terminal_sequence,
                true,
            ));
        }
        {
            let mut activated = self
                .activated
                .write()
                .map_err(|err| storage_poisoned("activation map", &err))?;
            let _prior = activated.insert(
                (
                    request.repo_id.as_str().to_string(),
                    request.revision_id.as_str().to_string(),
                ),
                ActivatedHeadV1 {
                    manifest_generation: request.manifest_generation.get(),
                    epoch: outcome.epoch,
                    candidate_commitment: *commitment.as_bytes(),
                },
            );
        }
        Ok(receipt(
            outcome
                .prior_candidate_commitment
                .map(CandidateCommitmentV1::from_bytes),
            commitment,
            outcome.epoch,
            outcome.terminal_sequence,
            false,
        ))
    }

    pub fn activate_generation_v2(
        &self,
        request: &RepoMapActivateGenerationRequestV2,
    ) -> Result<RepoMapTerminalReceiptV2, CoreError> {
        let identity = &request.request_v1;
        let candidate = self
            .catalog
            .repomap_candidate_row(
                identity.repo_id.as_str(),
                identity.revision_id.as_str(),
                identity.manifest_generation.get(),
            )?
            .ok_or_else(|| {
                CoreError::NotFound(format!(
                    "repomap V2 activate: no snapshot for repo={} revision={} generation={}",
                    identity.repo_id.as_str(),
                    identity.revision_id.as_str(),
                    identity.manifest_generation.get()
                ))
            })?;
        let meta = CandidateProjectionMetaV1::from_json(candidate.projection_meta.as_str())?;
        let durable_manifest_digest = meta.manifest_digest.clone().ok_or_else(|| {
            CoreError::InvalidContract(
                "repomap V2 activate: candidate predates strong manifest custody".to_string(),
            )
        })?;
        let durable_source_digest = meta.source_bundle_digest.clone().ok_or_else(|| {
            CoreError::InvalidContract(
                "repomap V2 activate: candidate predates strong source-bundle custody".to_string(),
            )
        })?;
        if durable_manifest_digest != identity.manifest_digest
            || meta.snapshot_id != request.snapshot_id
            || meta.projection_version != request.projection_version
            || meta.authority_digest != request.authority_digest
            || durable_source_digest != request.source_bundle_digest
        {
            return Err(CoreError::InvalidContract(
                "repomap V2 activate: request axes differ from the sealed candidate".to_string(),
            ));
        }
        let receipt = self.activate_generation(identity)?;
        Ok(RepoMapTerminalReceiptV2 {
            phase: RepoMapMutationPhaseV2::Activate,
            mutation: mutation_ack_v1(identity, receipt),
            manifest_digest: durable_manifest_digest,
            snapshot_id: meta.snapshot_id,
            projection_version: meta.projection_version,
            authority_digest: meta.authority_digest,
            source_bundle_digest: durable_source_digest,
        })
    }

    /// Acquire the serving snapshot for one logical identity in a single
    /// critical section (S21-05).
    ///
    /// Lock order (canonical, one owner): `activated` read guard, then
    /// `snapshots` read guard; the `pins` write guard comes after both
    /// are released. No store guard is ever held across a catalog call,
    /// a disk object open/verify, or a query execution — the pinned
    /// snapshot queries the `Arc` it holds and nothing else.
    ///
    /// The active identity, its candidate commitment, its activation
    /// epoch and the artifact reference are observed together: a
    /// concurrent activation or retirement either fully precedes this
    /// acquisition (the new head is what gets pinned) or fully follows it
    /// (the pinned view keeps the old commitment to completion). A
    /// generation that is not the serving head is refused typed; it is
    /// never resolved to another generation.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "both guards must be held through the whole acquisition block: one critical section observes head and snapshot together"
    )]
    pub fn acquire_pinned(
        &self,
        acquire: &RepoMapSnapshotAcquireV1,
    ) -> Result<PinnedRepoMapSnapshotV1, CoreError> {
        let key = RepoMapStoreKeyV1::new(
            &acquire.repo_id,
            &acquire.revision_id,
            acquire.manifest_generation,
        );
        let activation_key = (
            acquire.repo_id.as_str().to_string(),
            acquire.revision_id.as_str().to_string(),
        );
        let (head, snapshot) = {
            let activated = self
                .activated
                .read()
                .map_err(|err| storage_poisoned("activation map", &err))?;
            let snapshots = self
                .snapshots
                .read()
                .map_err(|err| storage_poisoned("registry", &err))?;
            let head = activated.get(&activation_key).ok_or_else(|| {
                CoreError::NotFound(format!(
                    "repomap acquire: no activated generation for repo={} revision={}",
                    acquire.repo_id.as_str(),
                    acquire.revision_id.as_str()
                ))
            })?;
            if head.manifest_generation != key.manifest_generation {
                return Err(CoreError::NotFound(format!(
                    "repomap acquire: requested generation {} is not the activated generation {} \
                     for repo={} revision={}",
                    key.manifest_generation,
                    head.manifest_generation,
                    acquire.repo_id.as_str(),
                    acquire.revision_id.as_str()
                )));
            }
            let snapshot = snapshots.get(&key).cloned().ok_or_else(|| {
                CoreError::NotFound(format!(
                    "repomap acquire: snapshot missing for repo={} revision={} generation={}",
                    acquire.repo_id.as_str(),
                    acquire.revision_id.as_str(),
                    key.manifest_generation
                ))
            })?;
            (
                ActivatedHeadV1 {
                    manifest_generation: head.manifest_generation,
                    epoch: head.epoch,
                    candidate_commitment: head.candidate_commitment,
                },
                snapshot,
            )
        };
        let evidence = RepoMapSnapshotEvidenceV1 {
            repo_id: acquire.repo_id.as_str().to_string(),
            revision_id: acquire.revision_id.as_str().to_string(),
            manifest_generation: key.manifest_generation,
            candidate_commitment: CandidateCommitmentV1::from_bytes(head.candidate_commitment)
                .to_wire_string(),
            activation_epoch: head.epoch,
        };
        let lease = RepoMapPinLease::new(Arc::clone(&self.pins), key)?;
        Ok(PinnedRepoMapSnapshotV1::new(snapshot, evidence, lease))
    }

    /// How many pinned read views hold `generation` of `repo`/`revision`.
    pub fn pinned_view_count(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
    ) -> Result<u64, CoreError> {
        let key = RepoMapStoreKeyV1::new(repo_id, revision_id, generation);
        let guard = self
            .pins
            .read()
            .map_err(|err| storage_poisoned("pin table", &err))?;
        Ok(guard.get(&key).copied().unwrap_or(0))
    }

    /// Reclaim the physical objects of retired candidates (S21-05).
    ///
    /// A candidate is retired when its activation was invalidated (a
    /// newer generation superseded it, or it was quarantined out). P03
    /// left such objects on disk (tombstone-only); this pass reclaims
    /// them, but only when no pinned read view still holds the logical
    /// generation: a pinned view keeps its artifact alive and the pass
    /// defers to a later one. The catalog rows stay as the durable
    /// tombstones; the bytes carry no authority once invalidated and can
    /// never resurrect.
    pub fn gc_retired_objects(&self) -> Result<RepoMapGcOutcomeV1, CoreError> {
        let objects = self
            .objects
            .as_ref()
            .ok_or_else(|| legacy_root_refusal(&self.root))?;
        let mut outcome = RepoMapGcOutcomeV1::default();
        let rows = self.catalog.repomap_candidate_rows()?;
        for row in rows {
            if row.state != quanta_index_catalog::RepoMapCandidateStateV1::ActivationInvalidated {
                continue;
            }
            outcome.considered = outcome.considered.saturating_add(1);
            let repo_id = RepoId::new(row.repo_id.as_str()).map_err(|error| CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                message: format!("repomap candidate row holds an invalid repo ID: {error}"),
            })?;
            let revision_id =
                RevisionId::new(row.revision_id.as_str()).map_err(|error| CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::CatalogRowCorrupt,
                    message: format!("repomap candidate row holds an invalid revision ID: {error}"),
                })?;
            let generation = ManifestGeneration::new(row.manifest_generation);
            let key = RepoMapStoreKeyV1::new(&repo_id, &revision_id, generation);
            // Pin gate first, under the pin read guard only: no other
            // store guard is held across the unlink or the registry edit.
            {
                let pins = self
                    .pins
                    .read()
                    .map_err(|err| storage_poisoned("pin table", &err))?;
                if pins.get(&key).copied().unwrap_or(0) > 0 {
                    outcome.deferred_pinned = outcome.deferred_pinned.saturating_add(1);
                    continue;
                }
            }
            let digest =
                quanta_index_contract::CandidateObjectDigestV1::from_bytes(row.object_address);
            // Already-reclaimed rows stay tombstoned in the catalog; a
            // pass that finds no bytes counts nothing.
            if !self
                .root
                .join(crate::layout_v3::CandidateObjectAddressV1::new(digest).relative_path())
                .exists()
            {
                continue;
            }
            objects.unlink_object(digest)?;
            {
                let mut registry = self
                    .snapshots
                    .write()
                    .map_err(|err| storage_poisoned("registry", &err))?;
                let _retired = registry.remove(&key);
            }
            outcome.reclaimed = outcome.reclaimed.saturating_add(1);
        }
        Ok(outcome)
    }

    pub fn activated_generation_for(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Option<u64>, CoreError> {
        let key = (
            repo_id.as_str().to_string(),
            revision_id.as_str().to_string(),
        );
        let guard = self
            .activated
            .read()
            .map_err(|err| storage_poisoned("activation map", &err))?;
        Ok(guard.get(&key).map(|head| head.manifest_generation))
    }

    /// The generations the registry holds for `repo`/`revision`, ascending.
    pub fn resident_generations_for(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<u64>, CoreError> {
        let guard = self
            .snapshots
            .read()
            .map_err(|err| storage_poisoned("registry", &err))?;
        Ok(guard
            .keys()
            .filter(|key| {
                key.repo_id == repo_id.as_str() && key.revision_id == revision_id.as_str()
            })
            .map(|key| key.manifest_generation)
            .collect())
    }
}

impl RepoMapSnapshotAcquirePort for RepoMapGenerationStore {
    fn acquire(
        &self,
        acquire: RepoMapSnapshotAcquireV1,
    ) -> Result<Box<dyn PinnedRepoMapSnapshot>, CoreError> {
        Ok(Box::new(self.acquire_pinned(&acquire)?))
    }
}

fn receipt(
    prior: Option<CandidateCommitmentV1>,
    new: CandidateCommitmentV1,
    epoch: u64,
    terminal_sequence: i64,
    replayed: bool,
) -> RepoMapMutationReceiptV1 {
    RepoMapMutationReceiptV1 {
        prior_candidate_commitment: prior.map(CandidateCommitmentV1::to_wire_string),
        new_candidate_commitment: new.to_wire_string(),
        activation_epoch: epoch,
        terminal_sequence: u64::try_from(terminal_sequence).map_or(0, |sequence| sequence),
        replayed,
    }
}

fn mutation_ack_v1(
    identity: &RepoMapActivateGenerationRequest,
    receipt: RepoMapMutationReceiptV1,
) -> RepoMapMutationAck {
    RepoMapMutationAck {
        repo_id: identity.repo_id.clone(),
        revision_id: identity.revision_id.clone(),
        manifest_generation: identity.manifest_generation,
        prior_candidate_commitment: receipt.prior_candidate_commitment,
        new_candidate_commitment: receipt.new_candidate_commitment,
        activation_epoch: receipt.activation_epoch,
        terminal_sequence: receipt.terminal_sequence,
        replayed: receipt.replayed,
    }
}

fn terminal_publish_receipt_v2(
    bundle: &RepoMapSourceBundle,
    source_bundle_digest: String,
    receipt: RepoMapMutationReceiptV1,
) -> RepoMapTerminalReceiptV2 {
    RepoMapTerminalReceiptV2 {
        phase: RepoMapMutationPhaseV2::Publish,
        mutation: RepoMapMutationAck {
            repo_id: bundle.repo_id.clone(),
            revision_id: bundle.revision_id.clone(),
            manifest_generation: bundle.manifest_generation,
            // Publish seals a candidate but does not mutate the active head.
            // Keeping activation context out of the V2 publish receipt makes
            // an ACK-loss replay independent of later activation changes.
            prior_candidate_commitment: None,
            new_candidate_commitment: receipt.new_candidate_commitment,
            activation_epoch: 0,
            terminal_sequence: receipt.terminal_sequence,
            replayed: receipt.replayed,
        },
        manifest_digest: bundle.manifest_digest.clone(),
        snapshot_id: bundle.snapshot_id.clone(),
        projection_version: bundle.projection_version,
        authority_digest: bundle.authority_digest.clone(),
        source_bundle_digest,
    }
}

impl RepoMapBundleIngestPort for RepoMapGenerationStore {
    fn ingest_bundle(
        &self,
        bundle: &RepoMapSourceBundle,
    ) -> Result<RepoMapMutationReceiptV1, CoreError> {
        Self::ingest_bundle(self, bundle)
    }

    fn ingest_bundle_v2(
        &self,
        request: &RepoMapPublishBundleRequestV2,
    ) -> Result<RepoMapTerminalReceiptV2, CoreError> {
        Self::ingest_bundle_v2(self, request)
    }
}

// The V1 wire contract accepts one path segment, while V3 source objects
// occupy a nested content-addressed layout. Name the durable incident,
// not an adapter-local path, so list/discard remain exact and unambiguous.
fn quarantine_entry_name(digest: &[u8; 32]) -> String {
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("incident-{hex}.cbor")
}

impl RepoMapQuarantinePort for RepoMapGenerationStore {
    fn quarantined_files(&self) -> Result<Vec<QuarantinedRepoMapFileV1>, CoreError> {
        let incidents = self.catalog.repomap_quarantine_incidents()?;
        Ok(incidents
            .into_iter()
            .filter(|incident| !incident.discarded)
            .map(|incident| QuarantinedRepoMapFileV1 {
                file_name: quarantine_entry_name(&incident.incident_digest),
                reason: incident.reason_code,
            })
            .collect())
    }

    fn discard_quarantined_file(
        &self,
        entry: &QuarantinedRepoMapFileV1,
    ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
        let objects = self
            .objects
            .as_ref()
            .ok_or_else(|| legacy_root_refusal(&self.root))?;
        let incidents = self.catalog.repomap_quarantine_incidents()?;
        let Some(incident) = incidents
            .iter()
            .find(|incident| quarantine_entry_name(&incident.incident_digest) == entry.file_name)
        else {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
                message: format!(
                    "repomap: refusing to discard quarantined `{}`: no durable incident under \
                     that source path",
                    entry.file_name
                ),
            });
        };
        if incident.reason_code != entry.reason {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
                message: format!(
                    "repomap: refusing to discard quarantined `{}`: it is recorded as `{}` now, \
                     not `{}` as listed; list again",
                    entry.file_name, incident.reason_code, entry.reason
                ),
            });
        }
        if incident.discarded {
            return Ok(QuarantineDiscardOutcomeV1::Absent);
        }
        // Journaled tombstone first, then the payload-only reclaim.
        let _sequence = self
            .catalog
            .discard_repomap_quarantine_payload(&incident.incident_digest)?;
        let payload_bytes = objects
            .read_quarantine_payload(
                quanta_index_contract::QuarantinePayloadDigestV1::from_bytes(
                    incident.payload_digest,
                ),
            )?
            .map_or(0_usize, |bytes| bytes.len());
        let reclaimed = u64::try_from(payload_bytes).map_or(0, |bytes| bytes);
        let _reclaimed = objects.reclaim_quarantine_payload(
            quanta_index_contract::QuarantinePayloadDigestV1::from_bytes(incident.payload_digest),
        )?;
        Ok(QuarantineDiscardOutcomeV1::Discarded { bytes: reclaimed })
    }
}

impl RepoMapGenerationActivatePort for RepoMapGenerationStore {
    fn activate_generation(
        &self,
        request: &RepoMapActivateGenerationRequest,
    ) -> Result<RepoMapMutationReceiptV1, CoreError> {
        Self::activate_generation(self, request)
    }

    fn activate_generation_v2(
        &self,
        request: &RepoMapActivateGenerationRequestV2,
    ) -> Result<RepoMapTerminalReceiptV2, CoreError> {
        Self::activate_generation_v2(self, request)
    }
}
