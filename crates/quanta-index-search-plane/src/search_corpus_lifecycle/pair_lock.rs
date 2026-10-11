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

use quanta_index_contract::{GenerationSnapshot, ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{
    CoreError, DiagnosticContextV1, DiagnosticLaneV1, DiagnosticSpanGuardV1, DiagnosticStageV1,
    DiagnosticTerminalV1,
};

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
        self.lock_pair_observed(repo_id, revision_id, None)
    }

    pub(crate) fn lock_pair_observed<'a>(
        &'a self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SearchCorpusPairMutationGuard<'a>, CoreError> {
        let stripe = search_corpus_lock_stripe_v1(repo_id, revision_id);
        let lock = self.pair_locks.get(stripe).ok_or_else(|| {
            CoreError::Storage(format!(
                "search-corpus lifecycle: computed pair-lock stripe {stripe} outside configured range"
            ))
        })?;
        let wait = diagnostic.and_then(|context| {
            context
                .enter(
                    DiagnosticStageV1::RetentionPairLockWait,
                    DiagnosticLaneV1(1),
                )
                .map_or_else(
                    |trace_error| {
                        context.health(match trace_error {
                            quanta_index_core::DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                                quanta_index_core::DiagnosticHealthCodeV1::CounterOverflow
                            }
                            quanta_index_core::DiagnosticTraceErrorV1::ClockRegressed => {
                                quanta_index_core::DiagnosticHealthCodeV1::ClockRegressed
                            }
                            quanta_index_core::DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                                quanta_index_core::DiagnosticHealthCodeV1::ForeignCaptureSpan
                            }
                            quanta_index_core::DiagnosticTraceErrorV1::MissingParentSpan => {
                                quanta_index_core::DiagnosticHealthCodeV1::MissingParentSpan
                            }
                        });
                        None
                    },
                    Some,
                )
        });
        let guard = lock.lock().map_err(|error| {
            CoreError::Storage(format!(
                "search-corpus lifecycle: pair-lock stripe {stripe} poisoned: {error}"
            ))
        });
        if let Some(wait) = wait {
            let _trace_completion = wait.finish(DiagnosticTerminalV1::from_result(&guard));
        }
        let guard = guard?;
        let hold = diagnostic.and_then(|context| {
            context
                .enter(
                    DiagnosticStageV1::RetentionPairLockHold,
                    DiagnosticLaneV1(1),
                )
                .map_or_else(
                    |trace_error| {
                        context.health(match trace_error {
                            quanta_index_core::DiagnosticTraceErrorV1::SpanIdentityExhausted => {
                                quanta_index_core::DiagnosticHealthCodeV1::CounterOverflow
                            }
                            quanta_index_core::DiagnosticTraceErrorV1::ClockRegressed => {
                                quanta_index_core::DiagnosticHealthCodeV1::ClockRegressed
                            }
                            quanta_index_core::DiagnosticTraceErrorV1::ForeignCaptureSpan => {
                                quanta_index_core::DiagnosticHealthCodeV1::ForeignCaptureSpan
                            }
                            quanta_index_core::DiagnosticTraceErrorV1::MissingParentSpan => {
                                quanta_index_core::DiagnosticHealthCodeV1::MissingParentSpan
                            }
                        });
                        None
                    },
                    Some,
                )
        });
        Ok(SearchCorpusPairMutationGuard {
            coordinator: self,
            stripe,
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            guard: Some(guard),
            hold,
        })
    }
}

pub(crate) struct SearchCorpusPairMutationGuard<'a> {
    coordinator: &'a SearchCorpusPairMutationCoordinator,
    stripe: usize,
    repo_id: RepoId,
    revision_id: RevisionId,
    guard: Option<MutexGuard<'a, ()>>,
    hold: Option<DiagnosticSpanGuardV1>,
}

impl Drop for SearchCorpusPairMutationGuard<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        if let Some(hold) = self.hold.take() {
            let _trace_completion = hold.finish(DiagnosticTerminalV1::Ok);
        }
    }
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

/// An unresolved source event occupies its generation before a manifest
/// digest exists.
///
/// Only a bound event can be compared to an exact sealed
/// lexical identity; both forms protect the generation from reclamation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UnresolvedSourceTargetV1 {
    Allocated(ManifestGeneration),
    Bound(GenerationSnapshot),
}

impl UnresolvedSourceTargetV1 {
    pub(crate) fn generation(&self) -> ManifestGeneration {
        match self {
            Self::Allocated(generation) => *generation,
            Self::Bound(target) => target.manifest_generation,
        }
    }
}

pub(crate) trait ActiveSearchCorpusPinReadPort: std::fmt::Debug + Send + Sync {
    fn active_search_corpus_under_guard_v1(
        &self,
        guard: &SearchCorpusPairMutationGuard<'_>,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Option<SearchCorpusGenerationV1>, CoreError>;

    /// Original source publications that still own an Allocated, Pending or
    /// Staged stream slot. Retention must protect the generation before the
    /// final digest exists, then compare the exact bound identity.
    fn unresolved_source_targets_for_pair_v1(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
    ) -> Result<Vec<UnresolvedSourceTargetV1>, CoreError>;

    fn all_active_search_corpora_for_bootstrap_v1(
        &self,
    ) -> Result<Vec<SearchCorpusGenerationV1>, CoreError>;
}
