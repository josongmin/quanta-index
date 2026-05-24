use quanta_index_contract::{
    RepoMapActivateGenerationRequestV1, RepoMapSourceBundleV1,
};

use crate::CoreError;

pub trait RepoMapBundleIngestPort: Send + Sync {
    fn ingest(&self, bundle: RepoMapSourceBundleV1) -> Result<(), CoreError>;
}

pub trait RepoMapGenerationActivatePort: Send + Sync {
    fn activate_generation(
        &self,
        request: RepoMapActivateGenerationRequestV1,
    ) -> Result<(), CoreError>;
}
