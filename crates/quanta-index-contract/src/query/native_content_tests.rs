use crate::{LqCase, LqCountBound, LqExpr, LqLeaf, LqPatternType};
use quanta_index_lq_norm::LqParseErrorCode;

use super::{
    MAX_NATIVE_CONTENT_INPUT_BYTES, native_content_query_backing_bytes, parse_native_content_query,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn canonical_content_atoms_and_boolean_negation() -> TestResult {
    let keyword = parse_native_content_query("alpha")?;
    assert_eq!(keyword.expr, LqExpr::Leaf(LqLeaf::Keyword("alpha".into())));

    let phrase = parse_native_content_query("\"beta gamma\"")?;
    assert_eq!(
        phrase.expr,
        LqExpr::Leaf(LqLeaf::Phrase("beta gamma".into()))
    );

    let negated = parse_native_content_query("NOT alpha")?;
    assert_eq!(
        negated.expr,
        LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Keyword("alpha".into()))))
    );

    let disjunction = parse_native_content_query("alpha OR NOT absent")?;
    let LqExpr::Any(children) = disjunction.expr else {
        return Err("OR with a negative child must remain an Any expression".into());
    };
    assert_eq!(children.len(), 2);
    assert!(children.contains(&LqExpr::Leaf(LqLeaf::Keyword("alpha".into()))));
    let excluded = LqExpr::Not(Box::new(LqExpr::Leaf(LqLeaf::Keyword("absent".into()))));
    assert!(children.contains(&excluded));
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn case_and_pattern_options_preserve_canonical_values() -> TestResult {
    let sensitive = parse_native_content_query("alpha case:yes patterntype:keyword")?;
    assert_eq!(sensitive.options.case, Some(LqCase::Sensitive));
    assert_eq!(sensitive.options.pattern_type, LqPatternType::Keyword);

    let insensitive = parse_native_content_query("alpha case:no patterntype:standard")?;
    assert_eq!(insensitive.options.case, Some(LqCase::Insensitive));
    assert_eq!(insensitive.options.pattern_type, LqPatternType::Standard);
    Ok(())
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn count_options_keep_parser_last_wins_and_leave_policy_to_reader() -> TestResult {
    let one_then_two = parse_native_content_query("alpha count:1 count:2")?;
    assert_eq!(one_then_two.options.count, Some(LqCountBound::Bounded(2)));

    let two_then_one = parse_native_content_query("alpha count:2 count:1")?;
    assert_eq!(two_then_one.options.count, Some(LqCountBound::Bounded(1)));

    let all = parse_native_content_query("alpha count:all")?;
    assert_eq!(all.options.count, Some(LqCountBound::All));

    let zero = parse_native_content_query("alpha count:0")?;
    assert_eq!(zero.options.count, Some(LqCountBound::Bounded(0)));
    Ok(())
}

#[test]
fn unsupported_engine_tokens_refuse_before_engine_specific_parse() {
    for source in ["'raw'", "match { foo }", "symbol.has.name(\"foo\")"] {
        assert!(matches!(
            parse_native_content_query(source),
            Err(error) if error.code == LqParseErrorCode::UnsupportedCombo
        ));
    }
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn quoted_regex_looking_text_remains_a_phrase() -> TestResult {
    let query = parse_native_content_query(r#""/(?<=alpha)/""#)?;
    assert_eq!(
        query.expr,
        LqExpr::Leaf(LqLeaf::Phrase("/(?<=alpha)/".into()))
    );
    Ok(())
}

#[test]
fn oversize_and_overflow_refuse_before_lowering_backing() {
    let oversized = "/".repeat(MAX_NATIVE_CONTENT_INPUT_BYTES + 1);
    assert_eq!(native_content_query_backing_bytes(oversized.len()), None);
    assert!(matches!(
        parse_native_content_query(&oversized),
        Err(error) if error.code == LqParseErrorCode::LimitExceededBytes
    ));
    assert_eq!(native_content_query_backing_bytes(usize::MAX), None);
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "fixed contract gold and typed refusal assertions are independent test oracles"
)]
fn regex_uses_canonical_guard_after_preparation_admission() -> TestResult {
    let tokens = super::tokenize_native_content_query("/alpha/")?;
    assert!(tokens.preparation_backing_bytes().ok_or("overflow")? > 0);
    assert!(matches!(
        tokens.parse()?.expr,
        LqExpr::Leaf(LqLeaf::Regex(_))
    ));
    assert!(
        matches!(parse_native_content_query(r"/(?<=foo)/"), Err(error) if error.code == LqParseErrorCode::ForbiddenSyntax)
    );
    Ok(())
}
