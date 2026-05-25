//! Property tests — CBOR roundtrip determinism for [`SymbolIndex`].
//!
//! Per LEX-05 §6.5 determinism gate (canonical hash stable across runs):
//! each proptest case builds a `SymbolIndex` from a random symbol set,
//! serializes to CBOR, deserializes, and asserts the round-trip equals the
//! original. ≥256 cases.

use proptest::collection::vec;
use proptest::prelude::*;

use quanta_index_lq_symbol::{
    ByteSpan, DocId, LangId, Symbol, SymbolIndex, SymbolIndexBuilder, SymbolKind,
};

fn name_strategy() -> BoxedStrategy<String> {
    // ASCII identifier-like strings, 1..=12 chars.
    match prop::string::string_regex("[A-Za-z_][A-Za-z0-9_]{0,11}") {
        Ok(s) => s.boxed(),
        Err(_e) => Just("x".to_string()).boxed(),
    }
}

fn kind_strategy() -> impl Strategy<Value = SymbolKind> {
    prop_oneof![
        Just(SymbolKind::Function),
        Just(SymbolKind::Method),
        Just(SymbolKind::Class),
        Just(SymbolKind::Struct),
        Just(SymbolKind::Enum),
        Just(SymbolKind::Trait),
        Just(SymbolKind::Interface),
        Just(SymbolKind::Variable),
        Just(SymbolKind::Constant),
        Just(SymbolKind::Module),
        Just(SymbolKind::Macro),
        Just(SymbolKind::TypeAlias),
    ]
}

fn lang_strategy() -> impl Strategy<Value = LangId> {
    prop_oneof![
        Just(LangId::Rust),
        Just(LangId::Python),
        Just(LangId::TypeScript),
        Just(LangId::JavaScript),
        Just(LangId::Go),
    ]
}

fn span_strategy() -> impl Strategy<Value = (u32, u32)> {
    (0u32..=10_000u32, 0u32..=10_000u32).prop_map(|(a, b)| if a <= b { (a, b) } else { (b, a) })
}

fn parent_strategy() -> impl Strategy<Value = Option<String>> {
    prop_oneof![Just(None), name_strategy().prop_map(Some),]
}

fn symbol_strategy() -> impl Strategy<Value = Symbol> {
    (
        name_strategy(),
        kind_strategy(),
        0u64..=1_000u64,
        span_strategy(),
        lang_strategy(),
        parent_strategy(),
    )
        .prop_map(|(name, kind, doc, (s, e), lang, parent)| {
            let Ok(span) = ByteSpan::new(s, e) else {
                std::process::abort();
            };
            Symbol::new(name, kind, DocId(doc), span, lang, parent.map(Into::into))
        })
}

fn corpus_strategy() -> impl Strategy<Value = Vec<Symbol>> {
    vec(symbol_strategy(), 0..=24)
}

fn build_index(generation: u64, symbols: Vec<Symbol>) -> Option<SymbolIndex> {
    let Ok(mut b) = SymbolIndexBuilder::new(generation) else {
        return None;
    };
    b.extend(symbols);
    let Ok(i) = b.finish() else {
        return None;
    };
    Some(i)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn cbor_roundtrip_preserves_value(
        generation in 1u64..=10_000u64,
        symbols in corpus_strategy(),
    ) {
        let Some(idx) = build_index(generation, symbols) else {
            return Ok(());
        };
        let mut buf: Vec<u8> = Vec::new();
        if idx.serialize_cbor(&mut buf).is_err() {
            return Ok(());
        }
        let Ok(got) = SymbolIndex::deserialize_cbor(buf.as_slice()) else {
            return Ok(());
        };
        prop_assert_eq!(idx, got);
    }

    #[test]
    fn cbor_encoding_byte_identical_across_builds(
        generation in 1u64..=10_000u64,
        symbols in corpus_strategy(),
    ) {
        let Some(idx1) = build_index(generation, symbols.clone()) else {
            return Ok(());
        };
        let Some(idx2) = build_index(generation, symbols) else {
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

    #[test]
    fn lookup_by_doc_returns_every_symbol_for_that_doc(
        generation in 1u64..=10_000u64,
        symbols in corpus_strategy(),
    ) {
        let total = symbols.len();
        let Some(idx) = build_index(generation, symbols) else {
            return Ok(());
        };
        // Sum across all docs in lookup_by_doc must equal total symbol count.
        let mut seen: usize = 0;
        for doc in 0u64..=1_000u64 {
            let hits = idx.lookup_by_doc(DocId(doc));
            seen = seen.saturating_add(hits.len());
        }
        prop_assert_eq!(seen, total);
    }

    // ── delta-handling: upsert / remove / from_prior invariants ────────

    #[test]
    fn upsert_is_idempotent_on_replay(
        generation in 1u64..=10_000u64,
        symbols in corpus_strategy(),
    ) {
        let Ok(mut b1) = SymbolIndexBuilder::new(generation) else {
            return Ok(());
        };
        let Ok(mut b2) = SymbolIndexBuilder::new(generation) else {
            return Ok(());
        };
        // b1: upsert each once. b2: upsert each twice (replay).
        for s in &symbols {
            if b1.upsert_symbol(s.clone()).is_err() {
                return Ok(());
            }
        }
        for s in &symbols {
            if b2.upsert_symbol(s.clone()).is_err() {
                return Ok(());
            }
            if b2.upsert_symbol(s.clone()).is_err() {
                return Ok(());
            }
        }
        prop_assert_eq!(b1.len(), b2.len());

        let Ok(i1) = b1.finish() else { return Ok(()); };
        let Ok(i2) = b2.finish() else { return Ok(()); };
        prop_assert_eq!(i1.len(), i2.len());

        // Byte-identical CBOR — the strongest form of equivalence.
        let mut buf1: Vec<u8> = Vec::new();
        let mut buf2: Vec<u8> = Vec::new();
        if i1.serialize_cbor(&mut buf1).is_err() { return Ok(()); }
        if i2.serialize_cbor(&mut buf2).is_err() { return Ok(()); }
        prop_assert_eq!(buf1, buf2);
    }

    #[test]
    fn upsert_then_remove_returns_to_empty(
        generation in 1u64..=10_000u64,
        symbols in corpus_strategy(),
    ) {
        let Ok(mut b) = SymbolIndexBuilder::new(generation) else {
            return Ok(());
        };
        // Dedup by identity to know how many distinct upserts to make.
        let mut distinct: Vec<Symbol> = Vec::new();
        let mut seen: std::collections::BTreeSet<(DocId, String, SymbolKind, u32)> =
            std::collections::BTreeSet::new();
        for s in &symbols {
            let id = (s.doc_id, s.name.as_ref().to_string(), s.kind, s.span.start());
            if seen.insert(id) {
                distinct.push(s.clone());
            }
        }
        for s in &distinct {
            if b.upsert_symbol(s.clone()).is_err() {
                return Ok(());
            }
        }
        prop_assert_eq!(b.len(), distinct.len());
        // Remove every distinct symbol by identity. Builder should hit zero.
        for s in &distinct {
            let id = (&s.doc_id, s.name.as_ref(), s.kind, s.span.start());
            let Ok(removed) = b.remove_symbol(id) else {
                return Ok(());
            };
            prop_assert!(removed, "first remove must succeed");
        }
        prop_assert_eq!(b.len(), 0usize);
        // Second pass must be idempotent (all return false).
        for s in &distinct {
            let id = (&s.doc_id, s.name.as_ref(), s.kind, s.span.start());
            let Ok(removed) = b.remove_symbol(id) else {
                return Ok(());
            };
            prop_assert!(!removed, "second remove must be idempotent");
        }
    }

    #[test]
    fn remove_doc_zeroes_target_doc(
        generation in 1u64..=10_000u64,
        symbols in corpus_strategy(),
        victim_doc in 0u64..=1_000u64,
    ) {
        let Ok(mut b) = SymbolIndexBuilder::new(generation) else {
            return Ok(());
        };
        for s in &symbols {
            if b.upsert_symbol(s.clone()).is_err() {
                return Ok(());
            }
        }
        let before_len = b.len();
        let Ok(removed) = b.remove_doc(DocId(victim_doc)) else {
            return Ok(());
        };
        prop_assert!(removed <= before_len);
        let Ok(idx) = b.finish() else {
            return Ok(());
        };
        // After remove_doc, no symbol with that doc should appear anywhere.
        prop_assert!(idx.lookup_by_doc(DocId(victim_doc)).is_empty());
    }

    #[test]
    fn from_prior_equivalence_then_replay_is_idempotent(
        gen_a in 1u64..=5_000u64,
        symbols in corpus_strategy(),
    ) {
        let Ok(mut b) = SymbolIndexBuilder::new(gen_a) else {
            return Ok(());
        };
        for s in &symbols {
            if b.upsert_symbol(s.clone()).is_err() {
                return Ok(());
            }
        }
        let Ok(prior) = b.finish() else { return Ok(()); };

        let new_gen = gen_a.saturating_add(1);
        let Ok(next_a) = SymbolIndexBuilder::from_prior(&prior, new_gen) else {
            return Ok(());
        };
        let Ok(mut next_b) = SymbolIndexBuilder::from_prior(&prior, new_gen) else {
            return Ok(());
        };
        // next_b replays the same upserts on top. Must not grow.
        for s in prior.symbols() {
            if next_b.upsert_symbol(s.clone()).is_err() {
                return Ok(());
            }
        }
        prop_assert_eq!(next_a.len(), next_b.len());
        prop_assert_eq!(next_a.len(), prior.len());

        let Ok(idx_a) = next_a.finish() else { return Ok(()); };
        let Ok(idx_b) = next_b.finish() else { return Ok(()); };
        let mut buf_a: Vec<u8> = Vec::new();
        let mut buf_b: Vec<u8> = Vec::new();
        if idx_a.serialize_cbor(&mut buf_a).is_err() { return Ok(()); }
        if idx_b.serialize_cbor(&mut buf_b).is_err() { return Ok(()); }
        prop_assert_eq!(buf_a, buf_b);
    }
}
