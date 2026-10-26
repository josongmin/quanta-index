//! Sealed search-corpus history retention: the per-pair and state-root
//! limits, and the plan that decides which sealed generations a pair
//! keeps (QI-BB-003).
//!
//! Byte limits are enforced over the **actual index bytes** the retained
//! generations occupy on disk — private directories and reachable shared
//! objects, deduplicated within the requested pair. Root admission measures
//! the entire physical tracks instead of summing overlapping pair closures;
//! garbage continues to count until it is physically collected. The
//! measurement is a port ([`SearchCorpusIndexBytesPort`]) because the
//! authority store owns no index layout; the composition root wires the
//! adapters' measurement in.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId, SearchPlaneErrorCodeV2};
use quanta_index_core::domains::generation::{
    RetentionMeasurementPolicy, SealedGenerationByteSession,
};
use quanta_index_core::{
    CoreError, DiagnosticContextV1, RequestBudgetV1, SealedGenerationBytesV1,
    SealedGenerationReclaimPort,
};

/// One retention request's exact pair measurement. A failed probe must not
/// change the answer to any later requested set.
pub trait SearchCorpusIndexByteSession {
    fn measure(
        &mut self,
        generations: &BTreeSet<ManifestGeneration>,
    ) -> Result<SealedGenerationBytesV1, CoreError>;
}

struct DirectSearchCorpusIndexByteSession<'a, T: SearchCorpusIndexBytesPort + ?Sized> {
    owner: &'a T,
    repo_id: RepoId,
    revision_id: RevisionId,
    diagnostic: Option<DiagnosticContextV1>,
    budget: RequestBudgetV1,
}

impl<T: SearchCorpusIndexBytesPort + ?Sized> SearchCorpusIndexByteSession
    for DirectSearchCorpusIndexByteSession<'_, T>
{
    fn measure(
        &mut self,
        generations: &BTreeSet<ManifestGeneration>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        pair_checkpoint(&self.budget, "retention-direct-pair:entry")?;
        let result = self.owner.measure_index_bytes(
            &self.repo_id,
            &self.revision_id,
            generations,
            &self.budget,
            self.diagnostic.as_ref(),
        )?;
        pair_checkpoint(&self.budget, "retention-direct-pair:complete")?;
        Ok(result)
    }
}

struct PairIndexByteSession<'a> {
    lexical: Box<dyn SealedGenerationByteSession + 'a>,
    semantic: Box<dyn SealedGenerationByteSession + 'a>,
    budget: RequestBudgetV1,
}

impl SearchCorpusIndexByteSession for PairIndexByteSession<'_> {
    fn measure(
        &mut self,
        generations: &BTreeSet<ManifestGeneration>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        pair_checkpoint(&self.budget, "retention-pair:entry")?;
        let lexical = self
            .lexical
            .measure(generations)
            .map_err(|error| scan_error(&self.budget, error))?;
        pair_checkpoint(&self.budget, "retention-pair:between-tracks")?;
        let semantic = self
            .semantic
            .measure(generations)
            .map_err(|error| scan_error(&self.budget, error))?;
        pair_checkpoint(&self.budget, "retention-pair:complete")?;
        combine_track_bytes(lexical, semantic)
    }
}

fn combine_track_bytes(
    lexical: SealedGenerationBytesV1,
    semantic: SealedGenerationBytesV1,
) -> Result<SealedGenerationBytesV1, CoreError> {
    let bytes = lexical.bytes.checked_add(semantic.bytes).ok_or_else(|| {
        CoreError::Storage("search-corpus history retention: index byte total overflow".into())
    })?;
    let mut absent = lexical.absent;
    absent.extend(semantic.absent);
    Ok(SealedGenerationBytesV1 { bytes, absent })
}

/// Measures the bytes a set of sealed generations of one pair occupies on
/// disk across both tracks, including the unique shared-object closure.
///
/// Called at admission and at restore with the sets retention is deciding
/// between; a generation whose directories are absent contributes nothing.
pub trait SearchCorpusIndexBytesPort: fmt::Debug + Send + Sync {
    /// One configured service policy for a complete authority admission attempt.
    fn measurement_policy(&self) -> RetentionMeasurementPolicy;

    /// Begin one pair-local retention plan's sequence of measurements. The
    /// default calls full-set measurement with the same budget for every probe.
    fn begin_pair_measurement(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        budget: &RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<Box<dyn SearchCorpusIndexByteSession + '_>, CoreError> {
        pair_checkpoint(budget, "retention-direct-pair:begin")?;
        Ok(Box::new(DirectSearchCorpusIndexByteSession {
            owner: self,
            repo_id: repo_id.clone(),
            revision_id: revision_id.clone(),
            diagnostic: diagnostic.cloned(),
            budget: budget.clone(),
        }))
    }

    fn measure_index_bytes(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
        budget: &RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError>;

    /// The physical root-wide disk fence. `None` declares an in-memory
    /// implementation whose owner uses the projected pair sum. Physical
    /// backends return the complete track total, so cross-pair sharing counts
    /// once and pending collection still counts.
    fn measure_state_root_index_bytes(
        &self,
        budget: &RequestBudgetV1,
        _diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<Option<u64>, CoreError> {
        pair_checkpoint(budget, "retention-root:default")?;
        Ok(None)
    }
}

/// The measurement over the two tracks' reclaim ports, which are the
/// adapters that know their generation directories.
pub struct PairIndexBytesMeasurer {
    lexical: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    semantic: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    measurement_policy: RetentionMeasurementPolicy,
}

impl PairIndexBytesMeasurer {
    #[must_use]
    pub fn new(
        lexical: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
        semantic: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    ) -> Self {
        Self {
            lexical,
            semantic,
            measurement_policy: RetentionMeasurementPolicy::DEFAULT,
        }
    }

    #[must_use]
    pub fn with_retention_measurement_policy(mut self, policy: RetentionMeasurementPolicy) -> Self {
        self.measurement_policy = policy;
        self
    }

    fn measure_physical_with_budget(
        &self,
        budget: &RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<u64, CoreError> {
        // Both custody guards remain held until the checked total is known.
        scan_checkpoint(budget)?;
        let lexical_session = self.lexical.begin_track_physical_measurement()?;
        scan_checkpoint(budget)?;
        let semantic_session = self.semantic.begin_track_physical_measurement()?;
        scan_checkpoint(budget)?;
        let lexical = lexical_session
            .measure(budget, diagnostic)
            .map_err(|error| scan_error(budget, error))?;
        scan_checkpoint(budget)?;
        let semantic = semantic_session
            .measure(budget, diagnostic)
            .map_err(|error| scan_error(budget, error))?;
        scan_checkpoint(budget)?;
        lexical.checked_add(semantic).ok_or_else(|| {
            CoreError::Storage(
                "search-corpus history retention: physical track total overflow".into(),
            )
        })
    }
}

fn scan_error(budget: &RequestBudgetV1, error: CoreError) -> CoreError {
    if budget.interruption().is_some()
        && matches!(
            &error,
            CoreError::Typed {
                code: SearchPlaneErrorCodeV2::RequestDeadlineExceeded
                    | SearchPlaneErrorCodeV2::RequestCancelled,
                ..
            }
        )
    {
        return CoreError::Typed {
            code: SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
            message: format!(
                "search-corpus history retention: service retention scan budget exhausted; retry measurement: {error}"
            ),
        };
    }
    error
}

fn scan_checkpoint(budget: &RequestBudgetV1) -> Result<(), CoreError> {
    budget
        .checkpoint("search-corpus-history:physical-scan")
        .map_err(|error| scan_error(budget, error))
}

pub(crate) fn pair_checkpoint(
    budget: &RequestBudgetV1,
    stage: &'static str,
) -> Result<(), CoreError> {
    budget
        .checkpoint(stage)
        .map_err(|error| scan_error(budget, error))
}

impl fmt::Debug for PairIndexBytesMeasurer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairIndexBytesMeasurer")
    }
}

impl SearchCorpusIndexBytesPort for PairIndexBytesMeasurer {
    fn measurement_policy(&self) -> RetentionMeasurementPolicy {
        self.measurement_policy
    }

    fn begin_pair_measurement(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        budget: &RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<Box<dyn SearchCorpusIndexByteSession + '_>, CoreError> {
        pair_checkpoint(budget, "retention-pair:begin")?;
        let lexical = self
            .lexical
            .begin_pair_measurement(repo_id, revision_id, budget.clone(), diagnostic)
            .map_err(|error| scan_error(budget, error))?;
        let semantic = self
            .semantic
            .begin_pair_measurement(repo_id, revision_id, budget.clone(), diagnostic)
            .map_err(|error| scan_error(budget, error))?;
        pair_checkpoint(budget, "retention-pair:ready")?;
        Ok(Box::new(PairIndexByteSession {
            lexical,
            semantic,
            budget: budget.clone(),
        }))
    }

    fn measure_state_root_index_bytes(
        &self,
        budget: &RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<Option<u64>, CoreError> {
        self.measure_physical_with_budget(budget, diagnostic)
            .map(Some)
    }

    fn measure_index_bytes(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
        budget: &RequestBudgetV1,
        diagnostic: Option<&DiagnosticContextV1>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        self.begin_pair_measurement(repo_id, revision_id, budget, diagnostic)?
            .measure(generations)
    }
}

/// Explicit state-root retention limits for sealed search-corpus history.
///
/// At least two generations are required so admitting a new generation cannot
/// erase its immediate predecessor rollback target. `max_bytes` bounds the
/// index bytes one pair's retained generations occupy together;
/// `max_total_bytes` bounds the entire physical index tracks, including shared
/// objects, unfinished publications and garbage awaiting collection. Planning
/// a retirement does not release this physical budget before deletion.
/// Both are measured on disk through [`SearchCorpusIndexBytesPort`].
///
/// The pair-local limits are allowed to reap generations only inside the pair
/// being mutated. The state-root limits are admission fences: without a
/// product-active pin authority this owner must reject growth instead of
/// guessing which other repo/revision pair is safe to delete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_field_names,
    reason = "every field is a ceiling, and the `max_` prefix is what distinguishes a cap from an observed value; `bytes`/`total_bytes` would read as current usage"
)]
pub struct SearchCorpusHistoryRetentionPolicyV1 {
    max_generations: usize,
    max_bytes: u64,
    max_revision_pairs: usize,
    max_total_bytes: u64,
}

impl SearchCorpusHistoryRetentionPolicyV1 {
    pub fn new(
        max_generations: usize,
        max_bytes: u64,
        max_revision_pairs: usize,
        max_total_bytes: u64,
    ) -> Result<Self, CoreError> {
        if max_generations < 2 {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionPolicyInvalid,
                message: format!(
                    "search-corpus history retention: max_generations must be at least 2, observed {max_generations}"
                ),
            });
        }
        if max_bytes == 0 {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionPolicyInvalid,
                message: "search-corpus history retention: max_bytes must be non-zero".to_string(),
            });
        }
        if max_revision_pairs == 0 {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionPolicyInvalid,
                message: "search-corpus history retention: max_revision_pairs must be non-zero"
                    .to_string(),
            });
        }
        if max_total_bytes == 0 {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionPolicyInvalid,
                message: "search-corpus history retention: max_total_bytes must be non-zero"
                    .to_string(),
            });
        }
        Ok(Self {
            max_generations,
            max_bytes,
            max_revision_pairs,
            max_total_bytes,
        })
    }

    #[must_use]
    pub const fn max_generations(self) -> usize {
        self.max_generations
    }

    #[must_use]
    pub const fn max_bytes(self) -> u64 {
        self.max_bytes
    }

    #[must_use]
    pub const fn max_revision_pairs(self) -> usize {
        self.max_revision_pairs
    }

    #[must_use]
    pub const fn max_total_bytes(self) -> u64 {
        self.max_total_bytes
    }

    /// Bootstrap may keep a missing historical record so a later rollback
    /// names its exact unavailable target. Until that record is repaired, a
    /// count-based sweep cannot safely choose another record to delete.
    pub(crate) fn preserve_unreconciled_restore(
        self,
        generations: BTreeSet<ManifestGeneration>,
        measured_bytes: u64,
    ) -> Result<SearchCorpusHistoryRetentionPlanV1, CoreError> {
        if generations.len() > self.max_generations || measured_bytes > self.max_bytes {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
                message: format!(
                    "search-corpus history retention: unreconciled restore needs generations={} index_bytes={}, limits are max_generations={} max_bytes={}; refusing to reap a healthy generation while a retained track is physically absent",
                    generations.len(),
                    measured_bytes,
                    self.max_generations,
                    self.max_bytes,
                ),
            });
        }
        Ok(SearchCorpusHistoryRetentionPlanV1 {
            retained_generations: generations,
            retained_bytes: measured_bytes,
        })
    }

    /// Reuse bootstrap's exact measurement when every observed generation is
    /// present and the complete set fits both pair limits. The caller must
    /// validate active and unresolved pins before using the returned plan.
    pub(crate) fn retain_all_preverified_for_restore(
        self,
        generations: &BTreeSet<ManifestGeneration>,
        measured_bytes: u64,
    ) -> Option<SearchCorpusHistoryRetentionPlanV1> {
        (generations.len() <= self.max_generations && measured_bytes <= self.max_bytes).then(|| {
            SearchCorpusHistoryRetentionPlanV1 {
                retained_generations: generations.clone(),
                retained_bytes: measured_bytes,
            }
        })
    }

    /// Decide what one pair retains.
    ///
    /// The required set — the candidate, active generation, unresolved
    /// source targets and the newest (or, with no active generation, the two newest) — must fit
    /// `max_generations` and `max_bytes` as measured together, else the
    /// admission is refused typed with nothing reaped. Older generations
    /// are then kept newest-first while the retained set, measured as one
    /// inode set, stays within both limits.
    pub(crate) fn plan(
        self,
        mut items: Vec<SearchCorpusHistoryRetentionItemV1>,
        measure: &mut dyn FnMut(&BTreeSet<ManifestGeneration>) -> Result<u64, CoreError>,
    ) -> Result<SearchCorpusHistoryRetentionPlanV1, CoreError> {
        items.sort_by(|left, right| right.generation.cmp(&left.generation));
        for pair in items.windows(2) {
            let [newer, older] = pair else {
                continue;
            };
            if newer.generation == older.generation {
                return Err(CoreError::Storage(format!(
                    "search-corpus history retention: duplicate generation {}",
                    newer.generation.get()
                )));
            }
        }

        let mut required_generations: BTreeSet<ManifestGeneration> = items
            .iter()
            .filter(|item| item.candidate || item.active || item.unresolved_source)
            .map(|item| item.generation)
            .collect();
        let has_active = items.iter().any(|item| item.active);
        if has_active {
            if let Some(newest) = items.first() {
                let _inserted = required_generations.insert(newest.generation);
            }
        } else {
            required_generations.extend(items.iter().take(2).map(|item| item.generation));
        }
        let required_bytes = measure(&required_generations)?;
        if required_generations.len() > self.max_generations || required_bytes > self.max_bytes {
            return Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
                message: format!(
                    "search-corpus history retention: required active/candidate set needs generations={} index_bytes={required_bytes}, limits are max_generations={} max_bytes={}",
                    required_generations.len(),
                    self.max_generations,
                    self.max_bytes,
                ),
            });
        }

        let mut retained_generations = required_generations;
        let mut retained_bytes = required_bytes;
        for item in &items {
            if retained_generations.contains(&item.generation) {
                continue;
            }
            if retained_generations.len() >= self.max_generations {
                break;
            }
            let mut widened = retained_generations.clone();
            let _inserted = widened.insert(item.generation);
            let widened_bytes = measure(&widened)?;
            if widened_bytes > self.max_bytes {
                break;
            }
            retained_generations = widened;
            retained_bytes = widened_bytes;
        }
        Ok(SearchCorpusHistoryRetentionPlanV1 {
            retained_generations,
            retained_bytes,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SearchCorpusHistoryRetentionItemV1 {
    pub(crate) generation: ManifestGeneration,
    pub(crate) candidate: bool,
    pub(crate) active: bool,
    pub(crate) unresolved_source: bool,
}

/// What one pair retains and the index bytes that set was measured at.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct SearchCorpusHistoryRetentionPlanV1 {
    retained_generations: BTreeSet<ManifestGeneration>,
    retained_bytes: u64,
}

impl SearchCorpusHistoryRetentionPlanV1 {
    pub(crate) fn retains(&self, generation: ManifestGeneration) -> bool {
        self.retained_generations.contains(&generation)
    }

    pub(crate) const fn retained_bytes(&self) -> u64 {
        self.retained_bytes
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::panic_in_result_fn,
        reason = "Result-returning retention tests use assertions as test-failure reporting"
    )]

    use std::collections::{BTreeSet, VecDeque};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use quanta_index_contract::{GenerationSnapshot, ManifestGeneration, RepoId, RevisionId};
    use quanta_index_core::domains::generation::{
        FinishedReclaims, RetentionMeasurementPolicy, SealedGenerationByteSession,
        TrackPhysicalByteSession,
    };
    use quanta_index_core::{
        CoreError, DiagnosticContextV1, RequestBudgetV1, SealedGenerationBytesV1,
    };
    use quanta_index_core::{SealedGenerationReclaimOutcomeV1, SealedGenerationReclaimPort};

    use super::{
        PairIndexByteSession, PairIndexBytesMeasurer, SearchCorpusHistoryRetentionItemV1,
        SearchCorpusHistoryRetentionPolicyV1, SearchCorpusIndexByteSession,
        SearchCorpusIndexBytesPort,
    };

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Every generation occupies ten bytes; sets add up. Fallible like
    /// the port it stands in for.
    fn ten_each(set: &BTreeSet<ManifestGeneration>) -> Result<u64, CoreError> {
        if set.len() > 1_000 {
            return Err(CoreError::Storage(
                "the test measure is bounded".to_string(),
            ));
        }
        Ok(10_u64.saturating_mul(u64::try_from(set.len()).map_or(u64::MAX, |len| len)))
    }

    fn g(value: u64) -> ManifestGeneration {
        ManifestGeneration::new(value)
    }

    #[derive(Debug, Default)]
    struct ScriptedPairBytes {
        calls: Mutex<Vec<BTreeSet<ManifestGeneration>>>,
        fail_on_two: bool,
    }

    impl SearchCorpusIndexBytesPort for ScriptedPairBytes {
        fn measurement_policy(&self) -> RetentionMeasurementPolicy {
            RetentionMeasurementPolicy::DEFAULT
        }

        fn measure_index_bytes(
            &self,
            _repo_id: &RepoId,
            _revision_id: &RevisionId,
            generations: &BTreeSet<ManifestGeneration>,
            _budget: &RequestBudgetV1,
            _diagnostic: Option<&DiagnosticContextV1>,
        ) -> Result<SealedGenerationBytesV1, CoreError> {
            self.calls
                .lock()
                .map_err(|error| CoreError::Storage(format!("fixture lock: {error}")))?
                .push(generations.clone());
            if self.fail_on_two && generations.contains(&g(2)) {
                return Err(CoreError::Storage("fixture failed probe".into()));
            }
            let shared = match (generations.contains(&g(4)), generations.contains(&g(3))) {
                (true, true) => 12_u64,
                (true, false) | (false, true) => 10_u64,
                (false, false) => 0_u64,
            };
            let bytes = shared
                .checked_add(if generations.contains(&g(2)) { 10 } else { 0 })
                .ok_or_else(|| CoreError::InvalidContract("fixture bytes overflow".into()))?;
            Ok(SealedGenerationBytesV1 {
                bytes,
                absent: BTreeSet::new(),
            })
        }
    }

    struct ScriptedTrackSession {
        results: VecDeque<Result<SealedGenerationBytesV1, CoreError>>,
    }

    impl SealedGenerationByteSession for ScriptedTrackSession {
        fn measure(
            &mut self,
            _generations: &BTreeSet<ManifestGeneration>,
        ) -> Result<SealedGenerationBytesV1, CoreError> {
            self.results
                .pop_front()
                .ok_or_else(|| CoreError::InvalidContract("track fixture exhausted".into()))?
        }
    }

    fn scripted_track(
        results: Vec<Result<SealedGenerationBytesV1, CoreError>>,
    ) -> Box<dyn SealedGenerationByteSession> {
        Box::new(ScriptedTrackSession {
            results: VecDeque::from(results),
        })
    }

    struct PhysicalSnapshotMock {
        track: &'static str,
        bytes: u64,
        refuse_acquire: bool,
        cancel_budget_on_measure: bool,
        events: Arc<Mutex<Vec<String>>>,
    }

    struct PhysicalSnapshotMockSession {
        track: &'static str,
        bytes: u64,
        cancel_budget_on_measure: bool,
        events: Arc<Mutex<Vec<String>>>,
    }

    struct PairBudgetMockSession {
        track: &'static str,
        bytes: u64,
        budget: RequestBudgetV1,
        cancel_on_measure: bool,
        events: Arc<Mutex<Vec<String>>>,
    }

    impl Drop for PairBudgetMockSession {
        fn drop(&mut self) {
            if let Ok(mut events) = self.events.lock() {
                events.push(format!("drop pair {}", self.track));
            }
        }
    }

    impl SealedGenerationByteSession for PairBudgetMockSession {
        fn measure(
            &mut self,
            generations: &BTreeSet<ManifestGeneration>,
        ) -> Result<SealedGenerationBytesV1, CoreError> {
            self.budget.checkpoint("fixture-pair-measure")?;
            self.events
                .lock()
                .map_err(|error| CoreError::Storage(format!("fixture lock: {error}")))?
                .push(format!("measure pair {}", self.track));
            if self.cancel_on_measure && generations.contains(&g(2)) {
                self.budget.cancel_handle().cancel();
            }
            Ok(SealedGenerationBytesV1 {
                bytes: self.bytes,
                absent: BTreeSet::new(),
            })
        }
    }

    impl Drop for PhysicalSnapshotMockSession {
        fn drop(&mut self) {
            if let Ok(mut events) = self.events.lock() {
                events.push(format!("drop {}", self.track));
            }
        }
    }

    impl TrackPhysicalByteSession for PhysicalSnapshotMockSession {
        fn measure(
            &self,
            budget: &RequestBudgetV1,
            _diagnostic: Option<&DiagnosticContextV1>,
        ) -> Result<u64, CoreError> {
            self.events
                .lock()
                .map_err(|error| CoreError::Storage(format!("fixture lock: {error}")))?
                .push(format!("measure {}", self.track));
            if self.cancel_budget_on_measure {
                budget.cancel_handle().cancel();
            }
            Ok(self.bytes)
        }
    }

    impl SealedGenerationReclaimPort for PhysicalSnapshotMock {
        fn begin_pair_measurement(
            &self,
            _repo_id: &RepoId,
            _revision_id: &RevisionId,
            budget: RequestBudgetV1,
            _diagnostic: Option<&DiagnosticContextV1>,
        ) -> Result<Box<dyn SealedGenerationByteSession + '_>, CoreError> {
            self.events
                .lock()
                .map_err(|error| CoreError::Storage(format!("fixture lock: {error}")))?
                .push(format!("acquire pair {}", self.track));
            Ok(Box::new(PairBudgetMockSession {
                track: self.track,
                bytes: self.bytes,
                budget,
                cancel_on_measure: self.cancel_budget_on_measure,
                events: Arc::clone(&self.events),
            }))
        }

        fn begin_track_physical_measurement(
            &self,
        ) -> Result<Box<dyn TrackPhysicalByteSession + '_>, CoreError> {
            self.events
                .lock()
                .map_err(|error| CoreError::Storage(format!("fixture lock: {error}")))?
                .push(format!("acquire {}", self.track));
            if self.refuse_acquire {
                return Err(CoreError::Storage(format!(
                    "{} snapshot unavailable",
                    self.track
                )));
            }
            Ok(Box::new(PhysicalSnapshotMockSession {
                track: self.track,
                bytes: self.bytes,
                cancel_budget_on_measure: self.cancel_budget_on_measure,
                events: Arc::clone(&self.events),
            }))
        }

        fn reclaim_sealed_generation_with_settlement(
            &self,
            _retired: &GenerationSnapshot,
            _on_absent: &dyn Fn() -> Result<(), CoreError>,
            _diagnostic: Option<&DiagnosticContextV1>,
        ) -> Result<SealedGenerationReclaimOutcomeV1, CoreError> {
            Err(CoreError::InvalidContract("fixture reclaim unused".into()))
        }

        fn sealed_generations_for_pair(
            &self,
            _repo_id: &RepoId,
            _revision_id: &RevisionId,
            _diagnostic: Option<&DiagnosticContextV1>,
        ) -> Result<Vec<GenerationSnapshot>, CoreError> {
            Err(CoreError::InvalidContract(
                "fixture inventory unused".into(),
            ))
        }

        fn measure_sealed_generations(
            &self,
            _repo_id: &RepoId,
            _revision_id: &RevisionId,
            _generations: &BTreeSet<ManifestGeneration>,
            _diagnostic: Option<&DiagnosticContextV1>,
        ) -> Result<SealedGenerationBytesV1, CoreError> {
            Err(CoreError::InvalidContract(
                "fixture pair measure unused".into(),
            ))
        }

        fn finish_interrupted_reclaims(
            &self,
            _diagnostic: Option<&DiagnosticContextV1>,
        ) -> Result<FinishedReclaims, CoreError> {
            Err(CoreError::InvalidContract(
                "fixture reclaim sweep unused".into(),
            ))
        }
    }

    #[test]
    fn physical_pair_acquires_both_snapshots_before_measuring() -> TestResult {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mock = |track, bytes| PhysicalSnapshotMock {
            track,
            bytes,
            refuse_acquire: false,
            cancel_budget_on_measure: false,
            events: Arc::clone(&events),
        };
        let pair = PairIndexBytesMeasurer::new(
            Arc::new(mock("lexical", 7)),
            Arc::new(mock("semantic", 11)),
        );
        assert_eq!(
            pair.measure_state_root_index_bytes(
                &pair.measurement_policy().physical_scan_budget()?,
                None
            )?,
            Some(18)
        );
        assert_eq!(
            *events
                .lock()
                .map_err(|error| format!("fixture lock: {error}"))?,
            [
                "acquire lexical",
                "acquire semantic",
                "measure lexical",
                "measure semantic",
                "drop semantic",
                "drop lexical",
            ]
        );
        Ok(())
    }

    #[test]
    fn physical_pair_releases_first_snapshot_on_second_refusal() -> TestResult {
        let events = Arc::new(Mutex::new(Vec::new()));
        let pair = PairIndexBytesMeasurer::new(
            Arc::new(PhysicalSnapshotMock {
                track: "lexical",
                bytes: 7,
                refuse_acquire: false,
                cancel_budget_on_measure: false,
                events: Arc::clone(&events),
            }),
            Arc::new(PhysicalSnapshotMock {
                track: "semantic",
                bytes: 11,
                refuse_acquire: true,
                cancel_budget_on_measure: false,
                events: Arc::clone(&events),
            }),
        );
        assert!(matches!(
            pair.measure_state_root_index_bytes(&pair.measurement_policy().physical_scan_budget()?, None),
            Err(CoreError::Storage(message)) if message == "semantic snapshot unavailable"
        ));
        assert_eq!(
            *events
                .lock()
                .map_err(|error| format!("fixture lock: {error}"))?,
            ["acquire lexical", "acquire semantic", "drop lexical"]
        );
        Ok(())
    }

    #[test]
    fn expired_service_budget_refuses_before_snapshot_acquisition() -> TestResult {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mock = |track| PhysicalSnapshotMock {
            track,
            bytes: 1,
            refuse_acquire: false,
            cancel_budget_on_measure: false,
            events: Arc::clone(&events),
        };
        let pair =
            PairIndexBytesMeasurer::new(Arc::new(mock("lexical")), Arc::new(mock("semantic")));
        let past = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .ok_or("fixture deadline underflow")?;
        let budget = RequestBudgetV1::until(past);
        assert!(matches!(
            pair.measure_physical_with_budget(&budget, None),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
                ..
            })
        ));
        assert!(
            events
                .lock()
                .map_err(|error| format!("fixture lock: {error}"))?
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn one_service_budget_fences_both_tracks_and_releases_both_guards() -> TestResult {
        let events = Arc::new(Mutex::new(Vec::new()));
        let pair = PairIndexBytesMeasurer::new(
            Arc::new(PhysicalSnapshotMock {
                track: "lexical",
                bytes: 7,
                refuse_acquire: false,
                cancel_budget_on_measure: true,
                events: Arc::clone(&events),
            }),
            Arc::new(PhysicalSnapshotMock {
                track: "semantic",
                bytes: 11,
                refuse_acquire: false,
                cancel_budget_on_measure: false,
                events: Arc::clone(&events),
            }),
        );
        let budget = RequestBudgetV1::until(
            Instant::now()
                .checked_add(Duration::from_secs(60))
                .ok_or("fixture deadline overflow")?,
        );
        assert!(matches!(
            pair.measure_physical_with_budget(&budget, None),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
                ..
            })
        ));
        assert_eq!(
            *events
                .lock()
                .map_err(|error| format!("fixture lock: {error}"))?,
            [
                "acquire lexical",
                "acquire semantic",
                "measure lexical",
                "drop semantic",
                "drop lexical",
            ]
        );
        Ok(())
    }

    #[test]
    fn pair_session_uses_one_budget_and_never_accepts_lexical_partial_success() -> TestResult {
        let events = Arc::new(Mutex::new(Vec::new()));
        let pair = PairIndexBytesMeasurer::new(
            Arc::new(PhysicalSnapshotMock {
                track: "lexical",
                bytes: 7,
                refuse_acquire: false,
                cancel_budget_on_measure: true,
                events: Arc::clone(&events),
            }),
            Arc::new(PhysicalSnapshotMock {
                track: "semantic",
                bytes: 11,
                refuse_acquire: false,
                cancel_budget_on_measure: false,
                events: Arc::clone(&events),
            }),
        );
        let repo = RepoId::new("pair-budget")?;
        let revision = RevisionId::new("revision")?;
        let budget = pair.measurement_policy().physical_scan_budget()?;
        let mut session = pair.begin_pair_measurement(&repo, &revision, &budget, None)?;
        assert_eq!(session.measure(&BTreeSet::from([g(1)]))?.bytes, 18);
        assert!(matches!(
            session.measure(&BTreeSet::from([g(2)])),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
                ..
            })
        ));
        drop(session);
        assert!(budget.is_cancelled());
        assert_eq!(
            *events
                .lock()
                .map_err(|error| format!("fixture lock: {error}"))?,
            [
                "acquire pair lexical",
                "acquire pair semantic",
                "measure pair lexical",
                "measure pair semantic",
                "measure pair lexical",
                "drop pair lexical",
                "drop pair semantic"
            ]
        );
        assert!(matches!(
            pair.measure_index_bytes(&repo, &revision, &BTreeSet::from([g(2)]), &pair.measurement_policy().physical_scan_budget()?, None),
            Err(CoreError::Typed {
                code: quanta_index_contract::SearchPlaneErrorCodeV2::SearchCorpusHistoryRetentionExhausted,
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn pair_measurement_session_probes_exact_sets_and_recovers_after_error() -> TestResult {
        let port = ScriptedPairBytes {
            fail_on_two: true,
            ..ScriptedPairBytes::default()
        };
        let repo = RepoId::new("retention-session")?;
        let revision = RevisionId::new("revision")?;
        let budget = port.measurement_policy().physical_scan_budget()?;
        let mut session = port.begin_pair_measurement(&repo, &revision, &budget, None)?;
        let required = BTreeSet::from([g(3), g(4)]);
        assert_eq!(session.measure(&required)?.bytes, 12);
        assert!(
            session
                .measure(&BTreeSet::from([g(2), g(3), g(4)]))
                .is_err()
        );
        assert_eq!(session.measure(&required)?.bytes, 12);
        drop(session);
        assert_eq!(
            *port
                .calls
                .lock()
                .map_err(|error| format!("fixture lock: {error}"))?,
            vec![
                required.clone(),
                BTreeSet::from([g(2), g(3), g(4)]),
                required,
            ]
        );
        Ok(())
    }

    #[test]
    fn retention_plan_uses_one_session_for_exact_subset_sequence() -> TestResult {
        let port = ScriptedPairBytes::default();
        let repo = RepoId::new("retention-plan-session")?;
        let revision = RevisionId::new("revision")?;
        let budget = port.measurement_policy().physical_scan_budget()?;
        let mut session = port.begin_pair_measurement(&repo, &revision, &budget, None)?;
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(4, 15, 8, 200)?;
        let plan = policy.plan(
            vec![item(4, true), item(3, false), item(2, false)],
            &mut |set| Ok(session.measure(set)?.bytes),
        )?;
        assert_eq!(plan.retained_bytes(), 12);
        assert!(plan.retains(g(4)) && plan.retains(g(3)) && !plan.retains(g(2)));
        drop(session);
        assert_eq!(
            *port
                .calls
                .lock()
                .map_err(|error| format!("fixture lock: {error}"))?,
            vec![
                BTreeSet::from([g(3), g(4)]),
                BTreeSet::from([g(2), g(3), g(4)]),
            ]
        );
        Ok(())
    }

    #[test]
    fn pair_track_sessions_preserve_absent_union_and_propagate_errors() -> TestResult {
        let set = BTreeSet::from([g(1), g(2)]);
        let mut session = PairIndexByteSession {
            lexical: scripted_track(vec![
                Ok(SealedGenerationBytesV1 {
                    bytes: 7,
                    absent: BTreeSet::from([g(1)]),
                }),
                Ok(SealedGenerationBytesV1 {
                    bytes: u64::MAX,
                    absent: BTreeSet::new(),
                }),
                Err(CoreError::Storage("lexical fixture failure".into())),
            ]),
            semantic: scripted_track(vec![
                Ok(SealedGenerationBytesV1 {
                    bytes: 5,
                    absent: BTreeSet::from([g(2)]),
                }),
                Ok(SealedGenerationBytesV1 {
                    bytes: 1,
                    absent: BTreeSet::new(),
                }),
            ]),
            budget: RetentionMeasurementPolicy::DEFAULT.physical_scan_budget()?,
        };
        assert_eq!(
            session.measure(&set)?,
            SealedGenerationBytesV1 {
                bytes: 12,
                absent: set.clone(),
            }
        );
        assert!(
            matches!(
                session.measure(&set),
                Err(CoreError::Storage(message)) if message.contains("index byte total overflow")
            ),
            "sum overflow must refuse with the pair-total error"
        );
        assert!(
            matches!(
                session.measure(&set),
                Err(CoreError::Storage(message)) if message == "lexical fixture failure"
            ),
            "track failure must propagate unchanged"
        );
        Ok(())
    }

    #[test]
    fn retention_policy_rejects_less_than_predecessor_window() {
        assert!(SearchCorpusHistoryRetentionPolicyV1::new(1, 1024, 8, 8192).is_err());
        assert!(SearchCorpusHistoryRetentionPolicyV1::new(2, 0, 8, 8192).is_err());
        assert!(SearchCorpusHistoryRetentionPolicyV1::new(2, 1024, 0, 8192).is_err());
        assert!(SearchCorpusHistoryRetentionPolicyV1::new(2, 1024, 8, 0).is_err());
    }

    #[test]
    fn restore_reuses_only_a_complete_set_within_both_pair_limits() -> TestResult {
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(2, 20, 8, 200)?;
        let generations = BTreeSet::from([g(1), g(2)]);
        let exact = policy
            .retain_all_preverified_for_restore(&generations, 20)
            .ok_or("exact count and byte boundary must fit")?;
        assert!(exact.retains(g(1)) && exact.retains(g(2)));
        assert_eq!(exact.retained_bytes(), 20);
        assert!(
            policy
                .retain_all_preverified_for_restore(&generations, 21)
                .is_none()
        );
        assert!(
            policy
                .retain_all_preverified_for_restore(&BTreeSet::from([g(1), g(2), g(3)]), 20)
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn retention_plan_applies_count_and_byte_caps() -> TestResult {
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(3, 25, 8, 200)?;
        let plan = policy.plan(
            vec![
                item(4, true),
                item(3, false),
                item(2, false),
                item(1, false),
            ],
            &mut ten_each,
        )?;
        assert!(plan.retains(g(4)));
        assert!(plan.retains(g(3)));
        assert!(!plan.retains(g(2)));
        assert_eq!(plan.retained_bytes(), 20);
        Ok(())
    }

    /// The byte cap is measured over the retained set as one inode set,
    /// not summed per generation: two generations that share their bytes
    /// fit where two independent ones would not.
    #[test]
    fn retention_plan_measures_the_retained_set_not_a_sum() -> TestResult {
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(4, 15, 8, 200)?;
        // Generations 3 and 4 hard-link one segment: together they cost 12,
        // apart 10 each; generation 2 costs its own 10.
        let mut shared = |set: &BTreeSet<ManifestGeneration>| -> Result<u64, CoreError> {
            let mut bytes = 0_u64;
            if set.contains(&g(4)) || set.contains(&g(3)) {
                bytes += 10;
            }
            if set.contains(&g(4)) && set.contains(&g(3)) {
                bytes += 2;
            }
            if set.contains(&g(2)) {
                bytes += 10;
            }
            Ok(bytes)
        };
        let plan = policy.plan(
            vec![item(4, true), item(3, false), item(2, false)],
            &mut shared,
        )?;
        assert!(plan.retains(g(4)) && plan.retains(g(3)));
        assert!(!plan.retains(g(2)), "2 would push the set to 22 > 15");
        assert_eq!(plan.retained_bytes(), 12);
        Ok(())
    }

    #[test]
    fn retention_plan_rejects_candidate_outside_window() -> TestResult {
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(2, 1024, 8, 8192)?;
        assert!(
            policy
                .plan(
                    vec![item(4, false), item(3, false), item(2, true)],
                    &mut ten_each
                )
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn retention_plan_rejects_byte_cap_that_cannot_preserve_predecessor() -> TestResult {
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(4, 19, 8, 200)?;
        assert!(
            policy
                .plan(vec![item(4, true), item(3, false)], &mut ten_each)
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn retention_plan_never_silently_reaps_only_newest_generation() -> TestResult {
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(2, 9, 8, 200)?;
        assert!(policy.plan(vec![item(4, false)], &mut ten_each).is_err());
        Ok(())
    }

    const fn item(generation: u64, candidate: bool) -> SearchCorpusHistoryRetentionItemV1 {
        SearchCorpusHistoryRetentionItemV1 {
            generation: ManifestGeneration::new(generation),
            candidate,
            active: false,
            unresolved_source: false,
        }
    }

    #[test]
    fn retention_plan_pins_rolled_back_active_generation_v1() -> TestResult {
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(2, 1024, 8, 8192)?;
        let plan = policy.plan(
            vec![
                SearchCorpusHistoryRetentionItemV1 {
                    generation: g(3),
                    candidate: true,
                    active: false,
                    unresolved_source: false,
                },
                SearchCorpusHistoryRetentionItemV1 {
                    generation: g(2),
                    candidate: false,
                    active: false,
                    unresolved_source: false,
                },
                SearchCorpusHistoryRetentionItemV1 {
                    generation: g(1),
                    candidate: false,
                    active: true,
                    unresolved_source: false,
                },
            ],
            &mut ten_each,
        )?;
        assert!(plan.retains(g(3)));
        assert!(plan.retains(g(1)));
        assert!(!plan.retains(g(2)));
        Ok(())
    }
}
