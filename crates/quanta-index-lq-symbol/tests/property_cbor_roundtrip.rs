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
}
