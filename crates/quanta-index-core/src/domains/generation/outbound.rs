use quanta_index_contract::{
    PublishedGenerationSet, PublishedSearchBundleInspectResponse, PublishedSearchBundleManifest,
    PublishedSearchGenerationActivateRequest, PublishedSearchGenerationActivateResponse,
    PublishedSearchGenerationReadinessResponse, RepoId, RevisionId,
};

use crate::CoreError;

/// Driven port: activate a published generation for serving.
pub trait PublishedSearchGenerationActivatePort {
    fn activate_generation(
        &mut self,
        request: PublishedSearchGenerationActivateRequest,
    ) -> Result<PublishedSearchGenerationActivateResponse, CoreError>;
}

/// Driven port: read prepare/activate readiness for a repo revision.
pub trait PublishedSearchGenerationReadinessPort {
    fn read_readiness(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<PublishedSearchGenerationReadinessResponse, CoreError>;
}

/// Driven port: inspect bundle metadata for a generation set.
pub trait PublishedSearchBundleInspectPort {
    fn inspect_bundle(
        &self,
        generation: &PublishedGenerationSet,
    ) -> Result<PublishedSearchBundleInspectResponse, CoreError>;
}

/// Driven port: record manifest metadata in the generation catalog.
pub trait PublishedSearchGenerationCatalogPort {
    fn record_generation_manifest(
        &mut self,
        manifest: PublishedSearchBundleManifest,
    ) -> Result<(), CoreError>;
}

/// Driven port: mark the active generation head after build success.
///
/// Unlike [`PublishedSearchGenerationActivatePort`], this skips activation
/// policy validation — the caller has already proven both readiness flags by
/// completing the lexical and semantic builds. Used by the materialize
/// orchestrator to ratify activation only after successful index builds.
pub trait PublishedSearchActivationStatePort {
    fn mark_active_generation(
        &mut self,
        generation: &PublishedGenerationSet,
        active_at_ms: u64,
    ) -> Result<(), CoreError>;
}
