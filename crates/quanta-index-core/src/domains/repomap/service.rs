use quanta_index_contract::{
    RepoMapActivateGenerationRequestV1, RepoMapQueryRequestV1, RepoMapSnapshotReadRequestV1,
    RepoMapSourceBundleV1,
};

use crate::CoreError;

use super::RepoMapPolicy;

pub struct RepoMapService;

impl RepoMapService {
    pub fn validate_bundle(bundle: &RepoMapSourceBundleV1) -> Result<(), CoreError> {
        RepoMapPolicy::validate_bundle(bundle)
    }

    pub fn validate_query(request: &RepoMapQueryRequestV1) -> Result<(), CoreError> {
        RepoMapPolicy::validate_query(request)
    }

    pub fn validate_snapshot_read(
        request: &RepoMapSnapshotReadRequestV1,
    ) -> Result<(), CoreError> {
        RepoMapPolicy::validate_snapshot_read(request)
    }

    pub fn validate_activation(
        request: &RepoMapActivateGenerationRequestV1,
    ) -> Result<(), CoreError> {
        RepoMapPolicy::validate_activation(request)
    }
}
