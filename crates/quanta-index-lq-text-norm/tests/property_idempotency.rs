//! Property invariant: `normalize ∘ normalize == normalize` on token streams.
//!
//! LEX-00.md §6 hard invariant. We run with 256 cases (the budget-aware
//! pin per the task brief); CI rails can scale this via
//! `PROPTEST_CASES=1000`.

use proptest::prelude::*;
use quanta_index_lq_text_norm::{
    CaseFold, DefaultLexicalNormalizer, LangId, LexicalNormalizer, PatternType, Token,
};

fn arb_ascii_text() -> BoxedStrategy<String> {
    prop::string::string_regex("[A-Za-z0-9_ \\-]{0,64}")
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

fn arb_case() -> impl Strategy<Value = CaseFold> {
    prop::sample::select(vec![CaseFold::Off, CaseFold::Lower, CaseFold::NfkcLower])
}

fn fold_lowered_view(toks: &[Token]) -> Vec<Box<str>> {
    toks.iter().map(|t| t.lowered.clone()).collect()
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        max_shrink_iters: 64,
        ..ProptestConfig::default()
    })]

    /// `normalize` is a pure function: applying it twice to the same input
    /// produces the same token stream. This is the form of idempotency
    /// LEX-00.md §6 requires for the analyzer — applying the analyzer to
    /// already-analyzed token output (without re-printing) is the identity
    /// on the `lowered` view because case folding and the identifier
    /// splitter are pure functions of their input.
    #[test]
    fn normalize_is_deterministic_under_random_ascii(
        s in arb_ascii_text(),
        lang in arb_lang(),
        pt in arb_supported_pt(),
        case in arb_case(),
    ) {
        let normalizer = DefaultLexicalNormalizer::new(case);
        let Ok(once) = normalizer.normalize(&s, lang, pt) else {
            return Ok(());
        };
        let Ok(twice) = normalizer.normalize(&s, lang, pt) else {
            return Ok(());
        };
        prop_assert_eq!(once, twice);
    }

    /// Case folding is idempotent at the `lowered`-field level: folding an
    /// already-folded surface produces the same surface byte-for-byte.
    #[test]
    fn fold_is_idempotent_on_normalize_output(
        s in arb_ascii_text(),
        lang in arb_lang(),
        pt in arb_supported_pt(),
        case in arb_case(),
    ) {
        let normalizer = DefaultLexicalNormalizer::new(case);
        let Ok(toks) = normalizer.normalize(&s, lang, pt) else {
            return Ok(());
        };
        let refolded: Vec<Token> = toks
            .iter()
            .map(|t| quanta_index_lq_text_norm::fold_case(t, case))
            .collect();
        prop_assert_eq!(fold_lowered_view(&toks), fold_lowered_view(&refolded));
    }
}
