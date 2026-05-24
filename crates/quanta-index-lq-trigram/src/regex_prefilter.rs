//! Regex prefilter — bounded candidate-set producer for `/…/` leaves.
//!
//! [`regex_prefilter`] takes a list of byte-string literals extracted
//! from a regex AST (by a sibling ticket, e.g. LEX-04 wiring
//! `regex_syntax::hir::literal::Extractor`) and AND-intersects the
//! per-literal trigram sets against [`TrigramIndex`].
//!
//! The function is intentionally narrow: it does NOT extract literals
//! from a regex itself, and it does NOT do final regex matching. Both
//! belong to sibling tickets per LEX-02 spec §2.3.
//!
//! Pure-wildcard regexes (`/.*/`, `/\w+/`, etc.) yield zero extractable
//! literals; the caller MUST pass an empty slice in that case, and this
//! function surfaces an explicit [`TrigramErrorCode::RegexPrefilterUnusable`]
//! so the planner routes the query through the verify-only path rather
//! than silently degrading.

use crate::errors::{LimitDimension, TrigramError, TrigramErrorCode};
use crate::index::TrigramIndex;
use crate::types::{MAX_TRIGRAMS_PER_QUERY, TRIGRAM_LEN, Trigram, trigrams_of};

/// Run a regex prefilter against `idx` using a precomputed set of
/// mandatory byte literals.
///
/// All literals in `required_literals` must match (logical AND); the
/// final result is the bounded candidate set returned by
/// [`TrigramIndex::intersect_trigrams`] over the union of every
/// literal's trigram set.
///
/// Inputs:
///
/// - empty `required_literals` → [`TrigramErrorCode::RegexPrefilterUnusable`].
/// - every literal shorter than `n=3` → [`TrigramErrorCode::RegexPrefilterUnusable`]
///   (no trigram discrimination available).
/// - aggregate distinct-trigram count > [`MAX_TRIGRAMS_PER_QUERY`] →
///   [`TrigramErrorCode::PlanLimitExceeded`] with
///   [`LimitDimension::Trigrams`].
pub fn regex_prefilter(
    idx: &TrigramIndex,
    required_literals: &[Vec<u8>],
) -> Result<Vec<crate::types::DocId>, TrigramError> {
    if required_literals.is_empty() {
        return Err(TrigramError::new(
            TrigramErrorCode::RegexPrefilterUnusable,
            "regex extracted zero mandatory literals; caller must verify-only",
        ));
    }

    let mut all_tris: Vec<Trigram> = Vec::new();
    let mut had_usable_literal = false;
    for lit in required_literals {
        if lit.len() < TRIGRAM_LEN {
            continue;
        }
        had_usable_literal = true;
        for t in trigrams_of(lit) {
            all_tris.push(t);
        }
    }
    if !had_usable_literal {
        return Err(TrigramError::new(
            TrigramErrorCode::RegexPrefilterUnusable,
            "every required literal shorter than trigram width",
        ));
    }

    all_tris.sort_unstable();
    all_tris.dedup();

    if all_tris.len() > MAX_TRIGRAMS_PER_QUERY {
        return Err(TrigramError::plan_limit(
            LimitDimension::Trigrams,
            format!(
                "regex prefilter trigram set {} exceeds cap {}",
                all_tris.len(),
                MAX_TRIGRAMS_PER_QUERY
            ),
        ));
    }

    idx.intersect_trigrams(&all_tris)
}

#[cfg(test)]
mod tests {
    use super::regex_prefilter;
    use crate::builder::TrigramIndexBuilder;
    use crate::errors::TrigramErrorCode;
    use crate::index::TrigramIndex;
    use crate::types::DocId;

    fn fatal(msg: &str) -> ! {
        assert!(false, "{msg}");
        std::process::abort();
    }

    fn fixture() -> TrigramIndex {
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        b.add_doc(DocId(1), b"fn handle_request(...)");
        b.add_doc(DocId(2), b"fn handle_response(...)");
        b.add_doc(DocId(3), b"fn other(...)");
        b.add_doc(DocId(4), b"struct Handler {}");
        b.finish()
    }

    #[test]
    fn empty_literals_is_typed_unusable() {
        let idx = fixture();
        match regex_prefilter(&idx, &[]) {
            Ok(_) => assert!(false, "expected REGEX_PREFILTER_UNUSABLE"),
            Err(e) => assert_eq!(e.code, TrigramErrorCode::RegexPrefilterUnusable),
        }
    }

    #[test]
    fn all_short_literals_is_typed_unusable() {
        let idx = fixture();
        let lits: &[Vec<u8>] = &[b"a".to_vec(), b"ab".to_vec()];
        match regex_prefilter(&idx, lits) {
            Ok(_) => assert!(false, "expected REGEX_PREFILTER_UNUSABLE"),
            Err(e) => assert_eq!(e.code, TrigramErrorCode::RegexPrefilterUnusable),
        }
    }

    #[test]
    fn one_required_literal_narrows() {
        let idx = fixture();
        let lits: &[Vec<u8>] = &[b"handle_".to_vec()];
        let v = match regex_prefilter(&idx, lits) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        // docs 1 and 2 contain handle_; docs 3 and 4 do not.
        assert_eq!(v, vec![DocId(1), DocId(2)]);
    }

    #[test]
    fn anded_literals_narrow_further() {
        let idx = fixture();
        let lits: &[Vec<u8>] = &[b"fn ".to_vec(), b"handle_".to_vec()];
        let v = match regex_prefilter(&idx, lits) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        // docs 1 and 2 contain both "fn " and "handle_"; doc 4 has
        // "Handler" but no "fn ".
        assert_eq!(v, vec![DocId(1), DocId(2)]);
    }

    #[test]
    fn short_literal_is_skipped_but_long_one_is_used() {
        let idx = fixture();
        // "ab" is shorter than n=3 and is skipped; "handle_" is used.
        let lits: &[Vec<u8>] = &[b"ab".to_vec(), b"handle_".to_vec()];
        let v = match regex_prefilter(&idx, lits) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(v, vec![DocId(1), DocId(2)]);
    }
}
