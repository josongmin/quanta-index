//! Posting sources — one algorithm over one index or over a shard set.
//!
//! [`crate::query::query_raw_substring`] and
//! [`crate::regex_prefilter::regex_prefilter_any_of`] only ever ask an index
//! one question: "which documents hold every one of these trigrams?". The
//! [`TrigramPostingSource`] trait is that question, so the query algorithms
//! are written once and run unchanged over a single [`TrigramIndex`] or over
//! a [`ShardedTrigramIndex`] whose shards partition the doc-id space.
//!
//! A sharded source answers by concatenating the per-shard intersections in
//! shard order. Because the shards hold disjoint, ascending doc-id ranges,
//! the concatenation is already the sorted, de-duplicated answer the single
//! index returns — byte-identical for every query the single index answers
//! successfully. The pre-verify candidate cap is enforced on the union, so
//! the verify pass that follows is bounded exactly as it is for one index.

use crate::errors::{LimitDimension, TrigramError, TrigramErrorCode};
use crate::index::{TrigramIndex, ensure_query_trigram_count};
use crate::types::{DocId, MAX_CANDIDATE_PRE_VERIFY, Trigram};

/// Where a query looks up trigram postings.
///
/// Implemented by [`TrigramIndex`] (one posting map) and by
/// [`ShardedTrigramIndex`] (a doc-id-partitioned set of posting maps).
pub trait TrigramPostingSource {
    /// AND-intersect the posting lists for `query_trigrams`.
    ///
    /// Returns the sorted, de-duplicated `DocId`s that appear in every
    /// posting list; an empty `query_trigrams` yields an empty result. The
    /// caps and their typed errors are those of
    /// [`TrigramIndex::intersect_trigrams`].
    fn intersect_trigrams(&self, query_trigrams: &[Trigram]) -> Result<Vec<DocId>, TrigramError>;
}

impl TrigramPostingSource for TrigramIndex {
    fn intersect_trigrams(&self, query_trigrams: &[Trigram]) -> Result<Vec<DocId>, TrigramError> {
        Self::intersect_trigrams(self, query_trigrams)
    }
}

/// A set of trigram indexes that partition one doc-id space.
///
/// The shards are given in ascending doc-id order and hold disjoint doc-id
/// ranges; the owner of the shards guarantees that by construction (each
/// shard is a fixed doc-id range). The union checks the guarantee on every
/// answer: a doc id that does not increase across a shard boundary is a
/// corrupt shard set, reported as
/// [`TrigramErrorCode::IndexCorrupted`], never silently merged.
pub struct ShardedTrigramIndex<'a> {
    shards: Vec<&'a TrigramIndex>,
}

impl<'a> ShardedTrigramIndex<'a> {
    /// Build the union over `shards`, which must be in ascending doc-id order.
    #[must_use]
    pub const fn new(shards: Vec<&'a TrigramIndex>) -> Self {
        Self { shards }
    }

    /// How many shards the union spans.
    #[must_use]
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }
}

impl TrigramPostingSource for ShardedTrigramIndex<'_> {
    /// The concatenation of every shard's intersection, in shard order.
    ///
    /// The per-query trigram cap is checked once up front so an empty shard
    /// set refuses an over-cap query exactly as one index does; the
    /// pre-verify candidate cap is enforced on the running union.
    fn intersect_trigrams(&self, query_trigrams: &[Trigram]) -> Result<Vec<DocId>, TrigramError> {
        ensure_query_trigram_count(query_trigrams)?;
        let mut union: Vec<DocId> = Vec::new();
        for shard in &self.shards {
            let part = shard.intersect_trigrams(query_trigrams)?;
            if let (Some(last), Some(first)) = (union.last(), part.first())
                && first <= last
            {
                return Err(TrigramError::new(
                    TrigramErrorCode::IndexCorrupted,
                    format!(
                        "sharded trigram index: doc {first} of a later shard does not follow doc {last} of an earlier one"
                    ),
                ));
            }
            union.extend(part);
            if union.len() > MAX_CANDIDATE_PRE_VERIFY {
                return Err(TrigramError::plan_limit(
                    LimitDimension::CandidateSet,
                    format!(
                        "candidate set across shards {} exceeds cap {}",
                        union.len(),
                        MAX_CANDIDATE_PRE_VERIFY
                    ),
                ));
            }
        }
        Ok(union)
    }
}

#[cfg(test)]
mod tests {
    use super::{ShardedTrigramIndex, TrigramPostingSource};
    use crate::builder::TrigramIndexBuilder;
    use crate::errors::{LimitDimension, TrigramErrorCode};
    use crate::index::TrigramIndex;
    use crate::types::{DocId, MAX_TRIGRAMS_PER_QUERY, Trigram};

    fn index_of(docs: &[(u64, &str)]) -> TrigramIndex {
        let mut builder = TrigramIndexBuilder::new(1).expect("builder");
        for (id, text) in docs {
            builder.add_doc(DocId(*id), text.as_bytes());
        }
        builder.finish()
    }

    fn trigrams(text: &str) -> Vec<Trigram> {
        let mut out: Vec<Trigram> = crate::types::trigrams_of(text.as_bytes()).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    #[test]
    fn union_over_shards_equals_the_single_index_answer() {
        let all = index_of(&[
            (1, "alpha beta"),
            (2, "beta gamma"),
            (5, "alpha gamma"),
            (9, "delta beta"),
        ]);
        let low = index_of(&[(1, "alpha beta"), (2, "beta gamma")]);
        let high = index_of(&[(5, "alpha gamma"), (9, "delta beta")]);
        let sharded = ShardedTrigramIndex::new(vec![&low, &high]);
        for needle in ["beta", "alpha", "gamma", "zzz", "delta"] {
            let query = trigrams(needle);
            let expected = all.intersect_trigrams(&query).expect("single");
            let observed = sharded.intersect_trigrams(&query).expect("sharded");
            assert_eq!(observed, expected, "needle {needle}");
        }
    }

    /// The query algorithms are generic over the source, so this is the
    /// proof that the shard union feeds them exactly what one index does:
    /// raw-substring and regex-prefilter answers, in order.
    #[test]
    fn substring_and_prefilter_over_shards_equal_the_single_index_answers() {
        use crate::query::{DocResolver, query_raw_substring};
        use crate::regex_prefilter::regex_prefilter_any_of;
        use std::collections::BTreeMap;

        struct Corpus(BTreeMap<DocId, Vec<u8>>);
        impl DocResolver for Corpus {
            fn resolve(&self, doc_id: DocId) -> Option<&[u8]> {
                self.0.get(&doc_id).map(Vec::as_slice)
            }
        }

        let corpus: Vec<(u64, &str)> = vec![
            (1, "alpha beta gamma"),
            (2, "beta gamma delta"),
            (2047, "gamma delta alpha"),
            (2048, "delta alpha beta"),
            (4096, "alpha alpha epsilon"),
        ];
        let all = index_of(&corpus);
        let shards: Vec<TrigramIndex> = corpus.chunks(2).map(index_of).collect();
        let sharded = ShardedTrigramIndex::new(shards.iter().collect());
        let resolver = Corpus(
            corpus
                .iter()
                .map(|(id, text)| (DocId(*id), text.as_bytes().to_vec()))
                .collect(),
        );
        for needle in ["alpha", "beta gamma", "epsilon", "zeta", "a a"] {
            assert_eq!(
                query_raw_substring(&sharded, needle.as_bytes(), &resolver).expect("sharded"),
                query_raw_substring(&all, needle.as_bytes(), &resolver).expect("single"),
                "needle {needle}"
            );
        }
        for alternation in [
            vec![b"alpha".to_vec()],
            vec![b"beta".to_vec(), b"epsilon".to_vec()],
            vec![b"zeta".to_vec()],
        ] {
            assert_eq!(
                regex_prefilter_any_of(&sharded, &alternation).expect("sharded"),
                regex_prefilter_any_of(&all, &alternation).expect("single"),
                "alternation {alternation:?}"
            );
        }
    }

    #[test]
    fn an_empty_shard_set_answers_nothing_but_still_refuses_an_over_cap_query() {
        let sharded = ShardedTrigramIndex::new(Vec::new());
        assert_eq!(sharded.shard_count(), 0);
        assert!(
            sharded
                .intersect_trigrams(&trigrams("alpha"))
                .expect("empty union")
                .is_empty()
        );
        let over_cap: Vec<Trigram> = (0..=MAX_TRIGRAMS_PER_QUERY)
            .map(|i| {
                let bytes = u32::try_from(i).expect("fits").to_be_bytes();
                [bytes[1], bytes[2], bytes[3]]
            })
            .collect();
        let err = sharded
            .intersect_trigrams(&over_cap)
            .expect_err("over-cap query must be refused");
        assert_eq!(err.code, TrigramErrorCode::PlanLimitExceeded);
        assert_eq!(err.dimension, Some(LimitDimension::Trigrams));
    }

    #[test]
    fn shards_out_of_doc_order_are_corrupt_not_merged() {
        let low = index_of(&[(1, "alpha beta")]);
        let high = index_of(&[(5, "alpha gamma")]);
        let sharded = ShardedTrigramIndex::new(vec![&high, &low]);
        let err = sharded
            .intersect_trigrams(&trigrams("alpha"))
            .expect_err("out-of-order shards must be refused");
        assert_eq!(err.code, TrigramErrorCode::IndexCorrupted);
    }
}
