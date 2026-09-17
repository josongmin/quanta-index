//! Search-plane control orchestration.
//!
//! Control mutations are intentionally isolated from the read/query socket so
//! headless CLIs can remain view-only while admin or producer surfaces bind to
//! a separate control plane.

use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    CurrentGenerationRequest, GenerationSnapshot, GenerationStatusReport, GenerationStatusRequest,
    MetricsSnapshotV1, RepoMapActivateGenerationRequest, RepoMapMutationAck,
    SearchCorpusGenerationIdentityV1, SearchPlaneActivateSearchCorpusGenerationCasRequest,
    SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneIpcError,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneSearchCorpusActivationCasAck,
    SearchPlaneSearchCorpusRollbackCasAck, TrackReadinessRecord,
};
use quanta_index_core::{
    CoreError, GenerationIdentityValidatePort, RepoMapGenerationActivatePort, RequestBudgetV1,
};

use crate::observability::ObservabilityScrape;
use crate::search_corpus_lifecycle::SearchCorpusLifecycleService;
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
    search_corpus_lifecycle: SearchCorpusLifecycleService,
    /// The metrics scrape (QI-BB-015): the query plane's aggregates and
    /// every source the composition root registered.
    observability: Arc<ObservabilityScrape>,
}

impl SearchPlaneControlDispatcher {
    #[must_use]
    pub fn new(
        repo_map_activate: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
        activation_catalog: Arc<ActivationCatalog>,
        ledger: Arc<RwLock<Ledger>>,
        lexical_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
        semantic_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
        observability: Arc<ObservabilityScrape>,
    ) -> Self {
        let search_corpus_lifecycle = SearchCorpusLifecycleService::new(
            activation_catalog.lifecycle_coordinator(),
            Arc::clone(&activation_catalog),
            ledger,
            lexical_generation_validator,
            semantic_generation_validator,
        );
        Self {
            repo_map_activate,
            activation_catalog,
            search_corpus_lifecycle,
            observability,
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
        prepared: &PreparedSearchCorpusGenerationV1,
    ) -> Result<SearchCorpusGenerationActivationV1, CoreError> {
        self.search_corpus_lifecycle.activate_prepared_v1(prepared)
    }

    fn activate_search_corpus_generation_cas(
        &self,
        request: &SearchPlaneActivateSearchCorpusGenerationCasRequest,
    ) -> Result<SearchPlaneSearchCorpusActivationCasAck, CoreError> {
        request.validate_v1().map_err(|error| {
            CoreError::InvalidContract(format!(
                "search-corpus activation: invalid request: {}",
                error.code_v1()
            ))
        })?;
        let candidate = search_corpus_generation_from_validated_contract(&request.candidate)?;
        let expected_active = request
            .expected_active
            .as_ref()
            .map(search_corpus_generation_from_validated_contract)
            .transpose()?;
        let prepared = PreparedSearchCorpusGenerationV1::new(candidate, expected_active)?;
        let activation = self.activate_prepared_search_corpus_generation_v1(&prepared)?;
        Ok(SearchPlaneSearchCorpusActivationCasAck {
            active: search_corpus_generation_into_contract(&activation.active),
            previous_sealed_active: activation
                .previous_active
                .map(|identity| search_corpus_generation_into_contract(&identity)),
        })
    }

    fn rollback_search_corpus_generation_cas(
        &self,
        request: &SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    ) -> Result<SearchPlaneSearchCorpusRollbackCasAck, CoreError> {
        request.validate_v1().map_err(|error| {
            CoreError::InvalidContract(format!(
                "search-corpus rollback: invalid request: {}",
                error.code_v1()
            ))
        })?;
        let target = search_corpus_generation_from_validated_contract(&request.target)?;
        self.search_corpus_lifecycle.rollback_v1(request, &target)
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

    /// QI-BB-015: every metric the daemon aggregates, in one snapshot.
    fn metrics_snapshot(&self) -> Result<MetricsSnapshotV1, CoreError> {
        self.observability.scrape()
    }

    /// Serve one control request (QI-BB-002).
    ///
    /// The budget is checked once, at entry: a request whose peer left or
    /// whose deadline passed while it waited for the serial dispatch slot is
    /// refused before it takes any lock. Past that point a control mutation
    /// is owned by the dispatcher and runs to its durable end regardless of
    /// the peer, because a half-applied activation is worse than an answer
    /// nobody reads.
    #[must_use]
    pub fn dispatch(
        &self,
        request: SearchPlaneControlIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneControlIpcResponse {
        if let Err(err) = budget.checkpoint("control:entry") {
            return SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err));
        }
        match request {
            SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(request) => {
                match self.activate_search_corpus_generation_cas(&request) {
                    Ok(resp) => SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(resp),
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(request) => {
                match self.rollback_search_corpus_generation_cas(&request) {
                    Ok(resp) => SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(resp),
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
            SearchPlaneControlIpcRequest::MetricsSnapshot(_request) => {
                match self.metrics_snapshot() {
                    Ok(snapshot) => SearchPlaneControlIpcResponse::MetricsSnapshot(snapshot),
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

fn search_corpus_generation_from_validated_contract(
    identity: &SearchCorpusGenerationIdentityV1,
) -> Result<crate::SearchCorpusGenerationV1, CoreError> {
    crate::SearchCorpusGenerationV1::new(identity.lexical.clone(), identity.semantic.clone())
}

fn search_corpus_generation_into_contract(
    identity: &crate::SearchCorpusGenerationV1,
) -> SearchCorpusGenerationIdentityV1 {
    SearchCorpusGenerationIdentityV1 {
        lexical: identity.lexical().clone(),
        semantic: identity.semantic().clone(),
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::panic_in_result_fn,
        reason = "Result-returning control tests use assertions as test-failure reporting"
    )]
    use std::sync::{Arc, RwLock};

    use super::SearchPlaneControlDispatcher;
    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, MetricsSnapshotRequest, MetricsSnapshotV1, RepoId,
        RepoMapActivateGenerationRequest, RepoMapMutationAck, RevisionId,
        SearchCorpusGenerationIdentityV1, SearchPlaneActivateSearchCorpusGenerationCasRequest,
        SearchPlaneControlIpcRequest, SearchPlaneControlIpcResponse, SearchPlaneIpcError,
        SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneTrackKind,
    };
    use quanta_index_core::{
        CoreError, GenerationIdentityValidatePort, MetricPointV1, MetricSourcePort,
        RepoMapGenerationActivatePort, RequestBudgetV1,
    };
    use quanta_index_lq_obs::{Dimensions, MetricKind, MetricSample};
    use tempfile::tempdir;

    use crate::observability::{BoundedQueryObsStore, ObservabilityScrape, QueryObsSink};
    use crate::{
        ActivationCatalog, Ledger, PreparedSearchCorpusGenerationV1, SearchCorpusGenerationV1,
    };

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

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

    fn composite_identity(
        repo_id: &str,
        revision_id: &str,
        generation: u64,
        manifest_digest: &str,
    ) -> Result<SearchCorpusGenerationIdentityV1, quanta_index_core::CoreError> {
        let generation = composite_generation(repo_id, revision_id, generation, manifest_digest)?;
        Ok(SearchCorpusGenerationIdentityV1 {
            lexical: generation.lexical().clone(),
            semantic: generation.semantic().clone(),
        })
    }

    struct StubRepoMapActivatePort;

    struct AlwaysValidGeneration;

    impl GenerationIdentityValidatePort for AlwaysValidGeneration {
        fn validate_generation_identity(
            &self,
            _candidate: &GenerationSnapshot,
        ) -> Result<(), CoreError> {
            Ok(())
        }
    }

    fn always_valid_generation() -> Arc<dyn GenerationIdentityValidatePort + Send + Sync> {
        Arc::new(AlwaysValidGeneration)
    }

    fn empty_scrape() -> Arc<ObservabilityScrape> {
        Arc::new(ObservabilityScrape::new(
            Arc::new(BoundedQueryObsStore::default()),
            Vec::new(),
        ))
    }

    /// A source that reports fixed points, or fails typed.
    struct FixedSource(Result<Vec<MetricPointV1>, &'static str>);

    impl MetricSourcePort for FixedSource {
        fn scrape(&self) -> Result<Vec<MetricPointV1>, CoreError> {
            match &self.0 {
                Ok(points) => Ok(points.clone()),
                Err(message) => Err(CoreError::Storage((*message).to_string())),
            }
        }
    }

    fn scrape_dispatcher(
        store: Arc<BoundedQueryObsStore>,
        sources: Vec<Arc<dyn MetricSourcePort>>,
    ) -> TestResult<SearchPlaneControlDispatcher> {
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        Ok(SearchPlaneControlDispatcher::new(
            Arc::new(StubRepoMapActivatePort),
            activation_catalog,
            Arc::new(RwLock::new(Ledger::new())),
            always_valid_generation(),
            always_valid_generation(),
            Arc::new(ObservabilityScrape::new(store, sources)),
        ))
    }

    fn scrape_via_control(
        dispatcher: &SearchPlaneControlDispatcher,
    ) -> Result<MetricsSnapshotV1, SearchPlaneIpcError> {
        match dispatcher.dispatch(
            SearchPlaneControlIpcRequest::MetricsSnapshot(MetricsSnapshotRequest),
            &RequestBudgetV1::unbounded(),
        ) {
            SearchPlaneControlIpcResponse::MetricsSnapshot(snapshot) => Ok(snapshot),
            SearchPlaneControlIpcResponse::Error(error) => Err(error),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapMutationAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)) => {
                Err(SearchPlaneIpcError {
                    code: "TEST_UNEXPECTED_RESPONSE".to_string(),
                    message: format!("{other:?}"),
                    repair: None,
                })
            }
        }
    }

    /// The control route answers the store's aggregates merged with every
    /// source, sorted by name, and a scrape after more traffic sees the
    /// larger totals (QI-BB-015).
    #[test]
    fn metrics_snapshot_route_merges_the_store_and_every_source() -> TestResult {
        let store = Arc::new(BoundedQueryObsStore::default());
        let dimensions = || Dimensions::new("LXE-10", "8", "local", "repo-control", 3);
        for _ in 0..7 {
            store.emit(MetricSample::new(
                "lq_query_intake_total",
                MetricKind::Counter,
                1.0,
                dimensions(),
            ));
        }
        store.emit(MetricSample::new(
            "lq_route_lexical_latency_ms",
            MetricKind::Histogram,
            4.0,
            dimensions(),
        ));
        let sources: Vec<Arc<dyn MetricSourcePort>> = vec![
            Arc::new(FixedSource(Ok(vec![
                MetricPointV1::counter("ipc_query_requests_dispatched_total", 9),
                MetricPointV1::gauge("ipc_query_connections_live", 2.0),
            ]))),
            Arc::new(FixedSource(Ok(vec![MetricPointV1::counter(
                "boot_lexical_sealed_generations",
                1,
            )]))),
        ];
        let dispatcher = scrape_dispatcher(Arc::clone(&store), sources)?;

        let first = scrape_via_control(&dispatcher).map_err(|error| error.code)?;
        let counters: Vec<(&str, u64)> = first
            .counters
            .iter()
            .map(|counter| (counter.name.as_str(), counter.value))
            .collect();
        assert_eq!(
            counters,
            vec![
                ("boot_lexical_sealed_generations", 1),
                ("ipc_query_requests_dispatched_total", 9),
                ("lq_query_intake_total", 7),
            ],
            "counters come from the store and both sources, sorted by name"
        );
        assert_eq!(
            first
                .gauges
                .iter()
                .map(|gauge| (gauge.name.as_str(), gauge.value))
                .collect::<Vec<_>>(),
            vec![("ipc_query_connections_live", 2.0)]
        );
        let histogram = first
            .histograms
            .iter()
            .find(|histogram| histogram.name == "lq_route_lexical_latency_ms")
            .ok_or("the route histogram is in the snapshot")?;
        assert_eq!((histogram.count, histogram.sum), (1, 4.0));
        assert_eq!(first.diagnostics.samples_recorded, 8);

        for _ in 0..5 {
            store.emit(MetricSample::new(
                "lq_query_intake_total",
                MetricKind::Counter,
                1.0,
                dimensions(),
            ));
        }
        let second = scrape_via_control(&dispatcher).map_err(|error| error.code)?;
        let intake = second
            .counters
            .iter()
            .find(|counter| counter.name == "lq_query_intake_total")
            .ok_or("intake counter present")?;
        assert_eq!(intake.value, 12, "a later scrape reads the larger total");
        Ok(())
    }

    /// A source that cannot read itself, names an invalid metric, or
    /// collides with another name fails the whole scrape typed; nothing is
    /// served with a hole in it (QI-BB-015).
    #[test]
    fn metrics_snapshot_route_refuses_a_defective_source_typed() -> TestResult {
        let failing: Vec<Arc<dyn MetricSourcePort>> =
            vec![Arc::new(FixedSource(Err("writer cache poisoned")))];
        let dispatcher = scrape_dispatcher(Arc::new(BoundedQueryObsStore::default()), failing)?;
        let error = scrape_via_control(&dispatcher)
            .err()
            .ok_or("a failing source refuses")?;
        assert_eq!(error.code, "INTERNAL");
        assert!(
            error.message.contains("writer cache poisoned"),
            "the source's own failure is the answer: {}",
            error.message
        );

        let bad_name: Vec<Arc<dyn MetricSourcePort>> =
            vec![Arc::new(FixedSource(Ok(vec![MetricPointV1::counter(
                "Ipc-Bad Name",
                1,
            )])))];
        let dispatcher = scrape_dispatcher(Arc::new(BoundedQueryObsStore::default()), bad_name)?;
        let error = scrape_via_control(&dispatcher)
            .err()
            .ok_or("a bad name refuses")?;
        assert_eq!(error.code, "METRICS_SOURCE_DEFECT");
        assert!(error.message.contains("Ipc-Bad Name"), "{}", error.message);

        let store = Arc::new(BoundedQueryObsStore::default());
        store.emit(MetricSample::new(
            "lq_query_intake_total",
            MetricKind::Counter,
            1.0,
            Dimensions::new("LXE-10", "8", "local", "repo-control", 3),
        ));
        let colliding: Vec<Arc<dyn MetricSourcePort>> =
            vec![Arc::new(FixedSource(Ok(vec![MetricPointV1::counter(
                "lq_query_intake_total",
                1,
            )])))];
        let dispatcher = scrape_dispatcher(store, colliding)?;
        let error = scrape_via_control(&dispatcher)
            .err()
            .ok_or("a collision refuses")?;
        assert_eq!(error.code, "METRICS_SOURCE_DEFECT");
        assert!(
            error.message.contains("more than one source"),
            "{}",
            error.message
        );
        Ok(())
    }

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
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)) => {
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
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)) => {
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
            guard.record_historically_sealed_search_corpus(
                &repo_id,
                &revision_id,
                ManifestGeneration::new(11),
                "manifest-digest-11",
            );
        }
        let dispatcher = SearchPlaneControlDispatcher::new(
            Arc::new(StubRepoMapActivatePort),
            activation_catalog.clone(),
            ledger,
            always_valid_generation(),
            always_valid_generation(),
            empty_scrape(),
        );

        let activate = into_repo_map_mutation_ack(dispatcher.dispatch(
            SearchPlaneControlIpcRequest::RepoMapActivate(RepoMapActivateGenerationRequest {
                repo_id: RepoId::new("repo-map-ipc"),
                revision_id: RevisionId::new("rev-map-ipc"),
                manifest_generation: ManifestGeneration::new(9),
                manifest_digest: "manifest-digest-9".to_string(),
            }),
            &RequestBudgetV1::unbounded(),
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
            &RequestBudgetV1::unbounded(),
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
            always_valid_generation(),
            always_valid_generation(),
            empty_scrape(),
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
            &RequestBudgetV1::unbounded(),
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
    fn rollback_generation_uses_explicit_composite_cas_and_preserves_activate_monotonicity()
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
                guard.record_historically_sealed_search_corpus(
                    &repo_id,
                    &revision_id,
                    ManifestGeneration::new(generation),
                    digest,
                );
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
            always_valid_generation(),
            always_valid_generation(),
            empty_scrape(),
        );

        let response = dispatcher.dispatch(
            SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(
                SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                    expected_active: composite_identity(
                        "repo-rollback",
                        "rev-rollback",
                        11,
                        "manifest-digest-11",
                    )?,
                    target: composite_identity(
                        "repo-rollback",
                        "rev-rollback",
                        10,
                        "manifest-digest-10",
                    )?,
                },
            ),
            &RequestBudgetV1::unbounded(),
        );
        let SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(ack) = response else {
            return Err("expected rollback ack".into());
        };
        if ack.previous_sealed_active.lexical.manifest_generation != ManifestGeneration::new(11)
            || ack.active.lexical.manifest_generation != ManifestGeneration::new(10)
            || ack.active.lexical.manifest_digest != "manifest-digest-10"
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

        let stale = dispatcher.dispatch(
            SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(
                SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                    expected_active: composite_identity(
                        "repo-rollback",
                        "rev-rollback",
                        11,
                        "manifest-digest-11",
                    )?,
                    target: composite_identity(
                        "repo-rollback",
                        "rev-rollback",
                        10,
                        "manifest-digest-10",
                    )?,
                },
            ),
            &RequestBudgetV1::unbounded(),
        );
        let SearchPlaneControlIpcResponse::Error(error) = stale else {
            return Err("stale rollback unexpectedly succeeded".into());
        };
        if error.code != super::ERR_ROLLBACK_CAS_CONFLICT {
            return Err(format!("unexpected stale rollback code: {}", error.code).into());
        }

        let unsealed_target = dispatcher.dispatch(
            SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(
                SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                    expected_active: composite_identity(
                        "repo-rollback",
                        "rev-rollback",
                        10,
                        "manifest-digest-10",
                    )?,
                    target: composite_identity(
                        "repo-rollback",
                        "rev-rollback",
                        9,
                        "manifest-digest-9",
                    )?,
                },
            ),
            &RequestBudgetV1::unbounded(),
        );
        let SearchPlaneControlIpcResponse::Error(error) = unsealed_target else {
            return Err("rollback to unsealed historical target unexpectedly succeeded".into());
        };
        if error.code != crate::readiness::ERR_SEARCH_TRACK_GENERATION_NOT_SEALED {
            return Err(format!("unexpected unsealed rollback code: {}", error.code).into());
        }
        Ok(())
    }
}
