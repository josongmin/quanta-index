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
use quanta_index_contract::{RepoMapActivateGenerationRequest, RepoMapSourceBundle};
use quanta_index_core::{
    CoreError, QuarantineDiscardOutcomeV1, QuarantinedRepoMapFileV1, RepoMapBundleIngestPort,
    RepoMapGenerationActivatePort, RepoMapMutationReceiptV1, RepoMapOpenReportV1,
    RepoMapQuarantinePort, RepoMapQueryPort,
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
use crate::query::RepoMapQueryEngine;

#[derive(Debug)]
pub struct RepoMapGenerationStore {
    root: PathBuf,
    catalog: Arc<SqliteCatalog>,
    objects: Option<RepoMapObjectStore>,
    snapshots: RwLock<BTreeMap<RepoMapStoreKeyV1, Arc<RepoMapIndexedSnapshot>>>,
    activated: RwLock<BTreeMap<(String, String), u64>>,
}

/// A store opened from its root and catalog, with what the open found.
#[derive(Debug)]
pub struct OpenedRepoMapStore {
    pub store: RepoMapGenerationStore,
    pub report: RepoMapOpenReportV1,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct RepoMapStoreKeyV1 {
    repo_id: String,
    revision_id: String,
    manifest_generation: u64,
}

impl RepoMapStoreKeyV1 {
    fn new(
        repo_id: &RepoId,
        revision_id: &RevisionId,
        manifest_generation: ManifestGeneration,
    ) -> Self {
        Self {
            repo_id: repo_id.as_str().to_string(),
            revision_id: revision_id.as_str().to_string(),
            manifest_generation: manifest_generation.get(),
        }
    }
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
            if candidate.state == quanta_index_catalog::RepoMapCandidateStateV1::Quarantined {
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
        let mut reactivations: Vec<((String, String), u64)> = Vec::new();
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
                    candidate.manifest_generation,
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
        let payload_digest = failure.raw_bytes.as_deref().map(|bytes| {
            use sha2::Digest as _;
            let mut hasher = sha2::Sha256::new();
            hasher.update(b"quanta-index/quarantine-payload/v1\0");
            hasher.update(bytes);
            quanta_index_contract::QuarantinePayloadDigestV1::from_bytes(hasher.finalize().into())
        });
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
            file_name: source_path,
            reason: reason_text,
        });
        Ok(())
    }

    pub fn ingest_bundle(
        &self,
        bundle: &RepoMapSourceBundle,
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
        let meta = CandidateProjectionMetaV1::from_bundle(bundle);
        let meta_json = meta.to_json()?;
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
                None,
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
                request.manifest_generation.get(),
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

    pub fn read_query_snapshot(
        &self,
        request: &quanta_index_contract::RepoMapQueryRequest,
    ) -> Result<quanta_index_contract::RepoMapQueryResponse, CoreError> {
        self.ensure_generation_activated(request)?;
        let key = RepoMapStoreKeyV1::new(
            &request.repo_id,
            &request.revision_id,
            request.manifest_generation,
        );
        let snapshot = {
            let guard = self
                .snapshots
                .read()
                .map_err(|err| storage_poisoned("registry", &err))?;
            guard.get(&key).map(Arc::clone).ok_or_else(|| {
                CoreError::NotFound(format!(
                    "repomap snapshot missing for repo={} revision={} generation={}",
                    request.repo_id.as_str(),
                    request.revision_id.as_str(),
                    request.manifest_generation.get()
                ))
            })?
        };
        RepoMapQueryEngine::query(&snapshot, request)
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
        Ok(guard.get(&key).copied())
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

    fn ensure_generation_activated(
        &self,
        request: &quanta_index_contract::RepoMapQueryRequest,
    ) -> Result<(), CoreError> {
        let key = (
            request.repo_id.as_str().to_string(),
            request.revision_id.as_str().to_string(),
        );
        let guard = self
            .activated
            .read()
            .map_err(|err| storage_poisoned("activation map", &err))?;
        match guard.get(&key) {
            Some(active_generation) if *active_generation == request.manifest_generation.get() => {
                Ok(())
            }
            Some(active_generation) => Err(CoreError::NotFound(format!(
                "repomap query: requested generation {} is not the activated generation {} for repo={} revision={}",
                request.manifest_generation.get(),
                active_generation,
                request.repo_id.as_str(),
                request.revision_id.as_str()
            ))),
            None => Err(CoreError::NotFound(format!(
                "repomap query: no activated generation for repo={} revision={}",
                request.repo_id.as_str(),
                request.revision_id.as_str()
            ))),
        }
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

impl RepoMapBundleIngestPort for RepoMapGenerationStore {
    fn ingest_bundle(
        &self,
        bundle: &RepoMapSourceBundle,
    ) -> Result<RepoMapMutationReceiptV1, CoreError> {
        Self::ingest_bundle(self, bundle)
    }
}

impl RepoMapQuarantinePort for RepoMapGenerationStore {
    fn quarantined_files(&self) -> Result<Vec<QuarantinedRepoMapFileV1>, CoreError> {
        let incidents = self.catalog.repomap_quarantine_incidents()?;
        Ok(incidents
            .into_iter()
            .map(|incident| QuarantinedRepoMapFileV1 {
                file_name: incident.source_path,
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
            .find(|incident| incident.source_path == entry.file_name)
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
}

impl RepoMapQueryPort for RepoMapGenerationStore {
    fn query(
        &self,
        request: quanta_index_contract::RepoMapQueryRequest,
    ) -> Result<quanta_index_contract::RepoMapQueryResponse, CoreError> {
        Self::read_query_snapshot(self, &request)
    }
}
