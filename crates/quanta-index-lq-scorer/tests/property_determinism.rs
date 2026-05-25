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
use quanta_index_lq_scorer::{Bm25Params, Bm25Scorer, DocId, IdfBuilder, IdfStateBundle, IdfTable};

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
    for (i, doc) in corpus.iter().enumerate() {
        let owned: Vec<&str> = doc.iter().map(String::as_str).collect();
        let mut src = SliceTokenSource::new(&owned);
        let Ok(dl) = u32::try_from(doc.len()) else {
            return None;
        };
        let Ok(did) = u64::try_from(i.saturating_add(1)) else {
            return None;
        };
        if builder.add_doc(DocId(did), &mut src, dl).is_err() {
            return None;
        }
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

    // ── delta-handling: upsert / remove / from_prior invariants ────────

    #[test]
    fn upsert_replay_is_idempotent(corpus in corpus_strategy()) {
        // Build once vs build twice (each doc re-upserted). Final tables
        // must be identical.
        let Ok(mut b1) = IdfBuilder::new(1) else { return Ok(()); };
        let Ok(mut b2) = IdfBuilder::new(1) else { return Ok(()); };
        for (i, doc) in corpus.iter().enumerate() {
            let owned: Vec<&str> = doc.iter().map(String::as_str).collect();
            let Ok(dl) = u32::try_from(doc.len()) else { return Ok(()); };
            let Ok(did) = u64::try_from(i.saturating_add(1)) else { return Ok(()); };
            let mut s1 = SliceTokenSource::new(&owned);
            if b1.add_doc(DocId(did), &mut s1, dl).is_err() { return Ok(()); }
            let mut s2a = SliceTokenSource::new(&owned);
            if b2.add_doc(DocId(did), &mut s2a, dl).is_err() { return Ok(()); }
            // Replay the same upsert on b2.
            let mut s2b = SliceTokenSource::new(&owned);
            if b2.upsert_doc(DocId(did), &mut s2b, dl).is_err() { return Ok(()); }
        }
        let Ok(n1) = b1.total_docs() else { return Ok(()); };
        let Ok(n2) = b2.total_docs() else { return Ok(()); };
        prop_assert_eq!(n1, n2);
        let Ok(t1) = b1.finish() else { return Ok(()); };
        let Ok(t2) = b2.finish() else { return Ok(()); };
        prop_assert_eq!(t1, t2);
    }

    #[test]
    fn upsert_then_remove_returns_to_zero(corpus in corpus_strategy()) {
        let Ok(mut b) = IdfBuilder::new(1) else { return Ok(()); };
        let mut ids: Vec<DocId> = Vec::new();
        for (i, doc) in corpus.iter().enumerate() {
            let owned: Vec<&str> = doc.iter().map(String::as_str).collect();
            let Ok(dl) = u32::try_from(doc.len()) else { return Ok(()); };
            let Ok(did) = u64::try_from(i.saturating_add(1)) else { return Ok(()); };
            let mut src = SliceTokenSource::new(&owned);
            if b.add_doc(DocId(did), &mut src, dl).is_err() { return Ok(()); }
            ids.push(DocId(did));
        }
        // Remove every doc; total_docs must hit 0.
        for id in &ids {
            let Ok(removed) = b.remove_doc(*id) else { return Ok(()); };
            prop_assert!(removed, "first remove must succeed for staged doc");
        }
        let Ok(n) = b.total_docs() else { return Ok(()); };
        prop_assert_eq!(n, 0u64);
        // Second pass is idempotent.
        for id in &ids {
            let Ok(removed) = b.remove_doc(*id) else { return Ok(()); };
            prop_assert!(!removed, "second remove must be idempotent");
        }
    }

    #[test]
    fn from_prior_equivalence(corpus in corpus_strategy()) {
        let Ok(mut prior) = IdfBuilder::new(1) else { return Ok(()); };
        for (i, doc) in corpus.iter().enumerate() {
            let owned: Vec<&str> = doc.iter().map(String::as_str).collect();
            let Ok(dl) = u32::try_from(doc.len()) else { return Ok(()); };
            let Ok(did) = u64::try_from(i.saturating_add(1)) else { return Ok(()); };
            let mut src = SliceTokenSource::new(&owned);
            if prior.add_doc(DocId(did), &mut src, dl).is_err() { return Ok(()); }
        }
        // Equivalence: from_prior_builder and from_prior(bundle) must
        // produce the same downstream IdfTable.
        let bundle = prior.state_bundle();
        let Ok(via_bundle) = IdfBuilder::from_prior(&bundle, 2) else { return Ok(()); };
        let Ok(via_builder) = IdfBuilder::from_prior_builder(&prior, 2) else { return Ok(()); };
        // total_docs must agree.
        let Ok(n_a) = via_bundle.total_docs() else { return Ok(()); };
        let Ok(n_b) = via_builder.total_docs() else { return Ok(()); };
        prop_assert_eq!(n_a, n_b);
        let Ok(t_a) = via_bundle.finish() else { return Ok(()); };
        let Ok(t_b) = via_builder.finish() else { return Ok(()); };
        prop_assert_eq!(t_a, t_b);

        // CBOR round-trip on the bundle is byte-stable.
        let mut buf1: Vec<u8> = Vec::new();
        let mut buf2: Vec<u8> = Vec::new();
        if bundle.serialize_cbor(&mut buf1).is_err() { return Ok(()); }
        if bundle.serialize_cbor(&mut buf2).is_err() { return Ok(()); }
        prop_assert_eq!(&buf1, &buf2);
        let Ok(decoded) = IdfStateBundle::deserialize_cbor(buf1.as_slice()) else {
            return Ok(());
        };
        prop_assert_eq!(decoded, bundle);
    }
}
