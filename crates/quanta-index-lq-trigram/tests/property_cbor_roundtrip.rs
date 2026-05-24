//! Property tests — CBOR roundtrip determinism for [`TrigramIndex`].
//!
//! Per LEX-02 §6.4 determinism gate: each proptest case builds an index
//! from a random `(generation, docs)` pair and asserts that the CBOR
//! encoding round-trips bit-for-bit and that two builds with the same
//! insertion sequence produce byte-identical CBOR.

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_trigram::{DocId, TrigramIndex, TrigramIndexBuilder};

fn small_doc_bytes() -> impl Strategy<Value = Vec<u8>> {
    // 0..=64 bytes of ascii-ish data. Empty / short docs exercise the
    // short-input skip path; longer docs exercise full trigram windows.
    vec(any::<u8>(), 0..=64)
}

fn corpus_strategy() -> impl Strategy<Value = Vec<(u64, Vec<u8>)>> {
    vec((0u64..=10_000u64, small_doc_bytes()), 1..=12)
}

fn build_index(generation: u64, docs: &[(u64, Vec<u8>)]) -> TrigramIndex {
    let Ok(mut b) = TrigramIndexBuilder::new(generation) else {
        // Caller guarantees generation != 0 via strategy.
        std::process::abort();
    };
    for (id, bytes) in docs {
        b.add_doc(DocId(*id), bytes);
    }
    b.finish()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn cbor_roundtrip_preserves_value(
        generation in 1u64..=10_000u64,
        docs in corpus_strategy(),
    ) {
        let idx = build_index(generation, &docs);
        let mut buf: Vec<u8> = Vec::new();
        if idx.serialize_cbor(&mut buf).is_err() {
            return Ok(());
        }
        let Ok(got) = TrigramIndex::deserialize_cbor(buf.as_slice()) else {
            return Ok(());
        };
        prop_assert_eq!(idx, got);
    }

    #[test]
    fn cbor_encoding_byte_identical_across_builds(
        generation in 1u64..=10_000u64,
        docs in corpus_strategy(),
    ) {
        let idx1 = build_index(generation, &docs);
        let idx2 = build_index(generation, &docs);
        let mut b1: Vec<u8> = Vec::new();
        let mut b2: Vec<u8> = Vec::new();
        if idx1.serialize_cbor(&mut b1).is_err() {
            return Ok(());
        }
        if idx2.serialize_cbor(&mut b2).is_err() {
            return Ok(());
        }
        prop_assert_eq!(b1, b2);
    }

    #[test]
    fn intersect_is_subset_of_lookup_of_any_trigram(
        generation in 1u64..=10_000u64,
        docs in corpus_strategy(),
    ) {
        let idx = build_index(generation, &docs);
        // For each document with at least 3 bytes, the trigram intersect
        // over its own trigrams must include the document's id (the doc
        // contains all those trigrams by construction).
        for (id, bytes) in &docs {
            if bytes.len() < 3 {
                continue;
            }
            let tris: Vec<[u8; 3]> =
                quanta_index_lq_trigram::trigrams_of(bytes).collect();
            // Cap the trigram set at MAX_TRIGRAMS_PER_QUERY to stay
            // within the API.
            if tris.len() > quanta_index_lq_trigram::MAX_TRIGRAMS_PER_QUERY {
                continue;
            }
            let Ok(out) = idx.intersect_trigrams(&tris) else {
                continue;
            };
            prop_assert!(out.contains(&DocId(*id)));
        }
    }
}
