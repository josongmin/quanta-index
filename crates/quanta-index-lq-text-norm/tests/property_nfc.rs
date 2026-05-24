//! Property invariants for the Unicode NFC / NFKC normalization layer.
//!
//! Three properties at 256 cases each (proptest budget pin; CI can scale
//! via `PROPTEST_CASES=1000`):
//!
//! 1. `nfc_idempotent` — NFC is a projection: `nfc(nfc(x)) == nfc(x)`.
//! 2. `tokenize_after_nfc_equiv_tokenize_after_double_nfc` —
//!    tokenization is stable under repeated NFC at the input boundary.
//! 3. `nfkc_fold_idempotent` — `CaseFold::NfkcLower` is idempotent on
//!    the `lowered` field of every token.

use proptest::prelude::*;
use quanta_index_lq_text_norm::{
    CaseFold, LangId, PatternType, Token, fold_case, normalize_nfc, tokenize_text,
};

fn arb_unicode_text() -> BoxedStrategy<String> {
    // Mix of ASCII, Latin precomposed/decomposed, ligatures, and roman
    // numerals — the surfaces NFC and NFKC actually move.
    prop::string::string_regex(
        "[A-Za-z0-9_ \\-\u{00E0}-\u{00FF}\u{0300}-\u{036F}\u{FB00}-\u{FB06}\u{2160}-\u{216F}]{0,64}",
    )
    .map_or_else(|_| Just(String::new()).boxed(), Strategy::boxed)
}

fn arb_lang() -> impl Strategy<Value = LangId> {
    prop::sample::select(vec![
        LangId::Rust,
        LangId::Python,
        LangId::TypeScript,
        LangId::JavaScript,
        LangId::Go,
    ])
}

fn arb_supported_pt() -> impl Strategy<Value = PatternType> {
    prop::sample::select(vec![
        PatternType::Literal,
        PatternType::Keyword,
        PatternType::Standard,
    ])
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 64,
        ..ProptestConfig::default()
    })]

    /// NFC is idempotent: applying it twice yields the same output as
    /// applying it once. This is the projection property the tokenizer's
    /// input-boundary contract relies on.
    #[test]
    fn nfc_idempotent(s in arb_unicode_text()) {
        let once = normalize_nfc(&s);
        let twice = normalize_nfc(&once);
        prop_assert_eq!(once, twice);
    }

    /// Tokenization is idempotent under repeated NFC at the input boundary.
    /// That is, `tokenize(s) == tokenize(nfc(s))` for any valid `&str`
    /// input — because `tokenize` already runs NFC on non-ASCII inputs,
    /// pre-normalizing the caller's string must not change the output.
    #[test]
    fn tokenize_after_nfc_equiv_tokenize_after_double_nfc(
        s in arb_unicode_text(),
        lang in arb_lang(),
        pt in arb_supported_pt(),
    ) {
        let pre = normalize_nfc(&s);
        let Ok(raw) = tokenize_text(&s, lang, pt) else {
            return Ok(());
        };
        let Ok(pre_norm) = tokenize_text(&pre, lang, pt) else {
            return Ok(());
        };
        prop_assert_eq!(raw, pre_norm);
    }

    /// `CaseFold::NfkcLower` is idempotent on the `lowered` field: applying
    /// the fold to an already-folded token leaves the `lowered` surface
    /// byte-identical.
    #[test]
    fn nfkc_fold_idempotent(
        s in arb_unicode_text(),
        lang in arb_lang(),
        pt in arb_supported_pt(),
    ) {
        let Ok(toks) = tokenize_text(&s, lang, pt) else {
            return Ok(());
        };
        let folded: Vec<Token> = toks
            .iter()
            .map(|t| fold_case(t, CaseFold::NfkcLower))
            .collect();
        let refolded: Vec<Token> = folded
            .iter()
            .map(|t| fold_case(t, CaseFold::NfkcLower))
            .collect();
        let a: Vec<Box<str>> = folded.iter().map(|t| t.lowered.clone()).collect();
        let b: Vec<Box<str>> = refolded.iter().map(|t| t.lowered.clone()).collect();
        prop_assert_eq!(a, b);
    }
}
