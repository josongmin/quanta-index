//! Canonical search-corpus lifecycle mutation authority.
//!
//! Physical publication may span a wider ingest operation, but every durable
//! history-retention and composite activation mutation for one repo/revision
//! pair is serialized here.  Lower storage owners must not introduce another
//! pair-local mutation lock.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use quanta_index_contract::{
    GenerationSnapshot, RepoId, RevisionId, SearchPlaneRollbackSearchCorpusGenerationCasRequest,
    SearchPlaneSearchCorpusRollbackCasAck,
};
use quanta_index_core::{
    CoreError, LexicalIndexOpenPort, LexicalSearcher, SemanticIndexOpenPort, SemanticSearcher,
};

use crate::ingest_dispatcher::SearchCorpusAuthorityInspectPort;
use crate::readiness::{
    ERR_SEARCH_TRACK_GENERATION_NOT_SEALED, SEARCH_CORPUS_LOCK_STRIPES_V1,
    search_corpus_lock_stripe_v1,
};
use crate::search_corpus_retention::SearchCorpusIndexBytesPort;
use crate::{
    ActivationCatalog, AuxiliaryAuthorityStore, Ledger, OpenedSnapshot,
    PreparedSearchCorpusGenerationV1, SealedSearchCorpusAuthorityStateV1,
    SearchCorpusGenerationActivationV1, SearchCorpusGenerationV1, SnapshotKey, SnapshotRegistries,
};

pub(crate) const ERR_ACTIVATION_TARGET_UNOPENABLE: &str = "ACTIVATION_TARGET_UNOPENABLE";
const ERR_ROLLBACK_TARGET_UNOPENABLE: &str = "ROLLBACK_TARGET_UNOPENABLE";

#[derive(Debug)]
pub(crate) struct SearchCorpusPairMutationCoordinator {
    pair_locks: [Mutex<()>; SEARCH_CORPUS_LOCK_STRIPES_V1],
}

impl SearchCorpusPairMutationCoordinator {
    #[must_use]
    pub(crate) fn shared() -> Arc<Self> {
        Arc::new(Self {
            pair_locks: std::array::from_fn(|_index| Mutex::new(())),
        })
    }

    pub(crate) fn lock_pair<'a>(
        &'a self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<SearchCorpusPairMutationGuard<'a>, CoreError> {
        let stripe = search_corpus_lock_stripe_v1(repo_id, revision_id);
        let lock = self.pair_locks.get(stripe).ok_or_else(|| {
            CoreError::Storage(format!(
                "search-corpus lifecycle: computed pair-lock stripe {stripe} outside configured range"
            ))
        })?;
        let guard = lock.lock().map_err(|error| {
            CoreError::Storage(format!(
                "search-corpus lifecycle: pair-lock stripe {stripe} poisoned: {error}"
            ))
        })?;
        Ok(SearchCorpusPairMutationGuard {
            coordinator: self,
            stripe,
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            _guard: guard,
        })
    }
}

pub(crate) struct SearchCorpusPairMutationGuard<'a> {
    coordinator: &'a SearchCorpusPairMutationCoordinator,
    stripe: usize,
    repo_id: RepoId,
    revision_id: RevisionId,
    _guard: MutexGuard<'a, ()>,
}

impl SearchCorpusPairMutationGuard<'_> {
    pub(crate) fn require_pair_v1(
        &self,
        expected: &SearchCorpusPairMutationCoordinator,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<(), CoreError> {
        if !std::ptr::eq(self.coordinator, expected) {
            return Err(CoreError::Storage(
                "search-corpus lifecycle: mutation guard belongs to a different coordinator"
                    .to_string(),
            ));
        }
        if self.repo_id != *repo_id || self.revision_id != *revision_id {
            return Err(CoreError::Storage(format!(
                "search-corpus lifecycle: mutation guard pair identity mismatch: protected repo={} revision={}, requested repo={} revision={}",
                self.repo_id.as_str(),
                self.revision_id.as_str(),
                repo_id.as_str(),
                revision_id.as_str(),
            )));
        }
        let expected_stripe = search_corpus_lock_stripe_v1(repo_id, revision_id);
        if self.stripe != expected_stripe {
            return Err(CoreError::Storage(format!(
                "search-corpus lifecycle: mutation guard stripe {} does not protect requested stripe {expected_stripe}",
                self.stripe,
            )));
        }
        Ok(())
    }
}

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
            promotion.prove_and_promote_pair(
                active,
                ERR_ACTIVATION_TARGET_UNOPENABLE,
                "restart rehydrate",
            )?;
        }
        Ok(active_pairs.len())
    }
}

pub(crate) trait ActiveSearchCorpusPinReadPort: std::fmt::Debug + Send + Sync {
    fn active_search_corpus_under_guard_v1(
        &self,
        guard: &SearchCorpusPairMutationGuard<'_>,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Option<SearchCorpusGenerationV1>, CoreError>;

    fn all_active_search_corpora_for_bootstrap_v1(
        &self,
    ) -> Result<Vec<SearchCorpusGenerationV1>, CoreError>;
}

/// Where an activation, a rollback or a restart proves a pair and puts the
/// handles it proved: the openers and the registries that keep the handles
/// resident (QI-BB-017 보완 #4).
///
/// The proof and the promotion are one step: [`LexicalIndexOpenPort::open_proven`]
/// and [`SemanticIndexOpenPort::open_proven`] prove the candidate exactly
/// as the activation validator would and return the handle that proof
/// opened, so each track is walked once and the first query is a registry
/// hit, not a second full open.
#[derive(Clone)]
pub struct ActivationPromotionParts {
    pub lexical_open: Arc<dyn LexicalIndexOpenPort + Send + Sync>,
    pub semantic_open: Arc<dyn SemanticIndexOpenPort + Send + Sync>,
    pub snapshots: SnapshotRegistries,
}

impl ActivationPromotionParts {
    /// Prove both tracks of `candidate` by opening them, and promote both
    /// handles into the registries.
    ///
    /// Runs outside every ledger guard: the opens hash the generation's
    /// decoded bytes, and nothing else in the process should wait on that.
    /// A track that does not prove is refused typed under `error_code` and
    /// nothing is promoted for the pair.
    fn prove_and_promote_pair(
        &self,
        candidate: &SearchCorpusGenerationV1,
        error_code: &str,
        operation: &str,
    ) -> Result<(), CoreError> {
        let key = SnapshotKey::new(
            candidate.repo_id(),
            candidate.revision_id(),
            candidate.manifest_generation(),
        );
        let lexical: Arc<dyn LexicalSearcher> = Arc::from(
            self.lexical_open
                .open_proven(candidate.lexical())
                .map_err(|source| {
                    generation_target_unopenable(
                        candidate.lexical(),
                        operation,
                        error_code,
                        &source,
                    )
                })?,
        );
        let semantic: Arc<dyn SemanticSearcher> = Arc::from(
            self.semantic_open
                .open_proven(candidate.semantic())
                .map_err(|source| {
                    generation_target_unopenable(
                        candidate.semantic(),
                        operation,
                        error_code,
                        &source,
                    )
                })?,
        );
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
        Ok(())
    }
}

/// The ports one `SearchCorpusLifecycleService` is composed from.
pub struct SearchCorpusLifecycleParts {
    pub activation_catalog: Arc<ActivationCatalog>,
    pub ledger: Arc<RwLock<Ledger>>,
    /// The durable sealed history, consulted under the pair guard right
    /// before the CAS commits.
    pub authority: Arc<dyn SearchCorpusAuthorityInspectPort + Send + Sync>,
    pub promotion: ActivationPromotionParts,
}

pub(crate) struct SearchCorpusLifecycleService {
    coordinator: Arc<SearchCorpusPairMutationCoordinator>,
    activation_catalog: Arc<ActivationCatalog>,
    ledger: Arc<RwLock<Ledger>>,
    authority: Arc<dyn SearchCorpusAuthorityInspectPort + Send + Sync>,
    promotion: ActivationPromotionParts,
}

impl SearchCorpusLifecycleService {
    pub(crate) fn new(parts: SearchCorpusLifecycleParts) -> Self {
        let SearchCorpusLifecycleParts {
            activation_catalog,
            ledger,
            authority,
            promotion,
        } = parts;
        Self {
            coordinator: activation_catalog.lifecycle_coordinator(),
            activation_catalog,
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
        self.prove_and_promote_v1(candidate, ERR_ACTIVATION_TARGET_UNOPENABLE, "activation")?;
        self.check_activation_preconditions_v1(candidate)?;
        self.require_durably_sealed_v1(candidate, "activation")?;
        self.activation_catalog
            .activate_prepared_under_guard_v1(&pair_guard, prepared)
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
        self.prove_and_promote_v1(target, ERR_ROLLBACK_TARGET_UNOPENABLE, "rollback")?;
        self.check_rollback_preconditions_v1(target)?;
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
                code: ERR_SEARCH_TRACK_GENERATION_NOT_SEALED.to_string(),
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
        error_code: &str,
        operation: &str,
    ) -> Result<(), CoreError> {
        self.promotion
            .prove_and_promote_pair(candidate, error_code, operation)
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
    error_code: &str,
    source: &CoreError,
) -> CoreError {
    CoreError::Typed {
        code: error_code.to_string(),
        message: format!(
            "search-corpus {operation}: target is not physically valid for repo={} revision={} track={:?} generation={}: {source:?}",
            candidate.repo_id.as_str(),
            candidate.revision_id.as_str(),
            candidate.track,
            candidate.manifest_generation.get(),
        ),
    }
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
        ActivationPromotionParts, ActiveSearchCorpusPinReadPort, ERR_ACTIVATION_TARGET_UNOPENABLE,
        SearchCorpusLifecycleOwner, SearchCorpusPairMutationCoordinator,
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

    /// Openers whose handles prove the digest the rehydrate fixture seals
    /// under, `manifest-rehydrate-<generation>`; `open_proven` refuses the
    /// rejected track the way a missing generation would.
    struct EchoLexicalOpener {
        reject: bool,
    }

    fn injected_missing(candidate: &GenerationSnapshot) -> CoreError {
        CoreError::NotFound(format!("injected missing {:?} generation", candidate.track))
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
                manifest_digest: Some(format!("manifest-rehydrate-{}", generation.get())),
            }))
        }

        fn open_proven(
            &self,
            candidate: &GenerationSnapshot,
        ) -> Result<Box<dyn LexicalSearcher>, CoreError> {
            if self.reject {
                return Err(injected_missing(candidate));
            }
            self.open(
                &candidate.repo_id,
                &candidate.revision_id,
                candidate.manifest_generation,
            )
        }
    }

    struct EchoSemanticOpener {
        reject: bool,
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
            if self.reject {
                return Err(injected_missing(candidate));
            }
            self.open(
                &candidate.repo_id,
                &candidate.revision_id,
                candidate.manifest_generation,
            )
        }
    }

    /// Promotion over the echo openers, refusing `rejected_track`'s proof.
    fn promotion_rejecting(
        rejected_track: Option<SearchPlaneTrackKind>,
    ) -> ActivationPromotionParts {
        ActivationPromotionParts {
            lexical_open: Arc::new(EchoLexicalOpener {
                reject: rejected_track == Some(SearchPlaneTrackKind::Lexical),
            }),
            semantic_open: Arc::new(EchoSemanticOpener {
                reject: rejected_track == Some(SearchPlaneTrackKind::Semantic),
            }),
            snapshots: SnapshotRegistries::new(SnapshotRegistryPolicy::DEFAULT),
        }
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
            repo_id: RepoId::new(repo_id),
            revision_id: RevisionId::new("revision-rehydrate"),
            track,
            manifest_generation: ManifestGeneration::new(generation),
            manifest_digest: digest.to_string(),
        };
        SearchCorpusGenerationV1::new(
            snapshot(SearchPlaneTrackKind::Lexical),
            snapshot(SearchPlaneTrackKind::Semantic),
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
        let repo_a = RepoId::new("repo-guard-a");
        let revision = RevisionId::new("revision-guard");
        let stripe_a = crate::readiness::search_corpus_lock_stripe_v1(&repo_a, &revision);
        let repo_b = (0_u64..4096)
            .map(|index| RepoId::new(format!("repo-guard-b-{index}")))
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
            &RepoId::new("repo-rehydrate"),
            &RevisionId::new("revision-rehydrate"),
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
