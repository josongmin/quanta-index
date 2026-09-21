//! The `RepoMap` generation store: indexed snapshots shared by reference,
//! durable activations, and retention of superseded generations (QI-BB-008).

use std::{collections::BTreeMap, path::Path, sync::Arc, sync::RwLock};

use quanta_index_contract::{
    ManifestGeneration, RepoId, RepoMapQueryRequest, RepoMapQueryResponse, RevisionId,
};
use quanta_index_contract::{RepoMapActivateGenerationRequest, RepoMapSourceBundle};
use quanta_index_core::{
    CoreError, QuarantineDiscardOutcomeV1, QuarantinedRepoMapFileV1, RepoMapBundleIngestPort,
    RepoMapGenerationActivatePort, RepoMapOpenReportV1, RepoMapQuarantinePort, RepoMapQueryPort,
};

use crate::{
    RepoMapMaterializer, RepoMapQueryEngine,
    model::{RepoMapIndexedSnapshot, RepoMapSnapshot},
    persistence::{RepoMapActivationRecordV1, RepoMapSnapshotPersistence},
};

#[derive(Debug)]
pub struct RepoMapGenerationStore {
    snapshots: RwLock<BTreeMap<RepoMapStoreKeyV1, Arc<RepoMapIndexedSnapshot>>>,
    activated: RwLock<BTreeMap<(String, String), u64>>,
    persistence: Option<RepoMapSnapshotPersistence>,
}

/// A store opened from disk, with what the open found.
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

impl Default for RepoMapGenerationStore {
    fn default() -> Self {
        Self {
            snapshots: RwLock::new(BTreeMap::new()),
            activated: RwLock::new(BTreeMap::new()),
            persistence: None,
        }
    }
}

impl RepoMapGenerationStore {
    /// Open the store under `root`, loading every trusted snapshot and
    /// activation and reporting what could not be trusted.
    ///
    /// An activation whose snapshot is missing is dropped with a reason
    /// rather than failing the open: the repo answers `NOT_FOUND` until it
    /// is re-activated, which is the fail-closed answer a lost file earns.
    pub fn open(root: impl AsRef<Path>) -> Result<OpenedRepoMapStore, CoreError> {
        let persistence = RepoMapSnapshotPersistence::open(root)?;
        let loaded = persistence.load()?;
        let mut report = loaded.report;
        let mut snapshots = BTreeMap::new();
        for snapshot in loaded.snapshots {
            let key = RepoMapStoreKeyV1::new(
                &snapshot.repo_id,
                &snapshot.revision_id,
                snapshot.manifest_generation,
            );
            let _prior = snapshots.insert(key, Arc::new(RepoMapIndexedSnapshot::new(snapshot)));
        }
        let mut activated = BTreeMap::new();
        for RepoMapActivationRecordV1 {
            repo_id,
            revision_id,
            manifest_generation,
        } in loaded.activations
        {
            let validated_repo = RepoId::new(&repo_id).map_err(|error| {
                CoreError::Storage(format!(
                    "repomap activation contains invalid repo ID: {error}"
                ))
            })?;
            let validated_revision = RevisionId::new(&revision_id).map_err(|error| {
                CoreError::Storage(format!(
                    "repomap activation contains invalid revision ID: {error}"
                ))
            })?;
            let key = RepoMapStoreKeyV1::new(
                &validated_repo,
                &validated_revision,
                ManifestGeneration::new(manifest_generation),
            );
            if !snapshots.contains_key(&key) {
                report.activations_without_snapshot.push(format!(
                    "repo={repo_id} revision={revision_id} generation={manifest_generation}"
                ));
                continue;
            }
            let _prior = activated.insert((repo_id, revision_id), manifest_generation);
        }
        Ok(OpenedRepoMapStore {
            store: Self {
                snapshots: RwLock::new(snapshots),
                activated: RwLock::new(activated),
                persistence: Some(persistence),
            },
            report,
        })
    }

    pub fn ingest_bundle(&self, bundle: &RepoMapSourceBundle) -> Result<(), CoreError> {
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
        let snapshot = RepoMapMaterializer::materialize(bundle);
        self.insert_snapshot(snapshot)
    }

    pub fn insert_snapshot(&self, snapshot: RepoMapSnapshot) -> Result<(), CoreError> {
        if let Some(persistence) = &self.persistence {
            persistence.persist_snapshot(&snapshot)?;
        }
        let key = RepoMapStoreKeyV1::new(
            &snapshot.repo_id,
            &snapshot.revision_id,
            snapshot.manifest_generation,
        );
        let indexed = Arc::new(RepoMapIndexedSnapshot::new(snapshot));
        {
            let mut guard = self
                .snapshots
                .write()
                .map_err(|err| CoreError::Storage(format!("repomap store poisoned: {err}")))?;
            let _prior = guard.insert(key, indexed);
        }
        Ok(())
    }

    /// Activate one generation for its repo and revision.
    ///
    /// Once the activation is durable, every older generation of the same
    /// repo and revision is retired from disk and memory: only the
    /// activated generation answers queries, so older ones are dead weight.
    /// Newer, not-yet-activated generations are kept.
    pub fn activate_generation(
        &self,
        request: &RepoMapActivateGenerationRequest,
    ) -> Result<(), CoreError> {
        if request.manifest_digest.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap activate: manifest_digest must not be empty".to_string(),
            ));
        }
        let key = RepoMapStoreKeyV1::new(
            &request.repo_id,
            &request.revision_id,
            request.manifest_generation,
        );
        let guard = self
            .snapshots
            .read()
            .map_err(|err| CoreError::Storage(format!("repomap store poisoned: {err}")))?;
        if !guard.contains_key(&key) {
            return Err(CoreError::NotFound(format!(
                "repomap activate: no snapshot for repo={} revision={} generation={}",
                request.repo_id.as_str(),
                request.revision_id.as_str(),
                request.manifest_generation.get()
            )));
        }
        drop(guard);
        if let Some(persistence) = &self.persistence {
            persistence.persist_activation(
                &request.repo_id,
                &request.revision_id,
                request.manifest_generation.get(),
            )?;
        }
        {
            let mut activated = self.activated.write().map_err(|err| {
                CoreError::Storage(format!("repomap activation map poisoned: {err}"))
            })?;
            let _prior = activated.insert(
                (
                    request.repo_id.as_str().to_string(),
                    request.revision_id.as_str().to_string(),
                ),
                request.manifest_generation.get(),
            );
        }
        self.retire_generations_before(
            &request.repo_id,
            &request.revision_id,
            request.manifest_generation,
        )
    }

    /// Retire every generation of `repo`/`revision` older than `keep_from`.
    ///
    /// The file goes first, then the resident snapshot, so a crash between
    /// the two leaves a resident snapshot the next open simply does not
    /// find — never a file the store forgot.
    fn retire_generations_before(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        keep_from: ManifestGeneration,
    ) -> Result<(), CoreError> {
        let older: Vec<RepoMapStoreKeyV1> = {
            let guard = self
                .snapshots
                .read()
                .map_err(|err| CoreError::Storage(format!("repomap store poisoned: {err}")))?;
            guard
                .keys()
                .filter(|key| {
                    key.repo_id == repo_id.as_str()
                        && key.revision_id == revision_id.as_str()
                        && key.manifest_generation < keep_from.get()
                })
                .cloned()
                .collect()
        };
        for key in older {
            if let Some(persistence) = &self.persistence {
                persistence.remove_snapshot(
                    repo_id,
                    revision_id,
                    ManifestGeneration::new(key.manifest_generation),
                )?;
            }
            let mut guard = self
                .snapshots
                .write()
                .map_err(|err| CoreError::Storage(format!("repomap store poisoned: {err}")))?;
            let _retired = guard.remove(&key);
        }
        Ok(())
    }

    pub fn read_query_snapshot(
        &self,
        request: &RepoMapQueryRequest,
    ) -> Result<RepoMapQueryResponse, CoreError> {
        self.ensure_generation_activated(request)?;
        let key = RepoMapStoreKeyV1::new(
            &request.repo_id,
            &request.revision_id,
            request.manifest_generation,
        );
        // The snapshot is shared, not copied: a query holds one reference
        // for its duration and never clones an entry it will not return.
        let snapshot = {
            let guard = self
                .snapshots
                .read()
                .map_err(|err| CoreError::Storage(format!("repomap store poisoned: {err}")))?;
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
            .map_err(|err| CoreError::Storage(format!("repomap activation map poisoned: {err}")))?;
        Ok(guard.get(&key).copied())
    }

    /// The generations the store holds for `repo`/`revision`, ascending.
    pub fn resident_generations_for(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<u64>, CoreError> {
        let guard = self
            .snapshots
            .read()
            .map_err(|err| CoreError::Storage(format!("repomap store poisoned: {err}")))?;
        Ok(guard
            .keys()
            .filter(|key| {
                key.repo_id == repo_id.as_str() && key.revision_id == revision_id.as_str()
            })
            .map(|key| key.manifest_generation)
            .collect())
    }

    fn ensure_generation_activated(&self, request: &RepoMapQueryRequest) -> Result<(), CoreError> {
        let key = (
            request.repo_id.as_str().to_string(),
            request.revision_id.as_str().to_string(),
        );
        let guard = self
            .activated
            .read()
            .map_err(|err| CoreError::Storage(format!("repomap activation map poisoned: {err}")))?;
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

impl RepoMapBundleIngestPort for RepoMapGenerationStore {
    fn ingest_bundle(&self, bundle: &RepoMapSourceBundle) -> Result<(), CoreError> {
        Self::ingest_bundle(self, bundle)
    }
}

impl RepoMapQuarantinePort for RepoMapGenerationStore {
    fn quarantined_files(&self) -> Result<Vec<QuarantinedRepoMapFileV1>, CoreError> {
        self.persistence.as_ref().map_or_else(
            || Ok(Vec::new()),
            RepoMapSnapshotPersistence::quarantined_files,
        )
    }

    fn discard_quarantined_file(
        &self,
        entry: &QuarantinedRepoMapFileV1,
    ) -> Result<QuarantineDiscardOutcomeV1, CoreError> {
        let Some(persistence) = self.persistence.as_ref() else {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::QuarantineTargetNotQuarantined,
                message: format!(
                    "repomap: refusing to discard quarantined `{}`: this store has no durable quarantine",
                    entry.file_name
                ),
            });
        };
        persistence.discard_quarantined_file(entry)
    }
}

impl RepoMapGenerationActivatePort for RepoMapGenerationStore {
    fn activate_generation(
        &self,
        request: &RepoMapActivateGenerationRequest,
    ) -> Result<(), CoreError> {
        Self::activate_generation(self, request)
    }
}

impl RepoMapQueryPort for RepoMapGenerationStore {
    fn query(&self, request: RepoMapQueryRequest) -> Result<RepoMapQueryResponse, CoreError> {
        Self::read_query_snapshot(self, &request)
    }
}
