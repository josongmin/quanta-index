//! Search-plane control orchestration.
//!
//! Control mutations are intentionally isolated from the read/query socket so
//! headless CLIs can remain view-only while admin or producer surfaces bind to
//! a separate control plane.

use std::sync::Arc;

use quanta_index_contract::{
    RepoMapActivateGenerationRequestV1, RepoMapMutationAckV1, RepoMapSourceBundleV1,
    SearchPlaneActivateGenerationRequest, SearchPlaneActivationAck, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcResponse, SearchPlaneIpcError,
};
use quanta_index_core::{CoreError, RepoMapBundleIngestPort, RepoMapGenerationActivatePort};

use crate::ActivationCatalog;

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";

pub struct SearchPlaneControlDispatcher {
    repo_map_ingest: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
    repo_map_activate: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
    activation_catalog: Arc<ActivationCatalog>,
}

impl SearchPlaneControlDispatcher {
    #[must_use]
    pub fn new(
        repo_map_ingest: Arc<dyn RepoMapBundleIngestPort + Send + Sync>,
        repo_map_activate: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
        activation_catalog: Arc<ActivationCatalog>,
    ) -> Self {
        Self {
            repo_map_ingest,
            repo_map_activate,
            activation_catalog,
        }
    }

    fn repo_map_ingest(
        &self,
        bundle: RepoMapSourceBundleV1,
    ) -> Result<RepoMapMutationAckV1, CoreError> {
        self.repo_map_ingest.ingest_bundle(&bundle)?;
        Ok(RepoMapMutationAckV1 {
            repo_id: bundle.repo_id,
            revision_id: bundle.revision_id,
            manifest_generation: bundle.manifest_generation,
        })
    }

    fn repo_map_activate(
        &self,
        request: RepoMapActivateGenerationRequestV1,
    ) -> Result<RepoMapMutationAckV1, CoreError> {
        self.repo_map_activate.activate_generation(&request)?;
        Ok(RepoMapMutationAckV1 {
            repo_id: request.repo_id,
            revision_id: request.revision_id,
            manifest_generation: request.manifest_generation,
        })
    }

    fn activate_generation(
        &self,
        request: SearchPlaneActivateGenerationRequest,
    ) -> Result<SearchPlaneActivationAck, CoreError> {
        self.activation_catalog.activate(&request)?;
        Ok(SearchPlaneActivationAck {
            repo_id: request.repo_id,
            revision_id: request.revision_id,
            manifest_generation: request.manifest_generation,
            manifest_digest: request.manifest_digest,
            tracks: request.tracks,
        })
    }

    #[must_use]
    pub fn dispatch(&self, request: SearchPlaneControlIpcRequest) -> SearchPlaneControlIpcResponse {
        match request {
            SearchPlaneControlIpcRequest::ActivateGeneration(request) => {
                match self.activate_generation(request) {
                    Ok(resp) => SearchPlaneControlIpcResponse::ActivationAck(resp),
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneControlIpcRequest::RepoMapIngest(bundle) => {
                match self.repo_map_ingest(bundle) {
                    Ok(resp) => SearchPlaneControlIpcResponse::RepoMapMutationAck(resp),
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneControlIpcRequest::RepoMapActivate(request) => {
                match self.repo_map_activate(request) {
                    Ok(resp) => SearchPlaneControlIpcResponse::RepoMapMutationAck(resp),
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
        }
    }
}

fn core_error_to_ipc(err: CoreError) -> SearchPlaneIpcError {
    let (code, message) = match err {
        CoreError::InvalidContract(msg) => (ERR_INVALID.to_string(), msg),
        CoreError::Typed { code, message } => (code, message),
        CoreError::NotReady(msg) => (ERR_NOT_READY.to_string(), msg),
        CoreError::NotImplemented(msg) => (ERR_NOT_IMPLEMENTED.to_string(), msg),
        CoreError::NotFound(msg) => (ERR_NOT_FOUND.to_string(), msg),
        CoreError::Storage(msg) => (ERR_INTERNAL.to_string(), msg),
    };
    SearchPlaneIpcError { code, message }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::SearchPlaneControlDispatcher;
    use quanta_index_contract::{
        ManifestGeneration, RepoId, RepoMapActivateGenerationRequestV1, RepoMapMutationAckV1,
        RepoMapSourceBundleV1, RevisionId, SearchPlaneActivateGenerationRequest,
        SearchPlaneActivationAck, SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
        SearchPlaneTrackKind,
    };
    use quanta_index_core::{CoreError, RepoMapBundleIngestPort, RepoMapGenerationActivatePort};
    use tempfile::tempdir;

    use crate::ActivationCatalog;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    struct StubRepoMapIngestPort;
    struct StubRepoMapActivatePort;

    impl RepoMapBundleIngestPort for StubRepoMapIngestPort {
        fn ingest_bundle(&self, bundle: &RepoMapSourceBundleV1) -> Result<(), CoreError> {
            if bundle.file_indices.is_empty() {
                return Err(CoreError::InvalidContract(
                    "repo-map ingest: file_indices must not be empty".to_string(),
                ));
            }
            Ok(())
        }
    }

    impl RepoMapGenerationActivatePort for StubRepoMapActivatePort {
        fn activate_generation(
            &self,
            request: &RepoMapActivateGenerationRequestV1,
        ) -> Result<(), CoreError> {
            if request.manifest_digest.is_empty() {
                return Err(CoreError::InvalidContract(
                    "repo-map activate: manifest_digest must not be empty".to_string(),
                ));
            }
            Ok(())
        }
    }

    fn into_repo_map_mutation_ack(
        response: SearchPlaneControlIpcResponse,
    ) -> Result<RepoMapMutationAckV1, Box<dyn std::error::Error>> {
        match response {
            SearchPlaneControlIpcResponse::RepoMapMutationAck(ack) => Ok(ack),
            other @ (SearchPlaneControlIpcResponse::ActivationAck(_)
            | SearchPlaneControlIpcResponse::Error(_)) => {
                Err(format!("expected repo-map mutation ack, got {other:?}").into())
            }
        }
    }

    fn into_activation_ack(
        response: SearchPlaneControlIpcResponse,
    ) -> Result<SearchPlaneActivationAck, Box<dyn std::error::Error>> {
        match response {
            SearchPlaneControlIpcResponse::ActivationAck(ack) => Ok(ack),
            other @ (SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::Error(_)) => {
                Err(format!("expected activation ack, got {other:?}").into())
            }
        }
    }

    #[test]
    fn repo_map_control_branches_ack() -> TestResult {
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let dispatcher = SearchPlaneControlDispatcher::new(
            Arc::new(StubRepoMapIngestPort),
            Arc::new(StubRepoMapActivatePort),
            activation_catalog.clone(),
        );

        let ingest = into_repo_map_mutation_ack(dispatcher.dispatch(
            SearchPlaneControlIpcRequest::RepoMapIngest(RepoMapSourceBundleV1 {
                repo_id: RepoId::new("repo-map-ipc"),
                revision_id: RevisionId::new("rev-map-ipc"),
                manifest_generation: ManifestGeneration::new(9),
                snapshot_id: "dispatch-snapshot".to_string(),
                projection_version: 1,
                authority_digest: "dispatch-digest".to_string(),
                item_index_availability: "available".to_string(),
                graph_coverage_class: "full".to_string(),
                exactness_summary: "exact".to_string(),
                redaction_state: "Unredacted".to_string(),
                file_indices: vec![quanta_index_contract::RepoMapFileIndexRecordV1 {
                    file_identity: "src/lib.rs".to_string(),
                    file_path: "src/lib.rs".to_string(),
                    file_kind: "library".to_string(),
                    line_count: 80,
                    symbol_records: Vec::new(),
                }],
                call_edges: Vec::new(),
                import_edges: Vec::new(),
                chunk_records: Vec::new(),
            }),
        ))?;
        if ingest.repo_id.as_str() != "repo-map-ipc" {
            return Err(format!("unexpected ingest repo id: {}", ingest.repo_id.as_str()).into());
        }

        let activate = into_repo_map_mutation_ack(dispatcher.dispatch(
            SearchPlaneControlIpcRequest::RepoMapActivate(RepoMapActivateGenerationRequestV1 {
                repo_id: RepoId::new("repo-map-ipc"),
                revision_id: RevisionId::new("rev-map-ipc"),
                manifest_generation: ManifestGeneration::new(9),
                manifest_digest: "manifest-digest-9".to_string(),
            }),
        ))?;
        if activate.manifest_generation.get() != 9 {
            return Err(format!(
                "unexpected activate manifest generation: {}",
                activate.manifest_generation.get()
            )
            .into());
        }

        let activation = into_activation_ack(dispatcher.dispatch(
            SearchPlaneControlIpcRequest::ActivateGeneration(
                SearchPlaneActivateGenerationRequest {
                    repo_id: RepoId::new("repo-map-ipc"),
                    revision_id: RevisionId::new("rev-map-ipc"),
                    manifest_generation: ManifestGeneration::new(11),
                    manifest_digest: "manifest-digest-11".to_string(),
                    tracks: vec![
                        SearchPlaneTrackKind::Lexical,
                        SearchPlaneTrackKind::Semantic,
                    ],
                },
            ),
        ))?;
        if activation.manifest_generation.get() != 11 {
            return Err(format!(
                "unexpected activation manifest generation: {}",
                activation.manifest_generation.get()
            )
            .into());
        }
        let lexical_pin = activation_catalog.resolve(
            &RepoId::new("repo-map-ipc"),
            &RevisionId::new("rev-map-ipc"),
            SearchPlaneTrackKind::Lexical,
        )?;
        if lexical_pin.manifest_generation.get() != 11 {
            return Err(format!(
                "unexpected lexical pin generation: {}",
                lexical_pin.manifest_generation.get()
            )
            .into());
        }
        Ok(())
    }
}
