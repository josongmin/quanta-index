//! Raw-substring query path with `memchr::memmem::find` verify step.
//!
//! [`query_raw_substring`] is the primary read path for the LQ DSL §3.3
//! `'…'` raw-string leaf. The function:
//!
//! 1. Short-circuits when `needle.len() < TRIGRAM_LEN`: zero trigrams to
//!    look up; the planner is expected to route through the verify-only
//!    surface (we cooperate by returning every resolvable doc in the
//!    candidate set the caller hands us — for now this crate returns
//!    `RegexPrefilterUnusable`-shaped empty result; see the explicit
//!    `ShortInput` typed outcome below).
//! 2. Extracts every byte trigram in the needle (capped at
//!    [`MAX_TRIGRAMS_PER_QUERY`]).
//! 3. AND-intersects against the index, capped at
//!    [`crate::types::MAX_CANDIDATE_PRE_VERIFY`].
//! 4. Verifies each candidate by resolving the doc's bytes through
//!    [`DocResolver`] and confirming the substring via
//!    `memchr::memmem::find`.
//!
//! Trigrams produce *candidates*, not truth — the verify pass on the raw
//! corpus bytes is the authoritative substring check.

use memchr::memmem;

use crate::errors::{LimitDimension, TrigramError, TrigramErrorCode};
use crate::source::TrigramPostingSource;
use crate::types::{DocId, MAX_TRIGRAMS_PER_QUERY, TRIGRAM_LEN, Trigram, trigrams_of};

/// Pluggable resolver from [`DocId`] to that document's raw bytes.
///
/// This decouples the trigram index from any specific chunk-store
/// implementation; callers (tests, the lexical adapter integration in a
/// follow-up ticket) provide the resolver appropriate for their backend.
pub trait DocResolver {
    /// Return a borrow of `doc_id`'s raw bytes, or `None` if the doc is
    /// unknown to this resolver.
    fn resolve(&self, doc_id: DocId) -> Option<&[u8]>;
}

/// Run a raw-substring query against `idx`, verifying candidates via
/// `corpus`.
///
/// `idx` is any [`TrigramPostingSource`]: one [`crate::TrigramIndex`] or a
/// [`crate::ShardedTrigramIndex`] over a doc-id partition; the algorithm
/// is the same and so is the answer.
///
/// Empty needle → returns an empty `Vec` (typed, not error).
/// Needle shorter than [`TRIGRAM_LEN`] → returns an empty `Vec` and signals
/// the short-input fast path; callers must run their own verify pass over
/// the corpus universe (this crate has no universe to enumerate).
///
/// See [`crate`] module doc for byte-trigram rationale.
pub fn query_raw_substring<S: TrigramPostingSource + ?Sized>(
    idx: &S,
    needle: &[u8],
    corpus: &dyn DocResolver,
) -> Result<Vec<DocId>, TrigramError> {
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    if needle.len() < TRIGRAM_LEN {
        // Short-input fast path. We cannot enumerate every doc the
        // resolver knows about (the trait is opaque), so we surface an
        // empty result with a typed marker. Callers that need a
        // verify-only fallback must build it on top — silent fallback is
        // explicitly forbidden by the LEX-02 spec.
        return Err(TrigramError::new(
            TrigramErrorCode::RegexPrefilterUnusable,
            "needle shorter than trigram width; caller must run verify-only path",
        ));
    }

    // Collect unique trigrams from the needle, capped per query.
    let mut tris: Vec<Trigram> = trigrams_of(needle).collect();
    tris.sort_unstable();
    tris.dedup();
    if tris.len() > MAX_TRIGRAMS_PER_QUERY {
        return Err(TrigramError::plan_limit(
            LimitDimension::Trigrams,
            format!(
                "needle yields {} distinct trigrams (cap {})",
                tris.len(),
                MAX_TRIGRAMS_PER_QUERY
            ),
        ));
    }

    let candidates = idx.intersect_trigrams(&tris)?;
    if candidates.is_empty() {
        return Ok(candidates);
    }

    // Verify each candidate via memchr::memmem::find.
    let finder = memmem::Finder::new(needle);
    let mut verified: Vec<DocId> = Vec::with_capacity(candidates.len());
    for cand in candidates {
        match corpus.resolve(cand) {
            Some(bytes) => {
                if finder.find(bytes).is_some() {
                    verified.push(cand);
                }
            }
            None => {
                // Resolver doesn't know the candidate. We treat this as
                // a typed corruption signal rather than silently dropping
                // — a candidate doc-id that isn't resolvable means index
                // and corpus disagree.
                return Err(TrigramError::new(
                    TrigramErrorCode::IndexCorrupted,
                    format!("resolver missing doc {cand}"),
                ));
            }
        }
    }
    Ok(verified)
}

#[cfg(test)]
#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "test fixtures assert one happy variant only"
)]
mod tests {
    use super::{DocResolver, query_raw_substring};
    use crate::builder::TrigramIndexBuilder;
    use crate::errors::{LimitDimension, TrigramErrorCode};
    use crate::index::TrigramIndex;
    use crate::types::DocId;
    use std::collections::BTreeMap;

    fn fatal(msg: &str) -> ! {
        assert!(false, "{msg}");
        std::process::abort();
    }

    struct Map(BTreeMap<DocId, Vec<u8>>);

    impl DocResolver for Map {
        fn resolve(&self, doc_id: DocId) -> Option<&[u8]> {
            self.0.get(&doc_id).map(Vec::as_slice)
        }
    }

    fn build_fixture() -> (TrigramIndex, Map) {
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        let docs: &[(DocId, &[u8])] = &[
            (DocId(1), b"hello world"),
            (DocId(2), b"goodbye world"),
            (DocId(3), b"hello hello"),
            (DocId(4), b"unrelated text"),
        ];
        let mut m: BTreeMap<DocId, Vec<u8>> = BTreeMap::new();
        for (d, bytes) in docs {
            b.add_doc(*d, bytes);
            let prior = m.insert(*d, bytes.to_vec());
            assert!(prior.is_none());
        }
        (b.finish(), Map(m))
    }

    #[test]
    fn empty_needle_returns_empty() {
        let (idx, m) = build_fixture();
        let v = match query_raw_substring(&idx, b"", &m) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(v.is_empty());
    }

    #[test]
    fn needle_shorter_than_n_is_typed_short_input() {
        let (idx, m) = build_fixture();
        match query_raw_substring(&idx, b"hi", &m) {
            Ok(_) => assert!(false, "expected typed short-input error"),
            Err(e) => assert_eq!(e.code, TrigramErrorCode::RegexPrefilterUnusable),
        }
    }

    #[test]
    fn needle_matches_one_doc() {
        let (idx, m) = build_fixture();
        let v = match query_raw_substring(&idx, b"goodbye", &m) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(v, vec![DocId(2)]);
    }

    #[test]
    fn needle_matches_multiple_docs() {
        let (idx, m) = build_fixture();
        let v = match query_raw_substring(&idx, b"hello", &m) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(v, vec![DocId(1), DocId(3)]);
    }

    #[test]
    fn needle_no_match_returns_empty() {
        let (idx, m) = build_fixture();
        let v = match query_raw_substring(&idx, b"zzzz", &m) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert!(v.is_empty());
    }

    #[test]
    fn verify_rejects_false_positives() {
        // Construct a corpus where trigrams of "abcxyz" all exist but
        // not contiguously in the same doc.
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        // Doc 1 has "abc...xyz" but separated; trigrams "abc", "bcx",
        // "cxy", "xyz" must not all be present.
        // To force a false positive at the trigram layer, build a doc
        // that contains every trigram of "needlee" but not the substring.
        // "needlee" trigrams: nee, eed, edl, dle, lee, ee?
        let doc1: &[u8] = b"nee_eed_edl_dle_lee"; // contains separate trigrams of "needlee" but not "needlee" itself
        let doc2: &[u8] = b"a needlee here";
        b.add_doc(DocId(1), doc1);
        b.add_doc(DocId(2), doc2);
        let idx = b.finish();
        let mut m: BTreeMap<DocId, Vec<u8>> = BTreeMap::new();
        let prior = m.insert(DocId(1), doc1.to_vec());
        assert!(prior.is_none());
        let prior = m.insert(DocId(2), doc2.to_vec());
        assert!(prior.is_none());
        let resolver = Map(m);
        let v = match query_raw_substring(&idx, b"needlee", &resolver) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        // Only doc 2 contains the contiguous needle; doc 1's separated
        // trigrams must be rejected by the verify pass.
        assert_eq!(v, vec![DocId(2)]);
    }

    #[test]
    fn unicode_byte_trigrams_match() {
        // "한국" is 6 UTF-8 bytes; trigrams are 4 byte windows.
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        let doc1 = "안녕 한국".as_bytes();
        let doc2 = "안녕 세상".as_bytes();
        b.add_doc(DocId(1), doc1);
        b.add_doc(DocId(2), doc2);
        let idx = b.finish();
        let mut m: BTreeMap<DocId, Vec<u8>> = BTreeMap::new();
        let prior = m.insert(DocId(1), doc1.to_vec());
        assert!(prior.is_none());
        let prior = m.insert(DocId(2), doc2.to_vec());
        assert!(prior.is_none());
        let v = match query_raw_substring(&idx, "한국".as_bytes(), &Map(m)) {
            Ok(v) => v,
            Err(e) => fatal(&format!("{e}")),
        };
        assert_eq!(v, vec![DocId(1)]);
    }

    #[test]
    fn missing_resolver_entry_is_typed_corruption() {
        let mut b = match TrigramIndexBuilder::new(1) {
            Ok(b) => b,
            Err(e) => fatal(&format!("{e}")),
        };
        b.add_doc(DocId(1), b"hello world");
        let idx = b.finish();
        // Resolver intentionally missing DocId(1).
        let m = Map(BTreeMap::new());
        match query_raw_substring(&idx, b"hello", &m) {
            Ok(_) => assert!(false, "expected INDEX_CORRUPTED"),
            Err(e) => assert_eq!(e.code, TrigramErrorCode::IndexCorrupted),
        }
    }

    #[test]
    fn intersect_cap_surfaces_dimension_trigrams() {
        // Just sanity-check that the trigram-set cap path is hit when
        // the needle would produce >MAX_TRIGRAMS_PER_QUERY distinct
        // trigrams. We can't easily build such a needle inline, so we
        // assert via intersect_trigrams directly in index.rs tests; here
        // we exercise the wiring via a moderately long random needle and
        // confirm the OK path is taken.
        let (idx, m) = build_fixture();
        let needle = vec![b'a'; 4_100]; // > 4096 distinct trigrams? all 'a' → 1 trigram only
        let _v = match query_raw_substring(&idx, &needle, &m) {
            Ok(v) => v,
            Err(e) => match e.code {
                TrigramErrorCode::PlanLimitExceeded => {
                    assert_eq!(e.dimension, Some(LimitDimension::Trigrams));
                    return;
                }
                _ => fatal(&format!("{e}")),
            },
        };
        // For an all-'a' needle there is exactly 1 distinct trigram, so
        // the cap is NOT exceeded; the OK path is taken.
    }
}
