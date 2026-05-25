//! Property test — cross-generation `from_prior` equivalence.
//!
//! For any partition of a list of upserts into a "prior" prefix and a
//! "new deltas" suffix, building gen N via
//! `SemanticIndexBuilder::from_prior(scratch(prior), N) + suffix` must
//! produce the same CBOR encoding as a scratch build of the equivalent
//! merged ops at gen N.
//!
//! The HNSW variant only asserts determinism of `from_prior` — graph
//! topology is path-dependent, so we compare two `from_prior` rebuilds
//! of the same prior, not the prior itself. See `src/hnsw.rs`'s
//! `HnswIndexBuilder::from_prior` doc for the determinism contract.
//!
//! Proptest 256 cases each.

use std::collections::BTreeMap;

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_semantic::{
    DocId, Embedding, HnswIndexBuilder, HnswParams, SemanticIndexBuilder,
};

const DIM: usize = 4;

fn fixed_vec() -> impl Strategy<Value = Vec<f32>> {
    vec(-2.0_f32..=2.0_f32, DIM..=DIM)
}

fn op_strategy() -> impl Strategy<Value = (u64, Vec<f32>)> {
    (0u64..=8u64, fixed_vec())
}

fn ops_strategy() -> impl Strategy<Value = Vec<(u64, Vec<f32>)>> {
    vec(op_strategy(), 1..=12)
}

fn make_embedding(v: &[f32]) -> Embedding {
    let cleaned: Vec<f32> = v
        .iter()
        .map(|f| if f.is_finite() { *f } else { 0.0_f32 })
        .collect();
    let Ok(e) = Embedding::new(cleaned) else {
        std::process::abort();
    };
    e
}

fn build_semantic_scratch(generation: u64, ops: &[(u64, Vec<f32>)]) -> Vec<u8> {
    let Ok(mut b) = SemanticIndexBuilder::new(generation, 4) else {
        std::process::abort();
    };
    for (id, v) in ops {
        let e = make_embedding(v);
        if b.upsert_embedding(DocId(*id), &e).is_err() {
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

    /// `from_prior(scratch(prior_ops), new_gen) + new_ops` must equal
    /// `scratch(merged_ops)` at `new_gen`, where `merged_ops` =
    /// final-state(prior_ops) ++ new_ops.
    #[test]
    fn semantic_from_prior_plus_deltas_equals_scratch(
        prior_gen in 1u64..=5_000u64,
        new_gen in 5_001u64..=10_000u64,
        prior_ops in ops_strategy(),
        new_ops in ops_strategy(),
    ) {
        // Build prior at prior_gen.
        let Ok(mut prior_b) = SemanticIndexBuilder::new(prior_gen, 4) else {
            std::process::abort();
        };
        for (id, v) in &prior_ops {
            let e = make_embedding(v);
            if prior_b.upsert_embedding(DocId(*id), &e).is_err() {
                std::process::abort();
            }
        }
        let prior = prior_b.finish();

        // Build A: from_prior + apply new_ops.
        let Ok(mut next_b) = SemanticIndexBuilder::from_prior(&prior, new_gen) else {
            std::process::abort();
        };
        for (id, v) in &new_ops {
            let e = make_embedding(v);
            if next_b.upsert_embedding(DocId(*id), &e).is_err() {
                std::process::abort();
            }
        }
        let idx_a = next_b.finish();

        // Build B: scratch of (final-state(prior_ops) ++ new_ops) at new_gen.
        let mut state: BTreeMap<u64, Vec<f32>> = BTreeMap::new();
        for (id, v) in &prior_ops {
            let cleaned: Vec<f32> = v
                .iter()
                .map(|f| if f.is_finite() { *f } else { 0.0_f32 })
                .collect();
            let _replaced: Option<Vec<f32>> = state.insert(*id, cleaned);
        }
        let mut merged: Vec<(u64, Vec<f32>)> = state.into_iter().collect();
        for op in &new_ops {
            merged.push(op.clone());
        }
        let buf_b = build_semantic_scratch(new_gen, &merged);

        let mut buf_a: Vec<u8> = Vec::new();
        if idx_a.serialize_cbor(&mut buf_a).is_err() {
            std::process::abort();
        }
        prop_assert_eq!(buf_a, buf_b);
    }

    /// `from_prior` with no deltas preserves every doc and the dim
    /// (modulo the bumped generation).
    #[test]
    fn semantic_from_prior_zero_deltas_preserves_state(
        prior_gen in 1u64..=5_000u64,
        new_gen in 5_001u64..=10_000u64,
        ops in ops_strategy(),
    ) {
        let Ok(mut b) = SemanticIndexBuilder::new(prior_gen, 4) else {
            std::process::abort();
        };
        for (id, v) in &ops {
            let e = make_embedding(v);
            if b.upsert_embedding(DocId(*id), &e).is_err() {
                std::process::abort();
            }
        }
        let prior = b.finish();

        let Ok(next_b) = SemanticIndexBuilder::from_prior(&prior, new_gen) else {
            std::process::abort();
        };
        let idx = next_b.finish();
        prop_assert_eq!(idx.generation(), new_gen);
        prop_assert_eq!(idx.dim(), prior.dim());
        prop_assert_eq!(idx.corpus_size(), prior.corpus_size());
        for (doc, vec) in prior.iter() {
            let Some(got) = idx.get(doc) else {
                prop_assert!(false, "missing doc {doc}");
                return Ok(());
            };
            prop_assert_eq!(got, vec);
        }
    }

    /// HnswIndexBuilder: two `from_prior` rebuilds of the same prior at
    /// the same `new_gen` are byte-identical (determinism of the
    /// rebuild path, no path-dependent surprises).
    #[test]
    fn hnsw_from_prior_is_deterministic(
        prior_gen in 1u64..=5_000u64,
        new_gen in 5_001u64..=10_000u64,
        ops in ops_strategy(),
    ) {
        let Ok(mut prior_b) = HnswIndexBuilder::new(prior_gen, 4, HnswParams::DEFAULTS) else {
            std::process::abort();
        };
        for (id, v) in &ops {
            let e = make_embedding(v);
            if prior_b.upsert_embedding(DocId(*id), &e).is_err() {
                std::process::abort();
            }
        }
        let prior = prior_b.finish();

        let Ok(a_b) = HnswIndexBuilder::from_prior(&prior, new_gen) else {
            std::process::abort();
        };
        let Ok(b_b) = HnswIndexBuilder::from_prior(&prior, new_gen) else {
            std::process::abort();
        };
        let a = a_b.finish();
        let b = b_b.finish();
        let mut buf_a: Vec<u8> = Vec::new();
        let mut buf_b: Vec<u8> = Vec::new();
        if ciborium::ser::into_writer(&a, &mut buf_a).is_err() {
            std::process::abort();
        }
        if ciborium::ser::into_writer(&b, &mut buf_b).is_err() {
            std::process::abort();
        }
        prop_assert_eq!(buf_a, buf_b);
    }

    /// HnswIndexBuilder: `from_prior(prior, gen) + new_ops` is
    /// deterministic — replay the same suffix and the resulting CBOR
    /// bytes are identical.
    #[test]
    fn hnsw_from_prior_plus_deltas_is_deterministic(
        prior_gen in 1u64..=5_000u64,
        new_gen in 5_001u64..=10_000u64,
        prior_ops in ops_strategy(),
        new_ops in ops_strategy(),
    ) {
        let Ok(mut prior_b) = HnswIndexBuilder::new(prior_gen, 4, HnswParams::DEFAULTS) else {
            std::process::abort();
        };
        for (id, v) in &prior_ops {
            let e = make_embedding(v);
            if prior_b.upsert_embedding(DocId(*id), &e).is_err() {
                std::process::abort();
            }
        }
        let prior = prior_b.finish();

        let build_next = |suffix: &[(u64, Vec<f32>)]| -> Vec<u8> {
            let Ok(mut next_b) = HnswIndexBuilder::from_prior(&prior, new_gen) else {
                std::process::abort();
            };
            for (id, v) in suffix {
                let e = make_embedding(v);
                if next_b.upsert_embedding(DocId(*id), &e).is_err() {
                    std::process::abort();
                }
            }
            let idx = next_b.finish();
            let mut buf: Vec<u8> = Vec::new();
            if ciborium::ser::into_writer(&idx, &mut buf).is_err() {
                std::process::abort();
            }
            buf
        };

        let a = build_next(&new_ops);
        let b = build_next(&new_ops);
        prop_assert_eq!(a, b);
    }
}
