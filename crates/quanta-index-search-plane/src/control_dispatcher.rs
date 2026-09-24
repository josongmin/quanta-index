//! Search-plane control orchestration.
//!
//! Control mutations are intentionally isolated from the read/query socket so
//! headless CLIs can remain view-only while admin or producer surfaces bind to
//! a separate control plane.

use std::sync::Arc;

use quanta_index_contract::{
    CurrentGenerationRequest, GenerationSnapshot, GenerationStatusReport, GenerationStatusRequest,
    MetricsSnapshotV1, RepoMapActivateGenerationRequestV2, RepoMapActiveHeadRequestV2,
    RepoMapActiveHeadResponseV2, RepoMapTerminalReceiptV2, SearchCorpusGenerationIdentityV1,
    SearchPlaneActivateSearchCorpusGenerationCasRequest, SearchPlaneControlIpcRequest,
    SearchPlaneControlIpcResponse, SearchPlaneIpcError,
    SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneSearchCorpusActivationCasAck,
    SearchPlaneSearchCorpusRollbackCasAck, TrackReadinessRecord,
};
use quanta_index_core::{CoreError, RepoMapGenerationActivatePort, RequestBudgetV1};

use crate::observability::ObservabilityScrape;
use crate::quarantine::QuarantineService;
use crate::search_corpus_lifecycle::{SearchCorpusLifecycleParts, SearchCorpusLifecycleService};
use crate::{
    ActivationCatalog, PreparedSearchCorpusGenerationV1, SearchCorpusGenerationActivationV1,
};

#[cfg(test)]
const ERR_ROLLBACK_CAS_CONFLICT: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::RollbackCasConflict;

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
    /// The live quarantine inventory and discard (QI-BB-026).
    quarantine: QuarantineService,
    /// The process-wide readiness authority, when one is wired.
    readiness: Option<Arc<dyn ProcessReadinessPort>>,
}

/// The ports one [`SearchPlaneControlDispatcher`] is composed from.
pub struct SearchPlaneControlDispatcherParts {
    pub repo_map_activate: Arc<dyn RepoMapGenerationActivatePort + Send + Sync>,
    pub lifecycle: SearchCorpusLifecycleParts,
    pub observability: Arc<ObservabilityScrape>,
    pub quarantine: QuarantineService,
    /// The process-wide readiness authority (S21-10). `None` means no
    /// authority is wired, and the readiness opcode refuses typed.
    pub readiness: Option<Arc<dyn ProcessReadinessPort>>,
}

/// The readiness authority the control plane asks for (S21-10).
///
/// The control plane never fabricates a verdict: with no port wired, the
/// request is refused typed instead of answered `ready`.
pub trait ProcessReadinessPort: Send + Sync {
    fn readiness(&self) -> Result<quanta_index_contract::ProcessReadinessV1, CoreError>;
}

/// The closed capability every control opcode requires (S21-10).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlCapabilityV1 {
    /// Read-only status, metrics and inventories.
    Observe,
    /// Durable mutations of the serving state.
    Admin,
}

impl ControlCapabilityV1 {
    /// The capability one request requires. Exhaustive over the closed
    /// request enum: a new opcode cannot compile without a decision.
    #[must_use]
    pub const fn required_for(request: &SearchPlaneControlIpcRequest) -> Self {
        match request {
            SearchPlaneControlIpcRequest::CurrentGeneration(_)
            | SearchPlaneControlIpcRequest::GenerationStatus(_)
            | SearchPlaneControlIpcRequest::MetricsSnapshot(_)
            | SearchPlaneControlIpcRequest::QuarantineInventory(_)
            | SearchPlaneControlIpcRequest::ProcessReadiness(_) => Self::Observe,
            SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(_)
            | SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(_)
            | SearchPlaneControlIpcRequest::SearchCorpusActiveHead(_)
            | SearchPlaneControlIpcRequest::RepoMapActivateV2(_)
            | SearchPlaneControlIpcRequest::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcRequest::QuarantineDiscard(_) => Self::Admin,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Admin => "admin",
        }
    }
}

/// The authorization context a control dispatch runs under (S21-10).
///
/// Two principals exist, and neither is inferred from the payload:
/// - `InProcessOperator` is the daemon calling its own dispatcher (the
///   composition root and in-process tests). It is constructed explicitly,
///   never selected by the absence of a credential.
/// - `Peer` carries the kernel-reported credentials of a connected peer
///   plus the uid the socket was bound as: a peer running as the socket
///   owner (or as root) is the operator, any other admitted peer is
///   observe-only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlAccessV1 {
    InProcessOperator,
    Peer { uid: u32, owner_uid: u32 },
}

impl ControlAccessV1 {
    /// Whether this access may exercise `capability`.
    #[must_use]
    pub const fn permits(self, capability: ControlCapabilityV1) -> bool {
        match (self, capability) {
            (Self::InProcessOperator, _) | (Self::Peer { .. }, ControlCapabilityV1::Observe) => {
                true
            }
            (Self::Peer { uid, owner_uid }, ControlCapabilityV1::Admin) => {
                uid == owner_uid || uid == 0
            }
        }
    }

    #[must_use]
    pub const fn principal_name(self) -> &'static str {
        match self {
            Self::InProcessOperator => "in-process-operator",
            Self::Peer { uid, owner_uid } if uid == owner_uid || uid == 0 => "peer-operator",
            Self::Peer { .. } => "peer-observer",
        }
    }
}

/// The typed authorization refusal, carrying provenance but never payload.
fn control_authorization_denied(
    access: ControlAccessV1,
    capability: ControlCapabilityV1,
) -> CoreError {
    CoreError::Typed {
        code: quanta_index_contract::SearchPlaneErrorCodeV2::ControlAuthorizationDenied,
        message: format!(
            "control: principal {} may not exercise the {} capability",
            access.principal_name(),
            capability.as_str()
        ),
    }
}

impl SearchPlaneControlDispatcher {
    #[must_use]
    pub fn new(parts: SearchPlaneControlDispatcherParts) -> Self {
        let SearchPlaneControlDispatcherParts {
            repo_map_activate,
            lifecycle,
            observability,
            quarantine,
            readiness,
        } = parts;
        let activation_catalog = Arc::clone(&lifecycle.activation_catalog);
        let search_corpus_lifecycle = SearchCorpusLifecycleService::new(lifecycle);
        Self {
            repo_map_activate,
            activation_catalog,
            search_corpus_lifecycle,
            observability,
            quarantine,
            readiness,
        }
    }

    fn process_readiness(&self) -> Result<quanta_index_contract::ProcessReadinessV1, CoreError> {
        let Some(port) = self.readiness.as_ref() else {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::ProcessNotReady,
                message: "control: no process-readiness authority is wired".to_string(),
            });
        };
        port.readiness()
    }

    fn repo_map_activate_v2(
        &self,
        request: &RepoMapActivateGenerationRequestV2,
    ) -> Result<RepoMapTerminalReceiptV2, CoreError> {
        self.repo_map_activate.activate_generation(request)
    }

    fn repo_map_active_head_v2(
        &self,
        request: &RepoMapActiveHeadRequestV2,
    ) -> Result<RepoMapActiveHeadResponseV2, CoreError> {
        self.repo_map_activate.active_head(request)
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
        let candidate = crate::SearchCorpusGenerationV1::from_contract_v1(&request.candidate)?;
        let prepared =
            PreparedSearchCorpusGenerationV1::new(candidate, request.expected_active.clone())?;
        let activation = self.activate_prepared_search_corpus_generation_v1(&prepared)?;
        Ok(SearchPlaneSearchCorpusActivationCasAck {
            active: activation.active,
            previous_sealed_active: activation.previous_active,
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
        let target = crate::SearchCorpusGenerationV1::from_contract_v1(&request.target)?;
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
        let active = self
            .activation_catalog
            .active_search_corpus_with_token_v1(&request.repo_id, &request.revision_id)?;
        let tracks = active
            .as_ref()
            .map_or_else(Vec::new, |(generation, _token)| {
                [generation.lexical(), generation.semantic()]
                    .into_iter()
                    .map(|snapshot| TrackReadinessRecord {
                        track: snapshot.track,
                        manifest_generation: snapshot.manifest_generation,
                        manifest_digest: snapshot.manifest_digest.clone(),
                    })
                    .collect()
            });
        let semantic_content =
            active.map(|(generation, _token)| generation.semantic_content().clone());
        Ok(GenerationStatusReport {
            repo_id: request.repo_id,
            revision_id: request.revision_id,
            tracks,
            semantic_content,
        })
    }

    /// Return the optional active head from one catalog read. An I/O or
    /// uncertain-durability failure remains an error, never an absent head.
    fn search_corpus_active_head(
        &self,
        request: GenerationStatusRequest,
    ) -> Result<quanta_index_contract::SearchCorpusActiveHeadObservationV1, CoreError> {
        let head = self
            .activation_catalog
            .active_search_corpus_with_token_v1(&request.repo_id, &request.revision_id)?
            .map(|(generation, activation_token)| {
                quanta_index_contract::SearchCorpusActiveHeadV1 {
                    generation: generation.to_contract_v1(),
                    activation_token,
                }
            });
        quanta_index_contract::SearchCorpusActiveHeadObservationV1::new(
            request.repo_id,
            request.revision_id,
            head,
        )
        .map_err(|error| CoreError::Storage(format!("invalid catalog active head: {error}")))
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
        // The daemon's own in-process entry: an explicitly constructed
        // operator context, never a default that a peer path inherits.
        self.dispatch_as(ControlAccessV1::InProcessOperator, request, budget)
    }

    /// Serve one control request under the transport's kernel-derived
    /// context (S21-10): the peer's credentials and the socket owner decide
    /// the principal, and the capability table decides the answer. A
    /// context without a peer credential is the in-process path only when
    /// the caller says so explicitly; here it is a typed refusal.
    #[must_use]
    pub fn dispatch_authorized(
        &self,
        context: &quanta_index_ipc::DispatchContextV1,
        request: SearchPlaneControlIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneControlIpcResponse {
        let access = match context.principal {
            Some(credentials) => ControlAccessV1::Peer {
                uid: credentials.uid,
                owner_uid: context.owner_uid,
            },
            None => {
                return SearchPlaneControlIpcResponse::Error(core_error_to_ipc(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::ControlAuthorizationDenied,
                    message: "control: request carried no kernel credential context".to_string(),
                }));
            }
        };
        self.dispatch_as(access, request, budget)
    }

    /// Serve one control request under an explicit access decision.
    #[must_use]
    pub fn dispatch_as(
        &self,
        access: ControlAccessV1,
        request: SearchPlaneControlIpcRequest,
        budget: &RequestBudgetV1,
    ) -> SearchPlaneControlIpcResponse {
        let capability = ControlCapabilityV1::required_for(&request);
        if !access.permits(capability) {
            return SearchPlaneControlIpcResponse::Error(core_error_to_ipc(
                control_authorization_denied(access, capability),
            ));
        }
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
            SearchPlaneControlIpcRequest::RepoMapActivateV2(request) => {
                match self.repo_map_activate_v2(&request) {
                    Ok(resp) => SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(resp),
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneControlIpcRequest::RepoMapActiveHeadV2(request) => {
                match self.repo_map_active_head_v2(&request) {
                    Ok(resp) => SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(resp),
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
            SearchPlaneControlIpcRequest::SearchCorpusActiveHead(request) => {
                match self.search_corpus_active_head(request) {
                    Ok(observation) => {
                        SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(
                            observation,
                        )
                    }
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneControlIpcRequest::MetricsSnapshot(_request) => {
                match self.metrics_snapshot() {
                    Ok(snapshot) => SearchPlaneControlIpcResponse::MetricsSnapshot(snapshot),
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneControlIpcRequest::QuarantineInventory(_request) => {
                match self.quarantine.inventory() {
                    Ok(inventory) => SearchPlaneControlIpcResponse::QuarantineInventory(inventory),
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneControlIpcRequest::QuarantineDiscard(request) => {
                match self.quarantine.discard(&request.target) {
                    Ok(ack) => SearchPlaneControlIpcResponse::QuarantineDiscardAck(ack),
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
            SearchPlaneControlIpcRequest::ProcessReadiness(_request) => {
                match self.process_readiness() {
                    Ok(report) => SearchPlaneControlIpcResponse::ProcessReadinessReport(report),
                    Err(err) => SearchPlaneControlIpcResponse::Error(core_error_to_ipc(err)),
                }
            }
        }
    }
}

fn core_error_to_ipc(err: CoreError) -> SearchPlaneIpcError {
    let (code, message) = err.into_search_plane_wire();
    // Control-plane failures carry no query-intent repair metadata (J7Q-06);
    // the wire field stays None.
    SearchPlaneIpcError {
        code,
        message,
        repair: None,
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::panic_in_result_fn,
        reason = "Result-returning control tests use assertions as test-failure reporting"
    )]
    use std::sync::{Arc, Mutex, RwLock};

    use super::{ControlAccessV1, ProcessReadinessPort, SearchPlaneControlDispatcher};
    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, MetricsSnapshotRequest, MetricsSnapshotV1,
        QuarantineDiscardOutcomeDtoV1, QuarantineDiscardRequest, QuarantineInventoryRequest,
        QuarantineTargetV1, RepoId, RepoMapActivateGenerationRequestV2, RepoMapActiveHeadRequestV2,
        RepoMapActiveHeadResponseV2, RepoMapMutationAck, RepoMapMutationPhaseV2,
        RepoMapTerminalReceiptV2, RevisionId, SearchCorpusGenerationIdentityV1,
        SearchPlaneActivateSearchCorpusGenerationCasRequest, SearchPlaneControlIpcRequest,
        SearchPlaneControlIpcResponse, SearchPlaneIpcError,
        SearchPlaneRollbackSearchCorpusGenerationCasRequest, SearchPlaneTrackKind,
    };
    use quanta_index_core::{
        CoreError, GenerationQuarantineReasonV1, MetricPointV1, MetricSourcePort,
        QUARANTINE_TARGET_NOT_QUARANTINED_CODE, RepoMapGenerationActivatePort, RequestBudgetV1,
    };
    use quanta_index_lq_obs::{Dimensions, MetricKind, MetricSample};
    use tempfile::tempdir;

    use quanta_index_core::{
        LexicalIndexOpenPort, LexicalSearcher, SemanticIndexOpenPort, SemanticSearcher,
    };

    use crate::content_roots_test_support::{generation_keyed_content_roots, roots_for_generation};
    use crate::door_findings_test_support::{RecordingDoorFindings, ScriptedFinding};
    use crate::ingest_dispatcher::SearchCorpusAuthorityInspectPort;
    use crate::observability::{BoundedQueryObsStore, ObservabilityScrape, QueryObsSink};
    use crate::quarantine::QuarantineService;
    use crate::query_dispatcher::tests::support::lexical::StubLexicalSearcher;
    use crate::query_dispatcher::tests::support::semantic::{
        RecordingSemanticOpener, RecordingSemanticState,
    };
    use crate::search_corpus_lifecycle::{ActivationPromotionParts, SearchCorpusLifecycleParts};
    use crate::{
        ActivationCatalog, Ledger, PreparedSearchCorpusGenerationV1,
        SealedSearchCorpusAuthorityStateV1, SearchCorpusGenerationV1,
        SearchPlaneControlDispatcherParts, SnapshotRegistries, SnapshotRegistryPolicy,
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
                repo_id: RepoId::new(repo_id).expect("test fixture ID satisfies canonical policy"),
                revision_id: RevisionId::new(revision_id)
                    .expect("test fixture ID satisfies canonical policy"),
                track: SearchPlaneTrackKind::Lexical,
                manifest_generation: ManifestGeneration::new(generation),
                manifest_digest: manifest_digest.to_string(),
            },
            GenerationSnapshot {
                repo_id: RepoId::new(repo_id).expect("test fixture ID satisfies canonical policy"),
                revision_id: RevisionId::new(revision_id)
                    .expect("test fixture ID satisfies canonical policy"),
                track: SearchPlaneTrackKind::Semantic,
                manifest_generation: ManifestGeneration::new(generation),
                manifest_digest: manifest_digest.to_string(),
            },
            roots_for_generation(generation),
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
            semantic_content: generation.semantic_content().clone(),
        })
    }

    struct StubRepoMapActivatePort;

    /// Openers whose generations are sealed under `manifest-digest-<generation>`.
    ///
    /// That is the convention every identity in these tests follows.
    /// `open_proven` refuses a candidate naming any other digest, as the
    /// adapters' proof does.
    struct EchoLexicalOpener;

    fn prove_echo_digest(candidate: &GenerationSnapshot) -> Result<(), CoreError> {
        let sealed = format!("manifest-digest-{}", candidate.manifest_generation.get());
        if candidate.manifest_digest == sealed {
            return Ok(());
        }
        Err(CoreError::Typed {
            code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
            message: format!(
                "{:?}: sealed under {sealed}, candidate names {}",
                candidate.track, candidate.manifest_digest
            ),
        })
    }

    impl LexicalIndexOpenPort for EchoLexicalOpener {
        fn open(
            &self,
            _repo: &RepoId,
            _revision: &RevisionId,
            generation: ManifestGeneration,
        ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
            Ok(Box::new(StubLexicalSearcher {
                results: Vec::new(),
                manifest_digest: Some(format!("manifest-digest-{}", generation.get())),
            }))
        }

        fn open_proven(
            &self,
            candidate: &GenerationSnapshot,
        ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
            prove_echo_digest(candidate)?;
            self.open(
                &candidate.repo_id,
                &candidate.revision_id,
                candidate.manifest_generation,
            )
        }
    }

    struct EchoSemanticOpener;

    impl SemanticIndexOpenPort for EchoSemanticOpener {
        fn open(
            &self,
            repo: &RepoId,
            revision: &RevisionId,
            generation: ManifestGeneration,
        ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
            let state = Arc::new(Mutex::new(RecordingSemanticState {
                manifest_digest: Some(format!("manifest-digest-{}", generation.get())),
                ..RecordingSemanticState::default()
            }));
            RecordingSemanticOpener { state }.open(repo, revision, generation)
        }

        fn open_proven(
            &self,
            candidate: &GenerationSnapshot,
        ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
            prove_echo_digest(candidate)?;
            self.open(
                &candidate.repo_id,
                &candidate.revision_id,
                candidate.manifest_generation,
            )
        }
    }

    /// A durable history whose answer is scripted.
    struct ScriptedAuthority(SealedSearchCorpusAuthorityStateV1);

    impl SearchCorpusAuthorityInspectPort for ScriptedAuthority {
        fn inspect_sealed_search_corpus(
            &self,
            _repo_id: &RepoId,
            _revision_id: &RevisionId,
            _generation: ManifestGeneration,
            _manifest_digest: &str,
        ) -> Result<SealedSearchCorpusAuthorityStateV1, CoreError> {
            Ok(self.0)
        }
    }

    fn exact_authority() -> Arc<dyn SearchCorpusAuthorityInspectPort + Send + Sync> {
        Arc::new(ScriptedAuthority(SealedSearchCorpusAuthorityStateV1::Exact))
    }

    /// The parts of a dispatcher over `activation_catalog` and `ledger`,
    /// with echo openers, a durable history that records everything, and
    /// fresh registries (handed back so a test can read them).
    fn control_parts(
        activation_catalog: Arc<ActivationCatalog>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> (SearchPlaneControlDispatcherParts, SnapshotRegistries) {
        let snapshots = SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT);
        let parts = SearchPlaneControlDispatcherParts {
            repo_map_activate: Arc::new(StubRepoMapActivatePort),
            lifecycle: SearchCorpusLifecycleParts {
                activation_catalog,
                ledger,
                authority: exact_authority(),
                promotion: ActivationPromotionParts {
                    lexical_open: Arc::new(EchoLexicalOpener),
                    semantic_open: Arc::new(EchoSemanticOpener),
                    semantic_content_roots: generation_keyed_content_roots(),
                    lexical_door_findings: Arc::new(RecordingDoorFindings::new(
                        ScriptedFinding::Quarantines,
                    )),
                    semantic_door_findings: Arc::new(RecordingDoorFindings::new(
                        ScriptedFinding::Quarantines,
                    )),
                    snapshots: snapshots.clone(),
                },
            },
            observability: empty_scrape(),
            quarantine: empty_quarantine(),
            readiness: None,
        };
        (parts, snapshots)
    }

    fn control_dispatcher(
        activation_catalog: Arc<ActivationCatalog>,
        ledger: Arc<RwLock<Ledger>>,
    ) -> SearchPlaneControlDispatcher {
        SearchPlaneControlDispatcher::new(control_parts(activation_catalog, ledger).0)
    }

    /// A dispatcher over the standard fixtures with a readiness authority.
    fn control_dispatcher_with_readiness(
        port: Arc<dyn ProcessReadinessPort>,
    ) -> SearchPlaneControlDispatcher {
        let dir = tempfile::tempdir().expect("tempdir");
        let activation_catalog =
            Arc::new(ActivationCatalog::open(dir.path()).expect("activation catalog"));
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        let (mut parts, _snapshots) = control_parts(activation_catalog, ledger);
        parts.readiness = Some(port);
        SearchPlaneControlDispatcher::new(parts)
    }

    struct FixedReadiness(Result<quanta_index_contract::ProcessReadinessV1, &'static str>);

    impl ProcessReadinessPort for FixedReadiness {
        fn readiness(&self) -> Result<quanta_index_contract::ProcessReadinessV1, CoreError> {
            match &self.0 {
                Ok(report) => Ok(report.clone()),
                Err(message) => Err(CoreError::Storage((*message).to_string())),
            }
        }
    }

    fn ready_report(active_repositories: u64) -> quanta_index_contract::ProcessReadinessV1 {
        quanta_index_contract::ProcessReadinessV1 {
            ready: true,
            supervisor_phase: quanta_index_contract::ProcessReadinessPhaseV1::Ready,
            components: quanta_index_contract::ProcessComponentsHealthV1 {
                query_plane: true,
                control_plane: true,
                ingest_plane: true,
                maintenance_heartbeat: true,
                required_backend: true,
                provider: quanta_index_contract::ProcessProviderReadinessV1 {
                    claim: quanta_index_contract::ProcessProviderClaimV1::Disabled,
                    healthy: true,
                },
            },
            active_candidate_integrity: None,
            active_repositories,
            not_ready_reasons: Vec::new(),
        }
    }

    fn readiness_request() -> SearchPlaneControlIpcRequest {
        SearchPlaneControlIpcRequest::ProcessReadiness(
            quanta_index_contract::ProcessReadinessRequest,
        )
    }

    fn admin_request() -> SearchPlaneControlIpcRequest {
        SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
            SearchPlaneActivateSearchCorpusGenerationCasRequest {
                candidate: composite_identity("repo-auth", "rev-auth", 1, "digest-auth-1")
                    .expect("fixture identity"),
                expected_active: None,
            },
        )
    }

    fn other_control_responses(other: &SearchPlaneControlIpcResponse) -> ! {
        match other {
            SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)
            | SearchPlaneControlIpcResponse::Error(_) => {
                panic!("expected an error response, got {other:?}")
            }
        }
    }

    fn error_code(
        response: SearchPlaneControlIpcResponse,
    ) -> quanta_index_contract::SearchPlaneErrorCodeV2 {
        match response {
            SearchPlaneControlIpcResponse::Error(error) => error.code,
            ref other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => {
                other_control_responses(other)
            }
        }
    }

    #[test]
    fn a_peer_observer_is_denied_admin_and_mutates_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path()).expect("catalog"));
        let dispatcher = control_dispatcher(
            Arc::clone(&activation_catalog),
            Arc::new(RwLock::new(Ledger::new())),
        );
        let observer = ControlAccessV1::Peer {
            uid: 2000,
            owner_uid: 1000,
        };
        let response =
            dispatcher.dispatch_as(observer, admin_request(), &RequestBudgetV1::unbounded());
        assert_eq!(
            error_code(response),
            quanta_index_contract::SearchPlaneErrorCodeV2::ControlAuthorizationDenied
        );
        // Default deny means zero mutation: the catalog still resolves no
        // active search-corpus generation for the fixture pair.
        let active = activation_catalog
            .active_search_corpus_v1(
                &RepoId::new("repo-auth").expect("fixture"),
                &RevisionId::new("rev-auth").expect("fixture"),
            )
            .expect("read active state");
        assert!(active.is_none(), "a denied mutation must commit nothing");
    }

    #[test]
    fn a_peer_without_a_kernel_credential_context_is_denied_even_observe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dispatcher = control_dispatcher(
            Arc::new(ActivationCatalog::open(dir.path()).expect("catalog")),
            Arc::new(RwLock::new(Ledger::new())),
        );
        let context = quanta_index_ipc::DispatchContextV1 {
            request_id: std::num::NonZeroU64::MIN,
            plane: quanta_index_ipc::IpcPlane::Control,
            principal: None,
            owner_uid: 1000,
            connection_id: 1,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(1),
            cancellation: RequestBudgetV1::unbounded().cancel_handle(),
            events: Arc::new(quanta_index_ipc::IpcServerCounters::for_plane("control")),
            request_started: std::time::Instant::now(),
        };
        let response = dispatcher.dispatch_authorized(
            &context,
            readiness_request(),
            &RequestBudgetV1::unbounded(),
        );
        assert_eq!(
            error_code(response),
            quanta_index_contract::SearchPlaneErrorCodeV2::ControlAuthorizationDenied
        );
    }

    #[test]
    fn the_socket_owner_peer_is_admitted_for_admin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dispatcher = control_dispatcher(
            Arc::new(ActivationCatalog::open(dir.path()).expect("catalog")),
            Arc::new(RwLock::new(Ledger::new())),
        );
        let context = quanta_index_ipc::DispatchContextV1 {
            request_id: std::num::NonZeroU64::MIN,
            plane: quanta_index_ipc::IpcPlane::Control,
            principal: Some(quanta_index_ipc::PeerCredentials {
                uid: 1000,
                gid: 1000,
                pid: None,
            }),
            owner_uid: 1000,
            connection_id: 1,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(1),
            cancellation: RequestBudgetV1::unbounded().cancel_handle(),
            events: Arc::new(quanta_index_ipc::IpcServerCounters::for_plane("control")),
            request_started: std::time::Instant::now(),
        };
        // The operator reaches the domain layer: the activation is attempted
        // and answered by the domain, not by the authorization gate.
        let response = dispatcher.dispatch_authorized(
            &context,
            admin_request(),
            &RequestBudgetV1::unbounded(),
        );
        assert_ne!(
            error_code(response),
            quanta_index_contract::SearchPlaneErrorCodeV2::ControlAuthorizationDenied
        );
    }

    #[test]
    fn readiness_without_an_authority_refuses_typed_never_fabricates_ready() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dispatcher = control_dispatcher(
            Arc::new(ActivationCatalog::open(dir.path()).expect("catalog")),
            Arc::new(RwLock::new(Ledger::new())),
        );
        let response = dispatcher.dispatch(readiness_request(), &RequestBudgetV1::unbounded());
        assert_eq!(
            error_code(response),
            quanta_index_contract::SearchPlaneErrorCodeV2::ProcessNotReady
        );
    }

    #[test]
    fn readiness_reports_pass_through_and_errors_stay_typed() {
        let dispatcher =
            control_dispatcher_with_readiness(Arc::new(FixedReadiness(Ok(ready_report(0)))));
        match dispatcher.dispatch(readiness_request(), &RequestBudgetV1::unbounded()) {
            SearchPlaneControlIpcResponse::ProcessReadinessReport(report) => {
                assert!(report.ready);
                assert_eq!(report.active_repositories, 0);
                assert!(report.not_ready_reasons.is_empty());
            }
            ref other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::Error(_)) => other_control_responses(other),
        }
        let failing =
            control_dispatcher_with_readiness(Arc::new(FixedReadiness(Err("backend down"))));
        match failing.dispatch(readiness_request(), &RequestBudgetV1::unbounded()) {
            SearchPlaneControlIpcResponse::Error(error) => {
                assert_eq!(
                    error.code,
                    quanta_index_contract::SearchPlaneErrorCodeV2::Internal
                );
            }
            ref other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => {
                other_control_responses(other)
            }
        }
    }

    /// A quarantine service whose adapters quarantine nothing.
    fn empty_quarantine() -> QuarantineService {
        crate::quarantine::tests::service(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Arc::new(RwLock::new(Ledger::new())),
        )
        .service
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
        let (mut parts, _snapshots) =
            control_parts(activation_catalog, Arc::new(RwLock::new(Ledger::new())));
        parts.observability = Arc::new(ObservabilityScrape::new(store, sources));
        Ok(SearchPlaneControlDispatcher::new(parts))
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
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => {
                Err(SearchPlaneIpcError {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::Internal,
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

        let first = scrape_via_control(&dispatcher).map_err(|error| error.code.to_string())?;
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
        let second = scrape_via_control(&dispatcher).map_err(|error| error.code.to_string())?;
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
        assert_eq!(
            error.code,
            quanta_index_contract::SearchPlaneErrorCodeV2::Internal
        );
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
        assert_eq!(
            error.code,
            quanta_index_contract::SearchPlaneErrorCodeV2::MetricsSourceDefect
        );
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
        assert_eq!(
            error.code,
            quanta_index_contract::SearchPlaneErrorCodeV2::MetricsSourceDefect
        );
        assert!(
            error.message.contains("more than one source"),
            "{}",
            error.message
        );
        Ok(())
    }

    impl RepoMapGenerationActivatePort for StubRepoMapActivatePort {
        fn active_head(
            &self,
            request: &RepoMapActiveHeadRequestV2,
        ) -> Result<RepoMapActiveHeadResponseV2, CoreError> {
            Ok(RepoMapActiveHeadResponseV2 {
                repo_id: request.repo_id.clone(),
                revision_id: request.revision_id.clone(),
                active: None,
            })
        }

        fn activate_generation(
            &self,
            request: &RepoMapActivateGenerationRequestV2,
        ) -> Result<RepoMapTerminalReceiptV2, CoreError> {
            if request.manifest_digest.is_empty() {
                return Err(CoreError::InvalidContract(
                    "repo-map activate: manifest_digest must not be empty".to_string(),
                ));
            }
            Ok(RepoMapTerminalReceiptV2 {
                phase: RepoMapMutationPhaseV2::Activate,
                mutation: RepoMapMutationAck {
                    repo_id: request.repo_id.clone(),
                    revision_id: request.revision_id.clone(),
                    manifest_generation: request.manifest_generation,
                    prior_candidate_commitment: None,
                    new_candidate_commitment: "sha256:".to_string(),
                    activation_epoch: 1,
                    terminal_sequence: 1,
                    replayed: false,
                },
                manifest_digest: request.manifest_digest.clone(),
                snapshot_id: request.snapshot_id.clone(),
                projection_version: request.projection_version,
                authority_digest: request.authority_digest.clone(),
                source_bundle_digest: request.source_bundle_digest.clone(),
            })
        }
    }

    fn into_repo_map_mutation_ack(
        response: SearchPlaneControlIpcResponse,
    ) -> Result<RepoMapMutationAck, Box<dyn std::error::Error>> {
        match response {
            SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(receipt) => {
                Ok(receipt.mutation)
            }
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::Error(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)) => {
                Err(format!("expected repo-map mutation ack, got {other:?}").into())
            }
        }
    }

    fn into_error_code(
        response: SearchPlaneControlIpcResponse,
    ) -> Result<quanta_index_contract::SearchPlaneErrorCodeV2, Box<dyn std::error::Error>> {
        match response {
            SearchPlaneControlIpcResponse::Error(err) => Ok(err.code),
            other @ (SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(_)
            | SearchPlaneControlIpcResponse::RepoMapTerminalReceiptV2(_)
            | SearchPlaneControlIpcResponse::RepoMapActiveHeadV2(_)
            | SearchPlaneControlIpcResponse::SearchCorpusRollbackCasAck(_)
            | SearchPlaneControlIpcResponse::CurrentGenerationSnapshot(_)
            | SearchPlaneControlIpcResponse::GenerationStatusReport(_)
            | SearchPlaneControlIpcResponse::SearchCorpusActiveHeadObservation(_)
            | SearchPlaneControlIpcResponse::MetricsSnapshot(_)
            | SearchPlaneControlIpcResponse::QuarantineInventory(_)
            | SearchPlaneControlIpcResponse::QuarantineDiscardAck(_)
            | SearchPlaneControlIpcResponse::ProcessReadinessReport(_)) => {
                Err(format!("expected error response, got {other:?}").into())
            }
        }
    }

    #[test]
    fn repo_map_control_and_prepared_corpus_activation_preserve_composite_identity() -> TestResult {
        // RepoMap activation is a control mutation; publication is ingest-only.
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let ledger = Arc::new(RwLock::new(Ledger::new()));
        {
            let mut guard = ledger
                .write()
                .map_err(|err| format!("ledger poisoned: {err}"))?;
            let repo_id =
                RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy");
            let revision_id = RevisionId::new("rev-map-ipc")
                .expect("static fixture ID satisfies canonical policy");
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
        let dispatcher = control_dispatcher(activation_catalog.clone(), ledger);

        let activate = into_repo_map_mutation_ack(
            dispatcher.dispatch(
                SearchPlaneControlIpcRequest::RepoMapActivateV2(
                    RepoMapActivateGenerationRequestV2 {
                        repo_id: RepoId::new("repo-map-ipc")
                            .expect("static fixture ID satisfies canonical policy"),
                        revision_id: RevisionId::new("rev-map-ipc")
                            .expect("static fixture ID satisfies canonical policy"),
                        manifest_generation: ManifestGeneration::new(9),
                        manifest_digest: "manifest-digest-9".to_string(),
                        snapshot_id: "snapshot-9".to_string(),
                        projection_version: 1,
                        authority_digest: "sha256:".to_string() + &"a".repeat(64),
                        source_bundle_digest: "sha256:".to_string() + &"b".repeat(64),
                        expected_active: None,
                    },
                ),
                &RequestBudgetV1::unbounded(),
            ),
        )?;
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
                            repo_id: RepoId::new("repo-map-ipc")
                                .expect("static fixture ID satisfies canonical policy"),
                            revision_id: RevisionId::new("rev-map-ipc")
                                .expect("static fixture ID satisfies canonical policy"),
                            track: SearchPlaneTrackKind::Lexical,
                            manifest_generation: ManifestGeneration::new(11),
                            manifest_digest: "manifest-digest-11".to_string(),
                        },
                        semantic: GenerationSnapshot {
                            repo_id: RepoId::new("repo-map-ipc")
                                .expect("static fixture ID satisfies canonical policy"),
                            revision_id: RevisionId::new("rev-map-ipc")
                                .expect("static fixture ID satisfies canonical policy"),
                            track: SearchPlaneTrackKind::Semantic,
                            manifest_generation: ManifestGeneration::new(11),
                            manifest_digest: "manifest-digest-11".to_string(),
                        },
                        semantic_content: roots_for_generation(11),
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
        if activation
            .active
            .generation
            .lexical
            .manifest_generation
            .get()
            != 11
        {
            return Err(format!(
                "unexpected activation manifest generation: {}",
                activation
                    .active
                    .generation
                    .lexical
                    .manifest_generation
                    .get()
            )
            .into());
        }
        let lexical_pin = activation_catalog.resolve(
            &RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
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
            &RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
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
        let dispatcher = control_dispatcher(
            Arc::clone(&activation_catalog),
            Arc::new(RwLock::new(Ledger::new())),
        );
        let code = into_error_code(
            dispatcher.dispatch(
                SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
                    SearchPlaneActivateSearchCorpusGenerationCasRequest {
                        candidate: SearchCorpusGenerationIdentityV1 {
                            lexical: GenerationSnapshot {
                                repo_id: RepoId::new("repo-invalid")
                                    .expect("static fixture ID satisfies canonical policy"),
                                revision_id: RevisionId::new("rev-invalid")
                                    .expect("static fixture ID satisfies canonical policy"),
                                track: SearchPlaneTrackKind::Semantic,
                                manifest_generation: ManifestGeneration::new(11),
                                manifest_digest: "manifest-digest-11".to_string(),
                            },
                            semantic: GenerationSnapshot {
                                repo_id: RepoId::new("repo-invalid")
                                    .expect("static fixture ID satisfies canonical policy"),
                                revision_id: RevisionId::new("rev-invalid")
                                    .expect("static fixture ID satisfies canonical policy"),
                                track: SearchPlaneTrackKind::Semantic,
                                manifest_generation: ManifestGeneration::new(11),
                                manifest_digest: "manifest-digest-11".to_string(),
                            },
                            semantic_content: roots_for_generation(11),
                        },
                        expected_active: None,
                    },
                ),
                &RequestBudgetV1::unbounded(),
            ),
        )?;
        assert_eq!(
            code,
            quanta_index_contract::SearchPlaneErrorCodeV2::InvalidRequest
        );
        assert!(
            activation_catalog
                .resolve_record(
                    &RepoId::new("repo-invalid")
                        .expect("static fixture ID satisfies canonical policy"),
                    &RevisionId::new("rev-invalid")
                        .expect("static fixture ID satisfies canonical policy"),
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
            let repo_id =
                RepoId::new("repo-rollback").expect("static fixture ID satisfies canonical policy");
            let revision_id = RevisionId::new("rev-rollback")
                .expect("static fixture ID satisfies canonical policy");
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
        if activation.active.generation.lexical.manifest_generation != ManifestGeneration::new(11) {
            return Err("expected initial composite activation at generation 11".into());
        }
        let dispatcher = control_dispatcher(Arc::clone(&activation_catalog), ledger);

        let response = dispatcher.dispatch(
            SearchPlaneControlIpcRequest::RollbackSearchCorpusGenerationCas(
                SearchPlaneRollbackSearchCorpusGenerationCasRequest {
                    expected_active: activation.active.clone(),
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
        if ack
            .previous_sealed_active
            .generation
            .lexical
            .manifest_generation
            != ManifestGeneration::new(11)
            || ack.active.generation.lexical.manifest_generation != ManifestGeneration::new(10)
            || ack.active.generation.lexical.manifest_digest != "manifest-digest-10"
        {
            return Err(format!("unexpected rollback ack: {ack:?}").into());
        }
        let current = activation_catalog.resolve_record(
            &RepoId::new("repo-rollback").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev-rollback").expect("static fixture ID satisfies canonical policy"),
            SearchPlaneTrackKind::Semantic,
        )?;
        if current.manifest_generation != ManifestGeneration::new(10)
            || current.manifest_digest != "manifest-digest-10"
        {
            return Err(format!("rollback did not update active state: {current:?}").into());
        }
        let lexical = activation_catalog.resolve_record(
            &RepoId::new("repo-rollback").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev-rollback").expect("static fixture ID satisfies canonical policy"),
            SearchPlaneTrackKind::Lexical,
        )?;
        if lexical.manifest_generation != ManifestGeneration::new(10)
            || lexical.manifest_digest != "manifest-digest-10"
        {
            return Err(format!("rollback did not update lexical state: {lexical:?}").into());
        }
        let reopened = ActivationCatalog::open(dir.path())?;
        let reopened_lexical = reopened.resolve_record(
            &RepoId::new("repo-rollback").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev-rollback").expect("static fixture ID satisfies canonical policy"),
            SearchPlaneTrackKind::Lexical,
        )?;
        let reopened_semantic = reopened.resolve_record(
            &RepoId::new("repo-rollback").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev-rollback").expect("static fixture ID satisfies canonical policy"),
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
                    expected_active: activation.active,
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
                    expected_active: ack.active,
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
        if error.code
            != quanta_index_contract::SearchPlaneErrorCodeV2::SearchTrackGenerationNotSealed
        {
            return Err(format!("unexpected unsealed rollback code: {}", error.code).into());
        }
        Ok(())
    }

    /// A ledger with `generations` of the fixture pair sealed on both
    /// tracks and recorded in the durable history, under
    /// `manifest-digest-<generation>`.
    fn sealed_ledger(generations: &[u64]) -> Arc<RwLock<Ledger>> {
        let mut ledger = Ledger::new();
        let repo_id =
            RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy");
        let revision_id =
            RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy");
        for generation in generations {
            let digest = format!("manifest-digest-{generation}");
            for track in [
                SearchPlaneTrackKind::Lexical,
                SearchPlaneTrackKind::Semantic,
            ] {
                ledger.record_track_materialized(
                    &repo_id,
                    &revision_id,
                    track,
                    ManifestGeneration::new(*generation),
                    Some(&digest),
                );
                ledger.record_track_seal_with_digest(
                    &repo_id,
                    &revision_id,
                    track,
                    ManifestGeneration::new(*generation),
                    &digest,
                );
            }
            ledger.record_historically_sealed_search_corpus(
                &repo_id,
                &revision_id,
                ManifestGeneration::new(*generation),
                &digest,
            );
        }
        Arc::new(RwLock::new(ledger))
    }

    fn activate_request(generation: u64) -> SearchPlaneControlIpcRequest {
        SearchPlaneControlIpcRequest::ActivateSearchCorpusGenerationCas(
            SearchPlaneActivateSearchCorpusGenerationCasRequest {
                candidate: composite_identity(
                    "repo-map-ipc",
                    "rev-map-ipc",
                    generation,
                    &format!("manifest-digest-{generation}"),
                )
                .expect("fixture identity is well formed"),
                expected_active: None,
            },
        )
    }

    fn fixture_key(generation: u64) -> crate::SnapshotKey {
        crate::SnapshotKey::new(
            &RepoId::new("repo-map-ipc").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("rev-map-ipc").expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(generation),
        )
    }

    /// Openers that need the ledger's write lock while they prove.
    ///
    /// The way an ingest of another repo, or a retention receipt, does.
    /// They succeed only if activation runs the physical proof outside its
    /// ledger read guard (QI-BB-020 보완 #2).
    ///
    /// They count their proofs, so a test can hold activation to one proof
    /// per track: the proof is the open whose handle is promoted.
    struct LedgerWritingOpener<O> {
        ledger: Arc<RwLock<Ledger>>,
        inner: O,
        proofs: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl<O> LedgerWritingOpener<O> {
        fn prove_outside_the_guard(&self, candidate: &GenerationSnapshot) -> Result<(), CoreError> {
            let _previous = self
                .proofs
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match self.ledger.try_write() {
                Ok(_guard) => Ok(()),
                Err(std::sync::TryLockError::WouldBlock) => Err(CoreError::Storage(format!(
                    "the physical proof of {:?} ran under the ledger read guard; every other repo's ingest would stall behind this open",
                    candidate.track
                ))),
                Err(std::sync::TryLockError::Poisoned(err)) => {
                    Err(CoreError::Storage(format!("ledger poisoned: {err}")))
                }
            }
        }
    }

    impl LexicalIndexOpenPort for LedgerWritingOpener<EchoLexicalOpener> {
        fn open(
            &self,
            repo: &RepoId,
            revision: &RevisionId,
            generation: ManifestGeneration,
        ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
            self.inner.open(repo, revision, generation)
        }

        fn open_proven(
            &self,
            candidate: &GenerationSnapshot,
        ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
            self.prove_outside_the_guard(candidate)?;
            self.inner.open_proven(candidate)
        }
    }

    impl SemanticIndexOpenPort for LedgerWritingOpener<EchoSemanticOpener> {
        fn open(
            &self,
            repo: &RepoId,
            revision: &RevisionId,
            generation: ManifestGeneration,
        ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
            self.inner.open(repo, revision, generation)
        }

        fn open_proven(
            &self,
            candidate: &GenerationSnapshot,
        ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
            self.prove_outside_the_guard(candidate)?;
            self.inner.open_proven(candidate)
        }
    }

    /// Activation proves the pair once, outside the ledger guard.
    ///
    /// It promotes the handles the proof opened: the first acquire of the
    /// activated generation on either track is a registry hit, with no
    /// opener run.
    #[test]
    fn activation_proves_outside_the_ledger_guard_and_promotes_the_handles() -> TestResult {
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let ledger = sealed_ledger(&[11]);
        let (mut parts, snapshots) = control_parts(activation_catalog, Arc::clone(&ledger));
        let lexical_proofs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let semantic_proofs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        parts.lifecycle.promotion.lexical_open = Arc::new(LedgerWritingOpener {
            ledger: Arc::clone(&ledger),
            inner: EchoLexicalOpener,
            proofs: Arc::clone(&lexical_proofs),
        });
        parts.lifecycle.promotion.semantic_open = Arc::new(LedgerWritingOpener {
            ledger: Arc::clone(&ledger),
            inner: EchoSemanticOpener,
            proofs: Arc::clone(&semantic_proofs),
        });
        let dispatcher = SearchPlaneControlDispatcher::new(parts);

        let SearchPlaneControlIpcResponse::SearchCorpusActivationCasAck(ack) =
            dispatcher.dispatch(activate_request(11), &RequestBudgetV1::unbounded())
        else {
            return Err("activation must succeed with the proof outside the guard".into());
        };
        assert_eq!(
            ack.active.generation.lexical.manifest_generation,
            ManifestGeneration::new(11)
        );

        let key = fixture_key(11);
        let lexical = snapshots
            .lexical
            .acquire(&key, &RequestBudgetV1::unbounded(), || {
                Err(CoreError::Storage(
                    "the lexical handle must already be resident".into(),
                ))
            })?;
        assert_eq!(
            lexical.handle.artifact_identity().manifest_digest,
            "manifest-digest-11"
        );
        let semantic = snapshots
            .semantic
            .acquire(&key, &RequestBudgetV1::unbounded(), || {
                Err(CoreError::Storage(
                    "the semantic handle must already be resident".into(),
                ))
            })?;
        assert_eq!(semantic.handle.manifest_digest(), "manifest-digest-11");
        for registry_stats in [snapshots.lexical.stats()?, snapshots.semantic.stats()?] {
            assert_eq!(registry_stats.promotions, 1, "{registry_stats:?}");
            assert_eq!(registry_stats.misses, 0, "{registry_stats:?}");
            assert_eq!(registry_stats.hits, 1, "{registry_stats:?}");
        }
        // One proof per track, and it is the open that produced the
        // promoted handle: nothing proves the generation a second time.
        assert_eq!(lexical_proofs.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(semantic_proofs.load(std::sync::atomic::Ordering::SeqCst), 1);
        Ok(())
    }

    /// A candidate the durable history no longer records when the CAS is
    /// about to commit — reaped by a concurrent seal after the in-memory
    /// preconditions passed — is refused typed and activates nothing.
    #[test]
    fn activation_refuses_a_candidate_the_durable_history_reaped_before_the_cas() -> TestResult {
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let (mut parts, _snapshots) =
            control_parts(Arc::clone(&activation_catalog), sealed_ledger(&[11]));
        parts.lifecycle.authority = Arc::new(ScriptedAuthority(
            SealedSearchCorpusAuthorityStateV1::Absent,
        ));
        let dispatcher = SearchPlaneControlDispatcher::new(parts);

        let code = into_error_code(
            dispatcher.dispatch(activate_request(11), &RequestBudgetV1::unbounded()),
        )?;
        assert_eq!(
            code,
            quanta_index_contract::SearchPlaneErrorCodeV2::SearchTrackGenerationNotSealed
        );
        assert!(
            activation_catalog
                .resolve_record(
                    &RepoId::new("repo-map-ipc")
                        .expect("static fixture ID satisfies canonical policy"),
                    &RevisionId::new("rev-map-ipc")
                        .expect("static fixture ID satisfies canonical policy"),
                    SearchPlaneTrackKind::Lexical,
                )
                .is_err(),
            "nothing was activated"
        );
        Ok(())
    }

    /// A track whose generation on disk is sealed under another digest
    /// fails the proof: the activation is refused typed, nothing is
    /// promoted on either track, and nothing is activated.
    #[test]
    fn activation_refuses_a_pair_whose_proof_names_another_digest() -> TestResult {
        struct ForeignDigestOpener;

        impl LexicalIndexOpenPort for ForeignDigestOpener {
            fn open(
                &self,
                _repo: &RepoId,
                _revision: &RevisionId,
                _generation: ManifestGeneration,
            ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
                Ok(Box::new(StubLexicalSearcher {
                    results: Vec::new(),
                    manifest_digest: Some("manifest-digest-foreign".to_string()),
                }))
            }

            fn open_proven(
                &self,
                candidate: &GenerationSnapshot,
            ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
                Err(CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationIdentityDigestMismatch,
                    message: format!(
                        "sealed under manifest-digest-foreign, candidate names {}",
                        candidate.manifest_digest
                    ),
                })
            }
        }

        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let (mut parts, snapshots) =
            control_parts(Arc::clone(&activation_catalog), sealed_ledger(&[11]));
        parts.lifecycle.promotion.lexical_open = Arc::new(ForeignDigestOpener);
        let dispatcher = SearchPlaneControlDispatcher::new(parts);

        let code = into_error_code(
            dispatcher.dispatch(activate_request(11), &RequestBudgetV1::unbounded()),
        )?;
        assert_eq!(
            code,
            crate::search_corpus_lifecycle::ERR_ACTIVATION_TARGET_UNOPENABLE
        );
        assert_eq!(
            snapshots.lexical.stats()?.entries,
            0,
            "nothing was promoted"
        );
        assert_eq!(
            snapshots.semantic.stats()?.entries,
            0,
            "nothing was promoted"
        );
        assert!(
            activation_catalog
                .resolve_record(
                    &RepoId::new("repo-map-ipc")
                        .expect("static fixture ID satisfies canonical policy"),
                    &RevisionId::new("rev-map-ipc")
                        .expect("static fixture ID satisfies canonical policy"),
                    SearchPlaneTrackKind::Lexical,
                )
                .is_err(),
            "nothing was activated"
        );
        Ok(())
    }

    /// The quarantine routes (QI-BB-026) answer with the listing, the ack
    /// and the typed refusal.
    ///
    /// The inventory request answers with the adapters' live listing, a
    /// discard of a listed entry answers with an ack naming that entry,
    /// and a discard the adapter refuses crosses the control surface as
    /// the typed error, not as a panic or an empty ack.
    #[test]
    fn quarantine_routes_answer_with_the_listing_the_ack_and_the_typed_refusal() -> TestResult {
        let dir = tempdir()?;
        let activation_catalog = Arc::new(ActivationCatalog::open(dir.path())?);
        let listed = crate::quarantine::tests::quarantined(
            SearchPlaneTrackKind::Lexical,
            "/state/indexes/lexical/repo/rev/g3",
        );
        let doubles = crate::quarantine::tests::service(
            vec![listed.clone()],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Arc::new(RwLock::new(Ledger::new())),
        );
        let quarantine = doubles.service;
        let lexical_discard = doubles.lexical_discard;
        let (mut parts, _snapshots) =
            control_parts(activation_catalog, Arc::new(RwLock::new(Ledger::new())));
        parts.quarantine = quarantine;
        let dispatcher = SearchPlaneControlDispatcher::new(parts);

        let SearchPlaneControlIpcResponse::QuarantineInventory(inventory) = dispatcher.dispatch(
            SearchPlaneControlIpcRequest::QuarantineInventory(QuarantineInventoryRequest),
            &RequestBudgetV1::unbounded(),
        ) else {
            return Err("the inventory request answers with the inventory".into());
        };
        let Some(entry) = inventory.lexical.first().cloned() else {
            return Err("the lexical listing carries the quarantined generation".into());
        };
        if inventory.lexical.len() != 1
            || !inventory.semantic.is_empty()
            || !inventory.repo_map.is_empty()
        {
            return Err(format!("the listing is exactly the adapters' own: {inventory:?}").into());
        }
        if entry.path != listed.path.display().to_string()
            || entry.reason != listed.reason.as_code_str()
        {
            return Err(format!("the entry is the adapter's, verbatim: {entry:?}").into());
        }

        let mut stale = entry.clone();
        stale.reason = GenerationQuarantineReasonV1::ScopeMismatch
            .as_code_str()
            .to_string();
        let SearchPlaneControlIpcResponse::Error(error) = dispatcher.dispatch(
            SearchPlaneControlIpcRequest::QuarantineDiscard(QuarantineDiscardRequest {
                target: QuarantineTargetV1::Generation(stale),
            }),
            &RequestBudgetV1::unbounded(),
        ) else {
            return Err("a stale target is refused, not acked".into());
        };
        if error.code != QUARANTINE_TARGET_NOT_QUARANTINED_CODE {
            return Err(format!("the refusal is typed: {}", error.code).into());
        }
        if !lexical_discard
            .discarded
            .lock()
            .map_err(|err| err.to_string())?
            .is_empty()
        {
            return Err("a refused discard removes nothing".into());
        }

        let target = QuarantineTargetV1::Generation(entry);
        let SearchPlaneControlIpcResponse::QuarantineDiscardAck(ack) = dispatcher.dispatch(
            SearchPlaneControlIpcRequest::QuarantineDiscard(QuarantineDiscardRequest {
                target: target.clone(),
            }),
            &RequestBudgetV1::unbounded(),
        ) else {
            return Err("a listed target is discarded and acked".into());
        };
        if ack.target != target {
            return Err(format!("the ack names the target as sent: {ack:?}").into());
        }
        if ack.outcome != (QuarantineDiscardOutcomeDtoV1::Discarded { bytes: 42 }) {
            return Err(format!("the ack carries the adapter's outcome: {:?}", ack.outcome).into());
        }
        let discarded = lexical_discard
            .discarded
            .lock()
            .map_err(|err| err.to_string())?
            .clone();
        if discarded != vec![listed] {
            return Err(format!("exactly the listed entry was discarded: {discarded:?}").into());
        }
        Ok(())
    }
}
