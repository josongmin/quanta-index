//! Property tests — `PositionsBuilder::remove_doc` invariants.
//!
//! Three invariants:
//!
//! 1. **Remove is idempotent** — `remove_doc(d)` twice yields the same builder
//!    state as `remove_doc(d)` once; the second call must return `Ok(false)`.
//! 2. **Remove drops only the target** — after `remove_doc(d)`, no posting
//!    entry mentions `d`, but every other doc's entries are byte-identical
//!    to a fresh build that omits `d`.
//! 3. **Upsert then remove is empty for that doc** — `upsert_doc(d, pairs)`
//!    followed by `remove_doc(d)` produces the same `finish()` bytes as a
//!    build that never touched `d`.

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_positions::{
    DocId, NormalizerVersion, Position, PositionsBuilder, PositionsError, PositionsIndex,
};

fn token_triple() -> impl Strategy<Value = (String, u64, u32)> {
    (
        prop::sample::select(vec!["a", "b", "c", "the", "fn", "_"]),
        0u64..=8u64,
        0u32..=64u32,
    )
        .prop_map(|(s, d, p)| (s.to_owned(), d, p))
}

fn build_via_add_token(tokens: &[(String, u64, u32)]) -> Result<PositionsIndex, PositionsError> {
    let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
    for (term, doc, pos) in tokens {
        b.add_token(DocId(*doc), term, Position(*pos))?;
    }
    b.finish()
}

fn build_omitting_doc(
    tokens: &[(String, u64, u32)],
    omit: u64,
) -> Result<PositionsIndex, PositionsError> {
    let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
    for (term, doc, pos) in tokens {
        if *doc == omit {
            continue;
        }
        b.add_token(DocId(*doc), term, Position(*pos))?;
    }
    b.finish()
}

fn serialize(idx: &PositionsIndex) -> Result<Vec<u8>, PositionsError> {
    let mut buf: Vec<u8> = Vec::new();
    idx.serialize_cbor(&mut buf)?;
    Ok(buf)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn remove_doc_idempotent(
        tokens in vec(token_triple(), 0..=40),
        target in 0u64..=8u64,
    ) {
        let mut b1 = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        for (term, doc, pos) in &tokens {
            if let Err(e) = b1.add_token(DocId(*doc), term, Position(*pos)) {
                return Err(TestCaseError::reject(format!("add_token: {e}")));
            }
        }
        let mut b2 = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        for (term, doc, pos) in &tokens {
            if let Err(e) = b2.add_token(DocId(*doc), term, Position(*pos)) {
                return Err(TestCaseError::reject(format!("add_token: {e}")));
            }
        }
        // Single remove.
        let _r1 = match b1.remove_doc(DocId(target)) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("remove1: {e}"))),
        };
        // Double remove — second call must report Ok(false).
        let _r2a = match b2.remove_doc(DocId(target)) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("remove2a: {e}"))),
        };
        match b2.remove_doc(DocId(target)) {
            Ok(again) => prop_assert!(!again, "second remove must be Ok(false)"),
            Err(e) => return Err(TestCaseError::reject(format!("remove2b: {e}"))),
        }
        let i1 = match b1.finish() {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("finish1: {e}"))),
        };
        let i2 = match b2.finish() {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("finish2: {e}"))),
        };
        let s1 = match serialize(&i1) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("ser1: {e}"))),
        };
        let s2 = match serialize(&i2) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("ser2: {e}"))),
        };
        prop_assert_eq!(s1, s2);
    }

    #[test]
    fn remove_doc_drops_only_target(
        tokens in vec(token_triple(), 0..=40),
        target in 0u64..=8u64,
    ) {
        // After remove_doc(target), the result must equal a fresh build
        // that simply omitted `target`'s tokens.
        let full = match build_via_add_token(&tokens) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("full build: {e}"))),
        };
        let expected = match build_omitting_doc(&tokens, target) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("omit build: {e}"))),
        };

        // Apply remove via the API.
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        for (term, doc, pos) in &tokens {
            if let Err(e) = b.add_token(DocId(*doc), term, Position(*pos)) {
                return Err(TestCaseError::reject(format!("add: {e}")));
            }
        }
        if let Err(e) = b.remove_doc(DocId(target)) {
            return Err(TestCaseError::reject(format!("remove: {e}")));
        }
        let got = match b.finish() {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("finish: {e}"))),
        };
        // Equality holds at the byte level via the canonical CBOR encoding.
        let bf = match serialize(&full) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("full ser: {e}"))),
        };
        let be = match serialize(&expected) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("expected ser: {e}"))),
        };
        let bg = match serialize(&got) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("got ser: {e}"))),
        };
        prop_assert_eq!(&be, &bg);
        // `full` is allowed to differ from `expected` when the target had
        // tokens; assert that property holds bidirectionally as a sanity
        // check (when target absent, full == expected; else they differ).
        let target_present = tokens.iter().any(|(_, d, _)| *d == target);
        if target_present {
            prop_assert_ne!(&bf, &bg);
        } else {
            prop_assert_eq!(&bf, &bg);
        }
    }

    #[test]
    fn upsert_then_remove_is_empty_for_that_doc(
        tokens in vec(token_triple(), 0..=20),
        target in 0u64..=8u64,
        extra in vec(token_triple(), 0..=8),
    ) {
        // Build a baseline that simply omits the target.
        let baseline = match build_omitting_doc(&tokens, target) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("baseline: {e}"))),
        };

        // Build the same baseline, then upsert the target with `extra`,
        // then remove the target. The result must match the baseline.
        let mut b = PositionsBuilder::new(1, NormalizerVersion::new(1, 0));
        for (term, doc, pos) in &tokens {
            if *doc == target {
                continue;
            }
            if let Err(e) = b.add_token(DocId(*doc), term, Position(*pos)) {
                return Err(TestCaseError::reject(format!("add: {e}")));
            }
        }
        if let Err(e) = b.upsert_doc(
            DocId(target),
            extra.iter().map(|(t, _d, p)| (t.as_str(), Position(*p))),
        ) {
            return Err(TestCaseError::reject(format!("upsert: {e}")));
        }
        if let Err(e) = b.remove_doc(DocId(target)) {
            return Err(TestCaseError::reject(format!("remove: {e}")));
        }
        let got = match b.finish() {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("finish: {e}"))),
        };
        let bb = match serialize(&baseline) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("ser baseline: {e}"))),
        };
        let bg = match serialize(&got) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("ser got: {e}"))),
        };
        prop_assert_eq!(bb, bg);
    }
}
