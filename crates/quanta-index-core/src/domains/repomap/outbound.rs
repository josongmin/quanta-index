use quanta_index_contract::{RepoMapActivateGenerationRequest, RepoMapSourceBundle};

use crate::CoreError;

pub trait RepoMapBundleIngestPort: Send + Sync {
    fn ingest_bundle(&self, bundle: &RepoMapSourceBundle) -> Result<(), CoreError>;
}

pub trait RepoMapGenerationActivatePort: Send + Sync {
    fn activate_generation(
        &self,
        request: &RepoMapActivateGenerationRequest,
    ) -> Result<(), CoreError>;
}
