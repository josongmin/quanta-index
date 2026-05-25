//! Property test — cross-generation `from_prior` equivalence.
//!
//! For any partition of a list of upserts into a "prior" prefix and a
//! "new deltas" suffix, building gen N from the prior + applying the
//! suffix via `from_prior` must produce the same CBOR encoding as a
//! scratch build of the full list at gen N. Proptest 256 cases.

use std::collections::BTreeMap;

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

fn build_scratch(generation: u64, ops: &[(u64, Vec<u8>)]) -> Vec<u8> {
    let Ok(mut b) = TrigramIndexBuilder::new(generation) else {
        std::process::abort();
    };
    for (id, bytes) in ops {
        if b.upsert_doc(DocId(*id), bytes).is_err() {
            std::process::abort();
        }
    }
    let idx = b.finish();
    let mut buf: Vec<u8> = Vec::new();
    if idx.serialize_cbor(&mut buf).is_err() {
        std::process::abort();
    }
    buf
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// `from_prior(scratch(prior_ops), new_gen) + replay(new_ops)`
    /// must equal `scratch(prior_ops ++ new_ops)` at the same `new_gen`.
    #[test]
    fn from_prior_plus_deltas_equals_scratch(
        prior_gen in 1u64..=5_000u64,
        new_gen in 5_001u64..=10_000u64,
        prior_ops in ops_strategy(),
        new_ops in ops_strategy(),
    ) {
        // Build prior at prior_gen.
        let Ok(mut prior_b) = TrigramIndexBuilder::new(prior_gen) else {
            std::process::abort();
        };
        for (id, bytes) in &prior_ops {
            if prior_b.upsert_doc(DocId(*id), bytes).is_err() {
                std::process::abort();
            }
        }
        let prior = prior_b.finish();

        // Build A: from_prior + apply new_ops.
        let Ok(mut next_b) = TrigramIndexBuilder::from_prior(&prior, new_gen) else {
            std::process::abort();
        };
        for (id, bytes) in &new_ops {
            if next_b.upsert_doc(DocId(*id), bytes).is_err() {
                std::process::abort();
            }
        }
        let idx_a = next_b.finish();

        // Build B: scratch of the equivalent merged ops at new_gen.
        // Merged ops = prior_ops's last-write-wins state + new_ops.
        let mut merged_state: BTreeMap<u64, Vec<u8>> = BTreeMap::new();
        for (id, bytes) in &prior_ops {
            let _replaced: Option<Vec<u8>> = merged_state.insert(*id, bytes.clone());
        }
        let mut merged_ops: Vec<(u64, Vec<u8>)> =
            merged_state.into_iter().collect();
        for op in &new_ops {
            merged_ops.push(op.clone());
        }
        let buf_b = build_scratch(new_gen, &merged_ops);

        let mut buf_a: Vec<u8> = Vec::new();
        if idx_a.serialize_cbor(&mut buf_a).is_err() {
            std::process::abort();
        }
        prop_assert_eq!(buf_a, buf_b);
    }

    /// `from_prior` with no deltas preserves the trigram dictionary
    /// (modulo the bumped generation).
    #[test]
    fn from_prior_zero_deltas_preserves_state(
        prior_gen in 1u64..=5_000u64,
        new_gen in 5_001u64..=10_000u64,
        ops in ops_strategy(),
    ) {
        let Ok(mut b) = TrigramIndexBuilder::new(prior_gen) else {
            std::process::abort();
        };
        for (id, bytes) in &ops {
            if b.upsert_doc(DocId(*id), bytes).is_err() {
                std::process::abort();
            }
        }
        let prior = b.finish();

        let Ok(next_b) = TrigramIndexBuilder::from_prior(&prior, new_gen) else {
            std::process::abort();
        };
        let idx = next_b.finish();
        prop_assert_eq!(idx.generation(), new_gen);
        // Every prior trigram → posting list must be preserved.
        for (tri, postings) in prior.iter() {
            prop_assert_eq!(idx.lookup(tri), postings);
        }
        prop_assert_eq!(idx.distinct_trigrams(), prior.distinct_trigrams());
    }
}
