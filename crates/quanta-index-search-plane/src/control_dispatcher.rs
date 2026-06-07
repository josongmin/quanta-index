//! Search-plane control orchestration.
//!
//! Control mutations are intentionally isolated from the read/query socket so
//! headless CLIs can remain view-only while admin or producer surfaces bind to
//! a separate control plane.

use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    CurrentGenerationRequest, GenerationSnapshot, GenerationStatusReport, GenerationStatusRequest,
    RepoMapActivateGenerationRequest, RepoMapMutationAck, SearchPlaneActivateGenerationRequest,
    SearchPlaneActivationAck, SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse,
    SearchPlaneIpcError, SearchPlaneTrackKind, TrackReadinessRecord,
};
use quanta_index_core::{CoreError, RepoMapGenerationActivatePort};

use crate::{ActivationCatalog, Ledger};

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";
const ERR_SEMANTIC_ACTIVATION_REGRESSION: &str = "SEMANTIC_ACTIVATION_REGRESSION";

pub struct SearchPlaneControlDispatcher {
    // QI-INT-01: `repo_map_ingest` field removed. RepoMap bundle ingest now
    // lives exclusively on the ingest IPC surface
    // (`SearchPlaneIngestDispatcher`). The composition root still wires
    // `RepoMapBundleIngestPort` into that dispatcher; this control surface
    // no longer needs the port.
    repo_map_activate: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
    activation_catalog: Arc<ActivationCatalog>,
    ledger: Arc<RwLock<Ledger>>,
}

impl SearchPlaneControlDispatcher {
    #[must_use]
    pub fn new(
        repo_map_activate: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
        activation_catalog: Arc<ActivationCatalog>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> Self {
        Self {
            repo_map_activate,
            activation_catalog,
            ledger,
        }
    }

    fn repo_map_activate(
        &self,
        request: RepoMapActivateGenerationRequest,
    ) -> Result<RepoMapMutationAck, CoreError> {
        self.repo_map_activate.activate_generation(&request)?;
        Ok(RepoMapMutationAck {
            repo_id: request.repo_id,
            revision_id: request.revision_id,
            manifest_generation: request.manifest_generation,
        })
    }

    fn activate_generation(
        &self,
        request: SearchPlaneActivateGenerationRequest,
    ) -> Result<SearchPlaneActivationAck, CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        for track in &request.tracks {
            self.validate_track_activation(&guard, &request, *track)?;
        }
        drop(guard);
        self.activation_catalog.activate(&request)?;
        Ok(SearchPlaneActivationAck {
            repo_id: request.repo_id,
            revision_id: request.revision_id,
            manifest_generation: request.manifest_generation,
            manifest_digest: request.manifest_digest,
            tracks: request.tracks,
        })
    }

    fn validate_track_activation(
        &self,
        guard: &Ledger,
        request: &SearchPlaneActivateGenerationRequest,
        track: SearchPlaneTrackKind,
    ) -> Result<(), CoreError> {
        if track == SearchPlaneTrackKind::Semantic {
            return validate_semantic_track_activation(&self.activation_catalog, guard, request);
        }
        let materialized = guard.track_sealed(&request.repo_id, &request.revision_id, track);
        match materialized {
            Some(generation) if generation.get() >= request.manifest_generation.get() => Ok(()),
            Some(generation) => Err(CoreError::NotReady(format!(
                "activate-generation: {track:?} materialized only up to {} for repo={} revision={}",
                generation.get(),
                request.repo_id.as_str(),
                request.revision_id.as_str(),
            ))),
            None => Err(CoreError::NotReady(format!(
                "activate-generation: no materialized {track:?} generation for repo={} revision={}",
                request.repo_id.as_str(),
                request.revision_id.as_str(),
            ))),
        }
    }

    /// QI-ACT-01: resolve one `(repo, revision, track)` triple to its active
    /// generation snapshot. Missing entry surfaces as `CoreError::NotReady`
    /// → `Error { code: "NOT_READY", ... }` (fail-closed; no silent fallback).
    fn current_generation(
        &self,
        request: &CurrentGenerationRequest,
    ) -> Result<GenerationSnapshot, CoreError> {
        let record = self.activation_catalog.resolve_record(
            &request.repo_id,
            &request.revision_id,
            request.track,
        )?;
        Ok(GenerationSnapshot {
            repo_id: record.repo_id,
            revision_id: record.revision_id,
            track: record.track,
            manifest_generation: record.manifest_generation,
            manifest_digest: record.manifest_digest,
        })
    }

    /// QI-ACT-01: return every active track for one `(repo, revision)` pair.
    /// An empty `tracks` vec means nothing is activated yet — a legitimate
    /// state distinct from a per-track `NotReady`.
    fn generation_status(
        &self,
        request: GenerationStatusRequest,
    ) -> Result<GenerationStatusReport, CoreError> {
        let records = self
            .activation_catalog
            .entries_for(&request.repo_id, &request.revision_id)?;
        let tracks = records
            .into_iter()
            .map(|record| TrackReadinessRecord {
                track: record.track,
                manifest_generation: record.manifest_generation,
                manifest_digest: record.manifest_digest,
            })
            .collect();
        Ok(GenerationStatusReport {
            repo_id: request.repo_id,
            revision_id: request.revision_id,
            tracks,
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
            SearchPlaneControlIpcRequest::RepoMapActivate(request) => {
                match self.repo_map_activate(request) {
                    Ok(resp) => SearchPlaneControlIpcResponse::RepoMapMutationAck(resp),
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneControlIpcRequest::CurrentGeneration(request) => {
                match self.current_generation(&request) {
                    Ok(snapshot) => {
                        SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(snapshot)
                    }
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneControlIpcRequest::GenerationStatus(request) => {
                match self.generation_status(request) {
                    Ok(report) => SearchPlaneControlIpcResponse::GenerationStatusReport(report),
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
    // Control-plane failures carry no query-intent repair metadata (J7Q-06);
    // the wire field stays None.
    SearchPlaneIpcError {
        code,
        message,
        repair: None,
    }
}

fn validate_semantic_track_activation(
    activation_catalog: &ActivationCatalog,
    guard: &Ledger,
    request: &SearchPlaneActivateGenerationRequest,
) -> Result<(), CoreError> {
    if let Ok(active) = activation_catalog.resolve_record(
        &request.repo_id,
        &request.revision_id,
        SearchPlaneTrackKind::Semantic,
    ) && request.manifest_generation.get() < active.manifest_generation.get()
    {
        return Err(CoreError::Typed {
            code: ERR_SEMANTIC_ACTIVATION_REGRESSION.to_string(),
            message: format!(
                "activate-generation: semantic generation regression for repo={} revision={}: requested={} active={}",
                request.repo_id.as_str(),
                request.revision_id.as_str(),
                request.manifest_generation.get(),
                active.manifest_generation.get(),
            ),
        });
    }
    guard.validate_semantic_generation(
        &request.repo_id,
        &request.revision_id,
        request.manifest_generation,
        Some(request.manifest_digest.as_str()),
        true,
        "activate-generation",
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use super::SearchPlaneControlDispatcher;
    use quanta_index_contract::{
        ManifestGeneration, RepoId, RepoMapActivateGenerationRequest, RepoMapMutationAck,
        RevisionId, SearchPlaneActivateGenerationRequest, SearchPlaneActivationAck,
        SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneTrackKind,
    };
    use quanta_index_core::{CoreError, RepoMapGenerationActivatePort};
    use tempfile::tempdir;

    use crate::{ActivationCatalog, Ledger};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    struct StubRepoMapActivatePort;

    impl RepoMapGenerationActivatePort for StubRepoMapActivatePort {
        fn activate_generation(
            &self,
            request: &RepoMapActivateGenerationRequest,
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
    ) -> Result<RepoMapMutationAck, Box<dyn std::error::Error>> {
        match response {
            SearchPlaneControlIpcResponse::RepoMapMutationAck(ack) => Ok(ack),
            other @ (SearchPlaneControlIpcResponse::ActivationAck(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
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
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
                Err(format!("expected activation ack, got {other:?}").into())
            }
        }
    }

    fn into_error_code(
        response: SearchPlaneControlIpcResponse,
    ) -> Result<String, Box<dyn std::error::Error>> {
        match response {
            SearchPlaneControlIpcResponse::Error(err) => Ok(err.code),
            other @ (SearchPlaneControlIpcResponse::ActivationAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
                Err(format!("expected error response, got {other:?}").into())
            }
        }
    }

    fn into_error(
        response: SearchPlaneControlIpcResponse,
    ) -> Result<quanta_index_contract::SearchPlaneIpcError, Box<dyn std::error::Error>> {
        match response {
            SearchPlaneControlIpcResponse::Error(err) => Ok(err),
            other @ (SearchPlaneControlIpcResponse::ActivationAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
                Err(format!("expected error response, got {other:?}").into())
            }
        }
    }

    #[test]
    fn repo_map_control_branches_ack() -> TestResult {
        // QI-INT-01: control surface only handles `RepoMapActivate` and
        // activation queries; the ingest variant moved to the ingest IPC
        // (`SearchPlaneIngestIpcRequest::PublishRepoMapBundle`). See
        // `ingest_dispatcher` tests for the ingest-side coverage.
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        {
            let mut guard = ledger
                .write()
                .map_err(|err| format!("ledger poisoned: {err}"))?;
            let repo_id = RepoId::new("repo-map-ipc");
            let revision_id = RevisionId::new("rev-map-ipc");
            guard.record_track_materialized(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Semantic,
                ManifestGeneration::new(11),
                Some("manifest-digest-11"),
            );
            guard.record_track_seal(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Lexical,
                ManifestGeneration::new(11),
            );
            guard.record_track_seal_with_digest(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Semantic,
                ManifestGeneration::new(11),
                "manifest-digest-11",
            );
        }
        let dispatcher = SearchPlaneControlDispatcher::new(
            Arc::new(StubRepoMapActivatePort),
            activation_catalog.clone(),
            ledger,
        );

        let activate = into_repo_map_mutation_ack(dispatcher.dispatch(
            SearchPlaneControlIpcRequest::RepoMapActivate(RepoMapActivateGenerationRequest {
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

    #[test]
    fn activate_generation_rejects_semantic_digest_mismatch() -> TestResult {
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        {
            let mut guard = ledger
                .write()
                .map_err(|err| format!("ledger poisoned: {err}"))?;
            let repo_id = RepoId::new("repo-sem");
            let revision_id = RevisionId::new("rev-sem");
            guard.record_track_seal(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Lexical,
                ManifestGeneration::new(11),
            );
            guard.record_track_materialized(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Semantic,
                ManifestGeneration::new(11),
                Some("manifest-digest-11"),
            );
            guard.record_track_seal_with_digest(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Semantic,
                ManifestGeneration::new(11),
                "manifest-digest-11",
            );
        }
        let dispatcher = SearchPlaneControlDispatcher::new(
            Arc::new(StubRepoMapActivatePort),
            activation_catalog,
            ledger,
        );
        let code = into_error_code(dispatcher.dispatch(
            SearchPlaneControlIpcRequest::ActivateGeneration(
                SearchPlaneActivateGenerationRequest {
                    repo_id: RepoId::new("repo-sem"),
                    revision_id: RevisionId::new("rev-sem"),
                    manifest_generation: ManifestGeneration::new(11),
                    manifest_digest: "manifest-digest-other".to_string(),
                    tracks: vec![
                        SearchPlaneTrackKind::Lexical,
                        SearchPlaneTrackKind::Semantic,
                    ],
                },
            ),
        ))?;
        if code != "SEMANTIC_MANIFEST_DIGEST_MISMATCH" {
            return Err(format!("unexpected semantic mismatch code: {code}").into());
        }
        Ok(())
    }

    #[test]
    fn activate_generation_rejects_unsealed_semantic_generation() -> TestResult {
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        {
            let mut guard = ledger
                .write()
                .map_err(|err| format!("ledger poisoned: {err}"))?;
            let repo_id = RepoId::new("repo-sem");
            let revision_id = RevisionId::new("rev-sem");
            guard.record_track_seal(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Lexical,
                ManifestGeneration::new(11),
            );
            guard.record_track_materialized(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Semantic,
                ManifestGeneration::new(11),
                Some("manifest-digest-11"),
            );
        }
        let dispatcher = SearchPlaneControlDispatcher::new(
            Arc::new(StubRepoMapActivatePort),
            activation_catalog,
            ledger,
        );
        let code = into_error_code(dispatcher.dispatch(
            SearchPlaneControlIpcRequest::ActivateGeneration(
                SearchPlaneActivateGenerationRequest {
                    repo_id: RepoId::new("repo-sem"),
                    revision_id: RevisionId::new("rev-sem"),
                    manifest_generation: ManifestGeneration::new(11),
                    manifest_digest: "manifest-digest-11".to_string(),
                    tracks: vec![
                        SearchPlaneTrackKind::Lexical,
                        SearchPlaneTrackKind::Semantic,
                    ],
                },
            ),
        ))?;
        if code != "SEMANTIC_GENERATION_NOT_SEALED" {
            return Err(format!("unexpected semantic unsealed code: {code}").into());
        }
        Ok(())
    }

    #[test]
    fn activate_generation_rejects_semantic_active_generation_regression() -> TestResult {
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        {
            let mut guard = ledger
                .write()
                .map_err(|err| format!("ledger poisoned: {err}"))?;
            let repo_id = RepoId::new("repo-sem");
            let revision_id = RevisionId::new("rev-sem");
            guard.record_track_seal(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Lexical,
                ManifestGeneration::new(10),
            );
            guard.record_track_seal(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Lexical,
                ManifestGeneration::new(11),
            );
            guard.record_track_materialized(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Semantic,
                ManifestGeneration::new(10),
                Some("manifest-digest-10"),
            );
            guard.record_track_seal_with_digest(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Semantic,
                ManifestGeneration::new(10),
                "manifest-digest-10",
            );
            guard.record_track_materialized(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Semantic,
                ManifestGeneration::new(11),
                Some("manifest-digest-11"),
            );
            guard.record_track_seal_with_digest(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Semantic,
                ManifestGeneration::new(11),
                "manifest-digest-11",
            );
        }
        activation_catalog.activate(&SearchPlaneActivateGenerationRequest {
            repo_id: RepoId::new("repo-sem"),
            revision_id: RevisionId::new("rev-sem"),
            manifest_generation: ManifestGeneration::new(11),
            manifest_digest: "manifest-digest-11".to_string(),
            tracks: vec![SearchPlaneTrackKind::Semantic],
        })?;
        let dispatcher = SearchPlaneControlDispatcher::new(
            Arc::new(StubRepoMapActivatePort),
            activation_catalog,
            ledger,
        );

        let err = into_error(dispatcher.dispatch(
            SearchPlaneControlIpcRequest::ActivateGeneration(
                SearchPlaneActivateGenerationRequest {
                    repo_id: RepoId::new("repo-sem"),
                    revision_id: RevisionId::new("rev-sem"),
                    manifest_generation: ManifestGeneration::new(10),
                    manifest_digest: "manifest-digest-10".to_string(),
                    tracks: vec![
                        SearchPlaneTrackKind::Lexical,
                        SearchPlaneTrackKind::Semantic,
                    ],
                },
            ),
        ))?;
        if err.code != super::ERR_SEMANTIC_ACTIVATION_REGRESSION {
            return Err(format!("unexpected semantic regression code: {}", err.code).into());
        }
        if err.message
            != "activate-generation: semantic generation regression for repo=repo-sem revision=rev-sem: requested=10 active=11"
        {
            return Err(format!("unexpected semantic regression message: {}", err.message).into());
        }
        Ok(())
    }
}
