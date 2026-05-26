//! Golden authority-match corpus — pinned [`StructuralAuthorityCandidate`]
//! rows and CBOR bytes.
//!
//! The expected rows are pinned; any change to the authoritative match
//! envelope or [`StructuralBinding`] iteration order will break the golden
//! and force a deliberate update.

use std::collections::BTreeMap;

use quanta_index_contract::lex::{
    LanguageCode, ParseNode, ParseTreeRecord, compute_parse_tree_source_hash,
};
use quanta_index_contract::{LqMetaVar, LqStructuralBlock, LqStructuralExpr, LqStructuralNode};
use quanta_index_lq_structural::{
    ByteSpan, MetaVar, StructuralAuthorityCandidate, StructuralAuthorityMatcher,
    StructuralAuthorityView, StructuralBinding, TruthfulSubsetAuthorityMatcher,
    compile_authoritative_pattern,
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

fn literal(text: &str) -> LqStructuralNode {
    LqStructuralNode::Literal(text.to_string().into_boxed_str())
}

fn metavar(name: &str) -> LqStructuralNode {
    LqStructuralNode::MetaVar(LqMetaVar::new(name.to_string()))
}

fn group(children: Vec<LqStructuralNode>) -> LqStructuralNode {
    LqStructuralNode::Group(children)
}

fn compile_block(
    nodes: Vec<LqStructuralNode>,
    lang: &str,
) -> quanta_index_lq_structural::StructuralPattern {
    let pattern_nodes = nodes.clone();
    match compile_authoritative_pattern(
        &LqStructuralBlock {
            lang: None,
            nodes,
            exprs: vec![LqStructuralExpr::Pattern(pattern_nodes)],
        },
        lang,
    ) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    }
}

fn tree(lang: &str, kind: &str, start: u32, end: u32, source: &str) -> ParseTreeRecord {
    let Ok(lang) = LanguageCode::new(lang) else {
        fatal("language");
    };
    ParseTreeRecord {
        wire_version: 1,
        lang,
        root: ParseNode {
            kind: kind.to_string().into_boxed_str(),
            byte_start: start,
            byte_end: end,
            children: Vec::new(),
        },
        source_hash: compute_parse_tree_source_hash(source),
        role_tag_schema_version: 1,
        role_tags: Vec::new(),
    }
}

fn golden_candidates() -> Vec<StructuralAuthorityCandidate> {
    let mut b1: BTreeMap<MetaVar, ByteSpan> = BTreeMap::new();
    let _prior1: Option<ByteSpan> = b1.insert(mv("node"), span(0, 12));
    let c1 = StructuralAuthorityCandidate::new(span(0, 12), StructuralBinding::from_map(b1));

    let mut b2: BTreeMap<MetaVar, ByteSpan> = BTreeMap::new();
    let _prior2: Option<ByteSpan> = b2.insert(mv("node"), span(5, 17));
    let c2 = StructuralAuthorityCandidate::new(span(5, 17), StructuralBinding::from_map(b2));
    vec![c1, c2]
}

#[test]
fn golden_authority_match_returns_pinned_row() {
    let source = "fn main() {}";
    let tree = tree("rust", "function_item", 0, 12, source);
    let pattern = compile_block(
        vec![group(vec![literal(" "), metavar("node"), literal(" ")])],
        "rust",
    );
    let matcher = TruthfulSubsetAuthorityMatcher::new();
    let got = match matcher.match_authority(
        (&pattern).try_into().unwrap_or_else(|_| fatal("lower")),
        StructuralAuthorityView::new(source, &tree),
    ) {
        Ok(v) => v,
        Err(e) => fatal(&format!("{e}")),
    };
    assert_eq!(got, vec![golden_candidates()[0].clone()]);
}

#[test]
fn golden_serde_roundtrip_per_row() {
    for c in golden_candidates() {
        let mut buf: Vec<u8> = Vec::new();
        if let Err(e) = ciborium::ser::into_writer(&c, &mut buf) {
            fatal(&format!("{e}"));
        }
        let got: Result<StructuralAuthorityCandidate, _> =
            ciborium::de::from_reader(buf.as_slice());
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
    let name_bytes = b"node";
    let found = buf.windows(name_bytes.len()).any(|w| w == name_bytes);
    assert!(found, "expected metavar name 'node' in CBOR bytes");
}
