//! Pure ranking-quality metrics over a produced ordering and graded judgments.
//!
//! These functions are the deterministic core of the relevance rail. They take
//! a produced ranking (top-first list of stable doc ids) plus a graded-judgment
//! table and return `MRR@k`, `NDCG@k`, and `Recall@k`. There is no I/O, no
//! ranker access, and no randomness here — the numbers are a pure function of
//! `(ranking, judgments)`, which is exactly why the corpus calibration in
//! [`super::corpus`] can be reviewed against checked-in expectations.
//!
//! Conventions:
//! - a doc is *relevant* iff its graded judgment is `> 0`;
//! - docs absent from the judgment table are treated as grade `0` (a hard
//!   negative by omission), never silently dropped;
//! - `k` truncates the produced ranking before scoring; gold ideal rankings for
//!   `NDCG` are likewise truncated at `k`.

use std::collections::BTreeMap;

/// Lossless-in-practice `usize -> f64` for ranks and small judged-doc counts.
///
/// Rank positions and corpus doc counts are far below `2^53`, so the
/// `cast_precision_loss` the workspace denies cannot actually occur on these
/// values; the localized allow documents that invariant instead of hiding it.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    clippy::as_conversions,
    reason = "rank positions and corpus doc counts are far below 2^53, so usize->f64 is exact on these values"
)]
pub(crate) fn usize_to_f64(n: usize) -> f64 {
    n as f64
}

/// A graded relevance judgment for one document under one query.
///
/// `grade` is an ordinal gain: `0` = irrelevant (hard negative / distractor),
/// higher = more relevant. Graded (not binary) so `NDCG@k` stays meaningful and
/// a "present but wrongly ordered" ranking is distinguishable from a correct one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GradedDoc {
    pub doc_id: String,
    pub grade: u8,
}

/// Build a `doc_id -> grade` lookup, rejecting duplicate judgments fail-closed.
///
/// A duplicate doc id in a judgment table is a corpus authoring bug, not a
/// recoverable condition: returning `Err` keeps the calibration honest instead
/// of letting the last-writer silently win.
///
/// # Errors
/// Returns [`MetricError::DuplicateJudgment`] if a doc id appears more than once.
pub fn grade_index(judgments: &[GradedDoc]) -> Result<BTreeMap<&str, u8>, MetricError> {
    let mut index = BTreeMap::new();
    for judged in judgments {
        if index.insert(judged.doc_id.as_str(), judged.grade).is_some() {
            return Err(MetricError::DuplicateJudgment(judged.doc_id.clone()));
        }
    }
    Ok(index)
}

/// Reciprocal rank of the first relevant doc within the top `k` (else `0.0`).
///
/// This is the per-query term of `MRR@k`; the rail averages it across the query
/// set for one route family.
#[must_use]
pub fn reciprocal_rank_at_k(ranked: &[String], grades: &BTreeMap<&str, u8>, k: usize) -> f64 {
    for (idx, doc) in ranked.iter().take(k).enumerate() {
        if grades.get(doc.as_str()).copied().unwrap_or(0) > 0 {
            // rank is 1-based: idx 0 -> 1/1.
            return 1.0 / (usize_to_f64(idx) + 1.0);
        }
    }
    0.0
}

/// Graded `NDCG@k`: `DCG@k / IDCG@k`, or `0.0` when no relevant doc exists.
///
/// Gain is `2^grade - 1` and the positional discount is `1 / log2(rank + 1)`
/// with a 1-based rank. The ideal ranking is the judgment grades sorted
/// descending, truncated at `k`.
#[must_use]
pub fn ndcg_at_k(ranked: &[String], grades: &BTreeMap<&str, u8>, k: usize) -> f64 {
    let dcg = discounted_cumulative_gain(
        ranked
            .iter()
            .take(k)
            .map(|doc| grades.get(doc.as_str()).copied().unwrap_or(0)),
    );
    let mut ideal: Vec<u8> = grades.values().copied().filter(|g| *g > 0).collect();
    ideal.sort_unstable_by(|a, b| b.cmp(a));
    let idcg = discounted_cumulative_gain(ideal.into_iter().take(k));
    if idcg <= 0.0 {
        return 0.0;
    }
    dcg / idcg
}

/// `Recall@k`: fraction of all relevant docs that appear within the top `k`.
///
/// Returns `0.0` when there are no relevant docs, so an all-negative query can
/// never look like perfect recall.
#[must_use]
pub fn recall_at_k(ranked: &[String], grades: &BTreeMap<&str, u8>, k: usize) -> f64 {
    let relevant_total = grades.values().filter(|g| **g > 0).count();
    if relevant_total == 0 {
        return 0.0;
    }
    let hit = ranked
        .iter()
        .take(k)
        .filter(|doc| grades.get(doc.as_str()).copied().unwrap_or(0) > 0)
        .count();
    usize_to_f64(hit) / usize_to_f64(relevant_total)
}

fn discounted_cumulative_gain(grades: impl Iterator<Item = u8>) -> f64 {
    grades
        .enumerate()
        .map(|(idx, grade)| {
            let gain = 2f64.powi(i32::from(grade)) - 1.0;
            let discount = (usize_to_f64(idx) + 2.0).log2();
            gain / discount
        })
        .sum()
}

/// Authoring / evaluation error for the relevance metrics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetricError {
    /// The same doc id was judged twice in one query's table.
    DuplicateJudgment(String),
}

impl core::fmt::Display for MetricError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DuplicateJudgment(doc) => {
                write!(f, "duplicate relevance judgment for doc id `{doc}`")
            }
        }
    }
}

impl std::error::Error for MetricError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(id: &str, grade: u8) -> GradedDoc {
        GradedDoc {
            doc_id: id.to_string(),
            grade,
        }
    }

    fn ranking(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| (*s).to_string()).collect()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn duplicate_judgment_is_rejected() {
        let judged = vec![doc("a", 3), doc("a", 1)];
        assert_eq!(
            grade_index(&judged),
            Err(MetricError::DuplicateJudgment("a".to_string()))
        );
    }

    #[test]
    fn perfect_ranking_scores_one() {
        let judged = vec![doc("a", 3), doc("b", 2), doc("c", 1)];
        let grades = grade_index(&judged).unwrap();
        let ranked = ranking(&["a", "b", "c"]);
        assert!(close(reciprocal_rank_at_k(&ranked, &grades, 10), 1.0));
        assert!(close(ndcg_at_k(&ranked, &grades, 10), 1.0));
        assert!(close(recall_at_k(&ranked, &grades, 10), 1.0));
    }

    #[test]
    fn first_relevant_at_rank_three_gives_one_third() {
        let judged = vec![doc("hit", 2)];
        let grades = grade_index(&judged).unwrap();
        let ranked = ranking(&["neg1", "neg2", "hit", "neg3"]);
        assert!(close(reciprocal_rank_at_k(&ranked, &grades, 10), 1.0 / 3.0));
    }

    #[test]
    fn reciprocal_rank_respects_k_cutoff() {
        let judged = vec![doc("hit", 2)];
        let grades = grade_index(&judged).unwrap();
        let ranked = ranking(&["neg", "neg2", "hit"]);
        // hit is at rank 3, but k=2 excludes it.
        assert!(close(reciprocal_rank_at_k(&ranked, &grades, 2), 0.0));
    }

    #[test]
    fn recall_counts_only_relevant_within_k() {
        let judged = vec![doc("a", 1), doc("b", 1), doc("c", 1), doc("d", 1)];
        let grades = grade_index(&judged).unwrap();
        let ranked = ranking(&["a", "x", "b", "y", "c", "d"]);
        // a,b within top-3 -> 2 of 4.
        assert!(close(recall_at_k(&ranked, &grades, 3), 0.5));
        assert!(close(recall_at_k(&ranked, &grades, 6), 1.0));
    }

    #[test]
    fn all_negative_query_never_looks_perfect() {
        let judged = vec![doc("a", 0), doc("b", 0)];
        let grades = grade_index(&judged).unwrap();
        let ranked = ranking(&["a", "b"]);
        assert!(close(reciprocal_rank_at_k(&ranked, &grades, 10), 0.0));
        assert!(close(ndcg_at_k(&ranked, &grades, 10), 0.0));
        assert!(close(recall_at_k(&ranked, &grades, 10), 0.0));
    }

    #[test]
    fn ndcg_penalizes_reversed_order() {
        let judged = vec![doc("hi", 3), doc("lo", 1)];
        let grades = grade_index(&judged).unwrap();
        // Reversed: lo first, hi second.
        let ranked = ranking(&["lo", "hi"]);
        // DCG = (2^1-1)/log2(2) + (2^3-1)/log2(3) = 1/1 + 7/1.5849625 = 1 + 4.41653...
        // IDCG = 7/1 + 1/1.5849625 = 7 + 0.63092... = 7.63092...
        let dcg = 1.0 + 7.0 / 3f64.log2();
        let idcg = 7.0 + 1.0 / 3f64.log2();
        assert!(close(ndcg_at_k(&ranked, &grades, 10), dcg / idcg));
    }

    #[test]
    fn unjudged_doc_is_treated_as_negative() {
        let judged = vec![doc("hit", 2)];
        let grades = grade_index(&judged).unwrap();
        let ranked = ranking(&["unjudged", "hit"]);
        // unjudged is grade 0, so first relevant is at rank 2.
        assert!(close(reciprocal_rank_at_k(&ranked, &grades, 10), 0.5));
    }
}
