//! Property tests for the PRE-NORM pipeline.
//!
//! Two harnesses, each 256 cases per the ticket requirement:
//!
//! 1. **Idempotency**: `normalize(normalize(parse(input))) == normalize(parse(input))`.
//! 2. **Hash determinism**: `canonical_hash(q) == canonical_hash(q)` —
//!    the hash is a pure function of the canonical form.
//!
//! Inputs are randomly generated LQ-like strings restricted to the
//! subset PRE-NORM is known to accept. Cases that fail to
//! tokenize/parse are rejected (not failed); the typed-error surface
//! is exercised elsewhere.

use proptest::prelude::{ProptestConfig, Strategy as _, TestCaseError, prop_oneof, proptest};
use proptest::test_runner::TestCaseResult;

use quanta_index_lq_norm::hasher::canonical_hash;
use quanta_index_lq_norm::normalizer::normalize;
use quanta_index_lq_norm::parser::parse;
use quanta_index_lq_norm::tokenizer::tokenize;

fn keyword_strategy() -> impl proptest::strategy::Strategy<Value = String> {
    // The generated regex always parses; the safe-strategy fallback is
    // a single hard-coded keyword so the property test does not abort
    // its setup phase. proptest::string::string_regex returns Err only
    // on invalid regex source, which is statically known to succeed
    // here — but match for total coverage.
    match proptest::string::string_regex("[a-z][a-zA-Z0-9_]{0,8}") {
        Ok(s) => s,
        Err(_e) => match proptest::string::string_regex("a") {
            Ok(s) => s,
            Err(_e2) => std::process::abort(),
        },
    }
}

fn atom_strategy() -> impl proptest::strategy::Strategy<Value = String> {
    prop_oneof![
        keyword_strategy(),
        keyword_strategy().prop_map(|k| format!("\"{k}\"")),
        keyword_strategy().prop_map(|k| format!("-{k}")),
        keyword_strategy().prop_map(|k| format!("repo:{k}")),
        keyword_strategy().prop_map(|k| format!("lang:{k}")),
    ]
}

fn query_strategy() -> impl proptest::strategy::Strategy<Value = String> {
    proptest::collection::vec(atom_strategy(), 1..=8).prop_map(|atoms| atoms.join(" "))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn normalize_is_idempotent(input in query_strategy()) {
        normalize_is_idempotent_inner(&input)?;
    }

    #[test]
    fn hash_is_deterministic(input in query_strategy()) {
        hash_is_deterministic_inner(&input)?;
    }
}

fn normalize_is_idempotent_inner(input: &str) -> TestCaseResult {
    let toks = match tokenize(input) {
        Ok(t) => t,
        Err(e) => return Err(TestCaseError::reject(format!("tokenize: {e}"))),
    };
    let parsed = match parse(&toks, input) {
        Ok(q) => q,
        Err(e) => return Err(TestCaseError::reject(format!("parse: {e}"))),
    };
    let q1 = match normalize(parsed) {
        Ok(q) => q,
        Err(e) => return Err(TestCaseError::reject(format!("first normalize: {e}"))),
    };
    let q2 = match normalize(q1.clone()) {
        Ok(q) => q,
        Err(e) => return Err(TestCaseError::fail(format!("second normalize: {e}"))),
    };
    proptest::prop_assert_eq!(q1, q2);
    Ok(())
}

fn hash_is_deterministic_inner(input: &str) -> TestCaseResult {
    let toks = match tokenize(input) {
        Ok(t) => t,
        Err(e) => return Err(TestCaseError::reject(format!("tokenize: {e}"))),
    };
    let parsed = match parse(&toks, input) {
        Ok(q) => q,
        Err(e) => return Err(TestCaseError::reject(format!("parse: {e}"))),
    };
    let q = match normalize(parsed) {
        Ok(q) => q,
        Err(e) => return Err(TestCaseError::reject(format!("normalize: {e}"))),
    };
    let h1 = match canonical_hash(&q) {
        Ok(h) => h,
        Err(e) => return Err(TestCaseError::fail(format!("first hash: {e}"))),
    };
    let h2 = match canonical_hash(&q) {
        Ok(h) => h,
        Err(e) => return Err(TestCaseError::fail(format!("second hash: {e}"))),
    };
    proptest::prop_assert_eq!(h1, h2);
    Ok(())
}
