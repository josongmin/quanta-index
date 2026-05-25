//! Property test — upsert+remove invariant.
//!
//! For any sequence of upserts followed by a removal of every touched
//! `doc_id`, the resulting index must be byte-identical to a never-built
//! index (modulo generation). Proptest 256 cases.

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_trigram::{DocId, TrigramIndexBuilder};

fn small_doc_bytes() -> impl Strategy<Value = Vec<u8>> {
    vec(any::<u8>(), 0..=32)
}

fn op_strategy() -> impl Strategy<Value = (u64, Vec<u8>)> {
    (0u64..=8u64, small_doc_bytes())
}

fn ops_strategy() -> impl Strategy<Value = Vec<(u64, Vec<u8>)>> {
    vec(op_strategy(), 1..=24)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Upsert a batch, then remove every doc_id touched. The resulting
    /// index must encode to the same bytes as an empty index of the same
    /// generation.
    #[test]
    fn upsert_then_remove_all_equals_empty(
        generation in 1u64..=10_000u64,
        ops in ops_strategy(),
    ) {
        let Ok(mut b) = TrigramIndexBuilder::new(generation) else {
            std::process::abort();
        };
        let mut touched: std::collections::BTreeSet<u64> =
            std::collections::BTreeSet::new();
        for (id, bytes) in &ops {
            if b.upsert_doc(DocId(*id), bytes).is_err() {
                std::process::abort();
            }
            let _newly: bool = touched.insert(*id);
        }
        // Remove every touched doc_id.
        for id in &touched {
            let r = b.remove_doc(DocId(*id));
            if r.is_err() {
                std::process::abort();
            }
        }
        let idx_a = b.finish();

        // Reference: empty builder of the same generation.
        let Ok(empty_b) = TrigramIndexBuilder::new(generation) else {
            std::process::abort();
        };
        let idx_b = empty_b.finish();

        let mut buf_a: Vec<u8> = Vec::new();
        let mut buf_b: Vec<u8> = Vec::new();
        if idx_a.serialize_cbor(&mut buf_a).is_err() {
            std::process::abort();
        }
        if idx_b.serialize_cbor(&mut buf_b).is_err() {
            std::process::abort();
        }
        prop_assert_eq!(buf_a, buf_b);
    }

    /// Single upsert immediately followed by remove of the same id is a
    /// no-op against an empty index, regardless of content.
    #[test]
    fn single_upsert_then_remove_is_noop(
        generation in 1u64..=10_000u64,
        id in 0u64..=100u64,
        bytes in small_doc_bytes(),
    ) {
        let Ok(mut b) = TrigramIndexBuilder::new(generation) else {
            std::process::abort();
        };
        if b.upsert_doc(DocId(id), &bytes).is_err() {
            std::process::abort();
        }
        let Ok(removed_or_not) = b.remove_doc(DocId(id)) else {
            std::process::abort();
        };
        // remove must report `true` only when at least one trigram was
        // associated (content len >= 3); otherwise the doc was never
        // registered and remove returns false.
        if bytes.len() >= 3 {
            prop_assert!(removed_or_not);
        } else {
            prop_assert!(!removed_or_not);
        }
        let idx = b.finish();

        let Ok(empty_b) = TrigramIndexBuilder::new(generation) else {
            std::process::abort();
        };
        let empty_idx = empty_b.finish();

        let mut buf_a: Vec<u8> = Vec::new();
        let mut buf_b: Vec<u8> = Vec::new();
        if idx.serialize_cbor(&mut buf_a).is_err() {
            std::process::abort();
        }
        if empty_idx.serialize_cbor(&mut buf_b).is_err() {
            std::process::abort();
        }
        prop_assert_eq!(buf_a, buf_b);
    }
}
