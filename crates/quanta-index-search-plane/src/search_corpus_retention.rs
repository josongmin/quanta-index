use std::collections::BTreeSet;

use quanta_index_core::CoreError;

pub(crate) const ERR_SEARCH_CORPUS_HISTORY_RETENTION_POLICY_INVALID: &str =
    "SEARCH_CORPUS_HISTORY_RETENTION_POLICY_INVALID";
pub(crate) const ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED: &str =
    "SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED";

/// Explicit state-root retention limits for sealed search-corpus history.
///
/// At least two generations are required so admitting a new generation cannot
/// erase its immediate predecessor rollback target. Byte limits apply to the
/// encoded immutable authority records, not filesystem allocation units.
///
/// The pair-local limits are allowed to reap generations only inside the pair
/// being mutated. The state-root limits are admission fences: without a
/// product-active pin authority this owner must reject growth instead of
/// guessing which other repo/revision pair is safe to delete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_POLICY_INVALID.to_string(),
                message: format!(
                    "search-corpus history retention: max_generations must be at least 2, observed {max_generations}"
                ),
            });
        }
        if max_bytes == 0 {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_POLICY_INVALID.to_string(),
                message: "search-corpus history retention: max_bytes must be non-zero".to_string(),
            });
        }
        if max_revision_pairs == 0 {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_POLICY_INVALID.to_string(),
                message: "search-corpus history retention: max_revision_pairs must be non-zero"
                    .to_string(),
            });
        }
        if max_total_bytes == 0 {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_POLICY_INVALID.to_string(),
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

    pub(crate) fn plan(
        self,
        mut items: Vec<SearchCorpusHistoryRetentionItemV1>,
    ) -> Result<SearchCorpusHistoryRetentionPlanV1, CoreError> {
        items.sort_by(|left, right| right.generation.cmp(&left.generation));
        for pair in items.windows(2) {
            if pair[0].generation == pair[1].generation {
                return Err(CoreError::Storage(format!(
                    "search-corpus history retention: duplicate generation {}",
                    pair[0].generation
                )));
            }
        }

        let minimum_rollback_window_bytes =
            items.iter().take(2).try_fold(0_u64, |total, item| {
                total.checked_add(item.encoded_len).ok_or_else(|| {
                    CoreError::Storage(
                        "search-corpus history retention: encoded byte total overflow".to_string(),
                    )
                })
            })?;
        if let Some(newest) = items.first()
            && newest.encoded_len > self.max_bytes
        {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
                message: format!(
                    "search-corpus history retention: newest generation {} requires {} bytes, exceeding max_bytes={}",
                    newest.generation, newest.encoded_len, self.max_bytes,
                ),
            });
        }
        if items.len() >= 2 && minimum_rollback_window_bytes > self.max_bytes {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
                message: format!(
                    "search-corpus history retention: newest generation plus predecessor require {minimum_rollback_window_bytes} bytes, exceeding max_bytes={}",
                    self.max_bytes,
                ),
            });
        }

        let mut retained_generations = BTreeSet::new();
        let mut retained_bytes = 0_u64;
        for item in &items {
            let next_bytes = retained_bytes
                .checked_add(item.encoded_len)
                .ok_or_else(|| {
                    CoreError::Storage(
                        "search-corpus history retention: encoded byte total overflow".to_string(),
                    )
                })?;
            if retained_generations.len() >= self.max_generations || next_bytes > self.max_bytes {
                break;
            }
            let _inserted = retained_generations.insert(item.generation);
            retained_bytes = next_bytes;
        }
        if let Some(candidate) = items.iter().find(|item| item.candidate)
            && !retained_generations.contains(&candidate.generation)
        {
            return Err(CoreError::Typed {
                code: ERR_SEARCH_CORPUS_HISTORY_RETENTION_EXHAUSTED.to_string(),
                message: format!(
                    "search-corpus history retention: candidate generation {} falls outside configured count/byte window",
                    candidate.generation
                ),
            });
        }
        Ok(SearchCorpusHistoryRetentionPlanV1 {
            retained_generations,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SearchCorpusHistoryRetentionItemV1 {
    pub(crate) generation: u64,
    pub(crate) encoded_len: u64,
    pub(crate) candidate: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct SearchCorpusHistoryRetentionPlanV1 {
    retained_generations: BTreeSet<u64>,
}

impl SearchCorpusHistoryRetentionPlanV1 {
    pub(crate) fn retains(&self, generation: u64) -> bool {
        self.retained_generations.contains(&generation)
    }
}

#[cfg(test)]
mod tests {
    use super::{SearchCorpusHistoryRetentionItemV1, SearchCorpusHistoryRetentionPolicyV1};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

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
        let plan = policy.plan(vec![
            item(4, 10, true),
            item(3, 10, false),
            item(2, 10, false),
            item(1, 10, false),
        ])?;
        assert!(plan.retains(4));
        assert!(plan.retains(3));
        assert!(!plan.retains(2));
        Ok(())
    }

    #[test]
    fn retention_plan_rejects_candidate_outside_window() -> TestResult {
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(2, 1024, 8, 8192)?;
        assert!(
            policy
                .plan(vec![
                    item(4, 10, false),
                    item(3, 10, false),
                    item(2, 10, true),
                ])
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn retention_plan_rejects_byte_cap_that_cannot_preserve_predecessor() -> TestResult {
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(4, 19, 8, 200)?;
        assert!(
            policy
                .plan(vec![item(4, 10, true), item(3, 10, false)])
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn retention_plan_never_silently_reaps_only_newest_generation() -> TestResult {
        let policy = SearchCorpusHistoryRetentionPolicyV1::new(2, 9, 8, 200)?;
        assert!(policy.plan(vec![item(4, 10, false)]).is_err());
        Ok(())
    }

    const fn item(
        generation: u64,
        encoded_len: u64,
        candidate: bool,
    ) -> SearchCorpusHistoryRetentionItemV1 {
        SearchCorpusHistoryRetentionItemV1 {
            generation,
            encoded_len,
            candidate,
        }
    }
}
