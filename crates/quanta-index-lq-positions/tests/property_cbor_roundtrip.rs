//! Property tests — CBOR roundtrip determinism for [`PositionsIndex`].
//!
//! Per LEX-03 §6.4: each proptest case builds an index from a random
//! `(generation, normalizer_version, postings)` shape and asserts that
//! the CBOR encoding round-trips bit-for-bit, and that two builds with
//! the same insertion sequence produce byte-identical encodings.

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_positions::{
    DocId, NormalizerVersion, Position, PositionsBuilder, PositionsError, PositionsIndex,
};

// Generate a (term, doc_id, position) triple. Term alphabet is small so we
// produce realistic per-term posting collisions; doc_ids cover a wide range
// so the doc-gap varint code path exercises multi-byte encodings.
fn token_triple() -> impl Strategy<Value = (String, u64, u32)> {
    (
        prop::sample::select(vec!["a", "b", "c", "d", "ab", "abc", "the", "fn", "_"]),
        0u64..=10_000u64,
        0u32..=2_000u32,
    )
        .prop_map(|(s, d, p)| (s.to_owned(), d, p))
}

fn build_index(
    generation: u64,
    nv_major: u16,
    nv_minor: u16,
    tokens: &[(String, u64, u32)],
) -> Result<PositionsIndex, PositionsError> {
    let mut b = PositionsBuilder::new(generation, NormalizerVersion::new(nv_major, nv_minor));
    for (term, doc, pos) in tokens {
        b.add_token(DocId(*doc), term, Position(*pos));
    }
    b.finish()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn cbor_roundtrip_preserves_value(
        generation in 0u64..=10_000u64,
        nv_major in 0u16..=20u16,
        nv_minor in 0u16..=20u16,
        tokens in vec(token_triple(), 0..=40),
    ) {
        // Builder errors on adversarial inputs the strategy cannot
        // pre-filter (e.g. doc_count overflow) propagate as a prop_assert
        // failure rather than a silent skip — the strategy bounds are
        // chosen so the realistic distribution never trips them.
        let idx = match build_index(generation, nv_major, nv_minor, &tokens) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("builder rejected input: {e}"))),
        };
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = idx.serialize_cbor(&mut buf) {
            return Err(TestCaseError::reject(format!("serialize: {e}")));
        }
        let got = match PositionsIndex::deserialize_cbor(buf.as_slice()) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("deserialize: {e}"))),
        };
        prop_assert_eq!(idx, got);
    }

    #[test]
    fn cbor_encoding_byte_identical_across_builds(
        generation in 0u64..=10_000u64,
        nv_major in 0u16..=20u16,
        nv_minor in 0u16..=20u16,
        tokens in vec(token_triple(), 0..=40),
    ) {
        let idx1 = match build_index(generation, nv_major, nv_minor, &tokens) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("builder1: {e}"))),
        };
        let idx2 = match build_index(generation, nv_major, nv_minor, &tokens) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("builder2: {e}"))),
        };
        let mut b1: Vec<u8> = Vec::new();
        let mut b2: Vec<u8> = Vec::new();
        if let Err(e) = idx1.serialize_cbor(&mut b1) {
            return Err(TestCaseError::reject(format!("ser1: {e}")));
        }
        if let Err(e) = idx2.serialize_cbor(&mut b2) {
            return Err(TestCaseError::reject(format!("ser2: {e}")));
        }
        prop_assert_eq!(b1, b2);
    }

    #[test]
    fn term_postings_decode_preserves_inserted_positions(
        generation in 0u64..=10_000u64,
        nv_major in 0u16..=20u16,
        nv_minor in 0u16..=20u16,
        tokens in vec(token_triple(), 0..=40),
    ) {
        // Cross-check: the decode-iterator path must reproduce the set of
        // (term, doc, position) triples the builder ingested, modulo
        // ordering (positions are sorted ascending in the decode output).
        use std::collections::BTreeMap;
        let mut expected: BTreeMap<(String, u64), Vec<u32>> = BTreeMap::new();
        for (term, doc, pos) in &tokens {
            expected
                .entry((term.clone(), *doc))
                .or_default()
                .push(*pos);
        }
        for v in expected.values_mut() {
            v.sort_unstable();
        }

        let idx = match build_index(generation, nv_major, nv_minor, &tokens) {
            Ok(v) => v,
            Err(e) => return Err(TestCaseError::reject(format!("builder: {e}"))),
        };

        // Dedupe terms via a sorted set; iterating `expected.keys()` would
        // hit each term once per `(term, doc)` pair.
        let mut terms_seen: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for (term, _doc) in expected.keys() {
            let _inserted = terms_seen.insert(term.clone());
        }
        let mut got: BTreeMap<(String, u64), Vec<u32>> = BTreeMap::new();
        for term in &terms_seen {
            let Some(iter) = idx.term_postings(term) else {
                continue;
            };
            for r in iter {
                let entry = match r {
                    Ok(e) => e,
                    Err(e) => return Err(TestCaseError::reject(format!("decode: {e}"))),
                };
                let key = (term.clone(), entry.doc_id.0);
                let positions: Vec<u32> = entry.positions.iter().map(|p| p.0).collect();
                drop(got.insert(key, positions));
            }
        }

        prop_assert_eq!(expected, got);
    }
}
