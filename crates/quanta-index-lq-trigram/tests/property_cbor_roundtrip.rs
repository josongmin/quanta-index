//! Property tests — CBOR roundtrip determinism for [`TrigramIndex`].
//!
//! Per LEX-02 §6.4 determinism gate: each proptest case builds an index
//! from a random `(generation, docs)` pair and asserts that the CBOR
//! encoding round-trips bit-for-bit and that two builds with the same
//! insertion sequence produce byte-identical CBOR.
//!
//! Fail-closed contract (TOPT-05 / WA-1): on the generated inputs every
//! serialize, deserialize, and in-cap intersect is total — an `Err` is a
//! defect, never a skip. Unexpected errors map to [`TestCaseError::fail`]
//! with the generated input attached. The only skips are documented
//! input-domain rejections: short inputs that never enter the index, and
//! queries past the API's trigram cap.

use proptest::collection::vec;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

use quanta_index_lq_trigram::{DocId, TrigramError, TrigramIndex, TrigramIndexBuilder};

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

/// Map an unexpected fallible-step error to a failing case, attaching the
/// generated input so the shrinker reports what broke.
fn fail_on_unexpected<T>(
    result: Result<T, TrigramError>,
    what: &str,
    generation: u64,
    docs: &[(u64, Vec<u8>)],
) -> Result<T, TestCaseError> {
    result.map_err(|err| {
        TestCaseError::fail(format!(
            "{what} unexpectedly failed for generation={generation} docs={docs:?}: {err}"
        ))
    })
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
        fail_on_unexpected(idx.serialize_cbor(&mut buf), "serialize", generation, &docs)?;
        let got = fail_on_unexpected(
            TrigramIndex::deserialize_cbor(buf.as_slice()),
            "deserialize",
            generation,
            &docs,
        )?;
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
        fail_on_unexpected(idx1.serialize_cbor(&mut b1), "serialize", generation, &docs)?;
        fail_on_unexpected(idx2.serialize_cbor(&mut b2), "serialize", generation, &docs)?;
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
                // Documented domain rejection: short inputs never enter
                // the index, so there is nothing to intersect.
                continue;
            }
            let tris: Vec<[u8; 3]> =
                quanta_index_lq_trigram::trigrams_of(bytes).collect();
            // Cap the trigram set at MAX_TRIGRAMS_PER_QUERY to stay
            // within the API.
            if tris.len() > quanta_index_lq_trigram::MAX_TRIGRAMS_PER_QUERY {
                // Documented domain rejection: the API refuses
                // over-cap queries typed instead of answering.
                continue;
            }
            let out = fail_on_unexpected(
                idx.intersect_trigrams(&tris),
                "intersect",
                generation,
                &docs,
            )?;
            prop_assert!(out.contains(&DocId(*id)));
        }
    }
}

/// Mutation controls: each proves its error class fails the case instead
/// of passing silently.
#[cfg(test)]
mod controls {
    use std::io::Write;

    use proptest::test_runner::TestCaseError;

    use quanta_index_lq_trigram::{
        DocId, MAX_CANDIDATE_PRE_VERIFY, TrigramIndex, TrigramIndexBuilder,
    };

    use super::{build_index, fail_on_unexpected};

    /// A writer that always fails: the serialize-then-map step must fail
    /// the case, carrying the generated input for the shrinker.
    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("control: writer always fails"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("control: writer always fails"))
        }
    }

    // Control setup is total by construction; only the mapped step is asserted.
    #[test]
    fn serialize_error_fails_the_case() {
        let docs = vec![(1, b"rust-embeddings".to_vec())];
        let idx = build_index(7, &docs);
        let failed = fail_on_unexpected(idx.serialize_cbor(FailingWriter), "serialize", 7, &docs);
        match failed {
            Err(TestCaseError::Fail(reason)) => {
                let message = format!("{reason:?}");
                assert!(
                    message.contains("generation=7"),
                    "the failure carries the generated input: {message}"
                );
            }
            other => panic!("a serialize error must fail the case, got {other:?}"),
        }
    }

    // Control setup is total by construction; only the mapped step is asserted.
    #[test]
    fn deserialize_error_fails_the_case() {
        let docs = vec![(1, b"rust-embeddings".to_vec())];
        let idx = build_index(7, &docs);
        let mut buf: Vec<u8> = Vec::new();
        idx.serialize_cbor(&mut buf).expect("valid bytes encode");
        // Truncated and empty encodings must decode to errors, and both
        // must fail the case rather than pass it.
        let mut truncated = buf.clone();
        let _dropped = truncated.pop();
        for (what, bytes) in [
            ("truncated", truncated.as_slice()),
            ("empty", [].as_slice()),
        ] {
            match fail_on_unexpected(
                TrigramIndex::deserialize_cbor(bytes),
                "deserialize",
                7,
                &docs,
            ) {
                Err(TestCaseError::Fail(_)) => {}
                other => {
                    panic!("a {what} encoding must fail the case, got {other:?}");
                }
            }
        }
        // A flipped byte may still decode to a different value; either the
        // decode errors (mapped to fail) or the roundtrip equality below
        // catches the mismatch. Both outcomes are red, never silent.
        let mut flipped = buf;
        let mid = flipped.len().div_euclid(2);
        let Some(mid_byte) = flipped.get_mut(mid) else {
            panic!("a midpoint byte exists: encoded bytes are never empty");
        };
        *mid_byte ^= 0xFF;
        match TrigramIndex::deserialize_cbor(flipped.as_slice()) {
            Err(_) => {}
            Ok(got) => assert_ne!(
                got, idx,
                "a bit-flip that still decodes must change the value, or the roundtrip assert catches it"
            ),
        }
    }

    // Control setup is total by construction; only the mapped step is asserted.
    #[test]
    fn over_cap_intersect_error_fails_the_case() {
        // One posting list past MAX_CANDIDATE_PRE_VERIFY: the intersect
        // deterministically errors typed, and the mapping fails the case.
        let over_cap = MAX_CANDIDATE_PRE_VERIFY.saturating_add(1);
        let mut builder = TrigramIndexBuilder::new(7).expect("generation valid");
        for id in 0..over_cap {
            let id64 = u64::try_from(id).expect("cap fits u64");
            builder.add_doc(DocId(id64), b"aaa");
        }
        let idx = builder.finish();
        let docs = vec![(0, b"aaa".to_vec())];
        let tris: Vec<[u8; 3]> = quanta_index_lq_trigram::trigrams_of(b"aaa").collect();
        match fail_on_unexpected(idx.intersect_trigrams(&tris), "intersect", 7, &docs) {
            Err(TestCaseError::Fail(_)) => {}
            other => panic!("an over-cap intersect error must fail the case, got {other:?}"),
        }
    }
}
