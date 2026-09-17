//! Regex prefilter — bounded candidate-set producer for `/…/` leaves.
//!
//! [`regex_prefilter_any_of`] takes the literal **alternation** a regex
//! literal extractor produced (e.g. `regex_syntax::hir::literal::Extractor`,
//! whose `Seq` is a set of alternatives, any one of which a match may begin
//! with) and returns the union of the documents that could contain each
//! alternative.
//!
//! The function is intentionally narrow: it does NOT extract literals from a
//! regex itself, and it does NOT do final regex matching. Both belong to
//! sibling tickets per LEX-02 spec §2.3.
//!
//! **Why union, not intersection.** An extractor's alternation is satisfied by
//! any one member: `/(foo|bar)/` extracts `["foo", "bar"]` and a document
//! holding only `foo` matches. Requiring every member — which this module did
//! until the alternation semantics were pinned down — drops such documents
//! silently. Case-insensitive patterns make that failure near-universal:
//! `(?i)fresh` extracts both `fresh` and `freſh` (U+017F LATIN SMALL LETTER
//! LONG S is the Unicode case-fold partner of `s`), so an intersection demands
//! trigrams no ASCII document can hold and every candidate is filtered away.
//!
//! Pure-wildcard regexes (`/.*/`, `/\w+/`, etc.) yield zero extractable
//! literals; the caller MUST pass an empty slice in that case, and this
//! function surfaces an explicit [`TrigramErrorCode::RegexPrefilterUnusable`]
//! so the planner routes the query through the verify-only path rather
//! than silently degrading.

use crate::errors::{LimitDimension, TrigramError, TrigramErrorCode};
use crate::source::TrigramPostingSource;
use crate::types::{DocId, MAX_TRIGRAMS_PER_QUERY, TRIGRAM_LEN, Trigram, trigrams_of};

/// Candidate documents for a regex whose match must contain at least one of
/// `literal_alternation`.
///
/// The result is the union, over alternatives, of the documents holding every
/// trigram of that alternative. `idx` is any [`TrigramPostingSource`]: one
/// [`crate::TrigramIndex`] or a [`crate::ShardedTrigramIndex`] over a doc-id
/// partition; the algorithm is the same and so is the answer.
///
/// Inputs:
///
/// - empty `literal_alternation` → [`TrigramErrorCode::RegexPrefilterUnusable`].
/// - **any** alternative shorter than `n=3` →
///   [`TrigramErrorCode::RegexPrefilterUnusable`]. One indiscriminable
///   alternative makes the whole prefilter unsound: a document that matches
///   only through that alternative has no trigram evidence, so filtering at all
///   would drop it. Skipping the short alternative and keeping the rest is the
///   silent-false-negative shape this contract exists to prevent.
/// - aggregate distinct-trigram count across alternatives >
///   [`MAX_TRIGRAMS_PER_QUERY`] → [`TrigramErrorCode::PlanLimitExceeded`] with
///   [`LimitDimension::Trigrams`].
pub fn regex_prefilter_any_of<S: TrigramPostingSource + ?Sized>(
    idx: &S,
    literal_alternation: &[Vec<u8>],
) -> Result<Vec<DocId>, TrigramError> {
    if literal_alternation.is_empty() {
        return Err(TrigramError::new(
            TrigramErrorCode::RegexPrefilterUnusable,
            "regex extracted zero mandatory literals; caller must verify-only",
        ));
    }

    // Case folding makes extractors emit the same alternative many times over
    // (`(?i)gamma` yields 32 copies). Collapse before doing any index work.
    let mut alternatives: Vec<&[u8]> = Vec::with_capacity(literal_alternation.len());
    for literal in literal_alternation {
        if literal.len() < TRIGRAM_LEN {
            return Err(TrigramError::new(
                TrigramErrorCode::RegexPrefilterUnusable,
                "regex alternation holds a literal shorter than trigram width; \
                 filtering would drop documents matching only through it",
            ));
        }
        alternatives.push(literal.as_slice());
    }
    alternatives.sort_unstable();
    alternatives.dedup();

    let mut distinct_trigrams: Vec<Trigram> = Vec::new();
    let mut per_alternative: Vec<Vec<Trigram>> = Vec::with_capacity(alternatives.len());
    for alternative in &alternatives {
        let mut trigrams: Vec<Trigram> = trigrams_of(alternative).collect();
        trigrams.sort_unstable();
        trigrams.dedup();
        distinct_trigrams.extend_from_slice(&trigrams);
        per_alternative.push(trigrams);
    }
    distinct_trigrams.sort_unstable();
    distinct_trigrams.dedup();
    if distinct_trigrams.len() > MAX_TRIGRAMS_PER_QUERY {
        return Err(TrigramError::plan_limit(
            LimitDimension::Trigrams,
            format!(
                "regex prefilter trigram set {} exceeds cap {}",
                distinct_trigrams.len(),
                MAX_TRIGRAMS_PER_QUERY
            ),
        ));
    }

    let mut candidates: Vec<DocId> = Vec::new();
    for trigrams in &per_alternative {
        candidates.extend(idx.intersect_trigrams(trigrams)?);
    }
    candidates.sort_unstable();
    candidates.dedup();
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::regex_prefilter_any_of;
    use crate::builder::TrigramIndexBuilder;
    use crate::errors::TrigramErrorCode;
    use crate::index::TrigramIndex;
    use crate::types::DocId;

    fn index_of(docs: &[(u64, &str)]) -> TrigramIndex {
        let mut builder = match TrigramIndexBuilder::new(1) {
            Ok(builder) => builder,
            Err(err) => panic!("builder: {err}"),
        };
        for (id, text) in docs {
            builder.add_doc(DocId(*id), text.as_bytes());
        }
        builder.finish()
    }

    fn literals(values: &[&str]) -> Vec<Vec<u8>> {
        values
            .iter()
            .map(|value| value.as_bytes().to_vec())
            .collect()
    }

    #[test]
    fn empty_alternation_is_unusable() {
        let idx = index_of(&[(1, "alpha")]);
        let Err(err) = regex_prefilter_any_of(&idx, &[]) else {
            panic!("empty alternation must be unusable");
        };
        assert_eq!(err.code, TrigramErrorCode::RegexPrefilterUnusable);
    }

    #[test]
    fn a_short_alternative_makes_the_whole_prefilter_unusable() {
        let idx = index_of(&[(1, "alphabet")]);
        let Err(err) = regex_prefilter_any_of(&idx, &literals(&["alpha", "ab"])) else {
            panic!("a sub-trigram alternative must make the prefilter unusable");
        };
        assert_eq!(err.code, TrigramErrorCode::RegexPrefilterUnusable);
    }

    #[test]
    fn a_document_matching_one_alternative_survives() {
        let idx = index_of(&[(1, "the foo document"), (2, "the bar document")]);
        let Ok(candidates) = regex_prefilter_any_of(&idx, &literals(&["foo", "bar"])) else {
            panic!("alternation prefilter must succeed");
        };
        assert_eq!(candidates, vec![DocId(1), DocId(2)]);
    }

    #[test]
    fn an_unrepresented_alternative_does_not_erase_the_others() {
        // The case-fold shape: `(?i)fresh` extracts `fresh` and `freſh`, and no
        // ASCII document holds the second. Intersecting would return nothing.
        let idx = index_of(&[(1, "gamma_replacement freshsentinel")]);
        let Ok(candidates) = regex_prefilter_any_of(&idx, &literals(&["fresh", "fre\u{17f}h"]))
        else {
            panic!("alternation prefilter must succeed");
        };
        assert_eq!(candidates, vec![DocId(1)]);
    }

    #[test]
    fn an_alternation_no_document_holds_returns_no_candidates() {
        let idx = index_of(&[(1, "alpha beta")]);
        let Ok(candidates) = regex_prefilter_any_of(&idx, &literals(&["zebra", "quokka"])) else {
            panic!("alternation prefilter must succeed");
        };
        assert!(candidates.is_empty(), "got {candidates:?}");
    }

    #[test]
    fn duplicate_alternatives_collapse() {
        let idx = index_of(&[(1, "gamma ray")]);
        let Ok(candidates) = regex_prefilter_any_of(&idx, &literals(&["gamma", "gamma", "gamma"]))
        else {
            panic!("alternation prefilter must succeed");
        };
        assert_eq!(candidates, vec![DocId(1)]);
    }
}
