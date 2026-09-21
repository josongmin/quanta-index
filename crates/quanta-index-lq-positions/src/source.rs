//! Posting sources — one algorithm over one index or over a shard set.
//!
//! [`crate::phrase_query::query_phrase`] and
//! [`crate::adjacency_query::query_adjacency`] only ever ask an index one
//! question: "walk the postings of this term, doc by doc, ascending". The
//! [`TermPostingSource`] trait is that question, so both algorithms are
//! written once and run unchanged over a single [`PositionsIndex`] or over a
//! [`ShardedPositionsIndex`] whose shards partition the doc-id space.
//!
//! A sharded source walks each shard's postings for the term in shard
//! order. Because the shards hold disjoint, ascending doc-id ranges, the
//! chain is the same ascending doc walk the single index yields, so every
//! join the algorithms perform sees identical input and the answers are
//! byte-identical, caps included: the adjacency scan-depth counter runs
//! over the whole chain, exactly as over one index.

use crate::errors::{PositionsError, PositionsErrorCode};
use crate::index::{PositionsIndex, TermPostings, TermPostingsEntry};
use crate::types::DocId;

/// Where a query walks a term's postings.
///
/// Implemented by [`PositionsIndex`] (one posting map) and by
/// [`ShardedPositionsIndex`] (a doc-id-partitioned set of posting maps).
pub trait TermPostingSource {
    /// The decoding walk over one term's postings, ascending by doc id.
    type Postings<'a>: Iterator<Item = Result<TermPostingsEntry, PositionsError>>
    where
        Self: 'a;

    /// Open the walk over `term`'s postings, or `None` when no posting for
    /// `term` exists anywhere in the source.
    fn term_postings(&self, term: &str) -> Option<Self::Postings<'_>>;
}

impl TermPostingSource for PositionsIndex {
    type Postings<'a> = TermPostings<'a>;

    fn term_postings(&self, term: &str) -> Option<TermPostings<'_>> {
        Self::term_postings(self, term)
    }
}

/// A set of position indexes that partition one doc-id space.
///
/// The shards are given in ascending doc-id order and hold disjoint doc-id
/// ranges; the owner of the shards guarantees that by construction (each
/// shard is a fixed doc-id range). The chain checks the guarantee on every
/// walk: a doc id that does not increase across a shard boundary is a
/// corrupt shard set, reported as [`PositionsErrorCode::IndexCorrupted`],
/// never silently merged.
pub struct ShardedPositionsIndex<'a> {
    shards: Vec<&'a PositionsIndex>,
}

impl<'a> ShardedPositionsIndex<'a> {
    /// Build the chain over `shards`, which must be in ascending doc-id order.
    #[must_use]
    pub const fn new(shards: Vec<&'a PositionsIndex>) -> Self {
        Self { shards }
    }

    /// How many shards the chain spans.
    #[must_use]
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }
}

impl TermPostingSource for ShardedPositionsIndex<'_> {
    type Postings<'b>
        = ShardedTermPostings<'b>
    where
        Self: 'b;

    /// The shards' walks for `term` chained in shard order; `None` when no
    /// shard holds the term, exactly as one index answers.
    fn term_postings(&self, term: &str) -> Option<ShardedTermPostings<'_>> {
        let walks: Vec<TermPostings<'_>> = self
            .shards
            .iter()
            .filter_map(|shard| shard.term_postings(term))
            .collect();
        if walks.is_empty() {
            return None;
        }
        Some(ShardedTermPostings {
            pending: walks.into_iter(),
            current: None,
            last_doc: None,
            errored: false,
        })
    }
}

/// One term's postings across a shard chain, ascending by doc id.
///
/// Fail-closed like [`TermPostings`]: after the first error nothing more is
/// yielded. A doc id that does not increase across a shard boundary is
/// reported as [`PositionsErrorCode::IndexCorrupted`].
pub struct ShardedTermPostings<'a> {
    pending: std::vec::IntoIter<TermPostings<'a>>,
    current: Option<TermPostings<'a>>,
    last_doc: Option<DocId>,
    errored: bool,
}

impl Iterator for ShardedTermPostings<'_> {
    type Item = Result<TermPostingsEntry, PositionsError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.errored {
            return None;
        }
        loop {
            if let Some(walk) = self.current.as_mut() {
                match walk.next() {
                    Some(Ok(entry)) => {
                        if let Some(last) = self.last_doc
                            && entry.doc_id <= last
                        {
                            self.errored = true;
                            return Some(Err(PositionsError::new(
                                PositionsErrorCode::IndexCorrupted,
                                format!(
                                    "sharded positions index: doc {} of a later shard does not follow doc {} of an earlier one",
                                    entry.doc_id.0, last.0
                                ),
                            )));
                        }
                        self.last_doc = Some(entry.doc_id);
                        return Some(Ok(entry));
                    }
                    Some(Err(err)) => {
                        self.errored = true;
                        return Some(Err(err));
                    }
                    None => self.current = None,
                }
            }
            match self.pending.next() {
                Some(walk) => self.current = Some(walk),
                None => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ShardedPositionsIndex, TermPostingSource};
    use crate::builder::PositionsBuilder;
    use crate::errors::PositionsErrorCode;
    use crate::index::{PositionsIndex, TermPostingsEntry};
    use crate::types::{DocId, NormalizerVersion, Position};

    fn index_of(docs: &[(u64, &[&str])]) -> PositionsIndex {
        let mut builder = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        for (doc_id, tokens) in docs {
            for (position, token) in tokens.iter().enumerate() {
                builder
                    .add_token(
                        DocId(*doc_id),
                        token,
                        Position(u32::try_from(position).expect("fits")),
                    )
                    .expect("add token");
            }
        }
        builder.finish().expect("finish")
    }

    fn walk<S: TermPostingSource>(source: &S, term: &str) -> Option<Vec<TermPostingsEntry>> {
        source
            .term_postings(term)
            .map(|postings| postings.map(|entry| entry.expect("entry")).collect())
    }

    #[test]
    fn chain_over_shards_equals_the_single_index_walk() {
        let all = index_of(&[
            (1, &["the", "quick", "fox"]),
            (2, &["the", "lazy", "dog"]),
            (5, &["quick", "the"]),
            (9, &["fox", "the", "the"]),
        ]);
        let low = index_of(&[(1, &["the", "quick", "fox"]), (2, &["the", "lazy", "dog"])]);
        let high = index_of(&[(5, &["quick", "the"]), (9, &["fox", "the", "the"])]);
        let sharded = ShardedPositionsIndex::new(vec![&low, &high]);
        assert_eq!(sharded.shard_count(), 2);
        for term in ["the", "quick", "fox", "dog", "absent"] {
            assert_eq!(walk(&sharded, term), walk(&all, term), "term {term}");
        }
    }

    /// The query algorithms are generic over the source, so this is the
    /// proof that the shard chain feeds them exactly what one index does:
    /// phrase and adjacency answers, in order, cap included.
    #[test]
    fn phrase_and_adjacency_over_shards_equal_the_single_index_answers() {
        use crate::adjacency_query::query_adjacency;
        use crate::phrase_query::query_phrase;
        use crate::types::AdjacencyConfig;

        let corpus: Vec<(u64, &[&str])> = vec![
            (1, &["the", "quick", "brown", "fox"]),
            (2, &["the", "lazy", "dog"]),
            (2047, &["quick", "brown", "the", "fox"]),
            (
                2048,
                &[
                    "the", "quick", "brown", "fox", "jumps", "over", "the", "quick",
                ],
            ),
            (4096, &["fox", "the", "the", "quick"]),
        ];
        let all = index_of(&corpus);
        let shards: Vec<PositionsIndex> = corpus.chunks(2).map(index_of).collect();
        let sharded = ShardedPositionsIndex::new(shards.iter().collect());
        for phrase in [
            vec!["the"],
            vec!["quick", "brown"],
            vec!["the", "quick", "brown"],
            vec!["brown", "the"],
            vec!["absent"],
            vec![],
        ] {
            assert_eq!(
                query_phrase(&sharded, &phrase).expect("sharded phrase"),
                query_phrase(&all, &phrase).expect("single phrase"),
                "phrase {phrase:?}"
            );
        }
        let window = AdjacencyConfig::new(3).expect("window");
        for (a, b) in [("the", "fox"), ("quick", "the"), ("fox", "absent")] {
            assert_eq!(
                query_adjacency(&sharded, a, b, &window).expect("sharded adjacency"),
                query_adjacency(&all, a, b, &window).expect("single adjacency"),
                "adjacency {a} {b}"
            );
        }
    }

    #[test]
    fn a_term_held_by_no_shard_is_absent_not_empty() {
        let low = index_of(&[(1, &["alpha"])]);
        let sharded = ShardedPositionsIndex::new(vec![&low]);
        assert!(sharded.term_postings("beta").is_none());
        assert!(
            ShardedPositionsIndex::new(Vec::new())
                .term_postings("alpha")
                .is_none()
        );
    }

    #[test]
    fn shards_out_of_doc_order_are_corrupt_not_merged() {
        let low = index_of(&[(1, &["alpha"])]);
        let high = index_of(&[(5, &["alpha"])]);
        let sharded = ShardedPositionsIndex::new(vec![&high, &low]);
        let mut postings = sharded.term_postings("alpha").expect("both hold alpha");
        assert_eq!(postings.next().expect("first entry").expect("ok").doc_id, DocId(5));
        let err = postings
            .next()
            .expect("second entry")
            .expect_err("out-of-order shards must be refused");
        assert_eq!(err.code, PositionsErrorCode::IndexCorrupted);
        assert!(postings.next().is_none(), "nothing after the first error");
    }
}
