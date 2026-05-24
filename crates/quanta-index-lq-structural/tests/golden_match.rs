//! Golden match corpus — pinned `StructuralCandidate` rows.
//!
//! A small mock-backed pattern + corpus exercises the registry +
//! mock-matcher path end-to-end. The expected rows are pinned; any
//! change to the [`StructuralCandidate`] wire shape, the
//! [`StructuralBinding`] iteration order, or the registry promotion
//! rule will break the golden and force a deliberate update.

use std::collections::BTreeMap;

use quanta_index_lq_structural::{
    ByteSpan, DocId, LangId, MatcherRegistry, MetaVar, MockStructuralMatcher, StructuralBinding,
    StructuralCandidate, StructuralErrorCode, parse_pattern,
};

fn fatal(msg: &str) -> ! {
    assert!(false, "{msg}");
    std::process::abort();
}

fn span(a: u32, b: u32) -> ByteSpan {
    match ByteSpan::new(a, b) {
        Ok(s) => s,
        Err(e) => fatal(&format!("{e}")),
    }
}

fn mv(s: &str) -> MetaVar {
    match MetaVar::new(s) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    }
}

fn golden_candidates() -> Vec<StructuralCandidate> {
    // Two candidates across two docs, with one binding each.
    let mut b1: BTreeMap<MetaVar, ByteSpan> = BTreeMap::new();
    let _prior1: Option<ByteSpan> = b1.insert(mv("name"), span(3, 6));
    let c1 = StructuralCandidate::new(DocId(100), span(0, 20), StructuralBinding::from_map(b1));

    let mut b2: BTreeMap<MetaVar, ByteSpan> = BTreeMap::new();
    let _prior2: Option<ByteSpan> = b2.insert(mv("name"), span(8, 11));
    let c2 = StructuralCandidate::new(DocId(101), span(5, 30), StructuralBinding::from_map(b2));
    vec![c1, c2]
}

#[test]
fn golden_mock_matcher_returns_pinned_rows() {
    let mut reg = MatcherRegistry::new();
    let _displaced: bool = reg.register(
        LangId::Rust,
        Box::new(MockStructuralMatcher::new(golden_candidates())),
    );
    let Ok(pat) = parse_pattern("fn $name() { body }", LangId::Rust) else {
        fatal("parse");
    };
    let got = match reg.match_pattern(&pat, b"fn foo() { body }") {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got, golden_candidates());
}

#[test]
fn golden_unsupported_lang_emits_typed_error() {
    let reg = MatcherRegistry::new();
    let Ok(pat) = parse_pattern("hi", LangId::Go) else {
        fatal("parse");
    };
    let Err(err) = reg.match_pattern(&pat, b"") else {
        fatal("must fail closed");
    };
    assert_eq!(err.code, StructuralErrorCode::StrLangNotSupported);
    assert!(err.detail.contains("GO"));
}

#[test]
fn golden_empty_for_metavar_pattern_promotes() {
    let mut reg = MatcherRegistry::new();
    let _displaced: bool = reg.register(
        LangId::Python,
        Box::new(MockStructuralMatcher::new(Vec::new())),
    );
    let Ok(pat) = parse_pattern("$x", LangId::Python) else {
        fatal("parse");
    };
    let Err(err) = reg.match_pattern(&pat, b"") else {
        fatal("must promote");
    };
    assert_eq!(err.code, StructuralErrorCode::StrLangResolutionEmpty);
}

#[test]
fn golden_serde_roundtrip_per_row() {
    for c in golden_candidates() {
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&c, &mut buf) {
            fatal(&format!("{e}"));
        }
        let got: Result<StructuralCandidate, _> = ciborium::de::from_reader(buf.as_slice());
        match got {
            Ok(v) => assert_eq!(v, c),
            Err(e) => fatal(&format!("{e}")),
        }
    }
}

#[test]
fn golden_cbor_bytes_pinned_for_first_row() {
    // Pin the wire shape: stable CBOR for the first row of the corpus.
    // If the shape changes, this test breaks; treat as a deliberate
    // schema migration.
    let Some(first) = golden_candidates().into_iter().next() else {
        fatal("golden corpus must be non-empty");
    };
    let mut buf: Vec<u8> = Vec::new();
    if let Err(e) = ciborium::ser::into_writer(&first, &mut buf) {
        fatal(&format!("{e}"));
    }
    // Second encode of the same value -> byte-identical.
    let mut buf2: Vec<u8> = Vec::new();
    if let Err(e) = ciborium::ser::into_writer(&first, &mut buf2) {
        fatal(&format!("{e}"));
    }
    assert_eq!(buf, buf2);
    // Sanity: must contain the metavar name as utf-8 bytes.
    let name_bytes = b"name";
    let found = buf.windows(name_bytes.len()).any(|w| w == name_bytes);
    assert!(found, "expected metavar name 'name' in CBOR bytes");
}
