//! Bridge-packet carrier proof for native LQ directives (`into:codeql`,
//! `scope:results`, `with:lexical`). Sourcegraph syntax does not surface these
//! directives; the envelope must preserve them when they are already present on
//! the lowered `LqQuery`.

#![forbid(unsafe_code)]

use quanta_index_contract::{
    LQ_VERSION_TAG, LqDirective, LqExpr, LqLeaf, LqOptions, LqQuery, LqSpan,
};
use quanta_index_lq_bridge::{BridgeCandidate, TRANSLATOR_VERSION};

fn directive_query(directives: Vec<LqDirective>, source_syntax: &str) -> LqQuery {
    #[expect(
        clippy::manual_unwrap_or,
        clippy::option_if_let_else,
        reason = "Result::unwrap_or is disallowed by clippy.toml; saturate source length to u32::MAX"
    )]
    let span_len = match u32::try_from(source_syntax.len()) {
        Ok(len) => len,
        Err(_) => u32::MAX,
    };
    LqQuery {
        lq_version: LQ_VERSION_TAG,
        expr: LqExpr::Leaf(LqLeaf::Keyword("needle".to_string())),
        filters: Vec::new(),
        directives,
        options: LqOptions::defaults(),
        source_span: LqSpan::eof(span_len),
    }
}

fn assert_directive_packet(source_syntax: &str, directive: LqDirective) {
    let translated = directive_query(vec![directive.clone()], source_syntax);
    let candidate = BridgeCandidate::new(source_syntax, translated);
    assert_eq!(candidate.translator_version.as_ref(), TRANSLATOR_VERSION);
    assert_eq!(candidate.source_syntax.as_ref(), source_syntax);
    assert_eq!(candidate.translated.directives, vec![directive]);
}

#[test]
fn into_codeql_directive_survives_bridge_candidate_envelope() {
    assert_directive_packet("Iterator into:codeql", LqDirective::IntoCodeQl);
}

#[test]
fn scope_results_directive_survives_bridge_candidate_envelope() {
    assert_directive_packet("repo:r1 scope:results", LqDirective::ScopeResults);
}

#[test]
fn with_lexical_directive_survives_bridge_candidate_envelope() {
    assert_directive_packet(
        "with:lexical into:codeql /strcpy\\(/",
        LqDirective::WithLexical,
    );
}

#[test]
fn bridge_candidate_envelope_round_trips_directive_fields() {
    let translated = directive_query(
        vec![
            LqDirective::WithLexical,
            LqDirective::IntoCodeQl,
            LqDirective::ScopeResults,
        ],
        "with:lexical into:codeql scope:results needle",
    );
    let candidate =
        BridgeCandidate::new("with:lexical into:codeql scope:results needle", translated);
    let mut buf = Vec::new();
    ciborium::into_writer(&candidate, &mut buf).expect("encode bridge candidate");
    let decoded: BridgeCandidate =
        ciborium::from_reader(buf.as_slice()).expect("decode bridge candidate");
    assert_eq!(decoded.translated.directives.len(), 3);
    assert_eq!(
        decoded.translated.directives,
        vec![
            LqDirective::WithLexical,
            LqDirective::IntoCodeQl,
            LqDirective::ScopeResults,
        ]
    );
}
