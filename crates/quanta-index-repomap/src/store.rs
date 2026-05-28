use std::{collections::BTreeMap, path::Path, sync::RwLock};

use quanta_index_contract::{RepoId, RepoMapQueryRequest, RepoMapQueryResponse, RevisionId};
use quanta_index_contract::{RepoMapActivateGenerationRequest, RepoMapSourceBundle};
use quanta_index_core::{
    CoreError, RepoMapBundleIngestPort, RepoMapGenerationActivatePort, RepoMapQueryPort,
};

use crate::{
    RepoMapMaterializer, RepoMapQueryEngine,
    model::RepoMapSnapshot,
    persistence::{RepoMapActivationRecordV1, RepoMapSnapshotPersistence},
};

#[derive(Debug)]
pub struct RepoMapGenerationStore {
    snapshots: RwLock<BTreeMap<RepoMapStoreKeyV1, RepoMapSnapshot>>,
    activated: RwLock<BTreeMap<(String, String), u64>>,
    persistence: Option<RepoMapSnapshotPersistence>,
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
        manifest_generation: quanta_index_contract::ManifestGeneration,
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
    pub fn with_persistence_root(root: impl AsRef<Path>) -> Result<Self, CoreError> {
        let persistence = RepoMapSnapshotPersistence::open(root)?;
        let mut snapshots = BTreeMap::new();
        for snapshot in persistence.load_snapshots()? {
            let key = RepoMapStoreKeyV1::new(
                &snapshot.repo_id,
                &snapshot.revision_id,
                snapshot.manifest_generation,
            );
            let _prior = snapshots.insert(key, snapshot);
        }
        let mut activated = BTreeMap::new();
        for RepoMapActivationRecordV1 {
            repo_id,
            revision_id,
            manifest_generation,
        } in persistence.load_activations()?
        {
            let key = RepoMapStoreKeyV1::new(
                &RepoId::new(&repo_id),
                &RevisionId::new(&revision_id),
                quanta_index_contract::ManifestGeneration::new(manifest_generation),
            );
            if !snapshots.contains_key(&key) {
                return Err(CoreError::Storage(format!(
                    "repomap activation persisted without snapshot for repo={repo_id} revision={revision_id} generation={manifest_generation}"
                )));
            }
            let _prior = activated.insert((repo_id, revision_id), manifest_generation);
        }
        Ok(Self {
            snapshots: RwLock::new(snapshots),
            activated: RwLock::new(activated),
            persistence: Some(persistence),
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
        {
            let mut guard = self
                .snapshots
                .write()
                .map_err(|err| CoreError::Storage(format!("repomap store poisoned: {err}")))?;
            let _prior = guard.insert(key, snapshot);
        }
        Ok(())
    }

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
        let snapshot = {
            let guard = self
                .snapshots
                .read()
                .map_err(|err| CoreError::Storage(format!("repomap store poisoned: {err}")))?;
            guard.get(&key).cloned().ok_or_else(|| {
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
