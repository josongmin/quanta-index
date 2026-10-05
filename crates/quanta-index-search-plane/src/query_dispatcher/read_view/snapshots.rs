//! Resident opened-generation acquisition through the snapshot registries,
//! with the hit / coalesced / cold-open metrics.
//!
//! Private to the read view: a route acquires a track handle only as a
//! declared domain of its view, never here.

use std::sync::Arc;

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{CoreError, LexicalSearcher, RequestBudgetV1, SemanticSearcher};
use quanta_index_lq_obs::{Dimensions, MetricKind, MetricSample};

use crate::query_dispatcher::dispatcher::SearchPlaneDispatcher;
use crate::{SnapshotAcquireOutcome, SnapshotKey};

impl SearchPlaneDispatcher {
    /// Acquire the shared lexical handle for a pinned sealed generation.
    ///
    /// Goes through the snapshot registry: a resident handle is returned
    /// without touching disk, a miss runs the adapter's cold open once and
    /// concurrent misses wait for it under their own `budget`. The outcome
    /// is emitted as a metric under the pin's dimensions.
    pub(super) fn acquire_lexical(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        budget: &RequestBudgetV1,
    ) -> Result<Arc<dyn LexicalSearcher>, CoreError> {
        #[cfg(feature = "test-runtime-barriers")]
        crate::test_runtime_barriers::record_lexical_view_acquire_attempt();
        let key = SnapshotKey::new(repo_id, revision_id, generation);
        let acquired = self.snapshots.lexical.acquire(&key, budget, || {
            let handle: Arc<dyn LexicalSearcher> =
                Arc::from(
                    self.lex_opener
                        .open(repo_id, revision_id, generation, budget)?,
                );
            let resident_bytes = handle.resident_bytes_estimate();
            Ok(crate::OpenedSnapshot {
                handle,
                resident_bytes,
            })
        })?;
        self.emit_snapshot_metric("lexical", &key, acquired.outcome);
        Ok(acquired.handle)
    }

    /// Semantic counterpart of [`Self::acquire_lexical`].
    pub(super) fn acquire_semantic(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generation: ManifestGeneration,
        budget: &RequestBudgetV1,
    ) -> Result<Arc<dyn SemanticSearcher>, CoreError> {
        let key = SnapshotKey::new(repo_id, revision_id, generation);
        let acquired = self.snapshots.semantic.acquire(&key, budget, || {
            let handle: Arc<dyn SemanticSearcher> =
                Arc::from(self.sem_opener.open(repo_id, revision_id, generation)?);
            let resident_bytes = handle.resident_bytes_estimate();
            Ok(crate::OpenedSnapshot {
                handle,
                resident_bytes,
            })
        })?;
        self.emit_snapshot_metric("semantic", &key, acquired.outcome);
        Ok(acquired.handle)
    }

    fn emit_snapshot_metric(
        &self,
        track: &'static str,
        key: &SnapshotKey,
        outcome: SnapshotAcquireOutcome,
    ) {
        let dimensions = Dimensions::new(
            "LXE-10",
            "8",
            "local",
            key.repo_id.as_str(),
            key.generation.get(),
        );
        let (name, kind, value) = match outcome {
            SnapshotAcquireOutcome::Hit => match track {
                "lexical" => ("lq_snapshot_lexical_hit_total", MetricKind::Counter, 1.0),
                _ => ("lq_snapshot_semantic_hit_total", MetricKind::Counter, 1.0),
            },
            SnapshotAcquireOutcome::Coalesced => match track {
                "lexical" => (
                    "lq_snapshot_lexical_coalesced_total",
                    MetricKind::Counter,
                    1.0,
                ),
                _ => (
                    "lq_snapshot_semantic_coalesced_total",
                    MetricKind::Counter,
                    1.0,
                ),
            },
            SnapshotAcquireOutcome::Miss { cold_open_nanos } => {
                let millis = u64::try_from(cold_open_nanos.div_euclid(1_000_000))
                    .map_or(f64::MAX, |value| {
                        u32::try_from(value).map_or(f64::MAX, f64::from)
                    });
                match track {
                    "lexical" => (
                        "lq_snapshot_lexical_cold_open_ms",
                        MetricKind::Histogram,
                        millis,
                    ),
                    _ => (
                        "lq_snapshot_semantic_cold_open_ms",
                        MetricKind::Histogram,
                        millis,
                    ),
                }
            }
        };
        self.obs_sink
            .emit(MetricSample::new(name, kind, value, dimensions));
    }
}
