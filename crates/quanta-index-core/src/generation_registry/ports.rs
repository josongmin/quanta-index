use quanta_index_contract::{
    PublishedGenerationSet, PublishedSearchBundleInspectResponse, PublishedSearchBundleManifest,
    PublishedSearchGenerationActivateRequest, PublishedSearchGenerationActivateResponse,
    PublishedSearchGenerationReadinessResponse, RepoId, RevisionId,
};

use crate::CoreError;

pub trait PublishedSearchGenerationActivatePort {
    fn activate_generation(
        &mut self,
        request: PublishedSearchGenerationActivateRequest,
    ) -> Result<PublishedSearchGenerationActivateResponse, CoreError>;
}

pub trait PublishedSearchGenerationReadinessPort {
    fn read_readiness(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<PublishedSearchGenerationReadinessResponse, CoreError>;
}

pub trait PublishedSearchBundleInspectPort {
    fn inspect_bundle(
        &self,
        generation: &PublishedGenerationSet,
    ) -> Result<PublishedSearchBundleInspectResponse, CoreError>;
}

pub trait PublishedSearchGenerationCatalogPort {
    fn record_generation_manifest(
        &mut self,
        manifest: PublishedSearchBundleManifest,
    ) -> Result<(), CoreError>;
}

pub trait PublishedSearchActivationStatePort {
    fn mark_active_generation(
        &mut self,
        generation: &PublishedGenerationSet,
    ) -> Result<(), CoreError>;
}
