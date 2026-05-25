//! Property test — upsert+remove invariant.
//!
//! For any sequence of upserts followed by removal of every touched
//! `doc_id`, the resulting index must be byte-identical to a never-built
//! (empty) index of the same generation. Covers both [`SemanticIndexBuilder`]
//! and [`HnswIndexBuilder`]. Proptest 256 cases.

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
    vec(op_strategy(), 1..=16)
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

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// SemanticIndexBuilder: upsert then remove every touched id → empty.
    #[test]
    fn semantic_upsert_then_remove_all_equals_empty(
        generation in 1u64..=10_000u64,
        ops in ops_strategy(),
    ) {
        let Ok(mut b) = SemanticIndexBuilder::new(generation, 4) else {
            std::process::abort();
        };
        let mut touched: std::collections::BTreeSet<u64> =
            std::collections::BTreeSet::new();
        for (id, v) in &ops {
            let e = make_embedding(v);
            if b.upsert_embedding(DocId(*id), &e).is_err() {
                std::process::abort();
            }
            let _newly: bool = touched.insert(*id);
        }
        for id in &touched {
            if b.remove_embedding(DocId(*id)).is_err() {
                std::process::abort();
            }
        }
        let idx_a = b.finish();
        let Ok(empty_b) = SemanticIndexBuilder::new(generation, 4) else {
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

    /// SemanticIndexBuilder: single upsert then remove of the same id is
    /// a no-op (byte-identical to an empty builder).
    #[test]
    fn semantic_single_upsert_then_remove_is_noop(
        generation in 1u64..=10_000u64,
        id in 0u64..=100u64,
        v in fixed_vec(),
    ) {
        let Ok(mut b) = SemanticIndexBuilder::new(generation, 4) else {
            std::process::abort();
        };
        let e = make_embedding(&v);
        if b.upsert_embedding(DocId(id), &e).is_err() {
            std::process::abort();
        }
        let Ok(removed) = b.remove_embedding(DocId(id)) else {
            std::process::abort();
        };
        prop_assert!(removed, "newly-upserted doc must remove");
        let idx = b.finish();
        let Ok(empty_b) = SemanticIndexBuilder::new(generation, 4) else {
            std::process::abort();
        };
        let empty = empty_b.finish();
        let mut buf_a: Vec<u8> = Vec::new();
        let mut buf_b: Vec<u8> = Vec::new();
        if idx.serialize_cbor(&mut buf_a).is_err() {
            std::process::abort();
        }
        if empty.serialize_cbor(&mut buf_b).is_err() {
            std::process::abort();
        }
        prop_assert_eq!(buf_a, buf_b);
    }

    /// HnswIndexBuilder: upsert then remove every touched id → empty.
    #[test]
    fn hnsw_upsert_then_remove_all_equals_empty(
        generation in 1u64..=10_000u64,
        ops in ops_strategy(),
    ) {
        let Ok(mut b) = HnswIndexBuilder::new(generation, 4, HnswParams::DEFAULTS) else {
            std::process::abort();
        };
        let mut touched: std::collections::BTreeSet<u64> =
            std::collections::BTreeSet::new();
        for (id, v) in &ops {
            let e = make_embedding(v);
            if b.upsert_embedding(DocId(*id), &e).is_err() {
                std::process::abort();
            }
            let _newly: bool = touched.insert(*id);
        }
        for id in &touched {
            if b.remove_embedding(DocId(*id)).is_err() {
                std::process::abort();
            }
        }
        let idx_a = b.finish();
        let Ok(empty_b) = HnswIndexBuilder::new(generation, 4, HnswParams::DEFAULTS) else {
            std::process::abort();
        };
        let idx_b = empty_b.finish();
        let mut buf_a: Vec<u8> = Vec::new();
        let mut buf_b: Vec<u8> = Vec::new();
        if ciborium::ser::into_writer(&idx_a, &mut buf_a).is_err() {
            std::process::abort();
        }
        if ciborium::ser::into_writer(&idx_b, &mut buf_b).is_err() {
            std::process::abort();
        }
        prop_assert_eq!(buf_a, buf_b);
    }

    /// HnswIndexBuilder: single upsert then remove of the same id is a
    /// no-op (byte-identical to an empty builder).
    #[test]
    fn hnsw_single_upsert_then_remove_is_noop(
        generation in 1u64..=10_000u64,
        id in 0u64..=100u64,
        v in fixed_vec(),
    ) {
        let Ok(mut b) = HnswIndexBuilder::new(generation, 4, HnswParams::DEFAULTS) else {
            std::process::abort();
        };
        let e = make_embedding(&v);
        if b.upsert_embedding(DocId(id), &e).is_err() {
            std::process::abort();
        }
        let Ok(removed) = b.remove_embedding(DocId(id)) else {
            std::process::abort();
        };
        prop_assert!(removed, "newly-upserted doc must remove");
        let idx = b.finish();
        let Ok(empty_b) = HnswIndexBuilder::new(generation, 4, HnswParams::DEFAULTS) else {
            std::process::abort();
        };
        let empty = empty_b.finish();
        let mut buf_a: Vec<u8> = Vec::new();
        let mut buf_b: Vec<u8> = Vec::new();
        if ciborium::ser::into_writer(&idx, &mut buf_a).is_err() {
            std::process::abort();
        }
        if ciborium::ser::into_writer(&empty, &mut buf_b).is_err() {
            std::process::abort();
        }
        prop_assert_eq!(buf_a, buf_b);
    }
}
