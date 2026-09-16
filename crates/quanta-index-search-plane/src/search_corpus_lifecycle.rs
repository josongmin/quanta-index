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
use quanta_index_core::{CoreError, GenerationIdentityValidatePort};

use crate::readiness::{SEARCH_CORPUS_LOCK_STRIPES_V1, search_corpus_lock_stripe_v1};
use crate::{
    ActivationCatalog, AuxiliaryAuthorityStore, Ledger, PreparedSearchCorpusGenerationV1,
    SearchCorpusGenerationActivationV1, SearchCorpusGenerationV1,
};

const ERR_ACTIVATION_TARGET_UNOPENABLE: &str = "ACTIVATION_TARGET_UNOPENABLE";
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
    pub fn open(
        state_root: impl AsRef<Path>,
        retention: crate::readiness::SearchCorpusHistoryRetentionPolicyV1,
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

    /// Revalidate every rehydrated active composite against both physical
    /// generation owners before the runtime publishes any serving socket.
    /// Prove every active `(lexical, semantic)` pair physically, once, and
    /// report how many were proven. This is boot's only deep validation
    /// (QI-BB-026): a defective active pair fails boot with a typed cause
    /// before any socket binds; inactive generations are not examined here.
    pub fn validate_rehydrated_active_generations_v1(
        &self,
        lexical_generation_validator: &dyn GenerationIdentityValidatePort,
        semantic_generation_validator: &dyn GenerationIdentityValidatePort,
    ) -> Result<usize, CoreError> {
        let active_pairs = self
            .activation_catalog
            .all_active_search_corpora_for_bootstrap_v1()?;
        for active in &active_pairs {
            validate_physical_pair_v1(
                lexical_generation_validator,
                semantic_generation_validator,
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

pub(crate) struct SearchCorpusLifecycleService {
    coordinator: Arc<SearchCorpusPairMutationCoordinator>,
    activation_catalog: Arc<ActivationCatalog>,
    ledger: Arc<RwLock<Ledger>>,
    lexical_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    semantic_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
}

impl SearchCorpusLifecycleService {
    pub(crate) fn new(
        coordinator: Arc<SearchCorpusPairMutationCoordinator>,
        activation_catalog: Arc<ActivationCatalog>,
        ledger: Arc<RwLock<Ledger>>,
        lexical_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
        semantic_generation_validator: Arc<dyn GenerationIdentityValidatePort + Send + Sync>,
    ) -> Self {
        Self {
            coordinator,
            activation_catalog,
            ledger,
            lexical_generation_validator,
            semantic_generation_validator,
        }
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "the readiness read guard must remain held until the durable activation CAS commits"
    )]
    pub(crate) fn activate_prepared_v1(
        &self,
        prepared: &PreparedSearchCorpusGenerationV1,
    ) -> Result<SearchCorpusGenerationActivationV1, CoreError> {
        let candidate = prepared.candidate();
        let pair_guard = self
            .coordinator
            .lock_pair(candidate.repo_id(), candidate.revision_id())?;
        let ledger = self
            .ledger
            .read()
            .map_err(|error| CoreError::Storage(format!("ledger poisoned: {error}")))?;
        validate_currently_sealed_candidate_v1(&ledger, candidate.lexical())?;
        validate_currently_sealed_candidate_v1(&ledger, candidate.semantic())?;
        ledger.validate_historically_sealed_track_identity(
            candidate.lexical(),
            "search-corpus activation",
        )?;
        ledger.validate_historically_sealed_track_identity(
            candidate.semantic(),
            "search-corpus activation",
        )?;
        self.validate_physical_pair_v1(candidate, ERR_ACTIVATION_TARGET_UNOPENABLE, "activation")?;
        self.activation_catalog
            .activate_prepared_under_guard_v1(&pair_guard, prepared)
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "the history read guard must remain held until the durable rollback CAS commits"
    )]
    pub(crate) fn rollback_v1(
        &self,
        request: &SearchPlaneRollbackSearchCorpusGenerationCasRequest,
        target: &SearchCorpusGenerationV1,
    ) -> Result<SearchPlaneSearchCorpusRollbackCasAck, CoreError> {
        let pair_guard = self
            .coordinator
            .lock_pair(target.repo_id(), target.revision_id())?;
        let ledger = self
            .ledger
            .read()
            .map_err(|error| CoreError::Storage(format!("ledger poisoned: {error}")))?;
        ledger.validate_historically_sealed_track_identity(
            target.lexical(),
            "search-corpus rollback",
        )?;
        ledger.validate_historically_sealed_track_identity(
            target.semantic(),
            "search-corpus rollback",
        )?;
        self.validate_physical_pair_v1(target, ERR_ROLLBACK_TARGET_UNOPENABLE, "rollback")?;
        self.activation_catalog
            .rollback_under_guard_v1(&pair_guard, request)
    }

    fn validate_physical_pair_v1(
        &self,
        candidate: &SearchCorpusGenerationV1,
        error_code: &str,
        operation: &str,
    ) -> Result<(), CoreError> {
        validate_physical_pair_v1(
            self.lexical_generation_validator.as_ref(),
            self.semantic_generation_validator.as_ref(),
            candidate,
            error_code,
            operation,
        )
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

fn validate_physical_pair_v1(
    lexical_generation_validator: &dyn GenerationIdentityValidatePort,
    semantic_generation_validator: &dyn GenerationIdentityValidatePort,
    candidate: &SearchCorpusGenerationV1,
    error_code: &str,
    operation: &str,
) -> Result<(), CoreError> {
    lexical_generation_validator
        .validate_generation_identity(candidate.lexical())
        .map_err(|source| {
            generation_target_unopenable(candidate.lexical(), operation, error_code, &source)
        })?;
    semantic_generation_validator
        .validate_generation_identity(candidate.semantic())
        .map_err(|source| {
            generation_target_unopenable(candidate.semantic(), operation, error_code, &source)
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

    use super::{
        ActiveSearchCorpusPinReadPort, ERR_ACTIVATION_TARGET_UNOPENABLE,
        SearchCorpusLifecycleOwner, SearchCorpusPairMutationCoordinator,
    };
    use crate::readiness::SearchCorpusHistoryRetentionPolicyV1;
    use crate::{PreparedSearchCorpusGenerationV1, SearchCorpusGenerationV1};
    use quanta_index_core::{CoreError, GenerationIdentityValidatePort};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    struct RejectTrackGeneration {
        rejected_track: Option<SearchPlaneTrackKind>,
    }

    impl GenerationIdentityValidatePort for RejectTrackGeneration {
        fn validate_generation_identity(
            &self,
            candidate: &GenerationSnapshot,
        ) -> Result<(), CoreError> {
            if self.rejected_track == Some(candidate.track) {
                return Err(CoreError::NotFound(format!(
                    "injected missing {:?} generation",
                    candidate.track
                )));
            }
            Ok(())
        }
    }

    fn retention() -> Result<SearchCorpusHistoryRetentionPolicyV1, CoreError> {
        SearchCorpusHistoryRetentionPolicyV1::new(2, 1024 * 1024, 8, 8 * 1024 * 1024)
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
        let owner = SearchCorpusLifecycleOwner::open(state_root, retention()?)?;
        activate(&owner)?;
        drop(owner);
        let owner = SearchCorpusLifecycleOwner::open(state_root, retention()?)?;
        let lexical = RejectTrackGeneration {
            rejected_track: (rejected_track == SearchPlaneTrackKind::Lexical)
                .then_some(SearchPlaneTrackKind::Lexical),
        };
        let semantic = RejectTrackGeneration {
            rejected_track: (rejected_track == SearchPlaneTrackKind::Semantic)
                .then_some(SearchPlaneTrackKind::Semantic),
        };
        let Err(CoreError::Typed { code, message }) =
            owner.validate_rehydrated_active_generations_v1(&lexical, &semantic)
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
        let _owner = SearchCorpusLifecycleOwner::open(state_root.path(), retention()?)?;
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
        let owner = SearchCorpusLifecycleOwner::open(owned_root.path(), retention()?)?;

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

            let result = SearchCorpusLifecycleOwner::open(state_root.path(), retention()?);
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
        let owner = SearchCorpusLifecycleOwner::open(state_root.path(), retention()?)?;
        activate(&owner)?;
        drop(owner);
        let owner = SearchCorpusLifecycleOwner::open(state_root.path(), retention()?)?;
        let valid = RejectTrackGeneration {
            rejected_track: None,
        };
        let validated = owner.validate_rehydrated_active_generations_v1(&valid, &valid)?;
        if validated != 1 {
            return Err(format!("expected exactly one active pair proven, got {validated}").into());
        }
        Ok(())
    }

    #[test]
    fn separate_state_roots_rehydrate_same_repo_without_cross_root_aliasing_v1() -> TestResult {
        let state_root_a = tempdir()?;
        let state_root_b = tempdir()?;
        let owner_a = SearchCorpusLifecycleOwner::open(state_root_a.path(), retention()?)?;
        let owner_b = SearchCorpusLifecycleOwner::open(state_root_b.path(), retention()?)?;
        activate_generation(
            &owner_a,
            active_generation_for("shared-repo", 17, "manifest-root-a-17")?,
        )?;
        activate_generation(
            &owner_b,
            active_generation_for("shared-repo", 23, "manifest-root-b-23")?,
        )?;
        drop((owner_a, owner_b));

        let owner_a = SearchCorpusLifecycleOwner::open(state_root_a.path(), retention()?)?;
        let owner_b = SearchCorpusLifecycleOwner::open(state_root_b.path(), retention()?)?;
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
