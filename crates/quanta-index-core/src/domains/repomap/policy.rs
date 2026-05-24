use quanta_index_contract::{
    RepoMapActivateGenerationRequestV1, RepoMapQueryRequestV1, RepoMapSnapshotReadRequestV1,
    RepoMapSourceBundleV1,
};

use crate::CoreError;

pub struct RepoMapPolicy;

impl RepoMapPolicy {
    pub fn validate_bundle(bundle: &RepoMapSourceBundleV1) -> Result<(), CoreError> {
        if bundle.snapshot_id.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap bundle: snapshot_id must not be empty".to_string(),
            ));
        }
        if bundle.authority_artifact_id.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap bundle: authority_artifact_id must not be empty".to_string(),
            ));
        }
        if bundle.authority_digest.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap bundle: authority_digest must not be empty".to_string(),
            ));
        }
        if bundle.projection_version == 0 {
            return Err(CoreError::InvalidContract(
                "repomap bundle: projection_version must be > 0".to_string(),
            ));
        }
        Ok(())
    }

    pub fn validate_query(request: &RepoMapQueryRequestV1) -> Result<(), CoreError> {
        if request.query_text.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap query: query_text must not be empty".to_string(),
            ));
        }
        if request.top_k == 0 {
            return Err(CoreError::InvalidContract(
                "repomap query: top_k must be > 0".to_string(),
            ));
        }
        if request.token_budget == 0 {
            return Err(CoreError::InvalidContract(
                "repomap query: token_budget must be > 0".to_string(),
            ));
        }
        Ok(())
    }

    pub fn validate_snapshot_read(
        request: &RepoMapSnapshotReadRequestV1,
    ) -> Result<(), CoreError> {
        if request.generation.repo_id.as_str().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap read: repo_id must not be empty".to_string(),
            ));
        }
        if request.generation.revision_id.as_str().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap read: revision_id must not be empty".to_string(),
            ));
        }
        Ok(())
    }

    pub fn validate_activation(
        request: &RepoMapActivateGenerationRequestV1,
    ) -> Result<(), CoreError> {
        if request.manifest_digest.trim().is_empty() {
            return Err(CoreError::InvalidContract(
                "repomap activate: manifest_digest must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}
