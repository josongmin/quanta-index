//! Property tests — executor determinism (SEM-01 §5.6 / §6.1 UC-SEM-03).
//!
//! For an arbitrary `(corpus, query, top_k)` triple, two consecutive
//! invocations of [`query_cosine_topk`] must produce the byte-identical
//! result sequence. This is the spec-locked invariant that lets the
//! cross-instance reproducibility test in the wider system claim
//! byte-equal envelopes.

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_semantic::{
    DocId, Embedding, MAX_EMBEDDING_DIM, SemanticIndex, SemanticIndexBuilder, query_cosine_topk,
};

const TEST_DIM_MAX: usize = 16; // keep test fast; bounded by MAX_EMBEDDING_DIM
const TEST_CORPUS_MAX: usize = 24;

fn finite_f32() -> impl Strategy<Value = f32> {
    // Avoid the all-zero corner via a non-zero offset in test corpus
    // generation; here we let any finite value through and gate
    // zero-norm at the builder level.
    prop::num::f32::POSITIVE | prop::num::f32::NEGATIVE | prop::num::f32::NORMAL
}

fn embedding_strategy(dim: usize) -> impl Strategy<Value = Vec<f32>> {
    vec(finite_f32(), dim..=dim)
}

fn corpus_strategy(dim: usize) -> impl Strategy<Value = Vec<(u64, Vec<f32>)>> {
    vec(
        (0u64..=10_000u64, embedding_strategy(dim)),
        1..=TEST_CORPUS_MAX,
    )
}

fn build_corpus(generation: u64, dim_u32: u32, docs: &[(u64, Vec<f32>)]) -> Option<SemanticIndex> {
    let Ok(mut b) = SemanticIndexBuilder::new(generation, dim_u32) else {
        return None;
    };
    let mut seen: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
    for (id, vec) in docs {
        if !seen.insert(*id) {
            continue;
        }
        let Ok(e) = Embedding::new(vec.clone()) else {
            continue;
        };
        // Reject zero-norm: the cosine kernel would error and that's
        // the path we want to test, but the property test should focus
        // on the determinism invariant, not error propagation.
        let any_nonzero = vec.iter().any(|f| *f != 0.0_f32);
        if !any_nonzero {
            continue;
        }
        let ignored: Result<(), _> = b.add_embedding(DocId(*id), &e);
        drop(ignored);
    }
    Some(b.finish())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn topk_determinism_byte_identical(
        generation in 1u64..=10_000u64,
        dim in 1usize..=TEST_DIM_MAX,
        docs in corpus_strategy(TEST_DIM_MAX),
        q_raw in embedding_strategy(TEST_DIM_MAX),
        top_k in 1u32..=64u32,
    ) {
        // Truncate strategy-generated vectors to the chosen dim.
        let docs: Vec<(u64, Vec<f32>)> = docs
            .into_iter()
            .map(|(id, mut v)| {
                v.truncate(dim);
                (id, v)
            })
            .filter(|(_, v)| v.len() == dim)
            .collect();
        let mut q = q_raw;
        q.truncate(dim);
        if q.len() != dim {
            return Ok(());
        }
        // Skip zero-norm queries.
        if q.iter().all(|f| *f == 0.0_f32) {
            return Ok(());
        }
        let Ok(dim_u32) = u32::try_from(dim) else {
            return Ok(());
        };
        let Some(idx) = build_corpus(generation, dim_u32, &docs) else {
            return Ok(());
        };
        if idx.is_empty() {
            return Ok(());
        }
        let Ok(query) = Embedding::new(q) else {
            return Ok(());
        };

        let Ok(a) = query_cosine_topk(&idx, &query, top_k) else {
            return Ok(());
        };
        let Ok(b) = query_cosine_topk(&idx, &query, top_k) else {
            return Ok(());
        };
        prop_assert_eq!(a, b);
    }

    #[test]
    fn topk_bounded_by_min_corpus_and_k(
        dim in 1usize..=TEST_DIM_MAX,
        docs in corpus_strategy(TEST_DIM_MAX),
        q_raw in embedding_strategy(TEST_DIM_MAX),
        top_k in 1u32..=64u32,
    ) {
        let docs: Vec<(u64, Vec<f32>)> = docs
            .into_iter()
            .map(|(id, mut v)| {
                v.truncate(dim);
                (id, v)
            })
            .filter(|(_, v)| v.len() == dim)
            .collect();
        let mut q = q_raw;
        q.truncate(dim);
        if q.len() != dim {
            return Ok(());
        }
        if q.iter().all(|f| *f == 0.0_f32) {
            return Ok(());
        }
        let Ok(dim_u32) = u32::try_from(dim) else {
            return Ok(());
        };
        let Some(idx) = build_corpus(1, dim_u32, &docs) else {
            return Ok(());
        };
        let Ok(query) = Embedding::new(q) else {
            return Ok(());
        };
        let Ok(out) = query_cosine_topk(&idx, &query, top_k) else {
            return Ok(());
        };
        let Ok(k_us) = usize::try_from(top_k) else {
            return Ok(());
        };
        let want = core::cmp::min(idx.corpus_size(), k_us);
        prop_assert_eq!(out.len(), want);
    }
}

// Compile-time sanity: TEST_DIM_MAX must not exceed MAX_EMBEDDING_DIM.
const _: () = assert!(TEST_DIM_MAX <= MAX_EMBEDDING_DIM);
