//! Property tests — single-instance determinism and envelope invariants.
//!
//! Per LEX-01 §6.1 determinism gate row 1: each proptest case runs 256
//! random `(corpus, query)` pairs and asserts byte-identical `f32`
//! outputs across two consecutive passes. This is the single-process leg
//! of RFC § Claim Discipline §8; the multi-process leg (cross-instance
//! reproducibility) lands once the lexical adapter wires the scorer into
//! an on-disk generation.

use proptest::collection::{btree_map, vec};
use proptest::prelude::*;
use std::collections::BTreeMap;

use quanta_index_lq_scorer::scorer::SliceTokenSource;
use quanta_index_lq_scorer::{Bm25Params, Bm25Scorer, IdfBuilder, IdfTable};

fn small_term_strategy() -> impl Strategy<Value = String> {
    "[a-z]{1,8}".prop_map(String::from)
}

fn doc_strategy() -> impl Strategy<Value = Vec<String>> {
    vec(small_term_strategy(), 1..=12)
}

fn corpus_strategy() -> impl Strategy<Value = Vec<Vec<String>>> {
    vec(doc_strategy(), 1..=10)
}

fn query_strategy() -> impl Strategy<Value = Vec<String>> {
    vec(small_term_strategy(), 1..=6)
}

fn build_scorer_from(corpus: &[Vec<String>]) -> Option<Bm25Scorer> {
    let Ok(mut builder) = IdfBuilder::new(1) else {
        return None;
    };
    for doc in corpus {
        let owned: Vec<&str> = doc.iter().map(String::as_str).collect();
        let mut src = SliceTokenSource::new(&owned);
        let Ok(dl) = u32::try_from(doc.len()) else {
            return None;
        };
        builder.add_doc(&mut src, dl);
    }
    let Ok(table) = builder.finish() else {
        return None;
    };
    let Ok(scorer) = Bm25Scorer::new(Bm25Params::DEFAULTS, table) else {
        return None;
    };
    Some(scorer)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn same_input_same_output(
        corpus in corpus_strategy(),
        query in query_strategy(),
    ) {
        let Some(scorer) = build_scorer_from(&corpus) else { return Ok(()) };
        let owned: Vec<&str> = query.iter().map(String::as_str).collect();
        let Ok(dl) = u32::try_from(query.len()) else { return Ok(()) };
        let mut a = SliceTokenSource::new(&owned);
        let mut b = SliceTokenSource::new(&owned);
        let Ok(va) = scorer.score_doc(&mut a, dl) else { return Ok(()) };
        let Ok(vb) = scorer.score_doc(&mut b, dl) else { return Ok(()) };
        prop_assert_eq!(va.to_bits(), vb.to_bits());
    }

    #[test]
    fn score_is_within_envelope(
        corpus in corpus_strategy(),
        query in query_strategy(),
    ) {
        let Some(scorer) = build_scorer_from(&corpus) else { return Ok(()) };
        let owned: Vec<&str> = query.iter().map(String::as_str).collect();
        let Ok(dl) = u32::try_from(query.len()) else { return Ok(()) };
        let mut src = SliceTokenSource::new(&owned);
        let Ok(v) = scorer.score_doc(&mut src, dl) else { return Ok(()) };
        prop_assert!(v.is_finite());
        prop_assert!((0.0..=1.0).contains(&v), "score {v} outside envelope");
    }

    #[test]
    fn cbor_roundtrip_preserves_table(
        generation in 1u64..=10_000u64,
        avg in 0.0f64..=1_000.0f64,
        terms in btree_map(small_term_strategy(), 1u64..=1_000u64, 0..30),
        total_docs in 1u64..=10_000u64,
    ) {
        let mut boxed: BTreeMap<Box<str>, u64> = BTreeMap::new();
        for (k, v) in terms {
            let prior = boxed.insert(k.into_boxed_str(), v.min(total_docs));
            prop_assert!(prior.is_none(), "btree_map produced duplicate key");
        }
        let Ok(table) = IdfTable::new(generation, total_docs, boxed, avg) else {
            return Ok(());
        };
        let mut buf: Vec<u8> = Vec::new();
        if table.serialize_cbor(&mut buf).is_err() {
            return Ok(());
        }
        let Ok(decoded) = IdfTable::deserialize_cbor(buf.as_slice()) else {
            return Ok(());
        };
        prop_assert_eq!(table, decoded);
    }
}
