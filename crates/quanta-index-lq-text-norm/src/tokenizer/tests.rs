use super::{
    MAX_CHUNK_BYTES, Token, TokenKind, tokenize::split_identifier, tokenize::tokenize_literal,
    tokenize_text,
};
use crate::errors::{LexNormError, LexNormErrorCode};
use crate::lang::LangId;
use crate::patterntype::PatternType;

fn must_ok(label: &str, r: Result<Vec<Token>, LexNormError>) -> Vec<Token> {
    match r {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{label}: expected Ok, got Err({e})");
            Vec::new()
        }
    }
}

fn must_err(label: &str, r: Result<Vec<Token>, LexNormError>) -> LexNormErrorCode {
    match r {
        Ok(v) => {
            assert!(false, "{label}: expected Err, got {} tokens", v.len());
            LexNormErrorCode::EmptyTokenStream
        }
        Err(e) => e.code,
    }
}

#[test]
fn empty_input_yields_empty_token_stream() {
    let v = must_ok(
        "empty",
        tokenize_text("", LangId::Rust, PatternType::Standard),
    );
    assert!(v.is_empty());
}

#[test]
fn literal_does_not_split_camel_case() {
    let toks = must_ok(
        "literal-camel",
        tokenize_text("getUserName", LangId::Rust, PatternType::Literal),
    );
    assert_eq!(toks.len(), 1);
    let Some(t) = toks.first() else {
        assert!(false, "no token");
        return;
    };
    assert_eq!(&*t.surface, "getUserName");
    assert_eq!(&*t.lowered, "getusername");
    assert_eq!(t.kind, TokenKind::Word);
}

#[test]
fn literal_splits_only_on_whitespace() {
    let toks = match tokenize_literal("foo bar  baz") {
        Ok(v) => v,
        Err(e) => {
            assert!(false, "{e}");
            return;
        }
    };
    assert_eq!(toks.len(), 3);
    let surfaces: Vec<&str> = toks.iter().map(|t| &*t.surface).collect();
    assert_eq!(surfaces, vec!["foo", "bar", "baz"]);
}

#[test]
fn standard_splits_camel_case() {
    let toks = must_ok(
        "standard-camel",
        tokenize_text("getUserName", LangId::Rust, PatternType::Standard),
    );
    let surfaces: Vec<&str> = toks.iter().map(|t| &*t.surface).collect();
    assert!(surfaces.contains(&"getUserName"));
    assert!(surfaces.contains(&"get"));
    assert!(surfaces.contains(&"User"));
    assert!(surfaces.contains(&"Name"));
}

#[test]
fn standard_splits_snake_case() {
    let toks = must_ok(
        "standard-snake",
        tokenize_text("parse_query_v2", LangId::Rust, PatternType::Standard),
    );
    let surfaces: Vec<&str> = toks.iter().map(|t| &*t.surface).collect();
    assert!(surfaces.contains(&"parse_query_v2"));
    assert!(surfaces.contains(&"parse"));
    assert!(surfaces.contains(&"query"));
    assert!(surfaces.contains(&"v"));
    assert!(surfaces.contains(&"2"));
}

#[test]
fn standard_splits_acronym_then_word() {
    let parts = split_identifier("HTTPServer");
    assert_eq!(parts, vec!["HTTP", "Server"]);
}

#[test]
fn standard_splits_digit_boundaries() {
    let parts = split_identifier("foo123bar");
    assert_eq!(parts, vec!["foo", "123", "bar"]);
}

#[test]
fn standard_splits_kebab_case() {
    let parts = split_identifier("foo-bar-baz");
    assert_eq!(parts, vec!["foo", "bar", "baz"]);
}

#[test]
fn regexp_pattern_type_returns_typed_error() {
    let code = must_err(
        "regexp",
        tokenize_text("foo", LangId::Rust, PatternType::Regexp),
    );
    assert_eq!(code, LexNormErrorCode::UnknownPatternType);
}

#[test]
fn structural_pattern_type_returns_typed_error() {
    let code = must_err(
        "structural",
        tokenize_text("foo", LangId::Rust, PatternType::Structural),
    );
    assert_eq!(code, LexNormErrorCode::UnknownPatternType);
}

#[test]
fn unknown_lang_returns_typed_error() {
    let code = must_err(
        "unknown-lang",
        tokenize_text("foo", LangId::Unknown, PatternType::Standard),
    );
    assert_eq!(code, LexNormErrorCode::NormalizerUnknownLang);
}

#[test]
fn oversized_chunk_returns_typed_error() {
    let big = "a".repeat(MAX_CHUNK_BYTES.saturating_add(1));
    let code = must_err(
        "oversized",
        tokenize_text(&big, LangId::Rust, PatternType::Standard),
    );
    assert_eq!(code, LexNormErrorCode::OversizedChunk);
}

#[test]
fn byte_offsets_index_into_input() {
    let toks = must_ok(
        "hello-world",
        tokenize_text("hello world", LangId::Rust, PatternType::Standard),
    );
    let Some(first) = toks.first() else {
        assert!(false, "no token");
        return;
    };
    assert_eq!(first.byte_start, 0);
    assert_eq!(first.byte_end, 5);
}

#[test]
fn single_identifier_emits_no_extra_parts() {
    let toks = must_ok(
        "single-foo",
        tokenize_text("foo", LangId::Rust, PatternType::Standard),
    );
    assert_eq!(toks.len(), 1);
}

#[test]
fn ascii_fast_path_offsets_match_raw_input() {
    // ASCII path must skip NFC and preserve the byte-identical offset
    // contract callers had before NFC landed.
    let raw = "hello world";
    let toks = must_ok(
        "ascii-fast-path",
        tokenize_text(raw, LangId::Rust, PatternType::Standard),
    );
    assert_eq!(toks.len(), 2);
    let Some(t0) = toks.first() else {
        assert!(false, "no first token");
        return;
    };
    assert_eq!(&*t0.surface, "hello");
    assert_eq!(t0.byte_start, 0);
    assert_eq!(t0.byte_end, 5);
}

#[test]
fn empty_input_after_nfc_path() {
    let toks = must_ok(
        "empty-literal",
        tokenize_text("", LangId::Rust, PatternType::Literal),
    );
    assert!(toks.is_empty());
}

#[test]
fn nfc_equivalence_precomposed_vs_decomposed() {
    // Precomposed `é` (U+00E9) and decomposed `e + U+0301` must
    // tokenize to identical token streams (modulo internal offsets,
    // which both anchor into the same NFC form).
    let precomposed = "caf\u{00E9}";
    let decomposed = "caf\u{0065}\u{0301}";
    let a = must_ok(
        "nfc-precomposed",
        tokenize_text(precomposed, LangId::Rust, PatternType::Standard),
    );
    let b = must_ok(
        "nfc-decomposed",
        tokenize_text(decomposed, LangId::Rust, PatternType::Standard),
    );
    assert_eq!(a, b);
}

#[test]
fn token_offsets_index_into_normalized_form() {
    // Decomposed input "caf e + combining-acute" is 6 bytes raw
    // (c=1, a=1, f=1, e=1, U+0301=2). After NFC it is "café" = 5
    // bytes (é = 2 bytes). The single emitted Word token must span
    // 0..5, i.e. the normalized form's byte range, NOT the raw 0..6.
    let decomposed = "caf\u{0065}\u{0301}";
    assert_eq!(decomposed.len(), 6);
    let toks = must_ok(
        "nfc-offsets",
        tokenize_text(decomposed, LangId::Rust, PatternType::Standard),
    );
    assert_eq!(toks.len(), 1);
    let Some(t0) = toks.first() else {
        assert!(false, "no token");
        return;
    };
    assert_eq!(t0.byte_start, 0);
    assert_eq!(t0.byte_end, 5);
}
