//! Sealed search-corpus history retention: the per-pair and state-root
//! limits, and the plan that decides which sealed generations a pair
//! keeps (QI-BB-003).
//!
//! Byte limits are enforced over the **actual index bytes** the retained
//! generations occupy on disk — the lexical and semantic generation
//! directories, measured by unique inode so a delta's hard-linked base
//! segments count once — never over the size of an authority record. The
//! measurement is a port ([`SearchCorpusIndexBytesPort`]) because the
//! authority store owns no index layout; the composition root wires the
//! adapters' measurement in.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;

use quanta_index_contract::{ManifestGeneration, RepoId, RevisionId};
use quanta_index_core::{CoreError, SealedGenerationBytesV1, SealedGenerationReclaimPort};

/// Measures the bytes a set of sealed generations of one pair occupies on
/// disk across both tracks: what `du` would report for those directories
/// together, hard links counted once.
///
/// Called at admission and at restore with the sets retention is deciding
/// between; a generation whose directories are absent contributes nothing.
pub trait SearchCorpusIndexBytesPort: fmt::Debug + Send + Sync {
    fn measure_index_bytes(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
    ) -> Result<SealedGenerationBytesV1, CoreError>;
}

/// The measurement over the two tracks' reclaim ports, which are the
/// adapters that know their generation directories.
pub struct PairIndexBytesMeasurer {
    lexical: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    semantic: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
}

impl PairIndexBytesMeasurer {
    #[must_use]
    pub fn new(
        lexical: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
        semantic: Arc<dyn SealedGenerationReclaimPort + Send + Sync>,
    ) -> Self {
        Self { lexical, semantic }
    }
}

impl fmt::Debug for PairIndexBytesMeasurer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairIndexBytesMeasurer")
    }
}

impl SearchCorpusIndexBytesPort for PairIndexBytesMeasurer {
    fn measure_index_bytes(
        &self,
        repo_id: &RepoId,
        revision_id: &RevisionId,
        generations: &BTreeSet<ManifestGeneration>,
    ) -> Result<SealedGenerationBytesV1, CoreError> {
        let lexical = self
            .lexical
            .measure_sealed_generations(repo_id, revision_id, generations)?;
        let semantic =
            self.semantic
                .measure_sealed_generations(repo_id, revision_id, generations)?;
        let bytes = lexical.bytes.checked_add(semantic.bytes).ok_or_else(|| {
            CoreError::Storage(
                "search-corpus history retention: index byte total overflow".to_string(),
            )
        })?;
        Ok(SealedGenerationBytesV1 {
            bytes,
            absent: lexical.absent.union(&semantic.absent).copied().collect(),
        })
    }
}

/// Explicit state-root retention limits for sealed search-corpus history.
///
/// At least two generations are required so admitting a new generation cannot
/// erase its immediate predecessor rollback target. `max_bytes` bounds the
/// index bytes one pair's retained generations occupy together;
/// `max_total_bytes` bounds the same across every pair in the state root.
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

    use std::collections::BTreeSet;

    use quanta_index_contract::ManifestGeneration;
    use quanta_index_core::CoreError;

    use super::{SearchCorpusHistoryRetentionItemV1, SearchCorpusHistoryRetentionPolicyV1};

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

    #[test]
    fn retention_policy_rejects_less_than_predecessor_window() {
        assert!(SearchCorpusHistoryRetentionPolicyV1::new(1, 1024, 8, 8192).is_err());
        assert!(SearchCorpusHistoryRetentionPolicyV1::new(2, 0, 8, 8192).is_err());
        assert!(SearchCorpusHistoryRetentionPolicyV1::new(2, 1024, 0, 8192).is_err());
        assert!(SearchCorpusHistoryRetentionPolicyV1::new(2, 1024, 8, 0).is_err());
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
