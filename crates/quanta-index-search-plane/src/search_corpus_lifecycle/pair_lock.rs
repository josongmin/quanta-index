//! The one pair-local mutation lock of the search corpus, owned by the
//! lifecycle, and the read of a pair's active generation that may only
//! happen under it.
//!
//! Every durable history-retention and composite activation mutation for
//! one repo/revision pair takes the pair's stripe here. The stores that hold
//! that state (the activation catalog, the auxiliary authority store) check
//! the guard they are handed; the lifecycle owner takes it. The lock is a
//! leaf the stores can name without depending on the lifecycle service
//! that depends on them.

use std::sync::{Arc, Mutex, MutexGuard};

use quanta_index_contract::{GenerationSnapshot, RepoId, RevisionId};
use quanta_index_core::CoreError;

use crate::readiness::{
    SEARCH_CORPUS_LOCK_STRIPES_V1, SearchCorpusGenerationV1, search_corpus_lock_stripe_v1,
};

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

pub(crate) trait ActiveSearchCorpusPinReadPort: std::fmt::Debug + Send + Sync {
    fn active_search_corpus_under_guard_v1(
        &self,
        guard: &SearchCorpusPairMutationGuard<'_>,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Option<SearchCorpusGenerationV1>, CoreError>;

    /// Original source publications that still own a Pending/Staged stream
    /// slot. Retention must not reclaim their target before journal recovery.
    fn unresolved_source_targets_for_pair_v1(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<GenerationSnapshot>, CoreError>;

    fn all_active_search_corpora_for_bootstrap_v1(
        &self,
    ) -> Result<Vec<SearchCorpusGenerationV1>, CoreError>;
}
