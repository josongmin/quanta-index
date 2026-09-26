//! Canonical search-corpus lifecycle mutation authority.
//!
//! Physical publication may span a wider ingest operation, but every durable
//! history-retention and composite activation mutation for one repo/revision
//! pair is serialized here, under the pair's stripe of the
//! [`SearchCorpusPairMutationCoordinator`] (`pair_lock`, the leaf the stores
//! check guards against).  Lower storage owners must not introduce another
//! pair-local mutation lock.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use quanta_index_contract::{
    GenerationSnapshot, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    SearchPlaneSearchCorpusRollbackCasAck, SearchPlaneTrackKind, SourcePublicationEvent,
};
use quanta_index_core::{
    CoreError, DoorFindingOutcome, DoorFindingQuarantinePort, GENERATION_SIDECAR_CORRUPT_CODE,
    IdempotencyCatalogPort, LexicalIndexOpenPort, LexicalSearcher, OperationInspectV1,
    SemanticContentRootsPort, SemanticIndexOpenPort, SemanticSearcher,
    SourcePublicationCatalogPort,
};

use crate::ingest_dispatcher::SearchCorpusAuthorityInspectPort;
use crate::search_corpus_retention::SearchCorpusIndexBytesPort;
use crate::{
    ActivationCatalog, AuxiliaryAuthorityStore, Ledger, OpenedSnapshot,
    PreparedSearchCorpusGenerationV1, SealedSearchCorpusAuthorityStateV1,
    SearchCorpusGenerationActivationV1, SearchCorpusGenerationV1, SnapshotKey, SnapshotRegistries,
};

mod pair_lock;

pub(crate) use pair_lock::{
    ActiveSearchCorpusPinReadPort, SearchCorpusPairMutationCoordinator,
    SearchCorpusPairMutationGuard,
};

pub(crate) const ERR_ACTIVATION_TARGET_UNOPENABLE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::ActivationTargetUnopenable;
const ERR_ROLLBACK_TARGET_UNOPENABLE: quanta_index_contract::SearchPlaneErrorCodeV2 =
    quanta_index_contract::SearchPlaneErrorCodeV2::RollbackTargetUnopenable;

/// Opens the activation and rollback-history authorities as one lifecycle
/// unit.  Production composition must use this owner rather than opening either
/// mutable authority independently.
#[derive(Debug)]
pub struct SearchCorpusLifecycleOwner {
    state_root_identity_v1: PathBuf,
    activation_catalog: Arc<ActivationCatalog>,
    authority_store: Arc<AuxiliaryAuthorityStore>,
}

impl SearchCorpusLifecycleOwner {
    /// Open both authorities under `state_root`. `index_bytes` is what
    /// retention measures its byte limits over: the adapters' on-disk
    /// generation bytes.
    pub fn open(
        state_root: impl AsRef<Path>,
        retention: crate::readiness::SearchCorpusHistoryRetentionPolicyV1,
        index_bytes: Arc<dyn SearchCorpusIndexBytesPort>,
    ) -> Result<Self, CoreError> {
        // Resolve one immutable path identity before deriving either mutable
        // authority root. A symlink retarget between the two opens must not
        // split the activation head and rollback history across state roots.
        let state_root_identity_v1 = canonical_state_root_identity_v1(state_root.as_ref())?;
        let activation_root = state_root_identity_v1.join("activations");
        let authority_root = state_root_identity_v1.join("authorities");
        let coordinator = SearchCorpusPairMutationCoordinator::shared();
        let activation_catalog = Arc::new(ActivationCatalog::open_with_lifecycle_v1(
            &activation_root,
            Arc::clone(&coordinator),
        )?);
        let active_pins: Arc<dyn ActiveSearchCorpusPinReadPort> = activation_catalog.clone();
        let authority_store = Arc::new(AuxiliaryAuthorityStore::open_with_lifecycle_v1(
            &authority_root,
            retention,
            Arc::clone(&coordinator),
            active_pins,
            index_bytes,
        )?);
        Ok(Self {
            state_root_identity_v1,
            activation_catalog,
            authority_store,
        })
    }

    #[must_use]
    pub fn activation_catalog(&self) -> Arc<ActivationCatalog> {
        Arc::clone(&self.activation_catalog)
    }

    #[must_use]
    pub fn authority_store(&self) -> Arc<AuxiliaryAuthorityStore> {
        Arc::clone(&self.authority_store)
    }

    pub fn require_state_root_v1(&self, state_root: &Path) -> Result<(), CoreError> {
        let observed = canonical_state_root_identity_v1(state_root)?;
        if observed == self.state_root_identity_v1 {
            return Ok(());
        }
        Err(CoreError::InvalidContract(format!(
            "search-corpus lifecycle: state-root identity mismatch: owner={} requested={}",
            self.state_root_identity_v1.display(),
            observed.display(),
        )))
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn coordinator(&self) -> Arc<SearchCorpusPairMutationCoordinator> {
        self.activation_catalog.lifecycle_coordinator()
    }

    /// Prove every active `(lexical, semantic)` pair physically, once,
    /// promote the proven handles into the snapshot registries so the
    /// first query after restart is a hit, and report how many pairs were
    /// proven. This is boot's only deep validation (QI-BB-026): a defective
    /// active pair fails boot with a typed cause before any socket binds;
    /// inactive generations are not examined here.
    pub fn validate_rehydrated_active_generations_v1(
        &self,
        promotion: &ActivationPromotionParts,
    ) -> Result<usize, CoreError> {
        let active_pairs = self
            .activation_catalog
            .all_active_search_corpora_for_bootstrap_v1()?;
        for active in &active_pairs {
            let source_event = promotion.prove_and_promote_pair(
                active,
                ProofGate {
                    error_code: ERR_ACTIVATION_TARGET_UNOPENABLE,
                    operation: "restart rehydrate",
                    findings: DoorFindingPolicy::FailClosed,
                },
            )?;
            self.activation_catalog
                .validate_proved_source_history(active.lexical(), source_event.as_ref())?;
        }
        Ok(active_pairs.len())
    }
}

/// Where an activation, a rollback or a restart proves a pair and puts the
/// handles it proved: the openers and the registries that keep the handles
/// resident (QI-BB-017 보완 #4).
///
/// The proof and the promotion are one step: [`LexicalIndexOpenPort::open_proven`]
/// and [`SemanticIndexOpenPort::open_proven`] prove the candidate exactly
/// as the activation validator would and return the handle that proof
/// opened, so each track is walked once and the first query is a registry
/// hit, not a second full open. The semantic generation's sealed content
/// roots must also be exactly the ones the candidate names (QI-BB-028). A
/// content defect a door proves while activation or rollback picks a
/// generation is recorded through each track's [`DoorFindingQuarantinePort`]
/// (QI-BB-026).
#[derive(Clone)]
pub struct ActivationPromotionParts {
    pub lexical_open: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    pub semantic_open: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    pub semantic_content_roots: Arc<dyn SemanticContentRootsPort + Send + Sync>,
    pub lexical_door_findings: Arc<dyn DoorFindingQuarantinePort + Send + Sync>,
    pub semantic_door_findings: Arc<dyn DoorFindingQuarantinePort + Send + Sync>,
    pub snapshots: SnapshotRegistries,
}

/// What a gate does with a door's content-defect verdict (QI-BB-026).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DoorFindingPolicy {
    /// Activation and rollback pick a generation that is not the serve
    /// head. The verdict is re-proved and recorded as a quarantine, so the
    /// inventory lists the generation and no later activation or rollback
    /// picks it.
    Quarantine,
    /// Restart proves the serve head. A defect fails boot typed before any
    /// socket binds, and the disk is left as it was found.
    FailClosed,
}

/// The gate a pair proof runs for: the code a refusal carries, the
/// operation it names, and what a door's content defect leads to.
#[derive(Clone, Copy, Debug)]
struct ProofGate<'a> {
    error_code: quanta_index_contract::SearchPlaneErrorCodeV2,
    operation: &'a str,
    findings: DoorFindingPolicy,
}

impl ActivationPromotionParts {
    /// Prove both tracks of `candidate` by opening them, check the semantic
    /// content roots the candidate names, and promote both handles into the
    /// registries.
    ///
    /// Runs outside every ledger guard: the opens hash the generation's
    /// decoded bytes, and nothing else in the process should wait on that.
    /// A track that does not prove is refused typed under the gate's error
    /// code — after its content defect, under
    /// [`DoorFindingPolicy::Quarantine`], was recorded by the track's
    /// adapter — a semantic generation that sealed other content roots than
    /// the candidate names is refused `SEMANTIC_ROW_ROOT_MISMATCH` — the
    /// same source digest built in another state root does not pass as this
    /// one — and in either case nothing is promoted for the pair.
    fn prove_and_promote_pair(
        &self,
        candidate: &SearchCorpusGenerationV1,
        gate: ProofGate<'_>,
    ) -> Result<Option<SourcePublicationEvent>, CoreError> {
        let ProofGate {
            error_code,
            operation,
            findings: _,
        } = gate;
        let key = SnapshotKey::new(
            candidate.repo_id(),
            candidate.revision_id(),
            candidate.manifest_generation(),
        );
        let lexical: Arc<dyn LexicalSearcher> = Arc::from(
            self.lexical_open
                .open_proven(candidate.lexical())
                .map_err(|source| self.refuse_unproven(candidate.lexical(), gate, &source))?,
        );
        let semantic: Arc<dyn SemanticSearcher> = Arc::from(
            self.semantic_open
                .open_proven(candidate.semantic())
                .map_err(|source| self.refuse_unproven(candidate.semantic(), gate, &source))?,
        );
        let sealed = self
            .semantic_content_roots
            .sealed_content_roots(candidate.semantic())
            .map_err(|source| {
                generation_target_unopenable(candidate.semantic(), operation, error_code, &source)
            })?;
        if sealed != *candidate.semantic_content() {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SemanticRowRootMismatch,
                message: format!(
                    "search-corpus {operation}: the physical semantic generation for repo={} revision={} generation={} sealed row_root={} membership_root={}, but the identity names row_root={} membership_root={}; nothing was promoted",
                    candidate.repo_id().as_str(),
                    candidate.revision_id().as_str(),
                    candidate.manifest_generation().get(),
                    sealed.row_root_digest,
                    sealed.membership_root_digest,
                    candidate.semantic_content().row_root_digest,
                    candidate.semantic_content().membership_root_digest,
                ),
            });
        }
        let source_event = lexical.source_publication_event().cloned();
        let lexical_bytes = lexical.resident_bytes_estimate();
        let _retained = self.snapshots.lexical.promote(
            &key,
            &OpenedSnapshot {
                handle: lexical,
                resident_bytes: lexical_bytes,
            },
        )?;
        let semantic_bytes = semantic.resident_bytes_estimate();
        let _retained = self.snapshots.semantic.promote(
            &key,
            &OpenedSnapshot {
                handle: semantic,
                resident_bytes: semantic_bytes,
            },
        )?;
        Ok(source_event)
    }

    /// The typed refusal for a track whose door did not admit `target`.
    ///
    /// Under [`DoorFindingPolicy::Quarantine`], a content-defect verdict is
    /// handed to the track's adapter, which re-proves it and records the
    /// quarantine; the refusal then says what was recorded — the receipt,
    /// that the re-proof admitted the generation, or why nothing could be
    /// written. The refusal is the same typed failure either way.
    fn refuse_unproven(
        &self,
        target: &GenerationSnapshot,
        gate: ProofGate<'_>,
        source: &CoreError,
    ) -> CoreError {
        let is_content_defect = matches!(
            source,
            CoreError::Typed { code, .. } if *code == GENERATION_SIDECAR_CORRUPT_CODE
        );
        if gate.findings == DoorFindingPolicy::FailClosed || !is_content_defect {
            return generation_target_unopenable(target, gate.operation, gate.error_code, source);
        }
        let door_findings = match target.track {
            SearchPlaneTrackKind::Lexical => &self.lexical_door_findings,
            SearchPlaneTrackKind::Semantic => &self.semantic_door_findings,
            SearchPlaneTrackKind::Structural => {
                return CoreError::InvalidContract(format!(
                    "search-corpus {}: a structural generation is not a search-corpus track, yet its door answered: {source}",
                    gate.operation
                ));
            }
        };
        let recorded = match door_findings.quarantine_door_finding(target) {
            Ok(DoorFindingOutcome::Quarantined { quarantined }) => {
                format!(
                    "quarantined as {} at {}",
                    quarantined.reason,
                    quarantined.path.display()
                )
            }
            Ok(DoorFindingOutcome::NotReproduced) => {
                "the re-proof admitted the generation; nothing was quarantined".to_string()
            }
            Err(error) => format!("the quarantine was not recorded: {error}"),
        };
        CoreError::Typed {
            code: gate.error_code,
            message: format!(
                "{}; {recorded}",
                target_unopenable_message(target, gate.operation, source)
            ),
        }
    }
}

/// The ports one `SearchCorpusLifecycleService` is composed from.
pub struct SearchCorpusLifecycleParts {
    pub activation_catalog: Arc<ActivationCatalog>,
    pub idempotency: Arc<dyn IdempotencyCatalogPort>,
    pub ledger: Arc<RwLock<Ledger>>,
    /// The durable sealed history, consulted under the pair guard right
    /// before the CAS commits.
    pub authority: Arc<dyn SearchCorpusAuthorityInspectPort + Send + Sync>,
    pub promotion: ActivationPromotionParts,
}

pub(crate) struct SearchCorpusLifecycleService {
    coordinator: Arc<SearchCorpusPairMutationCoordinator>,
    activation_catalog: Arc<ActivationCatalog>,
    idempotency: Arc<dyn IdempotencyCatalogPort>,
    ledger: Arc<RwLock<Ledger>>,
    authority: Arc<dyn SearchCorpusAuthorityInspectPort + Send + Sync>,
    promotion: ActivationPromotionParts,
}

impl SearchCorpusLifecycleService {
    pub(crate) fn new(parts: SearchCorpusLifecycleParts) -> Self {
        let SearchCorpusLifecycleParts {
            activation_catalog,
            idempotency,
            ledger,
            authority,
            promotion,
        } = parts;
        Self {
            coordinator: activation_catalog.lifecycle_coordinator(),
            activation_catalog,
            idempotency,
            ledger,
            authority,
            promotion,
        }
    }

    /// Activate a prepared composite: validate → prove outside the ledger
    /// guard → re-check → commit (QI-BB-020 follow-up #2).
    ///
    /// The pair guard serializes every durable mutation of the pair, so it
    /// is held throughout. The ledger read guard is held only for the
    /// precondition checks — never across the physical proof, which hashes
    /// the generation's bytes and must not stall every other repo's ingest
    /// and every query behind a `RwLock` writer. The proof's handles are
    /// promoted into the snapshot registries, the preconditions are checked
    /// again (the in-memory ledger and the durable authority, which a
    /// concurrent seal may have reaped the candidate from), and only then
    /// does the CAS commit.
    pub(crate) fn activate_prepared_v1(
        &self,
        prepared: &PreparedSearchCorpusGenerationV1,
    ) -> Result<SearchCorpusGenerationActivationV1, CoreError> {
        let candidate = prepared.candidate();
        let pair_guard = self
            .coordinator
            .lock_pair(candidate.repo_id(), candidate.revision_id())?;
        self.check_activation_preconditions_v1(candidate)?;
        let source_event = self.prove_and_promote_v1(
            candidate,
            ProofGate {
                error_code: ERR_ACTIVATION_TARGET_UNOPENABLE,
                operation: "activation",
                findings: DoorFindingPolicy::Quarantine,
            },
        )?;
        self.check_activation_preconditions_v1(candidate)?;
        self.require_durably_sealed_v1(candidate, "activation")?;
        if let Some(event) = source_event.as_ref() {
            let record = self.activation_catalog.reconcile_source_event(
                candidate.repo_id(),
                event,
                self.idempotency.as_ref(),
            )?;
            let OperationInspectV1::Committed { receipt, .. } =
                self.idempotency.inspect(&record.binding.journal_key)?
            else {
                return Err(CoreError::NotReady(
                    "source activation lost its original committed journal".into(),
                ));
            };
            if receipt.semantic_content.as_ref() != Some(candidate.semantic_content()) {
                return Err(CoreError::Storage(
                    "source activation semantic roots differ from the original publication receipt"
                        .into(),
                ));
            }
        }
        self.activation_catalog.activate_prepared_under_guard_v1(
            &pair_guard,
            prepared,
            source_event.as_ref(),
        )
    }

    /// Roll back to a historically sealed composite: the same shape as
    /// [`Self::activate_prepared_v1`], with the rollback preconditions.
    pub(crate) fn rollback_v1(
        &self,
        request: &SearchPlaneRollbackSearchCorpusGenerationCasRequest,
        target: &SearchCorpusGenerationV1,
    ) -> Result<SearchPlaneSearchCorpusRollbackCasAck, CoreError> {
        let pair_guard = self
            .coordinator
            .lock_pair(target.repo_id(), target.revision_id())?;
        self.check_rollback_preconditions_v1(target)?;
        let source_event = self.prove_and_promote_v1(
            target,
            ProofGate {
                error_code: ERR_ROLLBACK_TARGET_UNOPENABLE,
                operation: "rollback",
                findings: DoorFindingPolicy::Quarantine,
            },
        )?;
        self.check_rollback_preconditions_v1(target)?;
        self.activation_catalog
            .validate_proved_source_history(target.lexical(), source_event.as_ref())?;
        self.require_durably_sealed_v1(target, "rollback")?;
        self.activation_catalog
            .rollback_under_guard_v1(&pair_guard, request)
    }

    /// The in-memory preconditions of an activation, under one short read
    /// guard: both tracks are the currently sealed identity and both are
    /// historically sealed.
    fn check_activation_preconditions_v1(
        &self,
        candidate: &SearchCorpusGenerationV1,
    ) -> Result<(), CoreError> {
        let ledger = self.read_ledger()?;
        validate_currently_sealed_candidate_v1(&ledger, candidate.lexical())?;
        validate_currently_sealed_candidate_v1(&ledger, candidate.semantic())?;
        ledger.validate_historically_sealed_track_identity(
            candidate.lexical(),
            "search-corpus activation",
        )?;
        ledger.validate_historically_sealed_track_identity(
            candidate.semantic(),
            "search-corpus activation",
        )
    }

    /// The in-memory preconditions of a rollback, under one short read
    /// guard: both tracks are historically sealed.
    fn check_rollback_preconditions_v1(
        &self,
        target: &SearchCorpusGenerationV1,
    ) -> Result<(), CoreError> {
        let ledger = self.read_ledger()?;
        ledger.validate_historically_sealed_track_identity(
            target.lexical(),
            "search-corpus rollback",
        )?;
        ledger.validate_historically_sealed_track_identity(
            target.semantic(),
            "search-corpus rollback",
        )
    }

    /// The durable authority's word, under the pair guard the caller
    /// holds: the exact `(generation, digest)` is recorded right now.
    fn require_durably_sealed_v1(
        &self,
        candidate: &SearchCorpusGenerationV1,
        operation: &str,
    ) -> Result<(), CoreError> {
        match self.authority.inspect_sealed_search_corpus(
            candidate.repo_id(),
            candidate.revision_id(),
            candidate.manifest_generation(),
            candidate.manifest_digest(),
        )? {
            SealedSearchCorpusAuthorityStateV1::Exact => Ok(()),
            SealedSearchCorpusAuthorityStateV1::Absent => Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchTrackGenerationNotSealed,
                message: format!(
                    "search-corpus {operation}: generation {} of repo={} revision={} is not recorded in the durable sealed history any more; it was reaped before the CAS committed",
                    candidate.manifest_generation().get(),
                    candidate.repo_id().as_str(),
                    candidate.revision_id().as_str(),
                ),
            }),
        }
    }

    fn read_ledger(&self) -> Result<std::sync::RwLockReadGuard<'_, Ledger>, CoreError> {
        self.ledger
            .read()
            .map_err(|error| CoreError::Storage(format!("ledger poisoned: {error}")))
    }

    /// The physical proof of both tracks, which is also the open of the
    /// handles it promotes, outside the ledger guard.
    fn prove_and_promote_v1(
        &self,
        candidate: &SearchCorpusGenerationV1,
        gate: ProofGate<'_>,
    ) -> Result<Option<SourcePublicationEvent>, CoreError> {
        self.promotion.prove_and_promote_pair(candidate, gate)
    }
}

fn canonical_state_root_identity_v1(state_root: &Path) -> Result<PathBuf, CoreError> {
    std::fs::canonicalize(state_root).map_err(|error| {
        CoreError::Storage(format!(
            "search-corpus lifecycle: resolve state-root identity {}: {error}",
            state_root.display(),
        ))
    })
}

fn validate_currently_sealed_candidate_v1(
    ledger: &Ledger,
    candidate: &GenerationSnapshot,
) -> Result<(), CoreError> {
    let observed_generation =
        ledger.track_sealed(&candidate.repo_id, &candidate.revision_id, candidate.track);
    let observed_digest =
        ledger.track_manifest_digest(&candidate.repo_id, &candidate.revision_id, candidate.track);
    if observed_generation == Some(candidate.manifest_generation)
        && observed_digest == Some(candidate.manifest_digest.as_str())
    {
        return Ok(());
    }
    Err(CoreError::NotReady(format!(
        "activate-generation-cas: candidate is not the currently sealed track identity for repo={} revision={} track={:?}: candidate_generation={} candidate_digest={} observed_generation={:?} observed_digest={:?}",
        candidate.repo_id.as_str(),
        candidate.revision_id.as_str(),
        candidate.track,
        candidate.manifest_generation.get(),
        candidate.manifest_digest,
        observed_generation.map(quanta_index_contract::ManifestGeneration::get),
        observed_digest,
    )))
}

fn generation_target_unopenable(
    candidate: &GenerationSnapshot,
    operation: &str,
    error_code: quanta_index_contract::SearchPlaneErrorCodeV2,
    source: &CoreError,
) -> CoreError {
    CoreError::Typed {
        code: error_code,
        message: target_unopenable_message(candidate, operation, source),
    }
}

fn target_unopenable_message(
    candidate: &GenerationSnapshot,
    operation: &str,
    source: &CoreError,
) -> String {
    format!(
        "search-corpus {operation}: target is not physically valid for repo={} revision={} track={:?} generation={}: {source}",
        candidate.repo_id.as_str(),
        candidate.revision_id.as_str(),
        candidate.track,
        candidate.manifest_generation.get(),
    )
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::panic_in_result_fn,
        clippy::significant_drop_tightening,
        reason = "Result-returning lifecycle tests use assertions and intentionally hold mutation guards through exact CAS calls"
    )]

    use std::path::Path;

    use quanta_index_contract::{
        GenerationSnapshot, ManifestGeneration, RepoId, RevisionId, SearchPlaneTrackKind,
    };
    use tempfile::tempdir;

    use std::sync::{Arc, Mutex};

    use super::{
        ActivationPromotionParts, ActiveSearchCorpusPinReadPort, DoorFindingPolicy,
        ERR_ACTIVATION_TARGET_UNOPENABLE, ERR_ROLLBACK_TARGET_UNOPENABLE, ProofGate,
        SearchCorpusLifecycleOwner, SearchCorpusPairMutationCoordinator,
    };
    use crate::content_roots_test_support::{generation_keyed_content_roots, roots_for_generation};
    use crate::door_findings_test_support::{
        RecordingDoorFindings, ScriptedFinding, scripted_quarantine_path,
    };
    use crate::query_dispatcher::tests::support::lexical::StubLexicalSearcher;
    use crate::query_dispatcher::tests::support::semantic::{
        RecordingSemanticOpener, RecordingSemanticState,
    };
    use crate::readiness::SearchCorpusHistoryRetentionPolicyV1;
    use crate::search_corpus_retention::SearchCorpusIndexBytesPort;
    use crate::{
        PreparedSearchCorpusGenerationV1, SearchCorpusGenerationV1, SnapshotKey,
        SnapshotRegistries, SnapshotRegistryPolicy,
    };
    use quanta_index_core::{
        CoreError, LexicalIndexOpenPort, LexicalSearcher, RequestBudgetV1, SemanticIndexOpenPort,
        SemanticSearcher,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// How an echo opener's `open_proven` refuses, if it does.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum DoorRefusal {
        /// The generation is not on disk.
        Missing,
        /// The door proved a committed file does not match the seal.
        ContentDefect,
        /// The generation already carries a quarantine receipt.
        Quarantined,
    }

    impl DoorRefusal {
        fn error(self, candidate: &GenerationSnapshot) -> CoreError {
            match self {
                Self::Missing => CoreError::NotFound(format!(
                    "injected missing {:?} generation",
                    candidate.track
                )),
                Self::ContentDefect => CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationSidecarCorrupt,
                    message: format!("injected content defect on {:?}", candidate.track),
                },
                Self::Quarantined => CoreError::Typed {
                    code: quanta_index_contract::SearchPlaneErrorCodeV2::GenerationQuarantined,
                    message: format!("injected quarantine on {:?}", candidate.track),
                },
            }
        }
    }

    /// Openers whose handles prove the digest the rehydrate fixture seals
    /// under, `manifest-rehydrate-<generation>`; `open_proven` refuses as
    /// scripted.
    struct EchoLexicalOpener {
        refusal: Option<DoorRefusal>,
    }

    impl LexicalIndexOpenPort for EchoLexicalOpener {
        fn preflight_query_primitives(
            &self,
            plan: &quanta_index_core::ValidatedLexicalPlan,
            budget: &quanta_index_core::RequestBudgetV1,
        ) -> Result<(), CoreError> {
            quanta_index_lexical::planner::LexicalPlanner::validate_query_primitives(
                plan,
                &quanta_index_lexical::regex::RegexPolicy::defaults(),
                budget,
            )
        }

        fn open(
            &self,
            _repo: &RepoId,
            _revision: &RevisionId,
            generation: ManifestGeneration,
        ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
            Ok(Box::new(StubLexicalSearcher {
                results: Vec::new(),
                manifest_digest: Some(format!("manifest-rehydrate-{}", generation.get())),
            }))
        }

        fn open_proven(
            &self,
            candidate: &GenerationSnapshot,
        ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
            if let Some(refusal) = self.refusal {
                return Err(refusal.error(candidate));
            }
            self.open(
                &candidate.repo_id,
                &candidate.revision_id,
                candidate.manifest_generation,
            )
        }
    }

    struct EchoSemanticOpener {
        refusal: Option<DoorRefusal>,
    }

    impl SemanticIndexOpenPort for EchoSemanticOpener {
        fn open(
            &self,
            repo: &RepoId,
            revision: &RevisionId,
            generation: ManifestGeneration,
        ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
            let state = Arc::new(Mutex::new(RecordingSemanticState {
                manifest_digest: Some(format!("manifest-rehydrate-{}", generation.get())),
                ..RecordingSemanticState::default()
            }));
            RecordingSemanticOpener { state }.open(repo, revision, generation)
        }

        fn open_proven(
            &self,
            candidate: &GenerationSnapshot,
        ) -> Result<Box<dyn SemanticSearcher>, CoreError> {
            if let Some(refusal) = self.refusal {
                return Err(refusal.error(candidate));
            }
            self.open(
                &candidate.repo_id,
                &candidate.revision_id,
                candidate.manifest_generation,
            )
        }
    }

    /// Promotion over the echo openers, refusing `rejected_track`'s proof
    /// as a missing generation; the sealed content roots are the fixtures'
    /// (keyed on the generation).
    fn promotion_rejecting(
        rejected_track: Option<SearchPlaneTrackKind>,
    ) -> ActivationPromotionParts {
        let refusal = rejected_track.map(|track| (track, DoorRefusal::Missing));
        let (promotion, _lexical, _semantic) =
            promotion_refusing(refusal, ScriptedFinding::Quarantines);
        promotion
    }

    /// Promotion whose `refusal.0` track's door refuses as `refusal.1`,
    /// with both tracks' door-finding doubles answering `findings`.
    fn promotion_refusing(
        refusal: Option<(SearchPlaneTrackKind, DoorRefusal)>,
        findings: ScriptedFinding,
    ) -> (
        ActivationPromotionParts,
        Arc<RecordingDoorFindings>,
        Arc<RecordingDoorFindings>,
    ) {
        let refusal_on = |track| {
            refusal
                .filter(|(refused, _how)| *refused == track)
                .map(|(_refused, how)| how)
        };
        let lexical_findings = Arc::new(RecordingDoorFindings::new(findings));
        let semantic_findings = Arc::new(RecordingDoorFindings::new(findings));
        let promotion = ActivationPromotionParts {
            lexical_open: Arc::new(EchoLexicalOpener {
                refusal: refusal_on(SearchPlaneTrackKind::Lexical),
            }),
            semantic_open: Arc::new(EchoSemanticOpener {
                refusal: refusal_on(SearchPlaneTrackKind::Semantic),
            }),
            semantic_content_roots: generation_keyed_content_roots(),
            lexical_door_findings: Arc::<RecordingDoorFindings>::clone(&lexical_findings),
            semantic_door_findings: Arc::<RecordingDoorFindings>::clone(&semantic_findings),
            snapshots: SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
        };
        (promotion, lexical_findings, semantic_findings)
    }

    const ACTIVATION_GATE: ProofGate<'static> = ProofGate {
        error_code: ERR_ACTIVATION_TARGET_UNOPENABLE,
        operation: "activation",
        findings: DoorFindingPolicy::Quarantine,
    };

    const ROLLBACK_GATE: ProofGate<'static> = ProofGate {
        error_code: ERR_ROLLBACK_TARGET_UNOPENABLE,
        operation: "rollback",
        findings: DoorFindingPolicy::Quarantine,
    };

    const RESTART_GATE: ProofGate<'static> = ProofGate {
        error_code: ERR_ACTIVATION_TARGET_UNOPENABLE,
        operation: "restart rehydrate",
        findings: DoorFindingPolicy::FailClosed,
    };

    /// The typed refusal a proof under `gate` answers; nothing promoted.
    fn refused_proof(
        promotion: &ActivationPromotionParts,
        gate: ProofGate<'_>,
    ) -> Result<(quanta_index_contract::SearchPlaneErrorCodeV2, String), Box<dyn std::error::Error>>
    {
        let candidate = active_generation()?;
        let Err(CoreError::Typed { code, message }) =
            promotion.prove_and_promote_pair(&candidate, gate)
        else {
            return Err(format!("the {} proof must be refused typed", gate.operation).into());
        };
        for stats in [
            promotion.snapshots.lexical.stats()?,
            promotion.snapshots.semantic.stats()?,
        ] {
            if stats.entries != 0 {
                return Err(format!("a refused proof promoted a handle: {stats:?}").into());
            }
        }
        Ok((code, message))
    }

    fn retention() -> Result<SearchCorpusHistoryRetentionPolicyV1, CoreError> {
        SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 8, 8 * 1024 * 1024)
    }

    fn scripted_bytes() -> Arc<dyn SearchCorpusIndexBytesPort> {
        Arc::new(crate::readiness::ScriptedIndexBytesV1)
    }

    fn active_generation_for(
        repo_id: &str,
        generation: u64,
        digest: &str,
    ) -> Result<SearchCorpusGenerationV1, CoreError> {
        let snapshot = |track| GenerationSnapshot {
            repo_id: RepoId::new(repo_id).expect("test fixture ID satisfies canonical policy"),
            revision_id: RevisionId::new("revision-rehydrate")
                .expect("static fixture ID satisfies canonical policy"),
            track,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: digest.to_string(),
        };
        SearchCorpusGenerationV1::new(
            snapshot(SearchPlaneTrackKind::Lexical),
            snapshot(SearchPlaneTrackKind::Semantic),
            roots_for_generation(generation),
        )
    }

    fn active_generation() -> Result<SearchCorpusGenerationV1, CoreError> {
        active_generation_for("repo-rehydrate", 17, "manifest-rehydrate-17")
    }

    fn activate_generation(
        owner: &SearchCorpusLifecycleOwner,
        active: SearchCorpusGenerationV1,
    ) -> TestResult {
        let coordinator = owner.coordinator();
        let guard = coordinator.lock_pair(active.repo_id(), active.revision_id())?;
        let _activation = owner
            .activation_catalog()
            .activate_prepared_under_guard_v1(
                &guard,
                &PreparedSearchCorpusGenerationV1::new(active, None)?,
                None,
            )?;
        Ok(())
    }

    fn activate(owner: &SearchCorpusLifecycleOwner) -> TestResult {
        activate_generation(owner, active_generation()?)
    }

    fn assert_rehydrated_active_track_rejected(
        state_root: &Path,
        rejected_track: SearchPlaneTrackKind,
    ) -> TestResult {
        let owner = SearchCorpusLifecycleOwner::open(state_root, retention()?, scripted_bytes())?;
        activate(&owner)?;
        drop(owner);
        let owner = SearchCorpusLifecycleOwner::open(state_root, retention()?, scripted_bytes())?;
        let promotion = promotion_rejecting(Some(rejected_track));
        let Err(CoreError::Typed { code, message }) =
            owner.validate_rehydrated_active_generations_v1(&promotion)
        else {
            return Err(format!(
                "missing rehydrated {rejected_track:?} generation unexpectedly validated"
            )
            .into());
        };
        assert_eq!(code, ERR_ACTIVATION_TARGET_UNOPENABLE);
        assert!(message.contains("restart rehydrate"));
        assert!(message.contains(&format!("track={rejected_track:?}")));
        Ok(())
    }

    #[test]
    fn lifecycle_owner_derives_both_authority_roots_from_one_state_root_v1() -> TestResult {
        let state_root = tempdir()?;
        let _owner =
            SearchCorpusLifecycleOwner::open(state_root.path(), retention()?, scripted_bytes())?;
        assert!(state_root.path().join("activations/.staging").is_dir());
        assert!(
            state_root
                .path()
                .join("authorities/search-corpus/.staging")
                .is_dir()
        );
        Ok(())
    }

    #[test]
    fn lifecycle_owner_rejects_a_different_state_root_identity_v1() -> TestResult {
        let owned_root = tempdir()?;
        let foreign_root = tempdir()?;
        let owner =
            SearchCorpusLifecycleOwner::open(owned_root.path(), retention()?, scripted_bytes())?;

        let rejected = owner.require_state_root_v1(foreign_root.path());
        let Err(CoreError::InvalidContract(message)) = rejected else {
            return Err("lifecycle owner accepted a foreign state-root identity".into());
        };
        assert!(message.contains("state-root identity mismatch"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_owner_refuses_symlink_activation_authority_root_v1() -> TestResult {
        use std::os::unix::fs::symlink;

        for root_name in ["activations", "authorities"] {
            let state_root = tempdir()?;
            let attacker_root = tempdir()?;
            let attacker_authority = attacker_root.path().join(root_name);
            std::fs::create_dir(&attacker_authority)?;
            symlink(&attacker_authority, state_root.path().join(root_name))?;

            let result =
                SearchCorpusLifecycleOwner::open(state_root.path(), retention()?, scripted_bytes());
            let Err(CoreError::Storage(message)) = result else {
                return Err(format!(
                    "lifecycle owner followed a symlink {root_name} authority root"
                )
                .into());
            };
            assert!(message.contains("durable directory path is a symlink"));
        }
        Ok(())
    }

    #[test]
    fn pair_guard_rejects_a_different_pair_even_on_the_same_stripe_v1() -> TestResult {
        let coordinator = SearchCorpusPairMutationCoordinator::shared();
        let repo_a =
            RepoId::new("repo-guard-a").expect("static fixture ID satisfies canonical policy");
        let revision = RevisionId::new("revision-guard")
            .expect("static fixture ID satisfies canonical policy");
        let stripe_a = crate::readiness::search_corpus_lock_stripe_v1(&repo_a, &revision);
        let repo_b = (0_u64..4096)
            .map(|index| {
                RepoId::new(format!("repo-guard-b-{index}"))
                    .expect("test fixture ID satisfies canonical policy")
            })
            .find(|repo| {
                crate::readiness::search_corpus_lock_stripe_v1(repo, &revision) == stripe_a
            })
            .ok_or("failed to find a lifecycle lock-stripe collision")?;
        let guard = coordinator.lock_pair(&repo_a, &revision)?;

        let rejected = guard.require_pair_v1(coordinator.as_ref(), &repo_b, &revision);
        let Err(CoreError::Storage(message)) = rejected else {
            return Err("pair guard accepted a different pair on the same lock stripe".into());
        };
        assert!(message.contains("pair identity mismatch"));
        Ok(())
    }

    #[test]
    fn restart_rehydrate_rejects_missing_active_lexical_generation_v1() -> TestResult {
        let state_root = tempdir()?;
        assert_rehydrated_active_track_rejected(state_root.path(), SearchPlaneTrackKind::Lexical)
    }

    #[test]
    fn restart_rehydrate_rejects_missing_active_semantic_generation_v1() -> TestResult {
        let state_root = tempdir()?;
        assert_rehydrated_active_track_rejected(state_root.path(), SearchPlaneTrackKind::Semantic)
    }

    /// An active composite whose named semantic content roots are not the
    /// ones the physical generation sealed is refused at restart rehydrate.
    ///
    /// Typed `SEMANTIC_ROW_ROOT_MISMATCH` (QI-BB-028), and nothing is
    /// promoted: the same source digest built in another state root does
    /// not pass as this one.
    #[test]
    fn restart_rehydrate_refuses_an_active_composite_whose_roots_differ_v1() -> TestResult {
        struct OtherRoots;
        impl quanta_index_core::SemanticContentRootsPort for OtherRoots {
            fn sealed_content_roots(
                &self,
                _sealed: &GenerationSnapshot,
            ) -> Result<quanta_index_contract::SemanticContentRootsV1, CoreError> {
                Ok(roots_for_generation(18))
            }
        }
        let state_root = tempdir()?;
        let owner =
            SearchCorpusLifecycleOwner::open(state_root.path(), retention()?, scripted_bytes())?;
        activate(&owner)?;
        drop(owner);
        let owner =
            SearchCorpusLifecycleOwner::open(state_root.path(), retention()?, scripted_bytes())?;
        // The physical generation sealed different roots than the active
        // composite names (the fixture's roots are keyed on generation 17;
        // this port reports generation 18's).
        let promotion = ActivationPromotionParts {
            semantic_content_roots: Arc::new(OtherRoots),
            ..promotion_rejecting(None)
        };
        let Err(CoreError::Typed { code, message }) =
            owner.validate_rehydrated_active_generations_v1(&promotion)
        else {
            return Err("an active composite with foreign roots was rehydrated".into());
        };
        assert_eq!(code, quanta_index_core::SEMANTIC_ROW_ROOT_MISMATCH_CODE);
        assert!(message.contains("restart rehydrate"), "{message}");
        assert!(
            message.contains(&roots_for_generation(17).row_root_digest)
                && message.contains(&roots_for_generation(18).row_root_digest),
            "the refusal names both roots: {message}"
        );
        for stats in [
            promotion.snapshots.lexical.stats()?,
            promotion.snapshots.semantic.stats()?,
        ] {
            assert_eq!(stats.entries, 0, "nothing was promoted: {stats:?}");
        }
        Ok(())
    }

    #[test]
    fn restart_rehydrate_accepts_complete_active_composite_v1() -> TestResult {
        let state_root = tempdir()?;
        let owner =
            SearchCorpusLifecycleOwner::open(state_root.path(), retention()?, scripted_bytes())?;
        activate(&owner)?;
        drop(owner);
        let owner =
            SearchCorpusLifecycleOwner::open(state_root.path(), retention()?, scripted_bytes())?;
        let promotion = promotion_rejecting(None);
        let validated = owner.validate_rehydrated_active_generations_v1(&promotion)?;
        if validated != 1 {
            return Err(format!("expected exactly one active pair proven, got {validated}").into());
        }
        // The proven handles are resident: the first acquire of the active
        // pair on either track is a hit and runs no opener.
        let key = SnapshotKey::new(
            &RepoId::new("repo-rehydrate").expect("static fixture ID satisfies canonical policy"),
            &RevisionId::new("revision-rehydrate")
                .expect("static fixture ID satisfies canonical policy"),
            ManifestGeneration::new(17),
        );
        let lexical =
            promotion
                .snapshots
                .lexical
                .acquire(&key, &RequestBudgetV1::unbounded(), || {
                    Err(CoreError::Storage(
                        "restart rehydrate must have promoted the lexical handle".into(),
                    ))
                })?;
        if lexical.handle.artifact_identity().manifest_digest != "manifest-rehydrate-17" {
            return Err("the resident lexical handle is not the proven one".into());
        }
        let semantic =
            promotion
                .snapshots
                .semantic
                .acquire(&key, &RequestBudgetV1::unbounded(), || {
                    Err(CoreError::Storage(
                        "restart rehydrate must have promoted the semantic handle".into(),
                    ))
                })?;
        if semantic.handle.manifest_digest() != "manifest-rehydrate-17" {
            return Err("the resident semantic handle is not the proven one".into());
        }
        let stats = promotion.snapshots.lexical.stats()?;
        if stats.promotions != 1 || stats.misses != 0 || stats.hits != 1 {
            return Err(format!("restart promotion stats drifted: {stats:?}").into());
        }
        Ok(())
    }

    /// A content defect a door proves while activation or rollback picks
    /// a generation is handed to that track's adapter and to no other
    /// (QI-BB-026).
    ///
    /// The refusal keeps its gate's code, carries the door's verdict, and
    /// says where the adapter recorded the quarantine; nothing is promoted.
    #[test]
    fn activation_and_rollback_hand_a_content_defect_to_the_owning_track() -> TestResult {
        let candidate = active_generation()?;
        for gate in [ACTIVATION_GATE, ROLLBACK_GATE] {
            for (track, target) in [
                (SearchPlaneTrackKind::Lexical, candidate.lexical()),
                (SearchPlaneTrackKind::Semantic, candidate.semantic()),
            ] {
                let (promotion, lexical, semantic) = promotion_refusing(
                    Some((track, DoorRefusal::ContentDefect)),
                    ScriptedFinding::Quarantines,
                );
                let (code, message) = refused_proof(&promotion, gate)?;
                assert_eq!(code, gate.error_code);
                let (owner, other) = if track == SearchPlaneTrackKind::Lexical {
                    (lexical.asked()?, semantic.asked()?)
                } else {
                    (semantic.asked()?, lexical.asked()?)
                };
                assert_eq!(owner, vec![target.clone()], "{} {track:?}", gate.operation);
                assert!(other.is_empty(), "the other track was asked: {other:?}");
                let recorded = format!(
                    "; quarantined as GENERATION_QUARANTINE_CONTENT_CORRUPT at {}",
                    scripted_quarantine_path(target).display()
                );
                assert!(
                    message.contains("GENERATION_SIDECAR_CORRUPT")
                        && message.contains(gate.operation)
                        && message.contains(&recorded),
                    "{message}"
                );
            }
        }
        Ok(())
    }

    /// Restart proves the serve head and fails closed.
    ///
    /// Its content defect is refused without asking any adapter to record
    /// it. A door that refused for another reason — the generation missing,
    /// or quarantined already — gave no content verdict, and no gate records
    /// one.
    #[test]
    fn restart_and_refusals_that_are_no_content_verdict_record_nothing() -> TestResult {
        for (gate, refusal) in [
            (RESTART_GATE, DoorRefusal::ContentDefect),
            (ACTIVATION_GATE, DoorRefusal::Missing),
            (ROLLBACK_GATE, DoorRefusal::Quarantined),
        ] {
            for track in [
                SearchPlaneTrackKind::Lexical,
                SearchPlaneTrackKind::Semantic,
            ] {
                let (promotion, lexical, semantic) =
                    promotion_refusing(Some((track, refusal)), ScriptedFinding::Quarantines);
                let (code, message) = refused_proof(&promotion, gate)?;
                assert_eq!(code, gate.error_code);
                assert!(
                    lexical.asked()?.is_empty() && semantic.asked()?.is_empty(),
                    "{} over {refusal:?} on {track:?} asked an adapter to record it",
                    gate.operation
                );
                assert!(!message.contains("; quarantined as"), "{message}");
            }
        }
        Ok(())
    }

    /// The refusal says what the adapter's re-proof did when it recorded
    /// nothing: the finding did not reproduce, or the re-proof failed. The
    /// gate is refused the same way either way.
    #[test]
    fn a_refusal_says_what_the_re_proof_did() -> TestResult {
        for (findings, says) in [
            (
                ScriptedFinding::DoesNotReproduce,
                "the re-proof admitted the generation; nothing was quarantined",
            ),
            (
                ScriptedFinding::Fails,
                "the quarantine was not recorded: not found: scripted: the generation directory is gone",
            ),
        ] {
            let (promotion, lexical, semantic) = promotion_refusing(
                Some((SearchPlaneTrackKind::Lexical, DoorRefusal::ContentDefect)),
                findings,
            );
            let (code, message) = refused_proof(&promotion, ROLLBACK_GATE)?;
            assert_eq!(code, ERR_ROLLBACK_TARGET_UNOPENABLE);
            assert!(
                message.contains("GENERATION_SIDECAR_CORRUPT") && message.contains(says),
                "{message}"
            );
            assert_eq!(lexical.asked()?.len(), 1);
            assert!(semantic.asked()?.is_empty());
        }
        Ok(())
    }

    #[test]
    fn separate_state_roots_rehydrate_same_repo_without_cross_root_aliasing_v1() -> TestResult {
        let state_root_a = tempdir()?;
        let state_root_b = tempdir()?;
        let owner_a =
            SearchCorpusLifecycleOwner::open(state_root_a.path(), retention()?, scripted_bytes())?;
        let owner_b =
            SearchCorpusLifecycleOwner::open(state_root_b.path(), retention()?, scripted_bytes())?;
        activate_generation(
            &owner_a,
            active_generation_for("shared-repo", 17, "manifest-root-a-17")?,
        )?;
        activate_generation(
            &owner_b,
            active_generation_for("shared-repo", 23, "manifest-root-b-23")?,
        )?;
        drop((owner_a, owner_b));

        let owner_a =
            SearchCorpusLifecycleOwner::open(state_root_a.path(), retention()?, scripted_bytes())?;
        let owner_b =
            SearchCorpusLifecycleOwner::open(state_root_b.path(), retention()?, scripted_bytes())?;
        let active_a = owner_a
            .activation_catalog()
            .all_active_search_corpora_for_bootstrap_v1()?;
        let active_b = owner_b
            .activation_catalog()
            .all_active_search_corpora_for_bootstrap_v1()?;
        let [active_a] = active_a.as_slice() else {
            return Err(format!("expected one active root A, observed {active_a:?}").into());
        };
        let [active_b] = active_b.as_slice() else {
            return Err(format!("expected one active root B, observed {active_b:?}").into());
        };
        assert_eq!(active_a.lexical().manifest_generation.get(), 17);
        assert_eq!(active_b.lexical().manifest_generation.get(), 23);
        assert_ne!(
            active_a.lexical().manifest_digest,
            active_b.lexical().manifest_digest
        );
        Ok(())
    }
}
