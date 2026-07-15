//! Search-plane control orchestration.
//!
//! Control mutations are intentionally isolated from the read/query socket so
//! headless CLIs can remain view-only while admin or producer surfaces bind to
//! a separate control plane.

use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    CurrentGenerationRequest, GenerationSnapshot, GenerationStatusReport, GenerationStatusRequest,
    ManifestGeneration, RepoMapActivateGenerationRequest, RepoMapMutationAck,
    SearchCorpusGenerationIdentityV1, SearchPlaneActivateSearchCorpusGenerationCasRequest,
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneIpcError,
    SearchPlaneRollbackGenerationAck, SearchPlaneRollbackGenerationRequest,
    SearchPlaneSearchCorpusActivationCasAck, SearchPlaneTrackKind, TrackReadinessRecord,
};
use quanta_index_core::{CoreError, RepoMapGenerationActivatePort};

use crate::{
    ActivationCatalog, Ledger, PreparedSearchCorpusGenerationV1, SearchCorpusGenerationActivationV1,
};

const ERR_INVALID: &str = "INVALID_REQUEST";
const ERR_NOT_READY: &str = "NOT_READY";
const ERR_NOT_FOUND: &str = "NOT_FOUND";
const ERR_NOT_IMPLEMENTED: &str = "NOT_IMPLEMENTED";
const ERR_INTERNAL: &str = "INTERNAL";
#[cfg(test)]
const ERR_ROLLBACK_CAS_CONFLICT: &str = crate::readiness::ERR_ROLLBACK_CAS_CONFLICT;

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

    /// Promote a prepared lexical plus semantic corpus after proving both
    /// sealed identities against the same readiness snapshot.
    pub fn activate_prepared_search_corpus_generation_v1(
        &self,
        prepared: PreparedSearchCorpusGenerationV1,
    ) -> Result<SearchCorpusGenerationActivationV1, CoreError> {
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        validate_candidate_activation(&guard, prepared.candidate().lexical())?;
        validate_candidate_activation(&guard, prepared.candidate().semantic())?;
        // Readiness only advances after sealing. Holding this guard through
        // the durable catalog CAS prevents either half of a prepared corpus
        // from becoming unsealed before the composite root is committed.
        self.activation_catalog
            .activate_prepared_search_corpus_generation_v1(&prepared)
    }

    fn activate_search_corpus_generation_cas(
        &self,
        request: SearchPlaneActivateSearchCorpusGenerationCasRequest,
    ) -> Result<SearchPlaneSearchCorpusActivationCasAck, CoreError> {
        let candidate = search_corpus_generation_from_contract(&request.candidate)?;
        let expected_active = request
            .expected_active
            .as_ref()
            .map(search_corpus_generation_from_contract)
            .transpose()?;
        let prepared = PreparedSearchCorpusGenerationV1::new(candidate, expected_active)?;
        let activation = self.activate_prepared_search_corpus_generation_v1(prepared)?;
        Ok(SearchPlaneSearchCorpusActivationCasAck {
            active: search_corpus_generation_into_contract(activation.active),
            previous_sealed_active: activation
                .previous_active
                .map(search_corpus_generation_into_contract),
        })
    }

    fn rollback_generation(
        &self,
        request: SearchPlaneRollbackGenerationRequest,
    ) -> Result<SearchPlaneRollbackGenerationAck, CoreError> {
        if request.track != SearchPlaneTrackKind::Semantic {
            return Err(CoreError::InvalidContract(
                "rollback-generation: only semantic track supports explicit rollback".to_string(),
            ));
        }
        let guard = self
            .ledger
            .read()
            .map_err(|err| CoreError::Storage(format!("ledger poisoned: {err}")))?;
        let lexical_target = GenerationSnapshot {
            repo_id: request.repo_id.clone(),
            revision_id: request.revision_id.clone(),
            track: SearchPlaneTrackKind::Lexical,
            manifest_generation: request.target_generation,
            manifest_digest: request.target_manifest_digest.clone(),
        };
        let semantic_target = GenerationSnapshot {
            repo_id: request.repo_id.clone(),
            revision_id: request.revision_id.clone(),
            track: SearchPlaneTrackKind::Semantic,
            manifest_generation: request.target_generation,
            manifest_digest: request.target_manifest_digest.clone(),
        };
        guard
            .validate_historically_sealed_track_identity(&lexical_target, "rollback-generation")?;
        guard
            .validate_historically_sealed_track_identity(&semantic_target, "rollback-generation")?;
        drop(guard);
        self.activation_catalog.rollback(&request)
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
            SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(request) => {
                match self.activate_search_corpus_generation_cas(request) {
                    Ok(resp) => SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(resp),
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneControlIpcRequest::RollbackGeneration(request) => {
                match self.rollback_generation(request) {
                    Ok(resp) => SearchPlaneControlIpcResponse::RollbackAck(resp),
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

fn validate_candidate_activation(
    guard: &Ledger,
    candidate: &GenerationSnapshot,
) -> Result<(), CoreError> {
    let observed_generation =
        guard.track_sealed(&candidate.repo_id, &candidate.revision_id, candidate.track);
    let observed_digest =
        guard.track_manifest_digest(&candidate.repo_id, &candidate.revision_id, candidate.track);
    if observed_generation != Some(candidate.manifest_generation)
        || observed_digest != Some(candidate.manifest_digest.as_str())
    {
        return Err(CoreError::NotReady(format!(
            "activate-generation-cas: candidate is not the currently sealed track identity for repo={} revision={} track={:?}: candidate_generation={} candidate_digest={} observed_generation={:?} observed_digest={:?}",
            candidate.repo_id.as_str(),
            candidate.revision_id.as_str(),
            candidate.track,
            candidate.manifest_generation.get(),
            candidate.manifest_digest,
            observed_generation.map(ManifestGeneration::get),
            observed_digest,
        )));
    }
    Ok(())
}

fn search_corpus_generation_from_contract(
    identity: &SearchCorpusGenerationIdentityV1,
) -> Result<crate::SearchCorpusGenerationV1, CoreError> {
    identity.validate_v1().map_err(|error| {
        CoreError::InvalidContract(format!(
            "search-corpus activation: invalid composite identity: {}",
            error.code_v1()
        ))
    })?;
    crate::SearchCorpusGenerationV1::new(identity.lexical.clone(), identity.semantic.clone())
}

fn search_corpus_generation_into_contract(
    identity: crate::SearchCorpusGenerationV1,
) -> SearchCorpusGenerationIdentityV1 {
    SearchCorpusGenerationIdentityV1 {
        lexical: identity.lexical().clone(),
        semantic: identity.semantic().clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use super::SearchPlaneControlDispatcher;
    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, RepoId, RepoMapActivateGenerationRequest,
        RepoMapMutationAck, RevisionId, SearchCorpusGenerationIdentityV1,
        SearchPlaneActivateSearchCorpusGenerationCasRequest, SearchPlaneControlIpcRequest,
        SearchPlaneControlIpcResponse, SearchPlaneRollbackGenerationRequest, SearchPlaneTrackKind,
    };
    use quanta_index_core::{CoreError, RepoMapGenerationActivatePort};
    use tempfile::tempdir;

    use crate::{
        ActivationCatalog, Ledger, PreparedSearchCorpusGenerationV1, SearchCorpusGenerationV1,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn composite_generation(
        repo_id: &str,
        revision_id: &str,
        generation: u64,
        manifest_digest: &str,
    ) -> Result<SearchCorpusGenerationV1, quanta_index_core::CoreError> {
        SearchCorpusGenerationV1::new(
            GenerationSnapshot {
                repo_id: RepoId::new(repo_id),
                revision_id: RevisionId::new(revision_id),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(generation),
                manifest_digest: manifest_digest.to_string(),
            },
            GenerationSnapshot {
                repo_id: RepoId::new(repo_id),
                revision_id: RevisionId::new(revision_id),
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation: ManifestGeneration::new(generation),
                manifest_digest: manifest_digest.to_string(),
            },
        )
    }

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
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::RollbackAck(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
                Err(format!("expected repo-map mutation ack, got {other:?}").into())
            }
        }
    }

    fn into_error_code(
        response: SearchPlaneControlIpcResponse,
    ) -> Result<String, Box<dyn std::error::Error>> {
        match response {
            SearchPlaneControlIpcResponse::Error(err) => Ok(err.code),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::RollbackAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
                Err(format!("expected error response, got {other:?}").into())
            }
        }
    }

    #[test]
    fn repo_map_control_and_prepared_corpus_activation_preserve_composite_identity() -> TestResult {
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
            guard.record_track_seal_with_digest(
                &repo_id,
                &revision_id,
                SearchPlaneTrackKind::Lexical,
                ManifestGeneration::new(11),
                "manifest-digest-11",
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

        let response = dispatcher.dispatch(
            SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
                SearchPlaneActivateSearchCorpusGenerationCasRequest {
                    candidate: SearchCorpusGenerationIdentityV1 {
                        lexical: GenerationSnapshot {
                            repo_id: RepoId::new("repo-map-ipc"),
                            revision_id: RevisionId::new("rev-map-ipc"),
                            track: SearchPlaneTrackKind::Lexical,
                            manifest_generation: ManifestGeneration::new(11),
                            manifest_digest: "manifest-digest-11".to_string(),
                        },
                        semantic: GenerationSnapshot {
                            repo_id: RepoId::new("repo-map-ipc"),
                            revision_id: RevisionId::new("rev-map-ipc"),
                            track: SearchPlaneTrackKind::Semantic,
                            manifest_generation: ManifestGeneration::new(11),
                            manifest_digest: "manifest-digest-11".to_string(),
                        },
                    },
                    expected_active: None,
                },
            ),
        );
        let SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(activation) = response
        else {
            return Err("expected composite activation acknowledgment".into());
        };
        if activation.active.lexical.manifest_generation.get() != 11 {
            return Err(format!(
                "unexpected activation manifest generation: {}",
                activation.active.lexical.manifest_generation.get()
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
        let semantic_pin = activation_catalog.resolve(
            &RepoId::new("repo-map-ipc"),
            &RevisionId::new("rev-map-ipc"),
            SearchPlaneTrackKind::Semantic,
        )?;
        assert_eq!(
            semantic_pin.manifest_generation,
            lexical_pin.manifest_generation
        );
        Ok(())
    }

    #[test]
    fn composite_ipc_rejects_malformed_identity_before_catalog_mutation() -> TestResult {
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let dispatcher = SearchPlaneControlDispatcher::new(
            Arc::new(StubRepoMapActivatePort),
            Arc::clone(&activation_catalog),
            Arc::new(RwLock::new(Ledger::new())),
        );
        let code = into_error_code(dispatcher.dispatch(
            SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
                SearchPlaneActivateSearchCorpusGenerationCasRequest {
                    candidate: SearchCorpusGenerationIdentityV1 {
                        lexical: GenerationSnapshot {
                            repo_id: RepoId::new("repo-invalid"),
                            revision_id: RevisionId::new("rev-invalid"),
                            track: SearchPlaneTrackKind::Semantic,
                            manifest_generation: ManifestGeneration::new(11),
                            manifest_digest: "manifest-digest-11".to_string(),
                        },
                        semantic: GenerationSnapshot {
                            repo_id: RepoId::new("repo-invalid"),
                            revision_id: RevisionId::new("rev-invalid"),
                            track: SearchPlaneTrackKind::Semantic,
                            manifest_generation: ManifestGeneration::new(11),
                            manifest_digest: "manifest-digest-11".to_string(),
                        },
                    },
                    expected_active: None,
                },
            ),
        ))?;
        assert_eq!(code, super::ERR_INVALID);
        assert!(
            activation_catalog
                .resolve_record(
                    &RepoId::new("repo-invalid"),
                    &RevisionId::new("rev-invalid"),
                    SearchPlaneTrackKind::Lexical,
                )
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn rollback_generation_uses_explicit_semantic_cas_and_preserves_activate_monotonicity()
    -> TestResult {
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        {
            let mut guard = ledger
                .write()
                .map_err(|err| format!("ledger poisoned: {err}"))?;
            let repo_id = RepoId::new("repo-rollback");
            let revision_id = RevisionId::new("rev-rollback");
            for (generation, digest) in [(10, "manifest-digest-10"), (11, "manifest-digest-11")] {
                for track in [
                    SearchPlaneTrackKind::Lexical,
                    SearchPlaneTrackKind::Semantic,
                ] {
                    guard.record_track_materialized(
                        &repo_id,
                        &revision_id,
                        track,
                        ManifestGeneration::new(generation),
                        Some(digest),
                    );
                    guard.record_track_seal_with_digest(
                        &repo_id,
                        &revision_id,
                        track,
                        ManifestGeneration::new(generation),
                        digest,
                    );
                }
            }
        }
        let active =
            composite_generation("repo-rollback", "rev-rollback", 11, "manifest-digest-11")?;
        let prepared = PreparedSearchCorpusGenerationV1::new(active, None)?;
        let activation =
            activation_catalog.activate_prepared_search_corpus_generation_v1(&prepared)?;
        if activation.active.manifest_generation() != ManifestGeneration::new(11) {
            return Err("expected initial composite activation at generation 11".into());
        }
        let dispatcher = SearchPlaneControlDispatcher::new(
            Arc::new(StubRepoMapActivatePort),
            Arc::clone(&activation_catalog),
            ledger,
        );

        let response = dispatcher.dispatch(SearchPlaneControlIpcRequest::RollbackGeneration(
            SearchPlaneRollbackGenerationRequest {
                repo_id: RepoId::new("repo-rollback"),
                revision_id: RevisionId::new("rev-rollback"),
                track: SearchPlaneTrackKind::Semantic,
                expected_active_generation: ManifestGeneration::new(11),
                expected_active_manifest_digest: "manifest-digest-11".to_string(),
                target_generation: ManifestGeneration::new(10),
                target_manifest_digest: "manifest-digest-10".to_string(),
            },
        ));
        let SearchPlaneControlIpcResponse::RollbackAck(ack) = response else {
            return Err("expected rollback ack".into());
        };
        if ack.previous_generation != ManifestGeneration::new(11)
            || ack.manifest_generation != ManifestGeneration::new(10)
            || ack.manifest_digest != "manifest-digest-10"
        {
            return Err(format!("unexpected rollback ack: {ack:?}").into());
        }
        let current = activation_catalog.resolve_record(
            &RepoId::new("repo-rollback"),
            &RevisionId::new("rev-rollback"),
            SearchPlaneTrackKind::Semantic,
        )?;
        if current.manifest_generation != ManifestGeneration::new(10)
            || current.manifest_digest != "manifest-digest-10"
        {
            return Err(format!("rollback did not update active state: {current:?}").into());
        }
        let lexical = activation_catalog.resolve_record(
            &RepoId::new("repo-rollback"),
            &RevisionId::new("rev-rollback"),
            SearchPlaneTrackKind::Lexical,
        )?;
        if lexical.manifest_generation != ManifestGeneration::new(10)
            || lexical.manifest_digest != "manifest-digest-10"
        {
            return Err(format!("rollback did not update lexical state: {lexical:?}").into());
        }
        let reopened = ActivationCatalog::open(dir.path())?;
        let reopened_lexical = reopened.resolve_record(
            &RepoId::new("repo-rollback"),
            &RevisionId::new("rev-rollback"),
            SearchPlaneTrackKind::Lexical,
        )?;
        let reopened_semantic = reopened.resolve_record(
            &RepoId::new("repo-rollback"),
            &RevisionId::new("rev-rollback"),
            SearchPlaneTrackKind::Semantic,
        )?;
        if reopened_lexical.manifest_generation != ManifestGeneration::new(10)
            || reopened_semantic.manifest_generation != ManifestGeneration::new(10)
            || reopened_lexical.manifest_digest != "manifest-digest-10"
            || reopened_semantic.manifest_digest != "manifest-digest-10"
        {
            return Err("composite rollback did not survive reopen".into());
        }

        let stale = dispatcher.dispatch(SearchPlaneControlIpcRequest::RollbackGeneration(
            SearchPlaneRollbackGenerationRequest {
                repo_id: RepoId::new("repo-rollback"),
                revision_id: RevisionId::new("rev-rollback"),
                track: SearchPlaneTrackKind::Semantic,
                expected_active_generation: ManifestGeneration::new(11),
                expected_active_manifest_digest: "manifest-digest-11".to_string(),
                target_generation: ManifestGeneration::new(10),
                target_manifest_digest: "manifest-digest-10".to_string(),
            },
        ));
        let SearchPlaneControlIpcResponse::Error(error) = stale else {
            return Err("stale rollback unexpectedly succeeded".into());
        };
        if error.code != super::ERR_ROLLBACK_CAS_CONFLICT {
            return Err(format!("unexpected stale rollback code: {}", error.code).into());
        }

        let unsealed_target =
            dispatcher.dispatch(SearchPlaneControlIpcRequest::RollbackGeneration(
                SearchPlaneRollbackGenerationRequest {
                    repo_id: RepoId::new("repo-rollback"),
                    revision_id: RevisionId::new("rev-rollback"),
                    track: SearchPlaneTrackKind::Semantic,
                    expected_active_generation: ManifestGeneration::new(10),
                    expected_active_manifest_digest: "manifest-digest-10".to_string(),
                    target_generation: ManifestGeneration::new(9),
                    target_manifest_digest: "manifest-digest-9".to_string(),
                },
            ));
        let SearchPlaneControlIpcResponse::Error(error) = unsealed_target else {
            return Err("rollback to unsealed historical target unexpectedly succeeded".into());
        };
        if error.code != crate::readiness::ERR_SEARCH_TRACK_GENERATION_NOT_SEALED {
            return Err(format!("unexpected unsealed rollback code: {}", error.code).into());
        }
        Ok(())
    }
}
