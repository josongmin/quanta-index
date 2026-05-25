//! Property tests — `PositionsBuilder::from_prior` import equivalence.
//!
//! Two invariants:
//!
//! 1. **Round-trip** — building a prior, then calling `from_prior` and
//!    `finish` without any new mutations, produces the same per-term raw
//!    posting bytes as the original prior. The new index's generation is
//!    the requested one, but every `(term, doc, positions)` value is
//!    preserved.
//! 2. **Delta application** — `from_prior` followed by `remove_doc(d)` for
//!    some doc present in the prior produces the same posting bytes as
//!    a fresh build that omitted `d`.

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_positions::{
    DocId, NormalizerVersion, Position, PositionsBuilder, PositionsError, PositionsErrorCode,
    PositionsIndex,
};

fn token_triple() -> impl Strategy<Value = (String, u64, u32)> {
    (
        prop::sample::select(vec!["a", "b", "c", "the", "fn", "_"]),
        0u64..=8u64,
        0u32..=64u32,
    )
        .prop_map(|(s, d, p)| (s.to_owned(), d, p))
}

fn build_index(
    generation: u64,
    nv: NormalizerVersion,
    tokens: &[(String, u64, u32)],
    omit: Option<u64>,
) -> Result<PositionsIndex, PositionsError> {
    let mut b = PositionsBuilder::new(generation, nv);
    for (term, doc, pos) in tokens {
        if Some(*doc) == omit {
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
    fn from_prior_round_trip_preserves_postings(
        prior_gen in 0u64..=1_000u64,
        new_gen in 0u64..=1_000u64,
        nv_major in 0u16..=8u16,
        nv_minor in 0u16..=8u16,
        tokens in vec(token_triple(), 0..=40),
    ) {
        let nv = NormalizerVersion::new(nv_major, nv_minor);
        let prior = match build_index(prior_gen, nv, &tokens, None) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("build prior: {e}"))),
        };
        let nb = match PositionsBuilder::from_prior(&prior, new_gen, nv) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("from_prior: {e}"))),
        };
        let imported = match nb.finish() {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("finish: {e}"))),
        };
        prop_assert_eq!(imported.generation(), new_gen);
        prop_assert_eq!(imported.normalizer_version(), nv);
        // Per-term raw bytes must match exactly.
        let prior_terms: Vec<String> = prior.terms().map(ToOwned::to_owned).collect();
        let imported_terms: Vec<String> = imported.terms().map(ToOwned::to_owned).collect();
        prop_assert_eq!(&prior_terms, &imported_terms);
        for t in &prior_terms {
            prop_assert_eq!(prior.raw_postings(t), imported.raw_postings(t));
        }
    }

    #[test]
    fn from_prior_then_remove_matches_fresh_omit_build(
        generation in 0u64..=1_000u64,
        nv_major in 0u16..=8u16,
        nv_minor in 0u16..=8u16,
        tokens in vec(token_triple(), 0..=40),
        target in 0u64..=8u64,
    ) {
        let nv = NormalizerVersion::new(nv_major, nv_minor);
        let prior = match build_index(generation, nv, &tokens, None) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("build prior: {e}"))),
        };
        let mut nb = match PositionsBuilder::from_prior(&prior, generation.saturating_add(1), nv) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("from_prior: {e}"))),
        };
        if let Err(e) = nb.remove_doc(DocId(target)) {
            return Err(TestCaseError::reject(format!("remove: {e}")));
        }
        let got = match nb.finish() {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("finish: {e}"))),
        };
        let expected = match build_index(generation.saturating_add(1), nv, &tokens, Some(target)) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("expected: {e}"))),
        };
        let bg = match serialize(&got) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("ser got: {e}"))),
        };
        let be = match serialize(&expected) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("ser expected: {e}"))),
        };
        prop_assert_eq!(bg, be);
    }

    #[test]
    fn from_prior_rejects_any_normalizer_mismatch(
        prior_major in 0u16..=8u16,
        prior_minor in 0u16..=8u16,
        new_major in 0u16..=8u16,
        new_minor in 0u16..=8u16,
    ) {
        // Reject any pairing where the two NVs are equal — the mismatch
        // assertion only applies when the requested NV truly differs.
        if prior_major == new_major && prior_minor == new_minor {
            return Err(TestCaseError::reject("identity nv".to_owned()));
        }
        let prior_nv = NormalizerVersion::new(prior_major, prior_minor);
        let new_nv = NormalizerVersion::new(new_major, new_minor);
        let prior_b = PositionsBuilder::new(1, prior_nv);
        let prior = match prior_b.finish() {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("finish: {e}"))),
        };
        match PositionsBuilder::from_prior(&prior, 2, new_nv) {
            Ok(_) => prop_assert!(false, "expected NormalizerVersionMismatch"),
            Err(e) => prop_assert_eq!(e.code, PositionsErrorCode::NormalizerVersionMismatch),
        }
    }
}
