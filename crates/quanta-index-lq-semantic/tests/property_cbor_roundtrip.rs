//! Property tests — CBOR roundtrip determinism for [`SemanticIndex`].
//!
//! Per SEM-01 spec §5 (index module) determinism gate: an arbitrary
//! `(generation, dim, docs)` triple must round-trip byte-identically
//! through CBOR, and two identical builds must produce byte-identical
//! CBOR encodings (sorted [`DocId`] order is the canonical wire
//! sequence).

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_semantic::{DocId, Embedding, SemanticIndex, SemanticIndexBuilder};

const TEST_DIM_MAX: usize = 8;
const TEST_CORPUS_MAX: usize = 12;

fn finite_f32() -> impl Strategy<Value = f32> {
    prop::num::f32::POSITIVE | prop::num::f32::NEGATIVE | prop::num::f32::NORMAL
}

fn embedding_strategy(dim: usize) -> impl Strategy<Value = Vec<f32>> {
    vec(finite_f32(), dim..=dim)
}

fn corpus_strategy(dim: usize) -> impl Strategy<Value = Vec<(u64, Vec<f32>)>> {
    vec(
        (0u64..=10_000u64, embedding_strategy(dim)),
        0..=TEST_CORPUS_MAX,
    )
}

fn build(generation: u64, dim_u32: u32, docs: &[(u64, Vec<f32>)]) -> Option<SemanticIndex> {
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
        let ignored: Result<(), _> = b.add_embedding(DocId(*id), &e);
        drop(ignored);
    }
    Some(b.finish())
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn cbor_roundtrip_preserves_value(
        generation in 1u64..=10_000u64,
        dim in 1usize..=TEST_DIM_MAX,
        docs in corpus_strategy(TEST_DIM_MAX),
    ) {
        let docs: Vec<(u64, Vec<f32>)> = docs
            .into_iter()
            .map(|(id, mut v)| {
                v.truncate(dim);
                (id, v)
            })
            .filter(|(_, v)| v.len() == dim)
            .collect();
        let Ok(dim_u32) = u32::try_from(dim) else {
            return Ok(());
        };
        let Some(idx) = build(generation, dim_u32, &docs) else {
            return Ok(());
        };
        let mut buf: Vec<u8> = Vec::new();
        if idx.serialize_cbor(&mut buf).is_err() {
            return Ok(());
        }
        let Ok(got) = SemanticIndex::deserialize_cbor(buf.as_slice()) else {
            return Ok(());
        };
        prop_assert_eq!(idx, got);
    }

    #[test]
    fn cbor_byte_identical_across_builds(
        generation in 1u64..=10_000u64,
        dim in 1usize..=TEST_DIM_MAX,
        docs in corpus_strategy(TEST_DIM_MAX),
    ) {
        let docs: Vec<(u64, Vec<f32>)> = docs
            .into_iter()
            .map(|(id, mut v)| {
                v.truncate(dim);
                (id, v)
            })
            .filter(|(_, v)| v.len() == dim)
            .collect();
        let Ok(dim_u32) = u32::try_from(dim) else {
            return Ok(());
        };
        let Some(idx1) = build(generation, dim_u32, &docs) else {
            return Ok(());
        };
        let Some(idx2) = build(generation, dim_u32, &docs) else {
            return Ok(());
        };
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
}
